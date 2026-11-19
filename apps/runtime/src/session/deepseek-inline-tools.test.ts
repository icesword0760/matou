import { describe, expect, it } from 'vitest'
import { convertDeepSeekInlineTools, isDeepSeekMessagesDestination } from './deepseek-inline-tools'

const definition = { name: 'lookup', description: 'Look up a value', input_schema: { type: 'object', properties: {} }, eager_input_streaming: true }
const baseTool = { name: 'search', input_schema: { type: 'object' } }
const addition = (tool = definition) => ({ type: 'tool_addition', tool: { type: 'tool_definition', definition: tool }, cache_control: { type: 'ephemeral' } })
const request = (blocks = [addition()]) => ({ model: 'deepseek-flash', tools: [baseTool], messages: [{ role: 'system', content: blocks }] })
const reference = (name: string, type = 'tool_addition') => ({ type, tool: { type: 'tool_reference', name } })
const removal = (name: string) => reference(name, 'tool_removal')
const availabilityRequest = (events: unknown[], tools: unknown[] = [baseTool]) => ({ tools, messages: [{ role: 'system', content: events }] })

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
      expect(convertDeepSeekInlineTools(withExisting).tools).toEqual([baseTool, definition])
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

  it('folds inline addition and removal into current tools and historical text without mutating input', () => {
    const unchanged = { type: 'text', text: 'Original context' }
    const source = availabilityRequest([unchanged, addition(), removal('lookup')])
    const before = structuredClone(source)
    const result = convertDeepSeekInlineTools(source)
    expect(result.tools).toEqual([baseTool])
    const messages = result.messages as typeof source.messages
    expect(messages[0]!.content[0]).toBe(unchanged)
    expect(messages[0]!.content.slice(1)).toEqual([
      expect.objectContaining({ type: 'text', text: expect.stringContaining('"lookup" was added'), cache_control: { type: 'ephemeral' } }),
      expect.objectContaining({ type: 'text', text: expect.stringContaining('"lookup" was removed') })
    ])
    expect(JSON.stringify(messages)).toContain('currently callable')
    expect(JSON.stringify(messages)).not.toContain('tool_definition')
    expect(source).toEqual(before)
    expect(convertDeepSeekInlineTools(result)).toBe(result)
  })

  it('supports reference additions, removal-only histories, and reactivating known definitions', () => {
    expect(convertDeepSeekInlineTools(availabilityRequest([reference('search'), removal('search')])).tools).toEqual([])
    expect(convertDeepSeekInlineTools(availabilityRequest([removal('search')])).tools).toEqual([])
    expect(convertDeepSeekInlineTools(availabilityRequest([removal('search'), reference('search')], [{ ...baseTool, defer_loading: true }])).tools)
      .toEqual([{ ...baseTool, defer_loading: false }])
    const untouched = { name: 'other', input_schema: {}, defer_loading: true }
    expect(convertDeepSeekInlineTools(availabilityRequest([removal('search'), reference('search')], [baseTool, untouched])).tools)
      .toEqual([{ ...baseTool, defer_loading: false }, untouched])
  })

  it('folds a multi-round three-tool history and preserves old calls and results', () => {
    const three = ['mcp_alpha', 'mcp_beta', 'mcp_gamma'].map(name => ({ name, input_schema: { type: 'object', properties: {} } }))
    const calls = { role: 'assistant', content: [{ type: 'tool_use', id: 'call-1', name: 'mcp_alpha', input: { history: removal('search') } }] }
    const results = { role: 'user', content: [{ type: 'tool_result', tool_use_id: 'call-1', content: 'ok' }] }
    const workflow = { ...definition, name: 'Workflow', type: 'custom' }
    const source = { tools: [baseTool], messages: [
      { role: 'system', content: three.map(tool => addition({ ...definition, ...tool })) },
      calls, results,
      { role: 'system', content: three.map(tool => removal(tool.name)) },
      { role: 'system', content: three.map(tool => addition({ ...definition, ...tool })) },
      { role: 'system', content: [addition(workflow)] }
    ] }
    const result = convertDeepSeekInlineTools(source)
    expect(result.tools).toEqual([baseTool, ...three.map(tool => ({ ...definition, ...tool, defer_loading: false })), { ...definition, name: 'Workflow', defer_loading: false }])
    expect((result.messages as unknown[])[1]).toBe(calls)
    expect((result.messages as unknown[])[2]).toBe(results)
    expect((result.messages as Array<{ role: string; content: unknown[] }>).filter(message => message.role === 'system').flatMap(message => message.content).every(block => (block as { type: string }).type === 'text')).toBe(true)
  })

  it('uses the latest same-name client definition when it is re-added', () => {
    const updated = { ...definition, type: 'custom', input_schema: { type: 'object', properties: { query: { type: 'string' } } } }
    expect(convertDeepSeekInlineTools(availabilityRequest([addition(), removal('lookup'), addition(updated)])).tools)
      .toEqual([baseTool, { ...definition, input_schema: updated.input_schema, defer_loading: false }])
    expect(convertDeepSeekInlineTools(availabilityRequest([addition(), addition(updated), removal('search')])).tools)
      .toEqual([{ ...definition, input_schema: updated.input_schema, defer_loading: false }])
  })

  it('rejects ambiguous, unknown, and non-client events atomically', () => {
    const malformedRemoval = { type: 'tool_removal', tool: { type: 'server_tool', name: 'search' } }
    const unsupportedAddition = { type: 'tool_addition', tool: { type: 'tool_definition', definition: { name: 'web', type: 'web_search' } } }
    for (const source of [
      availabilityRequest([removal('unknown')]),
      availabilityRequest([reference('unknown'), removal('search')]),
      availabilityRequest([malformedRemoval]),
      availabilityRequest([unsupportedAddition, removal('search')]),
      availabilityRequest([removal('search')], [baseTool, baseTool]),
      availabilityRequest([removal('search')], [baseTool, { type: 'web_search', name: 'web' }])
    ]) {
      const before = structuredClone(source)
      expect(() => convertDeepSeekInlineTools(source)).toThrow(/DeepSeek/)
      expect(source).toEqual(before)
    }
  })

  it('preserves tool choice and rejects a forced removed tool or all-deferred remainder', () => {
    const source = { ...availabilityRequest([removal('search')]), tool_choice: { type: 'auto', disable_parallel_tool_use: true } }
    expect(convertDeepSeekInlineTools(source).tool_choice).toBe(source.tool_choice)
    expect(() => convertDeepSeekInlineTools({ ...source, tool_choice: { type: 'tool', name: 'search' } })).toThrow(/tool_choice/)
    const deferred = { ...definition, defer_loading: true }
    expect(() => convertDeepSeekInlineTools(availabilityRequest([removal('search')], [baseTool, deferred]))).toThrow(/non-deferred/)
  })

  it('normalizes top-level custom tools even without availability events while retaining other fields and server types', () => {
    const client = { ...definition, type: 'custom', defer_loading: false, cache_control: { type: 'ephemeral' } }
    const server = { type: 'web_search_20250305', name: 'web_search' }
    const source = { tools: [client, baseTool, server], messages: [{ role: 'user', content: 'continue' }] }
    const before = structuredClone(source)
    const result = convertDeepSeekInlineTools(source)
    expect(result.tools).toEqual([{ ...definition, defer_loading: false, cache_control: { type: 'ephemeral' } }, baseTool, server])
    expect((result.tools as unknown[])[1]).toBe(baseTool)
    expect((result.tools as unknown[])[2]).toBe(server)
    expect(source).toEqual(before)
    expect(convertDeepSeekInlineTools(result)).toBe(result)
  })

  it('replaces only unavailable nested tool references and keeps declared references and sibling data intact', () => {
    const known = { type: 'tool_reference', tool_name: 'search' }
    const missing = { type: 'tool_reference', tool_name: 'retired_"tool', cache_control: { type: 'ephemeral' } }
    const text = { type: 'text', text: 'original' }
    const image = { type: 'image', source: { type: 'base64', media_type: 'image/png', data: 'image-bytes' } }
    const arbitrary = { type: 'tool_use', name: 'search', input: missing }
    const source = { tools: [baseTool], messages: [
      { role: 'assistant', content: [arbitrary] },
      { role: 'user', content: [{ type: 'tool_result', tool_use_id: 'call', content: [text, missing, image, known] }, missing] }
    ] }
    const before = structuredClone(source)
    const result = convertDeepSeekInlineTools(source)
    const messages = result.messages as typeof source.messages
    expect(messages[0]).toBe(source.messages[0])
    const user = messages[1]!.content
    expect(user[1]).toBe(missing)
    const blocks = (user[0] as { content: unknown[] }).content
    expect(blocks[0]).toBe(text)
    expect(blocks[1]).toEqual({ type: 'text', cache_control: missing.cache_control, text: expect.stringContaining(JSON.stringify(missing.tool_name)) })
    expect(blocks[2]).toBe(image)
    expect(blocks[3]).toBe(known)
    expect(source).toEqual(before)
    expect(convertDeepSeekInlineTools(result)).toBe(result)
  })

  it('uses final availability for nested references removed or re-added later', () => {
    const known = { type: 'tool_reference', tool_name: 'search' }
    const history = { role: 'user', content: [{ type: 'tool_result', tool_use_id: 'call', content: [known] }] }
    const source = { tools: [baseTool], messages: [history, { role: 'system', content: [removal('search')] }] }
    const removed = convertDeepSeekInlineTools(source)
    expect(JSON.stringify(removed.messages)).toContain('Historical tool discovery')
    const readded = convertDeepSeekInlineTools({ ...source, messages: [...source.messages, { role: 'system', content: [reference('search')] }] })
    expect((readded.messages as unknown[])[0]).toBe(history)
    const noChanges = { tools: [baseTool], messages: [history] }
    expect(convertDeepSeekInlineTools(noChanges)).toBe(noChanges)
  })

  it('normalizes schema patterns in final top-level and promoted inline tools without changing history', () => {
    const input_schema = { type: 'object', properties: { file_paths: { type: 'array', items: { type: 'string', pattern: String.raw`^[^\0]*$` } } } }
    const input = { pattern: String.raw`\0` }
    const topLevel = { ...definition, input_schema }
    const inline = { ...definition, name: 'inline_tool', input_schema }
    const source = {
      tools: [topLevel],
      messages: [
        { role: 'system', content: [addition(inline)] },
        { role: 'assistant', content: [{ type: 'tool_use', id: 'one', name: 'lookup', input }] }
      ]
    }
    const before = structuredClone(source)
    const result = convertDeepSeekInlineTools(source)
    const tools = result.tools as Array<typeof topLevel>
    expect(tools.map(tool => tool.input_schema.properties.file_paths.items.pattern)).toEqual([String.raw`^[^\x00]*$`, String.raw`^[^\x00]*$`])
    expect((result.messages as unknown[])[1]).toBe(source.messages[1])
    expect(source).toEqual(before)
    expect(convertDeepSeekInlineTools(result)).toBe(result)
    const onlyTopLevel = { tools: [topLevel], messages: [{ role: 'user', content: 'continue' }] }
    expect((convertDeepSeekInlineTools(onlyTopLevel).tools as Array<typeof topLevel>)[0]!.input_schema.properties.file_paths.items.pattern).toBe(String.raw`^[^\x00]*$`)
  })
})
