import { AbsoluteFill, OffthreadVideo, staticFile } from 'remotion'
import { Cursor } from './Cursor'
import { clamp, isClick, startsZoom } from './clip-events'
import { HEIGHT, WIDTH, type ClipData } from './manifest'
import { cursorAt, focusAt } from './zoom'

export interface ClipPlayerProps {
  clip: ClipData
  /** Time inside the source recording, in ms - recorder timestamps are clip-relative. */
  clipMs: number
  /** Frames cut off the head of the source video; 0 plays it from its own start. */
  trimBefore?: number
}

/**
 * One recording played back with its click-driven push-in and synthetic cursor.
 *
 * Shared by the full film (`SectionScene`, always from the clip's own start) and the 60s cut
 * (`Short60`, which drops into the middle of a recording via `trimBefore`). Captions deliberately
 * live outside this component so they never ride the zoom.
 */
export const ClipPlayer = ({ clip, clipMs, trimBefore = 0 }: ClipPlayerProps) => {
  const { viewport } = clip.events
  const toOutput = (x: number, y: number) => ({
    x: (x * WIDTH) / viewport.width,
    y: (y * HEIGHT) / viewport.height
  })
  const focus = focusAt(clip.events.events.filter(isClick).filter(startsZoom), clipMs, toOutput)
  const cursor = cursorAt(clip.events.events, clipMs)
  // Some recorded `move` targets sit outside the window: the recorder computes them from a
  // bounding box taken before the strip re-lays out, and a card that has since scrolled away
  // reports an x beyond the viewport (persist-b reaches 2293 on a 1504px window). The pointer is
  // drawn on top of footage that stops at the window edge, so it is clamped to it rather than
  // floating in the backdrop.
  const pointer = cursor
    ? toOutput(clamp(cursor.x, viewport.width), clamp(cursor.y, viewport.height))
    : null
  return (
    <AbsoluteFill
      style={{
        transformOrigin: '0 0',
        transform: `translate(${(1 - focus.scale) * focus.cx}px, ${(1 - focus.scale) * focus.cy}px) scale(${focus.scale})`
      }}
    >
      <OffthreadVideo
        src={staticFile(clip.file)}
        muted
        trimBefore={trimBefore > 0 ? trimBefore : undefined}
        style={{ width: '100%', height: '100%', objectFit: 'cover' }}
      />
      {pointer && cursor ? <Cursor x={pointer.x} y={pointer.y} pressed={cursor.pressed} /> : null}
    </AbsoluteFill>
  )
}
