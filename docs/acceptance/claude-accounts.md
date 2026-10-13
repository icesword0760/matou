# Claude 账户切换：实现与验收记录

日期：2026-10-13。当前实现：**0.2.5 本地验收版，独立临时浏览器登录；真实账户授权及切换继续由用户验收。**

## 用户可以完成什么

- 从底栏最右侧打开账户面板，识别已有登录并保存，或通过官方浏览器登录添加账户。
- 添加成功先核对、命名、保存；保存本身保留原默认登录。
- 查看账户用量和采集时间、手动刷新、手动切换本机默认账户。
- 首次切换说明全局范围；Matou 任务运行时安排切换，任务结束后执行；退出应用取消安排。
- 区分“本机默认已更新”与“已有进程实际生效”。重启续接当前对话需要独立确认。
- 中文备注、重新登录、去重更新、移除、错误反馈，保留 Esc / 外部点击关闭和键盘焦点。

## 按用户要求直接复用底层

依赖：[realiti4/claude-swap](https://github.com/realiti4/claude-swap/tree/3a4e5c14873eb5b32f182d55c68da98ac8c0db45)，版本 `0.27.0b1`，提交 `3a4e5c14873eb5b32f182d55c68da98ac8c0db45`，MIT。

这不是重新实现其原理，也不是一份改写分叉。`uv.lock` 锁定上游 Git 提交和传递依赖。此前自写的凭据、OAuth 与加密库草稿已删除。

| 职责 | 实际调用 |
| --- | --- |
| 账户列表、用量与失效状态 | 上游 `ClaudeAccountSwitcher.list_accounts(json_output=True, fetch=...)` |
| 保存已有登录 / 保存独立登录 | 上游 `ClaudeAccountSwitcher.add_account()` |
| 重复登录更新 | 上游 `add_account()` 的同身份更新逻辑 |
| 切换、凭据刷新、共享字段保留、锁和回滚 | 上游 `switch_to(slot, json_output=True)`，不使用 force，不做自动轮换 |
| 删除保存副本 | 上游 `remove_account(slot, assume_yes=True)`，由 Matou 先取得用户确认 |
| 独立登录结束后的临时钥匙串清理 | 上游 `delete_macos_keychain_entry()` |
| 读取首次引导候选身份 | 固定版本的只读 `_get_current_account()`；这是唯一使用的私有适配点，升级需跑契约测试 |
| 中文备注、记住范围、任务等待、面板 | Matou 的产品交互与偏好，无凭据 |

上游没有浏览器添加向导的完整 GUI 接口。薄桥接层只在独立 `CLAUDE_CONFIG_DIR` 与独立 secure-storage 目录运行官方 `claude auth login`，成功后调用上游 `add_account` 捕获该配置。没有自己实现 OAuth 授权、Token 刷新、钥匙串读写、账户切换或回滚算法。

**记录归属：**账户清单与安装的 `claude-swap` 共用同一上游存储；从 Matou 移除会移除上游保存的账户副本。中文名称仅存在 Matou 显示偏好中。对话与工作区仍归 Matou/Claude 自身管理。

## 交互对照矩阵

本次新增功能以用户确认的浅色 Mockup 为参考；既有终端模块继续保持原布局。下列证据来自真实 Electron 窗口，账户数据为隔离 fixture。

| Mockup 场景 | 实际结果 / 证据 | 结论 |
| --- | --- | --- |
| 首次使用 | `evidence/claude-accounts/08-first-use-electron.png` | 首次读取有状态反馈，无登录候选时展示两步引导 |
| 发现现有登录 | `01-detected-electron.png` | 提供直接保存，不要求重复登录 |
| 保存一个账户 | `02-single-electron.png` | 当前标记与添加第二个账户提示 |
| 浏览器添加 / 保存后另行切换 | Electron 流程 1、3；Python 独立捕获契约测试 | fixture 授权闭环通过；实际官方浏览器回调待联调 |
| 多账户 / 用量 | `04-multiple-electron.png` | 右下角向上面板、脱敏邮箱、已用百分比和采集时间 |
| 首次全局范围确认 | `05-scope-electron.png` | 明确 Matou 外实例、API Key 范围与记住选项 |
| 默认改变 / 当前对话续接分离 | Main 服务测试 + Runtime 过期 run 拒绝测试 | 默认成功后才更新 UI；真 Claude 续接效果待联调 |
| 有任务时排队、取消 | Main 排队测试 + Runtime 隐藏窗口任务测试 | 统一检测窗口外可见的运行任务，退出不保留队列 |
| 切换错误 | Main 引擎失败注入 | 保留已确认显示状态，释放 Runtime 等待，允许刷新；回滚由上游负责 |
| 编辑、移除与重启后偏好 | `03-manage-electron.png`、Electron 流程 1 | 中文备注持久化，移除确认说明共用存储 |
| 小窗口与滚动 | `06-compact-electron.png` | 面板高度受视口限制，内容滚动，底部动作保留 |
| 英文 | `07-english-electron.png` | 应用语言切换后账户面板同步 |
| Esc / 焦点 / 关闭再打开 | Electron 流程 2、3 | Esc 回到入口；关闭面板保留添加进度 |
| 多终端真实输出负载 | `09-three-live-terminals-electron.png` | 三卡片输出时的面板响应验证；账户依然使用 fixture |

## 运行与构建

开发环境首次执行 `pnpm accounts:setup`（需要 uv，安装到项目自己的 Python 环境）。之后运行常规 Matou 开发入口。

`pnpm accounts:build` 用 PyInstaller 打包未修改的上游依赖和桥接层。打包资源准备脚本会自动生成该组件，Electron Builder 带入 `extraResources/claude-accounts`。最终用户不需要另外安装 Python 或 uv；仍需本机存在 Claude Code。

构建机应原生匹配目标架构。当前已经构建并运行 macOS arm64 组件，体积约 26 MB；x64、Windows、Linux 产物与签名/公证仍待各平台发布门禁。

## 已执行验证

- 桌面类型检查、Runtime 类型检查通过。
- 桌面单元测试：90 个文件，754 项通过（包括账户编排）。
- Runtime server 回归：125 项通过，包括新增隐藏窗口、门禁排队与旧 run 拒绝。
- 上游真实库 + 合成 HOME/凭据文件契约测试：5 项通过；覆盖检测、保存、重复更新、隔离捕获、移除、真实上游切换以及共享字段保留。没有使用真实账户。
- Electron 交互验收：4 个场景流程；见对应 spec 与截图。
- 全量项目构建通过。
- 独立打包组件 `version` 请求返回 `0.27.0b1`，未读取账户。
- `pnpm check:identifiers` 与 `git diff --check` 通过。

## 发布前剩余验证

1. 由用户在官方页面完成一次新增登录；核对 macOS 钥匙串权限提示与取消/超时。
2. 用两个真实账户测试 A → B → A，确认当前 Claude Code 版本的缓存、刷新 Token 和额度接口行为。
3. 真 Claude 任务、额度等待、子代理、后台任务和续接。Matou 不以本机默认身份推断已有进程实际身份。
4. 当前版本只面向默认配置启动的 Matou。自定义 `CLAUDE_CONFIG_DIR` 显示明确提示，避免混用两个存储域；API Key/自定义服务会话不宣称已切换。
5. 网络、权限与系统休眠场景，安装后的签名和公证产物验证。

最初的隔离验收阶段未触及已安装应用。用户随后要求替换现有应用，安装结果见下。

## 本机验收安装（2026-10-13）

- 经用户确认重启，已替换并打开 `/Applications/码头.app`。这是 **0.2.4 稳定版基础 + Claude 账户功能** 的本地验收包，保留原有终端修复；没有降回开发分支的 0.1.14。
- 验收源码：`/Users/icesword/.codex/worktrees/claude-accounts-acceptance/matou`，分支 `codex/claude-accounts-acceptance`。原开发目录保留；未提交或发布。
- 新基础上重新验证：桌面 775 项、Runtime 131 项、上游契约 5 项、Electron 账户交互 4 流程、打包后 Runtime 启动与恢复测试通过；本地 ad-hoc 深层签名检查通过。此包不是公证发行包。
- 安装后实际原生窗口已显示右下角 Claude 账户入口及“发现本机已有 Claude 登录”引导。代理未执行真实账户保存、添加或切换。
- 替换前后数据计数一致：16 个工作区、26 个事项、107 条会话记录、23 条窗口事项位置，数据库 schema 均为 34。
- 旧应用、Runtime 全量数据和 Electron 用户数据备份：`/Users/icesword/Library/Application Support/Matou Backups/20261013-173453-claude-accounts`。该目录的 `installation-receipt.json` 记录源码、产物哈希及检查结果。
- 真实账户首次添加、A → B → A 切换和真实对话续接仍由用户继续验收。

## 0.2.5：添加另一个账户的修正

- 按钮改为“打开独立隐私窗口”。每次添加创建新的 Chromium 临时用户目录并开启无痕模式；与普通窗口、已有无痕窗口及其他添加流程隔离。
- 调用官方 `claude auth login`，仅对该子进程设置 `BROWSER=true`。适配层从其输出中提取白名单官方授权地址，由原 CLI 完成 OAuth、回调及凭据写入。未复制 OAuth 实现。
- 实测本机 CLI 当前地址是 `claude.com/cai/oauth/authorize`，同时兼容旧官方授权地址。地址只写入私有临时跳转文件，不进入日志或浏览器启动参数。
- 授权成功、取消、超时或正常退出会关闭该次启动的浏览器进程，清理临时浏览器数据；原浏览器保持登录。候选 CLI 登录仍由原有保存/放弃流程管理。
- 自动使用已安装的 Chrome / Edge / Brave；缺少支持的浏览器时明确提示安装，不回落到会复用旧账号的普通窗口。macOS Chrome 已实测，其他平台仍需平台验收。
- 重复登录原账户时，明确说明将更新原记录，并提供“登录其他账户”重新开始。

### 本次实测

- 官方 CLI 授权地址捕获与默认浏览器抑制通过；测试在授权前主动结束，没有登录、保存或切换真实账户。
- 已安装 Chrome 的两个并行临时窗口，以本机 HTTP Cookie 测试证明各自拥有独立 Cookie、保留自己的 Cookie；两组测试进程结束后正常关闭。
- 上游契约与浏览器测试共 11 项通过；桌面编排 11 项通过。
- 全项目类型检查和构建通过；单元回归：contracts 68、domain 7、桌面 775、桌面性能 3、Runtime 普通 1086、Runtime server 126、日志读取 9。日志性能首次与其他验证并行时 P95 为 109ms，超过 100ms 门槛；停止并行负载后独立复测通过，未放宽阈值。
- Electron 原账户流程及窗口焦点、HUD、DAG、终端回切回归 17 项通过。包含浏览器缺失、关闭与重复登录的账户专项 7 项全部通过；重复登录同一身份时曾发现旧候选未清空，已修复并补充单元及 Electron 回归。
