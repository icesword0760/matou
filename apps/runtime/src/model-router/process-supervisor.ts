// Standalone compiled entry. stdin is a lifetime lease owned by Electron main.
// EOF also handles main SIGKILL; main closes it when its runtime generation dies.
import { spawn, type ChildProcess } from 'node:child_process'
import { resolve } from 'node:path'

process.umask(0o077)
const [binary, config, directory] = process.argv.slice(2)
if (!binary || !config || !directory || ![binary, config, directory].every(path => resolve(path) === path)) process.exit(2)
let child: ChildProcess | undefined
let stopping = false
let restarts = 0
let killTimer: ReturnType<typeof setTimeout> | undefined
const report = (value: object) => process.stdout.write(`${JSON.stringify(value)}\n`)
function stop(): void {
  if (stopping) return
  stopping = true
  if (!child || child.exitCode !== null || child.signalCode !== null) { process.exit(0); return }
  child.kill('SIGTERM')
  killTimer = setTimeout(() => child?.kill('SIGKILL'), 1_500)
}
function launch(): void {
  if (stopping) return
  child = spawn(binary!, ['-config', config!, '-local-model', '-no-browser'], {
    cwd: directory!, stdio: 'ignore',
    // Outbound proxy is resolved from the login shell / system by the manager
    // and written as requests.proxy-url. Never forward ambient tokens/config.
    env: { PATH: process.env.PATH ?? '/usr/bin:/bin', LANG: process.env.LANG ?? 'en_US.UTF-8', HOME: directory! }
  })
  child.once('spawn', () => report({ state: 'spawned', pid: child!.pid, restarts }))
  child.once('error', () => { report({ state: 'failed', reason: 'spawn-failed' }); process.exit(1) })
  child.once('exit', () => {
    if (killTimer) clearTimeout(killTimer)
    if (stopping) process.exit(0)
    if (restarts === 0) { restarts++; launch() }
    else { report({ state: 'failed', reason: 'restart-exhausted' }); process.exit(1) }
  })
}
process.stdin.once('end', stop)
process.stdin.once('error', stop)
process.stdin.resume()
process.once('SIGTERM', stop)
process.once('SIGINT', stop)
launch()
