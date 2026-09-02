# Matou 内部版性能、稳定性与健壮性终审报告

- 终审日期：2026-09-02
- 工作树：`/Users/icesword/Documents/AIProjects/matou/.worktrees/internal-hardening-audit`
- 分支：`codex/internal-hardening-audit`
- 终审代码基线：`a606b93`

## 1. 产品结论

本轮已闭合内部版发布前的性能、稳定性和异常恢复工作。用户在大量工作空间、事项、会话和深层关系下，可以先进入当前工作现场；其他会话按优先级恢复。单张卡片、单个 Journal、单个 Worktree 或单个 Provider 恢复失败时，不会阻断其他工作。

本轮最后发现并修复了四个会破坏用户信任的恢复边界：同级会话离屏返回时历史重复；错误的 Claude 会话在身份确认前污染原卡片的 HUD、通知或 DAG；Fork 身份一直不返回时永久停在恢复中；Fork 准备期间源分支继续提交导致分支起点漂移。现在离屏终端复用同一 xterm 模型和 PTY，只补齐尚未显示的 Journal 序列；恢复身份确认前，PTY 输出只能进入终端画面和受限缓冲，不会改写卡片摘要、工作目录或工作状态，身份错误或超时后缓冲立即丢弃；过期 Fork 进程的后续 Hook 也不会产生 HUD、通知和 Team DAG；Fork 身份有明确截止时间；新工作树固定使用用户提交 Fork 时的不可变 commit。

当前版本可作为 **macOS 内部验收候选版**。对外分发仍需要正式签名、公证和独立安装升级验证；Windows/Linux 宿主未纳入本轮实机结论。

## 2. 用户场景结果

| 场景 | 用户结果 | 终审结果 |
|---|---|---|
| 启动恢复 | 当前工作空间、事项、场景和活动会话优先可用；其余会话有界恢复并逐卡显示阶段 | 通过 |
| 大量同级会话 | 横向滑出视野仍属于前台；PTY 和 VT 模型保持，卡片 DOM 可虚拟化 | 通过 |
| 离屏持续输出 | 返回卡片后只补齐缺失输出，历史不重复，原 PID 不变 | 81 会话、连续 3 次往返通过 |
| 长终端历史 | 最近 10,000 行即时显示，更早压缩历史仍可搜索并查看上下文 | 通过 |
| Claude Code 恢复 | 身份确认前不改写 HUD、通知、DAG 摘要、cwd 或工作状态；错绑时终止错误进程并在原卡片显示可重试失败 | 真实 Claude、错误身份与过期 Fork owner 故障注入通过 |
| Shell 恢复 | 保留已完成历史和 cwd；未完成命令不自动重跑 | 通过 |
| Fork / Worktree | 点击创建时冻结代码版本；创建可跨 Runtime 崩溃继续；身份超时或环境缺失均保留会话与关系 | 7 个崩溃点与基线漂移回归通过 |
| 分离终端 | 关闭独立窗口后同一会话回到原画布并保持可输入；瞬时 Runtime 故障有界重试 | 通过 |
| 数据库异常 | 较新 schema 以只读模式浏览历史；不启动进程、不接受写入；损坏进入恢复流程 | 安装包通过 |
| 单会话存储异常 | 只暂停受影响会话，其他会话继续；修复后从原序列续写 | 通过 |
| 极深关系 | 5,000 层链无递归溢出；10,000 节点 DAG 聚合、搜索和定位保持可操作 | 通过 |

## 3. 性能与容量证据

测量主机：Apple M4 Pro、14 CPU、48 GiB；真实 Electron 窗口全部放置在内建 `Color LCD`，外接主屏 `XV272U` 未承载自动验收窗口。

| 场景 | 终审实测 | 门槛/结论 |
|---|---:|---|
| 50 个同级会话滚动 | frame p95 9.2 ms，max 50.4 ms | 通过 |
| 200 个同级会话滚动 | frame p95 9.1 ms，max 17.1 ms | 通过 |
| 1,000 个同级会话滚动 | frame p95 9.1 ms，max 16.7 ms | 通过 |
| 32 工作空间 / 249 事项 / 1,992 会话恢复 | 1,638 ms | `<5,000 ms`，通过 |
| 跨工作空间事项切换 | 325 ms | `<1,500 ms`，通过 |
| 10,000 节点 DAG 首次可操作 | 276 ms；观测打开 487.92 ms | 交互门槛 `<300 ms`，通过 |
| DAG 聚合定位 | 89.42 ms | `<300 ms`，通过 |
| DAG 连续手势 | frame p95 9.1 ms；输入 p95 8.9 ms | `≤16.7/34 ms`，通过 |
| DAG 远端搜索 | 16.38 ms | `<100 ms`，通过 |
| 20 个真实终端持续输出 | frame p95 9.0–9.1 ms；最大未确认 24,624 bytes | 无 Long Task，通过 |
| 6 PTY × 320 MiB 历史 | event-loop max 25.25 ms；输入 p95 33.48 ms | 4/4 压力场景通过 |
| 长历史恢复 | 首屏 1,652.77 ms；压缩搜索 257.32 ms | 通过 |

重压连续执行中曾出现一次 event-loop 135.27 ms 和一次混合重复套件 180 秒超时；相同场景隔离复验分别恢复到 28.43/25.25 ms，并连续 3 次通过。终审结论采用完整门禁与隔离复验共同判断，不把单次最好值替代重复证据。

## 4. 最终验证矩阵

| 验证 | 结果 |
|---|---|
| `pnpm test` | Desktop 528、Runtime 747、Contracts 55、Domain 7、标识符 2，全部通过 |
| `pnpm typecheck` | 全仓通过 |
| `pnpm build` | 生产构建通过 |
| `pnpm check:identifiers` | 通过 |
| `pnpm test:e2e` | 90/90 真实 Electron 流程通过 |
| `pnpm test:e2e:real-claude-storage-resume` | 真实 Claude Code 2.1.251，1/1 通过 |
| 离屏终端与损坏历史门禁 | 81 会话离屏连续 3 次往返；损坏冷分片 1/1；同 PID、无重行/丢行 |
| `pnpm test:robustness` | 1/1 通过 |
| `pnpm test:scale` | 7/7 通过 |
| `pnpm test:dag-10000` | 真实 10,000 节点 1/1，相关单元门禁通过 |
| `pnpm test:e2e:recovery-scale` | 20 个真实 PTY 与 5/20/100 层级，1/1 通过 |
| `pnpm test:runtime:pty-stress` | 4/4 通过 |
| `pnpm test:e2e:long-terminal-history` | 1/1 通过 |
| `pnpm test:e2e:fork-crash-recovery` | 7/7 通过 |
| `pnpm test:packaged` | 打包后 SQLite、PTY、历史回放、断尾修复通过；未来 schema 只读时历史仍可见，输入不启进程且数据库 bytes 不变 |
| 独立终审 | 最终 diff 未发现 P0/P1 阻断项；身份副作用门与错误身份后续输出均复核通过 |

## 5. 仍需保留的发布边界

1. 当前安装包用于内部验收，macOS 正式签名和公证不在本轮范围。
2. Windows/Linux 的 Shell、路径拖入、原生窗口和安装包需要对应宿主实机回归后再扩展平台结论。
3. 终端即时视图保留最近 10,000 行；更早内容从同一搜索入口进入只读历史上下文，这是已确认的产品边界，不属于数据丢失。
4. DAG 远层以聚合卡片呈现，进入附近范围或搜索命中时展开真实会话，这是 10,000 节点下保持可操作性的已确认取舍。

## 6. 证据索引

- 产品决策：`docs/audits/2026-09-01-internal-hardening-product-decisions.md`
- 原始分析：`docs/audits/2026-09-01-internal-hardening-analysis.md`
- 大规模验收：`docs/acceptance/scale-dag-performance.md`
- 规模实施台账：`docs/audits/implementation-scale-ledger.md`
- 终端健壮性台账：`docs/audits/implementation-runtime-robustness-ledger.md`
- 关键真实流程：`tests/e2e/terminal-offscreen-continuity.spec.ts`、`tests/e2e/terminal-long-history.spec.ts`、`tests/e2e/fork-runtime-crash-recovery.spec.ts`、`tests/e2e/packaged-runtime.spec.ts`
