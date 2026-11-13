/** Adapted from CC Switch src-tauri/src/services/subscription.rs,
 * services/codex_oauth_models.rs, services/agy_account/quota.rs,
 * services/subscription_agy.rs and services/agy_account/oauth.rs (MIT).
 * Commit: 3c76d3bda670297dd8c27d7d9be9b18a4c9143f5
 * License: ../../../../packages/contracts/third-party/subscription-accounts-reference-LICENSE
 * Pure readers: no API calls, credential access, fallback catalog or fabricated quota.
 */
import { sanitizeProviderReason } from './errors.js';

export const MAX_ACCOUNT_BODY_BYTES = 256 * 1024;
export const MAX_MODEL_BODY_BYTES = 4 * 1024 * 1024;
export type ReadFailure = { ok: false; error: 'invalid-envelope' | 'http-error' | 'invalid-json' | 'response-too-large' | 'unknown-response'; statusCode?: number; reason?: string };
export type ReadResult<T> = { ok: true; value: T } | ReadFailure;
export interface ManagementApiCallResponse { status_code: number; header?: Record<string, unknown>; body: string }
type JsonObject = Record<string, unknown>;
const object = (v: unknown): JsonObject | undefined => v !== null && typeof v === 'object' && !Array.isArray(v) ? v as JsonObject : undefined;
const str = (v: unknown): string | undefined => typeof v === 'string' && v.trim() && v.length <= 4096 ? v.trim() : undefined;
const list = (v: unknown): unknown[] => Array.isArray(v) ? v : [];
const num = (v: unknown): v is number => typeof v === 'number' && Number.isFinite(v);
const positiveInteger = (v: unknown): v is number => num(v) && Number.isSafeInteger(v) && v > 0;
const unknownResponse = (): ReadFailure => ({ ok: false, error: 'unknown-response' });
function byteLength(v: string): number { return new TextEncoder().encode(v).length; }
function bounded(value: unknown, maximum = MAX_ACCOUNT_BODY_BYTES): boolean {
  try { const json = JSON.stringify(value); return json !== undefined && json.length <= maximum && byteLength(json) <= maximum; }
  catch { return false; }
}
function isoSeconds(value: unknown): string | undefined {
  if (!num(value) || !Number.isSafeInteger(value) || value < 0 || value > 8_640_000_000_000) return undefined;
  return new Date(value * 1000).toISOString();
}
function isoTime(value: unknown): string | undefined {
  const time = str(value);
  return time && /^\d{4}-\d\d-\d\dT/.test(time) && Number.isFinite(Date.parse(time)) ? time : undefined;
}

/** The management request's outer HTTP 200 never overrides inner status_code. */
export function unwrapApiCallResponse(value: unknown, maximum = MAX_ACCOUNT_BODY_BYTES): ReadResult<unknown> {
  const envelope = object(value);
  if (!envelope || !num(envelope.status_code) || !Number.isInteger(envelope.status_code) || envelope.status_code < 100 || envelope.status_code > 599 || typeof envelope.body !== 'string') return { ok: false, error: 'invalid-envelope' };
  const statusCode = envelope.status_code;
  if (envelope.body.length > maximum || byteLength(envelope.body) > maximum) return { ok: false, error: 'response-too-large', statusCode };
  let body: unknown;
  try { body = JSON.parse(envelope.body); }
  catch { return { ok: false, error: statusCode < 200 || statusCode >= 300 ? 'http-error' : 'invalid-json', statusCode }; }
  if (statusCode < 200 || statusCode >= 300) {
    const error = object(object(body)?.error);
    const reason = sanitizeProviderReason(error?.message ?? object(body)?.message);
    return { ok: false, error: 'http-error', statusCode, ...(reason ? { reason } : {}) };
  }
  return { ok: true, value: body };
}

/** codex_plan_label: unfamiliar plan names are title-cased verbatim, never assigned a known tier. */
export function parseChatGptPlan(value: unknown): string | null {
  const plan = str(value)?.toLowerCase();
  if (!plan || plan === 'unknown') return null;
  const labels: Record<string, string> = {
    free: 'Free', go: 'Go', plus: 'Plus', prolite: 'Pro 100', pro: 'Pro 200', promax: 'Pro 500',
    team: 'Business', self_serve_business_usage_based: 'Business', self_serve_business_prolite: 'Business Premium',
    edu: 'Edu', edu_plus: 'Edu Plus', edu_pro: 'Edu Pro',
  };
  if (plan === 'business' || plan === 'ent26' || plan.startsWith('enterprise')) return 'Enterprise';
  return Object.hasOwn(labels, plan) ? labels[plan]! : plan.split(/[_\- ]+/).filter(Boolean).map(word => word[0]!.toUpperCase() + word.slice(1)).join(' ');
}
export interface ChatGptQuotaWindow { name: 'five_hour' | 'weekly' | 'unknown'; windowSeconds?: number; usedPercent: number; resetsAt?: string }
export interface ChatGptUsage { plan: string | null; windows: ChatGptQuotaWindow[] }
export function parseChatGptUsage(value: unknown): ReadResult<ChatGptUsage> {
  if (!bounded(value)) return { ok: false, error: 'response-too-large' };
  const body = object(value);
  if (!body) return unknownResponse();
  const rate = object(body.rate_limit), windows: ChatGptQuotaWindow[] = [];
  for (const value of [rate?.primary_window, rate?.secondary_window]) {
    const window = object(value);
    if (!window || !num(window.used_percent) || window.used_percent < 0) continue;
    const seconds = positiveInteger(window.limit_window_seconds) ? window.limit_window_seconds : undefined;
    const reset = window.used_percent === 0 ? undefined : isoSeconds(window.reset_at);
    windows.push({ name: seconds === 18000 ? 'five_hour' : seconds === 604800 ? 'weekly' : 'unknown',
      ...(seconds ? { windowSeconds: seconds } : {}), usedPercent: Math.min(window.used_percent, 100), ...(reset ? { resetsAt: reset } : {}) });
  }
  windows.sort((a, b) => (a.windowSeconds ?? Infinity) - (b.windowSeconds ?? Infinity));
  const plan = parseChatGptPlan(body.plan_type);
  if (!windows.length && !str(body.plan_type)) return unknownResponse();
  return { ok: true, value: { plan, windows } };
}
export function readChatGptUsage(response: unknown): ReadResult<ChatGptUsage> {
  const body = unwrapApiCallResponse(response);
  return body.ok ? parseChatGptUsage(body.value) : body;
}

export interface AccountModel { id: string; displayName?: string; ownedBy?: string; contextWindow?: number; maxOutputTokens?: number }
function field(obj: JsonObject, keys: string[]): string | undefined {
  return keys.map(k => str(obj[k])).find(Boolean);
}
export function parseChatGptModels(value: unknown): ReadResult<AccountModel[]> {
  if (!bounded(value, MAX_MODEL_BODY_BYTES)) return { ok: false, error: 'response-too-large' };
  const root = object(value);
  if (root && ('error' in root || root.status === 'error')) return unknownResponse();
  const entries = [root?.data, root?.models, root?.items, value].find(Array.isArray);
  const models = new Map<string, AccountModel>();
  const push = (entry: unknown, fallback?: string) => {
    const obj = object(entry);
    const id = str(typeof entry === 'string' ? entry : undefined) ?? (obj ? field(obj, ['slug', 'id', 'model', 'name']) : undefined) ?? str(fallback);
    if (!id || models.has(id)) return;
    const displayName = obj && field(obj, ['display_name', 'displayName', 'name']);
    const ownedBy = (obj && field(obj, ['owned_by', 'ownedBy', 'provider', 'vendor', 'category', 'owner'])) || 'Codex';
    const context = obj?.context_window ?? obj?.contextWindow ?? obj?.context_length;
    const output = obj?.max_output_tokens ?? obj?.maxOutputTokens ?? obj?.max_completion_tokens;
    models.set(id, { id, ownedBy, ...(displayName ? { displayName } : {}),
      ...(positiveInteger(context) ? { contextWindow: context } : {}), ...(positiveInteger(output) ? { maxOutputTokens: output } : {}) });
  };
  if (entries) for (const entry of entries) push(entry);
  // CC's models-map shape, plus a direct model-id map from a management caller.
  const map = object(root?.models) ?? (!entries && root && !['data', 'items', 'models'].some(k => k in root) ? root : undefined);
  if (map) for (const [key, entry] of Object.entries(map)) push(entry, key);
  if (!models.size) return unknownResponse();
  return { ok: true, value: [...models.values()].sort((a, b) => a.id < b.id ? -1 : a.id > b.id ? 1 : 0) };
}
export function readChatGptModels(response: unknown): ReadResult<AccountModel[]> {
  const body = unwrapApiCallResponse(response, MAX_MODEL_BODY_BYTES);
  return body.ok ? parseChatGptModels(body.value) : body;
}

export interface AntigravityAccount { project: string | null; plan: string | null; eligibility: 'eligible' | 'ineligible' | 'unknown'; reason?: string }
function ultraVariant(name: string, id: unknown): string {
  const variant = id === 'g1-ultra-lite-tier' ? '5x' : id === 'g1-ultra-tier' ? '20x' : undefined;
  return variant && /ultra/i.test(name) && !/(?:5x|20x)/i.test(name) ? `${name} ${variant}` : name;
}
export function parseAntigravityEligibility(value: unknown): { eligibility: AntigravityAccount['eligibility']; reason?: string } {
  const body = object(value);
  if (!body || !bounded(value)) return { eligibility: 'unknown' };
  const current = object(body.currentTier), hasCurrent = !!str(current?.id);
  const allowed = list(body.allowedTiers).map(object), ineligible = list(body.ineligibleTiers).map(object);
  const hasFree = current?.id === 'free-tier' || allowed.some(t => t?.id === 'free-tier');
  const freeReason = body.gcpManaged !== true && !hasFree ? ineligible.find(t => t?.tierId === 'free-tier' && str(t.reasonMessage))?.reasonMessage : undefined;
  const reason = sanitizeProviderReason(freeReason ?? (!hasCurrent ? ineligible.find(t => str(t?.reasonMessage))?.reasonMessage : undefined));
  return reason ? { eligibility: 'ineligible', reason } : { eligibility: hasCurrent || hasFree ? 'eligible' : 'unknown' };
}
export function parseAntigravityAccount(value: unknown): ReadResult<AntigravityAccount> {
  if (!bounded(value)) return { ok: false, error: 'response-too-large' };
  const body = object(value);
  if (!body) return unknownResponse();
  const current = object(body.currentTier), paid = object(body.paidTier);
  const paidName = str(paid?.name), currentName = str(current?.name);
  const plan = paidName ? ultraVariant(paidName, paid?.id) : current?.id === 'free-tier' ? 'Free' : current?.id === 'standard-tier' ? 'Paid' : currentName ? ultraVariant(currentName, current?.id) : null;
  const projectValue = body.cloudaicompanionProject, projectObj = object(projectValue);
  const project = str(projectValue) ?? str(projectObj?.id) ?? str(projectObj?.projectId) ?? null;
  const eligibility = parseAntigravityEligibility(body);
  if (!project && !plan && eligibility.eligibility === 'unknown') return unknownResponse();
  return { ok: true, value: { project, plan, ...eligibility } };
}
export function readAntigravityAccount(response: unknown): ReadResult<AntigravityAccount> {
  const body = unwrapApiCallResponse(response);
  return body.ok ? parseAntigravityAccount(body.value) : body;
}
export interface AntigravityQuotaTier { name: string; group: 'gemini' | 'gemini-argon' | 'claude-gpt'; window: 'five_hour' | 'weekly'; remainingFraction: number; usedPercent: number; resetsAt?: string }
const bucketSpecs = [
  ['gemini-5h', 'agy_gemini_five_hour', 'gemini', 'five_hour'],
  ['gemini-weekly', 'agy_gemini_weekly', 'gemini', 'weekly'],
  ['gemini-argon-wtu-weekly', 'agy_gemini_argon_weekly', 'gemini-argon', 'weekly'],
  ['3p-5h', 'agy_third_party_five_hour', 'claude-gpt', 'five_hour'],
  ['3p-weekly', 'agy_third_party_weekly', 'claude-gpt', 'weekly'],
] as const;
export function parseAntigravityQuota(value: unknown): ReadResult<AntigravityQuotaTier[]> {
  if (!bounded(value)) return { ok: false, error: 'response-too-large' };
  const buckets = list(object(value)?.groups).flatMap(g => list(object(g)?.buckets)).map(object);
  const tiers: AntigravityQuotaTier[] = [];
  for (const [id, name, group, window] of bucketSpecs) {
    const bucket = buckets.find(b => b?.bucketId === id && num(b.remainingFraction));
    if (!bucket || !num(bucket.remainingFraction)) continue;
    const remainingFraction = Math.max(0, Math.min(1, bucket.remainingFraction));
    const reset = isoTime(bucket.resetTime);
    tiers.push({ name, group, window, remainingFraction, usedPercent: (1 - remainingFraction) * 100, ...(reset ? { resetsAt: reset } : {}) });
  }
  return tiers.length ? { ok: true, value: tiers } : unknownResponse();
}
export function readAntigravityQuota(response: unknown): ReadResult<AntigravityQuotaTier[]> {
  const body = unwrapApiCallResponse(response);
  return body.ok ? parseAntigravityQuota(body.value) : body;
}

/** Deliberate whitelist. No auth_index, file path, token, metadata object or raw body escapes. */
export interface CredentialRefreshError { code?: string; message?: string; http_status?: number }
export interface CredentialSummary { email?: string; accountId?: string; plan?: string; status?: string; requires_sign_in?: boolean; renewal_state?: string; refresh_error?: string | CredentialRefreshError }
export function parseCredentialSummary(value: unknown): ReadResult<CredentialSummary> {
  if (!bounded(value)) return { ok: false, error: 'response-too-large' };
  const body = object(value);
  if (!body) return unknownResponse();
  const summary: CredentialSummary = {};
  // v8 id_token is a parsed claims object here, not a token string. Read only these two claims.
  const claims = object(body.id_token);
  for (const [target, keys] of [
    ['email', ['email']], ['accountId', ['account_id', 'accountId', 'chatgpt_account_id']],
    ['plan', ['plan_type', 'plan']], ['status', ['status']], ['renewal_state', ['renewal_state']],
  ] as const) {
    const v = field(body, [...keys]);
    const safe = sanitizeProviderReason(v);
    if (safe) summary[target] = safe;
  }
  if (!summary.accountId) {
    const id = sanitizeProviderReason(claims?.chatgpt_account_id);
    if (id) summary.accountId = id;
  }
  if (!summary.plan) {
    const plan = sanitizeProviderReason(claims?.plan_type);
    if (plan) summary.plan = plan;
  }
  if (typeof body.requires_sign_in === 'boolean') summary.requires_sign_in = body.requires_sign_in;
  const refresh = object(body.refresh_error);
  if (refresh) {
    const safe: CredentialRefreshError = {};
    const code = sanitizeProviderReason(refresh.code), message = sanitizeProviderReason(refresh.message);
    if (code) safe.code = code;
    if (message) safe.message = message;
    if (positiveInteger(refresh.http_status) && refresh.http_status >= 100 && refresh.http_status <= 599) safe.http_status = refresh.http_status;
    if (Object.keys(safe).length) summary.refresh_error = safe;
  } else {
    const message = sanitizeProviderReason(body.refresh_error);
    if (message) summary.refresh_error = message;
  }
  return Object.keys(summary).length ? { ok: true, value: summary } : unknownResponse();
}
