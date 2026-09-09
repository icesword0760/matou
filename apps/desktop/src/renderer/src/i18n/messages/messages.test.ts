import { describe, expect, it } from 'vitest'

import { en, zhCN } from './index'

const keys = (value: unknown, prefix = ''): string[] =>
  value && typeof value === 'object'
    ? Object.entries(value).flatMap(([k, v]) => keys(v, `${prefix}${k}.`))
    : [prefix.slice(0, -1)]

describe('renderer catalogs', () => {
  it('have the same keys in both locales', () => {
    expect(keys(en).sort()).toEqual(keys(zhCN).sort())
  })
})
