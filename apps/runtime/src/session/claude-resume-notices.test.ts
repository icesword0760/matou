import { randomUUID } from 'node:crypto'
import * as fs from 'node:fs/promises'
import { appendFile, chmod, mkdir, mkdtemp, readFile, realpath, rm, symlink, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { ClaudeResumeNoticeStorageError, quietClaudeResumeNotices } from './claude-resume-notices'
import { encodeClaudeProjectPath } from './claude-session-catalog'
import { PtySession } from './pty-session'

let root: string
let path: string
let sessionId: string
let env: NodeJS.ProcessEnv
beforeEach(async () => {
  root = await realpath(await mkdtemp(join(tmpdir(), 'matou-quiet-notices-')))
  sessionId = randomUUID()
  env = { HOME: root, CLAUDE_CONFIG_DIR: join(root, 'config') }
  const project = join(env.CLAUDE_CONFIG_DIR!, 'projects', encodeClaudeProjectPath(root))
  await mkdir(project, { recursive: true })
  path = join(project, `${sessionId}.jsonl`)
})
afterEach(async () => { vi.restoreAllMocks(); vi.unstubAllEnvs(); await rm(root, { recursive: true, force: true }) })

function notice(overrides: Record<string, unknown> = {}) {
  return { type: 'system', subtype: 'informational', uuid: randomUUID(), parentUuid: randomUUID(),
    sessionId, cwd: root, timestamp: new Date().toISOString(), isMeta: false, level: 'notice',
    content: `agents-md: no CLAUDE.md found; AGENTS.md loaded: ${root}/AGENTS.md`, ...overrides }
}
function run() { return quietClaudeResumeNotices({ cwd: root, providerSessionId: sessionId, env }) }
function jsonl(rows: unknown[]) { return rows.map(row => JSON.stringify(row)).join('\n') + '\n' }

describe('Claude resume notice display metadata', () => {
  it('preserves every original byte and full message while moving old load receipts to transcript-only info', async () => {
    const notices = Array.from({ length: 10 }, () => notice())
    const rows = [
      { type: 'user', uuid: randomUUID(), message: { role: 'user', content: 'Keep my prompt' } },
      ...notices,
      { type: 'assistant', uuid: randomUUID(), message: { role: 'assistant', content: [{ type: 'tool_use', name: 'Bash', input: { command: 'ls' } }] } },
      { type: 'user', uuid: randomUUID(), message: { role: 'user', content: [{ type: 'tool_result', content: 'Keep my tool output' }] } }
    ]
    const original = jsonl(rows)
    await writeFile(path, original)
    expect(await run()).toEqual({ quieted: 10 })
    const actual = await readFile(path, 'utf8')
    expect(actual.slice(0, Buffer.byteLength(original))).toEqual(original)
    expect(actual.slice(original.length).trim().split('\n').map(line => JSON.parse(line)))
      .toEqual(notices.map(row => ({ ...row, level: 'info' })))
    // A second resume with no new receipt does not grow the log again.
    expect(await run()).toEqual({ quieted: 0 })
    expect(await readFile(path, 'utf8')).toBe(actual)
    const fresh = notice()
    await appendFile(path, jsonl([fresh]))
    expect(await run()).toEqual({ quieted: 1 })
    expect((await readFile(path, 'utf8')).startsWith(actual + jsonl([fresh]))).toBe(true)
  })

  it('leaves warnings, other plugins, quotes, user text and other session identities alone', async () => {
    const original = jsonl([
      notice({ level: 'warning' }), notice({ level: 'info' }), notice({ isMeta: true }),
      notice({ content: 'agents-md: failed to load AGENTS.md' }),
      notice({ content: 'other-plugin: no CLAUDE.md found; AGENTS.md loaded: /test/AGENTS.md' }),
      notice({ type: 'assistant' }), notice({ type: 'user' }),
      notice({ sessionId: randomUUID() }), notice({ uuid: 'unknown' }),
      notice({ subtype: 'compact_boundary' }),
      notice({ content: `Here is a quote: agents-md: no CLAUDE.md found; AGENTS.md loaded: ${root}/AGENTS.md` })
    ])
    await writeFile(path, original)
    expect(await run()).toEqual({ quieted: 0 })
    expect(await readFile(path, 'utf8')).toBe(original)
  })

  it('honors the last UUID revision and preserves all UUIDs, parent links and timestamps', async () => {
    const first = notice()
    const second = notice()
    await writeFile(path, jsonl([first, second, { ...first, level: 'info' }]))
    expect(await run()).toEqual({ quieted: 1 })
    const rows = (await readFile(path, 'utf8')).trim().split('\n').map(line => JSON.parse(line))
    expect(rows.at(-1)).toEqual({ ...second, level: 'info' })
  })

  it.each(['partial', 'malformed'])('skips %s logs without repairing or truncating history', async kind => {
    const original = jsonl([notice()]) + (kind === 'partial' ? '{"type":' : 'not json\n')
    await writeFile(path, original)
    expect(await run()).toEqual({ quieted: 0, skipped: 'incomplete' })
    expect(await readFile(path, 'utf8')).toBe(original)
  })

  it('does not create missing transcripts or follow symlinks', async () => {
    expect(await run()).toEqual({ quieted: 0 })
    const other = join(root, 'other.jsonl')
    const original = jsonl([notice()])
    await writeFile(other, original)
    await symlink(other, path)
    await expect(run()).rejects.toMatchObject({ code: 'ELOOP' })
    expect(await readFile(other, 'utf8')).toBe(original)
  })

  it('uses only the selected custom config and supports canonical workspace paths', async () => {
    await writeFile(path, jsonl([notice()]))
    const alias = join(root, 'alias')
    await symlink(root, alias)
    expect(await quietClaudeResumeNotices({ cwd: alias, providerSessionId: sessionId, env }))
      .toEqual({ quieted: 1 })
    const otherEnv = { HOME: root, CLAUDE_CONFIG_DIR: join(root, 'different') }
    expect(await quietClaudeResumeNotices({ cwd: root, providerSessionId: sessionId, env: otherEnv }))
      .toEqual({ quieted: 0 })
  })

  it('does not interpret resume paths or arbitrary IDs as native UUIDs', async () => {
    expect(await quietClaudeResumeNotices({ cwd: root, providerSessionId: '../../other', env }))
      .toEqual({ quieted: 0, skipped: 'unsupported' })
  })

  it('halts on a storage write failure and never truncates the original log', async () => {
    const original = jsonl([notice()])
    await writeFile(path, original)
    const probe = await fs.open(path, 'r')
    const prototype = Object.getPrototypeOf(probe)
    await probe.close()
    vi.spyOn(prototype, 'write').mockRejectedValueOnce(Object.assign(new Error('Disk full'), { code: 'ENOSPC' }))
    await expect(run()).rejects.toBeInstanceOf(ClaudeResumeNoticeStorageError)
    expect(await readFile(path, 'utf8')).toBe(original)
  })

  it.each([false, true])('prepares before the real PTY launch, while fork=%s leaves the source alone', async forkSession => {
    const original = jsonl([notice()])
    await writeFile(path, original)
    const executable = join(root, 'provider-fixture')
    await writeFile(executable, `#!/usr/bin/env node\nconst fs = require('node:fs'); const rows = fs.readFileSync(${JSON.stringify(path)}, 'utf8').trim().split('\\n').map(JSON.parse); console.log('NATIVE_LEVEL='+rows.at(-1).level); setInterval(()=>{}, 1000)\n`)
    await chmod(executable, 0o755)
    vi.stubEnv('MATOU_CLAUDE_COMMAND', executable)
    let output = ''
    const session = await PtySession.create({
      sessionId: randomUUID(), executionContextId: 'local-default', cwd: root, dataRoot: join(root, 'data'),
      cols: 80, rows: 24, profile: 'claude-code', providerSessionId: sessionId, forkSession,
      env: { HOME: env.HOME!, CLAUDE_CONFIG_DIR: env.CLAUDE_CONFIG_DIR!, MATOU_CLAUDE_PROJECTS_ROOT: '' },
      onOutput: data => { output += data }
    })
    try {
      await expect.poll(() => output, { timeout: 10_000 }).toContain(`NATIVE_LEVEL=${forkSession ? 'notice' : 'info'}`)
      const actual = await readFile(path, 'utf8')
      expect(actual.startsWith(original)).toBe(true)
      if (forkSession) expect(actual).toBe(original)
    } finally { await session.shutdownForRuntime({ gracePeriodMs: 100, hardKillWaitMs: 1000 }) }
  })
})
