export const LOCALES = ['zh-CN', 'en'] as const
export type Locale = (typeof LOCALES)[number]
export type LocalePreference = Locale | 'system'
export const DEFAULT_LOCALE: Locale = 'zh-CN'

export function isLocale(value: unknown): value is Locale {
  return typeof value === 'string' && (LOCALES as readonly string[]).includes(value)
}

export function isLocalePreference(value: unknown): value is LocalePreference {
  return value === 'system' || isLocale(value)
}

/** Any Chinese variant keeps the Chinese UI; every other system language gets English. */
export function localeFromSystem(system: string | null | undefined): Locale {
  if (!system) return DEFAULT_LOCALE
  return system.toLowerCase().startsWith('zh') ? 'zh-CN' : 'en'
}

export function resolveLocale(input: {
  env?: string | undefined
  preference?: LocalePreference | undefined
  system?: string | null | undefined
}): Locale {
  if (isLocale(input.env)) return input.env
  if (isLocale(input.preference)) return input.preference
  return localeFromSystem(input.system)
}
