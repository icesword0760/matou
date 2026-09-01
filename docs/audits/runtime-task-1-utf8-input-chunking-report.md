# Runtime Robustness Task 1：UTF-8 大段输入验收报告

日期：2026-09-01
结论：**已闭合**

## 用户结果

- 大段粘贴立即进入当前终端，不增加确认弹窗、进度弹窗或额外操作。
- 中文、四字节 Emoji、组合字符和超长单行按原顺序完整到达真实 PTY。
- 单次逻辑输入在 Renderer → Runtime 边界透明拆成不超过 256 KiB 的消息；用户看到的仍是一次连续输入。
- 只读恢复模式继续在分块前拦截输入，不产生任何 `terminal.input`。

## 实现边界

- 新增 `splitUtf8ForTransport(value, maxBytes)`：按 Unicode code point 计算 UTF-8 byte length，不使用字节下标切 JavaScript 字符串。
- `RuntimeClient.sendTerminalInput` 在原有只读栅栏之后同步、顺序发送所有 chunk。
- 未改变 xterm 输入、bracketed paste、PTY 写入、交互排序、协议上限或终端状态语义；`TerminalSurface` 无 Task 1 生产改动。

## RED → GREEN

| 阶段 | 运行证据 | 结果 |
|---|---|---|
| RED：纯函数 | `pnpm --filter @matou/desktop test -- terminal-input-chunker.test.ts` | `Cannot find module './terminal-input-chunker'`，目标失败成立 |
| RED：RuntimeClient | `pnpm exec vitest run src/renderer/src/runtime/RuntimeClient.test.ts` | 旧实现只发 1 条，断言要求多于 4 条，目标失败成立 |
| GREEN：定向单元 | `pnpm exec vitest run src/renderer/src/terminal/terminal-input-chunker.test.ts src/renderer/src/runtime/RuntimeClient.test.ts` | 2 files / 13 tests 通过 |
| GREEN：Desktop 全量 | `pnpm --filter @matou/desktop test` | 43 files / 361 tests 通过 |
| GREEN：Runtime 全量 | `pnpm --filter @matou/runtime test` | 71 files / 515 tests 通过 |
| GREEN：类型 | `pnpm typecheck` | 5 个 workspace package 通过 |
| GREEN：构建 | `pnpm build` | contracts、domain、ui、runtime、desktop 全部通过 |
| GREEN：真实 Electron / PTY | `pnpm exec playwright test tests/e2e/terminal-channel.spec.ts --grep "transparently chunks a large UTF-8 paste" --workers=1` | 1 passed；约 3.6 秒 |

根目录并发 `pnpm test` 运行时观察到两个既有 Runtime 时序测试偶发超时；两项定向复跑均通过，Runtime 71 files / 515 tests 独立全量也通过。失败路径未经过 Task 1 的 Desktop 输入代码。

## 真实 PTY 字节验收

输入为精确 `2.5 MiB = 2,621,440 bytes` 的单行内容，包含中文、Emoji `🙂`、组合字符 `e + U+0301` 和固定 ASCII 尾部。PTY 进入 non-canonical、no-echo 模式读取到换行，输出：

```text
LARGE_INPUT_RESULT 2621441 41494c454e44210a
```

- `2,621,441`：原 payload 加终端提交的一个 LF。
- `41494c454e44210a`：最后 8 bytes 为 `AILEND!\n`。
- 页面同时断言无可见 toast、dialog、alertdialog 或终端失败 banner。

## Reference product 交互对照矩阵

| 场景 | Reference product 基线证据 | Matou 实际结果 | 差异结论 |
|---|---|---|---|
| 大段粘贴入口 | `2026-09-01-internal-hardening-product-decisions.md` D-13：立即进入终端，不显示大小确认 | paste 事件直接进入 xterm；页面无确认或错误 UI | 一致 |
| Unicode 内容 | D-13：中文、Emoji 与 bracketed paste 边界完整 | unit 覆盖 surrogate / 组合字符；真实 PTY 覆盖中文、Emoji、组合字符 | 一致 |
| 超协议单消息输入 | D-13：透明有序分块，不因超过 1 MiB 静默丢弃 | 每块 UTF-8 bytes ≤ 256 KiB；1 MiB+ unit 与 2.5 MiB PTY 均完整 | 一致 |
| 只读恢复 | 现有 Matou 恢复合同：终端写入被拦截 | 栅栏位于分块之前；既有只读 mutation test 通过 | 无回归 |

本 Task 不涉及 reference product 黑色 CLI 工作区的布局、视觉、动画、菜单或操作路径变化，因此无需新增像素差异截图。
