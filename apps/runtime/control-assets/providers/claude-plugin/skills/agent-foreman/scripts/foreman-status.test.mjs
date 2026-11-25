import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

const script = fileURLToPath(new URL('./foreman.mjs', import.meta.url));

function runStatus(t) {
  const root = fs.mkdtempSync(path.join(path.dirname(script), '.foreman-status-'));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const runs = path.join(root, 'runs');
  return {
    runs,
    writeState(task, state) {
      const dir = path.join(runs, task);
      fs.mkdirSync(dir, { recursive: true });
      fs.writeFileSync(path.join(dir, 'state.json'), JSON.stringify(state));
    },
    status(...args) {
      const result = spawnSync(process.execPath, [script, 'status', ...args], {
        encoding: 'utf8',
        env: { ...process.env, FOREMAN_RUN_DIR: runs },
      });
      assert.equal(result.status, 0, result.error?.message || result.stderr);
      assert.equal(result.stderr, '');
      return result.stdout;
    },
  };
}

test('status without a task lists saved tasks, statuses, and agents', (t) => {
  const { runs, status, writeState } = runStatus(t);
  writeState('B-旧任务', { status: 'completed' });
  writeState('A-派活', { status: 'running', agent: 'agy' });
  fs.mkdirSync(path.join(runs, 'not-a-task'));
  fs.writeFileSync(path.join(runs, 'readme.txt'), '');

  assert.equal(status(), 'A-派活\trunning\tagy\nB-旧任务\tcompleted\tcodex\n');
});

test('status without a task succeeds for missing and empty runs directories', (t) => {
  const { runs, status } = runStatus(t);
  assert.equal(status(), '暂无任务\n');
  fs.mkdirSync(runs);
  assert.equal(status(), '暂无任务\n');
});

test('status with a task still returns its detailed JSON', (t) => {
  const { status, writeState } = runStatus(t);
  const task = '派活-01';
  writeState(task, {
    status: 'completed',
    agent: 'agy',
    options: { model: 'gemini-pro-orchestrator' },
    threadId: 'existing-thread',
    turns: [{ n: 1, result: 'completed' }],
  });

  assert.deepEqual(JSON.parse(status(task)), {
    task,
    status: 'completed',
    agent: 'agy',
    model: 'gemini-pro-orchestrator',
    effort: null,
    alive: false,
    idleSeconds: null,
    threadId: 'existing-thread',
    turns: [{ n: 1, result: 'completed' }],
    lastExternalSend: null,
    pendingOwnerNotifications: [],
  });
});
