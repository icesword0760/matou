import { useEffect, useRef, useState } from 'react'

import type { RuntimeMessage } from '@matou/contracts'

import { TerminalSurface } from '../terminal/TerminalSurface'
import { useRuntimeClient } from '../runtime/RuntimeProvider'
import { TerminalHud } from '../hud/TerminalHud'
import type { HudModelStrategy, HudPermissionMode, SessionHudView } from './hierarchy-types'

export function DetachedTerminalApp() {
  const client = useRuntimeClient()
  const query = new URLSearchParams(window.location.search)
  const sessionId = query.get('sessionId') ?? ''
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
  return <main className="detached-terminal-app">
    <header><strong>{title}</strong><span>独立窗口 · 会话保持运行</span></header>
    <TerminalSurface sessionId={sessionId} executionContextId={executionContextId}
      profile={profile} visible />
    <div className="shortcut-bar" aria-label="快捷指令栏">
      <button className="add-btn" aria-label="添加快捷指令">+</button><div className="btn-list" />
      <TerminalHud hud={hud} onPermissionMode={(_sessionId: string, permissionMode: HudPermissionMode, respawn: boolean) =>
        command('session.set-permission-mode', {
          sessionId, provider: 'claude-code', permissionMode, respawn
        })}
        onModel={(_sessionId: string, modelStrategy: HudModelStrategy) =>
          command('session.set-model', { sessionId, modelStrategy })} />
    </div>
  </main>
}
