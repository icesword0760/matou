// @vitest-environment jsdom
import { act, cleanup, render, screen, waitFor } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { LocaleProvider } from '../i18n/LocaleProvider'
import { TerminalHud } from './TerminalHud'
import type { ClaudeAccountsState } from '../../../shared/claude-accounts'
import type { SessionHudView } from '../hierarchy/hierarchy-types'

const original = window.matouDesktop
const base: ClaudeAccountsState = {revision:1,accounts:[{id:'a',name:'Work',email:'wo@example.com',plan:'Claude',needsLogin:false,usage:{checkedAt:1,stale:false,fiveHour:{used:38,resetsAt:''},weekly:{used:62,resetsAt:''}}}],currentId:'a',sessions:[],pendingSessionIds:[],rememberScope:false,operation:'idle'}
const hud: SessionHudView={sessionId:'s',mode:'agent',startedAt:1,defaultAccountUsage:true,contextPercent:72,model:'Claude',usageWindows:[{label:'5h',percent:99}]}
function setup() {
  const listeners = new Set<(v: ClaudeAccountsState)=>void>()
  const command=vi.fn(async()=>base)
  window.matouDesktop={getClaudeAccounts:async()=>base,onClaudeAccounts:(fn:(v:ClaudeAccountsState)=>void)=>{listeners.add(fn);return()=>{listeners.delete(fn)}},claudeAccountCommand:command} as never
  return {command,emit:(v:ClaudeAccountsState)=>act(()=>listeners.forEach(fn=>fn(v)))}
}
afterEach(()=>{cleanup();window.matouDesktop=original;vi.restoreAllMocks()})
describe('account quota in HUD',()=>{
  it('uses the new account cache immediately while context and model stay session-local',async()=>{
    const f=setup();const view=render(<TerminalHud hud={hud}/>);await screen.findByText('Work')
    await waitFor(()=>expect(view.container.textContent).toContain('38%'));expect(view.container.textContent).not.toContain('99%')
    const b={...base.accounts[0]!,id:'b',name:'Personal',usage:{checkedAt:2,stale:false,fiveHour:{used:12,resetsAt:''}}}
    f.emit({...base,currentId:'b',accounts:[...base.accounts,b],revision:2})
    await screen.findByText('Personal')
    expect(view.container.textContent).not.toContain('默认账户:')
    expect(view.container.querySelector('.ca-trigger-label')?.textContent).toBe('Personal')
    expect(view.container.querySelector('.status-usage')?.getAttribute('title')).toContain('默认账户: Personal')
    const next=view
    expect(next.container.textContent).toContain('12%');expect(next.container.textContent).not.toContain('38%')
    expect(next.container.textContent).toContain('72%');expect(next.container.textContent).toContain('Claude')
  })
  it('never falls back to old session usage when the new account has no quota',async()=>{
    setup();window.matouDesktop.getClaudeAccounts=async()=>({...base,currentId:'new',accounts:[]})
    const {container}=render(<TerminalHud hud={hud}/>);await waitFor(()=>expect(container.querySelector('.status-usage')?.textContent).toBe('—'));await waitFor(()=>expect(container.querySelector('.status-usage')?.getAttribute('title')).toContain('添加账户后查看用量'));expect(container.textContent).not.toContain('99%')
  })
  it('keeps independent providers on their own reported quota',async()=>{
    const {command}=setup();const {container}=render(<TerminalHud hud={{...hud,defaultAccountUsage:false}}/> )
    expect(container.textContent).toContain('99%');expect(container.textContent).not.toContain('默认账户:')
    expect(command).not.toHaveBeenCalledWith({type:'ensure-usage'})
  })
  it.each([
    ['refresh failure', {...base,usageRefreshFailed:true}, '读取失败，保留上次数据'],
    ['cached usage', {...base,accounts:[{...base.accounts[0]!,usage:{...base.accounts[0]!.usage!,stale:true}}]}, '上次记录，使用后更新']
  ] satisfies Array<[string, ClaudeAccountsState, string]>)('keeps quota visible and explains %s only in the tooltip',async(_reason,state,detail)=>{
    setup();window.matouDesktop.getClaudeAccounts=async()=>state
    const {container}=render(<TerminalHud hud={hud}/>);await screen.findByText('Work')
    await waitFor(()=>expect(container.textContent).toContain('38%'))
    expect(container.textContent).toContain('62%')
    expect(screen.queryByText('上次数据')).toBeNull()
    expect(screen.queryByText('Last known')).toBeNull()
    expect(container.textContent).not.toContain(detail)
    expect(container.querySelector('.status-usage')?.getAttribute('title')).toContain(detail)
  })
})

for (const locale of ['zh-CN', 'en'] as const) {
  describe(`minimal quota fields (${locale})`, () => {
    const {usage: _usage, ...withoutUsage} = base.accounts[0]!
    const states: Array<[string, ClaudeAccountsState]> = [
      ['no account', {...base, accounts: [], currentId: ''}],
      ['no usage', {...base, accounts: [withoutUsage]}],
      ['login expired', {...base, accounts: [{...base.accounts[0]!, needsLogin: true}]}],
      ...(['rate-limited', 'unsupported', 'unavailable'] as const).map(issue =>
        [issue, {...base, accounts: [{...base.accounts[0]!, usageIssue: issue}]}] as [string, ClaudeAccountsState]),
      ['refresh failed', {...base, usageRefreshFailed: true}],
      ['refreshing', {...base, usageRefreshing: true}],
      ['stale', {...base, accounts: [{...base.accounts[0]!, usage: {...base.accounts[0]!.usage!, stale: true}}]}]
    ]
    it.each(states)('%s keeps all explanations out of the visible quota', async (_label, state) => {
      const f = setup()
      const {container} = render(<LocaleProvider initialLocale={locale}><TerminalHud hud={hud}/></LocaleProvider>)
      await screen.findByText('Work')
      f.emit({...state, revision: 2})
      const fields = [...container.querySelectorAll('.status-usage')]
      expect(fields.length).toBeGreaterThan(0)
      for (const field of fields) {
        expect(field.textContent).toMatch(/^(—|(?:5h|Weekly) \d+% · —)$/)
        expect(field.getAttribute('title')).toBeTruthy()
      }
      expect(container.querySelector('.status-info__primary')?.textContent).not.toMatch(/上次数据|待输入|待更新|登录已失效|限流|读取失败|Last known|Sign-in expired|Pending|Rate limited/)
    })
    it('shows a dash during initial loading and preserves tooltip guidance', () => {
      setup();window.matouDesktop.getClaudeAccounts=()=>new Promise(()=>{})
      const {container}=render(<LocaleProvider initialLocale={locale}><TerminalHud hud={hud}/></LocaleProvider>)
      const field=container.querySelector('.status-usage')!
      expect(field.textContent).toBe('—')
      expect(field.getAttribute('title')).toContain(locale==='zh-CN'?'正在读取用量':'Loading usage')
    })
    it('shows the recorded date after reset, never a fabricated new reset or zero usage',async()=>{
      const reset = new Date(2026, 0, 2, 3, 4).getTime()
      vi.spyOn(Date,'now').mockReturnValue(new Date(2026,0,3,3,4).getTime())
      setup();window.matouDesktop.getClaudeAccounts=async()=>({...base,accounts:[{...base.accounts[0]!,usage:{checkedAt:1,stale:true,fiveHour:{used:38,resetsAt:new Date(reset).toISOString()}}}]})
      const {container}=render(<LocaleProvider initialLocale={locale}><TerminalHud hud={hud}/></LocaleProvider>)
      await screen.findByText('Work')
      const field=container.querySelector('.status-usage')!
      expect(field.textContent).toBe('5h 38% · 01/02 03:04')
      expect(field.getAttribute('title')).toContain(locale==='zh-CN'?'已到重置时间':'Reset time reached')
    })
    it('keeps a future countdown and handles invalid values without rendering NaN',async()=>{
      const f=setup()
      const {container}=render(<LocaleProvider initialLocale={locale}><TerminalHud hud={hud}/></LocaleProvider>)
      await screen.findByText('Work')
      f.emit({...base,revision:2,accounts:[{...base.accounts[0]!,usage:{checkedAt:1,stale:false,fiveHour:{used:38,resetsAt:new Date(Date.now()+3_600_000).toISOString()}}}]})
      expect(container.querySelector('.status-usage')?.textContent).toBe('5h 38% · 1h')
      f.emit({...base,revision:3,accounts:[{...base.accounts[0]!,usage:{checkedAt:1,stale:false,fiveHour:{used:NaN,resetsAt:'invalid'}}}]})
      expect(container.querySelector('.status-usage')?.textContent).toBe('—')
      expect(container.textContent).not.toContain('NaN')
    })
  })
}
