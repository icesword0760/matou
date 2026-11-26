import { readFile } from 'node:fs/promises'

// The installer requires native x64 images. Parse their regular and delay-load
// import tables so app-local CRT dependencies also include native sidecars.
export async function windowsPeImports(path) {
  const image = await readFile(path)
  const invalid = () => new Error(`Invalid Windows PE imports: ${path}`)
  if (image.length < 64 || image.readUInt16LE(0) !== 0x5a4d) throw invalid()
  const pe = image.readUInt32LE(0x3c)
  if (pe < 64 || pe + 24 > image.length || image.readUInt32LE(pe) !== 0x4550) throw invalid()

  const sectionCount = image.readUInt16LE(pe + 6)
  const optional = pe + 24
  const optionalSize = image.readUInt16LE(pe + 20)
  const sectionStart = optional + optionalSize
  if (optionalSize < 112 || sectionStart + sectionCount * 40 > image.length
      || image.readUInt16LE(optional) !== 0x20b) throw invalid()
  const directoryCount = image.readUInt32LE(optional + 108)
  if (112 + 8 * directoryCount > optionalSize) throw invalid()
  const headersSize = image.readUInt32LE(optional + 60)

  const sections = Array.from({ length: sectionCount }, (_, index) => {
    const offset = sectionStart + 40 * index
    return {
      address: image.readUInt32LE(offset + 12),
      length: image.readUInt32LE(offset + 16),
      fileOffset: image.readUInt32LE(offset + 20)
    }
  })

  function bytesAt(address) {
    if (address && address < headersSize && address < image.length) {
      return image.subarray(address, Math.min(headersSize, image.length))
    }
    const section = sections.find(({ address: start, length }) => address >= start && address - start < length)
    if (!section || section.fileOffset + section.length > image.length) throw invalid()
    return image.subarray(section.fileOffset + address - section.address, section.fileOffset + section.length)
  }

  const imports = new Set()
  for (const [directory, stride, nameOffset] of [[1, 20, 12], [13, 32, 4]]) {
    if (directory >= directoryCount) continue
    const offset = optional + 112 + 8 * directory
    const address = image.readUInt32LE(offset)
    const size = image.readUInt32LE(offset + 4)
    if (address === 0 && size === 0) continue
    if (!address || size < stride) throw invalid()
    const table = bytesAt(address)
    if (table.length < size) throw invalid()
    let terminated = false
    for (let index = 0; index + stride <= size; index += stride) {
      const record = table.subarray(index, index + stride)
      if (record.every(byte => byte === 0)) {
        terminated = true
        break
      }
      // Delay descriptors in PE32+ use RVAs (not the older 32-bit VAs).
      if (directory === 13 && record.readUInt32LE(0) !== 1) throw invalid()
      const nameRva = record.readUInt32LE(nameOffset)
      if (!nameRva) throw invalid()
      const bytes = bytesAt(nameRva).subarray(0, 256)
      const end = bytes.indexOf(0)
      if (end < 1) throw invalid()
      const name = bytes.subarray(0, end).toString('latin1')
      if (!/^[a-z0-9_.-]+\.dll$/i.test(name)) throw invalid()
      imports.add(name.toLowerCase())
    }
    if (!terminated) throw invalid()
  }
  return imports
}
