import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { expect, it } from 'vitest'
import { ClaudeSwapEngine } from './engine'

it('refreshes safe-browser env for every operation, removes absent values and retains the shell PATH cache', async () => {
  const base = resolve('../../.cache/account-env-tests')
  await mkdir(base, { recursive: true })
  const root = await mkdtemp(join(base, 'case-'))
  try {
    const shell = join(root, 'fixture-shell'), calls = join(root, 'shell-calls')
    const script = join(root, 'fixture.cjs')
    await writeFile(shell, `#!/bin/sh\nprintf 'called\\n' >> '${calls}'\nprintf '__MATOU_PATH__=/fixture/cached-path\\n'\n`, { mode: 0o700 })
    await writeFile(script, `
      let input = ''; process.stdin.on('data', data => input += data);
      process.stdin.on('end', () => {
        const expected = JSON.parse(input).expected;
        for (const suffix of ['BIN', 'TZ', 'SEED']) {
          if (process.env['MATOU_SAFE_BROWSER_' + suffix] !== expected[suffix]) process.exit(2);
        }
        if (process.env.PATH !== '/fixture/cached-path') process.exit(3);
        console.log(JSON.stringify({ok: true, value: {accounts: []}}));
      });
    `)
    let current: NodeJS.ProcessEnv = { ...process.env, SHELL: shell }
    for (const suffix of ['BIN', 'TZ', 'SEED']) delete current[`MATOU_SAFE_BROWSER_${suffix}`]
    const engine = new ClaudeSwapEngine({ executable: process.execPath, args: [script], stateRoot: root }, () => current)
    await expect(engine.run({ action: 'inspect', expected: {} })).resolves.toEqual({ accounts: [] })
    current = { ...current, MATOU_SAFE_BROWSER_BIN: '/fixture/kernel', MATOU_SAFE_BROWSER_TZ: 'Asia/Tokyo', MATOU_SAFE_BROWSER_SEED: '42' }
    await expect(engine.run({ action: 'login-fixture', expected: { BIN: '/fixture/kernel', TZ: 'Asia/Tokyo', SEED: '42' } })).resolves.toEqual({ accounts: [] })
    current.MATOU_SAFE_BROWSER_TZ = 'Europe/Berlin'
    await expect(engine.run({ action: 'reauth-fixture', expected: { BIN: '/fixture/kernel', TZ: 'Europe/Berlin', SEED: '42' } })).resolves.toEqual({ accounts: [] })
    for (const suffix of ['BIN', 'TZ', 'SEED']) delete current[`MATOU_SAFE_BROWSER_${suffix}`]
    await expect(engine.run({ action: 'inspect', expected: {} })).resolves.toEqual({ accounts: [] })
    expect(await readFile(calls, 'utf8')).toBe('called\n')
  } finally { await rm(root, { recursive: true, force: true }) }
})
