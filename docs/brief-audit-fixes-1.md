# 任务：审计第一轮修复（D1、D4、D6a，D3 调查）

## 目标

按第一轮审计的裁决，修复三个确认问题，调查一个代理 TLS 问题，记录四个不修项的理由。

## 已裁决

### 要修

- **D1（高）**：Codex 路径遇到“计费行 + 后续有效指令”的整块文本时，不能整块丢弃。改为只删除开头的计费行，保留后面的正文；如果删完为空才丢弃。字符串 system 和数组 text part 两种输入都要覆盖。
- **D4（中）**：Codex 的身份句替换要加代码块和行内代码豁免，规则仍只限三句；复用 Antigravity 清洗器的保护逻辑，避免两套实现。
- **D6a（中）**：中文品牌词改为子串匹配（例如“由腾讯开发”里的“腾讯”要命中），仍限制在每个 system part 的前 8 句/1500 字符内，并继续豁免代码块和行内代码；英文品牌保持整词匹配。

### 调查

- **D3（中）**：HTTP 代理下，Codex 的自定义 TLS 指纹没有用于目标站握手。用本地 HTTP CONNECT 代理抓一次真实握手，确认是否被绕过。
  - 如果确认且修复范围可控，实现修复并加测试；
  - 如果修复需要大改传输层，保留证据并报告，不在本关强行改。

### 不修，写进已知限制

- **D2（高）**：Claude 最终 `tool_choice` 固定 `VALIDATED`，会覆盖 `any`。保留现有行为；ag-proxy 和 sub2api 也这么做，改回 ANY 可能触发上游 400。
- **D5（中）**：只处理三句 Claude Code 身份；不扩展到 Codex/Cursor/OpenCode 开场。
- **D6b（中）**：保留前 8 句/1500 字符窗口，不做全量品牌替换。
- **D7（中）**：不改变文本与 functionCall 的排序策略。

## 实现要求

- 所有路由器改动写入现有 `tooling/model-router/patches/antigravity-client-identity.patch`，同步更新 `version.json` 的 `patch.sha256`。
- 补丁包含新增 Go 测试。
- `pnpm router:build` 必须通过。
- Matou `pnpm typecheck`、`pnpm test`、`pnpm build` 通过；测试临时目录按前两关口径（仅测试可用系统默认临时目录），时间门槛类失败保留日志。
- 不改变请求布局、缓存前缀、工具/schema 语义。
- 不复制参考项目代码；D3 可以参考上游 CLIProxyAPI 的 `internal/runtime/executor/helps/utls_client.go` 行为，但自己实现。

## 文档

在 `docs/acceptance/` 下新增或更新一份审计修复记录，包含：

- D1/D4/D6a 的前后对比和测试名；
- D3 的抓包证据和结论；
- D2/D5/D6b/D7 的已知限制说明。

## 范围

- 工作目录：`/Users/icesword/Documents/AIProjects/matou-login-integration`
- 不能碰主目录、其他 worktree、`safe-browser` 项目、参考仓库。
- 禁止 stash / reset / rebase。
- 不推送、不发布。

## 必须先问的地方

- D3 的修复需要改传输层结构或新增依赖，先提问。
- 发现 D1 修复会影响 Codex 的缓存前缀或工具布局，先提问。
- Go 工具链或补丁构建失败，先报告原文。

## 关卡与交付物

1. D1/D4/D6a 修复，带 Go 测试。
2. D3 抓包证据和结论（修或明确不改的理由）。
3. 已知限制文档。
4. `pnpm router:build`、Matou 三条检查通过。
5. 提交 commit，信息 `fix: close audit round 1 findings`。

回复中给出：改动文件、测试名、D3 抓包结果、已知限制清单、`我自己做的决定`。

## 验收

- 读代码确认 D1 不会丢正文、D4 代码豁免、D6a 中文命中且窗口/代码保护生效。
- 亲自跑 `pnpm router:build` 和三条检查。
- 亲自看 D1/D6a 的前后对比和 D3 抓包证据。
