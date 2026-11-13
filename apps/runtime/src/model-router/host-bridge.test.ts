import { afterEach, expect, it, vi } from 'vitest'
import { RouterHostBridge } from './host-bridge'
afterEach(() => vi.useRealTimers())
const url = 'https://auth.openai.com/oauth/authorize?redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback'
it('matches authorization replies to their requests without disturbing another pending operation', async () => {
  const sent: Array<any> = []
  const bridge = new RouterHostBridge(message => sent.push(message))
  const opening = bridge.openAuthorizationUrl('codex', url)
  const starting = bridge.start()
  const rejected = expect(opening).rejects.toThrow('process operation failed')
  expect(bridge.receive({ type: 'runtime.model-router-result', requestId: sent[1].requestId, ok: true })).toBe(true)
  await starting
  expect(bridge.receive({ type: 'runtime.model-router-result', requestId: sent[0].requestId, ok: false })).toBe(true)
  await rejected
})
it('bounds a missing browser reply at six seconds and ignores a late reply', async () => {
  vi.useFakeTimers()
  const sent: Array<any> = []
  const bridge = new RouterHostBridge(message => sent.push(message))
  const opening = bridge.openAuthorizationUrl('codex', url)
  const rejected = expect(opening).rejects.toThrow('host timed out')
  await vi.advanceTimersByTimeAsync(6000)
  await rejected
  expect(bridge.receive({ type: 'runtime.model-router-result', requestId: sent[0].requestId, ok: true })).toBe(true)
  const next = bridge.openAuthorizationUrl('codex', url)
  bridge.receive({ type: 'runtime.model-router-result', requestId: sent[1].requestId, ok: true })
  await next
  expect(vi.getTimerCount()).toBe(0)
})
