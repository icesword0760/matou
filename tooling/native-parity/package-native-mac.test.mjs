import assert from 'node:assert/strict'
import { execFileSync, spawnSync } from 'node:child_process'
import {
  chmod, lstat, mkdir, mkdtemp, readFile, readdir, realpath, rm, symlink, writeFile
} from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { after, before, test } from 'node:test'

const script = fileURLToPath(new URL('./package-native-mac.mjs', import.meta.url))
const appName = '码头.app'
const runtimePath = 'Contents/MacOS/matou-native-runtime'
const infoPath = 'Contents/Info.plist'
const iconPath = 'Contents/Resources/icon.icns'
const entitlementsPath = 'Contents/Resources/entitlements.mac.plist'
const workflowPath = 'Contents/Resources/finder-quick-action/进入码头.workflow/Contents/Resources/document.wflow'
const pluginPath = 'Contents/Resources/control-assets/providers/claude-plugin'
const skip = process.platform !== 'darwin' && 'macOS bundle verification requires macOS'

let root
let thin
let universal
before(async () => {
  if (skip) return
  root = await mkdtemp(join(tmpdir(), 'matou-native-mac-test-'))
  const source = join(root, 'main.c')
  await writeFile(source, 'int main(void) { return 0; }\n')
  const compile = arch => {
    const path = join(root, arch)
    execFileSync('/usr/bin/clang', [
      '-target', `${arch}-apple-macos12`, '-Wl,-no_adhoc_codesign', source, '-o', path
    ], { env: { ...process.env, TMPDIR: root } })
    return path
  }
  thin = compile('x86_64')
  const arm64 = compile('arm64')
  universal = join(root, 'universal')
  execFileSync('/usr/bin/lipo', ['-create', thin, arm64, '-output', universal])
})
after(async () => {
  if (root) await rm(root, { recursive: true, force: true })
})

function cli(args) {
  const result = spawnSync(process.execPath, [script, ...args], { encoding: 'utf8' })
  assert.ifError(result.error)
  return result
}

function success(result) {
  assert.equal(result.status, 0, result.stderr)
  return JSON.parse(result.stdout)
}

function failure(result, message) {
  assert.equal(result.status, 1, result.stdout || result.stderr)
  assert.match(result.stderr, message)
}

function assemble(out, { desktop = thin, runtime = thin, extra = [] } = {}) {
  return cli([
    '--desktop-bin', desktop, '--runtime-bin', runtime,
    '--out-dir', out, '--version', '0.2.24', '--unsigned', ...extra
  ])
}

test('assembles and verifies an app with frozen metadata and universal Mach-O payloads', { skip }, async () => {
  const out = join(root, 'complete output')
  const app = join(out, appName)
  const result = success(assemble(out, { desktop: universal, runtime: universal }))
  assert.equal(result.bundleId, 'com.matou.desktop')
  assert.equal(result.version, '0.2.24')
  assert.deepEqual(await readdir(out), [appName])
  const sourcePlugin = fileURLToPath(new URL('../../apps/runtime/control-assets/providers/claude-plugin/', import.meta.url))
  for (const path of ['.claude-plugin/plugin.json', 'hooks/hooks.json', 'skills/mt-terminal/SKILL.md',
    'skills/agent-foreman/SKILL.md', 'skills/agent-foreman/scripts/foreman.mjs']) {
    assert.deepEqual(await readFile(join(app, pluginPath, path)), await readFile(join(sourcePlugin, path)))
  }
  for (const name of ['matou-native-desktop', 'matou-native-runtime']) {
    assert.equal((await lstat(join(app, 'Contents', 'MacOS', name))).mode & 0o7777, 0o755)
  }

  const verified = success(cli(['--verify', app]))
  assert.deepEqual(verified.info.CFBundleDocumentTypes, [{
    CFBundleTypeName: 'Folder',
    CFBundleTypeRole: 'Viewer',
    LSHandlerRank: 'Alternate',
    LSItemContentTypes: ['public.folder']
  }])
  for (const [key, value] of Object.entries({
    CFBundleIdentifier: 'com.matou.desktop',
    CFBundleExecutable: 'matou-native-desktop',
    CFBundleName: '码头',
    CFBundleVersion: '0.2.24',
    CFBundleShortVersionString: '0.2.24',
    LSMinimumSystemVersion: '12.0',
    NSHighResolutionCapable: true
  })) assert.equal(verified.info[key], value, key)
  assert.deepEqual(verified.architectures.desktop.toSorted(), ['arm64', 'x86_64'])
  assert.deepEqual(verified.architectures.runtime.toSorted(), ['arm64', 'x86_64'])
  assert.equal(verified.signatures.app.status, 'unsigned')
  assert.equal(verified.signatures.runtime.status, 'unsigned')
})

test('copies custom entitlements and audits the requested bundle ID', { skip }, async () => {
  const out = join(root, 'custom')
  const entitlements = join(root, 'custom.plist')
  const override = fileURLToPath(new URL('../../apps/desktop/build/entitlements.mac.plist', import.meta.url))
  const plist = (await readFile(override, 'utf8')).replaceAll('<true/>', '<false/>')
  await writeFile(entitlements, plist)
  const app = join(out, appName)
  const bundleId = 'com.matou.native-parity'
  success(assemble(out, { extra: ['--bundle-id', bundleId, '--entitlements', entitlements] }))
  assert.equal(await readFile(join(app, entitlementsPath), 'utf8'), plist)
  failure(cli(['--verify', app]), /CFBundleIdentifier/)
  assert.equal(success(cli(['--verify', app, '--bundle-id', bundleId])).info.CFBundleIdentifier, bundleId)
})

test('assembles and verifies the separately named beta app and Finder workflow', { skip }, async () => {
  const out = join(root, 'beta')
  const name = '码头测试版'
  const bundleId = 'com.matou.desktop.beta'
  const app = join(out, `${name}.app`)
  const identity = ['--app-name', name, '--bundle-id', bundleId]
  const result = success(assemble(out, { extra: identity }))
  assert.equal(result.path, await realpath(app))
  assert.equal(result.bundleId, bundleId)
  assert.deepEqual(await readdir(out), [`${name}.app`])
  assert.deepEqual((await readdir(join(app, 'Contents/MacOS'))).sort(), ['matou-native-runtime', name])
  assert.equal((await lstat(join(app, 'Contents/MacOS', name))).mode & 0o7777, 0o755)

  const verified = success(cli(['--verify', app, ...identity]))
  assert.equal(verified.info.CFBundleIdentifier, bundleId)
  for (const field of ['CFBundleName', 'CFBundleDisplayName', 'CFBundleExecutable']) {
    assert.equal(verified.info[field], name, field)
  }
  assert.deepEqual(success(cli(['--verify', app, '--bundle-id', bundleId])).info, verified.info)
  failure(cli(['--verify', app, '--app-name', '码头', '--bundle-id', bundleId]), /CFBundleExecutable|CFBundleName/)
  failure(cli(['--verify', app, '--app-name', name]), /CFBundleIdentifier/)
  failure(cli(['--verify', app, ...identity, '--require-release-signature']), /Developer ID signature .*unsigned/)

  const workflow = join(app, 'Contents/Resources/finder-quick-action', `进入${name}.workflow`)
  const plist = JSON.parse(execFileSync('/usr/bin/plutil', [
    '-convert', 'json', '-o', '-', '--', join(workflow, infoPath)
  ], { encoding: 'utf8' }))
  assert.equal(plist.CFBundleIdentifier, `${bundleId}.finder-open-workspace`)
  assert.equal(plist.CFBundleName, `进入${name}`)
  assert.equal(plist.NSServices[0].NSMenuItem.default, `进入${name}`)
  const document = join(workflow, 'Contents/Resources/document.wflow')
  const original = await readFile(document, 'utf8')
  assert.ok(original.includes(`/usr/bin/open -b ${bundleId} "$target"`))
  await writeFile(document, original.replace(bundleId, 'com.matou.desktop'))
  failure(cli(['--verify', app, ...identity]), /Invalid Finder workflow name or bundle ID/)
})

test('rejects missing input files and removes bundles that fail staged validation', { skip }, async () => {
  const out = join(root, 'failed')
  const missing = join(root, 'absent')
  const nonExecutable = join(root, 'non-executable')
  const notMachO = join(root, 'not-mach-o')
  const badPlist = join(root, 'bad.plist')
  await mkdir(out)
  await writeFile(nonExecutable, await readFile(thin), { mode: 0o644 })
  await writeFile(notMachO, '#!/bin/sh\nexit 0\n', { mode: 0o755 })
  await writeFile(badPlist, 'not a plist')

  for (const [options, error] of [
    [{ extra: ['--app-name', '../escape'] }, /Invalid --app-name/],
    [{ desktop: missing }, /Missing desktop binary/],
    [{ runtime: missing }, /Missing runtime binary/],
    [{ extra: ['--entitlements', missing] }, /Missing entitlements resource/],
    [{ runtime: nonExecutable }, /runtime binary must be executable/],
    [{ desktop: notMachO }, /Mach-O executable/],
    [{ extra: ['--entitlements', badPlist] }, /Invalid plist dictionary/]
  ]) {
    failure(assemble(out, options), error)
    assert.deepEqual(await readdir(out), [])
  }
})

test('rejects damaged bundle metadata, required resources and executable permissions', { skip }, async () => {
  const out = join(root, 'damaged')
  const app = join(out, appName)
  success(assemble(out))
  const info = await readFile(join(app, infoPath), 'utf8')
  await writeFile(join(app, infoPath), info.replace('public.folder', 'public.text'))
  failure(cli(['--verify', app]), /CFBundleDocumentTypes/)
  await writeFile(join(app, infoPath), info)
  await chmod(join(app, runtimePath), 0o644)
  failure(cli(['--verify', app]), /runtime binary must have mode 0755/)
  await chmod(join(app, runtimePath), 0o755)
  for (const [path, error] of [
    [iconPath, /Missing icon resource/],
    [entitlementsPath, /Missing entitlements resource/],
    [workflowPath, /Missing Finder workflow resource/],
    [runtimePath, /Missing runtime binary/]
  ]) {
    const target = join(app, path)
    const bytes = await readFile(target)
    const { mode } = await lstat(target)
    await rm(target)
    failure(cli(['--verify', app]), error)
    await writeFile(target, bytes, { mode })
  }
})

test('requires Developer ID for release verification and reports unsigned and ad-hoc signatures', { skip }, async () => {
  const out = join(root, 'signatures')
  const app = join(out, appName)
  success(assemble(out))
  const strict = ['--verify', app, '--require-release-signature']
  failure(cli(strict), /app requires the Matou Developer ID signature \(Team JAKX72KQ6X\); found unsigned/)

  // All signing in this test explicitly uses the ad-hoc identity on disposable files.
  for (const target of [join(app, runtimePath), app]) {
    execFileSync('/usr/bin/codesign', ['--force', '--sign', '-', target])
  }
  const { signatures } = success(cli(['--verify', app]))
  assert.equal(signatures.app.status, 'ad-hoc')
  assert.equal(signatures.runtime.status, 'ad-hoc')
  assert.equal(signatures.app.identifier, 'com.matou.desktop')
  assert.match(signatures.app.designatedRequirement, /^cdhash /)
  failure(cli(strict), /app requires the Matou Developer ID signature \(Team JAKX72KQ6X\); found ad-hoc/)
})

test('rejects /Applications and its aliases before creating output', { skip }, async () => {
  const alias = join(root, 'application-directory-alias')
  await symlink('/Applications', alias)
  for (const path of [
    '/Applications',
    '/Applications/码头.app',
    '/System/Volumes/Data/Applications',
    join(alias, 'must-not-create')
  ]) failure(assemble(path), /--out-dir cannot write to \/Applications/)
})

test('refuses to overwrite an existing bundle', { skip }, async () => {
  const out = join(root, 'existing')
  success(assemble(out))
  const info = join(out, appName, infoPath)
  const original = await readFile(info, 'utf8')
  failure(assemble(out), /Output bundle already exists/)
  assert.equal(await readFile(info, 'utf8'), original)
  assert.deepEqual(await readdir(out), [appName])
})
