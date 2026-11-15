import { isDeepStrictEqual } from 'node:util'

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

/** Converts only system tool additions. The original request remains untouched, including on failure. */
export function convertDeepSeekInlineTools(message: Record<string, unknown>): Record<string, unknown> {
  if (!Array.isArray(message.messages)) return message
  const additions: Array<{ block: Record<string, unknown>; definition: Record<string, unknown> }> = []
  for (const value of message.messages) {
    const entry = object(value)
    if (entry?.role !== 'system' || !Array.isArray(entry.content)) continue
    for (const value of entry.content) {
      const block = object(value)
      const tool = object(block?.tool)
      if (block?.type !== 'tool_addition' || tool?.type !== 'tool_definition') continue
      const definition = object(tool.definition)
      if (!definition || typeof definition.name !== 'string' || !definition.name.trim() ||
          !object(definition.input_schema) || (definition.type !== undefined && definition.type !== 'custom')) {
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
