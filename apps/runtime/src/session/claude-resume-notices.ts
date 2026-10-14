import { constants } from 'node:fs'
import { open, realpath } from 'node:fs/promises'
import { homedir } from 'node:os'
import { join } from 'node:path'
import { createInterface } from 'node:readline'

import { encodeClaudeProjectPath } from './claude-session-catalog'

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i
const LOAD_NOTICE = /^agents-md: no CLAUDE\.md found; AGENTS\.md loaded: (?:\/|[A-Za-z]:[\\/])[^\r\n]+[\\/]AGENTS\.md$/

type Row = Record<string, unknown>
export type QuietResumeResult = { quieted: number; skipped?: 'changed' | 'incomplete' | 'unsupported' }

export class ClaudeResumeNoticeStorageError extends Error {}

/**
 * Claude's JSONL is a UUID-keyed append log. A later row with the same UUID
 * updates that message; `info` is its native transcript-only display level.
 * Append ONLY level corrections for old AGENTS load receipts. Original bytes,
 * message content/identity/parents and every user, assistant and tool row stay
 * intact. Ctrl+O still shows the receipts. The native rule loader is untouched.
 *
 * Call before launching a resumed process, never from a live transcript watcher.
 * Fail open on unknown formats, active writers or errors: cosmetics must not
 * prevent a conversation from resuming or endanger its source history.
 */
export async function quietClaudeResumeNotices(input: {
  cwd: string
  providerSessionId: string
  env?: NodeJS.ProcessEnv
}): Promise<QuietResumeResult> {
  if (!UUID.test(input.providerSessionId)) return { quieted: 0, skipped: 'unsupported' }
  const env = input.env ?? process.env
  const projectsRoot = env.MATOU_CLAUDE_PROJECTS_ROOT ||
    join(env.CLAUDE_CONFIG_DIR || join(env.HOME || homedir(), '.claude'), 'projects')
  const canonicalCwd = await realpath(input.cwd).catch(() => input.cwd)
  for (const directory of new Set([encodeClaudeProjectPath(input.cwd), encodeClaudeProjectPath(canonicalCwd)])) {
    const path = join(projectsRoot, directory, `${input.providerSessionId}.jsonl`)
    try {
      return await quietTranscriptNotices(path, input.providerSessionId)
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error
    }
  }
  return { quieted: 0 }
}

async function quietTranscriptNotices(path: string, sessionId: string): Promise<QuietResumeResult> {
  // No O_CREAT, truncation, rename or rewrite. O_APPEND keeps external appends
  // intact; O_NOFOLLOW avoids following a replaced transcript symlink.
  const file = await open(path, constants.O_RDWR | constants.O_APPEND | (constants.O_NOFOLLOW ?? 0))
  try {
    const before = await file.stat()
    if (!before.isFile() || !before.size) return { quieted: 0 }
    const tail = Buffer.alloc(1)
    await file.read(tail, 0, 1, before.size - 1)
    // A partial native write must never be joined to one of our updates.
    if (tail[0] !== 0x0a) return { quieted: 0, skipped: 'incomplete' }
    const notices = new Map<string, Row>()
    const stream = file.createReadStream({ start: 0, end: before.size - 1, autoClose: false })
    const lines = createInterface({ input: stream, crlfDelay: Infinity })
    try {
      for await (const line of lines) {
        if (!line.trim()) continue
        let row: Row
        try { row = JSON.parse(line) as Row } catch { return { quieted: 0, skipped: 'incomplete' } }
        if (!row || typeof row !== 'object' || typeof row.uuid !== 'string') continue
        if (row.type === 'system' && row.subtype === 'informational' && row.sessionId === sessionId &&
            UUID.test(row.uuid) && typeof row.content === 'string' && LOAD_NOTICE.test(row.content) &&
            row.level === 'notice' && row.isMeta !== true) {
          notices.set(row.uuid, row)
        } else {
          // Honor a later correction, including our own from a previous resume.
          notices.delete(row.uuid)
        }
      }
    } finally {
      lines.close()
      if (!stream.readableEnded) stream.destroy()
    }
    if (!notices.size) return { quieted: 0 }
    const after = await file.stat()
    if (before.size !== after.size || before.mtimeMs !== after.mtimeMs) {
      return { quieted: 0, skipped: 'changed' }
    }
    const updates = Buffer.from([...notices.values()].map(row => JSON.stringify({ ...row, level: 'info' }) + '\n').join(''))
    // One append operation. Never replace any source byte, even if another
    // process starts writing after the snapshot check above.
    try {
      const { bytesWritten } = await file.write(updates)
      if (bytesWritten !== updates.length) throw new Error('Incomplete metadata append')
      await file.sync()
    } catch (error) {
      // Stop this launch on a write/storage failure rather than letting a new
      // provider append a user message onto a partial metadata row. Best-effort
      // line termination preserves parsing of future native appends; no rewind,
      // truncation or deletion (including any concurrent writer's bytes).
      try {
        const failed = await file.stat()
        await file.read(tail, 0, 1, failed.size - 1)
        if (tail[0] !== 0x0a) await file.write(Buffer.from('\n'))
        await file.sync()
      } catch { /* Keep the provider stopped until storage is healthy again. */ }
      throw new ClaudeResumeNoticeStorageError(
        `Claude resume paused: receipt metadata storage error (${(error as NodeJS.ErrnoException).code ?? 'write'}). Original history is preserved; check disk storage and retry.`
      )
    }
    return { quieted: notices.size }
  } finally {
    await file.close()
  }
}
