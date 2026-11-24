import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { mkdir, mkdtemp, readFile, readdir, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { test } from 'node:test'
import { fileURLToPath } from 'node:url'

import { verifyWindowsDirPackage } from './native-parity/package-native-win.mjs'
import { desktopConfig } from './native-release-helpers.mjs'
import { generateWindowsManifest } from './release-native-win.mjs'

const script = fileURLToPath(new URL('./release-native-win.mjs', import.meta.url))
const { version } = await desktopConfig()
const releaseDate = '2026-11-24T11:00:00.000Z'
const feed = 'https://updates.baize-ailabs.com/stable'

const digest = (bytes, algorithm = 'sha512', encoding = 'base64') => createHash(algorithm).update(bytes).digest(encoding)

for (const [artifactName, url] of [
  ['Matou-0.3.0-win-x64.zip', 'Matou-0.3.0-win-x64.zip'],
  ['Matou Setup #1.exe', '"Matou%20Setup%20%231.exe"']
]) {
  test(`writes updater fields and verifies SHA-512 and size for ${artifactName}`, async t => {
    const root = await mkdtemp(join(tmpdir(), 'matou-win-manifest-test-'))
    t.after(() => rm(root, { recursive: true, force: true }))
    const artifact = join(root, artifactName)
    const bytes = Buffer.from([9, 0, 255, 9])
    await writeFile(artifact, bytes)
    const sha512 = digest(bytes)
    const manifest = await generateWindowsManifest({ version, artifact, releaseDate })
    assert.equal(manifest, `version: ${version}
files:
  - url: ${url}
    sha512: ${sha512}
    size: ${bytes.length}
path: ${url}
sha512: ${sha512}
releaseDate: '${releaseDate}'
`)
    await writeFile(artifact, Buffer.concat([bytes, bytes]))
    const changed = await generateWindowsManifest({ version, artifact, releaseDate })
    assert.match(changed, /size: 8/)
    assert.ok(!changed.includes(sha512))
  })
}

test('rejects invalid artifact paths, empty files and malformed versions', async t => {
  const root = await mkdtemp(join(tmpdir(), 'matou-win-invalid-manifest-'))
  t.after(() => rm(root, { recursive: true, force: true }))
  await assert.rejects(generateWindowsManifest({ version, artifact: join(root, 'package.dmg') }), /must be an .exe or .zip/)
  await assert.rejects(generateWindowsManifest({ version, artifact: join(root, 'missing.zip') }), { code: 'ENOENT' })
  const artifact = join(root, 'empty.zip')
  await writeFile(artifact, '')
  await assert.rejects(generateWindowsManifest({ version, artifact }), /non-empty/)
  await writeFile(artifact, 'bytes')
  await assert.rejects(generateWindowsManifest({ version: '1.2', artifact }), /Invalid release version/)
})

function peFixture(arch, dll = false) {
  // The directory packager checks the PE headers. These fixtures cannot execute.
  const bytes = Buffer.alloc(90)
  bytes.write('MZ')
  bytes.writeUInt32LE(64, 0x3c)
  bytes.write('PE\0\0', 64)
  bytes.writeUInt16LE(arch === 'x64' ? 0x8664 : 0xaa64, 68)
  bytes.writeUInt16LE(2, 84)
  bytes.writeUInt16LE(dll ? 0x2002 : 2, 86)
  bytes.writeUInt16LE(0x20b, 88)
  return bytes
}

for (const arch of ['x64', 'arm64']) {
  test(`CLI releases and verifies a Windows ${arch} ZIP, manifest and checksums`, async t => {
    const root = await mkdtemp(join(tmpdir(), 'matou-win-release-test-'))
    t.after(() => rm(root, { recursive: true, force: true }))
    const desktop = join(root, 'desktop.exe')
    const runtime = join(root, 'runtime.exe')
    const dll = join(root, 'ghostty-vt.dll')
    const installer = join(root, 'Matou-Setup.exe')
    await Promise.all([writeFile(desktop, peFixture(arch)), writeFile(runtime, peFixture(arch)),
      writeFile(dll, peFixture(arch, true)), writeFile(installer, peFixture(arch))])

    const outDir = join(root, 'release artifacts')
    const zip = `Matou-${version}-win-${arch}.zip`
    const checksums = `SHA256SUMS-${version}-win-${arch}.txt`
    const feedArtifact = arch === 'arm64' ? 'Matou-Setup.exe' : zip
    const args = [script, '--arch', arch, '--desktop-bin', desktop, '--runtime-bin', runtime,
      '--ghostty-dll', dll, '--out-dir', outDir, '--update-base-url', feed]
    if (arch === 'arm64') args.push('--installer', installer)
    const result = spawnSync(process.execPath, args, { encoding: 'utf8' })
    assert.ifError(result.error)
    assert.equal(result.status, 0, result.stderr)
    const names = [zip, 'stable.yml', ...(arch === 'arm64' ? [feedArtifact] : [])]
    assert.deepEqual((await readdir(outDir)).sort(), [...names, checksums].sort())
    const manifest = await readFile(join(outDir, 'stable.yml'), 'utf8')
    const payload = await readFile(join(outDir, feedArtifact))
    assert.ok(manifest.includes(`  - url: ${feedArtifact}\n    sha512: ${digest(payload)}\n    size: ${payload.length}\n`))
    assert.ok(manifest.includes(`\npath: ${feedArtifact}\n`))
    const hashes = await Promise.all(names.map(async name =>
      `${digest(await readFile(join(outDir, name)), 'sha256', 'hex')}  ${name}`))
    assert.deepEqual((await readFile(join(outDir, checksums), 'utf8')).trim().split('\n').sort(), hashes.sort())

    const extracted = join(root, arch === 'x64' ? 'win-unpacked' : 'win-arm64-unpacked')
    await mkdir(extracted)
    const zipPath = join(outDir, zip)
    const extract = process.platform === 'win32'
      ? spawnSync('tar.exe', ['-xf', zipPath, '-C', extracted], { encoding: 'utf8' })
      : spawnSync('unzip', ['-q', zipPath, '-d', extracted], { encoding: 'utf8' })
    assert.ifError(extract.error)
    assert.equal(extract.status, 0, extract.stderr)
    await verifyWindowsDirPackage(extracted, { arch, version, updateBaseUrl: feed })

    const duplicate = spawnSync(process.execPath, args, { encoding: 'utf8' })
    assert.equal(duplicate.status, 1)
    assert.match(duplicate.stderr, /Output already exists/)
    assert.equal(await readFile(join(outDir, 'stable.yml'), 'utf8'), manifest)
  })
}
