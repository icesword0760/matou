import { PassThrough } from 'node:stream'
import { gzipSync } from 'node:zlib'
import type { IncomingMessage } from 'node:http'
import { describe, expect, it } from 'vitest'
import { DeepSeekContextUsageTracker, formatTokenUsageSystemReminder } from './deepseek-token-usage'

function createMockResponse(headers: Record<string, string>, chunks: Array<string | Buffer>): IncomingMessage {
  const passThrough = new PassThrough()
  const stream = Object.assign(passThrough, { headers, complete: false }) as unknown as IncomingMessage
  queueMicrotask(() => {
    for (const chunk of chunks) {
      passThrough.write(typeof chunk === 'string' ? Buffer.from(chunk, 'utf8') : chunk)
    }
    stream.complete = true
    passThrough.end()
  })
  return stream
}

async function flushStream(stream: IncomingMessage): Promise<void> {
  await new Promise<void>(resolve => stream.once('end', resolve))
}

describe('DeepSeekContextUsageTracker', () => {
  it('tracks SSE usage across message_start and message_delta and attaches to matching tool_result turns while preserving cache_control', async () => {
    const tracker = new DeepSeekContextUsageTracker()
    const sse = [
      'event: message_start\n',
      'data: {"type":"message_start","message":{"id":"msg_1","role":"assistant","content":[],"usage":{"input_tokens":246,"cache_creation_input_tokens":0,"cache_read_input_tokens":382464,"output_tokens":1}}}\n\n',
      'event: content_block_start\n',
      'data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"call_bash_1","name":"Bash","input":{}}}\n\n',
      'event: message_delta\n',
      'data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":458}}\n\n'
    ]
    const response = createMockResponse({ 'content-type': 'text/event-stream; charset=utf-8' }, sse)
    tracker.observeResponse(response)
    response.resume()
    await flushStream(response)

    const toolResultBlock = {
      type: 'tool_result',
      tool_use_id: 'call_bash_1',
      content: 'ok',
      cache_control: { type: 'ephemeral' }
    }
    const request = {
      model: 'deepseek-flash',
      max_tokens: 32_000,
      tools: [{ name: 'Bash', input_schema: { type: 'object' } }],
      messages: [
        { role: 'user', content: 'run tests' },
        { role: 'assistant', content: [{ type: 'tool_use', id: 'call_bash_1', name: 'Bash', input: {} }] },
        { role: 'user', content: [toolResultBlock] }
      ]
    }
    const attached = tracker.attach(request, { sessionContextWindow: 872_000 })
    expect(attached).not.toBe(request)
    const messages = attached.messages as Array<{ role: string; content: unknown[] }>
    expect(messages[2]!.content[0]).toBe(toolResultBlock)
    expect(messages[2]!.content[1]).toEqual({
      type: 'text',
      text: formatTokenUsageSystemReminder(383_168, 840_000)
    })
  })

  it('matches text-only Stop hook continuations and isolates unrelated 1-turn utility calls', async () => {
    const tracker = new DeepSeekContextUsageTracker()
    const body = JSON.stringify({
      id: 'msg_stop_1',
      type: 'message',
      role: 'assistant',
      content: [
        { type: 'thinking', thinking: 'checking remaining items', signature: 'sig-stop-1' },
        { type: 'text', text: '18 passed, 19 remaining.' }
      ],
      usage: { input_tokens: 300, cache_creation_input_tokens: 0, cache_read_input_tokens: 412_700, output_tokens: 200 }
    })
    const compressed = createMockResponse(
      { 'content-type': 'application/json', 'content-encoding': 'gzip' },
      [gzipSync(Buffer.from(body, 'utf8'))]
    )
    tracker.observeResponse(compressed)
    compressed.resume()
    await flushStream(compressed)

    const stopHookRequest = {
      model: 'deepseek-flash',
      max_tokens: 32_000,
      messages: [
        { role: 'user', content: 'fix all tests' },
        { role: 'assistant', content: [{ type: 'text', text: '18 passed, 19 remaining.' }] },
        { role: 'user', content: 'Stop hook feedback: 19 tests still failing' }
      ]
    }
    const attached = tracker.attach(stopHookRequest, { sessionContextWindow: 872_000 })
    const lastUser = (attached.messages as Array<{ content: unknown }>)[2]!
    expect(lastUser.content).toEqual([
      { type: 'text', text: 'Stop hook feedback: 19 tests still failing' },
      { type: 'text', text: formatTokenUsageSystemReminder(413_200, 840_000) }
    ])

    const utilityRequest = {
      model: 'deepseek-flash',
      max_tokens: 256,
      messages: [{ role: 'user', content: 'Generate a short title for this session' }]
    }
    expect(tracker.attach(utilityRequest, { sessionContextWindow: 872_000 })).toBe(utilityRequest)
  })

  it('falls back to resumed session HUD usage on the first resumed turn and stops using stale HUD usage after compaction', () => {
    const tracker = new DeepSeekContextUsageTracker()
    const resumedRequest = {
      model: 'deepseek-flash',
      max_tokens: 32_000,
      tools: [{ name: 'Bash', input_schema: { type: 'object' } }],
      messages: [
        { role: 'user', content: 'earlier prompt' },
        { role: 'assistant', content: [{ type: 'text', text: 'earlier reply before restart' }] },
        { role: 'user', content: '继续' }
      ]
    }
    const attached = tracker.attach(resumedRequest, {
      sessionContextWindow: 872_000,
      sessionUsage: { usedTokens: 417_776, contextWindowSize: 872_000 }
    })
    expect((attached.messages as Array<{ content: unknown }>)[2]!.content).toEqual([
      { type: 'text', text: '继续' },
      { type: 'text', text: formatTokenUsageSystemReminder(417_776, 840_000) }
    ])

    // Does not duplicate when already present
    expect(tracker.attach(attached, {
      sessionContextWindow: 872_000,
      sessionUsage: { usedTokens: 417_776, contextWindowSize: 872_000 }
    })).toBe(attached)

    // After compaction, stale pre-compaction HUD fallback must not be injected
    tracker.noteCompaction()
    expect(tracker.attach(resumedRequest, {
      sessionContextWindow: 872_000,
      sessionUsage: { usedTokens: 417_776, contextWindowSize: 872_000 }
    })).toBe(resumedRequest)
  })
})
