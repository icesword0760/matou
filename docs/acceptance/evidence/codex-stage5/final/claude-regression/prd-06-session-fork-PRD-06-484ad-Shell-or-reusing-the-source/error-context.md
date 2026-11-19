# Instructions

- Following Playwright test failed.
- Explain why, be concise, respect Playwright best practices.
- Provide a snippet of code with the fix, if possible.

# Test info

- Name: prd-06-session-fork.spec.ts >> PRD 06 session fork >> keeps a failed fork visible and inert instead of opening a Shell or reusing the source
- Location: tests/e2e/prd-06-session-fork.spec.ts:243:3

# Error details

```
Error: expect(locator).toHaveCount(expected) failed

Locator:  locator('[data-session-card]').filter({ has: locator('.fork-failure-card') })
Expected: 1
Received: 0
Timeout:  15000ms

Call log:
  - Expect "toHaveCount" with timeout 15000ms
  - waiting for locator('[data-session-card]').filter({ has: locator('.fork-failure-card') })
    34 × locator resolved to 0 elements
       - unexpected value "0"

```

# Test source

```ts
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
  187 |       expect(databaseBeforeRestart.relations).toEqual([
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
> 276 |       await expect(failedCard).toHaveCount(1)
      |                                ^ Error: expect(locator).toHaveCount(expected) failed
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
  288 |         path: join(evidenceDirectory, 'fork-failure.png')
  289 |       })
  290 |       await expectOnlySecondaryColorLcd(fixture)
  291 |     } finally {
  292 |       await fixture.close()
  293 |     }
  294 |   })
  295 | })
  296 |
  297 | function visibleSurfaces(page: Page): Locator {
  298 |   // Adjacent carousel cards remain mounted and visible. These journeys assert
  299 |   // the selected branch's input surface, not the number of neighboring cards.
  300 |   return page.locator('.scene-stage:not([hidden]) .session-card.is-focused [data-testid="terminal-pane"]:visible .terminal-surface')
  301 | }
  302 |
  303 | function paneForSession(page: Page, sessionId: string): Locator {
  304 |   return page.locator(`[data-testid="terminal-pane"]:visible:has(.terminal-surface[data-session-id="${sessionId}"])`)
  305 | }
  306 |
  307 | async function openPaneMenu(pane: Locator): Promise<void> {
  308 |   // The menu overlay is mounted by this real context-menu pointer sequence.
  309 |   // Avoid retrying that already-successful sequence after the overlay appears.
  310 |   await pane.locator('.terminal-pane-header').click({
  311 |     button: 'right', position: { x: 72, y: 20 }, force: true
  312 |   })
  313 | }
  314 |
  315 | async function confirmCurrentWorktreeFork(page: Page, name: string): Promise<void> {
  316 |   await page.getByLabel('分支名称').fill(name)
  317 |   await expect(page.getByRole('radio', { name: /使用当前工作树/ })).toBeChecked()
  318 |   await page.getByRole('button', { name: '创建分支', exact: true }).click()
  319 | }
  320 |
  321 | async function terminalCommand(surface: Locator, command: string): Promise<void> {
  322 |   const textarea = surface.locator('.xterm-helper-textarea')
  323 |   await textarea.focus()
  324 |   await textarea.pressSequentially(command)
  325 |   await textarea.press('Enter')
  326 | }
  327 |
  328 | async function positivePid(surface: Locator): Promise<number> {
  329 |   let pid = 0
  330 |   await expect.poll(async () => {
  331 |     pid = Number(await surface.getAttribute('data-pid'))
  332 |     return pid
  333 |   }).toBeGreaterThan(0)
  334 |   return pid
  335 | }
  336 |
  337 | async function requiredAttribute(locator: Locator, name: string): Promise<string> {
  338 |   const value = await locator.getAttribute(name)
  339 |   if (!value) throw new Error(`Expected ${name}`)
  340 |   return value
  341 | }
  342 |
  343 | async function expectOnlySecondaryColorLcd(
  344 |   fixture: MatouFixture,
  345 |   visibleCount = 1
  346 | ): Promise<void> {
  347 |   if (primaryAcceptanceDisplayRequested()) {
  348 |     await expectVisibleWindowsOnPrimaryDisplay(fixture, visibleCount)
  349 |     return
  350 |   }
  351 |   await expect.poll(() => fixture.app.evaluate(({ BrowserWindow, screen }) => {
  352 |     const primary = screen.getPrimaryDisplay()
  353 |     const colorLcd = screen.getAllDisplays().find(({ id, internal, label }) =>
  354 |       id !== primary.id && (internal || /color\s*lcd|内建视网膜显示器/i.test(label)))
  355 |     const visible = BrowserWindow.getAllWindows().filter((window) => window.isVisible())
  356 |     const displays = visible.map((window) => screen.getDisplayMatching(window.getBounds()))
  357 |     return {
  358 |       primaryLabel: primary.label,
  359 |       colorLcdInternal: colorLcd?.internal,
  360 |       visible: visible.length,
  361 |       colorLcd: displays.filter(({ id }) => id === colorLcd?.id).length,
  362 |       primary: displays.filter(({ id }) => id === primary.id).length,
  363 |       xv272u: displays.filter(({ label }) => label === 'XV272U').length
  364 |     }
  365 |   })).toEqual({
  366 |     primaryLabel: 'XV272U', colorLcdInternal: true, visible: visibleCount,
  367 |     colorLcd: visibleCount, primary: 0, xv272u: 0
  368 |   })
  369 | }
  370 |
  371 | async function invocationLines(path: string): Promise<Array<{ args: string[]; providerId: string }>> {
  372 |   const text = await readFile(path, 'utf8').catch(() => '')
  373 |   return text.trim().split('\n').filter(Boolean).map((line) => JSON.parse(line))
  374 | }
  375 |
  376 | async function allProviderInput(root: string): Promise<string> {
```
