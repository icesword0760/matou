import { createHash } from 'node:crypto'
import { spawnSync } from 'node:child_process'
import { createReadStream, createWriteStream } from 'node:fs'
import { access, chmod, cp, mkdir, readFile, rm, writeFile } from 'node:fs/promises'
import { delimiter, dirname, join, resolve } from 'node:path'
import { Readable } from 'node:stream'
import { pipeline } from 'node:stream/promises'
import { fileURLToPath } from 'node:url'

import { checkPe } from '../native-parity/package-native-win.mjs'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..')
const pinned = JSON.parse(await readFile(new URL('./version.json', import.meta.url), 'utf8'))
const cacheRoot = join(root, '.cache/model-router')

async function digest(file) {
  const hash = createHash('sha256')
  for await (const chunk of createReadStream(file)) hash.update(chunk)
  return hash.digest('hex')
}
async function exists(path) { try { await access(path); return true } catch { return false } }
async function download(url, file, expected) {
  if (await exists(file)) {
    if (!expected || await digest(file) === expected) return
    await rm(file, { force: true })
  }
  await mkdir(dirname(file), { recursive: true })
  const temporary = `${file}.${process.pid}.tmp`
  try {
    const response = await fetch(url, { signal: AbortSignal.timeout(300_000) })
    if (!response.ok || !response.body) throw new Error(`Download failed: HTTP ${response.status} ${url}`)
    await pipeline(Readable.fromWeb(response.body), createWriteStream(temporary, { flags: 'wx', mode: 0o600 }))
    if (expected && await digest(temporary) !== expected) throw new Error(`Checksum mismatch: ${url}`)
    await cp(temporary, file)
  } finally { await rm(temporary, { force: true }) }
}

async function goArchiveChecksum(filename) {
  try {
    const response = await fetch('https://go.dev/dl/?mode=json&include=all', { signal: AbortSignal.timeout(20_000) })
    if (!response.ok) return undefined
    const releases = await response.json()
    for (const release of releases) for (const file of release.files ?? []) {
      if (file.filename === filename) return file.sha256
    }
  } catch { /* Offline: an existing verified archive is reused. */ }
  return undefined
}

function goArch(arch) {
  return arch === 'x64' ? 'amd64' : arch
}

async function ensureGo(version) {
  const forced = process.env.MATOU_GO_BIN
  if (forced) return resolve(forced)
  const { arch, platform } = process
  const windows = platform === 'win32'
  const system = windows ? 'windows' : platform
  const toolchainRoot = join(cacheRoot, 'go', `${version}-${platform}-${arch}`)
  const binary = join(toolchainRoot, 'bin', windows ? 'go.exe' : 'go')
  if (await exists(binary)) return binary
  const filename = `go${version}.${system}-${goArch(arch)}.${windows ? 'zip' : 'tar.gz'}`
  const archive = join(cacheRoot, 'go', filename)
  const checksum = await goArchiveChecksum(filename)
  if (!checksum) throw new Error(`Cannot verify the pinned Go toolchain checksum: ${filename}`)
  await download(`https://go.dev/dl/${filename}`, archive, checksum)
  const temporary = `${toolchainRoot}.${process.pid}.tmp`
  await rm(temporary, { recursive: true, force: true })
  await mkdir(temporary, { recursive: true })
  try {
    const tar = windows ? 'tar.exe' : 'tar'
    const listing = spawnSync(tar, ['-tf', archive], { encoding: 'utf8' })
    if (listing.status !== 0 || listing.stdout.split(/\r?\n/).filter(Boolean).some(path =>
      /^[/\\]|^[a-z]:/i.test(path) || path.split(/[/\\]/).includes('..'))) throw new Error('Unsafe Go archive')
    if (spawnSync(tar, ['-xf', archive, '-C', temporary], { encoding: 'utf8' }).status !== 0) throw new Error('Cannot extract Go toolchain')
    await rm(toolchainRoot, { recursive: true, force: true })
    await cp(join(temporary, 'go'), toolchainRoot, { recursive: true })
  } finally { await rm(temporary, { recursive: true, force: true }) }
  await chmod(binary, 0o755)
  return binary
}

async function ensureSource() {
  const source = join(cacheRoot, 'source', pinned.commit)
  if (await exists(join(source, 'go.mod'))) return source
  await rm(source, { recursive: true, force: true })
  await mkdir(dirname(source), { recursive: true })
  const clone = spawnSync('git', ['clone', '--depth', '1', '--branch', `v${pinned.version}`, `https://github.com/${pinned.repository}.git`, source], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] })
  if (clone.status !== 0) throw new Error(`Cannot clone pinned router source: ${clone.stderr.trim()}`)
  const head = spawnSync('git', ['-C', source, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).stdout.trim()
  if (head !== pinned.commit) throw new Error(`Pinned router source moved: ${head}`)
  return source
}

function git(args, cwd, label) {
  const result = spawnSync('git', args, { cwd, encoding: 'utf8' })
  if (result.status !== 0) throw new Error(`${label}: ${result.error?.message || result.stderr?.trim() || result.stdout?.trim()}`)
}

export async function buildPatchedRouter({ platform = 'darwin',
  arch = platform === 'win32' ? 'x64' : process.env.MATOU_MAC_ARCH || process.arch, packageResources = false } = {}) {
  if (!pinned.patch) throw new Error('No router patch is pinned')
  const windows = platform === 'win32'
  if (!(windows || platform === 'darwin') || !['arm64', 'x64'].includes(arch)) {
    throw new Error(`Patched router is available for macOS and Windows arm64/x64: ${platform}-${arch}`)
  }
  if (windows && packageResources) throw new Error('Pass the Windows output directory to package:win --model-router')
  const patchFile = join(root, 'tooling/model-router', pinned.patch.file)
  if (await digest(patchFile) !== pinned.patch.sha256) throw new Error('Pinned router patch checksum mismatch')
  const go = await ensureGo(pinned.patch.go)
  const goVersion = spawnSync(go, ['version'], { encoding: 'utf8' })
  if (goVersion.status !== 0 || !goVersion.stdout.startsWith(`go version go${pinned.patch.go} `)) {
    throw new Error(`Expected Go toolchain ${pinned.patch.go}`)
  }
  const source = await ensureSource()
  const target = `${windows ? 'windows' : 'darwin'}-${arch}`
  const buildRoot = join(cacheRoot, 'build', `${pinned.commit.slice(0, 12)}-${pinned.patch.sha256.slice(0, 12)}-${target}`)
  await rm(buildRoot, { recursive: true, force: true })
  git(['clone', '--quiet', '--no-checkout', source, buildRoot], root, 'Cannot stage pinned router source')
  git(['-c', 'core.autocrlf=false', 'checkout', '--quiet', '--detach', pinned.commit], buildRoot, 'Cannot check out pinned router source')
  git(['apply', '--unidiff-zero', '--whitespace=nowarn', patchFile], buildRoot, 'Cannot apply pinned router patch')

  const cache = join(cacheRoot, pinned.version, target)
  await mkdir(cache, { recursive: true })
  const filename = windows ? 'model-router.exe' : 'model-router'
  const output = join(buildRoot, filename)
  const buildDate = process.env.SOURCE_DATE_EPOCH
    ? new Date(Number(process.env.SOURCE_DATE_EPOCH) * 1000).toISOString()
    : new Date().toISOString()
  const ldflags = `-s -w -X main.Version=${pinned.version} -X main.Commit=${pinned.commit} -X main.BuildDate=${buildDate}`
  const build = spawnSync(go, ['build', '-trimpath', '-buildvcs=false', '-ldflags', ldflags, '-o', output, './cmd/server/'], {
    cwd: buildRoot,
    encoding: 'utf8',
    stdio: 'inherit',
    env: {
      ...process.env,
      CGO_ENABLED: windows ? '0' : '1',
      GOOS: windows ? 'windows' : 'darwin',
      GOARCH: goArch(arch),
      GOTOOLCHAIN: 'local',
      GOCACHE: join(cacheRoot, 'gocache'),
      GOMODCACHE: join(cacheRoot, 'gomod'),
      GOFLAGS: '-mod=mod',
      PATH: `${dirname(go)}${delimiter}${process.env.PATH ?? ''}`
    }
  })
  if (build.status !== 0) throw new Error('Patched router build failed')
  if (windows) await checkPe(output, 'patched model-router.exe', arch)
  if (process.platform === platform && process.arch === arch) {
    const version = spawnSync(output, ['-version'], { encoding: 'utf8' })
    if (!`${version.stdout}${version.stderr}`.includes(`Version: ${pinned.version}`)) {
      throw new Error('Patched router does not report the pinned version')
    }
  }

  await rm(join(cache, filename), { force: true })
  await cp(output, join(cache, filename))
  if (!windows) await chmod(join(cache, filename), 0o755)
  await cp(join(buildRoot, 'LICENSE'), join(cache, 'LICENSE'))
  const metadata = {
    version: pinned.version,
    commit: pinned.commit,
    repository: pinned.repository,
    arch,
    patched: true,
    patch: pinned.patch.name,
    patchSha256: pinned.patch.sha256,
    goVersion: pinned.patch.go,
    buildDate,
    ...(windows && { platform: 'windows' }),
    binarySha256: await digest(join(cache, filename))
  }
  await writeFile(join(cache, 'source.json'), JSON.stringify(metadata, null, 2) + '\n')
  if (packageResources) {
    const destination = join(root, 'apps/desktop/package-resources/model-router')
    await mkdir(destination, { recursive: true })
    for (const file of ['model-router', 'LICENSE', 'source.json']) await cp(join(cache, file), join(destination, file))
    await chmod(join(destination, 'model-router'), 0o755)
  }
  return cache
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const arch = process.argv.find(arg => arg.startsWith('--arch='))?.slice(7)
  const platform = process.argv.find(arg => arg.startsWith('--platform='))?.slice(11)
  console.log(await buildPatchedRouter({ arch, platform, packageResources: process.argv.includes('--package') }))
}
