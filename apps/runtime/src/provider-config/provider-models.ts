import { PROVIDER_PRESETS, type ProviderModelsInput, type ProviderModelsResult } from '@matou/contracts'

/** Discover against the draft, without saving or activating it. Never follow a
 * redirect with credentials, expose upstream error bodies, or log the request. */
export async function discoverProviderModels(
  input: ProviderModelsInput,
  request: typeof fetch = fetch
): Promise<ProviderModelsResult> {
  let url: URL
  try {
    url = new URL(input.endpoint.trim())
    if (!['https:', 'http:'].includes(url.protocol) || url.username || url.password || url.search || url.hash) return { error: 'invalid' }
    const endpoint = url.toString().replace(/\/$/, '')
    const preset = PROVIDER_PRESETS[input.cli]?.find(p => p.endpoint === endpoint)
    if (preset?.modelsUrl) url = new URL(preset.modelsUrl)
    else url.pathname = `${url.pathname.replace(/\/$/, '')}/models`
    if (!input.apiKey || !/^[\x21-\x7e]+$/.test(input.apiKey)) return { error: 'key-required' }
  } catch { return { error: 'invalid' } }
  try {
    const response = await request(url, {
      headers: {
        Accept: 'application/json', Authorization: `Bearer ${input.apiKey}`,
        ...(input.cli === 'claude-code' ? { 'x-api-key': input.apiKey!, 'anthropic-version': '2023-06-01' } : {})
      },
      redirect: 'error', signal: AbortSignal.timeout(10_000)
    })
    if ([401, 403].includes(response.status)) return { error: 'auth' }
    if ([404, 405].includes(response.status)) return { error: 'unsupported' }
    if (!response.ok || !response.body) return { error: 'network' }
    const reader = response.body.getReader()
    let size = 0, raw = ''
    const decoder = new TextDecoder()
    try {
      for (;;) {
        const { done, value } = await reader.read()
        if (done) break
        size += value.length
        if (size > 512_000) { await reader.cancel(); return { error: 'too-large' } }
        raw += decoder.decode(value, { stream: true })
      }
    } finally { reader.releaseLock() }
    const body = JSON.parse(raw + decoder.decode())
    if (!Array.isArray(body?.data)) return { error: 'unsupported' }
    const models = [...new Set<string>(body.data.flatMap((row: unknown) => {
      const id = row && typeof row === 'object' && 'id' in row ? row.id : undefined
      return typeof id === 'string' && id.trim() && id.length <= 240 && !/[\x00-\x1f]/.test(id) ? [id] : []
    }))].sort().slice(0, 1500)
    return models.length ? { models } : { error: 'empty' }
  } catch (error) {
    return { error: error instanceof Error && ['TimeoutError', 'AbortError'].includes(error.name) ? 'timeout' : 'network' }
  }
}
