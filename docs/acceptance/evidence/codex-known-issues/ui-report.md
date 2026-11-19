# Codex 已知问题：打包应用界面定点验收

当前结论：祖先历史场景首跑通过；错误通知场景第四次定点通过，通知、DB/DAG、终端错误与防升级提示均已验收。前三次测试服务/定位器失败全部保留，下面按尝试记录。

- 时间：2026-11-15（新加坡时间）；源码 HEAD `cc2b1dbc0e5956960bda2d2b17cf6709bb5410ed`。打包指纹文件记录相同源码；执行前实际核对 `app.asar`、`runtime/index.cjs`、`runtime/mt-cli.cjs` SHA-256 均与记录一致。
- 首批执行指定两条测试各一次，`workers=1`、`retries=0`；总退出码 **1**，1 通过、1 失败，总耗时 33.3 秒。原始 [Playwright 结果](ui/results/playwright.json) 与 [日志](ui/results/known-issues-ui.log)。

| 场景 | 结果与证据 | 耗时 / 尝试 |
| --- | --- | --- |
| 真实 Codex CLI、本机受控 404、错误通知与卡片/DAG 状态 | **失败，未闭合。** 已收到本地服务请求并返回 404，但等待 20 秒未在本轮 rollout 中找到 `task_complete.error`，测试在第 680 行超时。现场 rollout 只有 `task_started` 和用户消息；[失败截图](ui/last-model-error-uPFeZe.png)中 Codex 仍显示 Working。未到达唯一 Codex 错误通知、正文、卡片/DAG error 和禁用升级提示的断言，不能据此判断产品错误处理是否正确。[失败上下文](ui/results/error-context.md)与 `.cache/c10/ui/model-error-uPFeZe` 保留。 | 27.376 秒 / 1 次 |
| history_base 祖先预览、搜索与缺失提示 | **通过。** 使用明确标注的受控 rollout fixture，真实打包 App 中可见祖先内容、子会话内容以及祖先搜索命中；缺失祖先时可见“祖先历史不完整：祖先文件缺失或不可读，仅显示当前会话。” [搜索截图](ui/ancestor-preview-search.png)、[缺失提示截图](ui/ancestor-missing-note.png)、[结构化结果](ui/results/ancestor-ui.json)。该项不是现场创建真实 Fork。 | 5.133 秒 / 1 次 |

两项均以 `copyAuth: false` 创建独立 CODEX_HOME；错误服务绑定 127.0.0.1，未向外网模型发送请求。测试后 `find .cache/c10/ui -name auth.json` 无结果，按本批打包 App 路径和隔离目录检查未见残留进程；原有 `/Applications/码头.app` 及其会话保留。未改产品代码、测试、全局配置、CLI 或安装版 App。

自主决定：遵守单次尝试和保留失败现场的要求，不重跑、不修改测试或产品；将错误通知、DAG 和升级提示列为未验证，不把历史测试通过推及真实 Fork。

## 错误通知补跑（修正端点后）

2026-11-15 单独补跑一次，`workers=1`、`retries=0`，**失败，退出码 1，耗时 26.260 秒**。执行前重新核对 `app.asar`、`runtime/index.cjs`、`runtime/mt-cli.cjs` 三项 SHA-256 与原打包指纹一致。首跑证据保留不覆盖。

本次 [请求记录](ui-recheck/results/model-error-request-log.json)显示两个 `POST /v1/responses` 均完整读取：第一个 105549 字节、未响应；第二个 55137 字节、已响应。20 秒内当前 rollout 仍只有 `task_started` 和用户消息，未出现 `task_complete.error`；[失败截图](ui-recheck/last-model-error-CYS3tC.png)中界面仍显示 Working。存在单个 pending 被后一个请求覆盖的可能，但未确认哪个请求属于当前主轮次，不能将其断言为确定原因。

测试停在第 697 行，未执行同轮错误事件、唯一 Codex 通知和正文、数据库及 DAG error、999 版本种子不提示升级等后续验收；上述行为仍未闭合。[日志](ui-recheck/results/known-issues-ui-recheck.log)、[机器结果](ui-recheck/results/playwright.json)、[失败上下文](ui-recheck/results/error-context.md)及 `.cache/c10/ui-recheck/model-error-CYS3tC` 保留。

本批隔离目录 `auth.json` 数量为 0，按打包 App 与隔离路径检查未见残留进程。自主决定：遵照单次补跑边界，不重跑、不改产品或测试；本次运行真实 Codex CLI，模型服务为本机受控端点，不能代替外网模型故障验收。

## 错误通知最终定点验收（并发请求修正后）

2026-11-15 再单独运行一次，`workers=1`、`retries=0`，耗时 **21.167 秒，退出码 1**。此前首跑 27.376 秒和补跑 26.260 秒的失败材料均保留；三次各尝试一次。执行前六个打包组件 SHA-256 均与 `package-sha256.json` 一致。

本地 [请求日志](ui-final/results/model-error-request-log.json)中两个 POST 均 `ended=true`、`responded=true`。真实 Codex CLI 的 rollout 中 `task_started` 与含 404 错误的 `task_complete` 的 turn ID 一致；测试已通过数据库状态 `error`、DAG `status-error`、卡片通知标记、通知数量 1、标题 Codex 和错误正文断言。[现场截图](ui-final/last-model-error-7j1CDI.png)人工核看：通知中心显示一条 Codex 通知及 404 正文，其后方 Codex 终端也可见 404 内容。

测试随后在第 714 行失败：终端定位器随焦点变化，另一张 Shell 卡片已聚焦，因此实际读取到 Shell 提示符，而不是 Codex 终端。该断言以及后续“不显示升级提示”断言未通过或未执行，DAG 专用截图和成功 proof 未生成；DAG 状态结论来自此前已通过的自动断言，不是截图人工验收。999 版本种子已设置，但不能将未执行的升级提示断言记为通过。[原始日志](ui-final/results/known-issues-ui-final.log)、[结果](ui-final/results/playwright.json)、[失败上下文](ui-final/results/error-context.md)及 `.cache/c10/ui-final/model-error-7j1CDI` 保留。

本批隔离目录认证副本数为 0，按本批 App 路径及隔离目录未见残留进程。自主决定：不修改产品或测试、不再重跑；结果限定于真实打包 App 和真实 Codex CLI 对本机受控 404 的行为。

## 第四次定点尝试：终端定位器修正后

第四次单独运行 **通过**，测试自身耗时 8.034 秒、命令退出码 0，`workers=1`、`retries=0`。前三次失败分别为 27.376、26.260、21.167 秒，原始材料仍保留；这不是首跑通过。测试前六个打包组件 SHA-256 均匹配指纹。

真实打包 App 启动真实 Codex CLI，使用不复制认证的隔离环境与本机受控 404 服务。两个模型请求均完整接收且已响应；当前 `task_started` 与带 404 错误的 `task_complete` 的 turn ID 相同。测试全部断言通过：数据库状态 error、DAG error、唯一标题 Codex 的通知及错误正文、对应终端 404，以及预置 999 版本缓存时未出现升级提示。[请求日志](ui-fixed-selector/results/model-error-request-log.json)、[结构化错误证据](ui-fixed-selector/results/model-error-ui.json)、[测试结果](ui-fixed-selector/results/playwright.json)、[运行日志](ui-fixed-selector/results/known-issues-ui-fixed-selector.log)。

已人工核看 [通知截图](ui-fixed-selector/error-notification.png)，可见唯一 Codex 通知和 404 正文；[DAG 截图](ui-fixed-selector/error-dag.png)中 Codex 卡片显示红点“异常”。本批 `auth.json` 数量为 0，按隔离目录与打包 App 路径检查无残留进程。自主决定：仅按授权运行一次该场景，未重跑祖先历史；验证范围为本机模拟模型故障，不代表外网服务故障实测。
