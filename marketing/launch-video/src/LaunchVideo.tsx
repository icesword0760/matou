import { AbsoluteFill, Sequence } from 'remotion'
import type { TimelineData } from './manifest'
import { SectionScene } from './SectionScene'

export const LaunchVideo = ({ timeline }: { timeline: TimelineData }) => (
  <AbsoluteFill style={{ background: '#0b0f14' }}>
    {timeline.sections.map((section) => (
      <Sequence
        key={section.id}
        from={section.fromFrame}
        durationInFrames={section.durationInFrames}
        name={section.title}
      >
        <SectionScene section={section} />
      </Sequence>
    ))}
  </AbsoluteFill>
)
