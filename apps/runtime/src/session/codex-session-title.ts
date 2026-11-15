import { createReadStream } from 'node:fs'
import { createInterface } from 'node:readline'

/** Read only the exact Codex UUID; index ordering need not match updated_at. */
export async function latestCodexAutoTitle(path: string, sessionId: string): Promise<string | undefined> {
  if (!/^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/i.test(sessionId)) return undefined
  let title: string | undefined
  let latest = -Infinity
  const stream = createReadStream(path, { encoding: 'utf8' })
  const lines = createInterface({ input: stream, crlfDelay: Infinity })
  try {
    for await (const line of lines) {
      try {
        const row = JSON.parse(line) as Record<string, unknown>
        if (row?.id !== sessionId || typeof row.thread_name !== 'string' || !row.thread_name.trim()) continue
        const updated = typeof row.updated_at === 'string' ? Date.parse(row.updated_at) : NaN
        if (!Number.isFinite(updated) || updated < latest) continue
        latest = updated
        title = row.thread_name.trim()
      } catch { /* Ignore malformed and partially written index rows. */ }
    }
    return title
  } catch {
    return undefined
  } finally {
    lines.close()
    stream.destroy()
  }
}
