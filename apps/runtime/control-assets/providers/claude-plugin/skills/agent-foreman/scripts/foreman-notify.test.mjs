import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { promisify } from 'node:util';
import { test } from 'node:test';

const exec = promisify(execFile);
const script = fileURLToPath(new URL('./foreman.mjs', import.meta.url));
const task = 'notification-test';
const owner = { ref: 'session:00000000-0000-4000-8000-000000000001', title: '验收会话' };
const caller = { ref: 'session:00000000-0000-4000-8000-000000000002', title: '外部会话' };

function setup(t) {
  const root = fs.mkdtempSync(path.join(path.dirname(script), '.foreman-test-'));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const bin = path.join(root, 'bin');
  const runs = path.join(root, 'runs');
  const dir = path.join(runs, task);
  const mt = path.join(bin, 'mt');
  const dsh = path.join(bin, 'dsh');
  const callsFile = path.join(root, 'mt.jsonl');
  const prompt = path.join(root, 'prompt.md');
  const stateFile = path.join(dir, 'state.json');
  const retryPreload = path.join(root, 'retry-timer.mjs');

  fs.mkdirSync(bin);
  fs.symlinkSync(process.execPath, path.join(bin, 'node'));
  fs.writeFileSync(prompt, '用于验证通知的测试交代。\n');
  fs.writeFileSync(mt, `#!/usr/bin/env node
import fs from 'node:fs';
const args = process.argv.slice(2);
fs.appendFileSync(process.env.TEST_MT_CALLS, JSON.stringify(args) + '\\n');
if (args[0] === 'identify') {
  console.log(JSON.stringify({ target: JSON.parse(process.env.TEST_MATOU_SESSION) }));
} else if (args[0] === 'list') {
  const calls = fs.readFileSync(process.env.TEST_MT_CALLS, 'utf8').trim().split('\\n').map(JSON.parse);
  const query = calls.filter((call) => call[0] === 'list').length;
  const statuses = JSON.parse(process.env.TEST_OWNER_STATES || '["idle"]');
  const workStatus = statuses[Math.min(query - 1, statuses.length - 1)];
  if (workStatus === 'query-error') {
    console.error('CONNECTION_ERROR: status unavailable');
    process.exitCode = 1;
  } else {
    const owner = JSON.parse(process.env.TEST_OWNER);
    const targets = [{ ...owner, ref: 'session:decoy', workStatus: 'needs-input' }];
    if (workStatus !== 'missing') targets.push({ ...owner, workStatus });
    console.log(JSON.stringify({ targets, projectionRevision: 'test-projection' }));
  }
} else if (process.env.TEST_SEND_ERROR) {
  console.error(process.env.TEST_SEND_ERROR);
  process.exitCode = 1;
}
`, { mode: 0o755 });
  fs.writeFileSync(dsh, `#!/usr/bin/env node
process.stdin.resume();
process.stdin.on('end', () => {
  console.log(JSON.stringify({ type: 'session', sessionId: 'test-worker-session' }));
  setTimeout(() => {
    console.log(JSON.stringify({ type: 'final', reason: { kind: 'completed' }, text: '我自己做的决定：无' }));
  }, Number(process.env.TEST_WORKER_MS || 0));
});
`, { mode: 0o755 });
  fs.writeFileSync(retryPreload, `
if (process.argv[2] === '_notify-owner') {
  const setTimeout = globalThis.setTimeout;
  globalThis.setTimeout = (callback, milliseconds, ...args) => setTimeout(
    callback, milliseconds === 20_000 ? Number(process.env.TEST_NOTIFY_RETRY_MS || 10) : milliseconds, ...args,
  );
}
`);
  const invoke = (args, session, extraEnv = {}) => exec(process.execPath, [script, ...args], {
    timeout: 8000,
    env: {
      PATH: bin,
      FOREMAN_RUN_DIR: runs,
      FOREMAN_NETTOP: path.join(root, 'missing-nettop'),
      DSH_BIN: dsh,
      NODE_OPTIONS: `--import=${pathToFileURL(retryPreload)}`,
      TEST_MT_CALLS: callsFile,
      TEST_MATOU_SESSION: JSON.stringify(session ?? owner),
      TEST_OWNER: JSON.stringify(owner),
      ...extraEnv,
    },
  });
  const state = () => JSON.parse(fs.readFileSync(stateFile, 'utf8'));
  const start = () => invoke(['start', task, '--cd', root, '--brief', prompt, '--agent', 'dsh', '--sandbox', '--retries', '0']);
  const restart = (cmd, session, env, ...flags) => invoke([cmd, task, '--message', prompt, ...flags], session, env);
  const readLog = (name) => fs.readFileSync(path.join(dir, name), 'utf8');
  const calls = () => fs.readFileSync(callsFile, 'utf8').trim().split('\n').map((line) => JSON.parse(line));
  const status = async () => JSON.parse((await invoke(['status', task])).stdout.split('\n--- last message ---\n')[0]);
  const watch = async () => (await invoke(['watch', task])).stdout;

  async function waitFor(condition, description) {
    const deadline = Date.now() + 8000;
    while (Date.now() < deadline) {
      if (condition()) return;
      await sleep(20);
    }
    assert.fail(description);
  }

  function waitForSupervisor() {
    const pid = state().supervisorPid;
    return waitFor(() => {
      try { process.kill(pid, 0); return false; }
      catch (err) { if (err.code === 'ESRCH') return true; throw err; }
    }, '测试任务的监督进程未退出');
  }

  return { start, restart, state, stateFile, status, watch, readLog, calls, mt, waitFor, waitForSupervisor, dsh };
}

const identifyArgs = ['identify', '--json'];
const listArgs = ['list', '--all', '--json'];
const sentArgs = (session, round) => [
  'send', owner.ref, `任务 ${task} 被 ${session.title} 用 send 重新拉起一轮（第 ${round} 轮）`, '--enter',
];

test('send and resume notify only external callers, comparing session refs', { skip: process.platform === 'win32' }, async (t) => {
  const f = setup(t);
  await f.start();
  await f.waitForSupervisor();
  assert.deepEqual(f.state().ownerSession, owner);

  await f.restart('send', { ...owner, title: '验收会话已改名' });
  await f.waitForSupervisor();
  assert.deepEqual(f.calls(), [identifyArgs, identifyArgs]);
  assert.equal((await f.status()).lastExternalSend, null);

  const sameTitle = { ...caller, title: owner.title };
  await f.restart('send', sameTitle);
  await f.waitForSupervisor();
  await f.waitFor(() => f.calls().length >= 5, '外部 send 未投递');
  await f.restart('resume', caller, { TEST_OWNER_STATES: '["running"]' });
  await f.waitForSupervisor();
  await f.waitFor(() => f.calls().length >= 8, '外部 resume 未投递');
  const lastExternalSend = (await f.status()).lastExternalSend;
  assert.deepEqual(lastExternalSend.caller, caller);
  assert.equal(new Date(lastExternalSend.at).toISOString(), lastExternalSend.at);
  assert.deepEqual(f.calls(), [identifyArgs, identifyArgs, identifyArgs, listArgs, sentArgs(sameTitle, 3), identifyArgs, listArgs, sentArgs(caller, 4)]);
  const feed = f.readLog('feed.log');
  assert.deepEqual(feed.match(/✉ 外部 send by .+/g), [`✉ 外部 send by ${sameTitle.title}`, `✉ 外部 send by ${caller.title}`]);
  assert.match(feed, /第 3 轮开始[^\n]*\n[^\n]*✉ 外部 send by /);

  await f.restart('send', { ...caller, title: '第一行\r\u001b第二行' });
  await f.waitForSupervisor();
  await f.waitFor(() => f.calls().length >= 11, '带控制字符的标题未投递');
  assert.deepEqual(f.calls().at(-1), sentArgs({ title: '第一行  第二行' }, 5));
});

test('a failed delivery or unavailable mt leaves the new round intact', { skip: process.platform === 'win32' }, async (t) => {
  const f = setup(t);
  await f.start();
  await f.waitForSupervisor();
  await f.restart('send', caller, { TEST_SEND_ERROR: 'TARGET_NOT_FOUND: 发起会话已删除' });
  await f.waitForSupervisor();
  await f.waitFor(() => f.readLog('supervisor.log').includes('TARGET_NOT_FOUND'), '投递失败未记录日志');
  const delivered = await f.status();
  assert.equal(delivered.status, 'completed');
  assert.deepEqual(delivered.lastExternalSend.caller, caller);
  assert.match(f.readLog('supervisor.log'), /第 2 轮通知发起会话失败：TARGET_NOT_FOUND/);

  fs.unlinkSync(f.mt);
  await f.restart('resume', caller);
  await f.waitForSupervisor();
  const status = await f.status();
  assert.equal(status.status, 'completed');
  assert.equal(status.turns.length, 3);
  assert.deepEqual(status.lastExternalSend, delivered.lastExternalSend);
  assert.match(f.readLog('feed.log'), /✉ 外部 send（调用者未知）/);
  assert.deepEqual(f.calls(), [identifyArgs, identifyArgs, listArgs, sentArgs(caller, 2)]);
});

test('legacy tasks record the caller without guessing the owner', { skip: process.platform === 'win32' }, async (t) => {
  const f = setup(t);
  await f.start();
  await f.waitForSupervisor();
  const legacyState = f.state();
  delete legacyState.ownerSession;
  fs.writeFileSync(f.stateFile, JSON.stringify(legacyState));

  await f.restart('send', caller);
  await f.waitForSupervisor();
  const status = await f.status();
  assert.equal(status.status, 'completed');
  assert.deepEqual(status.lastExternalSend.caller, caller);
  assert.equal(f.state().ownerSession, undefined);
  assert.match(f.readLog('feed.log'), /✉ 外部 send by 外部会话/);
  assert.deepEqual(f.calls(), [identifyArgs, identifyArgs]);
});

test('an unsuccessful worker spawn does not publish or send an external notification', { skip: process.platform === 'win32' }, async (t) => {
  const f = setup(t);
  await f.start();
  await f.waitForSupervisor();
  await assert.rejects(f.restart('send', caller, { DSH_BIN: `${f.dsh}-missing` }), { code: 2 });
  await f.waitForSupervisor();
  assert.equal((await f.status()).lastExternalSend, null);
  assert.doesNotMatch(f.readLog('feed.log'), /✉ 外部 send/);
  assert.deepEqual(f.calls(), [identifyArgs, identifyArgs]);
});

test('concurrent send and resume publish one successful round and one notification', { skip: process.platform === 'win32' }, async (t) => {
  const f = setup(t);
  await f.start();
  await f.waitForSupervisor();
  const env = { TEST_WORKER_MS: '600' };
  const results = await Promise.allSettled([
    f.restart('send', caller, env, '--no-watch'),
    f.restart('resume', caller, env, '--no-watch'),
  ]);
  await f.waitForSupervisor();
  await f.waitFor(() => f.calls().length >= 4, '并发成功的轮次未投递');
  assert.equal(results.filter((result) => result.status === 'fulfilled').length, 1);
  assert.match(results.find((result) => result.status === 'rejected').reason.stderr, /仍在运行/);
  assert.equal((await f.status()).turns.length, 2);
  assert.deepEqual(f.calls(), [identifyArgs, identifyArgs, listArgs, sentArgs(caller, 2)]);
});

test('a waiting owner is retried without holding up the next round', { skip: process.platform === 'win32' }, async (t) => {
  const f = setup(t);
  await f.start();
  await f.waitForSupervisor();
  await f.restart('send', caller, {
    TEST_OWNER_STATES: '["needs-input", "starting", "running"]',
    TEST_NOTIFY_RETRY_MS: '2500',
  }, '--no-watch');
  await f.waitForSupervisor();
  await f.waitFor(() => f.calls().length >= 3, '未查询忙碌会话的状态');
  assert.deepEqual(f.calls(), [identifyArgs, identifyArgs, listArgs]);
  await f.restart('send', owner);
  await f.waitForSupervisor();
  await f.waitFor(() => f.calls().length >= 7, '发起会话恢复运行后未投递');
  assert.deepEqual(f.calls(), [identifyArgs, identifyArgs, listArgs, identifyArgs, listArgs, listArgs, sentArgs(caller, 2)]);
  assert.equal((await f.status()).turns.length, 3);
  assert.deepEqual((await f.status()).pendingOwnerNotifications, []);
});

test('blocked notifications survive completion and appear in both status and watch', { skip: process.platform === 'win32' }, async (t) => {
  for (const [workStatus, expectedStatus, queryCount, reason] of [
    ['needs-input', 'needs-input', 7, '所有者处于等待输入'],
    ['query-error', null, 7, '所有者状态无法确认'],
    ['exited', 'exited', 1, '所有者会话已退出'],
    ['error', 'error', 1, '所有者会话异常'],
    ['missing', 'not-found', 1, '找不到所有者会话'],
  ]) {
    await t.test(workStatus, async (subtest) => {
      const f = setup(subtest);
      await f.start();
      await f.waitForSupervisor();
      await f.restart('resume', caller, { TEST_OWNER_STATES: JSON.stringify([workStatus]) });
      await f.waitForSupervisor();
      const feedLine = `✉ 外部 send（${reason}，未注入）`;
      await f.waitFor(() => f.readLog('feed.log').includes(feedLine), '未记录降级通知');
      const status = await f.status();
      assert.equal(status.status, 'completed');
      assert.deepEqual(status.pendingOwnerNotifications, [{ round: 2, ...status.lastExternalSend, ownerWorkStatus: expectedStatus, reason }]);
      assert.ok((await f.watch()).includes(`第 2 轮由 ${caller.title} 拉起（${reason}，未注入）`));
      assert.deepEqual(f.calls(), [identifyArgs, identifyArgs, ...Array(queryCount).fill(listArgs)]);
    });
  }
});
