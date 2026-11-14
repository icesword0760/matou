# API 供应商模型名称与窗口信息 — 2026-11-14

## 实际接口核验

使用用户已保存的 DeepSeek 配置，仅向官方 `https://api.deepseek.com/models` 发起带鉴权 GET，返回 HTTP 200。未执行模型推理，没有把密钥或请求头写入证据。

| 调用 ID | 接口返回名称 | context_window | max_output_tokens |
| --- | --- | ---: | ---: |
| deepseek-flash | DeepSeek-V4.1-Flash | 1,048,576 | 393,216 |
| deepseek-v4-pro | DeepSeek-V4-Pro | 1,048,576 | 393,216 |

原始非敏感响应：[deepseek-models-response.json](evidence/provider-model-metadata-2026-11-14/deepseek-models-response.json)。字段含义参照 [DeepSeek 官方模型列表文档](https://api-docs.deepseek.com/api/list-models/)；本表来自实际响应，而非文档示例。窗口元数据不等于已执行该容量的推理压力测试。

## 问题与修复范围

原实现仅保留 `id`，丢弃 `name`、`context_window` 等字段；获取成功后仍固定提示“已带出预设模型”。API 配置没有保存模型元数据，重新打开以及“最大可扩展”策略也无法读到接口提供的容量。

本轮保留实际调用 ID，贯通模型展示名称与容量的解析、保存、重开、窗口策略。地址或密钥变化时旧元数据失效，缺失字段保持未知。展示名称不会替代发送给服务的 ID，不自动修改用户的模型或角色映射。

## 自动验证

- Runtime 三组定点测试 33/33：真实响应解析、字段过滤、配置保存与重开、换地址/密钥清除、多个模型的共同窗口、Codex 自定义地址不借用官方模型规格。
- 弹窗定点测试 23/23，设置页 9/9，相关语言、窗口解析与独立模型设置测试 18/18，共 83 项相关测试通过。
- contracts 构建、runtime/desktop 类型检查与生产构建通过；命名门禁和 `git diff --check` 通过。
- 整合时一个旧保存请求断言因新增 `modelMetadata: []` 失败；已更新为新接口预期，重跑设置页测试通过。
- 未执行全仓完整测试或真实大上下文推理压力测试。

## 真实 Electron 界面验证

使用独立临时数据目录。仅将本任务所需 DeepSeek 配置用于只读模型列表请求；没有切换用户当前供应商、修改真实角色映射或执行模型推理。

| 场景 | 实际结果 | 证据 |
| --- | --- | --- |
| 官方接口真实获取 | 显示 DeepSeek-V4.1-Flash / deepseek-flash / 1,048,576 tokens；Pro 同样显示实际名称和窗口 | `06-picker-visible-after-fetch.png` |
| 名称和 ID 搜索、选择、焦点 | 两种搜索均可用；获取/重开列表聚焦搜索；选择后回到 ID 输入；调用值保留 ID | `08-small-window-list-expanded.png`、组件定点测试 |
| 仅保存、重开、重载 | 元数据已落盘；卡片和编辑页仍显示名称/容量；最大窗口偏好数据库保持 maximum，快速重开后随异步读取正确同步 | `09-small-window-selected.png`、`12-small-window-card-fixed.png` |
| 手动未知 ID、改地址或密钥 | 名称与旧窗口立即失效；未知模型不套用其他模型规格 | 组件定点测试与真实表单操作 |
| 服务只提供 ID | 显示 ID，明确无展示名称/窗口；最大窗口保留未知 | `04-id-only-unknown-window.png`（文案优化前，状态行为一致） |
| 刷新失败与重试 | 错误反馈可见，旧成功提示消失；保留的信息明确标作上次记录 | `05-refresh-failure-retained-metadata.png`（本地 HTTP fixture，不冒充外部响应） |
| 960×650 / 1200×780 / 1280×900 | 列表与搜索结果扩展后保持在滚动区可见；底部保存可达；卡片长名称、完整窗口与按钮可读，无横向溢出 | `08-small-window-list-expanded.png`、`09-small-window-selected.png`、`12-small-window-card-fixed.png`、`13-normal-window-card-fixed.png` |
| 终端输出时操作 | 3 个 Shell 存活，其中一个就绪 PTY 内运行 3 个并发输出循环；打开设置并进入编辑的单次测量 238ms，输入及放弃修改正常 | `interactive-load-result.json`、`11-terminal-output-under-load.png` |

以上图片均位于 `evidence/provider-model-metadata-2026-11-14/`，已分别查看实际截图与可见区域尺寸。探索操作覆盖反复打开列表、搜索结果由 2 项变 1 项再清空、手动模型、错误恢复及快速重开。英文自动测试通过；额外英文界面探查受测试进程强制中文环境影响，未作为英文真实界面通过证据。

本轮界面检查发现并修复了三处问题：获取后列表在折叠区下方不易发现、异步偏好读取后弹窗停留在错误默认选项、小窗卡片长规格挤压按钮。截图 `02` / `07` / `10` 保留修正前的中间状态，不作为最终布局通过证据。初次冷启动终端负载样本未等 Shell 就绪，已作废；最终负载测量已由实际输出截图确认。以上均不等同于用户验收。

## reference product 交互对照矩阵（本次增量）

| 场景 | 基线 | 本轮对照边界 |
| --- | --- | --- |
| DeepSeek 配置表单、获取、选择、仅保存 | 用户本轮截图及当前 Matou 可运行流程 | 保留入口及原调用 ID 编辑能力，修复遗漏信息与错误提示 |
| 模型真实名称和上下文容量 | 实际 DeepSeek `/models` HTTP 200 | 以接口字段为准，不硬编码展示名或猜测窗口 |
| reference product 全范围 PRD 对照 | 本轮没有重新运行参考产品 | 本次是用户指定故障修复，不宣称原 PRD 对照矩阵全部闭合 |

## 打包与本机安装

- Developer ID 签名完成，打包产物、安装暂存副本、最终安装目录的 `codesign --verify --deep --strict` 均通过。未公证、未发布。
- 签名应用使用独立数据启动，重新实际调用官方模型列表成功；名称、窗口和已保存的 maximum 选择正确。证据：`evidence/provider-model-metadata-2026-11-14/15-packaged-real-metadata.png`。
- 修复版已安装到 `/Applications/码头.app`；原版完整保留于 `/Users/icesword/.matou/backups/model-metadata-20261114-4nlea25e/previous.app`。
- 用户当前运行的应用和会话没有被强制重启，重启码头后生效。旧配置首次在新版点击“获取模型”并保存后，会保留真实名称和窗口元数据。
- 所有独立验收窗口均已关闭，临时凭据副本与测试数据已删除。
- 本轮源码未提交、未推送；不宣称真实对话能力、最大窗口压力测试或用户验收已经完成。

## 2026-11-15：三档模型配置对齐 GPT

用户指出角色映射仍为输入框，要求下拉选择并对齐现有 GPT 配置。移除灰色“更多设置”折叠区，模型区直接采用 GPT `SubscriptionSetupView` 的三列结构与共享样式：供应商名称 → Opus/Sonnet/Haiku 名称下拉及强度下拉 → 上下文窗口。编辑底部为“保存 / 保存并切换”。API 必需的 Key、地址、默认模型保留；无目录服务保留显式手动 ID 后备。

| 对照场景 | GPT 基线 | API 实际结果 / 证据 |
| --- | --- | --- |
| 三档模型 | `SubscriptionSetupView.tsx` 三列原生 select、共享 `.provider-mapping` | 真实 Electron 显示名称下拉，无折叠和重复行说明；`provider-role-dropdown-2026-11-15/01-role-selects.png` |
| 强度联动 | 已知模型使用接口 supportedEfforts | DeepSeek 仅 follow/low/high/max；不支持的旧值保留、报错并禁用保存（组件测试） |
| 保存和重开 | 保存 / 保存并切换 | 实际保存 Opus Flash + high，重开保持；其它档位独立；两按钮 use 参数组件验证 |
| 手动后备 | GPT 登录目录不需要此入口 | API 可显式手动填写 custom ID 并返回列表，未知值保留；实际表单验证 |
| 小窗口 | 相同三列与窗口控件样式 | 960×650 渲染视口内弹窗 width/scrollWidth 均 518，footer 底边 627；`02-small-window.png` |
| 签名安装包 | 本次未重新登录 GPT | 签名副本真实打开三档下拉通过；`03-packaged.png` |

本轮组件 27 项（实施代理定点）及设置页整合 14 项均通过；desktop typecheck、生产构建、命名门禁与 diff 检查通过。整合检查中测试选择器的 `exact` 选项不适用于 Testing Library，改为精确正则后类型检查通过。真实界面使用隔离数据、合成 Key 与上一轮已核实元数据，未调用真实推理、未修改用户真实映射。本轮没有重跑 GPT 登录流程、全仓测试或完整参考产品矩阵，不等同于用户验收。

安装采用已安装应用副本，仅替换 renderer 后重新生成 ASAR 完整性并 Developer ID 签名；签名校验、签名副本启动通过。旧版备份：`/Users/icesword/.matou/backups/role-dropdown-i5xm2rv8/previous.app`。保留当前运行会话，不强制退出；重启应用使用新版。未提交、推送或发布。

## 2026-11-15：窗口刷新、当前供应商重新应用与压缩模型列表

- GPT 模型目录实际读取确认最大窗口为 872,000；修复设置初始化及打开中弹窗未同步异步模型信息的问题，保留用户正在编辑的选择。
- 当前供应商编辑后“保存并切换”不再因供应商 ID 相同而直接返回，重新提交应用请求；“保存”仍只保存配置。
- 压缩模型由会按当前完整 ID 过滤的 datalist 改为完整名称下拉，保留手动 ID、未知旧 ID；切换模型不会偷偷改变推理强度，不支持的组合禁止保存。
- 自动验证：设置页与订阅弹窗 18 项、压缩组件 15 项通过；desktop typecheck、生产构建、命名门禁和差异检查通过。
- 真实 Electron 端到端验证：同一供应商再次保存并切换实际提交第二次 activate；两个摘要模型均可选择；重启保留名称、强度及 512K 设置；PTY 发出的普通/压缩/普通三个请求模型与强度分别匹配设置。服务为本地合成 HTTP，不代表外部推理验证。新增多模型验收最初因选择器匹配多个控件、模型按 ID 排序的预期不符失败，修正测试后最终通过（10.8 秒）。
- 签名打包应用真实打开压缩设置，Flash 与 Pro 均可选择，选择 Flash/high 并保存成功；截图与请求证据位于 `evidence/provider-settings-followup-2026-11-15/`。
- 仅更新安装包 renderer，Developer ID 签名及安装后严格校验通过；旧版 `/Users/icesword/.matou/backups/provider-settings-followup-91n73qrz/previous.app` 可恢复。
- 未验证外部推理、大上下文压力、全部参考产品行为或用户验收。Carbon/Argon 是参考服务的多模型编排模式，当前目录不提供对应单模型项，本轮未伪造模型或新增编排能力。

已从菜单正常退出并重新启动本机安装版，真实用户配置页确认 GPT 卡片最大 872K，Antigravity 压缩下拉完整显示 11 个模型。为展示而切换的“独立配置 / Antigravity”仍为未保存草稿，实际压缩设置保持跟随会话；没有替用户选择新压缩模型。DeepSeek 当前会话未重新执行“保存并切换”，不宣称历史 Pro 会话已迁移或真实请求已改成 Flash/high。

一张冗余 PNG 的压缩二进制字节偶然匹配命名门禁，恢复先前已入库的截图并去掉重复生成语句，最终结果以签名应用截图与请求 JSON 为准；未更改或绕过门禁规则。
