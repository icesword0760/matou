# Claude 账户立即切换验收（0.2.14）

## 用户行为

- 首次确认页主按钮固定为「立即切换」，不再按任务运行状态分叉。
- 「暂不切换」仅返回列表，不创建安排；勾选「已了解范围，下次直接切换」后，后续选择账户直接切换。
- 删除等待队列、轮询执行器、排队提示、取消安排命令和 `waitForIdle` 字段。
- 可见窗口、隐藏窗口中的 Claude 正在运行时也能更新本机默认登录；不自动重启进程。
- 切换成功后沿用已有「立即重启」入口；点击后仍需确认中断任务，再重启并续接 Matou 内的 Claude 会话，普通 Shell 保持原状。
- 重新登录后的「应用到本机默认登录」同样不再等待任务结束；仍由用户明确点击，保存登录本身不切换。
- 登录、授权码粘贴、用户自行关闭隐私窗口的约定保持不变。

## 原因与边界

旧实现在 Renderer、Main、Runtime 三处判断忙碌并阻止切换；后来增加切换后的立即重启入口时，旧门禁残留。本次三层一起移除，不只是改文案。

底层仍使用固定版本 claude-swap，不改写 OAuth 或自建凭据切换。保留账户操作去重及 Runtime 配置变更的短时互斥；普通任务运行状态不参与拦截。切换成功以引擎返回账户确认，错误时保留错误提示。账户默认值更新不代表已发出的请求更换身份，现有 Claude 的凭据缓存生效时间仍受 CLI 版本和存储方式影响。

## Reference product 交互对照矩阵

基线：claude-swap 固定提交 `3a4e5c14873eb5b32f182d55c68da98ac8c0db45` 的 `switch_to` / `_perform_switch` / `_print_switch_followup`；复用引擎的隔离 HOME 测试运行上游实际代码。图形入口按用户本轮明确确认的 Matou 流程，不复刻 CLI 的排版。

| 场景 | 上游基线 | Matou 结果 / 证据 | 差异结论 |
|---|---|---|---|
| 运行中切换默认账户 | 直接切换凭据，以凭据锁协调刷新，不等待全部任务结束 | Manager busy 测试、Runtime hidden busy 测试、Electron 两个 busy Claude 切换 | 对齐 |
| 已有进程 | 不主动结束进程；提示缓存延迟与重启选择 | 切换及取消重启前后 PID、运行记录数量不变 | 对齐 |
| 立即重启 | 上游提示可重启以立即生效 | 保留现有独立按钮、确认中断提示、续接及部分失败反馈 | Matou 已确认的应用内便捷操作 |
| 隐藏窗口 | 切换是本机默认登录而非单卡片身份 | 分离隐藏窗口 busy 时切换成功，未确认前 PID 不变；确认后各重启一次 | 对齐全局范围 |
| 记住范围 | 显式 switch 不检查任务空闲 | 忙碌时记住范围后直接切换，不重复确认、不安排后台切换 | 对齐 |
| 重新登录应用 | force 恢复同账户已保存的新凭据 | 保存与应用分离，busy 时显式应用成功，不调用 resume/restart | 对齐 |
| 失败 / 并发 | 上游锁和失败处理 | 服务不可用、引擎失败保留当前值，重复点击只执行一次，互斥释放 | 保留 |
| 中英文、紧凑窗口 | CLI 无对应窗口排版 | 820×560 下主按钮在视口内，Escape 返回焦点；见图 | Matou 既有视觉延续 |

## 自动验证

- `pnpm test:accounts`：34 个 TS 用例、19 个 Python 用例通过。
- Desktop 全量标准 Vitest：98 文件、815 用例通过（包含上述 34 个）。
- Runtime server：128 用例通过。
- `pnpm typecheck`、`pnpm build` 通过。
- Electron 账户、用量、切换/重启共 24 个场景覆盖完成。首次运行 23 通过，新增英文测试被 fixture 的强制中文环境覆盖；修正 fixture 后切换/重启 4 场景全通过，未修改产品语言逻辑。
- 完整包 SQLite / PTY / 历史回放 / 截断日志恢复 / 前向 schema 冒烟：1 场景通过。
- `pnpm check:identifiers`、`git diff --check` 通过。
- DMG 校验、ZIP 完整性、提取后深层 ad-hoc 签名验证通过；ZIP 内 app.asar 与 Runtime 字节和本机安装来源一致。清单 SHA512 和产物大小匹配。

所有自动账户和 CLI 会话测试使用隔离的测试账户及进程，不替用户更换真实默认账户；真实账户的服务端生效由用户验收。

## 证据

- [中文立即切换](evidence/claude-accounts-immediate/switch-now-zh.png)
- [英文紧凑窗口](evidence/claude-accounts-immediate/immediate-switch-compact-en.jpg)
- [保留的立即重启确认](evidence/claude-accounts-immediate/restart-confirm.png)

## 本机替换

- 2026-10-14 17:48 +08:00 替换 `/Applications/码头.app`：0.2.13 → 0.2.14。
- 替换前正常退出应用，备份旧 App、Runtime 持久化目录、Electron 用户数据。未操作隐私浏览器窗口。
- 备份目录：`/Users/icesword/Library/Application Support/Matou Backups/20261014-174846-immediate-switch`。
- 替换前及备份核对：16 工作空间、27 事项、109 会话、24 窗口事项关联，schema 34。
- app.asar SHA256：`6f6b1d6c1450df45dc1790283350c42a16bf9413964efbcfa1052ddf5aa5fcca`。
- Runtime SHA256：`0e000a4536db29d46e1f66c44c14ca2a951a24551490e3ecfe634ba857682abe`。
- macOS Apple Silicon；仅 ad-hoc 本地签名，无 Developer ID 和公证。GitHub 下载包沿用该发行方式，不关闭自动更新签名验证。
- 启动后同一组数据计数保持一致；原工作现场和账户面板均已在安装版窗口恢复。用户已开启「记住范围」，后续点击列表中的切换会直接执行，未替用户点击真实账户切换。
