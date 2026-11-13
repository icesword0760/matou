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

  it('forks Codex from the source identity through its provider-specific subcommand', () => {
    expect(resolvePtyCommand({
      profile: 'codex', executable: '/fixture/codex',
      providerSessionId: 'codex-source-1', forkSession: true,
      codexDeveloperInstructions: 'Use mt.'
    })).toEqual({
      file: '/fixture/codex',
      args: [
        '-c', 'developer_instructions="Use mt."',
        'fork', 'codex-source-1'
      ],
      resuming: true
    })
  })

  it('loads the Matou-owned Codex identity hook for a managed launch', () => {
    const codexHooksConfig = '{ SessionStart = [{ hooks = [{ type = "command", command = "/fixture/provider-hooks/run.statusline.sh", timeout = 5 }] }] }'
    expect(resolvePtyCommand({
      profile: 'codex', executable: '/fixture/codex',
      codexHooksConfig
    }).args).toEqual([
      '-c', `hooks=${codexHooksConfig}`,
      '--dangerously-bypass-hook-trust'
    ])
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

  it('produces byte-identical args to main for Claude and Codex across all permission modes and launch kinds', () => {
    const modes = [
      undefined,
      'default',
      'auto',
      'acceptEdits',
      'plan',
      'bypassPermissions'
    ] as const
    const launchKinds = [
      { kind: 'new', providerSessionId: undefined, forkSession: false },
      { kind: 'resume', providerSessionId: 'session-123', forkSession: false },
      { kind: 'fork', providerSessionId: 'session-123', forkSession: true }
    ] as const

    function mainClaudeArgs(
      permissionMode: string | undefined,
      providerSessionId: string | undefined,
      forkSession: boolean
    ) {
      const args = [
        '--settings', '/private/matou/settings.json',
        '--plugin-dir', '/private/matou/assets/providers/claude-plugin',
        '--model', 'model-x'
      ]
      if (providerSessionId) args.push('--resume', providerSessionId)
      if (providerSessionId && forkSession) args.push('--fork-session')
      if (permissionMode === 'bypassPermissions') {
        args.push('--dangerously-skip-permissions')
      } else if (
        permissionMode === 'auto' ||
        permissionMode === 'acceptEdits' ||
        permissionMode === 'plan'
      ) {
        args.push('--permission-mode', permissionMode)
      }
      return { file: '/fixture/claude', args, resuming: Boolean(providerSessionId) }
    }

    function mainCodexArgs(
      permissionMode: string | undefined,
      providerSessionId: string | undefined,
      forkSession: boolean
    ) {
      const args = [
        '-c', 'hooks={}',
        '--dangerously-bypass-hook-trust',
        '-c', 'developer_instructions="Use mt."',
        '--model', 'gpt-x'
      ]
      if (permissionMode === 'bypassPermissions') {
        args.push('--dangerously-bypass-approvals-and-sandbox')
      }
      if (providerSessionId) args.push(forkSession ? 'fork' : 'resume', providerSessionId)
      return { file: '/fixture/codex', args, resuming: Boolean(providerSessionId) }
    }

    for (const permissionMode of modes) {
      for (const { providerSessionId, forkSession } of launchKinds) {
        const optional = {
          ...(providerSessionId === undefined ? {} : { providerSessionId }),
          ...(permissionMode === undefined ? {} : { permissionMode })
        }
        expect(resolvePtyCommand({
          profile: 'claude-code',
          executable: '/fixture/claude',
          settingsPath: '/private/matou/settings.json',
          controlAssetRoot: '/private/matou/assets',
          model: 'model-x',
          forkSession,
          ...optional
        })).toEqual(mainClaudeArgs(permissionMode, providerSessionId, forkSession))

        expect(resolvePtyCommand({
          profile: 'codex',
          executable: '/fixture/codex',
          codexHooksConfig: '{}',
          codexDeveloperInstructions: 'Use mt.',
          model: 'gpt-x',
          forkSession,
          ...optional
        })).toEqual(mainCodexArgs(permissionMode, providerSessionId, forkSession))
      }
    }
  })

  it('builds Antigravity launch and resume commands across permission modes without injecting --model on fresh launch', () => {
    expect(resolvePtyCommand({
      profile: 'antigravity',
      executable: 'agy',
      settingsPath: '/tmp/agy.settings.json',
      permissionMode: 'default'
    })).toEqual({
      file: 'agy',
      args: ['--settings', '/tmp/agy.settings.json'],
      resuming: false
    })

    expect(resolvePtyCommand({
      profile: 'antigravity',
      executable: 'agy',
      settingsPath: '/tmp/agy.settings.json',
      permissionMode: 'acceptEdits'
    })).toEqual({
      file: 'agy',
      args: ['--settings', '/tmp/agy.settings.json', '--mode', 'accept-edits'],
      resuming: false
    })

    expect(resolvePtyCommand({
      profile: 'antigravity',
      executable: 'agy',
      permissionMode: 'plan'
    })).toEqual({
      file: 'agy',
      args: ['--mode', 'plan'],
      resuming: false
    })

    expect(resolvePtyCommand({
      profile: 'antigravity',
      executable: 'agy',
      permissionMode: 'bypassPermissions',
      providerSessionId: 'conv-123',
      model: 'Gemini 4 Carbon (Medium)'
    })).toEqual({
      file: 'agy',
      args: ['--model', 'Gemini 4 Carbon (Medium)', '--conversation', 'conv-123', '--dangerously-skip-permissions'],
      resuming: true
    })
  })
})
