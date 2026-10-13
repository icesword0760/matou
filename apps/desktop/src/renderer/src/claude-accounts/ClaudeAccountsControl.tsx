import { useEffect, useId, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import type { AccountError, ClaudeAccountCommand, ClaudeAccountsState, ClaudeAccountView } from '../../../shared/claude-accounts'
import { useLocale } from '../i18n/LocaleProvider'
import './claude-accounts.css'

const EMPTY: ClaudeAccountsState = { revision: 0, accounts: [], sessions: [], pendingSessionIds: [], operation: 'idle', rememberScope: false }
type Page = 'list' | 'add' | 'detected' | 'draft' | 'saved' | 'scope' | 'manage' | 'edit' | 'remove' | 'resume'
const ERRORS: Record<AccountError, [string, string]> = {
  network: ['网络暂时没有响应，请稍后重试。', 'The network did not respond. Try again shortly.'],
  'login-required': ['登录已失效，请重新登录这个账户。', 'Sign in to this account again.'],
  'login-timeout': ['登录等待已结束。你可以重新打开登录流程。', 'Sign-in timed out. Start again when ready.'],
  'login-failed': ['登录尚未完成，请重试并在浏览器中完成授权。', 'Sign-in did not finish. Try again and complete it in your browser.'],
  storage: ['账户偏好保存失败，请检查磁盘空间后重试。', 'Preferences were not saved. Check disk space and retry.'],
  keychain: ['请解锁系统钥匙串，并允许账户组件访问后重试。', 'Unlock the system keychain and allow access, then retry.'],
  'cli-missing': ['请先安装 Claude Code，再添加账户。', 'Install Claude Code before adding an account.'],
  busy: ['任务状态有变化。请等任务结束，或先在终端中取消任务。', 'Task activity changed. Wait for completion or stop the task in its terminal.'],
  runtime: ['正在连接会话服务，账户保持原状。请稍后重试。', 'Connecting to the session service. Accounts are unchanged. Retry shortly.'],
  identity: ['登录的账户与选择的账户不一致，请核对后重新登录。', 'The signed-in identity differs from the selected account. Check it and sign in again.'],
  invalid: ['请检查名称和所选账户。', 'Check the name and selected account.'],
  conflict: ['账户状态已在别处更新，请刷新后重新选择。', 'Accounts changed elsewhere. Refresh and select again.'],
  unsupported: ['当前应用使用自定义 Claude 配置。请使用默认配置启动 Matou 后切换本机账户。', 'This app uses a custom Claude profile. Launch Matou with the default profile to switch the device account.'],
  'recovery-required': ['上次操作需要检查。请刷新账户状态后继续。', 'The previous operation needs checking. Refresh before continuing.'],
  'resume-failed': ['对话续接尚未完成。默认账户已更新，可检查终端后重试。', 'The default account changed, but resuming did not finish. Check the terminal and retry.'],
  'engine-missing': ['账户切换组件尚未就绪。开发版请运行 pnpm accounts:setup；正式版请重新安装完整应用。', 'The account component is missing. In development, run pnpm accounts:setup; otherwise reinstall the full app.'],
  'engine-version': ['账户组件版本不匹配，请更新完整应用。', 'The account engine version does not match. Update the full app.'],
  'engine-error': ['claude-swap 未完成本次操作。请刷新状态；若仍提示登录失效，请重新登录。', 'claude-swap did not finish the operation. Refresh; sign in again if the account requires it.'],
  'switch-failed': ['默认账户尚未确认切换成功，请刷新后重试。', 'The default-account change is not confirmed. Refresh and retry.']
}

export function ClaudeAccountsControl({ sessionId }: { sessionId: string }) {
  const locale = useLocale(), zh = locale === 'zh-CN'
  const t = (cn: string, en: string) => zh ? cn : en
  const bridge = window.matouDesktop
  const [state, setState] = useState(EMPTY)
  const [open, setOpen] = useState(false)
  const [checking, setChecking] = useState(false)
  const [page, setPage] = useState<Page>('list')
  const [selectedId, setSelectedId] = useState('')
  const [name, setName] = useState('')
  const [remember, setRemember] = useState(false)
  const [localError, setLocalError] = useState('')
  const [submitting, setSubmitting] = useState(false)
  const trigger = useRef<HTMLButtonElement>(null), panel = useRef<HTMLDivElement>(null)
  const title = useRef<HTMLHeadingElement>(null), latestRevision = useRef(-1)
  const titleId = useId(), nameId = useId()
  const apply = (next: ClaudeAccountsState) => {
    if (next.revision < latestRevision.current) return
    latestRevision.current = next.revision; setState(next)
  }
  const close = (restore = true) => { setOpen(false); if (restore) trigger.current?.focus() }
  useEffect(() => {
    if (!bridge?.getClaudeAccounts || !bridge.onClaudeAccounts) return
    let active = true
    void bridge.getClaudeAccounts().then(v => { if (active) apply(v) }).catch(() => {})
    const off = bridge.onClaudeAccounts(apply)
    return () => { active = false; off() }
  }, [bridge])
  useEffect(() => {
    if (!open) return
    title.current?.focus({ preventScroll: true })
    const click = (event: PointerEvent) => {
      if (event.target instanceof Node && !panel.current?.contains(event.target) && !trigger.current?.contains(event.target)) close(false)
    }
    const key = (event: KeyboardEvent) => {
      if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); close() }
    }
    document.addEventListener('pointerdown', click, true); document.addEventListener('keydown', key, true)
    return () => { document.removeEventListener('pointerdown', click, true); document.removeEventListener('keydown', key, true) }
  }, [open, page])
  useEffect(() => {
    if (state.draft) { setName(state.draft.name); setSelectedId(state.draft.id); setPage('draft') }
  }, [state.draft?.id])
  const call = async (command: ClaudeAccountCommand) => {
    if (!bridge?.claudeAccountCommand) return undefined
    setLocalError(''); setSubmitting(true)
    try { const next = await bridge.claudeAccountCommand(command); apply(next); return next }
    catch { setLocalError(t(...ERRORS.runtime)); return undefined }
    finally { setSubmitting(false) }
  }
  if (!bridge?.getClaudeAccounts) return null
  const current = state.accounts.find(a => a.id === state.currentId)
  const selected = state.accounts.find(a => a.id === selectedId)
  const activeSession = state.sessions.find(s => s.sessionId === sessionId)
  const pending = state.pendingSessionIds.includes(sessionId)
  const busy = state.operation !== 'idle' || submitting
  const running = state.sessions.filter(s => s.busy).length
  const error = localError || (state.error ? t(...ERRORS[state.error]) : '')
  const startLogin = (replaceId?: string) => {
    setPage('add')
    void call({ type: 'login', ...(replaceId ? { replaceId } : {}) })
  }
  const beginSwitch = (account: ClaudeAccountView) => {
    setSelectedId(account.id); setRemember(state.rememberScope)
    if (account.needsLogin) { startLogin(account.id); return }
    if (state.rememberScope && !running) { setPage('list'); void call({ type: 'switch', id: account.id, rememberScope: true, waitForIdle: false }) }
    else setPage('scope')
  }
  const displayPage = state.operation === 'login' ? 'add' : page
  const heading = displayPage === 'manage' ? t('管理账户', 'Manage accounts') : displayPage === 'edit' ? t('编辑账户', 'Edit account') : displayPage === 'scope' ? t('切换本机默认账户', 'Switch device default') : displayPage === 'resume' ? t('续接当前对话', 'Resume this conversation') : t('Claude 账户', 'Claude accounts')
  const back = () => { setLocalError(''); setPage('list') }
  const date = (stamp: string | number) => {
    if (!stamp || !Number.isFinite(new Date(stamp).getTime())) return '—'
    return new Intl.DateTimeFormat(locale, { month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit' }).format(new Date(stamp))
  }
  const quota = (account: ClaudeAccountView) => <>
    {account.needsLogin ? <span className="ca-expired">{t('登录已失效 · 点击重新登录', 'Sign-in expired · Sign in again')}</span> : <div className="ca-quotas">{(['fiveHour', 'weekly'] as const).map((key, i) => {
      const value = account.usage?.[key]
      return <div className="ca-quota" key={key}><div><span>{i === 0 ? t('5 小时已用', '5-hour used') : t('每周已用', 'Weekly used')}</span><b className={value && value.used >= 90 ? 'ca-warning' : ''}>{value ? `${Math.round(value.used)}%` : '—'}</b></div>
        <div className="ca-track"><span className={value && value.used >= 90 ? 'is-high' : ''} style={{ width: `${value?.used ?? 0}%` }} /></div>
        <small>{value ? `${date(value.resetsAt)} ${t('重置', 'reset')}` : t('尚未获取用量', 'Usage not loaded')}</small></div>
    })}</div>}
    {account.usage && <span className="ca-measured">{account.usage.stale ? t('上次数据', 'Last known') : t('更新于', 'Updated')} {date(account.usage.checkedAt)}</span>}
  </>
  return <>
    <button ref={trigger} type="button" className={`status-field ca-trigger ${open ? 'is-open' : ''}`} aria-expanded={open} aria-haspopup="dialog"
      aria-label={t('Claude 账户', 'Claude accounts')} title={t('切换本机默认 Claude 账户', 'Switch the device-default Claude account')}
      onClick={() => {
        if (open) { close(); return }
        setOpen(true)
        if (state.operation === 'idle' && !state.draft) { setChecking(true); void call({ type: 'inspect' }).finally(() => setChecking(false)) }
      }}><svg aria-hidden="true" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5"><circle cx="12" cy="8" r="3.5"/><path d="M5 21v-3a7 7 0 0 1 14 0v3"/></svg><span className="ca-trigger-label">{current ? `${t('默认：', 'Default: ')}${current.name}` : t('Claude 账户', 'Claude accounts')}</span>{state.queuedId || pending ? <i className="ca-dot" /> : null}<span aria-hidden="true">⌃</span></button>
    {open && createPortal(<div className="ca-panel" ref={panel} role="dialog" aria-labelledby={titleId} data-testid="claude-accounts-panel">
      <header className="ca-header">{displayPage !== 'list' && !busy && <button type="button" className="ca-icon" onClick={back} aria-label={t('返回账户列表', 'Back to accounts')}>‹</button>}<h2 id={titleId} ref={title} tabIndex={-1}>{heading}</h2>{displayPage === 'list' && <span className="ca-badge">{t('本机默认', 'Device default')}</span>}<button type="button" className="ca-icon ca-close" onClick={() => close()} aria-label={t('关闭账户面板', 'Close accounts')}>×</button></header>
      <div className="ca-scroll">
        {error && <div className="ca-notice ca-error" role="alert"><strong>{t('操作尚未完成', 'Action not completed')}</strong><p>{error}</p><button className="ca-link" type="button" disabled={busy} onClick={() => { void call({ type: 'inspect' }) }}>{t('刷新状态', 'Refresh status')}</button></div>}
        {(state.operation === 'switching' || state.operation === 'resuming') && <div className="ca-notice" role="status"><span className="ca-spinner" />{state.operation === 'switching' ? t('正在切换，请稍候…', 'Switching, please wait…') : t('正在重启并续接对话…', 'Restarting and resuming…')}</div>}
        {displayPage === 'list' && <>
          {state.queuedId && <div className="ca-notice" role="status"><strong>{t('已安排任务结束后切换', 'Switch scheduled after tasks finish')}</strong><p>{t('可以继续查看工作。退出 Matou 会取消这次安排。', 'Keep reviewing your work. Quitting Matou cancels this schedule.')}</p><button className="ca-link" disabled={state.operation === 'switching'} onClick={() => { void call({ type: 'cancel-switch' }) }}>{t('取消安排', 'Cancel schedule')}</button></div>}
          {state.switchedAt && <div className="ca-notice ca-success" role="status"><strong>{t('本机默认账户已更新', 'Device default updated')}</strong><p>{pending ? t('当前对话仍可能使用原账户。准备好后，可重启 Claude 并续接已保存的对话。', 'This conversation may still use the previous account. Restart Claude to resume the saved conversation.') : t('新启动的默认登录会话会读取该账户。运行中的进程以各自实际状态为准。', 'New default-login sessions read this account. Running processes may retain their previous login.')}</p>{pending && activeSession?.resumable && activeSession.defaultLogin && <button className="ca-link" disabled={busy || activeSession.busy} onClick={() => setPage('resume')}>{activeSession.busy ? t('任务结束后可续接', 'Resume after the task finishes') : t('重启并续接当前对话', 'Restart and resume this conversation')}</button>}</div>}
          {state.detected && <div className="ca-notice"><strong>{t('发现本机已有 Claude 登录', 'Existing Claude sign-in found')}</strong><p>{state.detected.email}<br />{t('保存后就能随时切回来，无需重新登录。', 'Save it to switch back later without signing in again.')}</p><button className="ca-primary" disabled={busy} onClick={() => { setName(state.detected!.name); setPage('detected') }}>{t('保存当前账户', 'Save current account')}</button></div>}
          {checking && !state.accounts.length && !state.detected && <div className="ca-welcome" role="status"><span className="ca-spinner" />{t('正在读取本机账户…', 'Reading device accounts…')}</div>}
          {!checking && !state.accounts.length && !state.detected && <div className="ca-welcome"><span className="ca-welcome-icon" aria-hidden="true">⇄</span><h3>{t('多个账户，随手切换', 'Your accounts, one click away')}</h3><p>{t('每个账户只需登录一次。以后在这里查看用量、选择账户，不打断你的工作节奏。', 'Sign in once per account. Check usage and choose an account here when you need it.')}</p><ol><li>{t('在官方页面登录 Claude', 'Sign in on the official Claude page')}</li><li>{t('保存账户，按需设为默认', 'Save it, then choose when to switch')}</li></ol><button className="ca-primary ca-wide" disabled={busy} onClick={() => setPage('add')}>{t('添加第一个账户', 'Add your first account')}</button></div>}
          {state.accounts.map((a, i) => <button type="button" key={a.id} className={`ca-account ${state.currentId === a.id ? 'is-current' : ''}`} disabled={busy || state.currentId === a.id && !a.needsLogin} onClick={() => beginSwitch(a)}>
            <span className={`ca-avatar tone-${i % 3}`}>{a.name.slice(0, 1)}</span><div className="ca-account-main"><div className="ca-account-title"><strong>{a.name}</strong><span>{state.currentId === a.id ? t('✓ 本机默认', '✓ Default') : '›'}</span></div><div className="ca-email">{a.email}</div>{quota(a)}</div></button>)}
          {state.accounts.length === 1 && <p className="ca-helper">{t('再添加一个账户，需要时就能在这里切换。', 'Add another account to switch whenever you need.')}</p>}
          {state.accounts.length > 0 && <div className="ca-refresh"><span>{t('用量由 claude-swap 获取', 'Usage from claude-swap')}</span><button className="ca-link" disabled={busy} onClick={() => { void call({ type: 'refresh-usage' }) }}>{busy ? t('更新中…', 'Updating…') : t('刷新用量', 'Refresh usage')}</button></div>}
        </>}
        {displayPage === 'add' && <div className="ca-form ca-center"><div className="ca-browser" aria-hidden="true">↗{state.operation === 'login' && <span className="ca-spinner" />}</div><h3>{state.operation === 'login' ? t('在浏览器中完成登录', 'Finish signing in in your browser') : t('添加 Claude 账户', 'Add a Claude account')}</h3><p>{t('密码和验证码仅在 Claude 官方页面输入。添加账户只保存登录，不切换当前默认账户。', 'Enter your password and verification code only on the official Claude page. Adding an account saves it without changing your current default.')}</p><div className="ca-steps"><span>1 {t('浏览器登录', 'Browser sign-in')}</span><span>2 {t('核对并保存', 'Review and save')}</span></div>{state.operation === 'login' ? <><p role="status">{t('正在等待授权，完成后会自动回到下一步。', 'Waiting for authorization. The next step will appear automatically.')}</p><button className="ca-secondary" onClick={() => { void call({ type: 'cancel-login' }) }}>{t('取消本次登录', 'Cancel sign-in')}</button></> : <button className="ca-primary ca-wide" disabled={busy} onClick={() => startLogin()}>{t('打开浏览器登录', 'Open browser to sign in')}</button>}</div>}
        {(displayPage === 'detected' || displayPage === 'draft' || displayPage === 'edit') && <form className="ca-form" onSubmit={event => {
          event.preventDefault()
          if (!name.trim() || [...name.trim()].length > 12) { setLocalError(t('请输入 1–12 个字符的账户名称。', 'Enter a name with 1–12 characters.')); return }
          const command: ClaudeAccountCommand = displayPage === 'edit' ? { type: 'rename', id: selectedId, name } : { type: displayPage === 'draft' ? 'save-draft' : 'save-detected', name }
          const savedId = displayPage === 'draft' ? state.draft?.id : state.detected?.id
          void call(command).then(next => { if (next && !next.error) { if (savedId) setSelectedId(savedId); setPage(displayPage === 'edit' ? 'manage' : 'saved') } })
        }}><h3>{displayPage === 'edit' ? selected?.name : t('确认账户，起个好记的名字', 'Review your account and give it a name')}</h3><p className="ca-identity">{displayPage === 'edit' ? selected?.email : displayPage === 'draft' ? state.draft?.email : state.detected?.email}</p>{displayPage === 'draft' && state.accounts.some(a => a.id === state.draft?.id) && <div className="ca-notice">{t('这个账户已经添加过。本次会更新原有登录，不创建重复条目。', 'This account already exists. Saving updates its sign-in without creating a duplicate.')}</div>}<label htmlFor={nameId}>{t('账户名称', 'Account name')}<small>{[...name].length}/12</small></label><input id={nameId} value={name} maxLength={24} onChange={e => setName(e.target.value)} placeholder={t('例如：工作账户', 'For example: Work')} autoComplete="off" /><p className="ca-helper">{t('名称只在 Matou 中显示，方便你区分账户。', 'This name is only shown in Matou to help you recognize the account.')}</p><button className="ca-primary ca-wide" type="submit" disabled={busy}>{t('保存账户', 'Save account')}</button>{displayPage === 'draft' && <button className="ca-link ca-wide" type="button" disabled={busy} onClick={() => { void call({ type: 'cancel-login' }).then(back) }}>{t('放弃本次添加', 'Discard this sign-in')}</button>}{displayPage === 'edit' && <div className="ca-edit-actions"><button className="ca-link" type="button" disabled={busy} onClick={() => startLogin(selectedId)}>{t('重新登录', 'Sign in again')}</button><button className="ca-link ca-danger" type="button" disabled={busy} onClick={() => setPage('remove')}>{t('移除账户…', 'Remove account…')}</button></div>}</form>}
        {displayPage === 'saved' && <div className="ca-form ca-center"><span className="ca-success-icon">✓</span><h3>{t('账户已保存', 'Account saved')}</h3><p>{t('当前默认登录保持原状。你可以现在切换，也可以等需要时再使用。', 'Your default login is unchanged. Switch now or use this account later.')}</p>{selected && selected.id !== state.currentId && <button className="ca-primary ca-wide" disabled={busy} onClick={() => beginSwitch(selected)}>{t('现在切换到这个账户', 'Switch to this account now')}</button>}<button className="ca-secondary ca-wide" onClick={back}>{t('返回账户列表', 'Back to accounts')}</button></div>}
        {displayPage === 'scope' && selected && <div className="ca-form"><h3>{t('切换到', 'Switch to')}「{selected.name}」</h3><p>{t('这是本机默认登录，不是只切换当前卡片。', 'This changes the device default, not only the current card.')}</p><div className="ca-notice"><strong>{t('共享默认登录的 Claude 实例都会受影响', 'Shared default-login instances are affected')}</strong><p>{t('包括 Matou 内的默认登录会话，以及应用外的终端和编辑器。API Key、自定义服务不在此范围。', 'This includes Matou default-login sessions and external terminals or editors. API keys and custom providers are separate.')}</p></div>{running > 0 && <div className="ca-notice ca-amber"><strong>{t('任务仍在运行', 'Tasks are still running')}</strong><p>{t('等 Matou 内任务结束后再切换；安排可随时取消。', 'Switch after Matou tasks finish. You can cancel the schedule.')}</p></div>}<p>{t('应用外任务请自行确认。已有对话是否生效，会在切换后单独提示。', 'Check external tasks yourself. Existing conversations may need a separate restart after switching.')}</p><label className="ca-check"><input type="checkbox" checked={remember} onChange={e => setRemember(e.target.checked)} /><span>{t('已了解范围，下次空闲时直接切换', 'Remember this scope and switch directly when idle')}</span></label><div className="ca-button-row"><button className="ca-secondary" onClick={back} disabled={busy}>{t('暂不切换', 'Not now')}</button><button className="ca-primary" disabled={busy} onClick={() => { void call({ type: 'switch', id: selected.id, rememberScope: remember, waitForIdle: true }).then(next => { if (next && !next.error) back() }) }}>{running ? t('任务结束后切换', 'Switch when idle') : t('确认切换', 'Confirm switch')}</button></div></div>}
        {displayPage === 'manage' && <div className="ca-form"><p>{t('账户登录由 claude-swap 管理。这里的名称只影响 Matou 显示。', 'Sign-ins are managed by claude-swap. Names here only affect Matou.')}</p>{state.accounts.map((a, i) => <div className="ca-manage-row" key={a.id}><span className={`ca-avatar tone-${i % 3}`}>{a.name.slice(0, 1)}</span><div><strong>{a.name}</strong><small>{a.email}{a.id === state.currentId ? ` · ${t('本机默认', 'Default')}` : ''}</small></div><button className="ca-link" disabled={busy} onClick={() => { setSelectedId(a.id); setName(a.name); setPage('edit') }}>{t('编辑', 'Edit')}</button></div>)}<p className="ca-helper">{t('移除会删除 claude-swap 保存的账户副本，不执行 Claude 退出登录。', 'Removing deletes the copy saved by claude-swap; it does not sign Claude out.')}</p><button className="ca-secondary ca-wide" disabled={busy} onClick={() => setPage('add')}>＋ {t('添加账户', 'Add account')}</button></div>}
        {displayPage === 'remove' && <div className="ca-form"><h3>{t('移除', 'Remove')}「{selected?.name}」？</h3><p>{t('将删除 claude-swap 中保存的登录。下次添加需要重新登录；本机当前登录与项目文件保留。', 'This deletes the saved sign-in from claude-swap. You will need to sign in to add it again. The current device login and project files remain.')}</p><div className="ca-button-row"><button className="ca-secondary" disabled={busy} onClick={() => setPage('edit')}>{t('保留账户', 'Keep account')}</button><button className="ca-primary ca-destructive" disabled={busy} onClick={() => { void call({ type: 'remove', id: selectedId }).then(next => { if (next && !next.error) setPage('manage') }) }}>{t('确认移除', 'Remove account')}</button></div></div>}
        {displayPage === 'resume' && <div className="ca-form"><h3>{t('重启 Claude，续接已保存的对话', 'Restart Claude and resume the saved conversation')}</h3><p>{t('当前卡片的 Claude 进程会重启。项目文件与已保存的对话保留；未提交的输入、进行中的命令和子代理临时状态不作恢复保证。', 'Claude in this card will restart. Project files and saved conversation history remain. Unsubmitted input, active commands, and temporary subagent state may be lost.')}</p><div className="ca-notice">{t('启动后请以 Claude 的实际账户状态为准。这里确认的是续接进程已启动。', 'Verify the account inside Claude after launch. This confirms that the resume process started, not the account used by a request.')}</div><div className="ca-button-row"><button className="ca-secondary" disabled={busy} onClick={back}>{t('稍后处理', 'Later')}</button><button className="ca-primary" disabled={busy || !activeSession?.resumable || activeSession.busy} onClick={() => { if (activeSession) void call({ type: 'resume', sessionId, runId: activeSession.runId }).then(next => { if (next && !next.error) back() }) }}>{t('重启并续接', 'Restart and resume')}</button></div></div>}
      </div>
      {displayPage === 'list' && state.accounts.length > 0 && <div className="ca-actions"><button type="button" className="ca-link" disabled={busy} onClick={() => setPage('add')}>＋ {t('添加账户', 'Add account')}</button><button type="button" className="ca-link" disabled={busy} onClick={() => setPage('manage')}>{t('管理账户', 'Manage accounts')}</button></div>}
      <footer className="ca-footer"><span aria-hidden="true">▣</span>{t('切换本机默认账户，会影响共用登录的 Claude 实例。', 'Changing the device default affects Claude instances sharing this login.')}</footer>
    </div>, document.body)}
  </>
}
