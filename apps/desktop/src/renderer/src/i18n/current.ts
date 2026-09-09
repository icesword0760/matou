import { DEFAULT_LOCALE, type Locale } from '@matou/contracts'

import { MESSAGES, type Messages } from './messages'

let locale: Locale = DEFAULT_LOCALE
const listeners = new Set<(locale: Locale) => void>()

export function currentLocale(): Locale { return locale }
export function setCurrentLocale(next: Locale): void {
  if (next === locale) return
  locale = next
  for (const listener of listeners) listener(next)
}
export function onCurrentLocaleChange(listener: (locale: Locale) => void): () => void {
  listeners.add(listener)
  return () => { listeners.delete(listener) }
}
/** For code outside React (command builders, notification text). Components use `useMessages()`. */
export function messages(): Messages { return MESSAGES[locale] }
