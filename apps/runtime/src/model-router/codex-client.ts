import { execFile, execFileSync } from 'node:child_process'
import { promisify } from 'node:util'
import { resolveProviderCommandEnvironment } from '../session/provider-command-environment'
import { readCredentialJson } from './credential-file'

const execFileAsync = promisify(execFile)

/** One ChatGPT subscription token must look like one Codex client.
 *
 * The installed native Codex CLI identifies itself as `codex_exec` with a
 * versioned user agent and no `Version` header. Matou's probes and the router's
 * inference path share this token, so both sides send the same values. The
 * router config pins the user agent for inference; credential headers pin the
 * Originator because the client (Claude Code) does not send Codex values.
 *
 * The OS product version comes from `sw_vers`, matching what the native binary
 * reports on this machine. 0.162.1 is the verified fallback floor when the
 * local Codex CLI is absent or older.
 */
export const CODEX_CLIENT_VERSION = '0.162.1'
export const CODEX_CLIENT_ORIGINATOR = 'codex-tui'

export interface CodexClientIdentity {
  version: string
  userAgent: string
}

function macOSProductVersion(): string {
  try {
    const value = execFileSync('/usr/bin/sw_vers', ['-productVersion'], { encoding: 'utf8', timeout: 2_000 }).trim()
    return /^\d+(?:\.\d+){0,2}$/.test(value) ? value : '15.0.0'
  } catch { return '15.0.0' }
}

function clientArch(): string { return process.arch === 'x64' ? 'x86_64' : 'arm64' }

// The native TUI reports the hosting terminal between the platform and the
// Codex suffix. When Matou was launched from Finder there is no TERM_PROGRAM,
// so use the verified iTerm baseline captured with the upstream identity.
const terminalProgram = process.env.TERM_PROGRAM
  ? `${process.env.TERM_PROGRAM}/${process.env.TERM_PROGRAM_VERSION ?? '1.0'}`
  : 'iTerm.app/3.6.11'

const osProductVersion = macOSProductVersion()

export function codexClientUserAgentForVersion(version: string): string {
  return `codex-tui/${version} (Mac OS ${osProductVersion}; ${clientArch()}) ${terminalProgram} (codex-tui; ${version})`
}

export const CODEX_CLIENT_USER_AGENT = codexClientUserAgentForVersion(CODEX_CLIENT_VERSION)

export const CODEX_CLIENT_HEADERS: Record<string, string> = { Originator: CODEX_CLIENT_ORIGINATOR }

export function parseSemver(raw: string): string | undefined {
  const match = /\b(\d+\.\d+\.\d+)\b/.exec(raw)
  return match?.[1]
}

export function compareSemver(a: string, b: string): number {
  const pa = a.split('.').map(Number)
  const pb = b.split('.').map(Number)
  for (let i = 0; i < 3; i++) {
    const diff = (pa[i] ?? 0) - (pb[i] ?? 0)
    if (diff !== 0) return diff
  }
  return 0
}

async function detectLocalCodexVersion(env: NodeJS.ProcessEnv): Promise<string | undefined> {
  try {
    const extraEnv = await resolveProviderCommandEnvironment('codex', env)
    const mergedEnv = Object.keys(extraEnv).length ? { ...env, ...extraEnv } : env
    const { stdout } = await execFileAsync('codex', ['--version'], {
      env: mergedEnv,
      encoding: 'utf8',
      timeout: 3_000,
      maxBuffer: 16 * 1024
    })
    return parseSemver(stdout)
  } catch {
    return undefined
  }
}

let cachedCodexIdentity: Promise<CodexClientIdentity> | undefined

/** Resolves the Codex CLI version from the local install when it exceeds the
 * verified fallback floor, caching once per process so probes and router
 * inference always share one client identity. */
export function resolveCodexClientIdentity(options?: {
  env?: NodeJS.ProcessEnv
  detectVersion?: () => Promise<string | undefined>
}): Promise<CodexClientIdentity> {
  const compute = async (): Promise<CodexClientIdentity> => {
    const detected = await (options?.detectVersion ?? (() => detectLocalCodexVersion(options?.env ?? process.env)))()
    const version = detected && compareSemver(detected, CODEX_CLIENT_VERSION) > 0
      ? detected
      : CODEX_CLIENT_VERSION
    return { version, userAgent: codexClientUserAgentForVersion(version) }
  }
  if (options?.env || options?.detectVersion) return compute()
  cachedCodexIdentity ??= compute()
  return cachedCodexIdentity
}

/** True when the credential already carries the Codex Originator. The user
 * agent comes from router config, so it needs no credential write. */
export async function credentialCarriesCodexIdentity(
  dataRoot: string,
  file: { name: string; path?: unknown }
): Promise<boolean> {
  const credential = await readCredentialJson(dataRoot, file)
  const headers = credential?.headers
  if (!headers || typeof headers !== 'object' || Array.isArray(headers)) return false
  return (headers as Record<string, unknown>).Originator === CODEX_CLIENT_ORIGINATOR
}
