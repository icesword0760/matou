# Runtime Task 2 验收报告：安全拖入路径

## 产品结论

Task 2 已闭合。用户把文件拖入终端后：

- 普通路径仍原样显示；只含空格的路径仍显示为双引号形式，与 reference product 当前体验一致。
- 单/双引号、反斜杠、`$()`、反引号、换行、回车、管道、重定向、glob、中文和 Emoji 等文件名会安全地成为一个 zsh 参数。
- 拖入只写入当前输入行，不追加 Enter，不触发文件或命令执行。
- 文件树的结构化路径数据优先；任意 `text/plain` 文本既不会覆盖结构化路径，也不会单独被当作路径送进 Shell。
- 多文件沿拖拽选择顺序写入。
- 普通粘贴链路未修改，用户不会看到新增确认或提示。

## reference product 交互对照矩阵

| 用户场景 | reference product 基线 | Matou 实际结果 | 证据 | 差异结论 |
|---|---|---|---|---|
| 普通路径 | 文件树 producer 原样拼入，终端前置一个空格且不提交 | 同样显示 ` /tmp/.../plain.txt`，光标停在末尾 | reference product `FileTreeNode.vue:669-684`、`ClaudeCodeTerminal.vue:713-724`；`ordinary-path-visible.png` | 逐字符行为一致 |
| 仅含空格 | producer 使用双引号 | 同样显示 ` "/tmp/.../with space.txt"` | `path-drop-source-baseline.json`；`space-path-visible.png` | 逐字符行为一致 |
| Shell 特殊字符 | 当前 reference product 只按空格处理 | 按 D-14 使用 POSIX 单引号；内嵌单引号编码为 `'\''` | `shell-path-quote.test.ts` 的真实 `/bin/zsh` argv 回显 | 已确认的安全增强；入口、反馈和不提交行为不变 |
| 结构化 file-tree + 任意文本 | file-tree MIME 是终端拖入入口 | 解析 MIME JSON 中每个 `path`；忽略同次 `text/plain` | `TerminalSurface.test.tsx` | 更严格的数据权威边界，无用户步骤变化 |
| 仅 `text/plain` | 不是文件拖入入口 | 不显示 drop overlay，不写终端 | `TerminalSurface.test.tsx` | 一致 |

reference product 代码和既有可运行 CLI 视觉上下文记录在 `docs/acceptance/evidence/runtime-task-02/reference/path-drop-source-baseline.json`。本项没有修改终端布局、颜色、动画或拖入 overlay。

## 真实运行验收

Electron 使用真实 PTY 和 zsh 创建并拖入 8 个实际文件名：普通、空格、单引号、`$()`、反斜杠、反引号、换行、Unicode/Emoji。

特殊文件名均由 Python 从真实 zsh 的 `sys.argv[-1]` 回显并逐字比对；若 `$()` 或反引号改变命令结构，本应出现的 `PWNED` / `PWNED_TICK` 文件均未出现。普通和空格路径在按 Enter 前采集 xterm 截图，证明拖入停留在输入行。

- reference product 源码基线：`docs/acceptance/evidence/runtime-task-02/reference/path-drop-source-baseline.json`
- Matou 运行记录：`docs/acceptance/evidence/runtime-task-02/matou/drop-path-runtime-validation.json`
- 普通路径截图：`docs/acceptance/evidence/runtime-task-02/matou/ordinary-path-visible.png`
- 空格路径截图：`docs/acceptance/evidence/runtime-task-02/matou/space-path-visible.png`

## TDD 与验证

1. RED：引用模块不存在；旧结构化拖入透传 `text/plain`；旧 Finder 路径对 `$()` 使用双引号。
2. GREEN：新增单职责 `quoteDroppedPath()`；file-tree MIME 解析结构化节点；Finder 使用 Electron `webUtils.getPathForFile()` 后逐项引用。
3. 定向单元/component：24 tests 通过。
4. 真实 Electron：drop-path 场景通过，8 个真实文件名全部闭合。
5. 全量：contracts 45、domain 7、desktop 361、runtime 515 tests 通过；`pnpm typecheck` 与 `pnpm build` 通过；`terminal-channel.spec.ts` 3 个 Electron 场景通过。
6. 当前审计分支尚未合入 central identifier script；本 Task 新增路径与内容已使用 `reference` 中性术语，统一门禁由后续品牌卫生合并波次执行。
