import { spawnSync } from 'node:child_process'
import { cp } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

import { accountEngineSourceDigest, writeAccountEngineManifest } from './artifact.mjs'

const root = dirname(fileURLToPath(import.meta.url))
const engine = 'matou-claude-accounts'
const defaultDestination = process.platform === 'win32'
  ? join(root, 'dist')
  : join(root, '../../apps/native/package-resources/claude-accounts')

export async function buildAccountEngine(destination = defaultDestination) {
  const inputDigest = process.platform === 'darwin' ? await accountEngineSourceDigest() : undefined
  const result = spawnSync('uv', ['run', '--frozen', '--project', root, 'pyinstaller', '--noconfirm', '--clean',
    '--onedir', '--name', engine, '--collect-submodules', 'claude_swap', '--collect-all', 'truststore',
    '--distpath', destination, '--workpath', join(root, 'build'), '--specpath', join(root, 'build'), join(root, 'bridge.py')
  ], { stdio: 'inherit' })
  if (result.error || result.status !== 0) throw new Error('Account engine packaging failed; install uv and retry.')
  const engineDirectory = join(destination, engine)
  await cp(join(root, 'UPSTREAM-LICENSE'), join(engineDirectory, 'UPSTREAM-LICENSE'))
  if (inputDigest) await writeAccountEngineManifest(engineDirectory, inputDigest)
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  await buildAccountEngine()
}
