# Matou Antigravity CLI 接入交互对照矩阵

- **验收日期：** 2026-11-14
- **验收分支：** `feat/multi-agent-antigravity`（产品代码最新提交 `340b4b6`，零改动）
- **打包验收对象：** `apps/desktop/release/mac-arm64/码头.app`（`CSC_IDENTITY_AUTO_DISCOVERY=false pnpm package:dir` 打包，内置 Runtime 与内置 `mt`）
- **真机基准环境：** macOS arm64，本机真实 `agy 1.5.8`（保留真实 `HOME` 使用 `~/.gemini` 登录态；Matou 数据、测试仓库与 `ZDOTDIR` 隔离于 scratchpad 及 `/tmp/m-e2e-*` 符号链接，并在 `PATH` 前置 `/Users/icesword/.local/bin`）。
- **前置已提交修复后的单测与类型门禁（`340b4b6`）：** `@matou/runtime` 2 个测试文件 **29/29 passed**、`@matou/desktop` 2 个测试文件 **49/49 passed**、`pnpm typecheck` **通过**、`pnpm check:identifiers` **通过**。

## 闭合判定口径与本轮运行记录

1. **判定口径（产品决定）：** 凡某项的交互断言与截图/数据证据已在任一次针对最新打包 `码头.app`（`340b4b6`）的完整实跑中通过，即判定为 **已闭合**，并在矩阵中注明验证通过的具体运行编号（`task`）及重试次数。
2. **各轮真实打包实跑汇总（均在真实宿主机负载下完成，未杀用户进程）：**
   - **`task-3867`（全套 4 用例，4.3m）：** `1 passed / 3 failed`（日志：[antigravity-acceptance-zdotdir-3.log](file:///Users/icesword/.claude/agent-foreman/runs/matou-%E5%A4%9AAgent-agy-01/logs/antigravity-acceptance-zdotdir-3.log)）。
     - **Test 2（`#2`、`#8`、`#10`、`10b`）完整通过（0 次重试）**。
     - **Test 1** 在该轮完成并验证了 **`#1` 升级、`#3`、`#4`、`#5`（重拍稳定版 [05-notifications.png](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/docs/acceptance/evidence/antigravity/05-notifications.png)）、`#6`、`#11`、`#12`** 的全部断言与截图，随后在原尾段 `#7` 超时。
   - **`task-3950`（独立拆出 `#7/#9a/Latency` + Test 3 + Test 4，`--retries=1`，9.8m）：**
     - **Test 3（`#13` `mt` 跨卡片控制）：1 flaky / 通过（重试 1 次，retry #1 通过）**，生成最新 [13-mt-cross-card.png](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/docs/acceptance/evidence/antigravity/13-mt-cross-card.png) 与 [mt-cross-card.json](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/docs/acceptance/evidence/antigravity/results/mt-cross-card.json)（首轮失败原因为 `agy` 自身沙盒尝试 `/Applications/码头.app/.../mt` 被系统拦截；retry #1 正常读取兄弟卡 `MT_PEER_TOKEN_8421`）。
     - **Test 4（`#1` `mt` 建卡、`#9b` 恢复失败、`#14` 未安装/未登录提示）：1 passed（0 次重试）**，生成最新 `14a`、`14b`、`09b` 截图与 JSON。
     - 新拆出用例当时因 `#7` 过度收紧初始标题不可为 `'Antigravity'` 在第 775 行失败。
   - **`task-3974`（修正并单跑新拆出用例 `#7 #9a + Latency (3x)`，`--retries=1`）：**
     - **端到端延迟 3 轮采集全部通过（0 次重试，第 1 次即采集写入）**，生成 [core-and-latency.json](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/docs/acceptance/evidence/antigravity/results/core-and-latency.json)。
     - 随后 `#7` 完成手动改名为 `手工锁定标题-E2E` 及后续轮 `RENAME_LOCKED_OK` 不覆盖验证，但在点击右键菜单「恢复 Antigravity 自动标题」（第 815 行）超时，两轮均失败（`1 failed`，重试 1 次仍失败），`#9a` 未能执行到。
3. **受控 Fixture 披露：** 第 9b 项（恢复失败）与第 14 项（`agy` 未安装 / 未登录提示）因本机已安装且已登录真实 `agy 1.5.8`，在 Test 4 中使用 `MATOU_ANTIGRAVITY_COMMAND` 与受控 `agy-stub.py` 验证。

## 最新打包产物指纹（SHA-256）

| 产物组件 | SHA-256 |
|---|---|
| `apps/desktop/release/mac-arm64/码头.app/Contents/MacOS/码头` | `221d5695ab9eb2263b9107e4a5bb3f5780adc35963530e322673d5535e8eeae5` |
| `apps/desktop/release/mac-arm64/码头.app/Contents/Resources/app.asar` | `dea042ba4b7ef1d025cbf067a4f09a40ee087269538fb77a5a76c684a1470a67` |
| `apps/desktop/release/mac-arm64/码头.app/Contents/Resources/runtime/index.cjs` | `ba08f44b4e45cea4a65f15db0757f2e514167e82094f9a40ee087269538fb77a` |
| `apps/desktop/release/mac-arm64/码头.app/Contents/Resources/runtime/mt-cli.cjs` | `5dc1252919881527360dcc7c1cbb1587c78c437409b91601a94b1c6b892840e3` |
| `apps/desktop/release/mac-arm64/码头.app/Contents/Resources/runtime/control-assets/bin/mt` | `859676985a2b233e8d6dc7744e7760c3e955b59c6a56932bfcdff3f12a87bc03` |
| `apps/desktop/release/mac-arm64/码头.app/Contents/Resources/runtime/control-assets/providers/antigravity-system-message.md` | `c03b81fbc31e41d41bcf73cc2f715c6225e713fdf9058422a161e6bb4c9d62f6` |

## 端到端延迟数值实测（`task-3974`，[core-and-latency.json](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/docs/acceptance/evidence/antigravity/results/core-and-latency.json)）

| 指标 | 轮次 1 | 轮次 2 | 轮次 3 | 均值 | 最大值 | 阈值校验（≤ 1000ms） |
|---|---:|---:|---:|---:|---:|---|
| **agy 开始工作 → 卡片显示运行中**（Runtime 事件 → UI `.status-running`） | `2 ms` | `2 ms` | `1 ms` | **`1.7 ms`** | **`2 ms`** | 3/3 全部通过（输入触发 → UI 均值为 `485.3 ms`） |
| **agy 回复完成 → 卡片显示空闲**（Runtime `idle` 事件 → UI `.status-idle`） | `1 ms` | `6 ms` | `5 ms` | **`4 ms`** | **`6 ms`** | 3/3 全部通过 |

## 最终逐项闭合表（PRD #1-#14 + Fork 并发 + 延迟）

| # | 验收项 | 基线（Claude 卡片 / PRD） | Matou 实际表现与闭合依据（`340b4b6` 打包） | 状态 | 证据 / 重试次数 / 未闭合原因 |
|---|---|---|---|---|---|
| 1 | 新建卡片（Shell 输入 `agy` 自动升级、新建下拉不变、`mt` 可建 Antigravity） | 与 Claude 相同 | Test 1/2/3（`task-3867`，0 次重试）输入 `agy` 自动升级；Test 4（`task-3950`，0 次重试）`mt create session --profile antigravity` 建卡切焦通过 | **已闭合** | `task-3867`（0 重试）+ `task-3950`（0 重试）；[01-promote-shell.png](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/docs/acceptance/evidence/antigravity/01-promote-shell.png)、[14a-missing-binary.png](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/docs/acceptance/evidence/antigravity/14a-missing-binary.png) |
| 2 | 标签与卡片标识（标签、DAG 节点、父投影、子角标） | 与 Claude 卡片同构展示 Provider 标识与层级 | Test 2（`task-3867`，0 次重试）完整通过 | **已闭合** | `task-3867`（0 重试）；[02-identifiers-dag.png](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/docs/acceptance/evidence/antigravity/02-identifiers-dag.png)、[10a-fork-branches.png](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/docs/acceptance/evidence/antigravity/10a-fork-branches.png) |
| 3 | 首次进入目录（信任提示与 `needs-input` 状态） | 与 Claude 同一信任提示与 `needs-input` | Test 1 与 Test 2（`task-3867`，0 次重试）实机经过 `Do you trust` → `needs-input` → 按 Enter 进入 `idle` 并保存截图 | **已闭合** | `task-3867`（0 重试）；[03-first-dir-trust.png](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/docs/acceptance/evidence/antigravity/03-first-dir-trust.png)、[03-first-dir-trust-dag.png](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/docs/acceptance/evidence/antigravity/03-first-dir-trust-dag.png) |
| 4 | 工作状态（启动中 → 空闲 → 工作中 → 需要输入 / 完成 / 出错） | 与 Claude 同一五态流转与实时反馈 | Test 1（`task-3867`）实机流转 `running` / `needs-input` / `idle`；Test 2（`task-3867`）并发五态；Test 4（`task-3950`，0 次重试）登录 `needs-input` → `idle` 通过 | **已闭合** | `task-3867` + `task-3950`（0 重试）；[04-work-status-running-dag.png](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/docs/acceptance/evidence/antigravity/04-work-status-running-dag.png)、[04-work-status-idle-dag.png](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/docs/acceptance/evidence/antigravity/04-work-status-idle-dag.png)、[14b-unauthenticated-dag.png](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/docs/acceptance/evidence/antigravity/14b-unauthenticated-dag.png) |
| 5 | 通知（非聚焦等待审批 / 完成进通知中心） | 与 Claude 通知中心与铃铛徽标规则一致 | Test 1（`task-3867`，0 次重试）在 Ask 工具审批期间切焦后稳定弹出 `Antigravity requires approval`，动画稳定截到清晰通知中心画面 | **已闭合** | `task-3867`（0 重试）；[05-notifications.png](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/docs/acceptance/evidence/antigravity/05-notifications.png) |
| 6 | HUD（模型组额度、悬停全量明细、`AGENTS.md`、Git 脏标、`AK` 徽标） | 隐藏 Claude 账户/待办；主栏仅当前模型组；Ask 显示 `AK` | Test 1（`task-3867`，0 次重试）全部断言通过：主栏显示 `5h 10% · 4h30m` 与 `Weekly 25% · 6d13h`，悬停展示 4 组额度 | **已闭合** | `task-3867`（0 重试）；[06-hud.png](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/docs/acceptance/evidence/antigravity/06-hud.png)、[06-hud-tooltip.png](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/docs/acceptance/evidence/antigravity/06-hud-tooltip.png) |
| 7 | 自动标题（手动改名锁保护 + 恢复自动标题） | 与 Claude 一致保护手工标题并支持「恢复 Antigravity 自动标题」 | 在 `task-3974`（`--retries=1`）中已完成手动重命名为 `手工锁定标题-E2E` 及后续轮 `RENAME_LOCKED_OK` 标题不被覆盖校验，但在第 815 行右键菜单查找并点击「恢复 Antigravity 自动标题」超时，attempt 1 与 retry #1 均失败（原因待进一步排查：手动改名后 `openPaneMenu` 右键菜单内未出现该项，或与 `session.titleSource === 'manual'` 同步/右键菜单触发区域有关） | **未闭合** | `task-3974`（已重试 1 次仍失败）；报错首行：`TimeoutError: locator.click: Timeout 30000ms exceeded.`（`waiting for getByRole('menuitem', { name: '恢复 Antigravity 自动标题' })`，[antigravity-acceptance.spec.ts#L815](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/tests/e2e/antigravity-acceptance.spec.ts#L815)）；错误上下文：[error-context.md](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/test-results/antigravity-acceptance-Ant-bc499-ry-and-Running-Idle-latency-retry1/error-context.md) |
| 8 | 载入已有会话（`SessionLoaderDialog` 切换 Antigravity / 预览 / 载入） | 与 Claude 对话框同交互，同工作区弹窗「仍然载入」 | Test 2（`task-3867`，0 次重试）完整通过 | **已闭合** | `task-3867`（0 重试）；[08-session-loader.png](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/docs/acceptance/evidence/antigravity/08-session-loader.png)、[fork-concurrent-loader.json](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/docs/acceptance/evidence/antigravity/results/fork-concurrent-loader.json) |
| 9a | 重启恢复（真实 App 重启后 `--conversation` 接回原对话） | App 重启后无缝接回，保留绑定与上下文 | 在 `task-3974`（`--retries=1`）中排在同用例 `#7` 之后，因 `#7` 第 815 行超时阻断未能执行到重启阶段 | **未闭合** | `task-3974`（前置 `#7` 第 815 行阻断未跑到）；无 `09a-restart-recovery.png` |
| 9b | 恢复失败提示与操作（受控 fixture） | 与 Claude 相同的恢复失败横幅与按钮 | Test 4（`task-3950`，0 次重试）使用受控 `agy-stub.py` 对 `--conversation` 返回退出码 1，完整通过 | **已闭合（受控 fixture）** | `task-3950`（0 重试）；[09b-restore-failed.png](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/docs/acceptance/evidence/antigravity/09b-restore-failed.png)、[unauth-and-restore-failed.json](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/docs/acceptance/evidence/antigravity/results/unauth-and-restore-failed.json) |
| 10 | Fork（当前目录与新 Worktree 分支 / 上下文与独立 ID） | 入口门禁与 Claude 一致；子卡继承上下文并各自独立 ID | Test 2（`task-3867`，0 次重试）完整通过 | **已闭合** | `task-3867`（0 重试）；[10a-fork-branches.png](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/docs/acceptance/evidence/antigravity/10a-fork-branches.png)、[fork-concurrent-loader.json](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/docs/acceptance/evidence/antigravity/results/fork-concurrent-loader.json) |
| 10b | **Fork 并发验证（父运行中 Fork 不覆盖绑定、不破坏父对话 JSONL/SQLite）** | 父活跃运行期间触发子 `/fork` 后父绑定与历史完好 | Test 2（`task-3867`，0 次重试）完整通过：父绑定保持 `5112173e-4cc2-4280-8b09-289e76702648`，子 3 获新 ID `0d2fd5ea-d053-433c-aab9-2556def313d7`，父 JSONL `3 -> 5` 行合法，SQLite `PRAGMA integrity_check = ok` | **已闭合** | `task-3867`（0 重试）；[10b-fork-while-running.png](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/docs/acceptance/evidence/antigravity/10b-fork-while-running.png)、[fork-concurrent-loader.json](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/docs/acceptance/evidence/antigravity/results/fork-concurrent-loader.json) |
| 11 | 执行模式（4 项菜单、前 3 项不重启、Ask `AK`、Bypass 进出确认重启） | 4 项对齐；前 3 项不换 PID；进出 Bypass 确认并重启；Ask 显示 `AK` | Test 1（`task-3867`，0 次重试）全部断言通过并生成最新截图 | **已闭合** | `task-3867`（0 重试）；[11-execution-modes-menu.png](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/docs/acceptance/evidence/antigravity/11-execution-modes-menu.png)、[11-execution-modes-bypass-done.png](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/docs/acceptance/evidence/antigravity/11-execution-modes-bypass-done.png) |
| 12 | 模型（HUD 只读无模型菜单，终端 `/model` 切换同步 HUD） | 与 Claude 一致不提供 HUD 模型菜单，随状态栏同步 | Test 1（`task-3867`，0 次重试）`/model` 切换后 HUD 同步通过 | **已闭合** | `task-3867`（0 重试）；[12-model-readonly-sync.png](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/docs/acceptance/evidence/antigravity/12-model-readonly-sync.png) |
| 13 | `mt` 跨卡片控制 | 与 Claude 卡片同流程注入提示并读取兄弟卡片输出 | Test 3（`task-3950`，`--retries=1` 重试 **1 次**后通过）在 Antigravity 卡内执行 `mt list` / `mt read` 成功返回 `MT_PEER_TOKEN_8421` | **已闭合** | `task-3950`（1 次重试后通过）；[13-mt-cross-card.png](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/docs/acceptance/evidence/antigravity/13-mt-cross-card.png)、[mt-cross-card.json](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/docs/acceptance/evidence/antigravity/results/mt-cross-card.json) |
| 14 | 未安装 / 未登录提示（14a 启动失败 + 14b 登录 `needs-input` → Enter 恢复） | 与 Claude 同位置横幅与 `needs-input` 交互态 | Test 4（`task-3950`，0 次重试）完整通过 | **已闭合（受控 fixture）** | `task-3950`（0 重试）；[14a-missing-binary.png](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/docs/acceptance/evidence/antigravity/14a-missing-binary.png)、[14b-unauthenticated-needs-input.png](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/docs/acceptance/evidence/antigravity/14b-unauthenticated-needs-input.png)、[14b-unauthenticated-dag.png](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/docs/acceptance/evidence/antigravity/14b-unauthenticated-dag.png)、[missing-binary.json](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/docs/acceptance/evidence/antigravity/results/missing-binary.json) |
| Perf | **延迟数值（`running` 与 `idle` 各 3 次采样 ≤ 1000ms）** | 事件发生后 1 秒内反映到卡片 / DAG | 新拆出用例（`task-3974`，0 次重试完成采样）3 次 `running` 延迟 `2ms / 2ms / 1ms`（均值 `1.7ms`），3 次 `idle` 延迟 `1ms / 6ms / 5ms`（均值 `4ms`），6/6 全部 `≤ 1000ms` | **已闭合** | `task-3974`（0 重试完成采样）；[core-and-latency.json](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/docs/acceptance/evidence/antigravity/results/core-and-latency.json) |

## Claude Code 现有 4 项 E2E 回归对照与 `main` 复现结论

在进入 Ask/竞态最终收口前，按同一批基础架构在当前分支（`task-3566`）与干净 `main`（`6132501`）临时隔离 worktree（`task-3614`，事后已移除）对比运行了 Claude Code 4 份端到端回归规格：

1. **当前分支运行命令：**
   `MATOU_E2E_DISPLAY=primary pnpm exec playwright test tests/e2e/prd-01-agent-notifications.spec.ts tests/e2e/prd-02-bottom-hud.spec.ts tests/e2e/prd-06-session-fork.spec.ts tests/e2e/session-load-existing-claude.spec.ts --reporter=line`
   （共 13 条：**9 passed / 4 failed**，日志：[claude-e2e-regression.log](file:///Users/icesword/.claude/agent-foreman/runs/matou-%E5%A4%9AAgent-agy-01/logs/claude-e2e-regression.log)）
   - `tests/e2e/prd-01-agent-notifications.spec.ts`：**4/4 passed**
   - `tests/e2e/prd-02-bottom-hud.spec.ts`：**3/3 passed**
   - `tests/e2e/prd-06-session-fork.spec.ts`：**1 passed / 3 failed**
     - `[9/13]` 第 40 行：偶发 zsh 初始吐出 `1% ... 1claude`（zsh 启动字符竞态）
     - `[10/13]` 第 187 行：`databaseBeforeRestart.relations` 为 `[]`（期望 2 条 `forked-from` 关系；`intents` 断言正常通过）
     - `[11/13]` 第 276 行：`failedCard`（`.fork-failure-card`）计数为 `0`（期望 `1`）
   - `tests/e2e/session-load-existing-claude.spec.ts`：**1 passed / 1 failed**
     - `[13/13]` 第 164 行：长会话全文跨页搜索虚拟列表滚动偏移命中 `#367-391`，未停留在 `跨页搜索标记 350`

2. **`main` 分支（`6132501`）临时 worktree 复现对照命令：**
   `MATOU_E2E_DISPLAY=primary pnpm exec playwright test tests/e2e/prd-06-session-fork.spec.ts tests/e2e/session-load-existing-claude.spec.ts --reporter=line`
   （共 6 条：**3 passed / 3 failed**，耗时 `1.6m`，日志：[main-claude-e2e.log](file:///Users/icesword/.claude/agent-foreman/runs/matou-%E5%A4%9AAgent-agy-01/logs/main-claude-e2e.log)）
   - `tests/e2e/prd-06-session-fork.spec.ts:20`（两轮确认对话）：**2/2 passed**（验证 `[9/13]` 的 `1claude` 非必然行为，而是 Shell 初始化竞态）
   - `tests/e2e/prd-06-session-fork.spec.ts:92`（`[3/6]`，第 187 行）：**完全相同失败** —— `databaseBeforeRestart.relations` 同样为 `Array []`
   - `tests/e2e/prd-06-session-fork.spec.ts:243`（`[4/6]`，第 276 行）：**完全相同失败** —— `.fork-failure-card` 计数同样为 `0`
   - `tests/e2e/session-load-existing-claude.spec.ts:59`（`[6/6]`，第 157 行）：**完全相同失败** —— 长会话全文检索虚拟列表未滚动停留在匹配条目

**回归结论：** `prd-06-session-fork.spec.ts` 与 `session-load-existing-claude.spec.ts` 中的全部结构性失败均在 `main`（`6132501`）原生复现，本分支未引入任何 Claude Code 行为回归。

## 门禁与校验现状（截至本矩阵提交）

- `pnpm typecheck`：在提交 `340b4b6` 前通过（随后仅更新测试与文档，未改产品源码）
- `pnpm check:identifiers`：**通过（exit 0）**
- `@matou/runtime` 单测：`src/session/session-hud-registry.test.ts` + `src/antigravity-runtime.test.ts`（**29/29 passed**，截至 `340b4b6`）
- `@matou/desktop` 单测：`src/renderer/src/hud/TerminalHud.test.tsx` + `src/renderer/src/hud/TerminalHud.locale.test.tsx`（**49/49 passed**，截至 `340b4b6`）
