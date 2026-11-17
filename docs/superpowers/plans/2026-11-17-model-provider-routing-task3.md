# Task3 交付：多目标出口保护与未用源账号解耦

日期：2026-11-17。本批仅本 worktree；未提交、安装、发布，无真实账户/钥匙串访问、外部探测或付费请求，无新增依赖。保留 Task1/2 累积改动。

## 用户可见结果

- 一个会话关联三档角色和自定义压缩的全部实际官方出口；任一异常冻结，另一出口正常、恢复或单独豁免不能放行仍有异常的会话。原有恢复二次确认和单目标行为保留。
- 禁用保护可恢复会话；注销清理全部成员和冻结状态。保存新配置不改变运行中的出口集合。
- 网关在发出消息、计数或压缩请求前检查实际目标。被保护的请求保留明确出口保护原因和网络检查操作提示，上游不收到请求。
- 三档全部指定其它合法目标时，未登录的默认官方账号、无 key 的默认 API、需要重新登录的默认订阅都不再阻塞准备。默认订阅不被无谓准备 lease，也不会先经过旧 launchSelection 元数据/账号路径。
- 未用的官方源不读取身份。晚选未在启动时绑定的源自定义 model 明确失败，不临时采用新账号/新 key，不 fallback；主模型明确为不可用源自定义 model 时启动前拒绝。
- 桌面沿用已有 chip/banner 视觉，逻辑优先集合内的异常成员；多出口共享一个冻结会话时总数去重。

## 本批文件清单

| 文件 | 本批变化 |
| --- | --- |
| apps/runtime/src/egress/egress-monitor.ts | 单值/数组注册，内部去重集合，wire 双字段，全成员恢复/override检查、注销/轻采样membership、发送前状态检查 |
| apps/runtime/src/egress/egress-monitor.test.ts | 多目标交叉恢复、单独豁免、全部确认、禁用、清理、轻采样覆盖；保留原单目标回归 |
| packages/contracts/src/egress.ts | 可选 sessionFamilySets；旧 sessionFamilies 保留首成员 |
| apps/runtime/src/runtime-server.ts | 准备优先于旧源选择；实际角色/压缩集合一次注册；传网关保护回调；按实际目标隐藏时区；旧无分配 custom 压缩亦加入出口 |
| apps/runtime/src/runtime-server.test.ts | 真 runtime+PTY+本地HTTP：未登录源、晚选拒绝、多出口、custom-only压缩、冻结与0转发、关闭清理、保存集合不变 |
| apps/runtime/src/session/claude-role-launch.ts | 移除无条件source requirement；源连接可选；不为未用源读取OAuth identity/启动订阅；源自定义主模型提前校验 |
| apps/runtime/src/session/claude-role-launch.test.ts | 官方未登录/API缺key/订阅needs-sign-in的全分配准备；源自定义主模型拒绝 |
| apps/runtime/src/session/claude-model-routing.ts | 有角色快照但无绑定source时明确拒绝；旧custom压缩透传官方family |
| apps/runtime/src/session/claude-model-routing.test.ts | 对齐旧订阅压缩新增family，保留原目标与lease断言 |
| apps/runtime/src/session/claude-role-routing.test.ts | 无绑定source的真实HTTP拒绝与零回落；旧fixture显式提供合法source连接 |
| apps/desktop/src/renderer/src/egress/use-egress-status.ts | 优先集合，冻结任一成员判断、优先异常成员、集合内恢复反馈；兼容旧wire |
| apps/desktop/src/renderer/src/egress/use-egress-status.test.ts | 主成员正常/豁免时其它异常仍阻塞，次成员恢复可显示 |
| apps/desktop/src/renderer/src/egress/egress-copy.ts | 全局冻结会话数按sessionId去重 |
| apps/desktop/src/renderer/src/egress/EgressChip.test.tsx | 同session出现在两个family时不重复计数 |

## 接口和执行细节

- `registerSession(sessionId, EgressFamily | readonly EgressFamily[])`：内部复制到 Set，运行时单目标仍传字符串，多目标传数组，不重复注册、不伪造sessionId。
- `sessionFamilySets?: Record<string, EgressFamily[]>`：新消费者优先集合；旧 `sessionFamilies` 保留确定的首成员。状态校验器原本透传对象，不需要改变协议版本或增加新存储。
- 一次sample先更新所有family的override有效性，再计算本轮已满足二次确认的family，恢复前检查整个会话集合，防止顺序处理A/B误放行。
- `EgressMonitor.assertTargetAllowed(family?)`：只读取既有evaluation/settings/override，不触发探测；无family或禁用保护保持原行为。runtime捕获保护错误并转换为 `GatewayRoutingError`，避免通用502吞掉原因。
- 角色启用时集合严格来自 `snapshot.officialFamilies`（实际roles+custom压缩）；没有角色分配的旧custom压缩补充其官方family，保持旧压缩选择流程。不会因默认第三方推断官方出口。
- 角色路径的时区隐藏由实际targets的hideTimeZone合并，使用既有隐藏策略，不指定某个默认出口国家代表多个出口；未改时区设置或监控探测器。
- `sourceConnection` 为可选启动快照：API有key可保留原快照，OAuth只复用实际target已绑定的身份，订阅只复用实际target已准备的对应连接；没有绑定就拒绝显式source请求，不在请求时读可变store重新绑定。
- 无源连接时给 CLI 本地网关占位 API key，避免CLI要求未用官方账号登录；它不是供应商凭据，角色override清除该头并注入实际目标凭据。无源上游为loopback关闭端口，显式model请求在路由处明确拒绝。

## 红绿与自检证据

日志位于 `.cache/task3-*.log`；另复制到监督运行目录 `logs/`。不重复累加中间轮次。

| 证据 | 结果 |
| --- | --- |
| task3-red.log | 实施前5失败、35原有通过：两项多family、三类未用source |
| task3-core.log | 初步核心51通过 |
| task3-runtime-final.log + task3-routing-final.log + task3-source-final.log | 18个相关runtime文件最终共223项通过；17项既有router集成跳过。首轮一个旧精确对象断言遗漏新增family，修正后该文件8项全通过；source测试输入改为needs-sign-in后11项再通过 |
| task3-all-targets-final.log | 三档与custom压缩全部指定第三方后，三类未用source准备+真实runtime补验4通过；该定点命令182项筛选排除，不重复累加 |
| task3-lifecycle-green.log | runtime-server相关44通过、131筛选排除，含Task2保存/附加/恢复与本批3个runtime场景 |
| task3-desktop-final.log + task3-desktop-chip-final.log | 桌面egress目录与RuntimeClient共93项通过；计数fixture增加跨family重复session后对应25项通过 |
| task3-contracts-final.log | contracts全部9文件105项通过 |
| task3-build-packages.log | 基础包构建通过 |
| task3-typecheck-runtime-final.log / task3-typecheck-contracts-final.log / task3-typecheck-desktop-final.log | 三处typecheck通过；最终all-targets输入补强后runtime再检查通过（task3-typecheck-runtime-delivery.log） |
| task3-identifiers.log | 命名门禁通过；git diff --check通过 |

最终去重合计 **465项通过**（runtime 223+44、desktop 93、contracts 105）；17项既有router集成跳过，另131项runtime-server用例为筛选排除。未跑全仓测试或全App构建。

实际运行证据：

1. 真runtime/PTY启动隔离假CLI，默认官方无身份时不调用captureOfficialClaudeAccount；角色消息确实到达本地HTTP。晚选source返回明确未绑定错误，上游计数不增。
2. 第三方默认+ChatGPT角色+官方压缩注册两个family；真PTY被冻结。直接请求网关的消息、count和custom压缩返回明确保护提示；本地上游从1条保持1条。单独override ChatGPT时另一官方异常仍冻结；禁用后count成功，累计2条。
3. 未启用任何角色分配、仅配置官方custom压缩的旧路径，也注册实际官方family并拒绝受保护的压缩请求。
4. 保存角色配置不改变本轮family集合；退出清空集合，准备租约释放。网关关闭/断流取消沿用既有gateway回归，已通过。
5. 全部监控样本、OAuth身份/header、订阅目标和key为fixture。官方域名目标只在保护拒绝路径使用；没有真实外部HTTP、OAuth、订阅协议转换或真实CLI新探针。

## 中间失败说明

- 新runtime fixture首轮遗漏SubscriptionAccounts.onChange桩，补齐后通过；未因此改产品接口。
- 生命周期回归两项旧断言要求单family字符串；保留单目标原调用形式后44项通过。首轮断言提前退出导致的database-closed清理警告未出现在最终通过日志。
- 旧订阅压缩精确对象断言未包含新增family，补充预期后8项通过。其余相关文件成功结果复用，未重复累加。

## 我自己做的决定

1. 不新增监控器/探测/存储：复用现有sample与family恢复阶段，先更新全体override，再做全成员恢复判断。
2. 旧wire保留首成员；现有chip/banner选实际阻塞成员，冻结会话数去重，不调整视觉布局。
3. 源连接仅复用已捕获API凭据或实际目标准备好的OAuth/订阅连接，不为可选源额外读取身份或准备lease。
4. 无绑定源的CLI仅使用本地网关占位key；显式源选择明确拒绝，避免登录前置或偷偷回落。
5. 没有角色分配的custom压缩也透传并注册实际官方family，避免同一压缩入口因未配置角色而漏保护。
6. 按当次运行约定主代理独立实施，没有派生实现或界面代理。

## 交回边界

Task3代码与本地fixture验收已完成，立即交回审查，未进入模型分配UI。真实界面、真实账户/订阅转换、付费请求、Task2原生子代理缺口仍未验证；不把本批通过作为整个功能验收通过。
