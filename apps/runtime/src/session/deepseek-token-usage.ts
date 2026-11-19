import { createHash } from 'node:crypto'
import type { IncomingMessage } from 'node:http'
import { StringDecoder } from 'node:string_decoder'
import { brotliDecompressSync, gunzipSync, inflateSync } from 'node:zlib'

const MAX_OBSERVED_KEYS = 256
const MAX_OBSERVED_BYTES = 4 * 1024 * 1024
const MAX_SSE_LINE_LENGTH = 256 * 1024
const MAX_TEXT_SAMPLE_LENGTH = 1024
const MAX_SIGNATURE_LENGTH = 128
const DEFAULT_DEEPSEEK_CONTEXT_WINDOW = 1_048_576
const MAX_RESERVED_OUTPUT_TOKENS = 32_000
const TOKEN_USAGE_REMINDER_PATTERN = /<system-reminder>\s*Token usage:\s*\d+\/\d+;/

export interface SessionContextUsageSnapshot {
  usedTokens?: number
  contextWindowSize?: number
}

export interface AttachDeepSeekTokenUsageOptions {
  sessionContextWindow?: number | null
  overrideContextWindow?: number
  sessionUsage?: SessionContextUsageSnapshot
}

interface UsageAccumulator {
  inputTokens?: number
  cacheCreationTokens?: number
  cacheReadTokens?: number
  outputTokens?: number
}

function object(value: unknown): Record<string, unknown> | undefined {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
    ? value as Record<string, unknown>
    : undefined
}

function nonNegativeInt(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isFinite(value) && value >= 0
    ? Math.floor(value)
    : undefined
}

function positiveInt(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isFinite(value) && value > 0
    ? Math.floor(value)
    : undefined
}

function applyUsageFields(target: UsageAccumulator, usage: Record<string, unknown> | undefined): void {
  if (!usage) return
  const input = nonNegativeInt(usage.input_tokens)
  const cacheCreation = nonNegativeInt(usage.cache_creation_input_tokens)
  const cacheRead = nonNegativeInt(usage.cache_read_input_tokens)
  const output = nonNegativeInt(usage.output_tokens)
  if (input !== undefined) target.inputTokens = input
  if (cacheCreation !== undefined) target.cacheCreationTokens = cacheCreation
  if (cacheRead !== undefined) target.cacheReadTokens = cacheRead
  if (output !== undefined) target.outputTokens = output
}

function totalUsageTokens(usage: UsageAccumulator): number | undefined {
  if (
    usage.inputTokens === undefined &&
    usage.cacheCreationTokens === undefined &&
    usage.cacheReadTokens === undefined &&
    usage.outputTokens === undefined
  ) {
    return undefined
  }
  const sum = (usage.inputTokens ?? 0) +
    (usage.cacheCreationTokens ?? 0) +
    (usage.cacheReadTokens ?? 0) +
    (usage.outputTokens ?? 0)
  return sum > 0 ? sum : undefined
}

function textKey(sample: string): string | undefined {
  const normalized = sample.slice(0, MAX_TEXT_SAMPLE_LENGTH)
  if (!normalized.trim()) return undefined
  return `text:${createHash('sha256').update(normalized).digest('hex').slice(0, 32)}`
}

function extractAssistantLookupKeys(messages: unknown[], assistantIndex: number): string[] {
  const keys: string[] = []
  const assistant = object(messages[assistantIndex])
  let textSample = ''
  if (typeof assistant?.content === 'string') {
    textSample = assistant.content.slice(0, MAX_TEXT_SAMPLE_LENGTH)
  } else if (Array.isArray(assistant?.content)) {
    for (const item of assistant.content) {
      const block = object(item)
      if (!block) continue
      if (block.type === 'tool_use' && typeof block.id === 'string' && block.id) {
        keys.push(`tool:${block.id}`)
      } else if (block.type === 'thinking' && typeof block.signature === 'string' && block.signature) {
        keys.push(`sig:${block.signature.slice(0, MAX_SIGNATURE_LENGTH)}`)
      } else if (block.type === 'text' && typeof block.text === 'string' && textSample.length < MAX_TEXT_SAMPLE_LENGTH) {
        textSample += block.text.slice(0, MAX_TEXT_SAMPLE_LENGTH - textSample.length)
      }
    }
  }
  for (let index = assistantIndex + 1; index < messages.length; index += 1) {
    const entry = object(messages[index])
    if (entry?.role !== 'user' || !Array.isArray(entry.content)) continue
    for (const item of entry.content) {
      const block = object(item)
      if (block?.type === 'tool_result' && typeof block.tool_use_id === 'string' && block.tool_use_id) {
        keys.push(`tool:${block.tool_use_id}`)
      }
    }
  }
  const hashed = textKey(textSample)
  if (hashed) keys.push(hashed)
  return keys
}

function hasClientTools(message: Record<string, unknown>): boolean {
  if (!Array.isArray(message.tools) || message.tools.length === 0) return false
  return message.tools.some(value => {
    const tool = object(value)
    return typeof tool?.name === 'string' && !(typeof tool.type === 'string' && tool.type.startsWith('web_search_'))
  })
}

function hasExistingTokenUsageReminder(entry: Record<string, unknown>): boolean {
  if (typeof entry.content === 'string') return TOKEN_USAGE_REMINDER_PATTERN.test(entry.content)
  if (!Array.isArray(entry.content)) return false
  return entry.content.some(value => {
    const block = object(value)
    return block?.type === 'text' && typeof block.text === 'string' && TOKEN_USAGE_REMINDER_PATTERN.test(block.text)
  })
}

export function formatTokenUsageSystemReminder(used: number, total: number): string {
  const remaining = Math.max(0, total - used)
  return `<system-reminder>\nToken usage: ${used}/${total}; ${remaining} remaining\n</system-reminder>`
}

export class DeepSeekContextUsageTracker {
  readonly #byKey = new Map<string, number>()
  #compactionObserved = false

  noteCompaction(): void {
    this.#compactionObserved = true
  }

  attach(message: Record<string, unknown>, options: AttachDeepSeekTokenUsageOptions = {}): Record<string, unknown> {
    if (!Array.isArray(message.messages) || message.messages.length < 2) return message
    const messages = message.messages
    const lastEntry = object(messages[messages.length - 1])
    if (!lastEntry || lastEntry.role !== 'user' || hasExistingTokenUsageReminder(lastEntry)) return message

    let lastAssistantIndex = -1
    for (let index = messages.length - 2; index >= 0; index -= 1) {
      if (object(messages[index])?.role === 'assistant') {
        lastAssistantIndex = index
        break
      }
    }
    if (lastAssistantIndex === -1) return message

    let usedTokens: number | undefined
    for (const key of extractAssistantLookupKeys(messages, lastAssistantIndex)) {
      const matched = this.#byKey.get(key)
      if (matched !== undefined) {
        usedTokens = matched
        break
      }
    }
    if (usedTokens === undefined && !this.#compactionObserved && hasClientTools(message)) {
      usedTokens = positiveInt(options.sessionUsage?.usedTokens)
    }
    if (usedTokens === undefined) return message

    const rawWindow = positiveInt(options.sessionContextWindow) ??
      positiveInt(options.sessionUsage?.contextWindowSize) ??
      positiveInt(options.overrideContextWindow) ??
      DEFAULT_DEEPSEEK_CONTEXT_WINDOW
    const maxTokens = positiveInt(message.max_tokens)
    const reservedOutput = maxTokens === undefined ? 0 : Math.min(maxTokens, MAX_RESERVED_OUTPUT_TOKENS)
    const effectiveTotal = rawWindow > reservedOutput ? rawWindow - reservedOutput : rawWindow
    const reminderBlock = { type: 'text', text: formatTokenUsageSystemReminder(usedTokens, effectiveTotal) }

    let updatedLastEntry: Record<string, unknown> | undefined
    if (typeof lastEntry.content === 'string') {
      updatedLastEntry = {
        ...lastEntry,
        content: lastEntry.content
          ? [{ type: 'text', text: lastEntry.content }, reminderBlock]
          : [reminderBlock]
      }
    } else if (Array.isArray(lastEntry.content)) {
      updatedLastEntry = {
        ...lastEntry,
        content: [...lastEntry.content, reminderBlock]
      }
    }
    if (!updatedLastEntry) return message

    const nextMessages = [...messages]
    nextMessages[nextMessages.length - 1] = updatedLastEntry
    return { ...message, messages: nextMessages }
  }

  observeResponse(incoming: IncomingMessage): void {
    const contentType = String(incoming.headers['content-type'] ?? '').toLowerCase()
    const isSse = contentType.includes('text/event-stream')
    const isJson = !isSse && contentType.includes('application/json')
    if (!isSse && !isJson) return

    const encoding = String(incoming.headers['content-encoding'] ?? '').trim().toLowerCase()
    const decompress = encoding === 'gzip'
      ? gunzipSync
      : encoding === 'deflate'
        ? inflateSync
        : encoding === 'br'
          ? brotliDecompressSync
          : undefined
    if (!decompress && encoding !== '' && encoding !== 'identity') return

    const decoder = new StringDecoder('utf8')
    const usage: UsageAccumulator = {}
    const toolIds: string[] = []
    const signatures = new Map<number, string>()
    let textSample = ''
    let pending = ''
    let decodedBytes = 0
    let aborted = false

    const ingestSseLine = (rawLine: string) => {
      const line = rawLine.endsWith('\r') ? rawLine.slice(0, -1) : rawLine
      if (!line.startsWith('data:')) return
      const payload = line.slice(5).trimStart()
      if (!payload || payload === '[DONE]') return
      let event: Record<string, unknown> | undefined
      try {
        event = object(JSON.parse(payload))
      } catch {
        return
      }
      if (!event) return
      if (event.type === 'message_start') {
        applyUsageFields(usage, object(object(event.message)?.usage))
      } else if (event.type === 'message_delta') {
        applyUsageFields(usage, object(event.usage) ?? object(object(event.delta)?.usage))
      } else if (event.type === 'content_block_start') {
        const index = nonNegativeInt(event.index) ?? -1
        const block = object(event.content_block)
        if (!block) return
        if (block.type === 'tool_use' && typeof block.id === 'string' && block.id && toolIds.length < 16) {
          toolIds.push(block.id)
        } else if (block.type === 'thinking' && typeof block.signature === 'string' && block.signature && index >= 0) {
          signatures.set(index, block.signature.slice(0, MAX_SIGNATURE_LENGTH))
        } else if (block.type === 'text' && typeof block.text === 'string' && textSample.length < MAX_TEXT_SAMPLE_LENGTH) {
          textSample += block.text.slice(0, MAX_TEXT_SAMPLE_LENGTH - textSample.length)
        }
      } else if (event.type === 'content_block_delta') {
        const index = nonNegativeInt(event.index) ?? -1
        const delta = object(event.delta)
        if (!delta) return
        if (delta.type === 'signature_delta' && typeof delta.signature === 'string' && index >= 0) {
          const current = signatures.get(index) ?? ''
          if (current.length < MAX_SIGNATURE_LENGTH) {
            signatures.set(index, current + delta.signature.slice(0, MAX_SIGNATURE_LENGTH - current.length))
          }
        } else if (delta.type === 'text_delta' && typeof delta.text === 'string' && textSample.length < MAX_TEXT_SAMPLE_LENGTH) {
          textSample += delta.text.slice(0, MAX_TEXT_SAMPLE_LENGTH - textSample.length)
        }
      }
    }

    const onDecodedChunk = (chunk: Buffer) => {
      if (aborted) return
      decodedBytes += chunk.length
      if (decodedBytes > MAX_OBSERVED_BYTES) {
        aborted = true
        return
      }
      const text = decoder.write(chunk)
      if (isJson) {
        pending += text
        return
      }
      pending += text
      if (pending.length > MAX_SSE_LINE_LENGTH && !pending.includes('\n')) {
        aborted = true
        pending = ''
        return
      }
      const lines = pending.split('\n')
      pending = lines.pop() ?? ''
      for (const line of lines) ingestSseLine(line)
    }

    const finalize = () => {
      if (aborted) return
      pending += decoder.end()
      if (isJson) {
        let parsed: Record<string, unknown> | undefined
        try {
          parsed = object(JSON.parse(pending))
        } catch {
          return
        }
        if (!parsed) return
        applyUsageFields(usage, object(parsed.usage))
        if (typeof parsed.content === 'string') {
          textSample = parsed.content.slice(0, MAX_TEXT_SAMPLE_LENGTH)
        } else if (Array.isArray(parsed.content)) {
          parsed.content.forEach((item, index) => {
            const block = object(item)
            if (!block) return
            if (block.type === 'tool_use' && typeof block.id === 'string' && block.id && toolIds.length < 16) {
              toolIds.push(block.id)
            } else if (block.type === 'thinking' && typeof block.signature === 'string' && block.signature) {
              signatures.set(index, block.signature.slice(0, MAX_SIGNATURE_LENGTH))
            } else if (block.type === 'text' && typeof block.text === 'string' && textSample.length < MAX_TEXT_SAMPLE_LENGTH) {
              textSample += block.text.slice(0, MAX_TEXT_SAMPLE_LENGTH - textSample.length)
            }
          })
        }
      } else if (pending.trim()) {
        ingestSseLine(pending)
      }

      const total = totalUsageTokens(usage)
      if (total === undefined) return
      const keys: string[] = [
        ...toolIds.map(id => `tool:${id}`),
        ...[...signatures.values()].filter(Boolean).map(sig => `sig:${sig}`)
      ]
      const hashed = textKey(textSample)
      if (hashed) keys.push(hashed)
      for (const key of keys) this.#remember(key, total)
    }

    if (decompress) {
      const compressedChunks: Buffer[] = []
      let compressedSize = 0
      incoming.on('data', (chunk: Buffer) => {
        if (aborted) return
        const buf = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk)
        compressedSize += buf.length
        if (compressedSize > MAX_OBSERVED_BYTES) {
          aborted = true
          compressedChunks.length = 0
          return
        }
        compressedChunks.push(buf)
      })
      incoming.once('end', () => {
        if (aborted) return
        try {
          const raw = Buffer.concat(compressedChunks, compressedSize)
          compressedChunks.length = 0
          const decoded = decompress(raw, { maxOutputLength: MAX_OBSERVED_BYTES })
          onDecodedChunk(decoded)
          finalize()
        } catch {
          /* Best-effort telemetry tap never disrupts stream delivery. */
        }
      })
      return
    }

    incoming.on('data', (chunk: Buffer) => {
      try { onDecodedChunk(Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk)) } catch { aborted = true }
    })
    incoming.once('end', () => {
      try { finalize() } catch { /* Best-effort telemetry tap never disrupts stream delivery. */ }
    })
  }

  #remember(key: string, usedTokens: number): void {
    if (this.#byKey.has(key)) this.#byKey.delete(key)
    this.#byKey.set(key, usedTokens)
    while (this.#byKey.size > MAX_OBSERVED_KEYS) {
      const oldest = this.#byKey.keys().next().value
      if (oldest === undefined) break
      this.#byKey.delete(oldest)
    }
  }
}
