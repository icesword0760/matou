import { isDeepStrictEqual } from 'node:util'
import { normalizeDeepSeekToolSchema } from './deepseek-tool-schema'

export class DeepSeekInlineToolCompatibilityError extends Error {}

export function isDeepSeekMessagesDestination(destination: URL): boolean {
  return destination.hostname === 'api.deepseek.com' &&
    ['/anthropic/v1/messages', '/anthropic/v1/messages/count_tokens'].includes(destination.pathname)
}

function object(value: unknown): Record<string, unknown> | undefined {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
    ? value as Record<string, unknown> : undefined
}

function comparableDefinition(tool: Record<string, unknown>): Record<string, unknown> {
  const { defer_loading: _deferred, ...definition } = tool
  // The official Anthropic-compatible endpoint represents client tools without type: 'custom'.
  if (definition.type === 'custom') delete definition.type
  return definition
}

function isClientDefinition(definition: Record<string, unknown> | undefined): definition is Record<string, unknown> & { name: string } {
  return Boolean(definition && typeof definition.name === 'string' && definition.name.trim() &&
    object(definition.input_schema) && (definition.type === undefined || definition.type === 'custom'))
}

function foldToolAvailability(message: Record<string, unknown>, messages: unknown[]): Record<string, unknown> {
  if (message.tools !== undefined && !Array.isArray(message.tools)) {
    throw new DeepSeekInlineToolCompatibilityError('DeepSeek tool availability conversion requires a tools array.')
  }
  const registry = new Map<string, { definition: Record<string, unknown>; active: boolean; touched: boolean }>()
  for (const value of (message.tools as unknown[] | undefined) ?? []) {
    const definition = object(value)
    if (!isClientDefinition(definition)) {
      throw new DeepSeekInlineToolCompatibilityError('DeepSeek tool availability conversion supports only named client tools with input_schema.')
    }
    if (registry.has(definition.name)) {
      throw new DeepSeekInlineToolCompatibilityError('DeepSeek cannot resolve duplicate top-level tool names during tool availability conversion.')
    }
    registry.set(definition.name, { definition, active: true, touched: false })
  }
  const convertedMessages = messages.map(value => {
    const entry = object(value)
    if (entry?.role !== 'system' || !Array.isArray(entry.content)) return value
    return { ...entry, content: entry.content.map(value => {
      const block = object(value)
      if (block?.type !== 'tool_addition' && block?.type !== 'tool_removal') return value
      const tool = object(block.tool)
      const addition = block.type === 'tool_addition'
      let name: string
      if (addition && tool?.type === 'tool_definition') {
        const definition = object(tool.definition)
        if (!isClientDefinition(definition)) {
          throw new DeepSeekInlineToolCompatibilityError('DeepSeek tool availability conversion supports only named client tool definitions with input_schema.')
        }
        name = definition.name
        registry.set(name, { definition, active: true, touched: true })
      } else {
        if (tool?.type !== 'tool_reference' || typeof tool.name !== 'string' || !tool.name.trim()) {
          throw new DeepSeekInlineToolCompatibilityError('DeepSeek cannot resolve this tool availability reference.')
        }
        name = tool.name
        const registered = registry.get(name)
        if (!registered) {
          throw new DeepSeekInlineToolCompatibilityError('DeepSeek cannot resolve an unknown tool availability reference.')
        }
        registry.set(name, { ...registered, active: addition, touched: true })
      }
      const { tool: _tool, ...fields } = block
      return { ...fields, type: 'text', text: `Historical tool availability change: ${JSON.stringify(name)} was ${addition ? 'added' : 'removed'}. The tools list in this request determines which tools are currently callable.` }
    }) }
  })
  const choice = object(message.tool_choice)
  if (choice?.type === 'tool' && typeof choice.name === 'string' && registry.get(choice.name)?.active === false) {
    throw new DeepSeekInlineToolCompatibilityError('DeepSeek cannot force tool_choice to a tool removed from the current tools list.')
  }
  const tools = [...registry.values()].filter(tool => tool.active).map(tool =>
    tool.touched ? { ...comparableDefinition(tool.definition), defer_loading: false } : tool.definition)
  if (tools.length && !tools.some(tool => tool.defer_loading !== true)) {
    throw new DeepSeekInlineToolCompatibilityError('DeepSeek tool availability conversion requires at least one non-deferred tool when tools remain.')
  }
  return { ...message, tools, messages: convertedMessages }
}

/** Converts only system tool availability events. The original request remains untouched, including on failure. */
function convertToolAvailability(message: Record<string, unknown>): Record<string, unknown> {
  if (!Array.isArray(message.messages)) return message
  if (message.messages.some(value => {
    const entry = object(value)
    return entry?.role === 'system' && Array.isArray(entry.content) &&
      entry.content.some(value => object(value)?.type === 'tool_removal')
  })) return foldToolAvailability(message, message.messages)
  const additions: Array<{ block: Record<string, unknown>; definition: Record<string, unknown> }> = []
  for (const value of message.messages) {
    const entry = object(value)
    if (entry?.role !== 'system' || !Array.isArray(entry.content)) continue
    for (const value of entry.content) {
      const block = object(value)
      const tool = object(block?.tool)
      if (block?.type !== 'tool_addition' || tool?.type !== 'tool_definition') continue
      const definition = object(tool.definition)
      if (!isClientDefinition(definition)) {
        throw new DeepSeekInlineToolCompatibilityError('DeepSeek cannot convert this inline tool definition: a named custom tool with input_schema is required.')
      }
      additions.push({ block, definition })
    }
  }
  if (!additions.length) return message
  if (message.tools !== undefined && !Array.isArray(message.tools)) {
    throw new DeepSeekInlineToolCompatibilityError('DeepSeek inline tool conversion requires a tools array.')
  }
  const tools: unknown[] = Array.isArray(message.tools) ? [...message.tools] : []
  const replacements = new Map<Record<string, unknown>, Record<string, unknown>>()
  for (const { block, definition } of additions) {
    const matches = tools.map(object).filter(tool => tool?.name === definition.name)
    if (matches.some(tool => !isDeepStrictEqual(comparableDefinition(tool!), comparableDefinition(definition)))) {
      throw new DeepSeekInlineToolCompatibilityError('DeepSeek cannot convert inline tools with conflicting definitions for the same name.')
    }
    if (!matches.length) tools.push({ ...comparableDefinition(definition), defer_loading: true })
    replacements.set(block, { ...block, tool: { type: 'tool_reference', name: definition.name } })
  }
  if (!tools.some(value => { const tool = object(value); return tool && tool.defer_loading !== true })) {
    throw new DeepSeekInlineToolCompatibilityError('DeepSeek inline tool conversion requires at least one existing non-deferred tool.')
  }
  return {
    ...message,
    tools,
    messages: message.messages.map(value => {
      const entry = object(value)
      if (entry?.role !== 'system' || !Array.isArray(entry.content)) return value
      return { ...entry, content: entry.content.map(block => replacements.get(block) ?? block) }
    })
  }
}

/** Normalizes known client-tool compatibility fields without mutating stored history or arbitrary tool input. */
export function convertDeepSeekInlineTools(message: Record<string, unknown>): Record<string, unknown> {
  let normalized = message
  if (Array.isArray(message.tools)) {
    const tools = message.tools.map(value => {
      const definition = object(value)
      if (!isClientDefinition(definition) || definition.type !== 'custom') return value
      const { type: _type, ...clientTool } = definition
      return clientTool
    })
    if (tools.some((tool, index) => tool !== (message.tools as unknown[])[index])) normalized = { ...normalized, tools }
  }
  normalized = convertToolAvailability(normalized)
  if (Array.isArray(normalized.tools)) {
    const tools = normalized.tools.map(value => {
      const definition = object(value)
      if (!isClientDefinition(definition)) return value
      const schema = definition.input_schema as Record<string, unknown>
      const input_schema = normalizeDeepSeekToolSchema(schema)
      return input_schema === schema ? value : { ...definition, input_schema }
    })
    if (tools.some((tool, index) => tool !== (normalized.tools as unknown[])[index])) normalized = { ...normalized, tools }
  }
  if (!Array.isArray(normalized.messages)) return normalized
  const available = new Set(Array.isArray(normalized.tools)
    ? normalized.tools.map(value => object(value)?.name).filter(name => typeof name === 'string') : [])
  const messages = normalized.messages.map(value => {
    const entry = object(value)
    if (entry?.role !== 'user' || !Array.isArray(entry.content)) return value
    const originalContent = entry.content
    const content = originalContent.map(value => {
      const result = object(value)
      if (result?.type !== 'tool_result' || !Array.isArray(result.content)) return value
      const originalBlocks = result.content
      const blocks = originalBlocks.map(value => {
        const reference = object(value)
        if (reference?.type !== 'tool_reference' || typeof reference.tool_name !== 'string' || available.has(reference.tool_name)) return value
        const { tool_name: _name, ...fields } = reference
        return { ...fields, type: 'text', text: `Historical tool discovery: ${JSON.stringify(reference.tool_name)} was previously discovered, but its definition is not provided in the current tools list and it is not callable.` }
      })
      return blocks.some((block, index) => block !== originalBlocks[index]) ? { ...result, content: blocks } : value
    })
    return content.some((block, index) => block !== originalContent[index]) ? { ...entry, content } : value
  })
  return messages.some((entry, index) => entry !== (normalized.messages as unknown[])[index]) ? { ...normalized, messages } : normalized
}
