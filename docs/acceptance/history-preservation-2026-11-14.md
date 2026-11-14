# 2026-11-14 会话历史保留与回看

## 用户问题与核实结果

用户要求供应商/权限切换、会话重启不清掉旧消息，并恢复本次不可见历史。

- 原始 provider transcript 和 Matou terminal journal 均存在。用户指定会话的本地完整记录读取验证得到 2,663 条事件，包含最早用户问题及压缩前内容；本次没有改写 provider transcript。
- Matou 两处运行中替换进程路径发送 `CSI 2J + CSI 3J + HOME`，确实会清除终端普通缓冲区的历史；每次新建空 Runtime projector 又可能在 reattach 用空的新 run 快照覆盖旧回看内容。
- 用户截图中的 Claude 使用 alternate screen，由 CLI 自己维护聊天列表。官方 fullscreen 文档明确：已压缩会话关闭再恢复后，列表停在 compact 边界，更早消息仍在 transcript。这和终端普通 scrollback 是两种独立行为；仅删除清屏不能恢复完整原生聊天列表。
- 官方行为基线：https://code.claude.com/docs/en/fullscreen （Scroll the conversation / Search and review the conversation）。本次不修改第三方 CLI，也不把完整旧消息重新注入模型上下文。

## 本轮改动

- 移除供应商、权限/账户重启的破坏性清屏。
- 非 Shell 新 run 从有效 checkpoint 与后续 journal 恢复普通终端画面；无 checkpoint 时从头有序解析，按尺寸边界应用，批量处理 UTF-8 输出。新 run 首输出前把旧可见页移入 scrollback，并恢复鼠标、粘贴等终端模式。
- 新进程身份先于首包输出发布；恢复发送函数时保留 ACK/背压，前端按 PID 请求回放并避免并发首包重复显示。首次连接不能把仅含新 run 的部分输出当成完整旧缓存。
- 在 Claude/Antigravity 卡片增加只读“会话历史”，复用已有完整历史查看器。直接读取当前绑定，支持搜索、较早分页、返回最新；不会载入另一会话、改变绑定或向模型发送请求。加载/错误期间焦点也留在对话框。
- 定向读取单条会话不再索引同工作区所有其他 transcript。

## 验证状态

- Runtime RPC/完整记录读取：37 项通过。
- PTY 恢复与模式复位：24 项通过。
- TerminalSurface：78 项通过。
- 历史入口相关 UI/i18n：代理定点 160 项通过；补初始焦点后 loader 13 项通过。
- Runtime/Desktop 类型检查、构建、命名门禁通过。
- 真实 Electron 原有显示恢复、回放内存边界：2 项通过。
- 新完整历史 E2E：1 项通过，604 条分页、跨压缩搜索、权限替换后旧普通屏回看、应用重启后历史和继续输入。分阶段记录确认三次打开/关闭历史窗口均未改变 PID 或启动记录，原 JSONL 字节一致。应用恢复阶段曾额外启动一次 provider，旧进程已退出；原因尚未查明，不归因于历史窗口。
- Runtime server 最终扩展回归 149/150 通过；时区启动测试超时，单独复跑通过。未声称全套一次通过。
- 签名安装包真实 Electron 冒烟 1 项通过，覆盖 SQLite、PTY、回放、尾部损坏恢复和 schema 兼容。
- 新包已安装到 `/Applications/码头.app`，深度签名验证通过；真实用户卡片“音乐交互与运行逻辑”已打开会话历史，读到 2,663 条记录，搜索“音乐”显示 400 处匹配并定位到第 1 条用户问题；截图 `evidence/history-preservation-2026-11-14/live-conversation-history.png`。窗口保留供用户验收。

## 产品行为对照

| 场景 | 基线 | 本次结果 | 边界 |
| --- | --- | --- | --- |
| 切换供应商/权限及重启普通终端 | 用户明确要求保留旧输出 | checkpoint + journal 恢复，旧页进入 scrollback | 保持既有 10,000 行窗口 |
| 压缩后的原生 Claude resume | 原产品文档明确只展示 compact 边界后 | CLI 行为保持，Matou 会话历史可读更早全文 | 不声称原生 CLI 列表已重建 |
| 只读查看当前会话 | 复用现有完整历史加载器 | 专用入口无载入/替换按钮、只读当前绑定 | 原始文件缺失则错误/重试，不换别的会话 |
| 原 reference product 全范围 | 本轮为用户新增修复要求 | 未重新运行原 reference product | 不声明原 PRD 全范围验收闭合 |

## 恢复与备份

用户本地备份：`~/.matou/backups/history-preservation-20261114/`，含 previous.app、original-conversation.jsonl 与读取验证报告。初步 journal-only 重建只恢复了普通屏，不能恢复 alternate-screen 完整聊天；该 dry-run 快照没有应用到用户数据库。最终完整聊天回看直接使用原始 transcript，不更换身份、不改写聊天，不新增 LLM 请求或费用。

缺少快照时完整日志重建会增加冷启动时间；损坏段恢复保留可读前缀，新进程仍启动，更早完整聊天可经只读历史入口读取。尚未进行超大日志无快照批量启动性能验收。

实际用户数据约 1.7 GB terminal journal，本次重启恢复工作区存在明显等待；未将该启动性能声明为验收通过。真实供应商切换本轮未在用户账户上操作，覆盖来自 Runtime 回归；权限切换与应用重启保留历史由独立 Electron 场景验证。
