import { mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { dirname } from 'node:path'

import { isLocalePreference, resolveLocale, type Locale, type LocalePreference } from '@matou/contracts'

export interface LocaleStoreOptions {
  /** JSON file holding `{ "preference": "system" | "zh-CN" | "en" }`. */
  file: string
  /** Returns the OS locale, e.g. `app.getLocale()`. */
  system: () => string
  /** `process.env.MATOU_LOCALE`; wins over everything when it is a valid locale. */
  env?: string | undefined
}

export class LocaleStore {
  readonly #listeners = new Set<(locale: Locale) => void>()
  constructor(private readonly options: LocaleStoreOptions) {}

  preference(): LocalePreference {
    try {
      const parsed: unknown = JSON.parse(readFileSync(this.options.file, 'utf8'))
      const value = (parsed as { preference?: unknown } | null)?.preference
      return isLocalePreference(value) ? value : 'system'
    } catch {
      return 'system'
    }
  }

  current(): Locale {
    return resolveLocale({ env: this.options.env, preference: this.preference(), system: this.options.system() })
  }

  set(preference: LocalePreference): Locale {
    mkdirSync(dirname(this.options.file), { recursive: true })
    writeFileSync(this.options.file, JSON.stringify({ preference }))
    const locale = this.current()
    for (const listener of this.#listeners) listener(locale)
    return locale
  }

  onChange(listener: (locale: Locale) => void): () => void {
    this.#listeners.add(listener)
    return () => { this.#listeners.delete(listener) }
  }
}
