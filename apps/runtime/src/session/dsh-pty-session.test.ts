import { chmod, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { expect, it } from 'vitest'
import { PtySession } from './pty-session'

it('runs the managed DSH PTY with the generated observation patch and exact resume target', async () => {
  const root = await mkdtemp(join(tmpdir(), 'matou-dsh-launch-'))
  const executable = join(root, 'dsh-stub.cjs'), patch = join(root, 'run.patch.yml')
  const old = process.env.MATOU_DSH_COMMAND
  let session: PtySession | undefined, output = ''
  await mkdir(join(root, 'assets'), { recursive: true })
  await writeFile(executable, `#!/usr/bin/env node
if (process.argv.includes('--dump-config')) {
  console.log('- id: dsh-tui\\n  inject:\\n    - agents\\n  config:\\n    model: native-model\\n    backend: codex');
} else {
  console.log('DSH_ARGV=' + JSON.stringify(process.argv.slice(2)));
  setInterval(() => {}, 1000);
}
`)
  await chmod(executable, 0o755)
  process.env.MATOU_DSH_COMMAND = executable
  try {
    session = await PtySession.create({ sessionId: 'dsh', executionContextId: 'context', profile: 'dsh',
      cwd: root, dataRoot: root, cols: 100, rows: 30, runId: 'run', providerSessionId: 'native-exact',
      settingsPath: patch, controlAssetRoot: join(root, 'assets'),
      env: { MATOU_PROVIDER_HOOK_URL: 'http://127.0.0.1:1/hooks/test' },
      onOutput: chunk => { output += chunk } })
    const deadline = Date.now() + 3000
    while (!output.includes('DSH_ARGV=') && Date.now() < deadline) await new Promise(resolve => setTimeout(resolve, 10))
    expect(output).toContain(JSON.stringify(['dsh-tui', '--patch', patch, '--resume', 'native-exact']))
    const content = await readFile(patch, 'utf8')
    expect(content).toContain('backend: dsh')
    expect(content).toContain('taskNotifications: "off"')
    expect(content).toContain('model: native-model')
    expect(content).toContain(join(root, 'assets', 'providers', 'dsh', 'bridge.mjs'))
  } finally {
    await session?.shutdownForRuntime({ gracePeriodMs: 100, hardKillWaitMs: 1000 })
    if (old === undefined) delete process.env.MATOU_DSH_COMMAND
    else process.env.MATOU_DSH_COMMAND = old
    await rm(root, { recursive: true, force: true })
  }
})
