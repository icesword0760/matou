import { describe, expect, it } from 'vitest'

import {
  AGENT_MANIFESTS,
  LAUNCHABLE_AGENT_PROFILES,
  getAgentDisplayName,
  getAgentManifest,
  getAgentShortName,
  isAgentProfileId,
  isAgentProviderId,
  isSessionKind,
  isSessionRunProfile,
  listAgentManifests
} from './agent-manifest'

describe('agent-manifest', () => {
  it('registers all launchable agent profiles with consistent lookup helpers', () => {
    expect(LAUNCHABLE_AGENT_PROFILES).toEqual(['claude-code', 'codex', 'antigravity'])
    expect(listAgentManifests().map((manifest) => manifest.id)).toEqual([
      'claude-code',
      'codex',
      'antigravity'
    ])

    for (const id of LAUNCHABLE_AGENT_PROFILES) {
      expect(isAgentProfileId(id)).toBe(true)
      expect(isSessionRunProfile(id)).toBe(true)
      expect(isSessionKind(id)).toBe(true)
      expect(isAgentProviderId(id)).toBe(true)
      expect(getAgentManifest(id)).toBe(AGENT_MANIFESTS[id])
    }

    expect(isAgentProfileId('shell')).toBe(false)
    expect(isSessionRunProfile('shell')).toBe(true)
    expect(isSessionKind('agent-team-member')).toBe(true)
    expect(isAgentProviderId('generic')).toBe(true)
    expect(getAgentManifest('unknown')).toBeUndefined()
  })

  it('preserves exact baseline capabilities for claude-code and codex', () => {
    const claude = AGENT_MANIFESTS['claude-code']
    expect(claude.displayName).toBe('Claude Code')
    expect(claude.shortName).toBe('Claude')
    expect(claude.capabilities).toMatchObject({
      fork: true,
      sessionLoader: true,
      autoTitle: true,
      providerConfig: true,
      accountSwitching: true,
      agentTeam: true,
      claudeTimeZone: true,
      autoRestore: true,
      permissionSwitchMode: 'shift-tab-hybrid'
    })

    const codex = AGENT_MANIFESTS.codex
    expect(codex.displayName).toBe('Codex')
    expect(codex.shortName).toBe('Codex')
    expect(codex.capabilities).toMatchObject({
      fork: false,
      sessionLoader: false,
      autoTitle: false,
      providerConfig: true,
      accountSwitching: false,
      agentTeam: false,
      claudeTimeZone: false,
      autoRestore: false,
      permissionSwitchMode: 'respawn-only'
    })
  })

  it('declares antigravity manifest according to finalized product decisions', () => {
    const agy = AGENT_MANIFESTS.antigravity
    expect(agy.displayName).toBe('Antigravity')
    expect(agy.shortName).toBe('Antigravity')
    expect(agy.executable).toEqual({
      envVar: 'MATOU_ANTIGRAVITY_COMMAND',
      defaultCommand: 'agy',
      interactiveAliases: ['agy']
    })
    expect(agy.launch.permissionFlags.default).toEqual([])
    expect(agy.capabilities.permissionModes).toEqual([
      'default',
      'acceptEdits',
      'plan',
      'bypassPermissions'
    ])
    expect(agy.capabilities.permissionSwitchMode).toBe('shift-tab-hybrid')
    expect(agy.capabilities.providerConfig).toBe(false)
    expect(agy.capabilities.accountSwitching).toBe(false)
    expect(agy.capabilities.agentTeam).toBe(false)
    expect(agy.capabilities.claudeTimeZone).toBe(false)
    expect('modelSwitch' in agy.capabilities).toBe(false)
    expect(agy.events.transcriptPathSource).toBe('hook')
    expect(agy.mtInjection.mode).toBe('session-start-system-message')
    expect(agy.instructionFiles).toEqual({
      candidates: ['AGENTS.md', 'GEMINI.md'],
      buttonLabel: 'AGENTS.md'
    })
  })

  it('formats display and short names with shell fallback', () => {
    expect(getAgentDisplayName('claude-code')).toBe('Claude Code')
    expect(getAgentDisplayName('codex')).toBe('Codex')
    expect(getAgentDisplayName('antigravity')).toBe('Antigravity')
    expect(getAgentDisplayName('shell')).toBe('Shell')
    expect(getAgentShortName('claude-code')).toBe('Claude')
    expect(getAgentShortName('codex')).toBe('Codex')
    expect(getAgentShortName('antigravity')).toBe('Antigravity')
    expect(getAgentShortName('shell')).toBe('Shell')
  })
})
