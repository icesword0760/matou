import type { ContextWindowMode, ModelContextChoice, ProviderConfigView } from '@matou/contracts'
import { CLI_DEFAULT_MODEL } from '@matou/contracts'

import { useLocale } from '../i18n/LocaleProvider'

export const CONTEXT_WINDOW_MODES: readonly ContextWindowMode[] = ['default', '512k', 'maximum']
export const TARGET_512K_TOKENS = 512_000

export interface ResolvedWindow {
  tokens: number | undefined
  status: 'fixed' | 'known' | 'unknown'
  multiModel?: boolean
}

/**
 * Mirrors runtime window resolution so the card badge and provider edit modal
 * display the exact token budget that new or resumed sessions will request.
 */
export function resolveProviderWindow(
  mode: ContextWindowMode,
  models: readonly ModelContextChoice[] = [],
  selectedModelIds: readonly string[] = []
): ResolvedWindow {
  const uniqueIds = [...new Set(selectedModelIds.map(id => id.trim()).filter(id => Boolean(id) && id !== CLI_DEFAULT_MODEL))]
  const multiModel = uniqueIds.length > 1
  if (mode === '512k') {
    return { tokens: TARGET_512K_TOKENS, status: 'fixed', multiModel }
  }
  const pickTokens = (model: ModelContextChoice | undefined) =>
    mode === 'maximum' ? model?.maximumTokens ?? model?.defaultTokens : model?.defaultTokens

  if (uniqueIds.length > 0) {
    const resolved = uniqueIds.map(id => pickTokens(models.find(model => model.id === id)))
    if (resolved.every((value): value is number => typeof value === 'number' && value > 0)) {
      return { tokens: Math.min(...resolved), status: 'known', multiModel }
    }
    return { tokens: undefined, status: 'unknown', multiModel }
  }

  const fallback = models.find(model => model.id === 'default') ?? models.find(model => model.id === 'opus') ?? models[0]
  const fallbackTokens = pickTokens(fallback)
  return typeof fallbackTokens === 'number' && fallbackTokens > 0
    ? { tokens: fallbackTokens, status: 'known', multiModel }
    : { tokens: undefined, status: 'unknown', multiModel }
}

export function providerRoleModelIds(provider: Pick<ProviderConfigView, 'model' | 'modelMapping' | 'kind'>): string[] {
  const mapped = Object.values(provider.modelMapping ?? {}).filter((value): value is string => Boolean(value && value.trim()))
  if (provider.kind === 'subscription') return mapped
  return [provider.model, ...mapped].filter((value): value is string => Boolean(value && value.trim() && value !== CLI_DEFAULT_MODEL))
}

export function formatCompactTokens(tokens: number): string {
  if (tokens % 1_000_000 === 0) return `${tokens / 1_000_000}M`
  if (tokens % 1_000 === 0) return `${tokens / 1_000}K`
  return tokens.toLocaleString('en-US')
}

export function formatContextWindowBadge(
  mode: ContextWindowMode,
  models: readonly ModelContextChoice[] = [],
  selectedModelIds: readonly string[] = [],
  zh = true
): string {
  const resolved = resolveProviderWindow(mode, models, selectedModelIds)
  if (mode === '512k') return zh ? '512K 窗口' : '512K window'
  if (mode === 'default') {
    if (resolved.tokens) return zh ? `默认 ${formatCompactTokens(resolved.tokens)} 窗口` : `Default ${formatCompactTokens(resolved.tokens)}`
    return zh ? '默认窗口' : 'Default window'
  }
  if (resolved.tokens) return zh ? `最大 ${formatCompactTokens(resolved.tokens)} 窗口` : `Max ${formatCompactTokens(resolved.tokens)}`
  return zh ? '最大可扩展' : 'Max extensible'
}

export function ContextWindowPicker({
  value,
  models = [],
  selectedModelIds = [],
  disabled = false,
  onChange
}: {
  value: ContextWindowMode
  models?: readonly ModelContextChoice[] | undefined
  selectedModelIds?: readonly string[] | undefined
  disabled?: boolean
  onChange(mode: ContextWindowMode): void
}) {
  const zh = useLocale() === 'zh-CN'
  const t = (cn: string, en: string) => zh ? cn : en
  const multiModel = [...new Set(selectedModelIds.map(id => id.trim()).filter(id => Boolean(id) && id !== CLI_DEFAULT_MODEL))].length > 1

  const titleFor = (mode: ContextWindowMode) => mode === 'default'
    ? t('默认窗口', 'Default')
    : mode === '512k'
      ? '512K'
      : t('最大可扩展', 'Max extensible')

  const subtitleFor = (mode: ContextWindowMode) => mode === 'default'
    ? t('沿用模型与 CLI 默认', 'Model & CLI default')
    : mode === '512k'
      ? t('固定请求；需通道支持', 'Fixed request; channel-dependent')
      : t('使用模型已知最大上限', 'Largest known model window')

  const tokenTextFor = (mode: ContextWindowMode) => {
    const resolved = resolveProviderWindow(mode, models, selectedModelIds)
    if (resolved.tokens !== undefined) return `${resolved.tokens.toLocaleString('en-US')} tokens`
    return mode === 'maximum'
      ? t('未提供（沿用默认）', 'Not provided (uses default)')
      : t('由 CLI 决定', 'Decided by CLI')
  }

  return <fieldset className="provider-context-picker">
    <legend>{t('上下文窗口长度', 'Context window')}</legend>
    <div className="provider-context-picker__options">
      {CONTEXT_WINDOW_MODES.map(mode => {
        const selected = value === mode
        return <label key={mode} className={`provider-context-picker__option${selected ? ' is-selected' : ''}`}>
          <input
            type="radio"
            name="provider-context-window"
            value={mode}
            checked={selected}
            disabled={disabled}
            onChange={() => onChange(mode)}
          />
          <span className="provider-context-picker__content">
            <strong>{titleFor(mode)}</strong>
            <b className="provider-context-picker__tokens">{tokenTextFor(mode)}</b>
            <small>{subtitleFor(mode)}</small>
          </span>
        </label>
      })}
    </div>
    {multiModel && <p className="provider-help">{t(
      '映射多个模型时，仅当所有模型都有窗口数据才取最小值；任一未知时保留 CLI 默认。',
      'When multiple models are mapped, the smallest known window applies only if all models provide data.'
    )}</p>}
  </fieldset>
}
