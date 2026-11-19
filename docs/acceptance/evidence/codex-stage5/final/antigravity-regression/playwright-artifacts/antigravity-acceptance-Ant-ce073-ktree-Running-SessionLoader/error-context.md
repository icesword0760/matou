# Instructions

- Following Playwright test failed.
- Explain why, be concise, respect Playwright best practices.
- Provide a snippet of code with the fix, if possible.

# Test info

- Name: antigravity-acceptance.spec.ts >> Antigravity Packaged App Acceptance (Real agy + Controlled Error Fixtures) >> PRD #2 #8 #10 + Running-Parent Fork: DAG/Badge/ParentProjection, Fork Current/Worktree/Running, SessionLoader
- Location: tests/e2e/antigravity-acceptance.spec.ts:862:3

# Error details

```
Error: expect(locator).toHaveAttribute(expected) failed

Locator: locator('[data-session-card="0f1c4ab5-e03b-4aea-83b4-f600e6e5b4c0"] [data-testid="terminal-pane"]:visible').locator('.pane-fork:not(.pane-load-session):not(.pane-fork-sibling):not(.pane-remove)')
Expected: "false"
Error: strict mode violation: locator('[data-session-card="0f1c4ab5-e03b-4aea-83b4-f600e6e5b4c0"] [data-testid="terminal-pane"]:visible').locator('.pane-fork:not(.pane-load-session):not(.pane-fork-sibling):not(.pane-remove)') resolved to 2 elements:
    1) <button title="会话历史" type="button" draggable="false" class="pane-fork pane-history" aria-label="查看“Memory Codeword Storage And Retrieval”的会话历史">…</button> aka getByRole('button', { name: '查看“Memory Codeword Storage' })
    2) <button type="button" title="创建子分支" class="pane-fork" draggable="false" aria-disabled="false" aria-label="从“Memory Codeword Storage And Retrieval”创建子分支">…</button> aka getByRole('button', { name: '从“Memory Codeword Storage And' })

Call log:
  - Expect "toHaveAttribute" with timeout 15000ms
  - waiting for locator('[data-session-card="0f1c4ab5-e03b-4aea-83b4-f600e6e5b4c0"] [data-testid="terminal-pane"]:visible').locator('.pane-fork:not(.pane-load-session):not(.pane-fork-sibling):not(.pane-remove)')

```

# Test source

```ts
  776 |       const runningRtMs = runningSamples.map((s) => s.runtimeToUiMs).filter((n): n is number => typeof n === 'number')
  777 |       const idleRtMs = idleSamples.map((s) => s.runtimeToUiMs).filter((n): n is number => typeof n === 'number')
  778 |       const avg = (arr: number[]) => (arr.length > 0 ? Math.round((arr.reduce((a, b) => a + b, 0) / arr.length) * 10) / 10 : null)
  779 |       const latencySummary = {
  780 |         runningCount: runningSamples.length,
  781 |         runningAvgMs: avg(runningRtMs),
  782 |         runningMaxMs: runningRtMs.length > 0 ? Math.max(...runningRtMs) : null,
  783 |         runningTriggerAvgMs: avg(runningSamples.map((s) => s.triggerToUiMs)),
  784 |         idleCount: idleSamples.length,
  785 |         idleAvgMs: avg(idleRtMs),
  786 |         idleMaxMs: idleRtMs.length > 0 ? Math.max(...idleRtMs) : null
  787 |       }
  788 |
  789 |       await saveProof('core-and-latency.json', {
  790 |         sessionId,
  791 |         providerSessionId,
  792 |         runningSamples,
  793 |         idleSamples,
  794 |         summary: latencySummary
  795 |       })
  796 |
  797 |       // #7 Auto Title, Manual Rename Lock, and Restore Auto Title
  798 |       const pane = paneForSession(harness.page, sessionId)
  799 |       await expect.poll(async () => {
  800 |         const title = (await pane.locator('.pane-title').innerText()).trim()
  801 |         return title && title !== 'Shell' && title !== 'Antigravity' ? title : ''
  802 |       }, { timeout: 30_000 }).toBeTruthy()
  803 |       const initialPaneTitle = await pane.locator('.pane-title').innerText()
  804 |
  805 |       await openPaneMenu(pane)
  806 |       await harness.page.getByRole('menuitem', { name: '重命名…' }).click()
  807 |       const renameInput = harness.page.getByRole('dialog').locator('input').first()
  808 |       await renameInput.fill('手工锁定标题-E2E')
  809 |       await renameInput.press('Enter')
  810 |       await expect(harness.page.getByRole('dialog')).toHaveCount(0, { timeout: 10_000 })
  811 |       await expect(pane.locator('.pane-title')).toHaveText('手工锁定标题-E2E')
  812 |
  813 |       // Run a short no-tool turn -> manual title must NOT be overwritten
  814 |       await sendTerminalLine(surface, 'Reply with only RENAME_LOCKED_OK')
  815 |       await expect(surface.locator('.xterm-rows')).toContainText('RENAME_LOCKED_OK', { timeout: 90_000 })
  816 |       await waitForDagNodeStatus(dagPage, sessionId, 'status-idle', 90_000)
  817 |       await expect(pane.locator('.pane-title')).toHaveText('手工锁定标题-E2E')
  818 |
  819 |       // Restore auto title
  820 |       await openPaneMenu(pane)
  821 |       await harness.page.getByRole('menuitem', { name: '恢复 Antigravity 自动标题' }).click()
  822 |       await expect(pane.locator('.pane-title')).not.toHaveText('手工锁定标题-E2E', { timeout: 15_000 })
  823 |       await expect(pane.locator('.pane-title')).not.toHaveText('Shell')
  824 |       const restoredAutoTitle = await pane.locator('.pane-title').innerText()
  825 |       await saveScreenshot(harness.page, '07-auto-title.png')
  826 |
  827 |       // #9a Real App Restart Recovery (--conversation resumes original conversation)
  828 |       await harness.close()
  829 |       await new Promise((r) => setTimeout(r, 500))
  830 |       harness = await launchPackagedApp(dirs)
  831 |       const restoredPane = paneForSession(harness.page, sessionId)
  832 |       const restoredSurface = restoredPane.locator('.terminal-surface')
  833 |       await positivePid(restoredSurface)
  834 |       await expect(hudForSession(harness.page, sessionId)).toHaveAttribute('data-hud-mode', 'agent', { timeout: 45_000 })
  835 |       await expect(hudForSession(harness.page, sessionId).locator('.status-model')).toBeVisible({ timeout: 45_000 })
  836 |       await expect(restoredPane.locator('.provider-restore-banner')).toHaveCount(0)
  837 |       await expect(restoredPane.locator('.pane-title')).not.toHaveText('手工锁定标题-E2E', { timeout: 15_000 })
  838 |       await expect(restoredPane.locator('.pane-title')).not.toHaveText('Shell')
  839 |       await harness.page.waitForTimeout(800)
  840 |
  841 |       await sendTerminalLine(restoredSurface, 'What codeword did I ask you to remember earlier? Reply with only the codeword and RESTART_OK.')
  842 |       await expect(restoredSurface.locator('.xterm-rows')).toContainText('TITAN_904', { timeout: 90_000 })
  843 |       await expect(restoredPane.locator('.provider-restore-banner')).toHaveCount(0)
  844 |       await saveScreenshot(harness.page, '09a-restart-recovery.png')
  845 |
  846 |       await saveProof('core-and-latency.json', {
  847 |         sessionId,
  848 |         providerSessionId,
  849 |         initialPaneTitle,
  850 |         restoredAutoTitle,
  851 |         runningSamples,
  852 |         idleSamples,
  853 |         summary: latencySummary,
  854 |         dbBindingsAfter: readDbRows(harness.dataDir, sessionId).bindings
  855 |       })
  856 |     } finally {
  857 |       await harness.close().catch(() => {})
  858 |       await cleanupWorkspaceLinks(dirs)
  859 |     }
  860 |   })
  861 |
  862 |   test('PRD #2 #8 #10 + Running-Parent Fork: DAG/Badge/ParentProjection, Fork Current/Worktree/Running, SessionLoader', async () => {
  863 |     const dirs = await createIsolatedWorkspace('fork-loader', { secondCommit: true })
  864 |     const harness = await launchPackagedApp(dirs)
  865 |
  866 |     try {
  867 |       const { sessionId: parentId, surface: parentSurface, dagPage } = await launchAgyInFreshRepo(harness)
  868 |
  869 |       // Complete turn 1 with a secret token to test fork context inheritance
  870 |       await sendTerminalLine(parentSurface, 'Remember the exact codeword COBALT_719 and reply ONLY TURN1_SAVED.')
  871 |       await waitForDagNodeStatus(dagPage, parentId, 'status-idle', 90_000)
  872 |       await expect(parentSurface.locator('.xterm-rows')).toContainText('TURN1_SAVED', { timeout: 30_000 })
  873 |       const parentProviderId = await waitForProviderSessionId(harness.dataDir, parentId)
  874 |
  875 |       const parentPane = paneForSession(harness.page, parentId)
> 876 |       await expect(parentPane.locator('.pane-fork:not(.pane-load-session):not(.pane-fork-sibling):not(.pane-remove)')).toHaveAttribute('aria-disabled', 'false', { timeout: 15_000 })
      |                                                                                                                        ^ Error: expect(locator).toHaveAttribute(expected) failed
  877 |
  878 |       // Fork Case A: Current Worktree (create child branch via pane header fork button)
  879 |       await parentPane.locator('.pane-fork:not(.pane-load-session):not(.pane-fork-sibling):not(.pane-remove)').click()
  880 |       await confirmForkDialog(harness.page, '子分支-当前目录', 'current')
  881 |
  882 |       const child1Surface = visibleFocusedSurface(harness.page)
  883 |       let child1Id = ''
  884 |       await expect.poll(async () => {
  885 |         child1Id = (await child1Surface.getAttribute('data-session-id')) ?? ''
  886 |         return child1Id && child1Id !== parentId ? child1Id : ''
  887 |       }, { timeout: 25_000 }).toBeTruthy()
  888 |       await expect(paneForSession(harness.page, child1Id).locator('.fork-progress-overlay')).toHaveCount(0, { timeout: 45_000 })
  889 |       await expect(harness.page.getByRole('button', { name: '返回父会话' })).toBeVisible({ timeout: 10_000 })
  890 |
  891 |       let child1ProviderId = ''
  892 |       await expect.poll(() => {
  893 |         const b = readDbRows(harness.dataDir, child1Id).bindings.find(
  894 |           (x) => x.provider === 'antigravity' && x.resume_state === 'available' && x.provider_session_id !== parentProviderId
  895 |         )
  896 |         child1ProviderId = b?.provider_session_id ?? ''
  897 |         return child1ProviderId
  898 |       }, { timeout: 45_000 }).toBeTruthy()
  899 |       expect(child1ProviderId).not.toBe(parentProviderId)
  900 |
  901 |       // Ask child1 for the inherited codeword!
  902 |       await waitForDagNodeStatus(dagPage, child1Id, 'status-idle', 90_000)
  903 |       await sendTerminalLine(child1Surface, 'What is the exact codeword I asked you to remember? Reply with ONLY that codeword.')
  904 |       await waitForDagNodeStatus(dagPage, child1Id, 'status-idle', 90_000)
  905 |       await expect(child1Surface.locator('.xterm-rows')).toContainText('COBALT_719', { timeout: 30_000 })
  906 |
  907 |       // Return to parent, verify ChildSessionBadge (#2) & DAG (#2)
  908 |       await harness.page.getByRole('button', { name: '返回父会话' }).click()
  909 |       await expect(parentPane.locator('.child-session-badge')).toContainText('1', { timeout: 15_000 })
  910 |       await expect(dagPage.locator(`.dag-node-card[data-session-id="${parentId}"]`)).toContainText('Antigravity')
  911 |       await expect(dagPage.locator(`.dag-node-card[data-session-id="${child1Id}"]`)).toContainText('Antigravity')
  912 |       await saveScreenshot(dagPage, '02-identifiers-dag.png')
  913 |
  914 |       // Fork Case B: New Worktree
  915 |       await parentPane.locator('.pane-fork:not(.pane-load-session):not(.pane-fork-sibling):not(.pane-remove)').click()
  916 |       await confirmForkDialog(harness.page, '子分支-独立Worktree', 'new')
  917 |
  918 |       const child2Surface = visibleFocusedSurface(harness.page)
  919 |       let child2Id = ''
  920 |       await expect.poll(async () => {
  921 |         child2Id = (await child2Surface.getAttribute('data-session-id')) ?? ''
  922 |         return child2Id && child2Id !== parentId && child2Id !== child1Id ? child2Id : ''
  923 |       }, { timeout: 30_000 }).toBeTruthy()
  924 |       await expect(paneForSession(harness.page, child2Id).locator('.fork-progress-overlay')).toHaveCount(0, { timeout: 45_000 })
  925 |
  926 |       let child2ProviderId = ''
  927 |       await expect.poll(() => {
  928 |         const b = readDbRows(harness.dataDir, child2Id).bindings.find(
  929 |           (x) => x.provider === 'antigravity' && x.resume_state === 'available' && x.provider_session_id !== parentProviderId
  930 |         )
  931 |         child2ProviderId = b?.provider_session_id ?? ''
  932 |         return child2ProviderId
  933 |       }, { timeout: 45_000 }).toBeTruthy()
  934 |       expect(child2ProviderId).not.toBe(parentProviderId)
  935 |       expect(child2ProviderId).not.toBe(child1ProviderId)
  936 |       await saveScreenshot(harness.page, '10a-fork-branches.png')
  937 |
  938 |       // Concurrent Check: Return to parent and Fork while Parent is ACTIVELY RUNNING ('running')
  939 |       await harness.page.getByRole('button', { name: '返回父会话' }).click()
  940 |       await expect(parentPane.locator('.child-session-badge')).toContainText('2', { timeout: 15_000 })
  941 |       await harness.page.locator(`[data-session-card="${parentId}"]`).click()
  942 |       const parentTranscriptPath = join(
  943 |         GEMINI_BRAIN_ROOT,
  944 |         parentProviderId,
  945 |         '.system_generated',
  946 |         'logs',
  947 |         'transcript.jsonl'
  948 |       )
  949 |       const parentDbPath = join(GEMINI_CONVERSATIONS_ROOT, `${parentProviderId}.db`)
  950 |       const parentTranscriptBefore = await readFile(parentTranscriptPath, 'utf8')
  951 |       const parentLineCountBefore = parentTranscriptBefore.split('\n').filter(Boolean).length
  952 |
  953 |       await sendTerminalLine(
  954 |         parentSurface,
  955 |         'Write 25 numbered short sentences (each about 15 words) explaining deterministic state machines. Do not use any tools or edit any files.'
  956 |       )
  957 |       await waitForDagNodeStatus(dagPage, parentId, 'status-running', 30_000)
  958 |
  959 |       // Trigger Fork immediately while parent is running!
  960 |       await parentPane.locator('.pane-fork:not(.pane-load-session):not(.pane-fork-sibling):not(.pane-remove)').click()
  961 |       await confirmForkDialog(harness.page, '子分支-父运行中Fork', 'current')
  962 |
  963 |       const child3Surface = visibleFocusedSurface(harness.page)
  964 |       let child3Id = ''
  965 |       await expect.poll(async () => {
  966 |         child3Id = (await child3Surface.getAttribute('data-session-id')) ?? ''
  967 |         return child3Id && ![parentId, child1Id, child2Id].includes(child3Id) ? child3Id : ''
  968 |       }, { timeout: 30_000 }).toBeTruthy()
  969 |
  970 |       // Wait for both child3 fork & parent active turn to finish cleanly
  971 |       await expect(paneForSession(harness.page, child3Id).locator('.fork-progress-overlay')).toHaveCount(0, { timeout: 60_000 })
  972 |       await waitForDagNodeStatus(dagPage, parentId, 'status-idle', 90_000)
  973 |       await waitForDagNodeStatus(dagPage, child3Id, 'status-idle', 90_000)
  974 |
  975 |       // Verify parent provider binding wasn't hijacked by child3
  976 |       const parentBindingAfter = readDbRows(harness.dataDir, parentId).bindings.find(
```
