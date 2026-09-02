import { mkdtemp, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { describe, expect, it } from 'vitest'

import { createE2eForkSetupPolicyProvider } from './e2e-fork-setup-policy'

describe('createE2eForkSetupPolicyProvider', () => {
  it('ignores setup injection unless MATOU_E2E is explicit', () => {
    expect(createE2eForkSetupPolicyProvider({
      MATOU_E2E: '0', MATOU_E2E_FORK_SETUP_CONTROL: '/tmp/not-read.json'
    })).toBeUndefined()
  })

  it('returns the configured real process step for E2E Forks', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-e2e-fork-setup-'))
    const control = join(root, 'control.json')
    await writeFile(control, JSON.stringify({
      idempotencyKey: 'slow-setup', command: '/usr/bin/true', args: ['--version']
    }))

    const provider = createE2eForkSetupPolicyProvider({
      MATOU_E2E: '1', MATOU_E2E_FORK_SETUP_CONTROL: control
    })

    expect(provider?.('workspace-1')).toEqual([{
      idempotencyKey: 'slow-setup', command: '/usr/bin/true', args: ['--version']
    }])
  })
})
