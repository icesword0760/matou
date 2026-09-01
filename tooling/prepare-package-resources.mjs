import { access, chmod, cp, mkdir, readFile, rm, writeFile } from 'node:fs/promises'
import { createRequire } from 'node:module'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const destination = join(root, 'apps', 'desktop', 'package-resources', 'runtime')
const require = createRequire(join(root, 'apps', 'runtime', 'package.json'))
const nodePtyRoot = resolve(dirname(require.resolve('node-pty/package.json')))

await rm(join(root, 'apps', 'desktop', 'package-resources'), { recursive: true, force: true })
await mkdir(destination, { recursive: true })
await cp(join(root, 'apps', 'runtime', 'dist'), destination, { recursive: true })
await cp(nodePtyRoot, join(destination, 'node-pty'), { recursive: true })
const runtimeEntry = join(destination, 'index.cjs')
const bundledRuntime = await readFile(runtimeEntry, 'utf8')
await writeFile(
  runtimeEntry,
  bundledRuntime.replaceAll('require("node-pty")', 'require("./node-pty/lib/index.js")')
)

const requiredRuntimeResources = [
  'index.cjs',
  'mt-cli.cjs',
  'control-assets/bin/mt',
  'control-assets/bin/mt.cmd',
  'control-assets/providers/host-control.md',
  'control-assets/providers/claude-plugin/.claude-plugin/plugin.json',
  'control-assets/providers/claude-plugin/skills/mt-terminal/SKILL.md',
  'control-assets/providers/codex-developer-instructions.md'
]
await Promise.all(requiredRuntimeResources.map((path) => access(join(destination, path))))
if (process.platform !== 'win32') {
  await chmod(join(destination, 'control-assets', 'bin', 'mt'), 0o755)
}
