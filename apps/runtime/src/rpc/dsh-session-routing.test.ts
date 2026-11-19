import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { zstdCompressSync } from 'node:zlib'
import { expect, it } from 'vitest'
import { RuntimeRpcRouter } from './runtime-rpc-router'
import { RuntimeDatabase } from '../storage/database'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'
import { encodeDshProjectPath } from '../session/dsh-session-catalog'

it('routes DSH catalog reads and loads the exact native identity into the existing restore flow', async () => {
  const root = await mkdtemp(join(tmpdir(), 'matou-dsh-rpc-')), cwd = join(root, 'workspace')
  await mkdir(cwd)
  const nativeRoot = join(root, 'native-sessions'), id = 'native-exact'
  const folder = join(nativeRoot, encodeDshProjectPath(cwd), id)
  await mkdir(folder, { recursive: true })
  await writeFile(join(folder, 'session.v4.jsonl.zstd'), Buffer.concat([
    { type: 'session', version: 4, id, cwd, createdAt: 1000 },
    { type: 'session/title', seq: 0, data: { title: 'Native catalog title' } },
    { type: 'request/header', seq: 1, data: { header: { config: { model: 'native-model' } } } },
    { type: 'user/message', seq: 2, surfaceOp: 'append', data: { content: [{ type: 'text', text: 'find-this-marker' }] } }
  ].map(row => zstdCompressSync(Buffer.from(JSON.stringify(row) + '\n')))))
  const db = RuntimeDatabase.open(join(root, 'test.sqlite'))
  try {
    await new MigrationRunner(db, FOUNDATION_MIGRATIONS).migrate()
    const router = new RuntimeRpcRouter(db, undefined, { dshSessionsRoot: nativeRoot })
    const command = (key: string, input: unknown) => ({ command: { commandId: key, commandType: 'test', requestHash: key }, input })
    const initial = await router.handle('hierarchy.bootstrap-window', command('bootstrap', {
      windowId: 'window-dsh', defaultRootDirectory: cwd, defaultName: 'DSH', now: 1
    })) as { session: { id: string } }
    const target = { sessionId: initial.session.id, provider: 'dsh', providerSessionId: id }
    expect(await router.handle('claude-sessions.list', target)).toMatchObject({ provider: 'dsh', sessions: [{ providerSessionId: id, title: 'Native catalog title' }] })
    expect(await router.handle('claude-sessions.detail', target)).toMatchObject({ provider: 'dsh', events: [{ text: 'find-this-marker' }] })
    expect(await router.handle('claude-sessions.search', { ...target, query: 'find-this-marker' })).toMatchObject({ total: 1 })
    expect(await router.handle('claude-sessions.load', command('load-dsh', { ...target, now: 2 }))).toMatchObject({ load: { provider: 'dsh', providerSessionId: id } })
    expect(db.get('SELECT kind, title, work_status FROM sessions WHERE id = ?', initial.session.id)).toEqual({ kind: 'dsh', title: 'Native catalog title', work_status: 'starting' })
    expect(db.get('SELECT provider, provider_session_id, restore_state FROM provider_bindings WHERE session_id = ?', initial.session.id)).toEqual({ provider: 'dsh', provider_session_id: id, restore_state: 'restoring' })
  } finally { db.close(); await rm(root, { recursive: true, force: true }) }
})
