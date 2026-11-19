# 任务：订阅请求的计费文本与身份句清洗（窄口径）

## 目标

在路由器的 Antigravity 和 Codex 两条翻译路径上：

1. 剥离 system 里的 Claude Code 计费文本行（`x-anthropic-billing-header:`）。
2. 把三句固定的 Claude Code 身份声明替换成中性句。

只作用于 system / instructions，不碰用户消息、项目文件、工具描述和 schema。目的是减少上游 WAF 把第三方客户端特征误判为限流（社区项目把这列为 429 的已知触发点），不是伪装成官方客户端。

## 实际场景

Claude Code 发出的 system 里带一行计费元数据和一句身份声明。Antigravity 路径目前原样转发；Codex 路径剥离了计费行但没有处理身份句。上游可能因此返回伪 429。

## 开发者意图

- 只删客户端元数据，不改变用户内容和工具行为。
- 替换是确定性的，不破坏 prompt 缓存前缀。
- 不注入任何“官方身份”提示词。

## 项目意图

在 worktree `/Users/icesword/Documents/AIProjects/matou-login-integration`（分支 `feat/login-browser-integration`）。路由器是 pinned 的 CLIProxyAPI 8.0.69，通过 `tooling/model-router/patches/antigravity-client-identity.patch` 打补丁后编译。

先读：

- `tooling/model-router/build-patched.mjs`、`prepare.mjs`、`version.json`
- 上游对应版本源码：`internal/translator/antigravity/claude/antigravity_claude_request.go`、`internal/translator/codex/claude/codex_claude_request.go`、`internal/util/claude_attribution.go`
- 可参考 `/tmp/router-research/repos` 下已克隆的三个项目（只读参考，不要改它们）

## 已经定下的决定（照做，不要改）

### 计费行

- 复用上游已有的 `util.IsClaudeCodeAttributionSystemText` / `StripClaudeCodeAttributionSystem` 语义。
- Antigravity 路径：拼接 `systemParts` 时跳过命中的文本块；如果整块只有计费行，整块丢弃。
- Codex 路径：现有过滤保持不变。

### 身份句

新增一个纯函数（放在 `internal/util/` 下即可），对 system / instructions 文本做精确替换：

- `You are Claude Code, Anthropic's official CLI for Claude.` → `You are a coding assistant.`
- `You are Claude Code, Anthropic's official CLI for Claude, running within the Claude Agent SDK.` → `You are a coding assistant.`
- `You are a Claude agent, built on Anthropic's Claude Agent SDK.` → `You are a coding agent.`

规则：

- 只替换完整句子，大小写敏感。
- 不替换 `CLAUDE.md`、`claude.ai`、`anthropic-version`、`x-anthropic-*`、裸词 `Claude`。
- 不处理用户消息、工具名、工具描述、JSON Schema、项目文件。
- 不做广谱竞品品牌替换。
- 不注入任何官方 Antigravity / Codex 提示词。

### 生效范围

- Antigravity：`systemInstruction` 的所有 text parts。
- Codex：最终写入顶层 `instructions` 的文本。
- 其他路径（Claude 官方、Gemini、OpenAI 等）不变。

### 补丁与构建

- 把改动写进现有的 `tooling/model-router/patches/antigravity-client-identity.patch`（构建脚本只支持一个补丁）。
- 同步更新 `version.json` 里的 `patch.sha256`，保证 `pnpm router:build` 的校验通过。
- 补丁里包含新增的 Go 测试。
- 重新构建路由器，产出新的本地二进制；不要改版本号、不要发版。

## 原因与方案

原因：Antigravity 路径缺少计费文本过滤，两条路径都保留固定身份句；社区项目（sub2api、Antigravity-Manager、antigravity-claude-proxy）把这列为上游 429 的触发点。方案：复用上游辅助函数 + 一个精确替换函数，最小改动，带测试。

## 范围

- 工作目录：`/Users/icesword/Documents/AIProjects/matou-login-integration`
- 不能碰主目录、其他 worktree、`safe-browser` 项目、`/tmp/router-research` 里的参考仓库。
- 禁止 stash / reset / rebase。
- 不改 Matou 的 UI、IPC、账户逻辑。
- 不推送、不发布。

## 必须先问的地方

- Go 工具链或补丁构建失败，先报告原文。
- 发现替换会影响 Claude Code 的工具调用或缓存行为，先提问。
- 发现 Codex 路径的 instructions 不是由该翻译器写入，先提问。

## 关卡与交付物

一关完成：

1. `pnpm router:build` 成功，补丁校验通过。
2. Go 测试覆盖：计费行删除、三句身份替换、`CLAUDE.md`/小写 header/裸词保留、用户消息不变、两条路径都生效。
3. Matou 的 `pnpm typecheck`、`pnpm test`、`pnpm build` 通过（必要时跑路由器相关测试）。
4. 用一个含计费行和身份句的样例 payload 过翻译器，给出前后对比。
5. 提交 commit，信息 `feat: quiet subscription prompts before upstream`。

回复中给出：改动文件、测试名、前后对比、补丁 sha 更新、`我自己做的决定`。

## 验收（我会亲自复核）

- 读代码确认只改了 system/instructions，没有碰用户内容和 schema。
- 亲自跑 `pnpm router:build` 和三条检查。
- 亲自看样例 payload 的前后对比。
- 确认没有广谱品牌替换、没有注入官方提示词。
