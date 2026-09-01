export const SEGMENT_BYTES = 16 * 1024 * 1024
export const RAW_HOT_BYTES = 256 * 1024 * 1024

export type SegmentState = 'active' | 'sealed-raw' | 'compressed'

export interface SegmentDescriptor {
  index: number
  path: string
  bytes: number
  state: SegmentState
  checkpointProtected?: boolean
}

export function selectCompressionCandidates(
  segments: readonly SegmentDescriptor[]
): SegmentDescriptor[] {
  const compressedIndexes = new Set(
    segments.filter(({ state }) => state === 'compressed').map(({ index }) => index)
  )
  const checkpointProtectedIndexes = new Set(
    segments.filter(({ checkpointProtected }) => checkpointProtected === true).map(({ index }) => index)
  )
  const rawByIndex = new Map<number, SegmentDescriptor>()
  for (const segment of segments) {
    if (segment.state !== 'sealed-raw' || compressedIndexes.has(segment.index)) continue
    const current = rawByIndex.get(segment.index)
    if (!current || segment.path.localeCompare(current.path) < 0) {
      rawByIndex.set(segment.index, segment)
    }
  }

  const rawNewestFirst = [...rawByIndex.values()].sort((left, right) => right.index - left.index)
  const hotIndexes = new Set<number>()
  let hotBytes = 0
  for (const segment of rawNewestFirst) {
    if (hotIndexes.size > 0 && hotBytes + segment.bytes > RAW_HOT_BYTES) break
    hotIndexes.add(segment.index)
    hotBytes += segment.bytes
  }

  return rawNewestFirst
    .filter(({ index }) => !hotIndexes.has(index) && !checkpointProtectedIndexes.has(index))
    .sort((left, right) => left.index - right.index || left.path.localeCompare(right.path))
}
