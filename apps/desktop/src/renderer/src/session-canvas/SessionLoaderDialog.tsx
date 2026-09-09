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
import { useMessages } from '../i18n/LocaleProvider'
import type { Messages } from '../i18n/messages'

type LoaderMessages = Messages['sessionCanvas']['loader']

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
  const messages = useMessages()
  const m = messages.sessionCanvas.loader
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
      aria-label={messages.hierarchyTerminal.pane.loadClaudeSession}>
      <header className="session-loader-header">
        <div><strong>{messages.hierarchyTerminal.pane.loadClaudeSession}</strong>
          <span>{m.loadInto(targetTitle)}</span></div>
        <button type="button" aria-label={m.close} onClick={onCancel}>×</button>
      </header>
      <div className="session-loader-body">
        <aside className="session-loader-list" aria-label={m.resumableSessions}>
          <label className="session-loader-search" data-search-scope="sessions">
            <SearchIcon />
            <input ref={sessionSearchRef} type="search" aria-label={m.filterSessions}
              value={sessionQuery} placeholder={m.filterSessionsPlaceholder}
              onChange={(event) => setSessionQuery(event.target.value)} />
            {sessionQuery && <button type="button" aria-label={m.clearSessionFilter}
              onClick={() => setSessionQuery('')}>×</button>}
          </label>
          <div className="session-loader-list-meta">
            <span>{loadingList ? m.searching : m.sessionsOf(sessions.length, sessionTotal)}</span>
            <span>{hasMoreSessions ? m.scrollForMore : m.allLoaded}</span>
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
                      aria-label={m.previewSession(session.title)}
                      onClick={() => setSelectedId(session.providerSessionId)}>
                      <strong>{session.title}</strong>
                      <span>{relativeTime(session.updatedAt, m)} · {session.model ?? 'Claude Code'}</span>
                      <small>{m.permission[session.permissionMode]} · {m.entries(session.eventCount)}</small>
                      {session.availability === 'loaded-here' && <small>{m.loadedHere}</small>}
                      {session.availability === 'loaded-elsewhere' &&
                        <small>{m.loadedElsewhere(session.loadedSessionTitle
                          ? m.quoted(session.loadedSessionTitle)
                          : m.otherCard)}</small>}
                    </button>
                  </article>
                </div>
              })}
            </div>
            {loadingMoreSessions && <div className="session-loader-page-status">{m.loadingMore}</div>}
            {!loadingList && sessions.length === 0 && <div className="session-loader-empty">
              {effectiveSessionQuery ? m.noMatchingSessions : m.noSessions}
            </div>}
          </div>
        </aside>
        <section className="session-loader-preview" aria-label={m.preview}>
          <header>
            <div><strong>{detail?.title ?? m.selectSession}</strong>
              {detail && <span>{detail.model ?? 'Claude Code'} · {m.permission[detail.permissionMode]}</span>}
            </div>
            <div className="session-loader-content-search-group">
              <label className="session-loader-search" data-search-scope="content">
                <SearchIcon />
                <input ref={contentSearchRef} type="search" aria-label={m.searchContent}
                  value={contentQuery} placeholder={m.searchContentPlaceholder} disabled={!selectedId}
                  onChange={(event) => setContentQuery(event.target.value)} />
                {contentQuery && <button type="button" aria-label={m.clearContentSearch}
                  onClick={() => setContentQuery('')}>×</button>}
              </label>
              {effectiveContentQuery && <div className="session-loader-match-nav" aria-label={m.matchNav}>
                <span>{searchPage?.total ? activeMatchAbsolute + 1 : 0}/{searchPage?.total ?? 0}</span>
                <button type="button" aria-label={m.previousMatch} onClick={() => void stepMatch(-1)}>↑</button>
                <button type="button" aria-label={m.nextMatch} onClick={() => void stepMatch(1)}>↓</button>
              </div>}
            </div>
          </header>
          <div ref={eventScrollRef} className="session-loader-events" aria-busy={loadingDetail}
            onScroll={(event) => { if (event.currentTarget.scrollTop < 64) void loadEarlier() }}>
            {detail && <div className="session-loader-history-status" role="status">
              {effectiveContentQuery
                ? m.matches(searchPage?.total ?? 0)
                : m.entriesLoaded(previewEvents.length, detail.page.total)}
            </div>}
            {loadingEarlier && <div className="session-loader-page-status">{m.loadingEarlier}</div>}
            <div className="session-loader-virtual-events" style={{ height: eventVirtualizer.getTotalSize() }}>
              {(visibleEventRows.length > 0 ? visibleEventRows : previewEvents.slice(0, 12).map((_, index) => ({
                index, start: index * 126
              }))).map((row) => {
                const event = previewEvents[row.index]!
                const matched = effectiveContentQuery && event.index === activeEventIndex
                return <div key={event.index} ref={eventVirtualizer.measureElement} data-index={row.index}
                  style={{ transform: `translateY(${row.start}px)` }} className="session-loader-virtual-row">
                  <article className={`session-loader-event is-${event.kind}${
                    matched ? ' matched active-match' : ''}`}>
                    <div><strong>{highlightMatches(
                      event.kind === 'tool' ? event.toolName ?? m.tool : event.role === 'user' ? m.you : 'Claude',
                      effectiveContentQuery
                    )}</strong>
                      <span>#{event.index}</span></div>
                    <p>{highlightMatches(event.text, effectiveContentQuery)}</p>
                  </article>
                </div>
              })}
            </div>
            {loadingDetail && <div className="session-loader-empty">{m.loadingPreview}</div>}
          </div>
        </section>
      </div>
      <footer className="session-loader-footer">
        <div>
          {error && <span className="session-loader-error" role="alert">{error}</span>}
          {confirmRunning && <span className="session-loader-confirm">{m.runningWarning}</span>}
        </div>
        <button type="button" onClick={onCancel}>{messages.common.cancel}</button>
        <button type="button" className="primary" disabled={!selectedId || loadingSession}
          onClick={() => void submitLoad()}>
          {loadingSession ? m.loading : confirmRunning ? m.endAndLoad : m.loadHere}
        </button>
      </footer>
      {confirmDuplicate && <ConfirmDialog
        title={m.duplicateTitle}
        body={m.duplicateBody(
          selectedSession?.title ? m.quoted(selectedSession.title) : m.thisSession,
          selectedSession?.loadedSessionTitle
            ? m.quoted(selectedSession.loadedSessionTitle)
            : m.otherCard
        )}
        confirmLabel={m.loadAnyway} onCancel={() => setConfirmDuplicate(false)}
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

function highlightMatches(text: string, query: string) {
  const needle = query.trim()
  if (!needle) return text
  const normalizedText = text.toLocaleLowerCase()
  const normalizedNeedle = needle.toLocaleLowerCase()
  const parts: Array<string | ReturnType<typeof markMatch>> = []
  let cursor = 0
  let matchIndex = normalizedText.indexOf(normalizedNeedle)
  while (matchIndex >= 0) {
    if (matchIndex > cursor) parts.push(text.slice(cursor, matchIndex))
    const end = matchIndex + normalizedNeedle.length
    parts.push(markMatch(text.slice(matchIndex, end), matchIndex))
    cursor = end
    matchIndex = normalizedText.indexOf(normalizedNeedle, cursor)
  }
  if (cursor === 0) return text
  if (cursor < text.length) parts.push(text.slice(cursor))
  return parts
}

function markMatch(text: string, key: number) {
  return <mark key={key}>{text}</mark>
}

function relativeTime(timestamp: number, m: LoaderMessages): string {
  if (!timestamp) return m.timeUnknown
  const elapsed = Math.max(0, Date.now() - timestamp)
  const hours = Math.floor(elapsed / 3_600_000)
  if (hours < 24) return hours <= 0 ? m.justNow : m.hoursAgo(hours)
  return m.daysAgo(Math.floor(hours / 24))
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}
