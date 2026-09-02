import { spawnSync } from 'node:child_process'

for (const [command, args] of [
  ['pnpm', ['build']],
  ['pnpm', ['exec', 'playwright', 'test', 'tests/e2e/runtime-recovery-scale.spec.ts', '--workers=1']]
]) {
  const result = spawnSync(command, args, { cwd: process.cwd(), env: process.env, stdio: 'inherit' })
  if (result.error) throw result.error
  if (result.status !== 0) process.exit(result.status ?? 1)
}
