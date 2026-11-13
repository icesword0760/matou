import { afterEach, describe, expect, it } from 'vitest'
import { mkdir, mkdtemp, rename, rm, symlink, writeFile } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { RouterProcessOwner } from './process-owner'
import { prepareRouterDirectory, writeRouterConfig } from './config'

const roots: string[] = []
afterEach(async () => { await Promise.all(roots.splice(0).map(root => rm(root, { recursive: true, force: true }))) })

describe('router owner canonical path boundary', () => {
  it.each(['model-router', 'config.json', 'credentials'] as const)('rejects a %s symlink escaping the canonical data root before spawn', async resource => {
    const output = resolve(import.meta.dirname, '../../../../output/model-router-stage8/path-tests')
    await mkdir(output, { recursive: true })
    const root = await mkdtemp(join(output, 'escape-')); roots.push(root)
    const dataRoot = join(root, 'data'), outside = join(root, 'data-other')
    await mkdir(outside)
    const { directory, keys } = await prepareRouterDirectory(dataRoot)
    await writeRouterConfig(directory, 12345, keys)
    const source = resource === 'model-router' ? directory : join(directory, resource)
    const target = join(outside, resource)
    await rename(source, target)
    await symlink(target, source, resource === 'config.json' ? 'file' : 'dir')
    const alias = join(root, 'data-alias'); await symlink(dataRoot, alias, 'dir')
    const owner = new RouterProcessOwner({ dataRoot: alias, executable: process.execPath, supervisor: join(root, 'never-spawn.cjs'), binary: join(root, 'never-run') })
    await expect(owner.start()).rejects.toThrow('Invalid model router directory')
    await owner.stop()
    if (resource !== 'config.json') await expect(prepareRouterDirectory(alias)).rejects.toThrow('Invalid model router directory')
  })

  it('rejects a configured credential directory outside the canonical data root', async () => {
    const output = resolve(import.meta.dirname, '../../../../output/model-router-stage8/path-tests')
    await mkdir(output, { recursive: true })
    const root = await mkdtemp(join(output, 'auth-escape-')); roots.push(root)
    const dataRoot = join(root, 'data'), outside = join(root, 'outside')
    await mkdir(outside)
    const { directory, keys } = await prepareRouterDirectory(dataRoot)
    await writeRouterConfig(directory, 12345, keys)
    const { readFile } = await import('node:fs/promises')
    const config = JSON.parse(await readFile(join(directory, 'config.json'), 'utf8'))
    config.oauth['auth-dir'] = outside
    await writeFile(join(directory, 'config.json'), JSON.stringify(config))
    const owner = new RouterProcessOwner({ dataRoot, executable: process.execPath, supervisor: join(root, 'never-spawn.cjs'), binary: join(root, 'never-run') })
    await expect(owner.start()).rejects.toThrow('Invalid model router configuration')
    await owner.stop()
  })
})
