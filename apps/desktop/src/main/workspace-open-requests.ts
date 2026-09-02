import { stat } from 'node:fs/promises'
import { resolve } from 'node:path'

export class WorkspaceOpenRequests {
  readonly #pending: string[] = []
  readonly #queued = new Set<string>()

  async enqueue(path: string): Promise<boolean> {
    const normalized = resolve(path)
    if (this.#queued.has(normalized)) return false
    this.#queued.add(normalized)
    const directory = await stat(normalized).catch(() => undefined)
    if (!directory?.isDirectory()) {
      this.#queued.delete(normalized)
      return false
    }
    this.#pending.push(normalized)
    return true
  }

  drain(): string[] {
    const paths = this.#pending.splice(0)
    for (const path of paths) this.#queued.delete(path)
    return paths
  }
}
