import { readFile } from 'node:fs/promises'
import { describe, expect, it, vi } from 'vitest'
import { AGENT_MANIFESTS } from '@matou/domain'

const helper = () => import(new URL('../../control-assets/providers/dsh/instructions.mjs', import.meta.url).href)
describe('DSH native system section and mode boundary', () => {
  it('requires this exact managed run and keeps credentials out of the text', async () => {
    const { hasManagedMtEnvironment, attachMtInstructions } = await helper()
    const env = { MATOU_CONTROL_PROTOCOL: '1', MATOU_CONTROL_CALLER_RUN: 'this-run', MATOU_CONTROL_CALLER_SESSION: 'card',
      MATOU_CONTROL_ENDPOINT: '/private/control.sock', MATOU_CONTROL_TOKEN: 'private-token', MATOU_CONTROL_NODE_EXECUTABLE: '/node' }
    expect(hasManagedMtEnvironment(env, 'this-run')).toBe(true)
    expect(hasManagedMtEnvironment(env, 'old-run')).toBe(false)
    expect(hasManagedMtEnvironment({ ...env, MATOU_CONTROL_TOKEN: '' }, 'this-run')).toBe(false)
    const section = vi.fn((_spec: Record<string, unknown>) => () => {})
    const agent = { ctx: { get: () => ({ section }) } }
    let foreground = true
    attachMtInstructions(agent, 'run mt identify --json; literal {{keep}}', () => foreground)
    const spec = section.mock.calls[0]![0] as any
    expect(spec).toMatchObject({ name: 'matou:host-control', interpolate: false })
    expect(spec.text({ scope: agent })).toContain('{{keep}}')
    expect(spec.text({ scope: {} })).toBe('')
    foreground = false
    expect(spec.text({ scope: agent })).toBe('')
    expect(JSON.stringify(spec)).not.toContain('private-token')
    expect(attachMtInstructions({}, 'mt', () => true)).toBeUndefined()
  })

  it('forwards the explicit shell identity only to the exact live foreground top-level agent', async () => {
    const { attachMtShellEnvironment } = await helper()
    const register = vi.fn((_spec: Record<string, unknown>) => () => {})
    const env = { MATOU_CONTROL_PROTOCOL: '1', MATOU_CONTROL_CALLER_RUN: 'run', MATOU_CONTROL_CALLER_SESSION: 'card',
      MATOU_CONTROL_ENDPOINT: '/control.sock', MATOU_CONTROL_TOKEN: 'test-session-value', MATOU_CONTROL_NODE_EXECUTABLE: '/node' }
    const main = {}, background = {}, child = {}
    let live = true
    attachMtShellEnvironment({ get: () => ({ register }) }, env, 'run', (agent: unknown) => live && agent === main)
    const spec = register.mock.calls[0]![0] as any
    expect(Object.keys(spec.resolve({ agent: main }))).toEqual(['DSH_MATOU_CONTROL_TOKEN'])
    expect(spec.resolve({ agent: main }).DSH_MATOU_CONTROL_TOKEN === env.MATOU_CONTROL_TOKEN).toBe(true)
    expect(spec.resolve({ agent: background })).toEqual({})
    expect(spec.resolve({ agent: child })).toEqual({})
    expect(spec.resolve({})).toEqual({})
    env.MATOU_CONTROL_CALLER_RUN = 'replacement'
    expect(spec.resolve({ agent: main })).toEqual({})
    env.MATOU_CONTROL_CALLER_RUN = 'run'; live = false
    expect(spec.resolve({ agent: main })).toEqual({})
    expect(JSON.stringify(spec)).not.toContain(env.MATOU_CONTROL_TOKEN)
  })

  it('reuses the existing collaboration rules with a DSH wrapper and no profile or project-file writes', async () => {
    const dsh = await readFile(new URL('../../control-assets/providers/dsh-system-message.md', import.meta.url), 'utf8')
    const baseline = await readFile(new URL('../../control-assets/providers/antigravity-system-message.md', import.meta.url), 'utf8')
    expect(dsh).toBe(baseline.replace('Matou-managed Antigravity terminal', 'Matou-managed DSH terminal'))
    expect(dsh).toContain('mt identify --json')
    expect(dsh).toContain('mt list --json')
    expect(dsh).toContain('mt read')
    expect(dsh).toContain('--enter')
  })

  it('keeps switching unavailable even when the native kernel exposes lower-level setters', () => {
    expect(AGENT_MANIFESTS.dsh.capabilities).toMatchObject({ permissionModes: [], permissionSwitchMode: 'none' })
  })
})
