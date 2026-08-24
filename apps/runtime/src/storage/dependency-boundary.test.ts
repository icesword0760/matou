import { readFile, readdir } from 'node:fs/promises'
import { dirname, extname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

import { describe, expect, it } from 'vitest'

const workspaceRoot = resolve(dirname(fileURLToPath(import.meta.url)), '../../../..')

describe('storage dependency boundary', () => {
  it('keeps node:sqlite imports inside Runtime storage', async () => {
    const sourceFiles = await collectSourceFiles(workspaceRoot)
    const offenders: string[] = []

    for (const path of sourceFiles) {
      if (/\.(?:test|spec)\.[cm]?[jt]sx?$/.test(path)) continue
      const source = await readFile(path, 'utf8')
      if (source.includes("'node:sqlite'") && !path.includes('/apps/runtime/src/storage/')) {
        offenders.push(path.slice(workspaceRoot.length + 1))
      }
    }

    expect(offenders).toEqual([])
  })
})

async function collectSourceFiles(directory: string): Promise<string[]> {
  const results: string[] = []
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    if (['node_modules', 'dist', 'out', 'release', 'package-resources', 'test-results', '.git'].includes(entry.name)) {
      continue
    }
    const path = join(directory, entry.name)
    if (entry.isDirectory()) {
      results.push(...(await collectSourceFiles(path)))
    } else if (['.ts', '.tsx', '.js', '.mjs', '.cjs'].includes(extname(path))) {
      results.push(path)
    }
  }
  return results
}
