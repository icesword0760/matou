import { appendFile, chmod, mkdir, mkdtemp, readFile, rm, symlink, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { CodexSessionCatalog } from './codex-session-catalog'

let root: string; let cwd: string; let directory: string; let catalog: CodexSessionCatalog
const id = (n: number) => `01a1ddd6-4064-70e1-a006-${String(n).padStart(12, '0')}`
const row = (type: string, payload: unknown) => ({ type, payload })
const item = (type: string, content: unknown, extra = {}) => row('event_msg', { type: 'item_completed', turn_id: 't', item: { type, content, ...extra } })
const normal = [row('turn_context', { turn_id: 't', model: 'gpt-6-astra' }),
  item('UserMessage', [{ type: 'text', text: 'hello needle' }], { id: 'u' }),
  item('AgentMessage', [{ type: 'Text', text: 'answer needle' }], { id: 'a', phase: 'final_answer' }),
  row('event_msg', { type: 'task_complete', turn_id: 't' })]
async function transcript(n: number, records: unknown[] = normal, workspace = cwd, filename?: string, metadata: Record<string, unknown> = {}) {
  const path = join(directory, filename ?? `rollout-2026-11-15T00-00-00-${id(n)}.jsonl`)
  await writeFile(path, [row('session_meta', { id: id(n), cwd: workspace, ...metadata }), ...records].map(v => JSON.stringify(v)).join('\n') + '\n')
  return path
}
beforeEach(async () => {
  root = await mkdtemp(join(tmpdir(), 'matou-codex-catalog-')); cwd = join(root, 'workspace')
  directory = join(root, 'sessions', '2026', '11', '15'); await mkdir(directory, { recursive: true }); await mkdir(cwd)
  catalog = new CodexSessionCatalog(root)
})
afterEach(async () => { await rm(root, { recursive: true, force: true }) })

describe('CodexSessionCatalog', () => {
  it('discovers only children created after this run with the exact parent UUID and canonical cwd', async () => {
    const startedAt = Date.parse('2026-11-15T10:00:00Z')
    const candidate = async (n: number, extra: Record<string, unknown> = {}) => {
      await writeFile(join(directory, `rollout-${n}.jsonl`), JSON.stringify(row('session_meta', {
        id: id(n), cwd, timestamp: '2026-11-15T10:00:01Z', forked_from_id: id(1), ...extra
      })) + '\n')
    }
    await candidate(2)
    await candidate(3, { cwd: join(cwd, 'nested') })
    await candidate(4, { forked_from_id: id(9) })
    await candidate(5, { timestamp: '2026-11-15T09:59:59Z' })
    await candidate(6, { timestamp: undefined })
    expect((await catalog.forkCandidates({ cwd, parentProviderSessionId: id(1), startedAt })).map(c => c.providerSessionId)).toEqual([id(2)])
    await candidate(7)
    expect(await catalog.forkCandidates({ cwd, parentProviderSessionId: id(1), startedAt })).toHaveLength(2)
  })
  it('filters canonical exact cwd, trusts metadata UUID, and uses the latest index title', async () => {
    const alias = join(root, 'alias'); await symlink(cwd, alias)
    await transcript(1, normal, alias, 'rollout-misleading-name.jsonl')
    await transcript(2, normal, join(cwd, 'nested'))
    await transcript(3, normal, join(root, 'elsewhere'))
    await writeFile(join(root, 'session_index.jsonl'), [
      { id: id(1), thread_name: 'new title', updated_at: '2026-11-15T09:00:00Z' },
      { id: id(1), thread_name: 'old title', updated_at: '2026-11-15T08:00:00Z' },
      { id: id(1) + 'suffix', thread_name: 'wrong', updated_at: '2026-11-15T10:00:00Z' }
    ].map(v => JSON.stringify(v)).join('\n'))
    const list = await catalog.list({ cwd, query: '' })
    expect(list.sessions.map(v => [v.providerSessionId, v.title])).toEqual([[id(1), 'new title']])
    expect((await catalog.detail({ cwd, query: '', providerSessionId: id(1) })).model).toBe('gpt-6-astra')
    await expect(catalog.detail({ cwd: join(root, 'elsewhere'), query: '', providerSessionId: id(1) })).rejects.toThrow(/工作目录/)
    await expect(catalog.detail({ cwd, query: '', providerSessionId: '../../etc/passwd' })).rejects.toThrow(/UUID/)
  })
  it('paginates lists without parsing bodies; unnamed sessions remain distinguishable by time and UUID', async () => {
    for (let n = 1; n <= 4; n++) await transcript(n, [])
    const path = await transcript(5, [])
    await appendFile(path, 'malformed body\nnot final\n')
    const page = await catalog.list({ cwd, query: '', offset: 1, limit: 2 })
    expect(page).toMatchObject({ provider: 'codex', total: 5, offset: 1, limit: 2, nextOffset: 3, hasMore: true })
    expect(new Set(page.sessions.map(v => v.title)).size).toBe(2)
    expect(page.sessions[0]?.title).toMatch(/2026-.* · 01a1ddd6/)
    await expect(catalog.detail({ cwd, query: '', providerSessionId: id(5) })).rejects.toThrow(/损坏/)
  })
  it('paginates preview/search consistently, drops message mirrors and private instruction records', async () => {
    await transcript(1, [row('turn_context', { turn_id: 't' }),
      row('response_item', { type: 'message', role: 'developer', content: [{ text: 'hidden needle' }] }),
      row('response_item', { type: 'message', id: 'user-mirror', role: 'user', content: [{ text: 'hello needle' }] }),
      item('UserMessage', [{ text: 'hello needle' }], { id: 'u' }),
      item('AgentMessage', [{ text: 'answer needle' }], { id: 'a', phase: 'final_answer' }),
      row('response_item', { type: 'message', id: 'a', role: 'assistant', content: [{ text: 'answer needle' }] }),
      row('response_item', { type: 'custom_tool_call', name: 'exec', input: 'wrapper needle' }),
      row('event_msg', { type: 'item_completed', item: { type: 'CommandExecution', id: 'e', command: ['echo', 'needle'], aggregated_output: 'needle' } })])
    const last = await catalog.detail({ cwd, providerSessionId: id(1), query: '', limit: 2 })
    expect(last.events.map(v => v.index)).toEqual([2, 3])
    expect(last.page).toMatchObject({ total: 3, hasEarlier: true, hasLater: false })
    const first = await catalog.detail({ cwd, providerSessionId: id(1), query: '', beforeEventIndex: 2, limit: 2 })
    expect(first.events.map(v => v.index)).toEqual([1])
    const search = await catalog.search({ cwd, providerSessionId: id(1), query: 'needle', offset: 1, limit: 1 })
    expect(search).toMatchObject({ total: 3, offset: 1, nextOffset: 2, hasMore: true })
    const around = await catalog.detail({ cwd, providerSessionId: id(1), query: 'needle', aroundEventIndex: search.hits[0]!.eventIndex, limit: 1 })
    expect(around.events).toMatchObject([{ index: 2, text: 'answer needle', matched: true }])
  })
  it('requires a real user, final answer and normal completion in the same nonfailed turn', async () => {
    await transcript(1); await transcript(2, normal.slice(0, -1)); await transcript(3, [normal[0], normal[2], normal[3]])
    await transcript(4, [...normal.slice(0, -1), row('event_msg', { type: 'turn_aborted', turn_id: 't' }), normal.at(-1)])
    for (const n of [2, 3, 4]) expect(await catalog.hasCompletedTurn({ cwd, providerSessionId: id(n) })).toBe(false)
    expect(await catalog.hasCompletedTurn({ cwd, providerSessionId: id(1) })).toBe(true)
  })
  it('invalidates indexes and search after append and falls back to response messages when unmirrored', async () => {
    const path = await transcript(1, [row('response_item', { type: 'message', role: 'user', content: [{ text: 'fallback' }] })])
    expect((await catalog.search({ cwd, providerSessionId: id(1), query: 'new' })).total).toBe(0)
    await appendFile(path, JSON.stringify(row('response_item', { type: 'message', role: 'assistant', content: [{ text: 'new reply' }] })) + '\n')
    expect((await catalog.search({ cwd, providerSessionId: id(1), query: 'new' })).total).toBe(1)
  })
  it('merges one cross-Worktree ancestor into preview and full-text listing/search with stable indexes', async () => {
    const at = (text: string, time: number, messageId: string) => ({ ...item('AgentMessage', [{ text }], { id: messageId }), timestamp: time })
    await transcript(1, [at('ancestor needle', 10, 'a'), at('shared', 20, 's')], join(root, 'parent-worktree'))
    await transcript(2, [at('shared', 20, 's'), at('child', 30, 'c')], cwd, undefined, { history_base: { thread_id: id(1) } })
    const detail = await catalog.detail({ cwd, providerSessionId: id(2), query: '' })
    expect(detail.events.map(e => [e.index, e.text])).toEqual([[1, 'ancestor needle'], [2, 'shared'], [3, 'child']])
    expect((await catalog.list({ cwd, query: 'needle' })).sessions.map(e => e.providerSessionId)).toEqual([id(2)])
    const search = await catalog.search({ cwd, providerSessionId: id(2), query: 'needle' })
    expect(search.hits).toMatchObject([{ eventIndex: 1, excerpt: 'ancestor needle' }])
    expect((await catalog.detail({ cwd, providerSessionId: id(2), query: 'needle', aroundEventIndex: 1, limit: 1 })).events)
      .toMatchObject([{ index: 1, text: 'ancestor needle', matched: true }])
  })

  it('follows multiple history_base levels and respects the recorded fork boundary', async () => {
    await transcript(1, [{ ...item('AgentMessage', [{ text: 'grandparent' }], { id: 'g' }), ordinal: 1, timestamp: 10 }])
    await transcript(2, [
      { ...item('AgentMessage', [{ text: 'parent at fork' }], { id: 'p' }), ordinal: 2, timestamp: 20 },
      { ...item('AgentMessage', [{ text: 'parent after fork private' }], { id: 'future' }), ordinal: 3, timestamp: 40 }
    ], cwd, undefined, { history_base: { thread_id: id(1) } })
    await transcript(3, [{ ...item('AgentMessage', [{ text: 'child' }], { id: 'c' }), timestamp: 30 }], cwd, undefined,
      { history_base: { thread_id: id(2), end_ordinal_exclusive: 3 } })
    expect((await catalog.detail({ cwd, providerSessionId: id(3), query: '' })).events.map(e => e.text))
      .toEqual(['grandparent', 'parent at fork', 'child'])
    expect((await catalog.search({ cwd, providerSessionId: id(3), query: 'private' })).total).toBe(0)
  })

  it('bounds ancestor depth at five and stops cycles without duplicating messages', async () => {
    for (let n = 1; n <= 8; n++) {
      await transcript(n, [{ ...item('AgentMessage', [{ text: `message-${n}` }], { id: `m-${n}` }), timestamp: n }], cwd, undefined,
        n === 1 ? {} : { history_base: { thread_id: id(n - 1) } })
    }
    const limited = await catalog.detail({ cwd, providerSessionId: id(8), query: '' })
    expect(limited.events.filter(e => e.kind === 'assistant').map(e => e.text)).toEqual(['message-3', 'message-4', 'message-5', 'message-6', 'message-7', 'message-8'])
    expect(limited.events.at(-1)).toMatchObject({ kind: 'system', text: expect.stringContaining('5') })
    await transcript(1, [item('AgentMessage', [{ text: 'first' }], { id: 'first' })], cwd, undefined, { history_base: { thread_id: id(2) } })
    await transcript(2, [item('AgentMessage', [{ text: 'second' }], { id: 'second' })], cwd, undefined, { history_base: { thread_id: id(1) } })
    const cycle = await catalog.detail({ cwd, providerSessionId: id(1), query: '' })
    expect(cycle.events.filter(e => e.kind === 'assistant').map(e => e.text).sort()).toEqual(['first', 'second'])
    expect(cycle.events.at(-1)).toMatchObject({ kind: 'system', text: expect.stringContaining('循环') })
  })

  it('falls back to the current file with a visible note when any ancestor is missing', async () => {
    await transcript(2, [item('AgentMessage', [{ text: 'partial ancestor' }])], cwd, undefined, { history_base: { thread_id: id(99) } })
    await transcript(3, [item('AgentMessage', [{ text: 'current' }])], cwd, undefined, { history_base: { thread_id: id(2) } })
    const detail = await catalog.detail({ cwd, providerSessionId: id(3), query: '' })
    expect(detail.events.map(e => e.kind)).toEqual(['assistant', 'system'])
    expect(detail.events[0]?.text).toBe('current')
    expect(detail.events[1]?.text).toContain('仅显示当前会话')
    expect((await catalog.search({ cwd, providerSessionId: id(3), query: 'partial' })).hits).toEqual([])
  })

  it('invalidates merged search when an ancestor changes and applies byte boundaries', async () => {
    const parent = await transcript(1, [item('AgentMessage', [{ text: 'original' }], { id: 'old' })])
    const size = (await readFile(parent)).length
    await transcript(2, [], cwd, undefined, { history_base: { thread_id: id(1) } })
    await transcript(3, [], cwd, undefined, { history_base: { thread_id: id(1), end_byte_offset: size } })
    expect((await catalog.search({ cwd, providerSessionId: id(2), query: 'appended' })).total).toBe(0)
    await appendFile(parent, JSON.stringify(item('AgentMessage', [{ text: 'appended' }], { id: 'new' })) + '\n')
    expect((await catalog.search({ cwd, providerSessionId: id(2), query: 'appended' })).total).toBe(1)
    expect((await catalog.search({ cwd, providerSessionId: id(3), query: 'appended' })).total).toBe(0)
  })

  it('returns readable corrupt and permission errors, with an absent archive treated as empty', async () => {
    expect((await new CodexSessionCatalog(join(root, 'absent')).list({ cwd, query: '' })).sessions).toEqual([])
    const path = await transcript(1)
    await chmod(path, 0)
    try { await expect(catalog.list({ cwd, query: '' })).rejects.toThrow(/权限/) }
    finally { await chmod(path, 0o600) }
    await writeFile(path, '{broken header\n')
    await expect(catalog.list({ cwd, query: '' })).rejects.toThrow(/损坏/)
  })
})
