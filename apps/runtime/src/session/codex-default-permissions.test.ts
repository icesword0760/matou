import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, describe, expect, it } from 'vitest'
import { parseCodexDefaultPermissions, readCodexDefaultPermissions } from './codex-default-permissions'

const roots: string[] = []
afterEach(async () => { await Promise.all(roots.splice(0).map(root => rm(root, { recursive: true, force: true }))) })

const fallback = { approvalPolicy: 'on-request', sandboxMode: 'workspace-write' }

describe('Codex Default permission config', () => {
  it('reads root policies, comments and quoted keys while ignoring profile values', () => {
    expect(parseCodexDefaultPermissions(`
"approval_policy" = 'never' # user intentionally chose this
sandbox_mode = "danger-full-access"
[profiles.safe]
approval_policy = "on-request"
sandbox_mode = "read-only"
`)).toEqual({ approvalPolicy: 'never', sandboxMode: 'danger-full-access' })
  })

  it.each([
    '', '# no permissions\nmodel = "test"',
    '[profiles.work]\napproval_policy = "never"\nsandbox_mode = "read-only"',
    'approval_policy = "never',
    'approval_policy = "never"\nvalues = [',
    'approval_policy = "never"\napproval_policy = "on-request"',
    'approval_policy = "unknown"\nsandbox_mode = true'
  ])('falls back for missing, malformed or invalid policy: %s', text => {
    expect(parseCodexDefaultPermissions(text)).toEqual(fallback)
  })

  it('falls back independently for a missing or invalid field', () => {
    expect(parseCodexDefaultPermissions('approval_policy = "never"')).toEqual({ ...fallback, approvalPolicy: 'never' })
    expect(parseCodexDefaultPermissions('approval_policy = false\nsandbox_mode = "read-only"')).toEqual({ ...fallback, sandboxMode: 'read-only' })
  })

  it('does not read fake policy lines inside multiline strings or containers', () => {
    expect(parseCodexDefaultPermissions(`
developer_instructions = """
approval_policy = "never"
[profiles.fake]
sandbox_mode = "danger-full-access"
"""
values = [
  '''
approval_policy = "never"
'''
]
approval_policy = "on-request"
sandbox_mode = "workspace-write"
[profiles.actual]
approval_policy = "never"
`)).toEqual(fallback)
  })

  it('uses the launch CODEX_HOME, reads updated values and never writes the file', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-codex-default-')); roots.push(root)
    const home = join(root, 'home'), codex = join(root, 'isolated')
    await mkdir(join(home, '.codex'), { recursive: true }); await mkdir(codex)
    await writeFile(join(home, '.codex/config.toml'), 'approval_policy="never"\nsandbox_mode="danger-full-access"')
    const path = join(codex, 'config.toml')
    const initial = '# keep exactly\napproval_policy="on-request"\nsandbox_mode="read-only"\n'
    await writeFile(path, initial)
    expect(await readCodexDefaultPermissions({ HOME: home, CODEX_HOME: codex })).toEqual({ approvalPolicy: 'on-request', sandboxMode: 'read-only' })
    expect(await readFile(path, 'utf8')).toBe(initial)
    await writeFile(path, 'approval_policy="never"\nsandbox_mode="workspace-write"')
    expect(await readCodexDefaultPermissions({ HOME: home, CODEX_HOME: codex })).toEqual({ approvalPolicy: 'never', sandboxMode: 'workspace-write' })
    expect(await readCodexDefaultPermissions({ HOME: home })).toEqual({ approvalPolicy: 'never', sandboxMode: 'danger-full-access' })
    expect(await readCodexDefaultPermissions({ CODEX_HOME: join(root, 'missing') })).toEqual(fallback)
  })
})
