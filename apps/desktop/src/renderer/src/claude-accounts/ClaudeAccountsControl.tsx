import { ConfirmDialog } from '../hierarchy/ConfirmDialog'
import { useAccountUsageDemand } from './use-account-usage'
import { useEffect, useId, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { validLoginCode } from '../../../shared/claude-accounts'
import type { AccountError, ClaudeAccountCommand, ClaudeAccountsState, ClaudeAccountView } from '../../../shared/claude-accounts'
import { useLocale } from '../i18n/LocaleProvider'
import './claude-accounts.css'

const EMPTY: ClaudeAccountsState = { revision: 0, accounts: [], sessions: [], pendingSessionIds: [], operation: 'idle', rememberScope: false }
type Page = 'list' | 'add' | 'detected' | 'draft' | 'saved' | 'scope' | 'manage' | 'edit' | 'remove' | 'reauth' | 'repaired'
const ERRORS: Record<AccountError, [string, string]> = {
  'browser-missing': ['账户登录需要 Chrome、Edge 或 Brave。安装其中一个后重试；原浏览器登录保持不变。', 'Install Chrome, Edge or Brave to add an account in an isolated window. Your usual browser stays signed in.'],
  'browser-closed': ['登录窗口已关闭。点击下方按钮，可以重新开始本次登录。', 'The sign-in window was closed. Use the button below to try again.'],
  network: ['网络暂时没有响应，请稍后重试。', 'The network did not respond. Try again shortly.'],
  'login-required': ['登录已失效，请重新登录这个账户。', 'Sign in to this account again.'],
  'login-timeout': ['登录等待已结束。你可以重新打开登录流程。', 'Sign-in timed out. Start again when ready.'],
  'login-code-invalid': ['请粘贴浏览器提供的完整授权码（包含 # 后面的部分），不要粘贴网址。', 'Paste the complete authorization code, including the part after #, not the page URL.'],
  'login-ended': ['这次登录已结束，请重新打开登录流程并获取新的授权码。', 'This sign-in has ended. Start again to get a new authorization code.'],
  'login-failed': ['登录尚未完成，请重试并在浏览器中完成授权。', 'Sign-in did not finish. Try again and complete it in your browser.'],
  storage: ['账户偏好保存失败，请检查磁盘空间后重试。', 'Preferences were not saved. Check disk space and retry.'],
  keychain: ['请解锁系统钥匙串，并允许账户组件访问后重试。', 'Unlock the system keychain and allow access, then retry.'],
  'cli-missing': ['请先安装 Claude Code，再登录账户。', 'Install Claude Code before adding an account.'],
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
  const [confirmRestart, setConfirmRestart] = useState(false)
  const restartButton = useRef<HTMLButtonElement>(null)
  useAccountUsageDemand(open)
  const [checking, setChecking] = useState(false)
  const [page, setPage] = useState<Page>('list')
  const [selectedId, setSelectedId] = useState('')
  const [name, setName] = useState('')
  const [loginCode, setLoginCode] = useState('')
  const [codeSending, setCodeSending] = useState(false)
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
  const close = (restore = true) => { setLoginCode(''); setOpen(false); if (restore) trigger.current?.focus() }
  useEffect(() => {
    if (!bridge?.getClaudeAccounts || !bridge.onClaudeAccounts) return
    let active = true
    void bridge.getClaudeAccounts().then(v => { if (active) apply(v) }).catch(() => {})
    const off = bridge.onClaudeAccounts(apply)
    return () => { active = false; off() }
  }, [bridge])
  useEffect(() => {
    if (!open || confirmRestart) return
    title.current?.focus({ preventScroll: true })
    const click = (event: PointerEvent) => {
      if (event.target instanceof Node && !panel.current?.contains(event.target) && !trigger.current?.contains(event.target)) close(false)
    }
    const key = (event: KeyboardEvent) => {
      if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); close() }
    }
    document.addEventListener('pointerdown', click, true); document.addEventListener('keydown', key, true)
    return () => { document.removeEventListener('pointerdown', click, true); document.removeEventListener('keydown', key, true) }
  }, [open, page, confirmRestart])
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
  useEffect(() => { setLoginCode(''); setCodeSending(false) }, [state.loginAttemptId])
  const submitCode = async (event: React.FormEvent) => {
    event.preventDefault()
    if (codeSending || state.loginCodeSubmitted || !state.loginAttemptId) return
    if (!validLoginCode(loginCode)) { setLocalError(t(...ERRORS['login-code-invalid'])); return }
    setCodeSending(true)
    const code = loginCode.trim(); setLoginCode('')
    try { await call({ type: 'submit-login-code', attemptId: state.loginAttemptId, code }) }
    finally { setCodeSending(false) }
  }
  useEffect(() => {
    if (!open || !bridge?.claudeAccountCommand) return
    const timer = setInterval(() => { void bridge.claudeAccountCommand!({ type: 'refresh-sessions' }).then(apply).catch(() => {}) }, 2000)
    return () => clearInterval(timer)
  }, [open, bridge])
  const refreshUsage = () => {
    if (!bridge?.claudeAccountCommand || state.usageRefreshing) return
    void bridge.claudeAccountCommand({ type: 'refresh-usage' }).then(apply).catch(() => setState(v => ({ ...v, usageRefreshFailed: true })))
  }
  if (!bridge?.getClaudeAccounts) return null
  const current = state.accounts.find(a => a.id === state.currentId)
  const selected = state.accounts.find(a => a.id === selectedId)
  const busy = state.operation !== 'idle' || submitting
  const running = state.sessions.filter(s => s.busy).length
  const error = localError || (state.error ? t(...ERRORS[state.error]) : '')
  const repairAccount = state.accounts.find(a => a.id === state.reauthId)
  const isRepair = Boolean(repairAccount) && (state.operation === 'login' || page === 'add' || page === 'draft')
  const prepareRepair = (a: ClaudeAccountView) => { setSelectedId(a.id); setName(a.name); setLocalError(''); setPage('reauth') }
  const startLogin = (replaceId?: string) => {
    setPage('add')
    void call({ type: 'login', ...(replaceId ? { replaceId } : {}) })
  }
  const beginSwitch = (account: ClaudeAccountView) => {
    setSelectedId(account.id); setRemember(state.rememberScope)
    if (state.renewedIds?.includes(account.id)) { setPage('repaired'); return }
    if (account.needsLogin) { prepareRepair(account); return }
    if (state.rememberScope && !running) { setPage('list'); void call({ type: 'switch', id: account.id, rememberScope: true, waitForIdle: false }) }
    else setPage('scope')
  }
  const displayPage = state.operation === 'login' ? 'add' : page
  const heading = displayPage === 'manage' ? t('管理账户', 'Manage accounts') : displayPage === 'edit' ? t('编辑账户', 'Edit account') : displayPage === 'scope' ? t('切换本机默认账户', 'Switch device default') : t('Claude 账户', 'Claude accounts')
  const back = () => { setLocalError(''); setPage('list'); if (state.reauthId && !state.draft && state.operation === 'idle') void call({ type: 'cancel-login' }) }
  const date = (stamp: string | number) => {
    if (!stamp || !Number.isFinite(new Date(stamp).getTime())) return '—'
    return new Intl.DateTimeFormat(locale, { month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit' }).format(new Date(stamp))
  }
  const quota = (account: ClaudeAccountView) => account.needsLogin ? <div className="ca-login-warning">
    <span aria-hidden="true">!</span><div><strong>{t('登录已失效', 'Sign-in expired')}</strong><small>{t('重新登录后可刷新用量', 'Sign in again to refresh usage')}</small></div>
    <button className="ca-relogin" disabled={busy} onClick={() => prepareRepair(account)}>{t('重新登录', 'Sign in again')}</button>
  </div> : <>
    <div className="ca-quotas">{(['fiveHour', 'weekly'] as const).map((key, i) => {
      const value = account.usage?.[key]
      return <div className="ca-quota" key={key}><div><span>{i === 0 ? t('5 小时已用', '5-hour used') : t('每周已用', 'Weekly used')}</span><b className={value && value.used >= 90 ? 'ca-warning' : ''}>{value ? `${Math.round(value.used)}%` : '—'}</b></div>
        {value && <><div className="ca-track"><span className={value.used >= 90 ? 'is-high' : ''} style={{ width: `${value.used}%` }} /></div><small>{date(value.resetsAt)} {t('重置', 'reset')}</small></>}</div>
    })}</div>
    {account.usageIssue ? <div className="ca-usage-feedback" role="status">{account.usageIssue === 'rate-limited' ? t('请求较频繁，稍后自动重试', 'Rate limited; retrying later') : account.usageIssue === 'unsupported' ? t('此登录方式暂无用量数据', 'Usage is not available for this sign-in') : t('用量更新失败', 'Usage update failed')}{account.usageIssue !== 'unsupported' && <button className="ca-link" disabled={busy || state.usageRefreshing} onClick={refreshUsage}>{t('重试', 'Retry')}</button>}</div> : null}
    {account.usage && <span className="ca-measured">{account.usage.stale || state.usageRefreshFailed ? t('上次数据', 'Last known') : t('更新于', 'Updated')} {date(account.usage.checkedAt)}</span>}
  </>
  return <>
    <button ref={trigger} type="button" className={`status-field ca-trigger ${open ? 'is-open' : ''}`} aria-expanded={open} aria-haspopup="dialog"
      aria-label={t('Claude 账户', 'Claude accounts')} title={t('切换本机默认 Claude 账户', 'Switch the device-default Claude account')}
      onClick={() => {
        if (open) { close(); return }
        setOpen(true)
        if (state.operation === 'idle' && !state.draft) { setChecking(true); void call({ type: 'inspect' }).finally(() => setChecking(false)) }
      }}><svg aria-hidden="true" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5"><circle cx="12" cy="8" r="3.5"/><path d="M5 21v-3a7 7 0 0 1 14 0v3"/></svg><span className="ca-trigger-label">{current ? `${t('默认：', 'Default: ')}${current.name}` : t('Claude 账户', 'Claude accounts')}</span>{state.queuedId || current?.needsLogin || state.renewedIds?.length ? <i className="ca-dot" /> : null}<span aria-hidden="true">⌃</span></button>
    {open && createPortal(<div className="ca-panel" ref={panel} role="dialog" aria-labelledby={titleId} data-testid="claude-accounts-panel">
      <header className="ca-header">{displayPage !== 'list' && !busy && <button type="button" className="ca-icon" onClick={back} aria-label={t('返回账户列表', 'Back to accounts')}>‹</button>}<h2 id={titleId} ref={title} tabIndex={-1}>{heading}</h2>{displayPage === 'list' && state.accounts.length > 0 && <button className="ca-refresh-button" type="button" aria-label={t('刷新用量', 'Refresh usage')} disabled={busy || state.usageRefreshing} onClick={refreshUsage}><svg className={state.usageRefreshing ? 'is-spinning' : ''} aria-hidden="true" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8"><path d="M20 7v5h-5M4 17v-5h5M6.1 6.1A8 8 0 0 1 20 12M4 12a8 8 0 0 0 13.9 5.9"/></svg>{t('刷新', 'Refresh')}</button>}<button type="button" className="ca-icon ca-close" onClick={() => close()} aria-label={t('关闭账户面板', 'Close accounts')}>×</button></header>
      <div className="ca-scroll">
        {error && <div className="ca-notice ca-error" role="alert"><strong>{t('操作尚未完成', 'Action not completed')}</strong><p>{error}</p><button className="ca-link" type="button" disabled={busy} onClick={() => { void call({ type: 'inspect' }) }}>{t('刷新状态', 'Refresh status')}</button></div>}
        {(state.operation === 'switching' || state.operation === 'resuming') && <div className="ca-notice" role="status"><span className="ca-spinner" />{state.operation === 'switching' ? t('正在切换，请稍候…', 'Switching, please wait…') : t('正在重启并续接对话…', 'Restarting and resuming…')}</div>}
        {displayPage === 'list' && <>
          {state.queuedId && <div className="ca-notice" role="status"><strong>{t('已安排任务结束后切换', 'Switch scheduled after tasks finish')}</strong><p>{t('可以继续查看工作。退出 Matou 会取消这次安排。', 'Keep reviewing your work. Quitting Matou cancels this schedule.')}</p><button className="ca-link" disabled={state.operation === 'switching'} onClick={() => { void call({ type: 'cancel-switch' }) }}>{t('取消安排', 'Cancel schedule')}</button></div>}
          {state.switchedAt && <div className="ca-switch-feedback" role="status"><span className="ca-switch-check" aria-hidden="true">✓</span><div>{state.appliedReauthId ? t('登录已更新，现有会话通常会在约 30 秒内使用新登录，或者选择立即重启并接续。', 'Sign-in updated. Existing sessions usually pick it up within about 30 seconds, or you can restart and resume now.') : t('已切换账户，现有会话通常会在约 30 秒内使用新账户，或者选择立即重启并接续。', 'Account switched. Existing sessions usually pick it up within about 30 seconds, or you can restart and resume now.')} <button ref={restartButton} type="button" className="ca-restart-inline" disabled={busy} onClick={() => setConfirmRestart(true)}>{state.operation === 'resuming' ? t('正在重启…', 'Restarting…') : t('立即重启', 'Restart now')}</button></div></div>}
          {state.restartResult && <div className={`ca-restart-result ${state.restartResult.failed ? 'has-error' : ''}`} role={state.restartResult.failed ? 'alert' : 'status'}>{!state.restartResult.completed ? t(`正在重启 Claude 会话… ${state.restartResult.restarted + state.restartResult.failed}/${state.restartResult.total}`, `Restarting Claude sessions… ${state.restartResult.restarted + state.restartResult.failed}/${state.restartResult.total}`) : state.restartResult.total === 0 ? t('没有正在运行的 Claude 会话，无需重启。', 'No running Claude sessions to restart.') : state.restartResult.failed ? t(`已接续 ${state.restartResult.restarted} 个，${state.restartResult.failed} 个未完成。请检查对应终端；启动中、存储异常或超时的会话会保留恢复入口。`, `Resumed ${state.restartResult.restarted}; ${state.restartResult.failed} did not finish. Check the affected terminals for startup, storage, or timeout recovery.`) : t(`已重启并接续 ${state.restartResult.restarted} 个正在运行的 Claude 会话。`, `Restarted and resumed ${state.restartResult.restarted} running Claude sessions.`)}</div>}
          {state.draft && <div className="ca-notice"><strong>{t('登录已完成，等待保存', 'Sign-in complete, ready to save')}</strong><button className="ca-link ca-wide" disabled={busy} onClick={() => setPage('draft')}>{t('继续核对账户', 'Continue reviewing account')}</button></div>}{state.detected && <div className="ca-notice"><strong>{t('发现本机已有 Claude 登录', 'Existing Claude sign-in found')}</strong><p>{state.detected.email}<br />{t('保存后就能随时切回来，无需重新登录。', 'Save it to switch back later without signing in again.')}</p><button className="ca-primary" disabled={busy} onClick={() => { setName(state.detected!.name); setPage('detected') }}>{t('保存当前账户', 'Save current account')}</button></div>}
          {checking && !state.accounts.length && !state.detected && <div className="ca-welcome" role="status"><span className="ca-spinner" />{t('正在读取本机账户…', 'Reading device accounts…')}</div>}
          {!checking && !state.accounts.length && !state.detected && <div className="ca-welcome"><span className="ca-welcome-icon" aria-hidden="true">⇄</span><h3>{t('多个账户，随手切换', 'Your accounts, one click away')}</h3><p>{t('每个账户只需登录一次。以后在这里查看用量、选择账户，不打断你的工作节奏。', 'Sign in once per account. Check usage and choose an account here when you need it.')}</p><ol><li>{t('在官方页面登录 Claude', 'Sign in on the official Claude page')}</li><li>{t('保存账户，按需设为默认', 'Save it, then choose when to switch')}</li></ol><button className="ca-primary ca-wide" disabled={busy} onClick={() => setPage('add')}>{t('添加第一个账户', 'Add your first account')}</button></div>}
          {state.accounts.map((a, i) => <article key={a.id} className={`ca-account ${state.currentId === a.id ? 'is-current' : ''}`} aria-label={a.name}>
            <div className="ca-account-identity"><span className={`ca-avatar tone-${i % 3}`}>{String(i + 1).padStart(2, '0')}</span><div className="ca-account-main"><strong>{a.name}</strong><div className="ca-email">{a.email}</div></div>
            {state.currentId === a.id ? <span className="ca-default-pill">{t('本机默认', 'Default')}</span> : !a.needsLogin && <button className="ca-switch" disabled={busy} onClick={() => beginSwitch(a)}>{t('切换', 'Switch')}</button>}</div>{quota(a)}</article>)}
          {state.usageRefreshFailed && <div className="ca-usage-feedback ca-refresh-error" role="status">{state.accounts.some(a => a.usage) ? t('用量更新失败，已保留上次数据', 'Usage update failed; previous data kept') : t('用量更新失败，请稍后重试', 'Usage update failed. Try again shortly.')}<button className="ca-link" disabled={busy || state.usageRefreshing} onClick={refreshUsage}>{t('重试', 'Retry')}</button></div>}
          {state.renewedIds?.map(id => { const a = state.accounts.find(a => a.id === id); return a && <div className="ca-notice" key={id}><strong>{a.name} · {t('新登录已保存', 'Renewed sign-in saved')}</strong><p>{id === state.currentId ? t('还需应用到本机默认登录；现有会话尚未重启。', 'Apply it to the device default next. No sessions have been restarted.') : t('当前默认账户保持不变。需要时再切换。', 'The current default is unchanged. Switch when needed.')}</p><button className="ca-link" disabled={busy} onClick={() => { setSelectedId(id); setPage('repaired') }}>{t('继续处理', 'Continue')}</button></div> })}
        </>}
        {displayPage === 'reauth' && selected && <div className="ca-form"><h3>{t('重新登录', 'Sign in again: ')}「{selected.name}」</h3><p className="ca-identity">{selected.email}</p><p>{t('请使用这个账户完成授权。只更新原账户登录，保留名称和账户列表。', 'Authorize this same account. Its saved sign-in is updated without changing the name or adding a duplicate.')}</p><div className="ca-notice">{t('现有对话保持运行。授权后先保存新登录，再由你决定何时应用、是否续接对话。隐私窗口由你自行关闭。', 'Conversations keep running. Save the renewed sign-in first, then choose when to apply it and whether to resume a conversation. Close the private window yourself.')}</div><button className="ca-primary ca-wide" disabled={busy} onClick={() => startLogin(selected.id)}>{t('打开隐私窗口重新登录', 'Open private window to sign in again')}</button><button className="ca-link ca-wide" onClick={back}>{t('暂不处理', 'Not now')}</button></div>}
        {displayPage === 'repaired' && selected && <div className="ca-form"><h3>{t('新登录已保存', 'Renewed sign-in saved')}</h3><p className="ca-identity">{selected.name} · {selected.email}</p><p>{selected.id === state.currentId ? t('下一步将新凭据应用到本机默认登录。账户不变，也不会自动重启任何对话。', 'Next, apply the new credentials to this device’s default. The account stays the same and no conversations restart automatically.') : t('这个账户的登录已更新，当前默认账户保持不变。需要时再切换。', 'This account’s sign-in is renewed. The device default is unchanged; switch when needed.')}</p><div className="ca-notice">{t('应用新登录会影响共用默认登录的 Claude 实例；应用外的任务请自行确认。', 'Applying credentials affects Claude instances sharing the default sign-in. Check tasks outside Matou yourself.')}{running > 0 && <p role="status">{t('Matou 内仍有任务运行。新登录已保存；任务结束后，下方按钮会恢复可用，由你点击应用。', 'Matou tasks are still running. Your renewed sign-in is saved; apply it yourself when the button becomes available after they finish.')}</p>}</div><button className="ca-primary ca-wide" disabled={busy || running > 0} onClick={() => { if (selected.id === state.currentId) void call({ type: 'apply-reauth', id: selected.id }).then(next => { if (next && !next.error) back() }); else setPage('scope') }}>{selected.id === state.currentId ? t('应用到本机默认登录', 'Apply to device default') : t('切换到这个账户…', 'Switch to this account…')}</button><button className="ca-secondary ca-wide" disabled={busy} onClick={back}>{t('稍后处理', 'Later')}</button></div>}
        {displayPage === 'add' && <div className={`ca-form ca-center${state.operation === 'login' ? ' ca-login' : ''}`}>
        {state.operation !== 'login' && <div className="ca-browser" aria-hidden="true">↗</div>}<h3>{isRepair ? t(`重新登录「${repairAccount!.name}」`, `Sign in again: ${repairAccount!.name}`) : state.operation === 'login' ? t('在独立窗口登录另一个账户', 'Sign in to another account in the separate window') : t('添加 Claude 账户', 'Add a Claude account')}</h3>{state.operation !== 'login' && isRepair && <p>{t('请使用原账户重新授权。现有对话与默认登录保持原状。', 'Authorize the original account. Existing conversations and the default sign-in are unchanged.')}</p>}{state.operation !== 'login' && !isRepair && <><p>{t('将打开独立的临时隐私窗口，不沿用原浏览器的登录。请在 Claude 官方页面登录要添加的账户。', 'A separate temporary private window opens without your existing browser sign-in. Sign in to the account you want to add on the official Claude page.')}</p><div className="ca-steps"><span>1 {t('独立窗口登录', 'Private sign-in')}</span><span>2 {t('核对并保存', 'Review and save')}</span></div></>}{state.operation === 'login' ? <><p role="status">{isRepair ? t(`请使用 ${repairAccount!.email} 完成授权。现有会话保持运行，授权后回到这里更新登录。`, `Authorize ${repairAccount!.email}. Existing sessions keep running. Return here to update the sign-in.`) : t('请在浏览器中完成授权，然后回到这里核对并保存。当前默认账户保持不变。', 'Authorize in your browser, then return here to review and save. Your default account stays unchanged.')}</p><form className="ca-code-form" onSubmit={submitCode}>
          <label htmlFor={`${titleId}-code`}>{t('浏览器显示了授权码？', 'Browser showing an authorization code?')}</label>
          <p>{t('复制完整授权码并粘贴到这里，即可继续添加。自动返回成功时无需填写。', 'Paste the complete code here to continue. Skip this if sign-in returns automatically.')}</p>
          <input id={`${titleId}-code`} aria-label={t('授权码', 'Authorization code')} type="password" autoComplete="off" spellCheck={false}
            value={loginCode} disabled={codeSending || state.loginCodeSubmitted || !state.loginAttemptId}
            placeholder={t('粘贴完整授权码', 'Paste the complete authorization code')}
            onChange={event => { setLoginCode(event.target.value); setLocalError('') }} />
          <button className="ca-primary ca-wide" type="submit" disabled={!loginCode.trim() || codeSending || state.loginCodeSubmitted || !state.loginAttemptId}>
            {codeSending || state.loginCodeSubmitted ? t('正在完成登录…', 'Completing sign-in…') : t('完成登录', 'Complete sign-in')}
          </button>
          {state.loginCodeSubmitted && <p role="status">{t('授权码已提交，正在核对账户。', 'Code submitted. Checking account identity.')}</p>}
        </form>
        <p className="ca-muted">{t('隐私窗口由你自行关闭，Matou 不会自动关闭。', 'Close the private window when you are ready. Matou leaves it open.')}</p>
        <button className="ca-secondary" onClick={() => { setLoginCode(''); void call({ type: 'cancel-login' }) }}>{t('取消本次登录', 'Cancel sign-in')}</button></> : <button className="ca-primary ca-wide" disabled={busy} onClick={() => startLogin(state.reauthId)}>{isRepair ? t('重新打开隐私窗口', 'Reopen private window') : t('打开独立隐私窗口', 'Open private sign-in window')}</button>}</div>}
        {(displayPage === 'detected' || displayPage === 'draft' || displayPage === 'edit') && <form className="ca-form" onSubmit={event => {
          event.preventDefault()
          if (!name.trim() || [...name.trim()].length > 12) { setLocalError(t('请输入 1–12 个字符的账户名称。', 'Enter a name with 1–12 characters.')); return }
          const command: ClaudeAccountCommand = displayPage === 'edit' ? { type: 'rename', id: selectedId, name } : { type: displayPage === 'draft' ? 'save-draft' : 'save-detected', name }
          const savedId = displayPage === 'draft' ? state.draft?.id : state.detected?.id
          void call(command).then(next => { if (next && !next.error) { if (savedId) setSelectedId(savedId); setPage(displayPage === 'edit' ? 'manage' : isRepair ? 'repaired' : 'saved') } })
        }}><h3>{displayPage === 'edit' ? selected?.name : isRepair ? t('确认并更新原账户登录', 'Confirm the renewed sign-in') : t('确认账户，起个好记的名字', 'Review your account and give it a name')}</h3><p className="ca-identity">{displayPage === 'edit' ? selected?.email : displayPage === 'draft' ? state.draft?.email : state.detected?.email}</p>{displayPage === 'draft' && !isRepair && state.accounts.some(a => a.id === state.draft?.id) && <div className="ca-notice">{t('这个账户已经添加过。本次会更新原有登录，不创建重复条目。', 'This account already exists. Saving updates its sign-in without creating a duplicate.')}<button className="ca-link ca-wide" type="button" disabled={busy} onClick={() => startLogin()}>{t('登录其他账户', 'Sign in to a different account')}</button></div>}<label htmlFor={nameId}>{t('账户名称', 'Account name')}<small>{[...name].length}/12</small></label><input id={nameId} readOnly={isRepair} value={name} maxLength={24} onChange={e => setName(e.target.value)} placeholder={t('例如：工作账户', 'For example: Work')} autoComplete="off" /><p className="ca-helper">{isRepair ? t('保留原账户名称，不创建重复账户。', 'Keeps the existing name without adding a duplicate account.') : t('名称只在 Matou 中显示，方便你区分账户。', 'This name is only shown in Matou to help you recognize the account.')}</p><button className="ca-primary ca-wide" type="submit" disabled={busy}>{isRepair ? t('保存新登录', 'Save renewed sign-in') : t('保存账户', 'Save account')}</button>{displayPage === 'draft' && isRepair && <p className="ca-helper">{t('将更新原记录。保存后仍由你决定何时应用到默认登录，现有会话不会自动重启。', 'Updates the existing record. You choose when to apply it to the default; existing conversations do not restart automatically.')}</p>}{displayPage === 'draft' && <button className="ca-link ca-wide" type="button" disabled={busy} onClick={() => { void call({ type: 'cancel-login' }).then(back) }}>{isRepair ? t('放弃本次重新登录', 'Discard this renewal') : t('放弃本次添加', 'Discard this sign-in')}</button>}{displayPage === 'edit' && <div className="ca-edit-actions"><button className="ca-link" type="button" disabled={busy} onClick={() => { if (selected) prepareRepair(selected) }}>{t('重新登录', 'Sign in again')}</button><button className="ca-link ca-danger" type="button" disabled={busy} onClick={() => setPage('remove')}>{t('移除账户…', 'Remove account…')}</button></div>}</form>}
        {displayPage === 'saved' && <div className="ca-form ca-center"><span className="ca-success-icon">✓</span><h3>{t('账户已保存', 'Account saved')}</h3><p>{t('当前默认登录保持原状。你可以现在切换，也可以等需要时再使用。', 'Your default login is unchanged. Switch now or use this account later.')}</p>{selected && selected.id !== state.currentId && <button className="ca-primary ca-wide" disabled={busy} onClick={() => beginSwitch(selected)}>{t('现在切换到这个账户', 'Switch to this account now')}</button>}<button className="ca-secondary ca-wide" onClick={back}>{t('返回账户列表', 'Back to accounts')}</button></div>}
        {displayPage === 'scope' && selected && <div className="ca-form"><h3>{t('切换到', 'Switch to')}「{selected.name}」</h3><p>{t('这是本机默认登录，不是只切换当前卡片。', 'This changes the device default, not only the current card.')}</p><div className="ca-notice"><strong>{t('共享默认登录的 Claude 实例都会受影响', 'Shared default-login instances are affected')}</strong><p>{t('包括 Matou 内的默认登录会话，以及应用外的终端和编辑器。API Key、自定义服务不在此范围。', 'This includes Matou default-login sessions and external terminals or editors. API keys and custom providers are separate.')}</p></div>{running > 0 && <div className="ca-notice ca-amber"><strong>{t('任务仍在运行', 'Tasks are still running')}</strong><p>{t('等 Matou 内任务结束后再切换；安排可随时取消。', 'Switch after Matou tasks finish. You can cancel the schedule.')}</p></div>}<p>{t('应用外任务请自行确认。已有对话是否生效，会在切换后单独提示。', 'Check external tasks yourself. Existing conversations may need a separate restart after switching.')}</p><label className="ca-check"><input type="checkbox" checked={remember} onChange={e => setRemember(e.target.checked)} /><span>{t('已了解范围，下次空闲时直接切换', 'Remember this scope and switch directly when idle')}</span></label><div className="ca-button-row"><button className="ca-secondary" onClick={back} disabled={busy}>{t('暂不切换', 'Not now')}</button><button className="ca-primary" disabled={busy} onClick={() => { void call({ type: 'switch', id: selected.id, rememberScope: remember, waitForIdle: true }).then(next => { if (next && !next.error) back() }) }}>{running ? t('任务结束后切换', 'Switch when idle') : t('确认切换', 'Confirm switch')}</button></div></div>}
        {displayPage === 'manage' && <div className="ca-form"><p>{t('账户登录由 claude-swap 管理。这里的名称只影响 Matou 显示。', 'Sign-ins are managed by claude-swap. Names here only affect Matou.')}</p>{state.accounts.map((a, i) => <div className="ca-manage-row" key={a.id}><span className={`ca-avatar tone-${i % 3}`}>{a.name.slice(0, 1)}</span><div><strong>{a.name}</strong><small>{a.email}{a.id === state.currentId ? ` · ${t('本机默认', 'Default')}` : ''}</small></div><button className="ca-link" disabled={busy} onClick={() => { setSelectedId(a.id); setName(a.name); setPage('edit') }}>{t('编辑', 'Edit')}</button></div>)}<p className="ca-helper">{t('移除会删除 claude-swap 保存的账户副本，不执行 Claude 退出登录。', 'Removing deletes the copy saved by claude-swap; it does not sign Claude out.')}</p><button className="ca-secondary ca-wide" disabled={busy} onClick={() => setPage('add')}>＋ {t('添加账户', 'Add account')}</button></div>}
        {displayPage === 'remove' && <div className="ca-form"><h3>{t('移除', 'Remove')}「{selected?.name}」？</h3><p>{t('将删除 claude-swap 中保存的登录。下次添加需要重新登录；本机当前登录与项目文件保留。', 'This deletes the saved sign-in from claude-swap. You will need to sign in to add it again. The current device login and project files remain.')}</p><div className="ca-button-row"><button className="ca-secondary" disabled={busy} onClick={() => setPage('edit')}>{t('保留账户', 'Keep account')}</button><button className="ca-primary ca-destructive" disabled={busy} onClick={() => { void call({ type: 'remove', id: selectedId }).then(next => { if (next && !next.error) setPage('manage') }) }}>{t('确认移除', 'Remove account')}</button></div></div>}

      </div>
      {displayPage === 'list' && state.accounts.length > 0 && <div className="ca-actions"><button type="button" className="ca-primary" disabled={busy} onClick={() => setPage('add')}>＋ {t('添加账户', 'Add account')}</button><button type="button" className="ca-link" disabled={busy} onClick={() => setPage('manage')}>{t('管理账户', 'Manage accounts')}</button></div>}
      {displayPage !== 'list' && <footer className="ca-footer"><span aria-hidden="true">▣</span>{t('切换本机默认账户，会影响共用登录的 Claude 实例。', 'Changing the device default affects Claude instances sharing this login.')}</footer>}
    </div>, document.body)}
    {confirmRestart && <ConfirmDialog title={t('确定重启所有正在运行的 Claude 会话窗口吗？', 'Restart all running Claude session windows?')}
      body={<><p>{t('这会造成进行中任务的中断。', 'This will interrupt tasks in progress.')}</p><p>{t('仅重启 Matou 内正在运行的 Claude 会话。已退出的会话不计入，也不会自动打开；普通 Shell 和其他会话不受影响。', 'Only running Claude sessions inside Matou restart. Exited sessions are not counted or opened. Ordinary shells and other sessions are unaffected.')}</p></>}
      confirmLabel={t('确定', 'Confirm')} cancelLabel={t('取消', 'Cancel')}
      onCancel={() => { setConfirmRestart(false); requestAnimationFrame(() => restartButton.current?.focus()) }}
      onConfirm={() => { setConfirmRestart(false); void call({ type: 'restart-all', confirmed: true }); requestAnimationFrame(() => restartButton.current?.focus()) }} />}
  </>
}
