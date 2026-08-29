import { useEffect, useMemo, useRef, useState } from 'react'

import type { RuntimeMessage } from '@matou/contracts'

import { TerminalSurface } from '../terminal/TerminalSurface'
import { useRuntimeClient } from '../runtime/RuntimeProvider'
import { TerminalHud } from '../hud/TerminalHud'
import type { HudModelStrategy, HudPermissionMode, SessionHudView } from './hierarchy-types'
import { ShortcutPanel } from './ShortcutPanel'
import { TerminalSearchBar, type TerminalSearchOptions } from './TerminalSearchBar'
import { useTerminalShortcuts } from './useTerminalShortcuts'
import { DEFAULT_TERMINAL_THEME, type TerminalThemeKey } from '../terminal/terminal-themes'
import { useDagShortcut } from '../dag/useDagShortcut'

export function DetachedTerminalApp() {
  const client = useRuntimeClient()
  const query = new URLSearchParams(window.location.search)
  const sessionId = query.get('sessionId') ?? ''
  const mainWindowId = query.get('mainWindowId') ?? ''
  const sceneId = query.get('sceneId') ?? ''
  const executionContextId = query.get('executionContextId') ?? 'local-default'
  const requestedProfile = query.get('profile')
  const profile = requestedProfile === 'claude-code' || requestedProfile === 'codex'
    ? requestedProfile : 'shell'
  const title = query.get('title') ?? '独立终端'
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
  const [fontSize, setFontSize] = useState(11)
  const [shortcutPanelOpen, setShortcutPanelOpen] = useState(false)
  const [searchOpen, setSearchOpen] = useState(false)
  const [focusRequest, setFocusRequest] = useState(0)
  const [searchResults, setSearchResults] = useState({ resultIndex: 0, resultCount: 0 })
  const [searchRequest, setSearchRequest] = useState({
    query: '', options: { caseSensitive: false, regex: false, wholeWord: false } as TerminalSearchOptions,
    direction: 'next' as 'next' | 'previous', sequence: 0
  })
  const sequence = useRef(0)
  useEffect(() => {
    if (!client) return
    const apply = (message: RuntimeMessage) => {
      if (message.type === 'terminal.hud' && message.sessionId === sessionId && message.hud) {
        setHud(message.hud)
      }
    }
    const unsubscribe = client.subscribeProjection(apply)
    void client.request<{ hierarchy?: { sessionHuds?: SessionHudView[] } }>('projection.snapshot', {
      windowId: query.get('windowId') ?? 'detached-window'
    }).then((snapshot) => {
      const current = snapshot.hierarchy?.sessionHuds?.find((item) => item.sessionId === sessionId)
      if (current) setHud(current)
    }).catch(() => {})
    return unsubscribe
  }, [client, sessionId])
  const command = (method: 'session.set-permission-mode' | 'session.set-model', input: Record<string, unknown>) => {
    if (!client) return
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
    increaseFontSize: () => setFontSize((value) => Math.min(24, value + 1)),
    decreaseFontSize: () => setFontSize((value) => Math.max(10, value - 1)),
    resetFontSize: () => setFontSize(11),
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
    onShortPress: () => window.dispatchEvent(new Event('matou:forward-terminal-tab')),
    onLongPress: () => {
      void window.matouDesktop?.openDagWindow?.({
        mainWindowId, sceneId, sessionId,
        theme: themeKey === 'dark' ? 'dark' : 'light'
      })
    }
  })
  useEffect(() => {
    document.body.classList.toggle('light-theme', themeKey === 'light')
    document.documentElement.dataset.theme = themeKey
    return () => {
      document.body.classList.remove('light-theme')
      delete document.documentElement.dataset.theme
    }
  }, [themeKey])
  return <main className="detached-terminal-app" data-theme={themeKey}>
    <header><strong>{title}</strong><span>独立窗口 · 会话保持运行</span></header>
    <TerminalSearchBar open={searchOpen} themeKey={themeKey}
      resultIndex={searchResults.resultIndex} resultCount={searchResults.resultCount}
      onSearch={(query, options) => search(query, options)}
      onNext={() => search(searchRequest.query, searchRequest.options, 'next')}
      onPrevious={() => search(searchRequest.query, searchRequest.options, 'previous')}
      onClose={() => {
        search('', searchRequest.options)
        setSearchOpen(false)
        setFocusRequest((value) => value + 1)
      }} />
    <TerminalSurface sessionId={sessionId} executionContextId={executionContextId}
      profile={profile} visible themeKey={themeKey} fontSize={fontSize} onFontSizeChange={setFontSize}
      {...(searchOpen ? { searchRequest } : {})} onSearchResults={setSearchResults}
      focusRequest={focusRequest} />
    <div className="shortcut-bar" aria-label="快捷指令栏">
      <button className="add-btn" aria-label="添加快捷指令">+</button><div className="btn-list" />
      <TerminalHud hud={hud} onPermissionMode={(_sessionId: string, permissionMode: HudPermissionMode, respawn: boolean) =>
        command('session.set-permission-mode', {
          sessionId, provider: 'claude-code', permissionMode, respawn
        })}
        onModel={(_sessionId: string, modelStrategy: HudModelStrategy) =>
          command('session.set-model', { sessionId, modelStrategy })} />
    </div>
    <ShortcutPanel open={shortcutPanelOpen} isMac={isMac} themeKey={themeKey}
      onClose={() => {
        setShortcutPanelOpen(false)
        setFocusRequest((value) => value + 1)
      }} />
  </main>
}
