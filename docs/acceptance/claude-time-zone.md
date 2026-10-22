# Claude Code 时区设置

入口：设置 → Claude Code → 时区。默认跟随系统，选择 IANA 时区并保存后，Matou 启动的 Claude Code 自动使用该时区。可随时切回跟随系统。

设置持久保存在 Matou preferences 中（claude.timeZone），不修改系统时区或 Claude 的全局配置。每次启动从共享数据库读取，无窗口级缓存。覆盖值仅注入 Claude Code 进程的 TZ；其子进程会继承。Shell 与 Codex 不注入覆盖值。运行中的 Claude 不会因保存而被终止，需要下次启动进程才生效。

## 验证

- 类型检查、构建、命名检查及 diff whitespace 检查通过。
- RPC / 存储 / 访问策略相关 27 项测试通过（包含新增持久化、跨实例读取、回退、无效值和只读模式测试）。
- 设置 UI 相关 12 项测试通过，覆盖中英文、保存失败及重试。
- 实际 PTY 启动测试通过：Claude 使用保存时区，Shell / Codex 继承原 TZ；后续启动读取更改与 system 回退，保存不重启已有进程。
- Electron 端到端测试通过：通过界面保存，正常退出并重开应用后保留设置，切回系统默认；小窗口保存按钮可见。
- macOS arm64 安装包构建及签名校验通过；独立数据目录下连续两次启动打包应用，确认 UTC 设置持久化。
- 截图：evidence/claude-time-zone/saved.png、compact.png。

本次增加独立 Claude Code 设置页，沿用现有设置导航、排版和原生表单控件；不改动现有供应商及语言设置的操作路径。现有工作区中的其他未提交修改保留。
