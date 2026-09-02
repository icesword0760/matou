import { join } from 'node:path'

interface DockIconTarget {
  setIcon(path: string): unknown
}

export function installDevelopmentDockIcon(input: {
  platform: NodeJS.Platform
  isPackaged: boolean
  appPath: string
  dock: DockIconTarget | undefined
}): string | undefined {
  if (input.platform !== 'darwin' || input.isPackaged || !input.dock) return undefined
  const iconPath = join(input.appPath, 'build', 'icon.png')
  input.dock.setIcon(iconPath)
  return iconPath
}
