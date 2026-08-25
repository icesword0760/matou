export interface PtyCommandInput {
  profile: 'shell' | 'claude-code' | 'codex'
  executable: string
  providerSessionId?: string
  forkSession?: boolean
  permissionMode?: string
  settingsPath?: string
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
    if (identity) args.push('--resume', identity)
    if (identity && input.forkSession) args.push('--fork-session')
    if (input.permissionMode === 'bypassPermissions') {
      args.push('--dangerously-skip-permissions')
    }
    return { file: input.executable, args, resuming: Boolean(identity) }
  }
  if (input.profile === 'codex') {
    const args = identity ? ['resume', identity] : []
    if (input.permissionMode === 'bypassPermissions') {
      args.push('--dangerously-bypass-approvals-and-sandbox')
    }
    return { file: input.executable, args, resuming: Boolean(identity) }
  }
  return {
    file: input.executable,
    args: process.platform === 'win32' ? [] : ['-l'],
    resuming: false
  }
}
