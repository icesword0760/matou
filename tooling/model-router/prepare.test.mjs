import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { chmod, copyFile, mkdir, mkdtemp, readFile, readdir, rm, stat, writeFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { test } from 'node:test'
import { fileURLToPath, pathToFileURL } from 'node:url'

const directory = dirname(fileURLToPath(import.meta.url))
const scratch = resolve(directory, '../../.cache/model-router')

async function fixture(t) {
  await mkdir(scratch, { recursive: true })
  const root = await mkdtemp(join(scratch, 'prepare-test-'))
  t.after(() => rm(root, { recursive: true, force: true }))
  const moduleDirectory = join(root, 'tooling/model-router')
  await mkdir(moduleDirectory, { recursive: true })
  for (const file of ['prepare.mjs', 'version.json']) await copyFile(join(directory, file), join(moduleDirectory, file))
  const { pinnedRouter, prepareModelRouter, stageModelRouter } = await import(pathToFileURL(join(moduleDirectory, 'prepare.mjs')).href)
  const cache = join(root, '.cache/model-router', pinnedRouter.version, 'darwin-arm64')
  await mkdir(cache, { recursive: true })
  const binary = Buffer.from('isolated router fixture')
  const info = {
    patched: true, arch: 'arm64', commit: pinnedRouter.commit,
    patchSha256: pinnedRouter.patch.sha256,
    binarySha256: createHash('sha256').update(binary).digest('hex')
  }
  await writeFile(join(cache, 'model-router'), binary)
  await writeFile(join(cache, 'LICENSE'), 'fixture license')
  const metadata = join(cache, 'source.json')
  await writeFile(metadata, JSON.stringify(info))
  const upstream = process.env.MATOU_ROUTER_UPSTREAM
  delete process.env.MATOU_ROUTER_UPSTREAM
  t.after(() => {
    if (upstream === undefined) delete process.env.MATOU_ROUTER_UPSTREAM
    else process.env.MATOU_ROUTER_UPSTREAM = upstream
  })
  return { root, cache, metadata, info, binary, prepareModelRouter, stageModelRouter }
}

test('current patched cache is reused and the verified bytes are packaged', async t => {
  const f = await fixture(t)
  assert.equal(await f.prepareModelRouter({ arch: 'arm64' }), f.cache)
  assert.equal(await f.prepareModelRouter({ arch: 'arm64', packageResources: true }), f.cache)
  for (const file of ['model-router', 'source.json', 'LICENSE']) {
    assert.deepEqual(await readFile(join(f.root, 'apps/native/package-resources/model-router', file)), await readFile(join(f.cache, file)))
  }
})

test('Windows packaging copies the .exe and its metadata without setting execute bits', async t => {
  const { root, cache, stageModelRouter } = await fixture(t)
  const executable = join(cache, 'model-router.exe')
  await writeFile(executable, 'Windows model router fixture')
  if (process.platform !== 'win32') await chmod(executable, 0o644)
  await rm(join(cache, 'model-router'))
  await stageModelRouter(cache, { platform: 'win32' })
  const output = join(root, 'apps/native/package-resources/model-router')
  const files = ['LICENSE', 'model-router.exe', 'source.json']
  assert.deepEqual((await readdir(output)).sort(), files)
  for (const file of files) {
    assert.deepEqual(await readFile(join(output, file)), await readFile(join(cache, file)))
  }
  if (process.platform !== 'win32') assert.equal((await stat(join(output, 'model-router.exe'))).mode & 0o777, 0o644)
})

for (const field of ['commit', 'patchSha256', 'arch', 'patched', 'binarySha256']) {
  test(`packaging rejects a mismatched ${field} without downloading or overwriting cache`, async t => {
    const f = await fixture(t)
    await writeFile(f.metadata, JSON.stringify({ ...f.info, [field]: field === 'patched' ? false : 'stale' }))
    const before = await readFile(f.metadata)
    const originalFetch = globalThis.fetch
    let downloads = 0
    globalThis.fetch = async () => { downloads++; throw new Error('unexpected network access') }
    t.after(() => { globalThis.fetch = originalFetch })
    await assert.rejects(f.prepareModelRouter({ arch: 'arm64', packageResources: true }), /run pnpm router:build/)
    assert.equal(downloads, 0)
    assert.deepEqual(await readFile(f.metadata), before)
    assert.deepEqual(await readFile(join(f.cache, 'model-router')), f.binary)
    await assert.rejects(readFile(join(f.root, 'apps/native/package-resources/model-router/model-router')), { code: 'ENOENT' })
  })
}

test('packaging rejects corrupted binary even with current commit and patch', async t => {
  const f = await fixture(t)
  await writeFile(join(f.cache, 'model-router'), 'tampered')
  await assert.rejects(f.prepareModelRouter({ arch: 'arm64', packageResources: true }), /run pnpm router:build/)
})

test('explicit non-packaging prepare treats stale patch as a cache miss', async t => {
  const f = await fixture(t)
  await writeFile(f.metadata, JSON.stringify({ ...f.info, commit: 'old' }))
  const originalFetch = globalThis.fetch
  const urls = []
  globalThis.fetch = async url => { urls.push(url); throw new Error('explicit upstream cache miss') }
  t.after(() => { globalThis.fetch = originalFetch })
  await assert.rejects(f.prepareModelRouter({ arch: 'arm64' }), /explicit upstream cache miss/)
  assert.equal(urls.length, 1)
  assert.match(urls[0], /\/checksums\.txt$/)
})
