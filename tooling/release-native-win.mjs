import { copyFile, mkdtemp, realpath, rm, stat, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { basename, extname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { parseArgs } from 'node:util'

import { assembleWindowsDirPackage, planWindowsPackage } from './native-parity/package-native-win.mjs'
import {
  defaultUpdateBaseUrl, desktopConfig, generateChecksums, generateManifest, nativeBinaries,
  repo, requireAbsent, runTool, stageRelease
} from './native-release-helpers.mjs'

const windowsHost = process.platform === 'win32'
const architectures = { x64: 'x86_64', arm64: 'aarch64' }
const usage = `Usage: pnpm package:win [--version X.Y.Z] [--arch x64|arm64] [--out-dir directory]
  [--desktop-bin file] [--runtime-bin file] [--ghostty-dll file] [--accounts-dir directory]
  [--model-router directory] [--update-base-url URL] [--installer file.exe]

Windows builds offline with Cargo, or packages prebuilt Windows binaries on any
supported host. Set CARGO_TARGET_DIR or MATOU_WIN_ARCH to override build defaults.
The output directory defaults to apps/native/release. Supply an existing EXE
installer to target it in stable.yml. The default ZIP is for manual extraction.`

export async function generateWindowsManifest({ version, artifact, releaseDate }) {
  if (typeof artifact !== 'string' || !/\.(exe|zip)$/i.test(artifact) ||
      /[\p{Cc}\\]/u.test(basename(artifact))) {
    throw new Error('Windows update artifact must be an .exe or .zip file')
  }
  const info = await stat(artifact)
  if (!info.isFile() || info.size === 0) throw new Error('Windows update artifact must be a non-empty file')
  return generateManifest(version, [artifact], releaseDate)
}

function windowsReleaseOptions(args, env, config) {
  const options = ['arch', 'version', 'out-dir', 'desktop-bin', 'runtime-bin', 'ghostty-dll', 'accounts-dir',
    'model-router', 'update-base-url', 'installer']
  const { values } = parseArgs({ args, options: {
    ...Object.fromEntries(options.map(name => [name, { type: 'string' }])),
    help: { type: 'boolean', short: 'h' }
  } })
  if (values.help) return { help: true }
  const plan = planWindowsPackage({
    arch: values.arch ?? (env.MATOU_WIN_ARCH || (windowsHost && process.arch === 'arm64' ? 'arm64' : 'x64')),
    version: values.version ?? config.version,
    updateBaseUrl: values['update-base-url'] ?? (env.MATOU_UPDATE_BASE_URL || defaultUpdateBaseUrl)
  })
  const installer = values.installer === undefined ? undefined : resolve(values.installer)
  if (installer && extname(installer).toLowerCase() !== '.exe') throw new Error('--installer must be an .exe file')
  return {
    ...plan, installer,
    outDir: values['out-dir'] ?? join(repo, 'apps/native/release'),
    desktopBin: values['desktop-bin'],
    runtimeBin: values['runtime-bin'],
    ghosttyDll: values['ghostty-dll'] && resolve(values['ghostty-dll']),
    accountsDir: values['accounts-dir'] && resolve(values['accounts-dir']),
    modelRouterDir: values['model-router'] && resolve(values['model-router'])
  }
}

async function releaseWindows(options) {
  const { arch, version, installer, outDir } = options
  if (!windowsHost && (!options.desktopBin || !options.runtimeBin)) {
    throw new Error('Supply --desktop-bin and --runtime-bin for cross-host Windows packaging')
  }
  const zipName = `Matou-${version}-win-${arch}.zip`
  const manifestName = 'stable.yml'
  const checksumName = `SHA256SUMS-${version}-win-${arch}.txt`
  return stageRelease(outDir, 'win', async (staging, output) => {
    const names = [zipName, manifestName, checksumName]
    const installerName = installer && basename(installer)
    if (installerName) names.splice(1, 0, installerName)
    for (const name of names) await requireAbsent(join(output, name))

    const [desktopBin, runtimeBin] = await nativeBinaries({ ...options, target: `${architectures[arch]}-pc-windows-msvc` })
    const unpacked = await mkdtemp(join(await realpath(tmpdir()), 'matou-native-win-release-'))
    const zip = join(staging, zipName)
    try {
      const { path } = await assembleWindowsDirPackage({ ...options, outDir: unpacked, desktopBin, runtimeBin })
      // Archive the contents so the user can extract and launch 码头.exe directly.
      if (windowsHost || process.platform === 'darwin') {
        await runTool(windowsHost ? 'tar.exe' : '/usr/bin/tar', ['-a', '-cf', zip, '-C', path, '.'])
      } else {
        await runTool('zip', ['-q', '-r', zip, '.'], { cwd: path })
      }
    } finally {
      await rm(unpacked, { force: true, recursive: true })
    }

    const manifest = join(staging, manifestName)
    const artifacts = [zip]
    if (installerName) {
      const destination = join(staging, installerName)
      await copyFile(installer, destination)
      artifacts.push(destination)
    } else {
      console.warn('stable.yml targets the ZIP. The Windows updater requires an EXE for installation.')
    }
    await writeFile(manifest, await generateWindowsManifest({ version, artifact: artifacts.at(-1) }))
    await writeFile(join(staging, checksumName), await generateChecksums([...artifacts, manifest]))
    return names
  })
}

const invokedPath = process.argv[1] && await realpath(process.argv[1]).catch(() => null)
if (invokedPath === fileURLToPath(import.meta.url)) {
  try {
    const options = windowsReleaseOptions(process.argv.slice(2), process.env, await desktopConfig())
    if (options.help) console.log(usage)
    else console.log((await releaseWindows(options)).join('\n'))
  } catch (error) {
    console.error(error.message)
    process.exitCode = 1
  }
}
