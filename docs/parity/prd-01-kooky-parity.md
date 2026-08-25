# PRD 01 Agent 通知：Kooky CLI 模块对照矩阵

## 对照边界与产品决策

- 只对照黑色 CLI 模块内的工作区 / 事项侧栏、页签、终端面板、通知入口与通知中心；Kooky 白色产品导航、Logo、登录引导和 Matou 窗口外壳不参加验收。
- 需求完整性以 `01-agent-通知.md` 为清单；同范围可见样式、入口、反馈时机和状态迁移以当前可运行 Kooky 及其现存代码为基线。
- 2026-08-25 产品确认采用方案 A：当前 Kooky 行为优先，PRD 只补充 Kooky 尚缺的能力。下表将 PRD 与 Kooky 冲突处明确记录为已确认产品基线，不再复制 PRD 旧描述。

## Kooky 交互对照矩阵

| 用户场景 | 当前 Kooky 基线 | Matou 实际结果 | 运行 / 自动化证据 | 差异结论 |
|---|---|---|---|---|
| 非聚焦面板收到 Agent 通知 | 面板出现蓝色脉冲边框；事项显示红色数字徽标；页签与通知入口出现红色动静提示 | 同步点亮面板、页签、事项、工作区与全局入口；声音开启且事件允许时播放提示音 | `matou/unread-ring.png`；PRD 01 Electron 场景 1；store sound tests | 一致 |
| 聚焦面板收到通知 | 条目直接记为已读，不响声、不增加上层未读；面板蓝环仍出现，点击面板后消失 | 同行为；不采用 PRD 的“一次短闪后自动消失” | PRD 01 Electron 场景 2；`notification-ui-integration.test.tsx` | 按方案 A 一致 |
| 点击有动静的面板 | 删除该面板全部通知与蓝环，而不是只把它们标为已读 | 同行为；事项 / 页签未读随剩余通知重算 | `AgentNotificationStore.test.ts`、UI integration | 一致 |
| 同面板再次来通知 | 旧未读转为已读，最新一条保持未读；历史条目仍可回溯 | 同行为 | `AgentNotificationStore.test.ts` | 一致 |
| 五秒冷却 | Kooky 按 `Stop`、`Notification`、`OSCNotification` 三类来源分别限流；同一 `Notification` 来源即使文案分类不同也共享冷却 | 同一 Session + 同来源分类五秒内只接收一次 | provider mapper + store tests | 一致 |
| Agent 语义事件 | Claude `Stop` 与 `Notification` 产生通知；`SessionEnd` 只更新会话上下文，不再产生重复通知 | Runtime 同事务写入语义 Outbox，立即推送 Renderer；`SessionEnd` 被忽略 | provider hook / repository / runtime-server tests | 一致 |
| 终端主动通知 | OSC 9 / 99 / 777 进入 `OSCNotification` 通道，使用终端原文并限长 | 同行为；Journal replay 期间抑制重复生成 | `osc-notification.test.ts`、`TerminalSurface.tsx` | 一致 |
| 四层未读回溯 | 工作区下拉有红点、事项有红色数量徽标、页签含红点、面板有蓝环 | 同行为；溢出页签菜单也保留红点 | Kooky `ClaudeCodeView.vue`；split UI / notification UI tests | 一致 |
| 切换工作区 | 当前 Kooky 会把被选择工作区的全部未读标为已读 | 同行为；不采用 PRD“只聚焦具体面板才清理”的旧规则 | store / UI integration tests | 按方案 A 一致 |
| 打开通知中心 | 位于工作区头部下方的左侧浮层，宽 382px，距 CLI 底部 5px，背景 `#212121`、圆角 12px | 同位置、尺寸、颜色、圆角与进入动效 | `kooky/notification-center.png`、`matou/notification-center.png` 及两份 JSON | 一致 |
| 收起通知中心 | 再点入口、关闭按钮、点击外部或按 Esc 均关闭；通知数据保留 | 同行为；不采用 PRD“外部点击不关闭”和“右侧滑入”旧描述 | PRD 01 Electron 场景 3；UI integration | 按方案 A 一致 |
| 通知中心头部 | 显示 `通知 (N)`、清空和关闭；当前界面没有“全部已读” | 同行为 | 双应用运行截图；`NotificationCenter.test.tsx` | 按方案 A 一致 |
| 通知条目 | 新到旧排列；显示工作区 / 事项、来源标题、副标题、正文、时分；正文最多四行；悬停显示单条关闭 | 同信息层级与四行截断；Claude 来源显示 `Claude Code` | 双应用运行截图 / JSON；center tests | 一致 |
| 团队通知补充信息 | 当前普通通知不显示团队信息；store 已保留角色 / 状态数据通道 | 普通通知不显示；有团队角色时显示角色，非 idle 状态按语义色显示 | `NotificationCenter.test.tsx` | PRD 缺口已补，普通场景无视觉差异 |
| 点击通知跳回现场 | 成功时按工作区 → 事项 → 页签 → 面板定位，关闭中心并删除该条；目标面板不存在时保留条目并提示 | 同行为；会先尽力切到仍存在的最近上层 | PRD 01 Electron 场景 1；UI integration 多场景 | 一致 |
| 清空 / 单条关闭 | 清空后显示 `暂无通知`；单条关闭只移除该条并重算徽标 | 同行为 | center / integration tests | 一致 |
| 空态 | 使用 Kooky 空态图片与 `暂无通知`，只保留声音开关和关闭入口 | 复用同一 Kooky 资源与文案 | `no-xiaoxi.png`；center tests | 一致 |
| 声音开关 | 底部 32×16 橙色开关，默认开启；使用 `kc-notification-sound-enabled` 跨应用会话记忆 | 同开关、存储键和两音提示；播放失败不影响视觉链路 | PRD 01 Electron 场景 3；browser store / sound tests | 一致 |
| 应用重启 | 通知、未读和蓝环归零；声音偏好保留 | 通知 store 仅在 Renderer 会话内存在；声音存 LocalStorage | store / browser preference tests | 一致 |
| 热更新 / 重放 | 历史语义事件不重新消费，终端恢复输出不再次生成 OSC 通知 | 语义投影从当前 sequence 继续；OSC handler 在 replay 完成后才放行 | projection path + `TerminalSurface.tsx` | 一致 |
| 独立窗口 | 独立窗口自身不显示通知 UI；事件仍按原归属汇总到主窗口 | detached mount 不渲染通知面板蓝环，主窗口上层与中心仍可汇总；点击时按“不在当前窗口”提示 | UI integration detached scenario + hierarchy ownership test | 一致 |
| 最后容器与窗口隐藏 | 窗口隐藏不会结束 Renderer 会话，因此通知上下文仍保留 | 沿用已验收的 PRD 05 / 04 窗口隐藏链路 | lifecycle / recovery Electron tests | 一致 |

## 当前 Kooky 代码基线

- 通知状态、覆盖、冷却、聚焦指示、声音偏好：`src/modules/terminal/stores/notification.js`。
- 通知中心层级与当前视觉：`src/modules/terminal/components/NotificationCenter.vue`。
- 工作区入口、工作区切换已读：`src/modules/terminal/components/ProjectDropdown.vue`。
- 事项徽标、页签 / 溢出页签红点、面板蓝环与导航：`src/modules/terminal/components/ClaudeCodeView.vue`、`NotificationRing.vue`。
- Claude Hook 文案与事件映射：`electron/hook-server.js`、`src/modules/terminal/utils/claudeNotificationContent.mjs`。
- OSC 9 / 99 / 777 与终端回放：`src/modules/terminal/components/ClaudeCodeTerminal.vue`。
- 两音提示：`src/modules/terminal/utils/notificationSound.js`。

## 双应用运行证据

- 当前 Kooky 通知中心：`docs/acceptance/evidence/prd-01/kooky/notification-center.png`。
- 当前 Kooky 几何 / 样式采样：`docs/acceptance/evidence/prd-01/kooky/notification-center.json`。
- Matou 四层未读与蓝环：`docs/acceptance/evidence/prd-01/matou/unread-ring.png`。
- Matou 通知中心：`docs/acceptance/evidence/prd-01/matou/notification-center.png`。
- Matou 几何 / 样式采样：`docs/acceptance/evidence/prd-01/matou/notification-center.json`。

Kooky 运行采证使用当前源码和隔离应用数据；为只呈现用户指定的黑色 CLI 对照范围，采证时移除了覆盖在 CLI 上方的外层登录引导，未修改 Kooky 源码或 CLI 内部样式。Kooky 与 Matou 的中心均实测为 382px 宽、`rgb(33, 33, 33)` 背景、12px 圆角；页面错误列表均为空。
