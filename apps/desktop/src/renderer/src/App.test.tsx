// @vitest-environment jsdom
import { act, cleanup, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { RuntimeLifecyclePresentation } from '../../shared/desktop-api'
import { App } from './App'

vi.mock('./hierarchy/HierarchyShell', () => ({
  HierarchyShell: ({ runtimeMode }: { runtimeMode?: string }) =>
    <main data-testid="workspace" data-runtime-mode={runtimeMode}>Workspace</main>
}))
vi.mock('./hierarchy/DetachedTerminalApp', () => ({
  DetachedTerminalApp: ({ runtimeMode }: { runtimeMode?: string }) =>
    <div data-testid="detached" data-runtime-mode={runtimeMode}>Detached</div>
}))
vi.mock('./dag/DagWindowApp', () => ({
  DagWindowApp: ({ runtimeMode }: { runtimeMode?: string }) =>
    <div data-testid="dag" data-runtime-mode={runtimeMode}>Dag</div>
}))
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

  it('passes the read-only lifecycle mode into the ordinary browsing workspace', async () => {
    const state = readyState()
    state.snapshot.mode = 'read-only'
    installApi(state)
    render(<App />)

    expect((await screen.findByTestId('workspace')).getAttribute('data-runtime-mode')).toBe('read-only')
  })

  it.each([
    ['detached-terminal', 'detached'],
    ['dag', 'dag']
  ])('passes read-only lifecycle into an existing %s presentation', async (kind, testId) => {
    window.history.replaceState({}, '', `/?kind=${kind}`)
    const state = readyState()
    state.snapshot.mode = 'read-only'
    installApi(state)
    render(<App />)

    expect((await screen.findByTestId(testId)).getAttribute('data-runtime-mode')).toBe('read-only')
  })

  it('updates an existing detached presentation immediately when Runtime becomes read-only', async () => {
    window.history.replaceState({}, '', '/?kind=detached-terminal')
    const initial = readyState()
    const api = installDynamicApi(initial)
    render(<App />)
    expect((await screen.findByTestId('detached')).getAttribute('data-runtime-mode')).toBe('normal')

    api.publish({ ...initial, snapshot: { ...initial.snapshot, revision: 3, mode: 'read-only' } })

    expect((await screen.findByTestId('detached')).getAttribute('data-runtime-mode')).toBe('read-only')
  })

  it('keeps an already-open workspace mounted while Runtime reconnects', async () => {
    const initial = readyState()
    const api = installDynamicApi(initial)
    render(<App />)
    expect(await screen.findByTestId('workspace')).toBeTruthy()

    act(() => {
      api.publish({
        ...initial,
        snapshot: {
          ...initial.snapshot,
          recoveryId: 'runtime-restart', revision: 0,
          stage: 'opening-database', completed: 0
        }
      })
    })

    expect(screen.getByTestId('workspace')).toBeTruthy()
    expect(screen.queryByText('正在打开工作区…')).toBeNull()
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
