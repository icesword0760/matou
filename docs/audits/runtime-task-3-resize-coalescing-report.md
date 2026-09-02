# Runtime Robustness Task 3：终端 Resize 合并交付报告

日期：2026-09-02
范围：16 个同级会话同时处于前台时的窗口拖动、终端尺寸同步与输入连续性

## 用户结果

- 用户连续拖动 App 窗口时，每张前台终端卡片仍即时适配可用空间，但发往 Runtime 的尺寸更新按 60Hz 上限合并。
- 同一帧内出现大量尺寸变化时，仅最后一个有效尺寸进入 Runtime；相邻相同尺寸不再重复触发 PTY resize，也不再写入重复 Journal frame。
- 拖动结束时会提交最后一个有效尺寸，因此最终真实 PTY 的 `stty size` 与 xterm 发出的最终 rows/cols 一致。
- 16 张前台卡片同时存在并持续拖动 2 秒时，终端输入和回显保持连续；滑出横向视野的卡片仍保持真实 PTY/xterm 绑定。

## RED → GREEN 证据

| 阶段 | 运行证据 | 结果 |
|---|---|---|
| RED：Renderer 合并器 | `pnpm --filter @matou/desktop test -- resize-coalescer.test.ts` | 因目标模块不存在而失败；证明测试确实覆盖新增行为 |
| GREEN：fake-rAF 单测 | `pnpm --filter @matou/desktop exec vitest run src/renderer/src/terminal/resize-coalescer.test.ts` | 1 file / 5 tests 通过；覆盖单帧 100 次 offer、跨帧去重、60Hz 节流、flush、dispose |
| RED：Runtime 二次去重 | `pnpm --filter @matou/runtime exec vitest run src/session/pty-session.test.ts` | 收到 2 个相同 resize Journal frame，预期 1 个；目标失败成立 |
| GREEN：Runtime + Journal | 同一命令复跑 | 1 file / 2 tests 通过；重复尺寸仅保留 1 个 frame |
| 组件回归 | `pnpm --filter @matou/desktop exec vitest run ...resize-coalescer.test.ts ...TerminalSurface.test.tsx` | 2 files / 27 tests 通过 |
| 真实 Electron / PTY | `playwright ...session-canvas-navigation.spec.ts --grep "coalesces real window dragging" --repeat-each=3` | 16 个唯一 Session / PTY PID、2.1 秒真实窗口尺寸变化、最终独立尺寸落点，连续 3 次全部通过 |
| 构建 | `pnpm build:packages && pnpm build:runtime && pnpm build:desktop` | packages、Runtime、Electron main/preload/renderer 全部构建成功 |

## 真实验收场景

1. 创建 16 个同级 Shell 会话，并等待 16 个真实 PTY PID 全部就绪。
2. 在 Renderer 的真实 `MessagePort.postMessage` 边界做只读计数，消息仍原样进入 Runtime。
3. 以 8ms 间隔连续改变真实 `BrowserWindow` 尺寸 2.1 秒，同时在活动终端键入并执行输出命令。
4. 对每个产生尺寸变化的会话计算任意连续 1 秒窗口内的 `terminal.resize` 数量，均不超过 60。
5. 先输入但不提交 `stty size`，再移动到独立最终窗口尺寸；等待活动会话的 resize 流连续 4 个动画帧保持稳定后提交，随后重新读取 probe。真实 PTY 结果与当前最终 rows/cols 完全一致。
6. 终端画面出现输入回显 marker，证明窗口拖动期间输入链路没有中断。

## 边界说明

- `ResizeObserver` 仍立即执行 `fit.fit()`，用户看到的卡片适配没有额外等待；仅高频 Runtime/PTY 更新被合并。
- 横向列表中滑出视野的会话仍属于前台，本任务没有引入 viewport 级终端解绑。
- Runtime 保留第二层去重，即使未来出现其它 Renderer 或重复消息，PTY 与 Journal 也不会重复处理相同尺寸。
