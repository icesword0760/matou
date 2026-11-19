import { CLI_DEFAULT_MODEL, type ProviderConfigView, type SubscriptionAccountDetails } from '@matou/contracts'

import { useLocale, useMessages } from '../i18n/LocaleProvider'

/**
 * Subscription account card. It shares the API-key card material and keeps the
 * card body at a stable minimum height so the quota line (read once when the
 * settings pane opens) never moves the actions when it arrives.
 */
export function SubscriptionProviderCard({ provider, details, quotaText, quotaTitle, windowBadge, current, switching, activating = false, subscriptionSupported = true, refreshingModels, refreshModelsDisabled, onRefreshModels, onEdit, onSignIn, onActivate, onUseOfficial }: {
  provider: ProviderConfigView
  details?: SubscriptionAccountDetails | undefined
  quotaText: string
  quotaTitle?: string | undefined
  windowBadge?: string | undefined
  current: boolean
  switching: boolean
  activating?: boolean
  subscriptionSupported?: boolean
  refreshingModels: boolean
  refreshModelsDisabled: boolean
  onRefreshModels(): void
  onEdit(): void
  onSignIn(): void
  onActivate(): void
  onUseOfficial?: (() => void) | undefined
}) {
  const zh = useLocale() === 'zh-CN'
  const t = (cn: string, en: string) => zh ? cn : en
  const m = useMessages().hierarchyShell.modelSettings
  const subscription = provider.subscription
  const account = details?.account ?? subscription?.account
  const status = account?.status ?? 'ready'
  const needsSignIn = status === 'needs-sign-in'
  const accountLine = [account?.email, account?.plan].filter(Boolean).join(' · ')
  const metadata = details?.models ?? subscription?.modelMetadata ?? []
  const opusId = provider.modelMapping?.opus
  const opus = opusId ? metadata.find(model => model.id === opusId) : undefined
  const modelName = opus?.displayName || opusId || (provider.model === CLI_DEFAULT_MODEL ? m.cliDefaultModel : provider.model)
  const missingOpus = Boolean(opusId && metadata.length && !opus)
  const modelListTitle = metadata.map(model => model.displayName || model.id).join('\n') || undefined
  const refreshDisabledReason = needsSignIn ? m.refreshModelsSignInRequired
    : !subscriptionSupported ? m.refreshModelsUnsupported : undefined
  const statusLabel = status === 'needs-sign-in' ? t('需要重新登录', 'Sign in again')
    : status === 'account-unavailable' ? t('账号不可用', 'Account unavailable')
      : status === 'router-not-running' ? t('模型路由未运行', 'Model router is not running') : ''
  const initial = (provider.subscription?.provider === 'antigravity' ? 'A' : 'C')

  return <article className={`model-provider model-provider--subscription${current ? ' is-current' : ''}${status === 'account-unavailable' ? ' is-account-unavailable' : ''}`}>
    <span className={`model-provider__logo tone-${subscription?.provider === 'codex' ? 'openai' : 'custom'}`}>{initial}</span>
    <div className="model-provider__name">
      <strong>{provider.name}{current && <i>{m.inUse}</i>}{windowBadge && <i className="model-provider__window-badge">{windowBadge}</i>}{statusLabel && <i className="model-provider__status">{statusLabel}</i>}</strong>
      {accountLine && <span className="model-provider__account">{accountLine}</span>}
      {quotaText && <span className="model-provider__quota" title={quotaTitle}>{quotaText}</span>}
      {status === 'account-unavailable' && account?.reason && <span className="model-provider__reason">{account.reason}</span>}
      {status === 'account-unavailable' && current && <button type="button" className="provider-link model-provider__official" disabled={switching || !onUseOfficial} onClick={() => onUseOfficial?.()}>{t('切回 Anthropic 官方', 'Switch back to Anthropic official')}</button>}
    </div>
    <div className="model-provider__model" title={modelListTitle}>
      <strong>{modelName}</strong>
      <span>{m.defaultModel}{missingOpus && ` · ${m.modelUnavailable}`}</span>
    </div>
    <div className="model-provider__actions">
      <span title={refreshDisabledReason}>
        <button type="button" className="model-provider__refresh" onClick={onRefreshModels}
          disabled={switching || refreshingModels || refreshModelsDisabled || Boolean(refreshDisabledReason)}
          aria-description={refreshDisabledReason} aria-busy={refreshingModels}>
          {refreshingModels ? m.refreshingModels : m.refreshModels}
        </button>
      </span>
      <button type="button" disabled={switching} onClick={onEdit}>{m.edit}</button>
      <button className="primary" type="button"
        disabled={switching || !subscriptionSupported || (current && !needsSignIn)}
        aria-label={needsSignIn ? t(`重新登录 ${provider.name}`, `Sign in again for ${provider.name}`) : current ? m.currentConfigurationOf(provider.name) : m.switchTo(provider.name)}
        onClick={needsSignIn ? onSignIn : onActivate}>
        {needsSignIn ? t('重新登录', 'Sign in again') : activating ? t('切换中…', 'Switching…') : current ? t('默认使用', 'Default') : m.switchAction}
      </button>
    </div>
  </article>
}
