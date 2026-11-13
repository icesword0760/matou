import { spawn, type ChildProcess } from 'node:child_process'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, describe, expect, it, vi } from 'vitest'

import { PtyExecutionPauser, SharedExecutionPauser } from './pty-execution-pauser'

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

describe('SharedExecutionPauser', () => {
  it('pauses inner once across multiple reasons and resumes only after the last reason resumes', async () => {
    const inner = {
      pause: vi.fn(),
      resume: vi.fn()
    }
    const shared = new SharedExecutionPauser(inner)
    const durability = shared.lease('durability')
    const egress = shared.lease('egress')

    expect(shared.isPausedFor('durability')).toBe(false)
    expect(shared.isPausedFor('egress')).toBe(false)

    await durability.pause()
    expect(shared.isPausedFor('durability')).toBe(true)
    expect(shared.isPausedFor('egress')).toBe(false)
    expect(inner.pause).toHaveBeenCalledTimes(1)

    await egress.pause()
    expect(shared.isPausedFor('durability')).toBe(true)
    expect(shared.isPausedFor('egress')).toBe(true)
    expect(inner.pause).toHaveBeenCalledTimes(1)
    expect(inner.resume).not.toHaveBeenCalled()

    await durability.resume()
    expect(shared.isPausedFor('durability')).toBe(false)
    expect(shared.isPausedFor('egress')).toBe(true)
    expect(inner.resume).not.toHaveBeenCalled()

    await egress.resume()
    expect(shared.isPausedFor('durability')).toBe(false)
    expect(shared.isPausedFor('egress')).toBe(false)
    expect(inner.resume).toHaveBeenCalledTimes(1)
  })

  it('treats repeated pause and resume calls on the same lease as idempotent', async () => {
    const inner = {
      pause: vi.fn(),
      resume: vi.fn()
    }
    const shared = new SharedExecutionPauser(inner)
    const egress = shared.lease('egress')

    await egress.resume()
    expect(inner.resume).not.toHaveBeenCalled()

    await egress.pause()
    await egress.pause()
    expect(inner.pause).toHaveBeenCalledTimes(1)

    await egress.resume()
    await egress.resume()
    expect(inner.resume).toHaveBeenCalledTimes(1)
  })

  it('keeps reason paused if inner resume rejects so a retry can still resume', async () => {
    let failResume = true
    const inner = {
      pause: vi.fn(async () => {}),
      resume: vi.fn(async () => {
        if (failResume) {
          failResume = false
          throw new Error('transient resume failure')
        }
      })
    }
    const shared = new SharedExecutionPauser(inner)
    const durability = shared.lease('durability')

    await durability.pause()
    await expect(durability.resume()).rejects.toThrow('transient resume failure')
    expect(shared.isPausedFor('durability')).toBe(true)

    await durability.resume()
    expect(shared.isPausedFor('durability')).toBe(false)
    expect(inner.resume).toHaveBeenCalledTimes(2)
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
