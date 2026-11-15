# Codex 第三关交付记录

2026-11-15，分支 `feat/multi-agent-codex`。范围为 HUD、自动标题、AGENTS.md；载入、Fork 和重启恢复仍关闭。本记录用于第三关审核，不构成整个 PRD 的产品签收。

## 真实样本结论

详见 [交互探针](codex-stage3-probe.md)。真实 HOME、交互式 `codex-cli 0.162.1` 三轮：

- 标题索引确实生成 `thread_name`，TUI 的 Thread name 与之相同；没有手动命名。三轮索引记录的更新时间未变化。
- 两次要求 `update_plan` 均报告工具不可用，rollout 中零调用；没有结构化计划样本。本关不解析文本待办，Codex HUD 隐藏待办。
- 第三轮最近 `last_token_usage.total_tokens = 34175`，窗口 `258400`；指定公式约为已用 13.226% / 剩余 86.774%，HUD 四舍五入显示已用 13%。TUI 显示剩余 91%，差 4.226 个百分点。没有猜测或反推 TUI 算法。
- 临时目录信任获监督批准后执行；退出后核对配置差异，仅有新增信任条目。`config.toml` 已恢复为备份，diff 为空；`hooks.json` 未变。

## 已修改

| 产品文件 | 用户可见结果 / 作用 |
| --- | --- |
| `packages/domain/src/agent-manifest.ts` | Codex 开自动标题；指令文件只用 AGENTS.md；隐藏额度、账户、待办、MCP/子代理和未经确认的 hooks 计数。其它关卡开关继续关闭。 |
| `apps/runtime/src/session/provider-transcript-hud.ts` | 复用增量流读取，解析最新模型/档位和最近请求用量；压缩或缺少有效样本时隐藏比例；只计稳定 ID 的 CommandExecution，忽略外层 exec 包装。 |
| `apps/runtime/src/session/session-hud-registry.ts` | 实时工具不被空历史覆盖；按执行 ID 合并去重；Codex 缺失数据不回落为 Claude 模型或虚构计数；只探测项目 AGENTS.md。 |
| `apps/runtime/src/runtime-server.ts` | 利用已有可见 HUD 刷新，补读 Stop 后迟到的 rollout 数据，保留 run 所有权复核。 |
| `apps/runtime/src/session/codex-session-title.ts` | 只读 session_index，UUID 精确匹配并按 updated_at 取最新非空 thread_name。 |
| `apps/runtime/src/session/provider-hook-server.ts` | 复用现有标题观察和有界监听，覆盖索引晚到、创建和替换；保留旧 run/retirement/身份校验。 |
| `apps/desktop/src/renderer/src/hud/TerminalHud.tsx` | Codex 显示实际模型与已观察档位；缺数据不显示等待占位、Claude 模型回退或额度。沿用原布局、样式和编辑弹窗。 |
| `apps/desktop/src/renderer/src/hierarchy/hierarchy-types.ts` | 接收已有 wire 中的可选 effort 字段。 |

对应新增或扩展测试：domain 的 `agent-manifest.test.ts`；runtime 的 `provider-transcript-hud.test.ts`、`session-hud-registry.test.ts`、`provider-hook-server.test.ts`、`codex-session-title.test.ts`、`runtime-rpc-router.test.ts`；desktop 的 `TerminalHud.test.tsx`、`TerminalPane.test.tsx`；`tests/e2e/codex-stage3-hud.spec.ts`。Claude / Antigravity 原有断言未改。

标题手动锁与恢复复用已有数据库字段、仓库方法、RPC、菜单和文案；没有新增存储、依赖、运行时 LLM 请求或平行 HUD 管线。

## 自动验证

- 5 个定点 runtime 文件：**116 条通过**。
- 额外 `runtime-server.test.ts`：首次与其他文件并行执行，151 条中 149 条通过；两处 Shell/PTY 时序检查失败（Shell 状态等待超时、reattach 前尚未获得 PID）。不改断言，独立复跑这两条：**2 条通过**。保留首轮失败日志，未把首次整文件运行描述为全绿。
- 3 个 desktop 文件（含 locale、TerminalPane）：**110 条通过**。
- domain manifest：**5 条通过**。
- `pnpm typecheck`、`pnpm check:identifiers`、`git diff --check` 通过。
- `pnpm build:runtime && pnpm build:desktop` 通过，仅供此次窗口冒烟；未制作安装包。
- Electron + PTY + HUD 的受控服务冒烟：**1 项通过，5.7 秒**。首次测试选错 header 选择器而超时，改为真实的 `.terminal-pane-header` 后通过；该修正未改产品代码。首次执行同时发生了现有 Electron 包的二进制补下载。

首次定点检查还出现了工作区 package dist 未更新、测试漏用已有 waitFor、前端可选 effort 类型遗漏以及测试用 undefined 违反 exactOptionalPropertyTypes，均已修正；一条增量读取测试错误地把文件覆盖得更长，修为真实 append-only 输入后通过。没有放宽断言或产品校验。

完整日志位于监督目录 `/Users/icesword/.claude/agent-foreman/runs/matou-多Agent-codex-01/logs/`：`stage3-runtime-tests.log`、`stage3-runtime-recheck.log`、`stage3-desktop-tests.log`、`stage3-typecheck.log`、`stage3-ui-smoke.log`、`stage3-ui-smoke-recheck.log`。

## 界面与行为验证

Terra 与主代理均实际查看本轮三图；[窗口冒烟报告](evidence/codex-stage3/ui-smoke.md) 区分了截图可见内容和自动断言。服务使用确定性 Codex fixture，窗口、PTY、hook、runtime、RPC 和文件保存路径均为真实 Matou 实现。

| 场景 | 结果 | 证据 |
| --- | --- | --- |
| HUD 模型、档位、最近用量、工具去重与缺失隐藏 | 通过；显示 gpt-6-astra (medium)、13%、1 Tools；无额度、Claude 账户或待办 | [HUD](evidence/codex-stage3/hud.png)；冒烟断言 |
| 自动标题晚到 | 通过；启动后创建索引，卡片更新为索引标题 | HUD 截图；hook 集成测试 |
| 手动标题锁与恢复 | 通过；索引更新不覆盖手动名，菜单恢复最新自动标题 | [恢复后](evidence/codex-stage3/restored-title.png)；冒烟断言及仓库集成测试 |
| AGENTS.md 编辑与保存 | 通过；保存后回读文件一致，CLAUDE.md 保持原内容 | [编辑器](evidence/codex-stage3/agents-editor.png)；RPC / UI / 冒烟测试 |
| 压缩、缺样本、旧 run、索引损坏/缺失 | 定点测试通过 | reader / registry / hook / title 单测；不冒充真实压缩 UI 验收 |

## reference product 交互对照矩阵（阶段性）

本轮没有重新操作原始 reference product，也没有声称以下比对已闭合。比对边界仍仅为 CLI 模块；截图中的其它 Matou 产品外壳不作为参考产品差异结论。

| 场景 | reference product 可运行基线 / 代码证据 | Matou 本关结果 | 差异结论 |
| --- | --- | --- | --- |
| HUD 信息与缺失隐藏 | 本轮未新增原始产品实测证据 | 上表受控窗口和单测通过 | 参考产品逐项对照未闭合 |
| 标题手动锁、恢复入口和反馈 | 本轮未新增原始产品实测证据 | 上表受控窗口和集成测试通过 | 参考产品逐项对照未闭合 |
| 指令文件编辑保存、焦点和异常状态 | 本轮未新增原始产品实测证据 | 编辑保存和回读通过；全部焦点/异常路径未逐项比对 | 参考产品逐项对照未闭合 |

## 自主决定与未闭合项

主代理：复用已有 reader、标题 watcher、RPC 和可见 HUD 刷新；索引监听沿用现有 10 秒有界窗口，每次 hook 再读并重新建立已过期监听，目录监听支持第一次创建和原子替换。未增加全局扫描或常驻轮询。采用实际模型标识原样展示，档位只拼接已观察值，不猜测显示名。受控 UI 冒烟用于验证 Matou 的窗口与文件交互，与真实 Codex 样本分开记录。

Terra：跳过探针升级提示；用 `/status` 获取有文字标识的模型/上下文；两次无计划调用后停止重试；保留上下文偏差；窗口冒烟保留首轮失败日志并检查本轮三张截图。这些决定已由主代理复核。

未闭合：真实三项 update_plan 载荷；TUI 上下文百分比内部算法；真实 Codex 接入 Matou 的端到端与安装包验收；原始 reference product 全量交互矩阵。未验证长时间迟到标题、真实压缩场景、全部窗口尺寸与负载性能。载入、Fork、重启恢复留在后续关卡，本关停止。
