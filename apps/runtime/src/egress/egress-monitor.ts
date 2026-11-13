import { randomUUID } from 'node:crypto'
import type {
  EgressFamily,
  EgressFamilyWire,
  EgressHostSample,
  EgressStatusWire
} from '@matou/contracts'
import {
  EGRESS_HOSTS,
  evaluateEgress,
  type EgressEvaluation,
  type EgressSample,
  type FamilyEvaluation
} from './egress-evaluator'
import type { EgressSettings } from './egress-settings'

export const EGRESS_LIGHT_SAMPLE_INTERVAL_MS = 60_000
export const EGRESS_NETWORK_POLL_INTERVAL_MS = 5_000
export const EGRESS_WAKE_DRIFT_THRESHOLD_MS = 15_000
export const EGRESS_FROZEN_RECHECK_INTERVAL_MS = 15_000
export const EGRESS_RECOVERY_CONFIRM_DELAY_MS = 5_000

const FAMILIES: readonly EgressFamily[] = ['anthropic', 'openai']

type RecoveryPhase = 'idle' | 'rechecking' | 'confirming'

export interface EgressMonitorDependencies {
  now(): number
  setTimer(callback: () => void, ms: number): () => void
  sampleFull(): Promise<EgressSample>
  sampleLight(): Promise<EgressHostSample>
  networkSignature(): string
  settings: EgressSettings
  freeze(sessionId: string): Promise<void>
  thaw(sessionId: string): Promise<void>
  publish(status: EgressStatusWire): void
}

function matchesOfficialHost(rawUrl: string | undefined, officialHost: string): boolean {
  const trimmed = rawUrl?.trim()
  if (!trimmed) return true
  try {
    return new URL(trimmed).hostname.toLowerCase() === officialHost
  } catch {
    return false
  }
}

/** Maps a session launch profile and provider environment to its monitored official egress family. */
export function egressFamilyForLaunch(
  profile: string,
  providerEnv: Record<string, string | undefined>
): EgressFamily | undefined {
  if (profile === 'claude-code') {
    return matchesOfficialHost(providerEnv.ANTHROPIC_BASE_URL, EGRESS_HOSTS.anthropic.api)
      ? 'anthropic'
      : undefined
  }
  if (profile === 'codex') {
    return matchesOfficialHost(providerEnv.OPENAI_BASE_URL, EGRESS_HOSTS.openai.api)
      ? 'openai'
      : undefined
  }
  return undefined
}

/** Monitors provider network exits and coordinates per-provider session freezing and auto-resume. */
export class EgressMonitor {
  readonly #deps: EgressMonitorDependencies
  readonly #sessionFamilies = new Map<string, EgressFamily>()
  readonly #frozenSessions = new Set<string>()
  readonly #overrides = new Map<EgressFamily, string>()
  readonly #lastResolvedIp = new Map<EgressFamily, string>()
  readonly #recoveryPhase = new Map<EgressFamily, RecoveryPhase>([
    ['anthropic', 'idle'],
    ['openai', 'idle']
  ])

  #started = false
  #stopped = false
  #lastSample: EgressSample | undefined
  #lastEvaluation: EgressEvaluation | undefined
  #checkedAt: number | undefined
  #lastTransition: EgressStatusWire['lastTransition']
  #lastNetworkSignature = ''
  #lastPollAt = 0
  #inFlightCheck: Promise<EgressStatusWire> | null = null
  #lightTimer: (() => void) | null = null
  #networkTimer: (() => void) | null = null
  #recoveryTimer: (() => void) | null = null
  #lastPublishedJson: string | undefined

  constructor(deps: EgressMonitorDependencies) {
    this.#deps = deps
  }

  start(): void {
    if (this.#started) return
    this.#stopped = false
    this.#started = true
    this.#lastNetworkSignature = this.#deps.networkSignature()
    this.#lastPollAt = this.#deps.now()
    this.#scheduleLightSample()
    this.#scheduleNetworkPoll()
    void this.#runFullCheck()
  }

  stop(): void {
    this.#stopped = true
    this.#started = false
    this.#cancelTimers()
  }

  registerSession(sessionId: string, family: EgressFamily): { bornFrozen: boolean } {
    this.#sessionFamilies.set(sessionId, family)
    const bornFrozen =
      this.#deps.settings.protectionEnabled() &&
      this.#isFamilyAbnormal(family) &&
      !this.#overrides.has(family)

    if (bornFrozen) {
      this.#frozenSessions.add(sessionId)
      if (this.#recoveryPhase.get(family) === 'idle') {
        this.#recoveryPhase.set(family, 'rechecking')
      }
      if (!this.#stopped) {
        this.#syncRecoveryTimer()
      }
    }

    this.#publishIfChanged()
    return { bornFrozen }
  }

  unregisterSession(sessionId: string): void {
    const removedSession = this.#sessionFamilies.delete(sessionId)
    const removedFrozen = this.#frozenSessions.delete(sessionId)
    if (!removedSession && !removedFrozen) return

    if (removedFrozen) {
      for (const family of FAMILIES) {
        if (this.#frozenSessionsForFamily(family).length === 0) {
          this.#recoveryPhase.set(family, 'idle')
        }
      }
      if (!this.#stopped) {
        this.#syncRecoveryTimer()
      }
    }

    this.#publishIfChanged()
  }

  exitCountry(family: EgressFamily): string | undefined {
    return this.#lastEvaluation?.families.find((f) => f.family === family)?.exit?.country
  }

  status(): EgressStatusWire {
    const rule = this.#deps.settings.rule()
    const protectionEnabled = this.#deps.settings.protectionEnabled()
    const sessionFamilies = Object.fromEntries(this.#sessionFamilies)

    if (!this.#lastEvaluation || !this.#lastSample) {
      const families: EgressFamilyWire[] = FAMILIES.map((family) => {
        const baseline = rule.baselines[family]
        const hosts = EGRESS_HOSTS[family]
        return {
          family,
          state: 'unknown',
          hosts: [{ host: hosts.api }, { host: hosts.login }],
          ...(baseline ? { baseline } : {}),
          overridden: false,
          frozenSessionIds: this.#frozenSessionsForFamily(family),
          recovering: false
        }
      })
      return {
        state: 'checking',
        families,
        checks: [],
        ruleMode: rule.mode,
        protectionEnabled,
        sessionFamilies
      }
    }

    const families: EgressFamilyWire[] = this.#lastEvaluation.families.map((evaluation) => {
      const family = evaluation.family
      const samplePair = this.#lastSample!.hosts[family]
      const baseline = rule.baselines[family]
      const frozenSessionIds = this.#frozenSessionsForFamily(family)
      const phase = this.#recoveryPhase.get(family) ?? 'idle'
      return {
        family,
        state: evaluation.state,
        ...(evaluation.exit ? { exit: evaluation.exit } : {}),
        hosts: [samplePair.api, samplePair.login],
        ...(evaluation.violation ? { violation: evaluation.violation } : {}),
        ...(baseline ? { baseline } : {}),
        overridden: this.#overrides.has(family),
        frozenSessionIds,
        recovering: frozenSessionIds.length > 0 && phase !== 'idle'
      }
    })

    return {
      state: this.#lastEvaluation.state,
      families,
      checks: this.#lastEvaluation.checks.map((check) => ({ ...check })),
      ruleMode: rule.mode,
      protectionEnabled,
      sessionFamilies,
      ...(this.#checkedAt === undefined ? {} : { checkedAt: this.#checkedAt }),
      ...(this.#lastTransition === undefined ? {} : { lastTransition: this.#lastTransition })
    }
  }

  recheck(): Promise<EgressStatusWire> {
    if (this.#stopped) return Promise.resolve(this.status())
    return this.#runFullCheck()
  }

  async setRuleMode(mode: 'country' | 'ip'): Promise<EgressStatusWire> {
    this.#deps.settings.setMode(mode)
    if (this.#lastSample) {
      await this.#applySample(this.#lastSample, { updateCheckedAt: false, immediateThaw: true })
      if (!this.#stopped) {
        this.#syncRecoveryTimer()
      }
    }
    this.#publishIfChanged()
    return this.status()
  }

  async acceptCurrent(family: EgressFamily): Promise<EgressStatusWire> {
    const api = this.#lastSample?.hosts[family].api
    if (!api?.ip || !api.country || api.country === 'CN') {
      throw new Error(`Current API exit for ${family} does not pass the mainland check`)
    }
    this.#deps.settings.setBaseline(family, { ip: api.ip, country: api.country })
    if (this.#lastSample) {
      await this.#applySample(this.#lastSample, { updateCheckedAt: false, immediateThaw: true })
      if (!this.#stopped) {
        this.#syncRecoveryTimer()
      }
    }
    this.#publishIfChanged()
    return this.status()
  }

  async setProtection(enabled: boolean): Promise<EgressStatusWire> {
    this.#deps.settings.setProtection(enabled)
    if (!enabled) {
      for (const family of FAMILIES) {
        await this.#thawSessions(this.#frozenSessionsForFamily(family))
        this.#recoveryPhase.set(family, 'idle')
      }
    } else if (this.#lastSample) {
      await this.#applySample(this.#lastSample, { updateCheckedAt: false, immediateThaw: false })
    }
    if (!this.#stopped) {
      this.#syncRecoveryTimer()
    }
    this.#publishIfChanged()
    return this.status()
  }

  async override(family: EgressFamily): Promise<EgressStatusWire> {
    const exitIp =
      this.#lastEvaluation?.families.find((f) => f.family === family)?.exit?.ip ??
      this.#lastResolvedIp.get(family)
    if (!exitIp) {
      throw new Error(`Cannot override ${family} without a resolved exit IP`)
    }
    this.#overrides.set(family, exitIp)
    await this.#thawSessions(this.#frozenSessionsForFamily(family))
    this.#recoveryPhase.set(family, 'idle')
    if (!this.#stopped) {
      this.#syncRecoveryTimer()
    }
    this.#publishIfChanged()
    return this.status()
  }

  #runFullCheck(): Promise<EgressStatusWire> {
    if (this.#stopped) return Promise.resolve(this.status())
    if (this.#inFlightCheck) return this.#inFlightCheck

    if (this.#recoveryTimer) {
      this.#recoveryTimer()
      this.#recoveryTimer = null
    }

    this.#inFlightCheck = (async () => {
      try {
        const sample = await this.#deps.sampleFull()
        if (this.#stopped) return this.status()
        await this.#applySample(sample, { updateCheckedAt: true, immediateThaw: false })
        this.#publishIfChanged()
        return this.status()
      } finally {
        this.#inFlightCheck = null
        if (!this.#stopped) {
          this.#syncRecoveryTimer()
        }
      }
    })()

    return this.#inFlightCheck
  }

  async #applySample(
    sample: EgressSample,
    options: { updateCheckedAt: boolean; immediateThaw: boolean }
  ): Promise<void> {
    const now = this.#deps.now()
    this.#lastSample = sample
    if (options.updateCheckedAt) {
      this.#checkedAt = now
    }

    let rule = this.#deps.settings.rule()
    let evaluation = evaluateEgress(sample, rule)

    let wroteBaseline = false
    for (const family of FAMILIES) {
      if (rule.baselines[family]) continue
      const mainlandCheck = evaluation.checks.find(
        (check) => check.id === 'mainland' && check.family === family
      )
      const api = sample.hosts[family].api
      if (mainlandCheck?.result === 'pass' && api.ip && api.country) {
        this.#deps.settings.setBaseline(family, { ip: api.ip, country: api.country })
        wroteBaseline = true
      }
    }
    if (wroteBaseline) {
      rule = this.#deps.settings.rule()
      evaluation = evaluateEgress(sample, rule)
    }

    this.#lastEvaluation = evaluation

    for (const familyEval of evaluation.families) {
      this.#updateOverrideState(familyEval)
      await this.#reconcileFamily(familyEval, now, options.immediateThaw)
    }
  }

  #updateOverrideState(familyEval: FamilyEvaluation): void {
    const family = familyEval.family
    if (familyEval.exit) {
      this.#lastResolvedIp.set(family, familyEval.exit.ip)
      const overriddenIp = this.#overrides.get(family)
      if (overriddenIp !== undefined && overriddenIp !== familyEval.exit.ip) {
        this.#overrides.delete(family)
      }
    }
    if (familyEval.state === 'ok' || familyEval.state === 'warning') {
      this.#overrides.delete(family)
    }
  }

  async #reconcileFamily(
    familyEval: FamilyEvaluation,
    now: number,
    immediateThaw: boolean
  ): Promise<void> {
    const family = familyEval.family
    const protectionEnabled = this.#deps.settings.protectionEnabled()

    if (familyEval.state === 'abnormal') {
      if (!protectionEnabled || this.#overrides.has(family)) {
        this.#recoveryPhase.set(family, 'idle')
        return
      }
      const toFreeze = this.#registeredSessionsForFamily(family).filter(
        (sessionId) => !this.#frozenSessions.has(sessionId)
      )
      const frozenCount = await this.#freezeSessions(toFreeze)
      if (frozenCount > 0) {
        this.#lastTransition = {
          id: randomUUID(),
          kind: 'frozen',
          family,
          sessionCount: frozenCount,
          at: now,
          ...(familyEval.violation ? { reason: familyEval.violation } : {})
        }
      }
      this.#recoveryPhase.set(
        family,
        this.#frozenSessionsForFamily(family).length > 0 ? 'rechecking' : 'idle'
      )
      return
    }

    const frozen = this.#frozenSessionsForFamily(family)
    if (frozen.length === 0) {
      this.#recoveryPhase.set(family, 'idle')
      return
    }

    if (familyEval.state === 'unknown') {
      this.#recoveryPhase.set(family, 'rechecking')
      return
    }

    const phase = this.#recoveryPhase.get(family)
    if (immediateThaw || phase === 'confirming') {
      const thawedCount = await this.#thawSessions(frozen)
      this.#recoveryPhase.set(family, 'idle')
      if (thawedCount > 0) {
        this.#lastTransition = {
          id: randomUUID(),
          kind: 'resumed',
          family,
          sessionCount: thawedCount,
          at: now
        }
      }
      return
    }

    this.#recoveryPhase.set(family, 'confirming')
  }

  async #freezeSessions(sessionIds: string[]): Promise<number> {
    let frozenCount = 0
    await Promise.allSettled(
      sessionIds.map(async (sessionId) => {
        try {
          await this.#deps.freeze(sessionId)
          if (!this.#stopped && this.#sessionFamilies.has(sessionId)) {
            this.#frozenSessions.add(sessionId)
            frozenCount += 1
          }
        } catch {
          this.#frozenSessions.delete(sessionId)
        }
      })
    )
    return frozenCount
  }

  async #thawSessions(sessionIds: string[]): Promise<number> {
    let thawedCount = 0
    await Promise.allSettled(
      sessionIds.map(async (sessionId) => {
        try {
          await this.#deps.thaw(sessionId)
          thawedCount += 1
        } catch {
          // Swallow errors for sessions that no longer exist while still clearing frozen state.
        } finally {
          this.#frozenSessions.delete(sessionId)
        }
      })
    )
    return thawedCount
  }

  #isFamilyAbnormal(family: EgressFamily): boolean {
    return this.#lastEvaluation?.families.find((f) => f.family === family)?.state === 'abnormal'
  }

  #registeredSessionsForFamily(family: EgressFamily): string[] {
    const ids: string[] = []
    for (const [sessionId, sessionFamily] of this.#sessionFamilies) {
      if (sessionFamily === family) ids.push(sessionId)
    }
    return ids
  }

  #frozenSessionsForFamily(family: EgressFamily): string[] {
    const ids: string[] = []
    for (const sessionId of this.#frozenSessions) {
      if (this.#sessionFamilies.get(sessionId) === family) ids.push(sessionId)
    }
    return ids
  }

  #scheduleLightSample(): void {
    if (this.#lightTimer) {
      this.#lightTimer()
      this.#lightTimer = null
    }
    if (this.#stopped) return
    this.#lightTimer = this.#deps.setTimer(() => {
      this.#lightTimer = null
      void this.#onLightSampleTick()
    }, EGRESS_LIGHT_SAMPLE_INTERVAL_MS)
  }

  async #onLightSampleTick(): Promise<void> {
    if (this.#stopped) return
    let nextApi: EgressHostSample
    try {
      nextApi = await this.#deps.sampleLight()
    } catch {
      if (!this.#stopped) this.#scheduleLightSample()
      return
    }
    if (this.#stopped) return
    this.#scheduleLightSample()

    const prevApi = this.#lastSample?.hosts.anthropic.api
    const shouldTriggerFull =
      (nextApi.ip !== undefined && nextApi.ip !== prevApi?.ip) || nextApi.country === 'CN'
    if (shouldTriggerFull) {
      await this.#runFullCheck()
    }
  }

  #scheduleNetworkPoll(): void {
    if (this.#networkTimer) {
      this.#networkTimer()
      this.#networkTimer = null
    }
    if (this.#stopped) return
    this.#networkTimer = this.#deps.setTimer(() => {
      this.#networkTimer = null
      this.#onNetworkPollTick()
    }, EGRESS_NETWORK_POLL_INTERVAL_MS)
  }

  #onNetworkPollTick(): void {
    if (this.#stopped) return
    const currentNow = this.#deps.now()
    const elapsed = currentNow - this.#lastPollAt
    const nextSignature = this.#deps.networkSignature()
    const signatureChanged = nextSignature !== this.#lastNetworkSignature
    const wakeDetected = elapsed > EGRESS_WAKE_DRIFT_THRESHOLD_MS

    this.#lastPollAt = currentNow
    this.#lastNetworkSignature = nextSignature
    this.#scheduleNetworkPoll()

    if (signatureChanged || wakeDetected) {
      void this.#runFullCheck()
    }
  }

  #syncRecoveryTimer(): void {
    if (this.#recoveryTimer) {
      this.#recoveryTimer()
      this.#recoveryTimer = null
    }
    if (this.#stopped) return

    let needsConfirm = false
    let needsRecheck = false
    for (const family of FAMILIES) {
      if (this.#frozenSessionsForFamily(family).length === 0) continue
      const phase = this.#recoveryPhase.get(family)
      if (phase === 'confirming') needsConfirm = true
      else if (phase === 'rechecking') needsRecheck = true
    }

    if (!needsConfirm && !needsRecheck) return

    const delay = needsConfirm
      ? EGRESS_RECOVERY_CONFIRM_DELAY_MS
      : EGRESS_FROZEN_RECHECK_INTERVAL_MS
    this.#recoveryTimer = this.#deps.setTimer(() => {
      this.#recoveryTimer = null
      void this.#runFullCheck()
    }, delay)
  }

  #cancelTimers(): void {
    if (this.#lightTimer) {
      this.#lightTimer()
      this.#lightTimer = null
    }
    if (this.#networkTimer) {
      this.#networkTimer()
      this.#networkTimer = null
    }
    if (this.#recoveryTimer) {
      this.#recoveryTimer()
      this.#recoveryTimer = null
    }
  }

  #publishIfChanged(): void {
    if (this.#stopped) return
    const nextStatus = this.status()
    const serialized = JSON.stringify(nextStatus)
    if (serialized === this.#lastPublishedJson) return
    this.#lastPublishedJson = serialized
    this.#deps.publish(nextStatus)
  }
}
