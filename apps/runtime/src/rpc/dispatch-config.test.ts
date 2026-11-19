import { mkdir, mkdtemp, readFile, readdir, rm, writeFile } from 'node:fs/promises'
import * as fs from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { DISPATCH_DEFAULTS } from '@matou/contracts'
import { DispatchConfig } from './dispatch-config'

vi.mock('node:fs/promises', async importOriginal => {
  const original = await importOriginal<typeof import('node:fs/promises')>()
  return { ...original, rename: vi.fn(original.rename) }
})

let home: string
let config: DispatchConfig
beforeEach(async () => {
  home = await mkdtemp(join(tmpdir(), 'matou-dispatch-'))
  vi.stubEnv('HOME', home)
  config = new DispatchConfig()
})
afterEach(async () => {
  vi.unstubAllEnvs()
  vi.clearAllMocks()
  await rm(home, { recursive: true, force: true })
})

async function seed(value: unknown) {
  await mkdir(dirname(config.path), { recursive: true })
  await writeFile(config.path, JSON.stringify(value))
}

describe('DispatchConfig', () => {
  it('reads absent files without creating directories', async () => {
    expect(await config.snapshot()).toEqual({
      exists: false, path: join(home, '.claude/agent-foreman/config.json'), defaults: DISPATCH_DEFAULTS
    })
    expect(await readdir(home)).toEqual([])
  })

  it.each([null, [], { version: 2 }, { version: '1' }, {}])('treats an unsupported document as absent: %j', async value => {
    await seed(value)
    expect(await config.snapshot()).toMatchObject({ exists: false, defaults: DISPATCH_DEFAULTS })
  })

  it('treats malformed JSON as absent and repairs it on explicit save', async () => {
    await seed({})
    await writeFile(config.path, '{broken')
    expect(await config.snapshot()).toMatchObject({ exists: false, defaults: DISPATCH_DEFAULTS })
    expect(await config.save(DISPATCH_DEFAULTS)).toMatchObject({ exists: true })
  })

  it('falls back per field and preserves valid script values outside UI ranges', async () => {
    await seed({ version: 1, defaults: {
      agent: 'dsh', model: 'custom/model', effort: 'medium',
      idleMin: 600, maxMin: 0, askWaitMin: 'bad', retries: 0
    } })
    expect(await config.snapshot()).toMatchObject({ exists: true, defaults: {
      agent: 'dsh', model: 'custom/model', effort: 'medium',
      idleMin: 600, maxMin: 45, askWaitMin: 10, retries: 0
    } })
    await seed({ version: 1, defaults: null })
    expect(await config.snapshot()).toMatchObject({ exists: true, defaults: DISPATCH_DEFAULTS })
  })

  it('creates the directory and atomically replaces the config with only known fields', async () => {
    await config.save(DISPATCH_DEFAULTS)
    const rename = vi.mocked(fs.rename)
    rename.mockImplementationOnce(async (source, destination) => {
      expect(JSON.parse(await readFile(destination, 'utf8')).defaults.model).toBe('sol')
      expect(JSON.parse(await readFile(source, 'utf8')).defaults.model).toBe('astra')
      const original = await vi.importActual<typeof fs>('node:fs/promises')
      await original.rename(source, destination)
    })
    const saved = await config.save({ ...DISPATCH_DEFAULTS, model: 'astra', extra: 'ignored' })
    expect(saved).toMatchObject({ exists: true, defaults: { ...DISPATCH_DEFAULTS, model: 'astra' } })
    expect(JSON.parse(await readFile(config.path, 'utf8'))).toEqual({ version: 1, defaults: saved.defaults })
    expect(await config.snapshot()).toEqual(saved)
    expect(await readdir(dirname(config.path))).toEqual(['config.json'])
  })

  it('rejects the first invalid field without touching existing data or creating directories', async () => {
    await expect(config.save({ ...DISPATCH_DEFAULTS, model: '', effort: 'bad' })).rejects.toThrow('model')
    expect(await readdir(home)).toEqual([])
    await config.save(DISPATCH_DEFAULTS)
    const before = await readFile(config.path, 'utf8')
    await expect(config.save({ ...DISPATCH_DEFAULTS, retries: 6 })).rejects.toThrow('retries')
    expect(await readFile(config.path, 'utf8')).toBe(before)
    await expect(config.save(undefined)).rejects.toThrow('defaults')
  })

  it('retains the old file and cleans up temporary output if replacement fails', async () => {
    await config.save(DISPATCH_DEFAULTS)
    vi.mocked(fs.rename).mockRejectedValueOnce(new Error('rename failed'))
    await expect(config.save({ ...DISPATCH_DEFAULTS, retries: 1 })).rejects.toThrow('rename failed')
    expect(await config.snapshot()).toMatchObject({ defaults: DISPATCH_DEFAULTS })
    expect(await readdir(dirname(config.path))).toEqual(['config.json'])
  })

  it('reports read failures instead of treating inaccessible paths as empty config', async () => {
    await mkdir(config.path, { recursive: true })
    await expect(config.snapshot()).rejects.toThrow()
  })
})
