import { spawnSync } from 'node:child_process'
import { cp, mkdir } from 'node:fs/promises'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
const root = dirname(fileURLToPath(import.meta.url))
const destination = join(root, '../../apps/desktop/package-resources/claude-accounts')
const result = spawnSync('uv', ['run', '--frozen', '--project', root, 'pyinstaller', '--noconfirm', '--clean', '--onedir', '--name', 'matou-claude-accounts', '--collect-submodules', 'claude_swap', '--collect-all', 'truststore', '--distpath', destination, '--workpath', join(root, 'build'), '--specpath', join(root, 'build'), join(root, 'bridge.py')], { stdio: 'inherit' })
if (result.error || result.status !== 0) throw new Error('Account engine packaging failed; install uv and retry.')
await mkdir(destination, { recursive: true })
await cp(join(root, 'UPSTREAM-LICENSE'), join(destination, 'UPSTREAM-LICENSE'))
