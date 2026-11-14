# 订阅客户端标识一致性（2026-11-14）

| 字段 | 值 |
|---|---|
| 触发 | 模拟对抗检测报告第 1、3 点，以及 Codex 同类问题 |
| 范围 | Antigravity / ChatGPT 订阅会话经本机模型路由转发到服务方的客户端标识、传输层与探测频次 |
| 结论 | Antigravity 与 Codex 的 UA、Codex Originator、TLS/HTTP 形态、连接复用和探测频次均已对齐到本机原生客户端抓取结果 |

## 1. Antigravity：同一 Token 一个标识

修复前：登录完成和每次打开设置页时，`management-client.ts` 对 `loadCodeAssist`、`retrieveUserQuotaSummary`、`fetchAvailableModels` 显式发送 `User-Agent: Matou (antigravity/cli/1.4.6)`；推理请求由路由发送 `antigravity/hub/2.29.0 (aidev_client; ...)`。

修复后：

- 探测与推理共用 `apps/runtime/src/model-router/antigravity-client.ts` 的单一常量。
- 凭证级 `user_agent` 写入路由凭据，推理优先使用该值；写入前先读文件，已有相同值不重复写。
- 原生 `language_server` 抓取确认是 Go 1.28 + boringcrypto、ALPN h2,http/1.1、HTTP/2；补丁路由复刻该 ClientHello 并池化 h2。

## 2. Codex：同一 Token 一个标识

修复前：客户端是 Claude Code，路由器把它的 User-Agent 转发给 ChatGPT；额度/模型探测没有 UA，落到 Go 默认值；Originator 推理为 `codex-tui`、探测为 `codex_cli_rs`；推理走 Chrome uTLS、探测走 Go TLS。

修复后：

- 以本机原生 **Codex CLI 0.162.1** 为基线，使用交互式 TUI 身份：
  - `User-Agent: codex-tui/0.162.1 (Mac OS <产品版本>; <arch>) <terminal> (codex-tui; 0.162.1)`；OS 版本由 `sw_vers -productVersion` 读取，terminal 优先取 `TERM_PROGRAM`，Finder 启动时用已验证的 `iTerm.app/3.6.11`。
  - `Originator: codex-tui`，不发送 `Version` 头（原生也没有）。
  - 模型探测的 `client_version=0.162.1`。
- 推理 UA 通过 `oauth.providers.codex.header-defaults.user-agent` 固定；凭据级 `headers` 固定 `Originator`；登录、保存、启动都会补齐，已有相同值不重复写。
- 两个 ChatGPT 探测（usage、models）携带同一组 UA / Originator。
- 推理、探测、图片请求统一走 `internal/codextls`：原生 Codex 抓取是 **TLS 1.2、无 ALPN、HTTP/1.1**，不是 Chrome/H2；该传输按凭据池化（每主机 2 条、90s 空闲），解决探测与推理指纹不一致和 Codex 每次请求重新握手的问题。
- OAuth 登录/令牌刷新也使用原生 TLS；只有调用方显式替换了 `http.DefaultTransport`（测试、自定义 CA）时才保留其传输。
- API-key Codex 不套用订阅身份，保留标准代理传输。

## 3. 降低探测频次

设置页原来每次重新打开都会对每个订阅账号重发探测。

- 账号详情（套餐、额度、模型）增加 5 分钟内存缓存，按凭据指纹（auth_index、文件大小、修改时间等）失效。
- 命中缓存时不再发任何服务方探测；超过 5 分钟只重读套餐/额度，模型列表优先用内存缓存，其次用登录时保存的 `modelMetadata`，不重复拉 `fetchAvailableModels`。
- 登录、保存、用户主动“刷新模型”始终读最新；删除、重登、凭据文件变化会清缓存。
- 缓存时长可通过 `SubscriptionAccounts` 的 `detailsTtlMs` 调整。

## 4. 路由补丁

`tooling/model-router/patches/antigravity-client-identity.patch` 在固定提交 `f7ae8fe` 上做六件事：

1. 新增 `internal/antigravitytls`：按原生 `language_server` 抓取结果构造 ClientHello 并池化 h2。
2. 新增 `internal/codextls`：按原生 Codex CLI 0.162.1 抓取结果构造 TLS 1.2 / 无 ALPN ClientHello，HTTP/1.1 连接池。
3. Antigravity 推理、探测、token 刷新走 `antigravitytls`；Codex 推理、探测、图片请求走 `codextls`，管理 `api-call` 对两类订阅都使用各自传输。
4. 保留调用方显式注入的 RoundTripper（测试、SDK 嵌入）和自建 `base_url` 的 HTTP/1.1 兼容路径。
5. 关闭路由后台的 Antigravity Hub 版本清单请求（原生由 Electron 更新器发出，路由重放只会多一种原生不会出现的组合）。
6. 更新上游测试：Antigravity 旧 HTTP/1.1 连接池断言按设计跳过；Codex 增加原生传输路由、UA 优先级和 ClientHello 结构用例。

`config.ts` 另外开启 `antigravity.connection-pool`（顶层，30s、每主机 2 条）作为 HTTP/1.1 回退；生产 Antigravity 流量由补丁后的 h2 池复用。检测报告里写的 `oauth.providers.antigravity.connection-pool` 不是 8.0.69 的配置路径。

## 5. 供应链

`tooling/model-router/version.json` 记录固定提交、补丁 SHA256 和构建用 Go 版本；`pnpm router:build` 从固定提交克隆、校验补丁、用 `go1.27.4` 构建，并把 `patched: true`、补丁哈希、Go 版本、二进制 SHA256 写入 `source.json`。`prepare.mjs` 只复用校验通过的补丁版；`MATOU_ROUTER_UPSTREAM=1` 才回到上游原版。打包链路复制同一份 `package-resources/model-router`，签名流程不变。

## 6. 验证

```bash
pnpm router:build
pnpm --filter @matou/runtime exec vitest run \
  src/model-router/antigravity-client.test.ts \
  src/model-router/codex-client.test.ts \
  src/model-router/config.test.ts \
  src/model-router/management-client.test.ts
MATOU_TEST_MODEL_ROUTER=1 pnpm --filter @matou/runtime exec vitest run \
  src/model-router/manager.test.ts src/model-router/subscription-accounts.test.ts
pnpm --filter @matou/runtime typecheck
pnpm check:identifiers
```

结果：

- Antigravity：原生抓取与补丁路由 `api-call` 抓取逐项一致（cipher、扩展顺序、groups、签名算法、ALPN、key share 组）。
- Codex：原生 CLI 0.162.1 抓取与 `codextls` 直接抓取逐项一致（TLS 1.2 cipher 列表、SNI、groups P-256/P-384/P-521、签名算法、OCSP/SCT/EMS，无 ALPN / supported_versions / key share）；路由 `chatgpt.com` 明确路由到池化的 `codextls` 传输。
- Codex 原生 UA 的 OS 段来自 `sw_vers -productVersion`；TUI 身份使用 `codex-tui/0.162.1 ... (codex-tui; 0.162.1)`。本机 `codex exec` 抓取只用于确认 TLS/HTTP 栈和“无 Version 头”，Originator 按交互式 TUI 选择。
- Matou：10 个单元用例、9 个真实引擎用例、17 个订阅账号集成用例全部通过；真实路由下确认 Codex 凭据写入 `headers: { Originator: codex-tui }`，设置页第二次打开不再产生服务方探测。
- 路由：Codex/Antigravity auth、管理、helps、codextls 测试通过；executor 全包通过（68s，7 个旧 HTTP/1.1 连接池用例按设计跳过），管理接口 25s。
- `typecheck`、`check:identifiers`、`git diff --check` 通过。

## 7. 已知边界

- Codex **websockets** 路径（Matou 未启用，OAuth 走 HTTP/SSE）仍使用 Go TLS。
- uTLS 生成的 ClientHello 带 32 字节 session ID，原生 Codex 为空；JA3/JA4 不含 session ID，其余结构一致。
- 客户端版本固定为 Antigravity 2.29.0、Codex 0.162.1；原生升版后需要重新抓取并更新常量/补丁。
- 没有真实 Antigravity/ChatGPT 账号的线上推理验证；本轮覆盖请求构造、原生抓取对比和本机路由生命周期。
