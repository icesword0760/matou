import {
  useEffect, useMemo, useRef, useState,
  type KeyboardEvent as ReactKeyboardEvent
} from 'react'
import { createPortal } from 'react-dom'

import type {
  ClaudeSessionDetail,
  ClaudeSessionListResult,
  ClaudeSessionSummary
} from '@matou/contracts'
import { ConfirmDialog } from '../hierarchy/ConfirmDialog'

export function SessionLoaderDialog(props: {
  targetTitle: string
  targetRunning: boolean
  listSessions(query: string, searchScope?: 'metadata' | 'all'): Promise<ClaudeSessionListResult>
  loadDetail(providerSessionId: string, query: string): Promise<ClaudeSessionDetail>
  onLoad(providerSessionId: string): Promise<unknown>
  onCancel(): void
  portalTarget?: Element
}) {
  const { targetTitle, targetRunning, listSessions, loadDetail, onLoad, onCancel, portalTarget } = props
  const [sessionQuery, setSessionQuery] = useState('')
  const [effectiveSessionQuery, setEffectiveSessionQuery] = useState('')
  const [contentQuery, setContentQuery] = useState('')
  const [effectiveContentQuery, setEffectiveContentQuery] = useState('')
  const [sessions, setSessions] = useState<ClaudeSessionSummary[]>([])
  const [selectedId, setSelectedId] = useState('')
  const [detail, setDetail] = useState<ClaudeSessionDetail | null>(null)
  const [loadingList, setLoadingList] = useState(true)
  const [loadingDetail, setLoadingDetail] = useState(false)
  const [loadingSession, setLoadingSession] = useState(false)
  const [confirmRunning, setConfirmRunning] = useState(false)
  const [confirmDuplicate, setConfirmDuplicate] = useState(false)
  const [error, setError] = useState('')
  const [activeMatch, setActiveMatch] = useState(0)
  const requestSequence = useRef(0)
  const eventRefs = useRef(new Map<number, HTMLElement>())
  const sessionSearchRef = useRef<HTMLInputElement>(null)
  const contentSearchRef = useRef<HTMLInputElement>(null)

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
  }, [confirmDuplicate, onCancel, selectedId])

  useEffect(() => {
    const timer = window.setTimeout(() => setEffectiveSessionQuery(sessionQuery), 180)
    return () => window.clearTimeout(timer)
  }, [sessionQuery])

  useEffect(() => {
    const timer = window.setTimeout(() => setEffectiveContentQuery(contentQuery), 180)
    return () => window.clearTimeout(timer)
  }, [contentQuery])

  useEffect(() => {
    const sequence = ++requestSequence.current
    setLoadingList(true)
    setError('')
    void listSessions(effectiveSessionQuery, 'metadata').then((result) => {
      if (sequence !== requestSequence.current) return
      setSessions(result.sessions)
      setSelectedId((current) => result.sessions.some(({ providerSessionId }) => providerSessionId === current)
        ? current
        : result.sessions[0]?.providerSessionId ?? '')
    }).catch((reason: unknown) => {
      if (sequence === requestSequence.current) setError(errorMessage(reason))
    }).finally(() => {
      if (sequence === requestSequence.current) setLoadingList(false)
    })
  }, [effectiveSessionQuery, listSessions])

  useEffect(() => {
    if (!selectedId) {
      setDetail(null)
      return
    }
    let alive = true
    setLoadingDetail(true)
    setActiveMatch(0)
    void loadDetail(selectedId, effectiveContentQuery).then((result) => {
      if (alive) setDetail(result)
    }).catch((reason: unknown) => {
      if (alive) setError(errorMessage(reason))
    }).finally(() => {
      if (alive) setLoadingDetail(false)
    })
    return () => { alive = false }
  }, [effectiveContentQuery, loadDetail, selectedId])

  const previewEvents = useMemo(() => detail?.events.slice(-240) ?? [], [detail])
  const matchedEvents = useMemo(
    () => previewEvents.filter(({ matched }) => matched),
    [previewEvents]
  )
  const selectedSession = sessions.find(({ providerSessionId }) => providerSessionId === selectedId)
  const selectedAvailability = selectedSession?.availability ?? detail?.availability ?? 'available'
  const jumpTo = (eventIndex: number) => {
    const matchIndex = matchedEvents.findIndex(({ index }) => index === eventIndex)
    if (matchIndex >= 0) setActiveMatch(matchIndex)
    eventRefs.current.get(eventIndex)?.scrollIntoView({ behavior: 'smooth', block: 'center' })
  }
  const stepMatch = (offset: number) => {
    if (matchedEvents.length === 0) return
    const next = (activeMatch + offset + matchedEvents.length) % matchedEvents.length
    setActiveMatch(next)
    jumpTo(matchedEvents[next]!.index)
  }
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
    const current = Math.max(0, sessions.findIndex(({ providerSessionId }) =>
      providerSessionId === selectedId))
    const offset = event.key === 'ArrowDown' ? 1 : -1
    setSelectedId(sessions[(current + offset + sessions.length) % sessions.length]!.providerSessionId)
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
            <span>{loadingList ? '正在查找…' : `${sessions.length} 个会话`}</span>
            <span>左侧会话列表</span>
          </div>
          <div className="session-loader-results" onKeyDown={onListKeyDown}>
            {sessions.map((session) => <article key={session.providerSessionId}
              className={`session-loader-result${selectedId === session.providerSessionId ? ' selected' : ''}`}>
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
            </article>)}
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
                <span>{matchedEvents.length ? activeMatch + 1 : 0}/{matchedEvents.length}</span>
                <button type="button" aria-label="上一个匹配" onClick={() => stepMatch(-1)}>↑</button>
                <button type="button" aria-label="下一个匹配" onClick={() => stepMatch(1)}>↓</button>
              </div>}
            </div>
          </header>
          <div className="session-loader-events" aria-busy={loadingDetail}>
            {detail && detail.eventCount > previewEvents.length && !effectiveContentQuery &&
              <div className="session-loader-preview-limit" role="note">
                为保持预览流畅，显示最近 {previewEvents.length} 条；载入后仍保留完整历史
              </div>}
            {previewEvents.map((event) => <article key={event.index}
              ref={(element) => {
                if (element) eventRefs.current.set(event.index, element)
                else eventRefs.current.delete(event.index)
              }}
              className={`session-loader-event is-${event.kind}${event.matched ? ' matched' : ''}${
                matchedEvents[activeMatch]?.index === event.index ? ' active-match' : ''}`}>
              <div><strong>{highlightMatches(
                event.kind === 'tool' ? event.toolName ?? '工具' : event.role === 'user' ? '你' : 'Claude',
                effectiveContentQuery
              )}</strong>
                <span>#{event.index}</span></div>
              <p>{highlightMatches(event.text, effectiveContentQuery)}</p>
            </article>)}
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
        <button type="button" className="primary"
          disabled={!selectedId || loadingSession}
          onClick={() => void submitLoad()}>
          {loadingSession
            ? '正在载入…'
            : confirmRunning ? '结束当前运行并载入' : '载入到当前卡片'}
        </button>
      </footer>
      {confirmDuplicate && <ConfirmDialog
        title="会话已在当前工作空间载入"
        body={`“${selectedSession?.title ?? '该会话'}”已载入到“${selectedSession?.loadedSessionTitle ?? '其他卡片'}”。仍然可以载入到当前卡片，两张卡片将关联同一个 Claude Code 会话。`}
        confirmLabel="仍然载入"
        onCancel={() => setConfirmDuplicate(false)}
        onConfirm={() => {
          setConfirmDuplicate(false)
          void submitLoad(true)
        }} />}
    </section>
  </div>, portalTarget ?? document.body)
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

function relativeTime(timestamp: number): string {
  if (!timestamp) return '时间未知'
  const elapsed = Math.max(0, Date.now() - timestamp)
  const hours = Math.floor(elapsed / 3_600_000)
  if (hours < 24) return hours <= 0 ? '刚刚' : `${hours} 小时前`
  const days = Math.floor(hours / 24)
  return `${days} 天前`
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}
