# Matou Antigravity CLI 接入交互对照矩阵

- **验收日期：** 2026-11-14
- **验收分支：** `feat/multi-agent-antigravity`（产品代码最新提交 `340b4b6`，测试与矩阵随后更新）
- **打包验收对象：** `apps/desktop/release/mac-arm64/码头.app`（`CSC_IDENTITY_AUTO_DISCOVERY=false pnpm package:dir` 打包，内置 Runtime 与内置 `mt`；产品代码自 `340b4b6` 后零改动）
- **真机基准环境：** macOS arm64，本机真实 `agy 1.5.8`（保留真实 `HOME` 使用 `~/.gemini` 登录态；Matou 数据与测试仓库隔离于 scratchpad 及 `/tmp/m-e2e-*` 符号链接）。测试启动打包 App 时使用空临时 `ZDOTDIR`（仅含空 `.zshrc`）并在 `PATH` 前置 `/Users/icesword/.local/bin`，彻底绕开宿主机 `~/.zshrc` 在高负载下 `9.840s` 的冷启动耗时（实机 `zsh` CLI 实测 `0.045s`；打包 `码头.app` 探针 `pidToPromptMs = 30ms`），同时将 Shell 提示符超时放宽至 `30s`、真实模型 `status-idle` 等待放宽至 `90s`。
- **最新打包单次完整运行结论（`task-3867`）：**
  `MATOU_E2E_DISPLAY=primary pnpm exec playwright test tests/e2e/antigravity-acceptance.spec.ts --reporter=line`
  共 4 个测试：**1 passed / 3 failed**（耗时 `4.3m`，日志：[antigravity-acceptance-zdotdir-3.log](file:///Users/icesword/.claude/agent-foreman/runs/matou-%E5%A4%9AAgent-agy-01/logs/antigravity-acceptance-zdotdir-3.log)）。
  - 注：同打包产物在注入 `ZDOTDIR` 后的前一次运行（`task-3856`，耗时 `4.9m`，仅差 Test 1 Ask 审批循环测试修正）曾跑出 **3 passed / 1 failed**（Test 2、Test 3 `#13`、Test 4 `#9b #14` 全过）。两次运行均未杀宿主机用户进程（CloudMounter、用户自用码头/Claude、另一 Electron 开发版、Antigravity IDE）。
- **前置已提交修复后的单测与类型门禁（`340b4b6`）：** `@matou/runtime` 2 个测试文件 **29/29 passed**、`@matou/desktop` 2 个测试文件 **49/49 passed**、`pnpm typecheck` **通过**、`pnpm check:identifiers` **通过**。

## 对照边界与说明

1. **基线约定：** reference product 不含 Antigravity；全部能力以 PRD 第 3 节为规格，以 Matou 现有同范围 Claude Code 卡片行为为 1:1 交互基线（通知英文正文与 `main` 上既有 Claude 通知一致，本次不改）。
2. **证据口径（严格以最新单次全量运行 `task-3867` 为准）：**
   - **已闭合：** 在最新单次全量运行 `task-3867` 中所在测试用例完整 pass，并产出当前版本截图 / JSON。
   - **未闭合：** 在 `task-3867` 中所在测试用例 failed 或因前置步骤失败未跑到。若 Test 1 在对应功能断言段已通过并生成了 `task-3867` 最新截图、或同包前次 `task-3856` 曾跑通，在下表如实分项注明，不粉饰为整项完全通过。
3. **受控 Fixture 披露：** 第 9b 项（恢复失败）与第 14 项（`agy` 未安装 / 未登录提示）因本机已安装且已登录真实 `agy 1.5.8`，在 E2E 第 4 个测试中使用 `MATOU_ANTIGRAVITY_COMMAND` 指向不存在路径与受控 Python stub（`agy-stub.py`）触发。
4. **关于 `05-notifications.png`：** 本轮 `task-3867` 已在动画完全稳定后重新生成 [05-notifications.png](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/docs/acceptance/evidence/antigravity/05-notifications.png)，清晰展示通知中心 `通知 (1)`、`Antigravity / Permission / Antigravity requires approval` 及后台审批提示，无半透明叠加或文字重影。

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

| # | 验收项 | 基线（Claude 卡片 / PRD） | Matou 实际表现（`340b4b6` 打包，最新运行 `task-3867`） | 状态 | 证据 / 报错首行与截图 |
|---|---|---|---|---|---|
| 1 | 新建卡片（Shell 输入 `agy` 自动升级、新建下拉不变、`mt` 可建 Antigravity） | 与 Claude 相同：Shell 输入触发命令自动升级；`mt create session --profile` 可指定 | Test 1 与 Test 2 在空 Shell 提示符后输入 `agy` 均稳定升级为 `Antigravity`；Test 4 的 `mt create session --canvas current --profile antigravity` 本轮（`task-3867`）在提示符检查第 360 行因前置出现字符 `'1'` 中断（同包前次 `task-3856` 通过） | **未闭合**（升级段本轮通过；Test 4 `mt` 建卡段本轮第 360 行前置失败，前次 `task-3856` 通过） | 本轮截图：`docs/acceptance/evidence/antigravity/01-promote-shell.png`；Test 4 报错首行：`Error: promptRows="icesword@icesworddeMacBook-Pro repo % 1 "`（`line 360`）；前次 `task-3856` 截图：`docs/acceptance/evidence/antigravity/14a-missing-binary.png` |
| 2 | 标签与卡片标识（标签、DAG 节点、父会话投影、子会话角标显示 `Antigravity`） | 与 Claude 卡片同构展示 Provider 标识与层级关系 | **本轮 Test 2 完整通过**：DAG 节点显示 `Antigravity`、父会话底部投影与子会话角标计数（最终 `3`）、标签标题与层级切换一致 | **已闭合** | `docs/acceptance/evidence/antigravity/02-identifiers-dag.png`、`docs/acceptance/evidence/antigravity/10a-fork-branches.png` |
| 3 | 首次进入目录（信任提示与 `needs-input` 状态） | 与 Claude 同一工作区信任提示交互与 `needs-input` 表现 | Test 1 与 Test 2 均实际触发 `Do you trust the contents of this project?`、DAG 进入 `needs-input`、按 `Enter` 确认后进入 `idle` 并保存最新截图；因 Test 1 后续在第 691 行失败，Test 2 中同流程亦全通 | **未闭合（对应段与截图本轮已通过，但 Test 1 整体未过；Test 2 同流程已通过）** | 本轮截图：`docs/acceptance/evidence/antigravity/03-first-dir-trust.png`、`docs/acceptance/evidence/antigravity/03-first-dir-trust-dag.png`；Test 1 报错见 #7 |
| 4 | 工作状态（启动中 → 空闲 → 工作中 → 需要输入 / 完成 / 出错） | 与 Claude 卡片同一五态流转与实时反馈 | Test 1 跑通 `running` → `idle`、工具审批 `needs-input` → `idle`，并生成最新 DAG 截图；Test 2 跑通父子并发五态回落 `idle`；Test 1 在 #7 跟进轮第 691 行 `status-idle` 等待 90s 超时，Test 4 登录 `needs-input` 本轮前置第 360 行中断（`task-3856` 通过） | **未闭合** | 本轮截图：`docs/acceptance/evidence/antigravity/04-work-status-running-dag.png`、`docs/acceptance/evidence/antigravity/04-work-status-idle-dag.png`；Test 1 报错首行：`Error: expect(locator).toHaveCount(expected) failed`（`line 691`, `status-idle` 90s timeout）；前次 `task-3856` 截图：`14b-unauthenticated-dag.png` |
| 5 | 通知（非聚焦完成 / 等待审批进通知中心，无双份系统通知） | 与 Claude 通知中心与铃铛徽标规则一致（英文通知正文属 `main` 既有行为） | **本轮 Test 1 中该段完整通过**：切换焦点至 Shell 卡后，在 Ask 模式触发真实工具审批，通知中心稳定弹出 `Antigravity requires approval` 并重新生成清晰截图（`task-3867` 于 `00:45` 写入）；但 Test 1 后续在 #7 第 691 行超时 | **未闭合（通知断言与 `05-notifications.png` 重拍本轮均通过，但 Test 1 后续第 691 行失败）** | 本轮重拍截图：`docs/acceptance/evidence/antigravity/05-notifications.png` |
| 6 | HUD（模型与档位、上下文%、按当前模型组显示额度与悬停明细、工具、规则文件、Git 脏状态、Ask 徽标 `AK`） | 隐藏不适用的 Claude 账户/待办，状态栏只显示当前模型组 `5h`/`Weekly`，悬停展示全部分组额度；Ask 模式显示 `AK`（悬停「逐项确认 / Ask every step」） | **本轮 Test 1 中该段全部断言通过**：主栏仅显示当前 Gemini 模型组 `5h 10% · 4h30m` 与 `Weekly 25% · 6d13h`、悬停面板展示全部 4 组额度、`AGENTS.md`、Git `main*` 脏标、无 Claude 账户/待办、Ask 模式徽标 `AK`；Test 1 后续在 #7 第 691 行失败 | **未闭合（HUD 断言与截图本轮均通过，但 Test 1 后续第 691 行失败）** | 本轮截图：`docs/acceptance/evidence/antigravity/06-hud.png`、`docs/acceptance/evidence/antigravity/06-hud-tooltip.png` |
| 7 | 自动标题（Stop 后读取标题、手动改名锁保护、恢复自动标题） | 与 Claude 一致保护手工标题并支持「恢复 Antigravity 自动标题」 | Test 1 完成摘要生成后手动重命名为 `手工锁定标题-E2E`，随后发送 `Reply with ONLY RENAME_LOCKED_OK` 在第 691 行等待 `status-idle` 超 90s 超时，未能执行到锁定复检与「恢复 Antigravity 自动标题」 | **未闭合** | Test 1 报错首行：`Error: expect(locator).toHaveCount(expected) failed`（`tests/e2e/antigravity-acceptance.spec.ts:326:22`, `line 691`, `.dag-node-card[data-session-id="a14a3f6c-1b4a-46d5-96aa-fd5f33bd22bc"].status-idle`）；失败上下文：`test-results/antigravity-acceptance-Ant-3e566-ifecycle-in-packaged-码头-app/error-context.md`（无 `07-auto-title.png`） |
| 8 | 载入已有会话（`SessionLoaderDialog` 切换 Antigravity、列表/步骤/预览/载入接回） | 与 Claude 载入对话框同交互，文案按 Agent 显示；已在当前场景载入的会话弹窗二次确认「仍然载入」 | **本轮 Test 2 完整通过**：切换至 Antigravity 页签、预览 `#25` 步长对话、点击「载入到当前卡片」并在提示重复时确认「仍然载入」，新卡片成功接入 `agent` HUD | **已闭合** | `docs/acceptance/evidence/antigravity/08-session-loader.png`、`docs/acceptance/evidence/antigravity/results/fork-concurrent-loader.json` |
| 9a | 重启恢复（真实 App 重启后 `--conversation` 接回原对话） | App 重启后无缝接回，保留绑定与上下文 | 因 Test 1 在前置第 7 项（第 691 行）超时，本轮未执行到 App 重启恢复段 | **未闭合（前置第 691 行阻断）** | 报错同 #7（`line 691`）；无 `09a-restart-recovery.png` |
| 9b | 恢复失败提示与操作（重试恢复 / 新开 Antigravity） | 与 Claude 相同的恢复失败横幅与按钮 | 本轮 Test 4（`task-3867`）在 14a 段起始提示符检查第 360 行因串入 `'1'` 中断，未走到 9b；同打包 `ZDOTDIR` 前次运行（`task-3856`，`00:43`）完整通过并生成 `09b-restore-failed.png` | **未闭合（本轮第 360 行前置失败；同包前次 `task-3856` 通过）** | Test 4 报错首行：`Error: promptRows="icesword@icesworddeMacBook-Pro repo % 1 "`（`tests/e2e/antigravity-acceptance.spec.ts:360:67`）；错误上下文：`test-results/antigravity-acceptance-Ant-c6ddc--prompt-and-restore-failure/error-context.md`；前次 `task-3856` 截图：`docs/acceptance/evidence/antigravity/09b-restore-failed.png`、`results/unauth-and-restore-failed.json` |
| 10 | Fork（完成 ≥1 轮启用、当前目录与新 Worktree 分支、继承上下文且各自独立 ID） | 入口门禁与 Claude 一致；子卡继承上下文并获得独立会话 ID | **本轮 Test 2 完整通过**：完成 `COBALT_719` 首轮后 Fork「当前目录」与「新 Worktree」两条子分支均自动确认信任并成功继承上下文、分配独立 `provider_session_id` | **已闭合** | `docs/acceptance/evidence/antigravity/10a-fork-branches.png`、`docs/acceptance/evidence/antigravity/results/fork-concurrent-loader.json` |
| 10b | **Fork 并发验证（父运行中 Fork 不覆盖绑定、不破坏父对话 JSONL/SQLite）** | 父活跃运行期间触发子 `/fork` 后父绑定与历史完好 | **本轮 Test 2 完整通过**：父卡流式输出 25 句期间发起子卡 3 `/fork`，父子均独立回到 `status-idle`；父绑定保持 `5112173e-4cc2-4280-8b09-289e76702648`、子 3 获新 ID `0d2fd5ea-d053-433c-aab9-2556def313d7`；父 JSONL 行数从 `3` 增至 `5` 且每行 JSON 合法，SQLite `PRAGMA integrity_check = ok` | **已闭合** | `docs/acceptance/evidence/antigravity/10b-fork-while-running.png`、`docs/acceptance/evidence/antigravity/results/fork-concurrent-loader.json` |
| 11 | 执行模式（4 项菜单、前 3 项发 `Shift+Tab` 不重启、进出 `Bypass Permissions` 确认并重启、终端 `Shift+Tab` 切入 Ask 显示 `AK`） | 4 项与 Claude 对齐；前 3 项不换 PID；进出 Bypass 确认并重启；Ask 模式显示 `AK` 但不入菜单 | **本轮 Test 1 中该段全部断言通过**：4 项菜单无 Ask、Accept Edits → Plan Mode 不换 PID、终端 `Shift+Tab` 切入 Ask 显示 `AK`、菜单切回 `Default`、进出 `Bypass Permissions` 弹窗确认并重启切换 `BP` → `D`；但 Test 1 后续在 #7 第 691 行失败 | **未闭合（执行模式断言与截图本轮均通过，但 Test 1 后续第 691 行失败）** | 本轮截图：`docs/acceptance/evidence/antigravity/11-execution-modes-menu.png`、`docs/acceptance/evidence/antigravity/11-execution-modes-bypass-done.png` |
| 12 | 模型（HUD 只读无模型菜单，终端内 `/model` 切换同步 HUD） | 与 Claude 一致不提供 HUD 模型菜单，随状态栏同步 | **本轮 Test 1 中该段全部断言通过**：终端内执行 `/model` 并选择切换后 HUD 模型标签正常同步；但 Test 1 后续在 #7 第 691 行失败 | **未闭合（模型同步断言与截图本轮均通过，但 Test 1 后续第 691 行失败）** | 本轮截图：`docs/acceptance/evidence/antigravity/12-model-readonly-sync.png` |
| 13 | `mt` 跨卡片控制（Antigravity 卡内自然语言调用 `mt list` / `mt read` 操作隔壁卡） | 与 Claude 卡片同流程注入提示并读取兄弟卡片输出 | 本轮 `task-3867` 在第 944 行（`openDagWindow` + `bringToFront()` 后输入 `agy`）向终端写入前串入 `\x93` 字节导致 `zsh: command not found: \M-^Sagy` 失败；同包 `ZDOTDIR` 前次运行（`task-3856`，`00:43`）完整通过并准确返回 `MT_PEER_TOKEN_8421` | **未闭合（本轮第 944 行切窗输入串入 `\x93`；前次 `task-3856` 通过）** | 本轮报错首行：`Error: expect(locator).toContainText(expected) failed`（`tests/e2e/antigravity-acceptance.spec.ts:944:57`, `Received string: "... <0093>agyzsh: command not found: \\M-^Sagy..."`）；错误上下文：`test-results/antigravity-acceptance-Ant-a623f-nside-real-Antigravity-card/error-context.md`；前次 `task-3856` 成功截图：`docs/acceptance/evidence/antigravity/13-mt-cross-card.png`、`results/mt-cross-card.json` |
| 14 | 未安装 / 未登录提示（14a 启动失败提示 + 14b 登录询问 → `needs-input` → 按 Enter 恢复空闲） | 与 Claude 同位置横幅与终端 `needs-input` 交互态 | 本轮 `task-3867` 在第 360 行因初始提示符后出现意外字符 `'1'` 中断；同包 `ZDOTDIR` 前次运行（`task-3856`，`00:43`）完整通过并校验 `未检测到 Antigravity` 与受控登录 `needs-input` → `idle` | **未闭合（本轮第 360 行出现提示符串字 `'1'`；前次 `task-3856` 通过）** | 本轮报错首行：`Error: promptRows="icesword@icesworddeMacBook-Pro repo % 1 "`（`tests/e2e/antigravity-acceptance.spec.ts:360:67`）；错误上下文：`test-results/antigravity-acceptance-Ant-c6ddc--prompt-and-restore-failure/error-context.md`；前次 `task-3856` 截图：`14a-missing-binary.png`、`14b-unauthenticated-needs-input.png`、`14b-unauthenticated-dag.png`、`results/missing-binary.json`、`results/unauth-and-restore-failed.json` |
| Perf | **延迟数值（hook 不明显阻塞、`running` / `needs-input` / `idle` 事件到 UI ≤ 1000ms）** | 事件发生后 1 秒内反映到卡片 / DAG | 因 Test 1 在第 691 行（#7）超时，未运行到用例末尾写入 `core-and-latency.json`，因此**本轮无端到端状态跳转毫秒汇总 JSON**；已实测 `ZDOTDIR` 隔离后打包 App Shell 提示符耗时（`windowAndPidMs = 852ms`、`pidToPromptMs = 30ms`、CLI `0.045s` vs 宿主机 `~/.zshrc` `9.840s`） | **未闭合（无 `core-and-latency.json`）** | 无 `core-and-latency.json`；环境探针记录 `pidToPromptMs = 30ms` |

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

- `pnpm typecheck`：在提交 `340b4b6` 前通过（本次仅修改测试与文档，未改产品源码）
- `pnpm check:identifiers`：对 `11-execution-modes-menu.png` 无损重编码并更新矩阵与截图后**通过（exit 0）**
- `@matou/runtime` 单测：`src/session/session-hud-registry.test.ts` + `src/antigravity-runtime.test.ts`（**29/29 passed**，截至 `340b4b6`）
- `@matou/desktop` 单测：`src/renderer/src/hud/TerminalHud.test.tsx` + `src/renderer/src/hud/TerminalHud.locale.test.tsx`（**49/49 passed**，截至 `340b4b6`）
