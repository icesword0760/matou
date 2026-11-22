import { createHash } from 'node:crypto'
import { createReadStream } from 'node:fs'
import { copyFile, cp, lstat, mkdir, mkdtemp, open, readFile, readdir, realpath, rename, rm, writeFile } from 'node:fs/promises'
import { homedir, tmpdir } from 'node:os'
import { basename, dirname, isAbsolute, join, parse, relative, resolve, sep, win32 } from 'node:path'
import { fileURLToPath } from 'node:url'
import { isDeepStrictEqual, parseArgs } from 'node:util'

const appId = 'com.matou.desktop'
const productName = '码头'
const runtimeExecutable = 'matou-native-runtime.exe'
const ghosttyFilename = 'ghostty-vt.dll'
const ghosttyVendor = fileURLToPath(new URL('../../apps/native/terminal/vendor/ghostty-vt/', import.meta.url))
const applicationExecutable = productName + '.exe'
const frozenVersion = '0.2.24'
const artifactNamePattern = 'Matou-${version}-${os}-${arch}.${ext}'
const defaultUpdateBaseUrl = 'https://updates.baize-ailabs.com/stable'
const stableChannel = 'stable'
const updateConfigFilename = 'app-update.yml'
const resourcesDirectory = 'resources'
const manifestFilename = 'manifest.json'
const versionFilename = 'version.json'
const claudePluginSource = fileURLToPath(new URL('../../apps/runtime/control-assets/providers/claude-plugin/', import.meta.url))
const accountsResource = 'claude-accounts'
const accountsDirectory = 'matou-claude-accounts'
const accountsExecutable = accountsDirectory + '.exe'
const defaultAccountsDir = fileURLToPath(new URL('../claude-accounts/dist/matou-claude-accounts/', import.meta.url))
const modelRouterResource = 'model-router'
const modelRouterExecutable = 'model-router.exe'
const modelRouterFiles = [modelRouterExecutable, 'LICENSE', 'source.json']
const machines = { x64: 0x8664, arm64: 0xaa64 }
const versionPattern = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/

function checkArch(arch) {
  if (!Object.hasOwn(machines, arch)) throw new Error('Arch must be x64 or arm64')
}

function checkVersion(version) {
  if (typeof version !== 'string' || !versionPattern.test(version)) {
    throw new Error('Version must be X.Y.Z with optional prerelease and build identifiers')
  }
}

function unpackedName(arch) {
  return arch === 'x64' ? 'win-unpacked' : 'win-arm64-unpacked'
}

function dirProjection(arch, version) {
  return {
    appId, version, target: 'dir', artifactTargets: [],
    unpackedDirectory: 'release/' + unpackedName(arch),
    applicationExecutable, nsis: null, windowsPackageScript: null, artifactNamePattern,
    installerFilename: null, appUpdateYml: null, releaseChannelManifest: null
  }
}

// The URL is offline metadata. The reference dir target skips both updater files.
export function planWindowsPackage({ target = 'dir', arch, version, updateBaseUrl = defaultUpdateBaseUrl }) {
  if (target !== 'dir') throw new Error('Target must be dir')
  checkArch(arch)
  checkVersion(version)
  const url = typeof updateBaseUrl === 'string' ? updateBaseUrl.trim() : ''
  if (!/^https?:\/\//i.test(url) || !URL.canParse(url) || /[\r\n\t]/.test(url)) {
    throw new Error('Update base URL must be an absolute HTTP(S) URL')
  }
  return {
    ...dirProjection(arch, version),
    arch, productName, runtimeExecutable, updateBaseUrl: url, unpackedDirectoryName: unpackedName(arch),
    resourcesDirectory, manifestFilename, versionFilename
  }
}

function noInstallerProjection() {
  const fields = [
    'perMachine', 'oneClick', 'allowToChangeInstallationDirectory', 'installDirectory',
    'requiresAdministrator', 'desktopShortcut', 'startMenuShortcut', 'uninstallEntry',
    'deleteAppDataOnUninstall', 'backupOnUninstall', 'restoreOnReinstall'
  ]
  return Object.fromEntries(fields.map(field => [field, null]))
}

function projectionError(code, message) {
  return { error: { ...(code && { code }), message } }
}

const invalidBuildConfiguration = 'ERR_ELECTRON_BUILDER_INVALID_CONFIGURATION'

function publishProjection({ version, buildBase, omitUrl }) {
  checkVersion(version)
  if (omitUrl) {
    return projectionError(invalidBuildConfiguration, 'Please specify "url" for "generic" update server')
  }
  if (buildBase === undefined) {
    return projectionError('ERR_ELECTRON_BUILDER_ENV_NOT_DEFINED',
      'cannot expand pattern "${env.MATOU_UPDATE_BASE_URL}": env MATOU_UPDATE_BASE_URL is not defined')
  }
  const prerelease = version.split('+')[0].match(/^\d+\.\d+\.\d+-([^.]+)/)?.[1]
  return {
    versionDerivedChannel: prerelease && !/^\d+$/.test(prerelease) ? prerelease : null,
    publish: [{ provider: 'generic', url: buildBase, channel: stableChannel }]
  }
}

// Read the plain scalars and files sequence used by the frozen release samples.
function releaseInfo(content) {
  const info = {}
  let entry
  for (const line of (content ?? '').split(/\r?\n/)) {
    if (/^files:\s*$/.test(line)) {
      info.files = []
      continue
    }
    const field = line.match(/^(\s*)(-\s+)?(version|path|url|sha256|sha512):[ \t]*(.*?)\s*$/)
    if (!field) continue
    const [, indent, startsEntry, key, value] = field
    if (!indent && !startsEntry) {
      info[key] = value
      entry = undefined
      continue
    }
    if (startsEntry && info.files) {
      entry = {}
      info.files.push(entry)
    }
    if (entry) entry[key] = value
  }
  if (!info.version) throw new Error('Fixture release data must include a version')
  return info
}

function providerUrl(base, path) {
  const url = new URL(path, base)
  if (base.search) url.search = base.search
  return url
}

function pretty(value) {
  return JSON.stringify(value, null, 2)
}

function installerProjection(info, base) {
  const files = info.files?.length ? info.files : info.path ? [{
    url: info.path, ...(info.sha512 !== undefined && { sha512: info.sha512 })
  }] : []
  if (!files.length) {
    return projectionError('ERR_UPDATER_NO_FILES_PROVIDED', 'No files provided: ' + pretty(info))
  }
  const missingChecksum = files.find(file => !file.sha256 && !file.sha512)
  if (missingChecksum) {
    return projectionError('ERR_UPDATER_NO_CHECKSUM',
      "Update info doesn't contain nor sha256 neither sha512 checksum: " + pretty(missingChecksum))
  }
  const urls = files.map(file => providerUrl(base, file.url))
  const installer = urls.find(url => url.pathname.toLowerCase().endsWith('.exe')) ?? urls[0]
  return {
    updateInfo: info,
    selectedInstallerUrl: installer.href,
    selectedInstallerFilename: decodeURIComponent(win32.basename(installer.pathname))
  }
}

function feedProjection({ env = {}, packagedConfig = '', release, httpStatus, resolveInstaller }) {
  const overrideUrl = env.MATOU_UPDATE_BASE_URL
  const packagedUrl = /^provider:\s*generic\s*$/m.test(packagedConfig)
    ? packagedConfig.match(/^url:\s*(.+?)\s*$/m)?.[1]?.trim()
    : undefined
  const url = overrideUrl?.trim() || packagedUrl || defaultUpdateBaseUrl
  const channel = env.MATOU_UPDATE_CHANNEL ?? stableChannel
  const result = {
    ...(overrideUrl && { overrideUrl }),
    packagedConfigPath: win32.join('%TEST_RESOURCES%', updateConfigFilename),
    channel, feed: { provider: 'generic', url, channel }, allowDowngrade: false
  }
  const base = URL.parse(url)
  if (!base) {
    return { ...result, requests: [], ...projectionError('ERR_INVALID_URL', 'Invalid URL') }
  }
  if (!base.pathname.endsWith('/')) base.pathname += '/'
  const channelFilename = channel + '.yml'
  const requested = { ...result, returnedUrl: url, requests: [providerUrl(base, channelFilename).href] }
  if (httpStatus === 404) {
    return {
      ...requested,
      ...projectionError('ERR_UPDATER_CHANNEL_FILE_NOT_FOUND',
        `Cannot find channel "${channelFilename}" update info: fixture missing channel`)
    }
  }
  const updateInfo = releaseInfo(release)
  return resolveInstaller
    ? { ...requested, ...installerProjection(updateInfo, base) }
    : { ...requested, updateInfo }
}

function dataProjection({ home, appData, env = {} }) {
  const desktopOverride = env.ELECTRON_USER_DATA_DIR
  const dataRoot = win32.resolve(env.MATOU_DATA_DIR ?? win32.join(home, '.matou'))
  return {
    userData: desktopOverride || win32.join(appData, 'Matou'),
    mkdir: desktopOverride ? [{ path: desktopOverride, options: { recursive: true } }] : [],
    dataRoot, databasePath: win32.join(dataRoot, 'matou.sqlite'),
    backupDirectory: win32.join(dataRoot, 'backups')
  }
}

function backupProjection(input) {
  if (input.action === 'restore-backup' && input.validBackupFound === false) {
    return projectionError(null, `backup ${input.backupId} is missing or invalid`)
  }
  const { databasePath, backupDirectory } = dataProjection(input)
  if (input.action === 'restore-backup') {
    return {
      targetDatabasePath: databasePath,
      replacedDatabasePath: input.targetExists ? `${databasePath}.replaced-${input.restoredAt}` : null,
      validateBackupBeforeReplace: true,
      removeAfterSuccess: [databasePath + '-wal', databasePath + '-shm']
    }
  }
  return {
    directory: backupDirectory, defaultKeepCount: 7,
    files: input.reasons.map(reason => {
      const backup = win32.join(backupDirectory, `matou-${input.createdAt}-${reason}-v${input.schemaVersion}`)
      return { reason, database: backup + '.sqlite', manifest: backup + '.json' }
    })
  }
}

const appFilename = '@matoudesktop'

function nsisProjection(input) {
  if (input.arch !== undefined) {
    checkArch(input.arch)
    checkVersion(input.version)
    return {
      artifactNamePattern, installerFilename: `Matou-${input.version}-win-${input.arch}.exe`,
      releaseManifest: input.manifestChannel + '.yml'
    }
  }

  const { oneClick = true, perMachine = false, allowToChangeInstallationDirectory = false,
    deleteAppDataOnUninstall = false } = input.options
  if (oneClick && allowToChangeInstallationDirectory) {
    return projectionError(invalidBuildConfiguration,
      'allowToChangeInstallationDirectory makes sense only for assisted installer (please set oneClick to false)')
  }
  if (input.args) {
    const shouldDelete = input.args.includes('--delete-app-data')
      || (deleteAppDataOnUninstall && !input.args.includes('--updated'))
    const { userData, dataRoot, backupDirectory } = dataProjection({
      appData: input.appData, home: win32.resolve(input.appData, '..', '..')
    })
    return {
      deleteAppDataOnUninstall,
      rmDirRecursive: shouldDelete
        ? [appFilename, productName, '@matou/desktop'].map(name => win32.join(input.appData, name))
        : [],
      pathsOutsideTemplateCleanup: [userData, dataRoot, backupDirectory],
      backupHook: null, restoreHook: null
    }
  }

  const options = {
    oneClick, perMachine, allowToChangeInstallationDirectory, deleteAppDataOnUninstall,
    desktopShortcutPolicy: 'FRESH_INSTALL', startMenuShortcut: true, shortcutName: productName,
    uninstallDisplayName: productName + ' ' + frozenVersion, appFilename
  }
  if (!input.defaultProgramsRoot) return options
  const installDirectory = input.previousInstallLocation
    || win32.join(input.userProgramFiles || input.defaultProgramsRoot, appFilename)
  const uninstallString = `"${win32.join(installDirectory, 'Uninstall ' + applicationExecutable)}" /currentuser`
  return {
    ...options, installMode: 'CurrentUser', installDirectory, requestExecutionLevel: 'user',
    requiresAdministrator: false, uninstallRegistryHive: 'HKCU',
    uninstallRegistryPath: 'Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\6dea9979-0ce2-53f9-9315-997405533636',
    uninstallString, quietUninstallString: uninstallString + ' /S',
    desktopShortcutPath: win32.join(input.desktop, productName + '.lnk'),
    startMenuShortcutPath: win32.join(input.startMenuPrograms, productName + '.lnk')
  }
}

// Evaluate a frozen fixture input without reading its expected output or touching the projected paths.
export function projectWindowsInstallCase({ id, input }) {
  if (id.startsWith('frozen-dir-') && input.target === 'dir') {
    checkArch(input.arch)
    return dirProjection(input.arch, frozenVersion)
  }
  if (id === 'frozen-no-install-or-uninstall' && input.target === 'dir') return noInstallerProjection()
  if (id.startsWith('feed-') || id.startsWith('manifest-')) return feedProjection(input)
  if (id.startsWith('mode-') && input.platform === 'win32') {
    return {
      installMode: 'automatic',
      enabled: input.isPackaged && input.env?.MATOU_DISABLE_AUTO_UPDATE !== '1',
      signatureInspections: 0
    }
  }
  if (id.startsWith('publish-')) return publishProjection(input)
  if (id.startsWith('data-')) return dataProjection(input)
  if (id.startsWith('runtime-')) return backupProjection(input)
  if (id.startsWith('nsis-') && input.target === 'nsis') return nsisProjection(input)
  throw new Error('Unsupported Windows install reference case: ' + id)
}

export function validateWindowsInstallContract(reference) {
  const scope = {
    platform: 'win32', configuredWindowsTarget: 'dir',
    buildPublish: [{ provider: 'generic', url: '${env.MATOU_UPDATE_BASE_URL}', channel: stableChannel }],
    defaultUpdateBaseUrl
  }
  if (reference.schema !== 'matou.native-parity.windows-install-reference/v1'
      || !isDeepStrictEqual(reference.scope, scope) || !reference.cases?.length) {
    throw new Error('Invalid Windows install reference contract')
  }
  const caseIds = new Set()
  for (const entry of reference.cases) {
    if (caseIds.has(entry.id)) throw new Error('Duplicate Windows install reference case: ' + entry.id)
    const actual = projectWindowsInstallCase(entry)
    if (!isDeepStrictEqual(actual, entry.expected)) {
      throw new Error(`Windows install contract mismatch for ${entry.id}\nExpected ${pretty(entry.expected)}\nReceived ${pretty(actual)}`)
    }
    caseIds.add(entry.id)
  }
  return { caseCount: caseIds.size, caseIds: [...caseIds] }
}

function packageMetadata(plan) {
  return {
    manifest: {
      target: plan.target, arch: plan.arch,
      applicationExecutable: plan.applicationExecutable, runtimeExecutable: plan.runtimeExecutable,
      versionMetadata: plan.versionFilename
    },
    version: {
      appId: plan.appId, productName: plan.productName, version: plan.version,
      arch: plan.arch, artifactNamePattern: plan.artifactNamePattern,
      update: {
        baseUrl: plan.updateBaseUrl, channel: stableChannel,
        appUpdateYml: plan.appUpdateYml, releaseChannelManifest: plan.releaseChannelManifest
      }
    }
  }
}

async function requirePath(path, label, directory = false) {
  const stat = await lstat(path).catch(error => {
    if (['ENOENT', 'ENOTDIR'].includes(error.code)) throw new Error(`Missing ${label}: ${path}`)
    throw error
  })
  if (directory ? !stat.isDirectory() : !stat.isFile() || stat.size === 0) {
    throw new Error(`${label} must be a ${directory ? 'real directory' : 'non-empty regular file'}: ${path}`)
  }
}

async function checkPe(path, label, arch, dll = false) {
  await requirePath(path, label)
  const file = await open(path, 'r')
  const invalid = () => new Error(`Expected a Windows PE ${dll ? 'DLL' : 'executable'} for ${label}: ${path}`)
  try {
    const dos = Buffer.alloc(64)
    if ((await file.read(dos, 0, dos.length, 0)).bytesRead !== dos.length
        || dos.readUInt16LE(0) !== 0x5a4d) throw invalid()
    const offset = dos.readUInt32LE(0x3c)
    const pe = Buffer.alloc(26)
    if (offset < dos.length || (await file.read(pe, 0, pe.length, offset)).bytesRead !== pe.length
        || pe.readUInt32LE(0) !== 0x4550) throw invalid()
    const optionalSize = pe.readUInt16LE(20)
    const flags = pe.readUInt16LE(22)
    if (optionalSize < 2 || offset + 24 + optionalSize > (await file.stat()).size
        || !(flags & 0x0002) || Boolean(flags & 0x2000) !== dll
        || pe.readUInt16LE(24) !== 0x20b) throw invalid()
    const machine = pe.readUInt16LE(4)
    if (machine !== machines[arch]) {
      throw new Error(`${label} architecture mismatch: expected ${arch}, found PE machine 0x${machine.toString(16)} in ${path}`)
    }
  } finally {
    await file.close()
  }
}

async function resolveGhosttyDll(explicit, runtimeBin, arch) {
  if (explicit !== undefined) {
    if (typeof explicit !== 'string' || !explicit.trim()) throw new Error('Missing Ghostty DLL path')
    return explicit
  }
  const sibling = join(dirname(runtimeBin), ghosttyFilename)
  const vendorArch = arch === 'arm64' ? 'aarch64' : 'x86_64'
  return lstat(sibling).then(() => sibling, error => {
    if (error.code !== 'ENOENT') throw error
    return join(ghosttyVendor, 'windows-' + vendorArch, 'bin', ghosttyFilename)
  })
}

async function optionalDirectory(path, label = 'Claude accounts path') {
  const stat = await lstat(path).catch(error => {
    if (error.code !== 'ENOENT') throw error
  })
  if (!stat) return
  if (!stat.isDirectory()) throw new Error(label + ' must be a real directory: ' + path)
  return path
}

async function verifyAccountsDirectory(directory, arch) {
  await requirePath(directory, 'Claude accounts onedir', true)
  await checkPe(join(directory, accountsExecutable), 'Claude accounts executable', arch)
}

async function requireEntries(directory, expected, label) {
  await requirePath(directory, label, true)
  const entries = (await readdir(directory)).sort()
  if (!isDeepStrictEqual(entries, [...expected].sort())) {
    throw new Error(`Invalid ${label} entries at ${directory}: expected ${expected.join(', ')}; found ${entries.join(', ')}`)
  }
}

async function verifyModelRouter(directory, arch) {
  await requireEntries(directory, modelRouterFiles, 'model-router directory')
  const binary = join(directory, modelRouterExecutable)
  await checkPe(binary, 'model-router binary', arch)
  await requirePath(join(directory, 'LICENSE'), 'model-router LICENSE')
  const manifest = join(directory, 'source.json')
  await requirePath(manifest, 'model-router source.json')
  const source = await readFile(manifest, 'utf8').then(JSON.parse).catch(error => {
    throw new Error(`Invalid model-router source.json: ${error.message}`)
  })
  const hash = createHash('sha256')
  for await (const chunk of createReadStream(binary)) hash.update(chunk)
  if (source?.binarySha256 !== hash.digest('hex')) throw new Error('model-router binarySha256 mismatch')
}

async function verifyClaudePlugin(resources) {
  const assets = join(resources, 'control-assets')
  const providers = join(assets, 'providers')
  const plugin = join(providers, 'claude-plugin')
  await requireEntries(assets, ['providers'], 'control assets')
  await requireEntries(providers, ['claude-plugin'], 'control asset providers')
  await requirePath(plugin, 'Claude plugin', true)
  const sourceEntries = await readdir(claudePluginSource, { recursive: true, withFileTypes: true })
  for (const entry of sourceEntries) {
    const source = join(entry.parentPath, entry.name)
    const destination = join(plugin, relative(claudePluginSource, source))
    await requirePath(destination, 'Claude plugin asset', entry.isDirectory())
    if (entry.isFile() && !(await readFile(destination)).equals(await readFile(source))) {
      throw new Error(`Claude plugin asset differs from source: ${destination}`)
    }
  }
  if ((await readdir(plugin, { recursive: true })).length !== sourceEntries.length) {
    throw new Error(`Unexpected Claude plugin assets at ${plugin}`)
  }
}

export async function verifyWindowsDirPackage(path, options) {
  const plan = planWindowsPackage(options)
  if (basename(resolve(path)) !== plan.unpackedDirectoryName) {
    throw new Error(`Expected directory named ${plan.unpackedDirectoryName}: ${path}`)
  }
  const resources = join(path, plan.resourcesDirectory)
  await requireEntries(path, [plan.applicationExecutable, plan.runtimeExecutable, ghosttyFilename, plan.resourcesDirectory], 'unpacked package')
  const expectedResources = [plan.manifestFilename, plan.versionFilename, 'control-assets']
  const accounts = join(resources, accountsResource)
  if (options.accountsDir !== undefined || await optionalDirectory(accounts)) {
    expectedResources.push(accountsResource)
    await requireEntries(accounts, [accountsDirectory], 'Claude accounts resources')
    await verifyAccountsDirectory(join(accounts, accountsDirectory), plan.arch)
  }
  const modelRouter = join(resources, modelRouterResource)
  if (options.modelRouterDir !== undefined || await optionalDirectory(modelRouter, 'model-router path')) {
    expectedResources.push(modelRouterResource)
    await verifyModelRouter(modelRouter, plan.arch)
  }
  await requireEntries(resources, expectedResources, 'resources')
  await verifyClaudePlugin(resources)
  const { manifest, version } = packageMetadata(plan)
  for (const [filename, expected] of [[plan.manifestFilename, manifest], [plan.versionFilename, version]]) {
    const metadata = join(resources, filename)
    await requirePath(metadata, filename)
    if (!isDeepStrictEqual(JSON.parse(await readFile(metadata, 'utf8')), expected)) {
      throw new Error(`Invalid ${filename}: metadata differs from the package plan`)
    }
  }
  await checkPe(join(path, plan.applicationExecutable), 'desktop binary', plan.arch)
  await checkPe(join(path, plan.runtimeExecutable), 'runtime binary', plan.arch)
  await checkPe(join(path, ghosttyFilename), 'Ghostty DLL', plan.arch, true)
  return plan
}

const windowsHost = process.platform === 'win32'
const protectedComponents = /^(?:program files(?: \(x86\)| \(arm\))?|programdata|localappdata|windows|system32|\.matou)$/i
const windowsDevice = /^(?:CON|PRN|AUX|NUL|COM[1-9¹²³]|LPT[1-9¹²³])(?:\.|$)/i

function within(root, path) {
  const suffix = relative(root, path)
  return suffix === '' || (suffix !== '..' && !suffix.startsWith('..' + sep) && !isAbsolute(suffix))
}

function protectedTempRoot(root) {
  if (root === parse(root).root || root.split(sep).some(part => protectedComponents.test(part))) return true
  if (windowsHost) {
    const local = process.env.LOCALAPPDATA
    if (local && root.toLowerCase() === join(local, 'Temp').toLowerCase()) return false
    const protectedRoots = [
      homedir(), local, process.env.APPDATA, process.env.PUBLIC, process.env.ProgramFiles,
      process.env['ProgramFiles(x86)'], process.env.ProgramW6432, process.env.ProgramData, process.env.SystemRoot
    ]
    return protectedRoots.some(path => path && within(resolve(path), root))
      || /^(?:users|documents and settings)(?:\\|$)/i.test(root.slice(parse(root).root.length))
  }
  return within(homedir(), root) || !/^\/(?:private\/)?(?:tmp|var\/(?:tmp|folders))(?:\/|$)/.test(root)
}

async function safeOutput(outDir) {
  const unsafe = () => new Error(`--out-dir must be an isolated temporary subdirectory: ${outDir}`)
  if (typeof outDir !== 'string' || !outDir.trim() || /[\u0000-\u001f]/.test(outDir)
      || /(^|[/\\])\.\.($|[/\\])/.test(outDir)) throw unsafe()
  if (windowsHost && /^[a-z]:(?![/\\])/i.test(outDir)) throw unsafe()
  const output = resolve(outDir)
  if (windowsHost && !/^[a-z]:\\/i.test(output)) throw unsafe()
  const parts = output.slice(parse(output).root.length).split(sep)
  if (parts.some(part => protectedComponents.test(part) || /[<>:"|?*\\/]/.test(part)
      || /[. ]$/.test(part) || windowsDevice.test(part))) throw unsafe()

  const roots = windowsHost ? [tmpdir()] : [tmpdir(), '/tmp']
  for (const path of new Set(roots)) {
    const temp = resolve(path)
    const canonical = await realpath(temp).catch(error => {
      if (error.code === 'ENOENT') return null
      throw error
    })
    if (!canonical || protectedTempRoot(temp) || protectedTempRoot(canonical)) continue
    const root = within(temp, output) ? temp : within(canonical, output) ? canonical : null
    if (!root) continue
    const suffix = relative(root, output)
    if (!suffix) throw unsafe()
    let parent = canonical
    for (const part of suffix.split(sep)) {
      parent = join(parent, part)
      const stat = await lstat(parent).catch(error => {
        if (error.code === 'ENOENT') return null
        throw error
      })
      if (stat && !stat.isDirectory()) throw unsafe()
    }
    return parent
  }
  throw unsafe()
}

async function requireAbsent(path) {
  try {
    await lstat(path)
  } catch (error) {
    if (error.code === 'ENOENT') return
    throw error
  }
  throw new Error('Output package already exists: ' + path)
}

export async function assembleWindowsDirPackage({ desktopBin, runtimeBin, ghosttyDll, outDir, ...options }) {
  const plan = planWindowsPackage(options)
  const output = await safeOutput(outDir)
  const destination = join(output, plan.unpackedDirectoryName)
  await requireAbsent(destination)
  for (const [path, label] of [[desktopBin, 'desktop binary'], [runtimeBin, 'runtime binary']]) {
    if (typeof path !== 'string' || !path.trim()) throw new Error('Missing ' + label)
    await checkPe(path, label, plan.arch)
  }
  const dll = await resolveGhosttyDll(ghosttyDll, runtimeBin, plan.arch)
  await checkPe(dll, 'Ghostty DLL', plan.arch, true)
  if (options.accountsDir !== undefined && (typeof options.accountsDir !== 'string' || !options.accountsDir.trim())) throw new Error('Missing Claude accounts directory')
  const accounts = options.accountsDir ?? await optionalDirectory(defaultAccountsDir)
  if (accounts) await verifyAccountsDirectory(accounts, plan.arch)
  const modelRouter = options.modelRouterDir
  if (modelRouter !== undefined) {
    if (typeof modelRouter !== 'string' || !modelRouter.trim()) throw new Error('Missing model-router directory')
    await requirePath(modelRouter, 'model-router directory', true)
  }

  await mkdir(output, { recursive: true })
  if (await safeOutput(outDir) !== output) throw new Error('Output path changed during assembly')
  const staging = await mkdtemp(join(output, '.matou-native-win-'))
  try {
    const directory = join(staging, plan.unpackedDirectoryName)
    const resources = join(directory, plan.resourcesDirectory)
    await mkdir(resources, { recursive: true })
    await copyFile(desktopBin, join(directory, plan.applicationExecutable))
    await copyFile(runtimeBin, join(directory, plan.runtimeExecutable))
    await copyFile(dll, join(directory, ghosttyFilename))
    await cp(claudePluginSource, join(resources, 'control-assets', 'providers', 'claude-plugin'), { recursive: true })
    if (accounts) {
      await cp(accounts, join(resources, accountsResource, accountsDirectory), { recursive: true })
    }
    if (modelRouter !== undefined) {
      const directory = join(resources, modelRouterResource)
      await mkdir(directory)
      for (const name of modelRouterFiles) {
        const source = join(modelRouter, name)
        await requirePath(source, `model-router ${name}`)
        await copyFile(source, join(directory, name))
      }
    }
    const { manifest, version } = packageMetadata(plan)
    await writeFile(join(resources, plan.manifestFilename), pretty(manifest) + '\n')
    await writeFile(join(resources, plan.versionFilename), pretty(version) + '\n')
    await verifyWindowsDirPackage(directory, options)
    await requireAbsent(destination)
    try {
      await rename(directory, destination)
    } catch (error) {
      await requireAbsent(destination)
      throw error
    }
    return { ...plan, path: destination }
  } finally {
    await rm(staging, { recursive: true, force: true })
  }
}

const requiredOptions = ['arch', 'desktop-bin', 'runtime-bin', 'out-dir', 'version']
const usage = `Usage:
  node tooling/native-parity/package-native-win.mjs --arch <x64|arm64> [--target dir]
    --desktop-bin <file> --runtime-bin <file> --out-dir <temporary-subdirectory>
    --version <X.Y.Z> [--update-base-url <http(s) URL>] [--ghostty-dll <file>] [--accounts-dir <dir>]
    [--model-router <dir>]

Assembles and verifies an offline Windows directory package. The approved output
location is a subdirectory of the host temporary directory (or /tmp on POSIX).`

async function main() {
  const { values } = parseArgs({
    options: {
      ...Object.fromEntries([...requiredOptions, 'target', 'update-base-url', 'ghostty-dll', 'accounts-dir', 'model-router'].map(name => [name, { type: 'string' }])),
      help: { type: 'boolean', short: 'h' }
    }
  })
  if (values.help) return console.log(usage)
  for (const name of requiredOptions) {
    if (!values[name]?.trim()) throw new Error('Missing required option --' + name)
  }
  const result = await assembleWindowsDirPackage({
    arch: values.arch, target: values.target, version: values.version,
    desktopBin: values['desktop-bin'], runtimeBin: values['runtime-bin'],
    ghosttyDll: values['ghostty-dll'], accountsDir: values['accounts-dir'], modelRouterDir: values['model-router'],
    outDir: values['out-dir'], updateBaseUrl: values['update-base-url']
  })
  console.log(pretty(result))
}

const invokedPath = process.argv[1] && await realpath(process.argv[1]).catch(() => null)
if (invokedPath === fileURLToPath(import.meta.url)) {
  try {
    await main()
  } catch (error) {
    console.error(error.message)
    process.exitCode = 1
  }
}
