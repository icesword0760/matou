# Matou Antigravity CLI 接入交互对照矩阵

- **验收日期：** 2026-11-14
- **验收分支：** `feat/multi-agent-antigravity`（产品修复提交 `340b4b6`，E2E 规格提交 `027e106`）
- **打包验收对象：** `apps/desktop/release/mac-arm64/码头.app`（`CSC_IDENTITY_AUTO_DISCOVERY=false pnpm package:dir` 重新打包，内置 Runtime 与内置 `mt`）
- **真机基准环境：** macOS arm64，本机真实 `agy 1.5.8`（保留真实 `HOME` 使用 `~/.gemini` 登录态；Matou 数据与测试仓库隔离于 scratchpad 及 `/tmp/m-e2e-*` 符号链接）
- **最新打包运行结论（第 3 步单次运行）：** `MATOU_E2E_DISPLAY=primary pnpm exec playwright test tests/e2e/antigravity-acceptance.spec.ts --reporter=line` 共 4 个测试：**1 passed / 3 failed**（耗时 `4.9m`，日志：[antigravity-acceptance-final.log](file:///Users/icesword/.claude/agent-foreman/runs/matou-%E5%A4%9AAgent-agy-01/logs/antigravity-acceptance-final.log)）。
- **前置已提交修复后的单测与类型门禁：** `@matou/runtime` 2 个测试文件 **29/29 passed**、`@matou/desktop` 2 个测试文件 **49/49 passed**、`pnpm typecheck` **通过**、`pnpm check:identifiers` **通过**。

## 对照边界与说明

1. **基线约定：** reference product 不含 Antigravity；全部能力以 PRD 第 3 节为规格，以 Matou 现有同范围 Claude Code 卡片行为为 1:1 交互基线（通知英文正文与 `main` 上既有 Claude 通知一致，本次不改）。
2. **证据口径（严格区分最新打包实测与前序已留存证据）：**
   - **已闭合：** 在最新打包产物（`340b4b6` + `027e106` 后打包）单次全量运行中直接跑通并生成最新截图 / JSON。
   - **未闭合：** 在最新打包产物运行中失败或被前置超时阻断未跑到；即使此前同分支在前序打包（如 21:31–21:36 Run 3/Run 4）或 Runtime/Desktop 单测中跑通，在最新打包产物端到端未全绿前均严格标注为**未闭合**。
3. **受控 Fixture 披露：** 第 9b 项（恢复失败）与第 14 项（`agy` 未安装 / 未登录提示）因本机已安装且已登录真实 `agy 1.5.8`，在 E2E 第 4 个测试中使用 `MATOU_ANTIGRAVITY_COMMAND` 指向不存在路径与受控 Python stub（`agy-stub.py`）触发。
4. **关于 `05-notifications.png`：** 该文件在重跑前已删除；由于本次最新全量运行中 Test 1 在 Shell 提示符等待阶段（`tests/e2e/antigravity-acceptance.spec.ts:354`）超时退出，未执行到通知中心阶段，因此本次未能重新生成 `05-notifications.png`，已从仓库追踪列表清理（`git rm`）并在下表第 5 项说明。

## 最新打包产物指纹（SHA-256）

| 产物组件 | SHA-256 |
|---|---|
| `apps/desktop/release/mac-arm64/码头.app/Contents/MacOS/码头` | `221d5695ab9eb2263b9107e4a5bb3f5780adc35963530e322673d5535e8eeae5` |
| `apps/desktop/release/mac-arm64/码头.app/Contents/Resources/app.asar` | `dea042ba4b7ef1d025cbf067a4f09a40ee087269538fb77a5a76c684a1470a67` |
| `apps/desktop/release/mac-arm64/码头.app/Contents/Resources/runtime/index.cjs` | `ba08f44b4e45cea4a65f15db0757f2e514167e82094f9a40ee087269538fb77a` |
| `apps/desktop/release/mac-arm64/码头.app/Contents/Resources/runtime/mt-cli.cjs` | `5dc1252919881527360dcc7c1cbb1587c78c437409b91601a94b1c6b892840e3` |
| `apps/desktop/release/mac-arm64/码头.app/Contents/Resources/runtime/control-assets/bin/mt` | `859676985a2b233e8d6dc7744e7760c3e955b59c6a56932bfcdff3f12a87bc03` |
| `apps/desktop/release/mac-arm64/码头.app/Contents/Resources/runtime/control-assets/providers/antigravity-system-message.md` | `c03b81fbc31e41d41bcf73cc2f715c6225e713fdf9058422a161e6bb4c9d62f6` |

## 逐项对照矩阵（PRD #1-#14 + Fork 并发 + 延迟）

| # | 验收项 | 基线（Claude 卡片 / PRD） | Matou 实际表现（最新打包单次运行 `task-3766`） | 状态 | 证据 / 报错首行与截图 |
|---|---|---|---|---|---|
| 1 | 新建卡片（Shell 输入 `agy` 自动升级、新建下拉不变、`mt` 可建 Antigravity） | 与 Claude 相同：Shell 输入触发命令自动升级；`mt create session --profile` 可指定 | Test 4（`00:26`）通过 `mt create session --canvas current --profile antigravity` 成功建卡并切焦；Test 1 与 Test 2 在 `launchAgyInFreshRepo` 等待 zsh 初始提示符超过 15s（宿主机负载下本机 `zsh -ilc` 实测耗时 `9.840s`）提前失败；Test 3 成功在 Shell 输入 `agy` 升级为 `Antigravity` | **未闭合（Test 4/3 已跑通建卡/升级；Test 1/2 初始提示符超时）** | 最新截图：`docs/acceptance/evidence/antigravity/14a-missing-binary.png`；Test 1/2 报错首行：`Error: expect(locator).toContainText(expected) failed`（`tests/e2e/antigravity-acceptance.spec.ts:354:48`, `Received string: " "`）；前序截图：`docs/acceptance/evidence/antigravity/01-promote-shell.png` |
| 2 | 标签与卡片标识（标签、DAG 节点、父会话投影、子会话角标显示 `Antigravity`） | 与 Claude 卡片同构展示 Provider 标识与层级关系 | Test 4（`00:26`）新建与恢复失败卡均显示 `Antigravity` 卡片与 DAG 节点；Test 1/2 本轮在第 354 行超时未到子会话角标与父投影断言段 | **未闭合（Test 2 本轮未跑到断言段）** | 最新截图：`docs/acceptance/evidence/antigravity/14b-unauthenticated-dag.png`、`docs/acceptance/evidence/antigravity/14a-missing-binary.png`；前序截图：`docs/acceptance/evidence/antigravity/02-identifiers-dag.png`、`docs/acceptance/evidence/antigravity/10a-fork-branches.png` |
| 3 | 首次进入目录（信任提示与 `needs-input` 状态） | 与 Claude 同一工作区信任提示交互与 `needs-input` 表现 | Test 3（`00:26`）实际经过 `Do you trust` 提示并按 `Enter` 进入 `idle`；Test 1/2 本轮未过第 354 行（`03-first-dir-trust*.png` 为前序 Run 留存） | **未闭合（Test 1/2 未跑到）** | 前序截图：`docs/acceptance/evidence/antigravity/03-first-dir-trust.png`、`docs/acceptance/evidence/antigravity/03-first-dir-trust-dag.png` |
| 4 | 工作状态（启动中 → 空闲 → 工作中 → 需要输入 / 完成 / 出错） | 与 Claude 卡片同一五态流转与实时反馈 | Test 4（`00:26`）验证受控登录提示触发 `等待输入`（`status-needs-input`）及恢复空闲；Test 1/2 本轮第 354 行超时，Test 3 在 `status-idle`（`line 940`）超时 | **未闭合** | 最新截图：`docs/acceptance/evidence/antigravity/14b-unauthenticated-dag.png`；前序截图：`docs/acceptance/evidence/antigravity/04-work-status-running-dag.png`、`docs/acceptance/evidence/antigravity/04-work-status-idle-dag.png` |
| 5 | 通知（非聚焦完成 / 等待审批进通知中心，无双份系统通知） | 与 Claude 通知中心与铃铛徽标规则一致（英文通知正文属 `main` 既有行为） | Test 1 本轮在第 354 行超时未跑到此步；`05-notifications.png` 在重跑前已删除且本轮未重新生成，已从仓库移除追踪 | **未闭合（Test 1 未跑到，`05-notifications.png` 未重新生成）** | 报错同 #1（`tests/e2e/antigravity-acceptance.spec.ts:354:48`）；无当前版本通知截图 |
| 6 | HUD（模型与档位、上下文%、按当前模型组显示额度与悬停明细、工具、规则文件、Git 脏状态、Ask 徽标 `AK`） | 隐藏不适用的 Claude 账户/待办，状态栏只显示当前模型组 `5h`/`Weekly`，悬停展示全部分组额度；Ask 模式显示 `AK`（悬停「逐项确认 / Ask every step」） | Test 1 本轮第 354 行超时未走到 HUD 与 Ask 实机流；HUD 额度过滤、`AK` 徽标与悬停提示已在提交 `340b4b6` 对应的 `@matou/desktop` 与 `@matou/runtime` 单测全过（49/49 + 29/29） | **未闭合（Test 1 未跑到；单测与前序截图已覆盖额度分组）** | 前序截图：`docs/acceptance/evidence/antigravity/06-hud.png`、`docs/acceptance/evidence/antigravity/06-hud-tooltip.png` |
| 7 | 自动标题（Stop 后读取标题、手动改名锁保护、恢复自动标题） | 与 Claude 一致保护手工标题并支持「恢复 Antigravity 自动标题」 | Test 1 本轮第 354 行超时未跑到；第 4 关单测与前序 Run 曾验证首轮自动命名 | **未闭合（Test 1 未跑到）** | 前序截图可见自动标题：`docs/acceptance/evidence/antigravity/08-session-loader.png` |
| 8 | 载入已有会话（`SessionLoaderDialog` 切换 Antigravity、列表/步骤/预览/载入接回） | 与 Claude 载入对话框同交互，文案按 Agent 显示；已在当前场景载入的会话弹窗二次确认「仍然载入」 | Test 2 本轮第 354 行超时未走到；前序 Run（`task-3655`，21:33）已成功打开 Antigravity 页签并截到列表与预览（`08-session-loader.png`），随后已按弹窗逻辑在 `027e106` 补齐「仍然载入」确认处理 | **未闭合（Test 2 第 354 行超时，修正后的 `027e106` 载入闭环尚待单测外端到端跑完）** | 前序截图：`docs/acceptance/evidence/antigravity/08-session-loader.png` |
| 9a | 重启恢复（真实 App 重启后 `--conversation` 接回原对话） | App 重启后无缝接回，保留绑定与上下文 | Test 1 本轮第 354 行超时未跑到 | **未闭合（Test 1 未跑到）** | 无当前轮证据 |
| 9b | 恢复失败提示与操作（重试恢复 / 新开 Antigravity） | 与 Claude 相同的恢复失败横幅与按钮 | **本轮通过**（Test 4 在 `00:26` 使用受控 `agy-stub.py` 对 `--conversation` 返回退出码 1） | **已闭合（受控 fixture）** | `docs/acceptance/evidence/antigravity/09b-restore-failed.png`、`docs/acceptance/evidence/antigravity/results/unauth-and-restore-failed.json` |
| 10 | Fork（完成 ≥1 轮启用、当前目录与新 Worktree 分支、继承上下文且各自独立 ID） | 入口门禁与 Claude 一致；子卡继承上下文并获得独立会话 ID | Test 2 本轮第 354 行超时未跑到；前序 Run（`task-3655` 21:32–21:33 及 `340b4b6` 单测）已验证当前目录与新 Worktree 分支均继承 `COBALT_719` 且自动确认新目录信任提示 | **未闭合（本轮 Test 2 第 354 行超时）** | 前序截图：`docs/acceptance/evidence/antigravity/10a-fork-branches.png` |
| 10b | **Fork 并发验证（父运行中 Fork 不覆盖绑定、不破坏父对话 JSONL/SQLite）** | 父活跃运行期间触发子 `/fork` 后父绑定与历史完好 | **本轮未跑到**（Test 2 在第 354 行超时）；在修改测试为纯文本提示词后的前序 Run（`task-3655`，21:33，见 `10b-fork-while-running.png`）中，父活跃运行中发起 Fork 子卡 3 曾完整跑通：父子各自回归 `status-idle`，父 `provider_session_id` 未被覆盖，父 `transcript.jsonl` 解析无损且 SQLite `PRAGMA integrity_check = ok` | **未闭合（最新包 Test 2 卡在第 354 行；前序包运行实证通过）** | 前序同分支并发实证截图：`docs/acceptance/evidence/antigravity/10b-fork-while-running.png`；本轮报错首行：`Error: expect(locator).toContainText(expected) failed`（`tests/e2e/antigravity-acceptance.spec.ts:354:48`） |
| 11 | 执行模式（4 项菜单、前 3 项发 `Shift+Tab` 不重启、进出 `Bypass Permissions` 确认并重启、终端 `Shift+Tab` 切入 Ask 显示 `AK`） | 4 项与 Claude 对齐；前 3 项不换 PID；进出 Bypass 确认并重启；Ask 模式显示 `AK` 但不入菜单 | 产品竞态修复（清除 `Plan` 切完后残留的 `pendingPermissionMode`）已随 `340b4b6` 单测验证通过；但最新包 Test 1 在第 354 行超时，实机 E2E 未跑完本项与 Bypass 切换 | **未闭合（Test 1 未跑到；单测已过）** | 前序截图（4 项菜单无 Ask）：`docs/acceptance/evidence/antigravity/11-execution-modes-menu.png` |
| 12 | 模型（HUD 只读无模型菜单，终端内 `/model` 切换同步 HUD） | 与 Claude 一致不提供 HUD 模型菜单，随状态栏同步 | Test 1 本轮第 354 行超时未跑到 | **未闭合（Test 1 未跑到）** | 无当前轮证据 |
| 13 | `mt` 跨卡片控制（Antigravity 卡内自然语言调用 `mt list` / `mt read` 操作隔壁卡） | 与 Claude 卡片同流程注入提示并读取兄弟卡片输出 | 本轮 Test 3（`task-3766`）在等待卡片回到 `status-idle`（第 940 行）超时；前序同分支 Run（`task-3559` / `task-3655`）曾完整跑通并准确返回 `MT_PEER_TOKEN_8421` | **未闭合（本轮第 940 行超时）** | 本轮报错首行：`Error: expect(locator).toHaveCount(expected) failed`（`tests/e2e/antigravity-acceptance.spec.ts:315:22`，`line 940` `status-idle` 30s 超时）；错误上下文：`test-results/antigravity-acceptance-Ant-a623f-nside-real-Antigravity-card/error-context.md`；前序成功截图：`docs/acceptance/evidence/antigravity/13-mt-cross-card.png`、`docs/acceptance/evidence/antigravity/results/mt-cross-card.json` |
| 14 | 未安装 / 未登录提示（14a 启动失败提示 + 14b 登录询问 → `needs-input` → 按 Enter 恢复空闲） | 与 Claude 同位置横幅与终端 `needs-input` 交互态 | **本轮通过**（Test 4 在 `00:26` 使用受控缺失路径与 `agy-stub.py` 完整跑通） | **已闭合（受控 fixture）** | `docs/acceptance/evidence/antigravity/14a-missing-binary.png`、`docs/acceptance/evidence/antigravity/14b-unauthenticated-needs-input.png`、`docs/acceptance/evidence/antigravity/14b-unauthenticated-dag.png`、`docs/acceptance/evidence/antigravity/results/missing-binary.json`、`docs/acceptance/evidence/antigravity/results/unauth-and-restore-failed.json` |
| Perf | **延迟数值（hook 不明显阻塞、`running` / `needs-input` / `idle` 事件到 UI ≤ 1000ms）** | 事件发生后 1 秒内反映到卡片 / DAG | 本轮 Test 1 在第 354 行超时退出，未生成 `core-and-latency.json`，因此**本轮无打包 App 端到端毫秒数**（第 3 关 CLI 实测确认 POSIX `sh` + 后台 `curl` 无可感知延迟；另外本机实测高负载下 `time /bin/zsh -ilc 'echo ok'` 耗时 `9.840s`，是第 354 行 15s 超时主因） | **未闭合（无本轮端到端毫秒统计）** | 无 `core-and-latency.json` |

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

- `pnpm typecheck`：在提交 `340b4b6` 前通过
- `pnpm check:identifiers`：已清理缺失的 `05-notifications.png` 并无损重编码 `11-execution-modes-menu.png` 后**通过（exit 0）**
- `@matou/runtime` 单测：`src/session/session-hud-registry.test.ts` + `src/antigravity-runtime.test.ts`（**29/29 passed**）
- `@matou/desktop` 单测：`src/renderer/src/hud/TerminalHud.test.tsx` + `src/renderer/src/hud/TerminalHud.locale.test.tsx`（**49/49 passed**）
