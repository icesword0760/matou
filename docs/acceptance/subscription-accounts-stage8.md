# 第八关：真实 ChatGPT 账号验收

当前状态：**按用户反馈，真实登录已成功；模型列表为空的修复与构建已完成，等待监督者重启后继续产品验收。**

路径与登录错误修复见 [第八关返工记录](subscription-accounts-stage8-rework.md)。用户随后已真实登录成功，但模型为空；当前以 [模型列表修复记录](subscription-accounts-models-rework.md) 为准，已构建、等待监督者重启。下文保留首次准备的过程。 本关只测 ChatGPT，不测真实 Antigravity；不提交、不发布、不关闭或重启正式版。真实账号登录由用户本人操作。

## 已修改

- `subscription-accounts.test.ts` 通过已有的测试端口参数使用随机回调端口，不与固定 1455/51121 端口的测试竞争；产品 OAuth 端口不变。
- 并发失败的另一原因是模拟换码替身的在途轮询：该请求进入时尚未采用合成凭据，返回时真实交换已被测试取消，于是陈旧错误覆盖模拟成功。替身在响应时复核模拟状态，并添加刻意延迟的轮询回归用例；未为合成交换改变产品 OAuth 状态机。
- `provider-command-environment.ts` 对绝对路径或带 `/` 的明确命令路径跳过无用的登录 Shell PATH 探测。缺失路径不能由 PATH 修复；现在直接由 PTY 报告退出，不再等待最多 10 秒的无关探测。
- PTY 故障用例先确认进程退出及租约已释放，再等日志收尾，综合等待上限为 15 秒。租约释放逻辑没有泄漏；定点约 70ms，最终默认并发两轮为 201ms / 640ms。
- 原生综合测试包含多次真实 Claude 启动、模型菜单、工具、恢复、fork 和切回。第一轮完整运行约 67 秒，下一轮到最后切回时撞到 90 秒总时限；总预算改为 180 秒，保留每步骤独立超时与产品恢复时限。没有更改产品等待时间。

## 自动验证

最终两轮均运行默认 Vitest 文件并发，无 `maxWorkers` 或全局超时覆盖，同时开启固定路由与原生 Claude 测试开关。完整日志与 JSON 在 `output/model-router-stage8/`，摘要为 `test-summary.json`；历史失败轮次保留，不用合并结果声称全量通过。

| 轮次 | model-router | 原生完整链路 | PTY 故障退出/释放租约 | 全量 runtime |
| --- | --- | --- | --- | --- |
| `runtime-stable1` | 12 文件，159/159 通过，0 跳过 | 69.259s | 201ms | 1475 通过 / 9 失败 / 1 跳过 |
| `runtime-stable2` | 12 文件，159/159 通过，0 跳过 | 85.347s | 640ms | 1468 通过 / 16 失败 / 1 跳过，另 1 个旧 journal 未捕获 ENOENT |

全量未全绿。失败文件属于既有 runtime-server、Git、journal、fork、storage 和会话目录测试，多为 5/15 秒超时与性能阈值；本任务不修改这些旧模块。第二轮旧 `segment-journal.test.ts` 的文件清理竞争另报未捕获 ENOENT，保留完整记录。此次功能的 12 个文件在连续两轮各自全部通过。

已通过 runtime 类型、命名门禁与空白检查。本轮不重复用户已验证的 desktop 892/892；renderer 未由本次修复改动。正式产品 UI、真实账号与代理生效尚未验证。

## 正式版与开发版隔离

- 正式版标识 `com.matou.desktop`；开发包标识 `com.matou.desktop.dev`，已读取开发包 Info.plist 核实。
- 既有 `resolvePackagedApplication` / `claimSingleInstance` 仅为正式打包实例申请单实例锁。本轮品牌开发包的 Electron `app.isPackaged` 实测为 `true`；开发入口显式设置 `MATOU_DEV_BUNDLE=1`，使产品 `resolvePackagedApplication=false`，不申请正式版锁。此处以运行证据为准，不把 Electron 原始标记当作产品模式。
- `output/model-router-stage8/acceptance-main.cjs` 在加载产品入口前显式设置 userData、sessionData、crashDumps、logs 到 scratchpad 独立目录。运行时 `MATOU_DATA_DIR`、Claude 的 `CLAUDE_CONFIG_DIR`、无害读文件工作区也各自独立；未复制正式版数据库或账号凭据。
- 使用从本机已安装客户端复制并核实的 Claude Code 2.1.323 独立可执行文件，避免并行升级影响验收。开发版加载本 worktree 构建产物。
- 窗口标题“码头 · ChatGPT 验收开发版”，Dock 标记“验收”。只写脱敏启动日志，不记录 token、授权 URL 或邮箱。
- 归属、实际进程、数据目录：`output/model-router-stage8/dev-ownership.json`。真实界面观察记录：`ui-ready.json`，运行时隔离与 socket 证据：`isolation-evidence.json`。初次失败窗口的 ready 记录单独保留，不当作当前运行证据。

## 已验证的登录前真实界面

2026-11-13 13:44（本机 UTC+8），Terra 通过 CUA 按开发包精确路径核实：标题为“码头 · ChatGPT 验收开发版”；当前 URL 的工作区根包含 `chatgpt-2026-11-13T05-42-01-848Z/workspace`。已进入「设置 → 服务供应商」，Claude Code 已选中，可见「Anthropic 官方 · 使用中」和「新增供应商」。停留此页，没有点新增或浏览器登录，没有模型请求，没有操作正式版。

CUA 截图已目视核实，但没有保存到文件；这不是最终带截图的账号验收。真实登录后的邮箱脱敏截图仍待补。

初次开发启动因完整 scratchpad 路径导致 Unix socket `EINVAL`，真实 UI 一直显示“正在打开工作区…”。仅验收启动器增加短符号链接 `/private/tmp/mg8-giuoBt/d` 指向新的归属 scratchpad 根；数据物理位置仍在 scratchpad，socket 路径缩短到 59 字节，已实际存在。符号链接目录权限 0700，归属及 `realpath` 已核实，未更改产品源码或增加存储。

旧失败开发版在 SIGTERM 后仍留存，CUA 曾误激活并重开旧窗口；核对精确 PID/命令后只 SIGKILL 本任务旧 PID 95244，保留新 PID 3025 与正式 PID 97797。重新核实的界面已匹配新目录。当前正式版仍运行，未关闭或重启。旧失败日志/ready 记录保留，不掩盖这次准备问题。

## 用户登录后的验收项

2026-11-13 已执行本轮 ChatGPT 真实验收，详见 [真实验收记录](subscription-accounts-stage8-real.md)。以下结果是本轮增量；前文测试与准备过程保留为历史，不作为当前全部通过的声明。

| 场景 | 当前结果 | 证据 / 边界 |
| --- | --- | --- |
| ChatGPT 登录、保存并切换 | 用户本人已完成；保存/激活结果已读回 | 本轮不重复登录，不触碰浏览器账号 |
| 卡片邮箱、套餐、5 小时与每周额度 | 真实卡片 [email redacted] · Pro 500，每周 0%；未见 5 小时 | 无保存截图；邮箱统一打码，5 小时缺口保留 |
| Claude Code 切换与会话恢复 | 新建订阅会话通过；切回官方身份/历史保留 | 切入订阅时既有会话恢复不是本轮观察；官方未登录且别名残留 |
| 一句 GPT 问答与一次无害 Read | 合并一个用户回合通过，实际响应 gpt-6-astra，标记正确 | 无压力测试 |
| `/model` 真实名称 | Default 加三档；真实名通过；显示名 GPT 6.0 Astra 等 | 仅文字观察，无 CLI 区域 PNG |
| 实际路由代理 | 配置及实际双向流量通过 | 内核被动计数，不保存认证头/请求体 |
| WebSearch 网页搜索 | 一次搜索，结果与引用返回 | 固定源码映射至 web_search；引用为新闻目录，标题未逐条核实 |
| WebFetch 公开小页面与总结 | 一次抓取 example.com，200/577 字节并总结 | 代理及真实 Haiku 用量有证据；单次摘要的模型归因缺逐请求标注；预检未失败，未改 skipWebFetchPreflight |
| 强制结束自有路由并自动重启 | 仅引擎被杀后约 319ms 恢复；原会话下一问成功 | supervisor/正式版未动，卡片最终正常，瞬态未捕获 |
| 安全可复现的登录取消/端口占用/启动失败 | 未验证 | 依 PRD 第 4 节逐项记录，不能实测的明确保留 |
| 切回 Anthropic 官方 | 已激活，原会话恢复，未登录，官方零提问 | 不复制正式账号；底栏订阅别名残留是待闭合风险 |
| PRD 第 8 节 ChatGPT 对照矩阵 | 本轮结果已回填，仍有缺口 | 不以自动测试、Toast 或文字观察冒充全部截图/模型恢复通过 |

尚未验证真实服务方拒绝、续期失败或额度耗尽，不人为消耗额度或破坏凭据来制造这些状态。正式安装包、签名与 Intel 原生仍不属于本轮已验证结果。
