import { spawn } from 'node:child_process'
import { mkdtemp, rm, readFile } from 'node:fs/promises'
import { tmpdir, homedir } from 'node:os'
import { join } from 'node:path'
import { createInterface } from 'node:readline'
import type { SubscriptionModel } from '@matou/contracts'
import { parseChatGptModels } from '../model-router/account-readers'
import { resolveProviderCommandEnvironment } from './provider-command-environment'

let cached: Promise<SubscriptionModel[]> | undefined
let cachedAt = 0
/** Read native client metadata in an isolated, offline process; never submit a user/model request. */
export function nativeClaudeModels(): Promise<SubscriptionModel[]> {
  if (cached && Date.now() - cachedAt < 60_000) return cached
  cachedAt = Date.now()
  cached = probe().catch(() => [])
  return cached
}
async function probe(): Promise<SubscriptionModel[]> {
  const root = await mkdtemp(join(tmpdir(), 'matou-model-catalog-'))
  const executable = process.env.MATOU_CLAUDE_COMMAND || 'claude'
  const env = { ...process.env, ...await resolveProviderCommandEnvironment(executable, process.env) }
  for (const key of Object.keys(env)) if (/^(ANTHROPIC_|CLAUDE_|MATOU_)/.test(key)) delete env[key]
  Object.assign(env, { CLAUDE_CONFIG_DIR: root, ANTHROPIC_API_KEY: 'LOCAL_METADATA_ONLY', ANTHROPIC_BASE_URL: 'http://127.0.0.1:1', CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: '1' })
  const child = spawn(executable, ['-p', '--input-format', 'stream-json', '--output-format', 'stream-json', '--verbose', '--setting-sources', 'user', '--strict-mcp-config', '--mcp-config', '{"mcpServers":{}}', '--tools', ''], { cwd: root, env, stdio: ['pipe', 'pipe', 'ignore'] })
  const lines = createInterface({ input: child.stdout })
  let serial = 0
  const pending = new Map<string, { resolve(value: any): void; reject(error: Error): void }>()
  const fail = () => { for (const item of pending.values()) item.reject(new Error('Native metadata unavailable')); pending.clear() }
  const timer = setTimeout(() => { fail(); child.kill('SIGKILL') }, 6000)
  child.on('error', fail); child.on('exit', fail); child.stdin.on('error', fail)
  lines.on('line', line => {
    try { const row = JSON.parse(line); const response = row.response; if (row.type === 'control_response' && response) { pending.get(response.request_id)?.resolve(response.subtype === 'success' ? response.response : undefined); pending.delete(response.request_id) } } catch { /* Non-protocol output is not metadata. */ }
  })
  const request = (subtype: string, extra = {}): Promise<any> => new Promise((resolve, reject) => {
    const request_id = String(++serial); pending.set(request_id, { resolve, reject })
    child.stdin.write(JSON.stringify({ type: 'control_request', request_id, request: { subtype, ...extra } }) + '\n')
  })
  try {
    const init = await request('initialize')
    const models: SubscriptionModel[] = []
    for (const entry of (init?.models ?? []).slice(0, 12)) {
      if (!entry.resolvedModel || !/^(default|opus|sonnet|haiku|fable)$/.test(entry.value)) continue
      const chosen = await request('set_model', { model: entry.value })
      if (!chosen) continue
      const base = await request('get_context_usage', { detail: 'summary' })
      const sameModel = (value: unknown) => typeof value === 'string' && value.replace(/\[1m\]$/i, '') === entry.resolvedModel.replace(/\[1m\]$/i, '')
      if (!sameModel(base?.model)) continue
      const expanded = await request('set_model', { model: entry.value + '[1m]' })
      const extended = expanded ? await request('get_context_usage', { detail: 'summary' }) : undefined
      const contextWindow = positive(base?.rawMaxTokens), maxContextWindow = sameModel(extended?.model) ? positive(extended?.rawMaxTokens) : undefined
      const model: SubscriptionModel = { id: entry.resolvedModel, displayName: entry.displayName,
        ...(contextWindow ? { contextWindow } : {}), ...(maxContextWindow ? { maxContextWindow } : {}),
        ...(Array.isArray(entry.supportedEffortLevels) ? { supportedEfforts: entry.supportedEffortLevels.filter((v: string) => ['low','medium','high','xhigh','max'].includes(v)) } : {}) }
      const existing = models.findIndex(row => row.id === model.id)
      if (existing < 0) models.push(model)
      else if (maxContextWindow) models[existing] = { ...models[existing]!, maxContextWindow }
      models.push({ ...model, id: entry.value, resolvedModel: entry.resolvedModel, displayName: entry.displayName })
    }
    return models.map(model => model.resolvedModel ? { ...models.find(row => row.id === model.resolvedModel), ...model, ...(model.maxContextWindow ? {} : models.find(row => row.id === model.resolvedModel)?.maxContextWindow ? { maxContextWindow: models.find(row => row.id === model.resolvedModel)!.maxContextWindow! } : {}) } : model)
  } finally {
    clearTimeout(timer); lines.close(); child.kill(); child.stdin.destroy()
    await rm(root, { recursive: true, force: true }).catch(() => undefined)
  }
}
function positive(value: unknown): number | undefined { return typeof value === 'number' && Number.isSafeInteger(value) && value > 0 ? value : undefined }
export async function nativeCodexModels(): Promise<SubscriptionModel[]> {
  try {
    const cache = JSON.parse(await readFile(join(process.env.CODEX_HOME || join(homedir(), '.codex'), 'models_cache.json'), 'utf8'))
    const result = parseChatGptModels(cache)
    return result.ok ? result.value.filter(model => !/image|video|codex-auto-review|embedding|rerank|moderation|(?:^|[-_/])(?:tts|whisper|transcribe)(?:$|[-_/])/i.test(model.id)) : []
  } catch { return [] }
}

export async function nativeCodexDefaultModel(): Promise<string | undefined> {
  try {
    const text = await readFile(join(process.env.CODEX_HOME || join(homedir(), '.codex'), 'config.toml'), 'utf8')
    const root = text.split(/^\s*\[/m)[0] ?? ''
    return /^model\s*=\s*["']([^"']+)["']/m.exec(root)?.[1]
  } catch { return undefined }
}
