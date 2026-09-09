import { describe, expect, it } from 'vitest'

import { en, zhCN } from './messages'

const keys = (value: unknown, prefix = ''): string[] =>
  value && typeof value === 'object'
    ? Object.entries(value).flatMap(([k, v]) => keys(v, `${prefix}${k}.`))
    : [prefix.slice(0, -1)]

describe('runtime catalogs', () => {
  it('have the same keys in both locales', () => {
    expect(keys(en).sort()).toEqual(keys(zhCN).sort())
  })

  it.each([['zh-CN', zhCN], ['en', en]] as const)(
    'keeps the fork batch mismatch fragment inside the full message in %s',
    (_locale, catalog) => {
      expect(catalog.control.forkBatch.inputMismatch('X'))
        .toContain(catalog.control.forkBatch.inputMismatchFragment)
    }
  )
})
