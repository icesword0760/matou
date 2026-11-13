# Matou Antigravity CLI 接入交互对照矩阵

- **验收日期：** 2026-11-13
- **验收分支：** `feat/multi-agent-antigravity`
- **打包验收对象：** `apps/desktop/release/mac-arm64/码头.app`（内置 Runtime 与内置 `mt`）
- **真机基准环境：** macOS arm64，本机真实 `agy 1.5.8`（保留真实 `HOME` 使用 `~/.gemini` 登录态；Matou 数据与测试仓库隔离于 scratchpad 及 `/tmp/m-e2e-*` 符号链接）
- **最终运行结论（收口模式第 3 步单次全量执行）：** `tests/e2e/antigravity-acceptance.spec.ts` 共 4 个测试：**2 passed / 2 failed**（日志：[antigravity-acceptance-step3.log](file:///Users/icesword/.claude/agent-foreman/runs/matou-%E5%A4%9AAgent-agy-01/logs/antigravity-acceptance-step3.log)）。另有 PRD 第 14 项及 9b 使用受控 stub / 缺失命令 fixture。四个 Claude Code E2E 回归（`prd-01`、`session-load-existing-claude`、`prd-06`、`prd-02`）本轮未跑。

## 对照边界与说明

1. **基线约定：** reference product 不含 Antigravity；全部能力以 PRD 第 3 节为规格，以 Matou 现有同范围 Claude Code 卡片行为为 1:1 交互基线（通知英文正文与 `main` 上既有 Claude 通知一致，本次不改）。
2. **证据口径：**
   - **已闭合：** 在当前代码对应的打包产物第 3 步全量运行中直接跑通并生成当前轮截图 / JSON。
   - **未闭合：** 在第 3 步单次全量运行中失败或被前置失败阻断未跑到；即使此前同代码同产物上跑通并已生成截图（已在下表如实区分“前序同构建截图”与“第 3 步失败/未跑到原因”），在未复跑全绿前均按未闭合登记。
3. **受控 Fixture 披露：** 第 9b 项（恢复失败）与第 14 项（`agy` 未安装 / 未登录提示）因本机已安装且已登录真实 `agy 1.5.8`，在 E2E 第 4 个测试中使用 `MATOU_ANTIGRAVITY_COMMAND` 指向不存在路径与受控 Python stub（`agy-stub.py`）触发，不代表真实 CLI 二进制异常的线上输出。

## 打包产物指纹（SHA-256）

| 产物组件 | SHA-256 |
|---|---|
| `apps/desktop/release/mac-arm64/码头.app/Contents/MacOS/码头` | `c8464857aca8ed932c2d18bb973bcad0e494cd0cc7f3652a90e94da95243a2d8` |
| `apps/desktop/release/mac-arm64/码头.app/Contents/Resources/app.asar` | `926964413c6bd57bd448f584a71b7b3ff57da350e1e792a2c95d83db642c470a` |
| `apps/desktop/release/mac-arm64/码头.app/Contents/Resources/runtime/index.cjs` | `9235d8f7f1351030e9fc0756df8e65d41610a8f69243ebee28486bd625239dad` |
| `apps/desktop/release/mac-arm64/码头.app/Contents/Resources/runtime/mt-cli.cjs` | `5dc1252919881527360dcc7c1cbb1587c78c437409b91601a94b1c6b892840e3` |
| `apps/desktop/release/mac-arm64/码头.app/Contents/Resources/runtime/control-assets/bin/mt` | `859676985a2b233e8d6dc7744e7760c3e955b59c6a56932bfcdff3f12a87bc03` |
| `apps/desktop/release/mac-arm64/码头.app/Contents/Resources/runtime/control-assets/providers/antigravity-system-message.md` | `c03b81fbc31e41d41bcf73cc2f715c6225e713fdf9058422a161e6bb4c9d62f6` |

## 逐项对照矩阵（PRD #1-#14 + Fork 并发 + 延迟）

| # | 验收项 | 基线（Claude 卡片 / PRD） | Matou 实际表现（第 3 步单次运行结果） | 状态 | 证据 / 报错首行与截图 |
|---|---|---|---|---|---|
| 1 | 新建卡片（Shell 输入 `agy` 自动升级、新建下拉不变、`mt` 可建 Antigravity） | 与 Claude 相同：Shell 输入触发命令自动升级；`mt create session --profile` 可指定 | 第 2、3 个测试在 Shell 输入 `agy` 均成功升级为 `Antigravity` 卡片；第 4 个测试通过 `mt create session --profile antigravity` 成功建卡。第 1 个测试在本次因 zsh 先吐出 `1%` 提示符竞态导致键入 `1agy` 失败 | **部分验证 / 已在 Test 2、3、4 跑通**（Test 1 失败见下） | 已有截图：`docs/acceptance/evidence/antigravity/01-promote-shell.png`、`docs/acceptance/evidence/antigravity/14a-missing-binary.png`；Test 1 失败首行：`Error: expect(locator).toContainText(expected) failed`（`Received string: "1% ... 1agyzsh: command not found: 1agy"`） |
| 2 | 标签与卡片标识（标签、DAG 节点、父会话投影、子会话角标显示 `Antigravity`） | 与 Claude 卡片同构展示 Provider 标识与层级关系 | Test 2（第 3 步）跑通：DAG 节点明确显示 `Antigravity`，Fork 后父会话角标正确计为 `1`，父会话投影条正常回显 | **已闭合**（Test 2 通过段已验证） | `docs/acceptance/evidence/antigravity/02-identifiers-dag.png`、`docs/acceptance/evidence/antigravity/10a-fork-branches.png` |
| 3 | 首次进入目录（信任提示与 `needs-input` 状态） | 与 Claude 同一工作区信任提示交互与 `needs-input` 表现 | Test 2、Test 3（第 3 步）均出现真实 `agy` `Do you trust the contents of this project?` 提示，DAG 同步进入 `需要输入`，按 Enter 后通过；Test 1 本次因 `1agy` 未到达此步 | **已闭合**（Test 2、3 实证） | `docs/acceptance/evidence/antigravity/03-first-dir-trust.png`、`docs/acceptance/evidence/antigravity/03-first-dir-trust-dag.png` |
| 4 | 工作状态（启动中 → 空闲 → 工作中 → 需要输入 / 完成 / 出错） | 与 Claude 卡片同一五态流转与实时反馈 | Test 2、Test 3（第 3 步）在真实对话与工具调用中均按 `运行中`/`空闲`/`等待输入` 正常流转；Test 1 本次在启动前被 `1agy` 阻断，同构建前序运行有完整 `running` / `idle` DAG 截图 | **未闭合**（Test 1 在第 3 步未跑到断言段） | 前序同构建截图：`docs/acceptance/evidence/antigravity/04-work-status-running-dag.png`、`docs/acceptance/evidence/antigravity/04-work-status-idle-dag.png` |
| 5 | 通知（非聚焦完成 / 等待审批进通知中心，无双份系统通知） | 与 Claude 通知中心与铃铛徽标规则一致（英文通知正文属 `main` 既有行为） | 第 3 步 Test 1 未跑到；前序同构建运行已截得清晰稳定态通知面板（无动画叠影），但未在本步单次复跑中跑通 | **未闭合**（Test 1 未跑到） | 前序同构建截图：`docs/acceptance/evidence/antigravity/05-notifications.png` |
| 6 | HUD（模型与档位、上下文%、按当前模型组显示额度与悬停明细、工具、规则文件、Git 脏状态、隐藏待办与 Claude 账户） | 隐藏不适用的 Claude 账户/待办，状态栏只显示当前模型组 `5h`/`Weekly`，悬停展示全部分组额度 | 第 3 步 Test 1 未跑到；Test 2、Test 3 已实时呈现 `AGENTS.md`、`D`、`Gemini 4 Carbon (Medium) (1M context)`、上下文与按当前模型过滤的 `5h` / `Weekly`；单测与前序同构建截图已覆盖悬停明细与脏星号 | **未闭合**（Test 1 悬停提示与脏状态断言本轮未跑到） | 前序同构建截图：`docs/acceptance/evidence/antigravity/06-hud.png`、`docs/acceptance/evidence/antigravity/06-hud-tooltip.png` |
| 7 | 自动标题（Stop 后读取标题、手动改名锁保护、恢复自动标题） | 与 Claude 一致保护手工标题并支持「恢复 Antigravity 自动标题」 | 第 3 步 Test 1 未跑到（`1agy` 阻断）；Test 2 可见首轮回复后卡片标题已自动更新为 `Remember Codeword COBALT_719`；手工锁与恢复入口仅在第 4 关单测通过，打包 E2E 未跑到 | **未闭合**（Test 1 未跑到） | Test 2 侧栏/父投影可见自动标题：`docs/acceptance/evidence/antigravity/02-identifiers-dag.png`、`docs/acceptance/evidence/antigravity/10a-fork-branches.png` |
| 8 | 载入已有会话（`SessionLoaderDialog` 切换 Antigravity、列表/步骤/预览/载入接回） | 与 Claude 载入对话框同交互，文案按 Agent 显示 | Test 2 在第 760 行因 `conversations/<id>/transcript.jsonl` 路径错误中断，尚未执行到 `SessionLoaderDialog`（第 817 行） | **未闭合**（Test 2 前置失败未跑到） | 阻断报错见第 10 项并发栏 |
| 9a | 重启恢复（真实 App 重启后 `--conversation` 接回原对话） | App 重启后无缝接回，保留绑定与上下文 | 第 3 步 Test 1 未跑到，未能在打包 App 真机闭环验证 | **未闭合**（Test 1 未跑到） | 无新证据 |
| 9b | 恢复失败提示与操作（重试恢复 / 新开 Antigravity） | 与 Claude 相同的恢复失败横幅与按钮 | **已通过**（第 4 个测试使用受控 `agy-stub.py` 对 `--conversation` 返回退出码 1） | **已闭合（受控 fixture）** | `docs/acceptance/evidence/antigravity/09b-restore-failed.png`、`docs/acceptance/evidence/antigravity/results/unauth-and-restore-failed.json` |
| 10 | Fork（完成 ≥1 轮启用、当前目录与新 Worktree 分支、继承上下文且各自独立 ID） | 入口门禁与 Claude 一致；子卡继承上下文并获得独立会话 ID | Test 2（第 3 步）跑通：完成 1 轮后解锁 Fork；当前目录分支正确回答暗号 `COBALT_719` 并获得新 ID；新建独立 Worktree 分支自动越过信任屏完成 `/fork` 并获得第三枚独立 ID | **基本已闭合（单/新工作树均通；并发验证未跑到）** | `docs/acceptance/evidence/antigravity/02-identifiers-dag.png`、`docs/acceptance/evidence/antigravity/10a-fork-branches.png` |
| 10b | **Fork 并发验证（父运行中 Fork 不覆盖绑定、不破坏父对话 JSONL/SQLite）** | 父活跃运行期间触发子 `/fork` 后父绑定与历史完好 | Test 2 在发起父运行前因脚本读错父 `transcript.jsonl` 路径报错退出（真实路径为 `brain/<id>/.system_generated/logs/transcript.jsonl`，脚本写成了 `conversations/<id>/transcript.jsonl`），未跑通 | **未闭合**（测试脚本路径错误导致失败） | 失败首行：`Error: ENOENT: no such file or directory, open '/Users/icesword/.gemini/antigravity-cli/conversations/b4ad8e91-1ba5-43de-bf7b-6796f4fba2db/transcript.jsonl'`；错误上下文：`test-results/antigravity-acceptance-Ant-ce073-ktree-Running-SessionLoader/error-context.md` |
| 11 | 执行模式（4 项菜单、前 3 项发 `Shift+Tab` 不重启、进出 `Bypass Permissions` 确认并重启、终端快捷键同步 HUD） | 4 项与 Claude 对齐；前 3 项不换 PID；进出 Bypass 确认并重启 | 第 3 步 Test 1 未跑到；前序同构建运行已截到 4 项菜单且点击 `Accept Edits` / `Plan Mode` 后 `PID` 保持不变，但在脚本用 `textarea.press('Shift+Tab')` 同步校验第 582 行卡在 `"PM"` 超时（且 Ask 模式下 `cycle_mode` 缺省时 `HUD` 权限徽标保持前态尚未映射回 `D`），Bypass 进出未跑到 | **未闭合** | 前序同构建截图：`docs/acceptance/evidence/antigravity/11-execution-modes-menu.png` |
| 12 | 模型（HUD 只读无模型菜单，终端内 `/model` 切换同步 HUD） | 与 Claude 一致不提供 HUD 模型菜单，随状态栏同步 | 第 3 步 Test 1 未跑到；前序运行也未走到 `/model` 段 | **未闭合**（Test 1 未跑到） | 无新证据 |
| 13 | `mt` 跨卡片控制（Antigravity 卡内自然语言调用 `mt list` / `mt read` 操作隔壁卡） | 与 Claude 卡片同流程注入提示并读取兄弟卡片输出 | **已通过**（Test 3 在第 3 步完整跑通）：真实 `agy 1.5.8` 执行 `mt identify / list / read` 准确读回兄弟 Shell 卡片的 `MT_PEER_TOKEN_8421` | **已闭合** | `docs/acceptance/evidence/antigravity/13-mt-cross-card.png`、`docs/acceptance/evidence/antigravity/results/mt-cross-card.json` |
| 14 | 未安装 / 未登录提示（14a 启动失败提示 + 14b 登录询问 → `needs-input` → 按 Enter 恢复空闲） | 与 Claude 同位置横幅与终端 `needs-input` 交互态 | **已通过**（Test 4 在第 3 步完整跑通，受控 fixture：14a 用不存在命令路径触发中文安装提示与「重试启动 / 移除失败会话」；14b 用 `agy-stub.py` 输出登录提示触发 `等待输入`） | **已闭合（受控 fixture）** | `docs/acceptance/evidence/antigravity/14a-missing-binary.png`、`docs/acceptance/evidence/antigravity/14b-unauthenticated-needs-input.png`、`docs/acceptance/evidence/antigravity/14b-unauthenticated-dag.png`、`docs/acceptance/evidence/antigravity/results/missing-binary.json` |
| Perf | **延迟数值（hook 不明显阻塞、`running` / `needs-input` / `idle` 事件到 UI ≤ 1000ms）** | 事件发生后 1 秒内反映到卡片 / DAG | 第 3 步 Test 1 未跑到，未生成 `core-and-latency.json`（仅第 3 关独立 CLI 实测确认 POSIX `sh` + 后台 `curl` 无可感知延迟，本轮未产出打包 App 端到端毫秒数） | **未闭合（无本轮端到端毫秒统计）** | 无 `core-and-latency.json` |

## 回归与门禁现状（截至本次收口提交）

- `pnpm typecheck`：通过
- `pnpm check:identifiers`：通过
- 受影响单元/组件测试：`@matou/runtime` 3 个文件 45 条通过、`@matou/desktop` 4 个文件 81 条通过
- Claude Code E2E 回归（`prd-01-agent-notifications.spec.ts`、`session-load-existing-claude.spec.ts`、`prd-06-session-fork.spec.ts`、`prd-02-bottom-hud.spec.ts`）：**本轮收口未跑**
