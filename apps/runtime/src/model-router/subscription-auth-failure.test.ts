import { mkdir, mkdtemp, rm } from 'node:fs/promises'
import { resolve } from 'node:path'
import { afterEach, expect, it, vi } from 'vitest'
import { ProviderConfigStore } from '../provider-config/provider-config-store'
import { SubscriptionAccounts } from './subscription-accounts'
import type { ModelRouterManager } from './manager'
const disposals:Array<()=>Promise<void>>=[]
afterEach(async()=>{for(const close of disposals.splice(0).reverse())await close()})
it('marks only the captured subscription identity, notifies once, and leaves credentials untouched',async()=>{
 const evidenceRoot=resolve(import.meta.dirname,'../../../../node_modules/.cache/search-auth');await mkdir(evidenceRoot,{recursive:true})
 const root=await mkdtemp(resolve(evidenceRoot,'status-'));disposals.push(()=>rm(root,{recursive:true,force:true}))
 const store=new ProviderConfigStore(root)
 const saved=await store.adoptSubscription({name:'Account',modelMapping:{opus:'m',sonnet:'m',haiku:'m'},subscription:{provider:'codex',credentialRef:{name:'account.json',authIndex:'current'},account:{status:'ready'},modelMetadata:[{id:'m'}]}})
 const accounts=new SubscriptionAccounts({dataRoot:root,store,manager:{onStateChange:()=>()=>{}} as unknown as ModelRouterManager,openAuthorizationUrl:async()=>{}});disposals.push(()=>accounts.close())
 const changed=vi.fn();accounts.onChange(changed)
 for(const identity of ['["account.json","old"]','["other.json","current"]','malformed'])await accounts.markNeedsSignIn(saved.id,identity)
 await accounts.markNeedsSignIn('deleted','["account.json","current"]')
 expect(changed).not.toHaveBeenCalled()
 expect((await accounts.snapshot()).providers['claude-code'].find(p=>p.id===saved.id)!.subscription!.account.status).toBe('ready')
 await accounts.markNeedsSignIn(saved.id,'["account.json","current"]')
 await accounts.markNeedsSignIn(saved.id,'["account.json","current"]')
 expect(changed).toHaveBeenCalledOnce()
 expect((await accounts.snapshot()).providers['claude-code'].find(p=>p.id===saved.id)!.subscription!.account.status).toBe('needs-sign-in')
 expect((await store.subscriptions())[0]!.subscription.credentialRef).toEqual({name:'account.json',authIndex:'current'})
 vi.spyOn(accounts,'initialize').mockResolvedValue()
 await expect(accounts.prepareModelTarget(saved.id,'m')).rejects.toMatchObject({providerFailure:{status:'needs-sign-in',recovery:'sign-in'}})
 expect((await accounts.snapshot()).providers['claude-code'].find(p=>p.id===saved.id)!.subscription!.account.status).toBe('needs-sign-in')
})
