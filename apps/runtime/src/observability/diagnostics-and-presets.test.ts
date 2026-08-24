import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { RuntimeDatabase } from '../storage/database'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'
import { DiagnosticRecorder, RuntimeMetrics } from './diagnostics'
import {
  FileCapabilitySource,
  PresetCapabilityRegistry,
  checksumDirectory,
  type PresetCapabilityManifest
} from '../presets/preset-capability-registry'

let root: string
let database: RuntimeDatabase

beforeEach(async () => {
  root = await mkdtemp(join(tmpdir(), 'matou-presets-'))
  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
})

afterEach(() => database.close())

describe('diagnostics', () => {
  it('records required correlation fields and metrics without raw terminal payloads', () => {
    const recorder = new DiagnosticRecorder({ capacity: 2 })
    recorder.record('journal.append', {
      runtimeGeneration: 'gen', commandId: 'cmd', eventId: 'evt', sessionId: 'session', runId: 'run',
      journalLag: 2, outboxLag: 3, subscriberLag: 4, output: 'TOP SECRET', data: Buffer.from('SECRET')
    })
    recorder.record('migration.completed', { migrationStatus: 'ok' })
    recorder.record('recovery.completed', { recoveryResult: 'checkpoint', repairAction: 'tail-truncated' })
    const serialized = JSON.stringify(recorder.snapshot())

    expect(serialized).not.toContain('TOP SECRET')
    expect(serialized).not.toContain('SECRET')
    expect(recorder.snapshot()).toHaveLength(2)

    const metrics = new RuntimeMetrics()
    metrics.increment('commands.total')
    metrics.setGauge('journal.lag', 7)
    expect(metrics.snapshot()).toEqual({ counters: { 'commands.total': 1 }, gauges: { 'journal.lag': 7 } })
  })

  it('includes content samples only when explicitly selected for an export bundle', () => {
    const recorder = new DiagnosticRecorder()
    recorder.record('terminal', { sessionId: 'session-1', output: 'hidden' })
    expect(JSON.stringify(recorder.exportBundle())).not.toContain('hidden')
    expect(JSON.stringify(recorder.exportBundle([{ label: 'selected', content: 'visible' }]))).toContain('visible')
  })
})

describe('PresetCapabilityRegistry', () => {
  it('installs an offline seed after checksum verification and is command-idempotent', async () => {
    const seed = join(root, 'seed-v1')
    await mkdir(seed)
    await writeFile(join(seed, 'plugin.txt'), 'version one')
    const manifest = await makeManifest(seed, '1.0.0')
    const registry = createRegistry()

    const first = await registry.reconcile([manifest], 'command-1')
    const replay = await registry.reconcile([manifest], 'command-1')

    expect(first.installed).toEqual(['code-review'])
    expect(replay).toEqual({ ...first, replayed: true })
    expect(await readFile(registry.resolveActivePath('code-review') + '/plugin.txt', 'utf8')).toBe('version one')
  })

  it('detects drift and repairs it from the declared source', async () => {
    const seed = join(root, 'seed-v1')
    await mkdir(seed)
    await writeFile(join(seed, 'plugin.txt'), 'version one')
    const manifest = await makeManifest(seed, '1.0.0')
    const registry = createRegistry()
    await registry.reconcile([manifest], 'command-1')
    await writeFile(join(registry.resolveActivePath('code-review'), 'plugin.txt'), 'tampered')

    expect(await registry.detectDrift(manifest)).toBe(true)
    const repaired = await registry.reconcile([manifest], 'command-2')
    expect(repaired.repaired).toEqual(['code-review'])
    expect(await readFile(join(registry.resolveActivePath('code-review'), 'plugin.txt'), 'utf8')).toBe('version one')
  })

  it('preserves the previous version when an upgrade fails validation', async () => {
    const seedV1 = join(root, 'seed-v1')
    const seedV2 = join(root, 'seed-v2')
    await mkdir(seedV1); await mkdir(seedV2)
    await writeFile(join(seedV1, 'plugin.txt'), 'version one')
    await writeFile(join(seedV2, 'plugin.txt'), 'version two')
    const registry = createRegistry()
    await registry.reconcile([await makeManifest(seedV1, '1.0.0')], 'command-1')
    const invalid = { ...(await makeManifest(seedV2, '2.0.0')), checksum: '0'.repeat(64) }

    const result = await registry.reconcile([invalid], 'command-2')

    expect(result.failed).toEqual(['code-review'])
    expect(await readFile(join(registry.resolveActivePath('code-review'), 'plugin.txt'), 'utf8')).toBe('version one')
  })

  it('respects user-uninstall suppression across upgrades', async () => {
    const seed = join(root, 'seed-v1')
    await mkdir(seed); await writeFile(join(seed, 'plugin.txt'), 'version one')
    const registry = createRegistry()
    const v1 = await makeManifest(seed, '1.0.0')
    await registry.reconcile([v1], 'command-1')
    await registry.suppress('code-review', 'user-uninstalled', 2)

    const result = await registry.reconcile([{ ...v1, desiredVersion: '2.0.0' }], 'command-2')

    expect(result.suppressed).toEqual(['code-review'])
    expect(database.get<{ status: string }>('SELECT status FROM preset_capability_state WHERE capability_id = ?', 'code-review')?.status).toBe('suppressed')
  })

  it('uses a cross-process lock to prevent concurrent reconciliation', async () => {
    const seed = join(root, 'seed-v1')
    await mkdir(seed); await writeFile(join(seed, 'plugin.txt'), 'version one')
    const manifest = await makeManifest(seed, '1.0.0')
    let release!: () => void
    const gate = new Promise<void>((resolve) => { release = resolve })
    const slowSource = { materialize: async (_manifest: PresetCapabilityManifest, target: string) => {
      await gate
      await new FileCapabilitySource().materialize(manifest, target)
    } }
    const firstRegistry = new PresetCapabilityRegistry(root, database, slowSource)
    const secondRegistry = new PresetCapabilityRegistry(root, database, new FileCapabilitySource())
    const first = firstRegistry.reconcile([manifest], 'command-1')
    await new Promise((resolve) => setTimeout(resolve, 20))

    await expect(secondRegistry.reconcile([manifest], 'command-2')).rejects.toThrow('reconciliation is already running')
    release()
    await first
  })
})

function createRegistry() {
  return new PresetCapabilityRegistry(root, database, new FileCapabilitySource())
}

async function makeManifest(sourcePath: string, desiredVersion: string): Promise<PresetCapabilityManifest> {
  return {
    schemaVersion: 1, capabilityId: 'code-review', provider: 'claude-code',
    pluginId: 'code-review@official', desiredVersion, source: { kind: 'bundled', path: sourcePath },
    checksum: await checksumDirectory(sourcePath)
  }
}
