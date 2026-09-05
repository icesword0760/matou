import { AbsoluteFill, Audio, OffthreadVideo, Sequence, staticFile, useCurrentFrame } from 'remotion'
import type { RecorderEvent } from '../../../tests/e2e/launch-video/recorder'
import { Captions } from './Captions'
import { Cursor } from './Cursor'
import { FPS, HEIGHT, WIDTH, msToFrames, type SectionData } from './manifest'
import { cursorAt, focusAt } from './zoom'

const BACKDROP = '#0b0f14'

const isClick = (e: RecorderEvent): e is Extract<RecorderEvent, { type: 'click' }> => e.type === 'click'

/**
 * One narration section: its screen recordings laid out at their recorded offsets, each with a
 * click-driven push-in and a synthetic cursor, plus the narration track and burned-in captions.
 * Sections without recordings (intro / outro) get a flat backdrop until Task 8.
 */
export const SectionScene = ({ section }: { section: SectionData }) => {
  const frame = useCurrentFrame()
  const nowMs = (frame / FPS) * 1000
  return (
    <AbsoluteFill style={{ background: BACKDROP }}>
      {section.clips.length === 0 ? <AbsoluteFill style={{ background: BACKDROP }} /> : null}
      {section.clips.map((clip) => {
        const from = msToFrames(clip.startAtMs)
        // Never let a clip outrun its section - the narration length owns the timeline.
        const durationInFrames = Math.min(msToFrames(clip.durationMs), section.durationInFrames - from)
        if (durationInFrames <= 0) return null
        // Recorder timestamps are relative to each clip's own start, not the section's.
        const clipMs = nowMs - clip.startAtMs
        const { viewport } = clip.events
        const toOutput = (x: number, y: number) => ({
          x: (x * WIDTH) / viewport.width,
          y: (y * HEIGHT) / viewport.height
        })
        const focus = focusAt(clip.events.events.filter(isClick), clipMs, toOutput)
        const cursor = cursorAt(clip.events.events, clipMs)
        const pointer = cursor ? toOutput(cursor.x, cursor.y) : null
        return (
          <Sequence key={clip.file} from={from} durationInFrames={durationInFrames}>
            {/* Video + cursor scale together; captions stay outside so they never zoom. */}
            <AbsoluteFill
              style={{
                transformOrigin: '0 0',
                transform: `translate(${(1 - focus.scale) * focus.cx}px, ${(1 - focus.scale) * focus.cy}px) scale(${focus.scale})`
              }}
            >
              <OffthreadVideo
                src={staticFile(clip.file)}
                muted
                style={{ width: '100%', height: '100%', objectFit: 'cover' }}
              />
              {pointer && cursor ? <Cursor x={pointer.x} y={pointer.y} pressed={cursor.pressed} /> : null}
            </AbsoluteFill>
          </Sequence>
        )
      })}
      <Audio src={staticFile(section.audio)} />
      <Captions cues={section.cues} nowMs={nowMs} />
    </AbsoluteFill>
  )
}
