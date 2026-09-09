import { createHash } from 'node:crypto'

import { runtimeMessages } from '../i18n/messages'

export type DisplayNameValidation =
  | { ok: true; displayName: string }
  | {
      ok: false
      code: 'EMPTY' | 'TOO_LONG' | 'DUPLICATE'
      message: string
      input: string
    }

export function validateDisplayName(
  input: string,
  activeSiblingNames: readonly string[]
): DisplayNameValidation {
  const messages = runtimeMessages().sessionCanvas.branchName
  const displayName = input.trim()
  if (displayName.length === 0) {
    return { ok: false, code: 'EMPTY', message: messages.required, input }
  }
  if ([...displayName].length > 64) {
    return { ok: false, code: 'TOO_LONG', message: messages.tooLong, input }
  }
  if (activeSiblingNames.includes(displayName)) {
    return {
      ok: false,
      code: 'DUPLICATE',
      message: messages.duplicate(displayName),
      input
    }
  }
  return { ok: true, displayName }
}

export function createGitBranchName(displayName: string, sessionId: string): string {
  const slug = [...displayName.trim().normalize('NFKC').toLocaleLowerCase('en-US')]
    .map((character) => /[\p{L}\p{N}]/u.test(character) ? character : '-')
    .join('')
    .replace(/-+/g, '-')
    .replace(/^-|-$/g, '')
  return `matou/${takeCodePoints(slug || 'branch', 48)}-${shortIdentity(sessionId)}`
}

function shortIdentity(sessionId: string): string {
  const normalized = sessionId.trim().toLocaleLowerCase('en-US')
  const uuidPrefix = normalized.match(/^[a-f0-9]{8}(?=-)/)?.[0]
  if (uuidPrefix) return uuidPrefix
  return createHash('sha256').update(sessionId).digest('hex').slice(0, 8)
}

function takeCodePoints(value: string, maximum: number): string {
  return [...value].slice(0, maximum).join('').replace(/-+$/g, '') || 'branch'
}
