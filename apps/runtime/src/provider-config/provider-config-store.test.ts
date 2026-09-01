import { mkdtemp, readFile, stat } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { describe, expect, it } from 'vitest'

import { ProviderConfigStore } from './provider-config-store'

describe('ProviderConfigStore', () => {
  it('starts with one usable official configuration for each CLI', async () => {
    const store = new ProviderConfigStore(await mkdtemp(join(tmpdir(), 'matou-provider-config-')))

    const snapshot = await store.snapshot()

    expect(snapshot.activeProviderIds).toEqual({
      'claude-code': 'anthropic-official', codex: 'openai-official'
    })
    expect(snapshot.providers['claude-code'][0]).toMatchObject({
      name: 'Anthropic 官方', hasApiKey: false, builtIn: true
    })
    expect(snapshot.providers.codex[0]).toMatchObject({
      name: 'OpenAI 官方', hasApiKey: false, builtIn: true
    })
  })

  it('persists a custom provider, masks its key in snapshots, and activates it', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-provider-config-'))
    const store = new ProviderConfigStore(root)

    const saved = await store.upsert({
      cli: 'claude-code', name: 'PackyCode', endpoint: 'https://api.packy.example',
      model: 'claude-opus-5', apiKey: 'TOKEN'
    })
    await store.activate('claude-code', saved.id)

    const snapshot = await new ProviderConfigStore(root).snapshot()
    expect(snapshot.activeProviderIds['claude-code']).toBe(saved.id)
    expect(snapshot.providers['claude-code'].find(({ id }) => id === saved.id)).toMatchObject({
      name: 'PackyCode', endpoint: 'https://api.packy.example', hasApiKey: true
    })
    expect(JSON.stringify(snapshot)).not.toContain('TOKEN')
    expect((await stat(join(root, 'provider-config.json'))).mode & 0o777).toBe(0o600)
    expect(await readFile(join(root, 'provider-config.json'), 'utf8')).toContain('TOKEN')
  })

  it('resolves the active provider into the CLI process environment and model', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-provider-config-'))
    const store = new ProviderConfigStore(root)
    const claude = await store.upsert({
      cli: 'claude-code', name: 'Claude proxy', endpoint: 'https://claude.example/',
      model: 'claude-opus-5', apiKey: 'CLAUDE_TOKEN'
    })
    const codex = await store.upsert({
      cli: 'codex', name: 'Codex proxy', endpoint: 'https://codex.example/v1/',
      model: 'gpt-test', apiKey: 'OPENAI_TOKEN'
    })
    await store.activate('claude-code', claude.id)
    await store.activate('codex', codex.id)

    expect(await store.launchConfig('claude-code')).toEqual({
      model: 'claude-opus-5', env: {
        ANTHROPIC_API_KEY: 'CLAUDE_TOKEN', ANTHROPIC_AUTH_TOKEN: 'CLAUDE_TOKEN',
        ANTHROPIC_BASE_URL: 'https://claude.example'
      }
    })
    expect(await store.launchConfig('codex')).toEqual({
      model: 'gpt-test', env: {
        OPENAI_API_KEY: 'OPENAI_TOKEN', OPENAI_BASE_URL: 'https://codex.example/v1'
      }
    })
  })

  it('keeps an existing secret when an edit leaves the key blank and protects active configs', async () => {
    const store = new ProviderConfigStore(await mkdtemp(join(tmpdir(), 'matou-provider-config-')))
    const saved = await store.upsert({
      cli: 'codex', name: 'Proxy', endpoint: 'https://api.example/v1', model: 'gpt-test', apiKey: 'TOKEN'
    })
    await store.upsert({ ...saved, name: 'Renamed', apiKey: '' })
    await store.activate('codex', saved.id)

    expect((await store.launchConfig('codex')).env.OPENAI_API_KEY).toBe('TOKEN')
    await expect(store.delete('codex', saved.id)).rejects.toThrow(/使用中的供应商/)
  })
})
