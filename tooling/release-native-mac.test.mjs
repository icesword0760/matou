import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { test } from 'node:test'
import { fileURLToPath } from 'node:url'

import { desktopConfig } from './native-release-helpers.mjs'
import { generateMacManifest, macReleaseOptions } from './release-native-mac.mjs'

const config = await desktopConfig()
const date = '2026-11-24T11:00:00.000Z'
const script = fileURLToPath(new URL('./release-native-mac.mjs', import.meta.url))

test('uses the desktop release version, production feed and secure timestamp by default', () => {
  const options = macReleaseOptions([], {}, config)
  assert.equal(options.version, config.version)
  assert.equal(options.arch, 'arm64')
  assert.equal(options.updateBaseUrl, 'https://updates.baize-ailabs.com/stable')
  assert.equal(options.channel, 'stable')
  assert.equal(options.identity, config.build.mac.identity)
  assert.equal(options.timestamp, true)
  assert.match(options.outDir, /apps[/\\]native[/\\]release$/)
  assert.match(options.accountsDirs[0], /package-resources[/\\]claude-accounts[/\\]matou-claude-accounts$/)
  assert.match(options.accountsDirs[1], /claude-accounts[/\\]dist[/\\]matou-claude-accounts$/)
  assert.match(options.modelRouterDir, /package-resources[/\\]model-router$/)
})

test('supports environment settings, CLI overrides and explicit offline signing', () => {
  const env = {
    MATOU_MAC_ARCH: 'x64',
    MATOU_UPDATE_BASE_URL: 'https://updates.example.test/beta',
    MATOU_UPDATE_CHANNEL: 'beta',
    MATOU_CLAUDE_ACCOUNTS_ENGINE: '/engines/accounts',
    MATOU_MODEL_ROUTER_DIR: '/engines/router'
  }
  const defaults = macReleaseOptions([], env, config)
  assert.equal(defaults.arch, 'x64')
  assert.equal(defaults.updateBaseUrl, env.MATOU_UPDATE_BASE_URL)
  assert.equal(defaults.channel, 'beta')
  assert.equal(defaults.accountsDirs.length, 1)
  assert.match(defaults.accountsDirs[0], /engines[/\\]accounts$/)
  assert.match(defaults.modelRouterDir, /engines[/\\]router$/)

  const options = macReleaseOptions([
    '--arch', 'arm64', '--version', '1.2.3', '--update-base-url', 'http://localhost:3456/feed',
    '--update-channel', 'preview', '--claude-accounts-engine', './accounts', '--model-router', './router',
    '--desktop-bin', 'desktop', '--runtime-bin', 'runtime', '--out-dir', 'artifacts', '--no-timestamp'
  ], env, config)
  assert.equal(options.arch, 'arm64')
  assert.equal(options.version, '1.2.3')
  assert.equal(options.updateBaseUrl, 'http://localhost:3456/feed')
  assert.equal(options.channel, 'preview')
  assert.match(options.accountsDirs[0], /[/\\]accounts$/)
  assert.match(options.modelRouterDir, /[/\\]router$/)
  assert.equal(options.desktopBin, 'desktop')
  assert.equal(options.runtimeBin, 'runtime')
  assert.equal(options.outDir, 'artifacts')
  assert.equal(options.timestamp, false)
  assert.throws(() => macReleaseOptions(['--arch', 'toString'], env, config), /--arch/)
  assert.throws(() => macReleaseOptions(['--update-channel', '../stable'], env, config), /Invalid update channel/)
})

test('refuses an old explicit accounts engine before starting the release build', { skip: process.platform !== 'darwin' }, async t => {
  const root = await mkdtemp(join(tmpdir(), 'matou-mac-release-account-test-'))
  t.after(() => rm(root, { recursive: true, force: true }))
  await writeFile(join(root, 'matou-claude-accounts'), 'old executable without a source manifest')
  const result = spawnSync(process.execPath, [script, '--claude-accounts-engine', root,
    '--desktop-bin', join(root, 'must-not-build'), '--out-dir', join(root, 'release')], { encoding: 'utf8' })
  assert.ifError(result.error)
  assert.equal(result.status, 1, result.stdout)
  assert.match(result.stderr, /Invalid account engine source manifest.*pnpm accounts:build/)
})

test('formats the updater manifest with ZIP first and the legacy ZIP path and checksum', async t => {
  const root = await mkdtemp(join(tmpdir(), 'matou-mac-release-test-'))
  t.after(() => rm(root, { recursive: true, force: true }))
  const zip = join(root, 'Matou-0.3.0-mac-arm64.zip')
  const dmg = join(root, 'Matou-0.3.0-mac-arm64.dmg')
  const zipBytes = Buffer.from([0x50, 0x4b, 0x00, 0x01])
  const dmgBytes = Buffer.from([0x64, 0x6d, 0x67])
  await Promise.all([writeFile(zip, zipBytes), writeFile(dmg, dmgBytes)])
  const zipHash = createHash('sha512').update(zipBytes).digest('base64')
  const dmgHash = createHash('sha512').update(dmgBytes).digest('base64')
  assert.equal(await generateMacManifest({ version: '0.3.0', zip, dmg, releaseDate: date }), `version: 0.3.0
files:
  - url: Matou-0.3.0-mac-arm64.zip
    sha512: ${zipHash}
    size: 4
  - url: Matou-0.3.0-mac-arm64.dmg
    sha512: ${dmgHash}
    size: 3
path: Matou-0.3.0-mac-arm64.zip
sha512: ${zipHash}
releaseDate: '${date}'
`)
  assert.throws(() => generateMacManifest({ version: '0.3.0', zip: dmg, dmg: zip }), /requires a ZIP and DMG/)
})
