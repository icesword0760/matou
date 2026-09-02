import { useCallback, useEffect, useState } from 'react'

import type { RuntimeClient, SessionRecoveryStatus } from './RuntimeClient'

type SessionRecoveryClient = Partial<Pick<
  RuntimeClient,
  'subscribeSessionRecovery' | 'prioritizeSessionRecovery' | 'retrySessionRecovery'
>>

export function useSessionRecovery(
  client: SessionRecoveryClient | null,
  activeSceneId?: string,
  activeSessionId?: string
): {
  statusBySession: ReadonlyMap<string, SessionRecoveryStatus>
  retry(sessionId: string): void
} {
  const [statusBySession, setStatusBySession] = useState(
    () => new Map<string, SessionRecoveryStatus>()
  )

  useEffect(() => {
    if (!client?.subscribeSessionRecovery) return
    return client.subscribeSessionRecovery((status) => {
      setStatusBySession((current) => {
        const next = new Map(current)
        next.set(status.sessionId, status)
        return next
      })
    })
  }, [client])

  useEffect(() => {
    if (!client?.prioritizeSessionRecovery || !activeSceneId) return
    client.prioritizeSessionRecovery(activeSceneId, activeSessionId)
  }, [activeSceneId, activeSessionId, client])

  const retry = useCallback((sessionId: string) => {
    client?.retrySessionRecovery?.(sessionId)
  }, [client])

  return { statusBySession, retry }
}
