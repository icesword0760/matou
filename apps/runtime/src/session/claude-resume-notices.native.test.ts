import { randomUUID } from 'node:crypto'
import { mkdir, mkdtemp, readFile, realpath, writeFile } from 'node:fs/promises'
import { createServer } from 'node:http'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { Terminal } from '@xterm/headless'
import * as pty from 'node-pty'
import { expect, it } from 'vitest'

import { quietClaudeResumeNotices } from './claude-resume-notices'
import { encodeClaudeProjectPath } from './claude-session-catalog'

// Opt-in native-provider compatibility gate: isolated HOME/config + local fake
// model, no real credentials, user files or paid inference. Keep evidence in tmp.
it.skipIf(!process.env.MATOU_TEST_CLAUDE_COMMAND)('native Claude preserves history, transcript receipts and AGENTS context across repeated resumes', async () => {
  const root = await realpath(await mkdtemp(join(tmpdir(), 'matou-native-quiet-')))
  const cwd = join(root, 'project'), config = join(root, 'config'), home = join(root, 'home')
  const sessionId = randomUUID()
  const project = join(config, 'projects', encodeClaudeProjectPath(cwd))
  await Promise.all([mkdir(project, { recursive: true }), mkdir(cwd), mkdir(home)])
  const instructions = 'MATOU_AGENTS_QUIET_CANARY: This rule must still be loaded.\n'
  await writeFile(join(cwd, 'AGENTS.md'), instructions)
  await writeFile(join(config, '.claude.json'), JSON.stringify({
    hasCompletedOnboarding: true, lastOnboardingVersion: '2.1.280', theme: 'light',
    customApiKeyResponses: { approved: ['fixture-not-a-key'], rejected: [] },
    cachedGrowthBookFeatures: { tengu_agents_md_mod: true },
    projects: { [cwd]: { hasTrustDialogAccepted: true, hasCompletedProjectOnboarding: true } }
  }))
  const user = randomUUID(), assistant = randomUUID()
  const base = { sessionId, cwd, version: '2.1.280', gitBranch: '', isSidechain: false,
    timestamp: new Date(Date.now() - 60_000).toISOString() }
  const response = (uuid: string, parentUuid: string, text: string) => ({
    ...base, type: 'assistant', uuid, parentUuid,
    message: { id: `msg-${uuid}`, type: 'message', role: 'assistant', model: 'claude-opus-4-6',
      content: [{ type: 'text', text }], stop_reason: 'end_turn', stop_sequence: null,
      usage: { input_tokens: 1, output_tokens: 1 } }
  })
  const rows: Array<Record<string, unknown> & { uuid: string }> = [
    { ...base, type: 'user', uuid: user, parentUuid: null, message: { role: 'user', content: 'EARLY USER HISTORY' } },
    response(assistant, user, 'EARLY ASSISTANT HISTORY')
  ]
  for (let index = 0; index < 10; index++) rows.push({ ...base, type: 'system', subtype: 'informational',
    uuid: randomUUID(), parentUuid: rows.at(-1)!.uuid, level: 'notice', isMeta: false,
    content: `agents-md: no CLAUDE.md found; AGENTS.md loaded: ${cwd}/AGENTS.md` })
  const latestUser = randomUUID()
  rows.push({ ...base, type: 'user', uuid: latestUser, parentUuid: rows.at(-1)!.uuid,
    message: { role: 'user', content: 'LATEST USER AFTER OLD NOTICES' } })
  rows.push(response(randomUUID(), latestUser, 'LATEST ASSISTANT AFTER OLD NOTICES'))
  const original = rows.map(row => JSON.stringify(row)).join('\n') + '\n'
  const transcript = join(project, `${sessionId}.jsonl`)
  await writeFile(transcript, original)
  const requests: string[] = []
  const server = createServer(async (req, res) => {
    let body = ''
    for await (const chunk of req) body += chunk
    if (req.url?.includes('/v1/messages') && !req.url.includes('count_tokens')) {
      requests.push(body)
      res.setHeader('Content-Type', 'text/event-stream')
      const events: Array<[string, Record<string, unknown>]> = [
        ['message_start', { message: { id: `msg-${randomUUID()}`, type: 'message', role: 'assistant', model: 'claude-opus-4-6', content: [], stop_reason: null, stop_sequence: null, usage: { input_tokens: 10, output_tokens: 0 } } }],
        ['content_block_start', { index: 0, content_block: { type: 'text', text: '' } }],
        ['content_block_delta', { index: 0, delta: { type: 'text_delta', text: 'LOCAL NATIVE RESUME REPLY' } }],
        ['content_block_stop', { index: 0 }],
        ['message_delta', { delta: { stop_reason: 'end_turn', stop_sequence: null }, usage: { output_tokens: 5 } }],
        ['message_stop', {}]
      ]
      for (const [type, data] of events) res.write(`event: ${type}\ndata: ${JSON.stringify({ type, ...data })}\n\n`)
      res.end()
    } else {
      res.setHeader('Content-Type', 'application/json')
      res.end('{"input_tokens":10}')
    }
  })
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve))
  const port = (server.address() as { port: number }).port
  const env = { PATH: process.env.PATH!, HOME: home, TERM: 'xterm-256color', LANG: 'en_US.UTF-8',
    CLAUDE_CONFIG_DIR: config, ANTHROPIC_API_KEY: 'fixture-not-a-key',
    ANTHROPIC_BASE_URL: `http://127.0.0.1:${port}`, DISABLE_AUTOUPDATER: '1' }
  let current: ReturnType<typeof launch> | undefined
  function launch() {
    const term = new Terminal({ cols: 150, rows: 70, allowProposedApi: true, scrollback: 5000 })
    const proc = pty.spawn(process.env.MATOU_TEST_CLAUDE_COMMAND!, [
      '--resume', sessionId, '--settings', JSON.stringify({ enabledPlugins: { 'agents-md@builtin': true } })
    ], { cwd, env, cols: 150, rows: 70 })
    proc.onData(data => term.write(data))
    const closed = new Promise<void>(resolve => proc.onExit(() => resolve()))
    return { proc, term, close: async () => { proc.kill(); await closed; term.dispose() }, screen: () => {
      const lines = []
      for (let y = 0; y < term.buffer.active.length; y++) lines.push(term.buffer.active.getLine(y)?.translateToString(true) ?? '')
      return lines.join('\n')
    } }
  }
  try {
    current = launch()
    await expect.poll(() => current!.screen(), { timeout: 25_000 }).toContain('LATEST ASSISTANT AFTER OLD NOTICES')
    expect(current.screen().match(/agents-md: no CLAUDE/g)!.length).toBeGreaterThanOrEqual(10)
    await writeFile(join(root, 'before.txt'), current.screen())
    await current.close(); current = undefined
    expect((await readFile(transcript, 'utf8')).startsWith(original)).toBe(true)
    const result = await quietClaudeResumeNotices({ cwd, providerSessionId: sessionId, env })
    expect(result.quieted).toBeGreaterThanOrEqual(10)
    expect((await readFile(transcript, 'utf8')).startsWith(original)).toBe(true)
    expect(await quietClaudeResumeNotices({ cwd, providerSessionId: sessionId, env })).toEqual({ quieted: 0 })
    current = launch()
    await expect.poll(() => current!.screen(), { timeout: 25_000 }).toContain('LATEST ASSISTANT AFTER OLD NOTICES')
    // The current native loader may emit one new receipt; old ones are quiet.
    expect(current.screen().match(/agents-md: no CLAUDE/g)?.length ?? 0).toBeLessThanOrEqual(1)
    expect(current.screen()).toContain('EARLY USER HISTORY')
    await writeFile(join(root, 'after.txt'), current.screen())
    current.proc.write('\x0f')
    await expect.poll(() => current!.screen().match(/agents-md: no CLAUDE/g)?.length ?? 0, { timeout: 10_000 }).toBeGreaterThanOrEqual(10)
    await writeFile(join(root, 'full-transcript.txt'), current.screen())
    current.proc.write('\x0f')
    await expect.poll(() => current!.screen().match(/agents-md: no CLAUDE/g)?.length ?? 0, { timeout: 10_000 }).toBeLessThanOrEqual(1)
    current.proc.write('Verify preserved context\r')
    await expect.poll(() => current!.screen(), { timeout: 25_000 }).toContain('LOCAL NATIVE RESUME REPLY')
    const request = requests.find(body => body.includes('Verify preserved context'))!
    expect(request).toContain('MATOU_AGENTS_QUIET_CANARY')
    for (const text of ['EARLY USER HISTORY', 'EARLY ASSISTANT HISTORY', 'LATEST USER AFTER OLD NOTICES', 'LATEST ASSISTANT AFTER OLD NOTICES']) expect(request).toContain(text)
    expect(await readFile(join(cwd, 'AGENTS.md'), 'utf8')).toBe(instructions)
    await writeFile(join(root, 'continued.txt'), current.screen())
    await current.close(); current = undefined
    expect((await readFile(transcript, 'utf8')).startsWith(original)).toBe(true)
    console.log(`Native Claude evidence: ${root}`)
  } finally {
    await current?.close()
    await new Promise<void>(resolve => server.close(() => resolve()))
  }
}, 100_000)
