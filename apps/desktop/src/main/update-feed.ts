import { readFileSync } from 'node:fs'

export function parseUpdateBaseUrl(content: string): string | undefined {
  if (!/^provider:\s*generic\s*$/m.test(content)) return undefined
  const value = content.match(/^url:\s*(.+?)\s*$/m)?.[1]?.trim()
  return value || undefined
}

export function readUpdateBaseUrl(path: string): string | undefined {
  try {
    return parseUpdateBaseUrl(readFileSync(path, 'utf8'))
  } catch {
    return undefined
  }
}
