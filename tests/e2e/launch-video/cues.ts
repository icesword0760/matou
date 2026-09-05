// Narration cue lookup for the launch-video recorder. The subtitle cues produced by
// `marketing/launch-video/tts/synthesize.mjs` are the clock every recorded UI action runs on:
// `record.spec.ts` never sleeps a made-up number of milliseconds, it waits until the voice-over
// reaches the phrase that the action illustrates.
import { readFile } from 'node:fs/promises'
import { join, resolve } from 'node:path'

const AUDIO = resolve(import.meta.dirname, '../../../marketing/launch-video/public/audio')
export interface Cue { startMs: number; endMs: number; text: string }

export async function loadCues(sectionId: string): Promise<{ cues: Cue[]; durationMs: number }> {
  const manifest = JSON.parse(await readFile(join(AUDIO, 'manifest.json'), 'utf8'))
  const section = manifest.sections.find((s: { id: string }) => s.id === sectionId)
  if (!section) throw new Error(`no narration for section ${sectionId}; run npm run tts first`)
  const cues = JSON.parse(await readFile(join(AUDIO, `${sectionId}.cues.json`), 'utf8')) as Cue[]
  return { cues, durationMs: section.durationMs }
}

// Time (ms) when the narration reaches the first cue containing `keyword`.
export function cueTime(cues: Cue[], keyword: string, offsetMs = 0): number {
  const cue = cues.find((c) => c.text.replace(/\s/g, '').includes(keyword.replace(/\s/g, '')))
  if (!cue) throw new Error(`keyword "${keyword}" not found in cues: ${cues.map((c) => c.text).join(' | ')}`)
  return Math.max(0, cue.startMs + offsetMs)
}

// Time (ms) when the narration reaches `keyword` *inside* its cue. A cue is one spoken sentence
// group and several of this video's cues carry four different ideas in fourteen seconds (the whole
// four-layer structure lives in a single cue), so `cueTime` would fire every action of that cue at
// the same instant. Speech inside one cue runs at a near-constant rate, so interpolating by the
// phrase's character offset lands each action on the words that describe it.
export function phraseTime(cues: Cue[], keyword: string, offsetMs = 0): number {
  const needle = keyword.replace(/\s/g, '')
  for (const cue of cues) {
    const text = cue.text.replace(/\s/g, '')
    const index = text.indexOf(needle)
    if (index < 0) continue
    const ratio = text.length === 0 ? 0 : index / text.length
    return Math.max(0, Math.round(cue.startMs + (cue.endMs - cue.startMs) * ratio) + offsetMs)
  }
  throw new Error(`phrase "${keyword}" not found in cues: ${cues.map((c) => c.text).join(' | ')}`)
}
