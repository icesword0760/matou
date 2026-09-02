import { describe, expect, it, vi } from 'vitest'

import { resolveAppUpdateInstallMode } from './app-update-install-mode'

describe('resolveAppUpdateInstallMode', () => {
  it('uses manual DMG installation for an ad-hoc signed macOS bundle', () => {
    expect(resolveAppUpdateInstallMode({
      platform: 'darwin', isPackaged: true,
      inspectSignature: vi.fn(() => ({
        status: 0,
        output: 'Signature=adhoc\nTeamIdentifier=not set'
      }))
    })).toBe('manual')
  })

  it('keeps native automatic installation for a Developer ID signed macOS bundle', () => {
    expect(resolveAppUpdateInstallMode({
      platform: 'darwin', isPackaged: true,
      inspectSignature: vi.fn(() => ({
        status: 0,
        output: 'Authority=Developer ID Application: Matou (TEAM123456)\nTeamIdentifier=TEAM123456'
      }))
    })).toBe('automatic')
  })

  it('does not inspect signatures on other platforms', () => {
    const inspectSignature = vi.fn(() => ({ status: 1, output: '' }))
    expect(resolveAppUpdateInstallMode({
      platform: 'win32', isPackaged: true, inspectSignature
    })).toBe('automatic')
    expect(inspectSignature).not.toHaveBeenCalled()
  })
})
