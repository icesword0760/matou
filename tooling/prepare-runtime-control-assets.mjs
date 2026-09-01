import { chmod, cp, mkdir, rm } from 'node:fs/promises'
import { dirname, resolve } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')

export async function prepareRuntimeControlAssets({
  source = resolve(root, 'apps/runtime/control-assets'),
  destination = resolve(root, 'apps/runtime/dist/control-assets')
} = {}) {
  await rm(destination, { recursive: true, force: true })
  await mkdir(dirname(destination), { recursive: true })
  await cp(source, destination, { recursive: true })
  if (process.platform !== 'win32') {
    await chmod(resolve(destination, 'bin/mt'), 0o755)
  }
  return destination
}

const invokedPath = process.argv[1] ? pathToFileURL(resolve(process.argv[1])).href : undefined
if (invokedPath === import.meta.url) {
  await prepareRuntimeControlAssets()
}
