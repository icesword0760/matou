import { mkdtemp, realpath, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { parseArgs } from 'node:util'

import {
  assembleWindowsDirPackage, planWindowsPackage, windowsArm64UpdateBaseUrl
} from './native-parity/package-native-win.mjs'
import {
  defaultUpdateBaseUrl, desktopConfig, generateChecksums, generateManifest, nativeBinaries,
  repo, requireAbsent, runTool, stageRelease
} from './native-release-helpers.mjs'
import { buildWindowsInstaller, windowsFileVersion } from './windows-installer/build.mjs'

const windowsHost = process.platform === 'win32'
const architectures = { x64: 'x86_64', arm64: 'aarch64' }
const usage = `Usage: pnpm package:win [--version X.Y.Z] [--arch x64|arm64] [--out-dir directory]
  [--desktop-bin file] [--runtime-bin file] [--ghostty-dll file] [--accounts-dir directory]
  --model-router directory [--vc-runtime-dir directory] [--update-base-url URL] [--with-zip]

Run on Windows with Inno Setup 7, the Windows SDK signing tools, and licensed
Microsoft Visual Studio 2022 app-local CRT files. The default build prepares
GPUI and Ghostty, then builds both binaries with Cargo offline. Pass both
prebuilt binaries to skip that build. Paths for ISCC, SignTool, Python and Zig
can be set with MATOU_WIN_ISCC, MATOU_WIN_SIGNTOOL, MATOU_WIN_PYTHON and MATOU_WIN_ZIG.
The signing certificate is selected by MATOU_WIN_SIGN_CERT_SHA1. An optional
MATOU_WIN_TIMESTAMP_URL overrides the default RFC 3161 timestamp server.
Prepare the model router, Python account engine and CRT for the selected architecture.
ARM64 uses MATOU_WIN_ARM64_UPDATE_BASE_URL and defaults to the /stable/win-arm64 feed.
--internal-installer builds an unsigned EXE without a stable update feed.
--portable-only builds a manual ZIP for x64 or arm64, without an update feed.
The output directory defaults to apps/native/release for x64 or its win-arm64 subdirectory.`

export function windowsReleaseOptions(args, env, config) {
  const strings = ['arch', 'version', 'out-dir', 'desktop-bin', 'runtime-bin', 'ghostty-dll', 'accounts-dir',
    'model-router', 'vc-runtime-dir', 'update-base-url', 'installer']
  const booleans = ['with-zip', 'internal-installer', 'portable-only']
  const { values } = parseArgs({ args, options: {
    ...Object.fromEntries(strings.map(name => [name, { type: 'string' }])),
    ...Object.fromEntries(booleans.map(name => [name, { type: 'boolean' }])),
    help: { type: 'boolean', short: 'h' }
  } })
  if (values.help) return { help: true }
  if (values.installer !== undefined) throw new Error('--installer is unsupported. The release builds its installer from the verified payload.')
  if (values['portable-only'] && (values['with-zip'] || values['internal-installer'])) {
    throw new Error('--portable-only cannot be combined with --with-zip or --internal-installer')
  }
  const arch = values.arch ?? (env.MATOU_WIN_ARCH || 'x64')
  const plan = planWindowsPackage({
    arch,
    version: values.version ?? config.version,
    updateBaseUrl: values['update-base-url'] ?? (arch === 'arm64'
      ? env.MATOU_WIN_ARM64_UPDATE_BASE_URL || windowsArm64UpdateBaseUrl
      : env.MATOU_UPDATE_BASE_URL || defaultUpdateBaseUrl)
  })
  const result = {
    ...plan, env,
    portableOnly: values['portable-only'] === true,
    withZip: values['with-zip'] === true,
    internalInstaller: values['internal-installer'] === true,
    outDir: values['out-dir'] ?? join(repo, 'apps/native/release', arch === 'arm64' ? 'win-arm64' : ''),
    desktopBin: values['desktop-bin'],
    runtimeBin: values['runtime-bin'],
    ghosttyDll: values['ghostty-dll'] && resolve(values['ghostty-dll']),
    accountsDir: values['accounts-dir'] && resolve(values['accounts-dir']),
    modelRouterDir: values['model-router'] && resolve(values['model-router']),
    vcRuntimeDir: values['vc-runtime-dir'] && resolve(values['vc-runtime-dir'])
  }
  validateReleaseOptions(result)
  return result
}

async function buildBinaries(options, command) {
  const arch = architectures[options.arch]
  const target = `${arch}-pc-windows-msvc`
  let ghosttyDll = options.ghosttyDll
  if (!options.desktopBin || !options.runtimeBin) {
    const python = options.env.MATOU_WIN_PYTHON || 'python.exe'
    await command(python, ['apps/native/scripts/prepare-gpui.py'])
    await command(python, ['apps/native/terminal/scripts/build-libs.py', '--target', target,
      '--zig', options.env.MATOU_WIN_ZIG || 'zig'])
    ghosttyDll ??= join(repo, `apps/native/terminal/vendor/ghostty-vt/windows-${arch}/bin/ghostty-vt.dll`)
  }
  const [desktopBin, runtimeBin] = await nativeBinaries({ ...options, target })
  return { desktopBin, runtimeBin, ghosttyDll }
}

async function archiveWindowsPayload(zip, payload, command) {
  // Archive the contents so the user can extract and launch 码头.exe directly.
  if (windowsHost || process.platform === 'darwin') {
    const encoding = windowsHost ? ['--options', 'zip:hdrcharset=UTF-8'] : []
    await command(windowsHost ? 'tar.exe' : '/usr/bin/tar', [...encoding, '-a', '-cf', zip, '-C', payload, '.'])
  } else {
    await command('zip', ['-q', '-r', zip, '.'], { cwd: payload })
  }
}

function validateReleaseOptions({ arch, version, portableOnly, internalInstaller, updateBaseUrl }) {
  if (portableOnly) return
  if (!internalInstaller && !/^(0|[1-9]\d*)(\.(0|[1-9]\d*)){2}$/.test(version)) {
    throw new Error('The Windows stable feed requires an X.Y.Z version')
  }
  if (!internalInstaller) {
    const feed = new URL(updateBaseUrl)
    const configuredBase = feed.origin + feed.pathname.replace(/\/+$/, '')
    if ((arch === 'arm64' && configuredBase === defaultUpdateBaseUrl) ||
        (arch === 'x64' && configuredBase === windowsArm64UpdateBaseUrl)) {
      throw new Error('The Windows release requires a stable feed for the selected architecture')
    }
  }
  windowsFileVersion(version)
}

export async function releaseWindows(options, command = runTool) {
  const { arch, version, outDir, portableOnly, internalInstaller, withZip } = options
  validateReleaseOptions(options)
  const stem = `Matou-${version}-win-${arch}${internalInstaller ? '-internal' : ''}`
  const artifactNames = [
    ...(!portableOnly ? [`${stem}.exe`] : []),
    ...(portableOnly || withZip ? [`${stem}.zip`] : [])
  ]
  const manifestName = !portableOnly && !internalInstaller ? 'stable.yml' : null
  const checksumName = `SHA256SUMS-${stem.slice('Matou-'.length)}.txt`
  return stageRelease(outDir, 'win', async (staging, output) => {
    const names = [...artifactNames, checksumName, ...(manifestName ? [manifestName] : [])]
    for (const name of names) await requireAbsent(join(output, name))

    const bins = await buildBinaries(options, command)
    const unpacked = await mkdtemp(join(await realpath(tmpdir()), 'matou-native-win-release-'))
    try {
      const { path: payload } = await assembleWindowsDirPackage({
        ...options, ...bins, outDir: unpacked, requireReleaseComponents: !portableOnly
      })
      if (!portableOnly) {
        await buildWindowsInstaller({ ...options, payload, destination: join(staging, `${stem}.exe`), workDir: unpacked }, command)
      }
      if (portableOnly || withZip) await archiveWindowsPayload(join(staging, `${stem}.zip`), payload, command)
    } finally {
      await rm(unpacked, { force: true, recursive: true })
    }

    const artifacts = artifactNames.map(name => join(staging, name))
    if (manifestName) {
      const manifest = join(staging, manifestName)
      await writeFile(manifest, await generateManifest(version, [artifacts[0]], options.releaseDate))
      artifacts.push(manifest)
    }
    await writeFile(join(staging, checksumName), await generateChecksums(artifacts))
    return names
  })
}

const invokedPath = process.argv[1] && await realpath(process.argv[1]).catch(() => null)
if (invokedPath === fileURLToPath(import.meta.url)) {
  try {
    const options = windowsReleaseOptions(process.argv.slice(2), process.env, await desktopConfig())
    if (options.help) console.log(usage)
    else {
      validateReleaseOptions(options)
      if (!windowsHost && !options.portableOnly) throw new Error('Build the Windows installer on Windows with Inno Setup 7')
      if (!windowsHost && (!options.desktopBin || !options.runtimeBin)) {
        throw new Error('Supply --desktop-bin and --runtime-bin for cross-host Windows packaging')
      }
      console.log((await releaseWindows(options)).join('\n'))
    }
  } catch (error) {
    console.error(error.message)
    process.exitCode = 1
  }
}
