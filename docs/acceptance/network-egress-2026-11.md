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
- **Antigravity**：不在本期范围，接入后补 `google` 类别（PRD 第 8 节）。
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
