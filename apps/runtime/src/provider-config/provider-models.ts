import { PROVIDER_PRESETS, type ProviderModelMetadata, type ProviderModelsInput, type ProviderModelsResult } from '@matou/contracts'

const MAX_MODELS = 1500

function modelText(value: unknown): string | undefined {
  return typeof value === 'string' && value.length <= 240 && value.trim() && !/\p{Cc}/u.test(value) ? value : undefined
}

function positiveTokenCount(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isSafeInteger(value) && value > 0 ? value : undefined
}

function supportedEffort(value: unknown): value is NonNullable<ProviderModelMetadata['supportedEfforts']>[number] {
  return value === 'low' || value === 'medium' || value === 'high' || value === 'xhigh' || value === 'max'
}

function normalizeModel(value: unknown): ProviderModelMetadata | undefined {
  if (!value || typeof value !== 'object') return undefined
  const row = value as Record<string, unknown>
  const id = modelText(row.id)
  if (!id) return undefined
  const displayName = modelText(row.displayName)
  const contextWindow = positiveTokenCount(row.contextWindow)
  const maxContextWindow = positiveTokenCount(row.maxContextWindow)
  const maxOutputTokens = positiveTokenCount(row.maxOutputTokens)
  const efforts = Array.isArray(row.supportedEfforts) ? [...new Set(row.supportedEfforts.filter(supportedEffort))] : []
  return {
    id,
    ...(displayName !== undefined ? { displayName } : {}),
    ...(contextWindow !== undefined ? { contextWindow } : {}),
    ...(maxContextWindow !== undefined ? { maxContextWindow } : {}),
    ...(maxOutputTokens !== undefined ? { maxOutputTokens } : {}),
    ...(efforts.length ? { supportedEfforts: efforts } : {})
  }
}

function hasDetails(model: ProviderModelMetadata): boolean {
  return model.displayName !== undefined || model.contextWindow !== undefined || model.maxContextWindow !== undefined ||
    model.maxOutputTokens !== undefined || model.supportedEfforts !== undefined
}

function normalizeModels(rows: unknown[]): ProviderModelMetadata[] {
  const byId = new Map<string, ProviderModelMetadata>()
  for (const row of rows) {
    const model = normalizeModel(row)
    if (!model) continue
    const existing = byId.get(model.id)
    if (!existing || (!hasDetails(existing) && hasDetails(model))) byId.set(model.id, model)
  }
  return [...byId.keys()].sort().slice(0, MAX_MODELS).map(id => byId.get(id)!)
}

export function normalizeProviderModelMetadata(value: unknown): ProviderModelMetadata[] | undefined {
  return Array.isArray(value) ? normalizeModels(value) : undefined
}

function readUpstreamModel(row: unknown): unknown {
  if (!row || typeof row !== 'object') return undefined
  const model = row as Record<string, unknown>
  const effort = model.effort as { supported_levels?: unknown } | undefined
  return {
    id: model.id,
    displayName: modelText(model.name) ?? model.display_name,
    contextWindow: model.context_window,
    maxContextWindow: model.max_context_window,
    maxOutputTokens: model.max_output_tokens,
    supportedEfforts: effort?.supported_levels
  }
}

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
    const metadata = normalizeModels(body.data.map(readUpstreamModel))
    if (!metadata.length) return { error: 'empty' }
    const models = metadata.map(model => model.id)
    return metadata.some(hasDetails) ? { models, modelMetadata: metadata } : { models }
  } catch (error) {
    return { error: error instanceof Error && ['TimeoutError', 'AbortError'].includes(error.name) ? 'timeout' : 'network' }
  }
}
