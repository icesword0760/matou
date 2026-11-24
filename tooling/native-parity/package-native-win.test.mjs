import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { appendFile, mkdir, mkdtemp, readFile, readdir, rename, rm, symlink, writeFile } from 'node:fs/promises'
import { homedir, tmpdir } from 'node:os'
import { basename, join } from 'node:path'
import { after, before, test } from 'node:test'
import { fileURLToPath } from 'node:url'

import {
  assembleWindowsDirPackage, planWindowsPackage, projectWindowsInstallCase,
  validateWindowsInstallContract, verifyWindowsDirPackage
} from './package-native-win.mjs'

const reference = JSON.parse(await readFile(new URL('./fixtures/windows-install-reference.json', import.meta.url), 'utf8'))
const script = fileURLToPath(new URL('./package-native-win.mjs', import.meta.url))
const version = '0.2.24'
const desktopName = '码头.exe'
const runtimeName = 'matou-native-runtime.exe'
const ghosttyName = 'ghostty-vt.dll'
const modelRouterName = 'model-router.exe'
let root
const binaries = {}

function peFixture(machine, payload, dll = false) {
  // A DOS and PE32+ header with a marker payload. The file cannot run.
  const dos = Buffer.alloc(64)
  dos.write('MZ')
  dos.writeUInt32LE(dos.length, 0x3c)
  const pe = Buffer.alloc(24 + 0xf0)
  pe.write('PE\0\0')
  pe.writeUInt16LE(machine, 4)
  pe.writeUInt16LE(0xf0, 20)
  pe.writeUInt16LE(dll ? 0x2002 : 0x0002, 22)
  pe.writeUInt16LE(0x020b, 24)
  return Buffer.concat([dos, pe, Buffer.from(payload)])
}

before(async () => {
  root = await mkdtemp(join(tmpdir(), 'matou-native-win-test-'))
  for (const [arch, machine] of [['x64', 0x8664], ['arm64', 0xaa64]]) {
    const directory = join(root, arch)
    await mkdir(directory)
    const desktopBin = join(directory, 'desktop.exe')
    const runtimeBin = join(directory, 'runtime.exe')
    const ghosttyDll = join(directory, ghosttyName)
    await writeFile(desktopBin, peFixture(machine, 'desktop'))
    await writeFile(runtimeBin, peFixture(machine, 'runtime'))
    await writeFile(ghosttyDll, peFixture(machine, 'ghostty', true))
    binaries[arch] = { desktopBin, runtimeBin }
  }
})

after(async () => {
  if (root) await rm(root, { recursive: true, force: true })
})

for (const entry of reference.cases) {
  test(`reference / ${entry.id}`, () => {
    assert.deepEqual(projectWindowsInstallCase(entry), entry.expected)
  })
}

test('validates the complete reference and reports a changed result by case ID', () => {
  assert.equal(reference.cases.length, 44)
  assert.deepEqual(validateWindowsInstallContract(reference), {
    caseCount: 44, caseIds: reference.cases.map(entry => entry.id)
  })
  const changed = structuredClone(reference)
  changed.cases[0].expected.applicationExecutable = 'changed.exe'
  assert.throws(() => validateWindowsInstallContract(changed), /contract mismatch for frozen-dir-x64/)
  const duplicate = { ...reference, cases: [reference.cases[0], reference.cases[0]] }
  assert.throws(() => validateWindowsInstallContract(duplicate), /Duplicate Windows install reference case/)
})

function options(arch, outDir, extra = {}) {
  return { arch, target: 'dir', version, outDir, ...binaries[arch], ...extra }
}

for (const [arch, unpacked] of [['x64', 'win-unpacked'], ['arm64', 'win-arm64-unpacked']]) {
  test(`assembles and verifies the offline ${arch} directory`, async () => {
    const outDir = join(root, 'directory packages')
    const updateBaseUrl = 'https://feed.example.invalid/build?sample=1'
    const input = options(arch, outDir, { updateBaseUrl })
    const packageResult = await assembleWindowsDirPackage(input)
    assert.equal(basename(packageResult.path), unpacked)
    assert.equal(packageResult.unpackedDirectory, 'release/' + unpacked)
    assert.deepEqual(await readdir(packageResult.path).then(names => names.sort()),
      [desktopName, runtimeName, ghosttyName, 'resources'].sort())
    assert.deepEqual(await readFile(join(packageResult.path, desktopName)), await readFile(input.desktopBin))
    assert.deepEqual(await readFile(join(packageResult.path, runtimeName)), await readFile(input.runtimeBin))
    assert.deepEqual(await readFile(join(packageResult.path, ghosttyName)), await readFile(join(root, arch, ghosttyName)))
    const resources = join(packageResult.path, 'resources')
    assert.deepEqual(await readdir(resources).then(names => names.sort()), ['control-assets', 'manifest.json', 'version.json'])
    const plugin = join(resources, 'control-assets', 'providers', 'claude-plugin')
    assert.deepEqual(await readdir(plugin).then(names => names.sort()), ['.claude-plugin', 'hooks', 'skills'])
    assert.ok(JSON.parse(await readFile(join(plugin, '.claude-plugin', 'plugin.json'), 'utf8')).name)
    const load = async filename => JSON.parse(await readFile(join(resources, filename), 'utf8'))
    assert.deepEqual(await load('manifest.json'), {
      target: 'dir', arch, applicationExecutable: desktopName, runtimeExecutable: runtimeName,
      versionMetadata: 'version.json'
    })
    assert.deepEqual(await load('version.json'), {
      appId: 'com.matou.desktop', productName: '码头', version, arch,
      artifactNamePattern: 'Matou-${version}-${os}-${arch}.${ext}',
      update: { baseUrl: updateBaseUrl, channel: 'stable', appUpdateYml: null, releaseChannelManifest: null }
    })
    assert.deepEqual(await verifyWindowsDirPackage(packageResult.path, input), planWindowsPackage(input))
    assert.deepEqual((await readdir(outDir)).filter(name => name.startsWith('.matou-native-win-')), [])
  })
}

test('rejects missing, empty, non-PE or wrong-architecture inputs before writing', async () => {
  const outDir = join(root, 'invalid-inputs')
  const missing = join(root, 'missing.exe')
  const empty = join(root, 'empty.exe')
  const invalid = join(root, 'invalid.exe')
  await writeFile(empty, Buffer.alloc(0))
  await writeFile(invalid, 'fixture without a PE header')
  for (const [extra, error] of [
    [{ desktopBin: missing }, /Missing desktop binary/],
    [{ runtimeBin: missing }, /Missing runtime binary/],
    [{ runtimeBin: empty }, /non-empty regular file/],
    [{ desktopBin: invalid }, /Expected a Windows PE executable for desktop binary/],
    [{ desktopBin: binaries.arm64.desktopBin }, /desktop binary architecture mismatch: expected x64/],
    [{ runtimeBin: binaries.arm64.runtimeBin }, /runtime binary architecture mismatch: expected x64/],
    [{ ghosttyDll: join(root, 'missing.dll') }, /Missing Ghostty DLL/],
    [{ ghosttyDll: binaries.x64.runtimeBin }, /Expected a Windows PE DLL for Ghostty DLL/],
    [{ ghosttyDll: join(root, 'arm64', ghosttyName) }, /Ghostty DLL architecture mismatch: expected x64/]
  ]) {
    await assert.rejects(assembleWindowsDirPackage(options('x64', outDir, extra)), error)
  }
  await assert.rejects(readdir(outDir), { code: 'ENOENT' })
})

test('verifier rejects a missing Ghostty DLL, an executable or a DLL for the wrong architecture', async () => {
  const input = options('x64', join(root, 'verify-ghostty-dll'))
  const { path } = await assembleWindowsDirPackage(input)
  const dll = join(path, ghosttyName)
  await rm(dll)
  await assert.rejects(verifyWindowsDirPackage(path, input), /Invalid unpacked package entries/)
  await writeFile(dll, await readFile(binaries.x64.runtimeBin))
  await assert.rejects(verifyWindowsDirPackage(path, input), /Expected a Windows PE DLL/)
  await writeFile(dll, await readFile(join(root, 'arm64', ghosttyName)))
  await assert.rejects(verifyWindowsDirPackage(path, input), /Ghostty DLL architecture mismatch/)
})

test('verifier rejects missing or renamed companions, mismatched metadata and updater files', async () => {
  const input = options('x64', join(root, 'verify-layout'))
  const { path } = await assembleWindowsDirPackage(input)
  const runtime = join(path, runtimeName)
  const renamedRuntime = join(path, 'different.exe')
  await rename(runtime, renamedRuntime)
  await assert.rejects(verifyWindowsDirPackage(path, input), /Invalid unpacked package entries/)
  await rename(renamedRuntime, runtime)
  await writeFile(runtime, await readFile(binaries.arm64.runtimeBin))
  await assert.rejects(verifyWindowsDirPackage(path, input), /runtime binary architecture mismatch/)
  await writeFile(runtime, await readFile(input.runtimeBin))

  const resources = join(path, 'resources')
  const manifest = join(resources, 'manifest.json')
  const original = await readFile(manifest, 'utf8')
  await writeFile(manifest, JSON.stringify({ ...JSON.parse(original), runtimeExecutable: 'different.exe' }))
  await assert.rejects(verifyWindowsDirPackage(path, input), /Invalid manifest.json/)
  await writeFile(manifest, original)
  await assert.rejects(verifyWindowsDirPackage(path, { ...input, version: '0.2.25' }), /Invalid version.json/)

  for (const [file, error] of [
    [join(resources, 'app-update.yml'), /Invalid resources entries/],
    [join(path, 'stable.yml'), /Invalid unpacked package entries/]
  ]) {
    await writeFile(file, 'version: 0.2.24\n')
    await assert.rejects(verifyWindowsDirPackage(path, input), error)
    await rm(file)
  }
  await verifyWindowsDirPackage(path, input)
})

test('copies an accounts onedir and verifies the bundled PE32+ architecture', async () => {
  const accountsDir = join(root, 'accounts source')
  const executable = 'matou-claude-accounts.exe'
  const source = join(accountsDir, executable)
  await mkdir(join(accountsDir, '_internal'), { recursive: true })
  const engine = peFixture(0xaa64, 'account engine')
  await writeFile(source, engine)
  await writeFile(join(accountsDir, '_internal', 'dependency.dll'), 'dependency')
  const input = options('arm64', join(root, 'with accounts'), { accountsDir })
  const { path } = await assembleWindowsDirPackage(input)
  const installed = join(path, 'resources', 'claude-accounts', 'matou-claude-accounts')
  assert.deepEqual(await readFile(join(installed, executable)), engine)
  assert.equal(await readFile(join(installed, '_internal', 'dependency.dll'), 'utf8'), 'dependency')
  await verifyWindowsDirPackage(path, { arch: 'arm64', version })
  await assert.rejects(assembleWindowsDirPackage(options('x64', join(root, 'wrong engine'), {
    accountsDir
  })), /Claude accounts executable architecture mismatch/)
  await writeFile(join(installed, executable), 'not an executable')
  await assert.rejects(verifyWindowsDirPackage(path, input), /Expected a Windows PE executable for Claude accounts/)
})

test('rejects non-isolated, protected and invalid output paths', async () => {
  const guarded = join(root, 'guarded')
  const existingFile = join(root, 'out-file')
  await writeFile(existingFile, 'keep')
  const protectedNames = ['Program Files', 'PROGRAM FILES (x86)', 'LOCALAPPDATA', 'Windows', 'ProgramData']
  for (const path of [
    '', tmpdir(), homedir(), 'C:\\Program Files\\Matou', 'C:\\Users\\Fixture\\AppData\\Local\\Programs',
    ...protectedNames.map(name => join(guarded, name, 'output')),
    join(guarded, 'NUL.exe'), join(guarded, 'trailing.'), join(guarded, 'trailing '),
    guarded + '/child/../output', guarded + '/with\0null', existingFile, join(existingFile, 'child')
  ]) {
    await assert.rejects(assembleWindowsDirPackage(options('x64', path, {
      desktopBin: join(root, 'unset.exe')
    })), /--out-dir must be an isolated temporary subdirectory/, String(path))
  }
  assert.equal(await readFile(existingFile, 'utf8'), 'keep')
  await assert.rejects(readdir(guarded), { code: 'ENOENT' })
})

test('verifier rejects a missing Claude plugin manifest or a changed hook', async () => {
  const input = options('x64', join(root, 'verify-claude-plugin'))
  const { path } = await assembleWindowsDirPackage(input)
  const plugin = join(path, 'resources', 'control-assets', 'providers', 'claude-plugin')
  const manifest = join(plugin, '.claude-plugin', 'plugin.json')
  const original = await readFile(manifest)
  await rm(manifest)
  await assert.rejects(verifyWindowsDirPackage(path, input), /Missing Claude plugin asset/)
  await writeFile(manifest, original)
  await writeFile(join(plugin, 'hooks', 'session-start.mjs'), 'changed')
  await assert.rejects(verifyWindowsDirPackage(path, input), /Claude plugin asset differs from source/)
})

test('rejects output links and Windows junctions without writing through them', async () => {
  const protectedFixture = join(root, 'Program Files')
  const redirect = join(root, 'redirect')
  await mkdir(protectedFixture)
  await symlink(protectedFixture, redirect, process.platform === 'win32' ? 'junction' : 'dir')
  await assert.rejects(assembleWindowsDirPackage(options('x64', join(redirect, 'output'))),
    /--out-dir must be an isolated temporary subdirectory/)
  assert.deepEqual(await readdir(protectedFixture), [])
})

test('concurrent assembly leaves one complete package and rejects replacement', async () => {
  const outDir = join(root, 'concurrent')
  const input = options('x64', outDir)
  const results = await Promise.allSettled([assembleWindowsDirPackage(input), assembleWindowsDirPackage(input)])
  const complete = results.filter(result => result.status === 'fulfilled')
  const rejected = results.filter(result => result.status === 'rejected')
  assert.equal(complete.length, 1)
  assert.equal(rejected.length, 1)
  assert.match(rejected[0].reason.message, /Output package already exists/)
  await verifyWindowsDirPackage(complete[0].value.path, input)
  await assert.rejects(assembleWindowsDirPackage(input), /Output package already exists/)
  assert.deepEqual(await readdir(outDir), ['win-unpacked'])
})

test('plans a safe HTTP(S) URL without creating packaged or published updater manifests', () => {
  const localFeed = 'http://127.0.0.1:9/releases?q=1'
  const plan = planWindowsPackage({ arch: 'arm64', version: '0.2.24-beta.1', updateBaseUrl: '  ' + localFeed + '  ' })
  assert.equal(plan.updateBaseUrl, localFeed)
  assert.equal(plan.appUpdateYml, null)
  assert.equal(plan.releaseChannelManifest, null)
  assert.deepEqual(plan.artifactTargets, [])
  // Current native package metadata follows the GitHub feed. The fixture captures the old feed.
  assert.equal(planWindowsPackage({ arch: 'x64', version }).updateBaseUrl,
    'https://github.com/icesword0760/matou/releases/latest/download')
  for (const url of ['', ' ', 'relative/path', 'https://', 'file:///releases', 'https://example.invalid/\nnext']) {
    assert.throws(() => planWindowsPackage({ arch: 'x64', version, updateBaseUrl: url }), /absolute HTTP\(S\) URL/)
  }
})

function cli(args) {
  const result = spawnSync(process.execPath, [script, ...args], { encoding: 'utf8' })
  assert.ifError(result.error)
  return result
}

async function routerFixture(label, binary) {
  const directory = join(root, label)
  await mkdir(directory)
  await writeFile(join(directory, modelRouterName), binary)
  await writeFile(join(directory, 'LICENSE'), 'Fixture license\n')
  const binarySha256 = createHash('sha256').update(binary).digest('hex')
  await writeFile(join(directory, 'source.json'), JSON.stringify({ version: '8.0.69', binarySha256 }))
  return directory
}

for (const [arch, machine] of [['x64', 0x8664], ['arm64', 0xaa64]]) {
  test(`CLI assembles and verifies the ${arch} model-router resources`, async () => {
    const outDir = join(root, `CLI with ${arch} model-router`)
    const modelRouterDir = await routerFixture(`${arch} model-router`, peFixture(machine, 'engine'))
    const { desktopBin, runtimeBin } = binaries[arch]
    const result = cli([
      '--arch', arch, '--desktop-bin', desktopBin, '--runtime-bin', runtimeBin,
      '--out-dir', outDir, '--version', version, '--model-router', modelRouterDir
    ])
    assert.equal(result.status, 0, result.stderr)
    const { path } = JSON.parse(result.stdout)
    const packaged = join(path, 'resources', 'model-router')
    assert.deepEqual((await readdir(packaged)).sort(), ['LICENSE', modelRouterName, 'source.json'])
    for (const name of await readdir(packaged)) {
      assert.deepEqual(await readFile(join(packaged, name)), await readFile(join(modelRouterDir, name)))
    }
    await verifyWindowsDirPackage(path, options(arch, outDir))
  })
}

test('rejects a wrong-model-router machine, PE32 executable and hash tampering', async () => {
  const outDir = join(root, 'rejected model-router')
  const modelRouterDir = await routerFixture('wrong model-router', peFixture(0x8664, 'engine'))
  const input = options('arm64', outDir, { modelRouterDir })
  await assert.rejects(assembleWindowsDirPackage(input), /model-router binary architecture mismatch: expected arm64, found PE machine 0x8664/)
  const pe32 = peFixture(0xaa64, 'engine')
  pe32.writeUInt16LE(0x010b, 64 + 24)
  await writeFile(join(modelRouterDir, modelRouterName), pe32)
  await assert.rejects(assembleWindowsDirPackage(input), /Expected a Windows PE executable for model-router binary/)

  const valid = peFixture(0xaa64, 'engine')
  const validSource = await routerFixture('valid model-router', valid)
  const validInput = options('arm64', outDir, { modelRouterDir: validSource })
  const sourceBinary = join(validSource, modelRouterName)
  await appendFile(sourceBinary, 'tampered')
  await assert.rejects(assembleWindowsDirPackage(validInput), /model-router binarySha256 mismatch/)
  assert.deepEqual(await readdir(outDir), [])

  await writeFile(sourceBinary, valid)
  const { path } = await assembleWindowsDirPackage(validInput)
  const packaged = join(path, 'resources', 'model-router')
  await writeFile(join(packaged, 'LICENSE'), '')
  await assert.rejects(verifyWindowsDirPackage(path, validInput), /model-router LICENSE must be a non-empty regular file/)
  await writeFile(join(packaged, 'LICENSE'), 'Fixture license\n')
  await appendFile(join(packaged, modelRouterName), 'tampered')
  await assert.rejects(verifyWindowsDirPackage(path, options('arm64', outDir)), /model-router binarySha256 mismatch/)
  await rm(packaged, { recursive: true })
  await assert.rejects(verifyWindowsDirPackage(path, validInput), /Missing model-router directory/)
})

test('CLI assembles from all supported options and prints the verification result', async () => {
  const outDir = join(root, 'from CLI')
  const updateBaseUrl = 'https://feed.example.invalid/cli'
  const { desktopBin, runtimeBin } = binaries.arm64
  const ghosttyDll = join(root, 'explicit.dll')
  const explicitBytes = peFixture(0xaa64, 'explicit Ghostty', true)
  await writeFile(ghosttyDll, explicitBytes)
  const result = cli([
    '--arch', 'arm64', '--target', 'dir', '--desktop-bin', desktopBin, '--runtime-bin', runtimeBin,
    '--out-dir', outDir, '--version', version, '--update-base-url', updateBaseUrl, '--ghostty-dll', ghosttyDll
  ])
  assert.equal(result.status, 0, result.stderr)
  const assembled = JSON.parse(result.stdout)
  assert.equal(assembled.updateBaseUrl, updateBaseUrl)
  assert.equal(basename(assembled.path), 'win-arm64-unpacked')
  assert.deepEqual(await readFile(join(assembled.path, ghosttyName)), explicitBytes)
  await verifyWindowsDirPackage(assembled.path, options('arm64', outDir, { updateBaseUrl }))
})

test('CLI validates flags and rejects non-dir targets', async () => {
  const help = cli(['--help'])
  assert.equal(help.status, 0, help.stderr)
  assert.match(help.stdout, /Usage:/)
  const outDir = join(root, 'bad CLI')
  const { desktopBin, runtimeBin } = binaries.x64
  const base = [
    '--arch', 'x64', '--desktop-bin', desktopBin, '--runtime-bin', runtimeBin,
    '--out-dir', outDir, '--version', version
  ]
  for (const [args, error] of [
    [[], /Missing required option --arch/],
    [base.slice(0, 4), /Missing required option --runtime-bin/],
    [[...base, '--arch'], /--arch.*argument/],
    [[...base, '--arch', 'ia32'], /Arch must be x64 or arm64/],
    [[...base, '--target', 'nsis'], /Target must be dir/],
    [[...base, '--version', '../2.0'], /Version must be X\.Y\.Z/],
    [[...base, '--update-base-url', 'not-a-url'], /absolute HTTP\(S\) URL/],
    [[...base, '--sign'], /Unknown option/]
  ]) {
    const result = cli(args)
    assert.equal(result.status, 1, result.stdout || result.stderr)
    assert.match(result.stderr, error)
  }
  await assert.rejects(readdir(outDir), { code: 'ENOENT' })
})
