# 最终 UI 验收记录

## 最终源码组件浏览器与运行时联动

- 2026-11-17 03:42，最后一次缺 key 校验修复与重新构建完成后重跑：退出码 0，1 passed（17.0 秒），`.cache/round2-e2e.log`。本轮覆盖之前同名日志及截图/请求证据。五张截图均已逐张查看，三档、保存成功、失效保留和 560px 纵排可见。7 条假上游请求按三档 messages/count 和 Sonnet 压缩正确到达；非法 role 返回 502，未产生第 8 条。此 E2E 不专门覆盖最后的缺 key 边界。

## 03:38 修复后组件浏览器与运行时联动

- 2026-11-17 03:38 在最新两项复审修复及 desktop 构建完成后重跑相同命令，退出码 0，1 passed（17.0 秒）；原日志 `.cache/round2-e2e.log` 已被 03:42 重跑覆盖。本轮仅覆盖既有测试场景，不能单凭此测试断言新修复的全部边界。
- 本轮生成的五张截图均已逐张查看：默认三档、编辑、保存成功、模型失效及 560px 纵排可见。`evidence/requests.json` 本轮记录 7 条本地假上游请求：各档 messages/count 分别抵达对应目标，压缩抵达 Sonnet；非法 role 返回 502 且无第 8 条上游请求。截图与请求文件已被本轮覆盖。

## 03:27 修复前组件浏览器与运行时联动

- 2026-11-17 03:27（本机时间），`pnpm exec playwright test tests/e2e/model-provider-routing.spec.ts` 退出码 0，1 passed（17.6 秒）；完整日志 `.cache/final-app-e2e.log`。Playwright 仅为 runner，页面操作由测试自有 OpenCLI session 完成。
- 测试使用正式 `ModelSwitchSettings`、真实本地 RPC/runtime/PTY、假 CLI 和三个本地假上游，隔离配置和假凭据；它是浏览器组件夹具，不是完整 Electron，亦不证明真实 CLI 或订阅可用。
- 页面选择 Opus GPT、Sonnet DeepSeek、Haiku 默认，保存并重开读回；恢复默认后另行保存，压缩设置保留、旧进程 PID 不变。失效模型保留并展示错误。保存后焦点和滚动位置由测试断言。
- `evidence/requests.json` 记录 7 条请求：三档 messages/count 各抵达对应本地目标，压缩抵达 Sonnet；非法 role 返回 502，未产生额外上游请求。所有 key 均为测试假值。
- 已逐张查看 `evidence/default.png`、`editing.png`、`saved.png`、`error-model.png` 和 `narrow-560.png`：可见三档与目标、保存成功提示、失效提示及 560px 纵向排列。截图为组件夹具，不是 Electron 窗口。窄屏截图仅覆盖失效状态，不能代替完整窄屏交互验收。

## OpenCLI 与完整应用

- 本机 OpenCLI 1.8.8；`opencli browser --help` 和 `opencli browser <session> bind --help` 描述 Chrome Browser Bridge / 当前 Chrome 标签绑定，无通用 Electron renderer 或端口参数。`opencli list` 有专用应用适配器，但无 Matou 适配器。现有命令无通用的隔离 Electron 连接入口；按主代理要求不尝试绑定其他 Chrome 标签，也不使用 CDP 或其他 UI 工具。
- 完整 Electron 的正式入口、保存重开、恢复默认、错误、键盘、窄屏操作：OpenCLI 现有能力无可用连接入口，尚未验证。隔离实例尚待启动。

## 本次决定

使用现有测试自有 session 和本地假上游；不改产品代码、不更新 OpenCLI、不连接其他 Chrome 标签、不读取真实账号或使用外部 API。完整 Electron 无法连接时不将组件截图冒充应用验收。
