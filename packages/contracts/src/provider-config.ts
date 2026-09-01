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

export interface ProviderConfigInput {
  id?: string
  cli: ProviderCli
  name: string
  endpoint: string
  model: string
  apiKey?: string
}
