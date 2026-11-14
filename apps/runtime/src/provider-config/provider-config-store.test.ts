import { mkdtemp, readFile, stat } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { describe, expect, it, vi } from 'vitest'

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

  it('resolves a Session-bound provider by stable configuration identity after the global default changes', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-provider-config-'))
    const store = new ProviderConfigStore(root)
    const bound = await store.upsert({
      cli: 'claude-code', name: 'Bound gateway', endpoint: 'https://bound.example/',
      model: 'bound-default-model', apiKey: 'BOUND_TOKEN'
    })
    const laterDefault = await store.upsert({
      cli: 'claude-code', name: 'Later gateway', endpoint: 'https://later.example/',
      model: 'later-default-model', apiKey: 'LATER_TOKEN'
    })
    await store.activate('claude-code', laterDefault.id)

    expect(await store.launchSelection('claude-code', bound.id)).toEqual({
      providerConfigId: bound.id,
      model: 'bound-default-model',
      env: {
        ANTHROPIC_API_KEY: 'BOUND_TOKEN', ANTHROPIC_AUTH_TOKEN: 'BOUND_TOKEN',
        ANTHROPIC_BASE_URL: 'https://bound.example'
      }
    })
  })

  it('keeps an existing secret when an edit leaves the key blank and protects active configs', async () => {
    const store = new ProviderConfigStore(await mkdtemp(join(tmpdir(), 'matou-provider-config-')))
    const saved = await store.upsert({
      cli: 'codex', name: 'Proxy', endpoint: 'https://api.example/v1', model: 'gpt-test', apiKey: 'TOKEN'
    })
    await store.upsert({ ...saved, kind: 'api-key', name: 'Renamed', apiKey: '' })
    await store.activate('codex', saved.id)

    expect((await store.launchConfig('codex')).env.OPENAI_API_KEY).toBe('TOKEN')
    await expect(store.delete('codex', saved.id)).rejects.toThrow(/使用中的供应商/)
  })

  it('requires a new secret for a changed destination in both edit and discovery', async () => {
    const store = new ProviderConfigStore(await mkdtemp(join(tmpdir(), 'matou-provider-config-')))
    const saved = await store.upsert({ cli: 'claude-code', name: 'Private gateway', endpoint: 'https://original.example/v1', model: 'test', apiKey: 'PRIVATE_TOKEN' })
    await expect(store.upsert({ ...saved, kind: 'api-key', endpoint: 'https://other.example/v1', apiKey: '' })).rejects.toThrow('Endpoint changed')
    expect(await store.discoverModels({ cli: 'claude-code', providerId: saved.id, endpoint: 'https://other.example/v1' })).toEqual({ error: 'key-required' })
    expect((await store.launchSelection('claude-code', saved.id)).env.ANTHROPIC_BASE_URL).toBe('https://original.example/v1')
  })

  it('roundtrips, preserves and explicitly clears model role mappings', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-provider-config-'))
    const store = new ProviderConfigStore(root)
    const saved = await store.upsert({ cli: 'claude-code', name: 'Mapped', endpoint: 'https://mapping.example', model: 'test', apiKey: 'TOKEN', modelMapping: { opus: 'big', haiku: 'small' } })
    await store.upsert({ cli: 'claude-code', id: saved.id, name: 'Renamed', endpoint: saved.endpoint, model: saved.model })
    expect((await new ProviderConfigStore(root).launchSelection('claude-code', saved.id)).env).toMatchObject({ ANTHROPIC_DEFAULT_OPUS_MODEL: 'big', ANTHROPIC_DEFAULT_HAIKU_MODEL: 'small' })
    await store.upsert({ ...saved, kind: 'api-key', modelMapping: { opus: '', haiku: '' } })
    expect((await store.launchSelection('claude-code', saved.id)).env).not.toHaveProperty('ANTHROPIC_DEFAULT_OPUS_MODEL')
  })

  it('exposes discovered models without changing the saved configuration until save', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-provider-config-'))
    const store = new ProviderConfigStore(root)
    const input = { cli: 'claude-code' as const, endpoint: 'https://api.example/v1', apiKey: 'TOKEN' }
    const saved = await store.upsert({ ...input, name: 'Provider', model: 'test' })
    const original = await readFile(join(root, 'provider-config.json'), 'utf8')
    const request = vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response(JSON.stringify({ data: [{ id: 'test', name: 'Test Model', context_window: 1_048_576 }] })))
    try {
      const discovered = await store.discoverModels({ cli: input.cli, endpoint: input.endpoint, providerId: saved.id })
      expect(discovered).toEqual({ models: ['test'], modelMetadata: [{ id: 'test', displayName: 'Test Model', contextWindow: 1_048_576 }] })
      expect(await readFile(join(root, 'provider-config.json'), 'utf8')).toEqual(original)
    } finally {
      request.mockRestore()
    }
  })

  it('whitelists model metadata and preserves it on reopening and same-service edits', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-provider-config-'))
    const store = new ProviderConfigStore(root)
    const input = { cli: 'claude-code' as const, name: 'Provider', endpoint: 'https://api.example/v1', model: 'test', apiKey: 'TOKEN' }
    const metadata = { id: 'test', displayName: 'Test Model', contextWindow: 1_048_576, maxOutputTokens: 393_216, supportedEfforts: ['high' as const] }
    const rawModel = { ...metadata, apiKey: 'MODEL_SECRET', raw: 'RAW_UPSTREAM_DATA', maxContextWindow: 0 }
    const saved = await store.upsert({ ...input, modelMetadata: [rawModel] })
    const reopened = new ProviderConfigStore(root)
    expect((await reopened.snapshot()).providers['claude-code'].find(provider => provider.id === saved.id)?.modelMetadata).toEqual([metadata])
    const renamed = await reopened.upsert({ ...input, id: saved.id, name: 'Renamed', apiKey: '' })
    expect(renamed.modelMetadata).toEqual([metadata])
    expect(JSON.stringify(renamed)).not.toContain('TOKEN')
    expect(await readFile(join(root, 'provider-config.json'), 'utf8')).not.toMatch(/MODEL_SECRET|RAW_UPSTREAM_DATA/)
    const cleared = await reopened.upsert({ ...input, id: saved.id, modelMetadata: [] })
    expect(cleared.modelMetadata).toEqual([])
  })

  it.each([
    { endpoint: 'https://other.example/v1' },
    { apiKey: 'REPLACED_TOKEN' }
  ])('invalidates omitted model metadata after a service credential or address change', async change => {
    const store = new ProviderConfigStore(await mkdtemp(join(tmpdir(), 'matou-provider-config-')))
    const input = { cli: 'claude-code' as const, name: 'Provider', endpoint: 'https://api.example/v1', model: 'test', apiKey: 'TOKEN' }
    const saved = await store.upsert({ ...input, modelMetadata: [{ id: 'test', contextWindow: 1_048_576 }] })
    const updated = await store.upsert({ ...input, ...change, id: saved.id })
    expect(updated.modelMetadata).toBeUndefined()
    expect((await store.snapshot()).providers['claude-code'].find(provider => provider.id === saved.id)?.modelMetadata).toBeUndefined()
  })
})
