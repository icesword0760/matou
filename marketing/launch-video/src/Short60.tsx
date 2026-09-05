import { AbsoluteFill, Audio, Sequence, staticFile, useCurrentFrame } from 'remotion'
import type { RecorderEvent } from '../../../tests/e2e/launch-video/recorder'
import { Captions } from './Captions'
import { ClipPlayer } from './ClipPlayer'
import { Intro } from './Intro'
import { Outro } from './Outro'
import {
  FPS,
  HEIGHT,
  WIDTH,
  fetchJson,
  loadTimeline,
  msToFrames,
  type AudioManifest,
  type ClipData,
  type CueData,
  type TimelineData
} from './manifest'

const BACKDROP = '#0b0f14'
/** Bilibili cuts anything past a minute out of the vertical feed, so this is a hard ceiling. */
export const SHORT_MAX_FRAMES = 60 * FPS
/** Breathing room after each line of narration - the short's tighter answer to `TAIL_MS`. */
const PAD_MS = 500
/** Enter the AI segment this far before the `mt-read` mark, so the click that starts it is on screen. */
const AI_LEAD_MS = 2000
/** Share of the DAG segment spent on the graph before cutting to the restart. */
const DAG_SPLIT = 0.6

/** One recording playing inside a segment, entered at `startMs` into the source. */
export interface ShortPart {
  clip: ClipData
  startMs: number
  fromFrame: number
  durationInFrames: number
}

export interface ShortSegment {
  id: string
  title: string
  audio: string
  cues: CueData[]
  fromFrame: number
  durationInFrames: number
  /** A synthetic card instead of footage, for the hook and the call to action. */
  card: 'intro' | 'outro' | null
  parts: ShortPart[]
}

export interface ShortData {
  fps: number
  width: number
  height: number
  segments: ShortSegment[]
  totalFrames: number
}

/** `defaultProps` before `calculateMetadata` resolves; Remotion needs `totalFrames >= 1`. */
export const EMPTY_SHORT: ShortData = { fps: FPS, width: WIDTH, height: HEIGHT, segments: [], totalFrames: 1 }

const clipOf = (timeline: TimelineData, sectionId: string, file?: string): ClipData => {
  const section = timeline.sections.find((s) => s.id === sectionId)
  const clip = file ? section?.clips.find((c) => c.file.includes(file)) : section?.clips[0]
  if (!clip) throw new Error(`short cut needs clip "${file ?? '#0'}" of section "${sectionId}"`)
  return clip
}

const eventTime = (clip: ClipData, match: (e: RecorderEvent) => boolean, what: string): number => {
  const found = clip.events.events.find(match)
  if (!found) throw new Error(`short cut needs ${what} in ${clip.file}`)
  return found.t
}

/** A part clamped to what is actually left in the recording after `startMs`. */
const part = (clip: ClipData, startMs: number, fromFrame: number, wanted: number): ShortPart => {
  const durationInFrames = Math.min(wanted, msToFrames(clip.durationMs - startMs))
  if (durationInFrames <= 0) {
    throw new Error(`${clip.file} has nothing left after ${startMs}ms (clip is ${clip.durationMs}ms)`)
  }
  return { clip, startMs, fromFrame, durationInFrames }
}

/** What each line of the short narration is cut against. */
const planFor = (
  id: string,
  timeline: TimelineData,
  durationInFrames: number
): { card: ShortSegment['card']; parts: ShortPart[] } => {
  switch (id) {
    case 's-hook':
      return { card: 'intro', parts: [] }
    case 's-cta':
      return { card: 'outro', parts: [] }
    case 's-what':
      return { card: null, parts: [part(clipOf(timeline, 'why'), 0, 0, durationInFrames)] }
    case 's-ai': {
      const clip = clipOf(timeline, 'ai-control')
      const mark = eventTime(clip, (e) => e.type === 'mark' && e.name === 'mt-read', "a 'mt-read' mark")
      return { card: null, parts: [part(clip, Math.max(0, mark - AI_LEAD_MS), 0, durationInFrames)] }
    }
    case 's-dag': {
      const graph = clipOf(timeline, 'fork-dag')
      const dagAt = eventTime(graph, (e) => e.type === 'source' && e.source === 'dag', "a 'dag' source switch")
      const split = Math.round(durationInFrames * DAG_SPLIT)
      return {
        card: null,
        parts: [
          part(graph, dagAt, 0, split),
          part(clipOf(timeline, 'persist', 'persist-b'), 0, split, durationInFrames - split)
        ]
      }
    }
    default:
      throw new Error(`no short-cut plan for narration section "${id}"`)
  }
}

/**
 * Cuts the 60s version out of the full film's footage against its own narration
 * (`public/audio-short`, from `node tts/synthesize.mjs --script script/narration-short.json --out public/audio-short`).
 * Each segment runs for its line of narration plus `PAD_MS`; the total is asserted under a minute.
 */
export const loadShort60 = async (): Promise<ShortData> => {
  const [timeline, narration] = await Promise.all([
    loadTimeline(),
    fetchJson<AudioManifest>('audio-short/manifest.json')
  ])
  const cues = await Promise.all(narration.sections.map((entry) => fetchJson<CueData[]>(entry.cues)))

  let cursor = 0
  const segments = narration.sections.map((entry, index): ShortSegment => {
    const durationInFrames = msToFrames(entry.durationMs + PAD_MS)
    const { card, parts } = planFor(entry.id, timeline, durationInFrames)
    const segment: ShortSegment = {
      id: entry.id,
      title: entry.title,
      audio: entry.audio,
      cues: cues[index]!,
      fromFrame: cursor,
      durationInFrames,
      card,
      parts
    }
    cursor += durationInFrames
    return segment
  })

  if (cursor > SHORT_MAX_FRAMES) {
    throw new Error(
      `short cut is ${(cursor / FPS).toFixed(1)}s, over the 60s ceiling - shorten script/narration-short.json`
    )
  }
  return { fps: FPS, width: WIDTH, height: HEIGHT, segments, totalFrames: cursor }
}

/** Inside a part's `<Sequence>` the frame is part-relative, so the source time is offset by `startMs`. */
const ShortPart = ({ part: shortPart }: { part: ShortPart }) => {
  const frame = useCurrentFrame()
  return (
    <ClipPlayer
      clip={shortPart.clip}
      clipMs={shortPart.startMs + (frame / FPS) * 1000}
      trimBefore={msToFrames(shortPart.startMs)}
    />
  )
}

const ShortSegmentScene = ({ segment }: { segment: ShortSegment }) => {
  const frame = useCurrentFrame()
  const nowMs = (frame / FPS) * 1000
  return (
    <AbsoluteFill style={{ background: BACKDROP }}>
      {segment.card === 'intro' ? <Intro /> : null}
      {segment.card === 'outro' ? <Outro /> : null}
      {segment.parts.map((shortPart) => (
        <Sequence
          key={`${shortPart.clip.file}-${shortPart.fromFrame}`}
          from={shortPart.fromFrame}
          durationInFrames={shortPart.durationInFrames}
        >
          <ShortPart part={shortPart} />
        </Sequence>
      ))}
      <Audio src={staticFile(segment.audio)} />
      <Captions cues={segment.cues} nowMs={nowMs} />
    </AbsoluteFill>
  )
}

export const Short60 = ({ short }: { short: ShortData }) => (
  <AbsoluteFill style={{ background: BACKDROP }}>
    {short.segments.map((segment) => (
      <Sequence
        key={segment.id}
        from={segment.fromFrame}
        durationInFrames={segment.durationInFrames}
        name={segment.title}
      >
        <ShortSegmentScene segment={segment} />
      </Sequence>
    ))}
  </AbsoluteFill>
)
