import { describe, expect, it } from 'vitest'

import { resolvePackagedApplication } from './app-environment'

describe('application environment', () => {
  it('treats the localized development bundle as development', () => {
    expect(resolvePackagedApplication({ electronPackaged: true, developmentBundle: '1' })).toBe(false)
  })

  it('keeps ordinary packaged and development launches unchanged', () => {
    expect(resolvePackagedApplication({ electronPackaged: true })).toBe(true)
    expect(resolvePackagedApplication({ electronPackaged: false })).toBe(false)
  })
})
