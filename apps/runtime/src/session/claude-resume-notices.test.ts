import { randomUUID } from 'node:crypto'
import { chmod, mkdir, mkdtemp, readFile, realpath, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

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
function jsonl(rows: unknown[]) { return rows.map(row => JSON.stringify(row)).join('\n') + '\n' }

describe('Claude resume preserves native transcripts', () => {
  it.each([false, true])('leaves native transcript bytes untouched on PTY launch (fork=%s)', async forkSession => {
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
      await expect.poll(() => output, { timeout: 10_000 }).toContain('NATIVE_LEVEL=notice')
      const actual = await readFile(path, 'utf8')
      expect(actual).toBe(original)
    } finally { await session.shutdownForRuntime({ gracePeriodMs: 100, hardKillWaitMs: 1000 }) }
  })
})
