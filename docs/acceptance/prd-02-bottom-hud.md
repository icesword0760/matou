# PRD 02 Agent HUD / 终端 HUD 验收

状态：已通过产品验收

## 1. 用户可获得的结果

- 用户查看普通终端时，底部会常驻显示 Shell、当前目录、Git 分支 / 脏状态和面板寿命；`cd`、切分支或产生文件改动后会自动更新，不必再敲 `pwd` / `git status`。
- 用户在 Shell 中运行 `claude` 后，同一面板立即进入 Agent HUD；权限、模型、上下文、任务、工具、待办和环境信息集中在一行内，退出 AI 后原位回到可继续输入的 Shell。
- 用户可以直接在底部切模型或切 Default / Accept Edits / Plan Mode。跨越 Bypass 边界时会先看到明确确认，再中断并重建当前 AI 进程；有可恢复身份时继续原话题，没有身份时明确新开。
- 多个面板各自保有独立 HUD，焦点切换不会串状态；独立窗口复用同一组件和 Runtime 命令。
- 用户点击底部 Git 分支后，会直接进入紧凑分支选择器；可搜索或键盘切换分支，并在同一控制器内创建分支、管理 Worktree、提交和推送。
- 窗口变窄时字段按 reference product 当前优先级逐级让位，始终保持单行，不显示 `--`、`N/A` 等噪音。

## 2. 已确认的产品基线

2026-08-25 产品确认采用方案 A：reference product 已实现的视觉与交互优先，PRD 补齐未落地能力。

1. 可运行 reference product 当前仍是 50px 旧快捷栏且没有 HUD；这是需求缺口。Matou 采用 reference product 仓库内完整 HUD 的 38px 形态，而不是复制“没有 HUD”。
2. Agent 环境区顺序采用 reference product 当前完整实现：目录 → Git → 时长。
3. 折叠模型名采用 `Opus Plan / Opus / Sonnet`，菜单使用完整模型名。
4. Bypass 确认沿用 reference product 的 `Claude / resume / sessionId` 文案。
5. 极窄窗口按 reference product 的八级阈值隐藏，不采用 PRD 旧描述中的永久底线字段。

详细双基线矩阵：`docs/parity/prd-02-reference-parity.md`。

## 3. 30 项 PRD 验收台账

| # | 用户场景 | 当前用户结果 | 权威证据 | 状态 |
|---:|---|---|---|---|
| 1 | 打开 Shell 面板 | 只显示 Shell HUD | PRD 02 Electron 场景 1 | 通过 |
| 2 | Shell 启动 AI | 同一面板整体切到 Agent HUD，无字段混杂 | Electron 场景 2 | 通过 |
| 3 | AI 会话结束 | 立即回到可用 Shell HUD，Agent 字段清空 | Electron 场景 2 + Runtime natural-exit test | 通过 |
| 4 | 切换面板焦点 | 一次 React 提交内替换为焦点 Session HUD | `HierarchyShell.test.tsx` | 通过 |
| 5 | 面板拖出独立窗口 | 独立窗口底部显示同一 HUD 与控件 | `DetachedTerminalApp.test.tsx` | 通过 |
| 6 | 独立窗口归还 | 沿用 PRD 05 同 Session 归还，主窗口 HUD 从共享 Runtime 状态恢复 | PRD 05 detached Electron + HUD projection | 通过 |
| 7 | `cd` 到新目录 | 回到提示符后目录末级名刷新 | Electron 场景 1 + Runtime chained-cd test | 通过 |
| 8 | Git 分支 / 改动变化 | 分支与 `*` 随命令 / Hook 刷新 | Electron 场景 1 | 通过 |
| 9 | 离开 Git 仓库 | Git 字段安静消失 | Electron 场景 1 | 通过 |
| 10 | Git 视觉 | 使用 reference product 橙红 `#ff6b35` | Electron CSS 断言 + `shell-hud.png` | 通过 |
| 11 | 前台态 / 终端尺寸 | HUD 不展示这两类字段 | component field tests | 通过 |
| 12 | 点击权限徽章 | 四档菜单完整出现 | component + Electron 场景 2 | 通过 |
| 13 | 三档之间切换 | Shift+Tab 循环，同一 PID，不清屏 | Runtime live-permission test | 通过 |
| 14 | 选中 Bypass | 有身份 / 无身份显示各自 reference product 确认文案 | component tests + Electron 场景 2 | 通过（方案 A 文案） |
| 15 | Bypass 取消 | 徽章、进程和模式均保持不变 | component test | 通过 |
| 16 | Bypass 确认 | AI 进程更换、带高权限参数、清屏；有身份 resume | Electron 场景 2 + Runtime respawn test | 通过 |
| 17 | 从 Bypass 切回 | 复用同一跨边界确认与重建路径 | component / Runtime 双向逻辑 | 通过 |
| 18 | 点击模型 | 三档菜单及当前选中态出现 | component + Electron 场景 2 | 通过 |
| 19 | 选择模型 | 立即显示新模型，写入 `/model`，PID 不变，无失败弹窗 | component + Runtime live-model test | 通过 |
| 20 | 上下文到 70% | 圆环切为 `#d29922` | 69 / 70 边界测试 | 通过 |
| 21 | 上下文到 85% | 圆环切为 `#f85149` | 84 / 85 / 超 100 测试 | 通过 |
| 22 | 任务状态 | 任务中 / 待输入 / 错误按状态出现，idle 隐藏 | component task-label tests | 通过 |
| 23 | 工具 / 待办 | 有数据才显示；工具取最近两项并过滤 Bash / Skill；待办显示进行项与进度 | HUD component + registry tests | 通过 |
| 24 | 窗口从宽到窄 | 八级优先级逐级隐藏，始终单行 | CSS container queries + Electron 场景 2 | 通过 |
| 25 | 极端窄窗口 | 严格沿用 reference product 当前阈值，而非 PRD 旧底线描述 | parity matrix + CSS | 通过（方案 A） |
| 26 | 字段数据缺失 | 当前字段消失，其他字段继续展示，无占位符 | component Shell / Agent tests | 通过 |
| 27 | 不常驻指标 | 配置计数、累计工具、速度、费用、尺寸和 Shell 前台态均不可见 | component negative assertions | 通过 |
| 28 | 会话指标整体缺失 | 保留权限徽章与默认模型入口 | detached / component minimum tests | 通过 |
| 29 | Bypass 中关闭面板 | 本期按 Runtime 终止 / 重建时序处理，不增加额外产品承诺 | PRD 边界定义 | 通过（P2 无硬承诺） |
| 30 | 多面板混合 | 每个 Session 独立存储 HUD；焦点决定唯一可见 HUD | registry isolation + focus-switch test | 通过 |

### 3.1 Git 控制器 reference product 交互对照矩阵

2026-09-01 产品确认以交互 Mockup 为本项最终基准，Git 控制器保持紧凑、就地完成，不恢复旧的顶栏和三页签结构。

| 场景 | reference product / Mockup 基线 | Matou 实际结果 | 运行证据 | 差异结论 |
|---|---|---|---|---|
| 打开入口 | 点击底部当前分支，直接出现已聚焦的搜索框 | 显示 316px 紧凑浮层，搜索框自动聚焦 | `git-control.png`、PRD 02 Electron Git 场景 | 一致 |
| 查找与切换分支 | 当前分支显示勾选和未提交摘要；方向键选择、Enter 切换 | 支持过滤、鼠标选择、上下键和 Enter；已在其它 Worktree 的分支不可重复检出 | component tests、Electron Git 场景 | 一致，并保留冲突保护 |
| 创建分支 | 底部单行入口进入二级表单，创建后直接检出并收起 | 显示当前基线分支；创建成功后 HUD 更新为新分支 | component tests、Electron Git 场景 | 一致 |
| 查看 Worktree | 列表显示分支、路径、当前/有更改/会话状态 | 可进入当前画布、在 Finder 显示，并对可移除项显示操作；入口数量只统计主工作目录之外的附加 Worktree | component tests、Electron Git 场景 | 一致 |
| 创建与移除 Worktree | 二级表单创建；更多菜单执行 Finder 或移除 | 创建后回到列表；有会话的 Worktree 禁用移除，有本地更改时保留现场并反馈 | component tests、Runtime Git tests | 一致，并明确保护状态 |
| 提交 | 提交信息可留空自动生成；可选择包含未暂存更改 | 显示增删行统计；空信息生成稳定摘要，提交完成后即时反馈 | component tests、Electron Git 场景 | 一致 |
| 提交并推送 / 推送 | 有远端时开放组合操作；无远端时保持禁用 | 组合操作严格先提交再推送；独立推送按仓库状态启用 | component tests、Runtime Git tests | 一致 |
| 返回与关闭 | 二级页 Esc 返回分支列表，再按 Esc 关闭；点浮层外关闭 | 键盘、返回键和浮层外点击均按相同层级处理 | component tests、Electron Git 场景 | 一致 |
| 重复操作与错误 | 操作中锁定入口，结果在浮层底部就地反馈 | 同步互斥避免双击产生并发 Git 写入；错误保留当前页面并提供刷新 | component tests、Electron Git 场景 | 一致 |

### 3.2 模型切换设置视觉统一迭代（2026-09-01）

产品确认以 `docs/prd/mockups/Matou-主界面统一看板与设置.html` 为视觉基准后，模型切换设置已对齐主工作区：

- 点击左下角“设置”后，右侧仍由设置页面完整接管，不恢复页签顶栏，也不在四周保留空白；
- 设置导航和主内容共享主工作区浅灰画布与细分隔线，取消独立的大面积渐变卡片；
- 当前供应商只使用一层浅蓝状态强调；供应商配置合并为单个高密度列表表面，配置之间使用 1px 分隔线；
- 新增、编辑、切换和状态反馈保留原能力，按钮、输入框、弹窗、焦点环与进入动效统一为主界面节奏；
- Electron 真实窗口已校验画布颜色、零外边距、透明内容框、列表圆角和行内无独立卡片圆角，运行截图为 `docs/acceptance/evidence/prd-02/matou/model-switch-settings.png`。

## 4. 当前运行与自动化证据

- reference product 当前运行程序和 reference product 完整 HUD 源码已双重对照；可运行程序仍装载旧栏这一缺口已单独记录，没有被误写成最终标准。
- Matou 已覆盖 Shell 环境刷新、Shell → Agent → Shell、权限菜单、模型切换、Bypass 有身份重建、清屏、窄窗口和独立窗口。
- 完整工作区自动化：Contracts 15 项、Domain 3 项、Desktop 98 项、Runtime 239 项，共 355 项单元 / 集成测试通过。
- 全量 Electron 回归 32 个用户场景通过；最终 HUD 修正后，PRD 02 两个 Electron 场景再次单独通过。
- 类型检查与生产构建通过。
- 运行证据：
  - `docs/acceptance/evidence/prd-02/reference/cli-runtime-baseline.png`
  - `docs/acceptance/evidence/prd-02/reference/runtime-baseline.json`
  - `docs/acceptance/evidence/prd-02/matou/shell-hud.png`
  - `docs/acceptance/evidence/prd-02/matou/shell-hud.json`
  - `docs/acceptance/evidence/prd-02/matou/agent-hud.png`
  - `docs/acceptance/evidence/prd-02/matou/agent-hud.json`
  - `docs/acceptance/evidence/prd-02/matou/bypass-confirmation.png`
  - `docs/acceptance/evidence/prd-02/matou/git-control.png`

## 5. 产品验收建议

重点体验五条路径：

1. **Shell 环境感知**：执行 `cd`、切分支、改文件，确认底部目录 / Git / `*` 自动更新。
2. **进入与退出 AI**：在 Shell 输入 `claude`，确认 HUD 整体切换；结束 AI 后确认原位回到可输入 Shell。
3. **就地控制**：切 Plan Mode、切 Sonnet，再跨到 Bypass，确认普通切换无中断、Bypass 有确认且清屏续话。
4. **多面板与窄窗口**：切换不同面板、拖出独立窗口、缩窄窗口，确认状态不串扰且始终单行。
5. **Git 闭环**：从底部分支入口完成搜索切换、创建分支、Worktree 管理、提交与推送，确认无额外顶栏或页签。

PRD 02 的需求台账与 reference product 双基线已经闭合，等待产品验收。
