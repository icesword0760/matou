# 订阅 system 文本清洗验收记录

基线：CLIProxyAPI 8.0.69，commit `f7ae8feeb3710319f524730849fbe32fde1751e4`。仅更新现有补丁及其 SHA，不更新版本号。

## 范围与已确认调整

- Antigravity：只清洗来自顶层 system 的各个 text part；现有 `IsClaudeCodeAttributionSystemText` 过滤已在 pinned 源码中存在，继续复用，计费块整体丢弃。
- Codex：准确源码将 system 写入 `input` 的 developer 文本，顶层 `instructions` 仅初始化为空。已按监督答复 A，在 `appendSystemText` 内清洗，保持请求布局、计费过滤及空 instructions。
- 替换仅包含三句完整、大小写敏感的身份文本；不新增官方提示词，不做品牌词匹配，不移动、排序或修剪其他文本。
- 本任务未修改用户消息处理、工具名、工具描述、schema、其它翻译路径或 Matou UI/账户逻辑。

## 补丁

旧 SHA-256：`6bc3f5221fe88400a359e250f9899de5470b13f729cfb411f42d78c835436a04`。

新 SHA-256：`86c952bccc51e2b2d51dbb39084f0067fdb29bc4b41f555765ad7563e5a33ff4`。

补丁内本次涉及的文件：

- `internal/translator/antigravity/claude/antigravity_claude_request.go`
- `internal/translator/codex/claude/codex_claude_request.go`
- `internal/util/claude_system_identity.go`（新增纯函数）
- `internal/util/claude_system_identity_test.go`（新增）
- `internal/translator/subscription_prompt_test.go`（新增）

## Go 验证

新增测试：

- `TestNeutralizeClaudeCodeSystemIdentity`：三句替换、周围字节保留、幂等。
- `TestNeutralizeClaudeCodeSystemIdentityPreservesOtherText`：CLAUDE.md、claude.ai、小写 header、裸词 Claude、非完整句、大小写变化均保留。
- `TestSubscriptionPromptTranslations`：Antigravity/Codex × string/array；各 text part 生效，计费块过滤，用户文本、工具名/描述与 schema 描述保留；原 payload 不变，同输入输出稳定；Codex developer 布局及空 instructions 保留。
- `TestSubscriptionPromptBillingOnlySystem`：两条路径、string/array 的纯计费块不生成 system/developer 内容。

已有测试 `TestIsClaudeCodeAttributionSystemText`、`TestStripClaudeCodeAttributionSystem` 和两条翻译器包测试一并执行。

最终构建副本内执行：

```sh
# 在工作区根目录执行；构建副本目录由 commit、patch SHA 和架构决定。
TASK_ROUTER_ROOT="$PWD/.cache/model-router/build/f7ae8feeb371-86c952bccc51-arm64"
TMPDIR="$PWD/.cache/task-tmp" \
GOPATH="$PWD/.cache/model-router/gopath" \
GOCACHE="$PWD/.cache/model-router/gocache" \
GOMODCACHE="$PWD/.cache/model-router/gomod" \
GOFLAGS=-mod=mod \
.cache/model-router/go/1.27.4-darwin-arm64/bin/go -C "$TASK_ROUTER_ROOT" test \
  ./internal/util ./internal/translator \
  ./internal/translator/antigravity/claude ./internal/translator/codex/claude
```

## 实际样例

输入（同一份 payload 分别经过两条真实 Go 翻译函数）：

```json
{
  "system": [
    {
      "type": "text",
      "text": "x-anthropic-billing-header: cc_version=sample; cch=12345;"
    },
    {
      "type": "text",
      "text": "You are Claude Code, Anthropic's official CLI for Claude."
    },
    {
      "type": "text",
      "text": "You are Claude Code, Anthropic's official CLI for Claude, running within the Claude Agent SDK."
    },
    {
      "type": "text",
      "text": "You are a Claude agent, built on Anthropic's Claude Agent SDK."
    },
    {
      "type": "text",
      "text": "Read CLAUDE.md; keep Claude, claude.ai, anthropic-version and x-anthropic-beta."
    }
  ],
  "messages": [
    {
      "role": "user",
      "content": "You are Claude Code, Anthropic's official CLI for Claude.\nx-anthropic-billing-header: user text stays;"
    }
  ],
  "tools": [
    {
      "name": "Claude",
      "description": "You are Claude Code, Anthropic's official CLI for Claude.",
      "input_schema": {
        "type": "object",
        "properties": {
          "text": {
            "type": "string",
            "description": "You are a Claude agent, built on Anthropic's Claude Agent SDK."
          }
        }
      }
    }
  ]
}
```

修改前的输出字段（未列出字段保存在下方完整 JSON 中）：

```json
{
  "antigravity.request.systemInstruction": {
    "role": "user",
    "parts": [
      {
        "text": "You are Claude Code, Anthropic's official CLI for Claude."
      },
      {
        "text": "You are Claude Code, Anthropic's official CLI for Claude, running within the Claude Agent SDK."
      },
      {
        "text": "You are a Claude agent, built on Anthropic's Claude Agent SDK."
      },
      {
        "text": "Read CLAUDE.md; keep Claude, claude.ai, anthropic-version and x-anthropic-beta."
      }
    ]
  },
  "codex.instructions": "",
  "codex.input[0]": {
    "type": "message",
    "role": "developer",
    "content": [
      {
        "type": "input_text",
        "text": "You are Claude Code, Anthropic's official CLI for Claude."
      },
      {
        "type": "input_text",
        "text": "You are Claude Code, Anthropic's official CLI for Claude, running within the Claude Agent SDK."
      },
      {
        "type": "input_text",
        "text": "You are a Claude agent, built on Anthropic's Claude Agent SDK."
      },
      {
        "type": "input_text",
        "text": "Read CLAUDE.md; keep Claude, claude.ai, anthropic-version and x-anthropic-beta."
      }
    ]
  }
}
```

修改后的输出字段：

```json
{
  "antigravity.request.systemInstruction": {
    "role": "user",
    "parts": [
      {
        "text": "You are a coding assistant."
      },
      {
        "text": "You are a coding assistant."
      },
      {
        "text": "You are a coding agent."
      },
      {
        "text": "Read CLAUDE.md; keep Claude, claude.ai, anthropic-version and x-anthropic-beta."
      }
    ]
  },
  "codex.instructions": "",
  "codex.input[0]": {
    "type": "message",
    "role": "developer",
    "content": [
      {
        "type": "input_text",
        "text": "You are a coding assistant."
      },
      {
        "type": "input_text",
        "text": "You are a coding assistant."
      },
      {
        "type": "input_text",
        "text": "You are a coding agent."
      },
      {
        "type": "input_text",
        "text": "Read CLAUDE.md; keep Claude, claude.ai, anthropic-version and x-anthropic-beta."
      }
    ]
  }
}
```

前后完整 JSON 已做结构比对：仅两条路径各三句 system 身份文本改变；用户内容、工具名称/描述、schema 和其它输出完全一致。system 计费块在 pinned 基线中已被两条路径过滤，修改后继续过滤；用户消息里的同名计费文本保持原样。

完整本地产物：

- `.cache/subscription-quiet-evidence/payload.json`
- `.cache/subscription-quiet-evidence/before.json`
- `.cache/subscription-quiet-evidence/after.json`
- `.cache/model-router/8.0.69/darwin-arm64/model-router`
- `apps/desktop/package-resources/model-router/model-router`

## 验收状态

最终全部通过：`pnpm router:build`（含补丁 SHA 校验）、四个 Go 包测试、`pnpm typecheck`、`pnpm build`、统一 `pnpm test`、样例字段比对。

最终 `pnpm test`（2026-11-15，退出码 0）共 2886 项 Vitest 测试通过，30 项按仓库既有条件跳过；另有 6 项 Node 门禁测试通过。通过设置 `MATOU_MODEL_ROUTER_BINARY` 启用了 7 项 pinned 路由器本地协议回归测试，使用 loopback 假上游和假凭据。desktop 功能/性能测试、runtime 主套件、runtime-server 和 journal-range-reader 分段检查全部通过。此前两项时间门槛本轮通过，没有放宽门槛或修改无关代码。

本地二进制 SHA-256：`ac6b33d99354526541cccaf26e0e7c3b4202663b82b1804a0e904f50a124fbef`；缓存副本与 package-resources 副本一致。

验证过程记录（原始日志保留）：

- 首轮 `test.log`：desktop 万节点 p95 95.094ms 超过 50ms；runtime `SegmentJournal > persists the 10,000-line tail boundary and rebuilds a corrupt sidecar` 超过 15000ms。观察到共享机器其它应用高 CPU；未操作其它应用。
- 第二轮 `test-retry.log`：desktop 功能与性能测试全部通过。runtime 的 Unix socket ENOENT/EADDRINUSE、两项 notices 夹具 `require is not defined in ES module scope` 为本轮 TMPDIR 位于仓库深层且继承 type=module 的执行环境问题；另有 journal 时间门槛失败。未修改产品或测试源码。
- `socket-environment-retest.log`：缩短工作区内 TMPDIR 后，67 项 socket 定点测试全部通过。
- 已按监督答复 A，允许仅测试使用系统默认临时目录，最终统一 `pnpm test` 通过；源码、构建缓存和报告仍在本工作区。

日志目录：`/Users/icesword/.claude/agent-foreman/runs/Matou-提示词清洗-06/logs/`。最终日志为 `router-build-final.log`、`go-tests-final.log`、`typecheck.log`、`build.log`、`test-final.log`。

## 我自己做的决定

1. 保留 pinned 源码已经具备的计费过滤实现，用回归测试验证，避免重复实现。
2. 用 `strings.NewReplacer` 实现精确替换，保留所有其他字节，不引入依赖。
3. 样例同时保留修改前和修改后的完整翻译结果，并逐字段比较，确认变化边界。

Codex 清洗位置为已确认的 A 方案，不属于自行扩大范围。未验证线上 WAF/429 是否改善、真实上游工具执行或服务端缓存命中率；本地验证仅证明文本边界、布局保持与确定性。本任务没有界面改动，未启动 UI 验收。
