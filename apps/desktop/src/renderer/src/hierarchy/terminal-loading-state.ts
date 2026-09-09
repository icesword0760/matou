export interface TerminalLoadingState {
  visible: boolean
  foreground: boolean
  isTeamMember: boolean
  pathValid: boolean
  terminalVisualReady: boolean
  activationLoading?: boolean
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
  /** The component resolves the text through `hierarchyTerminal.loading`. */
  labelKey: 'loading' | 'recovering'
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
  const activationLoading = state.activationLoading === true
  if (
    !state.visible || !state.foreground || state.isTeamMember || !state.pathValid ||
    state.recoveryState === 'failed' ||
    state.providerRestoreState === 'failed' || state.forkState === 'failed' ||
    state.hasForkProgress || state.runtimeStatus === 'error' ||
    state.hasStorageFault ||
    state.environmentUnavailable
  ) return null

  if (activationLoading) return { phase: 'loading', labelKey: 'loading' }
  if (state.terminalVisualReady || state.runtimeStatus === 'exited') return null

  return state.recoveryState === undefined
    ? { phase: 'loading', labelKey: 'loading' }
    : { phase: 'recovery', labelKey: 'recovering' }
}
