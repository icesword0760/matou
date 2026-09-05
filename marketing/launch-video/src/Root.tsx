import { Composition } from 'remotion'
import { COVER_HEIGHT, COVER_WIDTH, Cover } from './Cover'
import { LaunchVideo } from './LaunchVideo'
import { EMPTY_SHORT, Short60, loadShort60 } from './Short60'
import { EMPTY_TIMELINE, FPS, HEIGHT, WIDTH, loadTimeline } from './manifest'

export const Root = () => (
  <>
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
    <Composition
      id="Short60"
      component={Short60}
      durationInFrames={EMPTY_SHORT.totalFrames}
      fps={FPS}
      width={WIDTH}
      height={HEIGHT}
      defaultProps={{ short: EMPTY_SHORT }}
      calculateMetadata={async () => {
        const short = await loadShort60()
        return { durationInFrames: short.totalFrames, props: { short } }
      }}
    />
    {/* A `Composition` rather than a `Still`: `Still` pins fps to 1, and the frozen intro frame
        inside the cover is timed in the film's 30fps. */}
    <Composition
      id="Cover"
      component={Cover}
      durationInFrames={1}
      fps={FPS}
      width={COVER_WIDTH}
      height={COVER_HEIGHT}
    />
  </>
)
