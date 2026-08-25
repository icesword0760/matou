# PRD 04 会话持久化：Kooky CLI 模块对照矩阵

## 对照边界

- 只验收 Kooky 截图中的黑色 CLI 模块：黑色区域内部的工作区 / 事项侧栏、页签、终端与分屏、底部快捷区，以及这些入口触发的菜单、弹窗和状态反馈；视觉证据默认裁切到该区域再比较。
- 黑色区域外的白色产品侧栏、Kooky Logo、顶部品牌区与 Matou 自身窗口外壳不参加视觉差异判断；主窗口关闭 / 再打开属于本 PRD 的应用行为，仍需验收。
- 需求完整性以 `04-会话持久化和自动恢复.md` 为清单；可见样式、操作步骤和反馈时机以当前可运行 Kooky及其现存代码为基线。

## 用户场景矩阵

| 用户场景 | Kooky 当前行为 | Matou 目标 | 验证方式 | 状态 |
|---|---|---|---|---|
| 再次打开应用 | 项目、事项、页签、分屏、激活位置与尺寸直接到位 | 结构、归属、选中项和几何状态全部还原 | Kooky / Matou 重启前后 CLI 裁切截图 + `prd-04-session-recovery.spec.ts` | 双应用运行与截图均已验证 |
| 普通 Shell 重启 | 默认 `claude_only` 恢复策略不回放 Shell 滚动历史 | 同目录启动干净的新 Shell；旧命令、输出和前台进程不出现 | Kooky `terminalRestorePolicy.mjs` + 强退/重启 E2E | 已验证 |
| 窗口隐藏后再打开 | 主窗口隐藏，进程与现场继续存活 | 不重启终端；PID、画面与输入状态保持 | Kooky 当前版本运行回放 JSON + Matou Electron E2E | 双应用均已验证 |
| AI 对话身份记录 | Claude 的 HTTP `SessionStart` 当前不会触发；Kooky 在首个可解析的后续 Hook 事件拿到非空 `sessionUuid` 后更新面板身份与目录，空值不覆盖上一次有效身份 | 每个 Claude 面板获得隔离的本地 Hook 通道；`SessionStart` 仅作前向兼容占位，首个受支持的后续事件拿到非空身份后立即原子落盘，空值不擦除、跨面板不串绑 | Kooky `hook-server.js:342-348`、`claude-code-manager.js:39-48` + `provider-hook-server.test.ts` + `session-repository.test.ts` | 已验证 |
| AI 对话恢复 | 带可用 `claudeSessionId` 的面板以 provider resume 方式启动，不在前面拼接本地历史 | 使用持久化会话身份续接；权限模式随身份恢复；恢复成功后的 Hook 继续刷新同一身份 | provider fixture 的 Electron 追问回放 + Runtime 集成测试 | 已验证 |
| 恢复后再次切权限 | Kooky 保留原 `claudeSessionId`，只更新该面板的 `aiPermissionMode`；跨 Bypass 边界时再用同一身份重启 | Runtime 以 `session.set-permission-mode` 原子更新同一有效身份的权限元数据，不新建或擦除身份；下一次 provider 启动读取更新后的档位 | repository / RPC / Runtime 参数集成测试 | 持久化与再次启动链路已验证；可见入口与确认流程归 PRD 02 |
| AI 续接失败 | 清除失效身份，面板降级到 Shell；Kooky 文案为 `[resume 失败，正在全新启动...]` | 单周期完成清理和降级；采用 PRD 文案 `[上次会话无法续接，已回到普通终端]`；Shell 立即可输入；下次启动不重试、不重复提示 | provider 明确失败 / 启动失败 / 提前正常退出 tests + Electron 双重启动回放 | 产品决策 A 已闭合；文案差异为确认后的例外 |
| AI 续接十秒无响应 | Kooky 十秒内没有明确错误即视为成功 | 十秒内未完成续接即清除失效身份并回到可输入 Shell | Runtime 定时状态机 + 十秒 Electron 真实时钟回放 | 产品决策 A 已闭合；可用性优先于复制 Kooky 的超时策略 |
| 多现场恢复 | 各项目 / 事项 / 页签分别恢复 | 一个面板异常不影响其它现场 | 单 Journal 损坏 Electron 回放 + Runtime 故障注入 | 已验证；损坏面板与正常面板均可继续输入 |
| 目录失效 | 面板使用可用的项目目录或默认目录继续打开 | 用户仍得到可输入的新 Shell；其它面板照常 | Session 子目录删除后重启 E2E + cwd fallback 集成测试 | 已验证 |
| 独立窗口归位 | 恢复时 detached 面板重新挂回原事项 / 页签，不重新弹出独立窗口 | 保留归属，不保留临时窗口形态和位置尺寸 | detached restart E2E | 全量 Electron 回放已验证 |
| 手动删除后重启 | 已删除的项目 / 事项 / 页签 / 面板不再出现 | 不发生“僵尸复活” | Workspace 生命周期 E2E + Task / Scene / 面板联合重启 E2E | 已验证 |
| 通知 | 未读和历史通知不跨进程重启 | 重启后无旧角标 | `NotificationProjection` session-memory 测试 | 已验证 |
| 首次启动与恢复态 | 首次启动走干净默认现场；恢复态不展示恢复提示 | 不出现“是否恢复”“正在恢复”、spinner、toast 或恢复弹窗 | DOM 断言 + 数据库损坏 E2E | 已验证 |
| 主窗口关闭 | macOS 点击关闭隐藏，Dock / 托盘入口可重新显示 | 主窗口关闭不退出 Runtime；再次打开原现场 | Electron E2E + 生命周期 E2E | 已验证 |
| 重复启动应用 | Kooky 打包版持有单实例锁；第二次启动只唤回已有窗口，不再开启第二套持久层写入者 | Matou 打包版保持一个 Runtime / SQLite 权威实例；第二次启动显示并聚焦已有窗口 | 生产 `.app` 双启动回放 + single-instance unit test | 已验证 |
| 崩溃 / 强退 | 已提交结构从持久层恢复，活动进程被标记中断 | 最近结构保留、前台进程不重跑、各面板独立重建 | Runtime recovery + `SIGKILL` E2E | 已验证 |
| 数据仍在的升级 / 重装 | 用户目录仍在即可读取现场 | Runtime 数据目录不依赖安装目录 | 同数据目录重启 + 新版本兼容副本测试 | 已验证 |

## Kooky 代码基线

- Shell 默认不回放：`src/modules/terminal/utils/terminalRestorePolicy.mjs:1-14`、`terminalRestore.mjs:10-39`。
- 层级恢复与 detached 归位：`src/modules/terminal/components/ClaudeCodeView.vue:3195-3305`。
- 会话身份清脏：`electron/claude-session-lookup.js:74-102`；运行期失败降级：`ClaudeCodeTerminal.vue:1461-1548`。
- 会话身份采集：`electron/hook-server.js:79-102,323-356,405-415`、`electron/claude-code-manager.js:39-48`、`src/modules/terminal/components/ClaudeCodeView.vue:3432-3455`；HTTP `SessionStart` 仅为占位，身份由首个受支持的后续事件建立，且只有非空 Hook 身份才覆盖现有记录。
- 权限模式持久字段：`src/modules/terminal/stores/panel.js:183-187,488-502`。
- 主窗口隐藏与再显示：`electron/main.js:1854-1877,2382-2397`。
- 打包版单实例锁与第二次启动聚焦：`electron/main.js:1673-1693`。

## Matou 运行证据

- 关闭前现场：`docs/acceptance/evidence/prd-04/matou/before-restart.png`
- 重启后同结构、干净 Shell：`docs/acceptance/evidence/prd-04/matou/after-restart-clean-shell.png`
- AI 身份失效后降级且 Shell 可实际执行命令：`docs/acceptance/evidence/prd-04/matou/invalid-ai-fallback-usable-shell.png`
- 自动化场景：`tests/e2e/prd-04-session-recovery.spec.ts`（结构与目录、干净 Shell、主窗口隐藏、整库损坏、强退与前台进程不重跑、AI 失败降级后可输入、第二次启动不重试）。
- AI 身份闭环：`apps/runtime/src/session/provider-hook-server.test.ts`、`apps/runtime/src/domain/session-repository.test.ts`、`apps/runtime/src/runtime-server.test.ts`；覆盖独立 Hook 配置、首个受支持后续事件落盘、空身份保护、同身份刷新、旧运行不回写过期权限模式、面板隔离、下次启动参数与权限模式。
- 本机 Claude Code 2.1.241 已确认接受 Matou 使用的 `--settings`、`--resume` 与 Bypass 参数，且生成的 Hook settings 可被 `doctor` 正常解析；用有效真实身份启动后，Claude Code 自行重绘历史并进入可输入状态，附加 HTTP Hook 收到匹配身份的 `SessionEnd`。真实无效身份输出会用水平光标控制码逐词绘制，Matou 已把该实际字节流纳入 monitor 回归测试，最终降级结果与 Kooky 的退出兜底一致。脱敏结果见 `real-claude-compatibility.json`。
- 有效 AI 身份交互回放：provider fixture 收到原身份和 Bypass 参数，重新呈现既有上下文；用户输入 `continue` 后返回与上文一致的答案，证明恢复面板保持可交互。
- 降级后的终端连续性：Runtime 对同一 Session 的重复 attach 串行化，活动 PTY 完成 replay 后重新接回实时输出；单元测试与 Electron 正常输入回放共同覆盖。
- 权限模式连续性：`session-repository.test.ts`、`runtime-rpc-router.test.ts`、`runtime-server.test.ts` 覆盖同一身份更新、非法档位拒绝与更新后按新档位续接。
- PRD 04 Electron 回归现为 12 个场景，新增有效 AI 追问、目录删除回退、单 Journal 损坏隔离、结构删除不复活、只读存储可用性、provider 提前正常退出及十秒无响应降级；全量 Electron 场景现为 27 个。
- 对照截图复核时发现 xterm viewport 默认留下黑色底带；已按 Kooky 的 `background: transparent` 与 `overflow-y: auto` 修正，并以计算样式断言锁定，最新三张 Matou 证据均已重新采集。
- 生产 `.app` 也完成多次同数据目录启动、结构恢复、窗口隐藏、第二次启动唤回原窗口、Journal 断尾修复和更高 schema 兼容副本回切；打包测试中两处已过时的旧入口断言同步更新为当前 Kooky 对齐交互：拖出靠面板标题拖拽，最后页签受保护，只有关闭主窗口才隐藏。

## Kooky 当前版本运行证据

- 当前 CLI 模块基线：`docs/acceptance/evidence/prd-04/kooky/cli-module-current.png`
- 关闭前在右侧 Shell 真实执行标记命令与 `pwd`：`docs/acceptance/evidence/prd-04/kooky/before-restart.png`
- 结束进程并重新启动后，工作区、事项、页签与左右分屏保持，两个 Shell 均以原目录的干净提示符启动，旧标记和输出不回放：`docs/acceptance/evidence/prd-04/kooky/after-restart-clean-shell.png`
- 点击主窗口关闭后，窗口 `visible=false`、`destroyed=false`；重新显示后，关闭前输出 `975` 仍在，并可继续执行命令得到 `333`：`docs/acceptance/evidence/prd-04/kooky/runtime-replay.json`。
- 上述运行基于当前 Kooky 源码与当前可恢复 CLI 现场；Electron 外层登录引导只在 QA 采证时移除。截图只保留黑色 CLI 模块，登录引导和白色产品导航不参加对照。

## 已确认的 Kooky / PRD 差异

2026-08-25 产品确认采用方案 A：

1. **降级提示文案**采用 `[上次会话无法续接，已回到普通终端]`，准确描述用户当前已获得的状态。
2. **十秒无响应**按续接失败处理并回到 Shell，保证面板最终可操作。

这两项是经产品确认的 Kooky 例外，其余同范围界面与交互继续按当前 Kooky 逐项对齐。失败状态机、真实时钟回放、一次清脏和第二次启动均已验证。

PRD 04 对照矩阵已闭合，等待产品验收。
