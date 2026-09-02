import { spawn, type ChildProcess } from 'node:child_process'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, describe, expect, it, vi } from 'vitest'

import { PtyExecutionPauser } from './pty-execution-pauser'

const children: ChildProcess[] = []
const roots: string[] = []

afterEach(async () => {
  for (const child of children.splice(0)) {
    if (child.pid === undefined) continue
    try { process.kill(-child.pid, 'SIGKILL') } catch {}
  }
  await Promise.all(roots.splice(0).map((root) => rm(root, { recursive: true, force: true })))
})

describe('PtyExecutionPauser', () => {
  it('pauses PTY reads before SIGSTOP and sends SIGCONT before resuming reads', async () => {
    const order: string[] = []
    const terminal = {
      pid: 42,
      pause: vi.fn(() => { order.push('pause-reads') }),
      resume: vi.fn(() => { order.push('resume-reads') })
    }
    const pauser = new PtyExecutionPauser(terminal, {
      platform: 'linux',
      signalProcessGroup: (_pid, signal) => { order.push(signal) }
    })

    await pauser.pause()
    await pauser.pause()
    await pauser.resume()
    await pauser.resume()

    expect(order).toEqual(['pause-reads', 'SIGSTOP', 'SIGCONT', 'resume-reads'])
  })

  it('stops and resumes a real POSIX counting process group', async () => {
    if (process.platform === 'win32') return
    const root = await mkdtemp(join(tmpdir(), 'matou-pty-pauser-'))
    roots.push(root)
    const counter = join(root, 'counter.txt')
    const child = spawn(process.execPath, ['-e', `
      const fs = require('node:fs')
      const path = ${JSON.stringify(counter)}
      setInterval(() => fs.appendFileSync(path, 'x'), 5)
    `], { detached: true, stdio: 'ignore' })
    children.push(child)
    if (child.pid === undefined) throw new Error('counter process did not start')
    const pauser = new PtyExecutionPauser({ pid: child.pid, pause() {}, resume() {} })

    await waitFor(async () => (await byteLength(counter)) >= 10)
    await pauser.pause()
    await new Promise((resolve) => setTimeout(resolve, 50))
    const stoppedAt = await byteLength(counter)
    await new Promise((resolve) => setTimeout(resolve, 100))
    expect(await byteLength(counter)).toBe(stoppedAt)

    await pauser.resume()
    await waitFor(async () => (await byteLength(counter)) > stoppedAt)
  })
})

async function byteLength(path: string): Promise<number> {
  try { return (await readFile(path)).byteLength } catch { return 0 }
}

async function waitFor(predicate: () => Promise<boolean>, timeoutMs = 2_000): Promise<void> {
  const started = Date.now()
  while (!(await predicate())) {
    if (Date.now() - started > timeoutMs) throw new Error('condition timed out')
    await new Promise((resolve) => setTimeout(resolve, 10))
  }
}
