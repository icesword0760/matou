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
  656 |     const requests: Array<{ method?: string; url?: string; bytes: number; ended: boolean; responded: boolean }> = []
  657 |     const endpoint = createServer((request, response) => {
  658 |       const observed = { method: request.method, url: request.url, bytes: 0, ended: false, responded: false }
  659 |       requests.push(observed)
  660 |       // Consume the entire context upload before replying. Otherwise the client's
  661 |       // request body can block on the server's unread socket buffer.
  662 |       request.on('data', (data: Buffer) => { observed.bytes += data.length })
  663 |       request.on('end', () => {
  664 |         observed.ended = true
  665 |         if (request.method === 'POST' && request.url?.endsWith('/responses')) {
  666 |           pending = response
  667 |         } else {
  668 |           response.writeHead(200, { 'content-type': 'application/json' }); response.end('{"data":[]}')
  669 |           observed.responded = true
  670 |         }
  671 |       })
  672 |       response.on('finish', () => { observed.responded = true })
  673 |     })
  674 |     await new Promise<void>(resolve => endpoint.listen(0, '127.0.0.1', resolve))
  675 |     const address = endpoint.address() as { port: number }
  676 |     const configPath = join(dirs.rootDir, 'codex-home/config.toml')
  677 |     let config = (await readFile(configPath, 'utf8')).replace('model_provider = "openai"', 'model_provider = "local_probe"')
  678 |       .replace('check_for_update_on_startup = false\n', '')
  679 |     config += `\n[model_providers.local_probe]\nname = "Local failure probe"\nbase_url = "http://127.0.0.1:${address.port}/v1"\nrequires_openai_auth = false\nwire_api = "responses"\nrequest_max_retries = 0\nstream_max_retries = 0\n`
  680 |     await writeFile(configPath, config)
  681 |     // Neither fixture config nor wrapper disables updates: the product must.
  682 |     const wrapper = join(dirs.rootDir, 'codex-wrapper')
  683 |     await writeFile(wrapper, (await readFile(wrapper, 'utf8')).replace(' -c check_for_update_on_startup=false', ''))
  684 |     await writeFile(join(dirs.rootDir, 'codex-home/version.json'), JSON.stringify({ latest_version: '999.0.0',
  685 |       last_checked_at: new Date().toISOString(), dismissed_version: null }))
  686 |     const h = await launchPackagedApp(dirs)
  687 |     try {
  688 |       const { sessionId: sid, surface, dagPage } = await launchCodex(h)
  689 |       await sendTerminalLine(surface, 'Reply only MODEL_PROBE_READY.')
  690 |       const peer = await createSiblingShellAndFocus(h.page, sid)
  691 |       await expect.poll(() => Boolean(pending), { timeout: 20_000 }).toBe(true)
  692 |       const body = JSON.stringify({ error: { message: 'The model matou-probe-missing does not exist',
  693 |         type: 'invalid_request_error', code: 'model_not_found' } })
  694 |       pending!.writeHead(404, { 'content-type': 'application/json' }); pending!.end(body)
  695 |       const providerId = await waitForProviderSessionId(h.dataDir, sid)
  696 |       await expect.poll(async () => (await rolloutEvents(h, providerId))
> 697 |         .some(e => e.type === 'event_msg' && e.payload?.type === 'task_complete' && e.payload.error), { timeout: 20_000 }).toBe(true)
      |                                                                                                                            ^ Error: expect(received).toBe(expected) // Object.is equality
  698 |       await expect.poll(() => readCurrentWorkStatus(h.dataDir, sid)?.status, { timeout: 10_000 }).toBe('error')
  699 |       await waitForDagNodeStatus(dagPage, sid, 'status-error')
  700 |       await expect(paneForSession(h.page, sid)).toHaveClass(/has-notification/)
  701 |       await h.page.getByRole('button', { name: '通知中心', exact: true }).click()
  702 |       const center = h.page.getByRole('region', { name: '通知中心', exact: true })
  703 |       await expect(center.locator('.notification-item')).toHaveCount(1)
  704 |       await expect(center.locator('.notification-item__title')).toHaveText('Codex')
  705 |       await expect(center).toContainText('matou-probe-missing does not exist')
  706 |       await expect(surface.locator('.xterm-rows')).toContainText('unexpected status 404')
  707 |       await expect(surface.locator('.xterm-rows')).not.toContainText('Update available')
  708 |       await saveScreenshot(h.page, 'error-notification.png')
  709 |       await saveScreenshot(dagPage, 'error-dag.png')
  710 |       await saveProof('model-error-ui.json', { sid, peerId: peer.sessionId, providerId,
  711 |         status: readCurrentWorkStatus(h.dataDir, sid)?.status,
  712 |         failedTurn: (await rolloutEvents(h, providerId)).findLast(e => e.payload?.type === 'task_complete'),
  713 |         notificationCount: await center.locator('.notification-item').count(),
  714 |         notificationTitle: await center.locator('.notification-item__title').innerText(),
  715 |         startupUpgradePrompt: false, wrapperDisablesUpdates: false, configDisablesUpdates: false })
  716 |     } finally {
  717 |       await saveProof('model-error-request-log.json', requests)
  718 |       await dispose(h)
  719 |       endpoint.closeAllConnections(); await new Promise<void>(resolve => endpoint.close(() => resolve()))
  720 |     }
  721 |   })
  722 |
  723 |   test('known issues: history_base ancestor appears in preview and search; missing parent is labeled', async () => {
  724 |     const dirs = await createIsolatedWorkspace('ancestor-history', { copyAuth: false })
  725 |     const folder = join(dirs.rootDir, 'codex-home/sessions/2026/11/15')
  726 |     await mkdir(folder, { recursive: true })
  727 |     const id = (n: number) => `01a1ddd6-4064-70e1-a006-${String(n).padStart(12, '0')}`
  728 |     const message = (text: string, n: number) => ({ timestamp: `2026-11-15T12:00:0${n}Z`, type: 'event_msg',
  729 |       payload: { type: 'item_completed', turn_id: `t-${n}`, item: { type: 'AgentMessage', id: `a-${n}`,
  730 |         content: [{ text }], phase: 'final_answer' } } })
  731 |     for (const n of [1, 2, 3]) {
  732 |       const records = [{ type: 'session_meta', payload: { id: id(n), cwd: dirs.repoDir,
  733 |         ...(n === 1 ? {} : { history_base: { thread_id: id(n === 2 ? 1 : 99) } }) } },
  734 |         message(n === 1 ? 'ancestral-needle-913' : `current-child-${n}`, n)]
  735 |       await writeFile(join(folder, `rollout-${id(n)}.jsonl`), records.map(r => JSON.stringify(r)).join('\n') + '\n')
  736 |     }
  737 |     const h = await launchPackagedApp(dirs)
  738 |     try {
  739 |       const sid = (await visibleFocusedSurface(h.page).getAttribute('data-session-id'))!
  740 |       await paneForSession(h.page, sid).locator('.pane-load-session').click()
  741 |       const loader = h.page.getByRole('dialog')
  742 |       await loader.getByRole('tab', { name: 'Codex', exact: true }).click()
  743 |       const sessionsSearch = loader.locator('[data-search-scope="sessions"] input')
  744 |       await sessionsSearch.fill(id(2))
  745 |       await expect(loader.locator('.session-loader-result-main')).toHaveCount(1)
  746 |       await loader.locator('.session-loader-result-main').click()
  747 |       await expect(loader.locator('.session-loader-events')).toContainText('ancestral-needle-913')
  748 |       await expect(loader.locator('.session-loader-events')).toContainText('current-child-2')
  749 |       await loader.locator('.session-loader-content-search-group input').fill('ancestral-needle-913')
  750 |       await expect(loader.locator('.session-loader-events')).toContainText('ancestral-needle-913')
  751 |       await saveScreenshot(h.page, 'ancestor-preview-search.png')
  752 |       await loader.locator('.session-loader-content-search-group input').fill('')
  753 |       await sessionsSearch.fill(id(3))
  754 |       await loader.locator('.session-loader-result-main').click()
  755 |       await expect(loader.locator('.session-loader-events')).toContainText('仅显示当前会话')
  756 |       await expect(loader.locator('.session-loader-events')).toContainText('current-child-3')
  757 |       await saveScreenshot(h.page, 'ancestor-missing-note.png')
  758 |       await saveProof('ancestor-ui.json', { fixture: 'controlled rollout records; no inference or live Fork',
  759 |         ancestorVisibleInChild: true, ancestorSearchVisible: true, missingAncestorLabeled: true })
  760 |     } finally { await dispose(h) }
  761 |   })
  762 |
  763 |   test('closure PRD 5: Default restores user permissions after Bypass and Plan', async () => {
  764 |     const dirs = await createIsolatedWorkspace('default-fix')
  765 |     const configPath = join(dirs.rootDir, 'codex-home/config.toml')
  766 |     await writeFile(configPath, (await readFile(configPath, 'utf8')).replace('sandbox_mode = "read-only"', 'sandbox_mode = "workspace-write"'))
  767 |     const h = await launchPackagedApp(dirs)
  768 |     try {
  769 |       const { sessionId: sid, surface } = await launchCodex(h)
  770 |       await completeTurn(h, sid, surface, 'Remember mode codeword river-slate-842. Reply only MODES_READY.', /MODES_READY/)
  771 |       const providerId = await waitForProviderSessionId(h.dataDir, sid)
  772 |       const modes = []
  773 |       for (const [index, mode] of ['Bypass Permissions', 'Default', 'Plan Mode', 'Default'].entries()) {
  774 |         const processChange = await changeMode(h, sid, mode)
  775 |         await completeTurn(h, sid, paneForSession(h.page, sid).locator('.terminal-surface'),
  776 |           'Which mode codeword did I ask you to remember? Reply only that codeword.', /river-slate-842/)
  777 |         expect(await waitForProviderSessionId(h.dataDir, sid)).toBe(providerId)
  778 |         const context = (await rolloutEvents(h, providerId)).findLast((e) => e.type === 'turn_context')?.payload
  779 |         modes.push({ mode, ...processChange, sandbox: context?.sandbox_policy?.type, approval: context?.approval_policy })
  780 |         await saveProof('default-fix-modes.json', { sid, providerId,
  781 |           configuredDefault: { approval: 'on-request', sandbox: 'workspace-write' }, modes, turns: h.turnProofs })
  782 |         await saveScreenshot(h.page, `default-fix-${index + 1}-${mode.replaceAll(' ', '-').toLowerCase()}.png`)
  783 |         const expectedSandbox = mode === 'Bypass Permissions' ? 'danger-full-access'
  784 |           : mode === 'Plan Mode' ? 'read-only' : 'workspace-write'
  785 |         expect(context?.sandbox_policy?.type).toBe(expectedSandbox)
  786 |         expect(context?.approval_policy).toBe(mode === 'Bypass Permissions' ? 'never' : 'on-request')
  787 |       }
  788 |     } finally { await dispose(h) }
  789 |   })
  790 |
  791 |   test('closure PRD 8: independent restart stays starting for 10 seconds then recalls a codeword', async () => {
  792 |     const dirs = await createIsolatedWorkspace('restart-closure')
  793 |     let h = await launchPackagedApp(dirs)
  794 |     try {
  795 |       const { sessionId: sid, surface } = await launchCodex(h)
  796 |       await completeTurn(h, sid, surface, 'Remember restart codeword cedar-brook-573. Reply only RESTART_READY.', /RESTART_READY/)
  797 |       const providerId = await waitForProviderSessionId(h.dataDir, sid)
```
