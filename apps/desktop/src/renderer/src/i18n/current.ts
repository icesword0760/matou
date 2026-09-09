import { DEFAULT_LOCALE, type Locale } from '@matou/contracts'

import { MESSAGES, type Messages } from './messages'

let locale: Locale = DEFAULT_LOCALE

export function currentLocale(): Locale { return locale }
export function setCurrentLocale(next: Locale): void { locale = next }
/** For code outside React (command builders, notification text). Components use `useMessages()`. */
export function messages(): Messages { return MESSAGES[locale] }
