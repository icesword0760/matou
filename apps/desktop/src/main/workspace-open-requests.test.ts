import { mkdtemp, mkdir, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

import { describe, expect, it } from 'vitest'

import { WorkspaceOpenRequests } from './workspace-open-requests'

describe('WorkspaceOpenRequests', () => {
  it('queues each valid directory once and drains requests in arrival order', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-open-workspace-'))
    const first = join(root, 'first')
    const second = join(root, 'second')
    await mkdir(first)
    await mkdir(second)
    const requests = new WorkspaceOpenRequests()

    expect(await requests.enqueue(first)).toBe(true)
    expect(await requests.enqueue(join(first, '..', 'first'))).toBe(false)
    expect(await requests.enqueue(second)).toBe(true)
    expect(requests.drain()).toEqual([resolve(first), resolve(second)])
    expect(requests.drain()).toEqual([])
  })

  it('filters ordinary files and missing paths', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-open-filter-'))
    const file = join(root, 'README.md')
    await writeFile(file, 'content')
    const requests = new WorkspaceOpenRequests()

    expect(await requests.enqueue(file)).toBe(false)
    expect(await requests.enqueue(join(root, 'missing'))).toBe(false)
    expect(requests.drain()).toEqual([])
  })
})
