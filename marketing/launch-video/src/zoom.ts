import type { RecorderEvent } from '../../../tests/e2e/launch-video/recorder'

export interface ZoomConfig { scale: number; inMs: number; holdMs: number; outMs: number; mergeGapMs: number }
// 1.35 rather than a more dramatic 1.6: at 1.6 a 1504x846 recording upscaled into a 1920x1080
// frame reads soft, and the push-in crops enough of the card strip that the surrounding context
// (which is the point of every one of these shots) leaves the frame.
export const DEFAULT_ZOOM: ZoomConfig = { scale: 1.35, inMs: 450, holdMs: 1800, outMs: 650, mergeGapMs: 2500 }
/** cx / cy are output-frame pixels (1920x1080), not the recording's CSS pixels. */
export interface Focus { scale: number; cx: number; cy: number }

const easeInOut = (p: number) => (p < 0.5 ? 4 * p * p * p : 1 - Math.pow(-2 * p + 2, 3) / 2)
const clamp01 = (v: number) => Math.min(1, Math.max(0, v))

// Groups clicks into zoom "shots": a shot starts at a click and extends while following clicks arrive within mergeGapMs.
function shots(clicks: Array<{ t: number; x: number; y: number }>, cfg: ZoomConfig) {
  const sorted = [...clicks].sort((a, b) => a.t - b.t)
  const out: Array<{ start: number; end: number; points: typeof sorted }> = []
  for (const click of sorted) {
    const last = out[out.length - 1]
    if (last && click.t - last.points[last.points.length - 1]!.t <= cfg.mergeGapMs) { last.points.push(click); last.end = click.t + cfg.inMs + cfg.holdMs }
    else out.push({ start: click.t, end: click.t + cfg.inMs + cfg.holdMs, points: [click] })
  }
  return out
}

export function focusAt(clicks: Array<{ t: number; x: number; y: number }>, tMs: number,
  toOutput: (x: number, y: number) => { x: number; y: number }, cfg: ZoomConfig = DEFAULT_ZOOM): Focus {
  const idle: Focus = { scale: 1, cx: 960, cy: 540 }
  for (const shot of shots(clicks, cfg)) {
    if (tMs < shot.start || tMs > shot.end + cfg.outMs) continue
    const scale = tMs < shot.start + cfg.inMs
      ? 1 + (cfg.scale - 1) * easeInOut(clamp01((tMs - shot.start) / cfg.inMs))
      : tMs <= shot.end ? cfg.scale : cfg.scale - (cfg.scale - 1) * easeInOut(clamp01((tMs - shot.end) / cfg.outMs))
    // focus point: current target, eased from the previous point over inMs after each click
    let index = 0
    while (index + 1 < shot.points.length && shot.points[index + 1]!.t <= tMs) index += 1
    const target = toOutput(shot.points[index]!.x, shot.points[index]!.y)
    if (index === 0) return { scale, cx: target.x, cy: target.y }
    const previous = toOutput(shot.points[index - 1]!.x, shot.points[index - 1]!.y)
    const p = easeInOut(clamp01((tMs - shot.points[index]!.t) / cfg.inMs))
    return { scale, cx: previous.x + (target.x - previous.x) * p, cy: previous.y + (target.y - previous.y) * p }
  }
  return idle
}

/** Synthetic pointer position in the recording's CSS pixels, or null before the first move. */
export function cursorAt(events: RecorderEvent[], tMs: number): { x: number; y: number; pressed: boolean } | null {
  const moves = events.filter((e): e is Extract<RecorderEvent, { type: 'move' }> => e.type === 'move' && e.t <= tMs)
  if (moves.length === 0) return null
  const current = moves[moves.length - 1]!
  const previous = moves.length > 1 ? moves[moves.length - 2]! : current
  const p = current.ms > 0 ? easeInOut(clamp01((tMs - current.t) / current.ms)) : 1
  const pressed = events.some((e) => e.type === 'click' && tMs >= e.t && tMs < e.t + 120)
  return { x: previous.x + (current.x - previous.x) * p, y: previous.y + (current.y - previous.y) * p, pressed }
}
