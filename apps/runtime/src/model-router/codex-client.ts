import { execFileSync } from 'node:child_process'
import { readCredentialJson } from './credential-file'

/** One ChatGPT subscription token must look like one Codex client.
 *
 * The installed native Codex CLI identifies itself as `codex_exec` with a
 * versioned user agent and no `Version` header. Matou's probes and the router's
 * inference path share this token, so both sides send the same values. The
 * router config pins the user agent for inference; credential headers pin the
 * Originator because the client (Claude Code) does not send Codex values.
 *
 * The OS product version comes from `sw_vers`, matching what the native binary
 * reports on this machine. 0.162.1 is the verified Codex baseline; keep it in
 * sync when the native CLI changes.
 */
export const CODEX_CLIENT_VERSION = '0.162.1'
export const CODEX_CLIENT_ORIGINATOR = 'codex-tui'

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

export const CODEX_CLIENT_USER_AGENT =
  `codex-tui/${CODEX_CLIENT_VERSION} (Mac OS ${macOSProductVersion()}; ${clientArch()}) ${terminalProgram} (codex-tui; ${CODEX_CLIENT_VERSION})`

export const CODEX_CLIENT_HEADERS: Record<string, string> = { Originator: CODEX_CLIENT_ORIGINATOR }

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
