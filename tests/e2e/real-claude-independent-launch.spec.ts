import { createServer } from 'node:http'
import { randomUUID } from 'node:crypto'
import { homedir } from 'node:os'
import { mkdir, mkdtemp, readFile, readdir, rm, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { DatabaseSync } from 'node:sqlite'
import { _electron as electron, expect, test } from '@playwright/test'
import { launchMatou, restartMatouGracefully, type MatouFixture } from './matou-fixture'
import { terminalCommand, visibleSurfaces, waitForShell } from './fixtures/session-canvas-fixture'

// Unmodified official CLI; disposable HOME/config; only a loopback model server.
// No login, credentials, upstream API calls or edits to the user's conversations.
test('agent-launched app and normal restarts persist and resume the latest official CLI turns', async () => {
  test.skip(process.platform !== 'darwin' || !process.env.MATOU_TEST_CLAUDE_COMMAND)
  test.setTimeout(240_000)
  const root = await mkdtemp('/private/tmp/matou-independent-launch-')
  const home = join(root, 'home'), config = join(root, 'config'), cwd = join(root, 'matou_workspace')
  await Promise.all([mkdir(home), mkdir(config), mkdir(cwd)])
  const key = 'fixture-not-a-key'
  await writeFile(join(config, '.claude.json'), JSON.stringify({
    hasCompletedOnboarding: true, theme: 'dark',
    customApiKeyResponses: { approved: [key], rejected: [] },
    projects: { [cwd]: { hasTrustDialogAccepted: true, hasCompletedProjectOnboarding: true } }
  }))
  await writeFile(join(config, 'settings.json'), JSON.stringify({ disableDeepLinkRegistration: 'disable' }))
  const requests: Array<{ text: string; answer: string }> = []
  const server = createServer(async (req, res) => {
    let raw = ''; for await (const part of req) raw += part
    if (req.url?.includes('/v1/messages') && !req.url.includes('count_tokens')) {
      if (req.headers['x-api-key'] !== key) { res.writeHead(403); res.end(); return }
      const body = JSON.parse(raw), text = JSON.stringify(body.messages)
      const markers = [...text.matchAll(/PERSIST_TURN_([123])/g)]
      const round = markers.at(-1)?.[1] ?? '0', answer = `PERSIST_REPLY_${round}`
      requests.push({ text, answer })
      res.setHeader('Content-Type', 'text/event-stream')
      const events: Array<[string, object]> = [
        ['message_start', { message: { id: `msg_${randomUUID()}`, type: 'message', role: 'assistant', model: body.model,
          content: [], stop_reason: null, stop_sequence: null, usage: { input_tokens: 10, output_tokens: 0 } } }],
        ['content_block_start', { index: 0, content_block: { type: 'text', text: '' } }],
        ['content_block_delta', { index: 0, delta: { type: 'text_delta', text: answer } }],
        ['content_block_stop', { index: 0 }],
        ['message_delta', { delta: { stop_reason: 'end_turn', stop_sequence: null }, usage: { output_tokens: 5 } }],
        ['message_stop', {}]
      ]
      for (const [type, data] of events) res.write(`event: ${type}\ndata: ${JSON.stringify({ type, ...data })}\n\n`)
      res.end()
    } else { res.setHeader('Content-Type', 'application/json'); res.end('{"input_tokens":10}') }
  })
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve))
  const port = (server.address() as { port: number }).port
  const sandbox = join(root, 'isolation.sb'), wrapper = join(root, 'claude-fixture.sh')
  await writeFile(sandbox, `(version 1)
(allow default)
(deny file-write*)
(allow file-write* (subpath "${root}") (literal "/dev/null"))
(deny file-read* (subpath "${homedir()}/.claude") (literal "${homedir()}/.claude.json"))
(deny process-exec (literal "/usr/bin/security") (literal "/usr/bin/open"))
(deny network*)
(allow network-outbound (remote ip "localhost:${port}"))
(allow network-outbound (remote ip "localhost:*"))
`)
  await writeFile(wrapper, `#!/bin/sh
printf '%s|%s|%s\\n' "\${CLAUDE_CODE_CHILD_SESSION-unset}" "\${CLAUDECODE-unset}" "\${CLAUDE_CODE_SESSION_ID-unset}" >> '${root}/spawn-markers.txt'
export ANTHROPIC_BASE_URL='http://127.0.0.1:${port}' ANTHROPIC_API_KEY='${key}'
unset ANTHROPIC_AUTH_TOKEN CLAUDE_CODE_OAUTH_TOKEN
exec /usr/bin/sandbox-exec -f '${sandbox}' '${process.env.MATOU_TEST_CLAUDE_COMMAND}' --tools '' --strict-mcp-config --mcp-config '{"mcpServers":{}}' --debug-file '${root}/cli.debug' "$@"
`, { mode: 0o700 })
  const env = { HOME: home, CLAUDE_CONFIG_DIR: config, SHELL: '/bin/sh',
    ANTHROPIC_API_KEY: key, ANTHROPIC_AUTH_TOKEN: '', CLAUDE_CODE_OAUTH_TOKEN: '',
    ANTHROPIC_BASE_URL: `http://127.0.0.1:${port}`, HTTPS_PROXY: '', HTTP_PROXY: '', ALL_PROXY: '',
    DISABLE_AUTOUPDATER: '1', DISABLE_TELEMETRY: '1', CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: '1',
    MATOU_CLAUDE_COMMAND: wrapper }
  const polluted = { ...env, CLAUDE_CODE_CHILD_SESSION: '1', CLAUDECODE: '1',
    CLAUDE_CODE_SESSION_ID: randomUUID(), CLAUDE_CODE_SESSION_ATTENDED: '1', CLAUDE_PID: '999999',
    CLAUDE_CODE_ENTRYPOINT: 'cli' }
  const launch = async (environment: Record<string, string>): Promise<MatouFixture> => {
    if (!process.env.MATOU_TEST_PACKAGED_APP) return launchMatou({ root, env: environment })
    const dataDirectory = join(root, 'data'), electronUserDataDirectory = join(root, 'electron-user-data')
    await mkdir(electronUserDataDirectory, { recursive: true })
    const app = await electron.launch({ executablePath: process.env.MATOU_TEST_PACKAGED_APP,
      args: [`--user-data-dir=${electronUserDataDirectory}`],
      env: { ...process.env, ...environment, MATOU_E2E: '1', MATOU_LOCALE: 'zh-CN',
        MATOU_DATA_DIR: dataDirectory, MATOU_DEFAULT_WORKSPACE: cwd,
        ELECTRON_USER_DATA_DIR: electronUserDataDirectory }
    })
    return { app, page: await app.firstWindow(), rootDirectory: root, workspaceDirectory: cwd,
      dataDirectory, electronUserDataDirectory, close: async () => {
        await app.evaluate(({ app }) => app.quit()).catch(() => {})
        await app.close().catch(() => {})
        await rm(root, { recursive: true, force: true })
      } }
  }
  let fixture: MatouFixture | undefined
  let sessionId = '', providerId = ''
  const transcript = async () => {
    if (!providerId) return ''
    const projects = join(config, 'projects')
    for (const directory of await readdir(projects).catch(() => [])) {
      try { return await readFile(join(projects, directory, `${providerId}.jsonl`), 'utf8') } catch {}
    }
    return ''
  }
  try {
    fixture = await launch(polluted)
    const initial = visibleSurfaces(fixture.page).first()
    await waitForShell(initial)
    sessionId = (await initial.getAttribute('data-session-id'))!
    await terminalCommand(initial, 'claude')
    for (let round = 1; round <= 3; round++) {
      const surface = fixture.page.locator(`.terminal-surface[data-session-id="${sessionId}"]`)
      await expect(surface).toHaveAttribute('data-profile', 'claude-code', { timeout: 45_000 })
      await expect(surface.locator('.xterm-rows')).toContainText(round === 1 ? 'Claude Code' : `PERSIST_REPLY_${round - 1}`, { timeout: 45_000 })
      await terminalCommand(surface, `PERSIST_TURN_${round}`)
      await expect.poll(() => requests.some(r => r.answer === `PERSIST_REPLY_${round}`), { timeout: 30_000 }).toBe(true)
      await expect.poll(() => {
        const db = new DatabaseSync(join(fixture!.dataDirectory, 'matou.sqlite'), { readOnly: true })
        try {
          const binding = db.prepare('SELECT provider_session_id AS id FROM provider_bindings WHERE session_id = ? ORDER BY updated_at DESC LIMIT 1').get(sessionId) as { id: string } | undefined
          if (!providerId && binding) providerId = binding.id
          return binding?.id
        } finally { db.close() }
      }).toMatch(/^[0-9a-f-]{36}$/)
      await expect.poll(transcript, { timeout: 20_000 }).toContain(`PERSIST_REPLY_${round}`)
      const latest = requests.findLast(r => r.answer === `PERSIST_REPLY_${round}`)!
      for (let before = 1; before < round; before++) {
        expect(latest.text).toContain(`PERSIST_TURN_${before}`)
        expect(latest.text).toContain(`PERSIST_REPLY_${before}`)
      }
      await fixture.page.screenshot({ path: test.info().outputPath(`round-${round}.png`) })
      if (round < 3) {
        const nextEnvironment = round === 1 ? polluted : env
        if (process.env.MATOU_TEST_PACKAGED_APP) {
          await Promise.all([fixture.app.waitForEvent('close'), fixture.app.evaluate(({ app }) => app.quit())])
          fixture = await launch(nextEnvironment)
        } else fixture = await restartMatouGracefully(fixture, { env: nextEnvironment })
      }
    }
    const launches = (await readFile(join(root, 'spawn-markers.txt'), 'utf8')).trim().split('\n')
    expect(launches).toHaveLength(3)
    expect(launches.every(line => line === 'unset|unset|unset')).toBe(true)
    const proof = JSON.stringify({ sessionId, providerId, launches,
      completedTurns: 3, restoredWithFullContext: 2, realAccountUsed: false,
      packaged: Boolean(process.env.MATOU_TEST_PACKAGED_APP) }, null, 2)
    await writeFile(test.info().outputPath('persistence-proof.json'), proof)
    await test.info().attach('persistence-proof', { body: proof, contentType: 'application/json' })
  } catch (error) {
    if (fixture) {
      await test.info().attach('terminal-on-failure', { body: await fixture.page.locator('.xterm-rows').allTextContents().then(x => x.join('\n')), contentType: 'text/plain' })
    }
    await test.info().attach('cli-debug', { body: await readFile(join(root, 'cli.debug')).catch(() => Buffer.from('no debug log')), contentType: 'text/plain' })
    throw error
  } finally {
    if (fixture) await fixture.close()
    server.closeAllConnections(); await new Promise<void>(resolve => server.close(() => resolve()))
  }
})
