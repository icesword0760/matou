import { createHash } from 'node:crypto'
import { spawnSync } from 'node:child_process'
import { createReadStream, createWriteStream } from 'node:fs'
import { access, chmod, cp, mkdir, readFile, rm, writeFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { Readable } from 'node:stream'
import { pipeline } from 'node:stream/promises'
import { fileURLToPath } from 'node:url'

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

async function ensureGo(version, arch) {
  const forced = process.env.MATOU_GO_BIN
  if (forced) return resolve(forced)
  const toolchainRoot = join(cacheRoot, 'go', `${version}-darwin-${arch}`)
  const binary = join(toolchainRoot, 'bin/go')
  if (await exists(binary)) return binary
  const filename = `go${version}.darwin-${arch}.tar.gz`
  const archive = join(cacheRoot, 'go', filename)
  await download(`https://go.dev/dl/${filename}`, archive, await goArchiveChecksum(filename))
  const temporary = `${toolchainRoot}.${process.pid}.tmp`
  await rm(temporary, { recursive: true, force: true })
  await mkdir(temporary, { recursive: true })
  try {
    const listing = spawnSync('tar', ['-tzf', archive], { encoding: 'utf8' })
    if (listing.status !== 0 || listing.stdout.split(/\r?\n/).filter(Boolean).some(path => path.startsWith('/') || path.split('/').includes('..'))) throw new Error('Unsafe Go archive')
    if (spawnSync('tar', ['-xzf', archive, '-C', temporary], { encoding: 'utf8' }).status !== 0) throw new Error('Cannot extract Go toolchain')
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

export async function buildPatchedRouter({ arch = process.env.MATOU_MAC_ARCH || process.arch, packageResources = false } = {}) {
  if (!pinned.patch) throw new Error('No router patch is pinned')
  if (!['arm64', 'x64'].includes(arch)) throw new Error(`Patched router is available only for macOS arm64/x64: ${arch}`)
  const patchFile = join(root, 'tooling/model-router', pinned.patch.file)
  if (await digest(patchFile) !== pinned.patch.sha256) throw new Error('Pinned router patch checksum mismatch')
  const go = await ensureGo(pinned.patch.go, arch)
  const source = await ensureSource()
  const buildRoot = join(cacheRoot, 'build', `${pinned.commit.slice(0, 12)}-${pinned.patch.sha256.slice(0, 12)}-${arch}`)
  await rm(buildRoot, { recursive: true, force: true })
  await cp(source, buildRoot, { recursive: true, filter: path => !path.split('/').includes('.git') })
  const applied = spawnSync('patch', ['-p1', '-i', patchFile], { cwd: buildRoot, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] })
  if (applied.status !== 0) throw new Error(`Cannot apply pinned router patch: ${applied.stderr.trim() || applied.stdout.trim()}`)

  const cache = join(cacheRoot, pinned.version, `darwin-${arch}`)
  await mkdir(cache, { recursive: true })
  const output = join(buildRoot, 'model-router')
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
      CGO_ENABLED: '1',
      GOOS: 'darwin',
      GOARCH: arch,
      GOCACHE: join(cacheRoot, 'gocache'),
      GOMODCACHE: join(cacheRoot, 'gomod'),
      GOFLAGS: '-mod=mod',
      PATH: `${dirname(go)}:${process.env.PATH ?? '/usr/bin:/bin'}`
    }
  })
  if (build.status !== 0) throw new Error('Patched router build failed')
  const version = spawnSync(output, ['-version'], { encoding: 'utf8' })
  if (!`${version.stdout}${version.stderr}`.includes(`Version: ${pinned.version}`)) throw new Error('Patched router does not report the pinned version')

  await rm(join(cache, 'model-router'), { force: true })
  await cp(output, join(cache, 'model-router'))
  await chmod(join(cache, 'model-router'), 0o755)
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
    binarySha256: await digest(join(cache, 'model-router'))
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
  console.log(await buildPatchedRouter({ ...(arch ? { arch } : {}), packageResources: process.argv.includes('--package') }))
}
