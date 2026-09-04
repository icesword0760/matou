import { join } from 'node:path'

export interface PtyCommandInput {
  profile: 'shell' | 'claude-code' | 'codex'
  executable: string
  providerSessionId?: string
  forkSession?: boolean
  permissionMode?: string
  settingsPath?: string
  codexHooksConfig?: string
  controlAssetRoot?: string
  codexDeveloperInstructions?: string
  model?: string
}

export interface PtyCommand {
  file: string
  args: string[]
  resuming: boolean
}

export function resolvePtyCommand(input: PtyCommandInput): PtyCommand {
  const identity = input.providerSessionId?.trim()
  if (input.profile === 'claude-code') {
    const settingsPath = input.settingsPath?.trim()
    const args = settingsPath ? ['--settings', settingsPath] : []
    const controlAssetRoot = input.controlAssetRoot?.trim()
    if (controlAssetRoot) {
      args.push('--plugin-dir', join(controlAssetRoot, 'providers', 'claude-plugin'))
    }
    if (input.model?.trim()) args.push('--model', input.model.trim())
    if (identity) args.push('--resume', identity)
    if (identity && input.forkSession) args.push('--fork-session')
    if (input.permissionMode === 'bypassPermissions') {
      args.push('--dangerously-skip-permissions')
    }
    return { file: input.executable, args, resuming: Boolean(identity) }
  }
  if (input.profile === 'codex') {
    const args: string[] = []
    if (input.codexHooksConfig?.trim()) {
      args.push(
        '-c',
        `hooks=${input.codexHooksConfig.trim()}`,
        '--dangerously-bypass-hook-trust'
      )
    }
    if (input.codexDeveloperInstructions) {
      args.push(
        '-c',
        `developer_instructions=${JSON.stringify(input.codexDeveloperInstructions)}`
      )
    }
    if (input.model?.trim()) args.push('--model', input.model.trim())
    if (input.permissionMode === 'bypassPermissions') {
      args.push('--dangerously-bypass-approvals-and-sandbox')
    }
    if (identity) args.push(input.forkSession ? 'fork' : 'resume', identity)
    return { file: input.executable, args, resuming: Boolean(identity) }
  }
  return {
    file: input.executable,
    args: process.platform === 'win32' ? [] : ['-l'],
    resuming: false
  }
}
