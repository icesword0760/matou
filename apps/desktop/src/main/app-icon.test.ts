import { describe, expect, it, vi } from 'vitest'

import { installDevelopmentDockIcon } from './app-icon'

describe('development Dock icon', () => {
  it('uses the Matou icon when Electron runs directly on macOS', () => {
    const setIcon = vi.fn()

    const iconPath = installDevelopmentDockIcon({
      platform: 'darwin', isPackaged: false, appPath: '/fixture/apps/desktop',
      dock: { setIcon }
    })

    expect(iconPath).toBe('/fixture/apps/desktop/build/icon.png')
    expect(setIcon).toHaveBeenCalledWith('/fixture/apps/desktop/build/icon.png')
  })

  it('leaves packaged and non-macOS application icons to the platform bundle', () => {
    const setIcon = vi.fn()

    expect(installDevelopmentDockIcon({
      platform: 'darwin', isPackaged: true, appPath: '/fixture/apps/desktop', dock: { setIcon }
    })).toBeUndefined()
    expect(installDevelopmentDockIcon({
      platform: 'linux', isPackaged: false, appPath: '/fixture/apps/desktop', dock: { setIcon }
    })).toBeUndefined()
    expect(setIcon).not.toHaveBeenCalled()
  })
})
