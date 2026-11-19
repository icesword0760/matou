# Claude Code 跨供应商模型分配 · 本地 Mockup

已交付唯一已确认方案：沿用当前设置视觉，在供应商列表及影响提示后、对话压缩模型前加入模型分配。仅供点击评审，不是正式产品功能。

## 先看原型与源码

- [持续画布深链接](http://localhost:3000/magic-brush/p/13a33bd7-a1d2-4309-b851-68799ca2b63e?d=1876ad8c-357d-4bde-9fc3-809389c5969c&v=397d32ce-b3b8-4e67-9556-db961b1bc82f)
- [直接点击体验](http://localhost:3000/api/magic-brush/preview/13a33bd7-a1d2-4309-b851-68799ca2b63e/design_iterations/claude-code_1_0.html?revision=2)
- [完整原型源码 index.html](index.html)：自包含 HTML、CSS、脚本，无产品依赖。
- [当前设置源码复刻 baseline.html](baseline.html)：没有新增分配区的对照基准。
- [逐场景浏览器操作结果](browser-checks.md)
- [本关决定与取舍，含代理归属](ui-decisions.md)

本地画布服务保持运行。直接体验链接带版本参数，避免验收会话复用已打开的旧页；参数不改变原型状态。刷新或“重置演示”都恢复初始混用方案。

## 可见行为

| 档位 | 初始供应商 / 账号 | 初始模型 | 默认解析 |
| --- | --- | --- | --- |
| Opus | ChatGPT 演示账号 | GPT 演示模型 A | 显式分配 |
| Sonnet | DeepSeek | DeepSeek 演示模型 A | 显式分配 |
| Haiku | 沿用默认供应商 | Haiku 默认模型 | 最终 Claude 官方（当前默认供应商） |

切换供应商会清空旧模型并更新候选和账号说明，缺模型时不能保存。“恢复默认”使三档都沿用默认，仍需保存。合法编辑后点击“保存分配”即时反馈“演示配置已保存，未修改真实设置”。状态仅保存在页面内存。

底部独立演示控件可切换账号失效、模型不可用。错误显示在对应行，原选择保留，保存受阻；DeepSeek 演示模型 A 不可用时，可以手动改选 B 恢复。不会自动替换供应商或模型。

“拟定：新会话使用此分配，正在运行的会话不受影响”是本关提案文案，不是已实现的运行承诺。其他设置控件用于呈现现有布局；本关可点击验收范围为模型分配和原型演示控制。

## 截图

| 内容 | 证据 |
| --- | --- |
| Header、供应商列表及新增区位置 | [overview.png](overview.png) |
| 当前设置基准 | [baseline.png](baseline.png) |
| 初始混用 | [default.png](default.png) |
| 编辑及模型联动 | [editing.png](editing.png) |
| 模拟保存反馈 | [saved.png](saved.png) |
| 恢复默认 | [restored.png](restored.png) |
| 账号失效 | [error-account.png](error-account.png) |
| 模型不可用 | [error-model.png](error-model.png) |
| 画布中的基准与唯一方案 | [canvas-variant.png](canvas-variant.png) |
| 较窄窗口 | [900](viewport-900.png)、[560](viewport-560.png)、[560 下方](viewport-560-lower.png) |

截图均由 Terra 子代理通过 OpenCLI 操作获取，主代理已回看默认、编辑和两类错误画面。

## 已修改

- 本目录：两份自包含 HTML、截图、交付说明、浏览器记录及决定清单。
- `.magic-brush/design_iterations/claude-code_0_0.html`：画布基准。
- `.magic-brush/design_iterations/claude-code_1_0.html`：唯一方案，与 `index.html` 内容一致（CLI 会规范化末尾换行）。
- `.magic-brush/model-provider-routing-runtime.js`：本地内存交互源码，已内嵌到 HTML。
- `.magic-brush/` 中的项目、草稿、变体元数据、已确定 brief 和固定源码参照。

没有修改产品源码、真实账号、钥匙串、真实配置、模型请求路由或依赖；没有提交、推送、发布或安装应用。原有 `mobile-direct-2026-11-16/` 等未提交内容保留。

## 样式来源核对

| 可见部分 | 当前源码依据 | 核对结果 |
| --- | --- | --- |
| 设置导航、header、间距和背景 | `hierarchy.css:1-25,1064-1125`、`ModelSwitchSettings.tsx:268-339` | 基准复用当前规则；新增区不改变原结构 |
| 供应商卡片、按钮及字号 | `provider-setup.css:111-137,157,173-182`、`SubscriptionProviderCard.tsx` | 继续使用当前设置样式，内容换成演示数据 |
| 压缩区 | `AgentModelSettings.tsx`、`agent-model-settings.css` | 维持原布局关系；默认跟随会话 |
| 派活区 | `DispatchSettings.tsx`、`dispatch-settings.css` | 保留在压缩区之后 |
| 新增分配区 | 上述设置卡片、表单、错误及按钮规则 | 同一套 workspace token、系统字体及控件尺寸 |

基准共有基础 CSS 规则核对：设置框架 21 条、压缩区 16 条、派活区 23 条均与当前源码一致。基准是源码复刻，并参考仓库既有设置验收截图；没有读取当前真实账号或操作真实产品设置。

## 自动验证与真实界面验证

- 自动验证：脚本语法、25 个必要交互 ID、无重复 ID、无外链资源、无网络或存储 API 调用；本关自行编写产物的命名策略检查通过。
- 交付验证：独立预览 HTTP 200，服务内容与交付源码一致；画布中的两个节点真实可见。
- OpenCLI 操作通过：初始值、联动、缺模型禁存、合法保存、恢复默认、两种错误、手动恢复、保存后焦点保持及无滚动跳动；控制台 0 条消息，网络记录未观察到模型请求。
- 实际 1280×708 浏览器内，body、设置主区和分配区的 DOM 宽度没有横向溢出。
- 1440×900、900×900、560×900 为 OpenCLI 临时截图尺寸的视觉检查；900 保持三列，560 逐档纵排。没有把这些截图尺寸视为已完成全部持久视口操作。
- 未做产品全量测试或构建，本关不涉及正式实现。

命名检查仅覆盖本关自行编写的产物；没有为未提交的自动扫描副本扩大清理范围或改动产品源码，也没有宣称全仓门禁通过。

## 未验证与保留项

真实键盘 Enter/Space 激活及内层鼠标滚轮未完成：OpenCLI 的输入结果不足以证明真实操作；补查时 CUA 连接两次返回 `native pipe closed before response`。已有 Tab 顺序、点击及焦点证据不替代这两项。详见浏览器记录。

Magic Brush init 自动复制的历史素材约 **1.1GB**，按监督明确回复暂不删除，不阻塞本关评审。主要路径：

- `.magic-brush/evidence/`：约 797MB，其中 `assets/` 约 669MB。
- `.magic-brush/design-system/`：约 334MB，主要为 `assets/`。
- `.magic-brush/init/` 与 `moodboard/`：少量扫描及参照文件。

这些是本次工具扫描生成的本地副本，原型自包含且不依赖它们。所有原始仓库素材保持不动。
