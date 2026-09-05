import { AbsoluteFill, Audio, Sequence, staticFile, useCurrentFrame } from 'remotion'
import { Captions } from './Captions'
import { ClipPlayer } from './ClipPlayer'
import { Intro } from './Intro'
import { Outro } from './Outro'
import { FPS, msToFrames, type SectionData } from './manifest'

const BACKDROP = '#0b0f14'

/**
 * One narration section: its screen recordings laid out at their recorded offsets, each with a
 * click-driven push-in and a synthetic cursor, plus the narration track and burned-in captions.
 * The two sections without recordings play their synthetic cards instead.
 */
export const SectionScene = ({ section }: { section: SectionData }) => {
  const frame = useCurrentFrame()
  const nowMs = (frame / FPS) * 1000
  return (
    <AbsoluteFill style={{ background: BACKDROP }}>
      {section.id === 'intro' ? <Intro /> : null}
      {section.id === 'outro' ? <Outro /> : null}
      {section.clips.map((clip) => {
        const from = msToFrames(clip.startAtMs)
        // Never let a clip outrun its section - the narration length owns the timeline.
        const durationInFrames = Math.min(msToFrames(clip.durationMs), section.durationInFrames - from)
        if (durationInFrames <= 0) return null
        return (
          <Sequence key={clip.file} from={from} durationInFrames={durationInFrames}>
            {/* Recorder timestamps are relative to each clip's own start, not the section's. */}
            <ClipPlayer clip={clip} clipMs={nowMs - clip.startAtMs} />
          </Sequence>
        )
      })}
      <Audio src={staticFile(section.audio)} />
      <Captions cues={section.cues} nowMs={nowMs} />
    </AbsoluteFill>
  )
}
