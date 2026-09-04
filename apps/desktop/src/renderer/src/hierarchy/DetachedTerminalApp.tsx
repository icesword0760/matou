import { useEffect, useMemo, useRef, useState } from 'react'

import type { RuntimeMessage, RuntimeMode } from '@matou/contracts'

import {
  TerminalSurface,
  type TerminalStorageFaultMessage
} from '../terminal/TerminalSurface'
import { useRuntimeClient } from '../runtime/RuntimeProvider'
import { TerminalHud } from '../hud/TerminalHud'
import { VisibleHudRefreshLoop } from '../hud/visible-hud-refresh-loop'
import type { HudPermissionMode, SessionGraphNodeView, SessionHudView } from './hierarchy-types'
import { ShortcutPanel } from './ShortcutPanel'
import { TerminalSearchBar, type TerminalSearchOptions } from './TerminalSearchBar'
import { useTerminalShortcuts } from './useTerminalShortcuts'
import { DEFAULT_TERMINAL_THEME, type TerminalThemeKey } from '../terminal/terminal-themes'
import { useDagShortcut } from '../dag/useDagShortcut'
import { AgentTeamMemberSummary } from './AgentTeamMemberSummary'
import { ReadOnlyRecoveryBanner, READ_ONLY_REASON } from '../recovery/ReadOnlyRecoveryBanner'
import { StorageFaultOverlay } from './StorageFaultOverlay'
import {
  DEFAULT_TERMINAL_FONT_SIZE, MAX_TERMINAL_FONT_SIZE, MIN_TERMINAL_FONT_SIZE,
  usePersistentTerminalFontSize
} from '../terminal/usePersistentTerminalFontSize'

const STORAGE_FAULT_MUTATION_REASON = '终端存储异常，请先恢复或结束当前会话'

export function DetachedTerminalApp({ runtimeMode = 'normal' }: { runtimeMode?: RuntimeMode }) {
  const client = useRuntimeClient()
  const readOnly = runtimeMode === 'read-only'
  const query = new URLSearchParams(window.location.search)
  const sessionId = query.get('sessionId') ?? ''
  const mainWindowId = query.get('mainWindowId') ?? ''
  const sceneId = query.get('sceneId') ?? ''
  const executionContextId = query.get('executionContextId') ?? 'local-default'
  const requestedProfile = query.get('profile')
  const isTeamMember = requestedProfile === 'agent-team-member'
  const profile = requestedProfile === 'claude-code' || requestedProfile === 'codex'
    ? requestedProfile : 'shell'
  const title = query.get('title') ?? '独立终端'
  const windowId = query.get('windowId') ?? ''
  const [hud, setHud] = useState<SessionHudView>(() => ({
    sessionId,
    mode: profile === 'shell' ? 'shell' : 'agent',
    startedAt: Date.now(),
    ...(profile === 'shell' ? {} : {
      permissionMode: 'default' as const,
      modelStrategy: 'opusplan' as const
    })
  }))
  const [themeKey, setThemeKey] = useState<TerminalThemeKey>(DEFAULT_TERMINAL_THEME)
  const [fontSize, setFontSize] = usePersistentTerminalFontSize()
  const [shortcutPanelOpen, setShortcutPanelOpen] = useState(false)
  const [searchOpen, setSearchOpen] = useState(false)
  const [focusRequest, setFocusRequest] = useState(0)
  const [searchResults, setSearchResults] = useState({ resultIndex: 0, resultCount: 0 })
  const [searchRequest, setSearchRequest] = useState({
    query: '', options: { caseSensitive: false, regex: false, wholeWord: false } as TerminalSearchOptions,
    direction: 'next' as 'next' | 'previous', sequence: 0
  })
  const [teamMemberNode, setTeamMemberNode] = useState<SessionGraphNodeView>()
  const [storageFault, setStorageFault] = useState<TerminalStorageFaultMessage | null>(null)
  const sequence = useRef(0)
  useEffect(() => {
    if (!client) return
    const loadSnapshot = async () => {
      const snapshot = await client.request<{
        hierarchy?: {
          sessionHuds?: SessionHudView[]
          sessionGraphs?: Record<string, { nodes: SessionGraphNodeView[] }>
        }
      }>('projection.snapshot', {
        windowId: query.get('windowId') ?? 'detached-window'
      })
      const current = snapshot.hierarchy?.sessionHuds?.find((item) => item.sessionId === sessionId)
      if (current) setHud(current)
      if (isTeamMember) {
        const node = snapshot.hierarchy?.sessionGraphs?.[sceneId]?.nodes
          .find((item) => item.sessionId === sessionId)
        if (node) setTeamMemberNode(node)
      }
    }
    const apply = (message: RuntimeMessage) => {
      if (message.type === 'terminal.hud' && message.sessionId === sessionId && message.hud) {
        setHud(message.hud)
      }
      if (isTeamMember && message.type === 'events.batch') void loadSnapshot().catch(() => {})
    }
    const unsubscribe = client.subscribeProjection(apply)
    void loadSnapshot().catch(() => {})
    return unsubscribe
  }, [client, isTeamMember, sceneId, sessionId])
  useEffect(() => {
    if (!client || !sessionId || isTeamMember || typeof client.refreshTerminalHud !== 'function') return
    const loop = new VisibleHudRefreshLoop(
      () => client.refreshTerminalHud(sessionId),
      () => document.visibilityState === 'visible'
    )
    const onFocus = () => loop.focus()
    const onVisibility = () => loop.visibilityChanged()
    loop.start()
    window.addEventListener('focus', onFocus)
    document.addEventListener('visibilitychange', onVisibility)
    return () => {
      loop.stop()
      window.removeEventListener('focus', onFocus)
      document.removeEventListener('visibilitychange', onVisibility)
    }
  }, [client, isTeamMember, sessionId])
  const setPermissionMode = (targetSessionId: string, permissionMode: HudPermissionMode, respawn: boolean) => {
    if (!client || readOnly || storageFault !== null) return
    const method = 'session.set-permission-mode'
    const input = {
      sessionId: targetSessionId, provider: 'claude-code', permissionMode, respawn
    }
    const commandId = `${method}-${Date.now()}-${++sequence.current}`
    return client.request(method, {
      command: { commandId, commandType: method, requestHash: JSON.stringify(input) },
      input: { ...input, now: Date.now() }
    })
  }
  const search = (query: string, options: TerminalSearchOptions, direction: 'next' | 'previous' = 'next') => {
    setSearchRequest((current) => ({ query, options, direction, sequence: current.sequence + 1 }))
  }
  const shortcutHandlers = useMemo(() => ({
    closePane: () => window.close(),
    openSearch: () => setSearchOpen(true),
    increaseFontSize: () => setFontSize((value) => Math.min(MAX_TERMINAL_FONT_SIZE, value + 1)),
    decreaseFontSize: () => setFontSize((value) => Math.max(MIN_TERMINAL_FONT_SIZE, value - 1)),
    resetFontSize: () => setFontSize(DEFAULT_TERMINAL_FONT_SIZE),
    cycleTheme: () => {
      setThemeKey((value) => value === 'light' ? 'dark' : 'light')
      setFocusRequest((value) => value + 1)
    },
    toggleShortcutPanel: () => {
      if (shortcutPanelOpen) setFocusRequest((value) => value + 1)
      setShortcutPanelOpen(!shortcutPanelOpen)
    }
  }), [shortcutPanelOpen])
  const isMac = useTerminalShortcuts(shortcutHandlers)
  useDagShortcut({
    enabled: Boolean(mainWindowId && sceneId && sessionId),
    onPress: () => {
      void window.matouDesktop?.openDagWindow?.({
        mainWindowId, sceneId, sessionId,
        theme: themeKey === 'dark' ? 'dark' : 'light'
      })
    }
  })
  useEffect(() => window.matouDesktop?.onDagShortcut?.(() => {
    void window.matouDesktop?.openDagWindow?.({
      mainWindowId, sceneId, sessionId,
      theme: themeKey === 'dark' ? 'dark' : 'light'
    })
  }), [mainWindowId, sceneId, sessionId, themeKey])
  useEffect(() => {
    if (isTeamMember || !windowId || !sessionId) return
    const desktop = window.matouDesktop
    if (
      !desktop?.onDetachedTerminalFocusRequested ||
      !desktop.acknowledgeDetachedTerminalFocus ||
      !desktop.isCurrentWindowFocused
    ) return
    let alive = true
    let generation = 0
    const unsubscribe = desktop.onDetachedTerminalFocusRequested((request) => {
      if (
        request.targetWindowId !== windowId || request.routeWindowId !== mainWindowId ||
        request.sessionId !== sessionId
      ) return
      const currentGeneration = ++generation
      setSearchOpen(false)
      setShortcutPanelOpen(false)
      setFocusRequest((value) => value + 1)
      void proveDetachedTerminalFocus({
        request,
        sessionId,
        active: () => alive && generation === currentGeneration,
        isNativeWindowFocused: () => desktop.isCurrentWindowFocused()
      }).then(async (focused) => {
        if (!alive || generation !== currentGeneration) return
        await desktop.acknowledgeDetachedTerminalFocus({ ...request, focused })
      })
    })
    return () => {
      alive = false
      generation += 1
      unsubscribe()
    }
  }, [isTeamMember, mainWindowId, sessionId, windowId])
  useEffect(() => {
    document.body.classList.toggle('light-theme', themeKey === 'light')
    document.documentElement.dataset.theme = themeKey
    return () => {
      document.body.classList.remove('light-theme')
      delete document.documentElement.dataset.theme
    }
  }, [themeKey])
  return <main className="detached-terminal-app" data-theme={themeKey}>
    <header><strong>{title}</strong><span>{isTeamMember
      ? '独立窗口 · 队友摘要'
      : readOnly ? '独立窗口 · 只读历史' : '独立窗口 · 会话保持运行'}</span></header>
    {readOnly && <ReadOnlyRecoveryBanner onSearch={() => setSearchOpen(true)} exportBundle={() =>
      window.matouDesktop.exportDatabaseRecoveryBundle()} />}
    {!isTeamMember && <TerminalSearchBar open={searchOpen} themeKey={themeKey}
      resultIndex={searchResults.resultIndex} resultCount={searchResults.resultCount}
      onSearch={(query, options) => search(query, options)}
      onNext={() => search(searchRequest.query, searchRequest.options, 'next')}
      onPrevious={() => search(searchRequest.query, searchRequest.options, 'previous')}
      onClose={() => {
        search('', searchRequest.options)
        setSearchOpen(false)
        setFocusRequest((value) => value + 1)
      }} />}
    {isTeamMember ? <AgentTeamMemberSummary
      workStatus={teamMemberNode?.workStatus ?? 'starting'}
      latestLines={teamMemberNode?.latestLines ?? []} /> : <TerminalSurface sessionId={sessionId} executionContextId={executionContextId}
      profile={profile} visible readOnly={readOnly} inputDisabled={readOnly || storageFault !== null}
      themeKey={themeKey} fontSize={fontSize} onFontSizeChange={setFontSize}
      {...(searchOpen ? { searchRequest } : {})} onSearchResults={setSearchResults}
      focusRequest={focusRequest}
      onStorageFault={setStorageFault}
      onStorageRecovered={() => setStorageFault(null)} />}
    {storageFault && <StorageFaultOverlay sessionTitle={title}
      fault={{ code: storageFault.code, retainedBytes: storageFault.retainedBytes }}
      onRetry={() => client?.retryTerminalStorage(sessionId)}
      onEnd={() => client?.endTerminalAfterStorageFault(sessionId)} />}
    {!isTeamMember && <div className="shortcut-bar" aria-label="快捷指令栏">
      <TerminalHud hud={hud} onPermissionMode={setPermissionMode}
        {...(readOnly || storageFault ? {
          disabledReason: readOnly ? READ_ONLY_REASON : STORAGE_FAULT_MUTATION_REASON
        } : {})} />
    </div>}
    {!isTeamMember && <ShortcutPanel open={shortcutPanelOpen} isMac={isMac} themeKey={themeKey}
      onClose={() => {
        setShortcutPanelOpen(false)
        setFocusRequest((value) => value + 1)
      }} />}
  </main>
}

async function proveDetachedTerminalFocus(input: {
  request: { deadlineAt: number }
  sessionId: string
  active(): boolean
  isNativeWindowFocused(): Promise<boolean>
}): Promise<boolean> {
  while (input.active() && Date.now() < input.request.deadlineAt) {
    await nextDetachedFocusFrame()
    if (!input.active()) return false
    const nativeFocused = await input.isNativeWindowFocused().catch(() => false)
    if (nativeFocused && detachedTerminalOwnsInputFocus(input.sessionId)) return true
  }
  return false
}

function detachedTerminalOwnsInputFocus(sessionId: string): boolean {
  const active = document.activeElement
  if (!(active instanceof HTMLElement) || active === document.body) return false
  return [...document.querySelectorAll<HTMLElement>('.terminal-surface[data-session-id]')]
    .some((surface) => surface.dataset.sessionId === sessionId &&
      surface.closest('[hidden]') === null && (surface === active || surface.contains(active)))
}

function nextDetachedFocusFrame(): Promise<void> {
  return new Promise((resolve) => {
    let settled = false
    let frame: number | undefined
    const timer = window.setTimeout(finish, 40)
    function finish() {
      if (settled) return
      settled = true
      window.clearTimeout(timer)
      if (frame !== undefined) cancelAnimationFrame(frame)
      resolve()
    }
    frame = requestAnimationFrame(finish)
  })
}
