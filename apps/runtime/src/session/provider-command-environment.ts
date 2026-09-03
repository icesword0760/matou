import { execFile } from 'node:child_process'
import { constants } from 'node:fs'
import { access } from 'node:fs/promises'
import { delimiter, isAbsolute, join } from 'node:path'
import { promisify } from 'node:util'

const execFileAsync = promisify(execFile)
const PATH_MARKER = '__MATOU_LOGIN_PATH__='
const loginShellPaths = new Map<string, Promise<string | undefined>>()

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
  const commandExists = dependencies.commandExists ?? executableExists
  if (await commandExists(command, environment.PATH)) return {}

  const loadLoginShellPath = dependencies.loadLoginShellPath ?? cachedLoginShellPath
  const loginPath = await loadLoginShellPath(environment)
  if (!loginPath) return {}
  return { PATH: loginPath }
}

async function cachedLoginShellPath(environment: Environment): Promise<string | undefined> {
  const key = [
    environment.SHELL ?? '', environment.HOME ?? '', environment.ZDOTDIR ?? '',
    environment.PATH ?? ''
  ].join('\u0000')
  const existing = loginShellPaths.get(key)
  if (existing) return existing
  const pending = readLoginShellPath(environment)
  loginShellPaths.set(key, pending)
  return pending
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
