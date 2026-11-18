import type { ClaudeSessionDetail, ClaudeSessionEventKind, ClaudeSessionPreviewEvent } from '@matou/contracts'
import { isAgentProfileId } from '@matou/domain'
import { useEffect, useState } from 'react'

import { useLocale, useMessages } from '../i18n/LocaleProvider'
import { useRuntimeClient } from '../runtime/RuntimeProvider'
import type { SessionView } from './hierarchy-types'

interface PreviewItem {
  key: string
  kind: ClaudeSessionEventKind
  roleLabel: string
  text: string
}

const PREVIEW_EVENT_LIMIT = 30
const MAX_PREVIEW_ITEMS = 4
const MAX_PREVIEW_CHARS = 240

function formatPreviewText(text: string): string {
  const normalized = text.trim().replace(/\s+/g, ' ')
  return normalized.length > MAX_PREVIEW_CHARS ? `${normalized.slice(0, MAX_PREVIEW_CHARS)}…` : normalized
}

function formatUpdatedAt(timestamp: number | undefined, locale: 'zh-CN' | 'en'): string | undefined {
  if (!timestamp || !Number.isFinite(timestamp) || timestamp <= 0) return undefined
  try {
    return new Intl.DateTimeFormat(locale, {
      month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit'
    }).format(timestamp)
  } catch { return undefined }
}

function eventRoleLabel(
  event: ClaudeSessionPreviewEvent,
  agentShortName: string,
  m: ReturnType<typeof useMessages>['hierarchyTerminal']['pane']
): string {
  if (event.kind === 'user') return m.deferredRoleUser
  if (event.kind === 'assistant') return agentShortName
  if (event.kind === 'tool') return event.toolName?.trim() || m.deferredRoleTool
  return m.deferredRoleSystem
}

function selectPreviewItems(
  detail: ClaudeSessionDetail | undefined,
  latestLines: readonly string[],
  agentShortName: string,
  m: ReturnType<typeof useMessages>['hierarchyTerminal']['pane']
): PreviewItem[] {
  const events = detail?.events ?? []
  const dialogue = events.filter(event =>
    (event.kind === 'user' || event.kind === 'assistant') && event.text.trim().length > 0
  )
  const chosen = dialogue.length > 0
    ? dialogue.slice(-MAX_PREVIEW_ITEMS)
    : events.filter(event => event.text.trim().length > 0).slice(-3)
  if (chosen.length > 0) {
    return chosen.map(event => ({
      key: `event-${event.index}`,
      kind: event.kind,
      roleLabel: eventRoleLabel(event, agentShortName, m),
      text: formatPreviewText(event.text)
    }))
  }
  return latestLines
    .map(line => formatPreviewText(line))
    .filter(Boolean)
    .slice(-MAX_PREVIEW_ITEMS)
    .map((text, index) => ({
      key: `line-${index}`,
      kind: 'assistant' as const,
      roleLabel: agentShortName,
      text
    }))
}

/** Deferred startup recovery card showing the saved session title and recent replies before launch. */
export function DeferredRecoveryCard({
  session,
  cwd,
  agentDisplayName,
  agentShortName,
  latestLines,
  readOnly,
  storageBlocked,
  onRestore,
  onViewHistory
}: {
  session: SessionView
  cwd?: string | undefined
  agentDisplayName: string
  agentShortName: string
  latestLines: string[]
  readOnly: boolean
  storageBlocked: boolean
  onRestore?: ((sessionId: string) => unknown) | undefined
  onViewHistory?: ((sessionId: string) => unknown) | undefined
}) {
  const locale = useLocale()
  const m = useMessages().hierarchyTerminal
  const runtimeClient = useRuntimeClient()
  const isAgent = isAgentProfileId(session.kind)
  const [detail, setDetail] = useState<ClaudeSessionDetail>()
  const [loading, setLoading] = useState(false)

  useEffect(() => {
    if (!isAgent || typeof runtimeClient?.request !== 'function') {
      setDetail(undefined); setLoading(false)
      return
    }
    let active = true
    setLoading(true)
    void runtimeClient.request<ClaudeSessionDetail>('claude-sessions.detail', {
      sessionId: session.id,
      current: true,
      provider: session.kind,
      limit: PREVIEW_EVENT_LIMIT
    }).then(result => {
      if (!active) return
      if (result && Array.isArray(result.events)) setDetail(result)
    }).catch(() => {
      if (active) setDetail(undefined)
    }).finally(() => {
      if (active) setLoading(false)
    })
    return () => { active = false }
  }, [isAgent, runtimeClient, session.id, session.kind])

  const fallbackTitle = session.providerTitle?.trim() || detail?.title?.trim()
  const displayTitle = session.titleSource === 'default' && fallbackTitle ? fallbackTitle : session.title
  const secondaryTitle = session.providerTitle?.trim() && session.providerTitle.trim() !== displayTitle
    ? session.providerTitle.trim()
    : detail?.title?.trim() && detail.title.trim() !== displayTitle
      ? detail.title.trim()
      : undefined
  const location = detail?.cwd?.trim() || cwd?.trim()
  const updatedLabel = formatUpdatedAt(detail?.updatedAt, locale)
  const previewItems = selectPreviewItems(detail, latestLines, agentShortName, m.pane)

  return <div className="session-recovery-overlay state-deferred" data-testid="session-deferred-recovery"
    role="status" aria-label={m.pane.manualRecoveryFor(session.title)}
    onPointerDown={event => event.stopPropagation()}>
    <div className="session-recovery-overlay__content session-deferred-card">
      <div className="session-deferred-card__meta">
        <span className="session-deferred-card__badge">{m.pane.recoveryDeferred}</span>
        {isAgent && <span className="session-deferred-card__agent">{agentDisplayName}</span>}
        {updatedLabel && <span className="session-deferred-card__time">{updatedLabel}</span>}
      </div>
      <strong className="session-deferred-card__title" title={displayTitle}>{displayTitle}</strong>
      {secondaryTitle && <span className="session-deferred-card__subtitle" title={secondaryTitle}>{secondaryTitle}</span>}
      {location && <span className="session-deferred-card__cwd" title={location}>{location}</span>}
      {previewItems.length > 0 ? <div className="session-deferred-card__replies" aria-label={m.pane.deferredRepliesTitle}>
        <div className="session-deferred-card__replies-label">{m.pane.deferredRepliesTitle}</div>
        <ul className="session-deferred-card__list">
          {previewItems.map(item => <li key={item.key} className={`session-deferred-card__item is-${item.kind}`}>
            <span className="session-deferred-card__role">{item.roleLabel}</span>
            <p className="session-deferred-card__text">{item.text}</p>
          </li>)}
        </ul>
      </div> : loading ? <p className="session-deferred-card__hint">{m.pane.deferredRepliesLoading}</p> : null}
      <p className="session-deferred-card__note">{m.pane.recoveryDeferredBody}</p>
      <div className="session-deferred-card__actions">
        {onViewHistory && isAgent && <button type="button" className="session-deferred-card__secondary"
          onClick={event => { event.stopPropagation(); void onViewHistory(session.id) }}>
          {m.pane.sessionHistory}
        </button>}
        {onRestore && <button type="button" className="session-deferred-card__primary" disabled={readOnly || storageBlocked}
          aria-label={m.pane.manualRecoveryFor(session.title)}
          onClick={event => { event.stopPropagation(); void onRestore(session.id) }}>
          {m.pane.manualRecovery}
        </button>}
      </div>
    </div>
  </div>
}
