import { chmod, readdir, stat } from 'node:fs/promises'
import { createRequire } from 'node:module'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const require = createRequire(import.meta.url)
const nodePtyEntry = require.resolve('node-pty', { paths: [join(root, 'apps/runtime')] })
const nodePtyRoot = resolve(dirname(nodePtyEntry), '..')
const prebuildsRoot = join(nodePtyRoot, 'prebuilds')

for (const directory of await readdir(prebuildsRoot)) {
  if (!directory.startsWith('darwin-')) {
    continue
  }
  const helper = join(prebuildsRoot, directory, 'spawn-helper')
  const details = await stat(helper)
  if ((details.mode & 0o111) === 0) {
    await chmod(helper, 0o755)
    console.log(`Enabled executable permission for ${helper}`)
  }
}
