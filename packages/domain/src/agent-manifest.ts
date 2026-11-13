export const LAUNCHABLE_AGENT_PROFILES = ['claude-code', 'codex', 'antigravity'] as const
export type AgentProfileId = (typeof LAUNCHABLE_AGENT_PROFILES)[number]

export const SESSION_RUN_PROFILES = ['shell', ...LAUNCHABLE_AGENT_PROFILES] as const
export type SessionRunProfile = (typeof SESSION_RUN_PROFILES)[number]

/** Card runtime mode: ordinary terminal, launchable agent, or team member node. */
export const SESSION_KINDS = [...SESSION_RUN_PROFILES, 'agent-team-member'] as const
export type SessionKind = (typeof SESSION_KINDS)[number]
export type SessionCurrentMode = SessionKind

/** Source provider persisted in provider_bindings and fork intents. */
export const AGENT_PROVIDERS = [...LAUNCHABLE_AGENT_PROFILES, 'generic'] as const
export type AgentProviderId = (typeof AGENT_PROVIDERS)[number]

export const AGENT_PERMISSION_MODES = [
  'default',
  'auto',
  'acceptEdits',
  'plan',
  'bypassPermissions'
] as const
export type AgentPermissionMode = (typeof AGENT_PERMISSION_MODES)[number]

export type AgentPermissionSwitchMode = 'shift-tab-hybrid' | 'respawn-only' | 'none'

export interface AgentManifest {
  id: AgentProfileId
  displayName: string
  shortName: string
  executable: {
    envVar: 'MATOU_CLAUDE_COMMAND' | 'MATOU_CODEX_COMMAND' | 'MATOU_ANTIGRAVITY_COMMAND'
    defaultCommand: 'claude' | 'codex' | 'agy'
    interactiveAliases: readonly string[]
  }
  launch: {
    resumeFlag?: string
    forkStrategy: 'cli-flag' | 'subcommand' | 'resume-then-slash-fork' | 'none'
    forkFlag?: string
    modelFlag?: string
    permissionFlags: Partial<Record<AgentPermissionMode, readonly string[]>>
  }
  events: {
    lifecycleSource: 'http-hook' | 'plugin-command-hook' | 'none'
    statusLineSource: 'settings-http' | 'settings-command' | 'none'
    identitySource: 'hook-or-statusline' | 'none'
    transcriptPathSource: 'hook' | 'hook-or-statusline' | 'none'
    forkIdentityRule: 'immediate-new-id' | 'differs-from-parent-after-slash-fork'
  }
  mtInjection: {
    mode: 'plugin-dir' | 'developer-instructions-flag' | 'session-start-system-message' | 'none'
    assetRelativePath?: string
  }
  instructionFiles: {
    candidates: readonly string[]
    buttonLabel?: string
  }
  hud: {
    stripClaudeModelPrefix: boolean
    hideEmptyQuotaPlaceholder: boolean
    supportsTodos: boolean
    supportsAgentsCount: boolean
    supportsMcpSummary: boolean
    supportsHooksCount: boolean
    supportsQuotaWindows: boolean
  }
  capabilities: {
    fork: boolean
    sessionLoader: boolean
    autoTitle: boolean
    permissionModes: readonly AgentPermissionMode[]
    permissionSwitchMode: AgentPermissionSwitchMode
    providerConfig: boolean
    accountSwitching: boolean
    agentTeam: boolean
    claudeTimeZone: boolean
    autoRestore: boolean
    forkRequiresCompletedTurn: boolean
  }
}

export const AGENT_MANIFESTS: Record<AgentProfileId, AgentManifest> = {
  'claude-code': {
    id: 'claude-code',
    displayName: 'Claude Code',
    shortName: 'Claude',
    executable: {
      envVar: 'MATOU_CLAUDE_COMMAND',
      defaultCommand: 'claude',
      interactiveAliases: ['claude']
    },
    launch: {
      resumeFlag: '--resume',
      forkStrategy: 'cli-flag',
      forkFlag: '--fork-session',
      modelFlag: '--model',
      permissionFlags: {
        default: [],
        acceptEdits: ['--permission-mode', 'acceptEdits'],
        plan: ['--permission-mode', 'plan'],
        bypassPermissions: ['--dangerously-skip-permissions']
      }
    },
    events: {
      lifecycleSource: 'http-hook',
      statusLineSource: 'settings-http',
      identitySource: 'hook-or-statusline',
      transcriptPathSource: 'hook-or-statusline',
      forkIdentityRule: 'immediate-new-id'
    },
    mtInjection: {
      mode: 'plugin-dir'
    },
    instructionFiles: {
      candidates: ['CLAUDE.md'],
      buttonLabel: 'ClaudeMd'
    },
    hud: {
      stripClaudeModelPrefix: true,
      hideEmptyQuotaPlaceholder: false,
      supportsTodos: true,
      supportsAgentsCount: true,
      supportsMcpSummary: true,
      supportsHooksCount: true,
      supportsQuotaWindows: true
    },
    capabilities: {
      fork: true,
      sessionLoader: true,
      autoTitle: true,
      permissionModes: ['default', 'acceptEdits', 'plan', 'bypassPermissions'],
      permissionSwitchMode: 'shift-tab-hybrid',
      providerConfig: true,
      accountSwitching: true,
      agentTeam: true,
      claudeTimeZone: true,
      autoRestore: true,
      forkRequiresCompletedTurn: true
    }
  },
  codex: {
    id: 'codex',
    displayName: 'Codex',
    shortName: 'Codex',
    executable: {
      envVar: 'MATOU_CODEX_COMMAND',
      defaultCommand: 'codex',
      interactiveAliases: ['codex']
    },
    launch: {
      resumeFlag: 'resume',
      forkStrategy: 'subcommand',
      forkFlag: 'fork',
      modelFlag: '--model',
      permissionFlags: {
        default: [],
        bypassPermissions: ['--dangerously-bypass-approvals-and-sandbox']
      }
    },
    events: {
      lifecycleSource: 'http-hook',
      statusLineSource: 'none',
      identitySource: 'hook-or-statusline',
      transcriptPathSource: 'hook-or-statusline',
      forkIdentityRule: 'immediate-new-id'
    },
    mtInjection: {
      mode: 'developer-instructions-flag',
      assetRelativePath: 'providers/codex-developer-instructions.md'
    },
    instructionFiles: {
      candidates: []
    },
    hud: {
      stripClaudeModelPrefix: false,
      hideEmptyQuotaPlaceholder: false,
      supportsTodos: true,
      supportsAgentsCount: true,
      supportsMcpSummary: true,
      supportsHooksCount: true,
      supportsQuotaWindows: true
    },
    capabilities: {
      fork: false,
      sessionLoader: false,
      autoTitle: false,
      permissionModes: ['default', 'bypassPermissions'],
      permissionSwitchMode: 'respawn-only',
      providerConfig: true,
      accountSwitching: false,
      agentTeam: false,
      claudeTimeZone: false,
      autoRestore: false,
      forkRequiresCompletedTurn: false
    }
  },
  antigravity: {
    id: 'antigravity',
    displayName: 'Antigravity',
    shortName: 'Antigravity',
    executable: {
      envVar: 'MATOU_ANTIGRAVITY_COMMAND',
      defaultCommand: 'agy',
      interactiveAliases: ['agy']
    },
    launch: {
      resumeFlag: '--conversation',
      forkStrategy: 'resume-then-slash-fork',
      modelFlag: '--model',
      permissionFlags: {
        default: [],
        auto: ['--mode', 'auto'],
        acceptEdits: ['--mode', 'accept-edits'],
        plan: ['--mode', 'plan'],
        bypassPermissions: ['--dangerously-skip-permissions']
      }
    },
    events: {
      lifecycleSource: 'plugin-command-hook',
      statusLineSource: 'settings-command',
      identitySource: 'hook-or-statusline',
      transcriptPathSource: 'hook',
      forkIdentityRule: 'differs-from-parent-after-slash-fork'
    },
    mtInjection: {
      mode: 'session-start-system-message',
      assetRelativePath: 'providers/antigravity-system-message.md'
    },
    instructionFiles: {
      candidates: ['AGENTS.md', 'GEMINI.md'],
      buttonLabel: 'AGENTS.md'
    },
    hud: {
      stripClaudeModelPrefix: false,
      hideEmptyQuotaPlaceholder: true,
      supportsTodos: false,
      supportsAgentsCount: false,
      supportsMcpSummary: false,
      supportsHooksCount: false,
      supportsQuotaWindows: true
    },
    capabilities: {
      fork: true,
      sessionLoader: true,
      autoTitle: true,
      permissionModes: ['default', 'acceptEdits', 'plan', 'bypassPermissions'],
      permissionSwitchMode: 'shift-tab-hybrid',
      providerConfig: false,
      accountSwitching: false,
      agentTeam: false,
      claudeTimeZone: false,
      autoRestore: true,
      forkRequiresCompletedTurn: true
    }
  }
}

export function isAgentProfileId(value: unknown): value is AgentProfileId {
  return typeof value === 'string' && value in AGENT_MANIFESTS
}

export function isSessionRunProfile(value: unknown): value is SessionRunProfile {
  return value === 'shell' || isAgentProfileId(value)
}

export function isSessionKind(value: unknown): value is SessionKind {
  return value === 'shell' || value === 'agent-team-member' || isAgentProfileId(value)
}

export function isAgentProviderId(value: unknown): value is AgentProviderId {
  return value === 'generic' || isAgentProfileId(value)
}

export function getAgentManifest(value: unknown): AgentManifest | undefined {
  return isAgentProfileId(value) ? AGENT_MANIFESTS[value] : undefined
}

export function listAgentManifests(): readonly AgentManifest[] {
  return LAUNCHABLE_AGENT_PROFILES.map((id) => AGENT_MANIFESTS[id])
}

export function getAgentDisplayName(kind: unknown, fallback = 'Shell'): string {
  return getAgentManifest(kind)?.displayName ?? fallback
}

export function getAgentShortName(kind: unknown, fallback = 'Shell'): string {
  return getAgentManifest(kind)?.shortName ?? fallback
}
