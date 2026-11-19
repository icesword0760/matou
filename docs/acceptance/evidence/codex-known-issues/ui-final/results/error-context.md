# Instructions

- Following Playwright test failed.
- Explain why, be concise, respect Playwright best practices.
- Provide a snippet of code with the fix, if possible.

# Test info

- Name: codex-acceptance.spec.ts >> Codex packaged acceptance — real CLI and explicit controlled fixtures >> known issues: a real Codex model error notifies once and marks the card error; startup upgrade is disabled
- Location: tests/e2e/codex-acceptance.spec.ts:653:3

# Error details

```
Error: expect(locator).toContainText(expected) failed

Locator: locator('.scene-stage:not([hidden]) .session-card.is-focused [data-testid="terminal-pane"]:visible .terminal-surface').first().locator('.xterm-rows')
Expected substring: "unexpected status 404"
Received string:    "icesword@icesworddeMacBook-Pro repo %  "
Timeout: 15000ms

Call log:
  - Expect "toContainText" with timeout 15000ms
  - waiting for locator('.scene-stage:not([hidden]) .session-card.is-focused [data-testid="terminal-pane"]:visible .terminal-surface').first().locator('.xterm-rows')
    34 × locator resolved to <div class="xterm-rows" aria-hidden="true">…</div>
       - unexpected value "icesword@icesworddeMacBook-Pro repo %  "

```

# Test source

```ts
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
  655 |     const pending: ServerResponse[] = []
  656 |     let releaseFailures = false
  657 |     const fail = (response: ServerResponse) => {
  658 |       response.writeHead(404, { 'content-type': 'application/json' })
  659 |       response.end(JSON.stringify({ error: { message: 'The model matou-probe-missing does not exist',
  660 |         type: 'invalid_request_error', code: 'model_not_found' } }))
  661 |     }
  662 |     const requests: Array<{ method?: string; url?: string; bytes: number; ended: boolean; responded: boolean }> = []
  663 |     const endpoint = createServer((request, response) => {
  664 |       const observed = { method: request.method, url: request.url, bytes: 0, ended: false, responded: false }
  665 |       requests.push(observed)
  666 |       // Consume the entire context upload before replying. Otherwise the client's
  667 |       // request body can block on the server's unread socket buffer.
  668 |       request.on('data', (data: Buffer) => { observed.bytes += data.length })
  669 |       request.on('end', () => {
  670 |         observed.ended = true
  671 |         if (request.method === 'POST' && request.url?.endsWith('/responses')) {
  672 |           if (releaseFailures) fail(response)
  673 |           else pending.push(response)
  674 |         } else {
  675 |           response.writeHead(200, { 'content-type': 'application/json' }); response.end('{"data":[]}')
  676 |           observed.responded = true
  677 |         }
  678 |       })
  679 |       response.on('finish', () => { observed.responded = true })
  680 |     })
  681 |     await new Promise<void>(resolve => endpoint.listen(0, '127.0.0.1', resolve))
  682 |     const address = endpoint.address() as { port: number }
  683 |     const configPath = join(dirs.rootDir, 'codex-home/config.toml')
  684 |     let config = (await readFile(configPath, 'utf8')).replace('model_provider = "openai"', 'model_provider = "local_probe"')
  685 |       .replace('check_for_update_on_startup = false\n', '')
  686 |     config += `\n[model_providers.local_probe]\nname = "Local failure probe"\nbase_url = "http://127.0.0.1:${address.port}/v1"\nrequires_openai_auth = false\nwire_api = "responses"\nrequest_max_retries = 0\nstream_max_retries = 0\n`
  687 |     await writeFile(configPath, config)
  688 |     // Neither fixture config nor wrapper disables updates: the product must.
  689 |     const wrapper = join(dirs.rootDir, 'codex-wrapper')
  690 |     await writeFile(wrapper, (await readFile(wrapper, 'utf8')).replace(' -c check_for_update_on_startup=false', ''))
  691 |     await writeFile(join(dirs.rootDir, 'codex-home/version.json'), JSON.stringify({ latest_version: '999.0.0',
  692 |       last_checked_at: new Date().toISOString(), dismissed_version: null }))
  693 |     const h = await launchPackagedApp(dirs)
  694 |     try {
  695 |       const { sessionId: sid, surface, dagPage } = await launchCodex(h)
  696 |       await sendTerminalLine(surface, 'Reply only MODEL_PROBE_READY.')
  697 |       const peer = await createSiblingShellAndFocus(h.page, sid)
  698 |       await expect.poll(() => pending.length, { timeout: 20_000 }).toBeGreaterThan(0)
  699 |       // Codex may submit a parallel internal request. Fail every request so
  700 |       // no pending main turn is accidentally left behind the test fixture.
  701 |       releaseFailures = true
  702 |       for (const response of pending.splice(0)) fail(response)
  703 |       const providerId = await waitForProviderSessionId(h.dataDir, sid)
  704 |       await expect.poll(async () => (await rolloutEvents(h, providerId))
  705 |         .some(e => e.type === 'event_msg' && e.payload?.type === 'task_complete' && e.payload.error), { timeout: 20_000 }).toBe(true)
  706 |       await expect.poll(() => readCurrentWorkStatus(h.dataDir, sid)?.status, { timeout: 10_000 }).toBe('error')
  707 |       await waitForDagNodeStatus(dagPage, sid, 'status-error')
  708 |       await expect(paneForSession(h.page, sid)).toHaveClass(/has-notification/)
  709 |       await h.page.getByRole('button', { name: '通知中心', exact: true }).click()
  710 |       const center = h.page.getByRole('region', { name: '通知中心', exact: true })
  711 |       await expect(center.locator('.notification-item')).toHaveCount(1)
  712 |       await expect(center.locator('.notification-item__title')).toHaveText('Codex')
  713 |       await expect(center).toContainText('matou-probe-missing does not exist')
> 714 |       await expect(surface.locator('.xterm-rows')).toContainText('unexpected status 404')
      |                                                    ^ Error: expect(locator).toContainText(expected) failed
  715 |       await expect(surface.locator('.xterm-rows')).not.toContainText('Update available')
  716 |       await saveScreenshot(h.page, 'error-notification.png')
  717 |       await saveScreenshot(dagPage, 'error-dag.png')
  718 |       await saveProof('model-error-ui.json', { sid, peerId: peer.sessionId, providerId,
  719 |         status: readCurrentWorkStatus(h.dataDir, sid)?.status,
  720 |         failedTurn: (await rolloutEvents(h, providerId)).findLast(e => e.payload?.type === 'task_complete'),
  721 |         notificationCount: await center.locator('.notification-item').count(),
  722 |         notificationTitle: await center.locator('.notification-item__title').innerText(),
  723 |         startupUpgradePrompt: false, wrapperDisablesUpdates: false, configDisablesUpdates: false })
  724 |     } finally {
  725 |       await saveProof('model-error-request-log.json', requests)
  726 |       await dispose(h)
  727 |       endpoint.closeAllConnections(); await new Promise<void>(resolve => endpoint.close(() => resolve()))
  728 |     }
  729 |   })
  730 |
  731 |   test('known issues: history_base ancestor appears in preview and search; missing parent is labeled', async () => {
  732 |     const dirs = await createIsolatedWorkspace('ancestor-history', { copyAuth: false })
  733 |     const folder = join(dirs.rootDir, 'codex-home/sessions/2026/11/15')
  734 |     await mkdir(folder, { recursive: true })
  735 |     const id = (n: number) => `01a1ddd6-4064-70e1-a006-${String(n).padStart(12, '0')}`
  736 |     const message = (text: string, n: number) => ({ timestamp: `2026-11-15T12:00:0${n}Z`, type: 'event_msg',
  737 |       payload: { type: 'item_completed', turn_id: `t-${n}`, item: { type: 'AgentMessage', id: `a-${n}`,
  738 |         content: [{ text }], phase: 'final_answer' } } })
  739 |     for (const n of [1, 2, 3]) {
  740 |       const records = [{ type: 'session_meta', payload: { id: id(n), cwd: dirs.repoDir,
  741 |         ...(n === 1 ? {} : { history_base: { thread_id: id(n === 2 ? 1 : 99) } }) } },
  742 |         message(n === 1 ? 'ancestral-needle-913' : `current-child-${n}`, n)]
  743 |       await writeFile(join(folder, `rollout-${id(n)}.jsonl`), records.map(r => JSON.stringify(r)).join('\n') + '\n')
  744 |     }
  745 |     const h = await launchPackagedApp(dirs)
  746 |     try {
  747 |       const sid = (await visibleFocusedSurface(h.page).getAttribute('data-session-id'))!
  748 |       await paneForSession(h.page, sid).locator('.pane-load-session').click()
  749 |       const loader = h.page.getByRole('dialog')
  750 |       await loader.getByRole('tab', { name: 'Codex', exact: true }).click()
  751 |       const sessionsSearch = loader.locator('[data-search-scope="sessions"] input')
  752 |       await sessionsSearch.fill(id(2))
  753 |       await expect(loader.locator('.session-loader-result-main')).toHaveCount(1)
  754 |       await loader.locator('.session-loader-result-main').click()
  755 |       await expect(loader.locator('.session-loader-events')).toContainText('ancestral-needle-913')
  756 |       await expect(loader.locator('.session-loader-events')).toContainText('current-child-2')
  757 |       await loader.locator('.session-loader-content-search-group input').fill('ancestral-needle-913')
  758 |       await expect(loader.locator('.session-loader-events')).toContainText('ancestral-needle-913')
  759 |       await saveScreenshot(h.page, 'ancestor-preview-search.png')
  760 |       await loader.locator('.session-loader-content-search-group input').fill('')
  761 |       await sessionsSearch.fill(id(3))
  762 |       await loader.locator('.session-loader-result-main').click()
  763 |       await expect(loader.locator('.session-loader-events')).toContainText('仅显示当前会话')
  764 |       await expect(loader.locator('.session-loader-events')).toContainText('current-child-3')
  765 |       await saveScreenshot(h.page, 'ancestor-missing-note.png')
  766 |       await saveProof('ancestor-ui.json', { fixture: 'controlled rollout records; no inference or live Fork',
  767 |         ancestorVisibleInChild: true, ancestorSearchVisible: true, missingAncestorLabeled: true })
  768 |     } finally { await dispose(h) }
  769 |   })
  770 |
  771 |   test('closure PRD 5: Default restores user permissions after Bypass and Plan', async () => {
  772 |     const dirs = await createIsolatedWorkspace('default-fix')
  773 |     const configPath = join(dirs.rootDir, 'codex-home/config.toml')
  774 |     await writeFile(configPath, (await readFile(configPath, 'utf8')).replace('sandbox_mode = "read-only"', 'sandbox_mode = "workspace-write"'))
  775 |     const h = await launchPackagedApp(dirs)
  776 |     try {
  777 |       const { sessionId: sid, surface } = await launchCodex(h)
  778 |       await completeTurn(h, sid, surface, 'Remember mode codeword river-slate-842. Reply only MODES_READY.', /MODES_READY/)
  779 |       const providerId = await waitForProviderSessionId(h.dataDir, sid)
  780 |       const modes = []
  781 |       for (const [index, mode] of ['Bypass Permissions', 'Default', 'Plan Mode', 'Default'].entries()) {
  782 |         const processChange = await changeMode(h, sid, mode)
  783 |         await completeTurn(h, sid, paneForSession(h.page, sid).locator('.terminal-surface'),
  784 |           'Which mode codeword did I ask you to remember? Reply only that codeword.', /river-slate-842/)
  785 |         expect(await waitForProviderSessionId(h.dataDir, sid)).toBe(providerId)
  786 |         const context = (await rolloutEvents(h, providerId)).findLast((e) => e.type === 'turn_context')?.payload
  787 |         modes.push({ mode, ...processChange, sandbox: context?.sandbox_policy?.type, approval: context?.approval_policy })
  788 |         await saveProof('default-fix-modes.json', { sid, providerId,
  789 |           configuredDefault: { approval: 'on-request', sandbox: 'workspace-write' }, modes, turns: h.turnProofs })
  790 |         await saveScreenshot(h.page, `default-fix-${index + 1}-${mode.replaceAll(' ', '-').toLowerCase()}.png`)
  791 |         const expectedSandbox = mode === 'Bypass Permissions' ? 'danger-full-access'
  792 |           : mode === 'Plan Mode' ? 'read-only' : 'workspace-write'
  793 |         expect(context?.sandbox_policy?.type).toBe(expectedSandbox)
  794 |         expect(context?.approval_policy).toBe(mode === 'Bypass Permissions' ? 'never' : 'on-request')
  795 |       }
  796 |     } finally { await dispose(h) }
  797 |   })
  798 |
  799 |   test('closure PRD 8: independent restart stays starting for 10 seconds then recalls a codeword', async () => {
  800 |     const dirs = await createIsolatedWorkspace('restart-closure')
  801 |     let h = await launchPackagedApp(dirs)
  802 |     try {
  803 |       const { sessionId: sid, surface } = await launchCodex(h)
  804 |       await completeTurn(h, sid, surface, 'Remember restart codeword cedar-brook-573. Reply only RESTART_READY.', /RESTART_READY/)
  805 |       const providerId = await waitForProviderSessionId(h.dataDir, sid)
  806 |       const originalTurn = h.turnProofs.at(-1)!
  807 |       const turns = h.turnProofs
  808 |       await h.close()
  809 |       h = await launchPackagedApp(dirs)
  810 |       h.turnProofs = turns
  811 |       const pane = paneForSession(h.page, sid)
  812 |       const restored = pane.locator('.terminal-surface')
  813 |       await positivePid(restored)
  814 |       const staticStartAt = Date.now()
```
