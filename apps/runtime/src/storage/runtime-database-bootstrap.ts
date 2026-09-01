import { createHash, randomUUID } from 'node:crypto'
import {
  accessSync,
  closeSync,
  constants,
  existsSync,
  openSync,
  readFileSync,
  statSync
} from 'node:fs'
import { link, open, readFile, rename, rm } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'

import {
  RuntimeDatabase,
  type RuntimeDatabaseOwnership
} from './database'
import {
  isDatabaseOwnershipRecoveryError,
  withDatabaseRecoveryActionFence,
  type DatabaseOwnershipRecoveryIssue
} from './database-owner'
import {
  DatabaseBackupService,
  type DatabaseBackupDescriptor
} from './database-backup-service'
import { MigrationRunner, type Migration } from './migration-runner'

export interface RuntimeDatabaseRecoveryError {
  code: 'BACKUP_LIST_FAILED' | 'RECOVERY_MARKER_FAILED' | 'RECOVERY_MOVE_FAILED'
  message: string
  retryable: true
}

export type RuntimeDatabaseBootstrapResult =
  | { kind: 'writable'; database: RuntimeDatabase; dataRoot: string }
  | {
      kind: 'read-only'
      database: RuntimeDatabase
      dataRoot: string
      reason: 'filesystem-read-only' | 'newer-schema'
    }
  | {
      kind: 'recovery-required'
      recoveryId: string
      reason: 'physical-corruption' | 'wal-recovery-required' | 'ownership-recovery-required'
      durableDatabasePath: string
      quarantinedPath: string
      markerPath: string
      backups: DatabaseBackupDescriptor[]
      ownershipIssue?: DatabaseOwnershipRecoveryIssue
      backupListError?: RuntimeDatabaseRecoveryError
      markerError?: RuntimeDatabaseRecoveryError
      moveError?: RuntimeDatabaseRecoveryError
    }

interface RuntimeDatabaseRecoveryMarker {
  version: 1
  state: 'required' | 'resolved'
  recoveryId: string
  reason: 'physical-corruption' | 'wal-recovery-required' | 'ownership-recovery-required'
  durableDatabasePath: string
  quarantinedPath: string
  markerPath: string
  createdAt: number
  ownershipIssue?: DatabaseOwnershipRecoveryIssue
}

export interface RuntimeRecoveryMarkerFinalizationObserver {
  beforeFileSync?(): void
  beforePublish?(): void
  afterPublish?(): void
  beforeDirectorySync?(): void
}

export interface RuntimeDatabaseBootstrapObserver {
  onDatabaseOpened?(
    database: RuntimeDatabase,
    dataRoot: string,
    backups: DatabaseBackupService
  ): void
  onDatabaseClosed?(database: RuntimeDatabase): void
  onRecoveryMarkerPublished?(
    marker: RuntimeDatabaseRecoveryMarker
  ): void | Promise<void>
  onRecoveryActionFenced?(): void | Promise<void>
  onRecoveryGenerationClaimPublished?(claim: {
    claimPath: string
    recoveryId: string
  }): void | Promise<void>
  isShutdownRequested?(): boolean
}

export async function openRecoverableRuntimeDatabaseWithOwnership(
  dataRoot: string,
  migrations: readonly Migration[],
  ownership: RuntimeDatabaseOwnership,
  observer: RuntimeDatabaseBootstrapObserver = {}
): Promise<Extract<RuntimeDatabaseBootstrapResult, { kind: 'writable' }>> {
  const databasePath = join(dataRoot, 'matou.sqlite')
  const backups = new DatabaseBackupService(dataRoot)
  let database: RuntimeDatabase | undefined
  try {
    assertWalBundleReady(databasePath)
    database = ownership.openWritable()
    observer.onDatabaseOpened?.(database, dataRoot, backups)
    assertFullIntegrity(database)
    await new MigrationRunner(database, migrations, backups).migrate()
    return { kind: 'writable', database, dataRoot }
  } catch (error) {
    ownership.release()
    if (database) closeObservedDatabase(database, observer)
    throw error
  }
}

export async function openRecoverableRuntimeDatabase(
  dataRoot: string,
  migrations: readonly Migration[],
  observer: RuntimeDatabaseBootstrapObserver = {}
): Promise<RuntimeDatabaseBootstrapResult> {
  const databasePath = join(dataRoot, 'matou.sqlite')
  const backups = new DatabaseBackupService(dataRoot)
  const initialRecoveryShape = readRecoveryMarkerShape(databasePath)
  const pendingClaim = readRecoveryGenerationClaimForSource(databasePath, initialRecoveryShape)
  if (pendingClaim) {
    try {
      await replaceRecoveryMarker(pendingClaim.marker.markerPath, pendingClaim.marker)
      return recoveryResult(requireCanonicalRecoveryGeneration(pendingClaim.marker), backups)
    } catch (error) {
      return recoveryResult(pendingClaim.marker, backups, {
        markerError: recoveryError('RECOVERY_MARKER_FAILED', error)
      })
    }
  }
  const persistedRecovery = initialRecoveryShape?.kind === 'legacy'
    ? activeRecoveryMarker(await upgradeLegacyRecoveryMarker(databasePath, observer))
    : activeRecoveryMarker(initialRecoveryShape?.marker)
  if (persistedRecovery) return recoveryResult(persistedRecovery, backups)

  let database: RuntimeDatabase | undefined
  let ownership: RuntimeDatabaseOwnership | undefined
  try {
    if (existsSync(databasePath)) {
      if (isWritable(dataRoot)) {
        try {
          ownership = RuntimeDatabase.acquireOwnership(databasePath)
        } catch (error) {
          const marker = await waitForRecoveryMarker(databasePath)
          if (marker) return recoveryResult(marker, backups)
          throw error
        }
      } else {
        RuntimeDatabase.assertNoLiveOwner(databasePath)
      }

      assertWalBundleReady(databasePath)
      if (!isWritable(dataRoot) || !isWritable(databasePath)) {
        database = ownership?.openReadOnly() ?? RuntimeDatabase.openReadOnly(databasePath)
        assertFullIntegrity(database)
        observer.onDatabaseOpened?.(database, dataRoot, backups)
        ownership?.release()
        ownership = undefined
        return {
          kind: 'read-only',
          database,
          dataRoot,
          reason: 'filesystem-read-only'
        }
      }

      const inspection = ownership!.openReadOnly()
      let currentVersion: number
      try {
        assertFullIntegrity(inspection)
        currentVersion = readSchemaVersion(inspection)
      } finally {
        inspection.close()
      }
      const supportedVersion = migrations.reduce(
        (highest, migration) => Math.max(highest, migration.version),
        0
      )
      if (currentVersion > supportedVersion) {
        database = ownership!.openReadOnly()
        assertFullIntegrity(database)
        observer.onDatabaseOpened?.(database, dataRoot, backups)
        ownership!.release()
        ownership = undefined
        return { kind: 'read-only', database, dataRoot, reason: 'newer-schema' }
      }
      database = ownership!.openWritable()
      ownership = undefined
    } else {
      ownership = RuntimeDatabase.acquireOwnership(databasePath)
      const recoveryAfterFenceShape = readRecoveryMarkerShape(databasePath)
      const recoveryAfterFence = recoveryAfterFenceShape?.kind === 'legacy'
        ? activeRecoveryMarker(await upgradeLegacyRecoveryMarker(databasePath, observer))
        : activeRecoveryMarker(recoveryAfterFenceShape?.marker)
      if (recoveryAfterFence) {
        ownership.release()
        ownership = undefined
        return recoveryResult(recoveryAfterFence, backups)
      }
      if (existsSync(databasePath)) {
        ownership.release()
        ownership = undefined
        return openRecoverableRuntimeDatabase(dataRoot, migrations, observer)
      }
      database = ownership.openWritable()
      ownership = undefined
    }

    observer.onDatabaseOpened?.(database, dataRoot, backups)
    assertFullIntegrity(database)
    await new MigrationRunner(database, migrations, backups).migrate()
    return { kind: 'writable', database, dataRoot }
  } catch (error) {
    if (database && observer.isShutdownRequested?.()) throw error

    if (isDatabaseOwnershipRecoveryError(error)) {
      ownership?.release()
      return preserveDatabaseForOwnershipRecovery(databasePath, backups, error.issue, observer)
    }

    const recoveryReason = recoveryReasonFor(error)
    if (database) {
      if (database.readOnly) {
        closeObservedDatabase(database, observer)
      } else if (recoveryReason || isWriteDenied(error) || isNewerSchema(error)) {
        ownership = database.closeRetainingOwnership()
        observer.onDatabaseClosed?.(database)
      } else {
        closeObservedDatabase(database, observer)
      }
      database = undefined
    }

    if (isWriteDenied(error) && !recoveryReason) {
      try {
        const readOnly = ownership?.openReadOnly() ?? RuntimeDatabase.openReadOnly(databasePath)
        assertFullIntegrity(readOnly)
        observer.onDatabaseOpened?.(readOnly, dataRoot, backups)
        ownership?.release()
        ownership = undefined
        return {
          kind: 'read-only',
          database: readOnly,
          dataRoot,
          reason: 'filesystem-read-only'
        }
      } catch (readOnlyError) {
        const readOnlyRecoveryReason = recoveryReasonFor(readOnlyError)
        if (!readOnlyRecoveryReason) {
          ownership?.release()
          throw readOnlyError
        }
        return quarantineDatabaseBundle(
          databasePath,
          backups,
          readOnlyRecoveryReason,
          ownership,
          observer
        )
      }
    }
    if (isNewerSchema(error)) {
      const readOnly = ownership?.openReadOnly() ?? RuntimeDatabase.openReadOnly(databasePath)
      assertFullIntegrity(readOnly)
      observer.onDatabaseOpened?.(readOnly, dataRoot, backups)
      ownership?.release()
      ownership = undefined
      return { kind: 'read-only', database: readOnly, dataRoot, reason: 'newer-schema' }
    }
    if (recoveryReason) {
      return quarantineDatabaseBundle(
        databasePath,
        backups,
        recoveryReason,
        ownership,
        observer
      )
    }
    ownership?.release()
    throw error
  }
}

async function preserveDatabaseForOwnershipRecovery(
  databasePath: string,
  backups: DatabaseBackupService,
  ownershipIssue: DatabaseOwnershipRecoveryIssue,
  observer: RuntimeDatabaseBootstrapObserver
): Promise<Extract<RuntimeDatabaseBootstrapResult, { kind: 'recovery-required' }>> {
  const marker = newRecoveryMarker(
    databasePath,
    'ownership-recovery-required',
    ownershipIssue
  )
  try {
    const published = await publishRecoveryMarker(marker, observer)
    await observer.onRecoveryMarkerPublished?.(published)
    return recoveryResult(published, backups)
  } catch (error) {
    const durable = durableMarkerAfterPublicationFailure(databasePath, error)
    if (!durable) throw error
    return recoveryResult(durable, backups, {
      markerError: recoveryError('RECOVERY_MARKER_FAILED', error),
      quarantinedPath: databasePath
    })
  }
}

async function quarantineDatabaseBundle(
  databasePath: string,
  backups: DatabaseBackupService,
  reason: RuntimeDatabaseRecoveryMarker['reason'],
  ownership: RuntimeDatabaseOwnership | undefined,
  observer: RuntimeDatabaseBootstrapObserver
): Promise<RuntimeDatabaseBootstrapResult> {
  const marker = newRecoveryMarker(databasePath, reason)
  let published: RuntimeDatabaseRecoveryMarker
  try {
    published = await publishRecoveryMarker(marker, observer)
    await observer.onRecoveryMarkerPublished?.(published)
  } catch (error) {
    ownership?.release()
    const durable = durableMarkerAfterPublicationFailure(databasePath, error)
    if (!durable) throw error
    return recoveryResult(durable, backups, {
      markerError: recoveryError('RECOVERY_MARKER_FAILED', error),
      quarantinedPath: databasePath
    })
  }

  let moveError: RuntimeDatabaseRecoveryError | undefined
  try {
    await moveDatabaseBundle(databasePath, published.quarantinedPath)
  } catch (error) {
    moveError = recoveryError('RECOVERY_MOVE_FAILED', error)
  } finally {
    ownership?.release()
  }
  return recoveryResult(published, backups, moveError ? { moveError } : {})
}

async function moveDatabaseBundle(databasePath: string, quarantinedPath: string): Promise<void> {
  if (existsSync(databasePath)) await rename(databasePath, quarantinedPath)
  for (const suffix of ['-wal', '-shm'] as const) {
    const source = `${databasePath}${suffix}`
    const target = `${quarantinedPath}${suffix}`
    if (existsSync(source)) await rename(source, target)
  }
}

function newRecoveryMarker(
  databasePath: string,
  reason: RuntimeDatabaseRecoveryMarker['reason'],
  ownershipIssue?: DatabaseOwnershipRecoveryIssue
): RuntimeDatabaseRecoveryMarker {
  const createdAt = Date.now()
  return {
    version: 1,
    state: 'required',
    recoveryId: randomUUID(),
    reason,
    durableDatabasePath: databasePath,
    quarantinedPath: reason === 'ownership-recovery-required'
      ? databasePath
      : `${databasePath}.corrupt-${createdAt}`,
    markerPath: `${databasePath}.recovery.json`,
    createdAt,
    ...(ownershipIssue ? { ownershipIssue } : {})
  }
}

async function publishRecoveryMarker(
  marker: RuntimeDatabaseRecoveryMarker,
  observer: RuntimeDatabaseBootstrapObserver
): Promise<RuntimeDatabaseRecoveryMarker> {
  try {
    return await withMarkerPublicationFence(marker.durableDatabasePath, async () => {
      const persisted = await readCanonicalRecoveryMarkerLocked(marker.durableDatabasePath)
      if (
        persisted?.state === 'required' &&
        !hasMatchingResolvedTombstone(persisted)
      ) {
        return persisted
      }
      await replaceRecoveryMarker(marker.markerPath, marker)
      return requireCanonicalRecoveryGeneration(marker)
    })
  } catch (error) {
    return publishRecoveryMarkerWithoutActionFence(marker, error, observer)
  }
}

async function publishRecoveryMarkerWithoutActionFence(
  proposed: RuntimeDatabaseRecoveryMarker,
  fenceError: unknown,
  observer: RuntimeDatabaseBootstrapObserver
): Promise<RuntimeDatabaseRecoveryMarker> {
  const shape = readRecoveryMarkerShape(proposed.durableDatabasePath)
  if (shape?.kind === 'modern') {
    const active = activeRecoveryMarker(shape.marker)
    if (active) return active
  }
  if (shape?.kind === 'legacy') {
    const upgraded: RuntimeDatabaseRecoveryMarker = {
      ...shape.marker,
      recoveryId: shape.marker.recoveryId ?? randomUUID(),
      state: 'required'
    }
    return publishClaimedRecoveryGeneration(upgraded, shape.source, fenceError, observer)
  }
  const source = recoveryGenerationSourceForShape(shape)
  if (!source) return shape!.marker
  return publishClaimedRecoveryGeneration(proposed, source, fenceError, observer)
}

async function publishClaimedRecoveryGeneration(
  proposed: RuntimeDatabaseRecoveryMarker,
  source: RuntimeRecoveryGenerationSource,
  fenceError: unknown,
  observer: RuntimeDatabaseBootstrapObserver = {}
): Promise<RuntimeDatabaseRecoveryMarker> {
  const claimPath = recoveryGenerationClaimPath(proposed.markerPath, source)
  const claim: RuntimeRecoveryGenerationClaim = {
    version: 1,
    kind: 'recovery-generation-claim',
    source,
    markerDigest: recoveryMarkerDigest(proposed),
    marker: proposed
  }
  const partialPath = `${claimPath}.partial-${randomUUID()}`
  const handle = await open(partialPath, 'wx', 0o600)
  try {
    await handle.writeFile(JSON.stringify(claim), 'utf8')
    await handle.sync()
  } finally {
    await handle.close()
  }

  try {
    try {
      await link(partialPath, claimPath)
    } catch (error) {
      if (errorCode(error) !== 'EEXIST') throw error
    }
    // Whether this process won or observed the winning hard-link, syncing the
    // parent establishes the claim as durable before it is used as authority.
    syncDirectory(dirname(claimPath))
    const durableClaim = readRecoveryGenerationClaim(
      claimPath,
      proposed.durableDatabasePath,
      source
    )
    await observer.onRecoveryGenerationClaimPublished?.({
      claimPath,
      recoveryId: durableClaim.marker.recoveryId
    })
  } finally {
    await rm(partialPath, { force: true }).catch(() => undefined)
  }

  const claimed = readRecoveryGenerationClaim(
    claimPath,
    proposed.durableDatabasePath,
    source
  ).marker
  try {
    await replaceRecoveryMarker(claimed.markerPath, claimed)
  } catch (error) {
    const canonical = matchingCanonicalRecoveryGeneration(claimed)
    if (canonical) return canonical
    throw new RecoveryMarkerPublicationError(
      `database recovery generation is durable but canonical publication failed: ${errorMessage(error)}`,
      readRecoveryGenerationClaim(claimPath, proposed.durableDatabasePath, source).marker,
      new AggregateError([fenceError, error])
    )
  }
  return requireCanonicalRecoveryGeneration(claimed)
}

function recoveryGenerationClaimPath(
  markerPath: string,
  source: RuntimeRecoveryGenerationSource
): string {
  const digest = createHash('sha256').update(JSON.stringify(source)).digest('hex')
  return `${markerPath}.generation-${digest}.claim`
}

function readRecoveryGenerationClaim(
  claimPath: string,
  databasePath: string,
  expectedSource: RuntimeRecoveryGenerationSource
): RuntimeRecoveryGenerationClaim {
  let bytes: string
  try {
    bytes = readFileSync(claimPath, 'utf8')
  } catch (error) {
    if (errorCode(error) === 'ENOENT') throw error
    throw new Error('database recovery generation claim is invalid')
  }
  let value: unknown
  try {
    value = JSON.parse(bytes) as unknown
  } catch {
    throw new Error('database recovery generation claim is invalid')
  }
  if (typeof value !== 'object' || value === null) {
    throw new Error('database recovery generation claim is invalid')
  }
  const claim = value as Partial<RuntimeRecoveryGenerationClaim>
  if (
    claim.version !== 1 ||
    claim.kind !== 'recovery-generation-claim' ||
    JSON.stringify(claim.source) !== JSON.stringify(expectedSource) ||
    typeof claim.marker !== 'object' || claim.marker === null
  ) {
    throw new Error('database recovery generation claim is invalid')
  }
  const shape = parseRecoveryMarkerShape(JSON.stringify(claim.marker), databasePath)
  if (shape.kind !== 'modern' || shape.marker.state !== 'required') {
    throw new Error('database recovery generation claim is invalid')
  }
  if (
    claim.markerDigest !== recoveryMarkerDigest(shape.marker) ||
    (expectedSource.kind === 'resolved' &&
      shape.marker.recoveryId === expectedSource.recoveryId)
  ) {
    throw new Error('database recovery generation claim is invalid')
  }
  assertRecoveryGenerationSourceEvidence(expectedSource, shape.marker, databasePath)
  return claim as RuntimeRecoveryGenerationClaim
}

function readRecoveryGenerationClaimForSource(
  databasePath: string,
  shape: RecoveryMarkerShape | undefined
): RuntimeRecoveryGenerationClaim | undefined {
  const source = recoveryGenerationSourceForShape(shape)
  if (!source) return undefined
  const markerPath = `${databasePath}.recovery.json`
  const claimPath = recoveryGenerationClaimPath(markerPath, source)
  try {
    return readRecoveryGenerationClaim(claimPath, databasePath, source)
  } catch (error) {
    if (errorCode(error) === 'ENOENT') return undefined
    throw error
  }
}

function recoveryGenerationSourceForShape(
  shape: RecoveryMarkerShape | undefined
): RuntimeRecoveryGenerationSource | undefined {
  if (!shape) return { kind: 'initial' }
  if (shape.kind === 'legacy') return shape.source
  if (activeRecoveryMarker(shape.marker)) return undefined
  return {
    kind: 'resolved',
    recoveryId: shape.marker.recoveryId,
    markerDigest: shape.markerDigest,
    markerBytes: shape.markerBytes
  }
}

function assertRecoveryGenerationSourceEvidence(
  source: RuntimeRecoveryGenerationSource,
  claimedMarker: RuntimeDatabaseRecoveryMarker,
  databasePath: string
): void {
  if (source.kind === 'initial') return
  if (createHash('sha256').update(source.markerBytes).digest('hex') !== source.markerDigest) {
    throw new Error('database recovery generation claim is invalid')
  }
  const sourceShape = parseRecoveryMarkerShape(source.markerBytes, databasePath)
  if (source.kind === 'legacy') {
    if (sourceShape.kind !== 'legacy' || !sameRecoveryEvidence(sourceShape.marker, claimedMarker)) {
      throw new Error('database recovery generation claim is invalid')
    }
    return
  }
  if (
    sourceShape.kind !== 'modern' ||
    sourceShape.marker.recoveryId !== source.recoveryId ||
    (sourceShape.marker.state !== 'resolved' &&
      !hasMatchingResolvedTombstone(sourceShape.marker))
  ) {
    throw new Error('database recovery generation claim is invalid')
  }
}

function sameRecoveryEvidence(
  legacy: LegacyRecoveryMarker,
  claimed: RuntimeDatabaseRecoveryMarker
): boolean {
  return legacy.version === claimed.version &&
    legacy.reason === claimed.reason &&
    legacy.durableDatabasePath === claimed.durableDatabasePath &&
    legacy.quarantinedPath === claimed.quarantinedPath &&
    legacy.markerPath === claimed.markerPath &&
    legacy.createdAt === claimed.createdAt &&
    legacy.ownershipIssue === claimed.ownershipIssue &&
    (legacy.recoveryId === undefined || legacy.recoveryId === claimed.recoveryId)
}

function recoveryMarkerDigest(marker: RuntimeDatabaseRecoveryMarker): string {
  return createHash('sha256').update(JSON.stringify(marker)).digest('hex')
}

function matchingCanonicalRecoveryGeneration(
  expected: RuntimeDatabaseRecoveryMarker
): RuntimeDatabaseRecoveryMarker | undefined {
  const shape = readRecoveryMarkerShape(expected.durableDatabasePath)
  if (shape?.kind !== 'modern') return undefined
  return shape.marker.recoveryId === expected.recoveryId ? shape.marker : undefined
}

function requireCanonicalRecoveryGeneration(
  expected: RuntimeDatabaseRecoveryMarker
): RuntimeDatabaseRecoveryMarker {
  const canonical = matchingCanonicalRecoveryGeneration(expected)
  if (!canonical) {
    throw new Error('database recovery generation was not durably published')
  }
  return canonical
}

function durableMarkerAfterPublicationFailure(
  databasePath: string,
  error: unknown
): RuntimeDatabaseRecoveryMarker | undefined {
  const shape = readRecoveryMarkerShape(databasePath)
  if (shape?.kind === 'modern') {
    const active = activeRecoveryMarker(shape.marker)
    if (active) return active
  }
  return error instanceof RecoveryMarkerPublicationError ? error.durableMarker : undefined
}

export async function resolveRuntimeDatabaseRecoveryMarker(
  markerPath: string,
  recoveryId: string,
  observer: RuntimeRecoveryMarkerFinalizationObserver = {}
): Promise<void> {
  const databasePath = markerPath.endsWith('.recovery.json')
    ? markerPath.slice(0, -'.recovery.json'.length)
    : ''
  const required = await readCanonicalRecoveryMarker(databasePath)
  if (
    !required ||
    required.state !== 'required' ||
    required.recoveryId !== recoveryId ||
    hasMatchingResolvedTombstone(required)
  ) {
    throw new Error('数据库恢复状态已更新，请使用最新恢复页面重试')
  }
  const tombstonePath = resolvedTombstonePath(required)
  const tombstone = {
    version: 1,
    state: 'resolved',
    recoveryId,
    markerPath,
    resolvedAt: Date.now()
  } as const
  const partialPath = `${tombstonePath}.partial-${randomUUID()}`
  const handle = await open(partialPath, 'wx', 0o600)
  try {
    try {
      await handle.writeFile(JSON.stringify(tombstone), 'utf8')
      observer.beforeFileSync?.()
      await handle.sync()
    } finally {
      await handle.close()
    }

    let committed = false
    observer.beforePublish?.()
    try {
      await link(partialPath, tombstonePath)
      committed = true
    } catch (error) {
      if (errorCode(error) === 'EEXIST' && hasMatchingResolvedTombstone(required)) {
        committed = true
      } else {
        throw error
      }
    }
    // The atomic namespace publication above is the success commit point.
    // A later durability error can only make a crash fall back to the still-
    // immutable required marker, so it must not turn a committed action into a
    // user-visible failure.
    try { observer.afterPublish?.() } catch { /* committed: required remains the safe fallback */ }
    try {
      observer.beforeDirectorySync?.()
      syncDirectory(dirname(tombstonePath))
    } catch {
      // Best effort after commit; losing the tombstone re-opens the same recovery generation.
    }
    if (!committed) throw new Error('database recovery resolution was not published')
  } finally {
    await rm(partialPath, { force: true }).catch(() => undefined)
  }
}

async function replaceRecoveryMarker(
  markerPath: string,
  marker: RuntimeDatabaseRecoveryMarker
): Promise<void> {
  const partialPath = `${markerPath}.partial-${randomUUID()}`
  const handle = await open(partialPath, 'wx', 0o600)
  try {
    await handle.writeFile(JSON.stringify(marker), 'utf8')
    await handle.sync()
  } finally {
    await handle.close()
  }
  try {
    await rename(partialPath, markerPath)
    syncDirectory(dirname(markerPath))
  } finally {
    await rm(partialPath, { force: true }).catch(() => undefined)
  }
}

function syncDirectory(path: string): void {
  let descriptor: number | undefined
  try {
    descriptor = openSync(path, constants.O_RDONLY)
    const fs = process.getBuiltinModule('node:fs') as typeof import('node:fs')
    fs.fsyncSync(descriptor)
  } finally {
    if (descriptor !== undefined) closeSync(descriptor)
  }
}

async function readRecoveryMarker(
  databasePath: string
): Promise<RuntimeDatabaseRecoveryMarker | undefined> {
  const marker = await readCanonicalRecoveryMarker(databasePath)
  return activeRecoveryMarker(marker)
}

type LegacyRecoveryMarker = Omit<RuntimeDatabaseRecoveryMarker, 'recoveryId' | 'state'> & {
  recoveryId?: string
}

type RuntimeRecoveryGenerationSource =
  | { kind: 'initial' }
  | { kind: 'legacy'; markerDigest: string; markerBytes: string }
  | {
      kind: 'resolved'
      recoveryId: string
      markerDigest: string
      markerBytes: string
    }

interface RuntimeRecoveryGenerationClaim {
  version: 1
  kind: 'recovery-generation-claim'
  source: RuntimeRecoveryGenerationSource
  markerDigest: string
  marker: RuntimeDatabaseRecoveryMarker
}

type RecoveryMarkerShape =
  | {
      kind: 'modern'
      marker: RuntimeDatabaseRecoveryMarker
      markerDigest: string
      markerBytes: string
    }
  | {
      kind: 'legacy'
      marker: LegacyRecoveryMarker
      markerDigest: string
      markerBytes: string
      source: Extract<RuntimeRecoveryGenerationSource, { kind: 'legacy' }>
    }

class RecoveryMarkerPublicationError extends Error {
  readonly durableMarker: RuntimeDatabaseRecoveryMarker

  constructor(message: string, durableMarker: RuntimeDatabaseRecoveryMarker, cause: unknown) {
    super(message, { cause })
    this.name = 'RecoveryMarkerPublicationError'
    this.durableMarker = durableMarker
  }
}

async function readCanonicalRecoveryMarker(
  databasePath: string
): Promise<RuntimeDatabaseRecoveryMarker | undefined> {
  const shape = readRecoveryMarkerShape(databasePath)
  if (!shape) return undefined
  if (shape.kind === 'modern') return shape.marker
  return upgradeLegacyRecoveryMarker(databasePath)
}

async function upgradeLegacyRecoveryMarker(
  databasePath: string,
  observer: RuntimeDatabaseBootstrapObserver = {}
): Promise<RuntimeDatabaseRecoveryMarker | undefined> {
  try {
    return await withMarkerPublicationFence(databasePath, () => (
      readCanonicalRecoveryMarkerLocked(databasePath)
    ))
  } catch (error) {
    const shape = readRecoveryMarkerShape(databasePath)
    if (!shape) return undefined
    if (shape.kind === 'modern') return shape.marker
    const candidate: RuntimeDatabaseRecoveryMarker = {
      ...shape.marker,
      recoveryId: shape.marker.recoveryId ?? randomUUID(),
      state: 'required'
    }
    try {
      return await publishClaimedRecoveryGeneration(candidate, shape.source, error, observer)
    } catch (publicationError) {
      if (publicationError instanceof RecoveryMarkerPublicationError) {
        return publicationError.durableMarker
      }
      throw publicationError
    }
  }
}

async function readCanonicalRecoveryMarkerLocked(
  databasePath: string
): Promise<RuntimeDatabaseRecoveryMarker | undefined> {
  const shape = readRecoveryMarkerShape(databasePath)
  if (!shape) return undefined
  if (shape.kind === 'modern') return shape.marker
  const upgraded: RuntimeDatabaseRecoveryMarker = {
    ...shape.marker,
    recoveryId: shape.marker.recoveryId ?? randomUUID(),
    state: 'required'
  }
  await replaceRecoveryMarker(upgraded.markerPath, upgraded)
  return requireCanonicalRecoveryGeneration(upgraded)
}

function readRecoveryMarkerShape(databasePath: string): RecoveryMarkerShape | undefined {
  const markerPath = `${databasePath}.recovery.json`
  let bytes: string
  try {
    bytes = readFileSync(markerPath, 'utf8')
  } catch (error) {
    if (errorCode(error) === 'ENOENT') return undefined
    throw error
  }
  return parseRecoveryMarkerShape(bytes, databasePath)
}

function parseRecoveryMarkerShape(
  bytes: string,
  databasePath: string
): RecoveryMarkerShape {
  let value: unknown
  try {
    value = JSON.parse(bytes) as unknown
  } catch {
    throw new Error('database recovery marker is invalid')
  }
  if (typeof value !== 'object' || value === null) {
    throw new Error('database recovery marker is invalid')
  }
  const marker = value as Partial<RuntimeDatabaseRecoveryMarker>
  const hasRecoveryId = Object.prototype.hasOwnProperty.call(marker, 'recoveryId')
  const hasState = Object.prototype.hasOwnProperty.call(marker, 'state')
  const durablePath = resolve(databasePath)
  const markerPath = `${databasePath}.recovery.json`
  if (
    marker.version !== 1 ||
    (hasRecoveryId && (
      typeof marker.recoveryId !== 'string' ||
      !/^[A-Za-z0-9][A-Za-z0-9._-]{0,159}$/.test(marker.recoveryId)
    )) ||
    (hasState && !['required', 'resolved'].includes(String(marker.state))) ||
    (hasState && !hasRecoveryId) ||
    ![
      'physical-corruption',
      'wal-recovery-required',
      'ownership-recovery-required'
    ].includes(String(marker.reason)) ||
    marker.durableDatabasePath !== durablePath ||
    marker.markerPath !== markerPath ||
    typeof marker.quarantinedPath !== 'string' ||
    dirname(marker.quarantinedPath) !== dirname(durablePath) ||
    (marker.reason === 'ownership-recovery-required'
      ? marker.quarantinedPath !== durablePath ||
        !['owner-record-malformed', 'takeover-sidecar-unusable'].includes(
          String(marker.ownershipIssue)
        )
      : !marker.quarantinedPath.startsWith(`${durablePath}.corrupt-`) ||
        marker.ownershipIssue !== undefined) ||
    !Number.isSafeInteger(marker.createdAt)
  ) {
    throw new Error('database recovery marker is invalid')
  }
  if (!hasState) {
    const markerDigest = createHash('sha256').update(bytes).digest('hex')
    return {
      kind: 'legacy',
      marker: marker as LegacyRecoveryMarker,
      markerDigest,
      markerBytes: bytes,
      source: { kind: 'legacy', markerDigest, markerBytes: bytes }
    }
  }
  return {
    kind: 'modern',
    marker: marker as RuntimeDatabaseRecoveryMarker,
    markerDigest: createHash('sha256').update(bytes).digest('hex'),
    markerBytes: bytes
  }
}

function resolvedTombstonePath(marker: RuntimeDatabaseRecoveryMarker): string {
  return `${marker.markerPath}.resolved-${marker.recoveryId}`
}

function activeRecoveryMarker(
  marker: RuntimeDatabaseRecoveryMarker | undefined
): RuntimeDatabaseRecoveryMarker | undefined {
  if (!marker || marker.state === 'resolved' || hasMatchingResolvedTombstone(marker)) {
    return undefined
  }
  return marker
}

const markerPublicationQueues = new Map<string, Promise<void>>()

async function withMarkerPublicationFence<T>(
  databasePath: string,
  operation: () => Promise<T>
): Promise<T> {
  const previous = markerPublicationQueues.get(databasePath) ?? Promise.resolve()
  let release!: () => void
  const current = new Promise<void>((resolveQueue) => { release = resolveQueue })
  markerPublicationQueues.set(databasePath, current)
  await previous
  try {
    return await withDatabaseRecoveryActionFence(databasePath, operation)
  } finally {
    release()
    if (markerPublicationQueues.get(databasePath) === current) {
      markerPublicationQueues.delete(databasePath)
    }
  }
}

function hasMatchingResolvedTombstone(marker: RuntimeDatabaseRecoveryMarker): boolean {
  let value: unknown
  try {
    value = JSON.parse(readFileSync(resolvedTombstonePath(marker), 'utf8')) as unknown
  } catch (error) {
    return false
  }
  if (typeof value !== 'object' || value === null) return false
  const tombstone = value as Record<string, unknown>
  return tombstone.version === 1 &&
    tombstone.state === 'resolved' &&
    tombstone.recoveryId === marker.recoveryId &&
    tombstone.markerPath === marker.markerPath &&
    Number.isSafeInteger(tombstone.resolvedAt)
}

export async function isRuntimeDatabaseRecoveryResolved(
  markerPath: string,
  recoveryId: string
): Promise<boolean> {
  const databasePath = markerPath.endsWith('.recovery.json')
    ? markerPath.slice(0, -'.recovery.json'.length)
    : ''
  const marker = await readCanonicalRecoveryMarker(databasePath)
  return Boolean(
    marker && marker.recoveryId === recoveryId &&
    (marker.state === 'resolved' || hasMatchingResolvedTombstone(marker))
  )
}

async function waitForRecoveryMarker(
  databasePath: string,
  timeoutMs = 250
): Promise<RuntimeDatabaseRecoveryMarker | undefined> {
  const deadline = Date.now() + timeoutMs
  do {
    const marker = await readRecoveryMarker(databasePath)
    if (marker) return marker
    await new Promise((resolve) => setTimeout(resolve, 10))
  } while (Date.now() < deadline)
  return readRecoveryMarker(databasePath)
}

async function recoveryResult(
  marker: RuntimeDatabaseRecoveryMarker,
  backups: DatabaseBackupService,
  overrides: {
    quarantinedPath?: string
    markerError?: RuntimeDatabaseRecoveryError
    moveError?: RuntimeDatabaseRecoveryError
  } = {}
): Promise<Extract<RuntimeDatabaseBootstrapResult, { kind: 'recovery-required' }>> {
  let validBackups: DatabaseBackupDescriptor[] = []
  let backupListError: RuntimeDatabaseRecoveryError | undefined
  try {
    validBackups = await backups.listValid()
  } catch (error) {
    backupListError = recoveryError('BACKUP_LIST_FAILED', error)
  }
  return {
    kind: 'recovery-required',
    recoveryId: marker.recoveryId,
    reason: marker.reason,
    durableDatabasePath: marker.durableDatabasePath,
    quarantinedPath: overrides.quarantinedPath ?? marker.quarantinedPath,
    markerPath: marker.markerPath,
    backups: validBackups,
    ...(marker.ownershipIssue ? { ownershipIssue: marker.ownershipIssue } : {}),
    ...(backupListError ? { backupListError } : {}),
    ...(overrides.markerError ? { markerError: overrides.markerError } : {}),
    ...(overrides.moveError ? { moveError: overrides.moveError } : {})
  }
}

function recoveryError(
  code: RuntimeDatabaseRecoveryError['code'],
  error: unknown
): RuntimeDatabaseRecoveryError {
  return { code, message: errorMessage(error), retryable: true }
}

function assertWalBundleReady(databasePath: string): void {
  const walPath = `${databasePath}-wal`
  if (!existsSync(walPath) || statSync(walPath).size <= 32) return
  const shmPath = `${databasePath}-shm`
  let bytes: Buffer
  try {
    accessSync(shmPath, constants.R_OK)
    const metadata = statSync(shmPath)
    if (!metadata.isFile() || metadata.size < 32_768) throw new Error('SHM is truncated')
    bytes = readFileSync(shmPath)
  } catch (error) {
    throw walRecoveryError(`committed WAL has no readable SHM: ${errorMessage(error)}`)
  }
  const firstHeader = bytes.subarray(0, 48)
  const secondHeader = bytes.subarray(48, 96)
  const version = firstHeader.length === 48 ? firstHeader.readUInt32LE(0) : 0
  if (version !== 3_007_000 || !firstHeader.equals(secondHeader)) {
    throw walRecoveryError('committed WAL has a corrupt SHM header')
  }
}

function walRecoveryError(message: string): Error & { code: 'WAL_RECOVERY_REQUIRED' } {
  return Object.assign(new Error(message), { code: 'WAL_RECOVERY_REQUIRED' as const })
}

function assertFullIntegrity(database: RuntimeDatabase): void {
  const rows = database.all<Record<string, unknown>>('PRAGMA integrity_check')
  const result = rows.map((row) => String(Object.values(row)[0] ?? ''))
  if (result.length !== 1 || result[0]?.toLowerCase() !== 'ok') {
    throw new Error(`database corrupt: integrity_check failed: ${result.slice(0, 3).join('; ')}`)
  }
}

function readSchemaVersion(database: RuntimeDatabase): number {
  const historyExists = database.get<{ present: number }>(
    `SELECT 1 AS present FROM sqlite_master
     WHERE type = 'table' AND name = 'schema_migrations'`
  ) !== undefined
  if (!historyExists) return 0
  return database.get<{ version: number | null }>(
    'SELECT MAX(version) AS version FROM schema_migrations'
  )?.version ?? 0
}

function closeObservedDatabase(
  database: RuntimeDatabase,
  observer: RuntimeDatabaseBootstrapObserver
): void {
  database.close()
  observer.onDatabaseClosed?.(database)
}

function recoveryReasonFor(
  error: unknown
): RuntimeDatabaseRecoveryMarker['reason'] | undefined {
  if (errorCode(error) === 'WAL_RECOVERY_REQUIRED' || /WAL requires recovery/i.test(errorMessage(error))) {
    return 'wal-recovery-required'
  }
  return isPhysicalDatabaseCorruption(error) ? 'physical-corruption' : undefined
}

function isPhysicalDatabaseCorruption(error: unknown): boolean {
  return /file is not a database|database disk image is malformed|database corrupt|integrity check failed/i.test(
    errorMessage(error)
  )
}

function isWriteDenied(error: unknown): boolean {
  const code = errorCode(error)
  return code === 'EACCES' || code === 'EPERM' || /readonly|read-only|permission denied/i.test(
    errorMessage(error)
  )
}

function isWritable(path: string): boolean {
  try {
    accessSync(path, constants.W_OK)
    return true
  } catch {
    return false
  }
}

function isNewerSchema(error: unknown): boolean {
  return /database schema version \d+ is newer than supported version \d+/i.test(
    errorMessage(error)
  )
}

function errorCode(error: unknown): string | undefined {
  return typeof error === 'object' && error !== null && 'code' in error
    ? String(error.code)
    : undefined
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}
