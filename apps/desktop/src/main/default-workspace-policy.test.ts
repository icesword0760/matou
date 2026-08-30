import { mkdtemp, stat } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { describe, expect, it } from 'vitest'

import { resolveDefaultWorkspacePath } from './default-workspace-policy'

describe('default Workspace path policy', () => {
  it('describes the macOS terminal start directory without recreating a moved folder', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-default-workspace-'))
    const movedPath = join(root, 'moved-workspace')

    expect(resolveDefaultWorkspacePath(movedPath, '/Users/tester')).toEqual({
      rootDirectory: movedPath,
      name: 'moved-workspace'
    })
    await expect(stat(movedPath)).rejects.toMatchObject({ code: 'ENOENT' })
  })

  it('uses the user home when no override is present', () => {
    expect(resolveDefaultWorkspacePath(undefined, '/Users/tester')).toEqual({
      rootDirectory: '/Users/tester', name: 'tester'
    })
  })
})
