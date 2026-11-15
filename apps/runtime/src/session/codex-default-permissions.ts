import { readFile } from 'node:fs/promises'
import { homedir } from 'node:os'
import { join } from 'node:path'

export interface CodexDefaultPermissions {
  approvalPolicy: string
  sandboxMode: 'read-only' | 'workspace-write' | 'danger-full-access'
}

export const CODEX_DEFAULT_PERMISSIONS: Readonly<CodexDefaultPermissions> = {
  approvalPolicy: 'on-request', sandboxMode: 'workspace-write'
}

/** Read only the user config used by this process; never write it or reuse a
 * previous turn's permissions. CODEX_HOME also keeps isolated launches isolated. */
export async function readCodexDefaultPermissions(environment: NodeJS.ProcessEnv): Promise<CodexDefaultPermissions> {
  try {
    const root = environment.CODEX_HOME || join(environment.HOME || homedir(), '.codex')
    return parseCodexDefaultPermissions(await readFile(join(root, 'config.toml'), 'utf8'))
  } catch { return { ...CODEX_DEFAULT_PERMISSIONS } }
}

export function parseCodexDefaultPermissions(text: string): CodexDefaultPermissions {
  const result = { ...CODEX_DEFAULT_PERMISSIONS }
  const statements = topLevelStatements(text)
  if (!statements) return result
  const seen = new Set<string>()
  for (const statement of statements) {
    const field = /^\s*(approval_policy|sandbox_mode|"approval_policy"|"sandbox_mode"|'approval_policy'|'sandbox_mode')\s*=\s*(.*?)\s*$/s.exec(statement)
    if (!field) continue
    const key = field[1]!.replace(/["']/g, '')
    if (seen.has(key)) return { ...CODEX_DEFAULT_PERMISSIONS }
    seen.add(key)
    let value: unknown
    try {
      const literal = field[2]!
      value = literal.startsWith('"') ? JSON.parse(literal)
        : /^'[^']*'$/.test(literal) ? literal.slice(1, -1) : undefined
    } catch { continue }
    if (key === 'approval_policy' && typeof value === 'string' &&
        ['on-request', 'never', 'untrusted', 'on-failure'].includes(value)) result.approvalPolicy = value
    if (key === 'sandbox_mode' && ['read-only', 'workspace-write', 'danger-full-access'].includes(value as string)) {
      result.sandboxMode = value as CodexDefaultPermissions['sandboxMode']
    }
  }
  return result
}

/** Split only root assignments. Track TOML strings and containers so instructions,
 * array values and table-local keys can never masquerade as user root policy. */
function topLevelStatements(text: string): string[] | undefined {
  const statements: string[] = []
  let statement = '', quote = '', depth = 0
  for (let index = 0; index < text.length; index++) {
    const char = text[index]!
    if (quote) {
      if (quote.startsWith('"') && char === '\\') {
        statement += char + (text[++index] ?? '')
        continue
      }
      if (text.startsWith(quote, index)) {
        statement += quote; index += quote.length - 1; quote = ''
      } else {
        if ((char === '\n' || char === '\r') && quote.length === 1) return undefined
        statement += char
      }
      continue
    }
    if (char === '#') {
      while (index < text.length && text[index] !== '\n') index++
      index--
      continue
    }
    if (char === '"' || char === "'") {
      quote = text.startsWith(char.repeat(3), index) ? char.repeat(3) : char
      statement += quote; index += quote.length - 1
      continue
    }
    if (char === '[' && depth === 0 && !statement.trim()) return statements
    if (char === '[' || char === '{') depth++
    if (char === ']' || char === '}') { if (--depth < 0) return undefined }
    if (char === '\n' && depth === 0) {
      if (statement.trim()) statements.push(statement.trim())
      statement = ''
    } else statement += char
  }
  if (quote || depth) return undefined
  if (statement.trim()) statements.push(statement.trim())
  return statements
}
