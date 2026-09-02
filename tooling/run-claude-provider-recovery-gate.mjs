import { spawnSync } from 'node:child_process'

const steps = [
  ['pnpm', ['check:identifiers']],
  ['pnpm', ['build:packages']],
  ['pnpm', ['typecheck']],
  ['pnpm', ['--filter', '@matou/contracts', 'test', '--', '--run']],
  ['pnpm', ['--filter', '@matou/runtime', 'exec', 'vitest', 'run',
    'src/domain/session-repository.test.ts',
    'src/session-canvas/provider-mode-service.test.ts',
    'src/runtime-server.test.ts']],
  ['pnpm', ['--filter', '@matou/desktop', 'exec', 'vitest', 'run',
    'src/renderer/src/hierarchy/TerminalPane.test.tsx',
    'src/renderer/src/hierarchy/hierarchy-components.test.tsx',
    'src/renderer/src/notifications/agent-event-ingestion.test.ts',
    'src/renderer/src/notifications/notification-ui-integration.test.tsx']],
  ['pnpm', ['build:runtime']],
  ['pnpm', ['build:desktop']],
  ['pnpm', ['exec', 'playwright', 'test',
    'tests/e2e/claude-provider-storage-recovery.spec.ts', '--workers=1', '--reporter=line']]
]

for (const [command, args] of steps) {
  const result = spawnSync(command, args, {
    cwd: process.cwd(), env: process.env, stdio: 'inherit'
  })
  if (result.error) throw result.error
  if (result.status !== 0) process.exit(result.status ?? 1)
}
