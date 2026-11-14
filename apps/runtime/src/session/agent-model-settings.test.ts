import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import { RuntimeDatabase } from '../storage/database'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'
import { ProviderConfigStore } from '../provider-config/provider-config-store'
import { AgentModelSettings } from './agent-model-settings'
vi.mock('./native-model-catalog', () => ({
  nativeClaudeModels: async () => [{ id: 'default', contextWindow: 1_000_000, maxContextWindow: 1_000_000 }, { id: 'claude-test', contextWindow: 200000, maxContextWindow: 1_000_000 }],
  nativeCodexModels: async () => [{ id: 'gpt-test', contextWindow: 272000, maxContextWindow: 872000, supportedEfforts: ['low', 'high'] }],
  nativeCodexDefaultModel: async () => 'gpt-test'
}))
let root: string, db: RuntimeDatabase, store: ProviderConfigStore, settings: AgentModelSettings
beforeEach(async () => {
  root = await mkdtemp(join(tmpdir(), 'matou-model-settings-')); db = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(db, FOUNDATION_MIGRATIONS).migrate(); store = new ProviderConfigStore(root); settings = new AgentModelSettings(db, store)
})
afterEach(async () => { db.close(); await rm(root, { recursive: true, force: true }) })
it('persists the independent policy without activating a different main provider', async () => {
  const before = await store.snapshot(), target = before.providers['claude-code'][0]!
  const preferences = { contextWindows: { [target.id]: 'maximum' }, compaction: { mode: 'custom', providerId: target.id, model: 'claude-test', effort: 'high' } } as const
  await settings.set(preferences)
  expect(new AgentModelSettings(db, store).preferences()).toEqual(preferences)
  expect((await store.snapshot()).activeProviderIds).toEqual(before.activeProviderIds)
  await expect(settings.set({ ...preferences, compaction: { ...preferences.compaction, providerId: 'deleted' } })).rejects.toThrow('unavailable')
  expect(settings.preferences()).toEqual(preferences)
})
it('shows native default, exact fixed and extended windows and resolves the chosen Codex default', async () => {
  const snapshot = await settings.snapshot()
  expect(snapshot.preferences.contextWindows).toMatchObject({ 'openai-official': '512k', 'anthropic-official': 'default' })
  expect(snapshot.providers.find(p => p.cli === 'codex')!.models[0]).toMatchObject({ defaultTokens: 272000, fixedTokens: 512000, maximumTokens: 872000 })
  const provider = (await store.snapshot()).providers.codex[0]!
  expect(await settings.launchWindow(provider)).toBe(512000)
  await settings.set({ contextWindows: { 'openai-official': 'default' }, compaction: { mode: 'follow', effort: 'follow' } })
  expect(await settings.launchWindow(provider)).toBeUndefined()
  await settings.set({ contextWindows: { 'openai-official': 'maximum' }, compaction: { mode: 'follow', effort: 'follow' } })
  expect(await settings.launchWindow(provider)).toBe(872000)
})
it('applies 512K to all models, preserves unknown limits, and uses the safe common window for role switching', async () => {
  const unknown = await store.upsert({ cli: 'claude-code', name: 'API', endpoint: 'https://api.example', model: 'private-model', apiKey: 'test' })
  await settings.set({ contextWindows: { [unknown.id]: '512k' }, compaction: { mode: 'follow', effort: 'follow' } })
  expect(await settings.launchWindow(unknown)).toBe(512000)
  expect((await settings.snapshot()).providers.find(p => p.id === unknown.id)!.models[0]).toEqual({ id: 'private-model', name: 'private-model', fixedTokens: 512000 })
  await settings.set({ contextWindows: { 'openai-official': 'maximum' }, compaction: { mode: 'follow', effort: 'follow' } })
  expect(await settings.launchWindow(unknown)).toBeUndefined()
  const mapped = await store.upsert({ cli: 'claude-code', name: 'API', endpoint: 'https://api.example', model: 'claude-test', apiKey: 'test', modelMapping: { opus: 'claude-test', sonnet: 'unknown' } })
  expect(await settings.launchWindow(mapped)).toBeUndefined()
})
it('rejects malformed input before changing the saved policy', async () => {
  await expect(settings.set({ contextWindows: { test: '512k' }, compaction: { mode: 'custom', model: 'bad\nmodel', effort: 'high' } })).rejects.toThrow()
  await expect(settings.set({ contextWindows: { test: 'guess' }, compaction: { mode: 'follow', effort: 'high' } })).rejects.toThrow()
  expect(settings.preferences().contextWindows).toEqual({})
})

it('keeps provider window choices independent across saves and launches', async () => {
  const snapshot = await settings.snapshot()
  const providers = (await store.snapshot()).providers
  await settings.set({ ...snapshot.preferences, contextWindows: { ...snapshot.preferences.contextWindows, 'anthropic-official': 'maximum', 'openai-official': '512k' } })
  expect(await settings.launchWindow(providers['claude-code'][0])).toBe(1000000)
  expect(await settings.launchWindow(providers.codex[0])).toBe(512000)
  await settings.set({ ...settings.preferences(), contextWindows: { ...settings.preferences().contextWindows, 'openai-official': 'default' } })
  expect(await settings.launchWindow(providers['claude-code'][0])).toBe(1000000)
  expect(await settings.launchWindow(providers.codex[0])).toBeUndefined()
})

it('shows saved API model names and uses the smallest known window across the default and mapped Claude models', async () => {
  const provider = await store.upsert({
    cli: 'claude-code', name: 'API', endpoint: 'https://api.example', model: 'main', apiKey: 'test',
    modelMapping: { opus: 'large', sonnet: '', haiku: 'small' },
    modelMetadata: [
      { id: 'main', displayName: 'Main Model', contextWindow: 90_000, maxContextWindow: 120_000, supportedEfforts: ['low'] },
      { id: 'large', contextWindow: 1_048_576 },
      { id: 'small', contextWindow: 200_000 },
      { id: 'output-only', maxOutputTokens: 8192 }
    ]
  })
  expect(await settings.launchWindow(provider)).toBeUndefined()
  const snapshot = await settings.set({ contextWindows: { [provider.id]: 'maximum' }, compaction: { mode: 'follow', effort: 'follow' } })
  expect(snapshot.providers.find(p => p.id === provider.id)?.models).toContainEqual({
    id: 'main', name: 'Main Model', defaultTokens: 90_000, maximumTokens: 120_000, fixedTokens: 512_000, supportedEfforts: ['low']
  })
  expect(await settings.launchWindow(provider)).toBe(120_000)
  expect(await settings.launchWindow(provider, 'matou-api/opus')).toBe(200_000)
  expect(await settings.launchWindow(provider, 'output-only')).toBeUndefined()
  const updated = await store.upsert({ ...provider, kind: 'api-key', modelMapping: { opus: 'large', haiku: 'unlisted' } })
  expect((await settings.snapshot()).providers.find(p => p.id === provider.id)?.models).toContainEqual({
    id: 'unlisted', name: 'unlisted', fixedTokens: 512_000
  })
  expect(await settings.launchWindow(updated)).toBeUndefined()
})

it('uses the custom Codex API catalog when its model ID matches a native model', async () => {
  const provider = await store.upsert({
    cli: 'codex', name: 'API', endpoint: 'https://api.example', model: 'gpt-test', apiKey: 'test',
    modelMetadata: [{ id: 'gpt-test', displayName: 'Gateway Model', contextWindow: 1_048_576 }]
  })
  const snapshot = await settings.set({ contextWindows: { [provider.id]: 'maximum' }, compaction: { mode: 'follow', effort: 'follow' } })
  expect(snapshot.providers.find(p => p.id === provider.id)?.models).toEqual([
    { id: 'gpt-test', name: 'Gateway Model', defaultTokens: 1_048_576, fixedTokens: 512_000 }
  ])
  expect(await settings.launchWindow(provider)).toBe(1_048_576)
  expect(await settings.launchWindow(provider, 'unknown')).toBeUndefined()
})

it('keeps native Codex metadata out of a built-in configuration edited to use a gateway', async () => {
  const provider = await store.upsert({
    id: 'openai-official', cli: 'codex', name: 'Gateway', endpoint: 'https://api.example', model: 'gpt-test'
  })
  const snapshot = await settings.set({ contextWindows: { [provider.id]: 'maximum' }, compaction: { mode: 'follow', effort: 'follow' } })
  expect(snapshot.providers.find(p => p.id === provider.id)?.models).toEqual([{ id: 'gpt-test', name: 'gpt-test', fixedTokens: 512_000 }])
  expect(await settings.launchWindow(provider)).toBeUndefined()
})
