import { describe, expect, it, vi } from 'vitest'

import { resolveProviderCommandEnvironment } from './provider-command-environment'

describe('provider command environment', () => {
  it('uses the login-shell PATH when a GUI launch cannot see the provider executable', async () => {
    const commandExists = vi.fn(async (_command: string, path: string | undefined) =>
      path === '/Users/test/.nvm/versions/node/v22/bin:/usr/bin:/bin'
    )
    const loadLoginShellPath = vi.fn(async () =>
      '/Users/test/.nvm/versions/node/v22/bin:/usr/bin:/bin'
    )

    await expect(resolveProviderCommandEnvironment('claude', {
      HOME: '/Users/test', SHELL: '/bin/zsh', PATH: '/usr/bin:/bin'
    }, { commandExists, loadLoginShellPath })).resolves.toEqual({
      PATH: '/Users/test/.nvm/versions/node/v22/bin:/usr/bin:/bin'
    })
    expect(loadLoginShellPath).toHaveBeenCalledOnce()
  })

  it('does not start a login shell when the inherited PATH already launches the provider', async () => {
    const loadLoginShellPath = vi.fn(async () => '/unexpected')

    await expect(resolveProviderCommandEnvironment('claude', {
      HOME: '/Users/test', SHELL: '/bin/zsh', PATH: '/tools/bin:/usr/bin'
    }, {
      commandExists: async () => true,
      loadLoginShellPath
    })).resolves.toEqual({})
    expect(loadLoginShellPath).not.toHaveBeenCalled()
  })
})
