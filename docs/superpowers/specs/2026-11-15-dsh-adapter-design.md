# Matou DSH 接入：第一关调研与实现方案

日期：2026-11-15。状态：第一关方案已获评审通过；第二关身份、状态与通知实现及实测见 [DSH 对照矩阵](../../acceptance/dsh-reference-matrix.md)。

评审决定：本期关闭卡片一键 Fork，保留后续载入原生子会话；一次性固定 DSH 内核；不修复/升级本机版本组合。实施前 worktree 已 rebase 到 Codex 期 a06812f。以下第一关调研段落保留当时的基线及未决问题，相关结论以本段决定和第二关矩阵为准。

## 结论和产品边界

**DSH 的状态、审批、轮次完成、标题、会话和 HUD 有真实数据源，可接入 Matou。当前证据不足以承诺 11 项全部达到其它 Agent 的完整体验：原生 Fork 有实现，但 0.19.0 没有已证实可供外部安全调用的一键 Fork 入口。** 不应把 `/hooks` 占位理解成内核没有事件，也不应把包内 Channel 类型理解成已开放的外部 API。

建议采用 **`dsh dsh-tui` + 一次性 `--patch` 加载 Matou 观察插件 + 现有 provider 管线**。会话文件用于目录、预览、离线标题和恢复前校验；实时状态用内核事件。插件、启动补丁和控制说明由 Matou 自己的运行目录/制品提供，不安装进用户 profile，不修改用户项目指令文件，不维护另一套 DSH 会话数据库。

本方案的正式实现基线是 **Codex 期接入后的底座**。当前 worktree 从 main 创建，尚无 `docs/acceptance/codex-reference-matrix.md`，Codex manifest 的 fork/sessionLoader/autoTitle/autoRestore 仍为 false。监督方已确认实施前负责 rebase 到 Codex 期之上；本轮不合并、不读取另一 worktree 的产品代码，不把假设写成已经看到的代码。

技术覆盖首先针对 **dsh-TUI 的 DSH 内核**。TUI 也能运行 Claude/Codex 内核，其事件与历史未必经过 DSH Session；不能仅看卡片名字就假定三种内核等价。正式启动是否一次性指定 `backend: dsh`，需要评审决定，见末尾。

## 11 项能力落点

“可做”表示存在明确入口/数据结构；“需间接实现”表示要增加 Matou 适配层，不能照搬另一 CLI 参数。都不等于已完成产品或真实 UI 验收。

| # | 能力 | 结论 | 依据与用户结果 | 实现落点 / 未闭合点 |
|---|---|---|---|---|
| 1 | 新建 | 可做 | 启动器支持 `dsh dsh-tui` 和 `dsh --profile dsh-tui`，支持逐次 patch；PTY cwd 就是工作目录。E01–E03 | manifest、launch-plan、shell-integration、runtime-server；`mt create session --profile dsh` 复用现有创建路径。实际启动界面待测。 |
| 2 | 工作状态 | 需间接实现 | `agent/status`、`turn/start/end`、审批与用户问答事件可表达启动中、空闲、工作中、需要输入、完成和错误。E05、E07 | 新增 DSH 观察插件与事件映射，送入 provider-hook-server / work-status；过滤后台及子代理事件。 |
| 3 | 通知 | 需间接实现 | 原生完成通知与 Matou OSC 通知可能重复；内核完成/审批/错误数据比终端文案可靠。E07、E10 | 复用通知管线，标题 DSH；仅对已确认接管通知的 DSH run 抑制重复 OSC 路径。不可全局禁用用户 TUI 通知设置。 |
| 4 | HUD | 需间接实现 | 模型、effort、上下文、工具、权限有真实事件/投影；Git 复用 Matou。缺失项隐藏。E05、E08、E09 | DSH 适配 → provider-hud-payload-handler / session-hud-registry → TerminalHud；项目指令以实际 preset 读取结果为准。 |
| 5 | 执行模式 | 需间接实现 | 原生默认/计划/完全访问及动态 preset 与 Claude 四项不等价；有官方 permissionPresets 与 `/plan` 路径。E09 | provider-mode-service + DSH 插件调用官方当前会话命令/服务，事件回读后才显示成功；不能只改 HUD 或发启动环境变量冒充切换。完整 TUI 模式映射待探针。 |
| 6 | 自动标题 | 可做 | v4 `session/title.data.title` 是标题事实；样本来源为 fallback，不必再请求 LLM。E05、E08 | DSH catalog / provider 标题观察 → 现有手动锁定与恢复自动跟随；不回写 DSH 标题。 |
| 7 | 载入会话 | 需间接实现 | 按 cwd 编码定位，再核对 header.cwd；v4 为多帧 zstd，不能直接复用普通 JSONL reader。E04–E06 | 新增 dsh-session-catalog 与有界多帧读取，接 Codex 期目录接口、SessionLoaderDialog；搜索/预览尊重 surfaceOp。 |
| 8 | 重启恢复 | 可做 | 显式 `--resume <id>` / `--resume=<id>` 被 TUI parser 消费，不成为用户消息。E03 | 复用 binding、resume-monitor、durability gate、恢复调度与失败横幅；必须确认同一 ID 和实际 ready，不以进程未退出作为成功。真实恢复待测。 |
| 9 | Fork | 需间接实现；当前稳定外部接口下的一键原生调用不可承诺 | TUI `/fork` 创建独立、脱离前台的子会话，父会话不换 ID；没有发现 CLI `--fork` 参数，注入 socket 只支持追加/提交输入。E11–E12 | 不能照搬 Antigravity 的“子进程 resume 父会话再 /fork”：DSH 有占用互斥。推荐先保留原生 `/fork` + Matou 载入子 ID；是否接受此阶段性差异，或要求先获得上游正式调用口，待拍板。 |
| 10 | `mt` 跨卡片 | 需间接实现 | 现有 CLI、run token、环境变量、指令正文可复用；DSH 有项目指令与 system-prompt section。E13 | 注入独立 system section，不改 AGENTS/CLAUDE/profile；必须实测默认沙箱下真正执行 `mt`，不能以提示词出现作为接通证据。 |
| 11 | 不提供的能力 | 按规格不提供，可落实 | 账户切换、供应商面板、Agent Team 不在本期；不复制 TUI 自带管理界面。E14 | manifest 关闭入口；空配额/费用/团队统计不补 0；DSH 原生功能留在终端。 |

## 调研证据

以下 `TUI` 表示本机只读包目录：

`~/.dsh/profiles/dsh-tui/node_modules/@deepseek-harness-tui/dsh-tui`

`KERNEL` 表示独立全局安装的包目录：

`~/.nvm/versions/node/v24.13.0/lib/node_modules/@deepseek-ai/dsh/node_modules/@deepseek-ai`

源码证据是本机安装版本，不借新版文档承诺旧版能力。上游 [v0.19.0 发布记录](https://github.com/ccch1mneyyy/dsh-TUI/releases/tag/v0.19.0) 也说明该 TUI 版本适配 DSH 0.2.2-rc.1；没有升级任何组件。

### E01：版本与启动器

执行：

```sh
command -v dsh
dsh --version
dsh --help
```

输出摘录：

```text
/Users/icesword/.local/bin/dsh
0.2.2-rc.1
Usage: dsh [--profile] <name> [options] [app-args...]
--patch <path>  extra patch-list overlay applied after the profile layer (repeatable)
```

shim 只读内容表明：它把 Node 固定为本机 v24.13.0，然后执行全局 `@deepseek-ai/dsh/lib/bin.js`，不是标准 PATH 下的 Node 22.16.0。TUI `package.json` 的版本为 **0.19.0**。启动器 `lib/bin.js:13–24` 明确说明 `dsh <name>` 是 profile 语法糖，profile 后的 app 参数交给应用。

`dsh` 裸命令的启动器语法并不保证默认选中 dsh-tui。Matou 的默认实际 argv 应为 `file=dsh, args=[dsh-tui, ...]`，界面上的启动简称可为 `dsh`。`MATOU_DSH_COMMAND` 按现有约定覆盖可执行文件/包装器路径，默认 `dsh`；`dsh-tui` 是单独参数，不能把 `dsh dsh-tui` 整串作为 executable，也不能交给 shell 任意求值。若未来要求在变量中写完整复合命令，应另定解析契约，不顺带改变其它 provider。

### E02：本机组合与版本不一致

执行 `dsh dsh-tui --dump-config`，退出码 0；完整输出仅留在私有临时探针目录，不纳入仓库。组合里包含：session、agent、session-persistence-jsonl、session-query-sqlite、session-projection、approval、permission、system-prompt 与 dsh-tui 等行；没有 sessionController 行。

stderr 有两条现有 patch warning：

```text
patch: entry "tool-ralph" not found
patch: entry "tool-subagent-list-agents" not found
```

通过 TUI 包位置的 `createRequire(...).resolve()` 与 package.json 只读核对：`dsh-session`、`dsh-agent`、`dsh-session-persistence-jsonl` 实际解析到本机另一源码 worktree，包版本 **0.1.5-rc.2**，其中 Session 格式常量为 **3**。全局安装对应包为 **0.2.2-rc.1**，格式为 **4**。监督方已额外允许只读这些 symlink 目标源码；未修改、构建或执行该 worktree。

这证明的是**本机解析链不一致**，尚未证明它就是所有启动问题的原因。不能把启动器版本当成 TUI 每个依赖的版本，不能自动重装或修补用户 profile。后续探针须同时报告启动器、TUI、实际加载依赖的版本；混装环境与产品适配失败分开记录。

### E03：启动、cwd、resume 的确切语义

可采用：

```sh
# PTY 的 spawn cwd 指向用户选定目录；不依赖一个未验证的 --cwd 参数
dsh dsh-tui --patch /absolute/matou-run.patch.yml
dsh --profile dsh-tui --patch /absolute/matou-run.patch.yml --resume SESSION_ID
```

`TUI/lib/types/dsh-adapter/plugin.js:682` 读取 `config.sessionId ?? resumeTargetFromArgv(...)`；`TUI/cordis.patch.yml:476–479` 支持 workspace/sessionId/backend 的本次启动环境。工作目录应使用 PTY cwd；TUI 的 `config.cwd` 也存在，但 argv `--cwd` 不是已验证的交互启动参数。不能将无界面 CLI 的 `--cwd` 套到交互 TUI。

直接导入已安装的纯 parser，调用 `resumeTargetFromArgv(args, () => undefined)` 和 `initialPromptFromCmdlineArgs(args)`，结果：

| argv | resume ID | 初始 prompt |
|---|---|---|
| `--resume session-probe` | `session-probe` | 空 |
| `--resume=session-probe` | `session-probe` | 空 |
| `--resume` | 无 | 空 |
| `--continue` | 无 | 空 |
| `-- --resume session-probe` | 无 | 字面 `--resume session-probe` |

bare resume 会回退到用户的 `~/.dsh-tui/resume.txt`，不是 Matou 某张卡的身份。Matou 恢复必须始终传完整 ID；不改 resume.txt，不靠“最近一个会话”。ID 是 DSH Session ID，既可能是 UUID，也可能是带 `session-` 的字符串，不能只接受 UUID。

**help 的实测边界**：`dsh dsh-tui --help` 在非 TTY 管道下没有给出 TUI 参数帮助，约两分钟后由本轮终止自己的 help 进程；不将其视为可用 help 或启动成功。源码 `plugin.js:156–162` 有 interactive/headless-host 区分。真实 TTY 行为见 E15。

### E04：零输入身份

`TUI/lib/types/dsh-adapter/inject-channel.d.ts` 和 `plugin.js:2725–2751`：

```text
~/.dsh-tui/inject/servers.json
[{ pid: number, sessionId: string, cwd: string, socketPath: string, startedAt: number }]
```

它跟随**当前前台 session**重绑；切换时旧 socket/记录关闭；还没有真实 ID 的 starting placeholder 不绑定。`/status` 的原生展示会显示完整 `channel.agentId`（`screens/Chat.js:3370` 起）；底栏也有 sessionId 显示开关，但都不宜靠截取屏幕文字当主身份源。

`KERNEL/dsh-session/lib/types/index.d.ts:43,63` 提供 `session/created`、`session/event`；`dsh-agent/lib/types/runtime-types.d.ts:254` 有 `agent/created`，参数带 agent/source。创建阶段已能拿真实 ID，无须发送一条模型消息。

重要限制：`TUI/lib/types/dsh-adapter/fresh-agent.js` 会延后只有初始权限事实的空会话落盘。**只扫描新文件或 header 无法可靠确认所有新卡身份。** 文件元数据适合历史载入和已落盘会话的校验，不能独立完成零输入新建握手。

建议插件在 TUI 创建 agent 之前安装观察；启动初始 agent 与 Matou run 关联，再用 `servers.json` 的准确 PID/cwd/当前 ID 交叉确认。PID 允许核实 PTY 的实际子进程链，不能按全局最新记录或仅 cwd 选会话。同目录同时开两张卡、多后台会话都必须测试。会话已 ready 不等于已可恢复：尚未落盘时 `resumable=false`，App 重启可以新开空白卡，不能伪称找回原对话。

### E05：真实 v4 会话副本

监督方明确允许：只读复制 Matou Codex 期会话目录里的一个历史文件，仅在临时目录解压，不运行、不恢复、不写回该会话。实际找到 **1 个 v4 压缩文件**；复制后读取。仓库不保留提示词、原目录、原 ID、账号或工具正文。

命令形式：

```sh
cp <获准目录>/<原ID>/session.v4.jsonl.zstd <TEMP>/sample.v4.jsonl.zstd
zstd --list <TEMP>/sample.v4.jsonl.zstd
zstd -d -c <TEMP>/sample.v4.jsonl.zstd > <TEMP>/sample.v4.jsonl
```

实测：压缩 **253,977 bytes**，**158 frames**，解压 **894,073 bytes / 304 JSONL 行**，header.version=4，header.isSeeded=false。所有非空行均能 JSON.parse；这项检查只证明物理样本结构，没有冒充官方全量 replay 校验。

事件计数：

| 事件 | 数量 | 已证实字段 |
|---|---:|---|
| user/message | 5 | data.role/id/content/source；顶层 surfaceOp |
| assistant/message | 35 | data.message、usage、stream、turn/step；顶层 surfaceOp |
| tool/call | 43 | data.callId/name/arguments/turn/step |
| tool/result | 43 | data.message.toolCallId/content/isError、meta.cwd；sourceEventSeqs/surfaceOp |
| request/header | 2 | data.header.config.provider/model/reasoningEffort/maxTokens；tools |
| request/context | 1 | data.provider/model/contextWindow/systemPromptUpdate |
| session/title | 1 | data.title/messageSeqs/source.kind；样本 kind=fallback |
| permission/preset、sandbox/mode、approval/policy | 各 2 | 当前会话权限事实 |
| turn/start、turn/end | 各 2 | turn/end.data.reason.kind；样本两次均 completed |
| session/end-seed | 1 | 恢复/继承边界；**出现此事件本身不证明发生 Fork** |

脱敏结构样例（值均替换，非可回放 fixture）：

```json
{"type":"session","version":4,"id":"SESSION_ID","createdAt":1000,"cwd":"/REDACTED","isSeeded":false,"delegationDepth":0}
{"type":"tool/call","seq":10,"time":1001,"data":{"turn":1,"step":1,"callId":"CALL_ID","name":"TOOL_NAME","arguments":"<JSON_STRING>"}}
{"type":"assistant/message","seq":11,"time":1002,"surfaceOp":"append","data":{"turn":1,"step":1,"message":{"role":"assistant","id":"MESSAGE_ID","content":[]},"usage":{"inputTokens":1,"outputTokens":1,"cacheReadTokens":1,"cacheWriteTokens":1,"totalTokens":4},"stream":[]}}
{"type":"session/title","seq":12,"time":1003,"data":{"title":"REDACTED_TITLE","messageSeqs":[1],"source":{"kind":"fallback"}}}
{"type":"turn/end","seq":13,"time":1004,"data":{"turn":1,"reason":{"kind":"completed"}}}
```

样本没有 approval/asked/decided、model/selection，也不是 Fork 样本；这些项目的依据来自版本源码，而非声称此样本全部覆盖。

### E06：存储定位与多帧解压陷阱

`KERNEL/dsh-session-persistence-jsonl/lib/index.js:880–898` 的 projectKey：路径分隔符压为 `-`，非 ASCII/`~` 等转为 UTF-16 编码的 `~XXXX`，去前导连字符后截断，外包 `--...--`。目录名可碰撞/截断，所以列表必须再次比较 **header.cwd**，不能仅依赖目录名，也不能反解目录名来宣称原 cwd。

物理结构：

```text
$DSH_HOME/sessions/<projectKey>/<encoded-session-id>/session.v4.jsonl.zstd
```

用户可配置 root：`TUI/cordis.patch.yml:188–194` 使用 `DSH_TUI_SESSION_ROOT ?? dshHomePath('sessions')`。实现应接收本次实际 root，不硬编码唯一的 `~/.dsh`；不读取其它项目全文。

关键实测：对这份完整压缩副本，Node 24.13.0 的 `zstdDecompressSync` 和 `createReadStream(...).pipe(createZstdDecompress())` 都只得到 **215 bytes / 1 行 header**；zstd CLI 正确得到 304 行。Node 22.16.0 也已提供 zstd API，所以问题不是简单的“有没有 zstd API”。

官方 `dsh-session-persistence-jsonl/lib/types/zstd.d.ts` 明确这是**串联独立帧容器**，提供结构帧扫描、逐帧校验、末尾 torn frame 的概念。产品实现不能复制本次 `zstd -d` 命令当作每台 Mac 必有的依赖；应在 DSH catalog 内有界扫描帧并逐帧使用 Node zlib，或使用经核实的已装 DSH 只读解码入口。优先不加 npm/native 依赖。

读取只做元数据/预览，不修复、截断或迁移源文件。完整帧损坏报错，活动写入的未完成尾帧等待下次读；未知格式不当空会话。预览按照 v4 surfaceOp/sourceEventSeqs 处理替换与压缩后的可见消息，不能直接拼接所有 assistant 正文。未知 required 事件应标不支持，不静默丢弃；不为做列表而移植整个 DSH 执行器。

### E07：事件、钩子与 JSON/channel 的区别

`TUI/guide/dsh-tui-guide/interaction.md:921` 确认 `/connect`、`/hooks` 为兼容占位。可是全局 0.2.2-rc.1 内核确有：

| 所需事实 | 官方接口/事件 | Matou 使用方法 |
|---|---|---|
| PreToolUse 等价点 | `dsh-tools/lib/types/events.d.ts`：`tools/pre-execute` waterfall | 仅观察并原样 `next()`；不擅自改权限或工具参数。 |
| 开始执行/结束执行 | `tools/execute`、`tools/post-execute`、最终 `tools/result`；持久化 `tool/call/result` | 工具状态按 callId 配对；一个工具失败不直接等同整轮失败。 |
| 等待审批 | `approval/asked`，配对 `approval/decided`；`approval/request` 是问答 waterfall | 根据问答是否仍 pending 显示 needs-input；自动拒绝且立刻 decided 的情况不制造长期等待。 |
| 用户问题/计划审批 | 原生 user-questions 与 plan-mode 请求；TUI approvals/questions answerer | 不只捕捉工具审批；后续探针确认各问答的 pending → answered/withdrawn。 |
| 工作开始/停止 | `agent/status` idle/running、`turn/start/end` | agent idle 只表示驱动静止；完成必须核对 turn reason。 |
| 完成/错误/取消 | `turn/end.data.reason.kind` | completed 才完成；error 读结构化 error；aborted 不报成功；interrupted/forked 不是新的完成通知。 |
| 标题/模型/权限 | `session/title`、`model/selection`、request/header/context、permission/sandbox/approval/plan 事件 | 同一当前 session 的真实事件驱动 HUD 与标题。 |

`KERNEL/dsh-session/lib/types/types.d.ts:170–216` 的 reason 还包括 blocked、max-tokens；未知新增 reason 不强行归为 completed。恢复时重放的旧事件只能重建快照，不能重新弹历史完成/审批通知。

**TUI Channel：** 包内存在可订阅的 Channel、`forkSession()`、`permissionPresets()`、contextOccupancy 等类型；`adapter/ports/channel.d.ts:1–20` 明确它们是 host-internal。`dsh-adapter/plugin-host.d.ts` 的 `getHostFacade` 标为 host-only，公共 package exports 没有直接导出这个控制入口。`tui-profile/registry/contracts/tui-channel-v1alpha1.json` 有 open/subscribe/invoke/close 的协议描述；`adapter/upstream/channel-driver.js:80–114` 却是进程内 provider/consumer 及 replay probe，**不能推导出一个正在监听的外部 JSON socket/HTTP 服务**。本方案不绕过宿主守卫读取内部能力。

**独立 JSON CLI：** `guide/dsh-tui-guide/cli.md` 与 `cli/adapters/dsh/index.mjs:11–16` 只允许 0.2.0-rc.2 / 0.2.1-rc.2–rc.5，当前启动器版本不在名单。`dst run --output-format stream-json` 是独立 headless 会话，不会旁听已开 TUI；即使升级后版本兼容，也不能当交互卡片的等价接入方式。本轮未运行模型请求。

### E08：HUD 数据与标题来源

| 数据 | 优先数据源 | 局限 |
|---|---|---|
| 模型/provider/effort | 0.2.2 `agent.model.current()`；其后 `model/selection` / request/header.config | API 是只读 current，不发请求；旧版/不完整路由缺失则隐藏，不能拿部署默认值冒充正在使用。 |
| 上下文占用 | 官方 sessionProjections 的 contextPressure：`projectedTokens ?? pressureTokens`，除以真实 contextWindow | `TUI/.../context-occupancy.d.ts` 明确口径；累计 token 不是上下文占用率。meter 未出样不显示 0%。 |
| token | assistant/message.data.usage 的 input/output/cacheRead/cacheWrite/total | 样本字段已证实；没有 usage 就未知。重放/继承前缀不能重复累计本次增量。 |
| 费用 | TUI 有 costReport/mainCost 等投影；其估算依赖模型价格信息 | 会话样本 usage 没有金额。拿不到可信价格则隐藏；不把 token 用固定单价换成“真实账单”。当前 SessionHudWire 没有费用/总 token 展示字段，不顺带新增面板。 |
| 工具 | tool/call/result，tools/result | callId 配对；参数只取必要摘要，不把完整源码/敏感结果写进通知日志。 |
| 当前模式 | permission/preset + sandbox/mode + approval/policy + plan/mode | plan 和只读并不完全等价；组合未知时只读显示真实值，不能猜成 Claude 某模式。 |
| cwd / Git | 真实 session cwd、working_directory 变化；Matou 现有 Git repository | 创建 cwd 与运行时目录分开，不因工作目录变化换错历史归属。 |
| 标题 | 最新 session/title；可由 sessionTitle/title 投影读回 | 标题不在 header。原生可能是 fallback 或原生 LLM provider 结果，Matou 不再生成一遍。 |

自动标题沿用 Matou 的“自动跟随 / 用户手动锁定 / 恢复自动跟随”；锁定是卡片层选择，不修改 DSH 原生用户标题或 unpin 原生标题。

### E09：执行模式与项目指令

`TUI/cordis.patch.yml:452–470` 的原生默认循环为 default / plan / full，分别由 plan、sandbox、approval 原子状态组合；还允许部署配置和动态 permission presets。`dsh-permission-presets/lib/types/index.d.ts` 提供 catalog/current/resolve/set；`dsh-plan-mode` 提供官方 `/plan` 与 set(agent, active)。原生 TUI 自己也是通过命令/这些服务写入，再回读确认（`channel/mode-permission.js:115–164`）。

Matou 可以复用菜单外观、进出高权限确认与反馈规则，但选项必须来自当前 DSH 能力；不能无条件塞入 acceptEdits/auto。计划模式进入/退出还涉及原权限恢复，必须验证官方命令完整状态迁移，不能只调用 sandbox setter。先复用官方当前 agent 的命令路径；若无法达到原生语义，就保留 TUI 内切换并只读同步 HUD，不能用“重启+env”冒充成功。点击本次会话模式不应通过写 profile 实现。

`KERNEL/dsh-agent-instructions/lib/index.js:16–30` 默认项目候选是 `AGENTS.md`、`CLAUDE.md`，local overlay 是 `AGENTS.local.md`、`CLAUDE.local.md`；向上查项目根并有字节上限。TUI host 层 agent-instructions 被禁用，实际由 preset 组合（`cordis.patch.yml:59–72`）。例如随包 liangshen 的 instruction-hint 只提示文件存在，不能因此宣称所有 preset 都自动加载了正文。

因此 HUD 的“项目指令文件”应以实际 active preset/loadedContext 或 agent 作用域真实 loader 配置为准；普通标准 preset 先验证 AGENTS/CLAUDE 及 local 顺序，有效候选再允许用户主动编辑。无 loader/仅 hint 的 preset 不伪装为“已加载”。本轮没有编辑任何指令文件。

### E10：通知去重

0.19.0 `plugin.js:271–308` 已有 task-complete notifier，condition 为 off/unfocused/always，输出可能是 OSC/BEL/Windows toast；这不是 `/hooks` 实现。不能借用较新版本的审批通知发布记录来宣称 0.19.0 已有全部通知。

Matou 现有 `TerminalSurface.tsx` 接收 OSC 9/99/777 进入通知管线。建议在已确认由 DSH bridge 接管的 run 内，以 provider 通知作为卡片完成/审批/错误唯一来源；消费其重复终端通知而不再次弹出。桥未就绪时不提前吞掉原有可用反馈。终端正文、TUI 内提示保留。

仅把 `taskNotifications: off` 写进一次性 TUI config 不足以证明关闭了所有重复来源：用户 settings 层和外部 notifier 插件仍可能介入。需实测 off/unfocused/always 与常见原生 notifier 的组合；若第三方直接发系统通知且不能无配置禁用，明确列为无法完全去重的组合，不擅改其设置。

### E11：Fork 的实际实现和祖先

源码入口：`TUI/lib/types/screens/Chat.js:3357–3363` 的 `/fork` 分支调用 `channel.forkSession()`；`dsh-adapter/channel/session-fork.js:14–123` 的行为：

1. 工作中拒绝；先 settle compaction，再取当前对话种子。
2. 生成新 UUID；继承当前 preset、provider/model 与对话；创建 detached child。
3. 非空对话的 meta 带 `parentSession: source.id`，通过 compat 层设置 isSeeded/inheritedEventCount。
4. 释放 child handle 后写原生 `Fork: ...` 标题；父前台不切换；通知提供新 ID 与 resume 命令。
5. 只有权限事实的空会话 fork 可能仍不发布为可恢复历史，不能把其新 ID 当作已落盘成功。

普通 Fork 的血缘不能仅凭一个字段推断：TUI 0.19.0 这条路径明确传 parentSession；较新内核 SessionHeader 的注释允许普通 fork 省略 parentSession，origin=subagent 才是子代理分类依据。继承前缀由最后一个带 `{ inherited:true }` 的 session/end-seed 标记界定；普通恢复也可有不带该标记的 end-seed。历史缺少父引用时，Matou 只保存自己实际发起并确认过的 parent→child 关系，不从时间/相似正文猜祖先。

本次 v4 样本不是 Fork；上述结果为源码结论，尚无真实界面 Fork/子 ID/恢复验证。

### E12：为什么不能直接照搬另一 Provider 的 Fork

本轮未发现 TUI CLI 的直接 `--fork <id>` 语法。`inject-channel` 的外部协议只有：

```json
{"type":"prompt.append","text":"..."}
{"type":"command.execute","command":"prompt.submit"}
```

它不是任意 command.execute，也不是读写草稿的远程编辑器。往这里追加 `/fork` 再提交会污染非空草稿，不符合无误触/不丢草稿要求；本方案不采用这一做法。

原生会话占用保护也明确拒绝另一个 TUI 进程打开同一会话（`guide/.../interaction.md:415–421`）；不能先让子卡 resume 正在父卡使用的同一 ID 再发 `/fork`。

DSH 的官方 SessionStore.fork 和 SessionController.fork 存在，但后者未装在当前 TUI 组合，且使用官方默认模型等语义，不等于 TUI 当前模型/preset/草稿/挂载流程。为了一个按钮搭另一套 fork 编排、挂载整个 Web API、深导入 host-only Channel，都不是本方案的默认做法，也违反“不重复实现 TUI 已有能力”的约束。

替代方案：用户在原生 TUI `/fork`，Matou 从原生新会话事实及历史目录拿到完整 ID，再用现有载入流程在新卡 `--resume child`。完整一键体验则需要一个正式、可被一次性插件调用的 native fork action/result 接口：返回 childId，保留父前台、草稿和 preset/model，完成持久化并释放租约。**这是后续技术验证/上游能力前提，不宣称 0.19.0 已提供，也没有向上游发送消息。**

### E13：`mt` 指令与执行

现有资产 `apps/runtime/control-assets/providers/codex-developer-instructions.md` 的规则正文可复用；开头写明是 Codex，因此不直接原文件原样注入 DSH，也不修改 Codex 资产。新增 DSH 包装文本或从中性 host-control 内容生成，保持目标解析、稳定键、批量部分失败、移除前预览与确认等同一规则。

`KERNEL/dsh-system-prompt/lib/types/index.d.ts:47–69,243–256` 提供有名有序的 `systemPrompt.section({name,order,text,interpolate:false})`，适合加入单独的 Matou section，避免覆盖用户 persona/system prompt。应绑定当前 Matou run/当前 agent；插入时机在首次模型请求前。项目 AGENTS/CLAUDE 的发现不是注入 `mt` 的必要条件，不编辑这些文件。

复用 runtime-server 已发放的 `MATOU_CONTROL_ENDPOINT/TOKEN/PROTOCOL/CALLER_SESSION/CALLER_RUN/NODE_EXECUTABLE` 和 control-assets/bin 的 PATH。令牌仅留在运行环境/受控桥接载荷，不进入提示词、会话正文或文档样例。

还必须验证 DSH 实际 shell 工具、Seatbelt 沙箱与 Matou Unix socket 的组合。Antigravity 的 PreToolUse 特殊处理不能照搬为 DSH 的权限判断。先测默认模式的 `mt identify --json`、`mt list --json`、只读兄弟卡与一次明确的 `mt send --enter`；若被沙箱拦住，带实际调用与拒绝事实回到主代理裁决，不能把整个会话切到完全访问、加入宽泛 allow-list，或悄悄新增另一套 control tool。相关能力未实测前，不能将 #10 记为已验收。

### E14：Matou 当前可复用位置

已读 `packages/domain/src/agent-manifest.ts`、`apps/runtime/src/session/provider-hook-server.ts`、`docs/acceptance/antigravity-reference-matrix.md`，以及 launch-plan、notification/work-status、HUD contract、runtime-server、mt 资产与 loader 入口。

可复用：run/session/binding 身份、hook HTTP token、过期 run 拒绝、标题手动锁定、HUD registry、通知聚合与冷却、历史列表与预览 UI、恢复失败横幅、fork intent/lease、mt 控制权限、Git 状态。DSH 事件仅在 DSH 分支归一化，既有三 provider 的参数、事件映射和行为不变。

当前 SessionWorkStatus 使用 idle/running/needs-input/error；“完成”由完成事件和通知表达，不为 DSH 新造一个不兼容全局状态。启动中沿用已有 run 生命周期。

### E15：真实界面探针、授权与恢复核对

发起前发现 TUI `channel/upstream-retry.js:119–132` 可能通过 settings.mutate 写入 provider retryPolicy，因此按交代先询问。监督方选 A，准许临时 overlay、精确备份、哈希核对、必要时只回滚本次变化；未获准时没有启动新会话。

准备的临时 patch 从只读 dump 中提取完整 dsh-tui config，保留已有字段，仅本次固定 DSH 内核并设置 upstreamRetry=false、taskNotifications=off；没有改写任何 `~/.dsh/profiles` 文件。备份和原始证据位于：

`/var/folders/pz/y1l2jfbn2k78f6__84v18jhc0000gn/T/matou-dsh-research-7kufykp7`

此目录含敏感原始副本/备份，不作为分享制品；方案只保留脱敏结构。没有新增产品依赖。

仅委派了任务允许的 `gpt-6.1-terra`、high、fork_turns=none 界面代理。CUA 选择系统终端时明确返回：

```text
Computer Use is not allowed to use the app 'com.apple.Terminal' for safety reasons.
```

因此 **未创建终端窗口、未启动交互新会话、未发模型消息、未执行真实 Fork、没有截图**。监督方已答复不绕过，界面行为本关列待验证。

保护性探针准备后及最终交付前两次核对：**179 份既有配置/偏好/全局存储文件 SHA256 全部一致，最终清单新增文件 0，未执行回滚**。没有以整文件覆盖其它并发会话的改动。历史样本源文件与复制样本的 SHA256 也一致；复制与解压仅发生在临时目录。

## 选定的实现路径

### A. 启动与观察，先闭合身份

1. 在 Codex 期基线上新增 dsh manifest；displayName/shortName 为 DSH。executable 默认 dsh，固定 profile 参数 dsh-tui；MATOU_DSH_COMMAND 是该 provider 独立覆盖项。清单字段确需扩展时采用可选项，既有三项默认值保持原样。
2. Shell 升级只识别 bare `dsh`（由 Matou 转成 dsh-tui）、`dsh dsh-tui`、`dsh --profile dsh-tui` 这几种托管形式。`dsh web/headless/plugin` 和其它 profile 不劫持成 DSH 卡。
3. 新增轻量 `dsh-runtime-adapter.ts` 负责为每个 run 生成一次性 patch 与插件配置；路径绝对、文件只在 Matou run 目录，退出清理。保留用户 patch 顺序和完整原行配置；不能只写 dsh-tui.config 的一个字段，因为该配置是整块替换。
4. 通过 Cordis 有序组合/依赖确保观察插件在 TUI 首个 agent 创建之前已注册监听；后加载时只读现有 agent 快照补握手，不以发送 prompt 补事件。此时不引入新的全局 Agent 框架。
5. 插件将原始事实最小化后发往已有 per-run hook HTTP 地址；载荷包含 provider、runId、DSH sessionId、进程身份、事件序号/类型、当前 snapshot。复用既有认证/注册撤销，不另起面向局域网的服务，不写另一份会话日志。
6. 初始 ID 与恢复 expected ID 核验后才设置 provider-ready；随后从 inject ledger 交叉确认前台。一个进程多个 session 时，后台事实不能覆盖前台 HUD；切换前台需新 identity generation。早到事件短暂缓冲，已退休 generation/run 全部拒绝。
7. 用户在 TUI `/new`、`/resume`、rewind 后可改变前台 ID。对同一 run 的合法前台切换更新 binding；恢复握手阶段却不接受任意不同 ID。Fork 创建 detached child 不改变父卡 binding。

### B. 状态、通知、标题与 HUD

对创建/恢复先发只读快照；只对随后新发生的事件触发通知。agent/created 的监听不 await 网络，不能让 Matou 断线卡住 TUI 首帧；发送采用有界队列，状态/HUD 可合并为最新值，审批/完成/错误按事件序号保留。HTTP 不可达不影响原生运行；重连拉快照，不补弹历史通知。

归一化规则集中于 DSH adapter，优先送 provider-hook-server 已有输出口；不把 DSH reason 硬塞为 Claude Stop 再丢掉错误。临时工具异常保留为工具异常；整轮 error 才进入粘性的 error 状态，新用户输入/重试才解除。审批与用户问答 pending 清空后回到真实 running/idle。

模型/上下文/工具/标题采用 E07–E09 的数据源。未知、未采样、未加载、格式不兼容四种情况都不显示为 0 或成功；费用/配额/账户/团队字段没有数据则不出现。Git 保持原管线，不额外按 token 事件刷新 Git。

### C. 目录和恢复

新增 DSH catalog，沿用 Codex 期 provider 目录 API、分页、取消、预览/全文搜索和目录冲突处理。按当前 cwd 的项目目录枚举会话，header 验证 id/cwd/版本；索引只保存在当前进程缓存，不创建新的磁盘数据库。

先实现真实 v4 的有界多帧读取：逐帧限额、JSONL 单行限额、分页/取消、文件增长增量读取；旧格式/未知版本显示“当前不可预览”并交由原生命令明确载入，不在后台迁移。精确上限随第二关真实样本测量定值，不以任意小限额悄悄截断内容而不提示。

恢复流程：读 binding → 校验文件与 cwd → 用显式 `--resume ID` 启动 → 插件/前台确认同一 ID + ready → 标记恢复成功。进程退出、ID 不匹配、原生 startupFailure、被其它 TUI 占用，都进入既有失败反馈，保留重试/新开选择。新开须清掉旧 expected ID 和 stale run，不能把新空会话当恢复成功。此路径不写用户 resume.txt。

### D. 模式与 `mt`

观察层先做只读模式同步；模式菜单命令通过 DSH 官方当前会话 command/service 路径，必要时由 DSH 专用的受认证 run 内请求/响应入口传递，复用 registration 生命周期与 token，不另增数据库。每次操作都携带 expectedSessionId/generation；TUI 换会话后过期命令拒绝。只有官方事实回读确认才更新菜单选中态。无法完整复用原生模式迁移时保持只读，不用按键盲循环切换。

`mt` 使用一个独立 system-prompt section 与现有环境，不把说明当成用户第一句。首次请求前、resume、fork 子卡都应覆盖，且只注入一次。默认沙箱连通性是第二关必须证明的前提；失败先报告，不自动扩大文件或网络权限。

### E. Fork 保持显式缺口

第一关不决定引入另一套 DSH fork 实现。后续若评审接受“原生 /fork 后载入”，复用已有目录/载入能力，只绑定原生已持久化并释放占用的 child ID；不靠目录最新文件猜配对。若要求完整卡片一键，则先证明存在正式 native action 接口，否则暂停 #9，不影响无冲突的观察/历史工作。Matou fork intent 在有权威 child ID 前不得记成功。

## 文件级实施清单

以下均为后续关卡计划，本关未修改。带“Codex 期”表示须在 rebase 后核对真实文件/接口，不视为本工作树现有成果。

| 文件/目录 | 复用或新增的具体职责 |
|---|---|
| `packages/domain/src/agent-manifest.ts` | 增加 dsh/DSH；独立 command env、profile argv、能力开关；模式/Fork 未证明前不启用；关闭 account/provider/team。 |
| `apps/runtime/src/session/provider-launch-plan.ts` | DSH 分支：profile、patch、显式 resume；不修改另外三种 argv。 |
| `apps/runtime/src/session/shell-integration.ts`、`provider-command-environment.ts`、`pty-session.ts` | 精确识别托管命令；沿用 PATH/cwd/PTY；只在必要处加 DSH 分支。 |
| 新增 `apps/runtime/src/agents/dsh-runtime-adapter.ts` | 临时 patch/插件路径、实际版本/服务能力探测、配置合成与清理；不安装 profile 插件。 |
| 新增 `apps/runtime/control-assets/providers/dsh/bridge.mjs` | 纯事件观察、初始快照、前台身份、独立 system section；模式控制仅调用已验证官方路径；不深导入 host-only Channel。 |
| 新增 `apps/runtime/control-assets/providers/dsh-system-message.md` | DSH 中性包装与 mt 规则；复用 host-control 内容语义，不改 Codex 原资产。 |
| `tooling/prepare-runtime-control-assets.mjs` | 将这些新增自有资产放进已有 runtime 控制资产制品；本关不打包。 |
| `apps/runtime/src/session/provider-hook-server.ts` | 沿 Codex 期泛化口注册 DSH、验证 run/identity/seq、输出状态/HUD/标题/通知；若泛化已完成则只增 DSH adapter 注册。 |
| 新增 `apps/runtime/src/session/dsh-provider-events.ts` | 原生事件到现有事件语义的定点映射，含错误/取消/重放/审批配对。 |
| `provider-notification-event.ts`、`provider-work-status.ts`、`provider-hud-payload-handler.ts`、`session-hud-registry.ts` | 复用消费管线；DSH 数据未知时保持 optional，不改其它 provider 的规则。 |
| 新增 `apps/runtime/src/session/dsh-session-catalog.ts`、按需同目录 `dsh-session-log-reader.ts` | cwd 定位、完整 header、多帧读取、可见消息预览/搜索、标题；有界内存与取消，不做写回或迁移。 |
| Codex 期 provider catalog 路由、`apps/runtime/src/runtime-server.ts` | 登记 DSH catalog/launch/bridge，复用 session binding、mt env、生命周期；不另造 session persistence。 |
| `apps/runtime/src/session/provider-resume-monitor.ts`、`session-durability-gate.ts`、`apps/runtime/src/recovery/runtime-session-recovery-scheduler.ts` | 显式 ID ready 握手、占用/错误/超时反馈、空白会话不可恢复语义。 |
| `apps/runtime/src/session-canvas/provider-mode-service.ts` | DSH 原生模式映射、官方命令回读、过期 identity 拒绝。 |
| `apps/runtime/src/session-canvas/fork-workflow-service.ts`、`fork-operation-coordinator.ts`、`session-fork-intent-repository.ts` | 保留已有 intent/lease；只有选定且验证的 DSH Fork 路径才启用。不能先认父 ID 为子 ID。 |
| `packages/contracts/src/hud.ts`、Codex 期 catalog/contracts | 复用现有字段；仅真实 DSH 模式身份/可用选项超出当前契约时做最小可选扩展，先评审，不预建通用协议。费用/账单 UI 不新增。 |
| `apps/desktop/src/renderer/src/hud/TerminalHud.tsx`、`session-canvas/SessionLoaderDialog.tsx`、既有 provider 菜单与 i18n | 沿已有 UI 组件增加 DSH 项；可用能力驱动显隐。无布局改造；如需前端实现，按交代交由指定界面执行流程。 |
| `apps/desktop/src/renderer/src/terminal/TerminalSurface.tsx`、notifications 入口 | 仅当前 DSH run 已接管时消除重复 OSC 通知，保持其它 provider 行为。 |
| 后续新增 `docs/acceptance/dsh-reference-matrix.md` | 真实 DSH 与 Matou CLI 黑色工作区内 11 项逐条证据、失败/未测与交互性能；不把本方案当验收矩阵已闭合。 |

预计不需要新增产品数据库、数据迁移、运行时 LLM 请求或 npm 依赖；只有 provider manifest/资产/适配所需增量。若实际证明需要其它存储、依赖或核心接口改造，先带具体原因与替代方案回评审，不直接实施。

## 建议的后续关卡和验收

**第二关：技术闭环探针。** 在获准且版本一致的真实 TUI 环境验证一次性插件能加载而 profile 哈希不变；首帧/零输入身份、同 cwd 双卡、TUI 前台切换与后台隔离、工具/审批/问题/完成/错误、上下文投影、默认沙箱的 mt 执行、多帧 v4 与新格式拒绝。模式完整迁移及 native fork action 在此单列去留，不让失败拖累另外三种 provider。此关允许的文件与写入范围须另行放行，本方案不等于已获实施授权。

**第三关：卡片核心集成。** manifest/启动/状态/通知/HUD/标题/mt。对真实负载测量状态延迟、首帧、焦点、滚动与误触；状态与通知至少采用 Antigravity 矩阵已有的 ≤1000ms 观察标准，并记录事件到 UI 的样本，不靠空载静态页面验收。

**第四关：历史、恢复与已决策的 Fork。** cwd/中文路径、同目录并发、长历史/compaction/search/分页、活动尾帧/损坏/未知事件/未知格式；重启后能回忆重启前对话，失败重试/新开、会话被占用、父子独立 ID 与上下文、手动标题锁定都做真实行为验证。没有 native 一键接口时 #9 明确未闭合。

**最终产品验收：** 仅在后续另行放行后，真实打包 App + 真实本机 dsh-TUI，补齐对照矩阵与三 provider 回归；统一执行必要 typecheck / identifiers / 相关测试，不每个文件重复全量构建。本关不做打包、版本、提交或发布。

定点自动验证应覆盖：启动 argv 不变回归、事件乱序/旧 run/后台/重放不误报、审批 pending 配对、相同 cwd 身份不串卡、多帧 reader 完整读到样本末尾而非只读 header、surface 替换/损坏拒绝、恢复 ID 不符不成功。使用脱敏最小 fixture，不把获准的用户真实历史提交到仓库；不写只镜像实现的无效测试。

## 待拍板与尚未验证

1. **Fork 产品取舍。** 是否允许第一版采用原生 `/fork` 后在 Matou 载入子会话？推荐不降低“完整接入”的验收标准：若仍要求卡片一键，先获得/验证正式 native action 接口，#9 保持未闭合；不通过深导入内部 Channel 或另写 fork 管理来凑齐。
2. **DSH 卡片对应的内核。** 推荐本期明确使用 DSH 内核，可用一次性 backend=dsh，不改用户记忆；这与“完全遵循 TUI 已记住的其它内核”有用户可感知差异。若必须保留 `/kernel` 启动记忆与任意切换，就要补充各 backend 的信号适配与验收，不能默认获得 Claude/Codex 能力。
3. **版本一致的真实界面环境。** 当前混合依赖与 CUA 禁止系统终端是两个独立限制。用户配置不自动修复；后续应提供获准可操作的真实 TUI 验证渠道。不是把此次非 TTY help 等待认作正常 TUI 启动。
4. **仍未实证：** 一次性桥插件运行闭环、默认沙箱中 mt、计划/权限完整切换、真实前台 ID 重绑、真实 Fork/恢复/审批通知与高负载延迟。它们有数据/源码依据或明确接口缺口，但本关没有产品代码或 UI 成功证据。

## 本关范围自检

- 相对初始 git 状态只新增本方案；原本未跟踪的 PRD 是监督方提供，保持不动、不提交。
- 未写产品代码、未新增依赖、未改用户 profile/全局配置、未恢复/运行其它会话、未访问其它项目的会话正文、未合并/rebase、未提交/push、未打包/发布/改版本。
- 临时 overlay/备份/解压是经确认的调研例外；原始资料仅留私有临时目录，不进入交付正文。
- 已回读 git status：只有初始未跟踪 PRD 与本轮新增方案；无受控产品文件改动。
- `pnpm check:identifiers` 通过；`git diff --check` 无受控改动错误，另检查新增文档无行尾空白、11 项编号完整且无重复。
- 179 份配置/偏好/全局存储哈希全部一致，清单无新增；获准历史样本源文件与副本哈希一致。
- 未运行 typecheck/产品测试/构建：本轮没有产品代码变更；真实 TUI 界面、桥接闭环、Fork/恢复和交互性能均未验证。
