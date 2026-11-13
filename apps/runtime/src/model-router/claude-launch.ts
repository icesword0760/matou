/** Adapted from CC Switch src-tauri/src/live/project/claude.rs (MIT).
 * Model menu: terminal_model_picker / TierModel::picker_row.
 * Unconfigured Fable falls back to Opus, matching proxy/model_mapper.rs::map_model.
 * Commit: 3c76d3bda670297dd8c27d7d9be9b18a4c9143f5
 * License: ../../../../packages/contracts/third-party/subscription-accounts-reference-LICENSE
 * Pure projection only; never writes user settings or launches a process.
 */
export type ModelRole = 'opus' | 'sonnet' | 'haiku';
export type SubscriptionKind = 'chatgpt' | 'antigravity';
export interface UpstreamModel {
  id: string;
  displayName?: string;
  contextWindow?: number;
  maxOutputTokens?: number;
}
export interface RoleModels extends Partial<Record<ModelRole, UpstreamModel>> {
  default?: UpstreamModel;
  smallFast?: UpstreamModel;
}
export interface ClaudeLaunchInput {
  subscription: SubscriptionKind;
  accountPrefix: 'matou-chatgpt' | 'matou-antigravity';
  routerUrl: string;
  clientKey: string;
  models: RoleModels;
  mainRole: ModelRole;
  providerName: string;
  /** Names only: the caller supplies Object.keys(inheritedEnv), never its secrets. */
  inheritedEnvNames?: readonly string[];
}
export interface ModelAlias { name: string; alias: string; fork: true }
export const FABLE_FALLBACK_ALIAS = 'matou-opus-fallback';
export interface ModelPickerRow { model: ModelRole; label: string; description: string }
export interface ClaudeLaunch {
  env: Record<string, string>;
  clearEnv: string[];
  settings: { env: { ANTHROPIC_API_KEY: ''; CLAUDE_CODE_OAUTH_TOKEN: ''; CLAUDE_CODE_DISABLE_1M_CONTEXT?: '1' }; modelPicker: { replaceBuiltInOptions: true; options: ModelPickerRow[] }; modelOverrides: Record<string, string>; autoCompactWindow?: number };
  oauth: { 'model-alias': Partial<Record<'codex' | 'antigravity', ModelAlias[]>> };
  mainModel: string;
  unknown: string[];
}
export type ClaudeLaunchResult = { ok: true; value: ClaudeLaunch } | {
  ok: false; error: 'invalid-input' | 'missing-role-model' | 'unsupported-context-window'; roles?: ModelRole[];
};
const roles: ModelRole[] = ['opus', 'sonnet', 'haiku'];
const text = (v: unknown): v is string => typeof v === 'string' && v.trim().length > 0 && v.length <= 4096 && !/[\r\n\0]/.test(v);
const tokens = (v: unknown): v is number => typeof v === 'number' && Number.isSafeInteger(v) && v > 0;
const stripMarker = (id: string) => id.replace(/\[1m\]$/i, '').trim();

/** route_roles: Haiku prioritizes SMALL_FAST; the other two prioritize default. */
export function resolveRoleModels(models: RoleModels): Record<ModelRole, UpstreamModel | undefined> {
  const valid = (m: UpstreamModel | undefined) => m && text(m.id) ? m : undefined;
  const fallback = valid(models?.default), small = valid(models?.smallFast);
  return {
    opus: valid(models?.opus) ?? fallback ?? small,
    sonnet: valid(models?.sonnet) ?? fallback ?? small,
    haiku: valid(models?.haiku) ?? small ?? fallback,
  };
}

/** point_release_client_id: a following digit or dot must not match 5.5. */
export function pointReleaseClientId(family: 'opus' | 'sonnet', upstream: string): string | undefined {
  const id = stripMarker(upstream).toLowerCase();
  return new RegExp(`${family}-5[.-]5(?![\\d.])`).test(id) ? `claude-${family}-5-5` : undefined;
}

/** client_model_for_subagent; exported for callers interpreting explicit child models.
 * Launch deliberately does not force a subagent model: children inherit the main model.
 */
export function clientModelForSubagent(upstream: string): string {
  const id = pointReleaseClientId('opus', upstream) ?? pointReleaseClientId('sonnet', upstream);
  return id ? id + (/\[1m\]$/i.test(upstream.trim()) ? '[1M]' : '') : upstream;
}

export function toClientModelId(subscription: SubscriptionKind, role: ModelRole, model: UpstreamModel): string {
  if (subscription === 'antigravity' && /^(?:anthropic\/)?claude-/i.test(model.id.trim())) {
    const id = role === 'haiku' ? 'claude-haiku-5-5' :
      pointReleaseClientId(role, model.id) ?? `claude-${role}-5`;
    return id + (role !== 'haiku' && (/\[1m\]$/i.test(model.id.trim()) || (model.contextWindow ?? 0) >= 1_000_000) ? '[1M]' : '');
  }
  return `matou-${role}`;
}

const fixedClear = [
  'ANTHROPIC_API_KEY', 'ANTHROPIC_AUTH_TOKEN', 'ANTHROPIC_BASE_URL', 'CLAUDE_CODE_OAUTH_TOKEN',
  'ANTHROPIC_MODEL', 'ANTHROPIC_SMALL_FAST_MODEL', 'CLAUDE_CODE_SUBAGENT_MODEL',
  'CLAUDE_CODE_FORCE_SUBAGENT_MODEL', 'CLAUDE_CODE_SUBAGENT_MODEL_FORCE',
  'CLAUDE_CODE_MAX_CONTEXT_TOKENS', 'CLAUDE_CODE_MAX_OUTPUT_TOKENS',
  'CLAUDE_CODE_AUTO_COMPACT_WINDOW', 'CLAUDE_AUTOCOMPACT_PCT_OVERRIDE',
  'CLAUDE_CODE_DISABLE_AUTO_COMPACT', 'DISABLE_COMPACT', 'DISABLE_AUTO_COMPACT',
  'MAX_THINKING_TOKENS', 'CLAUDE_CODE_USE_BEDROCK', 'CLAUDE_CODE_USE_VERTEX', 'CLAUDE_CODE_USE_FOUNDRY',
  'AWS_BEARER_TOKEN_BEDROCK', 'CLAUDE_CODE_AUTO_MODE_SERVER',
  ...['OPUS', 'SONNET', 'HAIKU', 'FABLE'].flatMap(role => ['', '_NAME', '_DESCRIPTION'].map(s => `ANTHROPIC_DEFAULT_${role}_MODEL${s}`)),
];
export function inheritedEnvToClear(names: readonly string[] = []): string[] {
  return [...new Set([...fixedClear, ...names.filter(n => n.startsWith('ANTHROPIC_DEFAULT_') || /^CLAUDE_CODE_.*SUBAGENT.*MODEL/.test(n))])].sort();
}
export function formatContextWindow(window: number): string {
  return window >= 1_000_000 ? `${window / 1_000_000}M` : `${window / 1000}K`;
}

export function buildClaudeLaunch(input: ClaudeLaunchInput): ClaudeLaunchResult {
  if (!input || !['chatgpt', 'antigravity'].includes(input.subscription) ||
      input.accountPrefix !== `matou-${input.subscription}` || !roles.includes(input.mainRole) ||
      !text(input.clientKey) || !text(input.providerName) || !text(input.routerUrl) ||
      (input.inheritedEnvNames !== undefined && (!Array.isArray(input.inheritedEnvNames) || input.inheritedEnvNames.some(n => typeof n !== 'string')))) return { ok: false, error: 'invalid-input' };
  try {
    const url = new URL(input.routerUrl);
    if (url.protocol !== 'http:' || !['127.0.0.1', '[::1]', 'localhost'].includes(url.hostname) ||
        url.username || url.password || url.search || url.hash) return { ok: false, error: 'invalid-input' };
  } catch { return { ok: false, error: 'invalid-input' }; }
  const resolved = resolveRoleModels(input.models);
  const missing = roles.filter(role => !resolved[role]);
  if (missing.length) return { ok: false, error: 'missing-role-model', roles: missing };
  const env: Record<string, string> = {
    ANTHROPIC_BASE_URL: input.routerUrl, ANTHROPIC_AUTH_TOKEN: input.clientKey, CLAUDE_CODE_AUTO_MODE_SERVER: '0',
  };
  const aliases: ModelAlias[] = [], unknown: string[] = [];
  for (const role of roles) {
    const model = resolved[role]!;
    const alias = toClientModelId(input.subscription, role, model);
    const key = `ANTHROPIC_DEFAULT_${role.toUpperCase()}_MODEL`;
    env[key] = `${input.accountPrefix}/${alias}`;
    env[`${key}_NAME`] = text(model.displayName) ? model.displayName.trim() : stripMarker(model.id);
    env[`${key}_DESCRIPTION`] = [input.providerName, stripMarker(model.id), ...(tokens(model.contextWindow) ? [formatContextWindow(model.contextWindow)] : [])].join(' · ');
    if (!aliases.some(a => a.alias === stripMarker(alias))) aliases.push({ name: stripMarker(model.id), alias: stripMarker(alias), fork: true });
  }
  // Fable is not offered by this card. Internal Fable selectors must resolve
  // through the same prefixed Opus target instead of an unregistered model.
  for (const suffix of ['', '_NAME', '_DESCRIPTION']) env[`ANTHROPIC_DEFAULT_FABLE_MODEL${suffix}`] = env[`ANTHROPIC_DEFAULT_OPUS_MODEL${suffix}`]!;
  aliases.push({ name: stripMarker(resolved.opus!.id), alias: FABLE_FALLBACK_ALIAS, fork: true });
  const settings: ClaudeLaunch['settings'] = {
    env: { ANTHROPIC_API_KEY: '', CLAUDE_CODE_OAUTH_TOKEN: '' },
    // The fixed client can use a literal Fable ID as well as the tier selector.
    // Its supported request overrides preserve the same Fable -> Opus fallback.
    // Give literal IDs a separate hidden route to the same Opus upstream.
    // Reusing the visible Opus alias here makes this client infer Fable's
    // identity/capabilities for Opus (including the Default row).
    modelOverrides: Object.fromEntries(['claude-fable-5', 'claude-fable-5-2'].map(id => [id, `${input.accountPrefix}/${FABLE_FALLBACK_ALIAS}`])),
    modelPicker: { replaceBuiltInOptions: true, options: roles.map(role => {
      const key = `ANTHROPIC_DEFAULT_${role.toUpperCase()}_MODEL`;
      return { model: role, label: `${role[0]!.toUpperCase()}${role.slice(1)} · ${env[`${key}_NAME`]}`, description: env[`${key}_DESCRIPTION`]! };
    }) }
  };
  // The running client cannot shrink its window when /model switches tiers.
  // Protect every selectable tier from the start, regardless of the main role.
  const windows = roles.map(role => resolved[role]!.contextWindow);
  // Claude Code 2.1.323 autoCompactWindow schema allows 100000..1000000. Never raise a smaller upstream window.
  if (windows.some(w => tokens(w) && w < 100_000)) return { ok: false, error: 'unsupported-context-window' };
  if (windows.every(tokens)) {
    const window = Math.min(...windows, 1_000_000);
    settings.autoCompactWindow = window;
    env.CLAUDE_CODE_MAX_CONTEXT_TOKENS = String(window);
    // 2.1.323 infers Opus's opt-in [1m] suffix even for custom role aliases.
    // Disable that inference for smaller cards. The native autoCompactWindow
    // remains authoritative (e.g. 272K), rather than falling back to 200K.
    if (window < 1_000_000) {
      env.CLAUDE_CODE_DISABLE_1M_CONTEXT = '1';
      settings.env.CLAUDE_CODE_DISABLE_1M_CONTEXT = '1';
    }
  } else unknown.push('context-window');
  const output = resolved[input.mainRole]!.maxOutputTokens;
  if (tokens(output)) env.CLAUDE_CODE_MAX_OUTPUT_TOKENS = String(Math.min(output, 128000));
  else unknown.push('max-output-tokens');
  return { ok: true, value: { env, clearEnv: inheritedEnvToClear(input.inheritedEnvNames), settings,
    oauth: { 'model-alias': { [input.subscription === 'chatgpt' ? 'codex' : 'antigravity']: aliases } },
    mainModel: env[`ANTHROPIC_DEFAULT_${input.mainRole.toUpperCase()}_MODEL`]!, unknown } };
}
