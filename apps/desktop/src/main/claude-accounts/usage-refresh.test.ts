import { afterEach, describe, expect, it, vi } from 'vitest'
import { ClaudeAccountsManager } from './manager'
import type { EngineState } from './engine'

const account = { id: 'a', name: 'Work', email: 'wo•••@example.com', plan: 'Claude', needsLogin: false, usage: { checkedAt: 1, stale: false, fiveHour: { used: 38, resetsAt: '' } } }
async function setup() {
  vi.useFakeTimers(); vi.setSystemTime(new Date('2026-10-14T04:00:00Z'))
  let value: EngineState = {accounts:[account], currentId:'a'}
  const run = vi.fn(async (input: Record<string,unknown>, _signal?: AbortSignal): Promise<EngineState> => {
    if (input.action === 'switch') value = {...value,currentId:String(input.id)}
    return value
  })
  const manager = new ClaudeAccountsManager({run},{load:async()=>({names:{},rememberScope:false}),save:async()=>{}},{request:async()=>[]},vi.fn())
  await manager.command({type:'inspect'})
  const calls = () => run.mock.calls.filter(([input])=>input.action==='refresh').length
  return {manager,run,calls}
}
afterEach(()=>vi.useRealTimers())
describe('shared account usage scheduler',()=>{
  it('fetches on visible open, coalesces windows, then polls at three minutes',async()=>{
    const {manager,calls}=await setup()
    manager.setUsageVisible(1,true); manager.setUsageVisible(2,true)
    await vi.advanceTimersByTimeAsync(0); expect(calls()).toBe(1)
    await vi.advanceTimersByTimeAsync(179999); expect(calls()).toBe(1)
    await vi.advanceTimersByTimeAsync(1); expect(calls()).toBe(2)
    manager.setUsageVisible(1,false); manager.setUsageVisible(2,false)
    await vi.advanceTimersByTimeAsync(600000); expect(calls()).toBe(2)
    manager.setUsageVisible(1,true); await vi.advanceTimersByTimeAsync(0); expect(calls()).toBe(3)
    await manager.dispose()
  })
  it('does not refetch on a quick focus return and resumes stale data after sleep',async()=>{
    const {manager,calls}=await setup(); manager.setUsageVisible(1,true)
    await vi.advanceTimersByTimeAsync(0)
    manager.setUsageVisible(1,false); await vi.advanceTimersByTimeAsync(10000)
    manager.setUsageVisible(1,true); await vi.advanceTimersByTimeAsync(0); expect(calls()).toBe(1)
    manager.suspendUsage(true); await vi.advanceTimersByTimeAsync(600000); expect(calls()).toBe(1)
    manager.suspendUsage(false); await vi.advanceTimersByTimeAsync(0); expect(calls()).toBe(2)
    await manager.dispose()
  })
  it('refreshes after saving or switching, without waiting for the regular interval',async()=>{
    const {manager,calls}=await setup();manager.setUsageVisible(1,true);await vi.advanceTimersByTimeAsync(0)
    await manager.command({type:'switch',id:'a',rememberScope:false,waitForIdle:false})
    await vi.advanceTimersByTimeAsync(0);expect(calls()).toBe(2)
    await manager.dispose()
  })
  it('checks a reset plus 15 seconds, without fabricating a zero percentage',async()=>{
    const {manager,run,calls}=await setup()
    const value = {accounts:[{...account,usage:{...account.usage,fiveHour:{used:100,resetsAt:new Date(Date.now()+20000).toISOString()}}}],currentId:'a'}
    run.mockResolvedValue(value)
    manager.setUsageVisible(1,true);await vi.advanceTimersByTimeAsync(0)
    await vi.advanceTimersByTimeAsync(34999);expect(calls()).toBe(1)
    await vi.advanceTimersByTimeAsync(1);expect(calls()).toBe(2)
    expect(manager.state().accounts[0]?.usage?.fiveHour?.used).toBe(100)
    await vi.advanceTimersByTimeAsync(1000);expect(calls()).toBe(2)
    await manager.dispose()
  })
  it('retains data and separates failed usage from login failure, with backed-off retries',async()=>{
    const {manager,run,calls}=await setup();run.mockRejectedValue(new Error('network'))
    manager.setUsageVisible(1,true);await vi.advanceTimersByTimeAsync(0)
    expect(manager.state()).toMatchObject({usageRefreshFailed:true,operation:'idle',accounts:[{needsLogin:false,usage:{checkedAt:1}}]})
    expect(manager.state().error).toBeUndefined()
    await vi.advanceTimersByTimeAsync(180000);expect(calls()).toBe(1)
    await vi.advanceTimersByTimeAsync(180000);expect(calls()).toBe(2)
    await manager.dispose()
  })
  it('rechecks on restored connectivity after the shared minimum interval', async()=>{
    const {manager,run,calls}=await setup();run.mockRejectedValue(new Error('offline'))
    manager.setUsageVisible(1,true);await vi.advanceTimersByTimeAsync(0)
    await vi.advanceTimersByTimeAsync(180000);expect(calls()).toBe(1)
    run.mockResolvedValue({accounts:[account],currentId:'a'})
    manager.setUsageVisible(1,true,true);await vi.advanceTimersByTimeAsync(0)
    expect(calls()).toBe(2);expect(manager.state().usageRefreshFailed).toBe(false)
    await manager.dispose()
  })
  it('coalesces manual refresh with automatic fetch and keeps other actions unlocked',async()=>{
    const {manager,run,calls}=await setup()
    let resolve!:(v:EngineState)=>void
    run.mockImplementation(input=>input.action==='refresh'?new Promise(yes=>{resolve=yes}):Promise.resolve({accounts:[account]}))
    manager.setUsageVisible(1,true);await vi.advanceTimersByTimeAsync(0)
    const manual=manager.command({type:'refresh-usage'})
    await vi.advanceTimersByTimeAsync(0);expect(calls()).toBe(1)
    expect(manager.state()).toMatchObject({operation:'idle',usageRefreshing:true})
    resolve({accounts:[account]});await manual
    expect(manager.state().usageRefreshing).toBe(false)
    await manager.dispose()
  })
  it('cancels obsolete collection before mutation, never overwrites the changed default',async()=>{
    const {manager,run}=await setup()
    let aborted=false
    run.mockImplementation((input,signal)=>{
      if(input.action==='refresh')return new Promise((_yes,no)=>signal?.addEventListener('abort',()=>{aborted=true;no(Error('cancelled'))},{once:true}))
      return Promise.resolve({accounts:[account],currentId:input.action==='switch'?'b':'a'})
    })
    manager.setUsageVisible(1,true);await vi.advanceTimersByTimeAsync(0)
    await manager.command({type:'switch',id:'b',rememberScope:false,waitForIdle:false})
    expect(aborted).toBe(true);expect(manager.state().currentId).toBe('b');expect(manager.state().usageRefreshFailed).toBe(false)
    await manager.dispose()
  })
  it('bounds a hung refresh and cancels timers on disposal',async()=>{
    const {manager,run,calls}=await setup()
    run.mockImplementation((_input,signal)=>new Promise((_yes,no)=>signal?.addEventListener('abort',()=>no(Error('timeout')),{once:true})))
    manager.setUsageVisible(1,true);await vi.advanceTimersByTimeAsync(30000)
    expect(manager.state()).toMatchObject({usageRefreshFailed:true,usageRefreshing:false,operation:'idle'})
    await manager.dispose();await vi.advanceTimersByTimeAsync(900000);expect(calls()).toBe(1)
  })
})
