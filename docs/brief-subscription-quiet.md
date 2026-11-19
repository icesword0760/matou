# 任务：订阅接管会话关闭 Claude Code 自身的遥测与非必要外联

## 目标

在订阅账号驱动的 Claude Code 会话启动环境里，追加三个官方支持的降噪变量，避免把 `matou-chatgpt/*`、`matou-antigravity/*` 模型别名和会话诊断信息上报给 Anthropic 官方遥测服务。

## 实际场景

用户用 ChatGPT 或 Antigravity 订阅跑 Claude Code。请求本身走本地路由，但 Claude Code 客户端仍会向 Anthropic 官方遥测和错误上报服务发请求。这既暴露了"正在用别的订阅跑 Claude Code"的使用特征，也产生无关外联。

## 开发者意图

- 只加环境变量，不改对话、工具执行、会话恢复或官方模式。
- 不影响非订阅会话。
- 不新增依赖、存储或 UI。

## 项目意图

在 worktree `/Users/icesword/Documents/AIProjects/matou-login-integration`（分支 `feat/login-browser-integration`）。先读：

- `apps/runtime/src/model-router/claude-launch.ts` 的 `buildClaudeLaunch`
- `apps/runtime/src/model-router/claude-launch.test.ts`
- `apps/runtime/src/model-router/subscription-native.test.ts`

## 已经定下的决定（照做，不要改）

在 `buildClaudeLaunch` 返回的 `env` 里追加，逐字：

```
CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: '1'
DISABLE_TELEMETRY: '1'
DISABLE_ERROR_REPORTING: '1'
```

- 不要改 `settings.env`（Claude 设置文件），只改进程环境。
- 不要改 `fixedClear` 的清理逻辑；这些变量显式设置后自然覆盖继承值。
- 不影响官方 Claude Code 会话（非订阅路径）。
- 如果现有测试逐字断言 env，更新期望并新增一条断言，确认三个变量存在。

## 原因与方案

原因：订阅会话只关掉了 auto-mode server，没有关掉客户端遥测。方案：在启动环境里显式设置三个官方开关，最小改动。

## 范围

- 工作目录：`/Users/icesword/Documents/AIProjects/matou-login-integration`
- 不能碰主目录、其他 worktree、`safe-browser` 项目。
- 禁止 stash / reset / rebase。
- 不改版本号、不发布、不推送。
- 不改 UI、账户探测缓存（那部分已经存在，默认 5 分钟）。

## 必须先问的地方

- 发现这三个变量会明显改变工具执行、会话恢复或官方模式行为，先提问。
- 发现订阅会话的环境不是由 `buildClaudeLaunch` 的 `env` 直接应用，先提问。

## 关卡与交付物

一关完成：

1. `pnpm typecheck`、`pnpm test`、`pnpm build` 通过。
2. `claude-launch.test.ts` 覆盖三个变量；订阅原生路径的测试不回归。
3. 提交 commit，信息 `feat: silence nonessential traffic for subscription sessions`。

回复中给出：改动文件、测试名、实际 env 片段、`我自己做的决定`。

## 验收（我会亲自复核）

- 读代码确认只改了订阅启动 env，没有改官方路径。
- 亲自跑三条命令。
- 检查三个变量逐字正确。
