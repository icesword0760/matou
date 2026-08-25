import { describe, expect, it } from 'vitest'

import { resolvePtyCommand } from './provider-launch-plan'

describe('PRD 04 provider launch plan', () => {
  it('resumes Claude Code with its validated identity and persisted bypass mode', () => {
    expect(resolvePtyCommand({
      profile: 'claude-code', executable: '/fixture/claude',
      providerSessionId: 'claude-session-1', permissionMode: 'bypassPermissions',
      settingsPath: '/private/matou/settings.json'
    })).toEqual({
      file: '/fixture/claude',
      args: [
        '--settings', '/private/matou/settings.json',
        '--resume', 'claude-session-1', '--dangerously-skip-permissions'
      ],
      resuming: true
    })
  })

  it('captures a new Claude conversation through additive hooks without changing user settings', () => {
    expect(resolvePtyCommand({
      profile: 'claude-code', executable: '/fixture/claude',
      settingsPath: '/private/matou/settings.json'
    })).toEqual({
      file: '/fixture/claude',
      args: ['--settings', '/private/matou/settings.json'],
      resuming: false
    })
  })

  it('resumes Codex by its independent session identity', () => {
    expect(resolvePtyCommand({
      profile: 'codex', executable: '/fixture/codex', providerSessionId: 'codex-session-1'
    })).toEqual({
      file: '/fixture/codex', args: ['resume', 'codex-session-1'], resuming: true
    })
  })

  it('opens an ordinary login Shell without any provider resume arguments', () => {
    const command = resolvePtyCommand({ profile: 'shell', executable: '/bin/zsh' })
    expect(command.file).toBe('/bin/zsh')
    expect(command.args).toEqual(process.platform === 'win32' ? [] : ['-l'])
    expect(command.resuming).toBe(false)
  })
})
