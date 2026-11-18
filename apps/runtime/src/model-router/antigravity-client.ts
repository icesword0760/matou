import { readFile } from 'node:fs/promises'
import os from 'node:os'
import { join } from 'node:path'
import { compareSemver, parseSemver } from './codex-client'
import { readCredentialJson } from './credential-file'

const ANTIGRAVITY_FALLBACK_VERSION = '2.29.0'
const ANTIGRAVITY_FALLBACK_CL = '1013060670'

function antigravityArch(): string { return process.arch === 'x64' ? 'x86_64' : 'arm64' }

export function antigravityClientUserAgentForVersion(version: string, changelist?: string): string {
  const cl = changelist ? `; cl=${changelist}` : ''
  return `antigravity/hub/${version} (aidev_client; os_type=darwin; arch=${antigravityArch()}${cl})`
}

/** One OAuth token must look like one client.
 *
 * Antigravity runtime requests (inference, quota and model probes, settings
 * reads) are all issued with the same Hub user agent. Matou's own product name
 * never appears on provider traffic. The verified fallback release is mirrored
 * here and written onto the credential so probes and inference share one token
 * identity.
 */
export const ANTIGRAVITY_CLIENT_USER_AGENT = antigravityClientUserAgentForVersion(
  ANTIGRAVITY_FALLBACK_VERSION,
  ANTIGRAVITY_FALLBACK_CL
)

function defaultPlistPaths(): string[] {
  return [
    '/Applications/Antigravity.app/Contents/Info.plist',
    join(os.homedir(), 'Applications/Antigravity.app/Contents/Info.plist')
  ]
}

async function detectLocalAntigravityVersion(plistPaths = defaultPlistPaths()): Promise<string | undefined> {
  if (process.platform !== 'darwin' && plistPaths === defaultPlistPaths()) return undefined
  for (const plistPath of plistPaths) {
    try {
      const xml = await readFile(plistPath, 'utf8')
      const match = /<key>CFBundleShortVersionString<\/key>\s*<string>([^<]+)<\/string>/.exec(xml)
      const version = match?.[1] ? parseSemver(match[1]) : undefined
      if (version) return version
    } catch { /* App not installed at this path. */ }
  }
  return undefined
}

let cachedAntigravityAgent: Promise<string> | undefined

/** Follows the locally installed Antigravity.app release when newer than the
 * verified fallback floor, without inventing a changelist for unknown builds. */
export function resolveAntigravityClientUserAgent(options?: {
  plistPaths?: string[]
  detectVersion?: () => Promise<string | undefined>
}): Promise<string> {
  const compute = async (): Promise<string> => {
    const detected = await (options?.detectVersion ?? (() => detectLocalAntigravityVersion(options?.plistPaths)))()
    if (detected && compareSemver(detected, ANTIGRAVITY_FALLBACK_VERSION) > 0) {
      return antigravityClientUserAgentForVersion(detected)
    }
    return ANTIGRAVITY_CLIENT_USER_AGENT
  }
  if (options?.plistPaths || options?.detectVersion) return compute()
  cachedAntigravityAgent ??= compute()
  return cachedAntigravityAgent
}

/** True when the credential file already carries the expected shared agent.
 * Reading first keeps repeated launches from rewriting an identical value and
 * waking the engine watcher. */
export async function credentialCarriesAntigravityIdentity(
  dataRoot: string,
  file: { name: string; path?: unknown },
  expectedUserAgent = ANTIGRAVITY_CLIENT_USER_AGENT
): Promise<boolean> {
  const credential = await readCredentialJson(dataRoot, file)
  return credential?.user_agent === expectedUserAgent
}
