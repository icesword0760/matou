import { execFile } from 'node:child_process'
import { chmod, writeFile } from 'node:fs/promises'
import { promisify } from 'node:util'

const execFileAsync = promisify(execFile)

/** Preserve the installed profile's literal config, including inert !!js values.
 * DSH replaces config as a whole. Never evaluate a user's expressions here. */
export function dshObservationPatch(dump: string, bridgePath: string, hookUrl: string, runId: string): string {
  const lines = dump.split(/\r?\n/)
  const start = lines.findIndex(line => line === '- id: dsh-tui')
  if (start < 0) throw new Error('DSH profile has no top-level dsh-tui entry')
  let end = lines.findIndex((line, index) => index > start && /^- /.test(line))
  if (end < 0) end = lines.length
  const row = lines.slice(start + 1, end)
  const configAt = row.indexOf('  config:')
  if (configAt < 0) throw new Error('DSH TUI config must be a literal mapping for a temporary overlay')
  let configEnd = row.findIndex((line, index) => index > configAt && /^  \S/.test(line))
  if (configEnd < 0) configEnd = row.length
  const config = row.slice(configAt + 1, configEnd)
  const keys = new Set(['backend', 'taskNotifications', 'upstreamRetry'])
  const preserved: string[] = []
  let dropping = false
  for (const line of config) {
    const key = /^    ([\w-]+):/.exec(line)?.[1]
    if (key) dropping = keys.has(key)
    if (!dropping) preserved.push(line)
  }
  const injectAt = row.indexOf('  inject:')
  const inject: string[] = []
  if (injectAt >= 0) {
    for (let i = injectAt + 1; i < row.length && /^    - /.test(row[i]!); i++) {
      const service = /^    - ([A-Za-z][\w-]*)\s*$/.exec(row[i]!)?.[1]
      if (!service) throw new Error('DSH TUI inject must be a literal service list')
      inject.push(service)
    }
  } else if (row.some(line => /^  inject:/.test(line))) {
    throw new Error('DSH TUI inject must be a literal service list')
  }
  return [
    '- insert:',
    '    - id: matou-dsh-observer',
    `      name: ${JSON.stringify(bridgePath)}`,
    '      config:',
    `        hookUrl: ${JSON.stringify(hookUrl)}`,
    `        runId: ${JSON.stringify(runId)}`,
    '- id: dsh-tui',
    '  inject:',
    ...[...new Set([...inject, 'matouDshBridge'])].map(name => `    - ${name}`),
    '  config:', ...preserved,
    '    backend: dsh',
    '    taskNotifications: "off"',
    // TUI otherwise seeds persistent provider settings even on an empty boot.
    '    upstreamRetry: false', ''
  ].join('\n')
}

export async function prepareDshObservationPatch(input: {
  executable: string; cwd: string; env: NodeJS.ProcessEnv; path: string
  bridgePath: string; hookUrl: string; runId: string
}): Promise<void> {
  const { stdout } = await execFileAsync(input.executable, ['dsh-tui', '--dump-config'], {
    cwd: input.cwd, env: input.env, encoding: 'utf8', timeout: 15_000, maxBuffer: 1024 * 1024
  })
  await writeFile(input.path, dshObservationPatch(stdout, input.bridgePath, input.hookUrl, input.runId), { mode: 0o600 })
  await chmod(input.path, 0o600)
}
