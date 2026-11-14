import type { SubscriptionAccountDetails, SubscriptionModel, SubscriptionProvider } from '@matou/contracts'

type Mapping = Record<'opus' | 'sonnet' | 'haiku', string>
/**
 * Image/video generators and the non-conversational review helper can appear in
 * an account's raw model list, but they cannot back a Claude Code role.
 */
export function isUsableSubscriptionModel(model: SubscriptionModel): boolean {
  return !/image|video|codex-auto-review/i.test(model.id)
}
export function usableSubscriptionModels(models: SubscriptionModel[]): SubscriptionModel[] {
  return models.filter(isUsableSubscriptionModel)
}
export function subscriptionDefaults(provider: SubscriptionProvider, models: SubscriptionModel[], saved?: Partial<Mapping>): Mapping {
  models = usableSubscriptionModels(models)
  const first = models[0]?.id ?? ''
  const choose = (role: keyof Mapping) => {
    if (saved?.[role] && models.some(model => model.id === saved[role])) return saved[role]!
    const preferred = provider === 'codex'
      ? models.find(model => model.id === ({ opus: 'gpt-6-astra', sonnet: 'gpt-6.1-sol', haiku: 'gpt-6-luna' }[role]))
      : models.find(model => {
        const label = `${model.id} ${model.displayName ?? ''}`.toLowerCase()
        return role === 'opus' ? /claude.*opus.*thinking/.test(label) : role === 'sonnet' ? /claude.*sonnet/.test(label) : /gemini.*flash/.test(label)
      })
    const opusFallback = provider === 'antigravity' && role === 'opus' ? models.find(model => /claude.*opus/i.test(`${model.id} ${model.displayName ?? ''}`)) : undefined
    return preferred?.id ?? opusFallback?.id ?? first
  }
  return { opus: choose('opus'), sonnet: choose('sonnet'), haiku: choose('haiku') }
}

/** Settings owns the only quota read. This function only formats its snapshot. */
export function subscriptionQuotaText(provider: SubscriptionProvider, details: SubscriptionAccountDetails | undefined, zh: boolean): string {
  const quota = details?.quota?.filter(window => Number.isFinite(window.usedPercent))
  if (!quota?.length) return ''
  const percent = (value: number) => Math.round(Math.max(0, Math.min(100, value)))
  if (provider === 'codex') {
    return [quota.find(window => window.name === 'five_hour' || window.windowSeconds === 18000), quota.find(window => window.name === 'weekly' || window.windowSeconds === 604800)]
      .flatMap((window, index) => window ? [`${index === 0 ? zh ? '5 小时' : '5h' : zh ? '每周' : 'Weekly'} ${percent(window.usedPercent)}%`] : []).join(' · ')
  }
  const groups = new Map<string, number>()
  for (const window of quota) {
    const group = window.group ?? window.name
    const name = /claude/i.test(group) ? 'Claude' : group === 'gemini' ? 'Gemini' : group === 'gemini-argon' ? 'Gemini Argon' : group
    // Several windows can constrain the same group; show its available budget.
    groups.set(name, Math.max(groups.get(name) ?? 0, window.usedPercent))
  }
  return [...groups].sort(([a], [b]) => a === 'Claude' ? -1 : b === 'Claude' ? 1 : 0).map(([name, used]) => zh ? `${name} 剩余 ${percent(100 - used)}%` : `${name} ${percent(100 - used)}% left`).join(' · ')
}

export function subscriptionQuotaTitle(details: SubscriptionAccountDetails | undefined, zh: boolean): string {
  return (details?.quota ?? []).filter(window => Number.isFinite(window.usedPercent)).map(window => {
    const name = window.name === 'five_hour' ? zh ? '5 小时' : '5h' : window.name === 'weekly' ? zh ? '每周' : 'Weekly' : window.name
    const left = Math.round(Math.max(0, Math.min(100, 100 - window.usedPercent)))
    const remaining = zh ? `剩余 ${left}%` : `${left}% left`
    const reset = window.resetsAt ? `${zh ? '重置' : 'Resets'}: ${window.resetsAt}` : ''
    return [name, remaining, reset].filter(Boolean).join(' · ')
  }).join('\n')
}
