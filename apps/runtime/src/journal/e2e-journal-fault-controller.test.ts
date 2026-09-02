import { mkdtemp, open, readFile, rename, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, describe, expect, it } from 'vitest'

import { createE2eJournalOptionsProvider } from './e2e-journal-fault-controller'

describe('E2E Journal fault controller', () => {
  const roots: string[] = []

  afterEach(async () => {
    await Promise.all(roots.splice(0).map((root) => rm(root, { recursive: true, force: true })))
  })

  it('stays absent unless the E2E gate and at least one Journal option are present', () => {
    expect(createE2eJournalOptionsProvider({ MATOU_E2E: '0' })).toBeUndefined()
    expect(createE2eJournalOptionsProvider({ MATOU_E2E: '1' })).toBeUndefined()
    expect(createE2eJournalOptionsProvider({
      MATOU_E2E: '0', MATOU_E2E_JOURNAL_FAULT_CONTROL: '/tmp/fault.json'
    })).toBeUndefined()
    expect(createE2eJournalOptionsProvider({
      MATOU_E2E: '0', MATOU_E2E_JOURNAL_MAX_SEGMENT_BYTES: '524288'
    })).toBeUndefined()
  })

  it('injects ENOSPC into only the selected Session and resumes real file writes when cleared', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-journal-fault-control-'))
    roots.push(root)
    const controlPath = join(root, 'control.json')
    const targetPath = join(root, 'target.bin')
    const healthyPath = join(root, 'healthy.bin')
    const provider = createE2eJournalOptionsProvider({
      MATOU_E2E: '1', MATOU_E2E_JOURNAL_FAULT_CONTROL: controlPath
    })
    expect(provider).toBeDefined()
    const targetWriter = provider!('session-a')!.writeFrame!
    const healthyWriter = provider!('session-b')!.writeFrame!
    const target = await open(targetPath, 'a+')
    const healthy = await open(healthyPath, 'a+')

    try {
      await atomicControlWrite(controlPath, { sessionId: 'session-a', code: 'ENOSPC' })
      await expect(targetWriter(target, Buffer.from('held'))).rejects.toMatchObject({ code: 'ENOSPC' })
      await healthyWriter(healthy, Buffer.from('live'))
      expect(await readFile(healthyPath, 'utf8')).toBe('live')
      expect(await readFile(targetPath, 'utf8')).toBe('')

      await atomicControlWrite(controlPath, {})
      await targetWriter(target, Buffer.from('recovered'))
      expect(await readFile(targetPath, 'utf8')).toBe('recovered')
    } finally {
      await target.close()
      await healthy.close()
    }
  })

  it('exposes deterministic rotation limits only behind the E2E gate', () => {
    expect(createE2eJournalOptionsProvider({
      MATOU_E2E: '1',
      MATOU_E2E_JOURNAL_MAX_SEGMENT_BYTES: '524288',
      MATOU_E2E_JOURNAL_RAW_HOT_BYTES: '1048576'
    })?.('session-a')).toMatchObject({
      maxSegmentBytes: 524_288,
      rawHotBytes: 1_048_576
    })
    expect(() => createE2eJournalOptionsProvider({
      MATOU_E2E: '1', MATOU_E2E_JOURNAL_MAX_SEGMENT_BYTES: '127'
    })).toThrow('MATOU_E2E_JOURNAL_MAX_SEGMENT_BYTES')
  })
})

async function atomicControlWrite(path: string, value: unknown): Promise<void> {
  const temporary = `${path}.next`
  await writeFile(temporary, JSON.stringify(value))
  await rename(temporary, path)
}
