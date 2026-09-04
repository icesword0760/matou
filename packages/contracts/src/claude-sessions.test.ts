import { describe, expect, it } from 'vitest'

import type {
  ClaudeSessionDetail,
  ClaudeSessionListResult,
  ClaudeSessionSearchResult
} from './claude-sessions'

describe('Claude session pagination contracts', () => {
  it('represents a bounded event window without treating it as the history total', () => {
    const detail: ClaudeSessionDetail = {
      providerSessionId: 'provider-1', title: '分页会话', cwd: '/workspace', updatedAt: 1,
      permissionMode: 'default', eventCount: 24_189, matchCount: 0, hits: [],
      availability: 'available',
      events: [{ index: 24_189, kind: 'assistant', text: '最新内容', matched: false }],
      page: {
        startEventIndex: 24_189, endEventIndex: 24_189,
        total: 24_189, hasEarlier: true, hasLater: false
      }
    }

    expect(detail.page.total).toBe(24_189)
    expect(detail.events).toHaveLength(1)
  })

  it('represents complete session-list and search totals separately from returned pages', () => {
    const list: ClaudeSessionListResult = {
      sessions: [], total: 600, offset: 100, limit: 50, nextOffset: 150, hasMore: true
    }
    const search: ClaudeSessionSearchResult = {
      query: 'needle', hits: [{ eventIndex: 9_001, kind: 'tool', excerpt: '…needle…' }],
      total: 6_898, offset: 100, limit: 100, nextOffset: 200, hasMore: true
    }

    expect(list.hasMore).toBe(true)
    expect(search.total).toBe(6_898)
  })
})
