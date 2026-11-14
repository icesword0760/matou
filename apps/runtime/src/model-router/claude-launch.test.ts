/** CC Switch live/project/claude.rs tests, adapted under MIT.
 * Commit: 3c76d3bda670297dd8c27d7d9be9b18a4c9143f5
 * License: ../../../../packages/contracts/third-party/subscription-accounts-reference-LICENSE
 */
import { describe, expect, it } from 'vitest';
import { buildClaudeLaunch, clientModelForSubagent, inheritedEnvToClear, pointReleaseClientId, resolveRoleModels, toClientModelId, type ClaudeLaunchInput, type ClaudeLaunchResult } from './claude-launch.js';
const input = (): ClaudeLaunchInput => ({
  contextWindow: 512_000, subscription: 'chatgpt', accountPrefix: 'matou-chatgpt', routerUrl: 'http://127.0.0.1:12345', clientKey: 'fixture-client-key', mainRole: 'sonnet', providerName: 'ChatGPT 账号',
  models: { opus: { id: 'gpt-large', displayName: 'GPT Large', contextWindow: 1_000_000, maxOutputTokens: 200000 },
    sonnet: { id: 'gpt-balanced', displayName: 'GPT Balanced', contextWindow: 400000, maxOutputTokens: 64000 },
    haiku: { id: 'gpt-small', contextWindow: 200000, maxOutputTokens: 16000 } },
});
function value(result: ClaudeLaunchResult) { expect(result.ok).toBe(true); if (!result.ok) throw Error(result.error); return result.value; }
describe('Claude subscription launch projection', () => {
  it('builds isolated aliases, real names/descriptions and secret-free temporary settings', () => {
    const source = input(), before = structuredClone(source), result = value(buildClaudeLaunch(source));
    expect(result.env).toMatchObject({ ANTHROPIC_BASE_URL: source.routerUrl, ANTHROPIC_AUTH_TOKEN: source.clientKey,
      ANTHROPIC_DEFAULT_OPUS_MODEL: 'matou-chatgpt/matou-opus', ANTHROPIC_DEFAULT_SONNET_MODEL_NAME: 'GPT Balanced',
      ANTHROPIC_DEFAULT_OPUS_MODEL_DESCRIPTION: 'ChatGPT 账号 · gpt-large · 512K',
      ANTHROPIC_DEFAULT_SONNET_MODEL_DESCRIPTION: 'ChatGPT 账号 · gpt-balanced · 512K', CLAUDE_CODE_AUTO_MODE_SERVER: '0' });
    expect(result.settings).toMatchObject({ env: { ANTHROPIC_API_KEY: '', CLAUDE_CODE_OAUTH_TOKEN: '', CLAUDE_CODE_DISABLE_1M_CONTEXT: '1' }, autoCompactWindow: 512000 });
    expect(result.settings.modelPicker).toEqual({ replaceBuiltInOptions: true, options: [
      { model: 'opus', label: 'Opus · GPT Large', description: 'ChatGPT 账号 · gpt-large · 512K' },
      { model: 'sonnet', label: 'Sonnet · GPT Balanced', description: 'ChatGPT 账号 · gpt-balanced · 512K' },
      { model: 'haiku', label: 'Haiku · gpt-small', description: 'ChatGPT 账号 · gpt-small · 512K' }
    ] });
    expect(result.env.ANTHROPIC_DEFAULT_FABLE_MODEL).toBe(result.env.ANTHROPIC_DEFAULT_OPUS_MODEL);
    expect(result.settings.modelOverrides).toEqual({ 'claude-fable-5': 'matou-chatgpt/matou-opus-fallback', 'claude-fable-5-2': 'matou-chatgpt/matou-opus-fallback' });
    expect(JSON.stringify(result.settings)).not.toContain(source.clientKey);
    expect(result.oauth['model-alias'].codex).toEqual([
      { name: 'gpt-large', alias: 'matou-opus', fork: true }, { name: 'gpt-balanced', alias: 'matou-sonnet', fork: true }, { name: 'gpt-small', alias: 'matou-haiku', fork: true }, { name: 'gpt-large', alias: 'matou-opus-fallback', fork: true },
    ]);
    expect(result.mainModel).toBe('matou-chatgpt/matou-sonnet');
    expect(result.env.CLAUDE_CODE_SUBAGENT_MODEL).toBeUndefined();
    expect(source).toEqual(before);
  });
  it('uses role-specific fallback order and reports missing models', () => {
    const fallback = { id: 'default' }, small = { id: 'small' }, own = { id: 'own' };
    expect(resolveRoleModels({ default: fallback, smallFast: small })).toEqual({ opus: fallback, sonnet: fallback, haiku: small });
    expect(resolveRoleModels({ default: fallback, smallFast: small, haiku: own, opus: own })).toEqual({ opus: own, sonnet: fallback, haiku: own });
    expect(resolveRoleModels({ smallFast: small })).toEqual({ opus: small, sonnet: small, haiku: small });
    expect(resolveRoleModels({ default: fallback })).toEqual({ opus: fallback, sonnet: fallback, haiku: fallback });
    expect(buildClaudeLaunch({ ...input(), models: {} })).toEqual({ ok: false, error: 'missing-role-model', roles: ['opus', 'sonnet', 'haiku'] });
  });
  it.each(['claude-opus-5.5', 'anthropic/claude-opus-5.5', 'claude-opus-5-5-20261101', 'CLAUDE-OPUS-5.5[1m]'])(
    'normalizes the exact Opus 5.5 boundary: %s', upstream => expect(pointReleaseClientId('opus', upstream)).toBe('claude-opus-5-5'));
  it.each(['claude-opus-5.55', 'claude-opus-5.5.1', 'claude-opus-5-55', 'claude-opus-5', 'claude-sonnet-5.5'])(
    'does not misidentify another version/family: %s', upstream => expect(pointReleaseClientId('opus', upstream)).toBeUndefined());
  it('normalizes Sonnet and explicit subagents, retaining the 1M marker', () => {
    expect(pointReleaseClientId('sonnet', 'anthropic/claude-sonnet-5.5')).toBe('claude-sonnet-5-5');
    expect(clientModelForSubagent('claude-sonnet-5-5-20261101[1m]')).toBe('claude-sonnet-5-5[1M]');
    expect(clientModelForSubagent('claude-opus-5.55')).toBe('claude-opus-5.55');
    expect(toClientModelId('antigravity', 'opus', { id: 'anthropic/claude-opus-5.5', contextWindow: 1_000_000 })).toBe('claude-opus-5-5[1M]');
    expect(toClientModelId('antigravity', 'haiku', { id: 'gemini-flash' })).toBe('matou-haiku');
    expect(toClientModelId('chatgpt', 'opus', { id: 'claude-opus-5.5' })).toBe('matou-opus');
  });
  it('keeps three route aliases for a repeated upstream model', () => {
    const result = value(buildClaudeLaunch({ ...input(), models: { default: { id: 'same-model' } } }));
    expect(result.oauth['model-alias'].codex?.map(a => a.alias)).toEqual(['matou-opus', 'matou-sonnet', 'matou-haiku', 'matou-opus-fallback']);
  });
  it.each(['opus', 'sonnet', 'haiku'] as const)('uses the same 512K GPT budget for the %s role', mainRole => {
    const result = value(buildClaudeLaunch({ ...input(), mainRole }));
    expect(result.env.CLAUDE_CODE_MAX_CONTEXT_TOKENS).toBe('512000');
    // Compression percentage is set by the process policy, not the catalog projection.
    expect(result.settings.autoCompactWindow).toBe(512000);

    expect(value(buildClaudeLaunch({ ...input(), mainRole: 'haiku' })).env.CLAUDE_CODE_MAX_OUTPUT_TOKENS).toBe('16000');
    expect(value(buildClaudeLaunch({ ...input(), mainRole: 'opus' })).env.CLAUDE_CODE_MAX_OUTPUT_TOKENS).toBe('128000');
  });
  it('applies the explicit GPT budget even with missing or smaller catalog defaults', () => {
    const source = input(); delete source.models.haiku!.contextWindow; source.models.sonnet!.contextWindow = 80000;
    expect(value(buildClaudeLaunch(source)).settings.autoCompactWindow).toBe(512000);
  });
  it('never raises a small upstream window to the client minimum, and caps larger windows conservatively', () => {
    const source = { ...input(), subscription: 'antigravity' as const, accountPrefix: 'matou-antigravity' as const }; delete source.contextWindow; source.models.sonnet!.contextWindow = 99999;
    expect(buildClaudeLaunch(source)).toEqual({ ok: false, error: 'unsupported-context-window' });
    source.models.sonnet!.contextWindow = 2000000; source.models.opus!.contextWindow = 3000000; source.models.haiku!.contextWindow = 1500000;
    const result = value(buildClaudeLaunch(source));
    expect(result.settings.autoCompactWindow).toBe(1000000);
    expect(result.env.CLAUDE_CODE_MAX_CONTEXT_TOKENS).toBe('1000000');
    expect(result.env.CLAUDE_CODE_DISABLE_1M_CONTEXT).toBeUndefined();
    expect(result.settings.env.CLAUDE_CODE_DISABLE_1M_CONTEXT).toBeUndefined();
  });
  it.each(['opus', 'sonnet', 'haiku'] as const)('rejects an under-100K %s tier even when it is not selected', role => {
    const source = { ...input(), subscription: 'antigravity' as const, accountPrefix: 'matou-antigravity' as const }; delete source.contextWindow; source.models[role]!.contextWindow = 99999;
    expect(buildClaudeLaunch(source)).toEqual({ ok: false, error: 'unsupported-context-window' });
  });
  it('does not guess a safe minimum when one tier has no window metadata', () => {
    const source = { ...input(), subscription: 'antigravity' as const, accountPrefix: 'matou-antigravity' as const }; delete source.contextWindow; delete source.models.haiku!.contextWindow;
    const result = value(buildClaudeLaunch(source));
    expect(result.settings.autoCompactWindow).toBeUndefined();
    expect(result.env.CLAUDE_CODE_MAX_CONTEXT_TOKENS).toBeUndefined();
    expect(result.unknown).toContain('context-window');
  });
  it('does not invent unknown context or output limits', () => {
    const source = input(); delete source.contextWindow;
    const result = value(buildClaudeLaunch({ ...source, subscription: 'antigravity', accountPrefix: 'matou-antigravity', models: { default: { id: 'unknown' } } }));
    expect(result.env.CLAUDE_CODE_MAX_CONTEXT_TOKENS).toBeUndefined();
    expect(result.env.CLAUDE_CODE_MAX_OUTPUT_TOKENS).toBeUndefined();
    expect(result.settings.autoCompactWindow).toBeUndefined();
    expect(result.unknown).toEqual(['context-window', 'max-output-tokens']);
  });
  it('clears old credentials, cloud selectors, force/subagent, output/compact and all inherited default variables', () => {
    const result = inheritedEnvToClear(['ANTHROPIC_DEFAULT_FUTURE_VALUE', 'CLAUDE_CODE_FORCE_SUBAGENT_MODEL', 'PATH']);
    for (const name of ['ANTHROPIC_API_KEY', 'CLAUDE_CODE_OAUTH_TOKEN', 'ANTHROPIC_MODEL', 'ANTHROPIC_SMALL_FAST_MODEL',
      'CLAUDE_CODE_SUBAGENT_MODEL', 'CLAUDE_CODE_FORCE_SUBAGENT_MODEL', 'CLAUDE_CODE_MAX_OUTPUT_TOKENS',
      'CLAUDE_CODE_AUTO_COMPACT_WINDOW', 'CLAUDE_AUTOCOMPACT_PCT_OVERRIDE', 'CLAUDE_CODE_USE_BEDROCK',
      'CLAUDE_CODE_USE_VERTEX', 'CLAUDE_CODE_USE_FOUNDRY', 'ANTHROPIC_DEFAULT_FUTURE_VALUE']) expect(result).toContain(name);
    expect(result).not.toContain('PATH');
  });
  it.each(['https://127.0.0.1:1', 'http://example.com', 'http://user:pass@localhost:1', 'not-url'])(
    'rejects invalid/nonlocal routing without exceptions: %s', routerUrl => expect(buildClaudeLaunch({ ...input(), routerUrl })).toEqual({ ok: false, error: 'invalid-input' }));
  it('returns structured failures for malformed launch fields', () => {
    expect(buildClaudeLaunch(null as unknown as ClaudeLaunchInput)).toEqual({ ok: false, error: 'invalid-input' });
    expect(buildClaudeLaunch({ ...input(), inheritedEnvNames: 7 } as unknown as ClaudeLaunchInput)).toEqual({ ok: false, error: 'invalid-input' });
    expect(buildClaudeLaunch({ ...input(), models: { opus: { id: 3 } } } as unknown as ClaudeLaunchInput)).toMatchObject({ ok: false, error: 'missing-role-model' });
  });
  it('rejects category/prefix mismatch', () => expect(buildClaudeLaunch({ ...input(), accountPrefix: 'matou-antigravity' })).toEqual({ ok: false, error: 'invalid-input' }));
});
