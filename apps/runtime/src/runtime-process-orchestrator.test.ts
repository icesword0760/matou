import { describe, expect, it } from 'vitest'

import { RuntimeShutdownRequestedError } from './runtime-lifecycle-coordinator'
import { RuntimeProcessOrchestrator } from './runtime-process-orchestrator'

describe('RuntimeProcessOrchestrator', () => {
  it('cleans up once, records the initialization error, and exits nonzero', async () => {
    const initializationError = new Error('migration initialization failed')
    const events: string[] = []
    const exits: number[] = []
    const errors: unknown[] = []
    let cleanupCalls = 0
    const orchestrator = new RuntimeProcessOrchestrator({
      runtimeReady: Promise.reject(initializationError),
      shutdown: async () => {
        cleanupCalls += 1
        events.push('cleanup')
        throw initializationError
      },
      reportError: (_label, error) => { errors.push(error); events.push('record') },
      exit: (code) => { exits.push(code); events.push(`exit:${code}`) }
    })

    await orchestrator.watchInitialization()

    expect(events).toEqual(['cleanup', 'record', 'exit:1'])
    expect(cleanupCalls).toBe(1)
    expect(errors).toEqual([initializationError])
    expect(exits).toEqual([1])
  })

  it('records an aggregated cleanup failure and still exits nonzero exactly once', async () => {
    const initializationError = new Error('provider initialization failed')
    const cleanupError = new Error('database cleanup failed')
    const aggregate = new AggregateError([initializationError, cleanupError], 'Runtime shutdown failed')
    const exits: number[] = []
    const errors: unknown[] = []
    let cleanupCalls = 0
    const orchestrator = new RuntimeProcessOrchestrator({
      runtimeReady: Promise.reject(initializationError),
      shutdown: async () => { cleanupCalls += 1; throw aggregate },
      reportError: (_label, error) => { errors.push(error) },
      exit: (code) => { exits.push(code) }
    })

    await orchestrator.watchInitialization()
    await orchestrator.terminateFromSignal()

    expect(cleanupCalls).toBe(1)
    expect(errors).toEqual([aggregate])
    expect(exits).toEqual([1])
  })

  it('preserves successful SIGTERM exit semantics without duplicate cleanup', async () => {
    const ready = deferred<void>()
    const events: string[] = []
    let cleanupCalls = 0
    const orchestrator = new RuntimeProcessOrchestrator({
      runtimeReady: ready.promise,
      shutdown: async () => { cleanupCalls += 1; events.push('cleanup') },
      reportError: () => { events.push('record') },
      exit: (code) => { events.push(`exit:${code}`) }
    })
    const watched = orchestrator.watchInitialization()

    const termination = orchestrator.terminateFromSignal()
    ready.reject(new RuntimeShutdownRequestedError())
    await Promise.all([watched, termination])

    expect(cleanupCalls).toBe(1)
    expect(events).toEqual(['cleanup', 'exit:0'])
  })
})

function deferred<T>(): { promise: Promise<T>; reject(error: unknown): void } {
  let reject!: (error: unknown) => void
  const promise = new Promise<T>((_resolve, fail) => { reject = fail })
  return { promise, reject }
}
