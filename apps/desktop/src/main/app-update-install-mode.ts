import type { AppUpdateInstallMode } from '../shared/desktop-api'

interface SignatureInspection {
  status: number | null
  output: string
}

interface ResolveAppUpdateInstallModeInput {
  platform: NodeJS.Platform
  isPackaged: boolean
  inspectSignature: () => SignatureInspection
}

export function resolveAppUpdateInstallMode(
  input: ResolveAppUpdateInstallModeInput
): AppUpdateInstallMode {
  if (input.platform !== 'darwin' || !input.isPackaged) return 'automatic'
  const result = input.inspectSignature()
  if (result.status !== 0) return 'manual'
  const hasDistributionAuthority = /^Authority=.+$/m.test(result.output)
  const teamIdentifier = result.output.match(/^TeamIdentifier=(.+)$/m)?.[1]?.trim()
  return hasDistributionAuthority && teamIdentifier && teamIdentifier !== 'not set'
    ? 'automatic'
    : 'manual'
}
