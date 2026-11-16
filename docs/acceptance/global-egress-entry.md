# 网络检查全局入口验收

日期：2026-11-16。分支：`feat/global-egress-entry`。

未登记出口类别的会话现在也能从底栏打开网络检查。官方会话继续显示原有国家、提醒、暂停或手动继续标签。面板、底栏位置、样式、冻结横幅和通知代码没有改动。

## 改动文件

- `apps/desktop/src/renderer/src/egress/EgressChip.tsx`：无 family 时使用全局状态文案；复用按钮、portal、面板、状态缓存与全局回调。
- `apps/desktop/src/renderer/src/egress/egress-copy.ts`：全局状态文案与所有 family 的暂停数、所有 warn/fail 的提醒数。
- `apps/desktop/src/renderer/src/egress/EgressChip.test.tsx`：25 项组件测试，包含原官方行为和新增全局入口。
- `tests/e2e/global-egress-entry.spec.ts`：真实 Electron 的 Shell 与官方入口验收及截图。
- `tests/e2e/network-egress.spec.ts`：将旧的 Shell 隐藏入口断言更新为全局入口。

## 自动检查

| 检查 | 结果 |
| --- | --- |
| `pnpm typecheck` | 通过 |
| `pnpm build` | 通过 |
| `pnpm test` | 通过；3088 项 Vitest 与 6 项 Node 测试通过，37 项既有跳过 |
| `pnpm check:identifiers` | 通过，提交前已单独执行 |
| `EgressChip.test.tsx` | 25 项通过 |
| `pnpm exec playwright test tests/e2e/global-egress-entry.spec.ts --workers=1` | 1 项通过，7.5 秒 |

首轮 `pnpm test` 因新工作区未生成 `apps/runtime/dist/control-assets/providers/antigravity-system-message.md` 失败；执行 `pnpm build` 生成既有资源后重跑，没有修改 runtime 源码。完整日志保存在监督运行目录的 `logs/`。

组件测试组为 `EgressChip` 与 `EgressChip global entry`；全局入口覆盖 Shell/DeepSeek/GLM/Kimi、检查中/正常/提醒/异常/未知、跨 family 暂停计数、warn/fail 全局计数与至少 1 项提醒、点击切换、Escape、外部点击、Tab/Enter、共享状态更新、四个全局回调、Mantle/客户端提醒/Google 分组、缺客户端/桥接及英文语言模式。

真实界面测试名：`opens the global network check from Shell and preserves the official session label`。

## 交互对照矩阵

基线：用户交代的状态文案和位置、用户确认的全局入口稿、改动前 `EgressChip`/`EgressPanel` 的官方会话行为。设计稿位于另一个 worktree，按监督答复仅作只读参考；实现沿用当前 egress-chip 样式，不新增按钮样式。本次核对底栏入口和原面板，其余应用外壳不参与差异判断。

| 场景 | 基线 | Matou 实际结果与证据 | 结论 |
| --- | --- | --- | --- |
| Shell 底栏 | 状态栏右侧，Claude 账户前 | [01-shell-entry.png](evidence/global-egress-entry/01-shell-entry.png)，Shell 已实际输出 `GLOBAL_EGRESS_SHELL_READY` | 一致 |
| 点开面板 | 复用原网络检查面板 | [02-shell-panel.png](evidence/global-egress-entry/02-shell-panel.png)，实际展示 Mantle 已安装 `148.0.7778.215`、检查更新和打开按钮 | 一致 |
| 客户端提醒 | 原面板保留提醒 | 02 截图展示真实 IPC 检测到的 ChatGPT 桌面版提醒；[03-shell-client-notice-fixture.png](evidence/global-egress-entry/03-shell-client-notice-fixture.png) 另展示隔离 IPC 模拟的 Claude Desktop 提醒与 Mantle | 一致；03 明确为测试数据 |
| 再点关闭、Escape、外部点击 | 原面板关闭行为 | 真实 Electron 自动操作与组件测试通过 | 一致 |
| 键盘 | 原生按钮可聚焦并开启 | 组件 Tab/Enter/Escape 与真实 Electron Enter/Escape 通过 | 一致 |
| 官方会话 | 原国家出口标签，只有一个入口 | [04-official-entry.png](evidence/global-egress-entry/04-official-entry.png)，`🇺🇸 美国 · 出口正常`；原冻结/手动继续/提醒文案组件测试保留 | 一致 |
| 状态计数和缺桥接 | 交代的文案、计数与降级边界 | 组件测试覆盖；无客户端或订阅桥接时隐藏入口 | 一致 |

全部截图由 Terra 子代理操作隔离 Electron 后生成并逐张实际查看，主代理复核面板和官方标签截图。Mantle 截图前滚动到卡片，并断言卡片进入视口；不能以 DOM 可见性代替截图里的实际可见性。

## 我自己做的决定

- 全局文案放进既有 `egress-copy.ts`，保留 selector 的精细订阅；面板继续使用原 `useEgressStatus` 和回调，避免新增状态存储、依赖或面板副本。
- 无 egress 客户端或订阅桥接时隐藏入口；有桥接但状态未返回时显示「检查中…」，采用交代允许的降级方式。
- 英文模式沿用产品现有本地化风格，中文按交代原文。
- 来自 DSH 界面子任务：用组件内部的 `ChipSelection { copy, isGlobal }` 区分入口；只给全局按钮设置 aria-label，保留官方 accessible name；把 isGlobal 纳入比较以正确处理会话切换。
- UI 验收使用真实 Shell 与隔离 Electron；网络出口使用项目既有 fixture，官方 Claude 使用回显脚本，避免调用付费模型或修改真实用户会话。
- UI 数据使用工作区已有忽略目录 `.c5` 的短路径，满足 macOS Unix socket 长度限制。
- 来自 Terra 的证据审查：首次截图没有拍到滚动区域下方的 Mantle，主代理补充滚动与 viewport 断言后重新验收。

## 验证边界

没有用真实付费 DeepSeek/GLM/Kimi 请求做网络验证；第三方入口分支由组件测试覆盖，真实桌面验收使用 Shell。出口检测结果为测试数据，不能作为实际公网出口证明。未打开 Mantle，也未验证真实 Claude Desktop 进程检测；02 的 ChatGPT 提醒和 Mantle 安装状态来自真实桌面桥接。既有完整 `network-egress.spec.ts` 未重跑，其本次修改的 Shell 入口预期由新增真实界面测试覆盖。

全量测试中的既有跳过：桌面端 1 项，runtime 36 项。日志另明确提示路由器二进制未配置，相关协议集成没有运行。

没有新增存储、依赖或 runtime 接口；未推送、未发布。
