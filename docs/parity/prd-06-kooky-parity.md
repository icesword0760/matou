# PRD 06 会话 Fork：Kooky CLI 模块对照矩阵

## 对照边界与产品决策

- 只对照黑色 CLI 模块内的面板标题、右键菜单、分屏终端、错误输出与独立窗口；外部产品侧栏、Logo 和窗口外壳不参加验收。
- 当前可运行 Kooky 的 `terminalContextMenu.mjs` 第 15–27 行临时直接返回空数组，因此运行程序看不到 Fork / Detach 菜单。
- 同一文件的 dormant 分支已经写出两项菜单；`forkSession.js` 写出完整资格判断；`ClaudeCodeView.vue` 写出右侧建面板、传递源身份并聚焦；`claude-code-launch.js` 写出 `--resume SOURCE --fork-session`；`ClaudeCodeTerminal.vue` 写出失败终止和警示行。
- 产品已确认激活上述 Kooky 源码基线。当前隐藏状态作为“尚未开放”的运行证据保留，不作为最终交互目标。
- 项目后续需要会话父子 / DAG；Matou 内部额外保存 `forked-from`，当前界面无视觉标记。

## 双基线对照

| 用户场景 | 可运行 Kooky | Kooky dormant 源码 | Matou 实际结果 | 差异结论 |
|---|---|---|---|---|
| 主窗口可恢复 Claude 内容区右键 | 菜单临时隐藏；终端内容区现有菜单只含复制/粘贴 | dormant 面板菜单含 `⑂ Fork 会话`、`↗ 独立窗口`；PRD 要求面板可操作区域右键 | 终端内容区直接显示两项，顺序一致 | 按已确认 PRD 扩大命中区域，修复标题条入口不易发现的问题 |
| Shell / Claude 类型表达 | Shell 面板标题来自 Shell；Claude 面板 fallback 使用 `claude ~/cwd` | `normalizePanelTitle()` 根据 `claudeActive/mode` 区分 | 面板进入 Claude 时标题为 `Claude`，退出时为 `Shell` | 类型语义一致；Matou 当前短标题更明确 |
| Shell 内通过 `cc` 启动 Claude | Claude 状态上报后将 Shell 面板提升为 Claude | `updateAiStatus()` 以新 Claude 身份提升 `mode/claudeActive` | 仅当当前 Shell 将 `cc` 配置为 Claude 别名时接管启动，保留 bypass 权限并切换标题 | 一致；普通系统 `cc` 仍作为编译器命令执行 |
| Shell 右键 | 菜单隐藏 | `canForkSession` 为 false | Fork 隐藏 | 一致 |
| 首轮对话尚未形成 | 菜单隐藏 | 缺少可恢复的 `claudeSessionId` 时隐藏 | 状态栏临时 ID 只更新展示；首个对话 Hook 确认前 HUD `resumable=false` | 一致，并避免提前开放后 Fork 到空会话 |
| 团队队友 | 菜单隐藏 | `teamId/teamRole` 存在时隐藏 | `agent-team-member` 隐藏 | 一致 |
| 独立窗口 | 菜单隐藏 | detached panel 排除 | Detached App 无 Fork 菜单 | 一致 |
| 菜单容器 | 没有可采样菜单 | fixed、z 9999、min 140、padding 4×0、radius 6、blur 12 | 逐项相同 | 一致 |
| 菜单项 | 没有可采样菜单 | padding 6×14、13px、系统字体 | 逐项相同 | 一致 |
| 点击后的布局 | 尚未开放 | `addSplitPanel('right', 'claude-code', {cwd}, leafId)` | 源节点局部横向一分为二，新面板 ordinal 1 | 一致 |
| 工作目录 | 尚未开放 | 新面板直接复制 `cwd` | 权威 Session 显式复制源 cwd | 一致 |
| 自动聚焦 | 尚未开放 | 建立后 `setActivePanel(newPanel.id)` | 导航焦点切到新 Session，xterm textarea 聚焦 | 一致 |
| Fork 启动参数 | 尚未开放 | `--resume SOURCE --fork-session` | 参数与顺序完全相同 | 一致 |
| 派生身份 | 尚未开放 | Claude Code 负责生成新 session ID | Hook 将新 ID 绑定到派生 Session | 一致并闭合恢复 |
| 源 / 派生隔离 | 尚未开放 | 两个 panel / process 独立 | 独立 PTY、Journal、provider binding | 一致 |
| 连续 Fork | 尚未开放 | 派生面板形成身份后可再次触发 | 每次创建新面板、意图与身份 | 一致 |
| 失败文案 | 尚未开放 | `[Fork 未完成，请检查上方原因后重试]` | 逐字一致，保留供应方原始错误 | 一致 |
| 失败行为 | 尚未开放 | 锁输入、销毁当前进程 | 终止派生进程、面板留存、输入无副作用、无 Shell 回落 | 一致 |
| 重启恢复 | 尚未开放 | Fork 标志只用于首次 launch | 成功结算意图；重启只按派生 ID resume | 一致并持久化闭环 |
| 关系可视化 | 无 | 无 | 无徽章、连线、树 | 一致 |
| 内部关系事实 | 无 | 无 | 保存 `forked-from` 供未来 DAG | 已确认的演进增量，不改变本期界面 |

## Kooky 源码行为锚点

- 菜单临时隐藏与 dormant 两项：`src/modules/terminal/utils/terminalContextMenu.mjs:15-27`
- Fork 资格：`src/modules/terminal/utils/forkSession.js`
- 右侧面板、cwd、源 identity、自动聚焦：`src/modules/terminal/components/ClaudeCodeView.vue:2314-2336`
- Portal 与右键菜单 DOM：`src/modules/terminal/components/ClaudeCodeView.vue:249-280`
- 菜单几何与 item 样式：`src/modules/terminal/components/ClaudeCodeView.vue:4503-4545`
- 启动参数：`electron/claude-code-launch.js:6-13`
- 失败终止与警示：`src/modules/terminal/components/ClaudeCodeTerminal.vue:1464-1479`

## 双应用运行证据

- Kooky 当前运行程序右键后菜单数量为 0：`docs/acceptance/evidence/prd-06/kooky/runnable-menu-hidden.png`、`fork-source-baseline.json`。
- Matou 激活后的两项菜单和计算样式：`docs/acceptance/evidence/prd-06/matou/fork-menu.png`、`fork-menu.json`。
- Matou 连续 Fork 后的三个并列会话：`docs/acceptance/evidence/prd-06/matou/forked-conversations.png`。
- Matou 失败面板：`docs/acceptance/evidence/prd-06/matou/fork-failure.png`。
- Matou 使用真实 Claude Code、系统级鼠标与键盘，在终端内容区完成右键、Fork、双侧隔离输入、重启恢复与重启后继续输入：`docs/acceptance/evidence/prd-06/matou/real-system-fork-menu.png`、`real-system-fork-validation.png`、`real-system-fork-validation.json`。

运行截图均只截取黑色 CLI 模块。Kooky 的当前空菜单与 dormant 源码分开记录，避免把“当前尚未开放”误写成最终产品行为。
