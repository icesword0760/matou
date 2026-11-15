# Instructions

- Following Playwright test failed.
- Explain why, be concise, respect Playwright best practices.
- Provide a snippet of code with the fix, if possible.

# Test info

- Name: codex-acceptance.spec.ts >> Codex packaged acceptance — real CLI and explicit controlled fixtures >> known issues: a real Codex model error notifies once and marks the card error; startup upgrade is disabled
- Location: tests/e2e/codex-acceptance.spec.ts:653:3

# Error details

```
Error: expect(received).toBe(expected) // Object.is equality

Expected: true
Received: false

Call Log:
- Timeout 20000ms exceeded while waiting on the predicate
```

# Test source

```ts
  580 |       await expect(hud).toHaveAttribute('data-hud-mode', 'agent')
  581 |       await expect(hud.locator('.status-model')).toContainText(/gpt|GPT/)
  582 |       await expect(hud).not.toContainText(/5h|Weekly/)
  583 |       await completeTurn(h, sid, surface, 'Read README.md using a shell tool, then reply only TOOL_OK.', /TOOL_OK/)
  584 |       await expect(hud.locator('.status-context-ring')).toBeVisible()
  585 |       await saveScreenshot(hud, '04-hud.png')
  586 |       await hud.getByRole('button', { name: 'AGENTS.md', exact: true }).click()
  587 |       const editor = h.page.getByRole('dialog', { name: 'AGENTS.md', exact: true })
  588 |       await editor.getByRole('textbox').fill('# Project Instructions\nKeep all changes inside this test repo.\n<!-- Saved by packaged UI -->\n')
  589 |       await editor.getByRole('button', { name: '保存', exact: true }).click()
  590 |       await expect.poll(() => readFile(join(dirs.repoDir, 'AGENTS.md'), 'utf8')).toContain('Saved by packaged UI')
  591 |       await saveScreenshot(h.page, '10-agents-saved.png')
  592 |
  593 |       const pane = paneForSession(h.page, sid)
  594 |       await expect.poll(() => pane.locator('.pane-title').innerText(), { timeout: 30_000 }).not.toMatch(/^(Shell|Codex)$/)
  595 |       const automaticTitle = await pane.locator('.pane-title').innerText()
  596 |       await openPaneMenu(pane)
  597 |       await h.page.getByRole('menuitem', { name: '重命名…' }).click()
  598 |       await h.page.getByRole('dialog').locator('input').first().fill('手工锁定-Codex')
  599 |       await h.page.getByRole('dialog').locator('input').first().press('Enter')
  600 |       await completeTurn(h, sid, surface, 'Reply only TITLE_LOCK_OK.', /TITLE_LOCK_OK/)
  601 |       await expect(pane.locator('.pane-title')).toHaveText('手工锁定-Codex')
  602 |       await openPaneMenu(pane)
  603 |       await h.page.getByRole('menuitem', { name: '恢复 Codex 自动标题' }).click()
  604 |       await expect(pane.locator('.pane-title')).toHaveText(automaticTitle)
  605 |       await saveScreenshot(h.page, '06-title.png')
  606 |
  607 |       // Completion notification while focus is on another genuine card.
  608 |       await sendTerminalLine(surface, 'Read README.md with a shell tool and reply only NOTIFY_DONE.')
  609 |       const peer = await createSiblingShellAndFocus(h.page, sid)
  610 |       await expect(pane).toHaveClass(/has-notification/, { timeout: 150_000 })
  611 |       await h.page.getByRole('button', { name: '通知中心', exact: true }).click()
  612 |       const center = h.page.getByRole('region', { name: '通知中心', exact: true })
  613 |       await expect(center).toContainText('Codex')
  614 |       await saveScreenshot(center, '03-completion-notification.png')
  615 |       await h.page.getByRole('button', { name: '通知中心', exact: true }).click()
  616 |       await pane.click()
  617 |       await expect.poll(() => readCurrentWorkStatus(h.dataDir, sid)?.status, { timeout: 150_000 }).toBe('idle')
  618 |       await saveProof('core-front.json', { sid, providerId, automaticTitle,
  619 |         model: await hud.locator('.status-model').innerText(), peerId: peer.sessionId, turns: h.turnProofs })
  620 |
  621 |       const modes = []
  622 |       for (const mode of ['Accept Edits', 'Plan Mode', 'Bypass Permissions', 'Default']) {
  623 |         const processChange = await changeMode(h, sid, mode)
  624 |         await completeTurn(h, sid, pane.locator('.terminal-surface'),
  625 |           'Which codeword did I ask you to remember? Reply only that codeword.', /opal-harbor-628/)
  626 |         expect(await waitForProviderSessionId(h.dataDir, sid)).toBe(providerId)
  627 |         const context = (await rolloutEvents(h, providerId)).findLast((e) => e.type === 'turn_context')?.payload
  628 |         const expectedSandbox = mode === 'Accept Edits' ? 'workspace-write'
  629 |           : mode === 'Bypass Permissions' ? 'danger-full-access' : 'read-only'
  630 |         expect(context?.sandbox_policy?.type).toBe(expectedSandbox)
  631 |         modes.push({ mode, ...processChange, sandbox: context.sandbox_policy.type, approval: context.approval_policy })
  632 |         await saveProof('modes-progress.json', { sid, providerId, modes, turns: h.turnProofs })
  633 |       }
  634 |       await saveProof('modes-and-core.json', { sid, providerId, peerId: peer.sessionId, modes, automaticTitle,
  635 |         actualModel: await hud.locator('.status-model').innerText(), db: readDbRows(h.dataDir, sid) })
  636 |       const turnsBeforeRestart = h.turnProofs
  637 |       await h.close()
  638 |       h = await launchPackagedApp(dirs)
  639 |       h.turnProofs = turnsBeforeRestart
  640 |       const restored = paneForSession(h.page, sid).locator('.terminal-surface')
  641 |       await positivePid(restored)
  642 |       await h.page.waitForTimeout(10_000)
  643 |       expect(readCurrentWorkStatus(h.dataDir, sid)?.status).toBe('starting')
  644 |       await saveScreenshot(h.page, '08-static-restart.png')
  645 |       await completeTurn(h, sid, restored, 'Which codeword did I ask you to remember? Reply only that codeword.', /opal-harbor-628/)
  646 |       expect(await waitForProviderSessionId(h.dataDir, sid)).toBe(providerId)
  647 |       await expect(paneForSession(h.page, sid).locator('.provider-restore-banner')).toHaveCount(0)
  648 |       await saveScreenshot(h.page, '08-restart-memory.png')
  649 |       await saveProof('restart.json', { sid, providerId, bindingAfter: readDbRows(h.dataDir, sid).bindings })
  650 |     } finally { await dispose(h) }
  651 |   })
  652 |
  653 |   test('known issues: a real Codex model error notifies once and marks the card error; startup upgrade is disabled', async () => {
  654 |     const dirs = await createIsolatedWorkspace('model-error', { copyAuth: false })
  655 |     let pending: ServerResponse | undefined
  656 |     const endpoint = createServer((_request, response) => { pending = response })
  657 |     await new Promise<void>(resolve => endpoint.listen(0, '127.0.0.1', resolve))
  658 |     const address = endpoint.address() as { port: number }
  659 |     const configPath = join(dirs.rootDir, 'codex-home/config.toml')
  660 |     let config = (await readFile(configPath, 'utf8')).replace('model_provider = "openai"', 'model_provider = "local_probe"')
  661 |       .replace('check_for_update_on_startup = false\n', '')
  662 |     config += `\n[model_providers.local_probe]\nname = "Local failure probe"\nbase_url = "http://127.0.0.1:${address.port}/v1"\nrequires_openai_auth = false\nwire_api = "responses"\nrequest_max_retries = 0\nstream_max_retries = 0\n`
  663 |     await writeFile(configPath, config)
  664 |     // Neither fixture config nor wrapper disables updates: the product must.
  665 |     const wrapper = join(dirs.rootDir, 'codex-wrapper')
  666 |     await writeFile(wrapper, (await readFile(wrapper, 'utf8')).replace(' -c check_for_update_on_startup=false', ''))
  667 |     await writeFile(join(dirs.rootDir, 'codex-home/version.json'), JSON.stringify({ latest_version: '999.0.0',
  668 |       last_checked_at: new Date().toISOString(), dismissed_version: null }))
  669 |     const h = await launchPackagedApp(dirs)
  670 |     try {
  671 |       const { sessionId: sid, surface, dagPage } = await launchCodex(h)
  672 |       await sendTerminalLine(surface, 'Reply only MODEL_PROBE_READY.')
  673 |       const peer = await createSiblingShellAndFocus(h.page, sid)
  674 |       await expect.poll(() => Boolean(pending), { timeout: 20_000 }).toBe(true)
  675 |       const body = JSON.stringify({ error: { message: 'The model matou-probe-missing does not exist',
  676 |         type: 'invalid_request_error', code: 'model_not_found' } })
  677 |       pending!.writeHead(404, { 'content-type': 'application/json' }); pending!.end(body)
  678 |       const providerId = await waitForProviderSessionId(h.dataDir, sid)
  679 |       await expect.poll(async () => (await rolloutEvents(h, providerId))
> 680 |         .some(e => e.type === 'event_msg' && e.payload?.type === 'task_complete' && e.payload.error), { timeout: 20_000 }).toBe(true)
      |                                                                                                                            ^ Error: expect(received).toBe(expected) // Object.is equality
  681 |       await expect.poll(() => readCurrentWorkStatus(h.dataDir, sid)?.status, { timeout: 10_000 }).toBe('error')
  682 |       await waitForDagNodeStatus(dagPage, sid, 'status-error')
  683 |       await expect(paneForSession(h.page, sid)).toHaveClass(/has-notification/)
  684 |       await h.page.getByRole('button', { name: '通知中心', exact: true }).click()
  685 |       const center = h.page.getByRole('region', { name: '通知中心', exact: true })
  686 |       await expect(center.locator('.notification-item')).toHaveCount(1)
  687 |       await expect(center.locator('.notification-item__title')).toHaveText('Codex')
  688 |       await expect(center).toContainText('matou-probe-missing does not exist')
  689 |       await expect(surface.locator('.xterm-rows')).toContainText('unexpected status 404')
  690 |       await expect(surface.locator('.xterm-rows')).not.toContainText('Update available')
  691 |       await saveScreenshot(h.page, 'error-notification.png')
  692 |       await saveScreenshot(dagPage, 'error-dag.png')
  693 |       await saveProof('model-error-ui.json', { sid, peerId: peer.sessionId, providerId,
  694 |         status: readCurrentWorkStatus(h.dataDir, sid)?.status,
  695 |         failedTurn: (await rolloutEvents(h, providerId)).findLast(e => e.payload?.type === 'task_complete'),
  696 |         notificationCount: await center.locator('.notification-item').count(),
  697 |         notificationTitle: await center.locator('.notification-item__title').innerText(),
  698 |         startupUpgradePrompt: false, wrapperDisablesUpdates: false, configDisablesUpdates: false })
  699 |     } finally {
  700 |       await dispose(h)
  701 |       endpoint.closeAllConnections(); await new Promise<void>(resolve => endpoint.close(() => resolve()))
  702 |     }
  703 |   })
  704 |
  705 |   test('known issues: history_base ancestor appears in preview and search; missing parent is labeled', async () => {
  706 |     const dirs = await createIsolatedWorkspace('ancestor-history', { copyAuth: false })
  707 |     const folder = join(dirs.rootDir, 'codex-home/sessions/2026/11/15')
  708 |     await mkdir(folder, { recursive: true })
  709 |     const id = (n: number) => `01a1ddd6-4064-70e1-a006-${String(n).padStart(12, '0')}`
  710 |     const message = (text: string, n: number) => ({ timestamp: `2026-11-15T12:00:0${n}Z`, type: 'event_msg',
  711 |       payload: { type: 'item_completed', turn_id: `t-${n}`, item: { type: 'AgentMessage', id: `a-${n}`,
  712 |         content: [{ text }], phase: 'final_answer' } } })
  713 |     for (const n of [1, 2, 3]) {
  714 |       const records = [{ type: 'session_meta', payload: { id: id(n), cwd: dirs.repoDir,
  715 |         ...(n === 1 ? {} : { history_base: { thread_id: id(n === 2 ? 1 : 99) } }) } },
  716 |         message(n === 1 ? 'ancestral-needle-913' : `current-child-${n}`, n)]
  717 |       await writeFile(join(folder, `rollout-${id(n)}.jsonl`), records.map(r => JSON.stringify(r)).join('\n') + '\n')
  718 |     }
  719 |     const h = await launchPackagedApp(dirs)
  720 |     try {
  721 |       const sid = (await visibleFocusedSurface(h.page).getAttribute('data-session-id'))!
  722 |       await paneForSession(h.page, sid).locator('.pane-load-session').click()
  723 |       const loader = h.page.getByRole('dialog')
  724 |       await loader.getByRole('tab', { name: 'Codex', exact: true }).click()
  725 |       const sessionsSearch = loader.locator('[data-search-scope="sessions"] input')
  726 |       await sessionsSearch.fill(id(2))
  727 |       await expect(loader.locator('.session-loader-result-main')).toHaveCount(1)
  728 |       await loader.locator('.session-loader-result-main').click()
  729 |       await expect(loader.locator('.session-loader-events')).toContainText('ancestral-needle-913')
  730 |       await expect(loader.locator('.session-loader-events')).toContainText('current-child-2')
  731 |       await loader.locator('.session-loader-content-search-group input').fill('ancestral-needle-913')
  732 |       await expect(loader.locator('.session-loader-events')).toContainText('ancestral-needle-913')
  733 |       await saveScreenshot(h.page, 'ancestor-preview-search.png')
  734 |       await loader.locator('.session-loader-content-search-group input').fill('')
  735 |       await sessionsSearch.fill(id(3))
  736 |       await loader.locator('.session-loader-result-main').click()
  737 |       await expect(loader.locator('.session-loader-events')).toContainText('仅显示当前会话')
  738 |       await expect(loader.locator('.session-loader-events')).toContainText('current-child-3')
  739 |       await saveScreenshot(h.page, 'ancestor-missing-note.png')
  740 |       await saveProof('ancestor-ui.json', { fixture: 'controlled rollout records; no inference or live Fork',
  741 |         ancestorVisibleInChild: true, ancestorSearchVisible: true, missingAncestorLabeled: true })
  742 |     } finally { await dispose(h) }
  743 |   })
  744 |
  745 |   test('closure PRD 5: Default restores user permissions after Bypass and Plan', async () => {
  746 |     const dirs = await createIsolatedWorkspace('default-fix')
  747 |     const configPath = join(dirs.rootDir, 'codex-home/config.toml')
  748 |     await writeFile(configPath, (await readFile(configPath, 'utf8')).replace('sandbox_mode = "read-only"', 'sandbox_mode = "workspace-write"'))
  749 |     const h = await launchPackagedApp(dirs)
  750 |     try {
  751 |       const { sessionId: sid, surface } = await launchCodex(h)
  752 |       await completeTurn(h, sid, surface, 'Remember mode codeword river-slate-842. Reply only MODES_READY.', /MODES_READY/)
  753 |       const providerId = await waitForProviderSessionId(h.dataDir, sid)
  754 |       const modes = []
  755 |       for (const [index, mode] of ['Bypass Permissions', 'Default', 'Plan Mode', 'Default'].entries()) {
  756 |         const processChange = await changeMode(h, sid, mode)
  757 |         await completeTurn(h, sid, paneForSession(h.page, sid).locator('.terminal-surface'),
  758 |           'Which mode codeword did I ask you to remember? Reply only that codeword.', /river-slate-842/)
  759 |         expect(await waitForProviderSessionId(h.dataDir, sid)).toBe(providerId)
  760 |         const context = (await rolloutEvents(h, providerId)).findLast((e) => e.type === 'turn_context')?.payload
  761 |         modes.push({ mode, ...processChange, sandbox: context?.sandbox_policy?.type, approval: context?.approval_policy })
  762 |         await saveProof('default-fix-modes.json', { sid, providerId,
  763 |           configuredDefault: { approval: 'on-request', sandbox: 'workspace-write' }, modes, turns: h.turnProofs })
  764 |         await saveScreenshot(h.page, `default-fix-${index + 1}-${mode.replaceAll(' ', '-').toLowerCase()}.png`)
  765 |         const expectedSandbox = mode === 'Bypass Permissions' ? 'danger-full-access'
  766 |           : mode === 'Plan Mode' ? 'read-only' : 'workspace-write'
  767 |         expect(context?.sandbox_policy?.type).toBe(expectedSandbox)
  768 |         expect(context?.approval_policy).toBe(mode === 'Bypass Permissions' ? 'never' : 'on-request')
  769 |       }
  770 |     } finally { await dispose(h) }
  771 |   })
  772 |
  773 |   test('closure PRD 8: independent restart stays starting for 10 seconds then recalls a codeword', async () => {
  774 |     const dirs = await createIsolatedWorkspace('restart-closure')
  775 |     let h = await launchPackagedApp(dirs)
  776 |     try {
  777 |       const { sessionId: sid, surface } = await launchCodex(h)
  778 |       await completeTurn(h, sid, surface, 'Remember restart codeword cedar-brook-573. Reply only RESTART_READY.', /RESTART_READY/)
  779 |       const providerId = await waitForProviderSessionId(h.dataDir, sid)
  780 |       const originalTurn = h.turnProofs.at(-1)!
```
