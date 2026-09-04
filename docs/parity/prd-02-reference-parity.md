# PRD 02 Agent HUD / 终端 HUD：reference product CLI 模块对照矩阵

## 对照边界与产品决策

- 只对照黑色 CLI 模块内、紧贴终端底部的快捷栏与 HUD；白色产品导航、Logo 和窗口外壳不参加验收。
- 需求完整性以 `02-agent-hud-和-终端-hud.md` 为清单；同范围视觉与交互以当前 reference product 源码和可运行程序共同为基线。
- 当前可运行 reference product 仍装载旧版 `src/modules/terminal/components/shortcut-bar/ShortcutBar.vue`：底栏高 50px、左右 12px，只显示快捷命令，没有 HUD。这属于 PRD 要补齐的缺口，不复制为 Matou 的最终状态。
- reference product 仓库中完整 HUD 实现位于 `src/modules/terminal/components/shortcut-bar/shortcut-bar/ShortcutBar.vue`。2026-08-25 产品确认采用方案 A：该实现已有的可见文案、顺序、颜色和交互优先，PRD 补齐尚未落地的运行能力。
- Matou 延续 reference product 的单行 HUD 与窄宽阈值；2026-09-04 产品进一步确认将操作与统计固定在左侧、目录与 Git 放入右侧让位区，并隐藏普通任务态、`Local` 与运行时长。

## 双基线结论

| 用户场景 | 可运行 reference product | reference product 完整 HUD 源码 | Matou 实际结果 | 差异结论 |
|---|---|---|---|---|
| Shell 底部信息 | 旧栏仅有快捷命令，没有 HUD | `Shell → ~/目录 → Git* → ⏱时长` | 显示 Shell、目录与 Git；运行时长隐藏 | 按产品确认精简 |
| Agent 底部信息 | 旧栏没有 HUD | 权限 → 模型 → 上下文 → 任务 → 次级 AI → 团队角色 → 工具 → 待办 → 环境区 | `ClaudeMd` 固定最左；权限、模型、上下文、MCP、Tools、Agents 为左侧固定区，目录与 Git 为右侧让位区 | 按产品确认优化信息密度 |
| 底栏几何 | 旧栏 50px / 左右 12px | 38px / 左右 20px / 间距 8px | 38px / 左右 20px / 间距 8px | 按方案 A 对齐完整 HUD |
| Shell 与 Agent 互斥 | 不适用 | 活跃 AI 时 Agent HUD 整体替换 Shell HUD | 输入 `claude` 后整体切换；AI 结束后回到可用 Shell | 一致 |
| 多面板焦点 | 旧栏没有会话字段 | 只展示当前焦点面板的数据 | 焦点改变即替换为对应 Session HUD，状态互不串扰 | 一致 |
| Shell / 目录 | 不显示 | 自动识别；`cd` 后刷新末级目录名 | zsh / bash 自动识别；OSC 7 与命令结束双路径刷新 | 一致 |
| Git / 脏状态 | 不显示 | 分支名 + `*`，橙红 `#ff6b35`；离开仓库隐藏 | 同文案、颜色与隐藏规则 | 一致 |
| 时长 | 不显示 | `<60s` 为秒、`<1h` 为整分、之后为 `HhMm`；10 秒刷新 | 不显示 | 按产品确认精简 |
| 权限徽章 | 不显示 | 展示当前权限并可切换 | 扩展支持 Auto 状态显示；点击徽章可切换权限 | 按本轮产品定稿调整 |
| 权限交互 | 不适用 | 可从底栏进入菜单 | 从底栏权限徽章进入菜单并切换；高权限变更前保留确认 | 按本轮产品定稿调整 |
| 模型字段 | 不适用 | 展示当前模型并可从底栏切换 | 只读展示当前模型和上下文容量；模型切换由 AI 终端自身承载 | 按本轮产品定稿调整 |
| 上下文圆环 | 不适用 | `<70` 绿、`70–84` 黄、`≥85` 红；数字保留真实值 | 同阈值与色值；仅圆环几何限制在 0–100 | 一致 |
| 任务态 | 不适用 | 忙碌“任务中”、需交互“待输入”、Agent 错误“错误”、idle 隐藏 | 隐藏普通“任务中”，保留“待输入”和“错误” | 按产品确认降低噪音 |
| MCP / 工具 / Agent | 不适用 | 数量与运行状态 | 常驻显示数量；悬停或键盘聚焦显示对应名称、次数、目标与运行态明细 | 按产品确认增强可读性 |
| ClaudeMd | 不适用 | 不适用 | 最左侧入口打开当前项目 `CLAUDE.md`，支持编辑和保存 | Matou 产品扩展 |
| 待办 | 不适用 | 当前进行项 + 完成 / 总数；50 字截断；全部完成显示 `All todos complete` | 同规则 | 一致 |
| 次级 AI / 团队角色 | 不适用 | 数量大于 0 才显示；团队态显示角色与状态色 | HUD 数据模型和组件已支持；没有团队数据时整块隐藏 | 一致；团队创建数据由后续团队模块接入 |
| 会话详情指标 | 不适用 | 已具备配置计数和工具累计的数据采集结构，但当前版本隐藏 | 状态流提供上下文、可信用量和配置；会话记录增量回填名称、工具累计、最近工具、待办和 MCP 异常 | 产品确认后的有意扩展 |
| 空数据 | 不适用 | 字段安静消失，不显示占位符 | 同行为 | 一致 |
| 窄窗口 | 不适用 | 单行、隐藏、不换行；按 8 级阈值让位 | 同 `container-query` 阈值、`nowrap` 和 `overflow:hidden`；先使用 Matou 底栏剩余宽度，避免新增指标遮挡权限和模型入口 | 交互一致，宽度适配当前布局 |
| 极窄底线 | 不适用 | 当前源码在 440px 后隐藏环境区、210px 后隐藏模型 | Matou 同当前源码，不采用 PRD 旧版“始终保留目录 / 模型”的冲突描述 | 按方案 A 一致 |
| 独立窗口 | 旧栏没有 HUD | 同一组件 | 复用同一 HUD；权限切换和状态更新路径与主窗口相同 | 一致 |

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
- Matou ClaudeMd 编辑器：`docs/acceptance/evidence/prd-02/matou/claude-md-editor.png`。

reference product 采样来自当前运行程序并只截取黑色 CLI 模块；登录遮罩仅在采样页面中移除，没有修改 reference product 源码或 CLI 内部样式。运行采样明确证实旧栏尚未装载 HUD，因此 Matou 的新增字段以 PRD 与 reference product 完整 HUD 源码共同闭合。
