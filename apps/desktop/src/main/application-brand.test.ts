import { describe, expect, it, vi } from 'vitest'

import { applyApplicationBrand } from './application-brand'

describe('application brand', () => {
  it('sets the running application name before the Dock registers it', () => {
    const setName = vi.fn()

    applyApplicationBrand({ setName }, '码头')

    expect(setName).toHaveBeenCalledWith('码头')
  })
})
