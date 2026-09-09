// @vitest-environment jsdom
import { cleanup, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'

import type { AppUpdateReleaseState, AppUpdateState, MatouDesktopApi } from '../../../shared/desktop-api'
import { LocaleProvider } from '../i18n/LocaleProvider'
import { AppUpdateControl } from './AppUpdateControl'

afterEach(() => {
  cleanup()
  localStorage.clear()
  Reflect.deleteProperty(window, 'matouDesktop')
})

describe('AppUpdateControl in English', () => {
  it('names the check action and the popover in English', async () => {
    render(<LocaleProvider initialLocale="en"><AppUpdateControl activeSessionCount={0} /></LocaleProvider>)

    await userEvent.setup().click(screen.getByRole('button', { name: 'App update' }))

    expect(screen.getByRole('dialog', { name: 'Matou app update' })).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Check for updates' })).toBeTruthy()
  })

  it('names the install action in English once an update is downloaded', async () => {
    installApi(downloaded())
    render(<LocaleProvider initialLocale="en"><AppUpdateControl activeSessionCount={0} /></LocaleProvider>)

    expect(await screen.findByRole('button', { name: 'Install and restart' })).toBeTruthy()
    expect(screen.getByText('Update ready')).toBeTruthy()
  })

  it('pluralises the active session count and reports retries in English', async () => {
    const api = installApi({
      status: 'checking', currentVersion: '1.0.0', retryAttempt: 2, maxRetryAttempts: 3
    })
    render(<LocaleProvider initialLocale="en"><AppUpdateControl activeSessionCount={1} /></LocaleProvider>)

    await userEvent.setup().click(await screen.findByRole('button', { name: 'App update: checking' }))
    expect(screen.getByText('Connection is unstable, retrying automatically (2/3)…')).toBeTruthy()

    api.publish(downloaded())
    expect(await screen.findByText('You have 1 active session')).toBeTruthy()
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

function downloaded(): AppUpdateReleaseState & { status: 'downloaded' } {
  return {
    status: 'downloaded', currentVersion: '1.0.0', version: '1.2.0',
    releaseDate: '2026-09-01T08:00:00.000Z', sizeBytes: 24_800_000,
    releaseNotes: ['Cloud updates and a safe restart'], installMode: 'automatic'
  }
}
