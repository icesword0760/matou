import { execFile, spawn } from 'node:child_process'
import { createHash } from 'node:crypto'
import { createReadStream } from 'node:fs'
import { lstat, mkdir, readFile, realpath, rename, rm } from 'node:fs/promises'
import { basename, dirname, join, resolve, sep } from 'node:path'
import { fileURLToPath } from 'node:url'
import { promisify } from 'node:util'

export const repo = fileURLToPath(new URL('../', import.meta.url))
export const defaultUpdateBaseUrl = 'https://updates.baize-ailabs.com/stable'
export const desktopConfig = () => readFile(new URL('../apps/desktop/package.json', import.meta.url), 'utf8').then(JSON.parse)

export function formatUpdateConfig(baseUrl, channel) {
  if (!/^https?:\/\//i.test(baseUrl) || !URL.canParse(baseUrl) || /[\s\p{Cc}]/u.test(baseUrl)) {
    throw new Error('Update base URL must be an absolute HTTP(S) URL without whitespace')
  }
  if (!/^[a-z][a-z0-9._-]*$/i.test(channel)) throw new Error('Invalid update channel')
  return `provider: generic\nurl: ${baseUrl}\nchannel: ${channel}\n`
}

export async function fileDigests(path) {
  const sha256 = createHash('sha256')
  const sha512 = createHash('sha512')
  let size = 0
  for await (const chunk of createReadStream(path)) {
    size += chunk.length
    sha256.update(chunk)
    sha512.update(chunk)
  }
  return { size, sha256: sha256.digest('hex'), sha512: sha512.digest('base64') }
}

function yamlFilename(name) {
  return /^[a-z0-9][a-z0-9._+-]*$/i.test(name) ? name : JSON.stringify(name)
}

export async function generateManifest(version, artifacts, releaseDate = new Date()) {
  if (!/^\d+\.\d+\.\d+(?:-[\w.-]+)?(?:\+[\w.-]+)?$/.test(version)) throw new Error('Invalid release version')
  const files = await Promise.all(artifacts.map(async path => {
    // The updater treats these as relative URLs, including when a supplied
    // installer filename contains spaces, #, %, or non-ASCII characters.
    const url = yamlFilename(encodeURIComponent(basename(path)))
    return { url, ...await fileDigests(path) }
  }))
  const first = files[0]
  return [
    `version: ${version}`,
    'files:',
    ...files.flatMap(({ url, sha512, size }) => [
      `  - url: ${url}`, `    sha512: ${sha512}`, `    size: ${size}`
    ]),
    `path: ${first.url}`,
    `sha512: ${first.sha512}`,
    `releaseDate: '${new Date(releaseDate).toISOString()}'`,
    ''
  ].join('\n')
}

export async function generateChecksums(artifacts) {
  const lines = await Promise.all(artifacts.map(async path => {
    const { sha256 } = await fileDigests(path)
    return `${sha256}  ${basename(path)}`
  }))
  return lines.join('\n') + '\n'
}

const exec = promisify(execFile)

export async function runTool(command, args, options) {
  try {
    const { stdout, stderr } = await exec(command, args, {
      cwd: repo, maxBuffer: 16 * 1024 * 1024, ...options
    })
    return { stdout, stderr }
  } catch (error) {
    throw new Error(`${command} ${args.join(' ')}\n${error.stderr || error.message}`, { cause: error })
  }
}

export async function canonicalOutput(path) {
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

export function requireSafeMacOutput(path) {
  const normalized = path.toLowerCase()
  if (['/applications', '/system/applications', '/system/volumes/data/applications'].some(
    directory => normalized === directory || normalized.startsWith(`${directory}${sep}`)
  ) || normalized.split(sep).some(part => part.endsWith('.app'))) {
    throw new Error(`--out-dir cannot write to /Applications or inside an existing .app: ${path}`)
  }
}

export async function requireAbsent(path) {
  try {
    await lstat(path)
  } catch (error) {
    if (error.code === 'ENOENT') return
    throw error
  }
  throw new Error(`Output already exists: ${path}`)
}

export async function stageRelease(outDir, platform, assemble) {
  const output = await canonicalOutput(outDir)
  if (platform === 'mac') {
    requireSafeMacOutput(resolve(outDir))
    requireSafeMacOutput(output)
  }
  await mkdir(output, { recursive: true })
  const staging = join(output, `.matou-native-release-${platform}`)
  await mkdir(staging)
  const published = []
  try {
    const names = await assemble(staging, output)
    for (const name of names) await requireAbsent(join(output, name))
    for (const name of names) {
      const destination = join(output, name)
      await rename(join(staging, name), destination)
      published.push(destination)
    }
    return published
  } catch (error) {
    for (const path of published.reverse()) await rm(path, { force: true, recursive: true })
    throw error
  } finally {
    await rm(staging, { force: true, recursive: true })
  }
}

export async function nativeBinaries({ target, desktopBin, runtimeBin }) {
  const cargoTargetDir = resolve(repo, process.env.CARGO_TARGET_DIR || 'apps/native/target')
  const bins = ['matou-native-desktop', 'matou-native-runtime']
  const supplied = [desktopBin, runtimeBin]
  const missing = bins.filter((_, index) => !supplied[index])
  if (missing.length) {
    const args = ['build', '--manifest-path', 'apps/native/Cargo.toml', '--offline', '--locked',
      '--release', '--target', target, ...missing.flatMap(name => ['--bin', name])]
    await new Promise((accept, reject) => {
      const cargo = spawn('cargo', args, { cwd: repo, stdio: 'inherit' })
      cargo.once('error', reject)
      cargo.once('close', (code, signal) => {
        if (code === 0) accept()
        else reject(new Error(`cargo build failed (${signal ?? code})`))
      })
    })
  }
  const extension = target.endsWith('-windows-msvc') ? '.exe' : ''
  return bins.map((name, index) => supplied[index] ? resolve(supplied[index]) :
    join(cargoTargetDir, target, 'release', name + extension))
}
