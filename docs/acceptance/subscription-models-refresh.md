# 订阅供应商手动刷新模型

在「设置 → Claude Code」的订阅卡片中，「刷新模型」位于「编辑」左侧。点击后显示「刷新中…」，同一设置页只发出一次刷新请求。成功后提示「模型列表已更新（N 个模型）」，卡片主模型名称及模型分配选项跟随新目录。已下架的映射和分配保留原值并标为不可用，可以改选。

## 验收记录

| 场景 | 结果 | 证据 |
| --- | --- | --- |
| 点按及刷新中 | 按钮文案变为「刷新中…」，刷新按钮禁用。请求不打开登录流程 | [刷新中](../../output/refresh-models-review/refresh-inflight.png) |
| 刷新成功 | 目录写入磁盘。主模型显示名更新，模型分配出现新选项，失效的旧选项仍选中 | [成功提示](../../output/refresh-models-review/success.png)、[模型分配](../../output/refresh-models-review/assignments.png) |
| 刷新失败 | 读失败、空目录与路由问题保留已保存目录。界面给出原因 | [中文失败](../../output/refresh-models-review/failure.png)、[英文深色失败](../../output/refresh-models-review/english-dark-failure.png) |
| 需重新登录 | 刷新禁用并显示原因，可从卡片发起重新登录。缺少本地凭据时直接停止查询 | [需重新登录](../../output/refresh-models-review/signed-out.png) |
| 关闭设置 | 关闭后的请求结果不会写入页面状态 | 组件用例先关闭设置，再完成挂起的刷新 |
| 英文深色模式 | 成功和失败提示对比度经修正后均可读 | [英文深色成功](../../output/refresh-models-review/english-dark.png)、[英文深色失败](../../output/refresh-models-review/english-dark-failure.png) |

浏览器使用隔离的 Chromium，渲染项目的组件与样式，RPC 使用本地模拟。运行时测试使用固定版本的模型路由和本地模拟上游，覆盖目录读取、持久化回读、已有映射、失败原因、RPC 分发和只读门禁。

- 新增运行时和组件用例在实现前复现入口缺失。最终 runtime 相关 4 个文件共 78 项通过，desktop 相关 6 个文件共 93 项通过。
- 原始 `f9760a3` 的 `SubscriptionSettings.test.tsx` 有 41 项失败。测试中的全页查找会同时命中模型分配和弹窗或卡片，测试模拟也缺少相邻设置组件的返回值。修正定位和模拟数据后，该文件随回归一起通过。
- runtime 与 desktop 类型检查、命名门禁和 `git diff --check` 均通过。运行日志见本次监督任务的 `logs` 目录。

## 实现取舍

- 订阅 RPC 实际由 `runtime-server.ts` 分发。新建 `provider-config.subscription.models-refresh`，旧 `models-read` 继续服务登录和编辑流程。复用现有目录解析和 `cacheSubscriptionModels`，只有账号探测成功且存在可路由模型时才写入。
- 刷新沿用当前账号和额度信息，额度缓存有效期保持原口径。较早启动的额度读取会保留已刷新的目录。
- 编辑弹窗用现有错误样式提示失效模型。冷启动跳过失效映射的别名注册，账号仍可打开设置并改选模型。
- DSH 的两次尝试均在 DeepSeek 传输阶段退出。主代理完成按钮，Terra 在隔离浏览器中实际点按、改选模拟模型并截图。

本关停在工作区审查。真实订阅账号、已安装的桌面应用和发布流程未纳入本轮验收。
