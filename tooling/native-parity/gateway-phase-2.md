# 原生 Claude 请求网关 · 第二阶段

API Key 会话冷恢复、Fork 或权限复起时，新 PTY 按这次启动读取到的分配和全局 API 源创建回环网关。运行中的卡片继续使用自己的快照。角色快照仍使用三档菜单和 Fable 映射。只有档位或全局 effort 的会话使用 `matou-api/<role>` 投影已映射的档位，在消息请求中逐次写入命中的 `output_config.effort`。两类网关都在环境和私有 settings 中设置回环地址、占位 Key、提示头开关和 `NO_PROXY` 两种拼法。

## 启动路径与对照证据

| 场景 | 原生调用点 | 进程验收 |
| --- | --- | --- |
| 冷恢复 | `host::prepare_launch` → `prepare_claude_launch` → `ShellOwner::launch_claude` | CLI 收到原绑定的 `--resume` ID 与原字面模型，新 run 命中新保存的 Sonnet 目标与 Key。旧 runtime 退出后及新 CLI 启动时，旧端口均返回 `ConnectionRefused`。绑定仍指向原对话。 |
| Fork | `host::prepare_launch` → `prepare_claude_fork` → `ShellOwner::launch_claude_fork` | 子 CLI 收到父 ID 的 `--resume --fork-session`，使用独立地址和新保存的 Sonnet 目标与 Key。子 hook 确认独立子 ID。父卡活跃时父地址仍可连，父卡后续复起时该地址拒绝连接。 |
| 权限复起 | `ShellOwner::stop_for_provider_relaunch` → `PermissionChange::advance` 的 `Stopping`／`Preparing` → `ShellOwner::launch_claude_permission` | 替换运行收到原 ID 的 `--resume` 和新的权限模式，命中新保存的 Sonnet 目标。启动时旧 CLI 已退出且旧端口拒绝连接。此前创建的 Fork 地址仍可连，关停 runtime 后两个余下地址都拒绝连接。 |

三条路径在准备阶段从与分配保存相同的队列读取偏好，经 `ClaudeLaunch::capture_restart_gateway` 生成当前快照。均由 `ShellOwner::launch_claude_with_monitor` 进入 `launch_run`，监听就绪后才创建 PTY。退休时先释放网关，网关线程关闭 listener 后才返回。权限复起继续等待旧 worker 完成关停。

[启动路径](../../apps/native/runtime/src/host.rs#L3938) · [统一 PTY 启动](../../apps/native/runtime/src/owner.rs#L1432) · [进程验收](../../apps/native/runtime/tests/process_contract/claude_gateway_restart.rs#L208)

## 通用 HTTP 与 effort 金样

- `POST /v1/models` 的客户端声明 1,048,576 字节并先发 16,384 字节。上游读完并确认该段，客户端才发剩余 1,032,192 字节。上游核对方法、路径、query、长度、Key 和完整正文，最终回 200。
- 分块上传的上游收到 67,108,864 字节后，客户端发送下一字节。上游随即收到 EOF 或 reset，额外字节未转发，客户端收到 413。带分块正文的 `GET /v1/messages` 使用通用流，保留方法和正文，过滤入站 trailer。
- 精确 `POST /v1/messages` 和 `POST /v1/messages/count_tokens` 继续缓冲正文。无档位分配时，落到 Sonnet effort 映射的别名和字面模型均写入 `high`，全局 effort 单独也能创建网关。计数请求保留原 effort。全为 `follow`，或缺可用 API Key 时，effort 本身不启用网关。
- 第 1 阶段回归验证同卡换档、保存偏好后在途会话继续用原目标、计数、SSE、取消和认证头清理。

[HTTP 金样](../../apps/native/runtime/src/claude_request_gateway/tests.rs#L593)

## 真实 CLI 冷恢复

在独立 HOME、数据库和两个回环假 API 中，用 Claude Code 2.1.329 完成一轮消息，关停 runtime，改 Opus 目标后恢复同一张卡并完成第二轮。恢复保留同一 providerSessionId，产生新的 native run 和 runtime generation，旧端口拒绝连接。

| 轮次 | 上游请求 | 合成 Key 的 SHA-256 前 12 位 |
| --- | --- | --- |
| 恢复前 | `/a/v1/messages`，SSE | `14761e1cf25f` |
| 恢复后 | `/b/v1/messages`，SSE | `1cb2e73babe4` |

## 自检

    cargo test --manifest-path apps/native/Cargo.toml --locked -p matou-native-runtime --lib claude_request_gateway -- --nocapture
    cargo test --manifest-path apps/native/Cargo.toml --locked -p matou-native-runtime --test process_contract claude_gateway -- --nocapture
    cargo test --manifest-path apps/native/Cargo.toml --locked -p matou-native-runtime
    cargo test --manifest-path apps/native/Cargo.toml --locked -p matou-native-core --test provider_launch_contract
    cargo check --manifest-path apps/native/Cargo.toml --locked --all-targets
    cargo fmt --manifest-path apps/native/Cargo.toml --all -- --check
    pnpm check:identifiers
    git diff --check

网关金样 8 项通过，网关进程定点 2 项通过，runtime 全量 129 项通过、4 项按仓库标记跳过，core 启动契约 24 项通过。既有 Shell 提升用例曾在默认并发下超时，单跑、4 线程全量和最终默认并发全量均通过。工作区全目标检查通过，仍有桌面和既有备份测试的编译警告。

## 范围

兼容性边界是现有的 API Key 直连目标。订阅、官方无 Key／OAuth、搜索、独立压缩、DeepSeek 内联工具转换、代理出站和运行中热切换留待后续。真实 CLI 实测覆盖冷恢复及回环上游，Fork 和权限复起由独立 runtime、替身 CLI 和本地 HTTP 上游验证。
