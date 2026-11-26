# 订阅账号实现与验收对照

参考来源为 CC Switch v4.0.31，提交 `3c76d3bda670297dd8c27d7d9be9b18a4c9143f5`。MIT 许可见原生工作区的 [subscription-accounts-reference-LICENSE](../../apps/native/third-party/subscription-accounts-reference-LICENSE)。

| 场景 | 当前实现 | 自动验证入口 |
| --- | --- | --- |
| Claude 启动参数、模型别名及环境变量 | [subscription_launch.rs](../../apps/native/runtime/src/subscription_launch.rs) | Rust runtime 单元测试与 [subscription_router.rs](../../apps/native/runtime/tests/process_contract/subscription_router.rs) |
| Claude 角色的窗口预算和回落 | [claude_role_budget.rs](../../apps/native/core/src/claude_role_budget.rs) | [claude_role_budget_contract.rs](../../apps/native/core/tests/claude_role_budget_contract.rs) |
| Codex 和 Antigravity 的账号摘要、额度与模型 | [account_details.rs](../../apps/native/runtime/src/subscription_service/account_details.rs) | Rust runtime 单元测试与 [subscription_router.rs](../../apps/native/runtime/tests/process_contract/subscription_router.rs) |
| 启动、账号绑定、切换及失败反馈 | [subscription_service.rs](../../apps/native/runtime/src/subscription_service.rs) | [subscription_router.rs](../../apps/native/runtime/tests/process_contract/subscription_router.rs) 与 [provider_settings_contract.rs](../../apps/native/runtime/tests/provider_settings_contract.rs) |
| Claude 本地账户凭据事务与隔离登录 | [账户组件](../../tooling/claude-accounts/README.md) | `pnpm test:accounts` |

准备好原生依赖和调试版 runtime 后，通过 `pnpm test:native` 运行 Rust 契约测试。进程测试使用 [订阅路由夹具](../../apps/native/runtime/tests/fixtures/subscription-router.py)。真实账号、真实客户端和发布包的验收需要单独记录。[原生工作区](../../apps/native/README.md)

此前逐个参考函数的移植说明和旧版固定二进制协议回归记录保存在本地归档标签中。

```sh
git show archive/electron-0.2-final:docs/acceptance/subscription-accounts-reference-map.md
```
