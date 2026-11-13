import { createHash, randomUUID } from 'node:crypto'
import { spawnSync } from 'node:child_process'
import { createReadStream, createWriteStream } from 'node:fs'
import { access, chmod, copyFile, lstat, mkdir, readFile, rename, rm, writeFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { Readable } from 'node:stream'
import { pipeline } from 'node:stream/promises'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..')
export const pinnedRouter = JSON.parse(await readFile(new URL('./version.json', import.meta.url), 'utf8'))
async function digest(file) {
  const hash = createHash('sha256')
  for await (const chunk of createReadStream(file)) hash.update(chunk)
  return hash.digest('hex')
}
async function download(url, file) {
  try { await access(file); return } catch { /* A missing cache entry is downloaded only by this explicit build command. */ }
  const temporary = `${file}.${randomUUID()}.tmp`
  try {
    const response = await fetch(url, { signal: AbortSignal.timeout(120_000) })
    if (!response.ok || !response.body) throw new Error(`Pinned router download failed: HTTP ${response.status}`)
    await pipeline(Readable.fromWeb(response.body), createWriteStream(temporary, { flags: 'wx', mode: 0o600 }))
    await rename(temporary, file)
  } finally { await rm(temporary, { force: true }) }
}
export async function prepareModelRouter({ arch = process.env.MATOU_MAC_ARCH || process.arch, packageResources = false } = {}) {
  const asset = pinnedRouter.assets[arch]
  if (!asset) throw new Error(`Model router is available only for macOS arm64/x64: ${arch}`)
  const cache = join(root, '.cache/model-router', pinnedRouter.version, `darwin-${arch}`)
  await mkdir(cache, { recursive: true })
  const base = `https://github.com/${pinnedRouter.repository}/releases/download/v${pinnedRouter.version}`
  const checksums = join(cache, 'checksums.txt'), archive = join(cache, asset.name)
  await download(`${base}/checksums.txt`, checksums)
  await download(`${base}/${asset.name}`, archive)
  const published = (await readFile(checksums, 'utf8')).split(/\r?\n/).find(line => line.trim().split(/\s+/)[1] === asset.name)?.trim().split(/\s+/)[0]
  if (published !== asset.sha256 || await digest(archive) !== published) throw new Error(`Pinned router checksum mismatch: ${asset.name}`)
  const extraction = join(cache, `.extract-${randomUUID()}`)
  await mkdir(extraction)
  try {
    const listing = spawnSync('tar', ['-tzf', archive], { encoding: 'utf8' })
    if (listing.status !== 0 || listing.stdout.split(/\r?\n/).filter(Boolean).some(path => path.startsWith('/') || path.split('/').includes('..'))) throw new Error('Unsafe model router archive')
    const extracted = spawnSync('tar', ['-xzf', archive, '-C', extraction, 'cli-proxy-api', 'LICENSE'], { encoding: 'utf8' })
    if (extracted.status !== 0) throw new Error('Cannot extract pinned model router')
    const binary = join(extraction, 'cli-proxy-api')
    if (!(await lstat(binary)).isFile() || !(await lstat(join(extraction, 'LICENSE'))).isFile()) throw new Error('Invalid model router release files')
    const header = (await readFile(binary)).subarray(0, 8)
    if (header.readUInt32LE(0) !== 0xfeedfacf || header.readUInt32LE(4) !== (arch === 'arm64' ? 0x100000c : 0x1000007)) throw new Error('Model router architecture mismatch')
    await copyFile(binary, join(cache, 'model-router')); await chmod(join(cache, 'model-router'), 0o755)
    await copyFile(join(extraction, 'LICENSE'), join(cache, 'LICENSE'))
    await writeFile(join(cache, 'source.json'), JSON.stringify({ version: pinnedRouter.version, commit: pinnedRouter.commit, repository: pinnedRouter.repository, arch, archive: asset.name, archiveSha256: published, binarySha256: await digest(binary) }, null, 2) + '\n')
  } finally { await rm(extraction, { recursive: true, force: true }) }
  if (packageResources) {
    const destination = join(root, 'apps/desktop/package-resources/model-router')
    await mkdir(destination, { recursive: true })
    for (const file of ['model-router', 'LICENSE', 'source.json']) await copyFile(join(cache, file), join(destination, file))
    await chmod(join(destination, 'model-router'), 0o755)
  }
  return cache
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const arch = process.argv.find(arg => arg.startsWith('--arch='))?.slice(7)
  console.log(await prepareModelRouter({ ...(arch ? { arch } : {}), packageResources: process.argv.includes('--package') }))
}
