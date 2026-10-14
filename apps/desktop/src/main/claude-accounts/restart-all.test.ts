import { describe, expect, it, vi } from 'vitest'
import { ClaudeAccountsManager } from './manager'
import { isAccountCommand } from '../../shared/claude-accounts'

function setup() {
  const sessions = [
    { sessionId: 'visible', runId: 'v1', defaultLogin: true, busy: true, resumable: true },
    { sessionId: 'hidden', runId: 'h1', defaultLogin: false, busy: false, resumable: true },
    { sessionId: 'problem', runId: 'p1', defaultLogin: true, busy: true, resumable: false }
  ]
  const request = vi.fn(async (action: string, id?: string) => {
    if (action === 'restart' && id === 'problem') throw Error('unavailable')
    return sessions
  })
  const engine = { run: vi.fn(async () => ({ accounts: [] })) }
  const manager = new ClaudeAccountsManager(engine, {load:async()=>({names:{},rememberScope:false}),save:async()=>{}}, {request}, vi.fn())
  return {manager,request,engine}
}
describe('explicit restart of all Matou Claude sessions', () => {
  it('requires a confirmed command; never accepts generic execution', () => {
    expect(isAccountCommand({type:'restart-all'})).toBe(false)
    expect(isAccountCommand({type:'restart-all',confirmed:false})).toBe(false)
    expect(isAccountCommand({type:'restart-all',confirmed:true})).toBe(true)
  })
  it('includes busy and custom-provider Claude sessions, reports partial failure without undoing other successes', async () => {
    const {manager,request,engine}=setup()
    const result=await manager.command({type:'restart-all',confirmed:true})
    for (const [id,run] of [['visible','v1'],['hidden','h1'],['problem','p1']]) expect(request).toHaveBeenCalledWith('restart',id,run)
    expect(result.restartResult).toEqual({total:3,restarted:2,failed:1,completed:true})
    expect(engine.run).not.toHaveBeenCalled() // no auth mutation or generated messages
    await manager.dispose()
  })
  it('coalesces double clicks and handles an empty runtime honestly',async()=>{
    const {manager,request}=setup();request.mockResolvedValue([])
    await Promise.all([manager.command({type:'restart-all',confirmed:true}),manager.command({type:'restart-all',confirmed:true})])
    expect(request.mock.calls.filter(([a])=>a==='restart')).toHaveLength(0)
    expect(manager.state().restartResult).toEqual({total:0,restarted:0,failed:0,completed:true})
    await manager.dispose()
  })
  it('initializes HUD demand once without repeated engine inspections',async()=>{
    const {manager,engine}=setup()
    await manager.command({type:'ensure-usage'});const calls=engine.run.mock.calls.length
    await manager.command({type:'ensure-usage'});expect(engine.run).toHaveBeenCalledTimes(calls)
    await manager.dispose()
  })
})
