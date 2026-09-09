// @vitest-environment jsdom
import { act, cleanup, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { EmptyWorkspaceState } from '../hierarchy/EmptyWorkspaceState'
import { LocaleProvider } from './LocaleProvider'
import { currentLocale, messages } from './current'

describe('LocaleProvider', () => {
  afterEach(() => { cleanup(); Reflect.deleteProperty(window, 'matouDesktop'); document.documentElement.lang = 'zh-CN' })

  it('defaults to Chinese without a desktop bridge', () => {
    render(<LocaleProvider><EmptyWorkspaceState onCreate={() => {}} /></LocaleProvider>)
    expect(screen.getByRole('heading', { name: '还没有工作区' })).toBeTruthy()
    expect(currentLocale()).toBe('zh-CN')
  })

  it('renders English when the bridge reports en and follows later changes', async () => {
    let emit: (locale: 'zh-CN' | 'en') => void = () => {}
    Object.assign(window, { matouDesktop: {
      getLocale: vi.fn(async () => 'en'),
      onLocaleChanged: vi.fn((listener: (locale: 'zh-CN' | 'en') => void) => { emit = listener; return () => {} })
    } })
    render(<LocaleProvider><EmptyWorkspaceState onCreate={() => {}} /></LocaleProvider>)
    expect(await screen.findByRole('heading', { name: 'No workspace yet' })).toBeTruthy()
    expect(document.documentElement.lang).toBe('en')
    expect(messages().common.cancel).toBe('Cancel')
    act(() => emit('zh-CN'))
    expect(screen.getByRole('heading', { name: '还没有工作区' })).toBeTruthy()
  })

  it('honours initialLocale for tests that render a single locale', () => {
    render(<LocaleProvider initialLocale="en"><EmptyWorkspaceState onCreate={() => {}} /></LocaleProvider>)
    expect(screen.getByRole('button', { name: 'New workspace' })).toBeTruthy()
  })
})
