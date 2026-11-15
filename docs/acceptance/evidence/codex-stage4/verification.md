# Codex 第四关实现与验证

2026-11-15，分支 `feat/multi-agent-codex`。本关要求的载入、重启接回和当前工作树 Fork 父子隔离已取得开发态真实对话证据。产品 PRD 继续处于实现阶段：第五关打包与 reference product 完整交互矩阵未开始，不能当作完整产品验收。

## 已修改与提交

- `864c967`：Codex 本地会话目录、列表/详情/搜索/载入路由、Codex 页签和载入资格。列表只读首条 `session_meta`，按 canonical cwd 精确过滤，以元数据 UUID 为身份；标题取 `session_index.jsonl` 最新标题。正文按需建立流式偏移索引，复用分页和有界缓存，不把整份历史放进内存。
- `853bdb2`：打开 Codex 载入、Fork、自动恢复能力；保留原生 `resume <UUID>` / `fork <父UUID>` 数组参数。拒绝父 UUID、无效身份和缺失身份，补齐当前 run、Fork authority 与超时护栏。
- 第四关文档与真实证据作为独立提交保存；没有推送、改版本号或发布安装包。

## A 裁决落地

1. 启动前记录当前 run 开始时间与父 UUID。只接受 `session_meta.timestamp` 晚于 run 开始、`forked_from_id` 精确等于父 UUID、canonical cwd 与卡片一致的子候选；不使用最近修改文件兜底。
2. 恰好一个候选才通过已有带租约校验的事务写入绑定并完成 Fork。0 个或多个候选继续等待真实钩子，沿用原身份超时。元数据轮询不能延长原超时；迟到读取结果在写入前重新核对 run/计时器/进程归属。
3. 候选完成后保留 `codexForkAwaitingHook` 和已有 operation/run/fence 元数据。首次真实钩子不同则转失败、作废候选绑定并显示现有失败反馈，不能静默改绑。相同则清除待确认标记。普通成功 Fork 的终态规则没有放宽；待确认候选在重启后仍可按同一来源校验撤销。
4. 恢复时，可交互进程/输出只释放启动队列，使终端能显示并接受用户第一条消息；不授予本轮身份确认。Codex 完整屏幕不终止失败监视，真实钩子之前保持启动/恢复过渡。既有 Claude / Antigravity 的就绪方式保持原样，没有注入额外 LLM 提示来触发钩子。
5. 同时在外部 Fork 同一父会话可能产生多个合格候选，从而等待真实钩子或超时失败；按裁决不加锁。这项边界必须进入第五关交互矩阵。

## 自动验证

- 相关 runtime 七组 **132 条通过**：catalog、provider hooks、provider mode、RPC router、launch plan、resume monitor、Fork intent repository。日志 `.cache/c4/stage4-related-runtime.log`。
- 最终 runtime 定点 **9 条通过，150 条未选中**：唯一候选成功、多候选超时、父/缺失身份失败、首次钩子不符、候选已完成后返回父身份、待确认候选重建钩子后的不符、慢扫描超时且迟到结果不得绑定、恢复屏幕不能确认身份。cwd/父来源/创建时间拒绝在 catalog 测试覆盖。日志 `.cache/c4/stage4-runtime-focused.log`。
- runtime-server 整组首次 **157/158 通过**；时区启动用例首次读到空值而失败。未改时区产品代码，单独与相关用例复跑 **9 条通过**，包括该时区用例；保留首次波动，不宣称整组一次全绿。新增慢扫描用例在随后最终定点执行通过。整组日志在任务运行目录 `logs/stage4-runtime-run5.log`，复跑日志 `.cache/c4/stage4-runtime-recheck.log`。
- Desktop 三组 **162 条通过**（前一轮结果；这些 UI 源码之后未变，复用结果）；本轮载入弹窗 **14 条复测通过**。Domain manifest **5 条通过**。
- `pnpm typecheck` 通过；最后 runtime 定点调整后再跑 runtime typecheck 通过。`pnpm check:identifiers`、`git diff --check` 通过。开发态 runtime 和 desktop 构建通过；没有进行新一轮打包。
- 类型检查的 package rebuild 与测试分开执行，避免清理 dist 时触发导入竞争。

## 真实行为

详细步骤、截图与每个新 turn 的 `final_answer` / 同 turn `task_complete` 在 [ui-run5.md](ui-run5.md)。主代理回读关键 rollout，并检查载入回复与子暗号截图；不是只根据历史回显验收。

- **静置恢复与接回**：真实启动后静置约 15 秒，父卡本轮 `work_status=starting`，binding 验证时间仍旧，没有“已恢复”提示；随后不在问题中提供暗号，新 turn 正确返回 `amber-orbit-482`，真实钩子后转 idle。原父卡没有恢复横幅，不能把旧 `available` / `Stop` 当成本轮成功。
- **Fork 与隔离**：当前工作树新子 UUID `01a1def0-2cf3-7e30-b5ba-b0f477dd0f53`，真实元数据父来源与 cwd 匹配，intent succeeded；父身份与标题不变。子能返回继承的父暗号和自己的 `child-silver-731`，父仅返回原暗号；首次子钩子后待确认标记为 false。
- **载入**：Codex 页签按父 UUID 筛选、正文搜索与预览，按产品重复载入确认流程完成实际绑定；输入前 restoring，新的无暗号问题得到正确回复后转 none。被替换的子绑定 expired，原 rollout 保留。
- 首轮受阻时先停下保留截图。旧隔离 Electron 收到 TERM 后仍在，已核对路径并停止其进程树；恢复队列也修正为允许终端先交互。两项一并修复后重测通过，没有单独证明哪一项造成全屏加载页。

## 未闭合项

- 第五关真实打包 App、完整 reference product 交互对照矩阵、未安装/未登录异常与新 Worktree 的真实 Fork 没有在本轮验证。
- 载入实测覆盖父会话。当前 catalog 索引当前 rollout，尚未展开 Codex `history_base` 引用的祖先历史；Fork 子会话的历史预览/搜索可能只呈现当前文件内容。这一格式边界尚未闭合，不宣称全部 Codex 历史格式均已完整支持。
- 多候选、cwd/父来源不符、后续身份不符以自动验证覆盖，没有伪造真实钩子作为 UI 实测。

## 清理与自主决定

- 创建时间采用元数据的会话创建时间；复用现有目录缓存、绑定元数据和超时，不新增存储表、依赖、锁或额外 LLM 请求。
- 候选的撤销仅允许待首次钩子确认且 operation/run/fence 一致的 Codex 绑定；真实匹配后不再保留撤销资格。
- 启动队列就绪与对话恢复确认分开：前者允许终端交互，后者仍只接受真实钩子。
- 界面代理 `gpt-6.1-terra / high` 决定：受阻先保留现场并停测；用当前工作树，父子验证后才正常退出父会话并执行现有载入流程；没有修改数据库或伪造 rollout。详见 UI 记录。
- 本轮控制测试与隔离 Electron 已退出。删除隔离 `auth.json`、配置/插件/缓存/临时运行副本与旧 Electron user-data，释放 **461,712 KiB（约 451 MiB）**；保留 Matou 测试数据库、rollout 和必要证据，`.cache/c4` 余约 60 MiB。没有清理其他项目或真实用户备份，本轮未生成重复安装包目录。

此前 CLI 误升级及恢复记录保留在 [live-probe.md](live-probe.md)，不因本轮重测通过而抹去。
