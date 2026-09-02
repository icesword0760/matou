import { execFile } from 'node:child_process'
import { cp, mkdir, rm } from 'node:fs/promises'
import { join } from 'node:path'
import { promisify } from 'node:util'

const execFileAsync = promisify(execFile)

export interface FinderQuickActionInstallOptions {
  platform: NodeJS.Platform
  sourcePath: string
  homeDirectory: string
  refreshServices?: () => Promise<void>
}

export async function installFinderQuickAction(
  options: FinderQuickActionInstallOptions
): Promise<string | undefined> {
  if (options.platform !== 'darwin') return undefined
  const servicesDirectory = join(options.homeDirectory, 'Library', 'Services')
  const destination = join(servicesDirectory, '进入码头.workflow')
  await mkdir(servicesDirectory, { recursive: true })
  await rm(destination, { recursive: true, force: true })
  await cp(options.sourcePath, destination, { recursive: true })
  await (options.refreshServices ?? refreshMacServices)()
  return destination
}

async function refreshMacServices(): Promise<void> {
  await execFileAsync('/System/Library/CoreServices/pbs', ['-update']).then(() => undefined)
}
