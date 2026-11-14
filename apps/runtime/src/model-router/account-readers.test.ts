/** CC Switch subscription.rs, codex_oauth_models.rs, agy_account/quota.rs and
 * subscription_agy.rs fixtures adapted under MIT.
 * Commit: 3c76d3bda670297dd8c27d7d9be9b18a4c9143f5
 * License: ../../../../packages/contracts/third-party/subscription-accounts-reference-LICENSE
 */
import { describe, expect, it } from 'vitest';
import { MAX_ACCOUNT_BODY_BYTES, MAX_MODEL_BODY_BYTES, parseAntigravityAccount, parseAntigravityEligibility, parseAntigravityQuota, parseChatGptModels,
  parseChatGptPlan, parseChatGptUsage, parseCredentialSummary, readAntigravityAccount, readAntigravityQuota,
  readChatGptModels, readChatGptUsage, unwrapApiCallResponse, type ReadResult } from './account-readers.js';
const envelope = (body: unknown, status_code = 200) => ({ status_code, header: {}, body: JSON.stringify(body) });
function value<T>(r: ReadResult<T>): T { expect(r.ok).toBe(true); if (!r.ok) throw Error(r.error); return r.value; }
// quota.rs SAMPLE: real retrieveUserQuotaSummary response shape, upstream's altered numeric fixtures.
const quotaSample = { groups: [
  { displayName: 'Gemini Models', buckets: [
    { bucketId: 'gemini-weekly', resetTime: '2026-10-28T07:41:54Z', remainingFraction: 1 },
    { bucketId: 'gemini-5h', resetTime: '2026-10-21T12:41:54Z', remainingFraction: 0.5 },
    { bucketId: 'gemini-argon-wtu-weekly', resetTime: '2026-10-28T09:00:00Z', remainingFraction: 0.6 },
  ] }, { displayName: 'Claude and GPT models', buckets: [
    { bucketId: '3p-weekly', resetTime: '2026-10-28T04:42:02Z', remainingFraction: 0.988746 },
    { bucketId: '3p-5h', resetTime: '2026-10-21T09:42:02Z', remainingFraction: 0 },
    { bucketId: 'future-bucket', remainingFraction: 0.3 },
  ] },
] };
describe('ChatGPT account readers', () => {
  it('reads a >600KiB ten-model catalog without projecting its long instructions, while small-account limits stay 256KiB', () => {
    const body = { models: Array.from({ length: 10 }, (_, index) => ({ slug: `gpt-fixture-${index}`, instructions: 'model instruction '.repeat(4200) })) };
    const response = envelope(body);
    expect(new TextEncoder().encode(response.body).length).toBeGreaterThan(600 * 1024);
    const models = value(readChatGptModels(response));
    expect(models).toHaveLength(10);
    expect(JSON.stringify(models)).not.toContain('instruction');
    for (const reader of [readChatGptUsage, readAntigravityAccount, readAntigravityQuota]) expect(reader(response)).toMatchObject({ ok: false, error: 'response-too-large' });
    expect(readChatGptModels(envelope({ models: [{ slug: 'too-large', instructions: 'x'.repeat(MAX_MODEL_BODY_BYTES) }] }))).toMatchObject({ ok: false, error: 'response-too-large' });
  });
  it.each([['pro', 'Pro 200'], ['prolite', 'Pro 100'], ['promax', 'Pro 500'], ['plus', 'Plus'], ['team', 'Business'],
    ['business', 'Enterprise'], ['enterprise_cbp_usage_based', 'Enterprise'], ['self_serve_business_prolite', 'Business Premium'],
    ['free_workspace', 'Free Workspace'], ['future_plan', 'Future Plan'], ['unknown', null], ['', null]])(
    'uses reference plan labels without guessing a known subscription: %s', (raw, expected) => expect(parseChatGptPlan(raw)).toBe(expected));
  it('identifies swapped primary/secondary windows by seconds and suppresses a zero-usage reset', () => {
    const r = value(readChatGptUsage(envelope({ plan_type: 'plus', rate_limit: {
      primary_window: { used_percent: 25, limit_window_seconds: 604800, reset_at: 1800000000 },
      secondary_window: { used_percent: 0, limit_window_seconds: 18000, reset_at: 1800000100 },
    } })));
    expect(r).toEqual({ plan: 'Plus', windows: [
      { name: 'five_hour', windowSeconds: 18000, usedPercent: 0 },
      { name: 'weekly', windowSeconds: 604800, usedPercent: 25, resetsAt: new Date(1800000000 * 1000).toISOString() },
    ] });
  });
  it('keeps fractional nonzero usage reset and unknown windows distinct', () => {
    const r = value(parseChatGptUsage({ rate_limit: { primary_window: { used_percent: 0.001, limit_window_seconds: 18000, reset_at: 1800000000 },
      secondary_window: { used_percent: 11, limit_window_seconds: 777, reset_at: 'invalid' } } }));
    expect(r.windows.find(w => w.name === 'five_hour')?.resetsAt).toBeDefined();
    expect(r.windows.find(w => w.name === 'unknown')).toEqual({ name: 'unknown', windowSeconds: 777, usedPercent: 11 });
  });
  it.each([{}, null, [], { rate_limit: { primary_window: { used_percent: '20' } } }])('rejects missing or malformed usage: %j', raw => expect(parseChatGptUsage(raw).ok).toBe(false));
  it.each([
    { data: [{ id: 'gpt-5.4' }, { id: 'gpt-5.4-mini', ownedBy: 'openai' }] },
    { models: [{ slug: 'gpt-5.4' }, 'gpt-5.4-mini'] },
    { items: [{ model: 'gpt-5.4' }, { name: 'gpt-5.4-mini' }] },
    ['gpt-5.4-mini', 'gpt-5.4', 'gpt-5.4'],
    { models: { 'gpt-5.4-mini': {}, 'gpt-5.4': { display_name: 'GPT-5.4' } } },
    { 'gpt-5.4-mini': {}, 'gpt-5.4': {} },
  ])('sorts and deduplicates supported model response shapes: %j', raw => {
    const models = value(readChatGptModels(envelope(raw)));
    expect(models.map(m => m.id)).toEqual(['gpt-5.4', 'gpt-5.4-mini']);
  });
  it('keeps advertised metadata and drops secrets and invalid token limits', () => {
    const models = value(parseChatGptModels({ data: [{ id: 'gpt-a', display_name: 'GPT A', context_window: 400000, max_output_tokens: 128000, token: 'secret' }, { id: 'gpt-b', context_window: -1, max_output_tokens: '5000' }] }));
    expect(models[0]).toEqual({ id: 'gpt-a', displayName: 'GPT A', ownedBy: 'Codex', contextWindow: 400000, maxOutputTokens: 128000 });
    expect(models[1]).toEqual({ id: 'gpt-b', ownedBy: 'Codex' });
  });
  it.each([[], {}, { data: [] }, { models: null }, { error: { message: 'failure' } }])('returns unknown for empty model lists: %j', raw => expect(parseChatGptModels(raw).ok).toBe(false));
});
describe('management api-call envelope', () => {
  it.each([readChatGptUsage, readChatGptModels, readAntigravityAccount, readAntigravityQuota])('never treats inner 403 as success: %s', reader => {
    expect(reader(envelope({ plan_type: 'pro', error: { message: 'Forbidden access_token=private-value' }, ...quotaSample }, 403)))
      .toEqual({ ok: false, error: 'http-error', statusCode: 403, reason: 'Forbidden access_token=[redacted]' });
  });
  it('handles non-JSON, malformed envelopes, multibyte oversized and cyclic inputs without throwing', () => {
    expect(unwrapApiCallResponse({ status_code: 200, body: '<html>' })).toMatchObject({ ok: false, error: 'invalid-json' });
    expect(unwrapApiCallResponse({ status_code: 403, body: '<html>' })).toMatchObject({ ok: false, error: 'http-error' });
    for (const raw of [null, {}, { status_code: '200', body: '{}' }, { status_code: 200, body: {} }]) expect(unwrapApiCallResponse(raw).ok).toBe(false);
    expect(unwrapApiCallResponse({ status_code: 200, body: '中'.repeat(MAX_ACCOUNT_BODY_BYTES / 2) })).toMatchObject({ ok: false, error: 'response-too-large' });
    const cyclic: Record<string, unknown> = {}; cyclic.self = cyclic;
    expect(parseChatGptUsage(cyclic)).toMatchObject({ ok: false });
  });
});
describe('Antigravity account readers', () => {
  it.each([['g1-pro-tier', 'Google AI Pro', 'Google AI Pro'], ['g1-ultra-lite-tier', 'Google AI Ultra', 'Google AI Ultra 5x'],
    ['g1-ultra-tier', 'Google AI Ultra', 'Google AI Ultra 20x'], ['g1-ultra-lite-tier', 'Google AI Ultra 5X', 'Google AI Ultra 5X']])(
    'prefers paid tier and distinguishes Ultra variants: %s', (id, name, expected) => {
      const r = value(readAntigravityAccount(envelope({ currentTier: { id: 'free-tier' }, paidTier: { id, name }, cloudaicompanionProject: { projectId: ' aicode-consumers ' } })));
      expect(r).toEqual({ project: 'aicode-consumers', plan: expected, eligibility: 'eligible' });
    });
  it('matches reference free/current-tier fallback and project shapes', () => {
    expect(value(parseAntigravityAccount({ currentTier: { id: 'standard-tier' }, cloudaicompanionProject: 'project' })).plan).toBe('Paid');
    expect(value(parseAntigravityAccount({ currentTier: { id: 'free-tier' }, cloudaicompanionProject: { id: ' p ' } }))).toEqual({ project: 'p', plan: 'Free', eligibility: 'eligible' });
    expect(parseAntigravityAccount({})).toEqual({ ok: false, error: 'unknown-response' });
  });
  it('detects region-locked paid accounts but honors free-tier and enterprise eligibility', () => {
    const locked = { currentTier: { id: 'standard-tier' }, paidTier: { id: 'g1-ultra-tier', name: 'Google AI Ultra' },
      allowedTiers: [{ id: 'standard-tier' }], ineligibleTiers: [{ tierId: 'free-tier', reasonCode: 'UNSUPPORTED_LOCATION', reasonMessage: 'Not available in your location.' }] };
    expect(parseAntigravityEligibility(locked)).toEqual({ eligibility: 'ineligible', reason: 'Not available in your location.' });
    expect(parseAntigravityEligibility({ ...locked, allowedTiers: [{ id: 'free-tier' }] })).toEqual({ eligibility: 'eligible' });
    expect(parseAntigravityEligibility({ ...locked, gcpManaged: true })).toEqual({ eligibility: 'eligible' });
    expect(parseAntigravityEligibility({ allowedTiers: null, ineligibleTiers: [{ reasonMessage: ' Account disabled ' }] })).toEqual({ eligibility: 'ineligible', reason: 'Account disabled' });
    expect(parseAntigravityEligibility({})).toEqual({ eligibility: 'unknown' });
    expect(parseAntigravityEligibility({ currentTier: {} })).toEqual({ eligibility: 'unknown' });
  });
  it('parses the reference SAMPLE in model-group/window order', () => {
    const tiers = value(readAntigravityQuota(envelope(quotaSample)));
    expect(tiers.map(t => Math.round(t.usedPercent))).toEqual([50, 0, 40, 100, 1]);
    expect(tiers.map(t => t.name)).toEqual(['agy_gemini_five_hour', 'agy_gemini_weekly', 'agy_gemini_argon_weekly', 'agy_third_party_five_hour', 'agy_third_party_weekly']);
    expect(tiers[0]?.resetsAt).toBe('2026-10-21T12:41:54Z');
    expect(tiers[2]?.remainingFraction).toBe(0.6);
  });
  it('skips missing/unknown buckets and clamps numeric ratios like collect_tiers', () => {
    expect(parseAntigravityQuota({ groups: [{ buckets: [{ bucketId: 'gemini-5h' }, { bucketId: 'future', remainingFraction: 1 }] }] }).ok).toBe(false);
    const tiers = value(parseAntigravityQuota({ groups: [{ buckets: [{ bucketId: 'gemini-5h', remainingFraction: -1 }, { bucketId: '3p-5h', remainingFraction: 2 }] }] }));
    expect(tiers.map(t => t.usedPercent)).toEqual([100, 0]);
  });
});
describe('credential summary whitelist', () => {
  it('copies only identity/plan/status/renewal fields and sanitizes refresh errors', () => {
    const input = { email: 'a@b.com', account_id: 'account-1', plan_type: 'plus', status: 'ready', requires_sign_in: true,
      renewal_state: 'failed', refresh_error: 'renewal rejected refresh_token=hidden-token', access_token: 'hidden-access',
      id_token: 'hidden-id', auth_index: 'internal-index', path: '/secret/path', metadata: { token: 'nested-secret' }, arbitrary: 'discard' };
    expect(value(parseCredentialSummary(input))).toEqual({ email: 'a@b.com', accountId: 'account-1', plan: 'plus', status: 'ready', requires_sign_in: true, renewal_state: 'failed', refresh_error: 'renewal rejected refresh_token=[redacted]' });
  });
  it('extracts only the two non-secret v8 id_token summary claims', () => {
    // Pinned auth_files.go:extractCodexIDTokenClaims emits this object; no JWT decoding in Matou.
    expect(value(parseCredentialSummary({ email: 'a@b.com', id_token: { chatgpt_account_id: 'account-id', plan_type: 'pro',
      chatgpt_subscription_active_until: 'private-extra', access_token: 'discard' }, metadata: { token: 'discard' } })))
      .toEqual({ email: 'a@b.com', accountId: 'account-id', plan: 'pro' });
  });
  it('whitelists the fixed v8 structured refresh_error without nested secrets', () => {
    expect(value(parseCredentialSummary({ renewal_state: 'sign_in_required', refresh_error: {
      code: 'invalid_grant', message: 'Rejected refresh_token=private', http_status: 401, access_token: 'discard', nested: { key: 'discard' },
    } }))).toEqual({ renewal_state: 'sign_in_required', refresh_error: { code: 'invalid_grant', message: 'Rejected refresh_token=[redacted]', http_status: 401 } });
  });
  it('does not coerce malformed fields or retain token-only responses', () => {
    expect(parseCredentialSummary({ email: {}, requires_sign_in: 'true', access_token: 'secret' })).toEqual({ ok: false, error: 'unknown-response' });
  });
});
