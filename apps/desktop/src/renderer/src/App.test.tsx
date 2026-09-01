// @vitest-environment jsdom
import { cleanup, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { RuntimeLifecyclePresentation } from '../../shared/desktop-api'
import { App } from './App'

vi.mock('./hierarchy/HierarchyShell', () => ({
  HierarchyShell: () => <main data-testid="workspace">Workspace</main>
}))
vi.mock('./hierarchy/DetachedTerminalApp', () => ({ DetachedTerminalApp: () => <div>Detached</div> }))
vi.mock('./dag/DagWindowApp', () => ({ DagWindowApp: () => <div>Dag</div> }))
vi.mock('./terminal/TerminalSurface', () => ({ TerminalSurface: () => <div /> }))

beforeEach(() => window.history.replaceState({}, '', '/'))
afterEach(() => {
  cleanup()
  Reflect.deleteProperty(window, 'matouDesktop')
})

describe('App database lifecycle gate', () => {
  it('never flashes the ordinary workspace before a recovery-required state is loaded', async () => {
    installApi(recoveryState())
    render(<App />)

    expect(screen.queryByTestId('workspace')).toBeNull()
    expect(await screen.findByRole('heading', { name: '数据库需要恢复' })).toBeTruthy()
    expect(screen.queryByTestId('workspace')).toBeNull()
  })

  it('enters the ordinary workspace only after ready', async () => {
    installApi(readyState())
    render(<App />)
    expect(screen.queryByTestId('workspace')).toBeNull()
    expect(await screen.findByTestId('workspace')).toBeTruthy()
  })

  it('keeps the recovery page visible while a restore reopens the database', async () => {
    const reopening = recoveryState()
    reopening.snapshot = {
      ...reopening.snapshot,
      revision: 3,
      mode: 'normal',
      stage: 'opening-database'
    }
    installApi(reopening)
    render(<App />)
    expect(await screen.findByRole('heading', { name: '数据库需要恢复' })).toBeTruthy()
    expect(screen.queryByText('正在打开工作区…')).toBeNull()
    expect(screen.getByRole('button', { name: '重新检查数据库' }).hasAttribute('disabled')).toBe(true)
  })

  it('keeps the recovery page and interrupted operation error visible through reconnect', async () => {
    const initial = recoveryState()
    const api = installDynamicApi(initial)
    render(<App />)
    expect(await screen.findByRole('heading', { name: '数据库需要恢复' })).toBeTruthy()

    api.publish({
      ...initial,
      snapshot: { ...initial.snapshot, mode: 'normal', stage: 'opening-database', revision: 3 },
      operation: {
        requestId: 'restore-crash', action: 'restore-backup', pending: false,
        error: '数据库恢复操作未完成：Runtime 在恢复操作期间退出'
      }
    })

    expect(await screen.findByRole('heading', { name: '数据库需要恢复' })).toBeTruthy()
    expect(screen.getByRole('alert').textContent).toContain('Runtime 在恢复操作期间退出')
    expect(screen.queryByText('正在打开工作区…')).toBeNull()
    expect(screen.queryByTestId('workspace')).toBeNull()
  })
})

function installApi(initial: RuntimeLifecyclePresentation): void {
  Object.defineProperty(window, 'matouDesktop', {
    configurable: true,
    value: {
      getRuntimeLifecycle: vi.fn().mockResolvedValue(initial),
      onRuntimeLifecycle: vi.fn(() => () => undefined),
      restoreDatabaseBackup: vi.fn(),
      exportDatabaseRecoveryBundle: vi.fn(),
      retryDatabaseOpen: vi.fn(),
      startWithEmptyDatabase: vi.fn()
    }
  })
}

function installDynamicApi(initial: RuntimeLifecyclePresentation): {
  publish(value: RuntimeLifecyclePresentation): void
} {
  let listener: ((value: RuntimeLifecyclePresentation) => void) | undefined
  Object.defineProperty(window, 'matouDesktop', {
    configurable: true,
    value: {
      getRuntimeLifecycle: vi.fn().mockResolvedValue(initial),
      onRuntimeLifecycle: vi.fn((next: (value: RuntimeLifecyclePresentation) => void) => {
        listener = next
        return () => { listener = undefined }
      }),
      restoreDatabaseBackup: vi.fn(),
      exportDatabaseRecoveryBundle: vi.fn(),
      retryDatabaseOpen: vi.fn(),
      startWithEmptyDatabase: vi.fn()
    }
  })
  return { publish: (value) => listener?.(value) }
}

function readyState(): RuntimeLifecyclePresentation {
  return {
    snapshot: {
      recoveryId: 'ready-app', revision: 2, mode: 'normal', stage: 'ready',
      completed: 1, total: 1, failures: []
    }
  }
}

function recoveryState(): RuntimeLifecyclePresentation {
  return {
    snapshot: {
      recoveryId: 'recovery-app', revision: 2, mode: 'recovery-required',
      stage: 'opening-database', completed: 0, total: 1, failures: []
    },
    recovery: {
      recoveryId: 'durable-recovery-app',
      reason: 'physical-corruption', durableDatabasePath: '/data/matou.sqlite',
      quarantinedPath: '/data/matou.sqlite.corrupt-1', backups: []
    }
  }
}
