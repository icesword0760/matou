import { useCallback, useEffect, useRef, useState } from 'react'
import type {
  EgressFamily,
  EgressFamilyWire,
  EgressStatusWire
} from '@matou/contracts'
import { useRuntimeClient } from '../runtime/RuntimeProvider'

export type EgressRuleMode = EgressStatusWire['ruleMode']

export interface UseEgressStatusResult {
  readonly status: EgressStatusWire | undefined
  recheck: () => Promise<void>
  setRule: (mode: EgressRuleMode) => Promise<void>
  acceptCurrent: (family: EgressFamily) => Promise<void>
  setProtection: (enabled: boolean) => Promise<void>
  override: (family: EgressFamily) => Promise<void>
}

export function sessionEgressFamily(
  status: EgressStatusWire | undefined,
  sessionId: string
): EgressFamily | undefined {
  return status?.sessionFamilies[sessionId]
}

export function findEgressFamily(
  status: EgressStatusWire | undefined,
  family: EgressFamily | undefined
): EgressFamilyWire | undefined {
  if (!status || !family) return undefined
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

/** Subscribe to live egress status from the runtime and expose the egress RPC actions. */
export function useEgressStatus(
  clientOverride?: ReturnType<typeof useRuntimeClient>
): UseEgressStatusResult {
  const contextClient = useRuntimeClient()
  const client = clientOverride === undefined ? contextClient : clientOverride
  const [status, setStatus] = useState<EgressStatusWire | undefined>(undefined)
  const generationRef = useRef(0)

  useEffect(() => {
    if (!client) {
      setStatus(undefined)
      return
    }
    let cancelled = false

    const fetchStatus = async (): Promise<void> => {
      const generation = generationRef.current
      try {
        const next = await client.request<EgressStatusWire>('egress.status', undefined)
        if (!cancelled && generationRef.current === generation) {
          setStatus(next)
        }
      } catch {
        // Silent fallback: RPC failure hides egress UI without blocking cards.
      }
    }

    const unsubscribe = client.subscribeEgressStatus?.(
      (pushed) => {
        if (cancelled) return
        generationRef.current += 1
        setStatus(pushed)
      },
      () => {
        if (cancelled) return
        generationRef.current += 1
        void fetchStatus()
      }
    )

    void fetchStatus()

    return () => {
      cancelled = true
      unsubscribe?.()
    }
  }, [client])

  const recheck = useCallback(async () => {
    if (!client) return
    try {
      const next = await client.request<EgressStatusWire>('egress.recheck', undefined)
      generationRef.current += 1
      setStatus(next)
    } catch {
      // Ignore RPC failures gracefully.
    }
  }, [client])

  const setRule = useCallback(async (mode: EgressRuleMode) => {
    if (!client) return
    try {
      const next = await client.request<EgressStatusWire>('egress.set-rule', { mode })
      generationRef.current += 1
      setStatus(next)
    } catch {
      // Ignore RPC failures gracefully.
    }
  }, [client])

  const acceptCurrent = useCallback(async (family: EgressFamily) => {
    if (!client) return
    try {
      const next = await client.request<EgressStatusWire>('egress.accept-current', { family })
      generationRef.current += 1
      setStatus(next)
    } catch {
      // Ignore RPC failures gracefully.
    }
  }, [client])

  const setProtection = useCallback(async (enabled: boolean) => {
    if (!client) return
    try {
      const next = await client.request<EgressStatusWire>('egress.set-protection', { enabled })
      generationRef.current += 1
      setStatus(next)
    } catch {
      // Ignore RPC failures gracefully.
    }
  }, [client])

  const override = useCallback(async (family: EgressFamily) => {
    if (!client) return
    try {
      const next = await client.request<EgressStatusWire>('egress.override', { family })
      generationRef.current += 1
      setStatus(next)
    } catch {
      // Ignore RPC failures gracefully.
    }
  }, [client])

  return { status, recheck, setRule, acceptCurrent, setProtection, override }
}
