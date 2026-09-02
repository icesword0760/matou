import { readFileSync } from 'node:fs'

import { describe, expect, it } from 'vitest'

import { APP_DISPLAY_NAME } from './brand'

describe('application branding', () => {
  it('uses the Chinese product name in the packaged application', () => {
    const packageJson = JSON.parse(readFileSync(new URL('../../package.json', import.meta.url), 'utf8')) as {
      productName?: string
    }

    expect(APP_DISPLAY_NAME).toBe('码头')
    expect(packageJson.productName).toBe(APP_DISPLAY_NAME)
  })
})
