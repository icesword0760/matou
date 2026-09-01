// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'

import type { AppUpdateReleaseState, AppUpdateState, MatouDesktopApi } from '../../../shared/desktop-api'
import { AppUpdateControl } from './AppUpdateControl'

afterEach(() => {
  cleanup()
  localStorage.clear()
  Reflect.deleteProperty(window, 'matouDesktop')
})

describe('AppUpdateControl', () => {
  it('opens a discovered update and starts download only on user request', async () => {
    const api = installApi(available())
    render(<AppUpdateControl activeSessionCount={0} />)

    expect(await screen.findByText('Matou 1.2.0 可用')).toBeTruthy()
    expect(screen.getByText('云端更新与安全重启')).toBeTruthy()
    expect(api.downloadAppUpdate).not.toHaveBeenCalled()
    await userEvent.setup().click(screen.getByRole('button', { name: '后台下载' }))
    expect(api.downloadAppUpdate).toHaveBeenCalledTimes(1)
  })

  it('renders live progress without requiring the popover to remain open', async () => {
    const api = installApi(downloading())
    render(<AppUpdateControl activeSessionCount={0} />)

    await userEvent.setup().click(await screen.findByRole('button', { name: '应用更新：下载中 47%' }))
    expect(screen.getByText('47% · 约 18 秒')).toBeTruthy()
    expect(screen.getByText('11.6 MB / 24.8 MB')).toBeTruthy()

    api.publish({ ...downloading(), progress: { ...downloading().progress, percent: 63 } })
    await waitFor(() => expect(screen.getByRole('button', { name: '应用更新：下载中 63%' })).toBeTruthy())
  })

  it('waits for active sessions to become idle before installing once', async () => {
    const api = installApi(downloaded())
    const view = render(<AppUpdateControl activeSessionCount={3} />)
    expect(await screen.findByText('当前有 3 个活动会话')).toBeTruthy()

    await userEvent.setup().click(screen.getByRole('button', { name: '空闲后自动更新' }))
    expect(screen.getByText('已安排：空闲后自动更新')).toBeTruthy()
    expect(api.installAppUpdate).not.toHaveBeenCalled()

    view.rerender(<AppUpdateControl activeSessionCount={0} />)
    await waitFor(() => expect(api.installAppUpdate).toHaveBeenCalledTimes(1))
    view.rerender(<AppUpdateControl activeSessionCount={0} />)
    expect(api.installAppUpdate).toHaveBeenCalledTimes(1)
  })

  it('allows immediate restart or install on normal exit after download', async () => {
    const api = installApi(downloaded())
    render(<AppUpdateControl activeSessionCount={0} />)

    expect(await screen.findByRole('button', { name: '退出时安装' })).toBeTruthy()
    await userEvent.setup().click(screen.getByRole('button', { name: '重启并更新' }))
    expect(api.installAppUpdate).toHaveBeenCalledTimes(1)
  })

  it('closes on Escape and outside pointer input', async () => {
    installApi(available())
    render(<div><AppUpdateControl activeSessionCount={0} /><button>外部区域</button></div>)
    expect(await screen.findByRole('dialog', { name: 'Matou 应用更新' })).toBeTruthy()

    fireEvent.keyDown(window, { key: 'Escape' })
    expect(screen.queryByRole('dialog', { name: 'Matou 应用更新' })).toBeNull()
    await userEvent.setup().click(screen.getByRole('button', { name: /应用更新/ }))
    expect(screen.getByRole('dialog', { name: 'Matou 应用更新' })).toBeTruthy()
    fireEvent.pointerDown(screen.getByRole('button', { name: '外部区域' }))
    expect(screen.queryByRole('dialog', { name: 'Matou 应用更新' })).toBeNull()
  })

  it('shows a retry action after an update error', async () => {
    const api = installApi({ status: 'error', currentVersion: '1.0.0', errorMessage: 'server unavailable' })
    render(<AppUpdateControl activeSessionCount={0} />)
    await userEvent.setup().click(await screen.findByRole('button', { name: /应用更新/ }))
    expect(screen.getByText('更新检查失败')).toBeTruthy()
    await userEvent.setup().click(screen.getByRole('button', { name: '重新检查' }))
    expect(api.checkForAppUpdates).toHaveBeenCalledTimes(1)
  })
})

function installApi(initial: AppUpdateState) {
  let listener: ((state: AppUpdateState) => void) | undefined
  const api = {
    getAppUpdateState: vi.fn(async () => initial),
    checkForAppUpdates: vi.fn(async () => undefined),
    downloadAppUpdate: vi.fn(async () => undefined),
    installAppUpdate: vi.fn(async () => undefined),
    onAppUpdateState: vi.fn((next: (state: AppUpdateState) => void) => {
      listener = next
      return () => { listener = undefined }
    }),
    publish: (state: AppUpdateState) => listener?.(state)
  }
  Object.defineProperty(window, 'matouDesktop', {
    configurable: true, value: api as unknown as MatouDesktopApi
  })
  return api
}

function available(): AppUpdateReleaseState & { status: 'available' } {
  return {
    status: 'available', currentVersion: '1.0.0', version: '1.2.0',
    releaseDate: '2026-09-01T08:00:00.000Z', sizeBytes: 24_800_000,
    releaseNotes: ['云端更新与安全重启', '优化会话恢复']
  }
}

function downloaded(): AppUpdateReleaseState & { status: 'downloaded' } {
  return { ...available(), status: 'downloaded' }
}

function downloading(): AppUpdateReleaseState & Extract<AppUpdateState, { progress: unknown }> {
  return {
    ...available(), status: 'downloading',
    progress: {
      percent: 47, transferredBytes: 11_600_000, totalBytes: 24_800_000,
      bytesPerSecond: 733_333, remainingSeconds: 18
    }
  }
}
