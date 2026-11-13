import { useCallback, useMemo, useRef, useSyncExternalStore } from 'react'
import type {
  EgressFamily,
  EgressFamilyWire,
  EgressStatusWire
} from '@matou/contracts'
import type { RuntimeClient } from '../runtime/RuntimeClient'
import { useRuntimeClient } from '../runtime/RuntimeProvider'
import { countryName } from './egress-copy'

export type EgressRuleMode = EgressStatusWire['ruleMode']

export interface UseEgressStatusResult {
  readonly status: EgressStatusWire | undefined
  recheck: () => Promise<void>
  setRule: (mode: EgressRuleMode) => Promise<void>
  acceptCurrent: (family: EgressFamily) => Promise<void>
  setProtection: (enabled: boolean) => Promise<void>
  override: (family: EgressFamily) => Promise<void>
}

export type SessionEgressBannerView =
  | { readonly kind: 'none'; readonly frozen: false }
  | {
      readonly kind: 'frozen'
      readonly frozen: true
      readonly family: EgressFamily
      readonly violation: 'mainland' | 'rule'
      readonly baselineCountryCode?: string | undefined
      readonly currentCountryCode?: string | undefined
    }
  | {
      readonly kind: 'resumed'
      readonly frozen: false
      readonly transitionId: string
    }

const NONE_BANNER_VIEW: SessionEgressBannerView = { kind: 'none', frozen: false }

interface LegacyMockClient {
  request: <T = unknown>(method: string, payload?: unknown) => Promise<T>
  subscribeEgressStatus?: (
    listener: (status: EgressStatusWire) => void,
    onReconnect?: () => void
  ) => () => void
  getEgressStatusSnapshot?: () => EgressStatusWire | undefined
  updateEgressStatus?: (status: EgressStatusWire) => void
}

interface FallbackEgressStore {
  status: EgressStatusWire | undefined
  fetched: boolean
  pushSeq: number
  listeners: Set<() => void>
  subscribed: boolean
}

const fallbackStores = new WeakMap<object, FallbackEgressStore>()

function getFallbackStore(client: LegacyMockClient): FallbackEgressStore {
  let store = fallbackStores.get(client)
  if (!store) {
    store = {
      status: undefined,
      fetched: false,
      pushSeq: 0,
      listeners: new Set(),
      subscribed: false
    }
    fallbackStores.set(client, store)
  }
  return store
}

function notifyFallbackStore(store: FallbackEgressStore): void {
  for (const listener of store.listeners) listener()
}

function ensureFallbackStoreStarted(
  client: LegacyMockClient,
  store: FallbackEgressStore
): void {
  if (!store.subscribed && typeof client.subscribeEgressStatus === 'function') {
    store.subscribed = true
    const legacySubscribe = client.subscribeEgressStatus as (
      listener?: (status?: EgressStatusWire) => void,
      onReconnect?: () => void
    ) => (() => void) | void
    legacySubscribe(
      (pushed) => {
        if (pushed && isEgressStatusWire(pushed)) {
          store.pushSeq += 1
          store.status = pushed
          notifyFallbackStore(store)
        }
      },
      () => {
        store.fetched = false
        store.status = undefined
        notifyFallbackStore(store)
        void fetchFallbackStatus(client, store)
      }
    )
  }
  if (!store.fetched) {
    void fetchFallbackStatus(client, store)
  }
}

async function fetchFallbackStatus(
  client: LegacyMockClient,
  store: FallbackEgressStore
): Promise<void> {
  if (store.fetched) return
  store.fetched = true
  const pushSeqAtStart = store.pushSeq
  try {
    const next = await client.request<EgressStatusWire>('egress.status', undefined)
    if (store.pushSeq === pushSeqAtStart && isEgressStatusWire(next)) {
      store.status = next
      notifyFallbackStore(store)
    }
  } catch {
    // Silent fallback: RPC failure hides egress UI without blocking cards.
  }
}

function publishClientStatus(
  client: LegacyMockClient,
  status: EgressStatusWire
): void {
  if (!isEgressStatusWire(status)) return
  if (typeof client.updateEgressStatus === 'function') {
    client.updateEgressStatus(status)
    return
  }
  const store = getFallbackStore(client)
  store.pushSeq += 1
  store.status = status
  notifyFallbackStore(store)
}

export function isEgressStatusWire(value: unknown): value is EgressStatusWire {
  if (!value || typeof value !== 'object') return false
  const record = value as Record<string, unknown>
  return (
    Array.isArray(record.families) &&
    Array.isArray(record.checks) &&
    Boolean(record.sessionFamilies) &&
    typeof record.sessionFamilies === 'object'
  )
}

export function sessionEgressFamily(
  status: EgressStatusWire | undefined,
  sessionId: string
): EgressFamily | undefined {
  return status?.sessionFamilies?.[sessionId]
}

export function findEgressFamily(
  status: EgressStatusWire | undefined,
  family: EgressFamily | undefined
): EgressFamilyWire | undefined {
  if (!status || !family || !Array.isArray(status.families)) return undefined
  return status.families.find((entry) => entry.family === family)
}

export function isSessionEgressFrozen(
  status: EgressStatusWire | undefined,
  sessionId: string
): boolean {
  const family = sessionEgressFamily(status, sessionId)
  const entry = findEgressFamily(status, family)
  if (!entry || entry.overridden) return false
  return entry.frozenSessionIds.includes(sessionId)
}

/** Extract only the frozen/resumed banner fields relevant to `sessionId`. */
export function selectSessionEgressBannerView(
  status: EgressStatusWire | undefined,
  sessionId: string
): SessionEgressBannerView {
  if (!status) return NONE_BANNER_VIEW
  const family = sessionEgressFamily(status, sessionId)
  const familyEntry = findEgressFamily(status, family)
  if (!family || !familyEntry) return NONE_BANNER_VIEW

  if (isSessionEgressFrozen(status, sessionId)) {
    const violation = familyEntry.violation ?? 'mainland'
    if (violation === 'rule') {
      const ruleCheck = status.checks.find(
        (check) => check.family === family && check.id === 'exit-rule'
      )
      return {
        kind: 'frozen',
        frozen: true,
        family,
        violation,
        baselineCountryCode:
          familyEntry.baseline?.country ?? ruleCheck?.facts?.baselineCountry,
        currentCountryCode:
          familyEntry.exit?.country ?? ruleCheck?.facts?.currentCountry
      }
    }
    return {
      kind: 'frozen',
      frozen: true,
      family,
      violation: 'mainland'
    }
  }

  if (
    status.lastTransition?.kind === 'resumed' &&
    status.lastTransition.family === family
  ) {
    return {
      kind: 'resumed',
      frozen: false,
      transitionId: status.lastTransition.id
    }
  }

  return NONE_BANNER_VIEW
}

export function equalSessionEgressBannerView(
  a: SessionEgressBannerView,
  b: SessionEgressBannerView
): boolean {
  if (a === b) return true
  if (a.kind !== b.kind || a.frozen !== b.frozen) return false
  if (a.kind === 'none') return true
  if (a.kind === 'resumed' && b.kind === 'resumed') {
    return a.transitionId === b.transitionId
  }
  if (a.kind === 'frozen' && b.kind === 'frozen') {
    return (
      a.family === b.family &&
      a.violation === b.violation &&
      a.baselineCountryCode === b.baselineCountryCode &&
      a.currentCountryCode === b.currentCountryCode
    )
  }
  return false
}

export function formatBannerFreezeMessage(
  banner: Extract<SessionEgressBannerView, { kind: 'frozen' }>,
  zh: boolean
): string {
  if (banner.violation === 'rule') {
    const baseline = countryName(banner.baselineCountryCode, zh)
    const current = countryName(banner.currentCountryCode, zh)
    return zh
      ? `出口与规则不符（${baseline} → ${current}），已暂停。网络恢复后自动继续。`
      : `Exit doesn't match your rule (${baseline} → ${current}). Paused; resumes automatically when the network recovers.`
  }
  return zh
    ? '出口异常（检测到中国大陆 IP），已暂停。网络恢复后自动继续。'
    : 'Exit problem (mainland China IP detected). Paused; resumes automatically when the network recovers.'
}

const noopSubscribe = () => () => {}
const undefinedSnapshot = () => undefined

/**
 * Subscribe to a derived slice of the shared `RuntimeClient` egress status cache via `useSyncExternalStore`.
 * Components only re-render when `isEqual(previousSlice, nextSlice)` returns false.
 */
export function useEgressSelector<T>(
  selector: (status: EgressStatusWire | undefined) => T,
  isEqual: (prev: T, next: T) => boolean = Object.is,
  clientOverride?: RuntimeClient | null
): T {
  const contextClient = useRuntimeClient()
  const rawClient = (clientOverride === undefined ? contextClient : clientOverride) as
    | LegacyMockClient
    | null
  const activeClient =
    rawClient && typeof rawClient.subscribeEgressStatus === 'function'
      ? rawClient
      : null

  const storeBinding = useMemo(() => {
    if (!activeClient) {
      return {
        subscribe: noopSubscribe,
        getRawSnapshot: undefinedSnapshot as () => EgressStatusWire | undefined
      }
    }
    if (
      typeof activeClient.getEgressStatusSnapshot === 'function' &&
      typeof activeClient.subscribeEgressStatus === 'function'
    ) {
      return {
        subscribe: activeClient.subscribeEgressStatus as (listener: () => void) => () => void,
        getRawSnapshot: activeClient.getEgressStatusSnapshot
      }
    }
    const fallbackStore = getFallbackStore(activeClient)
    return {
      subscribe: (listener: () => void) => {
        fallbackStore.listeners.add(listener)
        ensureFallbackStoreStarted(activeClient, fallbackStore)
        return () => {
          fallbackStore.listeners.delete(listener)
        }
      },
      getRawSnapshot: () => fallbackStore.status
    }
  }, [activeClient])

  const prevStatusRef = useRef<EgressStatusWire | undefined>(undefined)
  const hasSelectionRef = useRef(false)
  const prevSelectionRef = useRef<T>(undefined as T)

  const getSelectedSnapshot = useCallback(() => {
    const status = storeBinding.getRawSnapshot()
    if (hasSelectionRef.current && Object.is(prevStatusRef.current, status)) {
      return prevSelectionRef.current
    }
    const nextSelection = selector(status)
    prevStatusRef.current = status
    if (hasSelectionRef.current && isEqual(prevSelectionRef.current, nextSelection)) {
      return prevSelectionRef.current
    }
    hasSelectionRef.current = true
    prevSelectionRef.current = nextSelection
    return nextSelection
  }, [isEqual, selector, storeBinding])

  return useSyncExternalStore(
    storeBinding.subscribe,
    getSelectedSnapshot,
    getSelectedSnapshot
  )
}

/** Expose egress RPC mutators without subscribing the caller to status updates. */
export function useEgressActions(
  clientOverride?: RuntimeClient | null
): Omit<UseEgressStatusResult, 'status'> {
  const contextClient = useRuntimeClient()
  const rawClient = (clientOverride === undefined ? contextClient : clientOverride) as
    | LegacyMockClient
    | null

  const recheck = useCallback(async () => {
    if (!rawClient) return
    try {
      const next = await rawClient.request<EgressStatusWire>('egress.recheck', undefined)
      publishClientStatus(rawClient, next)
    } catch {
      // Ignore RPC failures gracefully.
    }
  }, [rawClient])

  const setRule = useCallback(async (mode: EgressRuleMode) => {
    if (!rawClient) return
    try {
      const next = await rawClient.request<EgressStatusWire>('egress.set-rule', { mode })
      publishClientStatus(rawClient, next)
    } catch {
      // Ignore RPC failures gracefully.
    }
  }, [rawClient])

  const acceptCurrent = useCallback(async (family: EgressFamily) => {
    if (!rawClient) return
    try {
      const next = await rawClient.request<EgressStatusWire>('egress.accept-current', { family })
      publishClientStatus(rawClient, next)
    } catch {
      // Ignore RPC failures gracefully.
    }
  }, [rawClient])

  const setProtection = useCallback(async (enabled: boolean) => {
    if (!rawClient) return
    try {
      const next = await rawClient.request<EgressStatusWire>('egress.set-protection', { enabled })
      publishClientStatus(rawClient, next)
    } catch {
      // Ignore RPC failures gracefully.
    }
  }, [rawClient])

  const override = useCallback(async (family: EgressFamily) => {
    if (!rawClient) return
    try {
      const next = await rawClient.request<EgressStatusWire>('egress.override', { family })
      publishClientStatus(rawClient, next)
    } catch {
      // Ignore RPC failures gracefully.
    }
  }, [rawClient])

  return useMemo(
    () => ({ recheck, setRule, acceptCurrent, setProtection, override }),
    [recheck, setRule, acceptCurrent, setProtection, override]
  )
}

/** Subscribe only to the frozen/resumed banner view for a single session. */
export function useSessionEgressBanner(
  sessionId: string,
  clientOverride?: RuntimeClient | null
): SessionEgressBannerView {
  const selector = useCallback(
    (status: EgressStatusWire | undefined) => selectSessionEgressBannerView(status, sessionId),
    [sessionId]
  )
  return useEgressSelector(selector, equalSessionEgressBannerView, clientOverride)
}

/** Subscribe to live egress status from the shared RuntimeClient store and expose the egress RPC actions. */
export function useEgressStatus(
  clientOverride?: RuntimeClient | null
): UseEgressStatusResult {
  const status = useEgressSelector((current) => current, Object.is, clientOverride)
  const actions = useEgressActions(clientOverride)
  return useMemo(() => ({ status, ...actions }), [status, actions])
}
