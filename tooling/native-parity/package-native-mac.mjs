import { execFile } from 'node:child_process'
import { constants } from 'node:fs'
import {
  access, chmod, copyFile, cp, lstat, mkdir, mkdtemp, open, readFile, readdir, realpath, rename, rm, writeFile
} from 'node:fs/promises'
import { basename, dirname, join, resolve, sep } from 'node:path'
import { fileURLToPath } from 'node:url'
import { isDeepStrictEqual, parseArgs, promisify } from 'node:util'

import { verifyInstallSource } from '../install-macos-app.mjs'

const desktopProject = fileURLToPath(new URL('../../apps/desktop/', import.meta.url))
const accountEngine = 'matou-claude-accounts'
const accountEnginePath = join('claude-accounts', accountEngine)
const accountBuild = fileURLToPath(new URL('../../tooling/claude-accounts/build/', import.meta.url))
const claudePlugin = 'control-assets/providers/claude-plugin'
const runtimeProject = fileURLToPath(new URL('../../apps/runtime/', import.meta.url))
const desktopName = 'matou-native-desktop'
const runtimeName = 'matou-native-runtime'
const iconName = 'icon.icns'
const entitlementsName = 'entitlements.mac.plist'
const teamId = 'JAKX72KQ6X'
// The frozen reference bundle's generated Info.plist declares macOS 12.0.
const minimumSystemVersion = '12.0'
const exec = promisify(execFile)
const usage = `Usage:
  node tooling/native-parity/package-native-mac.mjs --desktop-bin <file> --runtime-bin <file>
      --out-dir <directory> --version <X.Y.Z> [--app-name <name>] [--bundle-id <id>]
      [--entitlements <plist>] [--claude-accounts-engine <directory>] [--unsigned]
  node tooling/native-parity/package-native-mac.mjs --verify <path.app>
      [--app-name <name>] [--bundle-id <id>] [--require-release-signature]

The defaults are 码头 and com.matou.desktop. Assembly copies the binaries as
supplied and adds no signature. --unsigned explicitly selects that same offline mode.
The Finder workflow uses the selected app name and bundle ID.
Use a new output directory or remove an old build before assembling again.
Verification reads the bundle without launching it.`

async function run(command, args) {
  const { stdout, stderr } = await exec(command, args, { env: { ...process.env, LC_ALL: 'C' } })
  return { stdout, stderr }
}

async function readPlist(path) {
  try {
    const { stdout } = await run('/usr/bin/plutil', ['-convert', 'json', '-o', '-', '--', path])
    const plist = JSON.parse(stdout)
    if (!plist || typeof plist !== 'object' || Array.isArray(plist)) throw new Error('expected a dictionary')
    return plist
  } catch (error) {
    throw new Error(`Invalid plist dictionary ${path}: ${error.stdout?.trim() || error.message}`)
  }
}

async function requirePath(path, label, kind = 'file') {
  const stat = await lstat(path).catch(error => {
    if (error.code === 'ENOENT' || error.code === 'ENOTDIR') throw new Error(`Missing ${label}: ${path}`)
    throw error
  })
  if (kind === 'directory' ? !stat.isDirectory() : !stat.isFile() || stat.size === 0) {
    throw new Error(`${label} must be a ${kind === 'file' ? 'non-empty regular file' : 'real directory'}: ${path}`)
  }
  return stat
}

async function requireExecutable(path, label, packaged = false) {
  const { mode } = await requirePath(path, label)
  if (packaged && (mode & 0o7777) !== 0o755) {
    throw new Error(`${label} must have mode 0755: ${path}`)
  }
  if (!(mode & 0o111) || !await access(path, constants.X_OK).then(() => true, () => false)) {
    throw new Error(`${label} must be executable: ${path}`)
  }
}

async function sourceFile(path, label) {
  const source = await realpath(path).catch(error => {
    if (error.code === 'ENOENT' || error.code === 'ENOTDIR') throw new Error(`Missing ${label}: ${path}`)
    throw error
  })
  await requirePath(source, label)
  return source
}

// Read only Mach-O headers and the architecture table, even for large release binaries.
async function machoArchitectures(path) {
  const file = await open(path, 'r')
  const invalid = () => new Error(`Expected a macOS arm64 or x86_64 Mach-O executable: ${path}`)
  try {
    const { size } = await file.stat()
    async function read(length, offset) {
      const bytes = Buffer.alloc(length)
      if ((await file.read(bytes, 0, length, offset)).bytesRead !== length) throw invalid()
      return bytes
    }
    async function slice(offset, length, expectedCpu) {
      if (length < 32 || offset > size - length) throw invalid()
      const header = await read(32, offset)
      const cpu = header.readUInt32LE(4)
      const subtype = header.readUInt32LE(8) & 0x00ffffff
      const commands = header.readUInt32LE(16)
      const commandBytes = header.readUInt32LE(20)
      if (header.readUInt32LE(0) !== 0xfeedfacf || header.readUInt32LE(12) !== 2 ||
          (expectedCpu !== undefined && expectedCpu !== cpu) ||
          commands === 0 || commandBytes < commands * 8 || commandBytes > length - 32) {
        throw invalid()
      }
      if (cpu === 0x0100000c) return subtype === 2 ? 'arm64e' : 'arm64'
      if (cpu === 0x01000007) return subtype === 8 ? 'x86_64h' : 'x86_64'
      throw invalid()
    }

    const header = await read(8, 0)
    const magic = header.readUInt32BE(0)
    if (magic !== 0xcafebabe && magic !== 0xcafebabf) return [await slice(0, size)]
    const wide = magic === 0xcafebabf
    const count = header.readUInt32BE(4)
    const entrySize = wide ? 32 : 20
    const tableSize = count * entrySize
    if (count < 1 || count > 32 || tableSize > size - 8) throw invalid()
    const table = await read(tableSize, 8)
    const architectures = []
    for (let entry = 0; entry < tableSize; entry += entrySize) {
      const cpu = table.readUInt32BE(entry)
      const offset = wide ? Number(table.readBigUInt64BE(entry + 8)) : table.readUInt32BE(entry + 8)
      const length = wide ? Number(table.readBigUInt64BE(entry + 16)) : table.readUInt32BE(entry + 12)
      if (offset < 8 + tableSize) throw invalid()
      architectures.push(await slice(offset, length, cpu))
    }
    return architectures
  } finally {
    await file.close()
  }
}

async function signatureSummary(path) {
  try {
    const { stdout, stderr } = await run('/usr/bin/codesign', ['--display', '--verbose=4', '-r-', path])
    const output = `${stdout}\n${stderr}`
    const field = name => output.match(new RegExp(`^${name}=(.+)$`, 'm'))?.[1] ?? null
    const authorities = [...output.matchAll(/^Authority=(.+)$/gm)].map(match => match[1])
    let status = 'other'
    if (field('Signature') === 'adhoc') status = 'ad-hoc'
    else if (authorities[0]?.startsWith('Developer ID Application: ')) status = 'developer-id'
    return {
      status,
      identifier: field('Identifier'),
      teamIdentifier: field('TeamIdentifier'),
      format: field('Format'),
      authorities,
      designatedRequirement: output.match(/^(?:# )?designated => (.+)$/m)?.[1] ?? null
    }
  } catch (error) {
    const detail = error.stderr?.trim() || error.message
    const status = detail.includes('code object is not signed at all') ? 'unsigned' : 'unavailable'
    return { status, detail }
  }
}

async function optionalPath(path) {
  return lstat(path).catch(error => {
    if (error.code === 'ENOENT') return null
    throw error
  })
}

async function accountEngineSource(explicit) {
  if (explicit !== undefined) return realpath(explicit)
  const built = join(accountBuild, accountEngine)
  if (!await optionalPath(join(built, accountEngine))) return null
  // PyInstaller's work directory contains an executable but keeps its
  // dynamic libraries in the collected distribution.
  if (await optionalPath(join(built, 'COLLECT-00.toc')) &&
      !await optionalPath(join(built, '_internal'))) {
    const distribution = join(desktopProject, 'package-resources', accountEnginePath)
    return await optionalPath(join(distribution, accountEngine)) ? distribution : null
  }
  return built
}

async function verifyEngineCode(directory, requirement) {
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name)
    if (entry.isDirectory()) {
      await verifyEngineCode(path, requirement)
    } else if (entry.isFile()) {
      const file = await open(path, 'r')
      const magic = Buffer.alloc(4)
      try {
        const { bytesRead } = await file.read(magic)
        if (bytesRead !== 4) continue
      } finally {
        await file.close()
      }
      if ([0xfeedface, 0xcefaedfe, 0xfeedfacf, 0xcffaedfe, 0xcafebabe, 0xcafebabf]
        .includes(magic.readUInt32BE())) {
        await run('/usr/bin/codesign', ['--verify', '--strict', '-R', `=${requirement}`, path])
      }
    }
  }
}

async function requireReleaseSignature(bundle, runtime, engine, signatures, config, bundleId) {
  for (const [label, signature] of Object.entries(signatures)) {
    if (signature.status !== 'developer-id' || signature.teamIdentifier !== teamId ||
        signature.authorities[0] !== `Developer ID Application: ${config.build.mac.identity}`) {
      throw new Error(`${label} requires the Matou Developer ID signature (Team ${teamId}); found ${signature.status}`)
    }
  }
  const runtimeRequirement = [
    'anchor apple generic',
    'certificate 1[field.1.2.840.113635.100.6.2.6] exists',
    'certificate leaf[field.1.2.840.113635.100.6.1.13] exists',
    `certificate leaf[subject.OU] = "${teamId}"`
  ].join(' and ')
  if (bundleId === config.build.appId) {
    // Preserve the production installation identity and deep seal checks.
    await verifyInstallSource(bundle)
  } else {
    const requirement = `identifier "${bundleId}" and ${runtimeRequirement}`
    const clauses = value => value?.replaceAll(/\/\*\s*exists\s*\*\//g, 'exists')
      .replaceAll(/"([\w.-]+)"/g, '$1').trim().split(/\s+and\s+/).sort()
    if (signatures.app.identifier !== bundleId ||
        !isDeepStrictEqual(clauses(signatures.app.designatedRequirement), clauses(requirement))) {
      throw new Error(`App signature requires the designated requirement for ${bundleId}`)
    }
    await run('/usr/bin/codesign', ['--verify', '--deep', '--strict', bundle])
    await run('/usr/bin/codesign', ['--verify', '-R', `=${requirement}`, bundle])
  }
  await run('/usr/bin/codesign', ['--verify', '--strict', '-R', `=${runtimeRequirement}`, runtime])
  if (engine) await verifyEngineCode(dirname(engine), runtimeRequirement)
}

function validateVersion(version) {
  if (typeof version !== 'string' || !/^\d+\.\d+\.\d+$/.test(version)) {
    throw new Error('Version must have three numeric components (X.Y.Z)')
  }
}

function validateAppName(name) {
  if (typeof name !== 'string' || !name.trim() || name !== name.trim() ||
      name === '.' || name === '..' || /[\u0000-\u001f\u007f/\\:]/.test(name) ||
      name.toLowerCase() === runtimeName) {
    throw new Error('Invalid --app-name: expected a single name distinct from the runtime executable')
  }
}

function manifest(config, bundleId, version, appName, executableName) {
  return {
    CFBundleIdentifier: bundleId,
    CFBundleExecutable: executableName,
    CFBundleName: appName,
    CFBundleDisplayName: appName,
    CFBundlePackageType: 'APPL',
    CFBundleInfoDictionaryVersion: '6.0',
    CFBundleShortVersionString: version,
    CFBundleVersion: version,
    CFBundleIconFile: iconName,
    LSApplicationCategoryType: config.build.mac.category,
    LSMinimumSystemVersion: minimumSystemVersion,
    CFBundleDocumentTypes: config.build.mac.extendInfo.CFBundleDocumentTypes,
    NSHighResolutionCapable: true,
    NSPrincipalClass: 'NSApplication'
  }
}

const workflowFiles = ['Contents/Info.plist', 'Contents/Resources/document.wflow']

function workflowName(appName) {
  return `进入${appName}`
}

function workflowPath(resources, workflow, appName) {
  return join(resources, dirname(workflow.to), `${workflowName(appName)}.workflow`)
}

async function verifyLayout(bundle, config, bundleId, workflow, requestedAppName) {
  const contents = join(bundle, 'Contents')
  const macos = join(contents, 'MacOS')
  const resources = join(contents, 'Resources')
  for (const path of [bundle, contents, macos, resources]) {
    await requirePath(path, 'bundle directory', 'directory')
  }

  const infoPath = join(contents, 'Info.plist')
  await requirePath(infoPath, 'Info.plist')
  const info = await readPlist(infoPath)
  const appName = requestedAppName ?? info.CFBundleName ?? basename(bundle, '.app')
  validateAppName(appName)
  const executableName = appName === config.productName && info.CFBundleExecutable === desktopName
    ? desktopName : appName
  validateVersion(info.CFBundleShortVersionString)
  const expectedInfo = manifest(config, bundleId, info.CFBundleShortVersionString, appName, executableName)
  for (const [key, expected] of Object.entries(expectedInfo)) {
    if (!isDeepStrictEqual(info[key], expected)) {
      throw new Error(`Invalid Info.plist ${key}: expected ${JSON.stringify(expected)}`)
    }
  }

  await requirePath(join(resources, iconName), 'icon resource')
  const entitlements = join(resources, entitlementsName)
  await requirePath(entitlements, 'entitlements resource')
  await readPlist(entitlements)
  const bundledWorkflow = workflowPath(resources, workflow, appName)
  for (const path of [
    dirname(bundledWorkflow), bundledWorkflow, join(bundledWorkflow, 'Contents'),
    join(bundledWorkflow, 'Contents/Resources')
  ]) await requirePath(path, 'bundle directory', 'directory')
  const [finderInfo, finderDocument] = await Promise.all(workflowFiles.map(async file => {
    const path = join(bundledWorkflow, file)
    await requirePath(path, 'Finder workflow resource')
    return readPlist(path)
  }))
  const command = finderDocument.actions?.[0]?.action?.ActionParameters?.COMMAND_STRING
  if (finderInfo.CFBundleIdentifier !== `${bundleId}.finder-open-workspace` ||
      finderInfo.CFBundleName !== workflowName(appName) ||
      finderInfo.NSServices?.[0]?.NSMenuItem?.default !== workflowName(appName) ||
      typeof command !== 'string' || !command.includes(`/usr/bin/open -b ${bundleId} "$target"`)) {
    throw new Error(`Invalid Finder workflow name or bundle ID: ${bundledWorkflow}`)
  }

  const desktop = join(macos, executableName)
  const runtime = join(macos, runtimeName)
  await requireExecutable(desktop, 'desktop binary', true)
  await requireExecutable(runtime, 'runtime binary', true)
  const [desktopArchitectures, runtimeArchitectures] = await Promise.all([
    machoArchitectures(desktop), machoArchitectures(runtime)
  ])
  const engineDirectory = join(resources, accountEnginePath)
  let engine
  if (await optionalPath(join(resources, 'claude-accounts'))) {
    await requirePath(join(resources, 'claude-accounts'), 'account resources directory', 'directory')
    await requirePath(engineDirectory, 'account engine directory', 'directory')
    engine = join(engineDirectory, accountEngine)
    await requireExecutable(engine, 'account engine', true)
    await machoArchitectures(engine)
  }
  return { info, runtime, engine, architectures: { desktop: desktopArchitectures, runtime: runtimeArchitectures } }
}

async function canonicalOutput(path) {
  const missing = []
  let current = resolve(path)
  for (;;) {
    try {
      return join(await realpath(current), ...missing)
    } catch (error) {
      if (error.code !== 'ENOENT') throw error
      missing.unshift(basename(current))
      current = dirname(current)
    }
  }
}

function requireSafeOutput(path) {
  const normalized = path.toLowerCase()
  if (['/applications', '/system/applications', '/system/volumes/data/applications'].some(
    directory => normalized === directory || normalized.startsWith(`${directory}${sep}`)
  ) || normalized.split(sep).some(part => part.endsWith('.app'))) {
    throw new Error(`--out-dir cannot write to /Applications or inside an existing .app: ${path}`)
  }
}

async function requireAbsent(path) {
  try {
    await lstat(path)
  } catch (error) {
    if (error.code === 'ENOENT') return
    throw error
  }
  throw new Error(`Output bundle already exists: ${path}`)
}

async function assemble(options, config, bundleId, workflow) {
  const output = await canonicalOutput(options['out-dir'])
  requireSafeOutput(resolve(options['out-dir']))
  requireSafeOutput(output)
  const appName = options['app-name'] ?? config.productName
  const executableName = options['app-name'] ?? desktopName
  const bundleName = `${appName}.app`
  const destination = join(output, bundleName)
  await requireAbsent(destination)
  validateVersion(options.version)
  const desktop = await sourceFile(options['desktop-bin'], 'desktop binary')
  const runtime = await sourceFile(options['runtime-bin'], 'runtime binary')
  await requireExecutable(desktop, 'desktop binary')
  await requireExecutable(runtime, 'runtime binary')
  const engineSource = await accountEngineSource(options['claude-accounts-engine'])
  if (engineSource) {
    await requirePath(engineSource, 'account engine directory', 'directory')
    await requireExecutable(join(engineSource, accountEngine), 'account engine')
  }
  const icon = await sourceFile(join(desktopProject, 'build', iconName), 'icon resource')
  const entitlements = await sourceFile(
    options.entitlements ?? join(desktopProject, config.build.mac.entitlements), 'entitlements resource'
  )
  const finderFiles = await Promise.all(workflowFiles.map(file =>
    sourceFile(join(desktopProject, workflow.from, file), 'Finder workflow resource')
  ))

  await mkdir(output, { recursive: true })
  requireSafeOutput(await realpath(output))
  const staging = await mkdtemp(join(output, '.matou-native-mac-'))
  try {
    const bundle = join(staging, bundleName)
    const contents = join(bundle, 'Contents')
    const macos = join(contents, 'MacOS')
    const resources = join(contents, 'Resources')
    await mkdir(macos, { recursive: true })
    await mkdir(resources)
    for (const [source, name] of [[desktop, executableName], [runtime, runtimeName]]) {
      const path = join(macos, name)
      await copyFile(source, path)
      await chmod(path, 0o755)
    }
    await copyFile(icon, join(resources, iconName))
    await cp(join(runtimeProject, claudePlugin), join(resources, claudePlugin), { recursive: true })
    if (engineSource) {
      await cp(engineSource, join(resources, accountEnginePath), { recursive: true, verbatimSymlinks: true })
    }
    await copyFile(entitlements, join(resources, entitlementsName))
    const bundledWorkflow = workflowPath(resources, workflow, appName)
    for (const [index, file] of workflowFiles.entries()) {
      const path = join(bundledWorkflow, file)
      await mkdir(dirname(path), { recursive: true })
      if (appName === config.productName && bundleId === config.build.appId) {
        await copyFile(finderFiles[index], path)
      } else {
        const displayName = workflowName(appName).replaceAll('&', '&amp;')
          .replaceAll('<', '&lt;').replaceAll('>', '&gt;')
        const template = await readFile(finderFiles[index], 'utf8')
        await writeFile(path, template
          .replaceAll(config.build.appId, bundleId)
          .replaceAll(`<string>${workflowName(config.productName)}</string>`, () => `<string>${displayName}</string>`))
      }
    }
    const infoPath = join(contents, 'Info.plist')
    await writeFile(infoPath, JSON.stringify(manifest(config, bundleId, options.version, appName, executableName)))
    await run('/usr/bin/plutil', ['-convert', 'xml1', '--', infoPath])

    const { architectures } = await verifyLayout(bundle, config, bundleId, workflow, appName)
    await requireAbsent(destination)
    await rename(bundle, destination)
    return { path: destination, bundleId, version: options.version, architectures }
  } finally {
    await rm(staging, { recursive: true, force: true })
  }
}

async function main() {
  const input = ['desktop-bin', 'runtime-bin', 'out-dir', 'version']
  const { values } = parseArgs({ options: {
    ...Object.fromEntries([...input, 'app-name', 'bundle-id', 'entitlements', 'claude-accounts-engine', 'verify'].map(name => [name, { type: 'string' }])),
    unsigned: { type: 'boolean' },
    'require-release-signature': { type: 'boolean' },
    help: { type: 'boolean', short: 'h' }
  } })
  if (values.help) return console.log(usage)
  if (process.platform !== 'darwin') throw new Error('This toolkit requires macOS')
  const verifying = values.verify !== undefined
  if (verifying && (!values.verify || [...input, 'entitlements', 'claude-accounts-engine', 'unsigned'].some(name => name in values))) {
    throw new Error('--verify requires a .app path and accepts only --app-name, --bundle-id and --require-release-signature')
  }
  if (!verifying && values['require-release-signature']) {
    throw new Error('--require-release-signature is only available with --verify')
  }
  if (!verifying) {
    for (const flag of input) {
      if (!values[flag]) throw new Error(`Missing required option --${flag}`)
    }
  }

  const config = JSON.parse(await readFile(join(desktopProject, 'package.json'), 'utf8'))
  const workflow = config.build.extraResources.find(resource => resource.to.startsWith('finder-quick-action/'))
  if (!workflow) throw new Error('Missing Finder workflow in the frozen desktop package configuration')
  const bundleId = values['bundle-id'] ?? config.build.appId
  if (!/^[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)+$/.test(bundleId)) throw new Error('Invalid --bundle-id')
  if (values['app-name'] !== undefined) validateAppName(values['app-name'])
  if (!verifying) {
    console.log(JSON.stringify(await assemble(values, config, bundleId, workflow), null, 2))
    return
  }

  const path = resolve(values.verify)
  if (!path.endsWith('.app')) throw new Error('--verify requires a .app directory')
  const { info, runtime, engine, architectures } = await verifyLayout(path, config, bundleId, workflow, values['app-name'])
  const [appSignature, runtimeSignature] = await Promise.all([signatureSummary(path), signatureSummary(runtime)])
  const signatures = { app: appSignature, runtime: runtimeSignature }
  if (engine) signatures.claudeAccounts = await signatureSummary(engine)
  if (values['require-release-signature']) await requireReleaseSignature(path, runtime, engine, signatures, config, bundleId)
  console.log(JSON.stringify({ path, info, architectures, signatures }, null, 2))
}

main().catch(error => {
  console.error(error.message)
  process.exitCode = 1
})
