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

const sections = JSON.parse(await readFile(join(root, 'script/narration.json'), 'utf8'))
const audioDir = join(root, 'public/audio')
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

const manifest = { voice: VOICE, rate: RATE, sections: [] }
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
    manifest.sections.push({ id: section.id, title: section.title, durationMs, audio: `audio/${section.id}.mp3`, cues: `audio/${section.id}.cues.json` })
    console.log(`${section.id}: ${(durationMs / 1000).toFixed(1)}s, ${cues.length} cues`)
  } catch (err) {
    const stderr = err?.stderr ? `\n${err.stderr}` : ''
    throw new Error(`tts failed for section "${section.id}": ${err.message}${stderr}`)
  }
}
await writeFile(join(audioDir, 'manifest.json'), JSON.stringify(manifest, null, 2))
console.log('total', (manifest.sections.reduce((n, s) => n + s.durationMs, 0) / 1000).toFixed(1), 's')
