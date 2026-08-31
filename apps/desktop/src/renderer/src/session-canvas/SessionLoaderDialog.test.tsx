// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'

import type { ClaudeSessionDetail, ClaudeSessionListResult } from '@matou/contracts'
import { SessionLoaderDialog } from './SessionLoaderDialog'

afterEach(() => { vi.useRealTimers(); cleanup() })

describe('SessionLoaderDialog', () => {
  it('uses one compact search, previews a selected session, and loads only from the explicit action', async () => {
    const onLoad = vi.fn(async () => undefined)
    const listSessions = vi.fn(async (): Promise<ClaudeSessionListResult> => ({
      total: 1,
      sessions: [{
        providerSessionId: 'provider-1', title: '通知中心聚合', cwd: '/workspace',
        updatedAt: 10, model: 'claude-opus-4-6', permissionMode: 'bypassPermissions',
        eventCount: 2, matchCount: 1,
        hits: [{ eventIndex: 2, kind: 'assistant', excerpt: '定位 hover width 动画' }]
      }]
    }))
    const loadDetail = vi.fn(async (): Promise<ClaudeSessionDetail> => detail())
    render(<SessionLoaderDialog targetTitle="Shell" targetRunning={false}
      listSessions={listSessions} loadDetail={loadDetail}
      onLoad={onLoad} onCancel={() => undefined} />)

    expect(await screen.findByRole('dialog', { name: '载入 Claude Code 会话' })).toBeTruthy()
    expect(screen.getAllByRole('searchbox')).toHaveLength(1)
    const row = await screen.findByRole('button', { name: /预览会话：通知中心聚合/ })
    await userEvent.setup().click(row)
    expect(onLoad).not.toHaveBeenCalled()
    expect((await screen.findAllByText('定位 hover width 动画')).length).toBeGreaterThan(0)

    await userEvent.setup().click(screen.getByRole('button', { name: '载入到当前卡片' }))
    await waitFor(() => expect(onLoad).toHaveBeenCalledWith('provider-1'))
  })

  it('searches after a short debounce and jumps from a result snippet to its exact preview event', async () => {
    Element.prototype.scrollIntoView = vi.fn()
    const listSessions = vi.fn(async (): Promise<ClaudeSessionListResult> => ({
      total: 2,
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
      onLoad={async () => undefined} onCancel={() => undefined} />)
    const search = await screen.findByRole('searchbox', { name: '搜索会话内容' })
    fireEvent.change(search, { target: { value: 'hover width' } })
    await waitFor(() => expect(listSessions).toHaveBeenLastCalledWith('hover width', undefined))

    fireEvent.click(await screen.findByRole('button', { name: '跳转到第 2 条会话内容' }))
    await waitFor(() => expect(Element.prototype.scrollIntoView).toHaveBeenCalled())
    expect(screen.getByLabelText('会话预览').textContent).toContain('第二个会话')

    fireEvent.keyDown(window, { key: 'f', metaKey: true })
    await waitFor(() => expect(listSessions).toHaveBeenLastCalledWith('hover width', 'provider-2'))
    expect(screen.getByRole('button', { name: '当前会话' })).toBeTruthy()
  })

  it('keeps the dialog and current card intact when loading reports an error', async () => {
    render(<SessionLoaderDialog targetTitle="Claude" targetRunning={false}
      listSessions={async () => ({ total: 1, sessions: [summary()] })}
      loadDetail={async () => detail()}
      onLoad={async () => { throw new Error('所选会话已在另一张卡片中使用') }}
      onCancel={() => undefined} />)

    await userEvent.setup().click(await screen.findByRole('button', { name: '载入到当前卡片' }))
    expect((await screen.findByRole('alert')).textContent).toContain('所选会话已在另一张卡片中使用')
    expect(screen.getByRole('dialog', { name: '载入 Claude Code 会话' })).toBeTruthy()
  })
})

function summary() {
  return {
    providerSessionId: 'provider-1', title: '通知中心聚合', cwd: '/workspace',
    updatedAt: 10, model: 'claude-opus-4-6', permissionMode: 'bypassPermissions' as const,
    eventCount: 2, matchCount: 0, hits: []
  }
}

function detail(): ClaudeSessionDetail {
  return {
    ...summary(), matchCount: 1,
    hits: [{ eventIndex: 2, kind: 'assistant', excerpt: '定位 hover width 动画' }],
    events: [
      { index: 1, kind: 'user', role: 'user', text: '检查卡片闪烁', matched: false },
      { index: 2, kind: 'assistant', role: 'assistant', text: '定位 hover width 动画', matched: true }
    ]
  }
}
