export type ProviderCli = 'claude-code' | 'codex'

export interface ProviderConfigView {
  id: string
  cli: ProviderCli
  name: string
  endpoint: string
  model: string
  hasApiKey: boolean
  builtIn?: boolean
}

export interface ProviderConfigSnapshot {
  revision: number
  providers: Record<ProviderCli, ProviderConfigView[]>
  activeProviderIds: Record<ProviderCli, string>
}

export type ProviderSessionActivationDeferredReason =
  | 'durability-fault'
  | 'recovery-not-ready'
  | 'provider-identity-pending'
  | 'session-not-running'
  | 'restart-unavailable'

export type ProviderSessionActivationTransition =
  | { sessionId: string; status: 'updated' }
  | {
      sessionId: string
      status: 'deferred'
      reason: ProviderSessionActivationDeferredReason
    }

export interface ProviderConfigActivationResult extends ProviderConfigSnapshot {
  sessionTransitions: ProviderSessionActivationTransition[]
}

export interface ProviderConfigInput {
  id?: string
  cli: ProviderCli
  name: string
  endpoint: string
  model: string
  apiKey?: string
}
