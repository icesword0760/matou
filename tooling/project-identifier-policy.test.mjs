import assert from 'node:assert/strict'
import { mkdtemp, mkdir, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'

import { isGeneratedArtifactPath, scanProjectPaths } from './project-identifier-policy.mjs'

const retiredTerminalHost = ['c', 'm', 'u', 'x'].join('')
const retiredReferenceBrand = ['k', 'o', 'o', 'k', 'y'].join('')

test('reports retired external product identifiers in tracked paths and file content', async () => {
  const root = await mkdtemp(join(tmpdir(), 'matou-identifier-policy-'))
  await mkdir(join(root, `src-${retiredTerminalHost}`))
  await writeFile(join(root, `src-${retiredTerminalHost}`, 'clean.ts'), 'export const value = 1\n')
  await writeFile(join(root, 'legacy.ts'), `const oldName = '${retiredReferenceBrand}'\n`)

  const violations = await scanProjectPaths(root, [
    `src-${retiredTerminalHost}/clean.ts`,
    'legacy.ts'
  ])

  assert.deepEqual(violations.map(({ location }) => location), [
    `src-${retiredTerminalHost}/clean.ts`,
    'legacy.ts'
  ])
})

test('accepts neutral Matou names', async () => {
  const root = await mkdtemp(join(tmpdir(), 'matou-identifier-policy-'))
  await writeFile(join(root, 'terminal-control.ts'), 'export const product = "Matou"\n')

  assert.deepEqual(await scanProjectPaths(root, ['terminal-control.ts']), [])
})

test('skips directory entries from nested untracked work areas', async () => {
  const root = await mkdtemp(join(tmpdir(), 'matou-identifier-policy-'))
  await mkdir(join(root, 'nested-work-area'))

  assert.deepEqual(await scanProjectPaths(root, ['nested-work-area']), [])
})

test('classifies local build and browser evidence as generated artifacts', () => {
  assert.equal(isGeneratedArtifactPath('apps/desktop/release-preview/mac-arm64/Matou.app'), true)
  assert.equal(isGeneratedArtifactPath('output/playwright/reference.png'), true)
  assert.equal(isGeneratedArtifactPath('.playwright-cli/session.json'), true)
  assert.equal(isGeneratedArtifactPath('apps/desktop/src/main/index.ts'), false)
})
