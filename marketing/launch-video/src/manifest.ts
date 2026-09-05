import { staticFile } from 'remotion'
import type { RecorderEvent } from '../../../tests/e2e/launch-video/recorder'

export const FPS = 30
export const WIDTH = 1920
export const HEIGHT = 1080
/** Silent tail appended after every section's narration so cuts never clip the last word. */
export const TAIL_MS = 800

export interface CueData {
  startMs: number
  endMs: number
  text: string
}

export interface EventsData {
  fps: number
  scale: number
  viewport: { width: number; height: number }
  events: RecorderEvent[]
}

export interface ClipData {
  file: string
  startAtMs: number
  durationMs: number
  events: EventsData
}

export interface SectionData {
  id: string
  title: string
  audio: string
  durationMs: number
  cues: CueData[]
  clips: ClipData[]
  fromFrame: number
  durationInFrames: number
}

export interface TimelineData {
  fps: number
  width: number
  height: number
  sections: SectionData[]
  totalFrames: number
}

export const msToFrames = (ms: number): number => Math.round((ms / 1000) * FPS)

/**
 * Placeholder used as `defaultProps` before `calculateMetadata` has resolved.
 * Remotion requires `totalFrames >= 1`, so it cannot be 0.
 */
export const EMPTY_TIMELINE: TimelineData = {
  fps: FPS,
  width: WIDTH,
  height: HEIGHT,
  sections: [],
  totalFrames: 1
}

interface AudioManifest {
  voice: string
  rate: string
  sections: { id: string; title: string; durationMs: number; audio: string; cues: string }[]
}

interface RecordingsManifest {
  fps: number
  scale: number
  viewport: { width: number; height: number }
  sections: { id: string; clips: { file: string; events: string; startAtMs: number; durationMs: number }[] }[]
}

const fetchJson = async <T>(path: string): Promise<T> => {
  const response = await fetch(staticFile(path))
  if (!response.ok) {
    throw new Error(`failed to load ${path}: ${response.status} ${response.statusText}`)
  }
  return (await response.json()) as T
}

/**
 * Reads the audio + recordings manifests from `public/` and every `.cues.json` /
 * `.events.json` they reference, then lays the sections end to end on the timeline.
 *
 * Async on purpose: Remotion bundles with Webpack (no `import.meta.glob`), so the data is
 * fetched at `calculateMetadata` time rather than inlined at build time.
 */
export const loadTimeline = async (): Promise<TimelineData> => {
  const [audio, recordings] = await Promise.all([
    fetchJson<AudioManifest>('audio/manifest.json'),
    fetchJson<RecordingsManifest>('recordings/manifest.json')
  ])

  const loaded = await Promise.all(
    audio.sections.map(async (entry) => {
      const recorded = recordings.sections.find((section) => section.id === entry.id)
      const clipDescriptors = recorded?.clips ?? []
      const [cues, clipEvents] = await Promise.all([
        fetchJson<CueData[]>(entry.cues),
        Promise.all(clipDescriptors.map((clip) => fetchJson<EventsData>(clip.events)))
      ])
      const clips: ClipData[] = clipDescriptors.map((clip, index) => ({
        file: clip.file,
        startAtMs: clip.startAtMs,
        durationMs: clip.durationMs,
        events: clipEvents[index]!
      }))
      return { entry, cues, clips }
    })
  )

  let cursor = 0
  const sections = loaded.map(({ entry, cues, clips }): SectionData => {
    const durationMs = entry.durationMs + TAIL_MS
    const section: SectionData = {
      id: entry.id,
      title: entry.title,
      audio: entry.audio,
      durationMs,
      cues,
      clips,
      fromFrame: cursor,
      durationInFrames: msToFrames(durationMs)
    }
    cursor += section.durationInFrames
    return section
  })

  return { fps: FPS, width: WIDTH, height: HEIGHT, sections, totalFrames: cursor }
}
