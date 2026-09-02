export interface DisplayPlacement {
  id: number
  label?: string
  internal?: boolean
  workArea: { x: number; y: number; width: number; height: number }
}

export function secondaryDisplayWindowBounds(input: {
  enabled: boolean
  width: number
  height: number
  primaryDisplayId: number
  displays: DisplayPlacement[]
}): { x: number; y: number } | undefined {
  if (!input.enabled) return undefined
  const candidates = input.displays.filter(({ id }) => id !== input.primaryDisplayId)
  const display = candidates.find(({ internal }) => internal === true) ??
    candidates.find(({ label = '' }) => /color\s*lcd|built[- ]?in|内建/i.test(label)) ??
    candidates[0]
  if (!display) return undefined
  const { workArea } = display
  return {
    x: workArea.x + Math.max(0, Math.floor((workArea.width - input.width) / 2)),
    y: workArea.y + Math.max(0, Math.floor((workArea.height - input.height) / 2))
  }
}
