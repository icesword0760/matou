// Cross-language contract from frozen source on disposable SQLite only.
import { createHash } from 'node:crypto'
import { mkdtempSync, readFileSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { tmpdir } from 'node:os'
import { RuntimeDatabase } from '../../apps/runtime/src/storage/database'
import { FOUNDATION_MIGRATIONS } from '../../apps/runtime/src/storage/migrations'
import { MigrationRunner } from '../../apps/runtime/src/storage/migration-runner'
import { ShellCommandBlockCollector, ShellHistoryRepository, encodeShellCommandMarker,
  formatShellHistoryForTerminal } from '../../apps/runtime/src/shell-history/shell-history'
import type { CollectedShellBlock, ShellHistoryBlock } from '../../apps/runtime/src/shell-history/shell-history'

type Part = string | { repeat: string, count: number } | { lines: number, start: number, trailing: boolean }
const expand = (parts: Part[]) => parts.map(p => typeof p === 'string' ? p : 'repeat' in p ? p.repeat.repeat(p.count)
  : Array.from({ length: p.lines }, (_, i) => `line-${i + p.start}`).join('\n') + (p.trailing ? '\n' : '')).join('')

type GateOperation =
  | { op: 'ingest', parts: Part[], now: number }
  | { op: 'insert', block: CollectedShellBlock, sessionId: string }
  | { op: 'list' | 'purge', sessionId: string }
  | { op: 'launch', sessionId: string, enabled: boolean }
  | { op: 'reopen' }

async function exportGate(inputPath: string, outputPath: string) {
  const input: { seed: string[], cases: { name: string, operations: GateOperation[] }[] } =
    JSON.parse(readFileSync(inputPath, 'utf8'))
  const cases = []
  for (const [index, test] of input.cases.entries()) {
    const file = join(dirname(outputPath), `frozen-${index}.sqlite`)
    let db = RuntimeDatabase.open(file)
    try {
      await new MigrationRunner(db, FOUNDATION_MIGRATIONS).migrate()
      input.seed.forEach(sql => db.run(sql))
      let repository = new ShellHistoryRepository(db)
      let collector = new ShellCommandBlockCollector()
      const ids = new Map<string, string>()
      let writes = 0
      const normalized = (row: ShellHistoryBlock) => {
        const id = ids.get(row.id)
        if (id === undefined) throw new Error('Unmapped history row ID')
        return { ...row, id }
      }
      const insert = (block: CollectedShellBlock, sessionId: string) => {
        const id = `row-${++writes}`
        try {
          const row = repository.complete({ ...block, sessionId, cwd: '/tmp/workspace' })
          ids.set(row.id, id)
          return { row: normalized(row), error: null }
        } catch (error) {
          if (!(error instanceof Error) || !('errcode' in error)) throw error
          return { row: null, error: { sqliteCode: error.errcode, message: error.message } }
        }
      }
      const steps = test.operations.map(operation => {
        switch (operation.op) {
          case 'ingest': {
            const completed = collector.ingest(expand(operation.parts), operation.now)
            return { completed, writes: completed.map(block => insert(block, 's')) }
          }
          case 'insert':
            return insert(operation.block, operation.sessionId)
          case 'list':
            return repository.list(operation.sessionId).map(normalized)
          case 'launch': {
            const rows = repository.listForLaunch(operation.sessionId, operation.enabled)
            return { rows: rows.map(normalized), payload: formatShellHistoryForTerminal(rows) }
          }
          case 'purge':
            repository.purge(operation.sessionId)
            return null
          case 'reopen':
            db.close()
            db = RuntimeDatabase.open(file)
            repository = new ShellHistoryRepository(db)
            collector = new ShellCommandBlockCollector()
            return null
        }
      })
      cases.push({ name: test.name, steps })
    } finally {
      db.close()
    }
  }
  writeFileSync(outputPath, JSON.stringify({ cases }, null, 2) + '\n')
}

if (process.argv[2] === '--gate') {
  await exportGate(process.argv[3]!, process.argv[4]!)
  process.exit(0)
}

const describe = (output: string) => ({ sha256: createHash('sha256').update(output).digest('hex'),
  utf16Length: output.length, lines: output.split('\n').length, prefix: output.slice(0, 40), suffix: output.slice(-40) })
const marker = encodeShellCommandMarker('printf "你好 🙂"')
const end = '\x1b]133;D;0\x07'
const cases: { name: string, chunks: { parts: Part[], now: number }[] }[] = [
  { name: 'split-markers', chunks: [
    { parts: [marker.slice(0, 9)], now: 10 }, { parts: [marker.slice(9), '你好\r\n'], now: 11 },
    { parts: ['\x1b]133;D;'], now: 12 }, { parts: ['0\x07\x1b]133;A\x07'], now: 13 }] },
  { name: 'unfinished-replaced', chunks: [
    { parts: [encodeShellCommandMarker('sleep 10'), 'partial'], now: 1 },
    { parts: [encodeShellCommandMarker('echo next'), 'next\r\n', end], now: 2 }] },
  { name: 'st-and-style', chunks: [
    { parts: [encodeShellCommandMarker('false').replace('\x07', '\x1b\\'), '\x1b]133;C\x07', '\x1b[31mred\x1b[0m\n', '\x1b]2;title\x07', '\x1b]133;D;7\x1b\\'], now: 3 }] },
  { name: 'stream-line-limit', chunks: [
    { parts: [encodeShellCommandMarker('large output')], now: 1 },
    { parts: [{ lines: 3000, start: 0, trailing: true }], now: 2 },
    { parts: [{ lines: 3005, start: 3000, trailing: true }], now: 3 }, { parts: [end], now: 4 }] },
  { name: 'stream-character-limit', chunks: [
    { parts: [encodeShellCommandMarker('large'), 'old-', { repeat: '🙂', count: 600000 }, '-new', end], now: 5 }] },
  { name: 'unterminated-osc-limit', chunks: [
    { parts: [encodeShellCommandMarker('first'), '\x1b]title;', { repeat: 'x', count: 2 * 1024 * 1024 }], now: 1 },
    { parts: ['\x07', encodeShellCommandMarker('second'), 'done\r\n', end], now: 2 }] },
  { name: 'interrupted-not-complete', chunks: [{ parts: [encodeShellCommandMarker('sleep 10'), 'partial'], now: 6 }] },
]
const collectorCases = cases.map(test => {
  const collector = new ShellCommandBlockCollector()
  return { name: test.name, steps: test.chunks.map(chunk => ({ ...chunk,
    completed: collector.ingest(expand(chunk.parts), chunk.now).map(({ output, ...block }) => ({ ...block, output: describe(output) })) })) }
})
const root = mkdtempSync(join(tmpdir(), 'matou-history-contract-'))
const file = join(root, 'reference.sqlite')
let db = RuntimeDatabase.open(file)
await new MigrationRunner(db, FOUNDATION_MIGRATIONS).migrate()
const seed = [
  "INSERT INTO workspaces (id,name,root_directory,created_at,updated_at) VALUES ('w','Workspace','/tmp/workspace',1,1)",
  "INSERT INTO execution_contexts (id,workspace_id,kind,cwd,created_at) VALUES ('e','w','plain-directory','/tmp/workspace',1)",
  "INSERT INTO tasks (id,workspace_id,execution_context_id,title,status,sort_key,created_at,updated_at) VALUES ('t','w','e','Task','active','a',1,1)",
  "INSERT INTO sessions (id,task_id,execution_context_id,kind,status,title,created_at,updated_at,last_activity_at) VALUES ('s','t','e','shell','running','Shell',1,1,1)"
]
seed.forEach(sql => db.run(sql))
let repository = new ShellHistoryRepository(db)
const inputs = Array.from({ length: 105 }, (_, index) => {
  const i = index + 1
  return { sessionId: 's', command: `printf command-${i}`, cwd: '/tmp/workspace', output: `output-${i}\r\n`, exitCode: 0, startedAt: i * 10, completedAt: i * 10 + 1 }
})
const completed = inputs.map(input => repository.complete(input))
db.close()
db = RuntimeDatabase.open(file)
repository = new ShellHistoryRepository(db)
const reopened = repository.list('s')
const disabled = repository.listForLaunch('s', false)
const limitCases = [
  { name: 'line-limit', parts: [{ lines: 5010, start: 1, trailing: false }] as Part[] },
  { name: 'character-limit', parts: ['old-', { repeat: 'x', count: 1024 * 1024 }, '-new'] as Part[] },
]
const limits = limitCases.map(test => {
  const input = { sessionId: 's', command: test.name, cwd: '/tmp/workspace', exitCode: 0, startedAt: 2000, completedAt: 2001 }
  const block = repository.complete({ ...input, output: expand(test.parts) })
  return { ...test, input, id: block.id, expectedOutput: describe(block.output) }
})
const formatBlocks = completed.slice(0, 2).map((b, i) => ({ ...b, output: i ? 'one\ntwo' : 'hello\r\n' }))
const formatted = formatShellHistoryForTerminal(formatBlocks)
repository.purge('s')
const purged = repository.list('s')
db.close()
const fixture = { source: 'Frozen shell-history.ts on captured Electron; original completion IDs for native replay. Output hashes use UTF-8; limits count UTF-16 units.',
  collectorCases, repository: { seed, inputs, ids: completed.map(b => b.id), reopened, disabled, limits, purged },
  formatting: { blocks: formatBlocks, result: formatted, empty: formatShellHistoryForTerminal([]) } }
writeFileSync('apps/native/core/tests/fixtures/shell-history-reference.json', JSON.stringify(fixture, null, 2) + '\n')
console.log(JSON.stringify({ root, cases: collectorCases.length, blocks: inputs.length }))
