# DSH 接入对照矩阵：第二关

日期：2026-11-15。基线：`a06812f`（Codex 期），分支 `feat/multi-agent-dsh`。

本关交付一次性观察插件、零输入身份、状态与通知，以及显式 resume 的身份握手。**仍处于实现阶段，不是 11 项功能或 App 界面签收。** 未打包、发布或推送。

## 实测结果

真实 HOME、项目内临时 Git 仓库、Python `pty.fork()` 驱动 `dsh dsh-tui --patch <run.patch.yml>`；没有使用系统终端 App 或 CUA，只运行本轮新建会话及其中一个已结束的新会话的显式恢复。

| 场景 | 原生事实与 Matou 实际结果 | 结论 |
| --- | --- | --- |
| 零输入新建 | 当前进程的 inject ledger 与顶层 agent 匹配，输入消息前收到真实 ID；最初 `persisted=false`，绑定为 unknown，HUD 不宣称可恢复 | PTY + 真实 hook server 通过 |
| 完成 | `idle → running → idle`；`turn/end.reason.kind=completed` 生成一条标题为 DSH 的完成通知 | 通过 |
| 审批 | 请求 bash 的一次性原生沙箱升级，停在审批界面，未批准；`idle → running → needs-input`，一条 DSH permission 通知 | 通过；未扩大权限 |
| 整轮错误 | 仅在探针本次 patch 指定不存在的模型，收到结构化整轮 error；`idle → running → error`，一条 DSH error 通知，没有 completed | 通过；未更改模型偏好 |
| 显式恢复 | 对本轮已结束的新会话传 `--resume <完整ID>`；同一 ID + ready + persisted 后确认；没有模型输入，也没有回放完成通知 | 通过；不等于 App 重启恢复验收 |
| 前台 `/new` | 空白前台切换产生新 ID；新会话未保存时不回退到旧绑定 | 原生切换、hook 集成和回归通过 |
| 同 cwd 双终端 | 两个真实 Python PTY 共用同一临时 Git cwd，使用各自 run token；两个卡片拿到不同 ID | 通过 |
| 运行会话进入后台 | 用原生空输入左键打开后台管理，再 Ctrl+N 建立新前台；观察至原任务的等待窗口之后；卡片跟随新 ID，后台未产生当前卡通知 | PTY 通过；子代理过滤另由定点测试覆盖 |
| 前台解绑瞬间 | 每次发事件重新核对本进程输入归属，覆盖 100 ms 轮询之间的关闭窗口 | 定点自动验证通过 |
| 重复与旧 run | 同序号重送不重复通知；旧 run、旧 generation、其它会话、后台标记均不修改当前卡 | 定点自动验证通过 |
| 原生通知重复 | 所有成功 PTY 场景 OSC 9 / 99 / 777 通知数量为 0；Matou 只接收观察事件；OSC 7501 原生状态帧不作为通知 | 通过 |

脱敏汇总：[evidence/dsh-stage2/summary.json](evidence/dsh-stage2/summary.json)。原始 PTY 字节、临时 patch、备份和运行索引位于工作树忽略目录 `.cache/dsh-stage2/`，不提交真实会话 ID、凭据或用户会话正文。

最初两次后台探针把 slash 文本混入待提交输入，没有建立不同的前台 ID，因此判为探针失败，不作为后台隔离成功证据。改用原生空输入左键 + Ctrl+N 后重测通过。第一次简单 PTY 探针退出等待还需关闭主 fd；后续所有探针均有界停止并关闭自己的 fd/进程。

## 注入、身份和保护边界

- `dsh --dump-config` 的 dsh-tui 原行配置逐字保留，包括惰性的 `!!js` 表达式及原有 inject 依赖；无法安全表达的配置直接报错，不清空配置。dump 有 15 秒及 1 MiB 上限。
- 在 Matou provider-hooks 运行目录生成权限 0600 的单次 patch；自有 `providers/dsh/bridge.mjs` 随既有 control-assets 拷贝。通过 Cordis 依赖让观察器先于 TUI 创建 agent 就绪，不安装 profile 插件。
- 本次启动只覆盖 `backend=dsh`、`taskNotifications=off`、`upstreamRetry=false`。最后一项避免 TUI 空启动时向持久化 provider retryPolicy 写默认值；其它模型、preset 和用户配置保留。
- 插件不发送 prompt、工具参数/结果或完整日志；通过既有本地 run token hook URL 送出最小事件事实。网络发送不被原生事件监听 await，有界队列、状态合并、超时和周期快照；恢复连接不补弹历史通知。
- 身份由原生 agent + 精确 PID 前台 ledger 确认，不按最新文件或 cwd 猜测。只读文件存在性用于区分已识别与已保存，不承担历史预览或损坏校验。
- 初次 resume 必须匹配 expected ID 并且已保存；不同 ID 拒绝其后续流。原生 ready 之前不以大段终端输出宣称恢复成功。正常前台切换后按新 generation 更新绑定。
- 空白当前会话使用既有 provisional binding；DSH 的恢复查询只认最新绑定，防止 unknown 被跳过后误恢复旧对话。其它 provider 的查询逻辑不变。
- 只有真实用户请求进入等待才发审批/问答通知；audit asked、单个工具失败和终端错误文案不直接构成整轮错误。中断不冒充完成；错误保持到真实新一轮输入。

每次 PTY 启动前备份 35 份相关 profile、偏好及存储文件。33 份受保护文件逐场及最终 SHA256 全部一致。新建场景仅原生 `workspace.json` 和 `last-used.json` 运行索引变化，显式 resume 场景 35 份均一致；没有回滚共享索引，没有修改或升级用户 profile。无需新增运行时 LLM 步骤、依赖、数据库或迁移；真实模型调用仅用于这次探针。

启动器 0.2.2-rc.1，TUI 0.19.0，启动器使用 Node 24.13.0。本机 TUI 目录独立解析的 agent/session 包仍为 0.1.5-rc.2，第一关发现的混合解析链没有修复；本关只承诺已观察到的实测组合，不外推为所有版本兼容。

## reference product 交互对照矩阵

DSH 原生 TUI 是本 provider 的行为基线，Matou 卡片层沿用已批准的 CLI 模块范围。此表保留后续关卡，不能把本关技术闭环当作 UI 1:1 验收。

| # | 能力与原生基线 | 本关 Matou 结果 | 差异 / 后续条件 |
| --- | --- | --- | --- |
| 1 | `dsh dsh-tui` 在指定 cwd 新建 | manifest、新建参数、三种精确 Shell 升级形式已接；真实 PTY 启动通过 | App 菜单、首帧、焦点和滚动未验 |
| 2 | 原生 agent / turn / 审批事件 | 工作、等待、完成、错误和前台隔离通过 | App 可见状态与负载延迟未验 |
| 3 | 原生完成/审批通知 | 本次关闭原生完成通知；Matou DSH 标题、单次事件通过 | macOS 可见通知与点击回卡未验 |
| 4 | 原生模型/上下文/工具/权限 | 未接 HUD 数据，本关仅身份可恢复标记 | 后续关卡；未知数据不填 0 |
| 5 | 原生权限/计划模式 | 未开放 Matou 模式菜单 | 后续验证官方控制与事实回读 |
| 6 | 原生 session/title | 未开启自动标题 | 后续关卡 |
| 7 | 原生会话目录与选择器 | 未开启 Matou 载入入口 | 后续完成有界目录/预览/分页 |
| 8 | 显式 `--resume ID` | 同 ID ready 握手已实测；autoRestore 仍关闭 | App 重启队列、失败重试/新开未验 |
| 9 | 原生 `/fork` 子会话 | 按评审决定，manifest fork=false，不显示卡片一键入口 | 不注入 `/fork`、不改草稿；后续载入原生子会话；上游正式调用口可用后再评估一键入口 |
| 10 | 原生 system-prompt section / shell | 未注入 mt，沿既有环境但无能力成功声明 | 后续验证默认沙箱内真实跨卡 |
| 11 | 账户/供应商/团队等 | DSH manifest 关闭这些入口 | 符合本期范围；App 显隐未验 |

## 自动验证

- `pnpm typecheck` 全工作树通过；最后新增 runtime 定点测试后再执行 runtime typecheck 通过。
- DSH 事件/身份/启动 + 既有 hook/通知/恢复/HUD 相关 8 文件、133 测试通过。
- PTY、仓库身份和恢复相关 5 文件、74 测试通过；包括新增真实 PTY 启动边界的 stub 回归。
- manifest 注册表 5 测试通过，仅更新总注册列表并新增 DSH；没有改写 Claude/Antigravity/Codex 的能力或行为断言。
- runtime-server 身份/恢复/升级等相关子集 50 通过，111 因名称筛选未执行；没有宣称全量 runtime-server 通过。
- 前台即时解绑边界测试通过；观察器/overlay 定点测试通过。
- `pnpm check:identifiers`、`git diff --check` 通过。

未执行：全量测试、打包、真实 Electron 界面、通知中心、11 项完整产品及性能签收。HUD、标题、目录、自动恢复、原生 Fork 子会话载入、模式控制和 mt 属后续关卡，本关停下交回。
