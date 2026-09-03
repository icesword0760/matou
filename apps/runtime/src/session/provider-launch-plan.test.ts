import { describe, expect, it } from 'vitest'

import { resolvePtyCommand } from './provider-launch-plan'

describe('PRD 04 provider launch plan', () => {
  it('resumes Claude Code with its validated identity and persisted bypass mode', () => {
    expect(resolvePtyCommand({
      profile: 'claude-code', executable: '/fixture/claude',
      providerSessionId: 'claude-session-1', permissionMode: 'bypassPermissions',
      settingsPath: '/private/matou/settings.json',
      controlAssetRoot: '/Applications/Matou Resources/control assets'
    })).toEqual({
      file: '/fixture/claude',
      args: [
        '--settings', '/private/matou/settings.json',
        '--plugin-dir', '/Applications/Matou Resources/control assets/providers/claude-plugin',
        '--resume', 'claude-session-1', '--dangerously-skip-permissions'
      ],
      resuming: true
    })
  })

  it('captures a new Claude conversation through additive hooks without changing user settings', () => {
    expect(resolvePtyCommand({
      profile: 'claude-code', executable: '/fixture/claude',
      settingsPath: '/private/matou/settings.json',
      controlAssetRoot: '/private/matou/control-assets'
    })).toEqual({
      file: '/fixture/claude',
      args: [
        '--settings', '/private/matou/settings.json',
        '--plugin-dir', '/private/matou/control-assets/providers/claude-plugin'
      ],
      resuming: false
    })
  })

  it('resumes Codex by its independent session identity', () => {
    expect(resolvePtyCommand({
      profile: 'codex', executable: '/fixture/codex', providerSessionId: 'codex-session-1',
      codexDeveloperInstructions: '第一行\n第二行有 "引号" 和 \\ 路径'
    })).toEqual({
      file: '/fixture/codex',
      args: [
        '-c', 'developer_instructions="第一行\\n第二行有 \\"引号\\" 和 \\\\ 路径"',
        'resume', 'codex-session-1'
      ],
      resuming: true
    })
  })

  it('launches each provider with the globally selected default model', () => {
    expect(resolvePtyCommand({
      profile: 'claude-code', executable: '/fixture/claude', model: 'claude-team'
    }).args).toEqual(['--model', 'claude-team'])
    expect(resolvePtyCommand({
      profile: 'codex', executable: '/fixture/codex', model: 'gpt-team',
      providerSessionId: 'codex-session-1'
    }).args).toEqual(['--model', 'gpt-team', 'resume', 'codex-session-1'])
  })

  it('opens an ordinary login Shell without any provider resume arguments', () => {
    const command = resolvePtyCommand({ profile: 'shell', executable: '/bin/zsh' })
    expect(command.file).toBe('/bin/zsh')
    expect(command.args).toEqual(process.platform === 'win32' ? [] : ['-l'])
    expect(command.resuming).toBe(false)
  })

  it('forks Claude Code from the source identity instead of resuming it in place', () => {
    expect(resolvePtyCommand({
      profile: 'claude-code', executable: '/fixture/claude',
      providerSessionId: 'claude-source-1', forkSession: true,
      permissionMode: 'bypassPermissions',
      settingsPath: '/private/matou/settings.json',
      controlAssetRoot: '/private/matou/control-assets'
    })).toEqual({
      file: '/fixture/claude',
      args: [
        '--settings', '/private/matou/settings.json',
        '--plugin-dir', '/private/matou/control-assets/providers/claude-plugin',
        '--resume', 'claude-source-1', '--fork-session',
        '--dangerously-skip-permissions'
      ],
      resuming: true
    })
  })

  it.each([
    ['auto', 'auto'],
    ['acceptEdits', 'acceptEdits'],
    ['plan', 'plan']
  ])('launches Claude Code in the persisted %s permission mode', (permissionMode, cliMode) => {
    expect(resolvePtyCommand({
      profile: 'claude-code', executable: '/fixture/claude', permissionMode
    }).args).toEqual(['--permission-mode', cliMode])
  })

  it('keeps Codex permission flags global while loading session-only instructions', () => {
    expect(resolvePtyCommand({
      profile: 'codex', executable: '/fixture/codex',
      permissionMode: 'bypassPermissions', codexDeveloperInstructions: 'Use mt.'
    })).toEqual({
      file: '/fixture/codex',
      args: [
        '-c', 'developer_instructions="Use mt."',
        '--dangerously-bypass-approvals-and-sandbox'
      ],
      resuming: false
    })
  })
})
