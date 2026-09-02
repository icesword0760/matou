import { useCallback, useEffect, useState } from 'react'

import type { RuntimeClient, SessionRecoveryStatus } from './RuntimeClient'

type SessionRecoveryClient = Partial<Pick<
  RuntimeClient,
  'subscribeSessionRecovery' | 'prioritizeSessionRecovery' | 'retrySessionRecovery'
>>

export function useSessionRecovery(
  client: SessionRecoveryClient | null,
  activeSceneId?: string,
  activeSessionId?: string,
  foregroundSessionIds?: readonly string[]
): {
  statusBySession: ReadonlyMap<string, SessionRecoveryStatus>
  retry(sessionId: string): void
} {
  const [statusBySession, setStatusBySession] = useState(
    () => new Map<string, SessionRecoveryStatus>()
  )
  const foregroundSignature = foregroundSessionIds?.join('\u0000')

  useEffect(() => {
    if (!client?.subscribeSessionRecovery) return
    return client.subscribeSessionRecovery((status) => {
      setStatusBySession((current) => {
        const next = new Map(current)
        next.set(status.sessionId, status)
        return next
      })
    }, () => setStatusBySession(new Map()))
  }, [client])

  useEffect(() => {
    if (!client?.prioritizeSessionRecovery || !activeSceneId) return
    client.prioritizeSessionRecovery(
      activeSceneId,
      activeSessionId,
      foregroundSignature === undefined
        ? undefined
        : foregroundSignature === '' ? [] : foregroundSignature.split('\u0000')
    )
  }, [activeSceneId, activeSessionId, client, foregroundSignature])

  const retry = useCallback((sessionId: string) => {
    client?.retrySessionRecovery?.(sessionId)
  }, [client])

  return { statusBySession, retry }
}
