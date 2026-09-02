import { join } from 'node:path'

export function getLocalizedElectronBundlePaths({ projectRoot, electronVersion, productName }) {
  const cacheRoot = join(
    projectRoot,
    'node_modules',
    '.cache',
    `matou-electron-${electronVersion}-cn-v2`
  )
  const bundle = join(cacheRoot, `${productName}.app`)

  return {
    cacheRoot,
    bundle,
    launcher: join(bundle, 'Contents', 'MacOS', 'Electron')
  }
}
