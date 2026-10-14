# 0.2.11：接续提示去重与重启数量口径

## 用户可见结果

- 接续时，历史 `agents-md: no CLAUDE.md found; AGENTS.md loaded: ...` 通知从常规对话视图收起；本次原生加载仍可出现一条新通知。
- Ctrl+O 的完整记录视图仍能查看这些通知。用户、助手、工具消息，以及 AGENTS.md 的加载逻辑保持不变。
- 重启完成文案为“已重启并接续 N 个正在运行的 Claude 会话。”
- 确认框说明：已退出的会话不计入，也不会自动打开；普通 Shell 和其他会话不受影响。取消保留原页和焦点，确认才执行重启。

## 原因与处理边界

本机 Claude Code 2.1.280 的原生 agents-md 模块会将加载通知写入原会话，重新接续时又显示历史通知。不是 Matou 重复注入 AGENTS.md。

使用 Claude 现有的 `informational / level: info`（仅完整记录显示）语义，在**启动原会话之前**追加相同 UUID 的显示级别更新：

- 原 JSONL 字节完全保留，不覆盖、不截断、不删除任何记录；仅追加命中通知的 `level: info` 版本。
- UUID、父消息链接、时间、内容、所有未知字段保持原样，不切换 provider session ID、不分叉对话。
- 只命中本会话、system / informational / notice 类型、精确 agents-md 加载通知；普通提示、警告、错误、用户引用均保留。
- 不修改 AGENTS.md、CLAUDE.md、全局设置、Claude 可执行文件或模型请求，不启用实验性模块开关。
- 新建会话和 Fork 的来源记录不做处理；已收起的通知不重复追加。
- 文件正在变化、记录不完整、格式未知时保留原显示；读取错误记录诊断并继续。追加遇到存储故障则停止本次启动，保留恢复入口，避免后续消息接在不完整写入后面。

## reference product 交互对照矩阵

本次基线是用户截图、已确认的账户 Mockup，以及本机原生 Claude 2.1.280；不涉及 CLI 外部导航调整。

| 场景 | 原基线 | Matou 结果与证据 | 结论 |
| --- | --- | --- | --- |
| 接续历史通知 | 每次接续重复显示加载通知 | 原生 CLI 隔离测试 `before.txt` / `after.txt` | 历史通知收起，当前加载可见 |
| 完整记录 | Ctrl+O 查看完整记录 | `full-transcript.txt`，旧通知仍在 | 原生完整记录能力保留 |
| 真实接续上下文 | 同一会话承接旧对话 | 本地模型请求包含最早与最新用户/助手消息 | 无对话裁剪与身份改变 |
| 规则加载 | 官方 agents-md 加载 AGENTS.md | 请求包含独立 AGENTS canary，文件前后相同 | 原生加载保留 |
| 重启范围确认 | 所有运行中 Claude；不处理 Shell | Electron E2E 确认框、取消、焦点及混合会话 PID 比较 | 对齐，退出会话口径写明 |
| 后台分离窗口 | 重启不局限当前可见画布 | Electron E2E 隐藏分离窗口和主窗口一起重启一次 | 对齐 |
| 异常 | 逐会话错误，不虚报全成功 | Electron E2E 启动失败、存储错误单测 | 保留已有恢复与错误反馈 |

## 自动验证

- Runtime 常规测试 1111 项通过；原生集成另行启用通过。重启与跨窗口运行时 128 项通过，桌面账户相关 38 项通过，Electron/打包验收 4 项通过。
- 跨窗口测试的 Shell 信号回调等待由 2 秒调整至 5 秒，以覆盖系统 sleep 的信号延迟；首次短等待超时已复核，未以跳过替代验收。
- 新增单测覆盖原字节/消息完整性、重复操作、其他消息类型、父链接、损坏/半行、符号链接、自定义目录、存储失败、PTY 启动前处理及 Fork 排除。
- 本机真实 Claude 2.1.280 隔离集成通过：独立 HOME/config、合成历史、本地假模型，无真实凭据或付费推理；同一会话 UUID 接续成功，普通视图收起旧通知，Ctrl+O 显示旧通知，模型请求同时收到早期/最新对话和 AGENTS.md 内容。
- 原生文本证据：`evidence/claude-resume-notices/`。
- 账户界面证据：`evidence/claude-accounts-hud-restart/restart-confirm.png`、`restart-completed.png`。
- 测试日志：`/tmp/matou-0211-tests.log`、`/tmp/matou-0211-native.log`、`/tmp/matou-0211-runtime.log`、`/tmp/matou-0211-desktop-tests.log`、`/tmp/matou-0211-e2e.log`。

## 交付记录

- 已替换 `/Applications/码头.app` 为 **0.2.11** 并启动，实际 UI 的 3 个原 Claude 窗口均恢复。
- 每个窗口只剩本次加载通知，旧重复通知已收起。三份原会话 JSONL 原字节前缀 SHA-256 全部一致，AGENTS.md 哈希相同，旧通知 UUID/父链接/内容/时间逐项一致，只有新增显示级别修订。
- 数据保留：16 个工作空间、27 个事项、109 个会话、24 个窗口位置，schema 34；备份数据库 quick_check=ok。
- 备份目录：`/Users/icesword/Library/Application Support/Matou Backups/20261014-141531-0.2.11-resume-notices`。包含旧 App、完整 Runtime 数据、Electron 用户数据、三份原生会话文件；安装回执 `installation-receipt.json`，真实界面证据 `installed-startup.png`。
- App ASAR SHA-256：`4a25719f527855dd9002b7834fd100edf97b0a3ef0564d92c502b8fb9ee82398`。
- Runtime SHA-256：`6947729fab85ee129eb56960f5739b3f1f76baee03892238fccf9ab94c915b2b`。
- 本地包为 ad-hoc 签名，已验证；未发布线上更新，未在真实会话提交模型任务或切换账户。
- 原有更新入口仍提示线上旧版本 0.2.9；本次未下载或降级，未扩展本次修复范围。
