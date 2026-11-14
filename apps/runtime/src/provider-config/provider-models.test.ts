import { readFile } from 'node:fs/promises'
import { describe, expect, it, vi } from 'vitest'
import { discoverProviderModels } from './provider-models'
const input = { cli: 'claude-code' as const, endpoint:'https://api.deepseek.com/anthropic', apiKey:'TOKEN' }
describe('provider model discovery', () => {
  it('uses the pinned models URL, never follows redirects and filters invalid models', async () => {
    const request = vi.fn(async (_url: URL | RequestInfo, _options?: RequestInit) => new Response(JSON.stringify({data:[{id:'b'},{id:'a'},{id:'b'},{id:7},{id:'bad\nmodel'}]})))
    expect(await discoverProviderModels(input,request)).toEqual({models:['a','b']})
    expect(String(request.mock.calls[0]?.[0])).toBe('https://api.deepseek.com/models')
    expect(request).toHaveBeenCalledWith(expect.any(URL),expect.objectContaining({redirect:'error',headers:expect.objectContaining({Authorization:'Bearer TOKEN'})}))
  })
  it('retains the fields returned by the DeepSeek models endpoint', async () => {
    const body = await readFile(new URL('../../../../docs/acceptance/evidence/provider-model-metadata-2026-11-14/deepseek-models-response.json', import.meta.url), 'utf8')
    expect(await discoverProviderModels(input, async () => new Response(body))).toEqual({
      models: ['deepseek-flash', 'deepseek-v4-pro'],
      modelMetadata: [
        { id: 'deepseek-flash', displayName: 'DeepSeek-V4.1-Flash', contextWindow: 1048576, maxOutputTokens: 393216, supportedEfforts: ['low', 'high', 'max'] },
        { id: 'deepseek-v4-pro', displayName: 'DeepSeek-V4-Pro', contextWindow: 1048576, maxOutputTokens: 393216, supportedEfforts: ['low', 'high', 'max'] }
      ]
    })
  })
  it('aligns deduplicated metadata with model IDs and ignores unsupported fields', async () => {
    const body = { data: [
      { id: 'z' },
      { id: 'token-only', name: 'bad\u0000label', context_window: '1048576', max_context_window: 0, max_output_tokens: 2048 },
      { id: 'z', display_name: 'Readable name', max_context_window: 1_000_000, effort: { supported_levels: ['follow', 'low', 'low', 'max', 'unavailable', 1] } },
      { id: 'unknown', name: 'x'.repeat(241), context_window: 1024.5, max_context_window: Number.MAX_SAFE_INTEGER + 1, max_output_tokens: -1, effort: { supported_levels: ['follow'] } },
      { id: 'invalid\u007f', name: 'Ignored' }
    ] }
    expect(await discoverProviderModels(input, async () => new Response(JSON.stringify(body)))).toEqual({
      models: ['token-only', 'unknown', 'z'],
      modelMetadata: [
        { id: 'token-only', maxOutputTokens: 2048 },
        { id: 'unknown' },
        { id: 'z', displayName: 'Readable name', maxContextWindow: 1_000_000, supportedEfforts: ['low', 'max'] }
      ]
    })
  })
  it.each([[401,'auth'],[403,'auth'],[404,'unsupported'],[405,'unsupported'],[500,'network']] as const)('maps status %i without leaking bodies',async(status,error) => {
    expect(await discoverProviderModels(input,async()=>new Response('TOKEN private upstream detail',{status}))).toEqual({error})
  })
  it('does not use a preset models URL after the endpoint changes',async()=>{
    const request = vi.fn(async(_url: URL | RequestInfo, _options?: RequestInit)=>new Response('{"data":[]}'))
    expect(await discoverProviderModels({...input,endpoint:'https://custom.example/v1'},request)).toEqual({error:'empty'})
    expect(String(request.mock.calls[0]?.[0])).toBe('https://custom.example/v1/models')
  })
  it.each(['https://user:password@example.com','file:///tmp/a','https://example.com?key=TOKEN','not-a-url'])('rejects invalid endpoint %s before networking',async(endpoint)=>{
    const request=vi.fn(); expect(await discoverProviderModels({...input,endpoint},request)).toEqual({error:'invalid'}); expect(request).not.toHaveBeenCalled()
  })
  it('maps timeouts and oversized responses',async()=>{
    expect(await discoverProviderModels(input,async()=>{throw new DOMException('private','TimeoutError')})).toEqual({error:'timeout'})
    expect(await discoverProviderModels(input,async()=>new Response('x'.repeat(513_000)))).toEqual({error:'too-large'})
  })
})
