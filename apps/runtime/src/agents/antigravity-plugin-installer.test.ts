import { execFile } from 'node:child_process'
import { createServer } from 'node:http'
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { promisify } from 'node:util'

import { afterEach, describe, expect, it } from 'vitest'

import { AntigravityPluginInstaller } from './antigravity-plugin-installer'

const execFileAsync = promisify(execFile)

describe('AntigravityPluginInstaller', () => {
  const cleanups: Array<() => Promise<void>> = []

  afterEach(async () => {
    while (cleanups.length > 0) {
      await cleanups.pop()?.()
    }
  })

  it('installs plugin atomically, coalesces concurrent calls, and passes agy plugin validate when available', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-agy-plugin-test-'))
    cleanups.push(() => rm(root, { recursive: true, force: true }))
    const pluginDir = join(root, '.gemini', 'config', 'plugins', 'matou')
    const installer = new AntigravityPluginInstaller(pluginDir)

    await Promise.all([
      installer.ensureInstalled(),
      installer.ensureInstalled(),
      installer.ensureInstalled()
    ])

    const hooksJson = JSON.parse(await readFile(join(pluginDir, 'hooks.json'), 'utf8'))
    expect(Object.keys(hooksJson.matou)).toEqual([
      'SessionStart', 'PreInvocation', 'PostInvocation', 'PreToolUse', 'PostToolUse', 'Stop'
    ])
    const version = await readFile(join(pluginDir, '.matou-plugin-version'), 'utf8')
    expect(version.startsWith('1.0.0:')).toBe(true)

    const hookScript = await readFile(join(pluginDir, 'hooks', 'matou-hook.sh'), 'utf8')
    expect(hookScript).not.toContain('node')

    // Idempotent second call preserves version stamp and files cleanly.
    await installer.ensureInstalled()
    expect(await readFile(join(pluginDir, '.matou-plugin-version'), 'utf8')).toBe(version)

    try {
      const { stdout } = await execFileAsync('agy', ['plugin', 'validate', pluginDir, '--strict', '--json'])
      const report = JSON.parse(stdout) as {
        name: string
        components: { hooks: number }
        findings: Array<{ severity: string; message: string }>
      }
      expect(report.name).toBe('matou')
      expect(report.components.hooks).toBe(1)
      expect(report.findings.filter((item) => item.severity === 'error' || item.severity === 'warning')).toEqual([])
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error
    }
  })

  it('exits 0 with zero stdout/stderr outside Matou and posts hooks in background inside Matou', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-agy-hook-exec-'))
    cleanups.push(() => rm(root, { recursive: true, force: true }))
    const pluginDir = join(root, 'plugin')
    await new AntigravityPluginInstaller(pluginDir).ensureInstalled()

    const hookPath = join(pluginDir, 'hooks', 'matou-hook.sh')
    const sessionStartPath = join(pluginDir, 'hooks', 'session-start.sh')

    // 1. External agy (no MATOU_PROVIDER_HOOK_*): 0 bytes stdout/stderr, exit 0.
    const cleanEnv: NodeJS.ProcessEnv = { PATH: process.env.PATH }
    const externalResult = await runScriptWithStdin(hookPath, '{"hookEventName":"PreToolUse"}', cleanEnv)
    expect(externalResult).toEqual({ code: 0, stdout: '', stderr: '' })

    // 2. Managed agy PreToolUse: 0 bytes stdout/stderr, background POST delivered.
    const received: string[] = []
    let notifyReceived: (() => void) | undefined
    const receivedPromise = new Promise<void>((resolve) => { notifyReceived = resolve })
    const server = createServer((req, res) => {
      const chunks: Buffer[] = []
      req.on('data', (chunk) => chunks.push(Buffer.from(chunk)))
      req.on('end', () => {
        received.push(Buffer.concat(chunks).toString('utf8'))
        res.writeHead(200, { 'content-type': 'application/json' })
        res.end('{}')
        notifyReceived?.()
      })
    })
    await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', () => resolve()))
    cleanups.push(() => new Promise<void>((resolve) => server.close(() => resolve())))
    const address = server.address() as { port: number }
    const hookUrl = `http://127.0.0.1:${address.port}/hooks/tok-test`

    const managedEnv: NodeJS.ProcessEnv = {
      PATH: process.env.PATH,
      MATOU_PROVIDER_HOOK_URL: hookUrl,
      MATOU_PROVIDER_HOOK_TOKEN: 'tok-test'
    }
    const preToolPayload = '{"hookEventName":"PreToolUse","conversationId":"c1","toolCall":{"id":"t1","name":"run_command"}}'
    const preToolResult = await runScriptWithStdin(hookPath, preToolPayload, managedEnv)
    expect(preToolResult).toEqual({ code: 0, stdout: '', stderr: '' })
    await receivedPromise
    expect(received[0]).toBe(preToolPayload)

    // 3. SessionStart: cats pre-generated JSON file only when env var points to readable file.
    const sysMsgFile = join(root, 'system-message.json')
    const sysMsgJson = JSON.stringify({ injectSteps: [{ systemMessage: { systemMessage: 'Hello Matou' } }] })
    await writeFile(sysMsgFile, sysMsgJson, 'utf8')
    const startResult = await runScriptWithStdin(
      sessionStartPath,
      '{"hookEventName":"SessionStart","conversationId":"c1"}',
      { ...managedEnv, MATOU_ANTIGRAVITY_SYSTEM_MESSAGE_FILE: sysMsgFile }
    )
    expect(startResult).toEqual({ code: 0, stdout: sysMsgJson, stderr: '' })

    // SessionStart without system message file outputs 0 bytes.
    const startNoFileResult = await runScriptWithStdin(
      sessionStartPath,
      '{"hookEventName":"SessionStart","conversationId":"c1"}',
      managedEnv
    )
    expect(startNoFileResult).toEqual({ code: 0, stdout: '', stderr: '' })
  })
})

async function runScriptWithStdin(
  scriptPath: string,
  stdinPayload: string,
  env: NodeJS.ProcessEnv
): Promise<{ code: number | null; stdout: string; stderr: string }> {
  const { spawn } = await import('node:child_process')
  return new Promise((resolve, reject) => {
    const child = spawn(scriptPath, [], { env })
    const out: Buffer[] = []
    const err: Buffer[] = []
    child.stdout.on('data', (chunk) => out.push(Buffer.from(chunk)))
    child.stderr.on('data', (chunk) => err.push(Buffer.from(chunk)))
    child.on('error', reject)
    child.on('close', (code) => resolve({
      code,
      stdout: Buffer.concat(out).toString('utf8'),
      stderr: Buffer.concat(err).toString('utf8')
    }))
    child.stdin.end(stdinPayload)
  })
}
