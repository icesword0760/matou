import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { zstdCompressSync } from 'node:zlib'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { runtimeMessages } from '../i18n/messages'
import { DshSessionCatalog, encodeDshProjectPath } from './dsh-session-catalog'

let root: string, cwd: string, catalog: DshSessionCatalog
beforeEach(async () => { root = await mkdtemp(join(tmpdir(), 'matou-dsh-catalog-')); cwd = join(root, '工作区😀'); await mkdir(cwd); catalog = new DshSessionCatalog(join(root, 'sessions')) })
afterEach(async () => { await rm(root, { recursive: true, force: true }) })
const header = (id: string, directory: string) => ({ type: 'session', version: 4, id, cwd: directory, createdAt: 1000, isSeeded: false })
const user = (seq: number, text: string) => ({ type: 'user/message', seq, time: 1001 + seq, surfaceOp: 'append', data: { role: 'user', content: [{ type: 'text', text }] } })
async function fixture(id: string, rows: unknown[], compress = true, directory = cwd): Promise<string> {
  const folder = join(root, 'sessions', encodeDshProjectPath(cwd), id)
  await mkdir(folder, { recursive: true })
  const path = join(folder, 'session.v4.jsonl' + (compress ? '.zstd' : ''))
  await writeFile(path, compress ? Buffer.concat([header(id, directory), ...rows].map(row => zstdCompressSync(Buffer.from(JSON.stringify(row) + '\n')))) : [header(id, directory), ...rows].map(row => JSON.stringify(row)).join('\n') + '\n')
  return path
}

describe('DSH native session catalog', () => {
  it('reads every independent frame, follows the latest native title, and paginates visible content/search', async () => {
    await fixture('native-session', [user(0, 'first needle'), { type: 'session/title', seq: 1, time: 2, data: { title: 'Native title' } },
      { type: 'request/header', seq: 2, time: 3, data: { header: { config: { model: 'native-model' } } } },
      user(3, 'middle'), user(4, 'last needle')])
    const list = await catalog.list({ cwd, query: '' })
    expect(list.sessions).toMatchObject([{ provider: 'dsh', providerSessionId: 'native-session', title: 'Native title', eventCount: 3, model: 'native-model' }])
    const tail = await catalog.detail({ cwd, providerSessionId: 'native-session', query: '', limit: 1 })
    expect(tail.events.map(row => row.text)).toEqual(['last needle'])
    expect(tail.page).toMatchObject({ startEventIndex: 3, hasEarlier: true })
    const earlier = await catalog.detail({ cwd, providerSessionId: 'native-session', query: '', limit: 2, beforeEventIndex: 3 })
    expect(earlier.events.map(row => row.text)).toEqual(['first needle', 'middle'])
    expect(await catalog.search({ cwd, providerSessionId: 'native-session', query: 'needle', offset: 1, limit: 1 })).toMatchObject({ total: 2, hits: [{ eventIndex: 3 }], hasMore: false })
  })
  it('validates header cwd before reading a colliding foreign body and never permits path traversal', async () => {
    await fixture('foreign', [{ type: 'future/required', seq: 0, data: {} }], true, join(root, 'elsewhere'))
    await fixture('own', [user(0, 'own')])
    expect((await catalog.list({ cwd, query: '' })).sessions.map(row => row.providerSessionId)).toEqual(['own'])
    await expect(catalog.detail({ cwd, providerSessionId: 'foreign', query: '' })).rejects.toThrow(runtimeMessages().session.dshSessionNotInWorkspace)
    await expect(catalog.detail({ cwd, providerSessionId: '../escape', query: '' })).rejects.toThrow(runtimeMessages().session.dshSessionIdInvalid)
  })
  it('uses the current surface after compaction, excluding replaced text from preview and search', async () => {
    await fixture('compact', [user(0, 'retired-secret'), user(1, 'older-message'), {
      type: 'user/message', seq: 2, time: 1003, surfaceOp: { op: 'replace', startSeq: 0, endSeq: 1 }, sourceEventSeqs: [0, 1],
      data: { role: 'user', content: [{ type: 'text', text: 'current summary' }] }
    }, user(3, 'continuation')])
    expect((await catalog.detail({ cwd, providerSessionId: 'compact', query: '' })).events.map(row => row.text)).toEqual(['current summary', 'continuation'])
    expect((await catalog.search({ cwd, providerSessionId: 'compact', query: 'retired-secret' })).total).toBe(0)
  })
  it('refreshes a grown archive, tolerates only an incomplete final frame and never changes the file', async () => {
    const path = await fixture('live', [user(0, 'first')])
    const initial = await readFile(path)
    await writeFile(path, Buffer.concat([initial, zstdCompressSync(Buffer.from(JSON.stringify(user(1, 'last')) + '\n')).subarray(0, 8)]))
    const before = await readFile(path)
    expect((await catalog.detail({ cwd, providerSessionId: 'live', query: '' })).eventCount).toBe(1)
    expect(await readFile(path)).toEqual(before)
    await writeFile(path, Buffer.concat([initial, zstdCompressSync(Buffer.from(JSON.stringify(user(1, 'last')) + '\n'))]))
    expect((await catalog.detail({ cwd, providerSessionId: 'live', query: '' })).eventCount).toBe(2)
  })
  it('rejects complete frame corruption and unknown required semantics, allowing explicitly ignorable records', async () => {
    const path = await fixture('damaged', [user(0, 'record')])
    const raw = await readFile(path); raw[0] = 0
    await writeFile(path, raw)
    await expect(catalog.detail({ cwd, providerSessionId: 'damaged', query: '' })).rejects.toThrow(runtimeMessages().session.dshSessionCorrupt)
    await fixture('future', [{ type: 'future/required', seq: 0, data: {} }])
    await expect(catalog.detail({ cwd, providerSessionId: 'future', query: '' })).rejects.toThrow(runtimeMessages().session.dshSessionUnsupportedEvent)
    await fixture('optional', [{ type: 'future/metric', seq: 0, data: {}, ignorable: true }, user(1, 'visible')])
    expect((await catalog.detail({ cwd, providerSessionId: 'optional', query: '' })).eventCount).toBe(1)
  })
  it('supports native uncompressed v4 and gives untitled sessions a time/ID name', async () => {
    await fixture('untitled', [user(0, 'plain text')], false)
    expect((await catalog.list({ cwd, query: '' })).sessions[0]?.title).toContain('1970-01-01')
    expect((await catalog.detail({ cwd, providerSessionId: 'untitled', query: '' })).events[0]?.text).toBe('plain text')
    await expect(catalog.detail({ cwd, providerSessionId: 'missing', query: '' })).rejects.toThrow(runtimeMessages().session.dshSessionNotInWorkspace)
    expect((await new DshSessionCatalog(join(root, 'missing')).list({ cwd, query: '' })).sessions).toEqual([])
  })
  it('bounds decoded frame size and line size even for small highly compressed archives', async () => {
    const path = await fixture('oversized', [])
    const initial = await readFile(path)
    await writeFile(path, Buffer.concat([initial, zstdCompressSync(Buffer.alloc(33 * 1024 * 1024, 32))]))
    await expect(catalog.detail({ cwd, providerSessionId: 'oversized', query: '' })).rejects.toThrow()
    const plain = await fixture('long-line', [], false)
    await writeFile(plain, Buffer.concat([await readFile(plain), Buffer.alloc(17 * 1024 * 1024, 32), Buffer.from('\n')]))
    await expect(catalog.detail({ cwd, providerSessionId: 'long-line', query: '' })).rejects.toThrow(runtimeMessages().session.dshSessionReadLimit)
  })

})
