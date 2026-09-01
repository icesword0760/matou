import { describe, expect, it } from 'vitest'

import {
  DEFAULT_TERMINAL_INPUT_CHUNK_BYTES,
  splitUtf8ForTransport
} from './terminal-input-chunker'

const encoder = new TextEncoder()

describe('splitUtf8ForTransport', () => {
  it('keeps UTF-8 chunks within the transport budget without splitting Unicode code points', () => {
    const value = [
      'a'.repeat(DEFAULT_TERMINAL_INPUT_CHUNK_BYTES - 1),
      '🧭',
      'e\u0301',
      '中文',
      'b'.repeat(DEFAULT_TERMINAL_INPUT_CHUNK_BYTES),
      '🚀'
    ].join('')

    const chunks = splitUtf8ForTransport(value)

    expect(chunks.length).toBeGreaterThan(2)
    expect(chunks.join('')).toBe(value)
    expect(chunks.every((chunk) => encoder.encode(chunk).byteLength <= DEFAULT_TERMINAL_INPUT_CHUNK_BYTES)).toBe(true)
    expect(chunks.every(hasNoIsolatedSurrogate)).toBe(true)
  })

  it('preserves empty input and exact UTF-8 ordering for a small byte budget', () => {
    const value = 'ASCII-中-🧭-e\u0301-🚀'

    expect(splitUtf8ForTransport('')).toEqual([])
    expect(splitUtf8ForTransport(value, 7).join('')).toBe(value)
    expect(splitUtf8ForTransport(value, 7).every((chunk) => encoder.encode(chunk).byteLength <= 7)).toBe(true)
  })
})

function hasNoIsolatedSurrogate(value: string): boolean {
  for (let index = 0; index < value.length; index += 1) {
    const codeUnit = value.charCodeAt(index)
    if (codeUnit >= 0xd800 && codeUnit <= 0xdbff) {
      const next = value.charCodeAt(index + 1)
      if (next < 0xdc00 || next > 0xdfff) return false
      index += 1
    } else if (codeUnit >= 0xdc00 && codeUnit <= 0xdfff) {
      return false
    }
  }
  return true
}
