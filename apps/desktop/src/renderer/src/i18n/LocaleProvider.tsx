import type { Locale } from '@matou/contracts'
import { createContext, useContext, useEffect, useState, type ReactNode } from 'react'

import { currentLocale, setCurrentLocale } from './current'
import { MESSAGES, type Messages } from './messages'

const LocaleContext = createContext<Locale>(currentLocale())

export function LocaleProvider({ children, initialLocale }: { children: ReactNode; initialLocale?: Locale }) {
  const [locale, setLocale] = useState<Locale>(() => {
    if (initialLocale) setCurrentLocale(initialLocale)
    return currentLocale()
  })
  useEffect(() => {
    const bridge = window.matouDesktop as Partial<typeof window.matouDesktop> | undefined
    if (!bridge?.getLocale || !bridge.onLocaleChanged) return
    let active = true
    const apply = (next: Locale) => { if (!active) return; setCurrentLocale(next); setLocale(next) }
    void bridge.getLocale().then(apply)
    const unsubscribe = bridge.onLocaleChanged(apply)
    return () => { active = false; unsubscribe() }
  }, [])
  useEffect(() => { document.documentElement.lang = locale }, [locale])
  return <LocaleContext.Provider value={locale}>{children}</LocaleContext.Provider>
}

export function useLocale(): Locale { return useContext(LocaleContext) }
export function useMessages(): Messages { return MESSAGES[useLocale()] }
