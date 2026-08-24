# Execution Contexts

ExecutionContext 将 Task 映射到受控 cwd。Runtime 通过 `executionContextId` 解析 PlainDirectory 或 Git Worktree；Renderer 不直接指定实际路径。

