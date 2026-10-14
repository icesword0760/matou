import { useEffect, useRef, useState } from 'react'
import type { ClaudeAccountsState } from '../../../shared/claude-accounts'

// Aggregate panel + HUD demand before crossing IPC: closing one consumer must
// not cancel another consumer in the same window. Main coalesces all windows.
const consumers = new Set<symbol>()
export function useAccountUsageDemand(active: boolean) {
  useEffect(() => {
    const bridge = window.matouDesktop
    if (!bridge?.claudeAccountCommand || !active) return
    const token = Symbol('account-usage')
    consumers.add(token)
    const report = () => {
      const visible = consumers.size > 0 && document.visibilityState === 'visible' && document.hasFocus()
      if (visible) void bridge.claudeAccountCommand!({ type: 'ensure-usage' }).catch(() => {})
      void bridge.claudeAccountCommand!({ type: 'usage-visibility', visible, recheck: true }).catch(() => {})
    }
    report()
    window.addEventListener('focus', report); window.addEventListener('blur', report); window.addEventListener('online', report)
    document.addEventListener('visibilitychange', report)
    return () => {
      consumers.delete(token)
      window.removeEventListener('focus', report); window.removeEventListener('blur', report); window.removeEventListener('online', report)
      document.removeEventListener('visibilitychange', report)
      report()
    }
  }, [active])
}

export function useAccountUsage(active: boolean) {
  const [state, setState] = useState<ClaudeAccountsState>()
  const revision = useRef(-1)
  useAccountUsageDemand(active)
  useEffect(() => {
    const bridge = window.matouDesktop
    if (!active || !bridge?.getClaudeAccounts || !bridge.onClaudeAccounts) return
    let mounted = true
    const apply = (next: ClaudeAccountsState) => {
      if (mounted && next.revision >= revision.current) { revision.current = next.revision; setState(next) }
    }
    const off = bridge.onClaudeAccounts(apply)
    void bridge.getClaudeAccounts().then(apply).catch(() => {})
    return () => { mounted = false; off() }
  }, [active])
  return active ? state : undefined
}
