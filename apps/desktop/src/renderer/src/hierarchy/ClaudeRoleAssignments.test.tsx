// @vitest-environment jsdom
import { act, cleanup, render, screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, expect, it, vi } from 'vitest'
import type { AgentModelSettingsSnapshot, ProviderConfigView } from '@matou/contracts'
import { ClaudeRoleAssignments } from './ClaudeRoleAssignments'
import { LocaleProvider } from '../i18n/LocaleProvider'
afterEach(cleanup)
const official:ProviderConfigView={id:'official',cli:'claude-code',name:'Claude 官方',endpoint:'https://api.anthropic.com',model:'default',hasApiKey:false,builtIn:true}
const api:ProviderConfigView={id:'api',cli:'claude-code',name:'DeepSeek',endpoint:'https://api.invalid',model:'wrong-total-model',hasApiKey:true,modelMetadata:[{id:'a',displayName:'Model A'},{id:'b',displayName:'Model B'}]}
function fixture(assignments: Record<string,{providerId:string;model:string}>={}) {
 const snapshot:AgentModelSettingsSnapshot={preferences:{contextWindows:{api:'512k'},compaction:{mode:'follow',effort:'follow'},claudeRoleAssignments:assignments},providers:[{id:'official',name:'Official',cli:'claude-code',compactionAvailable:true,models:[{id:'opus',resolvedModel:'claude-opus-native',name:'Native Opus',fixedTokens:512000},{id:'sonnet',resolvedModel:'claude-sonnet-native',name:'Native Sonnet',fixedTokens:512000},{id:'haiku',resolvedModel:'claude-haiku-native',name:'Native Haiku',fixedTokens:512000}]},{id:'api',name:'DeepSeek',cli:'claude-code',compactionAvailable:true,models:[{id:'a',name:'Model A',fixedTokens:512000},{id:'b',name:'Model B',fixedTokens:512000},{id:'deleted',name:'Stale mapping',fixedTokens:512000}]}]}
 const request=vi.fn(async()=>snapshot)
 return {client:{request},providers:[official,api],activeProviderId:'official',snapshot,accounts:{},onSaved:vi.fn(),onRetry:vi.fn()}
}
it('shows three native default targets without using the provider total startup model',()=>{
 const f=fixture();render(<ClaudeRoleAssignments {...f}/> )
 expect(screen.getByLabelText('Haiku 模型')).toHaveProperty('disabled',true)
 expect(screen.getByText(/最终：Claude 官方.*Native Haiku/)).toBeTruthy()
 expect(screen.getByRole('button',{name:'保存分配'})).toHaveProperty('disabled',true)
})
it('clears model when provider changes, patches only assignments, and retains save focus',async()=>{
 const f=fixture();f.client.request.mockImplementation(async()=>({...f.snapshot,preferences:{...f.snapshot.preferences,claudeRoleAssignments:{opus:{providerId:'api',model:'a'}}}}))
 render(<ClaudeRoleAssignments {...f}/>);const user=userEvent.setup()
 await user.selectOptions(screen.getByLabelText('Opus 供应商'),'api')
 expect(screen.getByLabelText('Opus 模型')).toHaveProperty('value','')
 expect(screen.getByRole('button',{name:'保存分配'})).toHaveProperty('disabled',true)
 await user.selectOptions(screen.getByLabelText('Opus 模型'),'a')
 const save=screen.getByRole('button',{name:'保存分配'});save.focus();await user.keyboard(' ')
 expect(f.client.request).toHaveBeenCalledExactlyOnceWith('agent-model-settings.patch-assignments',{assignments:{opus:{providerId:'api',model:'a'}}})
 expect(await screen.findByText('模型分配已保存，新会话将使用此配置')).toBeTruthy()
 expect(document.activeElement).toBe(save);expect(save.getAttribute('aria-disabled')).toBe('true')
 await user.keyboard('{Enter}');expect(f.client.request).toHaveBeenCalledTimes(1)
})
it('retains draft and supports retry after save failure; restore defaults still requires save',async()=>{
 const f=fixture({opus:{providerId:'api',model:'a'}});f.client.request.mockRejectedValueOnce(Error('offline'))
 render(<ClaudeRoleAssignments {...f}/>);const user=userEvent.setup()
 await user.selectOptions(screen.getByLabelText('Opus 模型'),'b');await user.click(screen.getByRole('button',{name:'保存分配'}))
 expect((await screen.findByRole('alert')).textContent).toContain('保存未确认')
 expect(screen.getByLabelText('Opus 模型')).toHaveProperty('value','b')
 await user.click(screen.getByRole('button',{name:'恢复默认'}));expect(f.client.request).toHaveBeenCalledTimes(1)
 await user.click(screen.getByRole('button',{name:'保存分配'}));expect(f.client.request).toHaveBeenLastCalledWith('agent-model-settings.patch-assignments',{assignments:{}})
})
it('retains unavailable model values even if snapshot mapping candidates still include them',()=>{
 const f=fixture({opus:{providerId:'api',model:'deleted'}});render(<ClaudeRoleAssignments {...f}/> )
 expect(screen.getByLabelText('Opus 模型')).toHaveProperty('value','deleted');expect(screen.getByRole('alert').textContent).toContain('模型已不可用')
 expect(screen.getByRole('button',{name:'保存分配'})).toHaveProperty('disabled',true)
})
it('retains missing providers and signed-out accounts; restoring clears invalid assignments',async()=>{
 const f=fixture({opus:{providerId:'gone',model:'old'}});render(<ClaudeRoleAssignments {...f}/> )
 expect(screen.getByLabelText('Opus 供应商')).toHaveProperty('value','gone');expect(screen.getByRole('alert').textContent).toContain('供应商已不存在')
 await userEvent.setup().click(screen.getByRole('button',{name:'恢复默认'}))
 expect(screen.queryByRole('alert')).toBeNull();expect(screen.getByRole('button',{name:'保存分配'})).toHaveProperty('disabled',false)
})
it('allows manual API IDs only without an authoritative directory',async()=>{
 const f=fixture();f.providers=[official,((({modelMetadata,...rest})=>rest)(api))];render(<ClaudeRoleAssignments {...f}/> )
 const user=userEvent.setup();await user.selectOptions(screen.getByLabelText('Sonnet 供应商'),'api')
 await user.selectOptions(screen.getByLabelText('Sonnet 模型'),'__manual__')
 await user.type(screen.getByRole('textbox',{name:'Sonnet 模型 ID'}),'vendor/custom')
 await user.click(screen.getByRole('button',{name:'保存分配'}))
 expect(f.client.request).toHaveBeenCalledWith('agent-model-settings.patch-assignments',{assignments:{sonnet:{providerId:'api',model:'vendor/custom'}}})
})
it('keeps edits across provider refresh and reports failed reads without declaring models deleted',async()=>{
 const f=fixture();const {rerender}=render(<ClaudeRoleAssignments {...f}/>);const user=userEvent.setup()
 await user.selectOptions(screen.getByLabelText('Opus 供应商'),'api');await user.selectOptions(screen.getByLabelText('Opus 模型'),'b')
 rerender(<ClaudeRoleAssignments {...f} providers={[...f.providers]} snapshot={structuredClone(f.snapshot)} loadError="目录读取失败"/> )
 expect(screen.getByLabelText('Opus 模型')).toHaveProperty('value','b');expect(screen.queryByText(/模型已不可用/)).toBeNull()
 expect(screen.getByRole('button',{name:'保存分配'})).toHaveProperty('disabled',true)
 await user.click(screen.getByRole('button',{name:'重试'}));expect(f.onRetry).toHaveBeenCalledOnce()
})
it('reads persisted assignments after remount and renders formal English copy',()=>{
 const f=fixture({sonnet:{providerId:'api',model:'b'}})
 render(<LocaleProvider initialLocale="en"><ClaudeRoleAssignments {...f}/></LocaleProvider>)
 expect(screen.getByLabelText('Sonnet model')).toHaveProperty('value','b')
 expect(screen.getByText('New sessions use these assignments. Running sessions are unaffected.')).toBeTruthy()
 expect(screen.getByRole('button',{name:'Save assignments'})).toHaveProperty('disabled',true)
})

it('shows native tier names for an unmapped API default rather than its unrelated startup model',()=>{
 const f=fixture();render(<ClaudeRoleAssignments {...f} activeProviderId="api"/> )
 expect(screen.getByText(/最终：DeepSeek.*Native Haiku/)).toBeTruthy()
 expect(screen.queryByText(/最终：.*wrong-total-model/)).toBeNull()
})
it('keeps a signed-out subscription selection and allows choosing defaults to recover',async()=>{
 const f=fixture({opus:{providerId:'sub',model:'gpt'}})
 f.providers.push({id:'sub',cli:'claude-code',name:'Subscription',endpoint:'',model:'gpt',hasApiKey:false,kind:'subscription',subscription:{provider:'codex',account:{status:'needs-sign-in',email:'fixture@matou.invalid'},modelMetadata:[{id:'gpt'}]}})
 render(<ClaudeRoleAssignments {...f}/> )
 expect(screen.getByLabelText('Opus 模型')).toHaveProperty('value','gpt')
 expect(screen.getByRole('alert').textContent).toContain('账号已失效')
 expect(screen.getByRole('button',{name:'保存分配'})).toHaveProperty('disabled',true)
 await userEvent.setup().click(screen.getByRole('button',{name:'恢复默认'}))
 expect(screen.getByRole('button',{name:'保存分配'})).toHaveProperty('disabled',false)
})
it('does not save a loading, empty-provider or disconnected state',()=>{
 const f=fixture();const {rerender}=render(<ClaudeRoleAssignments {...f} snapshot={undefined}/> )
 expect(screen.getByRole('button',{name:'保存分配'})).toHaveProperty('disabled',true)
 rerender(<ClaudeRoleAssignments {...f} providers={[]}/> )
 expect(screen.getByRole('alert').textContent).toContain('没有可用供应商')
 rerender(<ClaudeRoleAssignments {...f} client={null}/> )
 expect(screen.getByRole('alert').textContent).toContain('未连接')
 expect(f.client.request).not.toHaveBeenCalled()
})
it('applies clean external preference updates but never overwrites an edited draft',async()=>{
 const f=fixture();const {rerender}=render(<ClaudeRoleAssignments {...f}/> )
 const updated={...f.snapshot,preferences:{...f.snapshot.preferences,claudeRoleAssignments:{opus:{providerId:'api',model:'a'}}}}
 rerender(<ClaudeRoleAssignments {...f} snapshot={updated}/> )
 expect(screen.getByLabelText('Opus 模型')).toHaveProperty('value','a')
 await userEvent.setup().selectOptions(screen.getByLabelText('Opus 模型'),'b')
 rerender(<ClaudeRoleAssignments {...f} snapshot={{...updated,preferences:{...updated.preferences,compaction:{mode:'follow',effort:'high'}}}}/> )
 expect(screen.getByLabelText('Opus 模型')).toHaveProperty('value','b')
})

it('round2 validates implicit default models in a mixed allocation and allows restoring the old default path', async () => {
 const f = fixture(); const user = userEvent.setup()
 render(<ClaudeRoleAssignments {...f} activeProviderId="api"/>)
 await user.selectOptions(screen.getByLabelText('Opus 供应商'), 'api'); await user.selectOptions(screen.getByLabelText('Opus 模型'), 'a')
 expect(screen.getByLabelText('Sonnet 模型').getAttribute('aria-invalid')).toBe('true')
 expect(screen.getByRole('button', {name:'保存分配'})).toHaveProperty('disabled', true)
 expect(screen.getAllByRole('alert').some(el => el.textContent?.includes('DeepSeek'))).toBe(true)
 await user.click(screen.getByRole('button', {name:'恢复默认'})); expect(screen.queryByRole('alert')).toBeNull()
 // This cleared the unsaved draft back to its baseline; a persisted mixed config
 // must also be able to exit through an explicit empty patch.
})
it('round2 allows clearing persisted mixed assignments even when the default subscription is signed out', async () => {
 const f = fixture({opus:{providerId:'api',model:'a'}})
 const sub:ProviderConfigView={id:'sub',cli:'claude-code',name:'Default subscription',endpoint:'',model:'gpt',hasApiKey:false,modelMapping:{opus:'gpt',sonnet:'gpt',haiku:'gpt'},subscription:{provider:'codex',account:{status:'needs-sign-in'},modelMetadata:[{id:'gpt'}]}}
 render(<ClaudeRoleAssignments {...f} providers={[...f.providers,sub]} activeProviderId="sub"/> )
 expect(screen.getByLabelText('Haiku 模型').getAttribute('aria-invalid')).toBe('true')
 const user=userEvent.setup();await user.click(screen.getByRole('button',{name:'恢复默认'}));await user.click(screen.getByRole('button',{name:'保存分配'}))
 expect(f.client.request).toHaveBeenLastCalledWith('agent-model-settings.patch-assignments',{assignments:{}})
})

it('round2 keeps fully explicit targets usable with an unused official source and accepts unknown API catalogs', async () => {
 const f=fixture({opus:{providerId:'api',model:'a'},sonnet:{providerId:'api',model:'b'},haiku:{providerId:'api',model:'a'}})
 const user=userEvent.setup();render(<ClaudeRoleAssignments {...f}/>)
 expect(screen.queryByRole('alert')).toBeNull();await user.selectOptions(screen.getByLabelText('Haiku 模型'),'b')
 expect(screen.getByRole('button',{name:'保存分配'})).toHaveProperty('disabled',false)
})

it('round2 does not treat an unknown API catalog as deletion for inherited default models', async () => {
 const f=fixture({opus:{providerId:'api',model:'a'}})
 const {modelMetadata,...unknown}=api
 const user=userEvent.setup();render(<ClaudeRoleAssignments {...f} providers={[official,unknown]} activeProviderId="api"/> )
 expect(screen.queryByRole('alert')).toBeNull()
 expect(screen.getByText(/最终：DeepSeek.*Native Haiku/)).toBeTruthy()
 await user.selectOptions(screen.getByLabelText('Opus 模型'),'b')
 expect(screen.getByRole('button',{name:'保存分配'})).toHaveProperty('disabled',false)
})

it('round2 checks a missing key on an edited built-in API default without inspecting official OAuth', () => {
 const f=fixture({opus:{providerId:'official',model:'opus'}})
 render(<ClaudeRoleAssignments {...f} providers={[official,{...api,builtIn:true,hasApiKey:false}]} activeProviderId="api"/> )
 expect(screen.getByLabelText('Opus 模型').getAttribute('aria-invalid')).toBe('false')
 expect(screen.getByLabelText('Sonnet 模型').getAttribute('aria-invalid')).toBe('true')
 expect(screen.getAllByRole('alert').every(el=>el.textContent?.includes('API Key'))).toBe(true)
})
