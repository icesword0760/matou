type SchemaObject = Record<string, unknown>

function object(value: unknown): SchemaObject | undefined {
  return typeof value === 'object' && value !== null && !Array.isArray(value) ? value as SchemaObject : undefined
}

function normalizeNullEscape(pattern: string): string {
  let result = ''
  for (let index = 0; index < pattern.length;) {
    if (pattern[index] !== '\\') {
      result += pattern[index++]
      continue
    }
    const start = index
    while (pattern[index] === '\\') index++
    const count = index - start
    if (count % 2 === 1 && pattern[index] === '0' && !/[0-9]/.test(pattern[index + 1] ?? '')) {
      result += '\\'.repeat(count - 1) + '\\x00'
      index++
    } else {
      result += pattern.slice(start, index)
    }
  }
  return result
}

const schemaMaps = new Set(['properties', 'patternProperties', '$defs', 'definitions', 'dependentSchemas'])
const schemaArrays = new Set(['allOf', 'anyOf', 'oneOf', 'prefixItems'])
const singleSchemas = new Set([
  'items', 'additionalItems', 'additionalProperties', 'unevaluatedItems', 'unevaluatedProperties',
  'propertyNames', 'contains', 'not', 'if', 'then', 'else', 'contentSchema'
])

/** Normalize only schema pattern keywords; instance data and patternProperties keys stay untouched. */
export function normalizeDeepSeekToolSchema(schema: SchemaObject): SchemaObject {
  let normalized = schema
  const assign = (key: string, value: unknown) => {
    if (schema[key] === value) return
    if (normalized === schema) normalized = { ...schema }
    normalized[key] = value
  }
  const normalizeChild = (value: unknown): unknown => {
    const child = object(value)
    return child ? normalizeDeepSeekToolSchema(child) : value
  }
  for (const [key, value] of Object.entries(schema)) {
    if (key === 'pattern' && typeof value === 'string') {
      assign(key, normalizeNullEscape(value))
    } else if (schemaMaps.has(key) && object(value)) {
      const children = value as SchemaObject
      let updated = children
      for (const [name, child] of Object.entries(children)) {
        const next = normalizeChild(child)
        if (next === child) continue
        if (updated === children) updated = { ...children }
        updated[name] = next
      }
      assign(key, updated)
    } else if ((schemaArrays.has(key) || key === 'items') && Array.isArray(value)) {
      const updated = value.map(normalizeChild)
      if (updated.some((child, index) => child !== value[index])) assign(key, updated)
    } else if (singleSchemas.has(key)) {
      assign(key, normalizeChild(value))
    }
  }
  return normalized
}
