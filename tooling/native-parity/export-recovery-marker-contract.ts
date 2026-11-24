// Capture public recovery operations from the frozen runtime, on disposable paths.
import { mkdir, mkdtemp, readFile, readdir, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { openRecoverableRuntimeDatabase, resolveRuntimeDatabaseRecoveryMarker, isRuntimeDatabaseRecoveryResolved } from '../../apps/runtime/src/storage/runtime-database-bootstrap'

if (process.argv[2] === '--gate') {
  // run.sh bundles the gate-only probe next to this exporter.
  const probe = new URL('./frozen-probe.js', import.meta.url)
  const { capture } = await import(probe.href)
  await capture(process.argv.slice(3))
} else {
const temporary = await mkdtemp(join(tmpdir(), 'matou-recovery-marker-contract-'))
const cases: unknown[] = []
const markerFor = (root: string, id = 'reference-generation') => ({
  version: 1, state: 'required', recoveryId: id, reason: 'physical-corruption',
  durableDatabasePath: join(root, 'matou.sqlite'), quarantinedPath: join(root, 'matou.sqlite.corrupt-123'),
  markerPath: join(root, 'matou.sqlite.recovery.json'), createdAt: 123,
})
async function files(root: string) {
  const result: Record<string, string> = {}
  for (const name of (await readdir(root)).sort()) {
    if (name.endsWith('.json') || name.endsWith('.claim') || name.includes('.resolved-')) result[name] = await readFile(join(root, name), 'utf8')
  }
  return result
}
async function load(root: string) {
  const result = await openRecoverableRuntimeDatabase(root, [])
  if (result.kind !== 'recovery-required') {
    result.database.close()
    throw new Error(`expected recovery, got ${result.kind}`)
  }
  return result
}
for (const name of ['legacy-no-id', 'legacy-with-id', 'required', 'invalid-state', 'invalid-path', 'invalid-json']) {
  const root = join(temporary, name); await mkdir(root)
  const marker: any = markerFor(root)
  if (name.startsWith('legacy')) delete marker.state
  if (name === 'legacy-no-id') delete marker.recoveryId
  if (name === 'legacy-with-id') { marker.extra = 'retained legacy data'; marker['2'] = 'two'; marker['1'] = 'one' }
  if (name === 'invalid-state') marker.state = 'unknown'
  if (name === 'invalid-path') marker.quarantinedPath = '/outside/unrelated'
  await writeFile(marker.markerPath, name === 'invalid-json' ? '{' : JSON.stringify(marker))
  const before = await files(root)
  let expected: unknown, error: string | undefined
  try { expected = await load(root) } catch (e) { error = (e as Error).message }
  cases.push({ name, action: 'load', sourceRoot: root, before, expected, error, after: await files(root) })
}
{
  const root = join(temporary, 'resolve'); await mkdir(root)
  const marker = markerFor(root)
  await writeFile(marker.markerPath, JSON.stringify(marker))
  const before = await files(root)
  const wasResolved = await isRuntimeDatabaseRecoveryResolved(marker.markerPath, marker.recoveryId)
  let wrongIdError: string | undefined
  try { await resolveRuntimeDatabaseRecoveryMarker(marker.markerPath, 'wrong') } catch (e) { wrongIdError = (e as Error).message }
  await resolveRuntimeDatabaseRecoveryMarker(marker.markerPath, marker.recoveryId)
  cases.push({ name: 'resolve', action: 'resolve', sourceRoot: root, before, expected: { wasResolved, wrongIdError, resolved: await isRuntimeDatabaseRecoveryResolved(marker.markerPath, marker.recoveryId) }, after: await files(root) })
}
{
  const root = join(temporary, 'initial-claim'); await mkdir(root)
  await writeFile(join(root, 'matou.sqlite.owner'), '{')
  await mkdir(join(root, 'matou.sqlite.recovery-action.sqlite'))
  const first = await load(root)
  await rm(first.markerPath)
  const before = await files(root)
  const expected = await load(root)
  cases.push({ name: 'initial-claim', action: 'load', sourceRoot: root, before, directories: ['matou.sqlite.recovery-action.sqlite'], expected, after: await files(root) })

  const previous = await readFile(first.markerPath, 'utf8')
  await resolveRuntimeDatabaseRecoveryMarker(first.markerPath, first.recoveryId)
  const next = await load(root)
  // Reopen from the immutable previous-generation marker and the winning claim.
  await writeFile(first.markerPath, previous)
  const resolvedBefore = await files(root)
  const resolvedExpected = await load(root)
  if (next.recoveryId !== resolvedExpected.recoveryId) throw new Error('source claim changed generation')
  cases.push({ name: 'resolved-claim', action: 'load', sourceRoot: root, before: resolvedBefore, directories: ['matou.sqlite.recovery-action.sqlite'], expected: resolvedExpected, after: await files(root) })
}
await writeFile(process.argv[2]!, JSON.stringify({ versions: process.versions, cases }, null, 2) + '\n')
console.log(JSON.stringify({ output: process.argv[2], cases: cases.length, temporary }))
}
