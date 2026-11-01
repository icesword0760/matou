import { describe, expect, it } from 'vitest'

import { OutputByteLedger } from './output-byte-ledger'

describe('OutputByteLedger', () => {
  it('sums output bytes from a sequence through the newest frame', () => {
    const ledger = new OutputByteLedger()
    ledger.record(5, 100)
    ledger.record(6, 20)
    ledger.record(9, 3)

    expect(ledger.bytesSince(5)).toBe(123)
    expect(ledger.bytesSince(6)).toBe(23)
    expect(ledger.bytesSince(7)).toBe(3)
    expect(ledger.bytesSince(9)).toBe(3)
    expect(ledger.bytesSince(10)).toBe(0)
  })

  it('reports no backlog before any output', () => {
    expect(new OutputByteLedger().bytesSince(1)).toBe(0)
  })

  it('treats ranges older than the retained frames as an unbounded backlog', () => {
    const ledger = new OutputByteLedger(2)
    for (let sequence = 1; sequence <= 4; sequence += 1) ledger.record(sequence, 10)

    expect(ledger.bytesSince(1)).toBe(Number.POSITIVE_INFINITY)
    expect(ledger.bytesSince(3)).toBe(20)
  })
})
