import {
  useCallback, useEffect, useRef, useState,
  type KeyboardEvent as ReactKeyboardEvent
} from 'react'
import { createPortal } from 'react-dom'
import { useVirtualizer } from '@tanstack/react-virtual'

import type {
  ClaudeSessionDetail,
  ClaudeSessionListResult,
  ClaudeSessionSearchHit,
  ClaudeSessionSearchResult,
  ClaudeSessionSummary
} from '@matou/contracts'
import { ConfirmDialog } from '../hierarchy/ConfirmDialog'

const SESSION_PAGE_SIZE = 50
const EVENT_PAGE_SIZE = 200
const SEARCH_PAGE_SIZE = 100

interface DetailOptions {
  beforeEventIndex?: number
  aroundEventIndex?: number
  limit?: number
}

export function SessionLoaderDialog(props: {
  targetTitle: string
  targetRunning: boolean
  listSessions(
    query: string, searchScope?: 'metadata' | 'all', offset?: number, limit?: number
  ): Promise<ClaudeSessionListResult>
  loadDetail(providerSessionId: string, options?: DetailOptions): Promise<ClaudeSessionDetail>
  searchSession(
    providerSessionId: string, query: string, offset?: number, limit?: number
  ): Promise<ClaudeSessionSearchResult>
  onLoad(providerSessionId: string): Promise<unknown>
  onCancel(): void
  portalTarget?: Element
}) {
  const {
    targetTitle, targetRunning, listSessions, loadDetail, searchSession,
    onLoad, onCancel, portalTarget
  } = props
  const [sessionQuery, setSessionQuery] = useState('')
  const [effectiveSessionQuery, setEffectiveSessionQuery] = useState('')
  const [contentQuery, setContentQuery] = useState('')
  const [effectiveContentQuery, setEffectiveContentQuery] = useState('')
  const [sessions, setSessions] = useState<ClaudeSessionSummary[]>([])
  const [sessionTotal, setSessionTotal] = useState(0)
  const [nextSessionOffset, setNextSessionOffset] = useState(0)
  const [hasMoreSessions, setHasMoreSessions] = useState(false)
  const [selectedId, setSelectedId] = useState('')
  const [detail, setDetail] = useState<ClaudeSessionDetail | null>(null)
  const [searchPage, setSearchPage] = useState<ClaudeSessionSearchResult | null>(null)
  const [activeMatchAbsolute, setActiveMatchAbsolute] = useState(0)
  const [activeEventIndex, setActiveEventIndex] = useState<number | null>(null)
  const [loadingList, setLoadingList] = useState(true)
  const [loadingMoreSessions, setLoadingMoreSessions] = useState(false)
  const [loadingDetail, setLoadingDetail] = useState(false)
  const [loadingEarlier, setLoadingEarlier] = useState(false)
  const [loadingSession, setLoadingSession] = useState(false)
  const [confirmRunning, setConfirmRunning] = useState(false)
  const [confirmDuplicate, setConfirmDuplicate] = useState(false)
  const [error, setError] = useState('')
  const listRequestSequence = useRef(0)
  const detailRequestSequence = useRef(0)
  const sessionSearchRef = useRef<HTMLInputElement>(null)
  const contentSearchRef = useRef<HTMLInputElement>(null)
  const sessionScrollRef = useRef<HTMLDivElement>(null)
  const eventScrollRef = useRef<HTMLDivElement>(null)

  const sessionVirtualizer = useVirtualizer({
    count: sessions.length,
    getScrollElement: () => sessionScrollRef.current,
    estimateSize: () => 83,
    overscan: 6,
    initialRect: { width: 340, height: 620 }
  })
  const previewEvents = detail?.events ?? []
  const eventVirtualizer = useVirtualizer({
    count: previewEvents.length,
    getScrollElement: () => eventScrollRef.current,
    estimateSize: () => 126,
    overscan: 8,
    initialRect: { width: 760, height: 620 }
  })
  const visibleSessionRows = sessionVirtualizer.getVirtualItems()
  const visibleEventRows = eventVirtualizer.getVirtualItems()

  useEffect(() => {
    sessionSearchRef.current?.focus()
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape' && !confirmDuplicate) onCancel()
      if (confirmDuplicate) return
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'f') {
        event.preventDefault()
        contentSearchRef.current?.focus()
      }
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [confirmDuplicate, onCancel])

  useEffect(() => {
    const timer = window.setTimeout(() => setEffectiveSessionQuery(sessionQuery), 180)
    return () => window.clearTimeout(timer)
  }, [sessionQuery])

  useEffect(() => {
    const timer = window.setTimeout(() => setEffectiveContentQuery(contentQuery), 180)
    return () => window.clearTimeout(timer)
  }, [contentQuery])

  useEffect(() => {
    const sequence = ++listRequestSequence.current
    setLoadingList(true)
    setError('')
    void listSessions(effectiveSessionQuery, 'metadata', 0, SESSION_PAGE_SIZE).then((result) => {
      if (sequence !== listRequestSequence.current) return
      setSessions(result.sessions)
      setSessionTotal(result.total)
      setNextSessionOffset(result.nextOffset)
      setHasMoreSessions(result.hasMore)
      setSelectedId((current) => result.sessions.some(({ providerSessionId }) =>
        providerSessionId === current) ? current : result.sessions[0]?.providerSessionId ?? '')
      sessionVirtualizer.scrollToOffset(0)
    }).catch((reason: unknown) => {
      if (sequence === listRequestSequence.current) setError(errorMessage(reason))
    }).finally(() => {
      if (sequence === listRequestSequence.current) setLoadingList(false)
    })
  }, [effectiveSessionQuery, listSessions])

  const loadMoreSessions = useCallback(async () => {
    if (loadingList || loadingMoreSessions || !hasMoreSessions) return
    const sequence = listRequestSequence.current
    setLoadingMoreSessions(true)
    try {
      const result = await listSessions(
        effectiveSessionQuery, 'metadata', nextSessionOffset, SESSION_PAGE_SIZE
      )
      if (sequence !== listRequestSequence.current) return
      setSessions((current) => {
        const known = new Set(current.map(({ providerSessionId }) => providerSessionId))
        return [...current, ...result.sessions.filter(({ providerSessionId }) => !known.has(providerSessionId))]
      })
      setSessionTotal(result.total)
      setNextSessionOffset(result.nextOffset)
      setHasMoreSessions(result.hasMore)
    } catch (reason) {
      setError(errorMessage(reason))
    } finally {
      setLoadingMoreSessions(false)
    }
  }, [effectiveSessionQuery, hasMoreSessions, listSessions, loadingList, loadingMoreSessions, nextSessionOffset])

  useEffect(() => {
    if (!selectedId) {
      setDetail(null)
      setSearchPage(null)
      return
    }
    const sequence = ++detailRequestSequence.current
    setLoadingDetail(true)
    setActiveMatchAbsolute(0)
    setActiveEventIndex(null)
    setError('')
    if (!effectiveContentQuery) {
      setSearchPage(null)
      void loadDetail(selectedId, { limit: EVENT_PAGE_SIZE }).then((result) => {
        if (sequence !== detailRequestSequence.current) return
        setDetail(result)
        requestAnimationFrame(() => eventVirtualizer.scrollToIndex(
          Math.max(0, result.events.length - 1), { align: 'end' }
        ))
      }).catch((reason: unknown) => {
        if (sequence === detailRequestSequence.current) setError(errorMessage(reason))
      }).finally(() => {
        if (sequence === detailRequestSequence.current) setLoadingDetail(false)
      })
      return
    }
    void searchSession(selectedId, effectiveContentQuery, 0, SEARCH_PAGE_SIZE).then(async (result) => {
      if (sequence !== detailRequestSequence.current) return
      setSearchPage(result)
      const hit = result.hits[0]
      if (!hit) {
        const nextDetail = await loadDetail(selectedId, { limit: EVENT_PAGE_SIZE })
        if (sequence === detailRequestSequence.current) setDetail(nextDetail)
        return
      }
      setActiveEventIndex(hit.eventIndex)
      const nextDetail = await loadDetail(selectedId, {
        aroundEventIndex: hit.eventIndex, limit: EVENT_PAGE_SIZE
      })
      if (sequence !== detailRequestSequence.current) return
      setDetail(nextDetail)
      scrollToEvent(nextDetail, hit.eventIndex, eventVirtualizer)
    }).catch((reason: unknown) => {
      if (sequence === detailRequestSequence.current) setError(errorMessage(reason))
    }).finally(() => {
      if (sequence === detailRequestSequence.current) setLoadingDetail(false)
    })
  }, [effectiveContentQuery, loadDetail, searchSession, selectedId])

  const loadEarlier = useCallback(async () => {
    if (!detail?.page.hasEarlier || loadingEarlier || effectiveContentQuery) return
    const anchorIndex = detail.events[0]?.index
    if (!anchorIndex) return
    setLoadingEarlier(true)
    try {
      const earlier = await loadDetail(selectedId, {
        beforeEventIndex: detail.page.startEventIndex, limit: EVENT_PAGE_SIZE
      })
      setDetail((current) => current?.providerSessionId === selectedId
        ? mergeEarlierDetail(earlier, current)
        : current)
      requestAnimationFrame(() => {
        const added = earlier.events.filter(({ index }) => index < anchorIndex).length
        eventVirtualizer.scrollToIndex(added, { align: 'start' })
      })
    } catch (reason) {
      setError(errorMessage(reason))
    } finally {
      setLoadingEarlier(false)
    }
  }, [detail, effectiveContentQuery, eventVirtualizer, loadDetail, loadingEarlier, selectedId])

  const navigateToHit = useCallback(async (
    absoluteIndex: number, page: ClaudeSessionSearchResult, hit: ClaudeSessionSearchHit
  ) => {
    const sequence = detailRequestSequence.current
    setActiveMatchAbsolute(absoluteIndex)
    setActiveEventIndex(hit.eventIndex)
    setLoadingDetail(true)
    try {
      const nextDetail = await loadDetail(selectedId, {
        aroundEventIndex: hit.eventIndex, limit: EVENT_PAGE_SIZE
      })
      if (sequence !== detailRequestSequence.current) return
      setSearchPage(page)
      setDetail(nextDetail)
      scrollToEvent(nextDetail, hit.eventIndex, eventVirtualizer)
    } catch (reason) {
      setError(errorMessage(reason))
    } finally {
      setLoadingDetail(false)
    }
  }, [eventVirtualizer, loadDetail, selectedId])

  const stepMatch = useCallback(async (delta: number) => {
    if (!searchPage?.total || !effectiveContentQuery) return
    const target = (activeMatchAbsolute + delta + searchPage.total) % searchPage.total
    let page = searchPage
    if (target < page.offset || target >= page.offset + page.hits.length) {
      const pageOffset = Math.floor(target / SEARCH_PAGE_SIZE) * SEARCH_PAGE_SIZE
      page = await searchSession(selectedId, effectiveContentQuery, pageOffset, SEARCH_PAGE_SIZE)
    }
    const hit = page.hits[target - page.offset]
    if (hit) await navigateToHit(target, page, hit)
  }, [activeMatchAbsolute, effectiveContentQuery, navigateToHit, searchPage, searchSession, selectedId])

  const selectedSession = sessions.find(({ providerSessionId }) => providerSessionId === selectedId)
  const selectedAvailability = selectedSession?.availability ?? detail?.availability ?? 'available'
  const submitLoad = async (duplicateConfirmed = false) => {
    if (!selectedId || loadingSession) return
    if (selectedAvailability === 'loaded-elsewhere' && !duplicateConfirmed) {
      setConfirmDuplicate(true)
      return
    }
    if (targetRunning && !confirmRunning) {
      setConfirmRunning(true)
      return
    }
    setLoadingSession(true)
    setError('')
    try {
      await onLoad(selectedId)
    } catch (reason) {
      setError(errorMessage(reason))
      setLoadingSession(false)
      setConfirmRunning(false)
    }
  }
  const onListKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return
    event.preventDefault()
    if (sessions.length === 0) return
    const current = Math.max(0, sessions.findIndex(({ providerSessionId }) => providerSessionId === selectedId))
    const offset = event.key === 'ArrowDown' ? 1 : -1
    const next = (current + offset + sessions.length) % sessions.length
    setSelectedId(sessions[next]!.providerSessionId)
    sessionVirtualizer.scrollToIndex(next, { align: 'auto' })
  }

  return createPortal(<div className="session-loader-backdrop" role="presentation"
    onPointerDown={(event) => { if (event.target === event.currentTarget) onCancel() }}>
    <section className="session-loader-dialog" role="dialog" aria-modal="true"
      aria-label="载入 Claude Code 会话">
      <header className="session-loader-header">
        <div><strong>载入 Claude Code 会话</strong><span>载入到“{targetTitle}”</span></div>
        <button type="button" aria-label="关闭会话管理" onClick={onCancel}>×</button>
      </header>
      <div className="session-loader-body">
        <aside className="session-loader-list" aria-label="可恢复会话">
          <label className="session-loader-search" data-search-scope="sessions">
            <SearchIcon />
            <input ref={sessionSearchRef} type="search" aria-label="筛选左侧会话"
              value={sessionQuery} placeholder="筛选左侧：标题、路径、模型或会话 ID"
              onChange={(event) => setSessionQuery(event.target.value)} />
            {sessionQuery && <button type="button" aria-label="清除会话筛选"
              onClick={() => setSessionQuery('')}>×</button>}
          </label>
          <div className="session-loader-list-meta">
            <span>{loadingList ? '正在查找…' : `${sessions.length} / ${sessionTotal} 个会话`}</span>
            <span>{hasMoreSessions ? '滚动加载更多' : '已加载全部'}</span>
          </div>
          <div ref={sessionScrollRef} className="session-loader-results" onKeyDown={onListKeyDown}
            onScroll={(event) => {
              const element = event.currentTarget
              if (element.scrollHeight - element.scrollTop - element.clientHeight < 140) void loadMoreSessions()
            }}>
            <div className="session-loader-virtual-list" style={{ height: sessionVirtualizer.getTotalSize() }}>
              {(visibleSessionRows.length > 0 ? visibleSessionRows : sessions.slice(0, 10).map((_, index) => ({
                index, start: index * 83
              }))).map((row) => {
                const session = sessions[row.index]!
                return <div key={session.providerSessionId} ref={sessionVirtualizer.measureElement}
                  data-index={row.index} style={{ transform: `translateY(${row.start}px)` }}
                  className="session-loader-virtual-row">
                  <article className={`session-loader-result${selectedId === session.providerSessionId ? ' selected' : ''}`}>
                    <button type="button" className="session-loader-result-main"
                      aria-label={`预览会话：${session.title}`}
                      onClick={() => setSelectedId(session.providerSessionId)}>
                      <strong>{session.title}</strong>
                      <span>{relativeTime(session.updatedAt)} · {session.model ?? 'Claude Code'}</span>
                      <small>{permissionLabel(session.permissionMode)} · {session.eventCount} 条内容</small>
                      {session.availability === 'loaded-here' && <small>已载入当前卡片</small>}
                      {session.availability === 'loaded-elsewhere' &&
                        <small>已载入“{session.loadedSessionTitle ?? '其他卡片'}”</small>}
                    </button>
                  </article>
                </div>
              })}
            </div>
            {loadingMoreSessions && <div className="session-loader-page-status">正在载入更多会话…</div>}
            {!loadingList && sessions.length === 0 && <div className="session-loader-empty">
              {effectiveSessionQuery ? '左侧没有匹配的会话' : '当前工作空间内没有 Claude Code 会话'}
            </div>}
          </div>
        </aside>
        <section className="session-loader-preview" aria-label="会话预览">
          <header>
            <div><strong>{detail?.title ?? '选择会话查看内容'}</strong>
              {detail && <span>{detail.model ?? 'Claude Code'} · {permissionLabel(detail.permissionMode)}</span>}
            </div>
            <div className="session-loader-content-search-group">
              <label className="session-loader-search" data-search-scope="content">
                <SearchIcon />
                <input ref={contentSearchRef} type="search" aria-label="查找右侧会话内容"
                  value={contentQuery} placeholder="查找右侧内容" disabled={!selectedId}
                  onChange={(event) => setContentQuery(event.target.value)} />
                {contentQuery && <button type="button" aria-label="清除内容查找"
                  onClick={() => setContentQuery('')}>×</button>}
              </label>
              {effectiveContentQuery && <div className="session-loader-match-nav" aria-label="右侧内容匹配位置">
                <span>{searchPage?.total ? activeMatchAbsolute + 1 : 0}/{searchPage?.total ?? 0}</span>
                <button type="button" aria-label="上一个匹配" onClick={() => void stepMatch(-1)}>↑</button>
                <button type="button" aria-label="下一个匹配" onClick={() => void stepMatch(1)}>↓</button>
              </div>}
            </div>
          </header>
          <div ref={eventScrollRef} className="session-loader-events" aria-busy={loadingDetail}
            onScroll={(event) => { if (event.currentTarget.scrollTop < 64) void loadEarlier() }}>
            {detail && <div className="session-loader-history-status" role="status">
              {effectiveContentQuery
                ? `全文共 ${searchPage?.total ?? 0} 处匹配`
                : `已加载 ${previewEvents.length} / ${detail.page.total} 条`}
            </div>}
            {loadingEarlier && <div className="session-loader-page-status">正在载入更早内容…</div>}
            <div className="session-loader-virtual-events" style={{ height: eventVirtualizer.getTotalSize() }}>
              {(visibleEventRows.length > 0 ? visibleEventRows : previewEvents.slice(0, 12).map((_, index) => ({
                index, start: index * 126
              }))).map((row) => {
                const event = previewEvents[row.index]!
                const matched = effectiveContentQuery && event.index === activeEventIndex
                return <div key={event.index} ref={eventVirtualizer.measureElement} data-index={row.index}
                  style={{ transform: `translateY(${row.start}px)` }} className="session-loader-virtual-row">
                  <article className={`session-loader-event is-${event.kind}${matched ? ' matched' : ''}`}>
                    <div><strong>{event.kind === 'tool' ? event.toolName ?? '工具' : event.role === 'user' ? '你' : 'Claude'}</strong>
                      <span>#{event.index}</span></div>
                    <p>{event.text}</p>
                  </article>
                </div>
              })}
            </div>
            {loadingDetail && <div className="session-loader-empty">正在载入预览…</div>}
          </div>
        </section>
      </div>
      <footer className="session-loader-footer">
        <div>
          {error && <span className="session-loader-error" role="alert">{error}</span>}
          {confirmRunning && <span className="session-loader-confirm">当前卡片正在运行，继续会结束当前进程。</span>}
        </div>
        <button type="button" onClick={onCancel}>取消</button>
        <button type="button" className="primary" disabled={!selectedId || loadingSession}
          onClick={() => void submitLoad()}>
          {loadingSession ? '正在载入…' : confirmRunning ? '结束当前运行并载入' : '载入到当前卡片'}
        </button>
      </footer>
      {confirmDuplicate && <ConfirmDialog
        title="会话已在当前工作空间载入"
        body={`“${selectedSession?.title ?? '该会话'}”已载入到“${selectedSession?.loadedSessionTitle ?? '其他卡片'}”。仍然可以载入到当前卡片，两张卡片将关联同一个 Claude Code 会话。`}
        confirmLabel="仍然载入" onCancel={() => setConfirmDuplicate(false)}
        onConfirm={() => { setConfirmDuplicate(false); void submitLoad(true) }} />}
    </section>
  </div>, portalTarget ?? document.body)
}

function mergeEarlierDetail(earlier: ClaudeSessionDetail, current: ClaudeSessionDetail): ClaudeSessionDetail {
  const existing = new Set(earlier.events.map(({ index }) => index))
  return {
    ...current,
    events: [...earlier.events, ...current.events.filter(({ index }) => !existing.has(index))],
    page: {
      startEventIndex: earlier.page.startEventIndex,
      endEventIndex: current.page.endEventIndex,
      total: current.page.total,
      hasEarlier: earlier.page.hasEarlier,
      hasLater: current.page.hasLater
    }
  }
}

type EventVirtualizer = ReturnType<typeof useVirtualizer<HTMLDivElement, Element>>

function scrollToEvent(
  detail: ClaudeSessionDetail, eventIndex: number, virtualizer: EventVirtualizer
): void {
  const index = detail.events.findIndex((event) => event.index === eventIndex)
  if (index >= 0) requestAnimationFrame(() => virtualizer.scrollToIndex(index, { align: 'center' }))
}

function SearchIcon() {
  return <svg viewBox="0 0 20 20" aria-hidden="true"><circle cx="8.5" cy="8.5" r="5.5" />
    <path d="m12.5 12.5 4 4" /></svg>
}

function permissionLabel(mode: ClaudeSessionSummary['permissionMode']): string {
  if (mode === 'bypassPermissions') return '开放所有权限'
  if (mode === 'auto') return '自动模式'
  if (mode === 'acceptEdits') return '自动接受编辑'
  if (mode === 'plan') return '计划模式'
  return '默认权限'
}

function relativeTime(timestamp: number): string {
  if (!timestamp) return '时间未知'
  const elapsed = Math.max(0, Date.now() - timestamp)
  const hours = Math.floor(elapsed / 3_600_000)
  if (hours < 24) return hours <= 0 ? '刚刚' : `${hours} 小时前`
  return `${Math.floor(hours / 24)} 天前`
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}
