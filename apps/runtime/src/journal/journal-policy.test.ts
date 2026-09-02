import { describe, expect, it } from 'vitest'

import {
  RAW_HOT_BYTES,
  SEGMENT_BYTES,
  selectCompressionCandidates,
  type SegmentDescriptor
} from './journal-policy'

describe('journal hot-window policy', () => {
  it('uses 16 MiB segments and a 256 MiB raw hot window', () => {
    expect(SEGMENT_BYTES).toBe(16 * 1024 * 1024)
    expect(RAW_HOT_BYTES).toBe(256 * 1024 * 1024)
  })

  it.each([
    { sealed: 17, candidates: [1, 2] },
    { sealed: 18, candidates: [1, 2, 3] },
    { sealed: 40, candidates: Array.from({ length: 25 }, (_, index) => index + 1) }
  ])('selects only sealed raw segments older than the latest 256 MiB for $sealed segments', ({
    sealed,
    candidates
  }) => {
    const segments = [
      ...Array.from({ length: sealed }, (_, index) => raw(index + 1)),
      active(sealed + 1)
    ]

    expect(selectCompressionCandidates(segments).map(({ index }) => index)).toEqual(candidates)
  })

  it('never selects the active segment or checkpoint-protected raw segments', () => {
    const segments = [
      ...Array.from({ length: 20 }, (_, index) => raw(index + 1)),
      { ...raw(2), checkpointProtected: true },
      active(21)
    ]

    expect(selectCompressionCandidates(segments).map(({ index }) => index)).toEqual([1, 3, 4, 5])
  })

  it('does not recompress a raw duplicate when the same segment index already has gzip', () => {
    const segments: SegmentDescriptor[] = [
      ...Array.from({ length: 18 }, (_, index) => raw(index + 1)),
      {
        index: 1,
        path: '/journal/segment-000001.mtj.gz',
        bytes: SEGMENT_BYTES,
        state: 'compressed'
      } satisfies SegmentDescriptor,
      active(19)
    ].reverse()

    expect(selectCompressionCandidates(segments).map(({ index }) => index)).toEqual([2, 3])
  })

  it('counts the active segment inside the 256 MiB raw window', () => {
    const segments = [
      ...Array.from({ length: 16 }, (_, index) => raw(index + 1)),
      { ...active(17), bytes: SEGMENT_BYTES / 2 }
    ]

    expect(selectCompressionCandidates(segments).map(({ index }) => index)).toEqual([1])
  })
})

function raw(index: number): SegmentDescriptor {
  return {
    index,
    path: `/journal/segment-${String(index).padStart(6, '0')}.mtj`,
    bytes: SEGMENT_BYTES,
    state: 'sealed-raw'
  }
}

function active(index: number): SegmentDescriptor {
  return {
    index,
    path: `/journal/segment-${String(index).padStart(6, '0')}.mtj`,
    bytes: SEGMENT_BYTES,
    state: 'active'
  }
}
