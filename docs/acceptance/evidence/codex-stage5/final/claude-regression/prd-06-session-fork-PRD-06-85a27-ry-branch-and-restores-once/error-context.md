# Instructions

- Following Playwright test failed.
- Explain why, be concise, respect Playwright best practices.
- Provide a snippet of code with the fix, if possible.

# Test info

- Name: prd-06-session-fork.spec.ts >> PRD 06 session fork >> forks a resumable Claude conversation on its right, isolates every branch, and restores once
- Location: tests/e2e/prd-06-session-fork.spec.ts:92:3

# Error details

```
Error: expect(received).toEqual(expected) // deep equality

- Expected  - 12
+ Received  +  1

- Array [
-   Object {
-     "fromSessionId": "91f58992-b3f1-40a9-bc1f-45a84646e875",
-     "kind": "forked-from",
-     "toSessionId": "8667d5de-4fb0-49ad-8b22-b7d91ea90615",
-   },
-   Object {
-     "fromSessionId": "ab9857a6-3a35-4392-a3f7-21c8b4a610f7",
-     "kind": "forked-from",
-     "toSessionId": "91f58992-b3f1-40a9-bc1f-45a84646e875",
-   },
- ]
+ Array []
```

# Test source

```ts
  87  |     } finally {
  88  |       await fixture.close()
  89  |     }
  90  |   })
  91  |
  92  |   test('forks a resumable Claude conversation on its right, isolates every branch, and restores once', async () => {
  93  |     let fixture: MatouFixture = await launchMatou()
  94  |     const provider = join(fixture.rootDirectory, 'claude-fork-fixture.py')
  95  |     const invocationLog = join(fixture.rootDirectory, 'fork-invocations.jsonl')
  96  |     const inputDirectory = join(fixture.rootDirectory, 'provider-inputs')
  97  |     await mkdir(inputDirectory)
  98  |     await writeFile(provider, providerScript())
  99  |     await chmod(provider, 0o755)
  100 |     await writeFile(join(fixture.rootDirectory, '.zshrc'), "alias cc='claude --dangerously-skip-permissions'\n")
  101 |     const environment = {
  102 |       MATOU_CLAUDE_COMMAND: provider,
  103 |       MATOU_PRD06_INVOCATIONS: invocationLog,
  104 |       MATOU_PRD06_INPUT_DIR: inputDirectory,
  105 |       SHELL: '/bin/zsh',
  106 |       ZDOTDIR: fixture.rootDirectory
  107 |     }
  108 |     try {
  109 |       fixture = await restartMatou(fixture, { env: environment })
  110 |       await expectOnlySecondaryColorLcd(fixture)
  111 |       await fixture.app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0]?.setSize(1800, 900))
  112 |       const source = visibleSurfaces(fixture.page).first()
  113 |       const sourceSessionId = await requiredAttribute(source, 'data-session-id')
  114 |       const sourcePane = paneForSession(fixture.page, sourceSessionId)
  115 |       await expect(sourcePane.locator('.pane-title')).toHaveText('Shell')
  116 |       await openPaneMenu(sourcePane)
  117 |       await expect(fixture.page.getByRole('menuitem', { name: '⑂ Fork 会话' })).toHaveCount(0)
  118 |       await expect(fixture.page.getByRole('menuitem', { name: '↗ 独立窗口' })).toBeVisible()
  119 |       await fixture.page.locator('.detach-context-overlay').click()
  120 |       await terminalCommand(source, 'cc')
  121 |       await expect(source.locator('.xterm-rows')).toContainText('READY:provider-source-e2e')
  122 |       await terminalCommand(source, 'INITIAL_CONVERSATION')
  123 |       await expect(source.locator('.xterm-rows'))
  124 |         .toContainText('REPLY:provider-source-e2e:INITIAL_CONVERSATION')
  125 |
  126 |       await expect(sourcePane.locator('.pane-title')).toHaveText('Claude')
  127 |       await expect.poll(async () => (await invocationLines(invocationLog))[0]?.args)
  128 |         .toContain('--dangerously-skip-permissions')
  129 |       await expect(sourcePane.getByRole('button', { name: '从“Claude”创建子分支' })).toBeVisible()
  130 |       await openPaneMenu(sourcePane)
  131 |       await expect(fixture.page.getByRole('menuitem', { name: '⑂ Fork 会话' })).toBeVisible()
  132 |       await expect(fixture.page.getByRole('menuitem', { name: '↗ 独立窗口' })).toBeVisible()
  133 |       await mkdir(evidenceDirectory, { recursive: true })
  134 |       await fixture.page.locator('.hierarchy-shell').screenshot({
  135 |         path: join(evidenceDirectory, 'fork-menu.png')
  136 |       })
  137 |       await writeFile(join(evidenceDirectory, 'fork-menu.json'), JSON.stringify(
  138 |         await menuGeometry(fixture.page), null, 2
  139 |       ))
  140 |       await fixture.page.getByRole('menuitem', { name: '⑂ Fork 会话' }).click()
  141 |       await confirmCurrentWorktreeFork(fixture.page, '第一子分支')
  142 |
  143 |       await expect(visibleSurfaces(fixture.page)).toHaveCount(1)
  144 |       const firstFork = visibleSurfaces(fixture.page).filter({ hasText: 'READY:provider-fork-e2e-1' })
  145 |       await expect(firstFork).toHaveCount(1)
  146 |       const firstForkSessionId = await requiredAttribute(firstFork, 'data-session-id')
  147 |       await expect(firstFork.locator('.xterm-helper-textarea')).toBeFocused()
  148 |       const firstForkPane = paneForSession(fixture.page, firstForkSessionId)
  149 |       await expect(firstForkPane.locator('.pane-title')).toHaveText('第一子分支')
  150 |       await terminalCommand(firstFork, 'FIRST_FORK_ONLY')
  151 |       await expect(firstFork.locator('.xterm-rows')).toContainText('REPLY:provider-fork-e2e-1:FIRST_FORK_ONLY')
  152 |       expect(await readProviderInput(inputDirectory, 'provider-source-e2e'))
  153 |         .not.toContain('FIRST_FORK_ONLY')
  154 |
  155 |       await openPaneMenu(firstForkPane)
  156 |       await fixture.page.getByRole('menuitem', { name: '⑂ Fork 会话' }).click()
  157 |       await confirmCurrentWorktreeFork(fixture.page, '孙分支')
  158 |       await expect(visibleSurfaces(fixture.page)).toHaveCount(1)
  159 |       const secondFork = visibleSurfaces(fixture.page).filter({ hasText: 'READY:provider-fork-e2e-2' })
  160 |       await expect(secondFork).toHaveCount(1)
  161 |       const secondForkSessionId = await requiredAttribute(secondFork, 'data-session-id')
  162 |       await expect(secondFork.locator('.xterm-helper-textarea')).toBeFocused()
  163 |       await expect(paneForSession(fixture.page, secondForkSessionId).locator('.pane-title'))
  164 |         .toHaveText('孙分支')
  165 |       await terminalCommand(secondFork, 'SECOND_FORK_ONLY')
  166 |       await expect(secondFork.locator('.xterm-rows'))
  167 |         .toContainText('REPLY:provider-fork-e2e-2:SECOND_FORK_ONLY')
  168 |       expect(await readProviderInput(inputDirectory, 'provider-fork-e2e-1'))
  169 |         .not.toContain('SECOND_FORK_ONLY')
  170 |
  171 |       const databaseBeforeRestart = readForkDatabase(join(fixture.dataDirectory, 'matou.sqlite'))
  172 |       expect(databaseBeforeRestart.sessions).toEqual(expect.arrayContaining([
  173 |         { id: sourceSessionId, cwd: fixture.workspaceDirectory, kind: 'claude-code' },
  174 |         { id: firstForkSessionId, cwd: fixture.workspaceDirectory, kind: 'claude-code' },
  175 |         { id: secondForkSessionId, cwd: fixture.workspaceDirectory, kind: 'claude-code' }
  176 |       ]))
  177 |       expect(databaseBeforeRestart.intents).toEqual([
  178 |         {
  179 |           sessionId: firstForkSessionId, sourceSessionId,
  180 |           permissionMode: 'bypassPermissions', state: 'succeeded'
  181 |         },
  182 |         {
  183 |           sessionId: secondForkSessionId, sourceSessionId: firstForkSessionId,
  184 |           permissionMode: 'bypassPermissions', state: 'succeeded'
  185 |         }
  186 |       ])
> 187 |       expect(databaseBeforeRestart.relations).toEqual([
      |                                               ^ Error: expect(received).toEqual(expected) // deep equality
  188 |         { fromSessionId: firstForkSessionId, toSessionId: sourceSessionId, kind: 'forked-from' },
  189 |         { fromSessionId: secondForkSessionId, toSessionId: firstForkSessionId, kind: 'forked-from' }
  190 |       ])
  191 |       await fixture.page.locator('.hierarchy-shell').screenshot({
  192 |         path: join(evidenceDirectory, 'forked-conversations.png')
  193 |       })
  194 |
  195 |       const forkLaunches = (await invocationLines(invocationLog))
  196 |         .filter(({ args }) => args.includes('--fork-session'))
  197 |       expect(forkLaunches).toHaveLength(2)
  198 |       expect(forkLaunches.every(({ args }) =>
  199 |         args.includes('--dangerously-skip-permissions'))).toBe(true)
  200 |       fixture = await restartMatou(fixture, { env: environment })
  201 |       await expectOnlySecondaryColorLcd(fixture)
  202 |       await expect(visibleSurfaces(fixture.page)).toHaveCount(1)
  203 |       await expect(paneForSession(fixture.page, secondForkSessionId).locator('.xterm-rows'))
  204 |         .toContainText('READY:provider-fork-e2e-2')
  205 |       await fixture.page.getByRole('button', { name: '返回父会话' }).click()
  206 |       await expect(paneForSession(fixture.page, firstForkSessionId).locator('.xterm-rows'))
  207 |         .toContainText('READY:provider-fork-e2e-1')
  208 |       await fixture.page.getByRole('button', { name: '返回父会话' }).click()
  209 |       await expect(paneForSession(fixture.page, sourceSessionId).locator('.xterm-rows'))
  210 |         .toContainText('READY:provider-source-e2e')
  211 |       await expect.poll(async () => (await invocationLines(invocationLog)).length).toBeGreaterThanOrEqual(6)
  212 |       const restoredInvocations = await invocationLines(invocationLog)
  213 |       expect(restoredInvocations.filter(({ args }) => args.includes('--fork-session'))).toHaveLength(2)
  214 |       expect(restoredInvocations.some(({ args }) =>
  215 |         args.includes('--resume') && args.includes('provider-source-e2e')
  216 |       )).toBe(true)
  217 |       expect(restoredInvocations.some(({ args }) =>
  218 |         args.includes('--resume') && args.includes('provider-fork-e2e-1') && !args.includes('--fork-session')
  219 |       )).toBe(true)
  220 |       expect(restoredInvocations.some(({ args }) =>
  221 |         args.includes('--resume') && args.includes('provider-fork-e2e-2') && !args.includes('--fork-session')
  222 |       )).toBe(true)
  223 |
  224 |       const sourceAfterRestart = paneForSession(fixture.page, sourceSessionId).locator('.terminal-surface')
  225 |       const sourcePid = await positivePid(sourceAfterRestart)
  226 |       await expect(sourceAfterRestart).toHaveAttribute('data-pid', String(sourcePid))
  227 |
  228 |       await openPaneMenu(paneForSession(fixture.page, sourceSessionId))
  229 |       await fixture.page.getByRole('menuitem', { name: '↗ 独立窗口' }).click()
  230 |       await expect(fixture.page.getByTestId('detached-placeholder')).toContainText('已脱出')
  231 |       await expect.poll(async () => (await fixture.app.windows()).length).toBe(2)
  232 |       await expectOnlySecondaryColorLcd(fixture, 2)
  233 |       const detached = (await fixture.app.windows()).find((candidate) => candidate !== fixture.page)!
  234 |       await expect(detached.locator('.terminal-surface')).toHaveAttribute('data-session-id', sourceSessionId)
  235 |       await detached.locator('.terminal-surface').dispatchEvent('contextmenu')
  236 |       await expect(detached.getByText('⑂ Fork 会话')).toHaveCount(0)
  237 |       await detached.close()
  238 |     } finally {
  239 |       await fixture.close()
  240 |     }
  241 |   })
  242 |
  243 |   test('keeps a failed fork visible and inert instead of opening a Shell or reusing the source', async () => {
  244 |     let fixture: MatouFixture = await launchMatou()
  245 |     const provider = join(fixture.rootDirectory, 'claude-fork-failure-fixture.py')
  246 |     const invocationLog = join(fixture.rootDirectory, 'fork-failure-invocations.jsonl')
  247 |     const inputDirectory = join(fixture.rootDirectory, 'provider-inputs')
  248 |     await mkdir(inputDirectory)
  249 |     await writeFile(provider, providerScript())
  250 |     await chmod(provider, 0o755)
  251 |     try {
  252 |       fixture = await restartMatou(fixture, { env: {
  253 |         MATOU_CLAUDE_COMMAND: provider,
  254 |         MATOU_PRD06_INVOCATIONS: invocationLog,
  255 |         MATOU_PRD06_INPUT_DIR: inputDirectory,
  256 |         MATOU_PRD06_FAIL_FORK: '1'
  257 |       } })
  258 |       await expectOnlySecondaryColorLcd(fixture)
  259 |       const source = visibleSurfaces(fixture.page).first()
  260 |       await terminalCommand(source, 'claude')
  261 |       await expect(source.locator('.xterm-rows')).toContainText('READY:provider-source-e2e')
  262 |       await terminalCommand(source, 'INITIAL_CONVERSATION')
  263 |       await expect(source.locator('.xterm-rows'))
  264 |         .toContainText('REPLY:provider-source-e2e:INITIAL_CONVERSATION')
  265 |       const sourceSessionId = await requiredAttribute(source, 'data-session-id')
  266 |       await expect(paneForSession(fixture.page, sourceSessionId)
  267 |         .getByRole('button', { name: '从“Claude”创建子分支' })).toBeVisible()
  268 |       await openPaneMenu(paneForSession(fixture.page, sourceSessionId))
  269 |       await fixture.page.getByRole('menuitem', { name: '⑂ Fork 会话' }).click()
  270 |       await confirmCurrentWorktreeFork(fixture.page, '失败分支')
  271 |
  272 |       await expect(visibleSurfaces(fixture.page)).toHaveCount(0)
  273 |       const failedCard = fixture.page.locator('[data-session-card]').filter({
  274 |         has: fixture.page.locator('.fork-failure-card')
  275 |       })
  276 |       await expect(failedCard).toHaveCount(1)
  277 |       await expect(failedCard).toContainText('父会话已失效')
  278 |       await expect(failedCard).toContainText('请返回父会话继续，或移除此失败节点后新建空会话')
  279 |       await expect(failedCard).not.toContainText('provider session not found')
  280 |       const failedSessionId = await requiredAttribute(failedCard, 'data-session-card')
  281 |       const beforeInputs = await allProviderInput(fixture.rootDirectory)
  282 |       await expect(failedCard.getByRole('textbox', { name: 'Terminal input' })).toHaveCount(0)
  283 |       await expect.poll(() => allProviderInput(fixture.rootDirectory)).toBe(beforeInputs)
  284 |       expect(readForkDatabase(join(fixture.dataDirectory, 'matou.sqlite')).intents)
  285 |         .toContainEqual({ sessionId: failedSessionId, sourceSessionId, state: 'failed' })
  286 |       await mkdir(evidenceDirectory, { recursive: true })
  287 |       await fixture.page.locator('.hierarchy-shell').screenshot({
```
