# 审计第二轮 R2-1：共享代码保护器

范围：仅修复 system 文本的共享代码区域识别，Antigravity / Codex 两条转换路径共用。身份句、品牌词、计费行本身的匹配与替换规则未改；用户消息、工具和 schema 的作用范围未改。无新增依赖、完整 Markdown 解析器或运行时 LLM 请求。

## 裁决与可见行为

监督确认原任务“未闭合行内代码不保护”是笔误，改为保留现有“保护到起点所在行行尾”。后续再次确认：跨行行内代码仍在本关范围内，不能因保留未闭合行为而跳过。

- 围栏在行首（允许前导空白）由至少三个反引号开始，允许 info string；仅独立一行、数量不少于开头的反引号闭合，允许两侧空白。
- 围栏中的 `marker = "```"`、更长的嵌入 run、带其它字符的 run 均不提前结束保护；未闭合围栏保护到文本末尾。
- 其它反引号 run 按行内代码处理，仅相同长度的完整 run 能闭合；允许跨行，最多 200 个物理行（包括起点所在行，CRLF 算一个换行）。
- 未找到行内闭合（包括超过 200 行）只保护起点所在行，下一行继续正常识别和清洗。已保护区域内的反引号不重复解析。

## 探针与回归

先在旧补丁上运行新探针，确认两条真实转换入口均失败：围栏内身份句被替换，Antigravity 同时改写中文品牌词，围栏后的正文反而被误保护。原有四个 Go 包基线全部通过。

修复后，经 `ConvertClaudeRequestToAntigravity` / `ConvertClaudeRequestToCodex` 直接构造真实请求：

| 场景 | 结果 |
| --- | --- |
| 审计探针：围栏内 `marker = "```"`，后接完整身份句和腾讯/通义千问/深度求索 | 两条路径逐字保留 |
| 普通围栏、缩进及空白、更长闭合、EOF 闭合 | 正确保护 |
| 内容里的更长 run、非独立 run、短闭合、未闭合围栏 | 不提前结束保护 |
| 行内包含不同长度反引号、跨行 1/2/3 反引号、受保护区域中的围栏形状 | 只由正确闭合 run 结束保护 |
| 200 行以内 / 第 201 行才闭合 | 前者完整保护，后者退回起点所在行 |
| 未闭合单/双反引号、行中三反引号 | 起点所在行保留，下一行继续清洗 |
| 未闭合行内代码之后出现有效围栏 | 后续围栏仍正常识别 |
| 围栏前后的普通身份句与品牌词 | 两路身份句继续替换；Antigravity 中文品牌继续替换，Codex 品牌继续沿用原规则 |
| 用户消息、工具、schema、计费行、品牌窗口 | 原有完整包回归保持通过 |

新增两组测试，共 23 个场景 × 2 条路径 × 2 种 system 形态 × 3 类换行（LF / CRLF / CR），276 次请求转换断言。旧有未闭合行内测试的预期未改。

## 验证记录

- `pnpm router:build`：通过，pinned Go 1.27.4、router 8.0.69。
- 最终构建树上的四个 Go 完整包：`internal/util`、`internal/translator`、`internal/translator/antigravity/claude`、`internal/translator/codex/claude`，`-count=1` 全部通过。
- `pnpm typecheck`：通过。
- `env -u TMPDIR CI=1 pnpm test`：完整退出 0，2879 项 Vitest 通过、37 项按既有配置跳过，另有 6 项 Node 工程门禁通过。未设置 `MATOU_MODEL_ROUTER_BINARY` 的真实路由协议用例按既有条件跳过，没有计入通过。
- `pnpm build`：通过，包含 runtime、Electron main/preload/renderer。
- 提交前 `pnpm check:identifiers` / `git diff --check`：通过。

补丁 SHA256：`96e59fa1889971734a1c2476f45a2f8381d4ace33663e1f8ed8bc843a48bc9e8`。

已读回 `apps/desktop/package-resources/model-router/source.json`，确认与 `version.json` 和补丁内容的 SHA256 相同，并核对二进制哈希。最终构建树与定点测试副本逐字一致；相对旧构建树仅共享保护器及其两路回归测试文件发生变化，`go.mod` / `go.sum` 未变。

测试临时目录沿用既有裁决：仅测试使用系统默认临时目录；非测试构建使用工作区 `.cache/tmp`。未改测试门槛、超时、重试或跳过条件。

原始日志：`/Users/icesword/.claude/agent-foreman/runs/Matou-审计修复-10/logs/`，包括 `go-baseline.log`、`go-probe-before.log`、`go-code-boundaries.log`、`go-final.log`、`router-build.log`、`typecheck.log`、`test-ci.log`、`build.log`。

## 自行决定及未验证

- 将 200 行上限解释为包括起点的物理行，并覆盖 LF / CRLF / CR 的 200/201 边界；无需解析库或新接口。
- 在任务开始时的 `feat/login-browser-integration` 分支提交，不切分支、不推送、不发布。
- 本关无 UI 改动，未操作桌面、未用真实账号进行线上推理；直接转换入口的自动验证不替代这些验收。
