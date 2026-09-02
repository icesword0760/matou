import {
  closeSync,
  mkdirSync,
  openSync,
  writeFileSync
} from 'node:fs'
import { dirname } from 'node:path'

import type {
  ForkKillPoint,
  ForkKillPointObserver
} from './fork-operation-coordinator'

interface ForkCrashEnvironment {
  MATOU_E2E?: string
  MATOU_E2E_FORK_CRASH_MARKER?: string
  MATOU_E2E_FORK_KILLPOINT?: string
}

interface ForkCrashDependencies {
  crash?: () => void
}

const KILL_POINTS = new Set<ForkKillPoint>([
  'intent-accepted',
  'branch-created',
  'path-created',
  'setup-completed',
  'session-bound',
  'provider-before'
])

/**
 * Real-process crash injection for visible E2E acceptance only. The exclusive
 * marker makes the crash one-shot across the Runtime process that replaces it.
 */
export function createE2eForkCrashObserver(
  environment: ForkCrashEnvironment,
  dependencies: ForkCrashDependencies = {}
): ForkKillPointObserver | undefined {
  const point = environment.MATOU_E2E_FORK_KILLPOINT as ForkKillPoint | undefined
  const marker = environment.MATOU_E2E_FORK_CRASH_MARKER
  if (environment.MATOU_E2E !== '1' || !point || !marker || !KILL_POINTS.has(point)) {
    return undefined
  }
  const crash = dependencies.crash ?? (() => process.kill(process.pid, 'SIGKILL'))
  return {
    reach(reached, operation): void {
      if (reached !== point) return
      mkdirSync(dirname(marker), { recursive: true })
      let handle: number
      try {
        handle = openSync(marker, 'wx')
      } catch (error) {
        if ((error as NodeJS.ErrnoException).code === 'EEXIST') return
        throw error
      }
      try {
        writeFileSync(handle, JSON.stringify({
          point: reached,
          operationId: operation.identity.operationId,
          sessionId: operation.identity.sessionId,
          runtimePid: process.pid
        }))
      } finally {
        closeSync(handle)
      }
      crash()
    }
  }
}
