import { execFile, spawn } from 'node:child_process'
import { access } from 'node:fs/promises'
import { join } from 'node:path'
import { promisify } from 'node:util'
import type { AccountError, ClaudeAccountView } from '../../shared/claude-accounts'

export class AccountFault extends Error {
  constructor(readonly code: AccountError) { super(code) }
}
export interface EngineState {
  accounts: ClaudeAccountView[]
  engineVersion?: string
  currentId?: string
  detected?: ClaudeAccountView
  draft?: ClaudeAccountView
}
export interface AccountEngine {
  run(input: Record<string, unknown>, signal?: AbortSignal): Promise<EngineState>
}
export interface EngineLaunch {
  executable: string
  args: string[]
  stateRoot: string
}
/** This process bridge contains no OAuth, Keychain, refresh, or switch logic. */
export class ClaudeSwapEngine implements AccountEngine {
  private environment: Promise<NodeJS.ProcessEnv> | undefined
  constructor(private readonly launch: EngineLaunch, private readonly baseEnv = process.env) {}
  private env() {
    this.environment ??= (async () => {
      const env: NodeJS.ProcessEnv = { ...this.baseEnv, PYTHONNOUSERSITE: '1', PYTHONUNBUFFERED: '1' }
      delete env.PYTHONPATH; delete env.PYTHONHOME
      if (process.platform !== 'win32') {
        try {
          const { stdout } = await promisify(execFile)(env.SHELL || '/bin/zsh', ['-ilc', 'printf "__MATOU_PATH__=%s\\n" "$PATH"'], { timeout: 10_000, maxBuffer: 64 * 1024, env })
          const match = stdout.split('\n').find(line => line.startsWith('__MATOU_PATH__='))
          if (match) env.PATH = match.slice('__MATOU_PATH__='.length)
        } catch { /* Inherit the app environment if the login shell is unavailable. */ }
      }
      return env
    })()
    return this.environment
  }
  async run(input: Record<string, unknown>, signal?: AbortSignal): Promise<EngineState> {
    try { await access(this.launch.executable) } catch { throw new AccountFault('engine-missing') }
    const env = await this.env()
    if (signal?.aborted) throw new AccountFault('login-failed')
    return new Promise((resolve, reject) => {
      const child = spawn(this.launch.executable, [...this.launch.args, this.launch.stateRoot], { env, stdio: ['pipe', 'pipe', 'pipe'] })
      let output = '', overflow = false
      // Never forward raw engine output: even error paths can include identities.
      child.stdout.on('data', (data: Buffer) => {
        if (output.length + data.length < 2 * 1024 * 1024) output += data.toString('utf8')
        else overflow = true
      })
      child.stderr.resume()
      const abort = () => child.kill('SIGTERM')
      signal?.addEventListener('abort', abort, { once: true })
      child.once('error', () => { signal?.removeEventListener('abort', abort); reject(new AccountFault('engine-missing')) })
      child.once('close', code => {
        signal?.removeEventListener('abort', abort)
        if (signal?.aborted) { reject(new AccountFault('login-failed')); return }
        try {
          if (code !== 0 || overflow) throw new Error('process')
          const result = JSON.parse(output)
          if (!result.ok) {
            const allowed = ['engine-version', 'cli-missing', 'login-failed', 'login-timeout', 'identity', 'conflict', 'invalid', 'switch-failed', 'engine-error', 'keychain', 'busy', 'unsupported']
            throw new AccountFault(allowed.includes(result.error) ? result.error : 'engine-error')
          }
          const value = result.value as EngineState
          if (!value || typeof value !== 'object') throw new Error('schema')
          resolve(value)
        } catch (error) { reject(error instanceof AccountFault ? error : new AccountFault('engine-error')) }
      })
      child.stdin.on('error', () => {})
      child.stdin.end(JSON.stringify(input) + '\n')
    })
  }
}
export function engineLaunch(appPath: string, resourcesPath: string, packaged: boolean, stateRoot: string): EngineLaunch {
  if (packaged) return { executable: join(resourcesPath, 'claude-accounts', 'matou-claude-accounts', process.platform === 'win32' ? 'matou-claude-accounts.exe' : 'matou-claude-accounts'), args: [], stateRoot }
  const root = join(appPath, '..', '..', 'tooling', 'claude-accounts')
  return { executable: join(root, '.venv', process.platform === 'win32' ? 'Scripts/python.exe' : 'bin/python'), args: [join(root, 'bridge.py')], stateRoot }
}
