// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'

import type { ClaudeSessionDetail, ClaudeSessionListResult } from '@matou/contracts'
import { SessionLoaderDialog } from './SessionLoaderDialog'

afterEach(() => { vi.useRealTimers(); cleanup() })

describe('SessionLoaderDialog', () => {
  it('separates left session filtering from right content search and loads only from the explicit action', async () => {
    const onLoad = vi.fn(async () => undefined)
    const listSessions = vi.fn(async (): Promise<ClaudeSessionListResult> => ({
      total: 1,
      offset: 0, limit: 50, nextOffset: 1, hasMore: false,
      sessions: [{
        providerSessionId: 'provider-1', title: '通知中心聚合', cwd: '/workspace',
        updatedAt: 10, model: 'claude-opus-4-6', permissionMode: 'bypassPermissions',
        eventCount: 2, matchCount: 1,
        hits: [{ eventIndex: 2, kind: 'assistant', excerpt: '定位 hover width 动画' }],
        availability: 'available'
      }]
    }))
    const loadDetail = vi.fn(async (): Promise<ClaudeSessionDetail> => detail())
    render(<SessionLoaderDialog targetTitle="Shell" targetRunning={false}
      listSessions={listSessions} loadDetail={loadDetail}
      searchSession={emptySearch}
      onLoad={onLoad} onCancel={() => undefined} />)

    expect(await screen.findByRole('dialog', { name: '载入 Claude Code 会话' })).toBeTruthy()
    expect(screen.getAllByRole('searchbox')).toHaveLength(2)
    expect(screen.getByRole('searchbox', { name: '筛选左侧会话' })).toBeTruthy()
    expect(screen.getByRole('searchbox', { name: '查找右侧会话内容' })).toBeTruthy()
    await waitFor(() => expect(listSessions).toHaveBeenLastCalledWith('', 'metadata', 0, 50))
    const row = await screen.findByRole('button', { name: /预览会话：通知中心聚合/ })
    await userEvent.setup().click(row)
    expect(onLoad).not.toHaveBeenCalled()
    expect((await screen.findAllByText('定位 hover width 动画')).length).toBeGreaterThan(0)

    await userEvent.setup().click(screen.getByRole('button', { name: '载入到当前卡片' }))
    await waitFor(() => expect(onLoad).toHaveBeenCalledWith('provider-1'))
  })

  it('filters only left metadata and searches only inside the selected right preview', async () => {
    Element.prototype.scrollIntoView = vi.fn()
    const listSessions = vi.fn(async (): Promise<ClaudeSessionListResult> => ({
      total: 2,
      offset: 0, limit: 50, nextOffset: 2, hasMore: false,
      sessions: [summary(), {
        ...summary(), providerSessionId: 'provider-2', title: '第二个会话', matchCount: 1,
        hits: [{ eventIndex: 2, kind: 'assistant', excerpt: 'hover width' }]
      }]
    }))
    render(<SessionLoaderDialog targetTitle="Shell" targetRunning={false}
      listSessions={listSessions} loadDetail={async (providerSessionId) => {
        await new Promise((resolve) => window.setTimeout(resolve, 5))
        return { ...detail(), providerSessionId, title: providerSessionId === 'provider-2'
          ? '第二个会话' : '通知中心聚合' }
      }}
      searchSession={async (_providerSessionId, query) => ({
        query, hits: [{ eventIndex: 2, kind: 'assistant', excerpt: 'hover width' }],
        total: 1, offset: 0, limit: 100, nextOffset: 1, hasMore: false
      })}
      onLoad={async () => undefined} onCancel={() => undefined} />)
    const sessionSearch = await screen.findByRole('searchbox', { name: '筛选左侧会话' })
    fireEvent.change(sessionSearch, { target: { value: '第二个会话' } })
    await waitFor(() => expect(listSessions).toHaveBeenLastCalledWith('第二个会话', 'metadata', 0, 50))

    const contentSearch = screen.getByRole('searchbox', { name: '查找右侧会话内容' })
    fireEvent.change(contentSearch, { target: { value: 'hover width' } })
    await waitFor(() => expect(screen.getByLabelText('右侧内容匹配位置')).toBeTruthy())
    expect(listSessions).not.toHaveBeenCalledWith('hover width', expect.anything())

    fireEvent.keyDown(window, { key: 'f', metaKey: true })
    expect(document.activeElement).toBe(contentSearch)
  })

  it('loads long histories by page while keeping only viewport rows mounted', async () => {
    const events = Array.from({ length: 200 }, (_, index) => ({
      index: index + 1801, kind: 'assistant' as const, role: 'assistant' as const,
      text: `输出 ${index + 1801}`, matched: false
    }))
    const loadDetail = vi.fn(async (_providerSessionId: string, options = {}) => {
      if ('beforeEventIndex' in options) {
        const earlier = Array.from({ length: 200 }, (_, index) => ({
          index: index + 1601, kind: 'assistant' as const, role: 'assistant' as const,
          text: `输出 ${index + 1601}`, matched: false
        }))
        return { ...detail(), eventCount: 2_000, events: earlier, page: page(1601, 1800, 2_000) }
      }
      return { ...detail(), eventCount: 2_000, events, page: page(1801, 2000, 2_000) }
    })
    render(<SessionLoaderDialog targetTitle="Shell" targetRunning={false}
      listSessions={async () => listResult([{ ...summary(), eventCount: 2_000 }])}
      loadDetail={loadDetail} searchSession={emptySearch}
      onLoad={async () => undefined} onCancel={() => undefined} />)

    expect((await screen.findByRole('status')).textContent).toContain('已加载 200 / 2000 条')
    expect(document.querySelectorAll('.session-loader-event').length).toBeLessThan(30)
    fireEvent.scroll(document.querySelector('.session-loader-events')!, { target: { scrollTop: 0 } })
    await waitFor(() => expect(loadDetail).toHaveBeenCalledWith('provider-1', {
      beforeEventIndex: 1801, limit: 200
    }))
    expect(screen.queryByText(/显示最近 240 条/)).toBeNull()
  })

  it('loads every session page when the user reaches the end of the left list', async () => {
    const first = Array.from({ length: 50 }, (_, index) => ({
      ...summary(), providerSessionId: `provider-${index + 1}`, title: `会话 ${index + 1}`
    }))
    const second = Array.from({ length: 20 }, (_, index) => ({
      ...summary(), providerSessionId: `provider-${index + 51}`, title: `会话 ${index + 51}`
    }))
    const listSessions = vi.fn(async (_query: string, _scope?: 'metadata' | 'all', offset = 0) => ({
      sessions: offset === 0 ? first : second,
      total: 70, offset, limit: 50, nextOffset: offset === 0 ? 50 : 70, hasMore: offset === 0
    }))
    render(<SessionLoaderDialog targetTitle="Shell" targetRunning={false}
      listSessions={listSessions} loadDetail={async () => detail()} searchSession={emptySearch}
      onLoad={async () => undefined} onCancel={() => undefined} />)

    expect((await screen.findByText('50 / 70 个会话')).textContent).toBeTruthy()
    const results = document.querySelector('.session-loader-results')!
    Object.defineProperties(results, {
      scrollTop: { configurable: true, value: 900 },
      scrollHeight: { configurable: true, value: 1_000 },
      clientHeight: { configurable: true, value: 100 }
    })
    fireEvent.scroll(results)
    await waitFor(() => expect(listSessions).toHaveBeenCalledWith('', 'metadata', 50, 50))
    expect(await screen.findByText('70 / 70 个会话')).toBeTruthy()
  })

  it('navigates all full-history matches across search result pages', async () => {
    const searchSession = vi.fn(async (
      _providerSessionId: string, query: string, offset = 0, limit = 100
    ) => ({
      query,
      hits: Array.from({ length: Math.min(limit, 205 - offset) }, (_, index) => ({
        eventIndex: offset + index + 1, kind: 'assistant' as const, excerpt: `命中 ${offset + index + 1}`
      })),
      total: 205, offset, limit, nextOffset: Math.min(205, offset + limit),
      hasMore: offset + limit < 205
    }))
    const loadDetail = vi.fn(async (_providerSessionId: string, options?: { aroundEventIndex?: number }) => {
      const target = options?.aroundEventIndex ?? 2
      return {
        ...detail(), eventCount: 400,
        events: [{ index: target, kind: 'assistant' as const, role: 'assistant' as const,
          text: `命中 ${target}`, matched: false }],
        page: page(target, target, 400)
      }
    })
    render(<SessionLoaderDialog targetTitle="Shell" targetRunning={false}
      listSessions={async () => listResult([summary()])} loadDetail={loadDetail}
      searchSession={searchSession} onLoad={async () => undefined} onCancel={() => undefined} />)

    fireEvent.change(await screen.findByRole('searchbox', { name: '查找右侧会话内容' }), {
      target: { value: '命中' }
    })
    await waitFor(() => expect(screen.getByLabelText('右侧内容匹配位置').textContent).toContain('1/205'))
    await userEvent.setup().click(screen.getByRole('button', { name: '上一个匹配' }))
    await waitFor(() => expect(searchSession).toHaveBeenCalledWith('provider-1', '命中', 200, 100))
    await waitFor(() => expect(loadDetail).toHaveBeenCalledWith('provider-1', {
      aroundEventIndex: 205, limit: 200
    }))
    expect(screen.getByLabelText('右侧内容匹配位置').textContent).toContain('205/205')
  })

  it('keeps the dialog and current card intact when loading reports an error', async () => {
    render(<SessionLoaderDialog targetTitle="Claude" targetRunning={false}
      listSessions={async () => listResult([summary()])}
      loadDetail={async () => detail()}
      searchSession={emptySearch}
      onLoad={async () => { throw new Error('所选会话已在另一张卡片中使用') }}
      onCancel={() => undefined} />)

    await userEvent.setup().click(await screen.findByRole('button', { name: '载入到当前卡片' }))
    expect((await screen.findByRole('alert')).textContent).toContain('所选会话已在另一张卡片中使用')
    expect(screen.getByRole('dialog', { name: '载入 Claude Code 会话' })).toBeTruthy()
  })

  it('warns before loading a session already present in another card and proceeds on confirmation', async () => {
    const onLoad = vi.fn(async () => undefined)
    const occupied = {
      ...summary(),
      availability: 'loaded-elsewhere' as const,
      loadedSessionId: 'session-owner',
      loadedSessionTitle: 'Claude 主会话'
    }
    render(<SessionLoaderDialog targetTitle="Shell" targetRunning={false}
      listSessions={async () => listResult([occupied])}
      loadDetail={async () => ({ ...detail(), ...occupied })}
      searchSession={emptySearch}
      onLoad={onLoad} onCancel={() => undefined} />)

    expect(await screen.findByText('已载入“Claude 主会话”')).toBeTruthy()
    await userEvent.setup().click(screen.getByRole('button', { name: '载入到当前卡片' }))
    const warning = await screen.findByRole('alertdialog', { name: '会话已在当前工作空间载入' })
    expect(warning.textContent).toContain('Claude 主会话')
    expect(onLoad).not.toHaveBeenCalled()

    await userEvent.setup().click(screen.getByRole('button', { name: '仍然载入' }))
    await waitFor(() => expect(onLoad).toHaveBeenCalledWith('provider-1'))
  })
})

function summary() {
  return {
    providerSessionId: 'provider-1', title: '通知中心聚合', cwd: '/workspace',
    updatedAt: 10, model: 'claude-opus-4-6', permissionMode: 'bypassPermissions' as const,
    eventCount: 2, matchCount: 0, hits: [], availability: 'available' as const
  }
}

function detail(): ClaudeSessionDetail {
  return {
    ...summary(), matchCount: 1,
    hits: [{ eventIndex: 2, kind: 'assistant', excerpt: '定位 hover width 动画' }],
    events: [
      { index: 1, kind: 'user', role: 'user', text: '检查卡片闪烁', matched: false },
      { index: 2, kind: 'assistant', role: 'assistant', text: '定位 hover width 动画', matched: true }
    ],
    page: page(1, 2, 2)
  }
}

function page(startEventIndex: number, endEventIndex: number, total: number) {
  return {
    startEventIndex, endEventIndex, total,
    hasEarlier: startEventIndex > 1, hasLater: endEventIndex < total
  }
}

function listResult(sessions: ClaudeSessionListResult['sessions']): ClaudeSessionListResult {
  return {
    sessions, total: sessions.length, offset: 0, limit: 50,
    nextOffset: sessions.length, hasMore: false
  }
}

async function emptySearch(_providerSessionId: string, query: string) {
  return { query, hits: [], total: 0, offset: 0, limit: 100, nextOffset: 0, hasMore: false }
}
