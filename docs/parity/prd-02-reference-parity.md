# PRD 02 Agent HUD / 终端 HUD：reference product CLI 模块对照矩阵

## 对照边界与产品决策

- 只对照黑色 CLI 模块内、紧贴终端底部的快捷栏与 HUD；白色产品导航、Logo 和窗口外壳不参加验收。
- 需求完整性以 `02-agent-hud-和-终端-hud.md` 为清单；同范围视觉与交互以当前 reference product 源码和可运行程序共同为基线。
- 当前可运行 reference product 仍装载旧版 `src/modules/terminal/components/shortcut-bar/ShortcutBar.vue`：底栏高 50px、左右 12px，只显示快捷命令，没有 HUD。这属于 PRD 要补齐的缺口，不复制为 Matou 的最终状态。
- reference product 仓库中完整 HUD 实现位于 `src/modules/terminal/components/shortcut-bar/shortcut-bar/ShortcutBar.vue`。2026-08-25 产品确认采用方案 A：该实现已有的可见文案、顺序、颜色和交互优先，PRD 补齐尚未落地的运行能力。
- 因此，Matou 的最终顺序采用 reference product：Agent 环境区为“目录 → Git → 时长”，模型折叠态显示 `Opus Plan / Opus / Sonnet`，Bypass 文案保留 `resume` 和 `sessionId`，窄宽阈值逐级采用 560 / 500 / 440 / 380 / 330 / 290 / 250 / 210px。

## 双基线结论

| 用户场景 | 可运行 reference product | reference product 完整 HUD 源码 | Matou 实际结果 | 差异结论 |
|---|---|---|---|---|
| Shell 底部信息 | 旧栏仅有快捷命令，没有 HUD | `Shell → ~/目录 → Git* → ⏱时长` | 同完整 HUD 字段与顺序，数据来自当前 Shell | 补齐缺口 |
| Agent 底部信息 | 旧栏没有 HUD | 权限 → 模型 → 上下文 → 任务 → 次级 AI → 团队角色 → 工具 → 待办 → 环境区 | 同字段、同条件显隐、同顺序 | 一致 |
| 底栏几何 | 旧栏 50px / 左右 12px | 38px / 左右 20px / 间距 8px | 38px / 左右 20px / 间距 8px | 按方案 A 对齐完整 HUD |
| Shell 与 Agent 互斥 | 不适用 | 活跃 AI 时 Agent HUD 整体替换 Shell HUD | 输入 `claude` 后整体切换；AI 结束后回到可用 Shell | 一致 |
| 多面板焦点 | 旧栏没有会话字段 | 只展示当前焦点面板的数据 | 焦点改变即替换为对应 Session HUD，状态互不串扰 | 一致 |
| Shell / 目录 | 不显示 | 自动识别；`cd` 后刷新末级目录名 | zsh / bash 自动识别；OSC 7 与命令结束双路径刷新 | 一致 |
| Git / 脏状态 | 不显示 | 分支名 + `*`，橙红 `#ff6b35`；离开仓库隐藏 | 同文案、颜色与隐藏规则 | 一致 |
| 时长 | 不显示 | `<60s` 为秒、`<1h` 为整分、之后为 `HhMm`；10 秒刷新 | 同格式、同节奏；Shell / Agent 连续计时 | 一致 |
| 权限徽章 | 不显示 | Default 灰、Accept Edits 黄、Plan 蓝、Bypass 橙红 | 同四档、同标签、同颜色、同大写视觉 | 一致 |
| 权限菜单 | 不适用 | 四项；选择 / Esc / 外部点击关闭；与模型菜单互斥 | 同行为 | 一致 |
| 普通权限切换 | 不适用 | Default / Accept Edits / Plan 通过 Shift+Tab 循环，不清屏 | 同一 PTY 内切换，徽章即时更新 | 一致 |
| Bypass 边界 | 不适用 | 二次确认；中断进程并以原 Session 身份 resume；成功清屏 | 同一 Matou Session 重建进程；有身份续接、无身份新开；成功清屏 | 一致 |
| Bypass 文案 | 不适用 | 使用 `Claude`、`resume`、`sessionId` 的当前产品文案 | 标题、正文、取消 / 确认按钮逐字采用 reference product | 按方案 A 一致 |
| Bypass 失败 | 不适用 | 保持旧徽章并提示 `切换失败：...` | RPC 报错；不把瞬时 Shell 回退误报为成功；提示同文案 | 一致并补强 |
| 模型入口 | 不适用 | 三档；菜单全名，折叠态 `Opus Plan / Opus / Sonnet` | 同菜单、同选中态、同折叠名 | 一致 |
| 模型切换 | 不适用 | 写入 `/model STRATEGY`，乐观更新，不清屏、失败静默 | 同命令与乐观更新；运行进程不重启 | 一致 |
| 上下文圆环 | 不适用 | `<70` 绿、`70–84` 黄、`≥85` 红；数字保留真实值 | 同阈值与色值；仅圆环几何限制在 0–100 | 一致 |
| 任务态 | 不适用 | 忙碌“任务中”、需交互“待输入”、Agent 错误“错误”、idle 隐藏 | 同文案与显隐 | 一致 |
| 运行工具 | 不适用 | 隐藏 Bash / Skill，取最近两项，运行图标 `◐` | 同筛选、数量、图标与路径截断 | 一致 |
| 待办 | 不适用 | 当前进行项 + 完成 / 总数；50 字截断；全部完成显示 `All todos complete` | 同规则 | 一致 |
| 次级 AI / 团队角色 | 不适用 | 数量大于 0 才显示；团队态显示角色与状态色 | HUD 数据模型和组件已支持；没有团队数据时整块隐藏 | 一致；团队创建数据由后续团队模块接入 |
| 未展示指标 | 不适用 | 不显示配置计数、工具累计、速度、费用、尺寸、Shell 前台态 | HUD 无残留字段 | 一致 |
| 空数据 | 不适用 | 字段安静消失，不显示占位符 | 同行为 | 一致 |
| 窄窗口 | 不适用 | 单行、隐藏、不换行；按 8 级阈值让位 | 同 `container-query` 阈值、`nowrap` 和 `overflow:hidden` | 一致 |
| 极窄底线 | 不适用 | 当前源码在 440px 后隐藏环境区、210px 后隐藏模型 | Matou 同当前源码，不采用 PRD 旧版“始终保留目录 / 模型”的冲突描述 | 按方案 A 一致 |
| 独立窗口 | 旧栏没有 HUD | 同一组件；轻交互一致，宿主型 Bypass 不做强承诺 | 复用同一 HUD；模型、普通权限、Bypass 均连接同一 Runtime 命令 | 达到并超过 PRD 本期承诺 |

## 当前 reference product 代码基线

- 当前运行入口与旧快捷栏：`src/modules/terminal/components/ClaudeCodeView.vue`、`src/modules/terminal/components/shortcut-bar/ShortcutBar.vue`。
- 完整 HUD 字段、菜单、文案、顺序、时长和让位策略：`src/modules/terminal/components/shortcut-bar/shortcut-bar/ShortcutBar.vue`。
- 任务态文案与团队角色：`src/modules/terminal/utils/panelRuntimeState.mjs`、`teamPanePresentation.mjs`。
- Agent 指标与会话身份：`electron/hook-server.js`、`electron/claude-hook-handler.js`。
- 权限重建、resume 和清屏：完整 HUD `ShortcutBar.vue` 与 `ClaudeCodeTerminal.vue`。

## 双应用运行证据

- reference product 当前黑色 CLI 运行基线：`docs/acceptance/evidence/prd-02/reference/cli-runtime-baseline.png`。
- reference product 当前加载组件的几何采样：`docs/acceptance/evidence/prd-02/reference/runtime-baseline.json`。
- Matou Shell HUD：`docs/acceptance/evidence/prd-02/matou/shell-hud.png` 与 `shell-hud.json`。
- Matou Agent HUD：`docs/acceptance/evidence/prd-02/matou/agent-hud.png` 与 `agent-hud.json`。
- Matou Bypass 确认：`docs/acceptance/evidence/prd-02/matou/bypass-confirmation.png`。

reference product 采样来自当前运行程序并只截取黑色 CLI 模块；登录遮罩仅在采样页面中移除，没有修改 reference product 源码或 CLI 内部样式。运行采样明确证实旧栏尚未装载 HUD，因此 Matou 的新增字段以 PRD 与 reference product 完整 HUD 源码共同闭合。
