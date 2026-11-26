import { copyFile, lstat, mkdir, readdir, writeFile } from 'node:fs/promises'
import { basename, dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

import { checkPe } from '../native-parity/package-native-win.mjs'
import { fileDigests, runTool } from '../native-release-helpers.mjs'
import { windowsPeImports } from './pe-imports.mjs'

const setupScript = fileURLToPath(new URL('./matou.iss', import.meta.url))
const applicationIcon = fileURLToPath(new URL('../../apps/desktop/build/icon.ico', import.meta.url))
const signatureScript = fileURLToPath(new URL('./file-signatures.ps1', import.meta.url))
const signingScript = fileURLToPath(new URL('./sign-file.ps1', import.meta.url))
const msvcRuntime = /^(?:msvc[pr]|vcruntime|concrt|vcomp|vcamp|vccorlib)(?:\d{2,3})(?:_\w+)?d?\.dll$/i
const releaseRuntime = /^(?:(?:msvcp|vcruntime)140(?:_\w+)?|(?:concrt|vcomp|vcamp|vccorlib)140)\.dll$/i
const nativeImage = /\.(exe|dll|pyd)$/i
const invalidWindowsName = /[<>:"|?*\\\p{Cc}]|[. ]$|^(?:CON|PRN|AUX|NUL|COM[1-9¹²³]|LPT[1-9¹²³])(?:\.|$)/iu
const accountsRoot = ['resources', 'claude-accounts', 'matou-claude-accounts'].join('/')
const routerRoot = ['resources', 'model-router'].join('/')

export function windowsFileVersion(version) {
  const parts = version.split(/[+-]/)[0].split('.')
  if (parts.some(part => Number(part) > 65535)) throw new Error('Windows version numbers must be at most 65535')
  return [...parts, '0'].map(Number).join('.')
}

async function payloadFiles(directory, relativePath = '') {
  const entries = await readdir(directory, { withFileTypes: true })
  const files = []
  const names = new Set()
  for (const entry of entries) {
    const name = entry.name
    const path = join(directory, name)
    const relative = relativePath ? `${relativePath}/${name}` : name
    if (invalidWindowsName.test(name) || names.has(name.toLowerCase())) {
      throw new Error(`Invalid or duplicate Windows installer path: ${relative}`)
    }
    names.add(name.toLowerCase())
    if (entry.isDirectory()) files.push(...await payloadFiles(path, relative))
    else if (entry.isFile()) files.push({ path, relative })
    else throw new Error(`Windows installer payload must contain only ordinary files and directories: ${relative}`)
  }
  return files.sort((a, b) => a.relative.localeCompare(b.relative, 'en'))
}

async function payloadDigests(directory) {
  const digests = []
  for (const { path, relative } of await payloadFiles(directory)) {
    digests.push([relative, (await fileDigests(path)).sha256])
  }
  return digests
}

function needsRuntime(dll) {
  if (dll === 'ucrtbased.dll' || msvcRuntime.test(dll) && !releaseRuntime.test(dll)) {
    throw new Error(`Unsupported Visual C++ runtime import in the release payload: ${dll}`)
  }
  return releaseRuntime.test(dll)
}

function windowsPowerShell(env) {
  return env.SystemRoot
    ? join(env.SystemRoot, 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe')
    : 'powershell.exe'
}

async function inspectSignatures(paths, workDir, env, command) {
  const listFile = join(workDir, 'signatures.json')
  await writeFile(listFile, JSON.stringify(paths))
  const { stdout } = await command(windowsPowerShell(env), [
    '-NoLogo', '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass',
    '-File', signatureScript, '-ListFile', listFile
  ])
  const signatures = JSON.parse(stdout)
  if (!Array.isArray(signatures) || signatures.length !== paths.length) {
    throw new Error('Unexpected Authenticode verification output')
  }
  return signatures
}

async function stageMsvcRuntime(payload, source, workDir, arch, env, command) {
  const copies = new Map([
    ['vcruntime140.dll', 'vcruntime140.dll']
  ])
  function addRuntime(directory, name) {
    const relative = directory ? `${directory}/${name}` : name
    if (!copies.has(relative.toLowerCase())) copies.set(relative.toLowerCase(), relative)
  }
  for (const { path, relative } of await payloadFiles(payload)) {
    if (!nativeImage.test(relative)) continue
    await checkPe(path, relative, arch, !/\.exe$/i.test(relative))
    const name = basename(path).toLowerCase()
    // PyInstaller may carry its own copies. Replace those with the same
    // Microsoft-signed toolset used by the rest of the application.
    if (needsRuntime(name)) copies.set(relative.toLowerCase(), relative)
    const directory = relative.startsWith(`${accountsRoot}/`) ? accountsRoot
      : relative.startsWith(`${routerRoot}/`) ? routerRoot : ''
    for (const dll of await windowsPeImports(path)) {
      if (needsRuntime(dll)) addRuntime(directory, dll)
    }
  }
  const checked = new Set()
  // Walk the growing set to close imports of the CRT DLLs themselves.
  for (const relative of copies.values()) {
    const name = relative.split('/').at(-1).toLowerCase()
    const directory = relative.split('/').slice(0, -1).join('/')
    const key = `${directory.toLowerCase()}/${name}`
    if (checked.has(key)) continue
    checked.add(key)
    const runtimeSource = join(source, name)
    await checkPe(runtimeSource, `Visual C++ runtime ${name}`, arch, true)
    for (const dll of await windowsPeImports(runtimeSource)) {
      if (needsRuntime(dll)) addRuntime(directory, dll)
    }
  }
  const destinations = []
  for (const relative of copies.values()) {
    const destination = join(payload, ...relative.split('/'))
    await copyFile(join(source, basename(destination).toLowerCase()), destination)
    destinations.push(destination)
  }
  const signatures = await inspectSignatures(destinations, workDir, env, command)
  for (const [index, signature] of signatures.entries()) {
    if (signature.status !== 'Valid' ||
        !['Microsoft Corporation', 'Microsoft Windows Software Compatibility Publisher'].includes(signature.signerName) ||
        !/(?:^|,\s*)O=Microsoft Corporation(?:,|$)/i.test(signature.signerSubject ?? '')) {
      throw new Error(`Expected a valid Microsoft Authenticode signature for ${destinations[index]}`)
    }
  }
}

function innoLiteral(value) {
  if (/["\p{Cc}]/u.test(value)) throw new Error(`Invalid Inno Setup path: ${value}`)
  return value
}

async function writeInnoFiles(payload, fileList) {
  const lines = (await payloadFiles(payload)).map(({ path, relative }) => {
    const parts = relative.split('/').slice(0, -1).map(part => innoLiteral(part).replaceAll('{', '{{'))
    const destination = ['{app}', ...parts].join('\\')
    return `Source: "${innoLiteral(path)}"; DestDir: "${destination}"; Flags: ignoreversion`
  })
  await writeFile(fileList, '\ufeff' + lines.join('\r\n') + '\r\n')
}

function signingConfiguration(env, uninstallerDir) {
  const thumbprint = env.MATOU_WIN_SIGN_CERT_SHA1
  if (!/^[a-f0-9]{40}$/i.test(thumbprint ?? '')) {
    throw new Error('Set MATOU_WIN_SIGN_CERT_SHA1 to the code-signing certificate thumbprint in the current user certificate store')
  }
  const signTool = env.MATOU_WIN_SIGNTOOL || 'signtool.exe'
  const timestamp = env.MATOU_WIN_TIMESTAMP_URL || 'http://timestamp.digicert.com'
  if (!/^https?:\/\/[A-Za-z0-9.:-]+(?:\/[A-Za-z0-9_./-]*)?$/i.test(timestamp)) {
    throw new Error('Invalid Windows signing tool path or timestamp URL')
  }
  const quote = value => `$q${innoLiteral(value).replaceAll('$', '$$$$')}$q`
  const signingCommand = [
    quote(windowsPowerShell(env)), '-NoLogo', '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass',
    '-File', quote(signingScript), '-SignTool', quote(signTool), '-Thumbprint', thumbprint,
    '-Timestamp', quote(timestamp), '-UninstallerDir', quote(uninstallerDir), '-Target', '$f'
  ]
  return {
    thumbprint: thumbprint.toUpperCase(),
    option: `-smatou_release_signing=${signingCommand.join(' ')}`
  }
}

export async function buildWindowsInstaller({ payload, destination, workDir, version, internalInstaller,
  arch = 'x64', vcRuntimeDir, env = process.env }, command = runTool) {
  if (!['x64', 'arm64'].includes(arch)) throw new Error('Windows installer architecture must be x64 or arm64')
  const source = vcRuntimeDir ?? (env.VCToolsRedistDir && join(env.VCToolsRedistDir, arch, 'Microsoft.VC143.CRT'))
  if (!source) throw new Error(`Use a VS 2022 ${arch} Developer Command Prompt or set --vc-runtime-dir to its Microsoft.VC143.CRT directory`)
  const uninstallerDir = join(workDir, 'signed-uninstallers')
  const signing = internalInstaller ? null : signingConfiguration(env, uninstallerDir)
  const compiler = env.MATOU_WIN_ISCC || 'ISCC.exe'
  const fileVersion = windowsFileVersion(version)
  const fileList = join(workDir, 'payload.iss')
  const outputName = basename(destination, '.exe')
  const signingArgs = []
  if (signing) {
    await mkdir(uninstallerDir)
    signingArgs.push('-DMatouSigned', `-DMatouUninstallerDir=${uninstallerDir}`, signing.option)
  }

  await stageMsvcRuntime(payload, resolve(source), workDir, arch, env, command)
  await copyFile(applicationIcon, join(payload, 'resources', 'matou.ico'))
  const before = JSON.stringify(await payloadDigests(payload))
  await writeInnoFiles(payload, fileList)
  await command(compiler, [
    '-ni', `-o${dirname(destination)}`, `-f${outputName}`, `-DMatouVersion=${version}`,
    `-DMatouFileVersion=${fileVersion}`, `-DMatouPayloadFiles=${fileList}`,
    ...(arch === 'arm64' ? ['-DMatouArm64'] : []), ...signingArgs, setupScript
  ])
  // Inno Setup 7 builds an x64 loader/uninstaller for both payload architectures.
  await checkPe(destination, 'Matou installer', 'x64')
  if (JSON.stringify(await payloadDigests(payload)) !== before) {
    throw new Error('Windows installer payload changed during compilation')
  }
  const toInspect = [destination]
  if (signing) {
    const uninstaller = join(uninstallerDir, 'uninstaller.exe')
    await checkPe(uninstaller, 'Matou uninstaller', 'x64')
    toInspect.push(uninstaller)
  }
  const signatures = await inspectSignatures(toInspect, workDir, env, command)
  for (const [index, signature] of signatures.entries()) {
    if (signature.productVersion?.trim() !== version) {
      throw new Error(`Windows installer version mismatch for ${toInspect[index]}: expected ${version}`)
    }
    const productName = arch === 'arm64' ? 'Matou ARM64' : 'Matou x64'
    if (signature.productName?.trim() !== productName) {
      throw new Error(`Windows installer payload architecture mismatch for ${toInspect[index]}: expected ${productName}`)
    }
    if (signing && (signature.status !== 'Valid' || !signature.timestamped ||
        signature.thumbprint?.toUpperCase() !== signing.thumbprint)) {
      throw new Error(`Missing valid, timestamped Matou Authenticode signature for ${toInspect[index]}`)
    }
  }
  // The staging directory was empty of executables before ISCC ran.
  if (!(await lstat(destination)).isFile()) throw new Error('Windows installer output is missing')
}
