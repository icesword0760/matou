# 任务：Antigravity 提示词清洗（ag-proxy 为底 + Manager 行为补齐）

## 目标

在路由器的 Antigravity 翻译路径上，对 `systemInstruction` 的文本做清洗：

- 剥离计费伪 header 行；
- 把 Claude Code 的身份声明替换成 Antigravity 风格的中性身份；
- 在受限窗口内归一化竞品品牌词；
- 保护代码块、用户消息、工具和 schema。

以 MIT 许可的 `antigravity-claude-proxy` 实现为底，行为上对照 `Antigravity-Manager` 查漏补缺，代码由我们自己写。

## 实际场景

Claude Code 的 system 里带计费元数据、身份声明和可能的竞品品牌词。上游网关会在推理前做静态检查，命中后返回伪 429。社区项目通过清洗这些字符串减少误拦。

## 开发者意图

- 只改 systemInstruction 文本，不改用户消息、工具、schema。
- 代码块和行内代码完全豁免。
- 不引入新的网络请求、不改变请求布局。
- 行为对齐 Manager，但不复制它的源码或测试。

## 项目意图

在 worktree `/Users/icesword/Documents/AIProjects/matou-login-integration`（分支 `feat/login-browser-integration`）。上一关已完成计费行剥离、纯函数、补丁构建和 Codex 窄口径；本关在其基础上扩展 Antigravity 规则。

参考（只读）：

- `/tmp/router-research/repos/antigravity-claude-proxy`（MIT）：`src/format/request-converter.js` 的计费行处理、`src/cloudcode/request-builder.js` 的 `DEFAULT_IDENTITY_SCRUB` 和 `scrubClientIdentity`
- `/tmp/router-research/repos/Antigravity-Manager`（CC BY-NC-SA，**只能看行为，不能复制代码或测试**）：`src-tauri/src/proxy/mappers/prompt_sanitizer.rs`
- `/tmp/router-research/repos/sub2api`（LGPL）：`backend/internal/pkg/antigravity/request_transformer.go` 的 `NeutralizeSystemIdentity`（可参考行为）

## 与上一关的关系

上一关新增了 `util.NeutralizeClaudeCodeSystemIdentity`，把三句身份句替换成 `You are a coding assistant.`，目前 Antigravity 和 Codex 两条路径都在用。

本关调整：

- **Antigravity 路径**改用新的完整清洗器（自己处理计费行、身份句、竞品词、代码块豁免），不再调用 `NeutralizeClaudeCodeSystemIdentity`，避免二次替换。
- **Codex 路径保持现状**，继续使用 `NeutralizeClaudeCodeSystemIdentity`，不扩展到本关规则。
- `NeutralizeClaudeCodeSystemIdentity` 保留给 Codex 使用，不要删除。

## 已经定下的决定（照做，不要改）

### 处理顺序（每个 systemInstruction text part）

1. **计费行**：删除以 `x-anthropic-billing-header:` 开头的那一行；删除后为空的 part 丢弃。
2. **身份句**：精确匹配并整体替换以下三句（大小写敏感）：
   - `You are Claude Code, Anthropic's official CLI for Claude.`
   - `You are Claude Code, Anthropic's official CLI for Claude, running within the Claude Agent SDK.`
   - `You are a Claude agent, built on Anthropic's Claude Agent SDK.`
   - 统一替换为：`You are Antigravity, an agentic AI coding assistant.`
3. **竞品词归一化**：只在每个 part 的前 8 句或前 1500 字符（以先到者为准）内生效，整词、大小写不敏感，替换为 `Google`：
   - `OpenAI`、`Anthropic`、`DeepSeek`、`Qwen`、`ByteDance`、`Tencent`、`Xiaomi`、`Huawei`、`Baidu`、`Mistral`、`Cohere`
   - `字节跳动`、`腾讯`、`小米`、`华为`、`通义千问`、`百度`、`深度求索`
4. **其它客户端身份**：整词、大小写敏感：
   - `Nous Research` → `the assistant team`
   - `Hermes Agent` → `the assistant`
   - `Hermes` → `the assistant`
   - `Claude Code` → `the assistant`

### 豁免规则

- 所有替换都跳过围栏代码块（三个反引号包裹）和行内代码（单反引号包裹）。
- 不替换 `CLAUDE.md`、`claude.ai`、`anthropic-version`、`x-anthropic-*`、裸词 `Claude`。
- 不处理用户消息、工具名、工具描述、JSON Schema。
- 不注入官方 Antigravity 的长提示词；只替换身份句本身。
- 不删除 CTF / 安全策略段落（这是模型行为规则，不是客户端指纹；本关不做）。

### 生效范围

- 只作用于 Antigravity 路径的 `systemInstruction` 文本。
- Codex 路径保持上一关的窄口径，不再扩展。
- 其他路径不变。

### 许可

- 在新增的 Go 文件头写明：部分行为参考 `antigravity-claude-proxy`（MIT, Copyright (c) 2024 Badri Narayanan S）。
- 在仓库 `THIRD_PARTY_NOTICES.md` 增加该 MIT 声明。
- 不复制 Antigravity-Manager 的源码、正则或测试；只按行为重写。

### 补丁

- 改动写入现有 `tooling/model-router/patches/antigravity-client-identity.patch`，同步更新 `version.json` 的 `patch.sha256`。
- 补丁包含新增的 Go 测试。
- `pnpm router:build` 必须通过。

## 原因与方案

原因：Antigravity 路径目前只做了最基础的清洗；社区项目证明计费行、身份声明和竞品词会触发上游静态检查。方案：以 MIT 实现为底，补上代码块保护、扫描窗口、身份归一和竞品词归一；全部自己实现，带测试。

## 范围

- 工作目录：`/Users/icesword/Documents/AIProjects/matou-login-integration`
- 不能碰主目录、其他 worktree、`safe-browser` 项目、`/tmp/router-research` 里的参考仓库。
- 禁止 stash / reset / rebase。
- 不改 Matou 的 UI、IPC、账户逻辑。
- 不推送、不发布。

## 必须先问的地方

- Go 工具链或补丁构建失败，先报告原文。
- 发现替换会影响工具调用、缓存前缀或用户消息，先提问。
- 发现 Antigravity 的 systemInstruction 不是由该翻译器写入，先提问。

## 关卡与交付物

一关完成：

1. `pnpm router:build` 成功，补丁校验通过。
2. Go 测试覆盖：
   - 计费行删除、空 part 丢弃；
   - 三句身份句替换为 Antigravity 身份；
   - 前 8 句/1500 字符内的竞品词替换，窗口外不变；
   - 代码块和行内代码豁免；
   - `CLAUDE.md`、小写 header、裸词 `Claude` 保留；
   - 用户消息、工具、schema 不变。
3. 样例 payload 的前后对比（至少包含计费行、身份句、代码块、窗口外品牌词）。
4. Matou 的 `pnpm typecheck`、`pnpm test`、`pnpm build` 通过。
5. 提交 commit，信息 `feat: sanitize Antigravity system prompts`。

回复中给出：改动文件、测试名、前后对比、补丁 sha、`我自己做的决定`。

## 验收（我会亲自复核）

- 读代码确认没有复制 Manager 代码、没有碰用户消息和工具。
- 亲自跑 `pnpm router:build` 和三条检查。
- 亲自看样例 payload 的前后对比。
- 确认代码块豁免和窗口限制生效。
