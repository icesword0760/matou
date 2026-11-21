#!/usr/bin/env node
// Supervised agent CLI runner. Usage and rules: ../SKILL.md
import { execFile, execFileSync, spawn } from 'node:child_process';
import fs from 'node:fs';
import { createHash } from 'node:crypto';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const SCRIPT = fileURLToPath(import.meta.url);
const RUNS = path.resolve(process.env.FOREMAN_RUN_DIR || path.join(os.homedir(), '.claude', 'agent-foreman', 'runs'));
const CONFIG = path.join(path.dirname(RUNS), 'config.json');
const CODEX_BIN = process.env.CODEX_BIN || 'codex';
const DSH_BIN = process.env.DSH_BIN || 'dsh';
const AGY_BIN = process.env.AGY_BIN || 'agy';
const AGY_RESULT_EXIT_MS = Number(process.env.FOREMAN_AGY_RESULT_EXIT_MS) || 10_000;
const AGY_QUOTA = /RESOURCE_EXHAUSTED|quota|Resets in|额度/i;

const DEFAULTS = { askWaitMin: 10, model: 'sol', effort: 'high', idleMin: 10, warnMin: 3, maxMin: 45, retries: 3, backoffMin: [2, 5, 10] };

function readDefaultsConfig() {
  const result = { path: CONFIG, exists: fs.existsSync(CONFIG), defaults: {}, ignored: [] };
  const ignore = (field, value) => result.ignored.push(`已忽略：${field}=${JSON.stringify(value) ?? 'undefined'}`);
  if (!result.exists) return result;
  let config;
  try { config = JSON.parse(fs.readFileSync(CONFIG, 'utf8')); }
  catch { ignore('config', '无法读取或不是合法 JSON'); return result; }
  if (config?.version !== 1) { ignore('version', config?.version); return result; }
  if (!config.defaults || typeof config.defaults !== 'object' || Array.isArray(config.defaults)) {
    ignore('defaults', config.defaults);
    return result;
  }
  const minutes = (value) => typeof value === 'number' && Number.isFinite(value) && value > 0 && value <= 600;
  const validators = {
    agent: (value) => ['codex', 'agy', 'dsh'].includes(value),
    model: (value) => typeof value === 'string' && value.length > 0 && value.length <= 64 && !/\s/u.test(value),
    effort: (value) => ['low', 'medium', 'high', 'xhigh'].includes(value),
    idleMin: minutes, maxMin: minutes, askWaitMin: minutes,
    retries: (value) => Number.isInteger(value) && value >= 0 && value <= 5,
  };
  for (const [field, value] of Object.entries(config.defaults)) {
    if (Object.hasOwn(validators, field) && validators[field](value)) result.defaults[field] = value;
    else ignore(field, value);
  }
  return result;
}

function configuredDefaults(defaults, agent, ignored = []) {
  const { agent: _agent, model, ...rest } = defaults;
  if (agent === 'dsh' && ['medium', 'xhigh'].includes(rest.effort)) {
    ignored.push(`已忽略：effort=${JSON.stringify(rest.effort)}（DSH 不支持，使用内置 high）`);
    delete rest.effort;
  }
  return { ...DEFAULTS,
    ...(agent === 'dsh' ? { model: 'deepseek-flash' } : agent === 'agy' ? { model: 'gemini-pro-orchestrator' } : {}),
    ...rest, ...(model != null && agent !== 'dsh' ? { model } : {}) };
}

const POLL_MS = Number(process.env.FOREMAN_POLL_MS) || 15_000;
const FATAL = /\b40[13]\b|unauthori[sz]ed|not logged in|please (log|sign) in|invalid api key|sandbox.*denied/i;
const RETRYABLE = /capacity|\b429\b|\b5\d\d\b|overloaded|rate.?limit|reconnect|stream disconnected|timed? ?out|AI 专线|ECONN|ETIMEDOUT|network|connection/i;

const usage = `Usage:
  node scripts/foreman.mjs config
  node scripts/foreman.mjs models [--model M] [--effort E] [--refresh-models]
  node scripts/foreman.mjs start  商城-购物车-01 --cd <dir> [--brief <file>] [options]
  node scripts/foreman.mjs send   商城-购物车-01 [--message <file>] [options] (resume is an alias)
  node scripts/foreman.mjs watch  商城-购物车-01
  node scripts/foreman.mjs status 商城-购物车-01
  node scripts/foreman.mjs stop   商城-购物车-01
  node scripts/foreman.mjs ask    商城-购物车-01 [--no-wait] [--wait-min N] (Codex; question from stdin)
  node scripts/foreman.mjs inbox  商城-购物车-01 (Codex)
  node scripts/foreman.mjs answer 商城-购物车-01 [n] (Claude; answer from stdin)
  node scripts/foreman.mjs note   商城-购物车-01 (Claude; note from stdin)
Omit --brief / --message to read the prompt from stdin.
会加载用户的 Codex 配置（MCP、插件、记忆等），速度档固定为普通。
配置里有自定义指令文件时拒绝派活。
Exit code 7: Codex 正在等回答。
Exit code 8: 跟踪到期，任务继续运行。
Options:
  默认值可写在 ${CONFIG}，命令行参数优先。
  FOREMAN_RUN_DIR overrides ~/.claude/agent-foreman/runs for all commands.
  --agent codex|dsh|agy (codex; start). DSH_BIN / AGY_BIN override dsh / agy from PATH.
  agy defaults: --model gemini-pro-orchestrator (fixed; no effort levels). --model hydrogen|argon|carbon picks the newest at --effort low|medium|high|xhigh (high); --effort needs one of these.
  --ask-wait-min N (10; start/send)
  --no-watch  --watch-min N (29)  --warn-min N (3)
  --model M (sol；系列最新版，可选 best/astra/sol/terra/luna 或型号)  --effort E (high)  --idle-min N (10)  --max-min N (45)
  --refresh-models (start/models；忽略 24 小时缓存，重新解析)
  --retries N (3)  --backoff 2,5,10  --add-dir D (repeatable)  --network  --no-project-doc
  --sandbox  (workspace-write; use --network / --add-dir to extend access)
  --full-access  (no sandbox; default for new tasks, accepted for compatibility. Out-of-scope git changes are reported after each turn)
  send/resume preserve existing access unless --full-access or --sandbox is specified.`;

function parseArgs(argv) {
  const [cmd] = argv;
  const task = ['models', 'config'].includes(cmd) ? undefined : argv[1];
  const rest = argv.slice(['models', 'config'].includes(cmd) ? 1 : 2);
  const opts = { addDirs: [] };
  for (let i = 0; i < rest.length; i++) {
    const key = rest[i];
    const next = () => { if (!rest[i + 1] || rest[i + 1].startsWith('--')) throw new Error(usage); return rest[++i]; };
    if (cmd === 'answer' && /^\d+$/.test(key) && opts.number == null) { opts.number = Number(key); continue; }
    switch (key) {
      case '--refresh-models':
        if (!['start', 'models'].includes(cmd)) throw new Error(usage);
        opts.refreshModels = true; break;
      case '--no-wait': opts.noWait = true; break;
      case '--wait-min': opts.waitMin = Number(next()); break;
      case '--ask-wait-min': opts.askWaitMin = Number(next()); break;
      case '--cd': opts.cd = path.resolve(next()); break;
      case '--brief': opts.brief = path.resolve(next()); break;
      case '--message': opts.message = path.resolve(next()); break;
      case '--agent': opts.agent = next(); break;
      case '--model': opts.model = next(); break;
      case '--effort': opts.effort = next(); break;
      case '--_stop':
        if (cmd !== '_supervise') throw new Error(usage);
        opts.internalStop = true; break;
      case '--watch-min': opts.watchMin = Number(next()); break;
      case '--no-watch': opts.noWatch = true; break;
      case '--warn-min': opts.warnMin = Number(next()); break;
      case '--idle-min': opts.idleMin = Number(next()); break;
      case '--max-min': opts.maxMin = Number(next()); break;
      case '--retries': opts.retries = Number(next()); break;
      case '--backoff': opts.backoffMin = next().split(',').map(Number); break;
      case '--add-dir': opts.addDirs.push(path.resolve(next())); break;
      case '--network': opts.network = true; break;
      case '--no-project-doc': opts.noProjectDoc = true; break;
      case '--full-access': opts.fullAccess = true; break;
      case '--sandbox': opts.fullAccess = false; break;
      default: throw new Error(`Unknown option ${key}\n${usage}`);
    }
  }
  for (const key of ['idleMin', 'maxMin', 'warnMin', 'askWaitMin', 'waitMin', 'watchMin']) {
    if (opts[key] != null && (!Number.isFinite(opts[key]) || opts[key] <= 0)) throw new Error(usage);
  }
  if (opts.retries != null && (!Number.isInteger(opts.retries) || opts.retries < 0)) throw new Error(usage);
  if (opts.agent != null && !['codex', 'dsh', 'agy'].includes(opts.agent)) throw new Error('--agent must be codex, dsh or agy');
  if (opts.backoffMin?.some((n) => !Number.isFinite(n) || n < 0)) throw new Error(usage);
  return { cmd, task, opts };
}

const runDir = (task) => path.join(RUNS, task);
const statePath = (task) => path.join(runDir(task), 'state.json');
const loadState = (task) => JSON.parse(fs.readFileSync(statePath(task), 'utf8'));
const now = () => new Date().toISOString();
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const log = (msg) => console.log(`[foreman ${new Date().toLocaleTimeString('zh-CN', { hour12: false })}] ${msg}`);

let publishAgentStatus = false;
let crashHandler;
let lastAgentStatus;
const agentName = (state) => ({ dsh: 'DSH', agy: 'Antigravity' }[state.agent] || 'Codex');
const networkNote = (state) => state.networkSampling ? '（网络也没有数据）' : '';
const quietDuration = (ms) => ms < 60_000 ? `${Math.floor(ms / 1000)} 秒` : `${Math.floor(ms / 60_000)} 分钟`;

function displayEffort(state) {
  return state.agent === 'agy' ? /-(low|medium|high|xhigh)$/.exec(state.options.model)?.[1] ?? null : state.options.effort;
}
function writeAgentStatus(state) {
  const dir = state.options.agentStatusDir;
  if (!dir) return;
  let tmp;
  try {
    const time = Date.now();
    let status = 'running';
    let detail = state.activity || '启动中';
    switch (state.status) {
      case 'new': detail = '启动中'; break;
      case 'running': {
        if (state.waitingQuestions?.length) {
          status = 'waiting';
          detail = `等待回答：${state.waitingQuestionText}`;
          break;
        }
        const quietMs = time - Date.parse(state.lastActivityAt);
        if (quietMs > state.options.warnMin * 60_000) {
          status = 'warning';
          detail = `${quietDuration(quietMs)}没有动静${networkNote(state)} · ${detail}`;
        }
        break;
      }
      case 'waiting-retry': status = 'waiting'; break;
      case 'completed': status = 'completed'; detail = '本轮完成'; break;
      case 'stopped': status = 'stopped'; detail = '已手动停止'; break;
      case 'failed': status = 'error'; detail = state.error; break;
      case 'timeout': status = 'error'; detail = '本轮超时'; break;
    }
    detail = Array.from(String(detail ?? '').split(/[\r\n\u2028\u2029]/)[0]).slice(0, 120).join('');
    const id = `codex-${/^[A-Za-z0-9._-]+$/.test(state.task) ? state.task : createHash('sha1').update(state.task, 'utf8').digest('hex').slice(0, 12)}`;
    const effort = displayEffort(state);
    const effortName = new Map([['off', '关闭'], ['minimal', '最低档'], ['low', '低档'], ['medium', '中档'], ['high', '高档'], ['xhigh', '超高档'], ['max', '最高档']]).get(effort) ?? effort;
    const model = state.options.model;
    const label = `${agentName(state)}：${state.task}（${model && model !== 'best' ? `${model}${effort ? ' · ' : ''}` : ''}${effort ? `${effortName}推理` : ''}）`;
    const file = path.join(dir, `${id}.json`);
    if (lastAgentStatus?.file === file && lastAgentStatus.status === status &&
        lastAgentStatus.label === label && lastAgentStatus.detail === detail && time - lastAgentStatus.time < 1000) return;
    tmp = `${file}.${process.pid}.tmp`;
    fs.writeFileSync(tmp, JSON.stringify({ version: 1, label, state: status,
      detail, startedAt: state.runStartedAt, updatedAt: new Date(time).toISOString(),
      ...(Number.isInteger(state.supervisorPid) && state.supervisorPid > 0 ? { pid: state.supervisorPid } : {}) }));
    fs.renameSync(tmp, file);
    lastAgentStatus = { file, status, label, detail, time };
  } catch { /* Host status must never interrupt the task or recreate a removed session. */ }
  finally {
    if (tmp) { try { fs.unlinkSync(tmp); } catch { /* renamed or unavailable */ } }
  }
}

// Persistence failures belong to this process, never to the on-disk task schema.
const persistenceErrors = new Map();
function persistenceError(state) {
  if (!persistenceErrors.has(state.task)) persistenceErrors.set(state.task, { code: null, feedCode: null, noticeAt: null, notice: null });
  return persistenceErrors.get(state.task);
}
function persistenceNotice(state, code) {
  const error = persistenceError(state);
  const time = Date.now();
  if (error.noticeAt == null || time - error.noticeAt >= 5 * 60_000) {
    error.noticeAt = time;
    error.notice = `⚠ ${state.task} 状态写不进磁盘（${code}），已暂存在内存，恢复后会补写`;
  }
  // The warning may itself be unwritable. Keep just this snapshot until recovery.
  if (error.notice && feed(state, error.notice)) error.notice = null;
}
function saveState(state) {
  const error = persistenceError(state);
  try {
    const tmp = `${statePath(state.task)}.tmp`;
    fs.writeFileSync(tmp, JSON.stringify(state, null, 2));
    fs.renameSync(tmp, statePath(state.task));
    if (publishAgentStatus) writeAgentStatus(state);
  } catch (err) {
    error.code = err.code || 'UNKNOWN';
    persistenceNotice(state, error.code);
    return false;
  }
  if (error.code || error.feedCode) {
    persistenceNotice(state, error.code || error.feedCode);
    if (!error.notice && feed(state, `✔ ${state.task} 状态已补写`)) {
      error.code = null;
      error.feedCode = null;
    }
  }
  return true;
}

function registerCrashHandlers(state) {
  const fail = (err) => {
    const message = Array.from(String(err?.message ?? err).split(/[\r\n\u2028\u2029]/)[0]).slice(0, 200).join('');
    state.status = 'failed';
    state.error = message;
    state.endedAt = now();
    state.exitCode = 1;
    try { saveState(state); } catch { /* best effort during a crash */ }
    try { writeAgentStatus(state); } catch { /* independent of state.json */ }
    try { console.error(`[foreman] 未捕获异常，任务停下：${message}`); } catch { /* stderr may also be unavailable */ }
    process.exit(1);
  };
  process.on('uncaughtException', fail);
  process.on('unhandledRejection', fail);
  crashHandler = fail;
}

function pidAlive(pid) {
  if (!pid) return false;
  try { process.kill(pid, 0); return true; } catch (err) { return err.code !== 'ESRCH'; }
}

const terminal = (s) => ['completed', 'failed', 'timeout', 'stopped'].includes(s.status);
const short = (text, n) => Array.from(String(text ?? '').split(/\r?\n/)[0]).slice(0, n).join('');
const clock = (at = now()) => new Date(at).toLocaleTimeString('zh-CN', { hour12: false });
function duration(ms) {
  const sec = Math.max(0, Math.floor(ms / 1000));
  if (sec < 60) return `${sec}秒`;
  if (sec < 3600) return `${Math.floor(sec / 60)}分${sec % 60}秒`;
  return `${Math.floor(sec / 3600)}时${String(Math.floor(sec / 60) % 60).padStart(2, '0')}分`;
}
const feedPath = (task) => path.join(runDir(task), 'feed.log');
const feedSize = (task) => { try { return fs.statSync(feedPath(task)).size; } catch { return 0; } };
function appendFeed(state, text) {
  try {
    fs.appendFileSync(feedPath(state.task), text);
    return true;
  } catch (err) {
    persistenceError(state).feedCode = err.code || 'UNKNOWN';
    return false;
  }
}
function feed(state, text) {
  return appendFeed(state, `${clock()} ${text}\n`);
}
function activity(state, value) {
  if (!value) return;
  if (state.waitingQuestions?.length) value = `等待回答 #${state.waitingQuestions[0]}`;
  value = short(value, 100);
  if (state.activity === value) return;
  state.activity = value;
  state.activityAt = now();
  saveState(state);
}
function commandDescription(value) {
  let command = String(value || '');
  const assignment = /^[ \t]*[A-Za-z_][A-Za-z0-9_]*=(?:"[^"\n]*"|'[^'\n]*'|\\.|[^\s;&|])*[ \t]+/;
  const cd = /^\s*cd\s+(?:"[^"\n]*"|'[^'\n]*'|(?:\\.|[^\s;&])+)[ \t]*(?:&&|;)[ \t]*/;
  for (;;) {
    const before = command;
    command = command.replace(assignment, '').replace(/^(?:\/\S*\/)?(?:ba|z|da)?sh\s+-lc\s+(['"])([\s\S]*)\1$/, '$2').replace(cd, '');
    if (command === before) break;
  }
  const ownCommand = command.split(/\r?\n/)[0].match(/^(?:(?:"[^"]*\bnode"|'[^']*\bnode'|(?:\S*\/)?node)\s+)?(?:"[^"]*\/foreman\.mjs"|'[^']*\/foreman\.mjs'|(?:[^\s'";|]*\/)?foreman\.mjs)\s+(ask|inbox)\s+[^\s;&|]+(.*)$/);
  if (ownCommand) return ownCommand[1] === 'inbox' ? '查看信箱'
    : /(?:^|\s)--no-wait(?:\s|$)/.test(ownCommand[2]) ? '提问（不阻塞）' : '提问';
  const word = String.raw`(?:"[^"\n]+"|'[^'\n]+'|[^\s<>;&|]+)`;
  const firstLine = command.split(/\r?\n/)[0].trim();
  const catWrite = firstLine.match(new RegExp(`^cat\\s*(>>?)\\s*(${word})\\s*<<-?\\s*${word}\\s*$`))
    || firstLine.match(new RegExp(`^cat\\s*<<-?\\s*${word}\\s*(>>?)\\s*(${word})\\s*$`));
  const teeWrite = firstLine.match(new RegExp(`^tee\\s+(-a\\s+)?(${word})\\s*<<-?\\s*${word}\\s*$`));
  if (catWrite || teeWrite) {
    const match = catWrite || teeWrite;
    const file = match[2].replace(/^(['"])(.*)\1$/, '$2');
    const append = catWrite ? match[1] === '>>' : Boolean(match[1]);
    return `${append ? '追加' : '写入'} ${path.basename(file)}`;
  }
  const heredoc = command.match(/^(?:\/\S*\/)?(python3?|node|bash|sh|ruby)\b[^\n]*?<<-?\s*(?:'([^']+)'|"([^"]+)"|(\w+))/);
  if (heredoc) {
    const name = heredoc[1].replace(/^python3?$/, 'python');
    const delimiter = heredoc[2] || heredoc[3] || heredoc[4];
    const lines = command.split(/\r?\n/).slice(1);
    const end = lines.findIndex((line) => line.trim() === delimiter);
    const code = (end < 0 ? lines : lines.slice(0, end)).map((line) => line.trim())
      .find((line) => line && !/^(?:#|\/\/|\/\*|\*|import\b|from\b|require\b)/.test(line));
    return `${name[0].toUpperCase()}${name.slice(1)} 脚本${code && Array.from(code).length <= 40 ? `：${code}` : ''}`;
  }
  return short(command, 60);
}

function eventActivity(ev) {
  const item = ev.item;
  if (!item || !['item.started', 'item.updated', 'item.completed'].includes(ev.type)) return;
  switch (item.type) {
    case 'command_execution': {
      if (ev.type === 'item.updated') break;
      const description = commandDescription(item.command);
      if (['提问', '提问（不阻塞）', '查看信箱'].includes(description)) return description;
      return `${ev.type === 'item.started' ? '运行' : '跑完'} ${description}`;
    }
    case 'file_change': {
      const files = Array.isArray(item.changes) ? item.changes.filter((c) => typeof c?.path === 'string') : [];
      if (files.length) return `修改 ${path.basename(files[0].path)}${files.length > 1 ? ` 等 ${files.length} 个文件` : ''}`;
      break;
    }
    case 'reasoning': return '思考中';
    case 'agent_message': if (ev.type === 'item.completed') return `说明：${short(item.text, 100)}`; break;
    case 'web_search': return `搜索 ${short(item.query, 32)}`;
    case 'mcp_tool_call': return `调用工具 ${short(item.tool, 30)}`;
    case 'todo_list': return '更新计划';
  }
}
function dshActivity(ev) {
  if (ev.type === 'thinking') return '思考中';
  if (ev.type === 'text') return `说明：${short(ev.text, 100)}`;
  if (ev.type === 'tool_call') {
    if (typeof ev.input?.command === 'string') {
      const description = commandDescription(ev.input.command);
      return ['提问', '提问（不阻塞）', '查看信箱'].includes(description) ? description : `运行 ${description}`;
    }
    return `调用工具 ${short(ev.tool, 30)}`;
  }
}
function agyActivity(ev) {
  const step = ev.event === 'step_update' && ev.step_update;
  if (!step || step.step_type !== 'tool') return;
  const name = step.tool_name || step.tool_info?.name || '';
  const params = step.tool_info?.parameters || {};
  const file = Object.entries(params).find(([key, value]) => /path|file/i.test(key) && typeof value === 'string')?.[1];
  if (name === 'run_command') {
    const description = commandDescription(params.CommandLine);
    if (['提问', '提问（不阻塞）', '查看信箱'].includes(description)) return description;
    return `${step.state === 'DONE' ? '跑完' : '运行'} ${description}`;
  }
  if (['write_file', 'write_to_file', 'replace_file_content', 'multi_replace_file_content', 'sed_file', 'notebook_edit'].includes(name)) return `修改 ${path.basename(file || '')}`;
  if (name === 'view_file') return `读取 ${path.basename(file || '')}`;
  if (name === 'search_web') return `搜索 ${short(params.query || params.Query, 32)}`;
  return `调用工具 ${short(name, 30)}`;
}
function finalText(state) {
  const task = state.task;
  const last = state.turns.at(-1);
  let text;
  if (state.status === 'completed') text = `✅ ${task} 本轮完成（第 ${last.n} 轮，用时 ${duration(Date.parse(last.endedAt) - Date.parse(last.startedAt))}）`;
  else if (state.status === 'stopped') text = `■ ${task} 已手动停止`;
  else if (state.status === 'timeout') text = `⏱ ${task} 超时停下，等待处理（不算失败；用 send 让它从断点接着做）`;
  else text = `❌ ${task} 失败：${short(state.error, 1000)}`;
  let reply = '';
  try { reply = fs.readFileSync(last.files.last, 'utf8'); } catch { /* no reply */ }
  if (state.status !== 'stopped' && (state.status === 'completed' || reply.trim())) {
    const chars = Array.from(reply);
    if (chars.length > 6000) reply = chars.slice(0, 6000).join('') + `…（完整内容见 ${last.files.last}）`;
    text += `\n${clock(state.endedAt)} --- ${agentName(state)} 回复 ---\n${reply}`;
  }
  return `${clock(state.endedAt)} ${text}\n`;
}
const exitStatus = (s) => s.exitCode ?? ({ completed: 0, failed: 2, timeout: 4, stopped: 5 }[s.status]);
// Read the mailbox itself: state.waitingQuestions can lag behind an answer.
function blockingQuestion(state) {
  return mailFiles(state.task, 'questions').map(readJSON).find((q) => q && !q.noWait && q.turn === state.turns.length
    && !fs.existsSync(mailPath(state.task, 'answers', `a-${q.n}`))
    && !fs.existsSync(mailPath(state.task, 'outcomes', `q-${q.n}`))
    && Date.now() - Date.parse(q.createdAt) < q.waitMin * 60_000);
}
const shellTask = (task) => /^[\p{L}\p{N}._-]+$/u.test(task) ? task : `'${task}'`;
const runDirPrefix = () => RUNS === path.resolve(os.homedir(), '.claude', 'agent-foreman', 'runs') ? '' : `FOREMAN_RUN_DIR="${RUNS}" `;
const watchCommand = (task) => `${runDirPrefix()}node "${SCRIPT}" watch ${shellTask(task)}`;
function questionNotice(state, question) {
  const command = `${runDirPrefix()}node "${SCRIPT}"`;
  return `❓ ${state.task} 正在等你回答 #${question.n}：${short(question.text, 300)}
完整问题：${mailPath(state.task, 'questions', `q-${question.n}`)}
回答：${command} answer ${shellTask(state.task)} ${question.n} <<'EOF' … EOF；回答后用 Monitor 运行 ${command} watch ${shellTask(state.task)} 继续跟踪`;
}
async function watch(task, offset, watchMin = 29) {
  const deadline = Date.now() + watchMin * 60_000;
  let state = loadState(task);
  const unexpectedExit = () => {
    console.log(`${clock()} ❌ ${task} 监督进程意外退出，详见 ${path.join(runDir(task), 'supervisor.log')}`);
    return 6;
  };
  if (heal(state) || (state.status === 'failed' && state.exitCode === 6)) return unexpectedExit();
  if (offset == null && terminal(state)) {
    process.stdout.write(finalText(state));
    return exitStatus(state);
  }
  if (offset == null) {
    const question = blockingQuestion(state);
    if (question) { console.log(questionNotice(state, question)); return 7; }
    offset = feedSize(task);
    const waiting = state.waitingQuestions?.[0];
    const answered = waiting != null && fs.existsSync(mailPath(task, 'answers', `a-${waiting}`));
    if (waiting != null && !answered) console.log(`${clock()} … ${task} 等待回答 #${waiting}：${state.waitingQuestionText}`);
    else {
      const detail = answered ? `已回答 #${waiting}，等待 ${agentName(state)} 继续` : state.activity || '启动中';
      console.log(`${clock()} … ${task} 进行中：${detail}（已运行 ${duration(Date.now() - Date.parse(state.runStartedAt || state.createdAt))}）`);
    }
  }
  const drain = () => {
    const size = feedSize(task);
    if (size <= offset) return;
    const fd = fs.openSync(feedPath(task), 'r');
    try {
      const data = Buffer.alloc(size - offset);
      const read = fs.readSync(fd, data, 0, data.length, offset);
      process.stdout.write(data.subarray(0, read));
      offset += read;
    } finally { fs.closeSync(fd); }
  };
  for (;;) {
    state = loadState(task);
    drain();
    if (terminal(state)) return state.exitCode === 6 ? unexpectedExit() : exitStatus(state);
    const question = blockingQuestion(state);
    if (question && (state.mailboxSeen?.[`q-${question.n}`] || Date.now() >= deadline)) {
      console.log(questionNotice(state, question));
      return 7;
    }
    if (!pidAlive(state.supervisorPid)) {
      // Recheck after observing death: the supervisor may just have published its final state.
      state = loadState(task);
      drain();
      if (heal(state)) return unexpectedExit();
      if (terminal(state)) return state.exitCode === 6 ? unexpectedExit() : exitStatus(state);
      return unexpectedExit();
    }
    if (Date.now() >= deadline) {
      console.log(`⏳ ${task} 仍在运行（跟踪满 ${watchMin} 分钟自动交回，任务不受影响）；继续跟踪：${watchCommand(task)}`);
      return 8;
    }
    await sleep(Math.min(500, Math.max(1, deadline - Date.now())));
  }
}

function heal(state) {
  if (!state.task || !['running', 'waiting-retry'].includes(state.status)
      || pidAlive(state.supervisorPid) || pidAlive(state.pid)) return false;
  state.status = 'failed';
  state.error = '监工进程意外退出（未留下结束状态）';
  state.endedAt = now();
  state.exitCode = 6;
  saveState(state);
  writeAgentStatus(state);
  return true;
}

function dshAvailable(cwd) {
  const candidates = DSH_BIN.includes(path.sep) ? [path.resolve(cwd, DSH_BIN)]
    : (process.env.PATH ?? '/usr/bin:/bin').split(path.delimiter).map((dir) => path.resolve(cwd, dir, DSH_BIN));
  return candidates.some((file) => {
    try { fs.accessSync(file, fs.constants.X_OK); return fs.statSync(file).isFile(); } catch { return false; }
  });
}

// Appended to every prompt so each brief does not have to restate the run protocol.
function protocolFooter(state) {
  const dir = runDir(state.task);
  const progress = path.join(dir, 'progress.log');
  const isAgyOrchestrator = state.agent === 'agy' && /orchestrator/i.test(state.options.model ?? '');
  const askWaitMin = state.options.askWaitMin ?? DEFAULTS.askWaitMin;
  return `

---
## 运行约定（由监督脚本附加）
- 你是执行者，负责把本任务交代的目标落地。创意方向、产品取舍和审美以交代为准；交代没写到、又会影响用户看到的结果或产品走向的地方，先提问，不要自行发挥。
- 交代没提到、做的过程中却发现必须新加存储、依赖或抽象，改核心结构或接口，或者做不可逆的迁移时，先提问：说明为什么需要、有没有更简单的做法，得到回答再动手。
- 不要使用 agent-foreman 技能，不要再启动 codex${isAgyOrchestrator ? '。' : state.agent !== 'agy' && state.options.terraModel ? '；只允许下述 terra 界面操作子代理，不要派生其他子 Agent。' : '，也不要派生子 Agent。'}
- 需要的工具（如某个 MCP 服务）一开始没出现时，先等 30 秒再查一次，仍然没有才算没有；不要一看不到就下结论或改用别的办法。
- 只在工作目录 ${state.options.cd} 内写文件${state.options.addDirs.length ? `（另允许：${state.options.addDirs.join('、')}）` : ''}；不要修改其他位置，不要执行 git push；只有交代书明确要求时才 git commit，并且只提交到交代书指定的分支。
- 提问：遇到必须先确认才能继续的问题，运行下面的命令（把命令超时设为 ${askWaitMin + 5} 分钟，即 ${(askWaitMin + 5) * 60_000} 毫秒）：
  FOREMAN_RUN_DIR="${RUNS}" node "${SCRIPT}" ask ${shellTask(state.task)} <<'EOF'
  问题：写清背景、可选做法，以及你倾向哪一种、为什么
  EOF
  它会等待回答，最长 ${askWaitMin} 分钟。拿到回答后按回答继续；等不到回答时按命令打印的说明处理。
- 不影响继续工作的问题，在同一命令后加 --no-wait，写明你先按哪种做法进行，然后继续工作。
- 每完成一个步骤，运行 FOREMAN_RUN_DIR="${RUNS}" node "${SCRIPT}" inbox ${shellTask(state.task)}，查看有没有新的回答或留言，有就照办。
- 进度：每完成一个步骤，向 ${progress} 追加一行「时间 步骤 结果」。
- 长命令（安装、渲染、录屏等超过 1 分钟的命令）把输出 tee 到 ${path.join(dir, 'logs')}/ 下的日志文件，例如 \`cmd 2>&1 | tee ${path.join(dir, 'logs')}/render.log\`。
- 自己做的取舍随时追加到 ${path.join(dir, 'decisions.md')}，每条写清决定和理由。
- 到达交代中写明的关卡时停下。最终回复依次写：完成了什么、产物路径、自检结果、「我自己做的决定」（逐条列出，没有就写「无」）、未解决的问题。不要越过关卡继续做。${['dsh', 'agy'].includes(state.agent) ? '' : `
- 派任何子代理（DSH 子任务、spawn_agent 子代理）时，防止意图在转述中丢失：交代开头写明「先读完原始交代 ${dir}/run-*.prompt.md（按序号全部读完），其中的用户意图、已定的决定和范围同样约束你」，再写这次交给它的具体部分：写清原因、方案（改哪些地方、用什么做法）和已定的决定，具体怎么写由它定，只有你已确切知道做法时才写到步骤；方案要先调研才定得下来的，先让它只交调研结果和方案建议，你定了再让它做；要求它交回自己做的决定。它交回后，你先对照原始交代里的用户意图检查有没有偏，再整合；它做的决定要并入你最终回复的「我自己做的决定」，注明来自哪个子代理。`}${['dsh', 'agy'].includes(state.agent) || !dshAvailable(state.options.cd) ? '' : `
前端 UI 设计的部分（页面和组件的视觉设计、布局、样式、交互外观，以及把它们实现成前端界面代码）交给 DSH 子代理：运行下面的命令派活（命令超时设为 10 分钟；它跟踪满 29 分钟会以退出码 8 交回，按最后一行的命令继续跟踪；退出码 7 表示它在提问，按提示回答后继续跟踪）：
  FOREMAN_RUN_DIR="${RUNS}" node "${SCRIPT}" start ${shellTask(state.task)}-界面-01 --agent dsh --cd <工作目录> <<'EOF'
  交代：用户要什么、已定的设计决定（配色、字号、间距、文案原文等）、要改哪些文件、不能碰什么、做完交回什么（截图或样稿路径）
  EOF
多次派活把序号 01 递增。它在改的文件，你等它交回后再动。交回后你检查结果、整合进其余代码；其余工作（逻辑、数据、后端、测试、构建）你自己做。`}${state.agent !== 'agy' && state.options.terraModel ? `
操作电脑界面的部分（computer use：点按软件界面、操作桌面应用，包括 Blender 建模）不要自己做，交给 terra 子代理：用 spawn_agent 派子代理完成，参数 model="${state.options.terraModel}"、reasoning_effort="${state.options.effort}"、fork_turns="none"（必须是 none，否则模型设置不生效）。在任务说明里写清它需要的全部上下文、要操作什么、做完交回什么；它做完后由你检查结果再继续。其余工作（写代码、跑命令、读写文件、生成）你自己做。` : ''}${state.agent === 'agy' ? `
- 工作目录（也就是当前目录）是 ${state.options.cd}，交代里的相对路径都按它算；运行目录 ${dir} 只用来写进度、日志和决定清单。
- 提问一律用上面的 ask 命令，不要用 ask_question 工具：这里没有人能在界面上回答它。
- ask 命令会一直等到有回答，最长 ${askWaitMin} 分钟。它如果被转到后台运行，用 command_status 等它结束、拿到输出里的回答后再继续，不要没等到就往下做。
${isAgyOrchestrator ? `- 可以按需要派子代理（define_subagent、invoke_subagent 等）并行做，一两步就能做完的活自己做。派子代理时，防止意图在转述中丢失：交代开头写明「先读完原始交代 ${dir}/run-*.prompt.md（按序号全部读完），其中的用户意图、已定的决定和范围同样约束你」，再写这次交给它的具体部分和要交回的东西；要求它每完成一步向 ${progress} 追加一行「时间 子代理名 步骤 结果」，只在工作目录内写文件，遇到要确认的问题交回给你、不要用 ask_question，并交回自己做的决定。它交回后，你先对照原始交代里的用户意图检查有没有偏，再整合；它做的决定要并入你最终回复的「我自己做的决定」，注明来自哪个子代理。` : '- 不要派生子代理（browser_subagent、invoke_subagent、define_subagent 等），全部自己做。'}` : ''}`;

}

const MODEL_TIERS = { astra: 4, sol: 3, terra: 2, luna: 1 };
const MODEL_NAMES = { astra: '前沿', sol: '主力', terra: '均衡', luna: '快速' };
function modelInfo(model) {
  const match = /^gpt-(\d+(?:\.\d+)*)-([a-z][a-z0-9-]*)$/.exec(model.slug);
  return { ...model, version: match?.[1], series: match?.[2], tier: Object.hasOwn(MODEL_TIERS, match?.[2]) ? MODEL_TIERS[match[2]] : 0 };
}
function compareModels(a, b) {
  if (a.tier !== b.tier) return b.tier - a.tier;
  return compareModelVersions(a, b) || a.slug.localeCompare(b.slug);
}
function compareModelVersions(a, b) {
  const av = (a.version || '').split('.').map(Number), bv = (b.version || '').split('.').map(Number);
  for (let i = 0; i < Math.max(av.length, bv.length); i++) {
    const diff = (bv[i] || 0) - (av[i] || 0);
    if (diff) return diff;
  }
  return 0;
}
const supportsEffort = (model, effort) => Array.isArray(model.supported_reasoning_levels)
  && model.supported_reasoning_levels.some((level) => (typeof level === 'string' ? level : level?.effort) === effort);
async function modelCatalog() {
  const output = await new Promise((resolve, reject) => {
    const child = execFile(CODEX_BIN, ['debug', 'models'], {
      timeout: 20_000, killSignal: 'SIGKILL', maxBuffer: 32 * 1024 * 1024, encoding: 'utf8',
    }, (err, stdout) => err ? reject(err) : resolve(stdout));
    child.stdin.end();
  });
  const catalog = JSON.parse(output);
  if (!Array.isArray(catalog.models) || catalog.models.some((m) => !m || typeof m.slug !== 'string')) {
    throw new Error('Invalid model catalog');
  }
  const models = catalog.models.map(modelInfo).sort(compareModels);
  rememberModels(models);
  return models;
}
function printModels(models, effort) {
  console.log(`slug\t档位\t版本\t支持 ${effort}\tvisibility`);
  for (const m of models) console.log(`${m.slug}\t${Object.hasOwn(MODEL_NAMES, m.series) ? MODEL_NAMES[m.series] : '未识别'}\t${m.version || '-'}\t${supportsEffort(m, effort) ? '是' : '否'}\t${m.visibility}`);
}
const modelKeyword = (request) => typeof request === 'string' && /^(best|astra|sol|terra|luna)$/i.test(request) ? request.toLowerCase() : null;
const MODEL_RECENT_MS = 7 * 24 * 60 * 60_000;
function readModelCache(file) {
  try {
    const data = JSON.parse(fs.readFileSync(file, 'utf8'));
    if (data?.version === 2 && data.entries && typeof data.entries === 'object' && !Array.isArray(data.entries)) {
      const recentModels = Array.isArray(data.recentModels) ? data.recentModels.filter((m) => {
        if (!m || typeof m.slug !== 'string' || typeof m.lastSeenAt !== 'string') return false;
        const age = Date.now() - Date.parse(m.lastSeenAt);
        return age >= 0 && age <= MODEL_RECENT_MS;
      }).map(modelInfo) : [];
      return { entries: data.entries, recentModels };
    }
    if (typeof data?.model === 'string' && typeof data.effort === 'string' && Number.isFinite(Date.parse(data.at))) {
      return { entries: { [`best:${data.effort}`]: data }, recentModels: [] };
    }
  } catch { /* no usable cache */ }
  return { entries: {}, recentModels: [] };
}
function saveModelCache(file, cache) {
  fs.mkdirSync(RUNS, { recursive: true });
  const tmp = `${file}.${process.pid}.tmp`;
  fs.writeFileSync(tmp, JSON.stringify({ version: 2, ...cache }, null, 2));
  fs.renameSync(tmp, file);
}
function rememberModels(models) {
  const file = path.join(RUNS, '.model-resolution.json');
  const cache = readModelCache(file);
  const seen = new Map(cache.recentModels.map((m) => [m.slug, m]));
  const lastSeenAt = now();
  for (const { slug, series, version, visibility, supported_in_api, supported_reasoning_levels } of models) {
    seen.set(slug, { slug, series, version, visibility, supported_in_api, supported_reasoning_levels, lastSeenAt });
  }
  saveModelCache(file, { entries: cache.entries, recentModels: [...seen.values()] });
}
async function resolveModel(request, effort, warn, { showTable = false, refresh = false, getCatalog = modelCatalog } = {}) {
  const cacheFile = path.join(RUNS, '.model-resolution.json');
  const key = `${request}:${effort}`;
  const cache = readModelCache(cacheFile).entries[key];
  const usableCache = typeof cache?.model === 'string' && cache.model && !modelKeyword(cache.model)
    && cache.effort === effort && Number.isFinite(Date.parse(cache.at))
    && (cache.catalog == null || (Array.isArray(cache.catalog) && cache.catalog.every((m) => m && typeof m.slug === 'string')))
    && (request === 'best' || modelInfo({ slug: cache.model }).series === request);
  const age = Date.now() - Date.parse(cache?.at);
  if (!refresh && usableCache && cache.effort === effort && age >= 0 && age < 24 * 60 * 60_000) {
    if (showTable) printModels(cache.catalog || [], effort);
    return { model: cache.model, at: cache.at, cached: true };
  }
  let models;
  try { models = await getCatalog(); }
  catch {
    if (!usableCache) {
      throw new Error('无法获取 Codex 模型目录');
    }
    warn(`⚠ 获取模型目录失败，沿用上次解析的 ${cache.model}`);
    if (showTable) printModels(cache.catalog || [], effort);
    return { model: cache.model, at: cache.at, cached: true };
  }
  if (showTable) printModels(models, effort);
  const available = models.filter((m) => m.visibility === 'list' && m.supported_in_api === true && supportsEffort(m, effort));
  const unknown = available.filter((m) => m.series && !m.tier);
  if (unknown.length) warn(`⚠ 模型目录里有未识别的系列：${unknown.map((m) => m.slug).join('、')}，本次未采用，请确认是否为更高档`);
  const candidates = available.filter((m) => m.tier && (request === 'best' || m.series === request));
  let selected = candidates[0];
  if (!selected) throw new Error(request === 'best' ? `没有找到支持 ${effort} 推理档位的可用模型` : `没有可用的 ${request} 系列模型（推理档位 ${effort}）`);
  const upgrade = typeof selected.upgrade === 'string' ? selected.upgrade : selected.upgrade?.model;
  selected = candidates.find((m) => m.slug === upgrade) || selected;
  const modelCache = readModelCache(cacheFile);
  // Current eligibility and upgrade hints still govern models listed in this catalog.
  const listed = new Set(models.map((m) => m.slug));
  const recent = modelCache.recentModels.filter((m) => m.series === selected.series && m.tier
    && !listed.has(m.slug) && m.visibility === 'list' && m.supported_in_api === true && supportsEffort(m, effort))
    .sort(compareModels)[0];
  if (recent && compareModelVersions(recent, selected) < 0) {
    selected = recent;
    warn(`⚠ 模型目录可能是旧的（缺 ${selected.slug}），沿用最近见过的 ${selected.slug}`);
  }
  const at = now();
  const catalog = models.map(({ slug, series, version, visibility, supported_reasoning_levels }) =>
    ({ slug, series, version, visibility, supported_reasoning_levels }));
  modelCache.entries[key] = { model: selected.slug, effort, at, candidates: candidates.map((m) => m.slug), catalog };
  saveModelCache(cacheFile, modelCache);
  return { model: selected.slug, at, cached: false };
}

function checkUserConfig(task) {
  const configFile = path.join(process.env.CODEX_HOME || path.join(os.homedir(), '.codex'), 'config.toml');
  let config;
  try {
    config = fs.readFileSync(configFile, 'utf8');
  } catch (err) {
    if (err.code !== 'ENOENT') {
      fs.mkdirSync(runDir(task), { recursive: true });
      fs.appendFileSync(path.join(runDir(task), 'supervisor.log'), `${now()} 无法读取 Codex 配置 ${configFile}（${err.code}），跳过配置检查。\n`);
    }
    return;
  }
  // Only root-level assignments before the first TOML table are relevant.
  for (const line of config.split(/\r?\n/)) {
    if (/^\s*\[/.test(line)) break;
    const match = line.match(/^\s*model_instructions_file\s*=\s*(.*)$/);
    if (!match) continue;
    const raw = match[1].trim();
    const quoted = raw.match(/^("(?:\\.|[^"\\])*"|'[^']*')/);
    const value = quoted ? quoted[1].slice(1, -1) : raw.split('#')[0].trim();
    throw new Error(`你的 Codex 配置里设置了自定义指令文件（${value}）。按约定，监工派的任务只使用 Codex 官方指令。请先从 ${configFile} 去掉这一行再派活。`);
  }
}

/** Codex 0.169+ rejects --output-last-message paths whose parent traverses a symlink. */
function physicalPath(file) {
  const resolve = (value) => fs.realpathSync.native ? fs.realpathSync.native(value) : fs.realpathSync(value);
  try { return resolve(file); } catch {
    try { return path.join(resolve(path.dirname(file)), path.basename(file)); } catch { return file; }
  }
}

function buildArgs(state, lastFile, resume) {
  const o = state.options;
  const args = [
    'exec', '--skip-git-repo-check', '--disable', 'unbounded_connection_retries', '--json',
    '-s', o.fullAccess ? 'danger-full-access' : 'workspace-write', '-C', o.cd, '-m', o.model, '-c', `model_reasoning_effort="${o.effort}"`,
    '-c', `service_tier="${process.env.CODEX_SERVICE_TIER || 'default'}"`,
    // Codex auto-approves MCP permission prompts with full access + never (including per-tool prompt mode).
    '-c', 'approval_policy="never"',
    '--add-dir', physicalPath(runDir(state.task)), '-o', physicalPath(lastFile),
  ];
  if (o.network) args.push('-c', 'sandbox_workspace_write.network_access=true');
  if (o.noProjectDoc) args.push('-c', 'project_doc_max_bytes=0');
  for (const d of o.addDirs) args.push('--add-dir', d);
  return resume ? [...args, 'resume', state.threadId, '-'] : [...args, '-'];
}

const dshPatchPath = (state) => path.join(runDir(state.task), 'dsh.patch.yml');
function configureDsh(state) {
  const o = state.options;
  const aliases = { flash: 'deepseek-flash', pro: 'deepseek-v4-pro' };
  if (Object.hasOwn(aliases, o.model)) o.model = aliases[o.model];
  if (!['off', 'low', 'high', 'max'].includes(o.effort)) throw new Error('DSH --effort must be off, low, high or max');
  delete o.modelRequest;
  delete o.terraModel;
  // JSON strings are valid YAML scalars, including model names containing punctuation.
  fs.writeFileSync(dshPatchPath(state), `- id: agent-default-model\n  config:\n    provider: deepseek-official\n    model: ${JSON.stringify(o.model)}\n    reasoningEffort: ${JSON.stringify(o.effort)}\n- id: bash-sandbox\n  config:\n    timeoutMs: 1800000\n`);
}
function buildDshArgs(state, resume) {
  // Launcher flags must precede the first headless flag: the rest is passed through.
  const args = ['--profile', 'headless', '--patch', dshPatchPath(state), '--json', '--approval-policy', 'never',
    '--permission-mode', state.options.fullAccess ? 'danger-full-access' : 'workspace-write', '--add-dir', runDir(state.task)];
  for (const d of state.options.addDirs) args.push('--add-dir', d);
  if (resume) args.push('--session-id', state.threadId);
  return [...args, '-'];
}

async function configureAgy(state, request, resolveSeries = false) {
  const o = state.options;
  if (!['low', 'medium', 'high', 'xhigh'].includes(o.effort)) throw new Error('Antigravity --effort must be low, medium, high or xhigh');
  delete o.modelRequest;
  delete o.terraModel;
  if (request == null) return; // send inherits the resolved model without another catalog request.
  const series = request.toLowerCase();
  if (!resolveSeries && !['hydrogen', 'argon', 'carbon'].includes(series)) { o.model = request; return; }
  const output = await new Promise((resolve, reject) => {
    const child = execFile(AGY_BIN, ['models'], { cwd: o.cd, encoding: 'utf8', timeout: 20_000, killSignal: 'SIGKILL' },
      (err, stdout, stderr) => {
        if (!err) return resolve(stdout);
        const error = stderr?.trim() || err.message;
        reject(new Error(`${AGY_QUOTA.test(error) ? 'Antigravity 额度用完：' : 'agy models 失败：'}${error}`));
      });
    child.stdin.end();
  });
  const models = [...output.matchAll(/\bgemini-(\d+(?:\.\d+)*)-([a-z0-9]+(?:-[a-z0-9]+)*)-(low|medium|high|xhigh)\b/g)]
    .filter((match) => match[2] === series && match[3] === o.effort)
    .map((match) => ({ slug: match[0], version: match[1] }))
    .sort(compareModelVersions);
  if (!models.length) throw new Error(`没有可用的 ${series[0].toUpperCase() + series.slice(1)} 模型（推理档位 ${o.effort}）`);
  o.model = models[0].slug;
  feed(state, `${series} → ${o.model}（推理档位 ${o.effort}）`);
}
function buildAgyArgs(state, prompt, resume) {
  const args = ['-p', prompt, '--model', state.options.model, '--output-format', 'stream-json',
    '--dangerously-skip-permissions', '--add-dir', runDir(state.task)];
  for (const d of state.options.addDirs) args.push('--add-dir', d);
  if (!state.options.fullAccess) args.push('--sandbox');
  if (resume) args.push('--conversation', state.threadId);
  return args;
}

function newestMtime(state) {
  const dir = runDir(state.task);
  let newest = 0;
  for (const p of [path.join(dir, 'progress.log'), path.join(dir, 'logs')]) {
    try {
      const st = fs.statSync(p);
      newest = Math.max(newest, st.mtimeMs);
      if (st.isDirectory()) for (const f of fs.readdirSync(p)) newest = Math.max(newest, fs.statSync(path.join(p, f)).mtimeMs);
    } catch { /* not created yet */ }
  }
  return newest;
}

// Codex starts commands in their own process groups, so killing our group alone leaves renders running.
// Collect the whole tree before signalling: once codex dies its children are reparented and untraceable.
function processTree(rootPid, out = execFileSync('ps', ['-A', '-o', 'pid=,ppid='], { encoding: 'utf8' })) {
  const children = new Map();
  for (const line of out.trim().split('\n')) {
    const [pid, ppid] = line.trim().split(/\s+/).map(Number);
    if (!children.has(ppid)) children.set(ppid, []);
    children.get(ppid).push(pid);
  }
  const tree = [];
  const stack = [rootPid];
  while (stack.length) {
    const pid = stack.pop();
    tree.push(pid);
    stack.push(...(children.get(pid) || []));
  }
  return tree;
}

// Both process discovery and nettop run asynchronously under one three-second deadline.
function sampleNetwork(rootPid, binary, signal) {
  const run = (file, args) => new Promise((resolve, reject) => {
    execFile(file, args, { encoding: 'utf8', signal, killSignal: 'SIGKILL', maxBuffer: 1024 * 1024 },
      (err, stdout) => err ? reject(err) : resolve(stdout));
  });
  return (async () => {
    const pids = processTree(rootPid, await run('ps', ['-A', '-o', 'pid=,ppid=']));
    const output = await run(binary, ['-P', '-L', '1', '-x', '-J', 'bytes_in', ...pids.flatMap((pid) => ['-p', String(pid)])]);
    let column = -1;
    let total = 0;
    let found = false;
    for (const line of output.split(/\r?\n/)) {
      const cells = line.split(',').map((cell) => cell.trim());
      if (cells.includes('bytes_in')) { column = cells.indexOf('bytes_in'); continue; }
      const pid = Number(cells[0].match(/\.(\d+)$/)?.[1]);
      if (column < 0 || !pids.includes(pid) || !/^\d+$/.test(cells[column] || '')) continue;
      total += Number(cells[column]);
      found = true;
    }
    return found ? total : null;
  })().catch(() => null);
}

async function killTree(rootPid) {
  if (!rootPid) return;
  let pids = [rootPid];
  try { pids = processTree(rootPid); } catch { /* fall back to the root */ }
  const signal = (sig) => { for (const pid of pids.toReversed()) { try { process.kill(pid, sig); } catch { /* gone */ } } };
  signal('SIGTERM');
  const deadline = Date.now() + 10_000;
  while (pids.some(pidAlive) && Date.now() < deadline) await sleep(50);
  signal('SIGKILL');
  const killDeadline = Date.now() + 5_000;
  while (pids.some(pidAlive) && Date.now() < killDeadline) await sleep(50);
}

// Without the sandbox, writes are bounded only by the brief, so each turn reports new git changes outside the task dir.
function gitChanges(root) {
  try { return new Set(execFileSync('git', ['status', '--porcelain', '--untracked-files=all'], { cwd: root, encoding: 'utf8' }).split('\n').filter(Boolean)); } catch { return new Set(); }
}

function outOfScope(before, after, state) {
  const relative = [state.options.cd, runDir(state.task), ...state.options.addDirs].map((d) => path.relative(state.options.gitRoot, d));
  if (relative.includes('')) return [];
  const allowed = relative.filter((d) => !d.startsWith('..')).map((d) => d + '/');
  return [...after].filter((line) => !before.has(line)).filter((line) => !allowed.some((a) => line.slice(3).startsWith(a)));
}

// One codex invocation. Resolves to { status: completed|stalled|timeout|retryable|fatal, error }.
function runTurn(state, prompt) {
  const isDsh = state.agent === 'dsh';
  const isAgy = state.agent === 'agy';
  const n = state.turns.length + 1;
  const dir = runDir(state.task);
  const files = {
    prompt: path.join(dir, `turn-${n}.prompt.md`),
    events: path.join(dir, `turn-${n}.events.jsonl`),
    stderr: path.join(dir, `turn-${n}.stderr.log`),
    last: path.join(dir, `turn-${n}.last.txt`),
  };
  const fullPrompt = prompt + protocolFooter(state);
  fs.writeFileSync(files.prompt, fullPrompt);
  const resume = Boolean(state.threadId);
  const turn = { n, resume, startedAt: now(), files };
  state.turns.push(turn);
  state.intent = null;
  state.generatingSince = null;
  const nettop = process.env.FOREMAN_NETTOP || '/usr/bin/nettop';
  state.networkSampling = false;
  if (process.platform === 'darwin') {
    try { fs.accessSync(nettop, fs.constants.X_OK); state.networkSampling = true; } catch {}
  }
  activity(state, '启动中');
  state.lastActivityAt = turn.startedAt;
  feed(state, `▶ ${state.task} 第 ${n} 轮开始（${resume ? '接续会话' : '新会话'} · ${isDsh || isAgy ? `${agentName(state)} · ` : ''}${state.options.model}${displayEffort(state) ? ` · ${displayEffort(state)}` : ''}）`);

  // The sandbox already confines writes, and other sessions' edits would show up as false alarms.
  const gitBefore = state.options.fullAccess && state.options.gitRoot ? gitChanges(state.options.gitRoot) : null;
  return new Promise((resolve) => {
    const stderrFd = fs.openSync(files.stderr, 'a');
    const child = spawn(isAgy ? AGY_BIN : isDsh ? DSH_BIN : CODEX_BIN, isAgy ? buildAgyArgs(state, fullPrompt, resume) : isDsh ? buildDshArgs(state, resume) : buildArgs(state, files.last, resume), {
      cwd: state.options.cd, detached: true, stdio: ['pipe', 'pipe', stderrFd],
    });
    fs.closeSync(stderrFd);
    state.pid = child.pid;
    state.status = 'running';
    saveState(state);
    log(`turn ${n} started (pid ${child.pid}, ${resume ? `resume ${state.threadId}` : 'new thread'})`);
    child.stdin.on('error', () => { /* reported via the child 'error' event */ });
    child.stdin.end(isAgy ? undefined : fullPrompt);

    const events = fs.createWriteStream(files.events);
    const started = Date.now();
    let lastOutput = Date.now();
    let lastNetwork = 0;
    let previousBytes = null;
    let eventVersion = 0;
    let latestActivity = '启动中';
    let sampling;
    let closed = false;
    const endGeneration = () => {
      if (state.generatingSince) {
        state.generatingSince = null;
        activity(state, latestActivity);
      }
    };
    let buffer = '';
    let completed = false;
    let dshFinal = false;
    let agyResult = false;
    let agyHasResponse = false;
    let agyResultTimer;
    const agyResponses = new Map();
    let spawnFailed = false;
    let failure = '';
    let lastError = '';
    let killedFor = '';
    let warnedAt = 0;
    let cleanup = Promise.resolve();
    const observeActivity = () => {
      const active = Math.max(lastOutput, lastNetwork, newestMtime(state), waitingHeartbeat(state));
      state.lastActivityAt = new Date(active).toISOString();
      if (warnedAt && active > warnedAt) {
        feed(state, `↺ ${state.task} 恢复活动：${state.activity || '启动中'}`);
        warnedAt = 0;
      }
      saveState(state);
      return active;
    };

    const handleLine = (line) => {
      let ev;
      try { ev = JSON.parse(line); } catch { return; }
      eventVersion++;
      endGeneration();
      if (isAgy) {
        const step = ev.event === 'step_update' && ev.step_update;
        if (step?.step_type === 'agent_response' && typeof step.text_delta === 'string') {
          const text = (agyResponses.get(step.step_index) || '') + step.text_delta;
          agyResponses.set(step.step_index, text);
          state.intent = short(text, 100);
          latestActivity = `说明：${state.intent}`;
        } else latestActivity = agyActivity(ev) || latestActivity;
        activity(state, latestActivity);
        if (ev.event === 'init' && ev.conversation_id && !state.threadId) {
          state.threadId = ev.conversation_id;
          saveState(state);
          log(`conversation ${ev.conversation_id}`);
        } else if (ev.event === 'result' && !agyResult) {
          agyResult = true;
          completed = /^(success|completed|ok)$/i.test(ev.result?.status || '');
          const response = ev.result?.response;
          agyHasResponse = typeof response === 'string' && Boolean(response.trim());
          if (agyHasResponse || completed) {
            fs.writeFileSync(files.last, agyHasResponse ? response : '（Antigravity 没有给出文字回复）');
          }
          if (!completed) {
            const error = ev.result?.error || ev.result?.status || 'Antigravity result without status';
            failure = typeof error === 'string' ? error : JSON.stringify(error);
          }
          // Some versions emit result but keep the process alive. The result remains authoritative.
          if (!closed) agyResultTimer = setTimeout(() => { cleanup = killTree(child.pid); }, AGY_RESULT_EXIT_MS);
        }
        return;
      }
      if (isDsh) {
        if (ev.type === 'text') state.intent = short(ev.text, 100);
        latestActivity = dshActivity(ev) || latestActivity;
        activity(state, latestActivity);
        if (ev.type === 'session' && ev.sessionId && !state.threadId) {
          state.threadId = ev.sessionId;
          saveState(state);
          log(`session ${ev.sessionId}`);
        } else if (ev.type === 'final') {
          dshFinal = true;
          completed = ev.reason?.kind === 'completed';
          if (completed) {
            fs.writeFileSync(files.last, typeof ev.text === 'string' && ev.text.trim() ? ev.text : '（DSH 没有给出文字回复）');
          } else {
            failure = JSON.stringify(ev.reason?.error || ev.reason || 'DSH final without reason');
          }
        } else if (ev.type === 'error') lastError = ev.message || '';
        return;
      }
      if (ev.type === 'item.completed' && ev.item?.type === 'agent_message') state.intent = short(ev.item.text, 100);
      latestActivity = eventActivity(ev) || latestActivity;
      activity(state, latestActivity);
      if (ev.type === 'thread.started' && ev.thread_id && !state.threadId) {
        state.threadId = ev.thread_id;
        saveState(state);
        log(`thread ${ev.thread_id}`);
      } else if (ev.type === 'turn.completed') completed = true;
      else if (ev.type === 'turn.failed') failure = ev.error?.message || 'turn.failed';
      else if (ev.type === 'error') lastError = ev.message || '';
    };
    child.stdout.setEncoding('utf8');
    child.stdout.on('data', (chunk) => {
      lastOutput = Date.now();
      events.write(chunk);
      buffer += chunk;
      let idx;
      while ((idx = buffer.indexOf('\n')) >= 0) {
        const line = buffer.slice(0, idx);
        buffer = buffer.slice(idx + 1);
        handleLine(line);
      }
      observeActivity();
    });

    const sample = async () => {
      if (sampling || closed || killedFor) return;
      const controller = new AbortController();
      sampling = controller;
      const version = eventVersion;
      const timeout = setTimeout(() => controller.abort(), 3000);
      const bytes = await sampleNetwork(child.pid, nettop, controller.signal);
      clearTimeout(timeout);
      sampling = null;
      if (closed || killedFor) return;
      const growth = bytes == null || previousBytes == null ? 0 : bytes >= previousBytes ? bytes - previousBytes : bytes;
      if (bytes != null) previousBytes = bytes;
      if (growth >= 2048 && version === eventVersion) {
        lastNetwork = Date.now();
        state.generatingSince ||= new Date(lastNetwork).toISOString();
        activity(state, `生成中 ${duration(lastNetwork - Date.parse(state.generatingSince))}${state.intent ? ` · ${state.intent}` : ''}`);
      } else endGeneration();
      observeActivity();
    };
    const stopSampling = () => {
      sampling?.abort();
      process.removeListener('SIGTERM', stopSampling);
      process.kill(process.pid, 'SIGTERM');
    };
    process.once('SIGTERM', stopSampling);
    const networkTimer = state.networkSampling ? setInterval(sample, 5000) : null;
    if (state.networkSampling) void sample();

    const mailboxTimer = setInterval(() => { scanMailbox(state); observeActivity(); }, 1000);
    const timer = setInterval(() => {
      if (isAgy && agyResult) return;
      const active = observeActivity();
      const quietMs = Date.now() - active;
      const quiet = quietDuration(quietMs);
      if (!warnedAt && Date.now() - active > state.options.warnMin * 60_000) {
        feed(state, `⏸ ${state.task} 已 ${quiet}没有动静${networkNote(state)}`);
        warnedAt = active;
      }
      if (Date.now() - started > state.options.maxMin * 60_000) killedFor = 'timeout';
      else if (Date.now() - active > state.options.idleMin * 60_000) killedFor = 'stalled';
      if (killedFor) {
        clearInterval(timer);
        log(`turn ${n} ${killedFor}: no activity for ${Math.round((Date.now() - active) / 1000)}s, elapsed ${Math.round((Date.now() - started) / 1000)}s; killing`);
        feed(state, killedFor === 'timeout'
          ? `⏱ ${state.task} 本轮超过 ${state.options.maxMin} 分钟，已停下，等待处理`
          : `✖ ${state.task} 连续 ${quiet}没有动静${networkNote(state)}，已结束本轮`);
        cleanup = killTree(child.pid);
      }
    }, POLL_MS);

    // A spawn failure (missing binary or cwd) must end this turn as fatal instead of crashing the runner.
    child.on('error', (err) => { spawnFailed = true; failure = `spawn failed: ${err.message}`; });

    child.on('close', async (code, signal) => {
      closed = true;
      if ((isDsh || isAgy) && buffer.trim()) handleLine(buffer);
      clearTimeout(agyResultTimer);
      process.removeListener('SIGTERM', stopSampling);
      clearInterval(networkTimer);
      sampling?.abort();
      endGeneration();
      clearInterval(timer);
      clearInterval(mailboxTimer);
      await cleanup;
      scanMailbox(state);
      state.waitingQuestions = [];
      state.waitingQuestionText = null;
      await new Promise((done) => events.end(done));
      turn.endedAt = now();
      turn.exit = code ?? signal;
      let quotaText = failure;
      if (isAgy && !(agyResult && completed)) {
        try { quotaText += '\n' + fs.readFileSync(files.stderr, 'utf8'); } catch { /* none */ }
      }
      let result;
      if (isAgy && agyResult && completed) result = { status: 'completed' };
      else if (isAgy && AGY_QUOTA.test(quotaText)) result = { status: 'fatal', error: `Antigravity 额度用完（${state.options.model}）：${quotaText.trim()}` };
      else if (killedFor) result = { status: killedFor, error: `${killedFor} (watchdog)` };
      else if (isAgy && agyResult && /Malformed function call/i.test(failure)) {
        if (agyHasResponse) {
          feed(state, `⚠ ${state.task} Antigravity 回复末尾的工具调用格式出错，已收下它写好的回复按完成处理；如果回复显示还没做完，用 send 让它接着做`);
          result = { status: 'completed' };
        } else result = { status: 'retryable', error: failure.slice(0, 1000) };
      } else if (!isAgy && completed && code === 0) result = { status: 'completed' };
      else {
        let stderrTail = '';
        try { stderrTail = fs.readFileSync(files.stderr, 'utf8').split('\n').filter((l) => !/failed to load skill/.test(l)).slice(-15).join('\n'); } catch { /* none */ }
        const msg = failure || lastError || stderrTail || `exit ${code ?? signal}`;
        const dshFatal = isDsh && (spawnFailed || /MISSING_CREDENTIAL/.test(msg));
        const dshUnexpected = isDsh && !dshFinal && !spawnFailed && !failure && !lastError;
        if (isAgy) {
          const unexpected = !agyResult && !spawnFailed;
          result = { status: spawnFailed || /invalid model selection/i.test(msg) || FATAL.test(msg) ? 'fatal' : unexpected || RETRYABLE.test(msg) ? 'retryable' : 'fatal',
            error: (unexpected ? `Antigravity 异常退出（未收到 result，exit ${code ?? signal}）：${stderrTail}` : msg).slice(0, 1000) };
        } else {
          result = { status: dshFatal || FATAL.test(msg) ? 'fatal' : RETRYABLE.test(msg) || dshUnexpected ? 'retryable' : 'fatal',
            error: (dshUnexpected ? `DSH 异常退出（未收到 final，exit ${code ?? signal}）：${msg}` : msg).slice(0, 1000) };
        }
      }
      if (result.status === 'completed') {
        let reply = '';
        try { reply = fs.readFileSync(files.last, 'utf8'); } catch {}
        if (!reply.includes('我自己做的决定')) feed(state, `⚠ ${state.task} 的回复缺少「我自己做的决定」`);
      }
      turn.result = result.status;
      if (result.error) turn.error = result.error;
      const stray = gitBefore ? outOfScope(gitBefore, gitChanges(state.options.gitRoot), state) : [];
      if (stray.length) { turn.outOfScope = stray; feed(state, `⚠ ${state.task} 工作目录外出现新改动（也可能来自其他会话，请核对）：${stray.join(' | ')}`); log(`WARNING out-of-scope changes: ${stray.join(' | ')}`); }
      state.pid = null;
      saveState(state);
      log(`turn ${n} ended: ${result.status}${result.error ? ` — ${result.error.split('\n')[0]}` : ''}`);
      resolve(result);
    });
  });
}

async function runWithRetries(state, prompt) {
  const o = state.options;
  for (let attempt = 0; ; attempt++) {
    const turnPrompt = attempt === 0 ? prompt
      : `上一轮因连接中断或超时没有完成。请先检查工作目录和 progress.log 的现状，不要重做已经完成的部分，然后继续完成下面这条要求：\n\n${prompt}`;
    const result = await runTurn(state, turnPrompt);
    if (result.status === 'completed') return finish(state, 'completed', 0);
    if (result.status === 'fatal') return finish(state, 'failed', 2, result.error);
    if (result.status === 'timeout') return finish(state, 'timeout', 4, result.error);
    if (attempt >= o.retries) return finish(state, 'failed', 3, `gave up after ${attempt + 1} attempts: ${result.error}`);
    const waitMin = o.backoffMin[Math.min(attempt, o.backoffMin.length - 1)];
    const reason = result.status === 'stalled' ? '卡住' : `繁忙或断线：${short(result.error, 120)}`;
    feed(state, `⚠ ${state.task} ${reason}；${waitMin} 分钟后第 ${attempt + 1}/${o.retries} 次重试`);
    state.status = 'waiting-retry';
    activity(state, `等待重试 ${attempt + 1}/${o.retries}（${waitMin} 分钟后）`);
    saveState(state);
    log(`retry ${attempt + 1}/${o.retries} in ${waitMin} min`);
    if (!o.agentStatusDir) await sleep(waitMin * 60_000);
    else {
      const deadline = Date.now() + waitMin * 60_000;
      while (Date.now() < deadline) {
        await sleep(Math.min(10_000, POLL_MS, deadline - Date.now()));
        saveState(state);
      }
    }
  }
}

async function finish(state, status, code, error) {
  state.waitingQuestions = [];
  state.waitingQuestionText = null;
  state.status = status;
  state.error = error || null;
  state.endedAt = now();
  state.exitCode = code;
  const text = finalText(state);
  let fed = appendFeed(state, text);
  let saved = saveState(state);
  if (publishAgentStatus) writeAgentStatus(state);
  for (let retry = 0; (!saved || !fed) && retry < 12; retry++) {
    await sleep(5000);
    if (!fed) fed = appendFeed(state, text);
    saved = saveState(state);
  }
  if (!saved || !fed) {
    console.error(`[foreman] 最终状态或 feed 写盘失败，重试 12 次后放弃（${persistenceError(state).code || persistenceError(state).feedCode}）`);
    return 1;
  }
  const last = state.turns.at(-1);
  console.log(JSON.stringify({ task: state.task, status, threadId: state.threadId, turns: state.turns.length, lastMessage: last?.files.last, outOfScope: last?.outOfScope ?? [], error: state.error }, null, 2));
  return code;
}

// Mailbox layout (all payloads are atomically renamed JSON files):
// questions/q-N.json: immutable question, number, turn, pid, waitMin, createdAt, noWait.
// answers/a-N.json, notes/n-N.json: immutable text and createdAt; notes have their own sequence.
// heartbeats/q-N.json: refreshed every 5 seconds; outcomes/q-N.json: answered/timedout.
// read/answer-N.json and read/note-N.json: delivery receipts shared by ask and inbox.
// .mailbox-lock/ serializes allocation, answers and delivery; owner.json permits dead-owner recovery.
// Only the supervisor reads these into state.mailboxSeen and publishes state/feed events.
const mailPath = (task, kind, name) => path.join(runDir(task), kind, `${name}.json`);
function atomicJSON(file, value) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  const tmp = `${file}.${process.pid}.tmp`;
  try { fs.writeFileSync(tmp, JSON.stringify(value)); fs.renameSync(tmp, file); }
  finally { try { fs.unlinkSync(tmp); } catch {} }
}
function readJSON(file) { try { return JSON.parse(fs.readFileSync(file, 'utf8')); } catch { return null; } }
function mailFiles(task, kind) {
  const dir = path.join(runDir(task), kind);
  try { return fs.readdirSync(dir).filter((f) => f.endsWith('.json')).sort((a, b) => a.localeCompare(b, undefined, { numeric: true })).map((f) => path.join(dir, f)); }
  catch { return []; }
}
async function mailLock(task, fn) {
  const dir = path.join(runDir(task), '.mailbox-lock');
  for (;;) {
    try { fs.mkdirSync(dir); break; }
    catch (err) {
      if (err.code !== 'EEXIST') throw err;
      const owner = readJSON(path.join(dir, 'owner.json'));
      try {
        if (owner ? !pidAlive(owner.pid) : Date.now() - fs.statSync(dir).mtimeMs > 10_000) fs.rmSync(dir, { recursive: true, force: true });
      } catch {}
      await sleep(25);
    }
  }
  try { atomicJSON(path.join(dir, 'owner.json'), { pid: process.pid }); return await fn(); }
  finally { fs.rmSync(dir, { recursive: true, force: true }); }
}
function ensureOpen(task) {
  const state = loadState(task);
  if (terminal(state)) throw new Error('任务已结束；请用 send 把回答连同后续要求发给它。');
  return state;
}
function waitTimeout(m) {
  return `等了 ${m} 分钟没有收到回答。
- 如果这个问题涉及产品范围、用户能看到的取舍、删除数据或其他不可逆的操作：现在停下，不要自己决定。在最终回复里列出这个问题，然后结束本轮。
- 其他问题：按你提问时写的倾向做法继续，并把它写进「我自己做的决定」。之后每完成一个步骤运行 inbox，迟到的回答仍会送到。`;
}
async function mailboxCommand(cmd, task, opts) {
  loadState(task);
  if (cmd === 'inbox') return mailLock(task, () => {
    const messages = [];
    for (const kind of ['answers', 'notes']) for (const file of mailFiles(task, kind)) {
      const msg = readJSON(file);
      const receipt = mailPath(task, 'read', `${kind === 'answers' ? 'answer' : 'note'}-${msg.n}`);
      if (!fs.existsSync(receipt)) messages.push({ ...msg, kind, receipt });
    }
    messages.sort((a, b) => a.createdAt.localeCompare(b.createdAt) || a.n - b.n);
    if (!messages.length) console.log('没有新的回答或留言。');
    for (const msg of messages) {
      console.log(`${msg.kind === 'answers' ? `【回答 #${msg.n}】` : `【留言 ${msg.createdAt}】`}\n${msg.text}`);
      atomicJSON(msg.receipt, { at: now() });
    }
    return 0;
  });
  ensureOpen(task);
  const text = await readPrompt();
  const entry = await mailLock(task, () => {
    const state = ensureOpen(task);
    if (cmd === 'answer') {
      const questions = mailFiles(task, 'questions').map(readJSON);
      const q = opts.number == null ? questions.find((q) => !fs.existsSync(mailPath(task, 'answers', `a-${q.n}`))) : questions.find((q) => q.n === opts.number);
      if (!q) throw new Error(opts.number == null ? '没有未回答的问题。' : `问题 #${opts.number} 不存在。`);
      const file = mailPath(task, 'answers', `a-${q.n}`);
      if (fs.existsSync(file)) throw new Error(`问题 #${q.n} 已回答。`);
      atomicJSON(file, { n: q.n, text, createdAt: now() });
      console.log(`已回答 #${q.n}`);
      return;
    }
    const kind = cmd === 'ask' ? 'questions' : 'notes';
    const n = Math.max(0, ...mailFiles(task, kind).map((file) => readJSON(file).n)) + 1;
    const value = { n, text, createdAt: now(), turn: state.turns.length, pid: process.pid,
      noWait: Boolean(opts.noWait), waitMin: opts.waitMin ?? state.options.askWaitMin ?? DEFAULTS.askWaitMin };
    if (cmd === 'ask' && !opts.noWait) atomicJSON(mailPath(task, 'heartbeats', `q-${n}`), { at: now(), pid: process.pid });
    atomicJSON(mailPath(task, kind, `${cmd === 'ask' ? 'q' : 'n'}-${n}`), value);
    return value;
  });
  if (cmd === 'answer') return 0;
  if (cmd === 'note') { console.log('已留言'); return 0; }
  if (opts.noWait) { console.log('已记录，先按你写的倾向继续；回答会出现在 inbox 里。'); return 0; }
  const { n, waitMin } = entry;
  const heartbeat = () => atomicJSON(mailPath(task, 'heartbeats', `q-${n}`), { at: now(), pid: process.pid });
  const timer = setInterval(heartbeat, 5000);
  try {
    for (;;) {
      const done = await mailLock(task, () => {
        const answer = readJSON(mailPath(task, 'answers', `a-${n}`));
        if (answer) {
          process.stdout.write(answer.text);
          atomicJSON(mailPath(task, 'read', `answer-${n}`), { at: now() });
          atomicJSON(mailPath(task, 'outcomes', `q-${n}`), { status: 'answered' });
          return true;
        }
        if (Date.now() - Date.parse(entry.createdAt) >= waitMin * 60_000) {
          atomicJSON(mailPath(task, 'outcomes', `q-${n}`), { status: 'timedout' });
          console.log(waitTimeout(waitMin));
          return true;
        }
        return false;
      });
      if (done) return 0;
      await sleep(100);
    }
  } finally { clearInterval(timer); }
}
function pendingQuestions(state) {
  return mailFiles(state.task, 'questions').map(readJSON).filter((q) => {
    if (q.noWait || q.turn !== state.turns.length || fs.existsSync(mailPath(state.task, 'answers', `a-${q.n}`)) || fs.existsSync(mailPath(state.task, 'outcomes', `q-${q.n}`))) return false;
    const hb = readJSON(mailPath(state.task, 'heartbeats', `q-${q.n}`));
    return hb && Date.now() - Date.parse(hb.at) < 7500 && pidAlive(hb.pid);
  });
}
// A live heartbeat lease counts as activity between the five-second refreshes,
// even when the configured idle threshold is shorter than five seconds.
function waitingHeartbeat(state) { return pendingQuestions(state).length ? Date.now() : 0; }
function scanMailbox(state) {
  const seen = state.mailboxSeen ||= {};
  const publish = (key, message) => { if (!seen[key]) { feed(state, message); seen[key] = true; } };
  for (const file of mailFiles(state.task, 'questions')) {
    const q = readJSON(file);
    const chars = Array.from(q.text);
    const text = chars.length > 3000 ? chars.slice(0, 3000).join('') + `…（完整内容见 ${file}）` : q.text;
    publish(`q-${q.n}`, `❓ ${state.task} 提问 #${q.n}${q.noWait ? `（不阻塞，${agentName(state)} 先按自己的倾向继续）` : ''}：${text}`);
    const outcome = readJSON(mailPath(state.task, 'outcomes', `q-${q.n}`));
    if (outcome?.status === 'timedout') publish(`timeout-${q.n}`, `⏳ ${state.task} #${q.n} 等了 ${q.waitMin} 分钟没有回答，${agentName(state)} 按约定处理`);
    if (fs.existsSync(mailPath(state.task, 'answers', `a-${q.n}`))) publish(`a-${q.n}`, `💬 ${state.task} #${q.n} 已收到回答`);
  }
  for (const file of mailFiles(state.task, 'notes')) publish(`note-${path.basename(file)}`, `✉ ${state.task} 已收到留言`);
  const waiting = pendingQuestions(state);
  const wasWaiting = state.waitingQuestions?.length;
  state.waitingQuestions = waiting.map((q) => q.n);
  state.waitingQuestionText = waiting.length ? short(waiting[0].text, 100) : null;
  if (waiting.length) state.activity = `等待回答 #${waiting[0].n}`;
  else if (wasWaiting) state.activity = '继续工作';
  saveState(state);
}

// A short-lived session leader owns PID publication. Its exit reparents the
// supervisor before start/send begins watching; no terminal or pipe is inherited.
async function detachSupervisor(task) {
  let child;
  let fd;
  try {
    fd = fs.openSync(path.join(runDir(task), 'supervisor.log'), 'a');
    child = spawn(process.execPath, [SCRIPT, '_supervise', task], {
      detached: true, stdio: ['ignore', fd, fd],
    });
    fs.closeSync(fd);
    fd = undefined;
    await new Promise((resolve, reject) => { child.once('spawn', resolve); child.once('error', reject); });
    const state = loadState(task);
    state.supervisorPid = child.pid;
    saveState(state);
    await new Promise((resolve, reject) => process.send({ pid: child.pid }, (err) => err ? reject(err) : resolve()));
    child.unref();
    process.disconnect();
    return 0;
  } catch (err) {
    if (fd != null) fs.closeSync(fd);
    if (child?.pid) await killTree(child.pid);
    if (process.connected) process.disconnect();
    throw err;
  }
}

async function launchSupervisor(task) {
  const launcher = spawn(process.execPath, [SCRIPT, '_detach', task], {
    detached: true, stdio: ['ignore', 'ignore', 'pipe', 'ipc'],
  });
  let pid;
  let error = '';
  launcher.stderr.on('data', (chunk) => { error += chunk; });
  launcher.on('message', (message) => { if (Number.isInteger(message?.pid) && message.pid > 0) pid = message.pid; });
  await new Promise((resolve, reject) => {
    launcher.once('error', reject);
    launcher.once('close', (code) => code === 0 && pid ? resolve() : reject(new Error(error.trim() || 'Could not detach supervisor')));
  });
}

async function main() {
  const { cmd, task, opts } = parseArgs(process.argv.slice(2));
  if (cmd === 'config') {
    const config = readDefaultsConfig();
    const agent = config.defaults.agent ?? 'codex';
    console.log(JSON.stringify({ ...config, defaults: { agent, ...configuredDefaults(config.defaults, agent, config.ignored) } }, null, 2));
    return 0;
  }
  if (cmd === 'models') {
    if (opts.agent === 'agy') throw new Error('Antigravity 不使用 Codex 模型目录；请用 agy models 查看');
    if (opts.agent === 'dsh') throw new Error('DSH 不使用 Codex 模型目录；start --agent dsh --model 可用 flash、pro 或型号名');
    const effort = opts.effort || DEFAULTS.effort;
    // Share a single live catalog across the resolutions printed by this command.
    let catalog;
    const getCatalog = () => catalog ||= modelCatalog();
    const requests = opts.model == null ? ['best', DEFAULTS.model] : [opts.model];
    for (const [index, raw] of requests.entries()) {
      const request = modelKeyword(raw);
      if (!request) {
        printModels(await getCatalog(), effort);
        console.log(`${raw} → ${raw}（推理档位 ${effort}）（显式型号）`);
        continue;
      }
      const result = await resolveModel(request, effort, console.log, { showTable: index === 0, refresh: opts.refreshModels, getCatalog });
      console.log(`${request} → ${result.model}（推理档位 ${effort}）${result.cached ? `（来自缓存，${result.at} 解析）` : '（刚刚解析）'}`);
    }
    return 0;
  }
  if (!['start', 'send', 'resume', 'watch', 'status', 'stop', 'ask', 'answer', 'note', 'inbox', '_supervise', '_detach'].includes(cmd) || task == null) throw new Error(usage);
  if (!task || task.startsWith('.') || /[\s\p{White_Space}'"`/\\\0]/u.test(task) || Array.from(task).length > 40) {
    throw new Error('任务名不能包含空格、引号或斜杠，最长 40 个字');
  }
  if (['ask', 'answer', 'note', 'inbox'].includes(cmd)) return mailboxCommand(cmd, task, opts);
  if (cmd === 'watch') return watch(task, undefined, opts.watchMin);
  if (cmd === '_detach') return detachSupervisor(task);
  if (cmd === '_supervise') {
    publishAgentStatus = true;
    // The launcher owns the initial PID publication; never overwrite it with a stale snapshot.
    let state = loadState(task);
    if (opts.internalStop) {
      registerCrashHandlers(state);
      state.pid = null;
      return finish(state, 'stopped', 5);
    }
    while (state.supervisorPid !== process.pid) { await sleep(20); state = loadState(task); }
    registerCrashHandlers(state);
    state.options = { ...DEFAULTS, ...state.options };
    saveState(state);
    return runWithRetries(state, fs.readFileSync(state.pendingPrompt, 'utf8'));
  }

  if (cmd === 'status') {
    const s = loadState(task);
    heal(s);
    const idle = s.lastActivityAt ? Math.round((Date.now() - Date.parse(s.lastActivityAt)) / 1000) : null;
    const last = s.turns.at(-1);
    let lastMsg = '';
    try { lastMsg = fs.readFileSync(last.files.last, 'utf8').slice(0, 2000); } catch { /* none yet */ }
    console.log(JSON.stringify({ task, agent: s.agent || 'codex', model: s.options.model, effort: displayEffort(s), status: s.status, alive: pidAlive(s.pid), idleSeconds: idle, threadId: s.threadId, turns: s.turns.map((t) => ({ n: t.n, result: t.result ?? 'running', error: t.error })), error: s.error }, null, 2));
    if (lastMsg) console.log(`\n--- last message ---\n${lastMsg}`);
    return 0;
  }
  if (cmd === 'stop') {
    const s = loadState(task);
    heal(s);
    if (pidAlive(s.supervisorPid)) {
      try { process.kill(s.supervisorPid, 'SIGTERM'); } catch (err) { if (err.code !== 'ESRCH') throw err; }
      while (pidAlive(s.supervisorPid)) await sleep(50);
    }
    await killTree(s.pid);
    // Keep feed.log single-writer: even stop is published by an internal supervisor.
    const fd = fs.openSync(path.join(runDir(task), 'supervisor.log'), 'a');
    const publisher = spawn(process.execPath, [fileURLToPath(import.meta.url), '_supervise', task, '--_stop'], {
      stdio: ['ignore', fd, fd],
    });
    fs.closeSync(fd);
    await new Promise((resolve, reject) => {
      publisher.once('error', reject);
      publisher.once('close', (code) => code === 5 ? resolve() : reject(new Error('Could not publish stopped state')));
    });
    return 0;
  }

  const previousState = cmd === 'start' ? null : loadState(task);
  if (previousState) heal(previousState);
  const configDefaults = cmd === 'start' ? readDefaultsConfig().defaults : {};
  const agent = cmd === 'start' ? opts.agent ?? configDefaults.agent ?? 'codex' : previousState.agent || 'codex';
  if (opts.agent && opts.agent !== agent) throw new Error(`Task ${task} uses --agent ${agent}; cannot switch agents on resume.`);
  if (agent === 'codex') checkUserConfig(task);
  let state;
  let prompt;
  if (cmd === 'start') {
    if (!opts.cd) throw new Error(usage);
    if (!fs.existsSync(opts.cd) || !fs.statSync(opts.cd).isDirectory()) throw new Error(`Work dir does not exist: ${opts.cd}`);
    if (fs.existsSync(statePath(task))) throw new Error(`Task ${task} already exists; use resume or pick a new name.`);
    const { brief, message, agent: _agent, ...rest } = opts;
    state = { task, agent, createdAt: now(), status: 'new', threadId: null, pid: null, turns: [], options: { ...configuredDefaults(configDefaults, agent), fullAccess: true, ...rest } };
    try { state.options.gitRoot = execFileSync('git', ['-C', opts.cd, 'rev-parse', '--show-toplevel'], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] }).trim(); } catch { state.options.gitRoot = null; }
    prompt = await readPrompt(brief);
  } else if (cmd === 'resume' || cmd === 'send') {
    state = previousState;
    if (pidAlive(state.supervisorPid) || pidAlive(state.pid)) throw new Error(`Task ${task} 仍在运行 (still running).`);
    if (!state.threadId) throw new Error(`Task ${task} has no thread id yet; start it again under a new name.`);
    for (const k of ['idleMin', 'maxMin', 'warnMin', 'retries', 'backoffMin', 'askWaitMin', 'model', 'effort']) if (opts[k] != null) state.options[k] = opts[k];
    if (opts.model != null) delete state.options.modelRequest;
    if (opts.fullAccess != null) state.options.fullAccess = opts.fullAccess;
    prompt = await readPrompt(opts.message);
  } else {
    throw new Error(usage);
  }
  state.agent = agent;
  const agentStatusDir = process.env.MATOU_AGENT_STATUS_DIR;
  if (agentStatusDir && path.isAbsolute(agentStatusDir)) state.options.agentStatusDir = agentStatusDir;
  fs.mkdirSync(path.join(runDir(task), 'logs'), { recursive: true });
  const offset = feedSize(task);
  if (agent === 'dsh') configureDsh(state);
  else if (agent === 'agy') {
    let request = cmd === 'start' ? state.options.model : opts.model;
    let resolveSeries = false;
    const effortError = () => new Error(`${state.options.model} 没有可单独指定的推理档位；要指定档位请同时写 --model hydrogen、argon 或 carbon`);
    if (opts.effort != null && request != null && !['hydrogen', 'argon', 'carbon'].includes(request.toLowerCase())) throw effortError();
    if (cmd !== 'start' && request == null && opts.effort != null) {
      request = /^gemini-\d+(?:\.\d+)*-([a-z0-9]+(?:-[a-z0-9]+)*)-(?:low|medium|high|xhigh)$/.exec(state.options.model)?.[1];
      if (!request) throw effortError();
      resolveSeries = true;
    }
    await configureAgy(state, request, resolveSeries);
  }
  else {
    // Share the live catalog, including a failed request, across both resolutions.
    let catalog;
    const getCatalog = () => catalog ||= modelCatalog();
    const resolutionOptions = { refresh: opts.refreshModels || cmd !== 'start', getCatalog };
    const request = modelKeyword(state.options.modelRequest || state.options.model);
    if (request) {
      const result = await resolveModel(request, state.options.effort, (message) => feed(state, message), resolutionOptions);
      state.options.model = result.model;
      state.options.modelRequest = request;
      feed(state, `${request} → ${result.model}（推理档位 ${state.options.effort}）`);
    }
    delete state.options.terraModel;
    if (modelInfo({ slug: state.options.model }).series !== 'terra') {
      try {
        const result = await resolveModel('terra', state.options.effort, (message) => feed(state, message), resolutionOptions);
        state.options.terraModel = result.model;
        feed(state, `界面操作子代理 → ${result.model}（推理档位 ${state.options.effort}）`);
      } catch {
        feed(state, '⚠ 没有可用的 terra 系列模型，本任务不派 terra 子代理');
      }
    }
  }
  state.error = null;
  state.endedAt = null;
  state.exitCode = null;
  state.runStartedAt = now();
  state.lastActivityAt = state.runStartedAt;
  state.activityAt = state.runStartedAt;
  state.activity = '启动中';
  state.status = 'new';
  state.supervisorPid = null;
  state.pendingPrompt = path.join(runDir(task), `run-${state.turns.length + 1}.prompt.md`);
  fs.writeFileSync(state.pendingPrompt, prompt);
  saveState(state);
  try {
    await launchSupervisor(task);
  } catch (err) {
    const failed = loadState(task);
    await killTree(failed.supervisorPid);
    await killTree(failed.pid);
    failed.status = 'failed';
    failed.exitCode = 1;
    failed.error = `Supervisor launch failed: ${err.message}`;
    failed.endedAt = now();
    failed.supervisorPid = null;
    failed.pid = null;
    saveState(failed);
    throw err;
  }
  return opts.noWatch ? 0 : watch(task, offset, opts.watchMin);
}

async function readPrompt(file) {
  if (file) return fs.readFileSync(file, 'utf8');
  if (process.stdin.isTTY) throw new Error(usage);
  let text = '';
  process.stdin.setEncoding('utf8');
  for await (const chunk of process.stdin) text += chunk;
  const trimmed = text.trim();
  if (!trimmed) throw new Error(usage);
  if (trimmed.length < 500 && /^(\$\([^\n]*\)|`[^\n]*`)$/.test(trimmed)) {
    throw new Error(`交代书没有被展开：只收到「${trimmed}」这样的命令文本。heredoc 写成 <<'EOF' 时 shell 不会展开 $(...)，请改用 --brief <文件>，或把正文直接写进 heredoc。`);
  }
  return text;
}

main().then((code) => { process.exitCode = code; }, (err) => {
  if (crashHandler) return crashHandler(err);
  console.error(err.message);
  process.exitCode = 1;
});
