// @vitest-environment jsdom
import { cleanup, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, expect, it, vi } from 'vitest'
import { ClaudeTimeZoneSettings } from './ClaudeTimeZoneSettings'
import { LocaleProvider } from '../i18n/LocaleProvider'

afterEach(cleanup)

it('loads, previews and saves a zone with an honest running-session notice', async () => {
  const request = vi.fn(async (_method: string, payload: unknown) => ({ timeZone: (payload as { timeZone?: string }).timeZone ?? 'system', systemTimeZone: 'Asia/Shanghai' }))
  render(<ClaudeTimeZoneSettings client={{ request }} />)
  const select = await screen.findByRole('combobox', { name: '时区' })
  const user = userEvent.setup()
  await user.selectOptions(select, 'America/Los_Angeles')
  expect(screen.getByText(/正在运行的会话需重启后生效/)).toBeTruthy()
  await user.click(screen.getByRole('button', { name: '保存' }))
  expect(request).toHaveBeenCalledWith('claude-settings.set', { timeZone: 'America/Los_Angeles' })
  expect(await screen.findByRole('status')).toHaveProperty('textContent', '已保存，下次启动 Claude Code 时自动生效。')
  expect(screen.getByRole('button', { name: '保存' })).toHaveProperty('disabled', true)
})

it('keeps the draft and retry available when saving fails', async () => {
  const request = vi.fn(async (method: string) => {
    if (method === 'claude-settings.set') throw new Error('disk full')
    return { timeZone: 'system', systemTimeZone: 'Asia/Shanghai' }
  })
  render(<ClaudeTimeZoneSettings client={{ request }} />)
  const user = userEvent.setup()
  await user.selectOptions(await screen.findByRole('combobox'), 'UTC')
  await user.click(screen.getByRole('button', { name: '保存' }))
  expect(await screen.findByRole('alert')).toHaveProperty('textContent', '保存未确认，请重试。')
  expect(screen.queryByRole('status')).toBeNull()
  expect(screen.getByRole('combobox')).toHaveProperty('value', 'UTC')
  expect(screen.getByRole('button', { name: '保存' })).toHaveProperty('disabled', false)
})

it('does not allow an unread preference to be overwritten and supports English', async () => {
  const request = vi.fn(async () => { throw new Error('offline') })
  render(<LocaleProvider initialLocale="en"><ClaudeTimeZoneSettings client={{ request }} /></LocaleProvider>)
  expect(await screen.findByRole('alert')).toHaveProperty('textContent', expect.stringContaining('Could not load'))
  expect(screen.getByRole('button', { name: 'Save' })).toHaveProperty('disabled', true)
  await userEvent.setup().click(screen.getByRole('button', { name: 'Retry' }))
  expect(request).toHaveBeenCalledTimes(2)
})
