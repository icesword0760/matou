/** Adapted from CC Switch src-tauri/src/proxy/error_mapper.rs and
 * src-tauri/src/proxy/codex_quota_protection.rs:Snapshot::from_quota/from_headers (MIT).
 * Commit: 3c76d3bda670297dd8c27d7d9be9b18a4c9143f5
 * License: ../../../../packages/contracts/third-party/subscription-accounts-reference-LICENSE
 * Classification/reading only: no thresholds, admission control, cache or automatic pause.
 */
export type AccountFailureStatus = 'needs-sign-in' | 'account-unavailable' | 'router-not-running' |
  'router-start-failed' | 'login-port-busy' | 'login-incomplete' | 'quota-exhausted';
export interface LocalizedText { zh: string; en: string }
export interface MappedFailure {
  status: AccountFailureStatus | null;
  /** null leaves the existing card status text unchanged (in particular quota exhaustion). */
  message: LocalizedText | null;
  reason?: string;
  reasonLocalized?: LocalizedText;
  recovery: 'sign-in' | 'switch-to-official' | 'retry' | 'finish-or-close-login' | 'none';
  recoveryMessage?: LocalizedText;
  quotaUsedPercent?: 100;
}
const messages: Record<Exclude<AccountFailureStatus, 'quota-exhausted'>, LocalizedText> = {
  'needs-sign-in': { zh: '需要重新登录', en: 'Sign in again' },
  'account-unavailable': { zh: '账号不可用', en: 'Account unavailable' },
  'router-not-running': { zh: '模型路由未运行', en: 'Model router is not running' },
  'router-start-failed': { zh: '模型路由未能启动', en: 'Model router failed to start' },
  'login-port-busy': { zh: '另一个程序正在占用登录端口，请先完成或关闭它', en: 'Another program is using the sign-in port. Finish or close it first.' },
  'login-incomplete': { zh: '登录未完成', en: 'Sign-in was not completed' },
};

/** Only call with the provider's message field, never a whole response or credential object. */
export function sanitizeProviderReason(value: unknown): string | undefined {
  if (typeof value !== 'string' || !value.trim()) return undefined;
  return value.slice(0, 8192)
    .replace(/https?:\/\/[^\s<>"']+/gi, '[redacted URL]')
    .replace(/\bBearer\s+[^\s,;"']+/gi, 'Bearer [redacted]')
    .replace(/(["']?(?:access[_-]?token|refresh[_-]?token|id[_-]?token|api[_-]?key|client[_-]?(?:key|secret)|authorization|password|code|state)["']?\s*[:=]\s*)(?:"[^"\r\n]*"|'[^'\r\n]*'|[^\s,;}]+)/gi, '$1[redacted]')
    .replace(/\b(?:sk-[\w-]+|eyJ[\w-]+\.[\w-]+\.[\w-]+)\b/g, '[redacted]')
    .replace(/[^\s]{80,}/g, '[redacted]')
    .replace(/[\x00-\x1f\x7f]/g, ' ').trim().slice(0, 512);
}
function failure(status: AccountFailureStatus, reason?: unknown): MappedFailure {
  if (status === 'quota-exhausted') return { status, message: null, recovery: 'none', quotaUsedPercent: 100 };
  const recovery = status === 'needs-sign-in' ? 'sign-in' : status === 'account-unavailable' ? 'switch-to-official' : status === 'login-port-busy' ? 'finish-or-close-login' : 'retry';
  const safe = sanitizeProviderReason(reason);
  return { status, message: { ...messages[status] }, recovery, ...(safe ? { reason: safe } : {}),
    ...(status === 'account-unavailable' ? { recoveryMessage: { zh: '可切回 Anthropic 官方', en: 'You can switch back to Anthropic official' } } : {}) };
}
export interface RouterFailureInput {
  phase: 'start' | 'running' | 'login';
  kind: 'spawn-error' | 'early-exit' | 'ready-timeout' | 'not-running' | 'port-busy' | 'cancelled' | 'timeout' | 'oauth-error';
  reason?: unknown;
}
export function mapRouterFailure(input: RouterFailureInput): MappedFailure {
  if (!input || !['start', 'running', 'login'].includes(input.phase) || !['spawn-error', 'early-exit', 'ready-timeout', 'not-running', 'port-busy', 'cancelled', 'timeout', 'oauth-error'].includes(input.kind)) return { status: null, message: null, recovery: 'none' };
  if (input.phase === 'login') return failure(input.kind === 'port-busy' ? 'login-port-busy' : 'login-incomplete', input.reason);
  return failure(input.phase === 'start' ? 'router-start-failed' : 'router-not-running', input.reason);
}
export type SubscriptionLaunchFailureKind = 'needs-sign-in' | 'models-not-ready' | 'unsupported-context-window' | 'router-start-failed';
/** Activation copy is controlled by Matou. Engine diagnostics only belong in
 * redacted logs, never in user-facing reasons or renderer error payloads.
 */
export function mapSubscriptionLaunchFailure(kind: SubscriptionLaunchFailureKind, diagnostic?: unknown): MappedFailure {
  if (kind === 'needs-sign-in') return failure('needs-sign-in');
  let reasonLocalized: LocalizedText;
  if (kind === 'models-not-ready') reasonLocalized = { zh: '账号的模型还没有准备好，请稍后重试', en: "The account's models are not ready yet. Try again shortly." };
  else if (kind === 'unsupported-context-window') reasonLocalized = { zh: '所选模型的上下文窗口小于 100K，Claude Code 无法使用。请编辑这张卡片，换一个模型。', en: "The selected model's context window is under 100K, which Claude Code can't use. Edit this card and choose another model." };
  else {
    const safe = sanitizeProviderReason(diagnostic) ?? '';
    reasonLocalized = /ENOENT|executable.*(?:missing|not found)|binary.*(?:missing|not found)/i.test(safe)
      ? { zh: '找不到本地模型路由文件，请检查安装后重试。', en: 'The local model router file is missing. Check the installation and try again.' }
      : /EACCES|EPERM|permission denied/i.test(safe)
        ? { zh: '本地模型路由无法运行，请检查安装权限后重试。', en: 'The local model router cannot run. Check installation permissions and try again.' }
        : /EADDRINUSE|address already in use/i.test(safe)
          ? { zh: '本地端口被占用，请稍后重试。', en: 'The local port is in use. Try again shortly.' }
          : /timeout|timed out|deadline|readiness/i.test(safe)
            ? { zh: '本地模型路由未能及时就绪，请重试。', en: 'The local model router did not become ready in time. Try again.' }
            : /ECONN|fetch failed|network|socket/i.test(safe)
              ? { zh: '暂时无法连接本地模型路由，请重试。', en: 'The local model router could not be reached. Try again.' }
              : { zh: '本地模型路由暂时无法使用，请重试。', en: 'The local model router is temporarily unavailable. Try again.' };
  }
  return { ...failure('router-start-failed'), reasonLocalized };
}

/** Sign-in errors are UI copy, not a transport for engine/browser diagnostics. */
export function mapSubscriptionLoginFailure(diagnostic?: unknown, outcome?: 'cancelled' | 'expired'): LocalizedText {
  const safe = sanitizeProviderReason(diagnostic) ?? '';
  if (outcome === 'cancelled') return { zh: '登录已取消。', en: 'Sign-in was cancelled.' };
  if (outcome === 'expired') return { zh: '登录已超时，请重试。', en: 'Sign-in timed out. Try again.' };
  if (/EADDRINUSE|sign-in port|登录端口/i.test(safe)) return { ...messages['login-port-busy'] };
  if (/model router|模型路由|supervisor/i.test(safe)) {
    const mapped = mapSubscriptionLaunchFailure('router-start-failed', safe), reason = mapped.reasonLocalized!;
    return { zh: `${mapped.message!.zh}：${reason.zh}`, en: `${mapped.message!.en}: ${reason.en}` };
  }
  if (/timed?\s*out|timeout|超时/i.test(safe)) return { zh: '登录已超时，请重试。', en: 'Sign-in timed out. Try again.' };
  if (/already in progress|another window/i.test(safe)) return { zh: '另一个窗口正在登录，请先完成或取消。', en: 'Another window is signing in. Finish or cancel it first.' };
  if (/account details|credential.*adopt/i.test(safe)) return { zh: '无法读取登录账号信息，请重试。', en: 'The signed-in account details could not be read. Try again.' };
  return { zh: '登录未完成，请重试。', en: 'Sign-in was not completed. Try again.' };
}
export interface AccountStatusInput {
  requires_sign_in?: unknown;
  renewal_state?: unknown;
  refresh_error?: unknown;
  /** Set only after the manager's single controlled refresh has actually failed. */
  refreshFailed?: boolean;
  httpStatus?: number;
  source?: 'quota' | 'models' | 'inference' | 'credential' | 'eligibility';
  /** Structured provider credential/eligibility evidence; a lone HTTP 403 is not evidence. */
  accountRejected?: boolean;
  ineligibleReason?: unknown;
  reason?: unknown;
  quotaExhausted?: boolean;
}
export function mapAccountStatus(input: AccountStatusInput): MappedFailure {
  if (!input || typeof input !== 'object') return { status: null, message: null, recovery: 'none' };
  // Fixed v8 credentials expose refresh_error as {code,message,http_status}; a transient
  // retry_scheduled error is not evidence that the user must sign in again.
  const refresh = input.refresh_error && typeof input.refresh_error === 'object' && !Array.isArray(input.refresh_error)
    ? input.refresh_error as Record<string, unknown> : undefined;
  const refreshReason = refresh?.message ?? (typeof input.refresh_error === 'string' ? input.refresh_error : undefined);
  const terminalRefresh = refresh?.http_status === 401 ||
    [refresh?.code, refreshReason].some(v => typeof v === 'string' && /\b(?:invalid[_ -]grant|invalid[_ -](?:refresh[_ -])?token|refresh[_ -]token[_ -](?:revoked|expired)|sign[_ -]in[_ -]required)\b/i.test(v));
  if (input.requires_sign_in === true || input.refreshFailed === true || terminalRefresh ||
      (typeof input.renewal_state === 'string' && ['failed', 'requires_sign_in', 'needs-sign-in', 'sign_in_required', 'expired_sign_in_required', 'error'].includes(input.renewal_state.toLowerCase()))) return failure('needs-sign-in', refreshReason ?? input.reason);
  if (input.accountRejected === true || (input.source === 'eligibility' && sanitizeProviderReason(input.ineligibleReason))) {
    return failure('account-unavailable', input.ineligibleReason ?? input.reason);
  }
  if (input.quotaExhausted === true) return failure('quota-exhausted');
  return { status: null, message: null, recovery: 'none' };
}

export interface QuotaWindowSnapshot { name: 'five_hour' | 'weekly'; usedPercent: number; resetsAt?: string }
const validUsed = (n: unknown): n is number => typeof n === 'number' && Number.isFinite(n) && n >= 0;
function isoSeconds(seconds: number): string | undefined {
  if (!Number.isSafeInteger(seconds) || seconds < 0 || seconds > 8_640_000_000_000) return undefined;
  return new Date(seconds * 1000).toISOString();
}
export function quotaSnapshotFromQuota(tiers: readonly { name: string; usedPercent: number; resetsAt?: string }[]): QuotaWindowSnapshot[] {
  const result = new Map<string, QuotaWindowSnapshot>();
  if (!Array.isArray(tiers)) return [];
  for (const tier of tiers) {
    if (!tier || typeof tier !== 'object') continue;
    const name = tier.name === 'five_hour' ? 'five_hour' : ['seven_day', 'weekly', 'weekly_limit'].includes(tier.name) ? 'weekly' : undefined;
    if (!name || !validUsed(tier.usedPercent)) continue;
    const reset = tier.usedPercent > 0 && tier.resetsAt && Number.isFinite(Date.parse(tier.resetsAt)) ? tier.resetsAt : undefined;
    result.set(name, { name, usedPercent: Math.min(tier.usedPercent, 100), ...(reset ? { resetsAt: reset } : {}) });
  }
  return [...result.values()];
}
/** nowSeconds is injected so reading reset-after-seconds remains deterministic. */
export function quotaSnapshotFromHeaders(headers: Record<string, unknown>, nowSeconds: number): QuotaWindowSnapshot[] {
  if (!headers || typeof headers !== 'object' || Array.isArray(headers)) return [];
  const normalized = new Map(Object.entries(headers).map(([key, value]) => [key.toLowerCase(), value]));
  const get = (key: string) => { const v = normalized.get(key); return typeof v === 'string' && v.trim() ? v.trim() : undefined; };
  const numeric = (key: string) => { const v = get(key); return v !== undefined && Number.isFinite(Number(v)) ? Number(v) : undefined; };
  const active = get('x-codex-active-limit');
  for (const prefix of [...(active && active !== 'codex' && /^[a-z0-9-]{1,64}$/i.test(active) ? [active] : []), 'codex']) {
    let reported = false;
    const result: QuotaWindowSnapshot[] = [];
    for (const slot of ['primary', 'secondary']) {
      const key = (field: string) => `x-${prefix}-${slot}-${field}`;
      const used = numeric(key('used-percent'));
      if (used === undefined) continue;
      reported = true;
      const minutes = numeric(key('window-minutes'));
      const name = minutes === 300 ? 'five_hour' : minutes === 10080 ? 'weekly' : undefined;
      if (!name || !validUsed(used)) continue;
      const at = numeric(key('reset-at')), after = numeric(key('reset-after-seconds'));
      const reset = used > 0 ? isoSeconds(at ?? (after !== undefined && after >= 0 ? nowSeconds + after : NaN)) : undefined;
      result.push({ name, usedPercent: Math.min(used, 100), ...(reset ? { resetsAt: reset } : {}) });
    }
    if (reported) return result;
  }
  return [];
}
