# 任务：对照 sub2api 补齐 Antigravity 请求转换

## 目标

把我们 pinned 的 CLIProxyAPI 8.0.69 + 现有补丁，和 sub2api 的 Antigravity 转换实现逐项对照，找出功能差距，只补真实缺口。

## 实际场景

sub2api 是同类里规模最大的 Go 网关之一（46k stars，LGPL-3.0），它的 Antigravity 转换覆盖了一些我们可能没有的协议细节。用户要求做完提示词清洗后，用它做一次查漏补缺。

## 开发者意图

- 只补会影响请求成功率和工具调用的差异，不做无依据的照搬。
- 不改变现有请求布局和缓存前缀，除非有明确的上游 400 证据。
- 可以复用 sub2api 的代码（LGPL-3.0 与 Matou 的 GPL-3.0 兼容），保留声明；也可以按行为自己实现。

## 项目意图

在 worktree `/Users/icesword/Documents/AIProjects/matou-login-integration`（分支 `feat/login-browser-integration`）。前一关已完成 Antigravity 提示词清洗；本关在同一个路由器补丁上继续。

参考（只读）：`/tmp/router-research/repos/sub2api/backend/internal/pkg/antigravity/request_transformer.go`。

## 已经定下的决定（照做，不要改）

### 先做对照矩阵

逐项列出：sub2api 怎么做（函数/行号）→ 我们怎么做（pinned 版本文件/行号）→ 差异 → 结论（已覆盖 / 真缺口 / 不适用）。

至少覆盖：

1. `systemInstruction` 结构与身份处理（我们上一关已做）。
2. `max_tokens` 与 `thinking.budget_tokens` 的关系：sub2api 有 `ensureMaxTokensGreaterThanBudget`；我们的 pinned 版本直接把两者分别写入 `maxOutputTokens` 和 `thinkingBudget`，没有校正。评估是否需要补。
3. 多轮工具调用：`tool_choice`、`functionCall` 归集、工具结果格式（sub2api 有 `EnableMCPXML`）。
4. session ID：sub2api 用首条用户消息生成稳定 session；我们的 pinned 版本有 reasoning replay 的 session 处理，确认是否等价。
5. thinking block / signature：dummy thought、历史 assistant 消息的处理。
6. 计费行与身份清洗（已完成，确认不回归）。
7. 其他对照中发现的真实差异。

### 只在以下情况动手

- 有明确的上游 400 / 请求失败证据，或会让工具调用失败；
- 不改变用户可见行为；
- 有测试能证明修复前后的差异；
- 不为了“和 sub2api 一样”而改布局或缓存前缀。

不满足以上条件的差异，写进报告并说明为什么不改。

### 许可证

- sub2api 是 LGPL-3.0，与 GPL-3.0 兼容。复用代码时在文件头保留来源声明，并在 `THIRD_PARTY_NOTICES.md` 增加 LGPL 声明。
- 不改 sub2api 仓库本身。

### 补丁

- 改动写入现有 `tooling/model-router/patches/antigravity-client-identity.patch`，同步更新 `version.json` 的 `patch.sha256`。
- `pnpm router:build` 必须通过。

## 原因与方案

原因：不同网关对 Antigravity 上游的协议细节处理不一致，可能有一方踩过我们没踩的坑。方案：先做矩阵，只补有证据的缺口，避免无依据改动。

## 范围

- 工作目录：`/Users/icesword/Documents/AIProjects/matou-login-integration`
- 不能碰主目录、其他 worktree、`safe-browser` 项目、参考仓库。
- 禁止 stash / reset / rebase。
- 不改 Matou 的 UI、IPC、账户逻辑。
- 不推送、不发布。

## 必须先问的地方

- 发现需要改请求布局或缓存前缀才能补的差异，先提问。
- Go 工具链或补丁构建失败，先报告原文。
- 发现现有实现已经覆盖、但行为与 sub2api 不同的项，先列出来由我裁决，不要擅自改。

## 关卡与交付物

一关完成：

1. 对照矩阵（至少覆盖上面 7 项），每项有文件/行号证据和结论。
2. 对判定为真缺口的项实现修复，带 Go 测试；对不修的项写明理由。
3. `pnpm router:build` 通过；Matou 的 `pnpm typecheck`、`pnpm test`、`pnpm build` 通过（时间门槛类失败按之前口径记录）。
4. 提交 commit，信息 `feat: close Antigravity translation gaps`。

回复中给出：矩阵、改动文件、测试名、不修项的理由、`我自己做的决定`。

## 验收（我会亲自复核）

- 读矩阵和代码，确认没有无依据的照搬。
- 亲自跑 `pnpm router:build` 和三条检查。
- 对修复项看前后对比；对不修项确认理由成立。
