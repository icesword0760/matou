# Runtime Task 2 验收报告：安全拖入路径

## 验收锚点

- Task 2 原始实现：`0eb5f3c9b47ad8a8a8116cffb0f99ee2516b87eb`
- 审查修复与本报告的运行基线：`37f080f5b797225916d91ad1e55d914a65e0f79c`
- 验证方式：从 `37f080f` 建立 clean detached checkout，离线安装锁定依赖后重新 build/test；没有使用共享工作树中的 Task 3、通知或 UI 未提交改动

## 产品结论

Task 2 在上述修复提交上闭合。用户把文件或目录拖入终端后：

- 普通绝对路径仍原样显示；只含空格的绝对路径仍显示为双引号形式，与 reference product 的源码基线一致。
- 单/双引号、反斜杠、`$()`、反引号、换行、回车、管道、重定向、glob、leading `=`、中文和 Emoji 等路径会在 zsh 中成为逐字一致的单个 argv。
- NUL 不属于任何 native 文件名或 shell argv；结构化 MIME 中含 NUL 的伪路径会被丢弃，不会截断成另一个路径进入终端。
- 多文件和目录保持拖拽顺序；拖入只写入当前输入行，不追加 Enter，也不触发命令执行。
- 结构化 file-tree 数据优先于同次 `text/plain`；任意 `text/plain` 或 URI-only `text/uri-list` 不会单独进入 Shell。
- 普通粘贴链路没有被本 Task 修改，用户不会看到新增确认或提示。

## reference product 交互对照矩阵

| 用户场景 | reference product 基线 | Matou 实际结果 | 证据 | 差异结论 |
|---|---|---|---|---|
| 普通路径 | 文件树 producer 原样拼入，终端前置一个空格且不提交 | 显示 ` /tmp/.../plain.txt`，光标停在末尾 | reference source `FileTreeNode.vue:669-684`、`ClaudeCodeTerminal.vue:713-724`；`ordinary-path-visible.png` | 可见文本一致 |
| 仅含空格 | producer 使用双引号 | 显示 ` "/tmp/.../with space.txt"` | `path-drop-source-baseline.json`；`space-path-visible.png` | 可见文本一致 |
| Shell 特殊字符 | reference 只对空格做可见处理 | 按 D-14 使用 POSIX 单引号；内嵌单引号编码为 `'\''`；leading `=` 同样引用 | 真实 `/bin/zsh` argv tests | 已确认的安全增强；入口、反馈和不提交行为不变 |
| 结构化 file-tree + 任意文本 | file-tree MIME 是内部拖入入口 | 逐项读取结构化 `path`；同次 `text/plain` 被忽略 | component + Electron structured drop | 权威数据优先，无额外用户步骤 |
| native 文件/目录 | OS 文件拖入进入标准 `Files` / FileList | Chromium native drag 生成 FileList，preload `webUtils.getPathForFile()` 取绝对路径；文件与目录同序进入 zsh | Electron CDP native `files` drag | macOS 实际主链闭合 |
| 仅 `text/plain` / URI-only `file://` | 不是文件路径权威入口 | 不显示 overlay，不写终端 | component `text/plain`；Electron URI-only drop | fail-closed，一致 |

reference 源码结论和 CLI 视觉上下文记录在 `docs/acceptance/evidence/runtime-task-02/reference/path-drop-source-baseline.json`。现有 reference 截图只用于限定 CLI 模块视觉上下文；普通/空格 drop 的具体基线来自所列源码行，不把空终端截图表述成运行时 drop 证据。

## 真实 Electron / PTY 验收

### 内部结构化拖入

Electron 使用真实 PTY 和 zsh 创建 8 个实际文件名：普通、空格、单引号、`$()`、反斜杠、反引号、换行、Unicode/Emoji。路径由结构化 file-tree MIME 进入 Renderer；同次恶意 `text/plain` 被忽略。

特殊文件名由 Python 从真实 zsh 的 `sys.argv[-1]` 回显并逐字比对。`$()` / backtick 使用绝对环境变量指向副作用文件，不依赖当前目录；两个文件均未出现。普通和空格路径在 Enter 前可见，并用 `Control+U` 清空当前输入，不执行路径。

### native Files / Directory 主链

第二个 Electron 场景通过 Chromium CDP `Input.dispatchDragEvent` 的 native `files` 字段拖入 3 个真实文件和 1 个真实目录。它没有在页面内构造 synthetic `DataTransfer`：Chromium 生成 `DataTransfer.files`，Renderer 经 preload 的 Electron `webUtils.getPathForFile()` 得到绝对路径。普通文件、空格目录、`$()` 文件、Unicode/Emoji 文件保持原顺序，Python 一次性回显完整 argv 数组；按 Enter 前没有结果或副作用。

同一场景再发送只有 `text/uri-list: file:///...`、没有 FileList 的 drop，当前输入行逐字不变。该边界避免把可伪造 URI 文本升级为本地路径；真实 OS 拖入必须提供 Chromium `Files` 主链。

### 平台边界

- 实际运行平台：macOS 15.7.4 arm64、Electron 43.4.1、Node 22.16.0、`/bin/zsh`。
- Renderer 对标准 `Files` / FileList 的处理和 Electron `webUtils.getPathForFile()` API 不含 OS 分支；Windows/Linux 的代码路径相同。
- 本轮没有 Windows/Linux host，因此不把其原生文件管理器 MIME 形态或 PowerShell/cmd 引用声明为运行时已验证。Task 2 的字节级引用结论限定为当前 PRD 指定的 zsh；跨平台发布仍需各 host 的 native drag smoke matrix。

## TDD 与精确验证

### RED → GREEN

1. 原始 RED：引用模块不存在；旧结构化拖入透传 `text/plain`；旧 native path 对 `$()` 使用双引号。
2. 原始 GREEN：`quoteDroppedPath()`、结构化 MIME 权威边界、Electron `webUtils.getPathForFile()` 路径完成。
3. 审查 RED（`37f080f` 前）：
   - leading `=ls` 期望 `'=ls'`，实际裸写为 `=ls`，真实 zsh 将 argv 改成 `/bin/ls`；
   - NUL 期望被丢弃，实际形成含 NUL 的输入；
   - component 期望保留 `/tmp/first.txt`、`=ls`、`/tmp/last.txt`，实际还包含被 zsh 截断的 NUL 路径。
4. 审查 GREEN：3 项回归与既有场景合计 `2 files / 27 tests` 全部通过；native Electron files/directory 和 URI-only 边界同时通过。

### clean `37f080f` fresh verification

```text
pnpm build
=> passed

Task 2 定向 Vitest
=> 2 files / 27 tests passed

pnpm --filter @matou/desktop test
=> 43 files / 355 tests passed

pnpm --filter @matou/contracts test
=> 4 files / 45 tests passed

pnpm --filter @matou/domain test
=> 2 files / 7 tests passed

pnpm --filter @matou/runtime test
=> 72 files / 524 tests passed

pnpm typecheck
=> contracts/domain/ui/desktop/runtime passed

pnpm exec playwright test tests/e2e/terminal-channel.spec.ts --workers=1
=> 4 passed: channel smoke、structured drop、native files/directory drop、large UTF-8 paste
```

`37f080f` 当时仓库还没有 `check:identifiers` script；提交前已对 staged additions 做禁用标识扫描，新增内容使用 `reference` 中性术语，并将本范围既有的品牌化常量/测试名一并替换。

## 证据文件

- reference source baseline：`docs/acceptance/evidence/runtime-task-02/reference/path-drop-source-baseline.json`
- Matou fresh runtime record：`docs/acceptance/evidence/runtime-task-02/matou/drop-path-runtime-validation.json`
- 普通路径截图：`docs/acceptance/evidence/runtime-task-02/matou/ordinary-path-visible.png`
- 空格路径截图：`docs/acceptance/evidence/runtime-task-02/matou/space-path-visible.png`
