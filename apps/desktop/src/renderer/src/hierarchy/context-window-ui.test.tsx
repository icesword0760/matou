// @vitest-environment jsdom
import { cleanup, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'

import type { ModelContextChoice } from '@matou/contracts'
import { ContextWindowPicker, formatContextWindowBadge, resolveProviderWindow } from './context-window-ui'
import { LocaleProvider } from '../i18n/LocaleProvider'

afterEach(cleanup)

const MODELS: ModelContextChoice[] = [
  { id: 'opus-a', name: 'Opus A', defaultTokens: 272_000, maximumTokens: 872_000, fixedTokens: 512_000 },
  { id: 'haiku-a', name: 'Haiku A', defaultTokens: 272_000, maximumTokens: 400_000, fixedTokens: 512_000 },
  { id: 'unknown-a', name: 'Unknown A', fixedTokens: 512_000 }
]

describe('context-window-ui', () => {
  it('takes the smallest known window across mapped models and falls back when any model is unknown', () => {
    expect(resolveProviderWindow('default', MODELS, ['opus-a', 'haiku-a'])).toEqual({
      tokens: 272_000, status: 'known', multiModel: true
    })
    expect(resolveProviderWindow('maximum', MODELS, ['opus-a', 'haiku-a'])).toEqual({
      tokens: 400_000, status: 'known', multiModel: true
    })
    expect(resolveProviderWindow('512k', MODELS, ['opus-a', 'unknown-a'])).toEqual({
      tokens: 512_000, status: 'fixed', multiModel: true
    })
    expect(resolveProviderWindow('maximum', MODELS, ['opus-a', 'unknown-a'])).toEqual({
      tokens: undefined, status: 'unknown', multiModel: true
    })
  })

  it('formats concise provider window badges in Chinese and English', () => {
    expect(formatContextWindowBadge('512k', MODELS, ['opus-a'], true)).toBe('512K 窗口')
    expect(formatContextWindowBadge('default', MODELS, ['opus-a'], true)).toBe('默认 272K 窗口')
    expect(formatContextWindowBadge('maximum', MODELS, ['opus-a', 'haiku-a'], true)).toBe('最大 400K 窗口')
    expect(formatContextWindowBadge('default', MODELS, ['unknown-a'], false)).toBe('Default window')
  })

  it('renders live token counts for mapped models and emits changes', async () => {
    const onChange = vi.fn()
    render(<LocaleProvider initialLocale="zh-CN">
      <ContextWindowPicker value="default" models={MODELS} selectedModelIds={['opus-a', 'haiku-a']} onChange={onChange} />
    </LocaleProvider>)

    expect(screen.getByText('272,000 tokens')).toBeTruthy()
    expect(screen.getByText('512,000 tokens')).toBeTruthy()
    expect(screen.getByText('400,000 tokens')).toBeTruthy()
    expect(screen.getByText(/映射多个模型时/)).toBeTruthy()

    await userEvent.setup().click(screen.getByRole('radio', { name: /最大可扩展/ }))
    expect(onChange).toHaveBeenCalledWith('maximum')
  })
})
