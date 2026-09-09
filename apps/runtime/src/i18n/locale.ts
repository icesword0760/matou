import { DEFAULT_LOCALE, isLocale, type Locale } from '@matou/contracts'

let resolved: Locale | undefined

export function resolveRuntimeLocale(value: string | undefined): Locale {
  return isLocale(value) ? value : DEFAULT_LOCALE
}

/** Read once at process start; the main process passes MATOU_LOCALE when it forks the runtime. */
export function runtimeLocale(): Locale {
  resolved ??= resolveRuntimeLocale(process.env.MATOU_LOCALE)
  return resolved
}

export function resetRuntimeLocaleForTests(): void {
  resolved = undefined
}
