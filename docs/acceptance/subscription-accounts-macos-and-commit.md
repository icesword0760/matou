# macOS 版本限制与本地提交验收

2026-11-13。用户已复核通过第九关：签名包、约 22.6 MiB 的体积增加、RunAsNode、打包态路由管理 200、只监听环回、退出无残留。随后按用户新裁决实现旧 macOS 置灰，并在 `feat/subscription-accounts` 做本地提交。没有改版本号、推送、合并、公证或发布，也没有重新生成签名安装包；第九关已有 DMG / ZIP 不包含本轮新增版本限制。

## 用户能看到与完成的行为

- macOS 15 以下，「新增供应商 → 订阅账号」中的 ChatGPT / Antigravity 两个选项仍可见，但灰显且不可选；下方显示「需要 macOS 15 或更高版本」/ "Requires macOS 15 or later"。
- 已有订阅卡片及其账号、已存模型照常显示，主「切换」或「重新登录」按钮禁用。API Key、官方供应商、切回官方入口保持原有行为。
- runtime 直接拒绝旧系统上的订阅切换，返回上述相同中英文文案且不可重试，不修改当前供应商，不启动路由。启动时账号对账、设置页额度读取也不会暗中启动路由。
- macOS 15.0 保持正常准备路径。判断来自 runtime 的 OS 读取，不依赖渲染层 UA，也不新增持久化配置。

系统判定在 `SubscriptionAccounts`：生产路径读取 `os.release()`，Darwin24 对应 macOS15；通过现有快照下发 `subscriptionSupported`。测试注入 macOS14.0 / 14.7 / 15.0，生产没有新增模拟版本的环境开关。基础存储操作结果可能不带支持标记，UI 记住最近实际 boolean，普通快照不会把 `false` 清掉。

## 验收矩阵与截图

| 场景 | 结果 | 证据 |
| --- | --- | --- |
| 注入 14.0、14.7 | 支持标记 false，settings-read 不申请路由 | `subscription-platform.test.ts` |
| 注入 15.0 | 支持标记 true，设置页及会话准备继续正常路径 | 同上 |
| 旧系统迁移卡片 | 卡片与模型元数据保留；启动对账和额度读取零 acquire；切换前拒绝、当前选择不变 | 同上、`subscription-rpc.test.ts` |
| 旧系统中英文 RPC | 激活和登录均原样返回指定文案，retryable=false | `subscription-rpc.test.ts` |
| 旧系统 API Key | API 配置与删除仍可用，不启动订阅路由 | `subscription-platform.test.ts` |
| 两个订阅入口 | 中英文真实 HTML disabled；点击不进入登录；Next 不能绕过；API 可选 | `SubscriptionSettings.test.tsx`，Terra 中文实测 |
| 已有卡片 | 卡片内容保留，切换/重登禁用，编辑和官方入口保留 | UI 组件测试；本轮未在真实旧系统设备上观察迁移卡片 |
| 无支持字段的普通快照 | 保持先前 false，不重新启用 | UI 组件测试 |

中文截图：`output/subscription-accounts-final/macos14-subscription-options-zh.png`。完整窗口为 `macos14-settings-full-zh.png`。主代理已回看：两个订阅选项、灰显状态、完整中文提示与仍正常的官方入口清楚可见。API 列表位于滚动区，截图未为展示全部列表修改原有弹窗布局。

Terra 用 CUA 按唯一标题和独立 workspace 核对窗口，进入设置 → 新增供应商，尝试点击两行禁用选项，均保持弹窗；实测 DeepSeek API 可选、选中后 Next 可用，随后取消重开，恢复截图的未选择状态。官方入口 AX 显示非 disabled，未进入下一步。没有登录、保存或发送模型请求。CUA 未直接显示 PID，不冒称其提供了 PID 证据。

这是在实际 macOS15 机器上，通过**隔离 runtime 注入 Darwin23.6.0，模拟 macOS14.6** 的真实 Electron 页面；不是实际 macOS14 设备，也不是直接伪造 UI 支持标记。隔离 App PID8935 的 runtime、数据、用户目录与 Claude 配置均在本 worktree 的 `.cache/mPAg6mA/` 下。实际过程没有路由进程；收尾所有已追踪进程退出，正式版 PID97797 仍运行。

证据：`macos14-ui-ownership.json`、`macos14-ui-result.json`、`terra-macos14-observation.md`。第一次截图实例因控制 socket 路径为105字节、超过macOS限制而未就绪；只缩短验收数据目录后重开，未改产品路径规则，首轮失败与清理结果保留在 `macos14-ui-first-*`、`ui-fixture-path-diagnosis.json`。

### 删除预览入口后的截图方式

原开发文件 `apps/desktop/src/renderer/subscription-preview.html` 与 `src/hierarchy/__fixtures__/subscription-preview.tsx` 均已删除，它们此前尚未入库。删除前读回第九关签名 App 的 `app.asar`，没有 preview/fixtures，renderer HTML 仅 `out/renderer/index.html`；删除后实际构建输出也仅 `index.html`，没有 `subscription-preview` 或 `fixture-attempt` 字串。证据：`preview-package-precheck.json`、`preview-build-check.log`。

后续截图不需要 renderer 开发页面。本轮脚本保存在忽略的 `output/subscription-accounts-final/launch-macos14-ui.mjs`，步骤为：

1. 构建 packages、runtime、desktop；在 worktree 的短 `.cache/` 目录建立独立数据、Electron、Claude 与工作区目录。
2. 仅在忽略目录建立验收 runtime wrapper，先执行 `require('node:os').release = () => '23.6.0'`，再 require 当前 `apps/runtime/dist/index.cjs`；用已有 `MATOU_RUNTIME_ENTRY` 指向它，不修改产品源码或新增版本覆盖开关。
3. 从 worktree 的独立开发 Electron bundle 启动，关闭更新和产品单实例锁，明确独立标题与工作区；Terra 用 CUA 操作到目标弹窗。
4. 主代理使用 Playwright 对同一已打开弹窗执行只读 `locator.screenshot()`，不点击、不改 DOM、不合成画面；同时核对 DOM 中两个 button.disabled 与进程树。
5. 退出本次自有进程并读回无残留。所有 wrapper、数据、截图均不提交，正式包不含截图夹具。

## 自动验证与本地提交

所有本轮日志位于 `output/subscription-accounts-final/`；长命令另同步到监督运行目录 `logs/final-*.log`。

| 检查 | 结果 | 日志 |
| --- | --- | --- |
| runtime 新边界与 RPC 定点 | 18/18 | `platform-point-tests.log` |
| runtime 相关测试（含 RuntimeServer、固定路由、原生 Claude 隔离链路） | 353 通过，首轮7项协议测试因缺少专属变量跳过 | `runtime-related-tests.log` |
| 7项协议测试补跑 | 指定固定 arm64 二进制后7/7；相关范围总计360项均实际通过 | `runtime-protocol-tests.log` |
| desktop 相关7文件 | 157/157 | `desktop-related-tests.log` |
| 订阅 UI 组件测试 | 44/44，DSH最终连续两轮通过；首次1项测试因同文案多处匹配失败，修正测试定位后通过，未隐藏日志 | `subscription-ui-tests.log`、`dsh-ui-tests.json` |
| `pnpm typecheck` | 全部 workspace 通过 | `typecheck.log` |
| runtime / desktop 构建 | 通过 | `runtime-build.log`、`desktop-build.log` |
| `pnpm check:identifiers`、空白检查 | 通过 | `identifiers-pre-code-commit.log`、`identifiers-pre-doc-commit.log` |

主要 runtime 命令开启 `MATOU_TEST_MODEL_ROUTER=1 MATOU_TEST_CLAUDE_SUBSCRIPTION=1`，以 `--maxWorkers=1 --testTimeout=15000` 跑 `src/model-router src/provider-config src/session/provider-command-environment.test.ts src/session/claude-resume-model.test.ts src/runtime-server.test.ts`。协议补跑额外给 `MATOU_MODEL_ROUTER_BINARY` 指向 `.cache/model-router/8.0.69/darwin-arm64/model-router`。使用本机假上游和合成凭据，没有登录或请求真实模型。

相关测试范围合计 runtime360项、desktop201项；点测和重复复跑不重复累加。不声称本轮运行过所有历史模块、性能压力测试或所有端到端测试。未因文档提交重复无源码变化的测试。

代码跨 contracts、runtime、会话及 UI 接口共同构成完整功能，合成一个代码提交，PRD 与各关验收记录单独提交。两条提交均在 `feat/subscription-accounts`，采用 `feat(subscription): ...`，结尾带用户指定 Co-Authored-By。代码提交为 `ee6014f`；文档提交与完整 `git log --oneline` 交付在 `output/subscription-accounts-final/git-log-oneline.txt`。

提交前逐项检查暂存文件：不含 `output/`、release、`.cache/`、凭据、密钥或压缩包；`.cache/` 全目录已加入本 worktree 的忽略规则。模式扫描用于补充检查，不把它当作能识别所有秘密格式的保证。

## 执行决定与边界

1. runtime 统一做能力判断、启动前拦截，并跳过旧系统的后台对账和额度读取；复用既有快照与本地化错误字段，不增加存储或协议状态。
2. 删除仅供截图的 preview HTML/TSX；不把截图夹具搬成另一个产品入口，不新增产品运行时 LLM 请求。
3. 代码接口相互依赖，代码一个提交、文档一个提交，避免为了分组制造中间不可运行状态；不改版本号、不推送或合并。
4. 来自 DSH：支持标记不受 provider revision 过滤，避免慢设置响应丢掉实际 capability；复用灰显风格，只加指定文案；保留编辑和官方入口，不额外增加卡片提示。
5. 来自 Terra：用唯一标题和 workspace 核对同名 App；未就绪先停报；API 测选后取消重开以恢复未选择截图；不进入授权、保存或模型请求。

尚未验证：真实 macOS14硬件/系统、旧系统真实迁移卡片、改动后的新签名安装包、DMG安装与Gatekeeper首次打开。本轮只完成用户指定源码限制、自动验证、中文界面截图与本地提交，不将这些缺口改写为已通过。
