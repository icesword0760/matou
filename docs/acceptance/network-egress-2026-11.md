# 验收记录：网络出口检测与出口保护

| 字段 | 值 |
|---|---|
| 日期 | 2026-11-13 |
| 分支 | `feat/network-egress` |
| PRD | `docs/prd/PRD-Matou-网络出口检测.md` |
| 计划 | `docs/superpowers/plans/2026-11-13-network-egress-protection.md` |
| 对照基线 | reference product 没有此能力，按 PRD 第 7 节场景验收 |
| 证据目录 | `docs/acceptance/evidence/network-egress-2026-11/` |

## 验收方式

- **真实网络：** 隔离数据目录启动构建版 Matou，走本机真实网络（Clash TUN、美国节点），假 Claude 打印自身环境变量。
- **异常场景：** 本机无法关闭 Clash 来制造大陆出口（会切断整机网络），服务商 trace 是 HTTPS，也无法本地伪造。因此用测试开关 `MATOU_E2E_EGRESS_FIXTURE` 让 runtime 的出口采样改读本地 JSON，在真实启动的 App 中切换「美国 / 大陆」。生产环境不设该变量，单测证明不设置时走真实网络。
- **逻辑分支：** 由 runtime 单元测试覆盖（evaluator、monitor 状态机、pauser、sampler、runtime-server 接线）。

## 场景矩阵

| # | 场景 | 预期 | 实际 | 证据 | 结论 |
|---|---|---|---|---|---|
| 1 | 代理正常（美国节点），开 Claude 会话 | HUD `🇺🇸 美国 · 出口正常`，启动无额外等待 | 真实网络下显示 `🇺🇸 美国 · 出口正常`；启动到首个终端可输入 2.8 s，检测在后台进行 | `01-normal.png`；真实网络手动截图 | 通过 |
| 2 | 出口变为大陆 | 横幅、通知一次、新开官方会话出生即冻结 | 横幅「出口异常（检测到中国大陆 IP），已暂停。网络恢复后自动继续。」，HUD「出口异常 · 已暂停 1 个会话」，进程状态 `T`，键入不送达；出生即冻结由 runtime-server 单测覆盖 | `02-frozen.png`；`network-egress.spec.ts` b 段 | 通过 |
| 3 | 规则模式把 `api.anthropic.com` 直连 | 只冻结 Claude，Codex 不受影响 | 按服务商分别冻结由 monitor 单测覆盖（anthropic 为 CN 时只冻结 anthropic 会话） | `egress-monitor.test.ts` | 通过（单测） |
| 3b | 登录与 API 走不同海外节点 | W0 提醒，不冻结 | evaluator 单测覆盖 | `egress-evaluator.test.ts` | 通过（单测） |
| 4 | 只开系统代理，无 TUN、无环境变量 | K1 冻结 + W1 修复建议 | W1 判定由 evaluator 单测覆盖；面板文案已核对 | `egress-evaluator.test.ts`；`EgressPanel.test.tsx` | 通过（单测） |
| 5 | 冻结中网络恢复 | 约 20 s 自动继续，任务不丢 | 夹具改回美国后自动出现「出口已恢复，已自动继续」并消失；冻结期间键入未被补发，恢复后键入正常回显 | `03-resumed.png`；spec c 段 | 通过 |
| 5b | 节点抖动 | 不反复冻结、解冻 | 连续两次通过才继续，由 monitor 单测覆盖 | `egress-monitor.test.ts` | 通过（单测） |
| 6 | 冻结中点「仍要继续」 | 二次确认后解冻，HUD「已手动继续」，出口再变前不再冻结 | 确认框后解冻，HUD「出口异常 · 已手动继续」，键入回显；ip 变化后越过失效由单测覆盖 | `04-overridden.png`；spec d 段 | 通过 |
| 7 | 运行中换国家（同一国家规则） | 冻结；「以当前出口为准」后恢复 | monitor 单测覆盖 | `egress-monitor.test.ts` | 通过（单测） |
| 8 | 断网 / 检测全部超时 | 黄色「无法确认出口」，不冻结 | monitor 与 evaluator 单测覆盖；HUD 文案由组件测试覆盖 | 单测 | 通过（单测） |
| 9 | 第三方服务商 Claude 会话、Shell 卡片 | 不冻结、不拦截、不显示标签 | Shell：spec 中 Shell 卡片冻结期间 `echo ok` 正常、无出口标签；第三方服务商不登记由 runtime-server 单测覆盖 | `02-frozen.png`；`runtime-server.test.ts` | 通过 |
| 10 | 海外用户、无代理 | 无任何提示 | 时区策略对非大陆系统时区不替换（单测）；出口非大陆时状态正常 | `time-zone-policy.test.ts` | 通过（单测） |
| 11 | 系统时区上海、出口美国 | 不冻结；会话时区为新加坡 | 真实网络与 spec 均显示 Claude 会话 `TZ=Asia/Singapore`，Shell 保持 `Asia/Shanghai`，W3 通过 | `01-normal.png`；真实网络手动截图 | 通过 |
| 12 | 背压暂停与出口冻结叠加 | 互不误解冻 | 真实子进程测试：背压恢复后进程仍为 `T` | `pty-session.test.ts` | 通过（单测） |
| 13 | 关闭「出口保护」 | 只显示状态，不冻结 | 面板关闭保护后，大陆出口不再冻结 | `05-protection-off.png`；spec e 段 | 通过 |
| 14 | 睡眠唤醒、切换 Wi-Fi | 立即复检 | 网络签名变化、定时器漂移 > 15 s 触发完整检查，由 monitor 单测覆盖 | `egress-monitor.test.ts` | 通过（单测） |
| 15 | 上海时区新开 Claude 会话 | `TZ=Asia/Singapore`，语言不变，Shell 不变 | 同 11 | 同 11 | 通过 |
| 15b | 出口在台湾 | `TZ=Asia/Taipei` | 时区策略单测覆盖 | `time-zone-policy.test.ts` | 通过（单测） |
| 16 | 时区选「跟随系统」 | 恢复本机时区，W3 提醒 | `claude-settings` 单测与 `claude-time-zone.spec.ts` 覆盖设置与持久化 | 单测 + e2e | 通过 |
| 17 | 第三方服务商会话、海外用户 | 不改时区 | `claude-settings` 单测（`official:false`）与策略单测覆盖 | 单测 | 通过（单测） |

## 验证中发现并已修复的问题

1. **只用 Codex 时漏检（监控）**：轻量采样原先只查 Anthropic，OpenAI 线路回落大陆时不会触发完整检查。改为对有会话的每个服务商分别采样。（`fix(egress): sample every active provider in light checks`）
2. **登录页人机验证导致常黄（评估）**：`login-api-match` 为未知时不再拉低状态。（`fix(egress): ignore unknown login match in exit state`）
3. **多卡片重复请求与重渲染（界面性能）**：出口状态改为全局共享缓存，20 张卡片只请求 1 次，无关服务商的变化不重渲染卡片。（`perf(egress): share egress status across panes`）
4. **切换卡片后 HUD 标签不跟随（界面）**：端到端测试发现，从 Claude 卡片切到 Shell 卡片时仍显示旧标签。已修复并补测试。（`fix(egress): reselect egress chip when the focused session changes`）

## 未覆盖 / 已知限制

- **Codex 的端到端流程**：e2e 只用了假 Claude；Codex 的登记、冻结与时区由单测覆盖。界面目前没有直接新建 Codex 卡片的入口，在 Shell 里手动输入 `codex` 不受保护，待多 Agent「Codex 补齐」期解决。
- **Antigravity**：2026-11-15 已扩展原生 `antigravity` profile 的 `google` 类别，见下方追加验收；真实 Google 看到的出口仍不能保证。
- **Windows**：冻结只停输出读取、阻止输入，未在 Windows 实机验证。
- **登录浏览器时区**：系统浏览器隐私窗口仍使用本机时区，能否跟随未实测（PRD 第 8 节）。
- **性能**：启动到首个终端 2.8 s（真实网络、检测在后台），未与改动前做严格 A/B；冻结与解冻 20 个会话的耗时未单独测量。
- **采样性质**：出口检测为周期抽查，最长约 1 分钟窗口内无法保证每条连接都走同一出口（面板已注明）。

## 自动化结果

合并后在 `feat/network-egress` 上运行：

- `pnpm test`：全部通过。contracts 71、domain 7、desktop 887 + 性能 3、runtime 1254（1 跳过）、runtime-server 144、journal 9。
- `pnpm typecheck`、`pnpm build`、`pnpm check:identifiers`：通过。
- `network-egress.spec.ts`、`claude-time-zone.spec.ts`、`prd-02-bottom-hud.spec.ts`：5/5 通过（亲自复跑）。
- 全量 `pnpm test:e2e`：93 通过、40 失败、1 跳过。逐项对照改动前的 `main`（`c78f0d1`）后，**没有本次改动引入的回归**：
  - 账户 3 组（25 项）：新 worktree 缺 `tooling/claude-accounts/.venv`。接上后 `claude-accounts`、`claude-accounts-refresh` 全部通过；`claude-accounts-hud-restart` 的 7 项在 `main` 上同样失败（期望「工作账户」，实际为「Claude 账户」），属既有问题。
  - 会话恢复、画布、分叉、worktree、窗口焦点等 9 个文件：同一批测试在 `main` 上 17 项失败，本分支 18 项，失败集合基本重合。仅有的两项差异（`prd-04:60` 焦点恢复、`prd-05-detached:50` 剪贴板复制）各重复 3 次，`main` 与本分支表现一致（均为时过时不过），原因是本机前台有正在使用的 Matou 窗口抢焦点。

## 同步 main（订阅账号功能合入后）

main 在验收期间合入了「订阅账号接入 Claude Code」（`e731b50`）。本分支已同步 main：3 处冲突都是两边新增内容，保留双方即可。同步后：

- `pnpm typecheck`、`pnpm build` 通过。
- `pnpm test` 全部通过：desktop 943、runtime 1420（35 跳过）、runtime-server 144。
- `network-egress`、`claude-time-zone`、`prd-02-bottom-hud`、`provider-onboarding` e2e：首轮 `network-egress` 失败 1 次，随后单独运行 1 次、连同 provider-onboarding 连续运行 3 次，均通过。属于本机前台窗口抢焦点导致的偶发失败。
- 订阅账号的模型路由只把登录 shell 里的代理变量用于路由自身的出站流量，agent 会话的环境不变，因此「探测走 agent 同一路径」仍然成立。
- 已知缺口：订阅账号驱动的 Claude 会话（`ANTHROPIC_BASE_URL` 指向本机路由）目前按第三方服务商处理，不检测、不冻结、不换时区。是否纳入保护待产品决定。

## 订阅账号会话（PRD 第 9 节，方案 B）

- **归类修正**：订阅会话不再被误判为 Anthropic 官方会话。ChatGPT 订阅 → `chatgpt-subscription` 类别，并隐藏时区；Antigravity 订阅 → 只隐藏时区，不冻结；第三方 API 服务商和官方会话的行为不变。由 `egressPlanForLaunch` 单测与 runtime-server 单测覆盖。
- **探测线路**：`chatgpt.com` / `auth.openai.com` 按 `resolveRouterProxy` 的结果选路（登录 shell 代理 → 系统代理 → 直连），与路由的真实出站路径一致。未配置 ChatGPT 订阅时不探测、不显示该分组、不影响整体状态。
- **时区**：订阅路由的 `clearEnv` 不包含 `TZ`，注入的东八区时区会保留。
- **结果**：typecheck、build 通过；desktop 944、runtime 主体 1429 项单测通过；`network-egress.spec.ts` 回归连续 2 次通过。
- **runtime-server.test.ts**：本分支每次运行有 1–2 项随机失败，且每次失败的测试不同（Shell 命令与工作目录时序类）。同时段在 `main`（`6132501`）上运行 2 次，分别有 7 项和 6 项随机失败。判定为本机负载下的既有时序不稳定，不是本次改动引入。
- **e2e 中的单字符混入**：执行者首两次运行时，Claude 终端出现单个 `1` 或 `(`。冻结期间测试敲入的是整串 `FROZEN_SHOULD_NOT_ECHO`，从未泄漏。单字符来自测试窗口在主屏抢焦点时的外部键盘输入，不是缺陷。


## Google 原生 Antigravity 会话（2026-11-15）

范围为 `feat/login-browser-integration` 上的原生 `antigravity` profile；Claude Code 的 Antigravity 订阅分支仍只隐藏时区。Google 没有可用 trace，通用出口取自 `www.cloudflare.com/cdn-cgi/trace`，登录探测目标为 `accounts.google.com`，无有效 trace 时显示未知。该近似不能保证 Google 实际看到的出口；代理按域名分流时可能不同。

### reference product 交互对照矩阵

reference product 没有此能力；按已确认 PRD 第 10 节和 `docs/mockups/login-browser-integration.html` 验收。检查详情依原始交代复用现有通用文案，不新增 Google 专用检查术语。

| 场景 | PRD / Mockup 基线 | Matou 结果与证据 | 结论 |
|---|---|---|---|
| Google 注册与启动计划 | `family=google`、`exitFamily=google`、`hideTimeZone=true` | `egress.test.ts`、`egress-monitor.test.ts` 启动计划测试 | 通过（自动测试） |
| 通用 api 为 CN、登录未知 | mainland 异常，暂停 Google 会话，Claude/Codex 不受影响 | evaluator CN 测试、monitor 隔离冻结测试 | 通过（自动测试） |
| 已保存国家 / IP 规则被违反 | rule 异常 | evaluator 参数化规则测试 | 通过（自动测试） |
| 登录未知 / 已知海外出口不同 | unknown 不拉低整体正常状态；不一致仅提醒 | evaluator unknown 与 mismatch 测试 | 通过（自动测试） |
| 异常中新开会话、网络恢复 | 出生即冻结；15 秒复检 + 5 秒确认后自动继续 | monitor Google 冻结恢复测试 | 通过（自动测试） |
| 全量 / 轻量采样 | 全量包括 Google api/login；活跃 Google 每分钟轻量只请求通用 api trace | sampler 全量目标、请求 URL 与 monitor 活跃计时测试 | 通过（自动测试） |
| 原生启动、时区与真实 PTY | 注册 Google，上海时区依现有策略替换（TW 取台北），异常调用现有冻结 | runtime-server 原生 antigravity 接线测试 | 通过（自动测试） |
| HUD 与面板 | 复用 HUD 状态标签；Google 组在订阅组之后、本机之前；标题中英文准确；底部限制句准确 | `EgressChip.test.tsx`、`EgressPanel.test.tsx`；真实界面证据补在下文 | 通过（组件测试 + 真实界面） |

### 新增测试名

- `registers antigravity launches as google`
- `protects antigravity with Google exit and existing time-zone policy`
- `marks Google CN api abnormal with mainland violation`
- `does not lower Google or overall status for unknown login`
- `enforces saved Google %s rule`（country / ip）
- `only warns when Google login and api exits differ`
- `freezes only Google sessions and auto-resumes after the existing recovery confirmations`
- `light-samples an active Google session on the existing one-minute timer`
- `light sample requests only the generic api trace through the agent proxy`
- `reads Google api and unknown login from the existing e2e fixture`
- `registers native antigravity with Google protection, exit TZ and actual PTY freeze`
- `applies the existing automatic time-zone policy to native Antigravity without provider configuration`
- `renders the shared HUD exit label for a registered Google session and opens its panel`
- `renders Google heading, unknown login and sampling limit in %s`（zh-CN / en）

### 本次验证结果

- `pnpm typecheck`、`pnpm build`：时区修正及最终枚举文案完成后已重新完整通过。
- `pnpm test`：最终完整命令退出码 0。domain 12、contracts 71、desktop 1102（1 跳过）+ 性能 3、runtime 主体 1529（36 跳过）、runtime-server 151、journal 9 全部通过。最终未开启重试；此前失败与复跑记录见下节。
- `pnpm check:identifiers`：最终通过。`git diff --check` 通过。
- 运行逻辑定点：evaluator / monitor / sampler 共 60 项通过；修正后的原生 Antigravity 真实 PTY 启动、台湾出口时区和冻结测试通过。

真实 Electron 界面使用独立数据目录和假 agent，经 `mt create session --canvas current --profile antigravity --enter && mt focus right` 创建原生 Antigravity 会话，`data-profile=antigravity`。主代理已回读最终截图：

- [HUD](evidence/network-egress-google-2026-11/01-antigravity-hud.png)：当前会话 Antigravity，HUD `🇺🇸 美国 · 出口正常`；终端 `AG_TZ=Asia/Singapore`（宿主 `TZ=Asia/Shanghai`）。HUD 复用现有国家/状态文案，类别名显示在面板中。
- [Google 面板](evidence/network-egress-google-2026-11/02-google-panel.png)：Google 组位于 Codex 后、本机前（该实例未配置 ChatGPT 订阅）；标题 `Google（通用出口检测）`；api 显示 `美国 · 203.0.113.20`，登录项显示现有通用 `无法确认（检测超时）`。底部完整显示「Google 线路使用通用出口检测，不能保证 Google 实际看到的出口。」；无文字遮挡。
- [保护开关](evidence/network-egress-google-2026-11/03-protection-switch.png)：最终构建实际显示「出口保护：出口异常时暂停 Claude / Codex / Antigravity 会话」，底部 Google 限制句仍完整可见。
- [文字证据](evidence/network-egress-google-2026-11/evidence.txt)：记录会话 profile、HUD 和面板实际文本。中文实际 App 与英文组件测试分别验证；本轮真实界面未操作 CN 冻结/自动恢复，该闭环由 monitor 状态机和 runtime-server 真实 PTY 测试覆盖。

英文实际 App 本轮未操作，英文标题、开关与说明由组件测试验证。

界面验收临时数据保留在工作区 `output/` 下。因 macOS Unix socket 路径长度限制，验证使用工作区内较短目录；未碰其他 worktree 或用户现有应用。清理命令 `rm -rf` 被自动审批拒绝（禁止该类命令），因此保留数据。


### 用户确认的枚举文案收尾

真实界面核对发现保护开关原先只列出 Claude / Codex。用户在监督渠道选择 B，要求所有相关枚举文案明确包含 Antigravity。现已将开关改为「出口保护：出口异常时暂停 Claude / Codex / Antigravity 会话」/「Exit protection: pause Claude / Codex / Antigravity sessions when the exit is wrong」，并同步时区设置说明中的 agent 枚举。冻结横幅、恢复横幅和通知不枚举 agent，直接复用通用会话文案。出口 UI 与时区文案相关组件测试共 34 项通过。

### 本次发现并修复

首轮全量 `pnpm test` 中 runtime-server 150 通过、1 失败，失败项是新增原生 Antigravity 时区接线测试。单独重跑同一测试仍失败，确认不是本机抖动：`ClaudeSettings.environment` 因 `providerConfig=false` 提前排除了 Antigravity，使启动计划的 `hideTimeZone` 无法生效。最小修正仅对 `antigravity` 放行该守卫，继续调用现有 `agentTimeZone` 自动策略，不改变 manifest 能力、Claude Code / Codex、PTY 或进程组。修正后该真实 PTY 定点测试通过，并补自动时区策略单测；最终全量结果见上文。

时区修正后的一轮全量测试中，runtime-server 的 `tracks real Shell commands as running, idle, error, and interrupted work` 在等待 interrupted 状态时超时（其余 150 项通过）；单独重跑整个 runtime-server 后该项通过，失败点变成 `watches idle provider configuration and republishes changed HUD counts` 的监控回调超时（其余 150 项通过）。随后相关 Shell 定点批次 8/8 通过，文件监控单项 1/1 通过。两个失败项均未被本次修改，表现为不稳定的时序超时；本次未修改它们的等待阈值或启用测试重试。最后在隔离验收实例关闭后重新运行完整 `pnpm test`，runtime-server 151/151 与后续 journal 9/9 通过，完整命令退出码 0。相关原始日志保留为 `test-final.log`、`runtime-server-isolated-rerun.log`、`runtime-server-shell-targeted.log`、`runtime-server-watch-targeted.log`。

### 边界

- 不改判定权重、定时器、PTY 或进程组逻辑；Claude Code / Codex / 订阅启动分支保持原样。
- 未对真实 Google 服务端所见 IP、真实 Antigravity 模型请求、MCP 子进程网络路径或 Windows 实机做验证；假 agent 和出口 fixture 用于检查 Matou 的登记、UI 与暂停恢复闭环。
- Google 检查详情保留原有通用 unknown 文案；准确的近似限制在分组标题和面板底部明确说明。


### 本次改动文件

- `apps/desktop/src/renderer/src/egress/EgressChip.test.tsx`
- `apps/desktop/src/renderer/src/egress/EgressChip.tsx`
- `apps/desktop/src/renderer/src/egress/EgressPanel.test.tsx`
- `apps/desktop/src/renderer/src/egress/EgressPanel.tsx`
- `apps/desktop/src/renderer/src/egress/egress-copy.ts`
- `apps/desktop/src/renderer/src/hierarchy/ClaudeTimeZoneSettings.test.tsx`
- `apps/desktop/src/renderer/src/hierarchy/ClaudeTimeZoneSettings.tsx`
- `apps/runtime/src/egress/egress-evaluator.test.ts`
- `apps/runtime/src/egress/egress-evaluator.ts`
- `apps/runtime/src/egress/egress-monitor.test.ts`
- `apps/runtime/src/egress/egress-monitor.ts`
- `apps/runtime/src/egress/egress-sampler.test.ts`
- `apps/runtime/src/egress/egress-sampler.ts`
- `apps/runtime/src/runtime-server.test.ts`
- `apps/runtime/src/session/claude-settings.test.ts`
- `apps/runtime/src/session/claude-settings.ts`
- `docs/acceptance/network-egress-2026-11.md`
- `docs/prd/PRD-Matou-网络出口检测.md`
- `packages/contracts/src/egress.test.ts`
- `packages/contracts/src/egress.ts`
- `docs/acceptance/evidence/network-egress-google-2026-11/01-antigravity-hud.png`
- `docs/acceptance/evidence/network-egress-google-2026-11/02-google-panel.png`
- `docs/acceptance/evidence/network-egress-google-2026-11/03-protection-switch.png`
- `docs/acceptance/evidence/network-egress-google-2026-11/evidence.txt`


### 最终日志

日志目录：`/Users/icesword/.claude/agent-foreman/runs/Matou-Antigravity出口-04/logs/`。

- `typecheck-final-copy.log`、`build-final-copy.log`：最终源码完整类型检查与构建通过。
- `test-complete.log`：最终完整 `pnpm test` 通过，包含 runtime-server 151/151、journal 9/9。
- `identifiers-final.log`：最终标识检查通过。
- `desktop-final-copy.log`：枚举文案收尾后的出口 UI 和时区说明定点 34/34 通过。
