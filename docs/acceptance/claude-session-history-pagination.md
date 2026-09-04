# Claude 会话完整历史验收记录

**日期：** 2026-09-04  
**范围：** 会话管理中的会话列表、历史预览、全文搜索与载入前确认。

## 用户可见结果

- 左侧不再以首批数量作为总量上限，显示“已加载 / 总数”，滚动到底继续加载。
- 右侧默认展示最近 200 条，向上滚动继续加载更早内容；200 是单次批量，不是可见上限。
- 右侧搜索覆盖完整会话，命中计数为完整结果数，上一个/下一个可跨搜索结果页定位。
- 会话与消息卡片按视口虚拟渲染；历史总量增长时，页面节点数量仍按视口范围控制。
- 原有的同会话提醒、运行中卡片确认、模型和权限恢复逻辑保持不变。

## 交互对照矩阵

| 场景 | 对照基线 | Matou 结果 | 证据 | 结论 |
| --- | --- | --- | --- | --- |
| 打开长会话 | 首屏只读取有界的最新消息页 | 显示 `已加载 200 / 400 条` | Electron E2E `session-load-existing-claude.spec.ts` | 一致 |
| 浏览全部历史 | 接近顶部时按页补载并保持阅读位置 | 以 200 条为一页向前补载，恢复原锚点 | `SessionLoaderDialog.test.tsx`、组件状态机 | 一致 |
| 全文搜索 | 搜索完整会话而非当前可见页 | 400 条样本中可定位第 50、350 条 | Electron E2E | 一致 |
| 超长会话 | 索引完整历史，预览保持有界 | 24,000 条精确计数，首批仅解析 200 条，末尾命中可检索 | `claude-session-catalog.test.ts` | 一致 |
| 大列表 | 分页获取，页面只渲染视口附近条目 | 每批 50，会话列表和消息列表均虚拟化 | `SessionLoaderDialog.test.tsx` | 一致 |
| 源文件追加 | 索引随原记录变化刷新 | 文件签名变化后页数据和搜索命中同步刷新 | `claude-session-catalog.test.ts` | 一致 |
| 载入会话 | 保留重复载入提醒和运行中确认 | 原交互及权限恢复回归通过 | 全量单元/集成测试 | 一致 |

## 自动化证据

- `pnpm test`：Contracts 65、Domain 7、Desktop 716+3、Runtime 1064+119+9，末轮全部通过。
- `pnpm typecheck`：全部工作区通过。
- `pnpm check:identifiers`：通过。
- `pnpm exec playwright test tests/e2e/session-load-existing-claude.spec.ts --workers=1`：1 项通过。
- `pnpm package:dir`：目录包构建通过，产物位于 `apps/desktop/release/mac-arm64/Matou.app`。
- `playwright test tests/e2e/packaged-runtime.spec.ts`：打包后 Runtime 场景 1 项通过。

## 证据边界

- Electron E2E 使用生产构建并验证真实 Runtime RPC、完整历史检索和界面跳转。
- 单元测试覆盖 24,000 条会话记录、分页边界、追加失效和视口节点上限。
- 本记录不把静态代码检查等同于人工视觉验收；配色、滚动手感和长文本阅读体验仍以最终 App 验收为准。
