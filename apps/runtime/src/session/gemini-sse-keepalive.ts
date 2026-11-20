import { Transform, type TransformCallback } from 'node:stream'

export const GEMINI_SSE_KEEPALIVE_INTERVAL_MS = 15_000
export const GEMINI_SSE_MAX_CONSECUTIVE_PINGS = 30
export const ANTHROPIC_SSE_PING_EVENT = 'event: ping\ndata: {"type":"ping"}\n\n'

const PING_FRAME_BUFFER = Buffer.from(ANTHROPIC_SSE_PING_EVENT, 'utf8')

export interface GeminiSseKeepAliveOptions {
  intervalMs?: number
  maxPings?: number
  now?: () => number
}

export function isGeminiModelId(model: string | undefined): boolean {
  if (typeof model !== 'string') return false
  return /(?:^|\/)gemini-/i.test(model.trim())
}

export function endsAtSseEventBoundary(tail: Buffer): boolean {
  const len = tail.length
  if (len >= 2 && tail[len - 2] === 0x0a && tail[len - 1] === 0x0a) return true
  if (
    len >= 4 &&
    tail[len - 4] === 0x0d &&
    tail[len - 3] === 0x0a &&
    tail[len - 2] === 0x0d &&
    tail[len - 1] === 0x0a
  ) {
    return true
  }
  return false
}

/**
 * Injects Anthropic-compatible SSE `ping` events during long silent gaps on
 * Gemini streams (e.g. while upstream buffers a large `functionCall` payload),
 * strictly at clean SSE event boundaries (`\n\n` or `\r\n\r\n`).
 */
export function createGeminiSseKeepAliveStream(options: GeminiSseKeepAliveOptions = {}): Transform {
  const intervalMs = Math.max(10, options.intervalMs ?? GEMINI_SSE_KEEPALIVE_INTERVAL_MS)
  const maxPings = Math.max(1, options.maxPings ?? GEMINI_SSE_MAX_CONSECUTIVE_PINGS)
  const now = options.now ?? Date.now

  let tail: Buffer = Buffer.alloc(0)
  let atEventBoundary = true
  let lastActivityAt = now()
  let pingCount = 0
  let stopped = false
  let timer: ReturnType<typeof setTimeout> | undefined

  const clearTimer = () => {
    if (timer !== undefined) {
      clearTimeout(timer)
      timer = undefined
    }
  }

  let stream!: Transform

  const armTimer = (delayMs: number) => {
    clearTimer()
    if (stopped || pingCount >= maxPings) return
    timer = setTimeout(() => {
      timer = undefined
      if (stopped || stream.destroyed || stream.writableEnded) return
      const elapsed = now() - lastActivityAt
      if (elapsed < intervalMs) {
        armTimer(Math.max(1, intervalMs - elapsed))
        return
      }
      if (atEventBoundary && pingCount < maxPings) {
        pingCount += 1
        lastActivityAt = now()
        stream.push(PING_FRAME_BUFFER)
      }
      if (!stopped && pingCount < maxPings) {
        armTimer(intervalMs)
      }
    }, Math.max(1, delayMs))
    timer.unref?.()
  }

  stream = new Transform({
    transform(chunk: Buffer, _encoding: BufferEncoding, callback: TransformCallback) {
      const buf = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk)
      if (buf.length > 0) {
        lastActivityAt = now()
        pingCount = 0
        const combined = buf.length >= 4 ? buf.subarray(buf.length - 4) : Buffer.concat([tail, buf])
        tail = Buffer.from(combined.length > 4 ? combined.subarray(combined.length - 4) : combined)
        atEventBoundary = endsAtSseEventBoundary(tail)
        if (!stopped && timer === undefined) armTimer(intervalMs)
      }
      callback(null, buf)
    },
    flush(callback: TransformCallback) {
      stopped = true
      clearTimer()
      callback()
    },
    destroy(error: Error | null, callback: (error: Error | null) => void) {
      stopped = true
      clearTimer()
      callback(error)
    }
  })

  armTimer(intervalMs)
  return stream
}
