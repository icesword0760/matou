import { describe, expect, it } from 'vitest'

import {
  terminalLoadingPresentation,
  type TerminalLoadingState
} from './terminal-loading-state'

const base = (): TerminalLoadingState => ({
  visible: true,
  foreground: true,
  isTeamMember: false,
  pathValid: true,
  terminalVisualReady: false,
  providerRestoreState: 'none',
  hasForkProgress: false,
  runtimeStatus: 'waiting-for-port',
  hasStorageFault: false,
  environmentUnavailable: false
})

describe('terminal loading presentation', () => {
  it.each([
    ['new Session waiting for its first frame', {}, 'loading'],
    ['existing idle Session absent from the recovery plan', { runtimeStatus: 'starting-session' }, 'loading'],
    ['recovery snapshot queued', { recoveryState: 'queued' }, 'recovery'],
    ['recovery process running', { recoveryState: 'restoring' }, 'recovery'],
    ['recovery ready before xterm paints', { recoveryState: 'ready', runtimeStatus: 'streaming' }, 'recovery'],
    ['provider recovery before xterm paints', { providerRestoreState: 'restoring' }, 'loading']
  ] as const)('shows water for %s', (_name, patch, phase) => {
    expect(terminalLoadingPresentation({ ...base(), ...patch })).toMatchObject({ phase })
  })

  it('uses loading water while an exited Session is repainting after activation', () => {
    expect(terminalLoadingPresentation({
      ...base(), activationLoading: true, runtimeStatus: 'exited'
    })).toEqual({ phase: 'loading', label: '加载中' })
  })

  it.each([
    ['first frame rendered', { terminalVisualReady: true }],
    ['background level', { foreground: false }],
    ['virtualized hidden card', { visible: false }],
    ['team member summary', { isTeamMember: true }],
    ['invalid workspace path', { pathValid: false }],
    ['generic recovery failed', { recoveryState: 'failed' }],
    ['provider recovery failed', { providerRestoreState: 'failed' }],
    ['Fork failed', { forkState: 'failed' }],
    ['Fork progress owns the card', { hasForkProgress: true }],
    ['Runtime start failed', { runtimeStatus: 'error' }],
    ['terminal exited', { runtimeStatus: 'exited' }],
    ['storage repair owns the card', { hasStorageFault: true }],
    ['environment repair owns the card', { environmentUnavailable: true }]
  ] as const)('hides water when %s', (_name, patch) => {
    expect(terminalLoadingPresentation({ ...base(), ...patch })).toBeNull()
  })
})
