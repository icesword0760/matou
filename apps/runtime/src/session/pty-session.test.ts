import { chmod, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, describe, expect, it } from 'vitest'

import { PtySession } from './pty-session'

const roots: string[] = []

afterEach(async () => {
  await Promise.all(roots.splice(0).map((root) => rm(root, { recursive: true, force: true })))
})

describe('PtySession Runtime shutdown', () => {
  it('loads packaged Codex guidance as a session-only launch argument', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-pty-codex-guidance-'))
    roots.push(root)
    const controlAssetRoot = join(root, 'control assets 空格')
    const providerDirectory = join(controlAssetRoot, 'providers')
    await mkdir(providerDirectory, { recursive: true })
    const instructions = '先 identify\n再使用 "mt" 和 \\path'
    await writeFile(join(providerDirectory, 'codex-developer-instructions.md'), instructions)
    const executable = join(root, 'capture-codex-args.js')
    const argumentFile = join(root, 'codex-args.json')
    await writeFile(executable, `#!/usr/bin/env node
const fs = require('node:fs')
fs.writeFileSync(process.env.MATOU_TEST_ARGS_FILE, JSON.stringify(process.argv.slice(2)))
setInterval(() => {}, 1_000)
`)
    await chmod(executable, 0o755)
    const previousCommand = process.env.MATOU_CODEX_COMMAND
    process.env.MATOU_CODEX_COMMAND = executable
    try {
      const session = await PtySession.create({
        sessionId: 'codex-guidance', executionContextId: 'local-default',
        cols: 80, rows: 24, cwd: root, dataRoot: root, profile: 'codex',
        providerSessionId: 'codex-resume-id', permissionMode: 'bypassPermissions',
        controlAssetRoot, env: { MATOU_TEST_ARGS_FILE: argumentFile }, send: () => {}
      })
      const args = await waitForJsonArray(argumentFile)
      expect(args).toEqual([
        '-c', `developer_instructions=${JSON.stringify(instructions)}`,
        '--dangerously-bypass-approvals-and-sandbox',
        'resume', 'codex-resume-id'
      ])
      await session.shutdownForRuntime({ gracePeriodMs: 40, hardKillWaitMs: 1_000 })
    } finally {
      if (previousCommand === undefined) delete process.env.MATOU_CODEX_COMMAND
      else process.env.MATOU_CODEX_COMMAND = previousCommand
    }
  })

  it('escalates an unresponsive PTY and still closes its journal', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-pty-shutdown-'))
    roots.push(root)
    const executable = join(root, 'ignore-graceful-signals.js')
    await writeFile(executable, `#!/usr/bin/env node
process.on('SIGHUP', () => {})
process.on('SIGTERM', () => {})
process.stdout.write('PTY_SHUTDOWN_READY\\n')
setInterval(() => {}, 1_000)
`)
    await chmod(executable, 0o755)
    const previousShell = process.env.SHELL
    process.env.SHELL = executable
    let ready: () => void = () => {}
    const outputReady = new Promise<void>((resolve) => { ready = resolve })
    try {
      const session = await PtySession.create({
        sessionId: 'unresponsive-pty', executionContextId: 'local-default',
        cols: 80, rows: 24, cwd: root, dataRoot: root, profile: 'shell',
        send: () => {},
        onOutput: (data) => { if (data.includes('PTY_SHUTDOWN_READY')) ready() }
      })
      await Promise.race([
        outputReady,
        new Promise((_, reject) => setTimeout(() => reject(new Error('PTY did not start')), 2_000))
      ])

      const startedAt = Date.now()
      await session.shutdownForRuntime({ gracePeriodMs: 40, hardKillWaitMs: 1_000 })

      expect(Date.now() - startedAt).toBeLessThan(1_500)
      await expect(session.whenClosed()).resolves.toBeUndefined()
    } finally {
      if (previousShell === undefined) delete process.env.SHELL
      else process.env.SHELL = previousShell
    }
  })
})

async function waitForJsonArray(path: string): Promise<unknown[]> {
  const deadline = Date.now() + 2_000
  while (Date.now() < deadline) {
    try {
      return JSON.parse(await readFile(path, 'utf8')) as unknown[]
    } catch {
      await new Promise((resolve) => setTimeout(resolve, 20))
    }
  }
  throw new Error('provider argument capture timed out')
}
