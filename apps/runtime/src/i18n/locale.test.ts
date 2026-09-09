import { afterEach, describe, expect, it } from 'vitest'

import { resetRuntimeLocaleForTests, resolveRuntimeLocale, runtimeLocale } from './locale'
import { runtimeMessages } from './messages'

describe('runtime locale', () => {
  const original = process.env.MATOU_LOCALE
  afterEach(() => { process.env.MATOU_LOCALE = original; resetRuntimeLocaleForTests() })

  it('defaults to zh-CN and accepts only known locales', () => {
    expect(resolveRuntimeLocale(undefined)).toBe('zh-CN')
    expect(resolveRuntimeLocale('en')).toBe('en')
    expect(resolveRuntimeLocale('fr')).toBe('zh-CN')
  })

  it('reads MATOU_LOCALE once', () => {
    process.env.MATOU_LOCALE = 'en'
    resetRuntimeLocaleForTests()
    expect(runtimeLocale()).toBe('en')
    expect(runtimeMessages().server.branchReady).toBe('Branch ready')
  })
})
