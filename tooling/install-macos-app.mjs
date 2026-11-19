import { execFile } from 'node:child_process'
import { lstat, mkdir, mkdtemp, realpath, rename, rm, rmdir } from 'node:fs/promises'
import { homedir } from 'node:os'
import { basename, dirname, isAbsolute, join, resolve, sep } from 'node:path'
import { fileURLToPath } from 'node:url'
import { promisify } from 'node:util'

const appName = '码头.app'
const appId = 'com.matou.desktop'
const teamId = 'JAKX72KQ6X'
const authority = `Developer ID Application: Lu'an Yidayi Electronic Commerce Co.,Ltd. (${teamId})`
const target = join('/Applications', appName)
const defaultSource = fileURLToPath(new URL(`../apps/desktop/release/mac-arm64/${appName}`, import.meta.url))
const expectedRequirement = [
  `identifier "${appId}"`,
  'anchor apple generic',
  'certificate 1[field.1.2.840.113635.100.6.2.6] exists',
  'certificate leaf[field.1.2.840.113635.100.6.1.13] exists',
  `certificate leaf[subject.OU] = "${teamId}"`
]
const exec = promisify(execFile)

async function runCommand(command, args) {
  try {
    const { stdout, stderr } = await exec(command, args, {
      env: { ...process.env, LC_ALL: 'en_US.UTF-8' },
      maxBuffer: 16 * 1024 * 1024
    })
    return `${stdout}\n${stderr}`
  } catch (error) {
    throw new Error(error.stderr?.trim() || error.message, { cause: error })
  }
}

function checkIntegrity(path, run) {
  return run('/usr/bin/codesign', ['--verify', '--deep', '--strict', path])
}

function requirementClauses(requirement) {
  return requirement
    .replaceAll(/\/\*\s*exists\s*\*\//g, 'exists')
    .replaceAll(/"([\w.]+)"/g, '$1')
    .trim()
    .split(/\s+and\s+/)
}

async function readSignature(path, run, allowExistingAdHoc = false) {
  await checkIntegrity(path, run)
  const details = (await run('/usr/bin/codesign', ['-d', '--verbose=4', path])).split(/\r?\n/)
  const requirementOutput = await run('/usr/bin/codesign', ['-d', '-r-', path])
  const requirement = requirementOutput.match(/^designated => (.+)$/m)?.[1].trim()
  const isAdHoc = details.includes('Signature=adhoc')
  const hasAppId = details.includes(`Identifier=${appId}`)

  if (allowExistingAdHoc && hasAppId && isAdHoc && details.includes('TeamIdentifier=not set')) {
    return undefined
  }

  if (!hasAppId || isAdHoc || !details.includes(`TeamIdentifier=${teamId}`) ||
      !details.includes(`Authority=${authority}`)) {
    throw new Error(`${path} 必须由指定的 Matou Developer ID 签名（${appId}，团队 ${teamId}）。`)
  }

  const clauses = requirement ? requirementClauses(requirement) : []
  const expected = requirementClauses(expectedRequirement.join(' and '))
  if (clauses.length !== expected.length || !expected.every(clause => clauses.includes(clause))) {
    throw new Error(`${path} 的指定代码要求不符合 Matou 的固定签名身份。`)
  }

  // Evaluate the certificate chain and team with codesign instead of trusting display fields alone.
  await checkRequirement(path, expectedRequirement.join(' and '), run)
  return requirement
}

export function verifyInstallSource(path, run = runCommand) {
  return readSignature(path, run)
}

function checkRequirement(path, requirement, run) {
  return run('/usr/bin/codesign', ['--verify', '-R', `=${requirement}`, path])
}

async function requireAppClosed(path, run) {
  const escapedPath = path.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
  const reference = new RegExp(`(?:^|[\\s="'])${escapedPath}(?=/|[\\s"']|$)`)
  const listing = await run('/bin/ps', ['-ww', '-axo', 'pid=,comm=,args='])
  const pids = []
  for (const line of listing.split('\n')) {
    const entry = line.match(/^\s*(\d+)\s+(.+)$/)
    if (entry && Number(entry[1]) !== process.pid && reference.test(entry[2])) {
      pids.push(entry[1])
    }
  }
  if (pids.length > 0) {
    throw new Error(`请先正常退出码头并等其后台进程结束，再重新安装。仍在使用 ${path} 的进程 PID ${pids.join('、')}。`)
  }
}

async function bundleStat(path) {
  const stat = await lstat(path).catch(error => {
    if (error.code !== 'ENOENT') throw error
    return undefined
  })
  if (stat && !stat.isDirectory()) {
    throw new Error(`${path} 必须是实际的应用目录。`)
  }
  return stat
}

function sameBundle(left, right) {
  return left?.dev === right?.dev && left?.ino === right?.ino
}

async function archiveBundle(path, backups, run) {
  await mkdir(backups, { recursive: true, mode: 0o700 })
  const timestamp = new Date().toISOString().replace(/[:.]/g, '-')
  const directory = await mkdtemp(join(backups, `local-install-${timestamp}-`))
  const archive = join(directory, 'previous.zip')
  try {
    await run('/usr/bin/ditto', ['-c', '-k', '--sequesterRsrc', '--keepParent', path, archive])
    await run('/usr/bin/unzip', ['-tqq', archive])
    return archive
  } catch (error) {
    await rm(directory, { recursive: true, force: true })
    throw error
  }
}

async function restoreBundle(path, original, archive, staging, run) {
  const restoreDirectory = join(staging, 'restore')
  const restored = join(restoreDirectory, basename(path))
  await mkdir(restoreDirectory, { mode: 0o700 })
  await run('/usr/bin/ditto', ['-x', '-k', archive, restoreDirectory])
  if (!await bundleStat(restored)) throw new Error('备份中缺少原应用目录。')
  await checkIntegrity(restored, run)

  const current = await bundleStat(path)
  if (current) {
    if (!sameBundle(current, original)) throw new Error(`${path} 已被其他进程替换。`)
    await requireAppClosed(path, run)
    await rm(path, { recursive: true })
  }
  await rename(restored, path)
}

export async function installMacosApp({ source, destination, backups, run = runCommand }) {
  if (!isAbsolute(source) || !source.endsWith('.app') || !await bundleStat(source)) {
    throw new Error('请通过 --source 指定存在的 .app 绝对路径。')
  }
  const parent = await realpath(dirname(destination))
  const canonicalTarget = join(parent, basename(destination))
  const canonicalSource = await realpath(source)
  if (canonicalSource === canonicalTarget || canonicalSource.startsWith(`${canonicalTarget}${sep}`)) {
    throw new Error('安装来源必须位于现有应用目录之外。')
  }

  const lock = join(parent, '.matou-install.lock')
  try {
    await mkdir(lock, { mode: 0o700 })
  } catch (error) {
    if (error.code !== 'EEXIST') throw error
    throw new Error(`安装锁已存在（${lock}）。如果上次安装被强制中断，请先确认没有安装进程，再移除此空目录。`, { cause: error })
  }

  let staging
  let archive
  let originalChanged = false
  let original
  try {
    await requireAppClosed(canonicalTarget, run)
    await verifyInstallSource(canonicalSource, run)
    original = await bundleStat(canonicalTarget)
    const originalRequirement = original && await readSignature(canonicalTarget, run, true)
    staging = await mkdtemp(join(parent, '.matou-install-'))
    const stagedApp = join(staging, appName)
    await run('/usr/bin/ditto', [canonicalSource, stagedApp])
    await verifyInstallSource(stagedApp, run)
    if (originalRequirement) await checkRequirement(stagedApp, originalRequirement, run)
    if (original) archive = await archiveBundle(canonicalTarget, backups, run)

    if (!sameBundle(original, await bundleStat(canonicalTarget))) {
      throw new Error(`${canonicalTarget} 在安装期间已变化。`)
    }
    await requireAppClosed(canonicalTarget, run)
    if (original) {
      originalChanged = true
      await rm(canonicalTarget, { recursive: true })
    }
    await rename(stagedApp, canonicalTarget)
    return { archive }
  } catch (error) {
    if (originalChanged) {
      try {
        await restoreBundle(canonicalTarget, original, archive, staging, run)
      } catch (restoreError) {
        throw new Error(`${error.message}\n自动恢复失败：${restoreError.message}\n原应用备份：${archive}`, { cause: restoreError })
      }
      throw new Error(`${error.message}\n原应用已恢复。备份：${archive}`, { cause: error })
    }
    if (archive) throw new Error(`${error.message}\n原应用备份：${archive}`, { cause: error })
    throw error
  } finally {
    try {
      if (staging) await rm(staging, { recursive: true, force: true })
    } finally {
      await rmdir(lock)
    }
  }
}

async function main(args) {
  if (args.length === 1 && args[0] === '--help') {
    console.log('用法：pnpm install:mac [--source /absolute/path/to/码头.app]')
    return
  }
  if (args.length !== 0 && (args.length !== 2 || args[0] !== '--source' || !isAbsolute(args[1]))) {
    throw new Error('用法：pnpm install:mac [--source /absolute/path/to/码头.app]')
  }
  if (process.platform !== 'darwin') throw new Error('此安装命令仅支持 macOS。')
  const { archive } = await installMacosApp({
    source: args[1] ?? defaultSource,
    destination: target,
    backups: join(homedir(), '.matou', 'backups')
  })
  console.log(`已安装 ${target}。`)
  if (archive) console.log(`原应用备份：${archive}`)
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main(process.argv.slice(2)).catch(error => {
    console.error(error.message)
    process.exitCode = 1
  })
}
