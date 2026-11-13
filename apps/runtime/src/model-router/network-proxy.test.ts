import { describe, expect, it, vi } from 'vitest'
import { mkdir, mkdtemp, readFile, writeFile } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { loadLoginShellProxyEnvironment } from '../session/provider-command-environment'
import { bypassesProvider, parseSystemProxy, resolveRouterProxy } from './network-proxy'

const output = resolve(import.meta.dirname, '../../../../output/model-router-stage2-rework')
describe('router outbound proxy', () => {
  it.runIf(process.platform === 'darwin')('reads interactive login shell exports once, isolates startup output, and returns only the proxy whitelist', async () => {
    await mkdir(output, { recursive: true })
    const home = await mkdtemp(join(output, 'shell-'))
    await writeFile(join(home, '.zshrc'), [
      'printf x >> "$HOME/reads"',
      "export https_proxy='http://127.0.0.1:17891'",
      "export no_proxy='localhost,127.0.0.1'",
      "export ANTHROPIC_API_KEY='never-forward-this-token'",
      "printf 'startup diagnostic\\n'"
    ].join('\n'))
    const environment = { SHELL: '/bin/zsh', HOME: home, ZDOTDIR: home, PATH: '/usr/bin:/bin' }
    const values = await Promise.all(Array.from({ length: 3 }, () => loadLoginShellProxyEnvironment(environment)))
    expect(values).toEqual(Array(3).fill({ https_proxy: 'http://127.0.0.1:17891', no_proxy: 'localhost,127.0.0.1' }))
    expect(await loadLoginShellProxyEnvironment(environment)).toEqual(values[0])
    expect(await readFile(join(home, 'reads'), 'utf8')).toBe('x')
  })

  it('selects HTTPS then ALL then HTTP, preferring uppercase within each pair, before system fallback', async () => {
    const readSystem = vi.fn(async () => '<dictionary> {\n HTTPSEnable : 1\n HTTPSProxy : system.invalid\n HTTPSPort : 7890\n}')
    const shell: Record<string, string> = {
      HTTPS_PROXY: 'http://upper.invalid:1', https_proxy: 'http://lower.invalid:2',
      ALL_PROXY: 'socks5://upper.invalid:3', all_proxy: 'socks5://lower.invalid:4',
      HTTP_PROXY: 'http://upper.invalid:5', http_proxy: 'http://lower.invalid:6'
    }
    for (const name of Object.keys(shell)) {
      const current = shell[name]
      await expect(resolveRouterProxy({}, { loadShell: async () => shell, readSystem, platform: 'darwin' })).resolves.toMatchObject({ source: 'shell', proxyUrl: current })
      delete shell[name]
    }
    expect(readSystem).not.toHaveBeenCalled()
    await expect(resolveRouterProxy({}, { loadShell: async () => ({}), readSystem, platform: 'darwin' })).resolves.toEqual({ source: 'system', proxyUrl: 'http://system.invalid:7890' })
  })

  it('uses root system HTTPS, then SOCKS; ignores disabled and interface-scoped settings', () => {
    expect(parseSystemProxy(`<dictionary> {
      HTTPSEnable : 1
      HTTPSProxy : 127.0.0.1
      HTTPSPort : 7890
      SOCKSEnable : 1
      SOCKSProxy : localhost
      SOCKSPort : 1080
      __SCOPED__ : <dictionary> { HTTPSEnable : 0 }
    }`)).toBe('http://127.0.0.1:7890')
    expect(parseSystemProxy(`<dictionary> {
      HTTPSEnable : 0
      SOCKSEnable : 1
      SOCKSProxy : ::1
      SOCKSPort : 1080
    }`)).toBe('socks5://[::1]:1080')
    expect(parseSystemProxy(`<dictionary> {
      HTTPSEnable : 0
      SOCKSEnable : 0
      __SCOPED__ : <dictionary> {
        HTTPSEnable : 1
        HTTPSProxy : wrong.invalid
        HTTPSPort : 7890
      }
    }`)).toBeUndefined()
  })

  it.each(['chatgpt.com', 'auth.openai.com', '*.googleapis.com', 'accounts.google.com', '*', '.openai.com', 'oauth2.googleapis.com', 'chatgpt.com:443'])(
    'omits the engine proxy for provider NO_PROXY=%s, including system fallback', async value => {
      expect(bypassesProvider(value)).toBe(true)
      const readSystem = vi.fn(async () => '')
      await expect(resolveRouterProxy({}, { loadShell: async () => ({ HTTPS_PROXY: 'http://unused.invalid:7890', NO_PROXY: value }), readSystem })).resolves.toEqual({ source: 'direct', noProxy: value, providerBypass: true })
      expect(readSystem).not.toHaveBeenCalled()
    }
  )
  it('ignores unrelated NO_PROXY per decision 7; falls back to direct when no proxy exists', async () => {
    for (const value of ['localhost,127.0.0.1', 'notchatgpt.com', 'api.openai.com', 'chatgpt.com:8443']) expect(bypassesProvider(value)).toBe(false)
    await expect(resolveRouterProxy({}, { loadShell: async () => ({ https_proxy: 'http://proxy.invalid:7890', no_proxy: 'localhost' }) })).resolves.toEqual({ source: 'shell', proxyUrl: 'http://proxy.invalid:7890', noProxy: 'localhost' })
    await expect(resolveRouterProxy({}, { loadShell: async () => ({}), readSystem: async () => '<dictionary> {\nHTTPSEnable : 0\n}', platform: 'darwin' })).resolves.toEqual({ source: 'direct' })
  })
  it('rejects an invalid explicit proxy without exposing its credentials or silently using direct', async () => {
    await expect(resolveRouterProxy({}, { loadShell: async () => ({ HTTPS_PROXY: 'ftp://user:private@proxy.invalid' }) })).rejects.toThrow('Network proxy configuration is invalid')
  })
})
