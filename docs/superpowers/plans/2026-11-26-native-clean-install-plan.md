# 码头新机零门槛改造实施计划（修订版）

> 实施时使用 `superpowers:subagent-driven-development` 或 `superpowers:executing-plans` 逐项推进。由主代理确定接口、协调依赖和验收，任务用复选框记录。

**目标**　让全新 Windows 和 macOS 电脑在装好码头和任意一个受支持的 Agent（如 Codex）后，无需另装 Node.js、Git 或 Mantle，也不被多余弹窗或漏包打断，双击即可完成基础会话及核心功能。

**架构**　沿用 `matou-native-runtime --mt` 的单二进制多入口模式，把码头自有的派活、Claude Hook、Antigravity Hook 全部收进 Native Runtime；DSH 保留在其自身进程内加载的薄插件；Git 坚持使用用户系统安装，不做内置打包，只做无弹窗探测与优雅降级；Windows 正式安装器负责 VC++ 运行库、必带组件校验、安装身份与更新。

**技术栈**　Rust、GPUI、Native Runtime、本地 IPC、WinRT Toast、Windows（M1 先 x64，M2 补 ARM64）与 macOS 原生发行工具链。

**规格来源**　本次新机开箱即用方案确认的边界与取舍。仓库基线为 `8a240a43`。系统通知既有 macOS 行为参考 [通知设计](../specs/2026-11-25-system-notifications-design.md)，受影响界面参考 [Native 功能清单](../../native-parity/UI-INVENTORY-2026-11-25.md) 和项目 `AGENTS.md`。开工前按当时主线复核下文文件入口。

**验收范围更新（2026-11-26）**　取消整机纯净系统专项。以现有目标平台物理机、独立测试数据及裁减 `PATH` 验证用户路径，保留无 Node、无 Git、浏览器降级、真实界面及其美观度等功能要求。

**验收环境更新（2026-11-26）**　用户使用宿主机时，交互、截图和系统通知实测放在能独立操作的测试窗口环境中。宿主机只执行后台检查。真实窗口的美观度和交互标准继续适用。精确式触控板和现用安装的系统权限连续性等专属硬件或账户状态的场景保留独立验收。

**交付顺序更新（2026-11-26）**　按用户确认，先完成 M1 和 M2 的全部代码，再提交到 `main`、更新本机 App，之后集中执行验收。验收失败的项目按实测结果修复并复验。开始 M2 开发无需等待 M1 实机验收闭合，M1 和 M2 的各项验收标准均保留。

**Agent 状态入口更新（2026-11-26）**　按用户确认移除工作区首启提示。Agent 状态在「设置 → 诊断」中按需检测，可手动刷新。

## 为什么做这件事（本次核心意图与校验标准挂钩）

### 1. 背景：为什么要做这轮改造

码头准备面向外部公开发版。此前开发与测试长期在“什么都装齐了”的开发机（自带 Node.js、Git、Xcode 命令行工具、Mantle 浏览器、macOS 环境）上进行，导致面向真实新电脑时暴露出两类结构性问题：

- **隐性外部依赖泄漏**：主程序虽是 Rust，但肚子里仍夹带了 `.mjs` 和 `.sh` 脚本，把开发机环境悄悄变成了用户的隐性门槛；同时 Windows 打包脚本对关键 sidecar 放行可选，极易发出“装完就缺功能”的残包。
- **新用户第一眼体验被打断**：没装 Mantle 时弹窗拦路、没装 Git 时触发 macOS 系统安装弹窗、缺 VC++ 运行库导致 Windows 打不开、没装或未登录某 Agent 时缺少一眼看懂的状态提示。

### 2. 本次不可动摇的 4 条意图红线

实施过程中遇到任何技术取舍，**必须优先服从以下 4 条红线**（技术能实现但违背红线的方案一律否决）：

1. **红线 I1 · 零外部开发工具门槛**：新电脑装好码头 + 至少 1 个受支持 Agent 后，**不需要**另装 Node.js、Git 或 Mantle，所有核心功能即刻可用，非依赖项绝不静默失败。
2. **红线 I2 · 纯 Rust 收口、保小包体**：码头自有的控制命令、Hook、派活全部收归现有 `matou-native-runtime`；**严禁**为了省事或边缘场景内嵌 JS 运行时、打包 Git、或引入 Windows App SDK 等几十 MB 级重型依赖。
3. **红线 I3 · 外部新用户零认知噪音**：对全新用户，系统浏览器就是默认浏览器（主流程不弹窗、不提“缺受控浏览器”）；无 Git 时不弹系统安装框、普通目录照常进；双端统一区分「未安装 / PATH 不可用 / 未登录 / 启动失败」。
4. **红线 I4 · 先发能打的公开发版，不被长尾绑架**：**M1（公开发版阻断项）** 优先闭合 macOS + Windows x64 新机零门槛体验并出候选包；**M2（体验补齐项）** 承接 Windows ARM64 通道与 Windows 触控板双指捏合，不阻塞 M1 交付。

### 3. 意图 ↔ 完成后校验标准挂钩表

验收时不只看“代码跑通”，**每一条意图都必须由对应校验编号（`V1`–`V6`）实证闭合；任一红线失守即视为未完成**：

| 核心意图 | 反面失败红线（出现即打回） | 挂钩的必过校验标准 |
| --- | --- | --- |
| **I1 · 无 Node.js 完整可用** | 无系统 `node` 时派活报错、Claude 注入静默丢失，或仍残留对系统 `node` 的隐性调用 | **V1**：在排除 `node` 的裁减 `PATH` 中，完成 Claude Hook 注入、Foreman 派活/提问/续跑/通知全流程，`mt` 与 `mt ctx` 正常；并纳入日常裁减 `PATH` 回归门禁防止反弹 |
| **I2 · 小包体与纯原生收口** | 安装包因本轮改造暴增（如引入 JS 运行时、内置 Git、Windows App SDK）；或 Windows 包漏打 sidecar | **V2**：对比基线包体无异常膨胀；Windows 打包脚本强制校验桌面、Runtime、Ghostty DLL、Claude 账户引擎、模型路由器及 `.exe` 安装器，缺任一项直接构建失败；新 Windows 机装完可直接双击启动（含 `VCRUNTIME140.dll`） |
| **I3 · 无 Mantle / 无 Git 零打断 + 清晰引导** | 没装 Mantle 时点外链或订阅授权弹窗拦人/提“受控浏览器”；没装 Git 时 macOS 弹 Xcode 安装框或普通目录打不开；缺 Agent 或未登录时只报晦涩错误 | **V3**：双端未启用 Mantle 时，点外链与订阅授权直接调起系统浏览器且主流程无 Mantle 提示（Claude 多账号仍保持独立隔离配置）；**V4**：macOS 与 Windows 在无可用 Git 的测试环境中启动全程零系统弹窗，macOS 探测测试覆盖只有 Xcode 占位命令的情形，普通目录照常会话，点 Git 专属操作给清晰原因和安装入口；双端设置诊断能准确区分已就绪 Agent、未找到 CLI、未登录与启动失败 |
| **I1 + I3 · Windows 昨日功能拉齐** | Windows 上 Antigravity 上下文优化失效、`mt` 免沙箱失效、本地 MCP 漏统计，或后台会话收不到系统通知 | **V5**：Windows 实机完成 Antigravity 工具回调与上下文优化、本地 MCP 路径匹配测试全过、后台/最小化状态收到轻量 WinRT Toast 且点击切回对应会话（含安装路径带空格与中文） |
| **I4 · 节奏可控、不扩散范围** | M1 候选包被 ARM64 / 触控板捏合 / 计划外新功能拖延 | **V6**：M1 交付 macOS + Windows x64 完整候选包并通过 `V1`–`V5`；M2 独立交付 Windows ARM64 与触控板双指捏合并完成实机验证 |

## 范围边界与分批策略

### M1 · 公开发版阻断项（本批闭合即交付 macOS + Windows x64 候选包）

| 工作 | 用户结果 | 对应校验 | 依赖 |
| --- | --- | --- | --- |
| 一 · Rust 派活与 Claude Hook | 无系统 Node 时，Claude 正常注入托管指令，Foreman 可完成派活、提问、续跑、取消和恢复 | V1、V2 | 先冻结少量旧任务基线；Agent 启动复用工作二，Git 检测复用工作四 |
| 二 · Windows x64 安装、更新与兼容拉齐 | 新机安装即带齐运行库与必带引擎。诊断页提供双端一致的 Agent 状态，Antigravity 使用原生 Hook 与上下文优化，本地 MCP 准确计数 | V2、V3、V5 | Antigravity 接入依赖工作一的私有 Hook 分派；安装身份与工作五先定接口 |
| 三 · 网页与登录去打断 | 没装 Mantle 时，普通外链与订阅授权默认直接用系统浏览器且主流程无概念噪音；Claude 多账号继续隔离 | V3 | 沿用现有链接校验和 OAuth 回调；真实账户验收使用工作二完整包 |
| 四 · Git 无弹窗探测与降级 | 没装 Git 时 macOS 不弹 Xcode 框、双端照常进普通工作区；点 Git 操作给明确提示与安装入口 | V4 | 先稳定 `git_executable()` 接口，再接入 Foreman 和上下文救援 |
| 五（M1）· Windows 后台系统通知 | 应用在后台或最小化时收到轻量 WinRT Toast，点击切回对应会话，开放通知设置页 | V2、V5 | 依赖工作二的安装身份与快捷方式注册 |

### M2 · 体验补齐项（独立验收，不阻塞 M1 发版）

| 工作 | 用户结果 | 对应校验 |
| --- | --- | --- |
| 二（M2）· Windows ARM64 发行通道 | ARM64 新机安装、升级均命中对应架构安装包，错架构不覆盖 | V6 |
| 五（M2）· Windows DAG 触控板捏合 | Windows 精确式触控板双指捏合连续缩放 DAG 画布 | V6 |

### 明确移出本计划（Scope Out）

以下事项不属于本次“新机零门槛”意图，单开独立任务，不在本计划内开发：

- **正式 Codex「从新工作树创建」新功能接入**（独立业务功能，不与工作四“无 Git 降级”混做）。
- **Windows 应用完全退出后点历史通知的“冷启动恢复会话”及 Windows App SDK 备选方案**（应用退出时后台 Agent 已停止，属伪刚需且违背红线 I2）。

### 执行阶段

1. **阶段一 · 冻结基线与共享接口**：采集代表性 Foreman 旧任务样例，在现有包和隔离数据目录上记录双端关键路径基线；定死 Windows 安装身份、Runtime 私有入口参数、`git_executable()` 返回值。
2. **阶段二 · 基础入口与零打断改造**：并行推进工作一（Runtime 分派 + Claude Hook）、工作二（Windows x64 安装器/打包门禁 + Antigravity 原生 Hook + 双端 CLI 状态提示）、工作三（去 Mantle 弹窗）、工作四（Git 无弹窗探测与降级）。
3. **阶段三 · Foreman 迁移与 Windows 通知**：工作一分三步迁完 Foreman（状态读取 → 进程启动/取消 → 邮箱/续跑/通知）；并行完成工作五（M1 轻量 WinRT Toast）。
4. **阶段四 · 全部代码集成与本机更新**：补齐 Windows ARM64 与触控板捏合。完成代码和提交前检查后提交到 `main`，由该提交构建并更新本机 App。
5. **阶段五 · 集中验收**：按 `V1`–`V6` 在对应平台和测试环境执行验收。发现失败项后修复、复验并更新证据。

实施期间同时最多两个实现型子代理；主代理先定接口与文件所有权，`host.rs`、`owner.rs`、`workspace.rs`、`settings_shell.rs` 顺序集成。

## 基线与无外部依赖防回退门禁

- [x] **留存 Foreman 旧任务对照样例**：覆盖运行中、等待回答、已结束、失败可续跑 4 种状态，记录退出码、状态、通知目标和可读历史（用于防回归，不在此扩做复杂版本迁移机制）。
- [x] **记录实机基线**：用现有 Windows x64 和 macOS 物理机及隔离用户数据核对原版安装包与当前候选的关键路径，用真实 Agent 验证最小可用路径。
- [x] **建立「纯净 PATH 冒烟门禁」（防未来反弹）**：在 CI/日常回归脚本中用裁减后的 `PATH`（明确排除 `node`、`git` 和 Mantle）运行 Claude Hook、Foreman、Antigravity Hook、无 Git 启动回归，确保后续改动不会再偷偷引入外部脚本依赖。
- [ ] **CLI 视觉与交互对照**：凡改到的 CLI 区域按 `AGENTS.md` 记录 reference product 基线与 Matou 实际结果（截图裁切到 CLI 黑色区域）；Windows 通知与安装器单列实机证据。

| 必测边缘场景 | 应有结果 | 对应校验 |
| --- | --- | --- |
| Windows 安装路径含空格与中文 | 原生 Hook、Foreman、Agent 正常启动；失败时给出可读原因 | V1、V5 |
| 隔离数据目录试用，仅提供已登录 Codex，测试进程无 Node、Git 或 Mantle | 双击即进主界面，设置诊断显示 Codex 已就绪及其他 Agent 状态；开会话、外链、派活均无阻断报错 | V1、V3、V4 |
| 旧 Foreman 任务仍在运行或双会话并发续跑 | 一次有效续跑只产生一轮，通知只发给原发起会话，保留既有历史 | V1 |
| 缺 VC++ 运行库 / 缺 macOS Xcode CLT | Windows 安装器自动补齐运行库；macOS 全程不弹 Xcode CLT 安装提示 | V2、V4 |
| Windows 通知点击时目标会话已删除或应用最小化 | 最小化时唤起并聚焦目标会话；会话已删时唤起应用并明确提示目标不可用 | V5 |

## 工作一 · Rust 派活与 Claude Hook（对应 V1、V2）

**现有入口**　`apps/native/runtime/src/host.rs`（`LaunchOptions::from_args`）、`mt_cli.rs`、`provider_hook.rs`、`owner.rs`、`apps/native/core/src/dispatch_roles.rs`。行为对照基线：`apps/runtime/control-assets/providers/claude-plugin/hooks/session-start.mjs` 与 `skills/agent-foreman/scripts/foreman.mjs`。

**交接接口**　`matou-native-runtime` 新增私有入口 `--mt-foreman <现有参数>`、`--hook claude-session-start`（工作二在同一分派处挂 `--hook agy`）。Hook 沿用 stdin/stdout JSON；用户与模型可见的 `mt`、`mt-foreman` 命令名保持不变。

- [x] **迁移 Claude `SessionStart` Hook 至 Rust**：复用现有 Rust 角色配置与上下文优化配置，覆盖 startup / resume / clear / compact 四种触发，保留角色优先级、派活深度与 `mt` 指引；会话级 Hook 直接指向当前安装目录的 Runtime，不再依赖系统 `node`。
- [x] **验证 Hook 容错与含空格/中文路径**：对照旧 `session-start.mjs` 标准输出验证 JSON 与退出码；配置损坏或异常时普通 Claude 会话仍可正常启动。
- [x] **固定 Foreman CLI 契约并分三步迁移至 Rust**：以 `foreman-status.test.mjs`、`foreman-notify.test.mjs` 和旧任务样例为基准，依次迁移：① 任务读取与状态查询（`guide`/`roles`/`status`/`watch`）；② 子进程启动、监控与取消（`start`/`stop`，复用工作二的原生 Agent 启动与工作四的 Git 探测）；③ 邮箱、跨会话续跑与原会话通知（`send`/`resume` 及邮箱命令，复用本机 `mt` 协议，彻底消灭 Windows `mt.cmd` 兼容坑）。
- [x] **切换控制目录垫片与模板引用**：将 `mt-foreman` / `mt-foreman.cmd` 指向原生 Runtime；同步更新内置提示词与用法文案；更新双端包资源清单校验，确认正式包内不再需要调用系统 `node`。DSH 观察插件继续由 DSH 进程内加载并回归状态回传。
- [x] **验收（闭合 V1）**：进程级回归覆盖并发续跑只起一轮、通知仅发原会话、停止清理子进程、旧任务可读可续；在无系统 Node 的双端候选包中跑通 Claude Hook、Foreman 全流程及 `mt` / `mt ctx`。

## 工作二 · Windows 安装、更新与双端兼容拉齐（对应 V2、V3、V5，M2 含 V6）

**现有入口**　`tooling/release-native-win.mjs`、`tooling/native-parity/package-native-win.mjs`、`tooling/native-release-helpers.mjs`、`apps/native/core/src/update_policy.rs`、`apps/native/runtime/src/update_http.rs`、`update_download.rs`、`codex_launch.rs`、`antigravity_launch.rs`、`dsh_launch.rs`、`provider_hook.rs`（含 `claude_config_counts`）、`context_guard/interceptor.rs`。

**交接接口**　Windows 公开安装器统一打包同一构建的桌面、Runtime、Ghostty DLL、Claude 账户引擎和模型路由器；与工作五对齐稳定的 AppUserModelID 和开始菜单快捷方式。

- [x] **固化 Windows x64 构建准备与强制打包门禁（M1）**：将 `prepare-gpui.py` 与 `build-libs.py` 纳入标准构建前步骤；检查产物 PE 导入表，安装器内置/引导安装所需 VC++ 运行库（解决新机缺 `VCRUNTIME140.dll`）；把 `package-native-win.mjs` 中的 `claude-accounts`、`model-router` 改为**正式发版必填项**，缺失即构建失败。
- [ ] **接通 Windows x64 正式 `.exe` 安装器与自动更新（M1）**：`release-native-win.mjs` 默认面向 `.exe` 安装器生成签名、哈希和 `stable.yml`（便携 ZIP 仅作可选补充，不再误导自动更新）；验证安装、覆盖升级、卸载与用户数据保留。
- [x] **统一双端 Agent 发现与就绪诊断（M1）**：Windows 统一兼容受支持 CLI 的 `.exe` / `.cmd` 启动形式（同时驱动会话入口与 Foreman）。macOS 与 Windows 的设置诊断区分「已就绪」「未找到 CLI」「PATH 不可用」「尚未登录」「启动失败」。
- [ ] **Antigravity 原生 Hook 与上下文优化兼容 Windows（M1）**：将 Antigravity 插件回调从 `.sh` + `/usr/bin/curl` 切换为调用包内 `matou-native-runtime --hook agy`，同时把 `interceptor.rs` 的命令改写与 `mt` 解析适配 Windows（不再使用 bash 单引号转义和 Unix 限定回退），恢复 Windows 下 Antigravity 上下文优化与 `mt` 免沙箱。
- [x] **修复 Claude 项目本地 MCP 路径统计（M1）**：`claude_config_counts` 匹配 `~/.claude.json` 的 `projects` 时，在保留原字符串精确匹配优先的前提下，兼容 Windows 斜杠差异、盘符大小写及 `fs::canonicalize` 的 `\\?\` 前缀，并确认指向同一物理目录。
- [ ] **扩展 Windows ARM64 发行与独立更新地址（M2，不阻塞 M1）**：为 ARM64 提供独立更新基础地址与 `stable.yml`，防止 x64/ARM64 错架构交叉覆盖，并在 ARM64 实机验证安装与升级。
- [ ] **验收（闭合 V2、V5）**：扩展 `release-native-win.test.mjs` 与 `package-native-win.test.mjs` 覆盖缺必带组件、缺 `.exe` 安装器、错架构校验；补齐 Windows 端 Antigravity 回调单测；在现有 Windows x64 物理机完成下载安装、Codex 首轮对话、Antigravity 工具回调与上下文优化、Claude 账户与路由组件验证。

## 工作三 · 网页与订阅登录彻底去 Mantle 打断（对应 V3）

**现有入口**　`apps/native/core/src/link_routing.rs`、`apps/native/desktop/src/workspace_wiring.rs`、`apps/native/runtime/src/browser_integration.rs`、`subscription_service/login.rs`、`tooling/claude-accounts/isolated_browser.py`。

**产品原则（对齐红线 I3）**　面向外部新用户，系统默认浏览器就是正常路径；**未安装 Mantle 时，主流程不弹窗、不提示“缺受控浏览器”**。Mantle 仅在设置页作为「可选的高安全隔离浏览器」供进阶用户开启。

- [x] **统一普通外链与供应商取 Key 链接行为**：若用户已安装并启用 Mantle 则走 Mantle；未安装或未启用时，直接静默调用系统默认浏览器打开，彻底去掉“Mantle 未安装”拦截弹窗；仅在系统浏览器唤起失败时给重试或复制链接。
- [ ] **简化订阅授权主流程**：未安装 Mantle 时直接走系统默认浏览器完成 OAuth 授权，不再在授权主流程暴露“缺受控浏览器”噪音；保留既有回调、超时、取消重试和账号校验（macOS 订阅遵守现有系统版本门槛）。
- [ ] **保留 Claude 多账号独立隔离浏览器能力**：macOS（Chrome / Edge / Brave）和 Windows（Chrome / Edge）继续使用独立临时配置启动隔离窗口，不污染用户日常浏览器登录态；若机器未装任何受支持的 Chromium 浏览器，给出清晰提示并允许返回设置。
- [ ] **验收（闭合 V3）**：单测与 `workspace_wiring_probe` 覆盖受信链接直开、非法链接拦截、无 Mantle 零弹窗直开、系统打开失败兜底；双端关闭 Mantle 偏好或检测不到 Mantle 时实测点外链、订阅授权、取消重试、先后登录两个 Claude 账号且与默认浏览器隔离。

## 工作四 · Git 无弹窗探测与优雅降级（对应 V4）

**现有入口**　`apps/native/core/src/git_repo.rs`、`apps/native/runtime/src/environment_runtime.rs`、`host.rs`、`owner.rs`、`host_control.rs`、`fork_worktree.rs`、`context_guard/cli.rs`、`context_guard/rescue.rs`。

**交接接口**　在 `git_repo.rs` 集中提供 `git_executable() -> Option<PathBuf>`，所有 Git 进程调用统一走此入口；缺失时调用方明确区分「Git 未安装」与「当前目录不是仓库」。`.git/HEAD` 的只读分支展示继续独立运行。

- [x] **实现零副作用 Git 探测**：Windows 从进程可见 PATH 解析；macOS 优先查找 PATH 中真实可执行文件，对 `/usr/bin/git` 先只读检查活动开发者目录与真实工具文件，**严禁在未装 Xcode CLT 的新 Mac 上触发系统安装弹窗**；支持在环境诊断中手动刷新重探。
- [x] **全链路接入无 Git 优雅降级**：覆盖仓库查询、底栏 Git 状态、Worktree 分叉、环境诊断、`mt ctx git`、上下文救援和 Foreman。无 Git 时普通工作区照常打开终端和 Agent，改动状态温和隐藏/标为不可用；仅当用户主动点击依赖 Git 的操作时，明确说明缺少 Git 并给安装入口；`mt ctx resume` 无 Git 时照常返回已保存线索。
- [x] **验收（闭合 V4）**：运行 `git_repo_contract`、`fork_worktree_contract`、`mt_context_contract` 覆盖无 Git、普通目录、已有 HEAD、含空格与中文路径；在现有 macOS 与 Windows 使用裁减 `PATH` 和隔离用户数据验证零系统弹窗、首次对话及诊断提示，并用探测测试覆盖 macOS Xcode 占位命令。

## 工作五 · Windows 系统通知（M1）与触控板捏合（M2）（对应 V2、V5、V6）

**现有入口**　`apps/native/desktop/src/system_notifications.rs`、`workspace.rs`、`settings_shell.rs`、`instance_open.rs`、`dag_canvas.rs`、`apps/native/scripts/prepare-gpui.py`、`apps/native/patches/`。

### 5A · Windows 轻量系统通知（M1，闭合 V5）

**红线约束**：坚持使用轻量原生 WinRT 桌面 Toast，**不引入 Windows App SDK**，不做应用完全退出后的冷启动跳转。

- [x] **对接安装身份与 Toast 发送**：复用工作二的 AppUserModelID 与开始菜单快捷方式；接入既有通知来源、同一会话替换、未读与声音偏好；在 Windows 设置页同步开放「通知」页，并在系统关闭通知/勿扰时保留应用内未读红点与状态说明。
- [x] **实现运行中/最小化点击回跳**：点击 Toast 通过单实例交接将窗口置前并定位到对应会话；若目标会话已删除，则置前应用并明确提示目标已不可用。
- [ ] **验收（闭合 V5）**：在 Windows x64 正式安装包上实测后台横幅、通知中心、同会话替换、声音/开关生效、最小化及后台窗口点击回跳、目标已删提示、系统禁用通知降级；macOS 回归既有授权与跳转。

### 5B · Windows DAG 触控板双指捏合（M2，不阻塞 M1，闭合 V6）

- [ ] **接入精确式触控板捏合输入**：在真实精确式触控板上验证 Windows 输入链路（优先 Direct Manipulation），将捏合手势转换为 GPUI 已有的 `MagnifyEvent`，复用现有 DAG 缩放锚点、范围与持久化，并将补丁纳入 `prepare-gpui.py`。
- [ ] **验收（闭合 V6）**：实机验证连续捏合缩放、双指平移、普通滚轮、`Ctrl + 滚轮`、不同 DPI 表现，确保普通触控板滚动不会误改终端字号。

## 最终候选包验收清单（与意图红线一一对应）

| 校验编号 | 对应意图 | 通过标准（缺一不可） | 所属批次 |
| --- | --- | --- | --- |
| **V1 · 零 Node 闭环** | I1 | 双端实机在无 Node 的裁减 `PATH` 完成 Claude Hook 注入、Foreman 全流程派活/提问/续跑/通知、`mt` 与 `mt ctx`；裁减 PATH 门禁纳入日常回归 | M1 |
| **V2 · 小包体与完整安装** | I2 | 包体相对基线无重型运行时膨胀；Windows x64 安装器必带组件齐全、自动满足 VC++ 运行库、`.exe` 安装/升级/卸载与数据保留通过；macOS 保持 `com.matou.desktop`、Developer ID（Team `JAKX72KQ6X`）、稳定 DR 与 `pnpm install:mac` 权限连续性 | M1 |
| **V3 · 零打断与 Agent 诊断** | I3 | 未装 Mantle 时外链与订阅授权直接用系统浏览器且主流程无 Mantle 提示，Claude 多账号仍保持独立隔离。双端在隔离数据目录的工作区直接进入终端，设置诊断清晰区分已就绪 Agent、未找到 CLI、未登录与启动失败 | M1 |
| **V4 · 无 Git 零弹窗** | I1、I3 | macOS 与 Windows 在无可用 Git 的实测环境全程无系统安装弹窗，普通工作区照常会话，Git 专属操作给清晰原因与安装引导 | M1 |
| **V5 · Windows 核心能力拉齐** | I1、I3 | Windows x64 实机通过 Antigravity 工具回调与上下文优化、本地 MCP 路径计数、带空格/中文安装路径、以及轻量 WinRT Toast 后台通知与点击回跳 | M1 |
| **V6 · 长尾体验补齐** | I4 | Windows ARM64 安装与独立更新通道通过实机验证；Windows 精确式触控板双指捏合缩放 DAG 通过实机验证 | M2 |

### 日常自动回归命令（含纯净 PATH 门禁）

~~~sh
pnpm check:identifiers
pnpm check:native-clean-install
node --test tooling/release-native-win.test.mjs tooling/native-parity/package-native-win.test.mjs
cargo test --manifest-path apps/native/Cargo.toml -p matou-native-core --offline --locked --test git_repo_contract
cargo test --manifest-path apps/native/Cargo.toml -p matou-native-runtime --offline --locked --test update_policy_contract --test update_http_contract
cargo test --manifest-path apps/native/Cargo.toml -p matou-native-runtime --offline --locked --test antigravity_contract --test dsh_core_contract
cargo test --manifest-path apps/native/Cargo.toml -p matou-native-runtime --offline --locked --test mt_context_contract --test fork_worktree_contract --test runtime_wiring_wave2b_contract
cargo test --manifest-path apps/native/Cargo.toml -p matou-native-desktop --offline --locked --test dag_canvas_contract
~~~

> 验收报告必须明确区分「自动通过」「实机/真窗通过」「尚未验证」，并附包版本、架构、Agent 版本与 CLI 区域对照证据。

### 交互与视觉验收

所有涉及交互或界面显示的改动均须在目标平台的真实窗口走完用户操作路径。逐场景保留截图或录屏，并检查视觉层级、布局与留白、对齐与间距、颜色与对比度、字体和图标一致性、文案清晰度以及状态反馈是否自然。涉及动态反馈时，同时检查焦点、键鼠操作与状态转场。编译和自动测试结果单独记录，真实窗口中功能、交互和视觉质量均通过后才能勾选验收。

| 用户入口 | 真窗视觉与交互检查 |
| --- | --- |
| 双端 Agent 状态 | 用隔离数据首次打开工作区，标签栏直接接终端区，初始 Shell 标题栏使用标准布局。从设置进入诊断后检查各 Agent 状态及刷新。验证中英文、窄窗口、键盘焦点、状态切换，以及通知中心打开时的界面。 |
| 普通外链、取 Key 与订阅授权 | 检查正常流程直接打开浏览器时的窗口反馈，以及打开失败后重试和复制按钮的层级、间距与焦点。 |
| Git 标签、专属操作与诊断 | 检查无 Git 时的留白与对齐、有 HEAD 时的分支展示，以及安装入口与刷新反馈。 |
| Windows 安装、通知和通知设置 | 检查安装器文字与布局、不同 DPI 下的设置页、系统横幅及其点击结果、目标已删时的提示。 |
| Windows DAG 双指捏合 | 用真实触控板录像检查内容锚点、缩放连续性、双指平移和滚轮反馈。 |

CLI 区域的证据按项目约定裁切，并附 reference product 交互对照矩阵。安装器、系统通知和其它系统级界面记录平台真窗证据，以及与 Matou 界面风格的衔接。

## 平台技术资料

- [微软 Visual C++ 运行库](https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist)
- [Windows 桌面 Toast 注册](https://learn.microsoft.com/en-us/windows/win32/shell/enable-desktop-toast-with-appusermodelid)
- [Windows 精确式触控板输入](https://learn.microsoft.com/en-us/windows/win32/input-precisiontouchpad/precision-touchpad-portal)
- [Apple 活动开发工具目录](https://developer.apple.com/documentation/xcode/configuring-command-line-tools-settings) 与 [首次安装命令行工具行为](https://developer.apple.com/documentation/xcode/installing-the-command-line-tools)
