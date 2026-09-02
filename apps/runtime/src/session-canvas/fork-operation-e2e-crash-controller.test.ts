import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, describe, expect, it, vi } from 'vitest'

import type { ForkOperationRecord } from '../session/session-fork-intent-repository'
import { createE2eForkCrashObserver } from './fork-operation-e2e-crash-controller'

let root = ''

afterEach(async () => {
  if (root) await rm(root, { recursive: true, force: true })
  root = ''
})

describe('fork operation E2E crash controller', () => {
  it('is available only for an explicit E2E killpoint and crashes once across restarts', async () => {
    root = await mkdtemp(join(tmpdir(), 'matou-fork-crash-'))
    const marker = join(root, 'crashed.json')
    const crash = vi.fn()
    const observer = createE2eForkCrashObserver({
      MATOU_E2E: '1',
      MATOU_E2E_FORK_KILLPOINT: 'session-bound',
      MATOU_E2E_FORK_CRASH_MARKER: marker
    }, { crash })
    expect(observer).toBeDefined()

    observer!.reach('provider-before', operation())
    expect(crash).not.toHaveBeenCalled()
    observer!.reach('session-bound', operation())
    observer!.reach('session-bound', operation())

    expect(crash).toHaveBeenCalledTimes(1)
    expect(JSON.parse(await readFile(marker, 'utf8'))).toMatchObject({
      point: 'session-bound', operationId: 'operation-1', sessionId: 'session-1'
    })
  })

  it('stays disabled outside E2E and for unknown points', () => {
    expect(createE2eForkCrashObserver({
      MATOU_E2E: '0', MATOU_E2E_FORK_KILLPOINT: 'session-bound',
      MATOU_E2E_FORK_CRASH_MARKER: '/tmp/unused'
    })).toBeUndefined()
    expect(createE2eForkCrashObserver({
      MATOU_E2E: '1', MATOU_E2E_FORK_KILLPOINT: 'unknown',
      MATOU_E2E_FORK_CRASH_MARKER: '/tmp/unused'
    })).toBeUndefined()
  })
})

function operation(): ForkOperationRecord {
  return {
    identity: {
      operationId: 'operation-1', submissionKey: 'submission-1',
      sessionId: 'session-1',
      worktreePath: '/tmp/worktree', branchName: 'branch'
    },
    progress: {
      operationId: 'operation-1', sessionId: 'session-1',
      submissionKey: 'submission-1', stage: 'binding-session',
      completedSteps: 4, totalSteps: 6, attempt: 0
    },
    windowId: 'window-1', sceneId: 'scene-1', worktreeMode: 'new',
    repositoryRoot: '/tmp/repository'
  }
}
