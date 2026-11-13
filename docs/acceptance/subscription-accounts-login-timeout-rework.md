# 登录请求超时返工

状态：实现、相关自动验证和构建完成，**可以重启开发版**。按 run-10 要求，本轮不启动或重启开发版，不提交。

## 排查结论

原实现把路由冷启动、对账、建立 OAuth、本机回调监听和打开浏览器串在一次请求里，客户端只给 10 秒。独立目录、随机回调端口和固定官方引擎下，分段计时确认：

- 不读取本机代理配置的控制组：对账 5.9ms，路由冷启动 1388.5ms，OAuth URL 192.9ms，回调监听 9.7ms；浏览器立即完成时总计 1602.2ms。
- 同一控制组模拟浏览器 11 秒，原 login-start 被拖到 12067.8ms。这里直接注入浏览器实现，**不是生产 bridge**；生产 bridge 自身有 6 秒上限。
- 使用产品原有的本机代理解析方式、浏览器立即完成时：代理解析 7970.5ms，路由 acquire 共 8849.3ms，OAuth URL 129.1ms，回调监听 15.1ms，总计 8997.3ms。10 秒预算仅剩约 1 秒给后续步骤。
- 补充样本：原实现的代理解析 3464ms、浏览器模拟耗时 2001.4ms，总响应 6165.2ms；修复后另一轮代理解析 1253.6ms、总响应 1628.8ms，在浏览器仍未完成时已返回。代理耗时会随启动环境和负载波动，**不能把两轮总时差都算作代码加速**。证据：`production-proxy.json`、`production-before.json`、`production-after.json`；控制组前后为 `baseline.json`、`fixed.json`。
- 再使用真实 Electron RuntimeHost → utility process → RouterHostBridge：正常打开与拒绝均能收到回复；模拟 main 的浏览器操作永不完成，bridge 在约 6 秒拒绝。**未复现 bridge 消息丢失，没有为此修改正常桥接逻辑。**

证据支持「冷启动累计耗时，加上等待浏览器打开，耗尽 10 秒预算」这一故障路径。用户现场没有分段日志，且本轮没有操作现场进程，因此不能断言那一次具体在哪一毫秒超时，也不能仅凭路由进程年龄判断发生过崩溃重启。

上述隔离探针不启动 Matou 产品界面，不打开真实浏览器，不做 OAuth 换码或发送模型请求。只有账号采用回归沿用既有受控测试边界：官方回调受理、合成换码结果和合成凭据、本机假上游，外部 CONNECT 拒绝。

## 已修改

1. `login-start` 在 attempt 和回调监听就绪后立即返回 `waiting`；浏览器操作在队列外异步执行，重开浏览器也不占用登录队列。
2. 浏览器拒绝或 bridge 超时后，状态为 `failed`，原因「浏览器没有打开」/ "The browser didn't open"，并带 `canReopen: true`。等待页显示原因、继续原来的状态轮询、保留可点击的「重新打开浏览器」。
3. 重开复用原 attempt、原授权地址、原超时和原回调，不向 renderer 返回授权地址。成功回调仍可继续到模型设置；取消、过期和成功后取消可重开标记。迟到的打开失败不会覆盖取消或成功结果。
4. 客户端 `login-start` 专属默认超时为 30 秒，弹窗也明确传入 30 秒；其它请求仍用原来的默认 10 秒。超时错误有中英文文案，不暴露 RPC 方法名。
5. 同一连接重新 login-start 时，先取消其仍存活的旧 attempt，再创建新 attempt；即使上次客户端超时没有拿到 attempt ID，也能释放自己的监听端口。其它连接仍不能接管本次登录。

没有新增存储、依赖、运行时 LLM 步骤或模型费用。没有调整代理策略、OAuth 总时限或页面布局。

## 自动验证与构建

| 检查 | 结果 | 证据（相对 output/login-timeout） |
| --- | --- | --- |
| 官方引擎账号采用与 host bridge | 2 文件，18/18，0 跳过 | `runtime-target-final.log` |
| OAuth、RPC、Claude 启动参数相关回归 | 3 文件，47/47 | `runtime-related.log` |
| RuntimeClient | 38/38 | `client-target.log` |
| RuntimeHost（含打开拒绝的回复） | 14/14 | `host-target.log` |
| 订阅弹窗中英回归 | 37/37 | `ui-final.log` |
| contracts 构建 | 通过 | `contracts-build.log` |
| runtime / desktop 构建 | 通过 | `runtime-build.log`、`desktop-build.log` |
| runtime / desktop 类型 | 通过 | `runtime-typecheck.log`、`desktop-typecheck-final.log` |
| 命名与空白 | 通过 | `identifiers-final.log`、`git diff --check` |

合计相关测试 154 项通过；没有重新运行历史上存在旧模块失败的全量套件，不将相关通过称为全量全绿。

保留第一轮失败：账号回归 15/16，新加的浏览器失败后回调用例误将 `failed + canReopen` 视为终态，早于账号详情处理完成便断言 ready。调整测试等待条件，忽略可恢复的浏览器失败后，最终 18/18 通过；产品代码没有为了这条失败放宽成功条件。原日志保存在 `runtime-target.log`。

## 真实进程边界实测

`electron-evidence.json` 使用真实 Electron 与 RuntimeHost，但注入浏览器操作，不启动产品页面：

| 浏览器行为 | login-start 返回 | 后续结果 |
| --- | --- | --- |
| 正常返回 | 923ms | waiting |
| 拒绝 | 405ms | failed，可重开，中英文原因正确 |
| 永不回复 | 347ms | 约 6 秒后 failed，可重开 |

所有该探针路由进程已回收（`allFixtureRoutersStopped: true`）。用户开发版主进程 40125、runtime 40183 和正式版 97797 在收尾检查时仍为原进程；没有向它们发送信号、刷新、点击或认证请求。

## 我自己做的决定

- 保留失败浏览器操作对应的 OAuth 监听和租约到原超时，允许同一 attempt 重开与回调恢复；没有新建登录会话或延长授权时间。
- 对同连接未确认的旧登录在 runtime 中处理取消，不依赖 renderer 一定知道旧 attempt ID；跨窗口所有权不变。
- bridge 实测正常，所以只补回复匹配、丢回复的有界超时与打开拒绝回归，不增加另一套桥或重试框架。
- 来自 DSH：复用等待页既有错误区域与按钮；用 ref 同步可重开许可；重开回复不创建第二条轮询链；分别断言中英文原因。主代理整合时使用已经扩展的公开 request 参数签名，去掉局部类型强转。

## 尚未验证

没有启动新构建的产品界面，没有代替用户登录真实账号，未验证本次修复后的真实浏览器授权、套餐/额度或模型请求。真实 UI 与 reference product 对照矩阵、第八关整体仍未闭合。需要监督者重启开发版后，由用户本人重试。

已收到监督者新增的 WebSearch 与 WebFetch 验收要求，并记录在 `subscription-accounts-stage8.md` 的待验收矩阵。两项都等本次真实登录成功后再做，各最多 1～2 个请求；WebFetch 域名预检失败时先报告，不自行添加 `skipWebFetchPreflight`。本轮没有执行这两项。

没有提交、推送、安装包或发布。
