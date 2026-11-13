import { describe, expect, it, vi } from 'vitest'

import { resolveProviderCommandEnvironment } from './provider-command-environment'
import { loadLoginShellProxyEnvironment } from './provider-command-environment'
import { mkdir, mkdtemp, readFile, rm, writeFile, chmod } from 'node:fs/promises'
import { resolve, join } from 'node:path'

describe('provider command environment', () => {
  it('shares a startup proxy prefetch with a concurrent launch and subsequent cached reads', async () => {
    const base = resolve(import.meta.dirname, '../../../../output/model-switch-rework')
    await mkdir(base, { recursive: true })
    const root = await mkdtemp(join(base, 'proxy-cache-')), shell = join(root, 'shell'), counter = join(root, 'calls')
    await writeFile(shell, `#!/bin/sh\nprintf x >> '${counter}'\nsleep 0.1\nprintf '__MATOU_LOGIN_PROXIES__\\000http://127.0.0.1:7890\\000\\000\\000\\000\\000\\000\\000\\000'\n`)
    await chmod(shell, 0o700)
    try {
      const env = { SHELL: shell, HOME: root, PATH: '/bin:/usr/bin' }
      const prefetched = loadLoginShellProxyEnvironment(env)
      const launch = loadLoginShellProxyEnvironment(env)
      expect(launch).toBe(prefetched)
      expect(await launch).toEqual({ HTTPS_PROXY: 'http://127.0.0.1:7890' })
      expect(await loadLoginShellProxyEnvironment(env)).toEqual(await prefetched)
      expect(await readFile(counter, 'utf8')).toBe('x')
    } finally { await rm(root, { recursive: true, force: true }) }
  })
  it.each(['/missing/provider', './missing/provider'])('does not probe the login-shell PATH for an explicit executable path: %s', async command => {
    const commandExists = vi.fn(async () => false)
    const loadLoginShellPath = vi.fn(async () => '/unrelated')
    await expect(resolveProviderCommandEnvironment(command, { PATH: '/usr/bin' }, { commandExists, loadLoginShellPath })).resolves.toEqual({})
    expect(commandExists).not.toHaveBeenCalled()
    expect(loadLoginShellPath).not.toHaveBeenCalled()
  })
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
