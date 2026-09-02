import { spawn, spawnSync } from 'node:child_process'
import { chmodSync, cpSync, existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

import { getLocalizedElectronBundlePaths } from './desktop-dev-bundle-path.mjs'

const require = createRequire(import.meta.url)
const projectRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const desktopRoot = join(projectRoot, 'apps', 'desktop')
const desktopPackage = JSON.parse(readFileSync(join(desktopRoot, 'package.json'), 'utf8'))

function setPlistValue(plistPath, key, value) {
  const result = spawnSync('/usr/bin/plutil', ['-replace', key, '-string', value, plistPath], {
    encoding: 'utf8'
  })
  if (result.status === 0) return
  const inserted = spawnSync('/usr/bin/plutil', ['-insert', key, '-string', value, plistPath], {
    encoding: 'utf8'
  })
  if (inserted.status !== 0) throw new Error(inserted.stderr || result.stderr)
}

function prepareLocalizedElectronBundle() {
  const electronExecutable = require('electron')
  const electronVersion = require('electron/package.json').version
  const sourceBundle = resolve(dirname(electronExecutable), '..', '..')
  const paths = getLocalizedElectronBundlePaths({
    projectRoot,
    electronVersion,
    productName: desktopPackage.productName
  })
  const { cacheRoot, bundle: cachedBundle } = paths

  if (!existsSync(cachedBundle)) {
    mkdirSync(cacheRoot, { recursive: true })
    cpSync(sourceBundle, cachedBundle, { recursive: true, verbatimSymlinks: true })
    const plistPath = join(cachedBundle, 'Contents', 'Info.plist')
    setPlistValue(plistPath, 'CFBundleName', desktopPackage.productName)
    setPlistValue(plistPath, 'CFBundleDisplayName', desktopPackage.productName)
    setPlistValue(plistPath, 'CFBundleIdentifier', `${desktopPackage.build.appId}.dev`)
    setPlistValue(plistPath, 'CFBundleExecutable', desktopPackage.productName)
    const macosDirectory = join(cachedBundle, 'Contents', 'MacOS')
    renameSync(join(macosDirectory, 'Electron'), join(macosDirectory, desktopPackage.productName))
    const launcherPath = join(macosDirectory, 'Electron')
    writeFileSync(launcherPath,
      `#!/bin/sh\nexec "$(dirname "$0")/${desktopPackage.productName}" "$@"\n`, { mode: 0o755 })
    chmodSync(launcherPath, 0o755)
    const signed = spawnSync('/usr/bin/codesign', ['--force', '--deep', '--sign', '-', cachedBundle], {
      encoding: 'utf8'
    })
    if (signed.status !== 0) throw new Error(signed.stderr)
  }

  return paths
}

const env = { ...process.env }
if (process.platform === 'darwin') {
  const electronBundle = prepareLocalizedElectronBundle()
  env.MATOU_DEV_BUNDLE = '1'
  env.ELECTRON_OVERRIDE_DIST_PATH = electronBundle.cacheRoot
  env.ELECTRON_EXEC_PATH = electronBundle.launcher
}

if (process.argv.includes('--prepare-only')) {
  process.stdout.write(`${env.ELECTRON_OVERRIDE_DIST_PATH ?? ''}\n`)
} else {
  const child = spawn('pnpm', ['exec', 'electron-vite', 'dev'], {
    cwd: desktopRoot,
    env,
    stdio: 'inherit'
  })
  child.on('exit', (code, signal) => {
    if (signal) process.kill(process.pid, signal)
    else process.exitCode = code ?? 1
  })
}
