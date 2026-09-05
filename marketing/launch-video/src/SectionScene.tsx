import { AbsoluteFill, Audio, OffthreadVideo, Sequence, staticFile, useCurrentFrame } from 'remotion'
import { Captions } from './Captions'
import { FPS, msToFrames, type SectionData } from './manifest'

const BACKDROP = '#0b0f14'

/**
 * One narration section: its screen recordings laid out at their recorded offsets, the
 * narration track, and the burned-in captions. Zoom and cursor overlays land in Task 7;
 * sections without recordings (intro / outro) get a flat backdrop until Task 8.
 */
export const SectionScene = ({ section }: { section: SectionData }) => {
  const frame = useCurrentFrame()
  return (
    <AbsoluteFill style={{ background: BACKDROP }}>
      {section.clips.length === 0 ? <AbsoluteFill style={{ background: BACKDROP }} /> : null}
      {section.clips.map((clip) => {
        const from = msToFrames(clip.startAtMs)
        // Never let a clip outrun its section - the narration length owns the timeline.
        const durationInFrames = Math.min(msToFrames(clip.durationMs), section.durationInFrames - from)
        if (durationInFrames <= 0) return null
        return (
          <Sequence key={clip.file} from={from} durationInFrames={durationInFrames}>
            <OffthreadVideo
              src={staticFile(clip.file)}
              muted
              style={{ width: '100%', height: '100%', objectFit: 'cover' }}
            />
          </Sequence>
        )
      })}
      <Audio src={staticFile(section.audio)} />
      <Captions cues={section.cues} nowMs={(frame / FPS) * 1000} />
    </AbsoluteFill>
  )
}
