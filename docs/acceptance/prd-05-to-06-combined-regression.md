# PRD 05 → 03 → 04 → 01 → 02 → 06 组合回归台账

状态：前五项已通过产品验收；PRD 06 待产品验收

## 1. 顺序与交付状态

| 顺序 | 模块 | 产品状态 | 独立提交 | reference product 对照 |
|---:|---|---|---|---|
| 1 | PRD 05 四级层级 | 已验收 | `a78a3c5` 及 PRD 05 收口提交 | `docs/parity/prd-05-reference-parity.md` |
| 2 | PRD 03 工作台信息 | 已验收 | `fb7bc3c` | `docs/parity/prd-03-reference-parity.md` |
| 3 | PRD 04 会话恢复 | 已验收 | `2c4ff72` | `docs/parity/prd-04-reference-parity.md` |
| 4 | PRD 01 Agent 通知 | 已验收 | `1029de0` | `docs/parity/prd-01-reference-parity.md` |
| 5 | PRD 02 底部 HUD | 已验收 | `137c28e` | `docs/parity/prd-02-reference-parity.md` |
| 6 | PRD 06 会话 Fork | 待验收 | `1ad08a4` | `docs/parity/prd-06-reference-parity.md` |

## 2. 组合用户旅程

| 用户旅程 | 跨模块结果 | 自动化证据 | 结果 |
|---|---|---|---|
| 首次进入工作区 | PRD 05 建立工作区、事项、页签与 Shell；PRD 02 显示当前 Shell 环境 | `prd-05-hierarchy`、`prd-02-bottom-hud` | 通过 |
| 多事项并行 | PRD 03 保持事项名称、顺序和焦点；PRD 01 将后台 Agent 动静汇总到对应层级 | `prd-03-workbench-information`、`prd-01-agent-notifications` | 通过 |
| Shell 进入 Claude | 同一 Session 从 Shell 进入 Agent HUD；形成 provider identity 后获得恢复与 Fork 资格 | `prd-02-bottom-hud`、`prd-06-session-fork` | 通过 |
| Claude 分叉探索 | PRD 06 在当前 Scene 右侧新增独立会话；PRD 05 保持源面板与布局；PRD 02 跟随新焦点 HUD | `prd-06-session-fork` + Runtime hierarchy / HUD tests | 通过 |
| 分支后台完成 | PRD 01 将非焦点分支的完成 / 求助映射到面板、页签、事项和工作区 | notification integration + fork hierarchy projection | 通过 |
| 关闭一条分支 | PRD 05 只归档目标 Session；源分支进程、层级和 HUD 继续保持 | PRD 06 success flow + lifecycle tests | 通过 |
| 把源会话脱出 | PRD 05 复用同一 Session / PID；PRD 06 在独立窗口隐藏 Fork；PRD 02 继续显示同一 HUD | `prd-05-detached-window`、`prd-06-session-fork` | 通过 |
| 应用重启 | PRD 04 恢复结构、cwd 与 provider identity；PRD 06 各分支独立 resume；PRD 03 恢复事项焦点 | `prd-04-session-recovery`、`prd-06-session-fork` | 通过 |
| 身份失效 | PRD 04 的普通恢复回到 Shell；PRD 06 的 Fork 失败保持右侧错误面板并阻止 Shell 回落 | Runtime resume / fork failure tests + Electron | 通过 |
| 路径失效与恢复 | PRD 05 保留结构并拦截执行；目录恢复后现有会话继续按原层级工作 | `prd-05-path-recovery` | 通过 |

## 3. 当前全量证据

- 单元 / 集成：369 项通过，其中 Contracts 16、Domain 3、Desktop 102、Runtime 248。
- Electron：34 个真实用户场景通过，覆盖 PRD 05、03、04、01、02、06 及 UtilityProcess → MessagePort → xterm 主链路。
- 生产构建与全工作区 TypeScript 类型检查通过。
- 每个 PRD 均保留独立验收文档、reference product 对照矩阵和黑色 CLI 模块运行证据。

## 4. 最终收口条件

PRD 06 产品验收通过后，重新执行一次当前提交上的单元 / 集成、Electron、类型检查与生产构建；结果保持全绿后，将本台账状态更新为“全部通过产品验收”，再结束本轮目标。
