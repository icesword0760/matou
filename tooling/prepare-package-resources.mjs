import { cp, mkdir, readFile, rm, writeFile } from 'node:fs/promises'
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
