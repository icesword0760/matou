import { createHash, randomUUID } from 'node:crypto'
import {
  closeSync,
  existsSync,
  openSync,
  readFileSync,
  statSync,
  unlinkSync,
  writeFileSync
} from 'node:fs'
import {
  cp, lstat, mkdir, open, readFile, readdir, rename, rm, writeFile
} from 'node:fs/promises'
import { basename, join, relative } from 'node:path'

import type { RuntimeDatabase } from '../storage/database'
import type { DiagnosticRecorder } from '../observability/diagnostics'

export interface PresetCapabilityManifest {
  schemaVersion: 1
  capabilityId: string
  provider: string
  pluginId: string
  desiredVersion: string
  source: { kind: 'bundled' | 'online'; path: string }
  checksum: string
}

export interface CapabilitySource {
  materialize(manifest: PresetCapabilityManifest, targetDirectory: string): Promise<void>
}

export interface ReconcileResult {
  commandId: string
  installed: string[]
  repaired: string[]
  suppressed: string[]
  failed: string[]
  replayed: boolean
}

interface ActivePointer {
  capabilityId: string
  version: string
  checksum: string
  releasePath: string
  switchedAt: number
}

interface StateRow {
  installed_version: string | null
  desired_version: string
  status: 'pending' | 'installed' | 'suppressed' | 'failed' | 'drifted'
}

export class FileCapabilitySource implements CapabilitySource {
  async materialize(manifest: PresetCapabilityManifest, targetDirectory: string): Promise<void> {
    if (!manifest.source.path) throw new Error('Capability source path is missing')
    await cp(manifest.source.path, targetDirectory, { recursive: true, errorOnExist: true })
  }
}

export class PresetCapabilityRegistry {
  readonly #dataRoot: string
  readonly #installRoot: string
  readonly #database: RuntimeDatabase
  readonly #source: CapabilitySource
  readonly #diagnostics: DiagnosticRecorder | undefined
  readonly #lockPath: string

  constructor(
    dataRoot: string,
    database: RuntimeDatabase,
    source: CapabilitySource,
    diagnostics?: DiagnosticRecorder
  ) {
    this.#dataRoot = dataRoot
    this.#installRoot = join(dataRoot, 'presets')
    this.#database = database
    this.#source = source
    this.#diagnostics = diagnostics
    this.#lockPath = join(dataRoot, 'preset-reconcile.lock')
  }

  async reconcile(manifest: readonly PresetCapabilityManifest[], commandId: string): Promise<ReconcileResult> {
    validateManifest(manifest)
    if (!commandId.trim()) throw new Error('Preset reconcile commandId is required')
    const fingerprint = manifestFingerprint(manifest)
    const replay = this.#database.get<{ manifest_fingerprint: string; result_json: string }>(
      'SELECT manifest_fingerprint, result_json FROM preset_reconcile_commands WHERE command_id = ?', commandId
    )
    if (replay) {
      if (replay.manifest_fingerprint !== fingerprint) throw new Error('Preset commandId was reused for a different manifest')
      return { ...(JSON.parse(replay.result_json) as ReconcileResult), replayed: true }
    }

    await mkdir(this.#dataRoot, { recursive: true, mode: 0o700 })
    const lock = acquireLock(this.#lockPath)
    if (lock === undefined) throw new Error('preset reconciliation is already running')
    try {
      await mkdir(this.#installRoot, { recursive: true, mode: 0o700 })
      const result: ReconcileResult = { commandId, installed: [], repaired: [], suppressed: [], failed: [], replayed: false }
      for (const capability of manifest) {
        if (this.#isSuppressed(capability.capabilityId)) {
          this.#writeState(capability, 'suppressed', this.#active(capability.capabilityId)?.version, undefined)
          result.suppressed.push(capability.capabilityId)
          this.#diagnostics?.record('preset.suppressed', { commandId, capabilityId: capability.capabilityId })
          continue
        }
        const previous = this.#database.get<StateRow>(
          'SELECT installed_version, desired_version, status FROM preset_capability_state WHERE capability_id = ?',
          capability.capabilityId
        )
        const drifted = await this.detectDrift(capability)
        const repairing = drifted && previous?.installed_version === capability.desiredVersion
        if (!drifted && previous?.status === 'installed') continue
        this.#writeState(capability, drifted ? 'drifted' : 'pending', previous?.installed_version ?? undefined, undefined)
        try {
          await this.#install(capability, commandId)
          this.#writeState(capability, 'installed', capability.desiredVersion, undefined)
          ;(repairing ? result.repaired : result.installed).push(capability.capabilityId)
          this.#diagnostics?.record(repairing ? 'preset.repaired' : 'preset.installed', {
            commandId, capabilityId: capability.capabilityId, desiredVersion: capability.desiredVersion
          })
        } catch (error) {
          const active = this.#active(capability.capabilityId)
          this.#writeState(capability, 'failed', active?.version, errorMessage(error))
          result.failed.push(capability.capabilityId)
          this.#diagnostics?.record('preset.failed', {
            commandId, capabilityId: capability.capabilityId, error: errorMessage(error)
          })
        }
      }
      this.#database.run(
        `INSERT INTO preset_reconcile_commands (command_id, manifest_fingerprint, result_json, completed_at)
         VALUES (?, ?, ?, ?)`, commandId, fingerprint, JSON.stringify(result), Date.now()
      )
      return result
    } finally {
      releaseLock(lock, this.#lockPath)
    }
  }

  async detectDrift(manifest: PresetCapabilityManifest): Promise<boolean> {
    const active = this.#active(manifest.capabilityId)
    if (!active || active.version !== manifest.desiredVersion || active.checksum !== manifest.checksum) return true
    try {
      return await checksumDirectory(active.releasePath) !== manifest.checksum
    } catch {
      return true
    }
  }

  async suppress(capabilityId: string, reason: string, now = Date.now()): Promise<void> {
    if (!capabilityId.trim() || !reason.trim()) throw new Error('Suppression capability and reason are required')
    this.#database.run(
      `INSERT INTO preset_capability_suppressions (capability_id, reason, suppressed_at)
       VALUES (?, ?, ?) ON CONFLICT(capability_id) DO UPDATE SET reason = excluded.reason,
       suppressed_at = excluded.suppressed_at`, capabilityId, reason, now
    )
    this.#database.run(
      "UPDATE preset_capability_state SET status = 'suppressed', updated_at = ? WHERE capability_id = ?",
      now, capabilityId
    )
  }

  clearSuppression(capabilityId: string): void {
    this.#database.run('DELETE FROM preset_capability_suppressions WHERE capability_id = ?', capabilityId)
  }

  resolveActivePath(capabilityId: string): string {
    const active = this.#active(capabilityId)
    if (!active) throw new Error(`Capability ${capabilityId} has no active version`)
    return active.releasePath
  }

  async #install(manifest: PresetCapabilityManifest, commandId: string): Promise<void> {
    const capabilityRoot = this.#capabilityRoot(manifest.capabilityId)
    const releases = join(capabilityRoot, 'releases')
    await mkdir(releases, { recursive: true, mode: 0o700 })
    await this.#removeAbandonedTemps(capabilityRoot)
    const temp = join(capabilityRoot, `.tmp-${randomUUID()}`)
    await this.#source.materialize(manifest, temp)
    const actual = await checksumDirectory(temp)
    if (actual !== manifest.checksum) {
      await rm(temp, { recursive: true, force: true })
      throw new Error(`checksum mismatch for ${manifest.capabilityId}`)
    }
    const releasePath = join(
      releases,
      `${safeName(manifest.desiredVersion)}-${manifest.checksum.slice(0, 12)}-${safeName(commandId).slice(0, 24)}-${randomUUID().slice(0, 8)}`
    )
    await rename(temp, releasePath)
    const pointer: ActivePointer = {
      capabilityId: manifest.capabilityId,
      version: manifest.desiredVersion,
      checksum: manifest.checksum,
      releasePath,
      switchedAt: Date.now()
    }
    await atomicWriteJson(join(capabilityRoot, 'active.json'), pointer)
  }

  #writeState(
    manifest: PresetCapabilityManifest,
    status: StateRow['status'],
    installedVersion: string | undefined,
    error: string | undefined
  ): void {
    this.#database.run(
      `INSERT INTO preset_capability_state (
         capability_id, provider, plugin_id, installed_version, desired_version, status,
         source_fingerprint, last_attempt, last_error, updated_at
       ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
       ON CONFLICT(capability_id) DO UPDATE SET provider = excluded.provider,
         plugin_id = excluded.plugin_id, installed_version = excluded.installed_version,
         desired_version = excluded.desired_version, status = excluded.status,
         source_fingerprint = excluded.source_fingerprint, last_attempt = excluded.last_attempt,
         last_error = excluded.last_error, updated_at = excluded.updated_at`,
      manifest.capabilityId, manifest.provider, manifest.pluginId, installedVersion ?? null,
      manifest.desiredVersion, status, sourceFingerprint(manifest), Date.now(), error ?? null, Date.now()
    )
  }

  #isSuppressed(capabilityId: string): boolean {
    return this.#database.get('SELECT 1 FROM preset_capability_suppressions WHERE capability_id = ?', capabilityId) !== undefined
  }

  #active(capabilityId: string): ActivePointer | undefined {
    try {
      const pointer = JSON.parse(readFileSync(join(this.#capabilityRoot(capabilityId), 'active.json'), 'utf8')) as ActivePointer
      if (pointer.capabilityId !== capabilityId || !pathWithin(this.#installRoot, pointer.releasePath)) return undefined
      return pointer
    } catch {
      return undefined
    }
  }

  #capabilityRoot(capabilityId: string): string {
    return join(this.#installRoot, safeName(capabilityId))
  }

  async #removeAbandonedTemps(capabilityRoot: string): Promise<void> {
    try {
      for (const entry of await readdir(capabilityRoot)) {
        if (entry.startsWith('.tmp-')) await rm(join(capabilityRoot, entry), { recursive: true, force: true })
      }
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error
    }
  }
}

export async function checksumDirectory(root: string): Promise<string> {
  const digest = createHash('sha256')
  async function visit(directory: string): Promise<void> {
    const entries = (await readdir(directory, { withFileTypes: true })).sort((a, b) => a.name.localeCompare(b.name))
    for (const entry of entries) {
      const path = join(directory, entry.name)
      const name = relative(root, path).replaceAll('\\', '/')
      if (entry.isSymbolicLink()) throw new Error('Capability sources may not contain symbolic links')
      if (entry.isDirectory()) {
        digest.update(`d\0${name}\0`)
        await visit(path)
      } else if (entry.isFile()) {
        digest.update(`f\0${name}\0`)
        digest.update(await readFile(path))
        digest.update('\0')
      } else {
        throw new Error('Capability sources may contain only files and directories')
      }
    }
  }
  const rootInfo = await lstat(root)
  if (!rootInfo.isDirectory()) throw new Error('Capability source must be a directory')
  await visit(root)
  return digest.digest('hex')
}

function validateManifest(manifest: readonly PresetCapabilityManifest[]): void {
  const identities = new Set<string>()
  for (const item of manifest) {
    if (
      item.schemaVersion !== 1 || !/^[a-z0-9][a-z0-9._-]*$/i.test(item.capabilityId) ||
      !item.provider.trim() || !item.pluginId.trim() || !item.desiredVersion.trim() ||
      !/^[a-f0-9]{64}$/.test(item.checksum) || !['bundled', 'online'].includes(item.source.kind)
    ) throw new Error(`Invalid preset capability manifest: ${item.capabilityId || '<unknown>'}`)
    if (identities.has(item.capabilityId)) throw new Error(`Duplicate preset capability: ${item.capabilityId}`)
    identities.add(item.capabilityId)
  }
}

function manifestFingerprint(manifest: readonly PresetCapabilityManifest[]): string {
  return createHash('sha256').update(JSON.stringify([...manifest].sort((a, b) => a.capabilityId.localeCompare(b.capabilityId)))).digest('hex')
}

function sourceFingerprint(manifest: PresetCapabilityManifest): string {
  return createHash('sha256').update(`${manifest.source.kind}\0${manifest.source.path}`).digest('hex')
}

async function atomicWriteJson(path: string, value: unknown): Promise<void> {
  await mkdir(join(path, '..'), { recursive: true })
  const temporary = `${path}.tmp-${randomUUID()}`
  await writeFile(temporary, `${JSON.stringify(value)}\n`, { mode: 0o600 })
  const handle = await open(temporary, 'r')
  await handle.sync(); await handle.close()
  await rename(temporary, path)
  if (process.platform !== 'win32') {
    const directory = await open(join(path, '..'), 'r')
    await directory.sync(); await directory.close()
  }
}

function acquireLock(path: string): number | undefined {
  try {
    const fd = openSync(path, 'wx', 0o600)
    writeFileSync(fd, JSON.stringify({ pid: process.pid, createdAt: Date.now() }))
    return fd
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== 'EEXIST') throw error
    if (isStaleLock(path)) {
      try { unlinkSync(path) } catch { return undefined }
      return acquireLock(path)
    }
    return undefined
  }
}

function isStaleLock(path: string): boolean {
  try {
    const record = JSON.parse(readFileSync(path, 'utf8')) as { pid?: number; createdAt?: number }
    const old = Date.now() - Number(record.createdAt) > 5 * 60_000
    return old || typeof record.pid !== 'number' || !isLive(record.pid)
  } catch {
    return Date.now() - statSync(path).mtimeMs > 5 * 60_000
  }
}

function isLive(pid: number): boolean {
  try { process.kill(pid, 0); return true } catch (error) {
    return (error as NodeJS.ErrnoException).code === 'EPERM'
  }
}

function releaseLock(fd: number, path: string): void {
  try { closeSync(fd) } finally { if (existsSync(path)) unlinkSync(path) }
}

function safeName(value: string): string {
  return value.replace(/[^a-z0-9._-]/gi, '_')
}

function pathWithin(root: string, path: string): boolean {
  const result = relative(root, path)
  return result !== '..' && !result.startsWith('../') && !result.startsWith('..\\')
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message.slice(0, 2_000) : String(error).slice(0, 2_000)
}
