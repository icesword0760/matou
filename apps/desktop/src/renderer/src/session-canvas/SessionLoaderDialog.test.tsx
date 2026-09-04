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
      onLoad={onLoad} onCancel={() => undefined} />)

    expect(await screen.findByRole('dialog', { name: '载入 Claude Code 会话' })).toBeTruthy()
    expect(screen.getAllByRole('searchbox')).toHaveLength(2)
    expect(screen.getByRole('searchbox', { name: '筛选左侧会话' })).toBeTruthy()
    expect(screen.getByRole('searchbox', { name: '查找右侧会话内容' })).toBeTruthy()
    await waitFor(() => expect(listSessions).toHaveBeenLastCalledWith('', 'metadata'))
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
    const sessionSearch = await screen.findByRole('searchbox', { name: '筛选左侧会话' })
    fireEvent.change(sessionSearch, { target: { value: '第二个会话' } })
    await waitFor(() => expect(listSessions).toHaveBeenLastCalledWith('第二个会话', 'metadata'))

    const contentSearch = screen.getByRole('searchbox', { name: '查找右侧会话内容' })
    fireEvent.change(contentSearch, { target: { value: 'hover width' } })
    await waitFor(() => expect(screen.getByLabelText('右侧内容匹配位置')).toBeTruthy())
    expect(await screen.findByText('hover width', { selector: 'mark' })).toBeTruthy()
    expect(listSessions).not.toHaveBeenCalledWith('hover width', expect.anything())

    fireEvent.keyDown(window, { key: 'f', metaKey: true })
    expect(document.activeElement).toBe(contentSearch)
  })

  it('bounds a very large preview to keep selection and search responsive', async () => {
    const events = Array.from({ length: 2_000 }, (_, index) => ({
      index: index + 1, kind: 'assistant' as const, role: 'assistant' as const,
      text: `输出 ${index + 1}`, matched: false
    }))
    render(<SessionLoaderDialog targetTitle="Shell" targetRunning={false}
      listSessions={async () => ({ total: 1, sessions: [{ ...summary(), eventCount: events.length }] })}
      loadDetail={async () => ({ ...detail(), eventCount: events.length, events })}
      onLoad={async () => undefined} onCancel={() => undefined} />)

    await screen.findByRole('note')
    expect(document.querySelectorAll('.session-loader-event')).toHaveLength(240)
    expect(screen.getByRole('note').textContent).toContain('完整历史')
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

  it('warns before loading a session already present in another card and proceeds on confirmation', async () => {
    const onLoad = vi.fn(async () => undefined)
    const occupied = {
      ...summary(),
      availability: 'loaded-elsewhere' as const,
      loadedSessionId: 'session-owner',
      loadedSessionTitle: 'Claude 主会话'
    }
    render(<SessionLoaderDialog targetTitle="Shell" targetRunning={false}
      listSessions={async () => ({ total: 1, sessions: [occupied] })}
      loadDetail={async () => ({ ...detail(), ...occupied })}
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
    ]
  }
}
