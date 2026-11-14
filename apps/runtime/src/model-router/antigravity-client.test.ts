import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, expect, it } from 'vitest'
import { ANTIGRAVITY_CLIENT_USER_AGENT, credentialCarriesAntigravityIdentity } from './antigravity-client'

const roots: string[] = []
afterEach(async () => { while (roots.length) await rm(roots.pop()!, { recursive: true, force: true }) })
async function fixture() {
  const root = await mkdtemp(join(tmpdir(), 'matou-antigravity-identity-'))
  roots.push(root)
  const directory = join(root, 'model-router', 'credentials')
  await mkdir(directory, { recursive: true })
  return { root, directory }
}

it('reports the Antigravity Hub client, never the relay product name', () => {
  expect(ANTIGRAVITY_CLIENT_USER_AGENT).toMatch(/^antigravity\/hub\/\d+\.\d+\.\d+ \(aidev_client; os_type=darwin; arch=arm64/)
  expect(ANTIGRAVITY_CLIENT_USER_AGENT).toMatch(/cl=\d+\)$/)
  expect(ANTIGRAVITY_CLIENT_USER_AGENT).not.toMatch(/matou|cliproxy/i)
})

it('accepts only a credential that already carries the shared agent', async () => {
  const { root, directory } = await fixture()
  const path = join(directory, 'agy.json')
  await writeFile(path, JSON.stringify({ user_agent: ANTIGRAVITY_CLIENT_USER_AGENT }))
  await expect(credentialCarriesAntigravityIdentity(root, { name: 'agy.json' })).resolves.toBe(true)
  await writeFile(path, JSON.stringify({ user_agent: 'Matou (antigravity/cli/1.4.6)' }))
  await expect(credentialCarriesAntigravityIdentity(root, { name: 'agy.json' })).resolves.toBe(false)
  await writeFile(path, JSON.stringify({ refresh_token: 'fixture' }))
  await expect(credentialCarriesAntigravityIdentity(root, { name: 'agy.json' })).resolves.toBe(false)
})

it('reads the reported credential path and refuses paths outside the credential directory', async () => {
  const { root, directory } = await fixture()
  const path = join(directory, 'agy.json')
  await writeFile(path, JSON.stringify({ user_agent: ANTIGRAVITY_CLIENT_USER_AGENT }))
  await expect(credentialCarriesAntigravityIdentity(root, { name: 'agy.json', path })).resolves.toBe(true)
  const outside = join(root, 'outside.json')
  await writeFile(outside, JSON.stringify({ user_agent: ANTIGRAVITY_CLIENT_USER_AGENT }))
  await expect(credentialCarriesAntigravityIdentity(root, { name: 'agy.json', path: outside })).resolves.toBe(false)
  await expect(credentialCarriesAntigravityIdentity(root, { name: '../outside.json' })).resolves.toBe(false)
})
