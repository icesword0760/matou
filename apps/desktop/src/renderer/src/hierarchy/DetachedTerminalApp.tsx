import { TerminalSurface } from '../terminal/TerminalSurface'

export function DetachedTerminalApp() {
  const query = new URLSearchParams(window.location.search)
  const sessionId = query.get('sessionId') ?? ''
  const executionContextId = query.get('executionContextId') ?? 'local-default'
  const requestedProfile = query.get('profile')
  const profile = requestedProfile === 'claude-code' || requestedProfile === 'codex'
    ? requestedProfile : 'shell'
  const title = query.get('title') ?? '独立终端'
  return <main className="detached-terminal-app">
    <header><strong>{title}</strong><span>独立窗口 · 会话保持运行</span></header>
    <TerminalSurface sessionId={sessionId} executionContextId={executionContextId}
      profile={profile} visible />
  </main>
}
