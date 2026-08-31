import { mkdtemp, readFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { describe, expect, it } from 'vitest'

import { shellIntegrationEnvironment } from './shell-integration'

describe('shellIntegrationEnvironment', () => {
  it('emits a private command identity marker before each zsh command', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-shell-integration-'))
    const environment = await shellIntegrationEnvironment(root, '/bin/zsh')
    const rc = await readFile(join(environment.ZDOTDIR!, '.zshrc'), 'utf8')

    expect(rc).toContain("printf '%s' \"$1\" | base64 | tr -d '\\n'")
    expect(rc).toContain("printf '\\033]633;E;%s\\007'")
    expect(rc.indexOf('633;E')).toBeLessThan(rc.indexOf('133;C'))
  })
})
