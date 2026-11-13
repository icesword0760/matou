import { spawn } from 'node:child_process'
import { copyFile } from 'node:fs/promises'
import { createRequire } from 'node:module'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { build } from 'tsup'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..')
const output = join(root, 'output/model-router-stage2')
await build({
  entry: { 'runtime-owner-probe': join(root, 'tooling/model-router/runtime-owner-probe.ts'), 'runtime-fixture': join(root, 'tooling/model-router/runtime-fixture.ts') },
  outDir: output, format: ['cjs'], outExtension: () => ({ js: '.cjs' }),
  platform: 'node', target: 'node22', external: ['electron'], noExternal: ['@matou/contracts'],
  clean: false
})
await copyFile(join(root, 'apps/runtime/dist/model-router-supervisor.cjs'), join(output, 'model-router-supervisor.cjs'))
const require = createRequire(import.meta.url)
const child = spawn(require('electron'), [join(output, 'runtime-owner-probe.cjs')], {
  cwd: root, stdio: 'inherit', env: { ...process.env, ELECTRON_RUN_AS_NODE: undefined, MATOU_PROBE_PROJECT: root }
})
const timer = setTimeout(() => {
  child.kill('SIGTERM')
  setTimeout(() => child.kill('SIGKILL'), 2_000).unref()
}, 35_000)
child.once('exit', code => { clearTimeout(timer); process.exitCode = code ?? 1 })
