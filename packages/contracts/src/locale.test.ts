import { describe, expect, it } from 'vitest'

import { DEFAULT_LOCALE, isLocale, localeFromSystem, resolveLocale } from './locale'

describe('locale', () => {
  it('accepts only the supported locales', () => {
    expect(isLocale('zh-CN')).toBe(true)
    expect(isLocale('en')).toBe(true)
    expect(isLocale('en-US')).toBe(false)
    expect(isLocale(undefined)).toBe(false)
  })

  it('maps any Chinese system locale to zh-CN and everything else to en', () => {
    expect(localeFromSystem('zh-CN')).toBe('zh-CN')
    expect(localeFromSystem('zh-Hant-TW')).toBe('zh-CN')
    expect(localeFromSystem('en-US')).toBe('en')
    expect(localeFromSystem('ja')).toBe('en')
    expect(localeFromSystem(undefined)).toBe(DEFAULT_LOCALE)
  })

  it('resolves env, then preference, then system', () => {
    expect(resolveLocale({ env: 'en', preference: 'zh-CN', system: 'zh-CN' })).toBe('en')
    expect(resolveLocale({ env: 'nope', preference: 'en', system: 'zh-CN' })).toBe('en')
    expect(resolveLocale({ preference: 'system', system: 'fr-FR' })).toBe('en')
    expect(resolveLocale({})).toBe('zh-CN')
  })
})
