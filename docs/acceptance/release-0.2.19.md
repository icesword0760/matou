# 0.2.19 发布验收

2026-11-14。整合基线为 `main` 的 `6c4e40ce875c932bae821ea671561bb6496976d1`，工作分支 `release/0.2.19`。本记录只交付本地签名制品和回归证据，停在推送之前；不代写发布说明。

最终交付状态：最后一批 Developer ID 签名 arm64 包完整性检查通过，原始安装包冒烟 1/1、agy 模拟异常及宽窄卡布局 1/1 通过；设置页/路由及退出清理已在横幅修复前通过，按指令复用该结果。早期只读历史标记等失败和旧版对照仍完整保留，不将最终通过解释为根因已修复。尚待真实账号和人工产品签收。

## 用户场景

- 最近会话恢复：启动只恢复最近 16 条，其他卡片保留历史，点击后才恢复；恢复后输入可用，再次启动的最近使用排序正确。
- Runtime 崩溃恢复：两个前台终端恢复；Host control CLI、结构操作、跨窗口导航均完成回归，首轮结构操作失败与对照见下文。
- 订阅启动与切换：固定路由 8.0.69、本机假上游及合成凭据下，验证官方 → 订阅 → 官方、原会话恢复、一次回答、无害 Read、模型列表与 opus/sonnet/haiku 切换；没有真实登录或付费推理。
- 出口保护：真实 Electron 读本地出口样本，验证冻结、输入丢弃、恢复、临时放行和关闭保护；时区设置保存及重启持久化通过。未切换用户真实网络出口。
- 多 Agent：schema v35、供应商存储和相关 Agent 全量单元测试覆盖合并结果；真实 agy 账号专项不跑，见发布边界。
- HUD：原始三项 Electron 用例通过；额外超宽子 Agent 名称探针实际截图显示两行，计时独立且不重叠。详见补充探针记录。

## 自动化验证

本轮本地证据根目录为 `output/release-0.2.19/`，未带目录的 JSON、探针与截图路径相对该目录；未带目录的 `.log` 位于其 `logs/`，完整 Playwright trace 位于其 `playwright/`。日志已从监督目录复制留存本 worktree，不作为公开发布附件。

独立执行 `pnpm install --frozen-lockfile`，本 worktree 自有 node_modules；没有软链主仓库 node_modules。

| 检查 | 本轮结果 | 本地证据（见上方目录约定） |
| --- | --- | --- |
| 命名门禁、全仓类型检查、开发构建 | 通过；横幅修复后全仓类型检查再次通过 | `output/release-0.2.19/logs/` |
| Contracts / Domain | 71 / 12 项通过 | `contracts.log`、`domain.log` |
| Desktop 全量 | 955 项单元测试 + 3 项性能测试通过 | `desktop.log` |
| Runtime 常规最终全量 | 150 文件、1500 项通过，0 跳过；首轮失败保留如下 | `runtime-regular-final.log`、`runtime-regular-final.json` |
| RuntimeServer | 148 项通过，含两个订阅出口分类交叉用例 | `runtime-server.log`、`runtime-server.json` |
| 历史范围读取 | 9/9 通过，含 256 MiB / 16 MiB RSS 预算；本轮没有该门禁失败 | `runtime-range.log`、`runtime-range.json` |
| 原生订阅兼容补跑 | 官方 2.1.323 隔离 2/2；本机 2.1.325 假上游 2/2 | `runtime-native-point.log`、`runtime-native-325.log` |
| 账号辅助程序 Python | 27/27 通过；桌面 TS 部分已在 Desktop 全量内覆盖 | `accounts-python.log` |

最终 Runtime 常规命令在 `apps/runtime` 下执行：

```sh
MATOU_TEST_MODEL_ROUTER=1 \
MATOU_TEST_CLAUDE_SUBSCRIPTION=1 \
MATOU_MODEL_ROUTER_BINARY="$PROJECT/.cache/model-router/8.0.69/darwin-arm64/model-router" \
MATOU_TEST_CLAUDE_COMMAND="$PROJECT/output/release-0.2.19/claude-2.1.323/package/claude" \
pnpm exec vitest run --testTimeout=15000 \
  --exclude src/runtime-server.test.ts \
  --exclude src/journal/journal-range-reader.test.ts
```

其中 `$PROJECT` 为本工作区绝对路径。RuntimeServer 与历史范围读取分别按现有脚本的 15000 / 120000ms 超时独立运行；未设置 CI 重试、未放宽原有产品断言。

### 端到端逐组结果

所有窗口测试逐组单独执行，`MATOU_LOCALE=zh-CN MATOU_E2E_DISPLAY=primary MATOU_DISABLE_AUTO_UPDATE=1`，单 worker、零自动重试；安装包冒烟另行单独运行。日志和每轮错误上下文均独立保留，不以重跑结果覆盖首轮。

| spec / 场景 | 首轮 | 后续结果 |
| --- | --- | --- |
| `startup-recent-sessions` | 1/1 | — |
| `runtime-crash-recovery` | 1/1 | — |
| `ai-host-control-cli` | 1/1 | — |
| `ai-host-structure-control` | 5 通过、3 失败 | 三项各自单独复跑 3/3；完整组再跑 8/8 |
| `ai-host-navigation` | 2/2 | — |
| `real-claude-independent-launch` | 1/1 | 固定 2.1.323、隔离 HOME/config、环回假 API；无真实账号 |
| `network-egress` | 1/1 | — |
| `claude-time-zone` | 1/1 | — |
| `prd-02-bottom-hud` | 3/3 | 另有长名称补充探针，下文单列 |
| `provider-onboarding` | 2/2 | 含三终端持续输出时的响应 |
| 打包态 agy 模拟异常 | UI 修复前后各 1/1 通过 | 最终补验 380 / 900px 布局、全文 title 与按钮可用；不调用真实 agy |
| 最终 `packaged-runtime` | UI 修复后最终签名包：1/1 通过 | 先前两轮只读历史标记失败均保留，0.2.18 同点失败；最终全流程通过 |
| 打包态设置页和路由 | 通过 | 全新隔离数据；路由 HTTP 200、0 账号、仅环回；退出后进程均清理 |

首轮原始十组共 18 通过、3 失败；后续已通过的同一用例不重复累加。不能将首轮写成连续全绿。

### 合并交叉点

| 交叉场景 | 代码与已确认行为 | 合并后证据 |
| --- | --- | --- |
| ChatGPT 订阅 vs Anthropic 出口保护 | 启动时依据供应商的订阅类型分类，再注入环回路由环境；Anthropic 异常不误冻结 ChatGPT 订阅 | `runtime-server.test.ts` 对应分类及冻结用例通过；egress monitor 23 项通过 |
| Antigravity 订阅 vs 出口/时区 | 隐藏大陆时区；不注册 Anthropic/ChatGPT 出口组，不误冻结 | RuntimeServer 对应交叉用例通过；egress plan 回归通过 |
| 订阅接管 vs 会话恢复 | 模型与身份保持、失败不破坏原会话、官方/订阅切换时清理继承环境 | Runtime 全量含原生测试；额外 2.1.325 两项通过 |
| agy schema v35 vs 供应商配置 | v35 扩展数据库 provider/kind 约束；供应商配置仍由独立 JSON 存储，订阅条目保持身份和模型映射 | `migrations-v35.test.ts` 2/2（外键、触发器、索引、备份、失败回滚）；`provider-config-store.test.ts` 7/7 |
| 设置页共享入口 | 供应商页、时区页、订阅能力边界共存；原 API 配置流程不受阻 | Desktop 全量，provider onboarding 2/2、时区 1/1；打包态补验见下 |
| HUD 名称与计时 | 两行截断、计时独立、浮层边界 | `TerminalHud.agents.test.tsx` 通过；原 HUD E2E 3/3；补充截图与几何检查通过 |

PRD 参考基线沿用 [订阅参考矩阵](subscription-accounts-reference-map.md)、[出口保护矩阵](network-egress-2026-11.md)、[多 Agent 对照矩阵](antigravity-reference-matrix.md) 和 [HUD 验收](prd-02-bottom-hud.md)。本次是合并回归，没有重新运行原始 reference product 的全部逐项交互，也不宣称完整 1:1 产品验收。

## 首轮失败、原因与 0.2.18 对照

### 1. 新增订阅测试的证据目录竞争：已批准修复

`subscription-accounts.test.ts` 的 `publishes router-not-running...` 在全部产品断言之后写 `output/model-router-stage4/router-status.json`，但自身只创建 stage3。干净 worktree 中依赖另一文件 `subscription-spawn.test.ts` 建 stage4，首轮先写时发生 ENOENT。

原样单独复跑已通过（其他测试已随后建好目录）。获监督回答 #2 明确批准后，仅增加 `mkdir(dirname(evidencePath), { recursive: true })`；把本轮 stage4 移开，再单独运行确认缺目录时也通过。产品断言和运行逻辑均未修改。独立提交 `6aa3350`。

0.2.18 `408bcf9` 没有 `apps/runtime/src/model-router/subscription-accounts.test.ts` 或该模块，无法运行不存在的同用例；`baseline-counterparts.json` 保留 `git cat-file -e` 的否定证据。该项定性为**新增测试缺陷，已修复**，不是既有问题。证据：`runtime-regular.log`、`runtime-status-point.log`、`runtime-status-fixed-clean.log`。

### 2. 两项原生订阅测试版本前置条件失败：环境不匹配

**本机已自动更新到 2.1.325，用官方 2.1.323 隔离复跑。** 首轮两项均在版本断言处失败，尚未进入模型接管。官方 npm 固定版本的原生 arm64 CLI 仅下载到本 worktree output；不改全局 CLI、项目依赖或锁文件。固定 2.1.323 两项通过，最终 Runtime 全量也实际跑完。

按补充要求，用本机 2.1.325 跑原专项的 output 临时副本：仅替换版本前置期望和导入/项目路径，保留全部产品、模型、协议和切换断言，两项通过。实际执行模型列表、effective modelPicker 和 opus/sonnet/haiku 切换；这是无头原生协议验证，不等于人工点击模型菜单。所有上游均为本机假服务，没有真实账号或模型请求。

0.2.18 同样没有 `subscription-native.test.ts`，不称既有问题。证据：`baseline-counterparts.json`、`runtime-native-point.log`、`runtime-native-325.log`；`native325-evidence/execution-version.json` 记录实际版本。原测试写出的 `model-menu.json` 自带静态 client 标签 2.1.323，325 轮保留该原标签并以执行版本记录补充，未把它冒称为真实版本读取。

### 3. 结构操作三项：疑似桌面输入干扰，未定位

| 用例（`ai-host-structure-control.spec.ts`） | 首轮失败位置/输入 | 0.2.19 单独复跑 | 408bcf9 单独复跑 |
| --- | --- | --- | --- |
| 299：慢启动超过五秒 | 等待 `READY:provider-source-control-e2e`；实际命令 `bcc` | 通过 | 通过 |
| 326：关闭前重验子树 | 同一前置位置，实际命令 `嗯cc` | 通过 | 通过 |
| 393：一次性关闭确认 | 同一前置位置，实际命令 `0cc` | 通过 | 通过 |

前缀出现在目标结构操作开始之前，shell 返回 command not found。用户同期正常使用电脑，且本机运行搜狗输入法，桌面输入/候选词上屏干扰的可能性高；**尚未定位，不能断言根因，也不称既有问题**。两版 spec 和夹具完全相同，blob 对照见 `baseline-source-evidence.json`。

按要求先各自单独复跑，再对照 0.2.18。旧版通过 `git archive 408bcf9` 提取到本 worktree output，独立 frozen 安装和构建，没有在主仓库 checkout/reset/stash。两个版本各自完整结构组再跑均 8/8；这些通过不抹掉首轮失败。证据：`e2e-ai-host-structure-control.log`、`e2e-structure-point-{299,326,393}.log`、`baseline-structure-{299,326,393}.log`、`baseline-structure-group.log`、`e2e-structure-group-repeat.log`、`structure-comparison.json` 与各轮 Playwright 错误上下文。

### 4. 长名称补充探针：前置瞬时失败与探针样本校准

为实测新名称显示，在 output 中复制原 HUD 用例，只调整导入/证据路径、模拟名称并增加几何检查；未修改受控 E2E 或产品。首轮在原有 `claude` 启动前置处仍显示 Shell，尚未到名称检查；单独复跑已进入 HUD。0.2.18 原始对应 HUD 单用例通过；当前原始 HUD 整组本来就 3/3。该瞬时失败**未定位**，不能确定与上述输入干扰同因，不称既有问题。证据：`e2e-hud-long.log`、`e2e-hud-long-point.log`、`baseline-hud-agent.log`。

该次单独复跑随后因新增探针错误假设而失败：首个名称实际能在 440px 浮层内完整放一行，高度 15.9453125px（行高 15.95px），不应该要求它换行；截图已实际回看。改为明确超宽的 120 字符后缀输入后，现有外部 Agent 读取器按原规则规范化为最多 60 字符，实际名称高度 31.890625px、两行，计时与名称间有独立间距，检查通过。没有放宽产品断言，也没有修改读取器长度规则。该新增探针和几何断言在 0.2.18 不存在，不将探针误判归为产品既有失败。

证据：`hud-long-evidence/first-label-fits-one-line.png`、`hud-long-evidence/agents-live-tooltip.png`、`hud-long-evidence/long-label-geometry.json`、`e2e-hud-long-calibrated.log`。人工 CUA 未执行。

### 5. 首轮安装包缺代理依赖：发布阻塞已批准修复

首轮打包态启动超时，第二次同包单独启动仍失败；Runtime 日志明确 `MODULE_NOT_FOUND: https-proxy-agent`，尚未创建 SQLite 数据库。开发目录能解析这两个库，而 tsup 把新增 `https-proxy-agent` / `socks-proxy-agent` 留作外部模块，package-resources 未携带它们。系统安全日志查询没有找到本 App 的对应安全评估耗时记录，不能把本次超时归因于 syspolicyd / XProtect；首轮超时只证明未在测试限时内就绪，不能给出正常首次打开耗时。

监督回答 #9 批准后，`noExternal` 加入这两个现有依赖，并携带它们及传递依赖的许可；未增加运行依赖或改变出口探测逻辑。独立修复提交 `64f848c`。另独立提交 `3b69445`：在资源准备阶段，用已声明构建工具 tsup 自带的 esbuild 解析三个 Runtime 入口中的静态 require；Node 内置模块之外的依赖必须真实位于随包目录，不能借开发目录解析成功。失败时列出入口和模块名。缺模块与允许随包 node-pty 的单测 2/2，实测该防线拒绝首轮坏包并列出两个代理库。出口代理定点测试通过。

证据：`packaged-runtime-result.json`、`packaged-runtime-point-result.json`、`packaging-dependency-diagnosis.json`、`startup-security-log-result.json`、`guard-rejects-first-package.log`、`package-imports-unit-result.json`、`egress-proxy-fixed-result.json` 及对应日志。首轮制品已作废，被修复后的最终包替换；首轮资源核对另存 `artifact-inspection-first.json`，不能作为修复后包的验收结果。

### 6. 旧版签名包只读历史标记失败：既有问题对照

按精确提交 `408bcf9` 在 output 隔离目录构建 Developer ID 签名 App、公证关闭，单独运行未修改的原始 `packaged-runtime.spec.ts`。SQLite、PTY、普通回放和撕裂尾恢复均能运行，但未来数据库只读恢复在第 120 行等待 `PACKAGED_HISTORY_SURVIVES_READ_ONLY` 失败：终端只显示 shell 提示符。对照日志、错误上下文和 trace 均保留，见 `baseline-packaged-runtime-result.json`、`logs/baseline-packaged-runtime.log`、`playwright/baseline-packaged-runtime/`。不能将这轮旧版对照写为通过；此结果也不能解释首轮 0.2.19 的缺模块启动失败，两者是不同失败。

修复后的 0.2.19 原始冒烟于 10:39:29–10:40:01 单独运行，SQLite、PTY、回放和撕裂尾步骤可用，在同一个第 120 行只读历史标记断言失败，接收到的也是 shell 提示符。两版 `packaged-runtime.spec.ts` 字节相同（`git diff 408bcf9 -- tests/e2e/packaged-runtime.spec.ts` 为空）；该失败有旧版对照，定性为**旧版也能复现的间歇性只读历史标记失败，根因未定位**。当轮未修改实现或断言，不能将该轮冒烟写为全绿；该轮后续恢复到正常 schema 的末段因失败未执行。证据：`packaged-runtime-fixed-result.json`、`logs/packaged-runtime-fixed.log`、`playwright/packaged-runtime-fixed/`。

恢复轮按指令再次运行原始用例（10:41:31–10:41:58），仍在同一只读历史标记断言失败，没有缺模块或 Runtime 启动失败；保留该结果。证据：`packaged-runtime-resume-result.json`、`logs/packaged-runtime-resume.log`、`playwright/packaged-runtime-resume/`。

在获准修复共用横幅并重打包后，按要求对最终签名包再跑同一原始用例（10:54:15–10:54:32），**1/1 通过**，包括此前只读历史标记及恢复正常 schema 后的末段。未修改该 spec 或其断言，横幅 CSS/title 修改与历史回放无逻辑关联，不能据此声称已修复历史标记失败的根因。最终通过与先前失败并列保留。证据：`packaged-runtime-ui-final-result.json`、`logs/packaged-runtime-ui-final.log`、`playwright/packaged-runtime-ui-final/`。

### 打包态设置页与隔离路由（恢复轮第二组）

10:42:15–10:42:19 通过。全新隔离 HOME、数据和工作空间下打开设置页，显示默认官方供应商；通过认证的本地路由管理接口返回 HTTP 200、账号数 0，版本响应为 8.0.69。lsof 确认仅 `127.0.0.1` 监听，进程树确认路由归属本轮签名 App。退出后 App、helper、PTY shell、supervisor、路由全部消失（`survivingPids=[]`）。没有接入真实订阅账号。

证据：`packaged-router-settings-resume-result.json`、`packaged-router-smoke.json`、`packaged-settings.png`。中断前同一检查的结果保留为 `packaged-router-smoke-before-interruption.json`，不将外层未落盘的一轮当作完整通过；恢复轮已补齐退出码 0。

### 打包态 agy 模拟异常（恢复轮第三组）

10:42:38–10:42:53 串行运行指定 Controlled fixtures 用例，1/1 通过。缺少命令时显示启动失败卡和命令名；本地 Python stub 输出未登录提示后，DAG 显示 needs-input，模拟 Enter 后转 idle；重启时 stub 故意拒绝会话恢复，界面显示“Antigravity 恢复失败”及“重试恢复 / 新开 Antigravity”入口。

仅复制原验收 spec 到 output，调整证据、插件、临时目录并隔离 HOME，执行原有断言；grep 仅选择模拟异常用例，未执行任何真实 agy 专项。证据：`agy-controlled-packaged-result.json`、`logs/agy-controlled-packaged.log`、`playwright/agy-controlled-packaged/`、`agy-evidence/results/` 和该目录截图。未修改产品或受控测试代码。

**截图回看发现并获准修复的视觉问题**：首次 `14a` 截图的双栏窄卡中，“会话启动失败”标题被挤成逐字竖排；恢复失败横幅标题也出现换行。自动化功能断言通过不能替代该排版结果。原因与现有横向 flex 布局可收缩标题、固定按钮宽度相符，未修改产品。0.2.18 无 agy 入口，未获得这个具体 agy 窄卡场景的旧版运行对照，不能称为已证实既有问题。监督回答 #14 批准本轮修复。共用横幅已在 `0f2d782` 单独提交：标题不缩窄不换行；窄卡原因最多两行并以 title 保留全文；按钮整组换行右对齐且不缩小，宽卡维持原单行。Claude / Codex / agy 共用同一段 TerminalPane 渲染，没有 Agent 分支，三者均受益。只改 CSS 与原因 title，不改启动/重试/禁用逻辑或恢复横幅。

界面实施按任务约定委派 DSH 官方 `deepseek-flash / high`，仅写入 `TerminalPane.tsx` 与 `hierarchy.css`；主代理复核整合。组件定点单测 56/56（`logs/ui-delegate/terminalpane-test-final.log`）、全仓 typecheck（`typecheck-ui-fix-result.json`）、命名门禁通过；无头 Chromium 的 380 / 900px 实际 CSS 几何通过。初版 wrap 曾让宽卡长原因下按钮换行，预检发现后由消息区 `flex:1 1 0` 修正，保留前后几何证据 `failure-css-prebuild-before-refinement.json` / `failure-css-prebuild.json`。该中间实现问题未进入最终安装包，也不是产品既有问题。

最终签名包在 10:55:02–10:55:17 运行相同模拟异常及新增几何检查，1/1 通过。380px 卡：标题 66×15px 保持一行，原因 286×30px 两行，按钮组位于下一行并右对齐；900px 卡高 39px、原因一行、按钮同行。截图已回看，标题不再竖排；原因 title 等于完整错误文本。两个实际按钮的 `disabled` 均为 false，偏浅是沿用现有浅色主题配色，不是禁用；未修改颜色或禁用逻辑。

新增布局探针克隆真实签名 App 的横幅 DOM，以当前 App 已加载样式分别固定为 380 / 900px，随后移除；原异常功能路径和断言保持。最终完整场景截图另存于同一证据目录。证据：`agy-controlled-ui-final-result.json`、`logs/agy-controlled-ui-final.log`、`agy-evidence/results/failure-banner-layout.json`、`agy-evidence/failure-banner-380px.png`、`agy-evidence/failure-banner-900px.png`、`agy-evidence/14a-missing-binary.png`。原首次视觉问题的前后原因以 `3b69445..0f2d782` 两个源文件 diff 对照，当前 `14a` 路径为修复后的最终截图。恢复横幅保持本轮原样；鼠标原生 title 浮层未另行截屏，人工产品签收仍未完成。

## 签名制品、资源与完整性

45 分钟中断后先核对原有制品且全部完整，未无故重复构建；随后按回答 #14 的横幅修复重打签名包（10:50:12–10:53:47），最终完整复核亦通过。以下哈希与体积均为含 `0f2d782` 的最后一批包；修复前检查保留为 `artifact-inspection-before-ui-fix.json`。`packaged-inline-proof.json` 记录两个代理的已内联模块和 0 个裸 require，三个入口的随包依赖防线亦通过。

修复后 `pnpm package:mac --config.mac.notarize=false` 完成，Developer ID 团队 `JAKX72KQ6X`，构建明确跳过公证。最终 App 严格及深层验签、路由严格验签、App/路由/PTY 的 arm64 架构、版本 0.2.19、ZIP CRC 与包内资源逐字节读回、DMG verify、更新清单 SHA512 和大小核对均通过。证据：`artifact-inspection-ui-final-result.json`、`artifact-inspection.json` 和同目录验签日志。

固定路由为 8.0.69，下载源 sha256 与 source.json 一致；路由签名后的哈希与原下载不同是正常签名变更。包内含路由 LICENSE、CC Switch 参考许可、供应商预设许可及代理依赖许可集合；代理许可与准备资源逐字节一致。

| 制品（`apps/desktop/release/`） | 字节数 | SHA256 |
| --- | ---: | --- |
| `Matou-0.2.19-mac-arm64.dmg` | 184647372 | `224a22d2fa43186aadae0027e7919d7a72b78bfa4dd41943e086c4badc436c58` |
| `Matou-0.2.19-mac-arm64.zip` | 183852181 | `8eb94bedfc77b63cddfda09b72853415d5ff27e697715858bd4f4a71b4daa675` |

校验汇总：`apps/desktop/release/SHA256SUMS.txt`。体积对比：

- original-0.2.18：DMG +14.86%，ZIP +15.00%。基线字节数来自既存第九关记录，未访问主工作区。
- subscription-stage9-0.2.18：DMG +0.11%，ZIP +0.12%。基线字节数来自既存第九关记录，未访问主工作区。

## 发布边界

- 仅 macOS arm64 本地制品；Developer ID 签名与 Apple 公证分开，按指令显式 `--config.mac.notarize=false`，不向 Apple 上传公证。
- 不推送、不打 tag、不合并 main、不创建 GitHub Release、不上传更新服务器；不写 `docs/release/0.2.19.md` 发布说明。
- **未验证（需要真实 agy 账号，多 Agent 接入的会话已单独验收过）**：真实 agy 生命周期、模型、自动标题、Fork 与真实跨卡控制。本轮只验证可隔离的模拟异常分支及单元/迁移/存储行为。
- 不登录真实订阅账号，不读取或带入用户凭据，不发送付费模型请求。订阅真实账号卡片最终视觉和真实账号额度/推理本轮未重验。
- 初轮已确认不执行 Terra 人工界面验收；本轮继续以 CLI 驱动真实 Electron 自动化和截图回看，人工点界面由用户亲自验收，不声称 Terra 已执行。
- 没有把包安装到 `/Applications`，没有退出/替换正在运行的正式版，没有进行 DMG 拖入安装或 Gatekeeper 首次打开验收。未验证 Intel、Windows 及其他系统。

## 本轮提交

1. `aa57ae3` — `release: 0.2.19`，仅同步桌面 package version。
2. `6aa3350` — `test(subscription): create evidence directory before writing router status`，按明确批准修测试证据目录。
3. `64f848c` — `fix(packaging): bundle runtime proxy agents and preserve licenses`，按回答 #9 批准修复包内缺模块。
4. `3b69445` — `build: reject unshipped runtime dependencies before packaging`，按同一回答新增打包防线与测试。
5. `0f2d782` — `fix(ui): keep startup failure banners readable in narrow cards`，按回答 #14 批准修共用横幅排版。
6. 本记录单独提交；每个提交均附交代要求的 Co-Authored-By。

## 本轮执行决定

1. 采用本 worktree 独立依赖；旧版对照用 output 内的精确提交归档及独立依赖，避免改动主仓库。
2. 固定 2.1.323 仅作为隔离测试工具，额外用 2.1.325 临时副本做兼容检查，不改全局 CLI 或项目依赖。
3. 所有窗口组串行、保留每轮失败；补充超宽 HUD 探针做实际截图和几何验证，区分探针错误与产品失败。
4. 测试生成的原有 docs/evidence 截图在 output 留存本轮结果后恢复原字节，不把历史证据覆盖纳入发布提交。
5. 恢复后先复核现有包，全部完整性与内联检查通过，复用该批制品；三组窗口测试严格串行，每组结果立即入档。
6. 只读历史标记保留失败，不放宽断言；agy 功能通过与窄卡提示视觉问题分开报告。视觉问题获回答 #14 批准后仅修共用启动失败横幅，重打包并补验；已通过且不受影响的设置页/路由组按指令不重复。
7. 仅新增的横幅 UI 按任务规定委派一次 DSH 官方 deepseek-flash/high，主代理确认共用组件与修复路径、实施后复核；未委派其他代理，未扩展到恢复横幅或产品逻辑。
