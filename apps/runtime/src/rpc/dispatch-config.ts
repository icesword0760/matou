import { randomUUID } from 'node:crypto'
import { mkdir, readFile, rename, rm, writeFile } from 'node:fs/promises'
import { homedir } from 'node:os'
import { dirname, join } from 'node:path'

import {
  DISPATCH_DEFAULTS, dispatchDefaultsSchema,
  type DispatchConfigSnapshot, type DispatchDefaults
} from '@matou/contracts'

export class DispatchConfigValidationError extends Error {}

/** Global defaults shared with the dispatch script, read fresh on every request. */
export class DispatchConfig {
  readonly path = join(homedir(), '.claude', 'agent-foreman', 'config.json')

  async snapshot(): Promise<DispatchConfigSnapshot> {
    const fallback = { exists: false, path: this.path, defaults: { ...DISPATCH_DEFAULTS } }
    let content: string
    try {
      content = await readFile(this.path, 'utf8')
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === 'ENOENT') return fallback
      throw error
    }
    let config: unknown
    try { config = JSON.parse(content) } catch { return fallback }
    if (!isRecord(config) || config.version !== 1) return fallback
    const input = isRecord(config.defaults) ? config.defaults : {}
    const defaults = { ...DISPATCH_DEFAULTS }
    for (const field of Object.keys(dispatchDefaultsSchema.shape) as Array<keyof DispatchDefaults>) {
      const result = dispatchDefaultsSchema.shape[field].safeParse(input[field])
      if (result.success) Object.assign(defaults, { [field]: result.data })
    }
    return { exists: true, path: this.path, defaults }
  }

  async save(input: unknown): Promise<DispatchConfigSnapshot> {
    const result = dispatchDefaultsSchema.safeParse(input)
    if (!result.success) {
      const field = result.error.issues[0]?.path[0] ?? 'defaults'
      throw new DispatchConfigValidationError(`Invalid dispatch config field: ${String(field)}`)
    }
    // Validate the whole request before creating directories or touching the file.
    await mkdir(dirname(this.path), { recursive: true })
    const temporary = `${this.path}.${randomUUID()}.tmp`
    try {
      await writeFile(temporary, `${JSON.stringify({ version: 1, defaults: result.data }, null, 2)}\n`, {
        encoding: 'utf8', flag: 'wx', mode: 0o600
      })
      await rename(temporary, this.path)
    } finally {
      await rm(temporary, { force: true })
    }
    return { exists: true, path: this.path, defaults: result.data }
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}
