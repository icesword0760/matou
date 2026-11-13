import { execFile } from 'node:child_process'
import { constants } from 'node:fs'
import { access } from 'node:fs/promises'
import { delimiter, isAbsolute, join } from 'node:path'
import { promisify } from 'node:util'

const execFileAsync = promisify(execFile)
const PATH_MARKER = '__MATOU_LOGIN_PATH__='
const loginShellPaths = new Map<string, Promise<string | undefined>>()
export const PROXY_ENVIRONMENT_KEYS = [
  'HTTPS_PROXY', 'https_proxy', 'ALL_PROXY', 'all_proxy',
  'HTTP_PROXY', 'http_proxy', 'NO_PROXY', 'no_proxy'
] as const
export type ProxyEnvironment = Partial<Record<typeof PROXY_ENVIRONMENT_KEYS[number], string>>
const PROXY_MARKER = '__MATOU_LOGIN_PROXIES__'
const loginShellProxies = new Map<string, Promise<ProxyEnvironment>>()

type Environment = Record<string, string | undefined>

interface ProviderCommandEnvironmentDependencies {
  commandExists?(command: string, path: string | undefined): Promise<boolean>
  loadLoginShellPath?(environment: Environment): Promise<string | undefined>
}

export async function resolveProviderCommandEnvironment(
  command: string,
  environment: Environment,
  dependencies: ProviderCommandEnvironmentDependencies = {}
): Promise<Record<string, string>> {
  if (process.platform === 'win32') return {}
  // PATH cannot repair an explicit path. In particular a missing executable
  // must fail at spawn, not wait up to 10s for an unrelated login-shell probe.
  if (isAbsolute(command) || command.includes('/')) return {}
  const commandExists = dependencies.commandExists ?? executableExists
  if (await commandExists(command, environment.PATH)) return {}

  const loadLoginShellPath = dependencies.loadLoginShellPath ?? cachedLoginShellPath
  const loginPath = await loadLoginShellPath(environment)
  if (!loginPath) return {}
  return { PATH: loginPath }
}

async function cachedLoginShellPath(environment: Environment): Promise<string | undefined> {
  const key = loginShellKey(environment)
  const existing = loginShellPaths.get(key)
  if (existing) return existing
  const pending = readLoginShellPath(environment)
  loginShellPaths.set(key, pending)
  return pending
}

function loginShellKey(environment: Environment): string {
  return [environment.SHELL ?? '', environment.HOME ?? '', environment.ZDOTDIR ?? '', environment.PATH ?? ''].join('\u0000')
}

/** Same interactive login-shell boundary as PATH recovery, including .zshrc.
 * Only these eight variables leave the shell. NUL framing tolerates startup
 * output/newlines; no `env` dump, token forwarding, or value interpolation.
 * Cache the in-flight read as well as its result, once per shell context.
 */
export function loadLoginShellProxyEnvironment(environment: Environment): Promise<ProxyEnvironment> {
  const key = [loginShellKey(environment), ...PROXY_ENVIRONMENT_KEYS.map(name => environment[name] ?? '')].join('\u0000')
  let pending = loginShellProxies.get(key)
  if (!pending) {
    pending = readLoginShellProxyEnvironment(environment)
    loginShellProxies.set(key, pending)
  }
  return pending
}

async function readLoginShellProxyEnvironment(environment: Environment): Promise<ProxyEnvironment> {
  try {
    const argumentsToPrintf = PROXY_ENVIRONMENT_KEYS.map(name => `"\${${name}-}"`).join(' ')
    const { stdout } = await execFileAsync(environment.SHELL ?? '/bin/zsh', [
      '-ilc', `printf '${PROXY_MARKER}\\000${'%s\\000'.repeat(PROXY_ENVIRONMENT_KEYS.length)}' ${argumentsToPrintf}`
    ], { env: environment, encoding: 'utf8', timeout: 10_000, maxBuffer: 256 * 1024 })
    const marker = `${PROXY_MARKER}\0`
    const start = stdout.lastIndexOf(marker)
    if (start === -1) return {}
    const values = stdout.slice(start + marker.length).split('\0')
    if (values.length < PROXY_ENVIRONMENT_KEYS.length + 1) return {}
    const result: ProxyEnvironment = {}
    PROXY_ENVIRONMENT_KEYS.forEach((name, index) => {
      const value = values[index]?.trim()
      if (value) result[name] = value
    })
    return result
  } catch { return {} }
}

export async function readLoginShellPath(
  environment: Environment
): Promise<string | undefined> {
  const shell = environment.SHELL ?? '/bin/zsh'
  try {
    const { stdout } = await execFileAsync(shell, [
      '-ilc', `printf '${PATH_MARKER}%s\\n' "$PATH"`
    ], {
      env: environment,
      encoding: 'utf8',
      timeout: 10_000,
      maxBuffer: 256 * 1024
    })
    const output = stdout
    const line = output.split('\n').reverse()
      .find((value: string) => value.startsWith(PATH_MARKER))
    const path = line?.slice(PATH_MARKER.length).trim()
    return path || undefined
  } catch {
    return undefined
  }
}

async function executableExists(command: string, path: string | undefined): Promise<boolean> {
  if (isAbsolute(command) || command.includes('/')) return canExecute(command)
  for (const directory of (path ?? '').split(delimiter)) {
    if (directory && await canExecute(join(directory, command))) return true
  }
  return false
}

async function canExecute(path: string): Promise<boolean> {
  try {
    await access(path, constants.X_OK)
    return true
  } catch {
    return false
  }
}
