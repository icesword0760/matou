import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { mkdir, mkdtemp, readFile, readdir, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { basename, join, sep } from 'node:path'
import { test } from 'node:test'

import { verifyWindowsDirPackage } from './native-parity/package-native-win.mjs'
import { runTool } from './native-release-helpers.mjs'
import { releaseWindows, windowsReleaseOptions } from './release-native-win.mjs'

const version = '1.2.3'
const releaseDate = '2026-11-24T11:00:00.000Z'
const feed = 'https://updates.example.invalid/stable'
const thumbprint = 'A'.repeat(40)
const digest = (bytes, algorithm = 'sha512', encoding = 'base64') => createHash(algorithm).update(bytes).digest(encoding)

function peFixture({ arch = 'x64', dll = false, imports = [], delay = [], marker = '' } = {}) {
  // Synthetic PE32+ headers and import tables. These images cannot execute.
  const bytes = Buffer.alloc(0x600)
  const optional = 0x58
  const section = optional + 0xf0
  bytes.write('MZ')
  bytes.writeUInt32LE(0x40, 0x3c)
  bytes.write('PE\0\0', 0x40)
  bytes.writeUInt16LE(arch === 'x64' ? 0x8664 : 0xaa64, 0x44)
  bytes.writeUInt16LE(1, 0x46)
  bytes.writeUInt16LE(0xf0, 0x54)
  bytes.writeUInt16LE(dll ? 0x2002 : 2, 0x56)
  bytes.writeUInt16LE(0x20b, optional)
  bytes.writeUInt32LE(0x200, optional + 60)
  bytes.writeUInt32LE(16, optional + 108)
  bytes.write('.rdata', section)
  bytes.writeUInt32LE(0x1000, section + 12)
  bytes.writeUInt32LE(0x400, section + 16)
  bytes.writeUInt32LE(0x200, section + 20)
  let nameAddress = 0x1200
  for (const [names, directory, address, stride, nameField] of [
    [imports, 1, 0x1000, 20, 12], [delay, 13, 0x1100, 32, 4]
  ]) {
    if (!names.length) continue
    const data = optional + 112 + 8 * directory
    bytes.writeUInt32LE(address, data)
    bytes.writeUInt32LE((names.length + 1) * stride, data + 4)
    for (const [index, name] of names.entries()) {
      const descriptor = address - 0xe00 + index * stride
      if (directory === 13) bytes.writeUInt32LE(1, descriptor)
      bytes.writeUInt32LE(nameAddress, descriptor + nameField)
      bytes.write(name + '\0', nameAddress - 0xe00)
      nameAddress += name.length + 1
    }
  }
  bytes.write(marker, 0x5e0)
  return bytes
}

async function fixtures(t, arch = 'x64') {
  const root = await mkdtemp(join(tmpdir(), 'matou-win-release-test-'))
  t.after(() => rm(root, { recursive: true, force: true }))
  const desktopBin = join(root, 'desktop.exe')
  const runtimeBin = join(root, 'runtime.exe')
  const ghosttyDll = join(root, 'ghostty-vt.dll')
  const accountsDir = join(root, 'Claude accounts 码头')
  const pythonDir = join(accountsDir, '_internal')
  const modelRouterDir = join(root, 'router')
  const vcRuntimeDir = join(root, 'Microsoft.VC143.CRT')
  await Promise.all([mkdir(pythonDir, { recursive: true }), mkdir(modelRouterDir), mkdir(vcRuntimeDir)])
  const router = peFixture({ arch, marker: 'router' })
  await Promise.all([
    writeFile(desktopBin, peFixture({ arch, imports: ['VCRUNTIME140.dll'], marker: 'desktop' })),
    writeFile(runtimeBin, peFixture({ arch, delay: ['msvcp140.dll'], marker: 'runtime' })),
    writeFile(ghosttyDll, peFixture({ arch, dll: true, marker: 'terminal' })),
    writeFile(join(accountsDir, 'matou-claude-accounts.exe'), peFixture({ arch, marker: 'accounts' })),
    writeFile(join(accountsDir, 'UPSTREAM-LICENSE'), 'accounts license'),
    writeFile(join(pythonDir, 'base_library.zip'), 'standard library'),
    writeFile(join(pythonDir, 'python312.dll'), peFixture({ arch, dll: true, imports: ['vcruntime140.dll'] })),
    writeFile(join(pythonDir, 'vcruntime140.dll'), peFixture({ arch, dll: true, marker: 'older runtime' })),
    writeFile(join(modelRouterDir, 'model-router.exe'), router),
    writeFile(join(modelRouterDir, 'LICENSE'), 'router license'),
    writeFile(join(modelRouterDir, 'source.json'), JSON.stringify({ binarySha256: digest(router, 'sha256', 'hex') })),
    writeFile(join(vcRuntimeDir, 'vcruntime140.dll'), peFixture({ arch, dll: true, marker: 'redistributable' })),
    writeFile(join(vcRuntimeDir, 'msvcp140.dll'), peFixture({ arch, dll: true, imports: ['vcruntime140_1.dll'] })),
    writeFile(join(vcRuntimeDir, 'vcruntime140_1.dll'), peFixture({ arch, dll: true }))
  ])
  return {
    root, options: { arch, version, releaseDate, updateBaseUrl: feed, desktopBin, runtimeBin, ghosttyDll,
      accountsDir, modelRouterDir, vcRuntimeDir, outDir: join(root, 'release artifacts'),
      env: { MATOU_WIN_SIGN_CERT_SHA1: thumbprint, MATOU_WIN_ISCC: 'fixture-iscc.exe' } }
  }
}

function windowsTools(options, variation = {}) {
  return async (tool, args, runOptions) => {
    if (tool === 'powershell.exe') {
      const files = JSON.parse(await readFile(args.at(-1), 'utf8'))
      const checkingCrt = files.every(path => path.endsWith('.dll'))
      assert.equal(args.includes('-EmbeddedSignTool'), checkingCrt)
      if (checkingCrt) assert.equal(args[args.indexOf('-EmbeddedSignTool') + 1],
        options.env.MATOU_WIN_SIGNTOOL ?? 'signtool.exe')
      const signatures = files.map(path => path.endsWith('.dll') ? {
        status: variation.crtStatus ?? 'Valid', signerName: variation.crtSigner ?? 'Microsoft Windows Software Compatibility Publisher',
        signerSubject: 'CN=Microsoft Windows Software Compatibility Publisher, O=Microsoft Corporation, C=US'
      } : {
        status: variation.status ?? 'Valid', thumbprint: variation.thumbprint ?? thumbprint,
        timestamped: variation.timestamped ?? true, productVersion: variation.version ?? options.version,
        productName: variation.productName ?? (options.arch === 'arm64' ? 'Matou ARM64' : 'Matou x64')
      })
      return { stdout: JSON.stringify(signatures) }
    }
    if (tool !== 'fixture-iscc.exe') return runTool(tool, args, runOptions)
    const parameter = prefix => args.find(arg => arg.startsWith(prefix))?.slice(prefix.length)
    const version = parameter('-DMatouVersion=')
    assert.equal(version, options.version)
    assert.equal(parameter('-DMatouFileVersion='), '1.2.3.0')
    const signed = parameter('-DMatouSigned') !== undefined
    assert.equal(signed, !options.internalInstaller)
    assert.equal(args.includes('-DMatouArm64'), options.arch === 'arm64')
    if (signed) {
      assert.match(parameter('-smatou_release_signing='), new RegExp(`-Thumbprint ${thumbprint} -Timestamp `))
    }
    const fileList = await readFile(parameter('-DMatouPayloadFiles='), 'utf8')
    for (const name of ['码头.exe', 'matou-native-runtime.exe', 'ghostty-vt.dll', 'matou-claude-accounts.exe',
      'model-router.exe', 'vcruntime140.dll', 'msvcp140.dll', 'vcruntime140_1.dll']) {
      assert.ok(fileList.includes(`${sep}${name}"; DestDir:`), name)
    }
    assert.ok(!fileList.includes('session-start.mjs'))
    const redistributed = fileList.match(/^Source: "([^"]+[\\/]_internal[\\/]vcruntime140\.dll)"/m)[1]
    assert.deepEqual(await readFile(redistributed), await readFile(join(options.vcRuntimeDir, 'vcruntime140.dll')))
    const versionMetadata = fileList.match(/^Source: "([^"]+[\\/]resources[\\/]version\.json)"/m)[1]
    const metadata = JSON.parse(await readFile(versionMetadata, 'utf8'))
    assert.equal(metadata.version, version)
    assert.equal(metadata.update.channel, options.internalInstaller ? 'internal' : 'stable')
    if (variation.changePayload) await writeFile(versionMetadata, '{}')
    if (!variation.noInstaller) {
      await writeFile(join(parameter('-o'), `${parameter('-f')}.exe`), peFixture({ arch: variation.installerArch }))
    }
    if (signed && !variation.noUninstaller) {
      await writeFile(join(parameter('-DMatouUninstallerDir='), 'uninstaller.exe'), peFixture())
    }
    return { stdout: '' }
  }
}

test('stable publishes only the validated signed x64 installer in the feed and hashes the final files', async t => {
  const { options } = await fixtures(t)
  options.withZip = true
  const names = [
    `Matou-${version}-win-x64.exe`, `Matou-${version}-win-x64.zip`,
    'stable.yml', `SHA256SUMS-${version}-win-x64.txt`
  ]
  const [exe, zip, manifest, checksums] = names.map(name => join(options.outDir, name))
  await releaseWindows(options, windowsTools(options))
  assert.deepEqual((await readdir(options.outDir)).sort(), names.sort())
  const payload = await readFile(exe)
  const yaml = await readFile(manifest, 'utf8')
  assert.equal(yaml, `version: ${version}\nfiles:\n  - url: ${basename(exe)}\n    sha512: ${digest(payload)}\n    size: ${payload.length}\npath: ${basename(exe)}\nsha512: ${digest(payload)}\nreleaseDate: '${releaseDate}'\n`)
  const hashes = await Promise.all([exe, zip, manifest].map(async path =>
    `${digest(await readFile(path), 'sha256', 'hex')}  ${basename(path)}`))
  assert.equal(await readFile(checksums, 'utf8'), hashes.join('\n') + '\n')
  await assert.rejects(releaseWindows(options, windowsTools(options)), /Output already exists/)
  assert.equal(await readFile(manifest, 'utf8'), yaml)
})

test('internal x64 installer uses a separate artifact and update channel with no stable.yml', async t => {
  const { options } = await fixtures(t)
  options.internalInstaller = true
  delete options.env.MATOU_WIN_SIGN_CERT_SHA1
  await releaseWindows(options, windowsTools(options, { status: 'NotSigned' }))
  assert.deepEqual((await readdir(options.outDir)).sort(), [
    `Matou-${version}-win-x64-internal.exe`, `SHA256SUMS-${version}-win-x64-internal.txt`
  ])
})

test('publishes an ARM64 payload on its own stable feed with the Inno x64 loader', async t => {
  const { options } = await fixtures(t, 'arm64')
  const setup = windowsReleaseOptions(['--arch', 'arm64'], {
    MATOU_UPDATE_BASE_URL: 'https://updates.example.invalid/win-x64'
  }, { version })
  assert.equal(setup.updateBaseUrl, 'https://updates.baize-ailabs.com/stable/win-arm64')
  assert.equal(basename(setup.outDir), 'win-arm64')
  assert.equal(windowsReleaseOptions(['--arch', 'arm64'], {
    MATOU_WIN_ARM64_UPDATE_BASE_URL: 'https://updates.example.invalid/arm64'
  }, { version }).updateBaseUrl, 'https://updates.example.invalid/arm64')
  assert.throws(() => windowsReleaseOptions(['--arch', 'arm64', '--update-base-url',
    'https://updates.baize-ailabs.com/stable/'], {}, { version }), /feed for the selected architecture/)
  assert.throws(() => windowsReleaseOptions(['--arch', 'x64', '--update-base-url',
    setup.updateBaseUrl], {}, { version }), /feed for the selected architecture/)
  await releaseWindows({ ...options, updateBaseUrl: setup.updateBaseUrl }, windowsTools(options))
  const exe = 'Matou-1.2.3-win-arm64.exe'
  const manifest = await readFile(join(options.outDir, 'stable.yml'), 'utf8')
  assert.match(manifest, /url: Matou-1\.2\.3-win-arm64\.exe/)
  assert.ok(manifest.includes(digest(await readFile(join(options.outDir, exe)))))
  assert.deepEqual((await readdir(options.outDir)).sort(), [exe, 'SHA256SUMS-1.2.3-win-arm64.txt', 'stable.yml'])
})

test('refuses a mislabeled ARM64 installer and x64 CRT in the ARM64 payload', async t => {
  const { options } = await fixtures(t, 'arm64')
  await assert.rejects(releaseWindows(options, windowsTools(options, { productName: 'Matou x64' })),
    /installer payload architecture mismatch/)
  await writeFile(join(options.vcRuntimeDir, 'vcruntime140.dll'), peFixture({ dll: true }))
  await assert.rejects(releaseWindows(options, windowsTools(options)), /Visual C\+\+ runtime.*architecture mismatch/)
  assert.deepEqual(await readdir(options.outDir), [])
})

test('rejects missing native components and refused release versions or architectures', async t => {
  const { options, root } = await fixtures(t)
  for (const [changes, error] of [
    [{ desktopBin: join(root, 'missing.exe') }, /Missing desktop binary/],
    [{ runtimeBin: join(root, 'missing.exe') }, /Missing runtime binary/],
    [{ ghosttyDll: join(root, 'missing.dll') }, /Missing Ghostty DLL/],
    [{ accountsDir: join(root, 'missing directory') }, /Missing Claude accounts onedir/],
    [{ modelRouterDir: undefined }, /Missing model-router directory/],
    [{ arch: 'arm64' }, /architecture mismatch/],
    [{ version: '1.2.3-beta.1' }, /stable feed requires an X.Y.Z version/],
    [{ version: '1.2.03' }, /stable feed requires an X.Y.Z version/],
    [{ version: '1.2.65536' }, /at most 65535/]
  ]) {
    await assert.rejects(releaseWindows({ ...options, ...changes }, windowsTools(options)), error)
    assert.deepEqual(await readdir(options.outDir), [])
  }
  assert.throws(() => windowsReleaseOptions(['--installer', 'external.exe'], {}, { version }), /builds its installer/)
})

test('rejects untrusted CRT and any installer without matching signed build evidence', async t => {
  const { options } = await fixtures(t)
  const env = { ...options.env, MATOU_WIN_SIGN_CERT_SHA1: undefined }
  await assert.rejects(releaseWindows({ ...options, env }, windowsTools(options)), /MATOU_WIN_SIGN_CERT_SHA1/)
  for (const [variation, error] of [
    [{ crtStatus: 'NotSigned' }, /valid Microsoft Authenticode signature/],
    [{ crtSigner: 'Matou' }, /valid Microsoft Authenticode signature/],
    [{ noInstaller: true }, /Missing Matou installer/],
    [{ installerArch: 'arm64' }, /Matou installer architecture mismatch/],
    [{ noUninstaller: true }, /Missing Matou uninstaller/],
    [{ changePayload: true }, /payload changed during compilation/],
    [{ version: '1.2.4' }, /Windows installer version mismatch/],
    [{ thumbprint: 'B'.repeat(40) }, /valid, timestamped Matou Authenticode signature/],
    [{ timestamped: false }, /valid, timestamped Matou Authenticode signature/],
    [{ status: 'HashMismatch' }, /valid, timestamped Matou Authenticode signature/]
  ]) {
    await assert.rejects(releaseWindows(options, windowsTools(options, variation)), error)
    assert.deepEqual(await readdir(options.outDir), [])
  }
  await rm(join(options.vcRuntimeDir, 'vcruntime140_1.dll'))
  await assert.rejects(releaseWindows(options, windowsTools(options)), /Missing Visual C\+\+ runtime vcruntime140_1.dll/)
  assert.deepEqual(await readdir(options.outDir), [])
})

for (const arch of ['x64', 'arm64']) {
  test(`${arch} portable ZIP includes the verified Visual C++ runtime without publishing an update manifest`, async t => {
    const { root, options } = await fixtures(t, arch)
    const { outDir, vcRuntimeDir } = options
    const zip = `Matou-${version}-win-${arch}.zip`
    const checksums = `SHA256SUMS-${version}-win-${arch}.txt`
    options.portableOnly = true
    await releaseWindows(options, windowsTools(options))
    assert.deepEqual((await readdir(outDir)).sort(), [zip, checksums])
    const zipPath = join(outDir, zip)
    const extracted = join(root, arch === 'x64' ? 'win-unpacked' : 'win-arm64-unpacked')
    await mkdir(extracted)
    if (process.platform === 'win32') await runTool('tar.exe', ['-xf', zipPath, '-C', extracted])
    else await runTool('unzip', ['-q', zipPath, '-d', extracted])
    for (const name of ['vcruntime140.dll', 'msvcp140.dll', 'vcruntime140_1.dll']) {
      assert.deepEqual(await readFile(join(extracted, name)), await readFile(join(vcRuntimeDir, name)))
    }
    await verifyWindowsDirPackage(extracted, { ...options })
  })
}
