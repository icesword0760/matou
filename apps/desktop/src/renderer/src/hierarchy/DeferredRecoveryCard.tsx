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

const PREVIEW_EVENT_LIMIT = 200
const MAX_PREVIEW_ITEMS = 4
const MAX_PREVIEW_CHARS = 240

const SYNTHETIC_XML_BLOCK_RE =
  /<(local-command-caveat|local-command-stdout|system-reminder|task-notification|teammate-message|agent-message)\b[^>]*>[\s\S]*?<\/\1>/gi
const UNCLOSED_SYNTHETIC_XML_RE =
  /<(local-command-caveat|local-command-stdout|system-reminder|task-notification|teammate-message|agent-message)\b[\s\S]*/i
const COMMAND_ARGS_RE = /<command-args>([\s\S]*?)<\/command-args>/i
const SYNTHETIC_USER_PREFIXES = [
  'Base directory for this skill:',
  'Launching skill:',
  'Stop hook feedback:',
  'A session-scoped Stop hook is now active',
  'Goal check-in:',
  '[Request interrupted by user]',
  '[Image: source:'
]
const SYNTHETIC_ASSISTANT_PREFIXES = [
  'API Error:',
  'Prompt is too long',
  'automatic compaction failed:',
  'All credentials for model ',
  'No response requested.'
]

function sanitizePreviewText(kind: 'user' | 'assistant' | 'other', rawText: string): string {
  const trimmed = rawText.trim()
  if (!trimmed) return ''
  if (kind === 'assistant') {
    if (SYNTHETIC_ASSISTANT_PREFIXES.some((prefix) => trimmed.startsWith(prefix))) return ''
    return trimmed
  }
  if (kind === 'user') {
    if (SYNTHETIC_USER_PREFIXES.some((prefix) => trimmed.startsWith(prefix))) return ''
    if (trimmed.includes('<command-name>')) {
      const args = trimmed.match(COMMAND_ARGS_RE)?.[1]?.trim() ?? ''
      return args ? sanitizePreviewText('user', args) : ''
    }
    const stripped = trimmed
      .replace(SYNTHETIC_XML_BLOCK_RE, ' ')
      .replace(UNCLOSED_SYNTHETIC_XML_RE, ' ')
      .replace(/^(?:\[Image #\d+\]\s*)+/i, '')
      .trim()
    if (!stripped || SYNTHETIC_USER_PREFIXES.some((prefix) => stripped.startsWith(prefix))) return ''
    return stripped
  }
  if (SYNTHETIC_ASSISTANT_PREFIXES.some((prefix) => trimmed.startsWith(prefix))) return ''
  const stripped = trimmed
    .replace(SYNTHETIC_XML_BLOCK_RE, ' ')
    .replace(UNCLOSED_SYNTHETIC_XML_RE, ' ')
    .trim()
  return stripped
}

function isGenericAgentTitle(value: string | undefined, agentShortName: string): boolean {
  const normalized = value?.trim().toLowerCase() ?? ''
  if (!normalized) return true
  const short = agentShortName.trim().toLowerCase()
  return normalized === short ||
    normalized === 'claude' ||
    normalized === 'claude code' ||
    normalized === 'antigravity' ||
    normalized === 'codex' ||
    normalized === 'shell' ||
    normalized === '未命名会话' ||
    normalized === '未命名 claude 会话'
}

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
  const dialogue = events.flatMap((event) => {
    if (event.kind !== 'user' && event.kind !== 'assistant') return []
    const text = sanitizePreviewText(event.kind, event.text)
    if (!text) return []
    return [{ ...event, text }]
  })
  const fallbackEvents = dialogue.length > 0
    ? dialogue
    : events.flatMap((event) => {
      const text = sanitizePreviewText('other', event.text)
      if (!text) return []
      return [{ ...event, text }]
    })
  const chosen = fallbackEvents.length > 0
    ? fallbackEvents.slice(-MAX_PREVIEW_ITEMS)
    : []
  if (chosen.length > 0) {
    return chosen.map(event => ({
      key: `event-${event.index}`,
      kind: event.kind,
      roleLabel: eventRoleLabel(event, agentShortName, m),
      text: formatPreviewText(event.text)
    }))
  }
  return latestLines
    .map(line => formatPreviewText(sanitizePreviewText('other', line)))
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
  taskName,
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
  taskName?: string | undefined
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

  const cleanTranscriptTitle = detail?.transcriptTitle
    ? sanitizePreviewText('user', detail.transcriptTitle)
    : detail?.title
      ? sanitizePreviewText('user', detail.title)
      : undefined
  const usableTranscriptTitle = cleanTranscriptTitle && !isGenericAgentTitle(cleanTranscriptTitle, agentShortName)
    ? cleanTranscriptTitle
    : undefined
  const cleanProviderTitle = session.providerTitle?.trim()
  const fallbackTitle = cleanProviderTitle || usableTranscriptTitle
  const useFallback = session.titleSource === 'default' ||
    (session.titleSource === undefined && isGenericAgentTitle(session.title, agentShortName))
  const displayTitle = useFallback && fallbackTitle
    ? fallbackTitle
    : useFallback && taskName?.trim() && isGenericAgentTitle(session.title, agentShortName)
      ? taskName.trim()
      : session.title
  const cleanTaskLabel = taskName?.trim()
  const secondaryTitle = cleanProviderTitle && cleanProviderTitle !== displayTitle
    ? cleanProviderTitle
    : usableTranscriptTitle && usableTranscriptTitle !== displayTitle
      ? usableTranscriptTitle
      : cleanTaskLabel && cleanTaskLabel !== displayTitle
        ? cleanTaskLabel
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
