import assert from 'node:assert/strict'
import test from 'node:test'

import { getLocalizedElectronBundlePaths } from './desktop-dev-bundle-path.mjs'

test('uses the Chinese product name as the macOS development bundle filename', () => {
  const paths = getLocalizedElectronBundlePaths({
    projectRoot: '/fixture/matou',
    electronVersion: '43.4.1',
    productName: '码头'
  })

  assert.equal(paths.bundle, '/fixture/matou/node_modules/.cache/matou-electron-43.4.1-cn-v2/码头.app')
  assert.equal(paths.launcher, '/fixture/matou/node_modules/.cache/matou-electron-43.4.1-cn-v2/码头.app/Contents/MacOS/Electron')
})
