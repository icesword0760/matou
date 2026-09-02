export const DEFAULT_TERMINAL_INPUT_CHUNK_BYTES = 256 * 1024

export function splitUtf8ForTransport(
  value: string,
  maxBytes = DEFAULT_TERMINAL_INPUT_CHUNK_BYTES
): string[] {
  if (!Number.isSafeInteger(maxBytes) || maxBytes <= 0) {
    throw new RangeError('maxBytes must be a positive safe integer')
  }
  if (value.length === 0) return []

  const chunks: string[] = []
  let chunkStart = 0
  let chunkBytes = 0
  let offset = 0

  for (const character of value) {
    const characterBytes = utf8ByteLength(character)
    if (characterBytes > maxBytes) {
      throw new RangeError('maxBytes is smaller than one UTF-8 code point')
    }
    if (chunkBytes > 0 && chunkBytes + characterBytes > maxBytes) {
      chunks.push(value.slice(chunkStart, offset))
      chunkStart = offset
      chunkBytes = 0
    }
    chunkBytes += characterBytes
    offset += character.length
  }

  chunks.push(value.slice(chunkStart))
  return chunks
}

function utf8ByteLength(character: string): number {
  const codePoint = character.codePointAt(0)!
  if (codePoint <= 0x7f) return 1
  if (codePoint <= 0x7ff) return 2
  if (codePoint <= 0xffff) return 3
  return 4
}
