# 第五批：分流、压缩预算与订阅换号修复

2026-11-17。本 worktree 内完成三项审查问题的实现与本地验证，交回审查关卡。没有提交、安装、发布、新依赖、真实账号登录、钥匙串读取或付费模型请求。本批不改界面、原型、主仓库和生产配置。

## 用户结果

1. `vendor/claude-sonnet-custom`、`claude-sonnet-private` 保持原供应商与原模型，消息和 count 都不再误投分配的 Sonnet。角色仅识别精确公共/虚拟选择器、已有订阅选择器、已捕获 native catalog 的具体模型；保留已定义 Fable 隐藏选择器。
2. 三档 1M、custom 压缩 128K 时，会话预算为 128K；压缩供应商选 maximum 且能力为 256K 时为 256K；follow 恢复角色共同预算 1M。未知压缩能力返回 `unknownCompactionContext=true`，不猜测窗口；低于100K报出 compaction 和供应商，拒绝启动。
3. 精确订阅目标捕获 credential 名/auth_index 和可撤销绑定。换号修改共享路由之前同步撤销旧目标并释放目标租约，消息、计数、custom压缩和已绑定source都拒绝继续使用，提示“订阅账号已变更，请重新启动会话以使用账号”。同账号只改模型设置不撤销；失败回滚恢复账号供新准备使用，不复活旧绑定。删除与服务关闭同样清理绑定，重复release幂等。
4. 目标撤销取消未完成的本地网关请求；计数保留取消信号，成功、取消和断连后移除请求监听器。已经送达旧上游的内容不可撤回，本批不声称撤回。

## 实现范围

| 文件 | 修复 |
| --- | --- |
| `apps/runtime/src/session/claude-role-launch.ts` | 精确选择器、捕获目录身份的main解析、custom压缩共同预算、source传递账号guard |
| `apps/runtime/src/session/claude-role-assignments.ts` | 捕获native具体模型身份；订阅目标必须有账号身份及guard；保留可撤销signal，不冻结其内部状态 |
| `apps/runtime/src/model-router/subscription-accounts.ts` | prepareModelTarget绑定身份、按credential跟踪并撤销租约；save/delete/close同步失效；回滚不复活 |
| `apps/runtime/src/model-router/subscription-account-binding.ts` | runtime-only安全错误文案，避免将管理层诊断或凭据带到网关 |
| `apps/runtime/src/session/claude-model-routing.ts` | 角色/source发送前调用guard，保留明确换号原因与signal |
| `apps/runtime/src/session/claude-request-gateway.ts` | 消息/count的撤销和未完成HTTP取消、监听器清理 |
| `apps/runtime/src/model-router/subscription-target-binding.test.ts` | 本地fake管理服务和共享prefix A→B、四种在途请求、回滚/同账号/删除/关闭/清理 |
| 相关role/launch/PTY/runtime-server测试 | 自定义名称与压缩预算回归；旧fake目录补齐sonnet身份，不用宽泛猜名兼容不完整目录 |

## 验证证据

日志位于 `.cache/task5-*.log`，同名副本交到监督运行目录的 `logs/`。中间轮次不重复累计。

| 日志 | 结果 |
| --- | --- |
| `task5-red-routing-window.log` | 原修复前3红灯：两个名称误分流、一个压缩预算遗漏；其余23通过 |
| `task5-binding-red-disabled-revocation.log` | 关闭换号撤销点的负向控制：回滚后旧guard仍可用，用例失败；2项筛选排除。此项是修复后的负向控制，不冒称修复前测试 |
| `task5-runtime-regression.log` | 首轮15文件：328通过、2失败、17项既有固定router集成跳过；两个失败均为旧fake目录只有default、缺sonnet身份 |
| `task5-changed-final.log` | 最终5文件51通过，含角色快照、launch、HTTP路由、真实PTY，以及6项账号绑定场景；账号场景覆盖role/count/compaction/source四种在途取消 |
| `task5-runtime-server-final.log` | Task2/3实际RuntimeServer+PTY相关4通过；另171为筛选排除。补齐fixture目录后，原失败场景通过，无database-closed警告 |
| `task5-desktop-regression.log` | 设置三组件、egress与RuntimeClient共10文件134通过 |
| `task5-contracts-regression.log` | contracts全9文件105通过 |
| `task5-typecheck-runtime-final.log` / `task5-typecheck-desktop.log` / `task5-typecheck-contracts.log` | 三处typecheck通过 |
| `task5-identifiers-final.log` | 命名门禁通过；`git diff --check`通过 |

最终去重为runtime **333项通过**：首轮330项执行用例中的两项失败经定点修正通过，随后账号取消覆盖由3项扩为6项；其余成功结果复用。加desktop134和contracts105，合计 **572项通过**。17项既有固定router集成未执行；171项是后续定点命令的筛选排除，不能重复算跳过或通过。没有重跑全仓构建、安装、完整Electron或浏览器E2E。

### 本地HTTP验收矩阵

| 场景 | 实际结果与证据 |
| --- | --- |
| 自定义模型带claude档位片段 | 两个本地source/assigned上游；消息/count仅source收到，模型不变；角色路由测试通过 |
| 标准选择器/Fable/native目录 | 精确别名保留；任意private/custom/未知供应商前缀不猜档位；真实PTY角色环境与HTTP通过 |
| 压缩128K/maximum/follow/unknown/低于100K | launch测试全部覆盖；仅本地预算验证，未真实发送超窗请求 |
| 换号修改前撤销 | fake管理服务在第一次路由修改入口断言旧signal已取消；修改被gate阻塞时旧请求已明确拒绝 |
| 四种在途请求 | role消息、count、custom压缩、source各自等待中换号，均返回明确换号原因，fake上游连接关闭 |
| 换号期间与完成后旧目标 | 旧role/count/compaction/source均502；B收到0个旧会话请求；新prepare使用B身份并请求命中B |
| 换号失败回滚 | 保存失败、store仍为A；旧guard继续拒绝；新prepare得到可用A目标 |
| 同账号模型保存 | 旧guard保持可用，后续HTTP仍命中A |
| 删除/close/租约 | 修改前失效，close撤销多个目标并释放租约，重复release不重复释放 |
| 监听器 | 12个顺序count请求完成后，目标signal的abort监听器归零；没有每请求管理API/额度/公网轮询 |

这些是fake管理服务和loopback HTTP证据，不是固定router二进制的协议转换、真实账号换号或真实上游压缩验收。

## 我自己做的决定

1. 角色选择器按确定白名单与本次native目录身份识别；main解析和入站解析共享精确规则，不保留按名称片段猜档位的兼容分支。
2. 压缩目标复用角色同一预算算法；单独返回unknown标志，不把未知能力伪造成1M。
3. 绑定只在runtime内存中存在，按credential跟踪lease的幂等release；撤销同时abort和释放，不增加存储、外部协议或网络轮询。
4. 订阅目标在快照边界必须带身份和guard；AbortSignal保留内部可变状态。仅将确定的绑定失效错误转成明确用户原因，其它诊断仍按既有安全错误处理。
5. 用小的共享错误模块避免会话路由为了识别错误而加载完整账号服务；不引入通用绑定框架。
6. 本批由主代理实现与自检，遵守当次运行约定，没有派其它代码代理，没有把自检当独立复审。

## 尚未验证

固定router真实转换与账号替换、真实账号、真实上游长对话压缩、原生CLI Task子代理、完整Electron、键盘/窄屏/英文界面和忙终端性能、reference product可运行对照仍未闭合。本批没有重做UI验收，沿用前批截图及明确缺口。旧无角色分配路径继续沿用既有订阅启动流程；本批账号绑定覆盖新精确目标及其角色/custom压缩/source快照，不声称给所有历史直连进程增加账号固定能力。

## 二次复审补充

后续独立复审发现同名身份变化时误删当前凭据，以及混合分配默认目标缺少保存校验。原六项绑定fixture只覆盖异名换号，不能据此声称同名路径完整。两项已获监督者授权修复，新增红绿测试并重新构建/类型检查/E2E；修改前后证据与实物启动状态统一见[最终构建与隔离应用记录](final-app-acceptance.md)。本节取代上文对换号全范围已闭合的宽泛表述，真实固定router仍未验证。
