# PRD 01 Agent 通知验收

状态：已通过产品验收

## 1. 用户可获得的结果

- 用户让多个 AI 面板并行工作后，可以沿“工作区 → 事项 → 页签 → 面板”看到完整的动静链路，不必逐个终端巡查。
- 后台面板完成、求助、等待或出错时，会出现蓝色面板光圈、红色上层标记和通知中心条目；声音开启且事件允许时播放与 Kooky 相同的两音提示。
- 正在看的面板保持安静：不响声、不增加上层未读，但保留一条已读历史和面板动静指示；用户点击面板后该面板的通知历史与指示一起清理。
- 用户从通知中心点击一条消息，会直接回到对应工作区、事项、页签和面板；现场已经消失时，应用停在最近可用上层并提示“原面板已不存在或不在当前窗口”。
- 通知中心支持清空、单条关闭、空态、外部点击 / Esc 收起和声音开关；声音偏好跨启动保留，通知列表本身在应用重启后归零。
- Claude Hook 的 `Stop` / `Notification` 和终端 OSC 9 / 99 / 777 均进入同一产品通知链路；`SessionEnd` 不再重复提醒，五秒冷却避免连续刷屏。

## 2. 已确认的产品基线

2026-08-25 产品确认采用方案 A：当前 Kooky 行为优先，PRD 只补齐缺失能力。因此最终用户行为为：

1. 通知中心是工作区头部下方的 **左侧 382px 浮层**，不是右侧抽屉。
2. 点击浮层外部或按 Esc 会收起；当前 Kooky 头部只有清空和关闭，没有“全部已读”。
3. 工作区切换会把该工作区全部未读标为已读。
4. 聚焦面板收到事件时仍显示蓝环；不响声、不增加上层徽标，点击面板后清理。
5. 页签 / 工作区使用红点、事项使用红色数量徽标、全局入口切换为动画螃蟹；正文最多展示四行。
6. 点击通知成功跳转后移除该条；目标已经消失时保留条目并提示。

详细双基线矩阵：`docs/parity/prd-01-kooky-parity.md`。

## 3. 18 项 PRD 验收台账

| # | 用户场景 | 当前用户结果 | 权威证据 | 状态 |
|---:|---|---|---|---|
| 1 | 非聚焦面板完成 / 求助 | 蓝环、红色上层标记、通知条目同步出现；声音策略独立生效 | Electron 场景 1 + sound tests | 通过 |
| 2 | 聚焦面板产生通知 | 已读记录、无声音、无上层未读，保留 Kooky 蓝环直到点击 | Electron 场景 2 + store tests | 通过（方案 A） |
| 3 | 点击有动静面板 | 该面板全部通知、蓝环及对应上层未读一起清理 | Electron 场景 1 / 2 + UI integration | 通过 |
| 4 | 同面板连续通知 | 旧未读转已读，最新一条保持未读 | store tests | 通过 |
| 5 | 五秒内同来源重复事件 | 只保留第一次；不同 Session / 来源互不影响 | store cooldown tests | 通过 |
| 6 | 点击通知跳回现场 | 工作区、事项、页签、面板依次定位，中心关闭，条目移除 | Electron 场景 1 + integration | 通过 |
| 7 | 目标面板消失 / 脱出 | 尽力定位上层，保留条目并显示准确提示 | UI integration | 通过 |
| 8 | 批量清理 | 当前 Kooky 提供“清空”，一次移除全部条目与层级未读 | center tests | 通过（方案 A，无“全部已读”入口） |
| 9 | 清空消息 | 进入 `暂无通知` 空态，清空入口隐藏 | center tests | 通过 |
| 10 | 单条关闭 | 只移除当前条目并重算所有层级 | center tests | 通过 |
| 11 | 关闭声音 | 后续不响声、视觉仍完整；偏好跨启动保留 | Electron 场景 3 + browser sound tests | 通过 |
| 12 | 应用重启 | 通知与未读归零，声音偏好保留 | session-memory store + preference tests | 通过 |
| 13 | 通知中心空态 | Kooky 原图 + `暂无通知`，只保留声音开关 | center tests | 通过 |
| 14 | 切换工作区 | 当前 Kooky 会清理被选工作区全部未读 | UI integration | 通过（方案 A） |
| 15 | 工作区 / 事项被删除 | 条目保留并展示未知占位；其它条目可继续使用 | ingestion / center tests | 通过 |
| 16 | SessionEnd | 不产生第二条可见通知 | provider mapper / hook tests | 通过 |
| 17 | 热更新 / Journal 重放 | 历史语义事件和 OSC 输出不重复生成通知 | projection sequence + replay suppression | 通过 |
| 18 | 窗口隐藏 | Renderer 会话继续，通知与徽标保持；重新显示仍可操作 | 已验收 lifecycle / recovery Electron tests | 通过 |

## 4. 当前运行与自动化证据

- 当前 Kooky 与 Matou 的通知中心均完成真实 Electron 运行采样；两者中心实测为 382px 宽、`#212121` 背景、12px 圆角，列表层级、卡片、清空、关闭、底部声音开关一致。
- Matou Electron 新增 3 个通知场景：四层未读与现场跳转、聚焦静默与点击清理、外部 / Esc 关闭和声音偏好。
- 完整工作区共 322 项自动化测试通过：Contracts 15 项、Domain 3 项、Desktop 79 项、Runtime 225 项。
- 类型检查与生产构建通过；全量 Electron 回归 30 个用户场景全部通过，其中 PRD 01 新增 3 个通知场景。
- 运行截图与几何采样：
  - `docs/acceptance/evidence/prd-01/kooky/notification-center.png`
  - `docs/acceptance/evidence/prd-01/kooky/notification-center.json`
  - `docs/acceptance/evidence/prd-01/matou/unread-ring.png`
  - `docs/acceptance/evidence/prd-01/matou/notification-center.png`
  - `docs/acceptance/evidence/prd-01/matou/notification-center.json`

## 5. 产品验收建议

重点体验三条路径即可：

1. **后台完成**：观察非聚焦面板蓝环、页签 / 事项标记和动画通知入口是否一眼可发现。
2. **全局回溯**：打开通知中心，点击条目后确认直接回到原面板且浮层让位。
3. **专注与安静**：当前面板收到事件时确认没有声音与上层红点；关闭声音后重新打开应用，开关仍保持关闭。

PRD 01 的实现与 Kooky 双基线矩阵已闭合，等待产品验收。
