import assert from 'node:assert/strict'
import { cp, lstat, mkdir, mkdtemp, readFile, readdir, realpath, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { basename, dirname, join } from 'node:path'
import test from 'node:test'

import { installMacosApp, verifyInstallSource } from './install-macos-app.mjs'

const signature = [
  'Identifier=com.matou.desktop',
  "Authority=Developer ID Application: Lu'an Yidayi Electronic Commerce Co.,Ltd. (JAKX72KQ6X)",
  'TeamIdentifier=JAKX72KQ6X'
].join('\n')

const designatedRequirement = [
  'identifier "com.matou.desktop"',
  'anchor apple generic',
  'certificate 1[field.1.2.840.113635.100.6.2.6] /* exists */',
  'certificate leaf[field.1.2.840.113635.100.6.1.13] /* exists */',
  'certificate leaf[subject.OU] = JAKX72KQ6X'
].join(' and ')

const adHocSignature = 'Identifier=com.matou.desktop\nSignature=adhoc\nTeamIdentifier=not set'

function codeSignDetails(args, details, requirement) {
  if (args.includes('--verbose=4')) return details
  if (args.includes('-r-')) return `designated => ${requirement}`
  return ''
}

test('requires the issued Matou Developer ID and its stable designated requirement', async () => {
  const source = '/build/码头.app'
  const calls = []
  const requirement = await verifyInstallSource(source, async (command, args) => {
    assert.equal(command, '/usr/bin/codesign')
    calls.push(args)
    return codeSignDetails(args, signature, designatedRequirement)
  })
  assert.equal(requirement, designatedRequirement)
  assert.deepEqual(calls[0], ['--verify', '--deep', '--strict', source])
  assert.equal(calls.at(-1)[1], '-R')
  assert.match(calls.at(-1)[2], /^=identifier .*anchor apple generic .*subject\.OU/)

  for (const [details, requirement, message] of [
    [adHocSignature, 'cdhash H"0000"', /Developer ID/],
    [signature.replace('com.matou.desktop', 'com.unknown.desktop'), designatedRequirement, /Developer ID/],
    [signature.replace('TeamIdentifier=JAKX72KQ6X', 'TeamIdentifier=anotherTeam'), designatedRequirement, /Developer ID/],
    [signature.replace('Developer ID Application', 'Apple Development'), designatedRequirement, /Developer ID/],
    [signature, 'identifier "com.matou.desktop"', /指定代码要求/],
    [signature, `${designatedRequirement} and cdhash H"0000"`, /指定代码要求/]
  ]) {
    await assert.rejects(verifyInstallSource(source, async (_, args) => codeSignDetails(args, details, requirement)), message)
  }
  await assert.rejects(verifyInstallSource(source, async (_, args) => {
    if (args.includes('-R')) throw new Error('certificate chain rejected')
    return codeSignDetails(args, signature, designatedRequirement)
  }), /certificate chain rejected/)
})

async function fixture(t, { currentSignature = signature, processScan = () => '' } = {}) {
  const root = await mkdtemp(join(await realpath(tmpdir()), 'matou-installer-test-'))
  t.after(() => rm(root, { recursive: true, force: true }))
  const applications = join(root, 'Applications')
  const destination = join(applications, '码头.app')
  const source = join(root, 'signed.app')
  const backups = join(root, 'backups')
  const contents = app => readFile(join(app, 'contents'), 'utf8')
  const archives = new Map()
  const requirements = []
  let stage
  let scans = 0

  async function createApp(app, value) {
    await mkdir(app, { recursive: true })
    await writeFile(join(app, 'contents'), value)
  }

  await createApp(source, 'new')
  await createApp(destination, 'original')

  async function run(command, args) {
    switch (command) {
      case '/bin/ps':
        return processScan({ destination, stage, scan: ++scans })
      case '/usr/bin/codesign': {
        const value = await contents(args.at(-1))
        const details = value === 'original' ? currentSignature : signature
        if (args.includes('-R')) requirements.push(args)
        return codeSignDetails(args, details, designatedRequirement)
      }
      case '/usr/bin/ditto': {
        const archive = args.at(-1)
        if (args[0] === '-c') {
          assert.equal(basename(archive), 'previous.zip')
          assert.equal(args.at(-2), destination)
          assert.equal((await lstat(dirname(archive))).mode & 0o777, 0o700)
          archives.set(archive, await contents(destination))
          await writeFile(archive, 'archive fixture')
        } else if (args[0] === '-x') {
          await createApp(join(archive, basename(destination)), archives.get(args.at(-2)))
        } else {
          stage = args[1]
          await cp(args[0], stage, { recursive: true })
        }
        return ''
      }
      case '/usr/bin/unzip':
        assert.equal(await readFile(args.at(-1), 'utf8'), 'archive fixture')
        return ''
      default:
        assert.fail(`Unexpected command ${command}`)
    }
  }

  return {
    install: () => installMacosApp({ source, destination, backups, run }),
    contents: () => contents(destination),
    applications, source, destination, archives, requirements
  }
}

test('blocks replacement when a process starts after staging and ignores lookalike paths', async t => {
  const app = await fixture(t, {
    processScan: ({ scan, destination }) => scan === 1
      ? `123 /bin/node node /prefix${destination}/Contents/run.js\n125 /bin/node node ${destination}.old/Contents/run.js`
      : `456 /bin/node node --app-path=${destination}/Contents/app.asar`
  })
  await assert.rejects(app.install(), /PID 456/)
  assert.equal(await app.contents(), 'original')
  assert.deepEqual(await readdir(app.applications), [basename(app.destination)])
  assert.equal(app.archives.size, 1)
})

test('upgrades the existing ad-hoc installation and leaves a zip backup', async t => {
  const app = await fixture(t, { currentSignature: adHocSignature })
  const { archive } = await app.install()
  assert.equal(await app.contents(), 'new')
  assert.equal(app.archives.get(archive), 'original')
  assert.deepEqual(await readdir(app.applications), [basename(app.destination)])
})

test('restores the original path and keeps the backup if replacement fails after removal', async t => {
  const app = await fixture(t, {
    processScan: async ({ scan, stage }) => {
      if (scan === 2) await rm(stage, { recursive: true })
      return ''
    }
  })
  await assert.rejects(app.install(), /原应用已恢复/)
  assert.equal(await app.contents(), 'original')
  assert.deepEqual(await readdir(app.applications), [basename(app.destination)])
  assert.equal(app.archives.size, 1)
  assert.ok(app.requirements.some(args => args[2] === `=${designatedRequirement}` && args.at(-1) !== app.destination))
})
