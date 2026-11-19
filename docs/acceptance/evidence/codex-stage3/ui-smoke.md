# Codex 第三关 Electron 界面冒烟

2026-11-15 执行 `MATOU_E2E_DISPLAY=primary pnpm exec playwright test tests/e2e/codex-stage3-hud.spec.ts --workers=1 --reporter=line`。首次运行 30 秒超时；主代理定位为测试选择器 `.pane-header` 不存在，修正为 `.terminal-pane-header`。修正后重跑 **1 项通过（5.7 秒）**。首轮和复核日志分别保留在监督目录的 `logs/stage3-ui-smoke.log` 与 `logs/stage3-ui-smoke-recheck.log`。

已实际查看本轮三张截图：

- `hud.png` 显示“Codex 自动标题样本”、`AGENTS.md`、`gpt-6-astra (medium) (258K context)`、`13%`、`1 Tools`。内容无明显遮挡；未见额度、Claude 账户、ClaudeMd 或待办。13% 是 fixture 数据估算的已用比例，不能视为真实 Codex TUI 的剩余百分比。
- `agents-editor.png` 显示 `AGENTS.md` 编辑框、待保存内容及保存、取消按钮，无明显遮挡。自动测试随后点击保存，并回读文件确认内容已写入，同时确认 `CLAUDE.md` 未变化。
- `restored-title.png` 显示“Codex 更新后的自动标题”，HUD 仍可见。自动测试覆盖手动改名后写入新的自动标题、检查手动标题保持不变，再通过菜单恢复最新自动标题。

本次使用真实 Electron 窗口、PTY 和 HUD 管线，但 Codex 服务和数据来自确定性 fixture；未启动真实 Codex，不代表真实 Codex 端到端、安装包验收或 reference product 完整对照。截图未显示手动标题保持期间的状态，该状态由自动断言覆盖；未额外验证长时间迟到更新或其他窗口尺寸。自主决定：保留首次失败日志，复核时以本轮生成的截图及自动断言区分视觉结果和保存、标题行为；没有修改源码、测试或全局配置。
