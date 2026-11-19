import type { SessionHudSnapshot } from './session-hud-registry'
const record = (value: unknown): Record<string, unknown> | undefined => value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : undefined
const text = (value: unknown, max = 256): string | undefined => typeof value === 'string' && value.trim() && value.length <= max ? value.trim() : undefined
const finite = (value: unknown): number | undefined => typeof value === 'number' && Number.isFinite(value) && value >= 0 ? value : undefined

/** Native DSH snapshots are complete optional facts, never Claude-style hooks. */
export function parseDshHud(value: unknown): Partial<SessionHudSnapshot> | undefined {
  const source = record(value)
  if (!source) return undefined
  const result: Partial<SessionHudSnapshot> = {}
  const model = text(source.model), effort = text(source.effort, 64), label = text(source.providerPermissionLabel)
  if (model) result.model = model
  if (model && effort) result.effort = effort
  if (label) result.providerPermissionLabel = label
  const cwd = text(source.cwd, 4096)
  if (cwd) result.cwd = cwd
  const percent = finite(source.contextPercent), window = finite(source.contextWindowSize)
  if (percent !== undefined && window !== undefined && window > 0) {
    result.contextPercent = Math.min(100, Math.round(percent)); result.contextWindowSize = window
  }
  if (Array.isArray(source.runningTools)) result.runningTools = source.runningTools.slice(0, 128).flatMap(row => {
    const name = text(record(row)?.name); return name ? [{ name }] : []
  })
  if (Array.isArray(source.toolCounts)) result.toolCounts = source.toolCounts.slice(0, 128).flatMap(row => {
    const item = record(row), name = text(item?.name), count = item?.count
    return name && typeof count === 'number' && Number.isSafeInteger(count) && count > 0 ? [{ name, count }] : []
  })
  const last = record(source.lastTool), name = text(last?.name)
  if (name && (last?.status === 'running' || last?.status === 'completed' || last?.status === 'error')) result.lastTool = { name, status: last.status }
  return result
}
