# Codex 第四关真实界面实测（run-5）

2026-11-15，在隔离数据与 CODEX_HOME、开发态 Electron、Codex 0.162.1 下，实测载入历史会话、重启后接回记忆、当前工作树 Fork 和父子对话隔离。以下是本轮事实，不代表第五关的打包或完整交互矩阵验收。

## 重启与静置

工作区恢复后静置约 15 秒，原父卡 `4b0d30fe-adbd-4ef0-9dba-0f08a3f2a865` 显示历史及输入提示，没有“已恢复”提示，也没有恢复横幅；`sessions.status=running`、`work_status=starting`，binding `resume_state=available`、`restore_state=none`。binding 的更新时间和验证时间仍为 14:53:55，`lastHookEvent=Stop` 是旧值，不能视为本轮钩子确认。按本轮裁决，这是一种启动中过渡状态。截图 `run5-restart.png` 同时显示另一张关联同 UUID 的卡仍在恢复，并提示该对话已在其他应用打开。

父 UUID 为 `01a1ddfb-09b3-7e41-a492-19e8854bf29e`。随后在父卡提问，问题未包含暗号，新 turn `01a1deef-53ae-7371-8d03-91b6044e86c2` 的 `final_answer` 和同 turn `task_complete` 均为 `amber-orbit-482`。真实 Stop 后 binding 更新至 19:19:52，work_status 转为 idle，因此记忆接回有新对话与钩子证据。

## Fork 与隔离

从完成正常回复的父卡选择“使用当前工作树”创建新子卡 `a767c315-7f15-45db-9021-5ba680709382`；新子 UUID `01a1def0-2cf3-7e30-b5ba-b0f477dd0f53`，其真实 rollout `session_meta.forked_from_id` 为父 UUID、cwd 与父卡一致；intent 的 state/stage 均为 `succeeded`，worktree_mode 为 `current`。父 binding UUID 和标题 `Remember secret phrase` 保持。首次子真实钩子之后子 binding 的 `codexForkAwaitingHook=false`。

- 子卡记住 `child-silver-731`：turn `01a1def0-a667-7de3-b35e-04110f364eff` 的 final_answer 和 task_complete 均为 `ACK`。
- 子卡被问及记住的所有暗号：turn `01a1def0-fee6-7682-991a-86d2cae18a85` 的 final_answer 和 task_complete 均列出 `amber-orbit-482` 与 `child-silver-731`，截图 `run5-child.png`。
- 父卡被问及记住的所有暗号，问题未泄露子暗号：turn `01a1def1-9800-7f63-a03b-ea967235a170` 的 final_answer 和 task_complete 均仅为 `amber-orbit-482`。

这验证了该次父子对话继承与隔离；此前失败子卡 `9a80b19d-9062-442e-a77f-7cb54e5c24d6` 不计入成功。

## 载入

父子验证后，从原父卡正常输入 `/exit` 释放会话，再在子卡打开载入入口，选择 Codex，按父 UUID 筛选得到 1 条；在正文搜索 `amber-orbit-482` 得到 4 处并预览（`run5-search.png`）。另一张卡仍关联同一 UUID，界面提示重复载入，按产品提供的“仍然载入”确认（`run5-load-confirm.png`）。实际绑定到目标卡的是父 UUID，初始 `restore_state=restoring`，终端可输入且未出现占用锁。

随后提问未包含暗号，新 turn `01a1def3-9ed6-7661-b9f3-814bb30c34f9` 的 final_answer 和 task_complete 均为 `amber-orbit-482`；该卡父 UUID binding 更新至 19:24:35、restore_state 转 `none`，截图 `run5-loaded-reply.png`。被替换的子 UUID binding 保留为 `expired`，其原 rollout 保留供只读检查。另一张原有重复载入卡显示占用提示，没有绕过。

## 首次启动受阻与重测

就绪后首个控制脚本因等待首个可见终端 PID 超时退出，未输入。复制脚本去掉此等待后，窗口超过 30 秒停在“正在打开工作区…正在恢复上次的工作状态”，无可见终端（`run5-startup-wait.png`）。只读进程检查还发现此前隔离 Electron PID 94693 仍存活且使用相同 user-data；两者的因果关系未单独验证。主代理修正恢复队列并清理旧进程、重建后，本轮控制进程正常启动，完成上述重测。

自主决定：遇到加载页先暂停并保留现场；按指示退出本轮控制测试、等待修复。验收时未改产品代码、数据库或伪造 rollout，未升级或安装 CLI；使用当前工作树，在父子对话完成后才用产品现有流程退出父会话并载入。截图不含账号资料。自动测试和完整产品验收由主代理统一报告；未验证打包版本、未登录异常及第五关矩阵。
