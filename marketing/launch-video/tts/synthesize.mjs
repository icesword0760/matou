// marketing/launch-video/tts/synthesize.mjs
import { execFile } from 'node:child_process'
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { homedir } from 'node:os'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { promisify } from 'node:util'

const run = promisify(execFile)
const here = dirname(fileURLToPath(import.meta.url))
const root = join(here, '..')
const EDGE_TTS = process.env.EDGE_TTS ?? join(homedir(), '.local/bin/edge-tts')
const VOICE = 'zh-CN-YunxiNeural'
const RATE = '-8%'

// `--script script/narration-short.json --out public/audio-short` builds the 60s cut's narration;
// with no arguments this stays the main film's `script/narration.json` -> `public/audio`.
// `--only why[,structure]` re-synthesises just those sections and merges their entries back into
// the existing `manifest.json`, leaving every other entry (and the order) exactly as it was - that
// is what lets a single reworded paragraph be re-voiced and re-recorded without touching the rest.
const argv = process.argv.slice(2)
const arg = (name, fallback) => {
  const at = argv.indexOf(`--${name}`)
  if (at === -1) return fallback
  const value = argv[at + 1]
  if (!value || value.startsWith('--')) throw new Error(`--${name} needs a value`)
  return value
}
const scriptRel = arg('script', 'script/narration.json')
const outRel = arg('out', 'public/audio').replace(/\/+$/, '')
// Manifest paths are resolved through Remotion's `staticFile()`, so they are relative to `public/`.
const publicPrefix = outRel.replace(/^public\//, '')

const only = (arg('only', '') || '').split(',').map((id) => id.trim()).filter(Boolean)

const allSections = JSON.parse(await readFile(join(root, scriptRel), 'utf8'))
const missing = only.filter((id) => !allSections.some((section) => section.id === id))
if (missing.length > 0) throw new Error(`--only names sections that are not in ${scriptRel}: ${missing.join(', ')}`)
const sections = only.length > 0 ? allSections.filter((section) => only.includes(section.id)) : allSections
const audioDir = join(root, outRel)
await mkdir(audioDir, { recursive: true })

const parseVtt = (vtt) => {
  const cues = []
  const stamp = (s) => { const [h, m, rest] = s.trim().split(':'); const [sec, ms] = rest.split(/[.,]/); return ((+h * 60 + +m) * 60 + +sec) * 1000 + +ms }
  for (const block of vtt.split(/\n\s*\n/)) {
    const lines = block.trim().split('\n')
    const timing = lines.find((l) => l.includes('-->'))
    if (!timing) continue
    const [from, to] = timing.split('-->')
    const text = lines.slice(lines.indexOf(timing) + 1).join(' ').trim()
    if (text) cues.push({ startMs: stamp(from), endMs: stamp(to), text })
  }
  return cues
}

// With `--only` the previous manifest is the base: entries for sections we are not re-synthesising
// keep their durations and paths, and stay in the order the last full run wrote them.
const previous = only.length > 0
  ? await readFile(join(audioDir, 'manifest.json'), 'utf8').then((text) => JSON.parse(text)).catch(() => undefined)
  : undefined
const manifest = { voice: VOICE, rate: RATE, sections: previous?.sections ?? [] }
const record = (entry) => {
  const at = manifest.sections.findIndex((section) => section.id === entry.id)
  if (at === -1) manifest.sections.push(entry)
  else manifest.sections[at] = entry
}
for (const section of sections) {
  const txt = join(audioDir, `${section.id}.txt`)
  const mp3 = join(audioDir, `${section.id}.mp3`)
  const vtt = join(audioDir, `${section.id}.vtt`)
  try {
    await writeFile(txt, section.text)
    await run(EDGE_TTS, ['--voice', VOICE, `--rate=${RATE}`, '-f', txt, '--write-media', mp3, '--write-subtitles', vtt])
    const cues = parseVtt(await readFile(vtt, 'utf8'))
    await writeFile(join(audioDir, `${section.id}.cues.json`), JSON.stringify(cues, null, 2))
    const { stdout } = await run('ffprobe', ['-v', 'error', '-show_entries', 'format=duration', '-of', 'csv=p=0', mp3])
    const durationMs = Math.round(parseFloat(stdout) * 1000)
    record({ id: section.id, title: section.title, durationMs, audio: `${publicPrefix}/${section.id}.mp3`, cues: `${publicPrefix}/${section.id}.cues.json` })
    console.log(`${section.id}: ${(durationMs / 1000).toFixed(1)}s, ${cues.length} cues`)
  } catch (err) {
    const stderr = err?.stderr ? `\n${err.stderr}` : ''
    throw new Error(`tts failed for section "${section.id}": ${err.message}${stderr}`)
  }
}
await writeFile(join(audioDir, 'manifest.json'), JSON.stringify(manifest, null, 2))
console.log('total', (manifest.sections.reduce((n, s) => n + s.durationMs, 0) / 1000).toFixed(1), 's')
