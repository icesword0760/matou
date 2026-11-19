// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'

import type { ClaudeSessionDetail, ClaudeSessionListResult } from '@matou/contracts'
import { SessionLoaderDialog } from './SessionLoaderDialog'

afterEach(() => cleanup())

describe('SessionLoaderDialog DSH', () => {
  it('adds a DSH tab and carries provider=dsh through list, preview, search and load', async () => {
    const onLoad = vi.fn(async () => undefined)
    const listSessions = vi.fn(async (
      _query: string, _scope?: 'metadata' | 'all', _offset?: number, _limit?: number,
      provider?: 'claude-code' | 'antigravity' | 'codex' | 'dsh'
    ): Promise<ClaudeSessionListResult> => (
      provider === 'dsh'
        ? {
            provider: 'dsh',
            total: 1, offset: 0, limit: 50, nextOffset: 1, hasMore: false,
            sessions: [{
              provider: 'dsh',
              providerSessionId: 'dsh-1', title: 'DSH 历史会话', cwd: '/workspace',
              updatedAt: Date.now(), permissionMode: 'bypassPermissions', eventCount: 3,
              matchCount: 0, hits: [], availability: 'available'
            }]
          }
        : listResult([{
            providerSessionId: 'provider-1', title: '通知中心聚合', cwd: '/workspace',
            updatedAt: 10, model: 'claude-opus-4-6', permissionMode: 'default',
            eventCount: 2, matchCount: 0, hits: [], availability: 'available'
          }])
    ))
    const dshDetail = (): ClaudeSessionDetail => ({
      provider: 'dsh',
      providerSessionId: 'dsh-1',
      title: 'DSH 历史会话',
      cwd: '/workspace',
      updatedAt: Date.now(),
      permissionMode: 'default',
      eventCount: 2,
      matchCount: 1,
      hits: [{ eventIndex: 2, kind: 'assistant', excerpt: 'DSH 命中' }],
      availability: 'available',
      events: [
        { index: 1, kind: 'user', role: 'user', text: '你好 DSH', matched: false },
        { index: 2, kind: 'assistant', role: 'assistant', text: 'DSH 命中', matched: true }
      ],
      page: { startEventIndex: 1, endEventIndex: 2, total: 2, hasEarlier: false, hasLater: false }
    })
    const loadDetail = vi.fn(async (_providerSessionId: string, options?: { provider?: string }) => (
      options?.provider === 'dsh'
        ? dshDetail()
        : {
            providerSessionId: 'provider-1', title: '通知中心聚合', cwd: '/workspace',
            updatedAt: 10, permissionMode: 'default' as const, eventCount: 2, matchCount: 0,
            hits: [], availability: 'available' as const,
            events: [{ index: 1, kind: 'user' as const, role: 'user' as const, text: '你好', matched: false }],
            page: { startEventIndex: 1, endEventIndex: 1, total: 1, hasEarlier: false, hasLater: false }
          }
    ))
    const searchSession = vi.fn(async (
      _providerSessionId: string, query: string, offset = 0, limit = 100, provider?: string
    ) => ({
      query,
      hits: provider === 'dsh'
        ? [{ eventIndex: 2, kind: 'assistant' as const, excerpt: 'DSH 命中' }]
        : [],
      total: provider === 'dsh' ? 1 : 0, offset, limit, nextOffset: 1, hasMore: false
    }))
    render(<SessionLoaderDialog targetTitle="Shell" targetRunning={false}
      listSessions={listSessions} loadDetail={loadDetail} searchSession={searchSession}
      onLoad={onLoad} onCancel={() => undefined} />)

    expect(await screen.findByRole('dialog', { name: '载入 Claude Code 会话' })).toBeTruthy()
    expect(screen.getAllByRole('tab').map((tab) => tab.textContent))
      .toEqual(['Claude Code', 'Antigravity', 'Codex', 'DSH'])
    await userEvent.setup().click(screen.getByRole('tab', { name: 'DSH' }))

    expect(await screen.findByRole('dialog', { name: '载入 DSH 会话' })).toBeTruthy()
    await waitFor(() => expect(listSessions).toHaveBeenLastCalledWith('', 'metadata', 0, 50, 'dsh'))
    const row = await screen.findByRole('button', { name: /预览会话：DSH 历史会话/ })
    expect(row.textContent).toContain('3 条内容')
    expect(row.textContent).not.toContain('开放所有权限')
    expect(await screen.findByText('你好 DSH')).toBeTruthy()
    await waitFor(() => expect(document.querySelector('.session-loader-preview header')?.textContent)
      .not.toContain('开放所有权限'))

    fireEvent.change(screen.getByRole('searchbox', { name: '查找右侧会话内容' }), {
      target: { value: 'DSH 命中' }
    })
    await waitFor(() => expect(searchSession).toHaveBeenCalledWith(
      'dsh-1', 'DSH 命中', 0, 100, 'dsh'
    ))

    await userEvent.setup().click(screen.getByRole('button', { name: '载入到当前卡片' }))
    await waitFor(() => expect(onLoad).toHaveBeenCalledWith('dsh-1', 'dsh'))
  })
})

function listResult(
  sessions: ClaudeSessionListResult['sessions']
): ClaudeSessionListResult {
  return {
    total: sessions.length, offset: 0, limit: 50, nextOffset: sessions.length,
    hasMore: false, sessions
  }
}
