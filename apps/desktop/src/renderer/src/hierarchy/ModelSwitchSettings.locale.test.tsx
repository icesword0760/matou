// @vitest-environment jsdom
import { cleanup, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { CLI_DEFAULT_MODEL, type ProviderConfigSnapshot } from '@matou/contracts'

import { LocaleProvider } from '../i18n/LocaleProvider'
import { setCurrentLocale } from '../i18n/current'
import { ModelSwitchSettings, type ProviderConfigClient } from './ModelSwitchSettings'

afterEach(() => {
  cleanup()
  setCurrentLocale('zh-CN')
})

describe('Model switch settings in English', () => {
  it('labels the CLI default sentinel instead of rendering it', async () => {
    render(<LocaleProvider initialLocale="en">
      <ModelSwitchSettings client={fakeClient()} onClose={vi.fn()} />
    </LocaleProvider>)

    expect((await screen.findAllByText('CLI default')).length).toBeGreaterThan(0)
    expect(screen.queryByText(CLI_DEFAULT_MODEL)).toBeNull()
  })
})

function fakeClient(): ProviderConfigClient {
  const state: ProviderConfigSnapshot = {
    revision: 1,
    activeProviderIds: { 'claude-code': 'anthropic', codex: 'openai' },
    providers: {
      'claude-code': [{
        id: 'anthropic', cli: 'claude-code', name: 'Anthropic (official)',
        endpoint: 'https://api.anthropic.com', model: CLI_DEFAULT_MODEL,
        hasApiKey: false, builtIn: true
      }],
      codex: [{
        id: 'openai', cli: 'codex', name: 'OpenAI (official)',
        endpoint: 'https://api.openai.com/v1', model: CLI_DEFAULT_MODEL,
        hasApiKey: false, builtIn: true
      }]
    }
  }
  return {
    request: vi.fn(async (method: string) => {
      if (method === 'provider-config.snapshot') return state
      throw new Error(`unexpected provider config method ${method}`)
    })
  }
}
