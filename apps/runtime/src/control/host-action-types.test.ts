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
