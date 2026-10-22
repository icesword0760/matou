# 外部 Agent 实时状态约定（版本 1）

第三方监督脚本、任务执行器可以将状态写入会话 PTY 的 `MATOU_AGENT_STATUS_DIR` 环境变量指向的绝对目录。无需连接 Matou 控制接口，也不依赖某个工具或项目的文件格式。目录位于 Matou 数据根目录下的 `agent-status/<sessionId>/`，与 `journal` 同级，由 Runtime 在启动 PTY 前尽力创建。若创建失败，Runtime 不注入该变量，终端仍正常启动；第三方工具应在变量缺失时跳过状态上报，继续主任务。目录删除失败也不阻断会话清理。

同一会话重启、恢复时沿用此目录，不清空；会话被移除或明确销毁时删除目录。窗口断开仅停止当前窗口的监听，不代表会话被销毁。只有已附着且处于 agent 模式的会话会读取并推送状态。

## 写入格式

一个任务一个 `<id>.json` 文件。id 仅允许 `[A-Za-z0-9._-]`，长度 1–80 字符。请保持 id 稳定；Matou 条目 ID 为 `external:<id>`。

```json
{"version":1,"label":"外部任务：render-01","state":"running","detail":"运行 python3 render.py","startedAt":"2026-10-15T03:00:00.000Z","updatedAt":"2026-10-15T03:06:12.000Z"}
```

必填字段为 `version`（固定 1）、`label`、`state`、`startedAt`、`updatedAt`。`detail` 可选。两个时间字段支持 ISO 日期字符串或毫秒时间戳数字。label 和 detail 只取首行，分别截断至 60、120 字符；空 label 无效。

state 可取 `running`、`waiting`、`warning`、`error`、`completed`、`stopped`。任务进行中（含等待或警告状态）至少每 20 秒刷新一次 updatedAt。失败时写 error 和简短原因；正常结束写 completed；主动停止写 stopped，也可删除文件。

必须先写同目录临时文件，再 rename 覆盖目标文件；临时文件不要以 `.json` 结尾。例如 Python：

```python
import json
import os
from datetime import datetime, timezone
from pathlib import Path

status_dir = Path(os.environ["MATOU_AGENT_STATUS_DIR"])
task_id = "render-01"
started_at = datetime.now(timezone.utc).isoformat()

def publish(state, detail):
    value = {
        "version": 1,
        "label": "外部任务：render-01",
        "state": state,
        "detail": detail,
        "startedAt": started_at,
        "updatedAt": datetime.now(timezone.utc).isoformat(),
    }
    temporary = status_dir / f"{task_id}.{os.getpid()}.tmp"
    temporary.write_text(json.dumps(value, ensure_ascii=False), encoding="utf-8")
    os.replace(temporary, status_dir / f"{task_id}.json")

publish("running", "开始处理")
# 执行任务期间每隔不超过 20 秒调用 publish，更新当前步骤。
# 完成后调用 publish("completed", "处理完成")。
```

## 显示与失联

- completed / stopped 不显示。
- error 显示至 updatedAt 后 5 分钟，到期隐藏。
- running / waiting / warning 超过 60 秒没有心跳，显示为 error，detail 固定为 `状态已停止更新`。这是 Runtime 的稳定失联标记；渲染层应将该完整字符串本地化，英文为 `Status stopped updating`。失联条目持续显示，直到收到新心跳、完成状态或文件删除；它不适用显式 error 的 5 分钟隐藏规则。
- 新的坏 JSON、缺少字段或空 label 被忽略；如果同文件曾读取成功，保留上次有效内容，仍按旧 updatedAt 判断失联或到期。文件删除则移除条目。
- Runtime 使用目录监听和每 2 秒轮询；仅在可见条目变化时推送。更新时间变化但显示结果不变，不重复推送。
- 外部任务与内置子 Agent 合并排序：error → warning → waiting → running，同级按 startedAt 升序，无 startedAt 排后。旧 subagentCount 与 subagents 分别同步为 agents.length 与 agents.map(a => a.label)。

本关提供 Runtime 数据和契约，列表的视觉呈现与本地化在渲染层后续接入。
