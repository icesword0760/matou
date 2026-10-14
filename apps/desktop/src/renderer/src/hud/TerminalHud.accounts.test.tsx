// @vitest-environment jsdom
import { act, cleanup, render, screen, waitFor } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
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
    const f=setup();const view=render(<TerminalHud hud={hud}/>);await screen.findByText('默认账户: Work')
    await waitFor(()=>expect(view.container.textContent).toContain('38%'));expect(view.container.textContent).not.toContain('99%')
    const b={...base.accounts[0]!,id:'b',name:'Personal',usage:{checkedAt:2,stale:false,fiveHour:{used:12,resetsAt:''}}}
    f.emit({...base,currentId:'b',accounts:[...base.accounts,b],revision:2})
    await screen.findByText('默认账户: Personal')
    const next=view
    expect(next.container.textContent).toContain('12%');expect(next.container.textContent).not.toContain('38%')
    expect(next.container.textContent).toContain('72%');expect(next.container.textContent).toContain('Claude')
  })
  it('never falls back to old session usage when the new account has no quota',async()=>{
    setup();window.matouDesktop.getClaudeAccounts=async()=>({...base,currentId:'new',accounts:[]})
    const {container}=render(<TerminalHud hud={hud}/>);await screen.findByText('添加账户后查看用量');expect(container.textContent).not.toContain('99%')
  })
  it('keeps independent providers on their own reported quota',async()=>{
    const {command}=setup();const {container}=render(<TerminalHud hud={{...hud,defaultAccountUsage:false}}/> )
    expect(container.textContent).toContain('99%');expect(container.textContent).not.toContain('默认账户:')
    expect(command).not.toHaveBeenCalledWith({type:'ensure-usage'})
  })
  it('preserves same-account values with a visible stale marker after failure',async()=>{
    setup();window.matouDesktop.getClaudeAccounts=async()=>({...base,usageRefreshFailed:true})
    const {container}=render(<TerminalHud hud={hud}/>);await screen.findByText('上次数据');expect(container.textContent).toContain('38%')
  })
})
