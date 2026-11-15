import { mkdtemp, writeFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { expect, it } from 'vitest'
import { latestCodexAutoTitle } from './codex-session-title'

it('matches exact UUID, selects latest timestamp, and ignores malformed/missing index data', async () => {
  const root = await mkdtemp(join(tmpdir(), 'matou-codex-title-'))
  const path = join(root, 'session_index.jsonl')
  const id = '01a1ddd6-4064-70e1-a006-79d310600776'
  try {
    expect(await latestCodexAutoTitle(path, id)).toBeUndefined()
    await writeFile(path, [
      JSON.stringify({ id, thread_name: 'Latest title', updated_at: '2026-11-15T06:14:00Z' }),
      JSON.stringify({ id, thread_name: 'Older title', updated_at: '2026-11-15T06:13:00Z' }),
      JSON.stringify({ id: id + '-suffix', thread_name: 'Wrong UUID', updated_at: '2026-11-15T06:15:00Z' }),
      JSON.stringify({ id, thread_name: 'Invalid date', updated_at: 'bad' }),
      '{bad json', 'null', JSON.stringify({ id, thread_name: ' ', updated_at: '2026-11-15T06:16:00Z' })
    ].join('\n'))
    expect(await latestCodexAutoTitle(path, id)).toBe('Latest title')
    expect(await latestCodexAutoTitle(path, 'not-a-uuid')).toBeUndefined()
  } finally { await rm(root, { recursive: true, force: true }) }
})
