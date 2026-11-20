# 原生 Claude 请求网关 · 第一阶段

## 使用边界

新启动的 Claude 卡片命中已有角色分配且可通过 API Key 完成准备时，CLI 按消息中的档位选择目标。Opus 和 Sonnet 即使都用同一个模型 ID，仍可分别命中两个上游及各自的 Key。运行中保存分配，在线卡片继续使用启动时的连接快照。冷恢复、Fork 和权限复起沿用已有的启动解析和对话身份；退役的运行释放自己的网关。

未命中分配、显式目标超出 API Key 范围、未配置档位必须依赖无可用 API Key 的源，或外部目标可能需要环境代理时，沿用现有直连启动。源 API 有 Key 但缺少某档映射时，该档别名返回目标不可用。常规角色分配保存和上下文预算仍走原有代码。入口对应现有的新建、Shell 提升和无绑定的重新启动。

## 设计

- **运行归属。** owner 在生成 runId 后启动网关，独立 Tokio 线程绑定 `127.0.0.1:0`，回传就绪地址后才投影环境并启动 PTY。每个 `RunningShell` 由 sessionId 索引并持有 runId；owner 落盘现有 runtime generation。三者共同约束该次运行的网关归属。listener 地址和连接详情不进入 RPC 或会话元数据。
- **本地鉴权。** 每个 listener 生成 32 随机字节并编码成 URL 首段。仅接收持有该路径能力的回环请求。CLI 通过 `ANTHROPIC_BASE_URL` 得到这个地址，通过占位 API Key 选择 HTTP API 传输。两项在进程环境和现有的私有 `--settings` 中保持一致。目标 Key 留在 runtime 快照，回环请求绕开环境代理。
- **档位和字面模型。** 启动 `--model`、三档默认变量使用 `matou-role/<role>`，Fable 映射到 Opus。回调中的内部别名由本代次过滤，原来的供应商与恢复模型仍可供直连使用。对于请求，先识别已知别名及 `[1m]` 后缀，再匹配源供应商配置中的三档具体 ID。其余字面模型保留原 ID 并走启动时的源连接。未知保留前缀、缺失目标或上游失败使用固定错误，单次请求始终只选一个连接。
- **实体和请求头。** `POST /v1/messages` 可改写模型、effort、thinking 和配置的输出上限。`POST /v1/messages/count_tokens` 只改目标与模型，保留客户端生成字段。双向移除逐跳头和 `Connection` 指定的头，请求侧剥离认证头和 Cookie 后注入目标 `x-api-key`。Beta 按出现顺序合并去重。正文改写后重算字节长度并移除正文摘要头。拼接 endpoint 的路径和查询，保留客户端查询。64 MiB 同时限制声明长度、接收累计和改写后的长度。
- **流和取消。** 上游响应头就绪后直接交给下游，SSE 和普通响应随下游消费逐块转发。下游 EOF 或运行退役时放弃出站连接任务。连接在等待响应头或传输正文时均可被取消。启动出错由本地句柄释放，写入运行记录失败先释放网关再停 worker。卡片关闭、权限重启、worker 退出或运行时关闭均释放旧地址。应用线程只等本地监听就绪，转发、DNS、TLS 和 SSE 都在独立执行上下文。
- **错误与脱敏。** 4xx、5xx、429 限流头和响应实体沿常规路径透传。3xx 带 `Location` 转 502，目标地址不回显。传输错误使用静态描述。网关不记录请求头、URL 密钥、请求或响应正文。hook 对照上游 Key 和私有 URL 过滤元数据。上游实体按字节转发，若上游自行在正文中回显敏感数据，CLI 仍可能显示该正文。
- **依赖。** Tokio 1.54.1、Hyper 1.12.0、hyper-util 0.1.21、hyper-rustls 0.27.10、http-body-util 0.1.5、getrandom 0.4.3、tower-service 0.3.3 均用原锁图版本。Hyper server 唯一新增传递依赖是已批准的 httpdate 1.0.3。锁文件增 14 行。使用 Hyper 低层 HTTP/1.1 handshake 和恰好一次 `send_request`。客户端没有重定向、解压、自动重试和连接池层。HTTPS 接入 rustls 系统信任根。真实 HTTPS 和需代理的出站在生产验收范围。

## 验收与复跑

| 场景 | 冻结行为 | 原生证据 |
| --- | --- | --- |
| 同卡跨档 | 每次按请求模型选择目标，同名模型可有不同凭据 | HTTP 金样断言两条回环路径的 Key，真实 CLI 在同 PID/run/generation 中完成 `/model sonnet` 并实际出站 A、B |
| CLI 档位菜单 | 网关 settings 按冻结 `pty-session.ts:388` 依次写入 Opus、Sonnet、Haiku，标签用目录模型名，说明用供应商 ID | 进程读回三条配置。原生初始 hook settings 只有 `hooks` 和 `statusLine`，因此无需清理旧 settings 环境键。Claude Code 2.1.329 展示这三条模型项，并由 CLI 自身额外展示 `Default (recommended)` |
| 字面和未知 | 已知档位匹配，未知保留前缀失败，字面模型可走源 | HTTP 金样覆盖别名、带窗口后缀、源映射、任意字面、无模型和错误前缀 |
| 计数 | 与普通消息同档，生成参数不覆盖 | HTTP 金样比对正文，进程测试验证 B 的计数入口成功 |
| 头与错误 | 清认证和逐跳头，合并 Beta，429 与 gzip 实体保持，拒绝跳转 | HTTP 金样断言头和字节。被重定向的地址和断线重试探针均为零连接 |
| 流与关停 | 第一段先达，慢读背压，客户端退出和退役取消出站 | 16 MiB 慢读金样、收到头前后取消。替身 CLI 流暂停时任务改名仍成功，关闭卡片使 B 收到 EOF 或 reset，旧端口拒绝连接 |
| 运行与身份 | 同一代次使用固定快照，旧代次不能继续使用旧地址 | 进程测试保存分配后仍命中原 B，检查同 PID/run、恢复模型落盘及持久事件与 stderr 中无目标 Key 或 URL 凭证 |
| 正文边界 | 64 MiB 接受，超过即 413 | HTTP 金样各测精确 Content-Length、声明超限和分块累计超限 |

    cargo check --manifest-path apps/native/Cargo.toml --locked -p matou-native-runtime --all-targets
    cargo test --manifest-path apps/native/Cargo.toml --locked -p matou-native-runtime --lib claude_request_gateway
    cargo test --manifest-path apps/native/Cargo.toml --locked -p matou-native-runtime --test process_contract -- --nocapture
    cargo test --manifest-path apps/native/Cargo.toml --locked -p matou-native-core --test provider_launch_contract
    python3 tooling/native-parity/probe-claude-request-gateway.py --cli "$(command -v claude)"

真实 CLI 探针使用独立 HOME、临时数据和两个假 API。在干净配置下，CLI 显示占位 API Key 确认。已有第一轮对话后，CLI 还显示切档缓存确认。探针在原生 PTY 接受两次 CLI 自身的确认。CLI 版本、出站请求和合成 Key 指纹见 [真实 CLI 证据](gateway-phase-1-cli-evidence.json)。

## 后续阶段和未验证项

- **阶段 2。** 计数 HTTP 已交付。目录探测和更多上下文窗口组合、冷恢复/权限复起/Fork 的新网关重建、运行中主动应用新分配、生产 TLS/代理和各供应商兼容留待验证。其它路由目前用受限缓冲转发给源。冻结的通用请求流式转发和畸形 URI 细节也在此阶段。Hyper 会先解析 URI，例如非标准请求行的裸 `#` 可在进入路由前被 HTTP 解析层截断，冻结实现会显式拒绝这种原始请求目标。
- **阶段 3。** 搜索和独立压缩的识别、目标选择、真实输出验证。
- **阶段 4。** 官方 OAuth 和订阅同账号绑定、撤销、路由器恢复和冻结规定的一次受控重发。
- **运行成本。** 当前每运行一个 I/O 线程，每 HTTP 请求一次上游连接，64 MiB 以单次请求为限。多请求可并发，真实多卡长时资源占用与 HTTPS 建连开销尚未实测。macOS 已验证。Windows 和实际第三方上游尚未实测。
