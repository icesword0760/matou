import { describe, expect, it } from 'vitest'
import { convertDeepSeekInlineTools, isDeepSeekMessagesDestination } from './deepseek-inline-tools'

const definition = { name: 'lookup', description: 'Look up a value', input_schema: { type: 'object', properties: {} }, eager_input_streaming: true }
const baseTool = { name: 'search', input_schema: { type: 'object' } }
const addition = (tool = definition) => ({ type: 'tool_addition', tool: { type: 'tool_definition', definition: tool }, cache_control: { type: 'ephemeral' } })
const request = (blocks = [addition()]) => ({ model: 'deepseek-flash', tools: [baseTool], messages: [{ role: 'system', content: blocks }] })

describe('DeepSeek inline tool compatibility', () => {
  it('promotes custom definitions and preserves block fields and original data', () => {
    const source = request()
    const before = structuredClone(source)
    const result = convertDeepSeekInlineTools(source)
    expect(result).toEqual({ ...source, tools: [baseTool, { ...definition, defer_loading: true }], messages: [{ role: 'system', content: [{ type: 'tool_addition', tool: { type: 'tool_reference', name: 'lookup' }, cache_control: { type: 'ephemeral' } }] }] })
    expect(source).toEqual(before)
    expect(convertDeepSeekInlineTools(result)).toBe(result)
  })

  it('deduplicates equivalent definitions and reuses top-level definitions regardless of deferred state', () => {
    for (const defer_loading of [undefined, true, false]) {
      const source = { ...request([addition(), addition()]), tools: [baseTool, { ...definition, ...(defer_loading === undefined ? {} : { defer_loading }) }] }
      expect(convertDeepSeekInlineTools(source).tools).toEqual(source.tools)
    }
    expect(convertDeepSeekInlineTools(request([addition(), addition()])).tools).toHaveLength(2)
  })

  it('omits custom type when promoting inline client tools and treats omitted and custom types as equivalent', () => {
    const custom = { ...definition, type: 'custom' }
    const source = request([addition(custom), addition()])
    expect(convertDeepSeekInlineTools(source).tools).toEqual([baseTool, { ...definition, defer_loading: true }])
    expect(source.messages[0]!.content[0]!.tool.definition).toHaveProperty('type', 'custom')
    for (const existing of [definition, custom]) {
      const withExisting = { ...source, tools: [baseTool, existing] }
      expect(convertDeepSeekInlineTools(withExisting).tools).toEqual(withExisting.tools)
    }
    expect(() => convertDeepSeekInlineTools({ ...source, tools: [baseTool, { ...definition, type: 'other' }] })).toThrow(/conflicting definitions/)
  })

  it('rejects name conflicts without partially rewriting a request', () => {
    for (const source of [
      { ...request(), tools: [baseTool, { ...definition, input_schema: { type: 'string' } }] },
      request([addition(), addition({ ...definition, description: 'Different definition' })])
    ]) {
      const before = structuredClone(source)
      expect(() => convertDeepSeekInlineTools(source)).toThrow(/conflicting definitions/)
      expect(source).toEqual(before)
    }
  })

  it('rejects unsupported definitions and an all-deferred final tool set', () => {
    const allDeferred = { ...request(), tools: [{ ...baseTool, defer_loading: true }] }
    expect(() => convertDeepSeekInlineTools(allDeferred)).toThrow(/non-deferred tool/)
    for (const invalid of [{ name: 'server', type: 'web_search' }, { input_schema: {} }, { name: 'lookup', input_schema: [] }]) {
      const source = { messages: [{ role: 'system', content: [{ type: 'tool_addition', tool: { type: 'tool_definition', definition: invalid } }] }] }
      expect(() => convertDeepSeekInlineTools(source)).toThrow(/named custom tool/)
    }
  })

  it('does not traverse user content, tool input, or ordinary system data', () => {
    const source = { tools: [baseTool], messages: [
      { role: 'user', content: [addition()] },
      { role: 'assistant', content: [{ type: 'tool_use', name: 'search', input: addition() }] },
      { role: 'system', content: [{ type: 'text', data: addition() }, { type: 'tool_addition', tool: { type: 'tool_reference', name: 'existing' } }] }
    ] }
    expect(convertDeepSeekInlineTools(source)).toBe(source)
  })

  it('matches only the official host and the supported final request paths', () => {
    for (const path of ['/anthropic/v1/messages', '/anthropic/v1/messages/count_tokens']) {
      expect(isDeepSeekMessagesDestination(new URL('https://api.deepseek.com' + path + '?beta=true'))).toBe(true)
    }
    for (const url of ['https://api.deepseek.com.example/anthropic/v1/messages', 'https://example.com/anthropic/v1/messages', 'https://api.deepseek.com/v1/messages', 'https://api.deepseek.com/anthropic/v1/messages/']) {
      expect(isDeepSeekMessagesDestination(new URL(url))).toBe(false)
    }
  })
})
