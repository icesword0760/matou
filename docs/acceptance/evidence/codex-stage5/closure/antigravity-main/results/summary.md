# Antigravity main 单项对照

在 main `73154003bc16e8612a23d7fca8c264b6fb6303c9` 的独立打包 App 中，按指定原选择器运行 Running-Parent Fork 一次；退出码 1，整次 22.2 秒，没有重试。app.asar SHA-256 `97f1c60e17cf99253c9feb1d907addf86e6b91d35bb54ad7b84ccd90523ee37d`；runtime SHA-256 `fafcd16e19101d0ac4399f7389db263f975a25c9576b71489f14244bbda0694c`，运行前核对一致。

第 876 行同一选择器严格模式失败：同时匹配标题“会话历史”和“创建子分支”的两个按钮，与本分支最终包的失败位置和原因相同。按指定对照标准，该选择器失败是既有问题；本场景后续 Fork / 载入未运行，不能据此宣称这些行为通过。日志 `run.log`、机器结果 `playwright.json`、trace `../playwright-artifacts/antigravity-acceptance-Ant-ce073-ktree-Running-SessionLoader/trace.zip`。

本main批仅执行指定的这一项测试一次，未额外调试或改脚本；检查无本批隔离 auth 或相关 App / PTY 进程残留。
