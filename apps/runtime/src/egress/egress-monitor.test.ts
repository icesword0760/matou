import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import type { EgressFamily, EgressHostSample, EgressStatusWire } from '@matou/contracts'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { RuntimeDatabase } from '../storage/database'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'
import { EGRESS_HOSTS, type EgressSample } from './egress-evaluator'
import {
  EGRESS_FROZEN_RECHECK_INTERVAL_MS,
  EGRESS_LIGHT_SAMPLE_INTERVAL_MS,
  EGRESS_NETWORK_POLL_INTERVAL_MS,
  EGRESS_RECOVERY_CONFIRM_DELAY_MS,
  EGRESS_WAKE_DRIFT_THRESHOLD_MS,
  EgressMonitor,
  egressFamilyForLaunch
} from './egress-monitor'
import { EgressSettings } from './egress-settings'

class FakeClock {
  nowMs = 1_000_000
  #nextId = 1
  #timers = new Map<number, { fireAt: number; callback: () => void }>()

  now = (): number => this.nowMs

  setTimer = (callback: () => void, ms: number): (() => void) => {
    const id = this.#nextId++
    this.#timers.set(id, { fireAt: this.nowMs + ms, callback })
    return () => {
      this.#timers.delete(id)
    }
  }

  activeCount(): number {
    return this.#timers.size
  }

  async flushMicrotasks(): Promise<void> {
    for (let i = 0; i < 12; i++) {
      await Promise.resolve()
    }
  }

  async advanceBy(ms: number): Promise<void> {
    const target = this.nowMs + ms
    while (true) {
      let nextEntry: [number, { fireAt: number; callback: () => void }] | undefined
      for (const entry of this.#timers.entries()) {
        if (entry[1].fireAt <= target && (!nextEntry || entry[1].fireAt < nextEntry[1].fireAt)) {
          nextEntry = entry
        }
      }
      if (!nextEntry) break
      const [id, timer] = nextEntry
      this.#timers.delete(id)
      this.nowMs = timer.fireAt
      timer.callback()
      await this.flushMicrotasks()
    }
    this.nowMs = target
    await this.flushMicrotasks()
  }

  async jumpClock(ms: number): Promise<void> {
    this.nowMs += ms
    const due = [...this.#timers.entries()]
      .filter(([, timer]) => timer.fireAt <= this.nowMs)
      .sort((a, b) => a[1].fireAt - b[1].fireAt)
    for (const [id, timer] of due) {
      if (!this.#timers.has(id)) continue
      this.#timers.delete(id)
      timer.callback()
    }
    await this.flushMicrotasks()
  }
}

function makeSample(options?: {
  anthropicApi?: Partial<EgressHostSample>
  anthropicLogin?: Partial<EgressHostSample>
  openaiApi?: Partial<EgressHostSample>
  openaiLogin?: Partial<EgressHostSample>
  chatgptSubscriptionApi?: Partial<EgressHostSample>
  chatgptSubscriptionLogin?: Partial<EgressHostSample>
}): EgressSample {
  return {
    hosts: {
      anthropic: {
        api: {
          host: EGRESS_HOSTS.anthropic.api,
          ip: '169.128.1.1',
          country: 'US',
          ...options?.anthropicApi
        },
        login: {
          host: EGRESS_HOSTS.anthropic.login,
          ip: '169.128.1.1',
          country: 'US',
          ...options?.anthropicLogin
        }
      },
      openai: {
        api: {
          host: EGRESS_HOSTS.openai.api,
          ip: '198.51.100.1',
          country: 'US',
          ...options?.openaiApi
        },
        login: {
          host: EGRESS_HOSTS.openai.login,
          ip: '198.51.100.1',
          country: 'US',
          ...options?.openaiLogin
        }
      },
      ...(options?.chatgptSubscriptionApi || options?.chatgptSubscriptionLogin
        ? {
            'chatgpt-subscription': {
              api: {
                host: EGRESS_HOSTS['chatgpt-subscription'].api,
                ip: '203.0.113.50',
                country: 'US',
                ...options?.chatgptSubscriptionApi
              },
              login: {
                host: EGRESS_HOSTS['chatgpt-subscription'].login,
                ip: '203.0.113.50',
                country: 'US',
                ...options?.chatgptSubscriptionLogin
              }
            }
          }
        : {})
    },
    agentHasProxyEnv: true,
    systemProxyEnabled: false,
    tunDetected: false,
    ipv6Bypass: false,
    agentTimeZoneMainland: false
  }
}

function makeTimeoutSample(): EgressSample {
  return {
    hosts: {
      anthropic: {
        api: { host: EGRESS_HOSTS.anthropic.api, error: 'timeout' },
        login: { host: EGRESS_HOSTS.anthropic.login, error: 'timeout' }
      },
      openai: {
        api: { host: EGRESS_HOSTS.openai.api, error: 'timeout' },
        login: { host: EGRESS_HOSTS.openai.login, error: 'timeout' }
      }
    },
    agentHasProxyEnv: true,
    systemProxyEnabled: false,
    tunDetected: false,
    ipv6Bypass: false,
    agentTimeZoneMainland: false
  }
}

let root: string
let database: RuntimeDatabase
let settings: EgressSettings

beforeEach(async () => {
  root = await mkdtemp(join(tmpdir(), 'matou-egress-monitor-'))
  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  settings = new EgressSettings(database)
})

afterEach(async () => {
  database.close()
  await rm(root, { recursive: true, force: true })
})

function createHarness(overrides?: {
  sampleFull?: () => Promise<EgressSample>
  sampleLight?: (family: EgressFamily) => Promise<EgressHostSample>
  networkSignature?: () => string
  freeze?: (sessionId: string) => Promise<void>
  thaw?: (sessionId: string) => Promise<void>
}) {
  const clock = new FakeClock()
  let currentFull = makeSample()
  const currentLight: Record<EgressFamily, EgressHostSample> = {
    anthropic: {
      host: EGRESS_HOSTS.anthropic.api,
      ip: '169.128.1.1',
      country: 'US'
    },
    openai: {
      host: EGRESS_HOSTS.openai.api,
      ip: '198.51.100.1',
      country: 'US'
    },
    'chatgpt-subscription': {
      host: EGRESS_HOSTS['chatgpt-subscription'].api,
      ip: '203.0.113.50',
      country: 'US'
    }
  }
  let signature = 'en0:192.168.1.10'
  const frozenCalls: string[] = []
  const thawedCalls: string[] = []
  const published: EgressStatusWire[] = []

  const sampleFullSpy = vi.fn(overrides?.sampleFull ?? (async () => currentFull))
  const sampleLightSpy = vi.fn(
    overrides?.sampleLight ?? (async (family: EgressFamily) => currentLight[family])
  )
  const networkSignatureSpy = vi.fn(overrides?.networkSignature ?? (() => signature))
  const freezeSpy = vi.fn(
    overrides?.freeze ??
      (async (sessionId: string) => {
        frozenCalls.push(sessionId)
      })
  )
  const thawSpy = vi.fn(
    overrides?.thaw ??
      (async (sessionId: string) => {
        thawedCalls.push(sessionId)
      })
  )
  const publishSpy = vi.fn((status: EgressStatusWire) => {
    published.push(status)
  })

  const monitor = new EgressMonitor({
    now: clock.now,
    setTimer: clock.setTimer,
    sampleFull: sampleFullSpy,
    sampleLight: sampleLightSpy,
    networkSignature: networkSignatureSpy,
    settings,
    freeze: freezeSpy,
    thaw: thawSpy,
    publish: publishSpy
  })

  return {
    clock,
    monitor,
    sampleFullSpy,
    sampleLightSpy,
    networkSignatureSpy,
    freezeSpy,
    thawSpy,
    publishSpy,
    frozenCalls,
    thawedCalls,
    published,
    setFullSample(sample: EgressSample) {
      currentFull = sample
    },
    setLightSample(sample: EgressHostSample, family: EgressFamily = 'anthropic') {
      currentLight[family] = sample
    },
    setSignature(next: string) {
      signature = next
    }
  }
}

describe('EgressMonitor', () => {
  it('exports the agreed timing constants', () => {
    expect(EGRESS_LIGHT_SAMPLE_INTERVAL_MS).toBe(60_000)
    expect(EGRESS_NETWORK_POLL_INTERVAL_MS).toBe(5_000)
    expect(EGRESS_WAKE_DRIFT_THRESHOLD_MS).toBe(15_000)
    expect(EGRESS_FROZEN_RECHECK_INTERVAL_MS).toBe(15_000)
    expect(EGRESS_RECOVERY_CONFIRM_DELAY_MS).toBe(5_000)
  })

  it('reports checking before the first full check finishes and populates status afterwards', async () => {
    let resolveSample!: (sample: EgressSample) => void
    const harness = createHarness({
      sampleFull: () =>
        new Promise<EgressSample>((resolve) => {
          resolveSample = resolve
        })
    })

    expect(harness.monitor.status().state).toBe('checking')
    expect(harness.monitor.exitCountry('anthropic')).toBeUndefined()

    harness.monitor.start()
    expect(harness.monitor.status().state).toBe('checking')

    resolveSample(makeSample({ anthropicApi: { country: 'TW' }, anthropicLogin: { country: 'TW' } }))
    await harness.clock.flushMicrotasks()

    const status = harness.monitor.status()
    expect(status.state).toBe('ok')
    expect(harness.monitor.exitCountry('anthropic')).toBe('TW')
    expect(status.families.find((f) => f.family === 'anthropic')?.hosts.map((h) => h.host)).toEqual([
      'api.anthropic.com',
      'claude.ai'
    ])
    expect(harness.publishSpy).toHaveBeenCalledTimes(1)

    harness.monitor.stop()
  })

  it('freezes only anthropic sessions when anthropic exit is CN while leaving codex sessions untouched', async () => {
    const harness = createHarness()
    harness.setFullSample(
      makeSample({
        anthropicApi: { ip: '220.181.1.1', country: 'CN' },
        anthropicLogin: { ip: '220.181.1.1', country: 'CN' }
      })
    )

    harness.monitor.registerSession('claude-1', 'anthropic')
    harness.monitor.registerSession('claude-2', 'anthropic')
    harness.monitor.registerSession('codex-1', 'openai')

    harness.monitor.start()
    await harness.clock.flushMicrotasks()

    expect(harness.frozenCalls).toEqual(['claude-1', 'claude-2'])
    expect(harness.thawedCalls).toEqual([])

    const status = harness.monitor.status()
    expect(status.state).toBe('abnormal')
    expect(status.lastTransition).toMatchObject({
      kind: 'frozen',
      family: 'anthropic',
      sessionCount: 2,
      reason: 'mainland'
    })
    expect(status.lastTransition?.id).toBeTruthy()
    expect(status.families.find((f) => f.family === 'anthropic')).toMatchObject({
      state: 'abnormal',
      frozenSessionIds: ['claude-1', 'claude-2'],
      recovering: true
    })
    expect(status.families.find((f) => f.family === 'openai')).toMatchObject({
      state: 'ok',
      frozenSessionIds: [],
      recovering: false
    })

    harness.monitor.stop()
  })

  it('rechecks at 15s and thaws only after a 5s secondary confirmation pass', async () => {
    const harness = createHarness()
    harness.setFullSample(
      makeSample({
        anthropicApi: { ip: '220.181.1.1', country: 'CN' },
        anthropicLogin: { ip: '220.181.1.1', country: 'CN' }
      })
    )
    harness.monitor.registerSession('claude-1', 'anthropic')
    harness.monitor.start()
    await harness.clock.flushMicrotasks()

    expect(harness.frozenCalls).toEqual(['claude-1'])

    harness.setFullSample(makeSample())
    await harness.clock.advanceBy(15_000)

    expect(harness.thawedCalls).toEqual([])
    expect(harness.monitor.status().families.find((f) => f.family === 'anthropic')).toMatchObject({
      frozenSessionIds: ['claude-1'],
      recovering: true
    })

    await harness.clock.advanceBy(4_999)
    expect(harness.thawedCalls).toEqual([])

    await harness.clock.advanceBy(1)
    expect(harness.thawedCalls).toEqual(['claude-1'])
    expect(harness.monitor.status().lastTransition).toMatchObject({
      kind: 'resumed',
      family: 'anthropic',
      sessionCount: 1
    })
    expect(harness.monitor.status().families.find((f) => f.family === 'anthropic')).toMatchObject({
      frozenSessionIds: [],
      recovering: false
    })

    harness.monitor.stop()
  })

  it('does not thaw during exit jitter until two consecutive checks pass', async () => {
    const harness = createHarness()
    const cnSample = makeSample({
      anthropicApi: { ip: '220.181.1.1', country: 'CN' },
      anthropicLogin: { ip: '220.181.1.1', country: 'CN' }
    })
    const usSample = makeSample()

    harness.setFullSample(cnSample)
    harness.monitor.registerSession('claude-1', 'anthropic')
    harness.monitor.start()
    await harness.clock.flushMicrotasks()
    expect(harness.frozenCalls).toEqual(['claude-1'])

    // 15s recheck passes -> enters 5s confirmation
    harness.setFullSample(usSample)
    await harness.clock.advanceBy(15_000)
    expect(harness.thawedCalls).toEqual([])

    // 5s confirmation fails back to CN -> stays frozen, resets to 15s recheck
    harness.setFullSample(cnSample)
    await harness.clock.advanceBy(5_000)
    expect(harness.thawedCalls).toEqual([])
    expect(harness.frozenCalls).toEqual(['claude-1'])
    expect(harness.monitor.status().families.find((f) => f.family === 'anthropic')).toMatchObject({
      frozenSessionIds: ['claude-1'],
      recovering: true
    })

    // 5s later must NOT thaw yet; next recheck is at 15s
    harness.setFullSample(usSample)
    await harness.clock.advanceBy(5_000)
    expect(harness.thawedCalls).toEqual([])

    // Complete remaining 10s to reach 15s recheck (pass #1)
    await harness.clock.advanceBy(10_000)
    expect(harness.thawedCalls).toEqual([])

    // Secondary confirmation 5s later (pass #2) -> thaws
    await harness.clock.advanceBy(5_000)
    expect(harness.thawedCalls).toEqual(['claude-1'])
    expect(harness.monitor.status().lastTransition?.kind).toBe('resumed')

    harness.monitor.stop()
  })

  it('keeps frozen sessions frozen and avoids freezing unfrozen sessions when all probes timeout', async () => {
    const harness = createHarness()
    harness.setFullSample(makeTimeoutSample())
    harness.monitor.registerSession('claude-1', 'anthropic')
    harness.monitor.start()
    await harness.clock.flushMicrotasks()

    expect(harness.frozenCalls).toEqual([])
    expect(harness.monitor.status().state).toBe('warning')
    expect(harness.monitor.status().families.map((f) => f.state)).toEqual(['unknown', 'unknown'])

    // Now confirm CN -> freezes claude-1
    harness.setFullSample(
      makeSample({
        anthropicApi: { ip: '220.181.1.1', country: 'CN' },
        anthropicLogin: { ip: '220.181.1.1', country: 'CN' }
      })
    )
    await harness.monitor.recheck()
    expect(harness.frozenCalls).toEqual(['claude-1'])

    // Network drops -> all timeout at 15s recheck: claude-1 stays frozen
    harness.setFullSample(makeTimeoutSample())
    await harness.clock.advanceBy(15_000)
    expect(harness.thawedCalls).toEqual([])
    expect(harness.monitor.status().state).toBe('warning')
    expect(harness.monitor.status().families.find((f) => f.family === 'anthropic')).toMatchObject({
      state: 'unknown',
      frozenSessionIds: ['claude-1'],
      recovering: true
    })

    harness.monitor.stop()
  })

  it('supports override for the same IP, skips bornFrozen, and invalidates override when IP changes', async () => {
    const harness = createHarness()
    harness.setFullSample(
      makeSample({
        anthropicApi: { ip: '220.181.1.1', country: 'CN' },
        anthropicLogin: { ip: '220.181.1.1', country: 'CN' }
      })
    )
    harness.monitor.registerSession('claude-1', 'anthropic')
    harness.monitor.start()
    await harness.clock.flushMicrotasks()
    expect(harness.frozenCalls).toEqual(['claude-1'])

    await harness.monitor.override('anthropic')
    expect(harness.thawedCalls).toEqual(['claude-1'])
    expect(harness.monitor.status().families.find((f) => f.family === 'anthropic')).toMatchObject({
      state: 'abnormal',
      overridden: true,
      frozenSessionIds: [],
      recovering: false
    })

    // Same abnormal exit IP -> new session is NOT bornFrozen
    expect(harness.monitor.registerSession('claude-2', 'anthropic')).toEqual({ bornFrozen: false })

    // Same IP on recheck -> override holds
    await harness.monitor.recheck()
    expect(harness.frozenCalls).toEqual(['claude-1'])
    expect(harness.monitor.status().families.find((f) => f.family === 'anthropic')?.overridden).toBe(true)

    // IP changes to a different CN IP -> override invalidated, all anthropic sessions freeze
    harness.setFullSample(
      makeSample({
        anthropicApi: { ip: '220.181.2.2', country: 'CN' },
        anthropicLogin: { ip: '220.181.2.2', country: 'CN' }
      })
    )
    await harness.monitor.recheck()
    expect(harness.frozenCalls).toEqual(['claude-1', 'claude-1', 'claude-2'])
    expect(harness.monitor.status().families.find((f) => f.family === 'anthropic')).toMatchObject({
      overridden: false,
      frozenSessionIds: ['claude-1', 'claude-2']
    })

    harness.monitor.stop()
  })

  it('marks newly registered sessions as bornFrozen while abnormal and recovers them automatically', async () => {
    const harness = createHarness()
    harness.setFullSample(
      makeSample({
        anthropicApi: { ip: '220.181.1.1', country: 'CN' },
        anthropicLogin: { ip: '220.181.1.1', country: 'CN' }
      })
    )
    harness.monitor.start()
    await harness.clock.flushMicrotasks()

    const reg = harness.monitor.registerSession('claude-late', 'anthropic')
    expect(reg).toEqual({ bornFrozen: true })
    expect(harness.monitor.status().families.find((f) => f.family === 'anthropic')).toMatchObject({
      frozenSessionIds: ['claude-late'],
      recovering: true
    })

    harness.setFullSample(makeSample())
    await harness.clock.advanceBy(15_000)
    await harness.clock.advanceBy(5_000)
    expect(harness.thawedCalls).toEqual(['claude-late'])

    harness.monitor.stop()
  })

  it('triggers full check on network signature change, wake clock drift, and qualifying light samples', async () => {
    const harness = createHarness()
    harness.monitor.start()
    await harness.clock.flushMicrotasks()
    expect(harness.sampleFullSpy).toHaveBeenCalledTimes(1)

    // Network signature unchanged at 5s -> no full check
    await harness.clock.advanceBy(5_000)
    expect(harness.sampleFullSpy).toHaveBeenCalledTimes(1)

    // Network signature changes -> full check within 5s
    harness.setSignature('en0:10.0.0.2')
    await harness.clock.advanceBy(5_000)
    expect(harness.sampleFullSpy).toHaveBeenCalledTimes(2)

    // Wake drift: clock jumps 20s -> triggers full check
    await harness.clock.jumpClock(20_000)
    expect(harness.sampleFullSpy).toHaveBeenCalledTimes(3)

    // Light sample at 60s with identical US IP -> no full check
    await harness.clock.advanceBy(60_000)
    expect(harness.sampleLightSpy).toHaveBeenCalledTimes(1)
    expect(harness.sampleFullSpy).toHaveBeenCalledTimes(3)

    // Light sample with changed IP -> triggers full check
    harness.setLightSample({ host: 'api.anthropic.com', ip: '169.128.9.9', country: 'US' })
    await harness.clock.advanceBy(60_000)
    expect(harness.sampleFullSpy).toHaveBeenCalledTimes(4)

    // Light sample returns CN -> triggers full check even if last full sample had same IP
    harness.setFullSample(makeSample({ anthropicApi: { ip: '169.128.9.9', country: 'US' } }))
    await harness.monitor.recheck()
    expect(harness.sampleFullSpy).toHaveBeenCalledTimes(5)

    harness.setLightSample({ host: 'api.anthropic.com', ip: '169.128.9.9', country: 'CN' })
    await harness.clock.advanceBy(60_000)
    expect(harness.sampleFullSpy).toHaveBeenCalledTimes(6)

    // Light sample recovering from previous full-check failure -> triggers full check
    harness.setFullSample(makeTimeoutSample())
    await harness.monitor.recheck()
    expect(harness.sampleFullSpy).toHaveBeenCalledTimes(7)

    harness.setLightSample({ host: 'api.anthropic.com', ip: '169.128.1.1', country: 'US' })
    await harness.clock.advanceBy(60_000)
    expect(harness.sampleFullSpy).toHaveBeenCalledTimes(8)

    harness.monitor.stop()
  })

  it('samples only anthropic during light checks when no sessions are registered', async () => {
    const harness = createHarness()
    harness.monitor.start()
    await harness.clock.flushMicrotasks()

    await harness.clock.advanceBy(60_000)
    expect(harness.sampleLightSpy.mock.calls).toEqual([['anthropic']])

    harness.monitor.stop()
  })

  it('samples openai when only codex is registered and freezes codex on CN light fallback', async () => {
    const harness = createHarness()
    harness.monitor.registerSession('codex-1', 'openai')
    harness.monitor.start()
    await harness.clock.flushMicrotasks()
    expect(harness.sampleFullSpy).toHaveBeenCalledTimes(1)

    const cnOpenaiSample = makeSample({
      openaiApi: { ip: '220.181.2.2', country: 'CN' },
      openaiLogin: { ip: '220.181.2.2', country: 'CN' }
    })
    harness.setFullSample(cnOpenaiSample)
    harness.setLightSample({ host: 'api.openai.com', ip: '220.181.2.2', country: 'CN' }, 'openai')

    await harness.clock.advanceBy(60_000)

    expect(harness.sampleLightSpy.mock.calls).toEqual([['openai']])
    expect(harness.sampleFullSpy).toHaveBeenCalledTimes(2)
    expect(harness.frozenCalls).toEqual(['codex-1'])
    expect(harness.monitor.status().families.find((f) => f.family === 'openai')).toMatchObject({
      state: 'abnormal',
      violation: 'mainland',
      frozenSessionIds: ['codex-1']
    })
    expect(harness.monitor.status().families.find((f) => f.family === 'anthropic')).toMatchObject({
      state: 'ok',
      frozenSessionIds: []
    })

    harness.monitor.stop()
  })

  it('samples all active providers concurrently and triggers only one full check when multiple change', async () => {
    const harness = createHarness()
    harness.monitor.registerSession('claude-1', 'anthropic')
    harness.monitor.registerSession('codex-1', 'openai')
    harness.monitor.start()
    await harness.clock.flushMicrotasks()
    expect(harness.sampleFullSpy).toHaveBeenCalledTimes(1)

    harness.setLightSample({ host: 'api.anthropic.com', ip: '169.128.2.2', country: 'US' }, 'anthropic')
    harness.setLightSample({ host: 'api.openai.com', ip: '198.51.100.2', country: 'US' }, 'openai')
    harness.setFullSample(
      makeSample({
        anthropicApi: { ip: '169.128.2.2', country: 'US' },
        anthropicLogin: { ip: '169.128.2.2', country: 'US' },
        openaiApi: { ip: '198.51.100.2', country: 'US' },
        openaiLogin: { ip: '198.51.100.2', country: 'US' }
      })
    )

    await harness.clock.advanceBy(60_000)

    expect(harness.sampleLightSpy.mock.calls).toEqual([['anthropic'], ['openai']])
    expect(harness.sampleFullSpy).toHaveBeenCalledTimes(2)

    harness.monitor.stop()
  })

  it('re-evaluates and freezes or thaws immediately when rule mode changes', async () => {
    const harness = createHarness()
    harness.monitor.registerSession('claude-1', 'anthropic')
    harness.monitor.start()
    await harness.clock.flushMicrotasks()

    // Same country (US), different IP -> ok under country mode
    harness.setFullSample(
      makeSample({
        anthropicApi: { ip: '169.128.55.55', country: 'US' },
        anthropicLogin: { ip: '169.128.55.55', country: 'US' }
      })
    )
    await harness.monitor.recheck()
    expect(harness.frozenCalls).toEqual([])

    // Switching to ip mode flags rule violation and freezes immediately
    await harness.monitor.setRuleMode('ip')
    expect(harness.frozenCalls).toEqual(['claude-1'])
    expect(harness.monitor.status().families.find((f) => f.family === 'anthropic')?.violation).toBe(
      'rule'
    )

    // Switching back to country mode resolves violation and thaws immediately
    await harness.monitor.setRuleMode('country')
    expect(harness.thawedCalls).toEqual(['claude-1'])

    harness.monitor.stop()
  })

  it('thaws all sessions and stops freezing when protection is disabled', async () => {
    const harness = createHarness()
    harness.setFullSample(
      makeSample({
        anthropicApi: { ip: '220.181.1.1', country: 'CN' },
        anthropicLogin: { ip: '220.181.1.1', country: 'CN' }
      })
    )
    harness.monitor.registerSession('claude-1', 'anthropic')
    harness.monitor.start()
    await harness.clock.flushMicrotasks()
    expect(harness.frozenCalls).toEqual(['claude-1'])

    await harness.monitor.setProtection(false)
    expect(harness.thawedCalls).toEqual(['claude-1'])
    expect(harness.monitor.status().protectionEnabled).toBe(false)
    expect(harness.monitor.registerSession('claude-2', 'anthropic')).toEqual({ bornFrozen: false })

    await harness.monitor.recheck()
    expect(harness.frozenCalls).toEqual(['claude-1'])

    harness.monitor.stop()
  })

  it('records baseline on first pass, freezes on country rule change, and thaws on acceptCurrent', async () => {
    const harness = createHarness()
    harness.monitor.registerSession('claude-1', 'anthropic')
    harness.monitor.start()
    await harness.clock.flushMicrotasks()

    expect(settings.rule().baselines.anthropic).toEqual({ ip: '169.128.1.1', country: 'US' })

    // Change exit to JP -> rule violation freezes claude-1
    harness.setFullSample(
      makeSample({
        anthropicApi: { ip: '203.0.113.20', country: 'JP' },
        anthropicLogin: { ip: '203.0.113.20', country: 'JP' }
      })
    )
    await harness.monitor.recheck()
    expect(harness.frozenCalls).toEqual(['claude-1'])
    expect(harness.monitor.status().families.find((f) => f.family === 'anthropic')).toMatchObject({
      state: 'abnormal',
      violation: 'rule',
      frozenSessionIds: ['claude-1']
    })

    // acceptCurrent updates baseline and thaws immediately
    await harness.monitor.acceptCurrent('anthropic')
    expect(harness.thawedCalls).toEqual(['claude-1'])
    expect(settings.rule().baselines.anthropic).toEqual({ ip: '203.0.113.20', country: 'JP' })
    expect(harness.monitor.status().families.find((f) => f.family === 'anthropic')).toMatchObject({
      state: 'ok',
      frozenSessionIds: []
    })

    // acceptCurrent throws when current exit fails mainland check
    harness.setFullSample(
      makeSample({
        anthropicApi: { ip: '220.181.1.1', country: 'CN' },
        anthropicLogin: { ip: '220.181.1.1', country: 'CN' }
      })
    )
    await harness.monitor.recheck()
    await expect(harness.monitor.acceptCurrent('anthropic')).rejects.toThrow()

    harness.monitor.stop()
  })

  it('coalesces concurrent full checks into a single in-flight probe', async () => {
    let resolveSample!: (sample: EgressSample) => void
    const harness = createHarness({
      sampleFull: () =>
        new Promise<EgressSample>((resolve) => {
          resolveSample = resolve
        })
    })

    harness.monitor.start()
    const p1 = harness.monitor.recheck()
    const p2 = harness.monitor.recheck()

    expect(harness.sampleFullSpy).toHaveBeenCalledTimes(1)

    resolveSample(makeSample())
    const [s1, s2] = await Promise.all([p1, p2])
    expect(s1.state).toBe('ok')
    expect(s2.state).toBe('ok')

    harness.monitor.stop()
  })

  it('swallows freeze and thaw errors per session without affecting other sessions', async () => {
    const harness = createHarness({
      freeze: async (sessionId) => {
        if (sessionId === 'claude-gone') throw new Error('session not found')
        harness.frozenCalls.push(sessionId)
      },
      thaw: async (sessionId) => {
        if (sessionId === 'claude-1') throw new Error('already closed')
        harness.thawedCalls.push(sessionId)
      }
    })

    harness.setFullSample(
      makeSample({
        anthropicApi: { ip: '220.181.1.1', country: 'CN' },
        anthropicLogin: { ip: '220.181.1.1', country: 'CN' }
      })
    )
    harness.monitor.registerSession('claude-gone', 'anthropic')
    harness.monitor.registerSession('claude-1', 'anthropic')
    harness.monitor.registerSession('claude-2', 'anthropic')

    harness.monitor.start()
    await harness.clock.flushMicrotasks()

    expect(harness.frozenCalls).toEqual(['claude-1', 'claude-2'])
    expect(
      harness.monitor.status().families.find((f) => f.family === 'anthropic')?.frozenSessionIds
    ).toEqual(['claude-1', 'claude-2'])

    harness.setFullSample(makeSample())
    await harness.clock.advanceBy(15_000)
    await harness.clock.advanceBy(5_000)

    expect(harness.thawedCalls).toEqual(['claude-2'])
    expect(
      harness.monitor.status().families.find((f) => f.family === 'anthropic')?.frozenSessionIds
    ).toEqual([])

    harness.monitor.stop()
  })

  it('removes sessions from all lists on unregisterSession and skips redundant publish when unchanged', async () => {
    const harness = createHarness()
    harness.setFullSample(
      makeSample({
        anthropicApi: { ip: '220.181.1.1', country: 'CN' },
        anthropicLogin: { ip: '220.181.1.1', country: 'CN' }
      })
    )
    harness.monitor.registerSession('claude-1', 'anthropic')
    harness.monitor.start()
    await harness.clock.flushMicrotasks()

    const publishCountBefore = harness.publishSpy.mock.calls.length
    harness.monitor.unregisterSession('unknown-session')
    expect(harness.publishSpy).toHaveBeenCalledTimes(publishCountBefore)

    harness.monitor.unregisterSession('claude-1')
    expect(harness.publishSpy).toHaveBeenCalledTimes(publishCountBefore + 1)
    expect(harness.monitor.status().sessionFamilies).toEqual({})
    expect(
      harness.monitor.status().families.find((f) => f.family === 'anthropic')?.frozenSessionIds
    ).toEqual([])

    harness.monitor.stop()
  })

  it('cancels all timers and never publishes after stop()', async () => {
    let resolveInFlight!: (sample: EgressSample) => void
    let callCount = 0
    const harness = createHarness({
      sampleFull: () => {
        callCount += 1
        if (callCount === 1) return Promise.resolve(makeSample())
        return new Promise<EgressSample>((resolve) => {
          resolveInFlight = resolve
        })
      }
    })

    harness.monitor.start()
    await harness.clock.flushMicrotasks()
    expect(harness.clock.activeCount()).toBeGreaterThan(0)

    const recheckPromise = harness.monitor.recheck()
    const publishedBeforeStop = harness.publishSpy.mock.calls.length

    harness.monitor.stop()
    expect(harness.clock.activeCount()).toBe(0)

    resolveInFlight(makeSample({ anthropicApi: { country: 'CN' } }))
    await recheckPromise
    await harness.clock.advanceBy(120_000)

    expect(harness.publishSpy).toHaveBeenCalledTimes(publishedBeforeStop)
  })

  it('omits chatgpt-subscription from checking and evaluated status when disabled', async () => {
    const harness = createHarness()
    expect(harness.monitor.status().state).toBe('checking')
    expect(harness.monitor.status().families.map((f) => f.family)).toEqual(['anthropic', 'openai'])

    harness.monitor.start()
    await harness.clock.flushMicrotasks()

    const status = harness.monitor.status()
    expect(status.state).toBe('ok')
    expect(status.families.map((f) => f.family)).toEqual(['anthropic', 'openai'])
    expect(status.checks.some((c) => c.family === 'chatgpt-subscription')).toBe(false)

    harness.monitor.stop()
  })

  it('freezes only chatgpt-subscription sessions when chatgpt-subscription is CN and recovers independently', async () => {
    const harness = createHarness()
    harness.setFullSample(
      makeSample({
        chatgptSubscriptionApi: { ip: '220.181.1.1', country: 'CN' },
        chatgptSubscriptionLogin: { ip: '220.181.1.1', country: 'CN' }
      })
    )

    harness.monitor.registerSession('claude-official', 'anthropic')
    harness.monitor.registerSession('codex-official', 'openai')
    harness.monitor.registerSession('claude-chatgpt', 'chatgpt-subscription')

    harness.monitor.start()
    await harness.clock.flushMicrotasks()

    expect(harness.frozenCalls).toEqual(['claude-chatgpt'])
    const status = harness.monitor.status()
    expect(status.state).toBe('abnormal')
    expect(status.families.map((f) => f.family)).toEqual([
      'anthropic',
      'openai',
      'chatgpt-subscription'
    ])
    expect(status.families.find((f) => f.family === 'anthropic')).toMatchObject({
      state: 'ok',
      frozenSessionIds: []
    })
    expect(status.families.find((f) => f.family === 'chatgpt-subscription')).toMatchObject({
      state: 'abnormal',
      violation: 'mainland',
      frozenSessionIds: ['claude-chatgpt'],
      recovering: true
    })

    // Recover chatgpt-subscription -> 15s recheck + 5s confirm -> thaws
    harness.setFullSample(
      makeSample({
        chatgptSubscriptionApi: { ip: '203.0.113.50', country: 'US' },
        chatgptSubscriptionLogin: { ip: '203.0.113.50', country: 'US' }
      })
    )
    await harness.clock.advanceBy(15_000)
    expect(harness.thawedCalls).toEqual([])
    await harness.clock.advanceBy(5_000)
    expect(harness.thawedCalls).toEqual(['claude-chatgpt'])
    expect(harness.monitor.status().state).toBe('ok')

    harness.monitor.stop()
  })
})

describe('egressFamilyForLaunch', () => {
  it('maps official claude-code and codex launches and excludes third-party or shell profiles', () => {
    expect(egressFamilyForLaunch('claude-code', {})).toBe('anthropic')
    expect(
      egressFamilyForLaunch('claude-code', { ANTHROPIC_BASE_URL: 'https://api.anthropic.com' })
    ).toBe('anthropic')
    expect(
      egressFamilyForLaunch('claude-code', { ANTHROPIC_BASE_URL: 'https://api.deepseek.com/anthropic' })
    ).toBeUndefined()
    expect(
      egressFamilyForLaunch('claude-code', { ANTHROPIC_BASE_URL: 'not a valid url' })
    ).toBeUndefined()

    expect(egressFamilyForLaunch('codex', {})).toBe('openai')
    expect(
      egressFamilyForLaunch('codex', { OPENAI_BASE_URL: 'https://api.openai.com/v1' })
    ).toBe('openai')
    expect(
      egressFamilyForLaunch('codex', { OPENAI_BASE_URL: 'https://openrouter.ai/api/v1' })
    ).toBeUndefined()
    expect(
      egressFamilyForLaunch('codex', { OPENAI_BASE_URL: '::bad-url::' })
    ).toBeUndefined()

    expect(egressFamilyForLaunch('shell', {})).toBeUndefined()
  })
})
