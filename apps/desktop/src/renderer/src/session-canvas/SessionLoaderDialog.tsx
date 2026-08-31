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

export function SessionLoaderDialog(props: {
  targetTitle: string
  targetRunning: boolean
  listSessions(query: string, providerSessionId?: string): Promise<ClaudeSessionListResult>
  loadDetail(providerSessionId: string, query: string): Promise<ClaudeSessionDetail>
  onLoad(providerSessionId: string): Promise<unknown>
  onCancel(): void
  portalTarget?: Element
}) {
  const { targetTitle, targetRunning, listSessions, loadDetail, onLoad, onCancel, portalTarget } = props
  const [query, setQuery] = useState('')
  const [effectiveQuery, setEffectiveQuery] = useState('')
  const [sessions, setSessions] = useState<ClaudeSessionSummary[]>([])
  const [selectedId, setSelectedId] = useState('')
  const [detail, setDetail] = useState<ClaudeSessionDetail | null>(null)
  const [loadingList, setLoadingList] = useState(true)
  const [loadingDetail, setLoadingDetail] = useState(false)
  const [loadingSession, setLoadingSession] = useState(false)
  const [confirmRunning, setConfirmRunning] = useState(false)
  const [error, setError] = useState('')
  const [activeMatch, setActiveMatch] = useState(0)
  const [scope, setScope] = useState<'all' | 'current'>('all')
  const [currentScopeId, setCurrentScopeId] = useState('')
  const [pendingJump, setPendingJump] = useState<{
    providerSessionId: string
    eventIndex: number
  } | null>(null)
  const requestSequence = useRef(0)
  const eventRefs = useRef(new Map<number, HTMLElement>())
  const searchRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    searchRef.current?.focus()
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') onCancel()
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'f') {
        event.preventDefault()
        if (selectedId) {
          setCurrentScopeId(selectedId)
          setScope('current')
        }
        searchRef.current?.focus()
      }
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [onCancel, selectedId])

  useEffect(() => {
    const timer = window.setTimeout(() => setEffectiveQuery(query), 180)
    return () => window.clearTimeout(timer)
  }, [query])

  useEffect(() => {
    const sequence = ++requestSequence.current
    setLoadingList(true)
    setError('')
    void listSessions(effectiveQuery, scope === 'current' ? currentScopeId : undefined).then((result) => {
      if (sequence !== requestSequence.current) return
      setSessions(result.sessions)
      setSelectedId((current) => scope === 'current'
        ? currentScopeId || current
        : result.sessions.some(({ providerSessionId }) => providerSessionId === current)
          ? current
          : result.sessions[0]?.providerSessionId ?? '')
    }).catch((reason: unknown) => {
      if (sequence === requestSequence.current) setError(errorMessage(reason))
    }).finally(() => {
      if (sequence === requestSequence.current) setLoadingList(false)
    })
  }, [currentScopeId, effectiveQuery, listSessions, scope])

  useEffect(() => {
    if (!selectedId) {
      setDetail(null)
      return
    }
    let alive = true
    setLoadingDetail(true)
    setActiveMatch(0)
    void loadDetail(selectedId, effectiveQuery).then((result) => {
      if (alive) setDetail(result)
    }).catch((reason: unknown) => {
      if (alive) setError(errorMessage(reason))
    }).finally(() => {
      if (alive) setLoadingDetail(false)
    })
    return () => { alive = false }
  }, [effectiveQuery, loadDetail, selectedId])

  const matchedEvents = useMemo(
    () => detail?.events.filter(({ matched }) => matched) ?? [],
    [detail]
  )
  const jumpTo = (eventIndex: number) => {
    const matchIndex = matchedEvents.findIndex(({ index }) => index === eventIndex)
    if (matchIndex >= 0) setActiveMatch(matchIndex)
    eventRefs.current.get(eventIndex)?.scrollIntoView({ behavior: 'smooth', block: 'center' })
  }
  useEffect(() => {
    if (!pendingJump || detail?.providerSessionId !== pendingJump.providerSessionId) return
    jumpTo(pendingJump.eventIndex)
    setPendingJump(null)
  }, [detail, pendingJump])
  const stepMatch = (offset: number) => {
    if (matchedEvents.length === 0) return
    const next = (activeMatch + offset + matchedEvents.length) % matchedEvents.length
    setActiveMatch(next)
    jumpTo(matchedEvents[next]!.index)
  }
  const submitLoad = async () => {
    if (!selectedId || loadingSession) return
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
          <label className="session-loader-search">
            <SearchIcon />
            <input ref={searchRef} type="search" aria-label="搜索会话内容"
              value={query} placeholder="搜索标题、内容、工具或会话 ID"
              onChange={(event) => setQuery(event.target.value)} />
            {query && <button type="button" aria-label="清除搜索" onClick={() => setQuery('')}>×</button>}
          </label>
          <div className="session-loader-list-meta">
            <span>{loadingList ? '正在查找…' : `${sessions.length} 个可恢复会话`}</span>
            <button type="button" className="session-loader-scope" onClick={() => {
              if (scope === 'all' && selectedId) {
                setCurrentScopeId(selectedId)
                setScope('current')
              } else {
                setScope('all')
              }
            }}>{scope === 'all' ? '全部会话' : '当前会话'}</button>
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
              </button>
              {session.hits.map((hit) => <button type="button" className="session-loader-hit"
                key={`${session.providerSessionId}:${hit.eventIndex}`}
                aria-label={`跳转到第 ${hit.eventIndex} 条会话内容`}
                onClick={() => {
                  setSelectedId(session.providerSessionId)
                  setPendingJump({
                    providerSessionId: session.providerSessionId,
                    eventIndex: hit.eventIndex
                  })
                }}>{hit.excerpt}</button>)}
            </article>)}
            {!loadingList && sessions.length === 0 && <div className="session-loader-empty">
              {effectiveQuery ? '当前工作空间内没有匹配内容' : '当前工作空间内没有可恢复的 Claude Code 会话'}
            </div>}
          </div>
        </aside>
        <section className="session-loader-preview" aria-label="会话预览">
          <header>
            <div><strong>{detail?.title ?? '选择会话查看内容'}</strong>
              {detail && <span>{detail.model ?? 'Claude Code'} · {permissionLabel(detail.permissionMode)}</span>}
            </div>
            {effectiveQuery && <div className="session-loader-match-nav" aria-label="搜索匹配位置">
              <span>{matchedEvents.length ? activeMatch + 1 : 0}/{matchedEvents.length}</span>
              <button type="button" aria-label="上一个匹配" onClick={() => stepMatch(-1)}>↑</button>
              <button type="button" aria-label="下一个匹配" onClick={() => stepMatch(1)}>↓</button>
            </div>}
          </header>
          <div className="session-loader-events" aria-busy={loadingDetail}>
            {detail?.events.map((event) => <article key={event.index}
              ref={(element) => {
                if (element) eventRefs.current.set(event.index, element)
                else eventRefs.current.delete(event.index)
              }}
              className={`session-loader-event is-${event.kind}${event.matched ? ' matched' : ''}`}>
              <div><strong>{event.kind === 'tool' ? event.toolName ?? '工具' : event.role === 'user' ? '你' : 'Claude'}</strong>
                <span>#{event.index}</span></div>
              <p>{event.text}</p>
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
        <button type="button" className="primary" disabled={!selectedId || loadingSession}
          onClick={() => void submitLoad()}>
          {loadingSession ? '正在载入…' : confirmRunning ? '结束当前运行并载入' : '载入到当前卡片'}
        </button>
      </footer>
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
