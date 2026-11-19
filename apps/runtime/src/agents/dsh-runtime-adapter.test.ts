import { describe, expect, it } from 'vitest'
import { dshObservationPatch } from './dsh-runtime-adapter'

const dump = `- id: dsh-tui
  name: dsh-tui
  inject:
    - agents
    - workspaceRegistry
  config:
    model: retained-model
    preset: !!js process.env.DSH_PRESET ?? undefined
    taskNotifications: all
    upstreamRetry:
      maxRetries: 5
    backend: codex
    fullscreen: true
- id: another-plugin
  config:
    backend: untouched
`

describe('DSH per-run overlay', () => {
  it('preserves literal native settings and dependencies while fixing only the managed run', () => {
    const patch = dshObservationPatch(dump, '/assets/bridge.mjs', 'http://127.0.0.1:1/hooks/token', 'run-1')
    expect(patch).toContain('    model: retained-model')
    expect(patch).toContain('    preset: !!js process.env.DSH_PRESET ?? undefined')
    expect(patch).toContain('    - agents\n    - workspaceRegistry\n    - matouDshBridge')
    expect(patch).toContain('    taskNotifications: "off"')
    expect(patch).toContain('    backend: dsh')
    expect(patch).not.toContain('maxRetries')
    expect(patch).not.toContain('another-plugin')
  })
  it('fails visibly rather than wiping an unsupported native config expression', () => {
    expect(() => dshObservationPatch('- id: dsh-tui\n  config: !!js makeConfig()\n', 'x', 'x', 'x')).toThrow('literal mapping')
    expect(() => dshObservationPatch(dump.replace('  inject:\n    - agents\n    - workspaceRegistry', '  inject: !!js makeInject()'), 'x', 'x', 'x')).toThrow('literal service list')
  })
})

describe('DSH observation privacy and subagent boundary', () => {
  it('sends only event facts and excludes tool bodies, prompts and child agents', async () => {
    const bridge = await import(new URL('../../control-assets/providers/dsh/bridge.mjs', import.meta.url).href)
    const data = bridge.observationData('tool/result', { callId: 'call-1', toolName: 'shell', arguments: { password: 'secret' }, result: 'private transcript', prompt: 'private prompt' })
    expect(data).toEqual({ callId: 'call-1', toolName: 'shell' })
    expect(bridge.isTopLevelAgent({ session: { id: 'child', header: { origin: 'subagent' } } })).toBe(false)
    expect(bridge.isTopLevelAgent({ session: { id: 'child', header: { delegationDepth: 1 } } })).toBe(false)
    expect(bridge.isTopLevelAgent({ session: { id: 'fork', header: { origin: 'fork' } } })).toBe(true)
  })
})
