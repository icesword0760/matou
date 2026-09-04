export interface TerminalLoadingState {
  visible: boolean
  foreground: boolean
  isTeamMember: boolean
  pathValid: boolean
  terminalVisualReady: boolean
  recoveryState?: 'queued' | 'restoring' | 'ready' | 'failed'
  providerRestoreState: 'none' | 'restoring' | 'failed'
  forkState?: 'pending' | 'starting' | 'succeeded' | 'failed'
  hasForkProgress: boolean
  runtimeStatus: 'waiting-for-port' | 'handshaking' | 'starting-session' | 'streaming' | 'error' | 'exited'
  hasStorageFault: boolean
  environmentUnavailable: boolean
}

export interface TerminalLoadingPresentation {
  phase: 'loading' | 'recovery'
  label: '加载中' | '恢复中'
}

/**
 * Keeps the terminal's blank-to-first-frame handoff independent from the
 * Runtime recovery queue. Existing idle Sessions and a newly created Session
 * can both be waiting for their first visible xterm frame without owning a
 * recovery job.
 */
export function terminalLoadingPresentation(
  state: TerminalLoadingState
): TerminalLoadingPresentation | null {
  if (
    !state.visible || !state.foreground || state.isTeamMember || !state.pathValid ||
    state.terminalVisualReady || state.recoveryState === 'failed' ||
    state.providerRestoreState === 'failed' || state.forkState === 'failed' ||
    state.hasForkProgress || state.runtimeStatus === 'error' ||
    state.runtimeStatus === 'exited' || state.hasStorageFault ||
    state.environmentUnavailable
  ) return null

  return state.recoveryState === undefined
    ? { phase: 'loading', label: '加载中' }
    : { phase: 'recovery', label: '恢复中' }
}
