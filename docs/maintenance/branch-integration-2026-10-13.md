# Matou 分支逐项检查与主线整合报告

日期：2026-10-13。范围：上轮清理后全部 50 个本地分支，以及原始目录的未提交账户工作。

## 结论

- 账户切换与独立临时浏览器登录已作为实际代码合入本地 main，版本 0.2.5。
- 20 个历史分支找到相同补丁；19 个通过逐提交 range-diff 与现有实现核对，确认已被调整后纳入主线。补齐历史归并关系，未将旧实现覆盖到新实现上。
- 2 个历史资料分支仅作历史归并：旧 0.1.1 版本实验、9 月 1 日工作备份。产品功能已有新版实现，原型仍遵循现有排除策略。原始提交完整保留在主线历史中。
- 原目录账户初稿先保存为 47c54b6，再由经过验证的 5c98ab8 实现覆盖相同功能。
- 全部 50 个分支头现已可从 main 到达，`git branch --no-merged main` 无结果。仍保留分支名和工作目录，未删除在做的工作或 cloud-release-test/output 未跟踪产物。
- 主项目目录已切回 main；原 main-release 工作目录在相同提交上转为 detached，文件保留。没有推送远程、打标签或公开发布。

## 合并方式（明确区分代码与历史）

- 实际功能提交：`5c98ab8`。原 0.2.4 的终端修复全部保留。
- 历史归并提交：`67b947d`，使用 `git merge -s ours`，包括 41 个历史分支和原账户初稿。只在上面的内容核对完成后执行。
- 已核对历史归并前后的 Git tree 完全相同；这一步补的是祖先关系，不是把这些分支所有旧行重新塞回产品。
- 原主线恢复点：`refs/archive/main-before-account-integration/20261013` → `1cf3d52`。

## 逐分支矩阵

| 分支 | 内容 | 核对结果及处理 | 证据 |
| --- | --- | --- | --- |
| `codex/archive-history-context` | 历史搜索的冷数据上下文 | 全部补丁等价，补齐历史归并 | 7cf7867 → 55e97cb |
| `codex/brand-gate-final` | 品牌命名清理与检查 | 已在主线调整整合，补齐历史归并 | 956d113 → fa829de,c1f2e4f |
| `codex/claude-accounts-acceptance` | 账户切换及独立临时浏览器登录 | 实际新增代码合入 | 5c98ab8；浏览器及账户专项验证 |
| `codex/claude-provider-recovery-e2e` | Claude 恢复失败时保留会话及恢复测试 | 已在主线调整整合，补齐历史归并 | e5f5ae5 → 38a6cfd |
| `codex/cloud-release-test` | 旧 0.1.1 云更新实验 | 历史归档；保留 0.2.5，不回退至 0.1.1 | 835fd6f 仅改版本号；output 原地保留 |
| `codex/d01-foreground-terminal-lifecycle` | 前台终端在卡片虚拟化过程中的连续性 | 已在主线调整整合，补齐历史归并 | 0245cde → 72a5f9c |
| `codex/d08-dag-aggregation` | 远处会话分支聚合 | 全部补丁等价，补齐历史归并 | 790e48c → 48c7658 |
| `codex/d09-delete-scope-ui` | 统一会话节点移除范围 | 全部补丁等价，补齐历史归并 | bd5a640 → ecf9aa1 |
| `codex/dag-10k-electron-e2e` | 一万节点的真实 Electron 测试 | 全部补丁等价，补齐历史归并 | 28219ec → 33282ca |
| `codex/detached-return-retry` | 独立窗口返回重试 | 全部补丁等价，补齐历史归并 | f32a6f0 → c8f3386 |
| `codex/durability-gate-overflow` | 终端持久化背压及压缩压力测试 | 已在主线调整整合，补齐历史归并 | f7f8c1d → ba18d9b |
| `codex/final-fork-e2e-fixes` | Fork 失败展示及恢复焦点 | 全部补丁等价，补齐历史归并 | 35893ac → 950485a |
| `codex/final-ui-e2e-fixes` | DAG、会话投影与卡片交互稳定性 | 已在主线调整整合，补齐历史归并 | 8e60147 → ea59fc7 |
| `codex/final-window-recovery-e2e-fixes` | 独立窗口、Shell 和回放恢复 | 已在主线调整整合，补齐历史归并 | 88c10a1 → 0ba451b |
| `codex/fork-background-e2e` | 后台 Fork 设置测试 | 全部补丁等价，补齐历史归并 | 32358bd → 2cd5440 |
| `codex/git-workflow` | Git 交互工作流 | 检查前已合入，保持现状 | c8d2bcb |
| `codex/hud-e2e-fix` | 底栏交互响应 | 全部补丁等价，补齐历史归并 | 5015d20 → b0e5c48 |
| `codex/internal-hardening-audit` | 内部稳定性审计 | 检查前已合入，保持现状 | 9351865 |
| `codex/journal-range-replay` | 日志按范围读取和流式回放 | 已在主线调整整合，补齐历史归并 | 1fbebce → bc9d7fb |
| `codex/layered-recovery` | 分层恢复调度与卡片进度 | 已在主线调整整合，补齐历史归并 | 5f3173a → ce68c81; d6f24e6 → 10415c4; b41c084 → 现行命名门禁 |
| `codex/layered-recovery-merge-fixes` | 恢复并发竞态修复 | 已在主线调整整合，补齐历史归并 | c9167c3 → 310e40e |
| `codex/ledger-update` | 验收台账整理 | 全部补丁等价，补齐历史归并 | 441b38d → d2266a5 |
| `codex/loaded-session-fork` | 已载入会话 Fork | 检查前已合入，保持现状 | 177aaaa |
| `codex/long-terminal-history-gate` | 长终端历史稳健性测试 | 已在主线调整整合，补齐历史归并 | 553d445 → 0ee3eb6 |
| `codex/main-wip-backup-20260901-154452` | 历史混合工作备份：看板、模型配置、标题、DAG、侧栏原型 | 新版已覆盖功能；保留旧原型历史 | 3db69f6、97dfa98、d153f27、297af76；a58b7e0 排除原型 |
| `codex/migration-interruption-e2e` | 数据库迁移中断恢复测试 | 已在主线调整整合，补齐历史归并 | 7435499 → e0091bb |
| `codex/native-window-focus-e2e` | 原生窗口焦点恢复测试 | 已在主线调整整合，补齐历史归并 | 82ef8c5 → 6dbac18 |
| `codex/projection-10k-incremental` | 一万节点增量状态投影 | 全部补丁等价，补齐历史归并 | 0bce168 → 314f9ac |
| `codex/provider-identity-gate` | 恢复时核对服务身份 | 全部补丁等价，补齐历史归并 | 5c62c75 → 1cdf80a |
| `codex/real-claude-storage-resume` | 等待真实 Claude 恢复就绪 | 全部补丁等价，补齐历史归并 | c52088b → 2faedac |
| `codex/recovery-notification-fix` | 恢复通知、关闭与移除确认 | 已在主线调整整合，补齐历史归并 | 2c50e37 → d822b62,57a9888 |
| `codex/recovery-readonly-gates` | 只读恢复验收 | 全部补丁等价，补齐历史归并 | 0434d6b → ed1a841 |
| `codex/recovery-scale-gate` | 大规模终端恢复 | 全部补丁等价，补齐历史归并 | 21e1175 → d356d94 |
| `codex/recovery-scale-worktrees` | 工作树规模恢复 | 全部补丁等价，补齐历史归并 | 66cdecc → 299acbf |
| `codex/runtime-crash-e2e` | 后台运行进程崩溃恢复 | 已在主线调整整合，补齐历史归并 | e36af44 → a66715f |
| `codex/runtime-enospc-card-isolation` | 磁盘写满时单卡片隔离及后台终端连续性 | 全部补丁等价，补齐历史归并 | 6c11809 → e4509f9; 4bccf5c → 3730d97 |
| `codex/runtime-pty-stress-matrix` | 真实终端压力测试 | 已在主线调整整合，补齐历史归并 | f7f8c1d → ba18d9b |
| `codex/scale-dag` | 大规模会话及 DAG 性能 | 已在主线调整整合，补齐历史归并 | 31b6539 → 2bea7b7 |
| `codex/session-history-pagination` | 完整历史分页 | 检查前已合入，保持现状 | 25f3f84 |
| `codex/task10-offscreen-terminal-e2e` | 离屏前台终端连续性 | 已在主线调整整合，补齐历史归并 | 2a827ab → e9f07f2 |
| `codex/task2-graph-layout-index` | 迭代式会话图遍历 | 全部补丁等价，补齐历史归并 | 1973fb0 → 6e53029 |
| `codex/task6-checkpoint-segment-protection` | 日志压缩时保护恢复检查点 | 已在主线调整整合，补齐历史归并 | 7c344a0 → 8716d0b |
| `codex/terminal-history` | 持久终端历史归档和搜索 | 已在主线调整整合，补齐历史归并 | ee0e947 → 437ea1f |
| `codex/terminal-history-fixes` | 日志轮转恢复 | 全部补丁等价，补齐历史归并 | c77af9c → b6349bc |
| `codex/unit-gate-audit` | 发布稳定性门禁 | 全部补丁等价，补齐历史归并 | e36e27d → 6670ad1 |
| `codex/worktree-recovery-e2e` | 工作树迁移恢复 | 全部补丁等价，补齐历史归并 | 7cb7598 → 1ecbc76 |
| `feat/settings-language` | 原开发目录的语言设置及账户初稿 | 初稿快照后归并，采用新版实现 | 47c54b6 → 5c98ab8 |
| `feature/prd-05-hierarchy` | 工作区、事项与标签层级 | 检查前已合入，保持现状 | 76972d3 |
| `feature/session-dag-canvas` | 会话画布与 DAG | 检查前已合入，保持现状 | 9ba568c |
| `main` | 稳定产品主线 | 检查前已合入，保持现状 | 1cf3d52 |

## 非完全同补丁的重点核对

- 前台终端、压力矩阵、独立窗口、长历史、迁移中断、运行进程崩溃、大规模 DAG、离屏连续性、检查点保护及历史搜索：range-diff 的差异主要是整合上下文，功能代码已纳入。
- Claude 恢复：保留后来加入的 startFreshProvider 等接口及签名适配。
- 分层恢复：主线扩展了 Fork 执行权限、attachView 传递和恢复遮罩门禁；保留新版，避免退回较早的调用方式。
- 最终 UI 修复：分支额外保留焦点的逻辑已经存在于主线 RuntimeProjectionStore.#replaceSessionGraphs，回归测试覆盖相关卡片与 DAG 操作。
- 日志读取：分支附带的压缩测试与后来独立压缩修复、现行 rawHotBytes 预算一起核对；没有用旧的 1 字节测试阈值覆盖当前预算。
- 品牌清理：主线已有两轮改名与更严格的 project-identifier-policy；现有全项目门禁通过。旧 check-identifiers 脚本按已被替代处理。
- 混合工作备份：模型服务配置、四列看板、手动标题、父会话拉回、DAG 入口、侧栏样式与迁移均在现有源码及测试中；5 个设计原型文件不进入当前发布源目录，原提交留在历史。

## 验证与已知边界

- 全项目构建、类型检查、标识门禁和 diff 检查通过。
- 单元回归：contracts 68、domain 7、桌面 776、桌面性能 3、Runtime 1086、Runtime server 126；日志读取 9；最终在主线串行执行完整 pnpm test 全部通过。账户编排最新 11 项、上游及浏览器 11 项通过。
- 日志性能首次并行验证 P95 109ms，超过原 100ms 门槛；结束并行负载后独立复测通过，未放宽断言。这一波动作为负载敏感项保留记录。
- Electron：账户、HUD、DAG、原生焦点和终端回切 17 项通过；更新后的账户专项 7 项通过，含浏览器缺失、关闭、重复账户重试。
- 浏览器：本机已安装 Chrome 的两个并行临时配置独立 Cookie 测试通过；官方 CLI 当前授权 URL 捕获通过，默认浏览器未打开。未代替用户完成真实账号授权。
- 当前验证平台为 macOS arm64；本次不声称其它平台已验收。
- 打包后 Runtime 启动、SQLite、PTY、重放与异常日志恢复专项 1 项通过；产物 ad-hoc 深层签名检查通过。

## 本机交付记录

- 已于 2026-10-13 18:21 正常退出旧应用，备份后替换并启动 /Applications/码头.app，实际版本 0.2.5，源码为 67b947d。
- 备份目录：/Users/icesword/Library/Application Support/Matou Backups/20261013-182117-accounts-private-browser，保留旧 0.2.4 应用、Runtime 全量数据、Electron 用户数据和 installation-receipt.json。
- 安装前后核对：16 个工作区、26 个事项、107 条会话记录、23 条窗口事项位置，数据库迁移版本 34，数量一致。
- 实际窗口已出现右下角 Claude 账户入口。首次启动实机检查时，终端及账户面板停留在加载；系统 TCC 日志显示新 ad-hoc 签名与原完整磁盘访问授权的代码要求不匹配，子进程处于文件访问等待。已请用户检查并自行处理系统权限提示。此项尚待用户处理后复验，不记为实机验收通过。
- 原生系统通知窗口受工具访问限制，未代替用户批准权限；未修改系统隐私数据库、未关闭安全机制。
- 更新入口仍显示公开稳定版 0.2.4 的提示，已关闭浮层，未下载安装旧版。当前 0.2.5 为本地验收包，未公开发布。
- 新增账户独立窗口流程已在隔离 Electron 和本机 Chrome 测试通过；真实账户添加、A → B → A 切换及原有任务续接仍待用户验收。未操作真实账户的登录、保存或切换。
