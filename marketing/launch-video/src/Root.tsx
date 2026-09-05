import { Composition } from 'remotion'
import { LaunchVideo } from './LaunchVideo'
import { EMPTY_TIMELINE, FPS, HEIGHT, WIDTH, loadTimeline } from './manifest'

export const Root = () => (
  <Composition
    id="LaunchVideo"
    component={LaunchVideo}
    durationInFrames={EMPTY_TIMELINE.totalFrames}
    fps={FPS}
    width={WIDTH}
    height={HEIGHT}
    defaultProps={{ timeline: EMPTY_TIMELINE }}
    calculateMetadata={async () => {
      const timeline = await loadTimeline()
      return { durationInFrames: timeline.totalFrames, props: { timeline } }
    }}
  />
)
