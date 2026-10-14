import type { AccountUsage } from './claude-accounts'

export function expiredUsageWindow(value: { resetsAt: string } | undefined, now = Date.now()): boolean {
  const reset = Date.parse(value?.resetsAt ?? '')
  return Number.isFinite(reset) && reset <= now
}
export function validAccountUsage(value: unknown): value is AccountUsage {
  if (!value || typeof value !== 'object') return false
  const v = value as AccountUsage
  if (!Number.isFinite(v.checkedAt) || v.checkedAt <= 0 || typeof v.stale !== 'boolean') return false
  return [v.fiveHour, v.weekly].every(w => w === undefined || (
    w && typeof w.used === 'number' && Number.isFinite(w.used) && w.used >= 0 && w.used <= 100 &&
    typeof w.resetsAt === 'string' && (w.resetsAt === '' || Number.isFinite(Date.parse(w.resetsAt)))
  ))
}
