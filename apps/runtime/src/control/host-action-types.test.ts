import { describe, expect, it } from 'vitest'

import { parseHostActionRequest } from './host-action-types'

describe('parseHostActionRequest', () => {
  it('parses an explicit three-item child batch without inventing Git choices', () => {
    expect(parseHostActionRequest('structure.fork.children', {
      source: { kind: 'self' },
      batchKey: 'three-options-v1',
      items: [
        { itemKey: 'light', title: '轻量适配', environment: { mode: 'current' } },
        { itemKey: 'service', title: '服务层重构', environment: {
          mode: 'new-worktree', branch: 'feature/service-refactor'
        } },
        { itemKey: 'architecture', title: '完整架构升级', environment: {
          mode: 'existing-worktree', branch: 'main', worktreeRef: 'worktree:main'
        } }
      ]
    })).toMatchObject({ method: 'structure.fork.children', batchKey: 'three-options-v1' })
  })

  it('requires an explicit environment and unique item keys for each batch item', () => {
    expect(() => parseHostActionRequest('structure.fork.children', {
      source: { kind: 'self' },
      batchKey: 'missing-environment',
      items: [{ itemKey: 'light', title: '轻量适配' }]
    })).toThrow()
    expect(() => parseHostActionRequest('structure.fork.children', {
      source: { kind: 'self' },
      batchKey: 'duplicate-item-keys',
      items: [
        { itemKey: 'same', title: '轻量适配', environment: { mode: 'current' } },
        { itemKey: 'same', title: '服务层重构', environment: { mode: 'current' } }
      ]
    })).toThrow()
  })

  it('requires a projection revision for every position-based high-level target selector', () => {
    const revision = 'projection-1'
    const missingRelative = {
      source: { kind: 'relative', direction: 'right' },
      title: '相邻事项', environment: { mode: 'current' }, submissionKey: 'missing-relative'
    }
    const missingChildOrdinal = {
      source: { kind: 'relation', relation: 'child', ordinal: 1 },
      title: '子事项', environment: { mode: 'current' }, submissionKey: 'missing-child-ordinal'
    }
    expect(() => parseHostActionRequest('structure.fork.child', missingRelative)).toThrow()
    expect(() => parseHostActionRequest('structure.fork.child', missingChildOrdinal)).toThrow()
    expect(() => parseHostActionRequest('structure.fork.child', {
      ...missingRelative, source: { kind: 'sibling', ordinal: 1 }
    })).toThrow()
    expect(() => parseHostActionRequest('structure.fork.child', {
      ...missingRelative, source: { kind: 'ref', ref: 'session:1' }
    })).toThrow()
    expect(() => parseHostActionRequest('structure.create.canvas', {
      task: { kind: 'relative', direction: 'left' }, submissionKey: 'missing-create-relative'
    })).toThrow()

    expect(parseHostActionRequest('structure.fork.child', {
      ...missingRelative,
      source: { kind: 'relative', direction: 'right', projectionRevision: revision }
    })).toMatchObject({ source: { projectionRevision: revision } })
    expect(parseHostActionRequest('structure.fork.children', {
      source: { kind: 'relation', relation: 'child', ordinal: 1, projectionRevision: revision },
      batchKey: 'positioned-batch',
      items: [{ itemKey: 'child', title: '子事项', environment: { mode: 'current' } }]
    })).toMatchObject({ source: { projectionRevision: revision } })
    expect(parseHostActionRequest('structure.create.canvas', {
      task: { kind: 'relative', direction: 'left', projectionRevision: revision },
      submissionKey: 'positioned-create'
    })).toMatchObject({ task: { projectionRevision: revision } })

    expect(parseHostActionRequest('structure.fork.child', {
      source: { kind: 'self' }, title: '当前会话', environment: { mode: 'current' },
      submissionKey: 'stable-self'
    })).toMatchObject({ source: { kind: 'self' } })
    expect(parseHostActionRequest('structure.fork.child', {
      source: { kind: 'session', sessionId: 'session-1' }, title: '指定会话',
      environment: { mode: 'current' }, submissionKey: 'stable-session'
    })).toMatchObject({ source: { kind: 'session', sessionId: 'session-1' } })
  })

  it('accepts 50 batch items and rejects the 51st', () => {
    const items = Array.from({ length: 50 }, (_, index) => ({
      itemKey: `item-${index + 1}`,
      title: `标题 ${index + 1}`,
      environment: { mode: 'current' as const }
    }))
    expect(parseHostActionRequest('structure.fork.children', {
      source: { kind: 'self' }, batchKey: 'fifty-items', items
    })).toMatchObject({ items: { length: 50 } })
    expect(() => parseHostActionRequest('structure.fork.children', {
      source: { kind: 'self' }, batchKey: 'fifty-one-items',
      items: [...items, { itemKey: 'item-51', title: '标题 51', environment: { mode: 'current' } }]
    })).toThrow()
  })

  it('keeps key and stable reference fields within their 160-character limits', () => {
    const atLimit = 'x'.repeat(160)
    const overLimit = 'x'.repeat(161)

    expect(parseHostActionRequest('structure.fork.children', {
      source: { kind: 'self' }, batchKey: atLimit,
      items: [{ itemKey: 'item', title: '标题', environment: { mode: 'current' } }]
    })).toMatchObject({ batchKey: atLimit })
    expect(() => parseHostActionRequest('structure.fork.children', {
      source: { kind: 'self' }, batchKey: overLimit,
      items: [{ itemKey: 'item', title: '标题', environment: { mode: 'current' } }]
    })).toThrow()

    expect(parseHostActionRequest('structure.fork.child', {
      source: { kind: 'self' }, title: '标题', submissionKey: atLimit,
      environment: { mode: 'existing-worktree', branch: 'main', worktreeRef: atLimit }
    })).toMatchObject({ submissionKey: atLimit })
    expect(() => parseHostActionRequest('structure.fork.child', {
      source: { kind: 'self' }, title: '标题', submissionKey: overLimit,
      environment: { mode: 'existing-worktree', branch: 'main', worktreeRef: atLimit }
    })).toThrow()
    expect(() => parseHostActionRequest('structure.fork.child', {
      source: { kind: 'self' }, title: '标题', submissionKey: atLimit,
      environment: { mode: 'existing-worktree', branch: 'main', worktreeRef: overLimit }
    })).toThrow()
  })

  it('rejects unknown action fields', () => {
    expect(() => parseHostActionRequest('structure.fork.child', {
      source: { kind: 'self' }, title: '标题', submissionKey: 'known-fields-only',
      environment: { mode: 'current' }, unexpected: true
    })).toThrow()
  })

  it.each([
    'structure.create.session',
    'structure.remove.preview'
  ] as const)('rejects params.method even when it equals or differs from outer %s', (outerMethod) => {
    const params = outerMethod === 'structure.create.session'
      ? {
          canvas: { kind: 'current', entity: 'canvas' },
          profile: 'shell',
          submissionKey: 'method-injection'
        }
      : {
          target: { kind: 'current', entity: 'session' },
          scope: 'node'
        }

    expect(() => parseHostActionRequest(outerMethod, {
      ...params,
      method: outerMethod
    })).toThrow(expect.objectContaining({ issues: expect.arrayContaining([
      expect.objectContaining({ code: 'unrecognized_keys', keys: ['method'] })
    ]) }))
    expect(() => parseHostActionRequest(outerMethod, {
      ...params,
      method: outerMethod === 'structure.create.session'
        ? 'structure.remove.commit'
        : 'structure.create.workspace'
    })).toThrow(expect.objectContaining({ issues: expect.arrayContaining([
      expect.objectContaining({ code: 'unrecognized_keys', keys: ['method'] })
    ]) }))
  })

  it('only accepts unique retry keys that belong to the submitted batch', () => {
    const request = {
      source: { kind: 'self' as const }, batchKey: 'retry-invariants',
      items: [
        { itemKey: 'light', title: '轻量适配', environment: { mode: 'current' as const } },
        { itemKey: 'service', title: '服务层重构', environment: { mode: 'current' as const } }
      ]
    }
    expect(parseHostActionRequest('structure.fork.children', {
      ...request, retryItemKeys: ['service']
    })).toMatchObject({ retryItemKeys: ['service'] })
    expect(() => parseHostActionRequest('structure.fork.children', {
      ...request, retryItemKeys: ['service', 'service']
    })).toThrow()
    expect(() => parseHostActionRequest('structure.fork.children', {
      ...request, retryItemKeys: ['unknown']
    })).toThrow()
  })

  it('enforces byte limits on titles and prompts', () => {
    expect(() => parseHostActionRequest('structure.fork.child', {
      source: { kind: 'self' },
      title: '界'.repeat(54),
      environment: { mode: 'current' },
      submissionKey: 'title-too-large'
    })).toThrow()
    expect(() => parseHostActionRequest('structure.fork.child', {
      source: { kind: 'self' },
      title: '短标题',
      prompt: 'a'.repeat(64 * 1024 + 1),
      environment: { mode: 'current' },
      submissionKey: 'prompt-too-large'
    })).toThrow()
  })
})
