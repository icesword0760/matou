// Checks that the recorded UI actions still land on the words that describe them.
//
//   npm run qc:sync          (plain `node qc/check-sync.mjs` cannot import the .ts below)
//
// For a handful of load-bearing moments it prints the absolute time of the recorded event and the
// absolute time the narration reaches the phrase that introduces it. `delta` is event minus
// phrase: an action should happen while its phrase is being spoken or just after, never before -
// so 0 to 1.2 s. Exits non-zero when a delta falls outside that window, so this can gate a render
// rather than being something to eyeball.
//
// The phrase clock is `phraseTime` from the recorder's own cue helper, not a copy of it: the whole
// point of the check is to measure against the same clock `record.spec.ts` scheduled the action
// on. Several cues carry four ideas in fourteen seconds, so a cue's start time is not where its
// later phrases are spoken.
import { readFile } from 'node:fs/promises'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { phraseTime } from '../../../tests/e2e/launch-video/cues.ts'

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const read = async (path) => JSON.parse(await readFile(resolve(ROOT, path), 'utf8'))

/** Silent tail appended after every section's narration - must match `TAIL_MS` in src/manifest.ts. */
const TAIL_MS = 800
const MIN_DELTA_S = 0
const MAX_DELTA_S = 1.2

// [section, recorder event label or mark name, the phrase in the narration that this event is the
// picture of]. `mt-read` is the moment the stub Claude has finished reading its neighbour, so the
// phrase it illustrates is the request being quoted, not the "you can just say" that leads into it.
const CHECKS = [
  ['fork-dag', 'fork', '点一下 Fork'],
  ['fork-dag', 'dag', '按 Option 加 Tab'],
  ['ai-control', 'mt-read', '看看左边那张卡片'],
  ['board-notify', 'board', '每个工作空间都有一个看板']
]

const audio = await read('public/audio/manifest.json')
const recordings = await read('public/recordings/manifest.json')

let failed = 0
let offset = 0
for (const section of audio.sections) {
  const cues = await read(`public/${section.cues}`)
  for (const [, label, phrase] of CHECKS.filter((check) => check[0] === section.id)) {
    const clips = recordings.sections.find((s) => s.id === section.id)?.clips ?? []
    let spokenAt
    try {
      spokenAt = offset + phraseTime(cues, phrase)
    } catch {
      console.log(`${section.id.padEnd(12)} ${label.padEnd(8)} NO CUE contains "${phrase}"`)
      failed += 1
      continue
    }
    let found = false
    for (const clip of clips) {
      const { events } = await read(`public/${clip.events}`)
      const event = events.find((e) => (e.label ?? e.name) === label)
      if (!event) continue
      found = true
      const eventAt = offset + clip.startAtMs + event.t
      const delta = (eventAt - spokenAt) / 1000
      const ok = delta >= MIN_DELTA_S && delta <= MAX_DELTA_S
      if (!ok) failed += 1
      console.log(
        `${section.id.padEnd(12)} ${label.padEnd(10)} event ${(eventAt / 1000).toFixed(1).padStart(6)}s` +
          `  phrase ${(spokenAt / 1000).toFixed(1).padStart(6)}s  delta ${delta.toFixed(2).padStart(6)}s` +
          `  ${ok ? 'ok' : 'OUT OF RANGE'}  "${phrase}"`
      )
    }
    if (!found) {
      console.log(`${section.id.padEnd(12)} ${label.padEnd(8)} NO EVENT with that label or mark`)
      failed += 1
    }
  }
  offset += section.durationMs + TAIL_MS
}

console.log(failed === 0
  ? `all checks within ${MIN_DELTA_S} to ${MAX_DELTA_S}s`
  : `${failed} check(s) outside ${MIN_DELTA_S} to ${MAX_DELTA_S}s`)
process.exit(failed === 0 ? 0 : 1)
