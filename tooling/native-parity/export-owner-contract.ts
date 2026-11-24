// Exercise the frozen ownership boundary with disposable paths and no app launch.
import {
  chmodSync, existsSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, writeFileSync
} from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import {
  acquireDatabaseOwner, releaseDatabaseOwner, readDatabaseOwner
} from '../../apps/runtime/src/storage/database-owner'
import { RuntimeDatabase } from '../../apps/runtime/src/storage/database'

if (process.argv[2] === '--gate') {
  const [, , , input, root, output] = process.argv
  writeFileSync(output, JSON.stringify(replayGate(input, root), null, 2) + '\n')
  process.exit(0)
}

const root = mkdtempSync(join(tmpdir(), 'matou-owner-contract-'))
const cases: unknown[] = []
for (const name of ['empty', 'live-file', 'stale-file', 'live-legacy', 'dead-legacy', 'malformed', 'invalid-pid', 'unusable-sidecar', 'generation-replaced']) {
  const directory = join(root, name); mkdirSync(directory)
  const owner = join(directory, 'matou.sqlite.owner')
  const live = name.startsWith('live')
  const initial = { pid: live ? process.pid : 2147483647, runtimeGeneration: 'previous' }
  if (name.includes('legacy')) {
    mkdirSync(owner); writeFileSync(join(owner, 'owner.json'), JSON.stringify(initial))
  } else if (name === 'malformed') writeFileSync(owner, '{')
  else if (name === 'invalid-pid') writeFileSync(owner, JSON.stringify({ pid: 0, runtimeGeneration: 'previous' }))
  else if (name === 'unusable-sidecar') { writeFileSync(owner, JSON.stringify(initial)); mkdirSync(`${owner}.takeover.sqlite`) }
  else if (name !== 'empty' && name !== 'generation-replaced') writeFileSync(owner, JSON.stringify(initial))
  let error: string | undefined, issue: string | undefined, code: string | undefined
  try { acquireDatabaseOwner(owner, 'native-contract') } catch (e: any) {
    error = e.message.replaceAll(directory, '<root>'); issue = e.issue; code = e.code
  }
  const normalize = (record: ReturnType<typeof readDatabaseOwner>) => record ? {
    ...record, pid: record.pid === process.pid ? '<self>' : record.pid,
  } : null
  const acquired = normalize(readDatabaseOwner(owner))
  const format = existsSync(owner) ? lstatSync(owner).isDirectory() ? 'directory' : 'file' : 'absent'
  if (name === 'generation-replaced') writeFileSync(owner, JSON.stringify({ pid: process.pid, runtimeGeneration: 'replacement' }))
  releaseDatabaseOwner(owner, 'native-contract')
  cases.push({ name, error, issue, code, acquired, format,
    afterRelease: normalize(readDatabaseOwner(owner)), remains: existsSync(owner) })
}
writeFileSync('apps/native/core/tests/fixtures/owner-reference.json', JSON.stringify({
  source: 'Frozen database-owner.ts on captured Electron; temporary paths; self pid normalized only.', cases
}, null, 2) + '\n')
console.log(JSON.stringify({ root, cases: cases.length }))

type GateStep =
  | { op: 'file'; path: string; text: string; mode: string }
  | { op: 'chmod'; mode: string }
  | { op: 'acquire'; lease: string; generation: string; db?: string; runtimeLease?: boolean }
  | { op: 'release'; lease: string }
  | { op: 'guard'; db?: string }

type GateCase = { id: string; seed?: Extract<GateStep, { op: 'file' }>[]; steps: GateStep[] }

function replayGate(input: string, root: string) {
  const fixtures = JSON.parse(readFileSync(input, 'utf8')) as GateCase[]
  const pids = {
    self: process.pid,
    live: Number(process.env.MATOU_OWNER_LIVE_PID),
    exited: Number(process.env.MATOU_OWNER_EXITED_PID)
  }
  const pidToken = (pid: number) => Object.entries(pids).find(([, value]) => value === pid)?.[0]

  return fixtures.map(({ id, seed = [], steps }) => {
    const directory = join(root, id)
    mkdirSync(directory)
    const leases = new Map<string, { owner: string; release: () => void }>()
    const generations = new Map<string, string>()
    const normalizeGeneration = (text: string) => {
      for (const [generated, fixed] of generations) text = text.replaceAll(generated, fixed)
      return text
    }
    const writeFixture = (file: Extract<GateStep, { op: 'file' }>) => {
      const parts = file.path.split('/')
      mkdirSync(join(directory, ...parts.slice(0, -1)), { recursive: true })
      const path = join(directory, file.path)
      const text = file.text.replace(/@(self|live|exited)@/g, (_, name: keyof typeof pids) => String(pids[name]))
      writeFileSync(path, text)
      chmodSync(path, Number.parseInt(file.mode, 8))
    }
    const mode = (path: string) => (lstatSync(path).mode & 0o777).toString(8).padStart(4, '0')
    function files(parent = ''): Record<string, unknown> {
      const entries: Record<string, unknown> = {}
      for (const name of readdirSync(join(directory, parent)).sort()) {
        const relative = parent ? `${parent}/${name}` : name
        const path = join(directory, relative)
        if (lstatSync(path).isDirectory()) {
          entries[relative] = { kind: 'directory', mode: mode(path) }
          Object.assign(entries, files(relative))
        } else {
          const bytes = readFileSync(path)
          const raw = bytes.toString('utf8')
          entries[relative] = {
            kind: 'file',
            mode: mode(path),
            ...(Buffer.from(raw).equals(bytes) ? {
              text: normalizeGeneration(raw).replace(/"pid":(\d+)/g, (match, pid) => {
                const token = pidToken(Number(pid))
                return token ? `"pid":@${token}@` : match
              })
            } : { hex: bytes.toString('hex') })
          }
        }
      }
      return entries
    }
    const snapshot = () => ({ directoryMode: mode(directory), files: files() })
    const display = (value: string) => value.replaceAll(directory, '<root>')
      .replace(/(\.claim-.*?)-[\da-f]{8}(?:-[\da-f]{4}){3}-[\da-f]{12}/g, '$1-<uuid>')

    for (const file of seed) writeFixture(file)
    const initial = snapshot()
    const observations = steps.map(step => {
      let owner: string | undefined
      let outcome: Record<string, unknown> = { status: 'ok' }
      if (step.op === 'file') writeFixture(step)
      if (step.op === 'chmod') chmodSync(directory, Number.parseInt(step.mode, 8))
      if (step.op === 'release' && !leases.has(step.lease)) {
        throw new Error(`unknown fixture lease ${step.lease}`)
      }
      try {
        if (step.op === 'acquire' || step.op === 'guard') {
          // Keep '/./' in the supplied path so the alias reaches the production entry.
          const database = `${directory}/${step.db ?? 'matou.sqlite'}`
          owner = `${database}.owner`
          if (step.op === 'acquire') {
            const path = owner
            if (step.runtimeLease) {
              const handle = RuntimeDatabase.acquireOwnership(database)
              generations.set(handle.runtimeGeneration, step.generation)
              leases.set(step.lease, { owner, release: () => handle.release() })
            } else {
              acquireDatabaseOwner(path, step.generation)
              leases.set(step.lease, { owner, release: () => releaseDatabaseOwner(path, step.generation) })
            }
          } else {
            RuntimeDatabase.assertNoLiveOwner(database)
          }
        }
        if (step.op === 'release') {
          const lease = leases.get(step.lease)!
          owner = lease.owner
          lease.release()
        }
      } catch (e) {
        const error = e as Error & { code?: string; issue?: string; statePath?: string }
        outcome = {
          status: 'error', message: display(error.message), code: error.code ?? null,
          issue: error.issue ?? null, statePath: error.statePath ? display(error.statePath) : null
        }
      }
      const state = snapshot()
      const record = owner ? readDatabaseOwner(owner) : undefined
      return {
        op: step.op, ...outcome, ...state,
        record: record ? {
          ...record, pid: pidToken(record.pid) ?? record.pid,
          runtimeGeneration: normalizeGeneration(record.runtimeGeneration)
        } : null
      }
    })
    return { id, initial, steps: observations, afterHandlesDropped: snapshot() }
  })
}
