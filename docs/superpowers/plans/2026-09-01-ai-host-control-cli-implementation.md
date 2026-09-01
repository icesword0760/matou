# Matou AI 宿主控制与 `mt` CLI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [x]`) syntax for tracking.

**Goal:** 让每个 Matou 托管的 Shell、Claude Code、Codex 会话自动获得 `mt` CLI 和受限宿主控制身份，使用户或 AI 能在不改变界面焦点、滚动位置与通知状态的前提下识别、查看和输入其他终端。

**Architecture:** 扩展现有 Runtime Host Control，不新增第二套 Renderer 权威。能力令牌同时携带调用方 SessionRun 身份；Runtime 从数据库投影窗口/工作空间/事项/画布/DAG 层级，从 PTY 输出维护最新终端屏幕；`mt` 只做参数解析和本地协议调用。Claude Code 与 Codex 在各自进程启动时加载同一份会话级行为规范，Shell 只注入 CLI。

**Tech Stack:** TypeScript 7、Node.js 22、Electron 43、node-pty、SQLite、`@xterm/headless` 6、Vitest、Playwright、tsup、electron-builder。

**Spec:** `docs/superpowers/specs/2026-09-01-ai-host-control-cli-design.md`

## Global Constraints

- 只交付 `identify/list/read/history/commands/send/key`；不暴露创建、Fork、移出、关闭、聚焦或切换视图。
- 相对位置由 Runtime 根据调用方当前窗口、画布和 DAG 层级解析；CLI 不自行猜测界面顺序。
- 所有 Matou 托管 Shell、Claude Code、Codex 都获得 CLI 和 run-bound token；只有 Claude Code、Codex 获得自然语言规则。
- `read` 返回最新运行屏幕，`history` 返回 Journal 可见历史，`commands` 返回 Shell Integration 命令边界。
- 输入按“文本 + 可选回车”或“一个控制键”作为不可穿插的动作排队。
- 读取和输入不聚焦、不滚动、不激活窗口、不闪框、不发通知。
- 不修改系统 PATH，不写 `~/.claude`、`~/.codex`，不依赖用户安装 Node/npm。
- 所有新增协议错误保留结构化错误码；Runtime 控制服务单次失败不影响 PTY。
- 当前工作区已有大量非本功能改动。每次提交只暂存本任务列出的路径，提交前用 `git diff --cached --name-only` 核对范围。

---

## Task 1: 固化调用方身份、目标选择器与首期协议

**Files:**
- Modify: `apps/runtime/src/control/host-control-server.ts`
- Modify: `apps/runtime/src/control/host-control-server.test.ts`
- Create: `apps/runtime/src/control/host-control-types.ts`

**Interfaces:**

```ts
export interface HostCallerIdentity {
  runId: string
  sessionId: string
}

export type HostTargetSelector =
  | { kind: 'self' }
  | { kind: 'relative'; direction: 'left' | 'right' }
  | { kind: 'relation'; relation: 'parent' | 'child'; ordinal?: number }
  | { kind: 'sibling'; ordinal: number; projectionRevision: string }
  | { kind: 'ref'; ref: string; projectionRevision: string }
  | { kind: 'session'; sessionId: string }

export type AllowedControlKey =
  | 'Enter' | 'Tab' | 'Escape' | 'Backspace' | 'Delete'
  | 'ArrowUp' | 'ArrowDown' | 'ArrowLeft' | 'ArrowRight'
  | 'Home' | 'End' | 'PageUp' | 'PageDown'
  | 'CtrlC' | 'CtrlD' | 'CtrlL' | 'CtrlU' | 'CtrlZ'
```

- `CapabilityRecord` 保存 `caller: HostCallerIdentity`，`validate()` 返回该 record。
- 新增 scope/method `host.identify`；`host.list` 接受 `{ scope: 'current-level' | 'all' }`。
- 所有目标型请求通过 `HostTargetSelector`；按序号/ref 的请求校验 `projectionRevision`。
- `terminal.send-text` 参数为 `{ target, text, submit: boolean }`，`submit` 默认 false。
- 错误码补齐 `TARGET_NOT_READY`；失活 PTY 映射为该错误，不再泄漏内部异常文字。

**Steps:**

- [x] 在 `host-control-server.test.ts` 写失败测试：token 能识别 caller、`host.identify` 返回 caller、自身 token 只含首期 scopes。
- [x] 写失败测试：相对/父子/同级序号 selector 被原样交给 backend；过期 revision 返回 `CONFLICT`。
- [x] 写失败测试：`send-text` 把 `text + submit` 作为一个 backend 动作；新键位全通过、任意宏仍返回 `UNSUPPORTED`。
- [x] 运行 `pnpm --filter @matou/runtime test -- host-control-server.test.ts`，确认因新协议缺失而失败。
- [x] 提取 `host-control-types.ts`，更新 token 与 server dispatch；删除 Host Control 对 task 写入/移动 scopes 的首期 token依赖，但保留内部 server 代码兼容既有调用。
- [x] 重跑目标测试并运行 `pnpm --filter @matou/runtime typecheck`。
- [x] 提交：`git add apps/runtime/src/control/host-control-{types,server}.ts apps/runtime/src/control/host-control-server.test.ts && git commit -m "feat(control): define caller-aware terminal protocol"`。

## Task 2: 建立 Matou 窗口/画布/DAG 权威拓扑投影

**Files:**
- Create: `apps/runtime/src/control/host-topology-projector.ts`
- Create: `apps/runtime/src/control/host-topology-projector.test.ts`
- Modify: `apps/runtime/src/control/runtime-control-backend.ts`
- Modify: `apps/runtime/src/control/runtime-control-backend.test.ts`

**Projected target shape:**

```ts
export interface HostTarget {
  ref: string
  title: string
  profile: 'shell' | 'claude-code' | 'codex'
  cwd: string
  workStatus: string
  window: { id: string; kind: 'main' | 'detached-terminal'; ordinal: number }
  workspace: { id: string; name: string; ordinal: number }
  task: { id: string; name: string; ordinal: number }
  canvas: { id: string; name: string; ordinal: number }
  session: { id: string; ordinal: number; detached: boolean }
  dag: { depth: number; parentRef?: string; childRefs: string[]; siblingRefs: string[] }
}
```

内部兼容类型 `agent-team-member` 在控制投影中规范化为 `claude-code`，不新增可见类型、入口或专属命令。

**Projection rules:**

- 主窗口来自 `app_windows + window_navigation + window_task_placements`；独立窗口来自 `scene_windows.state='detached'` 与 mount 归属。
- 工作空间、事项、画布分别按当前产品排序字段和现有投影顺序；不以数据库随机行序代替界面顺序。
- 同层卡片只包含同一 `scene_id`、相同结构父节点的未归档 Session，并按 `last_user_interaction_seq DESC, sibling_created_seq ASC, sessionId` 与当前图投影一致排序。
- 归档/移出节点不参与 sibling ordinal；独立窗口节点保留原 DAG 父子和 sibling 身份，并标记 detached。
- 相对目标只解析调用方所在 level；明确名称搜索由 `list(scope:'all')` 返回所有窗口候选。

**Steps:**

- [x] 用内存数据库写失败测试，构造两个窗口、同名工作空间/事项、两个画布、父子两层、屏外 sibling、归档节点和 detached 节点。
- [x] 断言 `identify(caller)` 返回完整层级；`listCurrentLevel` 只含同层；`listAll` 可跨窗口；父节点不混入 ordinal。
- [x] 断言 left/right、parent、child:N、sibling:N 的解析结果；边界返回 `TARGET_NOT_FOUND`；同名搜索由 CLI 层保留多候选而非静默取首个。
- [x] 断言排序变化导致 `projectionRevision` 改变，旧 ordinal 请求返回 `CONFLICT`。
- [x] 运行 `pnpm --filter @matou/runtime test -- host-topology-projector.test.ts runtime-control-backend.test.ts`，确认失败。
- [x] 实现 projector，并让 backend 的 `identify/listTargets/resolveTarget` 全部依赖该 projector；移除旧的全局 `surface:N` 平面排序。
- [x] 重跑测试与 runtime typecheck。
- [x] 提交：`git add apps/runtime/src/control/host-topology-projector* apps/runtime/src/control/runtime-control-backend* && git commit -m "feat(control): project Matou window and DAG topology"`。

## Task 3: 将“当前屏幕”与 Journal 历史拆开

**Files:**
- Modify: `apps/runtime/package.json`
- Modify: `pnpm-lock.yaml`
- Create: `apps/runtime/src/control/terminal-screen-projector.ts`
- Create: `apps/runtime/src/control/terminal-screen-projector.test.ts`
- Modify: `apps/runtime/src/session/pty-session.ts`
- Modify: `apps/runtime/src/session/pty-session.test.ts`
- Modify: `apps/runtime/src/control/runtime-control-backend.ts`
- Modify: `apps/runtime/src/control/runtime-control-backend.test.ts`

**Behavior:**

- 每个活跃 `PtySession` 持有 `@xterm/headless` Terminal；onData 写入，resize 同步调整 cols/rows。
- `snapshotScreen()` 序列化当前 viewport，去除每行右侧填充，但保留终端换行、光标行和 ANSI 控制后的实际画面。
- 进程未活跃时，backend 从 Journal 按 output/resize 顺序重放到临时 projector，再返回最后画面。
- `readHistory()` 继续读取 Journal 输出文本，返回 `{ text, truncated, firstSequence, lastSequence, source:'journal' }`；`readCurrent()` 返回 `{ text, cols, rows, source:'screen' }`。
- `readCommands()` 继续读取 Shell Integration 的命令边界，不从 screen 或原始 Journal 文本猜测命令。
- 用户在 Renderer 中滚动 scrollback 不写 Runtime projector，因此不会改变 `readCurrent()`。

**Steps:**

- [x] 添加 `@xterm/headless@6.0.0`，锁定 lockfile。
- [x] 写失败测试覆盖：回车覆盖、清屏、光标移动、resize、宽字符、长输出滚屏、ANSI 颜色不污染文本。
- [x] 写 backend 失败测试：active 读取 screen；inactive journal replay 得到相同 screen；history 与 current 返回不同 source/内容。
- [x] 运行 `pnpm --filter @matou/runtime test -- terminal-screen-projector.test.ts runtime-control-backend.test.ts`，确认失败。
- [x] 实现 projector 与 `PtySession.snapshotScreen()`；确保 projector 写入异步完成后 snapshot 才返回。
- [x] 实现有界历史元数据，UTF-8 截断从完整 code point 起始，避免首字符变成替换符。
- [x] 重跑相关测试、runtime typecheck。
- [x] 提交：`git add apps/runtime/package.json pnpm-lock.yaml apps/runtime/src/control/terminal-screen-projector* apps/runtime/src/session/pty-session* apps/runtime/src/control/runtime-control-backend* && git commit -m "feat(control): project the latest terminal screen"`。

## Task 4: 串行化远程输入并对齐 Kooky 键位

**Files:**
- Create: `apps/runtime/src/control/terminal-input-queue.ts`
- Create: `apps/runtime/src/control/terminal-input-queue.test.ts`
- Modify: `apps/runtime/src/control/runtime-control-backend.ts`
- Modify: `apps/runtime/src/control/runtime-control-backend.test.ts`

**Exact key map:**

```ts
Enter='\r' Tab='\t' Escape='\x1b' Backspace='\x7f' Delete='\x1b[3~'
ArrowUp='\x1b[A' ArrowDown='\x1b[B' ArrowLeft='\x1b[D' ArrowRight='\x1b[C'
Home='\x1b[H' End='\x1b[F' PageUp='\x1b[5~' PageDown='\x1b[6~'
CtrlC='\x03' CtrlD='\x04' CtrlL='\x0c' CtrlU='\x15' CtrlZ='\x1a'
```

**Steps:**

- [x] 写失败测试：同一 session 的两个并发 `sendText(..., submit:true)` 产生 `first\r` 后 `second\r`，中间不穿插；不同 session 可独立执行。
- [x] 写失败测试：session 在排队期间退出时，当前/后续动作返回 `TARGET_NOT_READY`，队列可被清理。
- [x] 写失败测试：完整 Kooky allowlist 映射精确；`return/esc/up` 等 CLI alias 在 CLI parser 归一化，server 只接收 canonical key。
- [x] 运行目标测试确认失败，实现 per-session promise queue 和 queue cleanup。
- [x] backend `sendText(sessionId,text,submit)` 每次只调用一次 `write(text + (submit?'\r':''))`；`sendKey` 走同一动作队列。
- [x] 重跑测试与 runtime typecheck。
- [x] 提交：`git add apps/runtime/src/control/terminal-input-queue* apps/runtime/src/control/runtime-control-backend* && git commit -m "feat(control): serialize terminal input actions"`。

## Task 5: 实现可独立运行的 `mt` CLI 客户端

**Files:**
- Create: `apps/runtime/src/control/host-control-client.ts`
- Create: `apps/runtime/src/control/host-control-client.test.ts`
- Create: `apps/runtime/src/cli/mt-cli.ts`
- Create: `apps/runtime/src/cli/mt-cli.test.ts`
- Modify: `apps/runtime/tsup.config.ts`
- Modify: `apps/runtime/package.json`

**CLI contract:**

```text
mt identify [--json]
mt list [--all] [--json]
mt read TARGET [--lines N] [--bytes N] [--json]
mt history TARGET [--lines N] [--bytes N] [--json]
mt commands TARGET [--limit N] [--json]
mt send TARGET TEXT [--enter] [--json]
mt key TARGET KEY [--json]
```

`TARGET` 支持 `self`、`left`、`right`、`parent`、`child:N`、`sibling:N`、上一次 `mt list --json` 返回的稳定 ref，以及明确的 session ref。名称搜索通过 `mt list --all --json` 完成，CLI 不静默挑选同名目标。

**Exit codes:** `0` 成功；`2` 参数错误；`3` 目标缺失/歧义；`4` 权限或状态错误；`5` 超时/连接错误；`1` 其他服务错误。

**Steps:**

- [x] 写 client 失败测试：长度前缀帧、requestId、deadline、单请求单连接、半帧响应、服务错误、连接超时。
- [x] 写 CLI 失败测试：七个命令、target parser、键位 alias、`--enter`、JSON 输出、纯文本输出、错误码和 stderr。
- [x] 断言输出使用标题/序号/层级等人类字段；默认文本不展示 token、socket、runId 或原始内部 ID。
- [x] 运行 `pnpm --filter @matou/runtime test -- host-control-client.test.ts mt-cli.test.ts`，确认失败。
- [x] 实现无第三方依赖的 client/parser/formatter；环境缺少 `MATOU_CONTROL_ENDPOINT/TOKEN/CALLER_SESSION_ID` 时给出“仅能在 Matou 托管终端中使用”的明确错误。
- [x] tsup 增加 `src/cli/mt-cli.ts -> dist/mt-cli.cjs`，保留 runtime entry。
- [x] 重跑测试、runtime build，并执行 `node apps/runtime/dist/mt-cli.cjs --help`。
- [x] 提交：`git add apps/runtime/src/control/host-control-client* apps/runtime/src/cli apps/runtime/tsup.config.ts apps/runtime/package.json && git commit -m "feat(cli): add the mt host control client"`。

## Task 6: 在每个 SessionRun 注入 CLI、身份与最小权限

**Files:**
- Modify: `apps/runtime/src/runtime-server.ts`
- Modify: `apps/runtime/src/runtime-server.test.ts`
- Modify: `apps/runtime/src/session/pty-session.ts`
- Modify: `apps/runtime/src/session/pty-session.test.ts`
- Modify: `apps/desktop/src/main/runtime-host.ts`
- Modify: `apps/desktop/src/main/runtime-host.test.ts`
- Modify: `apps/desktop/src/main/index.ts`

**Environment:**

```text
MATOU_CONTROL_ENDPOINT=<private socket or pipe>
MATOU_CONTROL_TOKEN=<run-bound token>
MATOU_CONTROL_PROTOCOL=1
MATOU_CONTROL_CALLER_SESSION=<session id>
MATOU_CONTROL_CALLER_RUN=<run id>
MATOU_CONTROL_ASSET_ROOT=<packaged/private resource root>
MATOU_CONTROL_NODE_EXECUTABLE=<Electron executable used with ELECTRON_RUN_AS_NODE=1>
PATH=<MATOU_CONTROL_ASSET_ROOT/bin>:<inherited PATH>
```

**Lifecycle:**

- RuntimeHost 显式传入资源根：开发环境为 `apps/runtime/dist/control-assets`，安装包为 `resources/runtime/control-assets`。
- 每次 spawn 都创建 runId；Shell 不再复用 sessionId 作为 token 生命周期。
- Shell/Claude/Codex 全部签发仅含 `host.identify/list` 与 terminal read/send scopes 的 token。
- Session exit、替换、dispose、Runtime shutdown 均撤销 runId；Runtime generation 重启天然清空旧 token。

**Steps:**

- [x] 写 RuntimeServer 失败测试：三种 profile 都注入 CLI/identity；token caller 与 session/run 匹配；无 task write/move scope。
- [x] 写失败测试覆盖新建、resume、load、fork、retry、runtime restart；每次 run token 不同，旧 token 失效。
- [x] 写 RuntimeHost 失败测试：开发与 packaged resource root 传入 utility process env。
- [x] 运行 `pnpm --filter @matou/runtime test -- runtime-server.test.ts pty-session.test.ts && pnpm --filter @matou/desktop test -- runtime-host.test.ts`，确认失败。
- [x] 实现所有 profile 的统一 runId、token 和 PATH 注入；Windows PATH key 使用大小写兼容查找，wrapper 路径用平台分隔符。
- [x] 在所有退出/替换分支验证 revoke；把重复清理收敛为一个私有 helper，避免漏掉 retry/fallback。
- [x] 重跑相关测试和两包 typecheck。
- [x] 提交：`git add apps/runtime/src/runtime-server* apps/runtime/src/session/pty-session* apps/desktop/src/main/runtime-host* apps/desktop/src/main/index.ts && git commit -m "feat(runtime): inject mt into every managed session"`。

## Task 7: 打包跨平台 wrapper 与自然语言规则资产

**Files:**
- Create: `apps/runtime/control-assets/bin/mt`
- Create: `apps/runtime/control-assets/bin/mt.cmd`
- Create: `apps/runtime/control-assets/providers/host-control.md`
- Create: `apps/runtime/control-assets/providers/claude-plugin/.claude-plugin/plugin.json`
- Create: `apps/runtime/control-assets/providers/claude-plugin/skills/mt-terminal/SKILL.md`
- Create: `apps/runtime/control-assets/providers/claude-plugin/skills/mt-terminal/references/target-resolution.md`
- Create: `apps/runtime/control-assets/providers/claude-plugin/skills/mt-terminal/references/commands.md`
- Create: `apps/runtime/control-assets/providers/codex-developer-instructions.md`
- Create: `tooling/prepare-runtime-control-assets.mjs`
- Create: `tooling/prepare-runtime-control-assets.test.mjs`
- Modify: `tooling/prepare-package-resources.mjs`
- Modify: `apps/runtime/package.json`

**Provider rules must include:**

- `identify → list → resolve → act`；优先元信息，必要时短读候选。
- 左右/第 N 个只在当前 DAG level；父子使用 relation；明确名字才跨窗口。
- 多候选最多展示 5 个并反问；超过 5 个要求补条件。
- 连续“它/那个”复用上一目标，明确“另一个/左边/换成”重新解析。
- 发送前确认目标和内容；发送后以自然语言确认；Host Control 错误转成中文下一步。
- 首期能力边界，不暗示 create/fork/close/focus 等不存在的命令。

**Steps:**

- [x] 参考 Kooky `kc-terminal` 完整 skill 和 references，先写资源测试：所有命令、目标解析、歧义、连续指代、错误映射、非目标均出现。
- [x] wrapper 使用 `MATOU_CONTROL_NODE_EXECUTABLE` 并设置 `ELECTRON_RUN_AS_NODE=1` 执行 `mt-cli.cjs`，不调用用户 PATH 中的 node；Unix/Windows 都从 wrapper 自身目录解析 `../../mt-cli.cjs`。
- [x] Claude plugin manifest 和 skill 使用 `mt` 命令；Codex developer instructions 与 Claude 规则语义一致。
- [x] `prepare-runtime-control-assets.mjs` 在每次 runtime build 后把源资产复制到 `dist/control-assets` 并 chmod Unix wrapper 0755；`apps/runtime/package.json` 的 build 串接该脚本，保证开发与安装包使用同一资产树。
- [x] `prepare-package-resources.mjs` 继续复制整个 runtime dist，并新增最终产物断言，避免 CLI 或 provider 资产漏包。
- [x] 运行 `node --test tooling/prepare-runtime-control-assets.test.mjs`、`pnpm --filter @matou/runtime build` 和 `node tooling/prepare-package-resources.mjs`，核对输出树。
- [x] 在干净临时 HOME/PATH 下调用 packaged wrapper `mt --help`，确认不依赖全局 Node/npm。
- [x] 提交：`git add apps/runtime/control-assets apps/runtime/package.json tooling/prepare-runtime-control-assets* tooling/prepare-package-resources.mjs && git commit -m "feat(package): bundle mt and provider guidance"`。

## Task 8: 将 Claude Code 与 Codex 会话级规则接入启动参数

**Files:**
- Modify: `apps/runtime/src/session/provider-launch-plan.ts`
- Modify: `apps/runtime/src/session/provider-launch-plan.test.ts`
- Modify: `apps/runtime/src/session/pty-session.ts`
- Modify: `apps/runtime/src/session/pty-session.test.ts`
- Modify: `apps/runtime/src/runtime-server.ts`
- Modify: `apps/runtime/src/runtime-server.test.ts`

**Launch behavior:**

- Claude Code 添加 `--plugin-dir <assetRoot/providers/claude-plugin>`；继续保留现有 `--settings` hooks、`--resume`、`--fork-session`、permission 参数及其顺序。
- Codex 添加 `-c developer_instructions=<host-control.md 内容>`；resume 子命令和 provider session id 保持有效，permission 参数保持原语义。
- Shell 不加载 provider instructions。
- 内容以参数/受管资源在当前进程生效，不落盘到用户配置目录。

**Steps:**

- [x] 写 provider launch 失败测试：Claude new/resume/fork 和 Codex new/resume 均包含正确会话级规则；Shell 参数完全不变。
- [x] 写失败测试：路径含空格/Unicode 时仍作为单个 argv；规则文本含换行/引号时 Codex `-c` 值是合法 TOML 字符串。
- [x] 写 RuntimeServer 失败测试：只给 Claude/Codex 传 provider instruction options；恢复权限参数不受影响。
- [x] 运行相关测试确认失败。
- [x] 扩展 `PtyCommandInput`，实现 provider-specific argv builder；Codex 使用官方支持的 `developer_instructions` config override，不替换内建 model instructions。
- [x] 重跑 provider/runtime/pty 测试和 typecheck。
- [x] 提交：`git add apps/runtime/src/session/provider-launch-plan* apps/runtime/src/session/pty-session* apps/runtime/src/runtime-server* && git commit -m "feat(provider): teach Claude and Codex to use mt"`。

## Task 9: 增加真实终端与跨窗口产品验收

**Files:**
- Create: `tests/e2e/ai-host-control-cli.spec.ts`
- Create: `tests/fixtures/fake-ai-host-provider.mjs`
- Modify: `package.json`

**Scenarios:**

1. Shell/Claude/Codex 中 `mt identify` 均成功，外部测试进程无 token 时被拒。
2. 当前层有父节点、左右 sibling、屏外 sibling 和 detached child 时，`mt list` ordinal 与卡片/DAG 顺序一致。
3. 从 child 读取 parent，从当前卡片读取 left/right；跨窗口同名返回多候选，不静默执行。
4. `mt read` 返回最新屏幕；Renderer 滚到历史后结果不变；`mt history` 可看到更早输出；`mt commands` 忽略初始化命令并保留拼写错误。
5. `mt send right 'printf target' --enter` 只改变目标 PTY；当前焦点、carousel scrollLeft、窗口前台状态、通知数量和呼吸边框状态不变。
6. 并发发送两条长文本不交叉；全部 allowlist key 有效，未知 key 返回 `UNSUPPORTED`。
7. 替换/退出 SessionRun 后旧 token 失效；重启 Runtime 后所有旧 token 失效，新 token 正常。
8. Claude/Codex fake provider 捕获到会话级规则；Shell argv 中不存在 provider rule 参数。

**Steps:**

- [ ] 先编写 E2E 并加入 `test:e2e`，运行单文件确认失败点与实现缺口一致。
- [ ] 只修复前八个任务遗留的真实集成问题，不在 E2E 内加入 product-only test hook 绕过控制面。
- [ ] 运行 `pnpm build && pnpm exec playwright test tests/e2e/ai-host-control-cli.spec.ts`。
- [ ] 提交：`git add tests/e2e/ai-host-control-cli.spec.ts tests/fixtures/fake-ai-host-provider.mjs package.json && git commit -m "test(e2e): verify AI host control flows"`。

## Task 10: 完成安装包与回归验收

**Files:**
- Modify: `tests/e2e/packaged-runtime.spec.ts`
- Modify: `docs/superpowers/specs/2026-09-01-ai-host-control-cli-design.md`
- Modify: `docs/superpowers/plans/2026-09-01-ai-host-control-cli-implementation.md`

**Steps:**

- [ ] 在 packaged test 增加检查：App resources 含 `mt-cli.cjs`、Unix/Windows wrapper、Claude plugin、Codex instructions；Unix wrapper 可执行。
- [ ] 启动 packaged App，新建 Shell/Claude/Codex，逐一运行 `mt identify --json`；确认 endpoint/token 不显示在默认 UI。
- [ ] 运行定向单测：`pnpm --filter @matou/runtime test -- host-control terminal-screen terminal-input mt-cli provider-launch runtime-server`。
- [ ] 运行 `pnpm typecheck`、`pnpm build`、`pnpm exec playwright test tests/e2e/ai-host-control-cli.spec.ts tests/e2e/terminal-channel.spec.ts tests/e2e/session-canvas-navigation.spec.ts`。
- [ ] 运行 `pnpm test:packaged`；若耗时环境使完整 packaged E2E 未完成，至少保存确切失败命令和日志，不把静态检查表述为安装包已验证。
- [ ] 运行 `git diff --check`；检查 `git status --short`，区分本功能文件与进入任务前已有改动。
- [ ] 更新设计状态为“已实现”，在本计划逐项勾选实际完成项，并记录验证命令与结果。
- [ ] 只暂存本功能文件，运行 `git diff --cached --name-only` 后提交：`git commit -m "feat: ship Matou AI host control CLI"`。

## Definition of Done

- [ ] 用户在任一 Matou 托管终端运行 `mt --help`，无需额外安装或配置。
- [ ] Claude Code 和 Codex 可根据自然语言稳定解析当前层级、父子与跨窗口明确目标。
- [ ] `read/history/commands` 三种读取语义在自动化测试中彼此区分。
- [ ] 输入不改变 Matou UI 状态，不触发通知，并在目标 PTY 上按动作串行。
- [ ] Shell/Claude/Codex token 全部绑定到独立 SessionRun，退出与 Runtime 重启后失效。
- [ ] App 安装包包含跨平台 wrapper 与 provider 规则，干净环境 smoke test 通过。
- [ ] 首期协议中不存在 create/fork/remove/close/focus/view capability。

## 实施记录（2026-09-01）

- Tasks 1–8 已完成并按任务拆分提交；Runtime、Desktop 全量单测通过（Runtime 66 文件 / 422 项，Desktop 38 文件 / 297 项）。
- 新增真实 App 验收已通过：Shell 内 `mt identify`、向右侧同级发送文本、读取右侧最新屏幕，并确认焦点、横向位置和通知计数保持不变。
- `pnpm typecheck`、`pnpm build` 以及 `ai-host-control-cli + terminal-channel + session-canvas-navigation` 共 16 项 E2E 已通过。
- runtime 产物和安装包资源已核对包含 `mt-cli.cjs`、两套 wrapper、Claude plugin、Codex instructions；Unix wrapper 在干净 HOME/PATH 下可直接显示帮助。
- packaged App 已通过资源存在性检查和真实 Shell 内的 `mt identify --json`。完整 packaged 回归继续执行到独立窗口关闭场景时，既有断言期望会话停止，但实测主窗口仍保留该终端；Host Control 验收在此之前已通过，保留确切失败位置 `tests/e2e/packaged-runtime.spec.ts:136` 供后续定位。
