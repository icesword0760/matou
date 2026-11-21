#!/usr/bin/env node
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

const RUNS = path.resolve(process.env.FOREMAN_RUN_DIR || path.join(os.homedir(), '.claude', 'agent-foreman', 'runs'));
const color = (code, text) => `\x1b[${code}m${text}\x1b[0m`;
const oneLine = (text) => String(text ?? '').split(/\r?\n/)[0];
const terminal = (state) => ['completed', 'failed', 'timeout', 'stopped'].includes(state.status);
function pidAlive(pid) {
  if (!Number.isInteger(pid) || pid <= 0) return false;
  try { process.kill(pid, 0); return true; } catch (err) { return err.code !== 'ESRCH'; }
}
function duration(ms) {
  const sec = Math.max(0, Math.floor(ms / 1000));
  if (sec < 60) return `${sec}秒`;
  if (sec < 3600) return `${Math.floor(sec / 60)}分${sec % 60}秒`;
  return `${Math.floor(sec / 3600)}时${String(Math.floor(sec / 60) % 60).padStart(2, '0')}分`;
}
function timestamp(value, fallback) {
  const time = Date.parse(value);
  return Number.isFinite(time) ? time : fallback;
}
function hud(input) {
  try {
    const settings = JSON.parse(fs.readFileSync(path.join(os.homedir(), '.claude', 'settings.json'), 'utf8'));
    const cmd = settings?.statusLine?.command;
    if (typeof cmd !== 'string' || !cmd.trim() || cmd.includes('foreman-statusline')) return '';
    const result = spawnSync('/bin/sh', ['-c', cmd], {
      input, timeout: 1500, encoding: 'utf8', killSignal: 'SIGKILL',
    });
    if (result.error || result.status !== 0) return '';
    return result.stdout.replace(/[\r\n]+$/, '');
  } catch { return ''; }
}
const graphemes = new Intl.Segmenter(undefined, { granularity: 'grapheme' });
function displayWidth(text) {
  if (/\p{Extended_Pictographic}|\p{Regional_Indicator}|\u20e3/u.test(text)) return 2;
  let width = 0;
  for (const ch of text) {
    if (/[\p{Mark}\p{Cf}\p{Cc}]/u.test(ch)) continue;
    width += /[\u1100-\u115f\u2e80-\ua4cf\uac00-\ud7a3\uf900-\ufaff\ufe10-\ufe19\ufe30-\ufe4f\uff00-\uff60\uffe0-\uffe6\u{20000}-\u{3ffff}]/u.test(ch) ? 2 : 1;
  }
  return width;
}
function truncateName(text, columns) {
  let out = '', width = 0;
  for (const { segment } of graphemes.segment(text)) {
    const w = displayWidth(segment);
    if (width + w > columns) break;
    out += segment;
    width += w;
  }
  return out;
}

function taskLines(limit = 4, panel = false) {
  const now = Date.now();
  const tasks = [];
  for (const entry of fs.readdirSync(RUNS, { withFileTypes: true })) {
    if (!entry.isDirectory()) continue;
    try {
      const state = JSON.parse(fs.readFileSync(path.join(RUNS, entry.name, 'state.json'), 'utf8'));
      if (!state || typeof state !== 'object') continue;
      const done = terminal(state);
      // The supervisor is the watchdog: if it died, a still-running Codex is unsupervised and must be flagged.
      const sup = state.supervisorPid;
      const alive = Number.isInteger(sup) && sup > 0 ? pidAlive(sup) : pidAlive(state.pid);
      const ended = timestamp(state.endedAt, 0);
      if (done ? (now - ended < 0 || now - ended > 300_000)
        : (alive && !['running', 'waiting-retry', 'new'].includes(state.status))) continue;
      const lastTurn = Array.isArray(state.turns) ? state.turns.at(-1) : undefined;
      const started = timestamp(state.runStartedAt, timestamp(lastTurn?.startedAt, timestamp(state.createdAt, now)));
      tasks.push({ state, done, alive, ended, name: entry.name, started });
    } catch { /* A missing or partially written state must not hide other tasks. */ }
  }
  // Unfinished tasks first (oldest first), then the most recently ended, so the line cap never hides a live problem.
  tasks.sort((a, b) => a.done - b.done || (a.done ? b.ended - a.ended : a.started - b.started) || a.name.localeCompare(b.name));
  const lines = tasks.slice(0, limit).map(({ state, done, alive, ended, name, started }) => {
    let status;
    if (done) {
      const labels = { completed: [32, '✅ 本轮完成'], failed: [31, '❌ 失败'],
        timeout: [31, '⏱ 超时'], stopped: [2, '■ 已停止'] };
      status = color(...labels[state.status]);
    } else if (!alive) status = color(31, '❌ 监督进程已退出');
    else if (state.waitingQuestions?.length) status = color(33, `❓ 等待回答 #${state.waitingQuestions[0]}`);
    else if (state.status === 'waiting-retry') status = color(33, oneLine(state.activity || '等待重试'));
    else {
      const idle = now - timestamp(state.lastActivityAt, started);
      const warnMs = (Number(state.options?.warnMin) || 3) * 60_000;
      const quiet = idle < 60_000 ? `${Math.floor(idle / 1000)}秒` : `${Math.floor(idle / 60_000)}分钟`;
      status = idle > warnMs ? color(33, `⚠ ${quiet}没动静${state.networkSampling ? '（网络也没有数据）' : ''}`)
        : oneLine(state.activity || (Array.isArray(state.turns) && state.turns.length ? '运行中' : '启动中'));
    }
    const task = truncateName(oneLine(state.task || name), 14);
    const time = duration((done ? ended : now) - started);
    return panel ? `${color(2, 'Codex·')}${task} · ${time}\n  ${status}` : `${color(2, 'Codex·')}${task}  ${status} · ${time}`;
  });
  if (tasks.length > limit) lines.push(`…另有 ${tasks.length - limit} 个 Codex 任务`);
  return lines;
}

// Status rendering must never interrupt Claude Code, including closed output pipes.
process.stdout.on('error', () => { process.exitCode = 0; });
// Narrow panels wrap long lines into noise; cut each line to the terminal width instead (CJK counts as 2 columns).
function fit(line, cols) {
  if (!cols) return line;
  let width = 0;
  let out = '';
  for (const part of line.split(/(\x1b\[[0-9;]*m)/)) {
    if (part.startsWith('\x1b[')) { out += part; continue; }
    for (const { segment: ch } of graphemes.segment(part)) {
      const w = displayWidth(ch);
      if (width + w > cols - 1) return out + '…\x1b[0m';
      width += w;
      out += ch;
    }
  }
  return out;
}
if (process.argv.includes('--watch')) {
  // Standalone panel for hosts that replace Claude Code's status line (e.g. the Matou app).
  const draw = () => {
    let lines = [];
    try { lines = taskLines(12, true); } catch { /* No accessible runs directory. */ }
    const time = new Date().toLocaleTimeString('zh-CN', { hour12: false });
    const body = [color(2, `Codex 状态 · ${time} 刷新`), ...(lines.length ? lines : [color(2, '暂无 Codex 任务')])]
      .join('\n').split('\n').map((line) => fit(line, process.stdout.columns));
    process.stdout.write(`\x1b[H\x1b[2J${body.join('\n')}\n`);
  };
  draw();
  setInterval(draw, 2000);
} else try {
  let input = Buffer.alloc(0);
  try { input = fs.readFileSync(0); } catch { /* Empty input is supported. */ }
  const lines = [];
  const existing = hud(input);
  if (existing) lines.push(existing);
  try { lines.push(...taskLines()); } catch { /* No accessible runs directory. */ }
  if (lines.length) process.stdout.write(lines.join('\n') + '\n');
  process.exitCode = 0;
} catch { process.exitCode = 0; /* Best effort only. */ }
