import { describe, expect, it } from 'vitest'
import { normalizeDeepSeekToolSchema } from './deepseek-tool-schema'

describe('DeepSeek tool schema regex compatibility', () => {
  it('normalizes the Artifact file-path pattern with equivalent NUL matching semantics', () => {
    const schema = { type: 'object', properties: { file_paths: { type: 'array', items: { type: 'string', pattern: String.raw`^[^\0]*$` } } } }
    const before = structuredClone(schema)
    const result = normalizeDeepSeekToolSchema(schema) as typeof schema
    const pattern = result.properties.file_paths.items.pattern
    expect(pattern).toBe(String.raw`^[^\x00]*$`)
    for (const path of ['src/file.ts', '/a/b', '', 'a\u0000b']) {
      expect(new RegExp(pattern).test(path)).toBe(new RegExp(schema.properties.file_paths.items.pattern).test(path))
    }
    expect(new RegExp(pattern).test('a\u0000b')).toBe(false)
    expect(new RegExp(pattern).test('/normal/path')).toBe(true)
    expect(schema).toEqual(before)
    expect(normalizeDeepSeekToolSchema(result)).toBe(result)
  })

  it('distinguishes escaped slashes, standalone NUL escapes, and numeric octal escapes', () => {
    for (const [input, expected] of [
      [String.raw`\0`, String.raw`\x00`],
      [String.raw`\\0`, String.raw`\\0`],
      [String.raw`\\\0`, String.raw`\\\x00`],
      [String.raw`\\\\0`, String.raw`\\\\0`],
      [String.raw`\00`, String.raw`\00`],
      [String.raw`\012`, String.raw`\012`],
      [String.raw`\09`, String.raw`\09`],
      [String.raw`\0a\0$`, String.raw`\x00a\x00$`],
      [String.raw`^(?!\.\.)\d+$`, String.raw`^(?!\.\.)\d+$`]
    ]) expect(normalizeDeepSeekToolSchema({ pattern: input }).pattern).toBe(expected)
    const unchanged = { pattern: String.raw`^(?!\.\.)\d+$` }
    expect(normalizeDeepSeekToolSchema(unchanged)).toBe(unchanged)
  })

  it('visits schema maps, singleton schemas, arrays, and legacy tuple items', () => {
    const child = { type: 'string', pattern: String.raw`\0` }
    for (const key of ['properties', 'patternProperties', '$defs', 'definitions', 'dependentSchemas']) {
      const result = normalizeDeepSeekToolSchema({ [key]: { [String.raw`\0`]: child } })
      expect(result).toEqual({ [key]: { [String.raw`\0`]: { type: 'string', pattern: String.raw`\x00` } } })
    }
    for (const key of ['items', 'additionalItems', 'additionalProperties', 'unevaluatedItems', 'unevaluatedProperties', 'propertyNames', 'contains', 'not', 'if', 'then', 'else', 'contentSchema']) {
      expect(normalizeDeepSeekToolSchema({ [key]: child })).toEqual({ [key]: { type: 'string', pattern: String.raw`\x00` } })
    }
    for (const key of ['allOf', 'anyOf', 'oneOf', 'prefixItems', 'items']) {
      expect(normalizeDeepSeekToolSchema({ [key]: [child, true] })).toEqual({ [key]: [{ type: 'string', pattern: String.raw`\x00` }, true] })
    }
  })

  it('does not interpret instance data or descriptions as schemas', () => {
    const data = { pattern: String.raw`\0`, properties: { nested: { pattern: String.raw`\0` } } }
    const schema = { type: 'object', description: String.raw`pattern: \0`, default: data, examples: [data], enum: [data], const: data, custom: data }
    expect(normalizeDeepSeekToolSchema(schema)).toBe(schema)
  })
})
