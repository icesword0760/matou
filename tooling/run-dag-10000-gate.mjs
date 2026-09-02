import { spawnSync } from 'node:child_process'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const steps = [
  ['check project identifiers', ['check:identifiers']],
  ['build shared packages', ['build:packages']],
  ['typecheck workspace', ['typecheck']],
  ['test Runtime graph lookup migration', [
    '--filter', '@matou/runtime', 'exec', 'vitest', 'run',
    'src/storage/migration-runner.test.ts',
    'src/session-canvas/session-graph-repository.test.ts',
    '--disableConsoleIntercept'
  ]],
  ['test DAG layout and interaction units', [
    '--filter', '@matou/desktop', 'exec', 'vitest', 'run',
    'src/main/dag-window-manager.test.ts',
    'src/renderer/src/dag/dag-layout.test.ts',
    'src/renderer/src/dag/dag-render-model.test.ts',
    'src/renderer/src/dag/DagCanvas.test.tsx',
    'src/renderer/src/dag/DagSearch.test.tsx',
    'src/renderer/src/dag/DagWindowApp.test.tsx',
    'src/renderer/src/hierarchy/HierarchyShell.test.tsx',
    '--disableConsoleIntercept'
  ]],
  ['build Runtime and Electron app', ['build:runtime']],
  ['build Electron renderer', ['build:desktop']],
  ['run real Electron 10,000-node DAG acceptance', [
    'exec', 'playwright', 'test',
    'tests/e2e/scale/dag-10000-electron.spec.ts',
    '--workers=1', '--reporter=line'
  ]]
]

for (const [label, args] of steps) {
  process.stdout.write(`\n[dag-10000-gate] ${label}\n`)
  const result = spawnSync('pnpm', args, {
    cwd: root,
    env: process.env,
    stdio: 'inherit'
  })
  if (result.error) throw result.error
  if (result.status !== 0) process.exit(result.status ?? 1)
}

process.stdout.write('\n[dag-10000-gate] all checks passed\n')
