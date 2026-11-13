/** CC Switch proxy/error_mapper.rs and codex_quota_protection.rs fixtures (MIT).
 * Commit: 3c76d3bda670297dd8c27d7d9be9b18a4c9143f5
 * License: ../../../../packages/contracts/third-party/subscription-accounts-reference-LICENSE
 */
import { describe, expect, it } from 'vitest';
import { mapAccountStatus, mapRouterFailure, mapSubscriptionLaunchFailure, mapSubscriptionLoginFailure, quotaSnapshotFromHeaders, quotaSnapshotFromQuota, sanitizeProviderReason } from './errors.js';
describe('subscription error mapping', () => {
  it('keeps sign-in copy bilingual and excludes raw diagnostics, secrets and callback URLs', () => {
    const startup = mapSubscriptionLoginFailure('Invalid model router directory access_token=private-value https://example.invalid/callback?code=private');
    expect(startup.zh).toContain('模型路由未能启动');
    expect(startup.en).toContain('Model router failed to start');
    expect(JSON.stringify(startup)).not.toMatch(/Invalid|private|https:|access_token/);
    expect(mapSubscriptionLoginFailure('Another program is using the sign-in port.').zh).toContain('登录端口');
    expect(mapSubscriptionLoginFailure(undefined, 'cancelled').en).toContain('cancelled');
    expect(mapSubscriptionLoginFailure(undefined, 'expired').zh).toContain('超时');
    expect(mapSubscriptionLoginFailure('Unexpected raw browser diagnostic').zh).toBe('登录未完成，请重试。');
  });
  it('maps startup, runtime exit, callback conflict and incomplete sign-in to exact bilingual copy', () => {
    expect(mapRouterFailure({ phase: 'start', kind: 'ready-timeout' }).message).toEqual({ zh: '模型路由未能启动', en: 'Model router failed to start' });
    expect(mapRouterFailure({ phase: 'running', kind: 'early-exit' }).message).toEqual({ zh: '模型路由未运行', en: 'Model router is not running' });
    expect(mapRouterFailure({ phase: 'login', kind: 'port-busy' })).toMatchObject({ status: 'login-port-busy', message: {
      zh: '另一个程序正在占用登录端口，请先完成或关闭它', en: 'Another program is using the sign-in port. Finish or close it first.' } });
    expect(mapRouterFailure({ phase: 'login', kind: 'cancelled' }).message).toEqual({ zh: '登录未完成', en: 'Sign-in was not completed' });
  });
  it.each([{ requires_sign_in: true }, { renewal_state: 'failed' }, { refresh_error: 'invalid_grant' }, { httpStatus: 401, refreshFailed: true }, { renewal_state: 'sign_in_required' }, { renewal_state: 'expired_sign_in_required' }, { refresh_error: { code: 'invalid_grant', http_status: 401 } }])(
    'requires reauthentication after credential/refresh failure: %j', input => expect(mapAccountStatus(input)).toMatchObject({ status: 'needs-sign-in', message: { zh: '需要重新登录', en: 'Sign in again' }, recovery: 'sign-in' }));
  it('does not infer account rejection from a lone quota/model/inference 403 or an unrefreshed 401', () => {
    for (const source of ['quota', 'models', 'inference'] as const) expect(mapAccountStatus({ httpStatus: 403, source, reason: 'You do not have a valid license' }).status).toBeNull();
    expect(mapAccountStatus({ httpStatus: 401 }).status).toBeNull();
  });
  it('keeps transient refresh retries distinct from terminal sign-in failures', () => {
    expect(mapAccountStatus({ renewal_state: 'retry_scheduled', refresh_error: { code: 'server_error', message: 'Try again', http_status: 503 } }).status).toBeNull();
    expect(mapAccountStatus({ renewal_state: 'awaiting_first_renewal' }).status).toBeNull();
    expect(mapAccountStatus(null as never).status).toBeNull();
    expect(mapRouterFailure({ phase: 'unknown', kind: 'unknown' } as never).status).toBeNull();
  });
  it('attaches sanitized provider reason and official recovery for confirmed account ineligibility', () => {
    const r = mapAccountStatus({ source: 'eligibility', ineligibleReason: 'Unsupported location; access_token=secret' });
    expect(r).toEqual({ status: 'account-unavailable', message: { zh: '账号不可用', en: 'Account unavailable' }, reason: 'Unsupported location; access_token=[redacted]', recovery: 'switch-to-official', recoveryMessage: { zh: '可切回 Anthropic 官方', en: 'You can switch back to Anthropic official' } });
  });
  it('maps preparation failures to controlled bilingual copy without engine diagnostics', () => {
    expect(mapSubscriptionLaunchFailure('needs-sign-in')).toMatchObject({ status: 'needs-sign-in', message: { zh: '需要重新登录', en: 'Sign in again' }, recovery: 'sign-in' });
    expect(mapSubscriptionLaunchFailure('models-not-ready').reasonLocalized).toEqual({ zh: '账号的模型还没有准备好，请稍后重试', en: "The account's models are not ready yet. Try again shortly." });
    expect(mapSubscriptionLaunchFailure('unsupported-context-window').reasonLocalized).toEqual({ zh: '所选模型的上下文窗口小于 100K，Claude Code 无法使用。请编辑这张卡片，换一个模型。', en: "The selected model's context window is under 100K, which Claude Code can't use. Edit this card and choose another model." });
    for (const diagnostic of ['EADDRINUSE addr=127.0.0.1:1999', 'ENOENT /private/engine', 'EACCES permission denied', 'readiness timeout', 'fetch failed', 'Fixture sensitive failure access_token=secret']) {
      const mapped = mapSubscriptionLaunchFailure('router-start-failed', diagnostic);
      expect(mapped.status).toBe('router-start-failed');
      expect(mapped.reasonLocalized?.zh).toMatch(/[\u4e00-\u9fff]/);
      expect(mapped.reasonLocalized?.en).toBeTruthy();
      for (const raw of ['EADDRINUSE', 'ENOENT', 'EACCES', '/private/', '127.0.0.1', 'access_token', 'secret', 'Fixture']) expect(JSON.stringify(mapped)).not.toContain(raw);
    }
  });
  it('marks exhausted quota 100% while preserving the card status copy', () => {
    expect(mapAccountStatus({ quotaExhausted: true })).toEqual({ status: 'quota-exhausted', message: null, recovery: 'none', quotaUsedPercent: 100 });
  });
  it('redacts URLs, bearer tokens, keyed secrets, JWTs, control characters and long opaque content', () => {
    const raw = 'Region denied\nhttps://example.test/cb?code=private Bearer bearer-secret {"refresh_token":"refresh-secret","api_key":"key-secret"} sk-privatekey eyJabc.def.ghi';
    const safe = sanitizeProviderReason(raw)!;
    for (const secret of ['?code=', 'bearer-secret', 'refresh-secret', 'key-secret', 'sk-privatekey', 'eyJabc', '\n']) expect(safe).not.toContain(secret);
    expect(safe).toContain('Region denied');
    expect(sanitizeProviderReason('x'.repeat(10000))!.length).toBeLessThanOrEqual(512);
    expect(sanitizeProviderReason({ token: 'secret' })).toBeUndefined();
  });
});
describe('quota explanation snapshots without protection', () => {
  it('uses header window durations rather than primary/secondary order, including active-limit prefix', () => {
    const r = quotaSnapshotFromHeaders({ 'X-Codex-Active-Limit': 'review', 'x-review-primary-used-percent': '100',
      'x-review-primary-window-minutes': '10080', 'x-review-primary-reset-after-seconds': '60',
      'x-review-secondary-used-percent': '0', 'x-review-secondary-window-minutes': '300', 'x-review-secondary-reset-at': '1800000000' }, 1700000000);
    expect(r).toEqual([{ name: 'weekly', usedPercent: 100, resetsAt: new Date(1700000060000).toISOString() }, { name: 'five_hour', usedPercent: 0 }]);
  });
  it('prefers absolute reset and does not invent windows from invalid header values', () => {
    expect(quotaSnapshotFromHeaders({ 'x-codex-primary-used-percent': '5', 'x-codex-primary-window-minutes': '300',
      'x-codex-primary-reset-at': '1700000100', 'x-codex-primary-reset-after-seconds': '99' }, 1700000000)[0]?.resetsAt).toBe(new Date(1700000100000).toISOString());
    expect(quotaSnapshotFromHeaders({ 'x-codex-primary-used-percent': '', 'x-codex-primary-window-minutes': '300' }, 1)).toEqual([]);
    expect(quotaSnapshotFromHeaders({ 'x-codex-primary-used-percent': 'NaN', 'x-codex-primary-window-minutes': '300' }, 1)).toEqual([]);
    expect(quotaSnapshotFromHeaders({}, 1)).toEqual([]);
  });
  it('reads supported quota windows, rejects invalid usage and ignores zero-usage placeholder resets', () => {
    expect(quotaSnapshotFromQuota([{ name: 'five_hour', usedPercent: 0, resetsAt: '2026-11-20T00:00:00Z' },
      { name: 'seven_day', usedPercent: 100, resetsAt: '2026-11-20T00:00:00Z' }, { name: 'other', usedPercent: 30 },
      { name: 'weekly', usedPercent: NaN }])).toEqual([{ name: 'five_hour', usedPercent: 0 }, { name: 'weekly', usedPercent: 100, resetsAt: '2026-11-20T00:00:00Z' }]);
  });
});
