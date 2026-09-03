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
