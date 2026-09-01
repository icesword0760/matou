# PRD 04 会话持久化与自动恢复验收

状态：已通过产品验收

## 1. 用户可获得的结果

- 再次打开 Matou 时，黑色 CLI 区域内的工作区、事项、页签、分屏、激活位置、分屏比例、面板类型和各自目录自动回到关闭前状态。
- 普通 Shell 只恢复位置和目录，以干净的新 Shell 打开；旧命令、旧输出和关闭前正在运行的前台进程不会重新出现。
- AI 面板以自己的 provider 会话身份续接，同一面板的权限档位与会话身份一起保留；空身份不会擦掉已确认身份，不同面板不会串会话。
- 一个面板、一个 journal 或整份数据库出现异常时，其余工作现场仍可进入；持久层整体不可用时静默进入可工作的干净现场。
- 点击 macOS 主窗口关闭按钮只隐藏窗口；重新显示后仍是同一批终端进程和现场。
- 打包版重复启动时只保留一个 Runtime 和 SQLite 权威写入者；第二次启动负责显示并聚焦原窗口，避免两个应用实例互相覆盖现场。

## 2. 视觉与交互边界

只对照 reference product 截图中的黑色 CLI 模块。黑色区域外的白色产品侧栏、Logo、顶部品牌区和 Matou 自身窗口外壳不参与视觉差异判断。视觉证据均按 CLI 黑色区域裁切。

详细双基线矩阵：`docs/parity/prd-04-reference-parity.md`。

## 3. 27 项验收台账

| # | 用户场景 | 当前用户结果 | 权威证据 | 状态 |
|---:|---|---|---|---|
| 1 | 正常关闭后重启 | 层级、选中项、页签、分屏、比例、面板类型与目录恢复 | `prd-04-session-recovery.spec.ts`、PRD 05 lifecycle E2E | 通过 |
| 2 | Shell 有大量旧输出 | 重启后保持原目录，但画面为干净新 Shell | `prd-04-session-recovery.spec.ts` | 通过 |
| 3 | AI 对话有有效历史 | Matou 使用原 provider 身份和权限参数启动续接，不前置本地历史；恢复后可基于上文继续回答 | 本机 Claude Code 2.1.241 有效身份历史重绘 + provider fixture Electron 追问回放 + hook Runtime 集成测试 | 真实 CLI 历史重绘与应用追问链路通过；最终人工语义追问留给产品验收 |
| 4 | AI 进程启动失败 | 同一面板清除坏身份并进入可操作 Shell，显示 `[上次会话无法续接，已回到普通终端]` | Runtime + Electron 集成测试 | 通过 |
| 5 | AI 运行中判定身份失效 | 仅该面板降级并清除失败身份；明确报错、启动失败、可交互前提前退出或十秒无响应均进入可输入 Shell | 本机 Claude Code 2.1.241 无效身份回放 + resume monitor / Runtime + Electron 集成测试 | 通过 |
| 6 | 第一次失败后再次重启 | 直接以 Shell 打开，不重试旧身份、不重复提示 | Electron second-start fixture | 通过 |
| 7 | 多终端目录不同 | 每个面板回到自己的最后确认目录 | cwd tracker unit + Runtime + Electron E2E | 通过 |
| 8 | 多工作区 / 多事项 | 各现场独立恢复，不串选中项和终端 | PRD 03 / 05 E2E | 通过 |
| 9 | 面板曾脱出窗口 | 重启后回到主窗口原事项，不重开临时窗口 | `prd-05-detached-window.spec.ts` | 通过 |
| 10 | 手动删除层级 | 已删除项目、事项、页签或面板不复活 | Workspace lifecycle + Task / Scene / panel restart E2E | 通过 |
| 11 | 首次启动 | 直接呈现干净默认现场，无恢复提示 | hierarchy bootstrap E2E | 通过 |
| 12 | 有恢复现场 | 不出现恢复 spinner、确认框、toast 或首次介绍 | DOM 断言 + recovery E2E | 通过 |
| 13 | 强制结束应用 | 已提交结构恢复，最近运行标记为中断 | SIGKILL E2E + recovery tests | 通过 |
| 14 | Shell 正在跑前台命令 | 重启后不重跑命令，也不保留尾屏 | SIGKILL E2E | 通过 |
| 15 | 原目录已删除 | 回退到可用项目目录或用户目录，其它面板正常 | Session 子目录删除后重启 Electron E2E + cwd fallback test | 通过 |
| 16 | 单面板数据损坏 | 故障限制在该 Session，其余现场正常恢复；两侧终端均可继续输入 | corrupt journal Electron E2E + malformed metadata isolation tests | 通过 |
| 17 | 数据目录只读 | 使用当次可写兼容副本恢复现场，终端仍可执行命令，原数据不被破坏 | read-only Electron E2E + database bootstrap test | 通过 |
| 18 | 恢复中再次强制结束 | 再次恢复具有幂等性，保留上一次有效结构与身份 | repeated recovery test | 通过 |
| 19 | 历史通知 | 进程重启后不恢复旧通知与角标 | `NotificationProjection` session-memory test | 通过 |
| 20 | 整个数据库损坏 | 原文件隔离，静默创建干净现场并保持可操作 | corrupt database Electron E2E | 通过 |
| 21 | Bypass 模式恢复 | 同一身份以 Bypass 参数续接 | provider launch integration test | 通过 |
| 22 | 高权限恢复后再次切换 | 更新同一有效身份的权限档位，下一次启动按新档位执行 | repository + RPC + Runtime 参数测试 | 持久化链路通过；可见 HUD 入口与确认流程归 PRD 02 |
| 23 | Fork 面板恢复 | provider 身份按 Session 独占，父子面板不会串绑 | identity ownership tests | 持久化隔离通过；完整 Fork 回放归 PRD 06 |
| 24 | 点击主窗口关闭 | 窗口隐藏，重新显示后 PID、关闭前输出和现场不变，并可继续执行新命令；关闭页签不触发隐藏 | `prd-04-session-recovery.spec.ts` | 通过 |
| 25 | 覆盖安装 | 数据根目录独立于安装目录，重新构建的 `.app` 使用同一数据目录恢复结构并继续运行 | packaged same-data-root multi-launch + migration tests | 应用与打包产物链路通过 |
| 26 | 卸载重装且数据仍在 | `.app` 与用户数据分离；打包产物仅依赖仍在的用户数据目录即可恢复 | packaged same-data-root multi-launch | 应用与打包产物链路通过 |
| 27 | 降级后再升级 | 较旧构建遇到更高 schema 时从兼容副本恢复现场且不修改原库；恢复受支持版本后原现场继续可读 | packaged newer-schema roundtrip + compatibility-copy unit test | 通过 |

## 4. 已确认的产品取舍

2026-08-25 确认采用方案 A：发生 PRD 与当前 reference product 冲突时，本项以“面板最终必须可操作”的 PRD 承诺为准。

### 4.1 AI 续接失败提示

- PRD：`[上次会话无法续接，已回到普通终端]`
- 当前 reference product：`[resume 失败，正在全新启动...]`

最终采用 PRD 文案。用户看到的是已经发生的最终结果，而不是仍在进行中的启动描述。

### 4.2 十秒无响应

- 最终采用 PRD 行为：十秒内未完成续接即回到 Shell，优先保证面板可操作。
- 当前 reference product 的“十秒后视为成功”作为已知对照差异保留在矩阵中，不再复制到 Matou。

十秒到期前不会提前降级；到期后清除失效身份、显示一行温和提示，并在同一面板启动可输入 Shell。第二次打开不再重试该身份或重复提示。

## 5. 当前自动化证据

- 类型检查：全 workspace 通过。
- 单元 / 集成：contracts 15、domain 3、desktop 42、runtime 215，共 275 项。
- Electron：27 个场景通过，其中 PRD 04 为 12 个；覆盖结构 / 目录 / 干净 Shell、有效 AI 追问、AI 明确失败、提前退出与十秒无响应降级、窗口隐藏连续性、强退、结构删除、Journal 损坏、整库损坏与只读存储。
- 打包产物：1 个生产 `.app` 场景通过，覆盖 SQLite / node-pty / MessagePort、结构多次启动、分屏、脱出归位、路径失效恢复、主窗口隐藏、第二次启动唤回原窗口、Journal 断尾修复，以及高版本 schema 兼容副本与回切。
- Matou 恢复前后截图：
  - `docs/acceptance/evidence/prd-04/matou/before-restart.png`
  - `docs/acceptance/evidence/prd-04/matou/after-restart-clean-shell.png`
- Matou AI 续接失败后可执行 Shell：
  - `docs/acceptance/evidence/prd-04/matou/invalid-ai-fallback-usable-shell.png`
- 当前 reference product 的同范围 CLI 运行证据：
  - `docs/acceptance/evidence/prd-04/reference/cli-module-current.png`
  - `docs/acceptance/evidence/prd-04/reference/before-restart.png`
  - `docs/acceptance/evidence/prd-04/reference/after-restart-clean-shell.png`
  - `docs/acceptance/evidence/prd-04/reference/runtime-replay.json`
- 本机真实 Claude Code 兼容性：2.1.241 的 `--resume`、`--settings`、`--dangerously-skip-permissions` 均为当前有效参数；Matou 生成的附加 Hook settings 已通过 `claude --settings <file> doctor` 解析。真实无效身份输出中的光标定位控制码已按实际字节流固化为 monitor 回归测试，避免错误文本被拆词后漏判。
  - `docs/acceptance/evidence/prd-04/matou/real-claude-compatibility.json`

PRD 04 的实现、reference product 对照、真实 Claude 兼容性与自动化验收台账均已闭合，等待产品验收。
