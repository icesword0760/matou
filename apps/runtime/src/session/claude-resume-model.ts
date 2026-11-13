import { open, realpath } from 'node:fs/promises'
import { homedir } from 'node:os'
import { join } from 'node:path'
import { encodeClaudeProjectPath } from './claude-session-catalog'

/** Native 2.1.323 can restore a model from an assistant/identity attachment even
 * when settings choose another default. Only override our own subscription IDs;
 * ordinary native model restoration must keep working. Read backwards so long
 * conversations do not make provider switching scan their entire history.
 */
export async function claudeResumeUsesSubscriptionModel(
  cwd: string, sessionId: string, environment: NodeJS.ProcessEnv
): Promise<boolean> {
  if (!/^[a-zA-Z0-9_-]+$/.test(sessionId)) return false
  const root = join(environment.CLAUDE_CONFIG_DIR || join(environment.HOME || homedir(), '.claude'), 'projects')
  const canonical = await realpath(cwd).catch(() => cwd)
  for (const bucket of new Set([encodeClaudeProjectPath(cwd), encodeClaudeProjectPath(canonical)])) {
    const file = await open(join(root, bucket, `${sessionId}.jsonl`), 'r').catch(() => undefined)
    if (!file) continue
    try {
      let offset = (await file.stat()).size
      let remainder = Buffer.alloc(0)
      let assistantModel: string | undefined
      while (offset > 0) {
        const size = Math.min(offset, 64 * 1024)
        offset -= size
        const chunk = Buffer.allocUnsafe(size)
        const { bytesRead } = await file.read(chunk, 0, size, offset)
        const data = Buffer.concat([chunk.subarray(0, bytesRead), remainder])
        let end = data.length
        for (let index = data.length - 1; index >= 0; index--) {
          if (data[index] !== 10 && !(offset === 0 && index === 0)) continue
          const start = data[index] === 10 ? index + 1 : 0
          try {
            const row = JSON.parse(data.subarray(start, end).toString('utf8'))
            if (row.type === 'assistant' && row.isMeta !== true && assistantModel === undefined) {
              const model = row.inPlaceOfModel ?? row.message?.model
              if (typeof model === 'string' && model && !model.startsWith('<')) {
                assistantModel = model
                if (model.startsWith('matou-')) return true
                // A known native model is authoritative over an older identity.
                if (/^(?:anthropic\/)?claude-/i.test(model)) return false
              }
            }
            if (row.type === 'attachment' && row.attachment?.type === 'model') {
              const model = row.attachment.identity?.modelId
              if (typeof model === 'string') return model.startsWith('matou-')
            }
          } catch { /* Empty, partially written or unrelated transcript line. */ }
          end = index
        }
        remainder = data.subarray(0, end)
      }
      return false
    } finally { await file.close() }
  }
  return false
}
