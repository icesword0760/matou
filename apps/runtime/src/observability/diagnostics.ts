const REDACTED_KEYS = new Set([
  'output', 'data', 'payload', 'content', 'body', 'terminal', 'terminalData',
  'stdin', 'stdout', 'stderr', 'textSnapshot', 'capturedText', 'token', 'secret'
])

export interface DiagnosticEntry {
  timestamp: number
  name: string
  fields: Record<string, unknown>
}

export class DiagnosticRecorder {
  readonly #capacity: number
  readonly #entries: DiagnosticEntry[] = []

  constructor(options: { capacity?: number } = {}) {
    this.#capacity = options.capacity ?? 2_000
    if (!Number.isSafeInteger(this.#capacity) || this.#capacity < 1) throw new Error('Invalid diagnostics capacity')
  }

  record(name: string, fields: Record<string, unknown>, timestamp = Date.now()): void {
    if (!name.trim()) throw new Error('Diagnostic event name is required')
    this.#entries.push({ timestamp, name, fields: sanitize(fields) as Record<string, unknown> })
    if (this.#entries.length > this.#capacity) this.#entries.splice(0, this.#entries.length - this.#capacity)
  }

  snapshot(): DiagnosticEntry[] {
    return structuredClone(this.#entries)
  }

  exportBundle(samples: Array<{ label: string; content: string }> = []): {
    schemaVersion: 1
    generatedAt: number
    events: DiagnosticEntry[]
    selectedSamples: Array<{ label: string; content: string }>
  } {
    for (const sample of samples) {
      if (!sample.label.trim() || Buffer.byteLength(sample.content) > 256 * 1024) {
        throw new Error('Invalid diagnostic content sample')
      }
    }
    return {
      schemaVersion: 1,
      generatedAt: Date.now(),
      events: this.snapshot(),
      selectedSamples: samples.map((sample) => ({ ...sample }))
    }
  }
}

export class RuntimeMetrics {
  readonly #counters = new Map<string, number>()
  readonly #gauges = new Map<string, number>()

  increment(name: string, amount = 1): void {
    assertMetric(name, amount)
    this.#counters.set(name, (this.#counters.get(name) ?? 0) + amount)
  }

  setGauge(name: string, value: number): void {
    assertMetric(name, value)
    this.#gauges.set(name, value)
  }

  snapshot(): { counters: Record<string, number>; gauges: Record<string, number> } {
    return {
      counters: Object.fromEntries([...this.#counters.entries()].sort(([a], [b]) => a.localeCompare(b))),
      gauges: Object.fromEntries([...this.#gauges.entries()].sort(([a], [b]) => a.localeCompare(b)))
    }
  }
}

function sanitize(value: unknown, key?: string): unknown {
  if (key !== undefined && REDACTED_KEYS.has(key)) return '[redacted]'
  if (value instanceof Uint8Array) return `[binary:${value.byteLength}]`
  if (Array.isArray(value)) return value.map((item) => sanitize(item))
  if (value && typeof value === 'object') {
    return Object.fromEntries(Object.entries(value as Record<string, unknown>).map(([childKey, item]) => [childKey, sanitize(item, childKey)]))
  }
  if (typeof value === 'string' && value.length > 4_096) return `${value.slice(0, 4_096)}[truncated]`
  return value
}

function assertMetric(name: string, value: number): void {
  if (!/^[a-z][a-z0-9_.-]*$/i.test(name) || !Number.isFinite(value)) throw new Error('Invalid metric')
}
