# 第八关返工：符号链接目录、登录本地化与重试

当前状态：**三项修复完成，正常隔离开发版已重开并停在供应商设置页；等待用户本人重试登录。** 真实 ChatGPT 登录、套餐/额度与模型请求仍未验收。本轮未发起真实 OAuth、未读取浏览器账号、未发送模型请求；不测试真实 Antigravity，不提交、不发布。

## 已修改与原因

### 符号链接目录可以启动，目录逃逸仍拒绝

- `process-owner.ts` 首次启动时用 `realpath(dataRoot)` 取得并保留规范化根目录。之后 model-router、config 和 credentials 都按真实路径检查，必须是该根目录的子路径；不是按字符串前缀判断，名称类似的相邻目录同样会被拒绝。
- config 的 `oauth.auth-dir` 也先规范化，再与已检查的 credentials 真实目录比较。既有配置即使拼写成别名，只要解析到同一个允许目录就可以通过；实际启动参数与工作目录使用已检查的真实路径。
- runtime 的 `prepareRouterDirectory` 先规范化 dataRoot，再返回真实 model-router 路径，因此新写的配置直接使用规范化的 auth-dir。准备目录时也检查 credentials、logs 和既有 keys 的真实位置，越界时不继续读写或更改权限。
- 保留用于 macOS 短 socket 路径的别名。没有把整个 runtime 的数据根强制换成长路径，避免再次触发本地 socket 长度限制。

新增真实固定引擎测试覆盖 dataRoot 符号链接成功启动与 auth-dir 一致；新增四项负例覆盖 model-router、config、credentials 以及配置内 auth-dir 越界。

### 登录失败返回双语文案，弹窗进入可重试状态

- contracts：`rpc.error.messageLocalized?: {zh,en}`，`SubscriptionLoginView.reasonLocalized?: {zh,en}`，保留既有字符串字段。
- runtime：登录类 RPC 的运行错误返回受控双语原因，错误的兼容字符串按请求语言选择；取消、超时、采用账号失败等终态也有双语字段。原始诊断仅在脱敏日志，不放入双语显示字段。
- RuntimeClient 保留 RPC 的 `messageLocalized`，登录弹窗按当前语言优先显示它；状态结果优先显示 `reasonLocalized`。旧字段仍可兜底。
- 登录启动立即拒绝、服务未连接等路径统一进入错误态并解除 busy。复用现有唯一「重试 / Retry」按钮，不改布局、样式或其它设置页面。重试取消旧 attempt、清空旧状态，只发起一次新 login-start；成功后进入模型设置，保存前没有卡片。

文案分类与路径校验都是确定性本机处理，没有新增运行时 LLM 请求或模型费用。

## 自动验证

最终相关测试单轮全部通过：

| 检查 | 结果 | 证据，相对 output/model-router-stage8-rework |
| --- | --- | --- |
| runtime 相关模块 | 15 文件，174/174，0 跳过；固定引擎和原生 Claude 开关开启 | `runtime-final.log` |
| desktop 相关模块 | 3 文件，79/79：RuntimeHost、RuntimeClient、订阅设置 | `desktop-tests.log` |
| DSH 弹窗定点 | 28/28；中英立即拒绝、状态失败、可用重试、精确新增一次请求 | DSH 子任务最终定点日志 |
| contracts、runtime 与 desktop 构建 | 通过 | `contracts-build.log`、`runtime-build.log`、`desktop-build.log` |
| runtime / desktop 类型检查 | 通过 | `runtime-typecheck.log`、`desktop-typecheck.log` |
| 命名与空白 | 通过 | `identifiers.log`、`git diff --check` |

保留失败历史：第一轮相关 runtime 为 173/174，独立 Node 强杀探针不能解析新增的无扩展本地导入。随后保持 process-owner 只依赖 Node 内置模块，将很小的路径边界检查留在该模块，完整复跑得到上述 174/174。DSH 的一次新增英文测试断言错误地要求“不包含英文标题”，与正确英文句子冲突，改为“不等于原始错误”后通过。没有把这些失败轮算作成功。

未重复前一轮已声明有旧模块超时/性能失败的全量 runtime；本轮相关测试通过不表示全量已绿。

## 真实界面与真实开发版路由

### 有意注入本机启动失败

使用单独的「码头 · 登录失败验证」窗口（PID 43010）和独立数据子目录；通过验收启动器让 RuntimeHost 的资源路径指向一个缺少路由二进制的夹具。产品 runtime、renderer 和 RPC 仍是真实构建，官方固定二进制及正式版资源没有改动。这个夹具无法进入 OAuth，因此只允许在该夹具点登录和重试。

Terra 经 CUA 实际操作：设置 → 服务供应商 → Claude Code → 新增供应商 → ChatGPT → 下一步 → 在浏览器中登录。实际显示：

> 模型路由未能启动：本地模型路由暂时无法使用，请重试。

同时可见可点击的蓝色「重试」和「取消」。点击重试一次后再次显示同一中文原因和可用重试。失败返回太快，**中间等待态未能目视捕获**。未见浏览器打开，未保存新增卡片。脱敏日志恰好记录两次 login-start 失败，与两次点击对应。证据：`ui-failure-evidence.json`、`ui-failure/dev-app-sanitized.log`。

CUA 截图已目视检查，但没有 PNG 文件：screenshot 技能预检发现 macOS Screen Recording 未授予，未继续运行捕获 helper，也未批准权限。真实账号后的脱敏截图仍待补，不能把文字观察当成已保存截图。

### 正常验收开发版

仅关闭归属本任务的故障窗口 PID 43010 后，使用修复后构建重开正常开发版，**PID 49992**。正式版 **PID 97797** 持续运行、未操作。正常开发版沿用此前同一 scratchpad 数据、Claude 配置、Electron 配置和工作区；不复制正式版数据或凭据。

- 窗口标题「码头 · ChatGPT 验收开发版」。Terra 已真实进入「设置 → 服务供应商 → Claude Code」，看见「新增供应商」，仅 Anthropic 官方使用中，停留此页；没有点登录。
- dataRoot 仍为 `/private/tmp/mg8-giuoBt/d/runtime-data`；实际物理目录由 `normal-router-ready.json` 记录并验证位于既有 scratchpad。
- 打开设置页后，后台路由真实启动为 **PID 52717**；认证管理接口 HTTP **200**，版本 **8.0.69**，空凭据列表（0 个），没有模型推理。
- 当前配置中的 auth-dir 已是规范化真实路径，并与 credentials 目录解析一致；config 权限 **600**、credentials 目录权限 **700**。
- 只将这些布尔值、进程、路径、HTTP 状态和版本写入证据，没有持久化管理响应、key 或账号凭据。证据：`output/model-router-stage8-rework/normal-router-ready.json`；应用归属/窗口为 `output/model-router-stage8/dev-ownership.json`、`dev-ready-49992.json`。

## 我自己做的决定

1. 只规范化路由资源与安全比较，runtime 的短通信路径保持别名；防止修路由时重新引入 macOS socket 路径过长。
2. auth-dir 两端比较真实路径，同时让新配置写真实路径；不需要迁移存储或增加依赖。
3. process-owner 保持可由独立 Node 探针直接运行；没有为了共享几行检查而扩大模块依赖。
4. 真实错误界面通过独立缺失二进制夹具复现，避免代理替用户发起真实 OAuth；验完关闭夹具，再打开正常开发版。
5. 来自 DSH：优先安全读取约定双语字段，兼容旧 message/reason；保留现有 View 和唯一重试按钮；内部捕获登录启动失败，确保错误态和 busy 解除；取消旧 attempt 成功后才发新请求。
6. 来自 Terra：无法捕获快速中间态就如实记录；录屏权限未授予时不继续截图或批准权限；正常窗口只进入设置，不点登录。

## 尚未验证

等待用户本人重新执行「新增供应商 → ChatGPT 账号 → 在浏览器中登录 → 保存并切换」。真正的 OAuth、邮箱/套餐/额度、GPT 回复、真实 Read、/model、用户代理的服务方流量、异常恢复与切回官方仍按原第八关执行；PRD 第 8 节矩阵继续未闭合。屏幕录制权限导致截图未落盘。没有提交、推送、正式安装包或发布。
