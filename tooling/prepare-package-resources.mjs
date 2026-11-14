import { access, chmod, cp, mkdir, readFile, readdir, rm, writeFile } from 'node:fs/promises'
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
await cp(join(root, 'packages/contracts/third-party/provider-presets-LICENSE'), join(destination, 'provider-presets-LICENSE'))
await cp(join(root, 'packages/contracts/third-party/subscription-accounts-reference-LICENSE'), join(destination, 'subscription-accounts-reference-LICENSE'))
// These pure-JS agents are bundled into Runtime. Preserve the licenses of their
// installed dependency closure, including available optional peers bundled by esbuild.
const licenseQueue = ['https-proxy-agent', 'socks-proxy-agent'].map(name => ({ name, resolveFrom: require }))
const proxyLicenses = new Map()
while (licenseQueue.length) {
  const { name, resolveFrom } = licenseQueue.shift()
  let directory = dirname(resolveFrom.resolve(name))
  while (true) {
    try { await access(join(directory, 'package.json')); break } catch {
      const parent = dirname(directory)
      if (parent === directory) throw new Error(`Cannot find license package: ${name}`)
      directory = parent
    }
  }
  const packageFile = join(directory, 'package.json')
  const manifest = JSON.parse(await readFile(packageFile, 'utf8'))
  const id = `${manifest.name}@${manifest.version}`
  if (proxyLicenses.has(id)) continue
  const license = (await readdir(directory)).find(file => /^licen[cs]e(?:\..*)?$/i.test(file))
  if (!license) throw new Error(`Missing bundled proxy dependency license: ${id}`)
  proxyLicenses.set(id, `${id}\n${await readFile(join(directory, license), 'utf8')}`)
  const resolveDependency = createRequire(packageFile)
  const required = Object.keys(manifest.dependencies ?? {})
    .filter(dependency => !(dependency in (manifest.optionalDependencies ?? {})))
  for (const dependency of required) licenseQueue.push({ name: dependency, resolveFrom: resolveDependency })
  for (const dependency of new Set([
    ...Object.keys(manifest.optionalDependencies ?? {}),
    ...Object.keys(manifest.peerDependencies ?? {}),
    // debug probes this undeclared optional module; esbuild includes it when present.
    ...(manifest.name === 'debug' ? ['supports-color'] : [])
  ])) {
    try { resolveDependency.resolve(dependency) } catch { continue }
    licenseQueue.push({ name: dependency, resolveFrom: resolveDependency })
  }
}
await writeFile(join(destination, 'proxy-agents-LICENSES'), [...proxyLicenses].sort(([a], [b]) => a.localeCompare(b)).map(([, license]) => license).join('\n\n'))
const runtimeEntry = join(destination, 'index.cjs')
const bundledRuntime = await readFile(runtimeEntry, 'utf8')
await writeFile(
  runtimeEntry,
  bundledRuntime.replaceAll('require("node-pty")', 'require("./node-pty/lib/index.js")')
)

const requiredRuntimeResources = [
  'index.cjs',
  'mt-cli.cjs',
  'model-router-supervisor.cjs',
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

// Ship the pinned upstream account engine; end users need neither uv nor Python.
if (process.platform === 'darwin') {
  const { prepareModelRouter } = await import('./model-router/prepare.mjs')
  await prepareModelRouter({ packageResources: true })
}
await import('./claude-accounts/build.mjs')
