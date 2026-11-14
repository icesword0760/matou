import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, expect, it } from 'vitest'
import { CODEX_CLIENT_HEADERS, CODEX_CLIENT_ORIGINATOR, CODEX_CLIENT_USER_AGENT, CODEX_CLIENT_VERSION, credentialCarriesCodexIdentity } from './codex-client'

const roots: string[] = []
afterEach(async () => { while (roots.length) await rm(roots.pop()!, { recursive: true, force: true }) })
async function fixture() {
  const root = await mkdtemp(join(tmpdir(), 'matou-codex-identity-'))
  roots.push(root)
  const directory = join(root, 'model-router', 'credentials')
  await mkdir(directory, { recursive: true })
  return { root, directory }
}

it('reports the Codex CLI client, not the Claude Code client', () => {
  expect(CODEX_CLIENT_USER_AGENT).toMatch(/^codex-tui\/0\.162\.1 \(Mac OS \d+(?:\.\d+){0,2}; (?:arm64|x86_64)\) .+ \(codex-tui; 0\.162\.1\)$/)
  expect(CODEX_CLIENT_USER_AGENT).not.toMatch(/claude|matou/i)
  expect(CODEX_CLIENT_ORIGINATOR).toBe('codex-tui')
  expect(CODEX_CLIENT_VERSION).toBe('0.162.1')
  expect(CODEX_CLIENT_HEADERS).toEqual({ Originator: CODEX_CLIENT_ORIGINATOR })
})

it('accepts only a credential that already carries the Codex headers', async () => {
  const { root, directory } = await fixture()
  const path = join(directory, 'codex.json')
  await writeFile(path, JSON.stringify({ headers: CODEX_CLIENT_HEADERS }))
  await expect(credentialCarriesCodexIdentity(root, { name: 'codex.json' })).resolves.toBe(true)
  await writeFile(path, JSON.stringify({ headers: { Originator: 'codex_exec' } }))
  await expect(credentialCarriesCodexIdentity(root, { name: 'codex.json' })).resolves.toBe(false)
  await writeFile(path, JSON.stringify({}))
  await expect(credentialCarriesCodexIdentity(root, { name: 'codex.json' })).resolves.toBe(false)
})
