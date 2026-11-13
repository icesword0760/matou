// @vitest-environment jsdom
import { cleanup, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, expect, it, vi } from 'vitest'
import { ClaudeTimeZoneSettings } from './ClaudeTimeZoneSettings'
import { LocaleProvider } from '../i18n/LocaleProvider'

afterEach(cleanup)

it('defaults to automatic, previews the non-mainland zone, and saves explicit choices', async () => {
  const request = vi.fn(async (_method: string, payload: unknown) => ({
    timeZone: (payload as { timeZone?: string }).timeZone ?? 'auto',
    systemTimeZone: 'Asia/Shanghai',
    autoTimeZone: 'Asia/Singapore'
  }))
  render(<ClaudeTimeZoneSettings client={{ request }} />)
  const select = await screen.findByRole('combobox', { name: '时区' }) as HTMLSelectElement
  expect(select.value).toBe('auto')
  expect(select.options[0]?.value).toBe('auto')
  expect(select.options[0]?.textContent).toBe('自动（隐藏大陆时区，推荐）')
  expect(select.options[1]?.value).toBe('system')
  expect(select.options[1]?.textContent).toBe('跟随系统 (Asia/Shanghai)')
  expect(screen.getByText(/· Asia\/Singapore$/)).toBeTruthy()
  expect(
    screen.getByText('自动：本机是大陆时区时，Claude Code 和 Codex 使用同为东八区的新加坡时区（出口在台湾时用台北），时间读数不变。')
  ).toBeTruthy()
  expect(screen.getByText(/仅影响码头启动的 Claude Code、Codex 及其子进程/)).toBeTruthy()

  const user = userEvent.setup()
  await user.selectOptions(select, 'America/Los_Angeles')
  expect(screen.getByText(/· America\/Los_Angeles$/)).toBeTruthy()
  expect(screen.getByText(/正在运行的会话需重启后生效/)).toBeTruthy()
  await user.click(screen.getByRole('button', { name: '保存' }))
  expect(request).toHaveBeenCalledWith('claude-settings.set', { timeZone: 'America/Los_Angeles' })
  expect(await screen.findByRole('status')).toHaveProperty('textContent', '已保存，下次启动 Claude Code 时自动生效。')
  expect(screen.getByRole('button', { name: '保存' })).toHaveProperty('disabled', true)
})

it('falls back preview to systemTimeZone when autoTimeZone is absent', async () => {
  const request = vi.fn(async () => ({
    timeZone: 'auto',
    systemTimeZone: 'Europe/Berlin'
  }))
  render(<ClaudeTimeZoneSettings client={{ request }} />)
  const select = await screen.findByRole('combobox', { name: '时区' }) as HTMLSelectElement
  expect(select.value).toBe('auto')
  expect(screen.getByText(/· Europe\/Berlin$/)).toBeTruthy()
})

it('renders English copy for automatic mode and scope notice', async () => {
  const request = vi.fn(async () => ({
    timeZone: 'auto',
    systemTimeZone: 'Asia/Shanghai',
    autoTimeZone: 'Asia/Singapore'
  }))
  render(<LocaleProvider initialLocale="en"><ClaudeTimeZoneSettings client={{ request }} /></LocaleProvider>)
  const select = await screen.findByRole('combobox', { name: 'Time zone' }) as HTMLSelectElement
  expect(select.options[0]?.textContent).toBe('Automatic (hide mainland China time zone, recommended)')
  expect(select.options[1]?.textContent).toBe('Follow system (Asia/Shanghai)')
  expect(
    screen.getByText(
      'Automatic: when this Mac uses a mainland China time zone, Claude Code and Codex use another UTC+8 zone (Singapore, or Taipei when your exit is in Taiwan), so clock times stay the same.'
    )
  ).toBeTruthy()
  expect(screen.getByText(/Applies only to Claude Code, Codex and their child processes launched by Matou\./)).toBeTruthy()
})

it('keeps the draft and retry available when saving fails', async () => {
  const request = vi.fn(async (method: string) => {
    if (method === 'claude-settings.set') throw new Error('disk full')
    return { timeZone: 'auto', systemTimeZone: 'Asia/Shanghai', autoTimeZone: 'Asia/Singapore' }
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
