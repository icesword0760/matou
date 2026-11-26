import { access, lstat, mkdir, readFile, realpath, symlink, writeFile } from 'node:fs/promises'
import { createRequire } from 'node:module'
import { basename, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { parseArgs } from 'node:util'
import { macAccountEngineCode, macAccountEngineDirectory, refreshAccountEngineManifestAfterSigning, verifyAccountEngineManifest } from './claude-accounts/artifact.mjs'

import {
  defaultUpdateBaseUrl, desktopConfig, fileDigests, formatUpdateConfig, generateChecksums, generateManifest,
  nativeBinaries, repo, requireAbsent, runTool, stageRelease
} from './native-release-helpers.mjs'

const packager = join(repo, 'tooling/native-parity/package-native-mac.mjs')
const desktopProject = join(repo, 'apps/desktop')
const resourcesSource = join(desktopProject, 'package-resources')
const accountEngine = 'matou-claude-accounts'
const codesign = '/usr/bin/codesign'
const architectures = { arm64: 'aarch64', x64: 'x86_64' }

const usage = `Usage: pnpm package:mac [--version X.Y.Z] [--arch arm64|x64] [--out-dir directory]
  [--desktop-bin file] [--runtime-bin file] [--claude-accounts-engine directory] [--model-router directory]
  [--update-base-url URL] [--update-channel channel] [--identity Developer-ID] [--no-timestamp]

The default inputs are under apps/desktop/package-resources. Override them with
the flags above or MATOU_CLAUDE_ACCOUNTS_ENGINE and MATOU_MODEL_ROUTER_DIR.
Build the complete account engine with pnpm accounts:build before packaging.
MATOU_MAC_ARCH, MATOU_UPDATE_BASE_URL, MATOU_UPDATE_CHANNEL and CARGO_TARGET_DIR
set the respective defaults. The output directory defaults to apps/native/release.
Set MATOU_APP_BUILDER to an existing app-builder binary to generate blockmaps.`

export function macReleaseOptions(args, env, config) {
  const options = ['version', 'arch', 'out-dir', 'desktop-bin', 'runtime-bin', 'claude-accounts-engine',
    'model-router', 'update-base-url', 'update-channel', 'identity']
  const { values } = parseArgs({ args, options: {
    ...Object.fromEntries(options.map(name => [name, { type: 'string' }])),
    'no-timestamp': { type: 'boolean' },
    help: { type: 'boolean', short: 'h' }
  } })
  if (values.help) return { help: true }
  const arch = values.arch ?? (env.MATOU_MAC_ARCH || 'arm64')
  if (!Object.hasOwn(architectures, arch)) throw new Error('--arch must be arm64 or x64')
  const version = values.version ?? config.version
  if (!/^\d+\.\d+\.\d+$/.test(version)) throw new Error('--version must be X.Y.Z')
  const updateBaseUrl = values['update-base-url'] ?? (env.MATOU_UPDATE_BASE_URL || defaultUpdateBaseUrl)
  const channel = values['update-channel'] ?? (env.MATOU_UPDATE_CHANNEL || 'stable')
  formatUpdateConfig(updateBaseUrl, channel)
  const accountsDir = values['claude-accounts-engine'] ?? env.MATOU_CLAUDE_ACCOUNTS_ENGINE
  return {
    version, arch, updateBaseUrl, channel,
    outDir: values['out-dir'] ?? join(repo, 'apps/native/release'),
    desktopBin: values['desktop-bin'],
    runtimeBin: values['runtime-bin'],
    accountsDirs: accountsDir ? [resolve(accountsDir)] : [
      join(resourcesSource, 'claude-accounts', accountEngine),
      join(repo, 'tooling/claude-accounts/dist', accountEngine)
    ],
    modelRouterDir: resolve(values['model-router'] ?? env.MATOU_MODEL_ROUTER_DIR ?? join(resourcesSource, 'model-router')),
    identity: values.identity ?? config.build.mac.identity,
    timestamp: !values['no-timestamp']
  }
}

export function generateMacManifest({ version, zip, dmg, releaseDate }) {
  if (!zip?.endsWith('.zip') || !dmg?.endsWith('.dmg')) throw new Error('macOS release requires a ZIP and DMG')
  return generateManifest(version, [zip, dmg], releaseDate)
}

async function resolveAccounts(paths) {
  for (const directory of paths.flatMap(path => [path, join(path, accountEngine)])) {
    const binary = await lstat(join(directory, accountEngine)).catch(error => {
      if (error.code === 'ENOENT' || error.code === 'ENOTDIR') return null
      throw error
    })
    if (binary?.isFile()) return directory
  }
  throw new Error(`Missing Claude accounts engine under ${paths.join(' or ')}. Pass --claude-accounts-engine or MATOU_CLAUDE_ACCOUNTS_ENGINE.`)
}

async function signApp(app, options, config, accountCode) {
  const contents = join(app, 'Contents')
  const resources = join(contents, 'Resources')
  const entitlements = join(desktopProject, config.build.mac.entitlements)
  const flags = ['--force', '--sign', options.identity, '--options', 'runtime',
    options.timestamp ? '--timestamp' : '--timestamp=none']
  const sign = (path, args = []) => runTool(codesign, [...flags, ...args, path])
  const inherited = ['--entitlements', join(desktopProject, config.build.mac.entitlementsInherit)]

  const engine = accountCode.get(accountEngine)
  for (const binary of accountCode.values()) {
    if (binary !== engine) await sign(binary)
  }
  await sign(engine, inherited)
  await refreshAccountEngineManifestAfterSigning(macAccountEngineDirectory(contents), accountCode)

  const router = join(contents, 'MacOS', 'model-router')
  await sign(router, ['--identifier', `${config.build.appId}.model-router`])
  // Signing changes the executable bytes. Update the existing resource seal
  // before signing the bundle that contains both files.
  const manifestPath = join(resources, 'model-router', 'source.json')
  const source = JSON.parse(await readFile(manifestPath, 'utf8'))
  source.binarySha256 = (await fileDigests(router)).sha256
  await writeFile(manifestPath, JSON.stringify(source, null, 2) + '\n')

  await sign(join(app, 'Contents/MacOS/matou-native-runtime'), inherited)
  await sign(app, ['--entitlements', entitlements])
}

async function verifyApp(app, timestamp) {
  await runTool(codesign, ['--verify', '--deep', '--strict', app])
  const { stdout } = await runTool(process.execPath, [packager, '--verify', app, '--require-release-signature'])
  const { stderr } = await runTool(codesign, ['-dvvv', app])
  const secureTimestamp = stderr.match(/^Timestamp=(.+)$/m)?.[1].trim()
  if (timestamp && (!secureTimestamp || secureTimestamp.toLowerCase() === 'none')) {
    throw new Error(`Expected an Apple secure timestamp for ${app}\n${stderr}`)
  }
  const { signatures } = JSON.parse(stdout)
  console.log(`Verified ${app}\nTeamIdentifier=${signatures.app.teamIdentifier}\n` +
    `designated => ${signatures.app.designatedRequirement}\nTimestamp=${secureTimestamp ?? 'none'}`)
}

function installedAppBuilder() {
  const require = createRequire(import.meta.url)
  try {
    return require('app-builder-bin').appBuilderPath
  } catch (error) {
    if (error.code !== 'MODULE_NOT_FOUND') throw error
  }
  try {
    const builderRequire = createRequire(require.resolve('electron-builder'))
    const libRequire = createRequire(builderRequire.resolve('app-builder-lib'))
    return libRequire('app-builder-bin').appBuilderPath
  } catch (error) {
    if (error.code !== 'MODULE_NOT_FOUND') throw error
  }
}

async function blockmaps(artifacts) {
  try {
    const command = process.env.MATOU_APP_BUILDER || installedAppBuilder() || 'app-builder'
    await runTool(command, ['--version'])
    const maps = []
    for (const artifact of artifacts) {
      const path = artifact + '.blockmap'
      await runTool(command, ['blockmap', '--input', artifact, '--output', path, '--compression', 'gzip'])
      maps.push(path)
    }
    return maps
  } catch (error) {
    console.warn(`Skipping blockmaps: app-builder unavailable or unsuccessful.\n${error.message}`)
    return []
  }
}

async function releaseMac(options, config) {
  const { version, arch, outDir, modelRouterDir, updateBaseUrl, channel, timestamp } = options
  const accounts = await resolveAccounts(options.accountsDirs)
  await verifyAccountEngineManifest(accounts)
  await access(join(modelRouterDir, 'model-router'))
  const stem = `Matou-${version}-mac-${arch}`
  const appName = `${config.productName}.app`
  const manifestName = `${channel}-mac.yml`
  const checksumName = `SHA256SUMS-${version}-mac-${arch}.txt`
  return stageRelease(outDir, 'mac', async (staging, output) => {
    for (const name of [appName, stem + '.zip', stem + '.dmg', stem + '.zip.blockmap',
      stem + '.dmg.blockmap', manifestName, checksumName]) await requireAbsent(join(output, name))
    const targetArch = architectures[arch]
    const [desktopBin, runtimeBin] = await nativeBinaries({ ...options, target: `${targetArch}-apple-darwin` })
    const { stdout } = await runTool(process.execPath, [packager, '--desktop-bin', desktopBin, '--runtime-bin',
      runtimeBin, '--out-dir', staging, '--version', version, '--claude-accounts-engine', accounts,
      '--model-router', modelRouterDir, '--update-base-url', updateBaseUrl, '--update-channel', channel])
    const { path: app, architectures: bundledArch } = JSON.parse(stdout)
    const machoArch = arch === 'x64' ? 'x86_64' : 'arm64'
    for (const [label, slices] of Object.entries(bundledArch)) {
      if (!slices.includes(machoArch)) throw new Error(`${label} does not contain a ${machoArch} slice`)
    }
    const accountCode = await macAccountEngineCode(join(app, 'Contents'))
    for (const file of accountCode.values()) {
      await runTool('/usr/bin/lipo', [file, '-verify_arch', machoArch])
    }
    await signApp(app, options, config, accountCode)
    await verifyApp(app, timestamp)

    const zip = join(staging, stem + '.zip')
    await runTool('/usr/bin/ditto', ['-c', '-k', '--sequesterRsrc', '--keepParent', app, zip])
    const extracted = join(staging, 'zip-verification')
    await runTool('/usr/bin/ditto', ['-x', '-k', zip, extracted])
    await verifyApp(join(extracted, appName), timestamp)

    const dmg = join(staging, stem + '.dmg')
    const volume = join(staging, 'dmg-volume')
    await mkdir(volume)
    await runTool('/usr/bin/ditto', [app, join(volume, appName)])
    await symlink('/Applications', join(volume, 'Applications'))
    await runTool('/usr/bin/hdiutil', ['create', '-volname', config.productName, '-format', 'UDZO', '-srcfolder', volume, dmg])
    await runTool('/usr/bin/hdiutil', ['verify', dmg])
    const mounted = join(staging, 'dmg-verification')
    await mkdir(mounted)
    await runTool('/usr/bin/hdiutil', ['attach', '-readonly', '-nobrowse', '-noautoopen', '-mountpoint', mounted, dmg])
    try {
      await verifyApp(join(mounted, appName), timestamp)
    } finally {
      await runTool('/usr/bin/hdiutil', ['detach', mounted])
    }

    const artifacts = [zip, dmg, ...await blockmaps([zip, dmg])]
    const manifest = join(staging, manifestName)
    await writeFile(manifest, await generateMacManifest({ version, zip, dmg }))
    artifacts.push(manifest)
    await writeFile(join(staging, checksumName), await generateChecksums(artifacts))
    return [appName, ...artifacts.map(path => basename(path)), checksumName]
  })
}

const invokedPath = process.argv[1] && await realpath(process.argv[1]).catch(() => null)
if (invokedPath === fileURLToPath(import.meta.url)) {
  try {
    const config = await desktopConfig()
    const options = macReleaseOptions(process.argv.slice(2), process.env, config)
    if (options.help) console.log(usage)
    else {
      if (process.platform !== 'darwin') throw new Error('macOS release packaging requires macOS')
      console.log((await releaseMac(options, config)).join('\n'))
    }
  } catch (error) {
    console.error(error.message)
    process.exitCode = 1
  }
}
