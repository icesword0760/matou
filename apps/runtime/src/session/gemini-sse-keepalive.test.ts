import { describe, expect, it } from 'vitest'
import {
  ANTHROPIC_SSE_PING_EVENT,
  createGeminiSseKeepAliveStream,
  endsAtSseEventBoundary,
  isGeminiModelId
} from './gemini-sse-keepalive'

function sleep(ms: number): Promise<void> {
  return new Promise(resolve => setTimeout(resolve, ms))
}

describe('gemini-sse-keepalive', () => {
  it('identifies Gemini model IDs while excluding DeepSeek, Claude, and GPT models', () => {
    expect(isGeminiModelId('gemini-4-neon-high')).toBe(true)
    expect(isGeminiModelId('matou-antigravity/gemini-4-neon-high')).toBe(true)
    expect(isGeminiModelId('google/gemini-2.5-pro')).toBe(true)

    expect(isGeminiModelId('deepseek-flash')).toBe(false)
    expect(isGeminiModelId('claude-opus-5-5-high')).toBe(false)
    expect(isGeminiModelId('matou-antigravity/claude-opus-5-5[1M]')).toBe(false)
    expect(isGeminiModelId('gpt-6.2-sol')).toBe(false)
    expect(isGeminiModelId('matou-chatgpt/gpt-6.2-sol')).toBe(false)
    expect(isGeminiModelId(undefined)).toBe(false)
  })

  it('detects LF and CRLF SSE event boundaries accurately', () => {
    expect(endsAtSseEventBoundary(Buffer.from('data: {}\n\n'))).toBe(true)
    expect(endsAtSseEventBoundary(Buffer.from('data: {}\r\n\r\n'))).toBe(true)
    expect(endsAtSseEventBoundary(Buffer.from('data: {}\n'))).toBe(false)
    expect(endsAtSseEventBoundary(Buffer.from('data: {"partial'))).toBe(false)
  })

  it('never injects pings mid-frame and resumes once the SSE event boundary completes', async () => {
    const stream = createGeminiSseKeepAliveStream({ intervalMs: 20, maxPings: 3 })
    const received: string[] = []
    stream.on('data', (chunk: Buffer) => received.push(chunk.toString('utf8')))

    // Incomplete SSE event (no trailing \n\n) -> must NOT inject a ping mid-JSON!
    stream.write('event: content_block_delta\ndata: {"partial":"hello')
    await sleep(55)
    expect(received.join('')).toBe('event: content_block_delta\ndata: {"partial":"hello')

    // Complete the SSE event across two chunks ("\n" then "\n") -> boundary reached, pings start
    stream.write(' world"}\n')
    await sleep(30)
    expect(received.join('')).toBe('event: content_block_delta\ndata: {"partial":"hello world"}\n')

    stream.write('\n')
    await sleep(55)
    const combined = received.join('')
    expect(combined.startsWith('event: content_block_delta\ndata: {"partial":"hello world"}\n\n')).toBe(true)
    expect(combined).toContain(ANTHROPIC_SSE_PING_EVENT)

    stream.end('event: message_stop\ndata: {"type":"message_stop"}\n\n')
    await new Promise<void>(resolve => stream.once('end', resolve))
    expect(received.join('').endsWith('event: message_stop\ndata: {"type":"message_stop"}\n\n')).toBe(true)
  })

  it('caps consecutive pings during a dead silence and resets count after real upstream data', async () => {
    const stream = createGeminiSseKeepAliveStream({ intervalMs: 15, maxPings: 2 })
    const chunks: string[] = []
    stream.on('data', (chunk: Buffer) => chunks.push(chunk.toString('utf8')))

    await sleep(75)
    expect(chunks.filter(chunk => chunk === ANTHROPIC_SSE_PING_EVENT)).toHaveLength(2)

    // Real upstream event resets the consecutive ping counter
    stream.write('event: message_start\ndata: {"type":"message_start"}\n\n')
    await sleep(60)
    expect(chunks.filter(chunk => chunk === ANTHROPIC_SSE_PING_EVENT)).toHaveLength(4)

    stream.destroy()
  })
})
