import { closeSync, constants, fstatSync, openSync, readSync } from 'node:fs';
import { homedir } from 'node:os';
import path from 'node:path';

const MAX_CONFIG_BYTES = 256 * 1024;
const MT_CTX_INSTRUCTIONS = [
  '## Matou Context Helpers (`mt ctx`)',
  '- Before reading an unfamiliar large file, run `mt ctx outline <file>` and read only targeted line ranges.',
  '- Combine multiple symbol lookups in one call with `mt ctx batch-search <q1> <q2> ...`.',
  '- When a log is trimmed and saved as `log_XXXX`, search it with `mt ctx search <query> --source log_XXXX`.',
  '- After `/compact`, run `mt ctx resume` to recover saved log sources and progress clues.'
].join('\n');
const AGENT_IDS = ['codex', 'dsh', 'agy'];
const DEFAULT_ROLES = {
  allowSubDispatch: true,
  defaultAgent: 'codex',
  agents: [
    { id: 'codex', mode: 'auto', model: 'sol', effort: 'high', speed: 'default',
      role: '方案定好后的一切落地：细节设计、编码与测试、重构、源码阅读与调研、图片/媒体生成、数据处理与打包；电脑界面操作（含 Blender）自动派 terra 最新版子代理完成。' },
    { id: 'dsh', mode: 'auto', model: 'deepseek-flash', effort: 'high',
      role: '前端 UI 设计与界面代码实现：页面和组件的视觉设计、布局、样式、交互外观，以及将其落地为前端界面代码。' },
    { id: 'agy', mode: 'explicit', model: 'gemini-pro-orchestrator', effort: 'high',
      role: '用户点名时接手落地执行：编码、调研与复杂任务执行（Orchestrator 可自派子代理并行推进；额度耗尽或型号不可用时直接停下汇报，不自动换人）。' },
  ],
};

function readConfig(file, { missing = null, defaultVersion } = {}) {
  if (!file) return null;
  try {
    const fd = openSync(file, constants.O_RDONLY | constants.O_NONBLOCK);
    try {
      const stat = fstatSync(fd);
      if (!stat.isFile() || stat.size > MAX_CONFIG_BYTES) return null;
      const buffer = Buffer.allocUnsafe(MAX_CONFIG_BYTES + 1);
      let size = 0;
      while (size < buffer.length) {
        const count = readSync(fd, buffer, size, buffer.length - size);
        if (!count) break;
        size += count;
      }
      if (size > MAX_CONFIG_BYTES) return null;
      const parsed = JSON.parse(buffer.toString('utf8', 0, size));
      if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) return null;
      const version = Object.hasOwn(parsed, 'version') ? parsed.version : defaultVersion;
      return version === 1 ? parsed : null;
    } finally {
      closeSync(fd);
    }
  } catch (error) {
    return error?.code === 'ENOENT' ? missing : null;
  }
}

function contextToolsEnabled() {
  if (!process.env.MATOU_DATA_ROOT) return false;
  const config = readConfig(path.join(process.env.MATOU_DATA_ROOT, 'context-optimizer.json'), {
    missing: {},
    defaultVersion: 1
  });
  if (!config) return false;
  const isObject = value => value && typeof value === 'object' && !Array.isArray(value);
  if (Object.hasOwn(config, 'agents') && !isObject(config.agents)) return false;
  const agents = config.agents ?? {};
  if (!Object.hasOwn(agents, 'claude-code')) return true;
  const settings = agents['claude-code'];
  if (!isObject(settings)) return false;
  return ['enabled', 'cliTools'].every(key => !Object.hasOwn(settings, key) || settings[key] === true);
}

function readMatouRolesConfig() {
  const parsed = readConfig(process.env.MATOU_FOREMAN_ROLES_FILE);
  if (!Array.isArray(parsed?.agents)) return DEFAULT_ROLES;
  const agents = parsed.agents.filter((item) => item && typeof item.id === 'string' && item.id.trim());
  const enabled = (id) => {
    const agent = agents.find((entry) => entry.id === id);
    return agent && agent.enabled !== false;
  };
  let defaultAgent;
  if (Object.hasOwn(parsed, 'defaultAgent')) {
    if (AGENT_IDS.includes(parsed.defaultAgent) && enabled(parsed.defaultAgent)) defaultAgent = parsed.defaultAgent;
  } else {
    const runs = path.resolve(process.env.FOREMAN_RUN_DIR || path.join(homedir(), '.claude', 'agent-foreman', 'runs'));
    const legacyAgent = readConfig(path.join(path.dirname(runs), 'config.json'))?.defaults?.agent;
    if (AGENT_IDS.includes(legacyAgent)) defaultAgent = legacyAgent;
  }
  return { agents, defaultAgent: defaultAgent ?? AGENT_IDS.find(enabled) ?? 'codex',
    allowSubDispatch: parsed.allowSubDispatch !== false };
}

function formatMatouRolesMarkdown(roles, defaultAgent = 'codex') {
  if (!roles) return '';
  const names = { codex: 'Codex', dsh: 'DSH', agy: 'Antigravity（反重力）' };
  const efforts = { off: '关闭', low: '低', medium: '中', high: '高', xhigh: '超高', max: '最高' };
  const speeds = { fast: '快速', ultrafast: '超快', priority: '优先', flex: '弹性' };
  const enabled = roles.agents.filter((a) => a.enabled !== false);
  const disabled = roles.agents.filter((a) => a.enabled === false);
  const lines = ['## 用户自定义 Agent 生态位与默认配置（以本节为准）', ''];
  for (const a of enabled) {
    const label = Object.hasOwn(names, a.id) ? names[a.id] : a.id;
    const tag = a.id === defaultAgent ? '默认兜底主力' : (a.mode === 'explicit' ? '仅点名时派' : '按职责自动派');
    const model = typeof a.model === 'string' ? a.model : '';
    const effortFixed = a.id === 'agy' && /orchestrator/i.test(model);
    const suffix = a.id === 'agy' ? /-(low|medium|high|xhigh)$/i.exec(model)?.[1].toLowerCase() : null;
    const effort = suffix || (typeof a.effort === 'string' ? a.effort : '');
    const speed = a.id === 'codex' && typeof a.speed === 'string' && a.speed !== 'default' ? a.speed : '';
    const specs = [
      model ? `模型: ${model}` : null,
      effortFixed ? '无推理档位' : (effort ? `推理: ${Object.hasOwn(efforts, effort) ? efforts[effort] : effort}` : null),
      speed ? `速度: ${Object.hasOwn(speeds, speed) ? speeds[speed] : speed}` : null,
    ].filter(Boolean).join(' · ');
    const role = typeof a.role === 'object' ? '' : a.role;
    const roleText = String(role || '').replace(/\r\n?/g, '\n').trim().replace(/\n/g, '\n  ');
    lines.push(`- **${label}**（${tag}${specs ? ` | ${specs}` : ''}）：${roleText || '按默认职责执行。'}`);
  }
  if (disabled.length) {
    lines.push(`- **已停用（禁止派活）**：${disabled.map((a) => Object.hasOwn(names, a.id) ? names[a.id] : a.id).join('、')}`);
  }
  const autoPeers = enabled.filter((a) => a.mode === 'auto').map((a) => Object.hasOwn(names, a.id) ? names[a.id] : a.id);
  if (!roles.allowSubDispatch) {
    lines.push('- **二次转包策略**：已关闭（所有执行者禁止二次转包，只允许顶层监工派活；Orchestrator 模型内部自带的并行子代理不受影响）');
  } else if (enabled.length < 2 || autoPeers.length === 0) {
    lines.push('- **二次转包策略**：当前无可转派协作者（需至少启用两个执行者且目标协作者处于「按职责自动派」，当前仅由顶层监工直接派活）');
  } else {
    lines.push(`- **二次转包策略**：允许当前执行者遇到其他「启用 + 按职责自动派」协作者（${autoPeers.join('、')}）职责范围内的子任务时，跨执行者转派 1 层子任务`);
  }
  return lines.join('\n');
}

let additionalContext = `Matou host control is active because the matou-host-control plugin is loaded.

For any request about another, adjacent, left, or right Matou card, session, or terminal, invoke \`matou-host-control:mt-terminal\` immediately and follow its \`mt identify -> mt list -> mt read/history/commands\` flow.

Treat Matou's \`mt\` control plane as authoritative for these requests. Skip alternate terminal host environment discovery and other terminal-host probes in this Matou-managed session.`

if (!process.env.MATOU_FOREMAN_DEPTH) {
  const roles = readMatouRolesConfig();
  additionalContext += `\n\n在当前码头托管会话中派活请直接运行 \`mt-foreman\`。配置变更后可运行 \`mt-foreman roles\` 查看最新配置。\n\n${formatMatouRolesMarkdown(roles, roles.defaultAgent)}`;
}

if (contextToolsEnabled()) additionalContext += `\n\n${MT_CTX_INSTRUCTIONS}`;

process.stdout.write(JSON.stringify({
  hookSpecificOutput: {
    hookEventName: 'SessionStart',
    additionalContext
  }
}))
