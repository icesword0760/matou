import { useEffect, useRef } from 'react'
import type { EgressStatusWire } from '@matou/contracts'
import type {
  AgentNotificationInput,
  AgentNotificationStore
} from '../notifications/AgentNotificationStore'
import { useEgressStatus } from './use-egress-status'

/**
 * Derive an attention notification from a transition between two egress status snapshots.
 * Returns `undefined` for the initial snapshot (historical transitions must not notify)
 * or when `lastTransition.id` has not changed since `previous`.
 */
export function egressNotification(
  previous: EgressStatusWire | undefined,
  next: EgressStatusWire,
  zh: boolean
): AgentNotificationInput | undefined {
  if (!previous || !next.lastTransition) return undefined
  if (previous.lastTransition?.id === next.lastTransition.id) return undefined

  const transition = next.lastTransition
  if (transition.kind === 'frozen') {
    const body =
      transition.reason === 'rule'
        ? zh
          ? '出口与规则不符。网络恢复后自动继续。'
          : "Exit doesn't match your rule. Resumes automatically when the network recovers."
        : zh
          ? '检测到中国大陆 IP。网络恢复后自动继续。'
          : 'Mainland China IP detected. Resumes automatically when the network recovers.'
    return {
      eventId: `egress:${transition.id}`,
      eventType: 'attention',
      replacementKey: 'egress',
      title: zh
        ? `出口异常，已暂停 ${transition.sessionCount} 个会话`
        : `Exit problem: ${transition.sessionCount} session(s) paused`,
      body
    }
  }

  return {
    eventId: `egress:${transition.id}`,
    eventType: 'attention',
    replacementKey: 'egress',
    title: zh
      ? `出口已恢复，${transition.sessionCount} 个会话已继续`
      : `Exit restored: ${transition.sessionCount} session(s) resumed`
  }
}

/** Watch live egress status transitions and push deduplicated system notifications once per transition id. */
export function useEgressNotifications(
  store: AgentNotificationStore,
  zh: boolean
): void {
  const { status } = useEgressStatus()
  const initializedRef = useRef(false)
  const previousRef = useRef<EgressStatusWire | undefined>(undefined)
  const seenTransitionIdsRef = useRef(new Set<string>())

  useEffect(() => {
    if (!status) return
    if (!initializedRef.current) {
      initializedRef.current = true
      if (status.lastTransition?.id) {
        seenTransitionIdsRef.current.add(status.lastTransition.id)
      }
      previousRef.current = status
      return
    }
    const previous = previousRef.current
    previousRef.current = status
    const transitionId = status.lastTransition?.id
    if (!transitionId || seenTransitionIdsRef.current.has(transitionId)) return
    const notification = egressNotification(previous, status, zh)
    if (!notification) return
    seenTransitionIdsRef.current.add(transitionId)
    store.push(notification)
  }, [status, store, zh])
}
