use std::collections::BTreeSet;

use serde::Deserialize;
use serde_json::{Map, Value, json};

// Offline capture from the frozen renderer and RPC. The gate runner checks that
// these bytes match a fresh replay. The core test needs neither Node nor the
// local evidence directory and does not call a native dispatch implementation.
const FIXTURE_JSON: &str = r####"
{
  "version": 1,
  "reference": "924de28da27999543aeef7a7c9797334fca45d97",
  "cases": [
    {
      "id": "workspace-global",
      "source": [{"file":"apps/desktop/src/renderer/src/hierarchy/ModelSwitchSettings.tsx","lines":"399-401"},{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"64-80"},{"file":"apps/runtime/src/rpc/runtime-rpc-router.ts","lines":"183-191"},{"file":"apps/runtime/src/rpc/dispatch-config.ts","lines":"15-35"},{"file":"packages/contracts/src/dispatch-config.ts","lines":"3-18"}],
      "boundary": "冻结入口没有工作区级覆盖；额外 payload 和文件顶层 workspaces 都是探测输入。",
      "input": {
        "file": {"version":1,"defaults":{"model":"astra","idleMin":14},"workspaces":{"alpha":{"agent":"dsh","model":"workspace-model","effort":"low","idleMin":1},"beta":{"model":"luna"}}},
        "steps": [
          {"op":"snapshot","as":"alpha","payload":{"workspaceId":"alpha","workspaceRoot":"workspace-alpha","defaults":{"agent":"dsh","model":"workspace-model","effort":"low","idleMin":1}}},
          {"op":"snapshot","as":"beta","payload":{"workspaceId":"beta","defaults":{"model":"luna"}}},
          {"op":"open"},
          {"op":"view","as":"panel"},
          {"op":"requests","as":"panelRequests"},
          {"op":"disk","as":"unchanged"}
        ]
      },
      "expected": {
        "alpha": {"exists":true,"path":"$FIXTURE_HOME/.claude/agent-foreman/config.json","defaults":{"agent":"codex","model":"astra","effort":"high","idleMin":14,"maxMin":45,"askWaitMin":10,"retries":3}},
        "beta": {"exists":true,"path":"$FIXTURE_HOME/.claude/agent-foreman/config.json","defaults":{"agent":"codex","model":"astra","effort":"high","idleMin":14,"maxMin":45,"askWaitMin":10,"retries":3}},
        "panel": {"hasForm":true,"choices":{"agent":{"label":"默认执行者","value":"codex","shownLabel":"Codex","options":[{"value":"codex","label":"Codex"},{"value":"agy","label":"Antigravity"},{"value":"dsh","label":"DSH"}],"disabled":false,"readOnly":false,"hints":[]},"model":{"label":"默认模型","value":"astra","shownLabel":"astra","options":[{"value":"sol","label":"sol"},{"value":"best","label":"best"},{"value":"astra","label":"astra"},{"value":"terra","label":"terra"},{"value":"luna","label":"luna"}],"disabled":false,"readOnly":false,"hints":[]},"effort":{"label":"推理档位","value":"high","shownLabel":"高","options":[{"value":"low","label":"低"},{"value":"medium","label":"中"},{"value":"high","label":"高"},{"value":"xhigh","label":"超高"}],"disabled":false,"readOnly":false,"hints":[]}},"numbers":{"idleMin":{"label":"没动静判卡","value":"14","error":null,"unit":"分钟"},"maxMin":{"label":"单轮最长","value":"45","error":null,"unit":"分钟"},"askWaitMin":{"label":"等回答","value":"10","error":null,"unit":"分钟"},"retries":{"label":"重试次数","value":"3","error":null,"unit":null}},"alerts":[],"activeField":null,"save":{"label":"保存","disabled":true,"ariaDisabled":true,"status":null},"restoreLabel":"恢复默认","scope":"只影响新派的任务；单个任务仍可用命令行参数覆盖。","path":"~/.claude/agent-foreman/config.json","missing":null},
        "panelRequests": [{"method":"dispatch-config.snapshot","payload":{}}],
        "unchanged": {"bytes":"{\"version\":1,\"defaults\":{\"model\":\"astra\",\"idleMin\":14},\"workspaces\":{\"alpha\":{\"agent\":\"dsh\",\"model\":\"workspace-model\",\"effort\":\"low\",\"idleMin\":1},\"beta\":{\"model\":\"luna\"}}}","files":["config.json"]}
      }
    },
    {
      "id": "workspace-without-global",
      "source": [{"file":"apps/runtime/src/rpc/runtime-rpc-router.ts","lines":"183-191"},{"file":"apps/runtime/src/rpc/dispatch-config.ts","lines":"15-35"},{"file":"packages/contracts/src/dispatch-config.ts","lines":"3-18"}],
      "boundary": "工作区候选不会填补缺省的全局值。",
      "input": {
        "file": {"version":1,"workspaces":{"alpha":{"agent":"dsh","model":"workspace-model","effort":"low","idleMin":1}}},
        "steps": [
          {"op":"snapshot","as":"alpha","payload":{"workspaceId":"alpha","workspaceRoot":"workspace-alpha","defaults":{"agent":"dsh","model":"workspace-model","effort":"low","idleMin":1}}}
        ]
      },
      "expected": {
        "alpha": {"exists":true,"path":"$FIXTURE_HOME/.claude/agent-foreman/config.json","defaults":{"agent":"codex","model":"sol","effort":"high","idleMin":10,"maxMin":45,"askWaitMin":10,"retries":3}}
      }
    },
    {
      "id": "workspace-save-is-global",
      "source": [{"file":"apps/runtime/src/rpc/runtime-rpc-router.ts","lines":"183-191"},{"file":"apps/runtime/src/rpc/dispatch-config.ts","lines":"15-35"},{"file":"packages/contracts/src/dispatch-config.ts","lines":"3-18"},{"file":"apps/runtime/src/rpc/runtime-rpc-router.ts","lines":"183-191"},{"file":"apps/runtime/src/rpc/dispatch-config.ts","lines":"38-55"},{"file":"packages/contracts/src/dispatch-config.ts","lines":"3-18"}],
      "boundary": "额外元数据不会把保存变成工作区专属写入。",
      "input": {
        "file": {"version":1,"defaults":{"model":"astra"},"workspaces":{"alpha":{"agent":"dsh","model":"workspace-model","effort":"low","idleMin":1}}},
        "steps": [
          {"op":"rpc-save","as":"saved","payload":{"workspaceId":"alpha","workspaceRoot":"workspace-alpha","defaults":{"agent":"agy","model":"hydrogen","effort":"high","idleMin":10,"maxMin":45,"askWaitMin":10,"retries":0}}},
          {"op":"snapshot","as":"alpha","payload":{"workspaceId":"alpha","workspaceRoot":"workspace-alpha","defaults":{"agent":"dsh","model":"workspace-model","effort":"low","idleMin":1}}},
          {"op":"snapshot","as":"beta","payload":{"workspaceId":"beta"}},
          {"op":"disk","as":"savedFile"}
        ]
      },
      "expected": {
        "saved": {"exists":true,"path":"$FIXTURE_HOME/.claude/agent-foreman/config.json","defaults":{"agent":"agy","model":"hydrogen","effort":"high","idleMin":10,"maxMin":45,"askWaitMin":10,"retries":0}},
        "alpha": {"exists":true,"path":"$FIXTURE_HOME/.claude/agent-foreman/config.json","defaults":{"agent":"agy","model":"hydrogen","effort":"high","idleMin":10,"maxMin":45,"askWaitMin":10,"retries":0}},
        "beta": {"exists":true,"path":"$FIXTURE_HOME/.claude/agent-foreman/config.json","defaults":{"agent":"agy","model":"hydrogen","effort":"high","idleMin":10,"maxMin":45,"askWaitMin":10,"retries":0}},
        "savedFile": {"bytes":"{\n  \"version\": 1,\n  \"defaults\": {\n    \"agent\": \"agy\",\n    \"model\": \"hydrogen\",\n    \"effort\": \"high\",\n    \"idleMin\": 10,\n    \"maxMin\": 45,\n    \"askWaitMin\": 10,\n    \"retries\": 0\n  }\n}\n","files":["config.json"]}
      }
    },
    {
      "id": "default-first-read",
      "source": [{"file":"apps/runtime/src/rpc/runtime-rpc-router.ts","lines":"183-191"},{"file":"apps/runtime/src/rpc/dispatch-config.ts","lines":"15-35"},{"file":"packages/contracts/src/dispatch-config.ts","lines":"3-18"},{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"13-19"},{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"187-248"},{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"332-375"},{"file":"apps/desktop/src/renderer/src/i18n/messages/hierarchyShell.ts","lines":"229-244"},{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"264-286"},{"file":"apps/desktop/src/renderer/src/i18n/messages/hierarchyShell.ts","lines":"245-263"}],
      "boundary": "记录中文标签、选项顺序、显示路径和首次读取的副作用。",
      "input": {
        "steps": [
          {"op":"snapshot","as":"first","payload":{}},
          {"op":"open"},
          {"op":"view","as":"panel"},
          {"op":"disk","as":"stillAbsent"}
        ]
      },
      "expected": {
        "first": {"exists":false,"path":"$FIXTURE_HOME/.claude/agent-foreman/config.json","defaults":{"agent":"codex","model":"sol","effort":"high","idleMin":10,"maxMin":45,"askWaitMin":10,"retries":3}},
        "panel": {"hasForm":true,"choices":{"agent":{"label":"默认执行者","value":"codex","shownLabel":"Codex","options":[{"value":"codex","label":"Codex"},{"value":"agy","label":"Antigravity"},{"value":"dsh","label":"DSH"}],"disabled":false,"readOnly":false,"hints":[]},"model":{"label":"默认模型","value":"sol","shownLabel":"sol","options":[{"value":"sol","label":"sol"},{"value":"best","label":"best"},{"value":"astra","label":"astra"},{"value":"terra","label":"terra"},{"value":"luna","label":"luna"}],"disabled":false,"readOnly":false,"hints":[]},"effort":{"label":"推理档位","value":"high","shownLabel":"高","options":[{"value":"low","label":"低"},{"value":"medium","label":"中"},{"value":"high","label":"高"},{"value":"xhigh","label":"超高"}],"disabled":false,"readOnly":false,"hints":[]}},"numbers":{"idleMin":{"label":"没动静判卡","value":"10","error":null,"unit":"分钟"},"maxMin":{"label":"单轮最长","value":"45","error":null,"unit":"分钟"},"askWaitMin":{"label":"等回答","value":"10","error":null,"unit":"分钟"},"retries":{"label":"重试次数","value":"3","error":null,"unit":null}},"alerts":[],"activeField":null,"save":{"label":"保存","disabled":true,"ariaDisabled":true,"status":null},"restoreLabel":"恢复默认","scope":"只影响新派的任务；单个任务仍可用命令行参数覆盖。","path":"~/.claude/agent-foreman/config.json","missing":"文件尚未创建，首次保存时会自动创建。"},
        "stillAbsent": {"bytes":null,"files":null}
      }
    },
    {
      "id": "version-defaults-missing",
      "source": [{"file":"apps/runtime/src/rpc/runtime-rpc-router.ts","lines":"183-191"},{"file":"apps/runtime/src/rpc/dispatch-config.ts","lines":"15-35"},{"file":"packages/contracts/src/dispatch-config.ts","lines":"3-18"}],
      "boundary": "存在性只看 v1 文档有效，defaults 非对象可回落。",
      "input": {
        "file": {"version":1,"defaults":null},
        "steps": [
          {"op":"snapshot","as":"read","payload":{}}
        ]
      },
      "expected": {
        "read": {"exists":true,"path":"$FIXTURE_HOME/.claude/agent-foreman/config.json","defaults":{"agent":"codex","model":"sol","effort":"high","idleMin":10,"maxMin":45,"askWaitMin":10,"retries":3}}
      }
    },
    {
      "id": "unsupported-version",
      "source": [{"file":"apps/runtime/src/rpc/runtime-rpc-router.ts","lines":"183-191"},{"file":"apps/runtime/src/rpc/dispatch-config.ts","lines":"15-35"},{"file":"packages/contracts/src/dispatch-config.ts","lines":"3-18"}],
      "boundary": "版本号要求数字 1，读取不修复文件。",
      "input": {
        "file": {"version":"1","defaults":{"model":"astra"}},
        "steps": [
          {"op":"snapshot","as":"read","payload":{}},
          {"op":"disk","as":"unchanged"}
        ]
      },
      "expected": {
        "read": {"exists":false,"path":"$FIXTURE_HOME/.claude/agent-foreman/config.json","defaults":{"agent":"codex","model":"sol","effort":"high","idleMin":10,"maxMin":45,"askWaitMin":10,"retries":3}},
        "unchanged": {"bytes":"{\"version\":\"1\",\"defaults\":{\"model\":\"astra\"}}","files":["config.json"]}
      }
    },
    {
      "id": "malformed-file",
      "source": [{"file":"apps/runtime/src/rpc/runtime-rpc-router.ts","lines":"183-191"},{"file":"apps/runtime/src/rpc/dispatch-config.ts","lines":"15-35"},{"file":"packages/contracts/src/dispatch-config.ts","lines":"3-18"},{"file":"apps/runtime/src/rpc/runtime-rpc-router.ts","lines":"183-191"},{"file":"apps/runtime/src/rpc/dispatch-config.ts","lines":"38-55"},{"file":"packages/contracts/src/dispatch-config.ts","lines":"3-18"}],
      "boundary": "读取不修复损坏文件，保存会替换它。",
      "input": {
        "raw": "{broken",
        "steps": [
          {"op":"snapshot","as":"read","payload":{}},
          {"op":"disk","as":"before"},
          {"op":"rpc-save","as":"saved","payload":{"defaults":{"agent":"codex","model":"luna","effort":"high","idleMin":10,"maxMin":45,"askWaitMin":10,"retries":3}}},
          {"op":"snapshot","as":"reread","payload":{}},
          {"op":"disk","as":"after"}
        ]
      },
      "expected": {
        "read": {"exists":false,"path":"$FIXTURE_HOME/.claude/agent-foreman/config.json","defaults":{"agent":"codex","model":"sol","effort":"high","idleMin":10,"maxMin":45,"askWaitMin":10,"retries":3}},
        "before": {"bytes":"{broken","files":["config.json"]},
        "saved": {"exists":true,"path":"$FIXTURE_HOME/.claude/agent-foreman/config.json","defaults":{"agent":"codex","model":"luna","effort":"high","idleMin":10,"maxMin":45,"askWaitMin":10,"retries":3}},
        "reread": {"exists":true,"path":"$FIXTURE_HOME/.claude/agent-foreman/config.json","defaults":{"agent":"codex","model":"luna","effort":"high","idleMin":10,"maxMin":45,"askWaitMin":10,"retries":3}},
        "after": {"bytes":"{\n  \"version\": 1,\n  \"defaults\": {\n    \"agent\": \"codex\",\n    \"model\": \"luna\",\n    \"effort\": \"high\",\n    \"idleMin\": 10,\n    \"maxMin\": 45,\n    \"askWaitMin\": 10,\n    \"retries\": 3\n  }\n}\n","files":["config.json"]}
      }
    },
    {
      "id": "save-reload-restart",
      "source": [{"file":"apps/runtime/src/rpc/runtime-rpc-router.ts","lines":"183-191"},{"file":"apps/runtime/src/rpc/dispatch-config.ts","lines":"15-35"},{"file":"packages/contracts/src/dispatch-config.ts","lines":"3-18"},{"file":"apps/runtime/src/rpc/runtime-rpc-router.ts","lines":"183-191"},{"file":"apps/runtime/src/rpc/dispatch-config.ts","lines":"38-55"},{"file":"packages/contracts/src/dispatch-config.ts","lines":"3-18"},{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"74-94"},{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"108-144"},{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"253-259"},{"file":"apps/desktop/src/renderer/src/i18n/messages/hierarchyShell.ts","lines":"250-255"}],
      "boundary": "新进程只验证文件回读。面板文本随保存响应从 \"1.50\" 变为 \"1.5\"。",
      "input": {
        "steps": [
          {"op":"open"},
          {"op":"change","field":"model","value":"astra"},
          {"op":"change","field":"idleMin","value":"1.50"},
          {"op":"change","field":"retries","value":"0"},
          {"op":"disk","as":"before"},
          {"op":"save-button"},
          {"op":"view","as":"savedPanel"},
          {"op":"requests","as":"requests"},
          {"op":"disk","as":"savedFile"},
          {"op":"snapshot","as":"reread","payload":{}},
          {"op":"fresh-process","as":"freshProcess"},
          {"op":"close"},
          {"op":"open"},
          {"op":"view","as":"remountedPanel"}
        ]
      },
      "expected": {
        "before": {"bytes":null,"files":null},
        "savedPanel": {"hasForm":true,"choices":{"agent":{"label":"默认执行者","value":"codex","shownLabel":"Codex","options":[{"value":"codex","label":"Codex"},{"value":"agy","label":"Antigravity"},{"value":"dsh","label":"DSH"}],"disabled":false,"readOnly":false,"hints":[]},"model":{"label":"默认模型","value":"astra","shownLabel":"astra","options":[{"value":"sol","label":"sol"},{"value":"best","label":"best"},{"value":"astra","label":"astra"},{"value":"terra","label":"terra"},{"value":"luna","label":"luna"}],"disabled":false,"readOnly":false,"hints":[]},"effort":{"label":"推理档位","value":"high","shownLabel":"高","options":[{"value":"low","label":"低"},{"value":"medium","label":"中"},{"value":"high","label":"高"},{"value":"xhigh","label":"超高"}],"disabled":false,"readOnly":false,"hints":[]}},"numbers":{"idleMin":{"label":"没动静判卡","value":"1.5","error":null,"unit":"分钟"},"maxMin":{"label":"单轮最长","value":"45","error":null,"unit":"分钟"},"askWaitMin":{"label":"等回答","value":"10","error":null,"unit":"分钟"},"retries":{"label":"重试次数","value":"0","error":null,"unit":null}},"alerts":[],"activeField":null,"save":{"label":"保存","disabled":false,"ariaDisabled":true,"status":"已保存，新派任务生效"},"restoreLabel":"恢复默认","scope":"只影响新派的任务；单个任务仍可用命令行参数覆盖。","path":"~/.claude/agent-foreman/config.json","missing":null},
        "requests": [{"method":"dispatch-config.snapshot","payload":{}},{"method":"dispatch-config.save","payload":{"defaults":{"agent":"codex","model":"astra","effort":"high","idleMin":1.5,"maxMin":45,"askWaitMin":10,"retries":0}}}],
        "savedFile": {"bytes":"{\n  \"version\": 1,\n  \"defaults\": {\n    \"agent\": \"codex\",\n    \"model\": \"astra\",\n    \"effort\": \"high\",\n    \"idleMin\": 1.5,\n    \"maxMin\": 45,\n    \"askWaitMin\": 10,\n    \"retries\": 0\n  }\n}\n","files":["config.json"]},
        "reread": {"exists":true,"path":"$FIXTURE_HOME/.claude/agent-foreman/config.json","defaults":{"agent":"codex","model":"astra","effort":"high","idleMin":1.5,"maxMin":45,"askWaitMin":10,"retries":0}},
        "freshProcess": {"exists":true,"path":"$FIXTURE_HOME/.claude/agent-foreman/config.json","defaults":{"agent":"codex","model":"astra","effort":"high","idleMin":1.5,"maxMin":45,"askWaitMin":10,"retries":0}},
        "remountedPanel": {"hasForm":true,"choices":{"agent":{"label":"默认执行者","value":"codex","shownLabel":"Codex","options":[{"value":"codex","label":"Codex"},{"value":"agy","label":"Antigravity"},{"value":"dsh","label":"DSH"}],"disabled":false,"readOnly":false,"hints":[]},"model":{"label":"默认模型","value":"astra","shownLabel":"astra","options":[{"value":"sol","label":"sol"},{"value":"best","label":"best"},{"value":"astra","label":"astra"},{"value":"terra","label":"terra"},{"value":"luna","label":"luna"}],"disabled":false,"readOnly":false,"hints":[]},"effort":{"label":"推理档位","value":"high","shownLabel":"高","options":[{"value":"low","label":"低"},{"value":"medium","label":"中"},{"value":"high","label":"高"},{"value":"xhigh","label":"超高"}],"disabled":false,"readOnly":false,"hints":[]}},"numbers":{"idleMin":{"label":"没动静判卡","value":"1.5","error":null,"unit":"分钟"},"maxMin":{"label":"单轮最长","value":"45","error":null,"unit":"分钟"},"askWaitMin":{"label":"等回答","value":"10","error":null,"unit":"分钟"},"retries":{"label":"重试次数","value":"0","error":null,"unit":null}},"alerts":[],"activeField":null,"save":{"label":"保存","disabled":true,"ariaDisabled":true,"status":null},"restoreLabel":"恢复默认","scope":"只影响新派的任务；单个任务仍可用命令行参数覆盖。","path":"~/.claude/agent-foreman/config.json","missing":null}
      }
    },
    {
      "id": "cancel-and-restore",
      "source": [{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"53-94"},{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"147-184"},{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"253-259"},{"file":"apps/runtime/src/rpc/runtime-rpc-router.ts","lines":"183-191"},{"file":"apps/runtime/src/rpc/dispatch-config.ts","lines":"38-55"},{"file":"packages/contracts/src/dispatch-config.ts","lines":"3-18"}],
      "boundary": "关闭指离线组件卸载，恢复默认只改草稿。",
      "input": {
        "file": {"version":1,"defaults":{"agent":"agy","model":"hydrogen","effort":"medium","maxMin":90}},
        "steps": [
          {"op":"open"},
          {"op":"change","field":"maxMin","value":"91"},
          {"op":"view","as":"edited"},
          {"op":"close"},
          {"op":"open"},
          {"op":"view","as":"reopened"},
          {"op":"restore-button"},
          {"op":"view","as":"restoredDraft"},
          {"op":"disk","as":"beforeSave"},
          {"op":"save-button"},
          {"op":"view","as":"savedPanel"},
          {"op":"snapshot","as":"afterSave","payload":{}}
        ]
      },
      "expected": {
        "edited": {"hasForm":true,"choices":{"agent":{"label":"默认执行者","value":"agy","shownLabel":"Antigravity","options":[{"value":"codex","label":"Codex"},{"value":"agy","label":"Antigravity"},{"value":"dsh","label":"DSH"}],"disabled":false,"readOnly":false,"hints":[]},"model":{"label":"默认模型","value":"hydrogen","shownLabel":"hydrogen","options":[{"value":"gemini-pro-orchestrator","label":"Orchestrator（固定）"},{"value":"hydrogen","label":"hydrogen"},{"value":"argon","label":"argon"},{"value":"carbon","label":"carbon"}],"disabled":false,"readOnly":false,"hints":[]},"effort":{"label":"推理档位","value":"medium","shownLabel":"中","options":[{"value":"low","label":"低"},{"value":"medium","label":"中"},{"value":"high","label":"高"},{"value":"xhigh","label":"超高"}],"disabled":false,"readOnly":false,"hints":[]}},"numbers":{"idleMin":{"label":"没动静判卡","value":"10","error":null,"unit":"分钟"},"maxMin":{"label":"单轮最长","value":"91","error":null,"unit":"分钟"},"askWaitMin":{"label":"等回答","value":"10","error":null,"unit":"分钟"},"retries":{"label":"重试次数","value":"3","error":null,"unit":null}},"alerts":[],"activeField":null,"save":{"label":"保存","disabled":false,"ariaDisabled":false,"status":null},"restoreLabel":"恢复默认","scope":"只影响新派的任务；单个任务仍可用命令行参数覆盖。","path":"~/.claude/agent-foreman/config.json","missing":null},
        "reopened": {"hasForm":true,"choices":{"agent":{"label":"默认执行者","value":"agy","shownLabel":"Antigravity","options":[{"value":"codex","label":"Codex"},{"value":"agy","label":"Antigravity"},{"value":"dsh","label":"DSH"}],"disabled":false,"readOnly":false,"hints":[]},"model":{"label":"默认模型","value":"hydrogen","shownLabel":"hydrogen","options":[{"value":"gemini-pro-orchestrator","label":"Orchestrator（固定）"},{"value":"hydrogen","label":"hydrogen"},{"value":"argon","label":"argon"},{"value":"carbon","label":"carbon"}],"disabled":false,"readOnly":false,"hints":[]},"effort":{"label":"推理档位","value":"medium","shownLabel":"中","options":[{"value":"low","label":"低"},{"value":"medium","label":"中"},{"value":"high","label":"高"},{"value":"xhigh","label":"超高"}],"disabled":false,"readOnly":false,"hints":[]}},"numbers":{"idleMin":{"label":"没动静判卡","value":"10","error":null,"unit":"分钟"},"maxMin":{"label":"单轮最长","value":"90","error":null,"unit":"分钟"},"askWaitMin":{"label":"等回答","value":"10","error":null,"unit":"分钟"},"retries":{"label":"重试次数","value":"3","error":null,"unit":null}},"alerts":[],"activeField":null,"save":{"label":"保存","disabled":true,"ariaDisabled":true,"status":null},"restoreLabel":"恢复默认","scope":"只影响新派的任务；单个任务仍可用命令行参数覆盖。","path":"~/.claude/agent-foreman/config.json","missing":null},
        "restoredDraft": {"hasForm":true,"choices":{"agent":{"label":"默认执行者","value":"codex","shownLabel":"Codex","options":[{"value":"codex","label":"Codex"},{"value":"agy","label":"Antigravity"},{"value":"dsh","label":"DSH"}],"disabled":false,"readOnly":false,"hints":[]},"model":{"label":"默认模型","value":"sol","shownLabel":"sol","options":[{"value":"sol","label":"sol"},{"value":"best","label":"best"},{"value":"astra","label":"astra"},{"value":"terra","label":"terra"},{"value":"luna","label":"luna"}],"disabled":false,"readOnly":false,"hints":[]},"effort":{"label":"推理档位","value":"high","shownLabel":"高","options":[{"value":"low","label":"低"},{"value":"medium","label":"中"},{"value":"high","label":"高"},{"value":"xhigh","label":"超高"}],"disabled":false,"readOnly":false,"hints":[]}},"numbers":{"idleMin":{"label":"没动静判卡","value":"10","error":null,"unit":"分钟"},"maxMin":{"label":"单轮最长","value":"45","error":null,"unit":"分钟"},"askWaitMin":{"label":"等回答","value":"10","error":null,"unit":"分钟"},"retries":{"label":"重试次数","value":"3","error":null,"unit":null}},"alerts":[],"activeField":null,"save":{"label":"保存","disabled":false,"ariaDisabled":false,"status":null},"restoreLabel":"恢复默认","scope":"只影响新派的任务；单个任务仍可用命令行参数覆盖。","path":"~/.claude/agent-foreman/config.json","missing":null},
        "beforeSave": {"bytes":"{\"version\":1,\"defaults\":{\"agent\":\"agy\",\"model\":\"hydrogen\",\"effort\":\"medium\",\"maxMin\":90}}","files":["config.json"]},
        "savedPanel": {"hasForm":true,"choices":{"agent":{"label":"默认执行者","value":"codex","shownLabel":"Codex","options":[{"value":"codex","label":"Codex"},{"value":"agy","label":"Antigravity"},{"value":"dsh","label":"DSH"}],"disabled":false,"readOnly":false,"hints":[]},"model":{"label":"默认模型","value":"sol","shownLabel":"sol","options":[{"value":"sol","label":"sol"},{"value":"best","label":"best"},{"value":"astra","label":"astra"},{"value":"terra","label":"terra"},{"value":"luna","label":"luna"}],"disabled":false,"readOnly":false,"hints":[]},"effort":{"label":"推理档位","value":"high","shownLabel":"高","options":[{"value":"low","label":"低"},{"value":"medium","label":"中"},{"value":"high","label":"高"},{"value":"xhigh","label":"超高"}],"disabled":false,"readOnly":false,"hints":[]}},"numbers":{"idleMin":{"label":"没动静判卡","value":"10","error":null,"unit":"分钟"},"maxMin":{"label":"单轮最长","value":"45","error":null,"unit":"分钟"},"askWaitMin":{"label":"等回答","value":"10","error":null,"unit":"分钟"},"retries":{"label":"重试次数","value":"3","error":null,"unit":null}},"alerts":[],"activeField":null,"save":{"label":"保存","disabled":false,"ariaDisabled":true,"status":"已保存，新派任务生效"},"restoreLabel":"恢复默认","scope":"只影响新派的任务；单个任务仍可用命令行参数覆盖。","path":"~/.claude/agent-foreman/config.json","missing":null},
        "afterSave": {"exists":true,"path":"$FIXTURE_HOME/.claude/agent-foreman/config.json","defaults":{"agent":"codex","model":"sol","effort":"high","idleMin":10,"maxMin":45,"askWaitMin":10,"retries":3}}
      }
    },
    {
      "id": "save-failure-retry",
      "source": [{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"108-144"},{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"253-260"},{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"385-390"},{"file":"apps/desktop/src/renderer/src/i18n/messages/hierarchyShell.ts","lines":"253-255"},{"file":"apps/runtime/src/rpc/runtime-rpc-router.ts","lines":"183-191"},{"file":"apps/runtime/src/rpc/dispatch-config.ts","lines":"38-55"},{"file":"packages/contracts/src/dispatch-config.ts","lines":"3-18"}],
      "boundary": "只在第一次 rename 注入故障，其他文件操作为真实离线操作。",
      "input": {
        "file": {"version":1,"defaults":{"agent":"codex","model":"sol","effort":"high","idleMin":10,"maxMin":45,"askWaitMin":10,"retries":3}},
        "steps": [
          {"op":"open"},
          {"op":"change","field":"model","value":"luna"},
          {"op":"change","field":"idleMin","value":" 20.0 "},
          {"op":"disk","as":"before"},
          {"op":"save-button","failRename":true},
          {"op":"view","as":"failedPanel"},
          {"op":"disk","as":"afterFailure"},
          {"op":"save-button"},
          {"op":"view","as":"savedPanel"},
          {"op":"disk","as":"afterRetry"}
        ]
      },
      "expected": {
        "before": {"bytes":"{\"version\":1,\"defaults\":{\"agent\":\"codex\",\"model\":\"sol\",\"effort\":\"high\",\"idleMin\":10,\"maxMin\":45,\"askWaitMin\":10,\"retries\":3}}","files":["config.json"]},
        "failedPanel": {"hasForm":true,"choices":{"agent":{"label":"默认执行者","value":"codex","shownLabel":"Codex","options":[{"value":"codex","label":"Codex"},{"value":"agy","label":"Antigravity"},{"value":"dsh","label":"DSH"}],"disabled":false,"readOnly":false,"hints":[]},"model":{"label":"默认模型","value":"luna","shownLabel":"luna","options":[{"value":"sol","label":"sol"},{"value":"best","label":"best"},{"value":"astra","label":"astra"},{"value":"terra","label":"terra"},{"value":"luna","label":"luna"}],"disabled":false,"readOnly":false,"hints":[]},"effort":{"label":"推理档位","value":"high","shownLabel":"高","options":[{"value":"low","label":"低"},{"value":"medium","label":"中"},{"value":"high","label":"高"},{"value":"xhigh","label":"超高"}],"disabled":false,"readOnly":false,"hints":[]}},"numbers":{"idleMin":{"label":"没动静判卡","value":" 20.0 ","error":null,"unit":"分钟"},"maxMin":{"label":"单轮最长","value":"45","error":null,"unit":"分钟"},"askWaitMin":{"label":"等回答","value":"10","error":null,"unit":"分钟"},"retries":{"label":"重试次数","value":"3","error":null,"unit":null}},"alerts":["保存失败，修改已保留。fixture rename failed"],"activeField":null,"save":{"label":"保存","disabled":false,"ariaDisabled":false,"status":null},"restoreLabel":"恢复默认","scope":"只影响新派的任务；单个任务仍可用命令行参数覆盖。","path":"~/.claude/agent-foreman/config.json","missing":null},
        "afterFailure": {"bytes":"{\"version\":1,\"defaults\":{\"agent\":\"codex\",\"model\":\"sol\",\"effort\":\"high\",\"idleMin\":10,\"maxMin\":45,\"askWaitMin\":10,\"retries\":3}}","files":["config.json"]},
        "savedPanel": {"hasForm":true,"choices":{"agent":{"label":"默认执行者","value":"codex","shownLabel":"Codex","options":[{"value":"codex","label":"Codex"},{"value":"agy","label":"Antigravity"},{"value":"dsh","label":"DSH"}],"disabled":false,"readOnly":false,"hints":[]},"model":{"label":"默认模型","value":"luna","shownLabel":"luna","options":[{"value":"sol","label":"sol"},{"value":"best","label":"best"},{"value":"astra","label":"astra"},{"value":"terra","label":"terra"},{"value":"luna","label":"luna"}],"disabled":false,"readOnly":false,"hints":[]},"effort":{"label":"推理档位","value":"high","shownLabel":"高","options":[{"value":"low","label":"低"},{"value":"medium","label":"中"},{"value":"high","label":"高"},{"value":"xhigh","label":"超高"}],"disabled":false,"readOnly":false,"hints":[]}},"numbers":{"idleMin":{"label":"没动静判卡","value":"20","error":null,"unit":"分钟"},"maxMin":{"label":"单轮最长","value":"45","error":null,"unit":"分钟"},"askWaitMin":{"label":"等回答","value":"10","error":null,"unit":"分钟"},"retries":{"label":"重试次数","value":"3","error":null,"unit":null}},"alerts":[],"activeField":null,"save":{"label":"保存","disabled":false,"ariaDisabled":true,"status":"已保存，新派任务生效"},"restoreLabel":"恢复默认","scope":"只影响新派的任务；单个任务仍可用命令行参数覆盖。","path":"~/.claude/agent-foreman/config.json","missing":null},
        "afterRetry": {"bytes":"{\n  \"version\": 1,\n  \"defaults\": {\n    \"agent\": \"codex\",\n    \"model\": \"luna\",\n    \"effort\": \"high\",\n    \"idleMin\": 20,\n    \"maxMin\": 45,\n    \"askWaitMin\": 10,\n    \"retries\": 3\n  }\n}\n","files":["config.json"]}
      }
    },
    {
      "id": "numbers-file-fallback",
      "source": [{"file":"apps/runtime/src/rpc/runtime-rpc-router.ts","lines":"183-191"},{"file":"apps/runtime/src/rpc/dispatch-config.ts","lines":"15-35"},{"file":"packages/contracts/src/dispatch-config.ts","lines":"3-18"}],
      "boundary": "文件逐字段校验，数值字符串不转换。",
      "input": {
        "file": {"version":1,"defaults":{"idleMin":601,"maxMin":-2,"askWaitMin":"12","retries":0}},
        "steps": [
          {"op":"snapshot","as":"read","payload":{}}
        ]
      },
      "expected": {
        "read": {"exists":true,"path":"$FIXTURE_HOME/.claude/agent-foreman/config.json","defaults":{"agent":"codex","model":"sol","effort":"high","idleMin":10,"maxMin":45,"askWaitMin":10,"retries":0}}
      }
    },
    {
      "id": "numbers-file-overflow-zero",
      "source": [{"file":"apps/runtime/src/rpc/runtime-rpc-router.ts","lines":"183-191"},{"file":"apps/runtime/src/rpc/dispatch-config.ts","lines":"15-35"},{"file":"packages/contracts/src/dispatch-config.ts","lines":"3-18"}],
      "boundary": "JSON 数字转为非有限值后拒绝；0.25 低于面板范围但符合存储范围。",
      "input": {
        "raw": "{\"version\":1,\"defaults\":{\"idleMin\":10000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000,\"maxMin\":0,\"askWaitMin\":0.25,\"retries\":1.5}}",
        "steps": [
          {"op":"snapshot","as":"read","payload":{}},
          {"op":"open"},
          {"op":"view","as":"panel"}
        ]
      },
      "expected": {
        "read": {"exists":true,"path":"$FIXTURE_HOME/.claude/agent-foreman/config.json","defaults":{"agent":"codex","model":"sol","effort":"high","idleMin":10,"maxMin":45,"askWaitMin":0.25,"retries":3}},
        "panel": {"hasForm":true,"choices":{"agent":{"label":"默认执行者","value":"codex","shownLabel":"Codex","options":[{"value":"codex","label":"Codex"},{"value":"agy","label":"Antigravity"},{"value":"dsh","label":"DSH"}],"disabled":false,"readOnly":false,"hints":[]},"model":{"label":"默认模型","value":"sol","shownLabel":"sol","options":[{"value":"sol","label":"sol"},{"value":"best","label":"best"},{"value":"astra","label":"astra"},{"value":"terra","label":"terra"},{"value":"luna","label":"luna"}],"disabled":false,"readOnly":false,"hints":[]},"effort":{"label":"推理档位","value":"high","shownLabel":"高","options":[{"value":"low","label":"低"},{"value":"medium","label":"中"},{"value":"high","label":"高"},{"value":"xhigh","label":"超高"}],"disabled":false,"readOnly":false,"hints":[]}},"numbers":{"idleMin":{"label":"没动静判卡","value":"10","error":null,"unit":"分钟"},"maxMin":{"label":"单轮最长","value":"45","error":null,"unit":"分钟"},"askWaitMin":{"label":"等回答","value":"0.25","error":null,"unit":"分钟"},"retries":{"label":"重试次数","value":"3","error":null,"unit":null}},"alerts":[],"activeField":null,"save":{"label":"保存","disabled":true,"ariaDisabled":true,"status":null},"restoreLabel":"恢复默认","scope":"只影响新派的任务；单个任务仍可用命令行参数覆盖。","path":"~/.claude/agent-foreman/config.json","missing":null}
      }
    },
    {
      "id": "numbers-rpc-reject",
      "source": [{"file":"apps/runtime/src/rpc/runtime-rpc-router.ts","lines":"183-191"},{"file":"apps/runtime/src/rpc/dispatch-config.ts","lines":"38-55"},{"file":"packages/contracts/src/dispatch-config.ts","lines":"3-18"}],
      "boundary": "同时传 idleMin=0 和 maxMin=-2 时只报 idleMin；存储校验拒绝整次写入。",
      "input": {
        "file": {"version":1,"defaults":{"agent":"codex","model":"sol","effort":"high","idleMin":10,"maxMin":45,"askWaitMin":10,"retries":3}},
        "steps": [
          {"op":"disk","as":"before"},
          {"op":"rpc-save","as":"zero","payload":{"defaults":{"agent":"codex","model":"sol","effort":"high","idleMin":0,"maxMin":-2,"askWaitMin":10,"retries":3}}},
          {"op":"rpc-save","as":"negative","payload":{"defaults":{"agent":"codex","model":"sol","effort":"high","idleMin":10,"maxMin":-2,"askWaitMin":10,"retries":3}}},
          {"op":"rpc-save","as":"nonNumber","payload":{"defaults":{"agent":"codex","model":"sol","effort":"high","idleMin":10,"maxMin":45,"askWaitMin":"12","retries":3}}},
          {"op":"rpc-save","as":"tooLarge","payload":{"defaults":{"agent":"codex","model":"sol","effort":"high","idleMin":10,"maxMin":601,"askWaitMin":10,"retries":3}}},
          {"op":"rpc-save","as":"retryOverflow","payload":{"defaults":{"agent":"codex","model":"sol","effort":"high","idleMin":10,"maxMin":45,"askWaitMin":10,"retries":6}}},
          {"op":"rpc-save","as":"retryFraction","payload":{"defaults":{"agent":"codex","model":"sol","effort":"high","idleMin":10,"maxMin":45,"askWaitMin":10,"retries":1.5}}},
          {"op":"disk","as":"after"}
        ]
      },
      "expected": {
        "before": {"bytes":"{\"version\":1,\"defaults\":{\"agent\":\"codex\",\"model\":\"sol\",\"effort\":\"high\",\"idleMin\":10,\"maxMin\":45,\"askWaitMin\":10,\"retries\":3}}","files":["config.json"]},
        "zero": {"error":{"code":"INVALID_REQUEST","message":"Invalid dispatch config field: idleMin","retryable":false}},
        "negative": {"error":{"code":"INVALID_REQUEST","message":"Invalid dispatch config field: maxMin","retryable":false}},
        "nonNumber": {"error":{"code":"INVALID_REQUEST","message":"Invalid dispatch config field: askWaitMin","retryable":false}},
        "tooLarge": {"error":{"code":"INVALID_REQUEST","message":"Invalid dispatch config field: maxMin","retryable":false}},
        "retryOverflow": {"error":{"code":"INVALID_REQUEST","message":"Invalid dispatch config field: retries","retryable":false}},
        "retryFraction": {"error":{"code":"INVALID_REQUEST","message":"Invalid dispatch config field: retries","retryable":false}},
        "after": {"bytes":"{\"version\":1,\"defaults\":{\"agent\":\"codex\",\"model\":\"sol\",\"effort\":\"high\",\"idleMin\":10,\"maxMin\":45,\"askWaitMin\":10,\"retries\":3}}","files":["config.json"]}
      }
    },
    {
      "id": "numbers-rpc-boundary",
      "source": [{"file":"apps/runtime/src/rpc/runtime-rpc-router.ts","lines":"183-191"},{"file":"apps/runtime/src/rpc/dispatch-config.ts","lines":"38-55"},{"file":"packages/contracts/src/dispatch-config.ts","lines":"3-18"},{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"30-36"},{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"96-123"},{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"288-306"},{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"315-330"},{"file":"apps/desktop/src/renderer/src/i18n/messages/hierarchyShell.ts","lines":"245-260"}],
      "boundary": "上下限与面板不同，也没有 idleMin ≤ maxMin 的约束。",
      "input": {
        "steps": [
          {"op":"rpc-save","as":"saved","payload":{"defaults":{"agent":"codex","model":"sol","effort":"high","idleMin":600,"maxMin":0.25,"askWaitMin":600,"retries":5}}},
          {"op":"snapshot","as":"reread","payload":{}},
          {"op":"open"},
          {"op":"view","as":"panel"}
        ]
      },
      "expected": {
        "saved": {"exists":true,"path":"$FIXTURE_HOME/.claude/agent-foreman/config.json","defaults":{"agent":"codex","model":"sol","effort":"high","idleMin":600,"maxMin":0.25,"askWaitMin":600,"retries":5}},
        "reread": {"exists":true,"path":"$FIXTURE_HOME/.claude/agent-foreman/config.json","defaults":{"agent":"codex","model":"sol","effort":"high","idleMin":600,"maxMin":0.25,"askWaitMin":600,"retries":5}},
        "panel": {"hasForm":true,"choices":{"agent":{"label":"默认执行者","value":"codex","shownLabel":"Codex","options":[{"value":"codex","label":"Codex"},{"value":"agy","label":"Antigravity"},{"value":"dsh","label":"DSH"}],"disabled":false,"readOnly":false,"hints":[]},"model":{"label":"默认模型","value":"sol","shownLabel":"sol","options":[{"value":"sol","label":"sol"},{"value":"best","label":"best"},{"value":"astra","label":"astra"},{"value":"terra","label":"terra"},{"value":"luna","label":"luna"}],"disabled":false,"readOnly":false,"hints":[]},"effort":{"label":"推理档位","value":"high","shownLabel":"高","options":[{"value":"low","label":"低"},{"value":"medium","label":"中"},{"value":"high","label":"高"},{"value":"xhigh","label":"超高"}],"disabled":false,"readOnly":false,"hints":[]}},"numbers":{"idleMin":{"label":"没动静判卡","value":"600","error":null,"unit":"分钟"},"maxMin":{"label":"单轮最长","value":"0.25","error":null,"unit":"分钟"},"askWaitMin":{"label":"等回答","value":"600","error":null,"unit":"分钟"},"retries":{"label":"重试次数","value":"5","error":null,"unit":null}},"alerts":[],"activeField":null,"save":{"label":"保存","disabled":true,"ariaDisabled":true,"status":null},"restoreLabel":"恢复默认","scope":"只影响新派的任务；单个任务仍可用命令行参数覆盖。","path":"~/.claude/agent-foreman/config.json","missing":null}
      }
    },
    {
      "id": "numbers-ui-ranges",
      "source": [{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"30-36"},{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"96-123"},{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"288-306"},{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"315-330"},{"file":"apps/desktop/src/renderer/src/i18n/messages/hierarchyShell.ts","lines":"245-260"}],
      "boundary": "保存会复核所有行，焦点落在按字段顺序的首错输入。",
      "input": {
        "steps": [
          {"op":"open"},
          {"op":"change","field":"idleMin","value":"61"},
          {"op":"enter","field":"idleMin"},
          {"op":"change","field":"maxMin","value":"4"},
          {"op":"blur","field":"maxMin"},
          {"op":"change","field":"askWaitMin","value":"61"},
          {"op":"blur","field":"askWaitMin"},
          {"op":"change","field":"retries","value":"6"},
          {"op":"enter","field":"retries"},
          {"op":"view","as":"committed"},
          {"op":"save-button"},
          {"op":"view","as":"afterClick"},
          {"op":"requests","as":"requests"},
          {"op":"disk","as":"stillAbsent"}
        ]
      },
      "expected": {
        "committed": {"hasForm":true,"choices":{"agent":{"label":"默认执行者","value":"codex","shownLabel":"Codex","options":[{"value":"codex","label":"Codex"},{"value":"agy","label":"Antigravity"},{"value":"dsh","label":"DSH"}],"disabled":false,"readOnly":false,"hints":[]},"model":{"label":"默认模型","value":"sol","shownLabel":"sol","options":[{"value":"sol","label":"sol"},{"value":"best","label":"best"},{"value":"astra","label":"astra"},{"value":"terra","label":"terra"},{"value":"luna","label":"luna"}],"disabled":false,"readOnly":false,"hints":[]},"effort":{"label":"推理档位","value":"high","shownLabel":"高","options":[{"value":"low","label":"低"},{"value":"medium","label":"中"},{"value":"high","label":"高"},{"value":"xhigh","label":"超高"}],"disabled":false,"readOnly":false,"hints":[]}},"numbers":{"idleMin":{"label":"没动静判卡","value":"61","error":"请输入 1–60 之间的数字。","unit":"分钟"},"maxMin":{"label":"单轮最长","value":"4","error":"请输入 5–180 之间的数字。","unit":"分钟"},"askWaitMin":{"label":"等回答","value":"61","error":"请输入 1–60 之间的数字。","unit":"分钟"},"retries":{"label":"重试次数","value":"6","error":"请输入 0–5 之间的整数。","unit":null}},"alerts":["请输入 1–60 之间的数字。","请输入 5–180 之间的数字。","请输入 1–60 之间的数字。","请输入 0–5 之间的整数。"],"activeField":null,"save":{"label":"保存","disabled":false,"ariaDisabled":true,"status":null},"restoreLabel":"恢复默认","scope":"只影响新派的任务；单个任务仍可用命令行参数覆盖。","path":"~/.claude/agent-foreman/config.json","missing":"文件尚未创建，首次保存时会自动创建。"},
        "afterClick": {"hasForm":true,"choices":{"agent":{"label":"默认执行者","value":"codex","shownLabel":"Codex","options":[{"value":"codex","label":"Codex"},{"value":"agy","label":"Antigravity"},{"value":"dsh","label":"DSH"}],"disabled":false,"readOnly":false,"hints":[]},"model":{"label":"默认模型","value":"sol","shownLabel":"sol","options":[{"value":"sol","label":"sol"},{"value":"best","label":"best"},{"value":"astra","label":"astra"},{"value":"terra","label":"terra"},{"value":"luna","label":"luna"}],"disabled":false,"readOnly":false,"hints":[]},"effort":{"label":"推理档位","value":"high","shownLabel":"高","options":[{"value":"low","label":"低"},{"value":"medium","label":"中"},{"value":"high","label":"高"},{"value":"xhigh","label":"超高"}],"disabled":false,"readOnly":false,"hints":[]}},"numbers":{"idleMin":{"label":"没动静判卡","value":"61","error":"请输入 1–60 之间的数字。","unit":"分钟"},"maxMin":{"label":"单轮最长","value":"4","error":"请输入 5–180 之间的数字。","unit":"分钟"},"askWaitMin":{"label":"等回答","value":"61","error":"请输入 1–60 之间的数字。","unit":"分钟"},"retries":{"label":"重试次数","value":"6","error":"请输入 0–5 之间的整数。","unit":null}},"alerts":["请输入 1–60 之间的数字。","请输入 5–180 之间的数字。","请输入 1–60 之间的数字。","请输入 0–5 之间的整数。"],"activeField":"dispatch-idleMin","save":{"label":"保存","disabled":false,"ariaDisabled":true,"status":null},"restoreLabel":"恢复默认","scope":"只影响新派的任务；单个任务仍可用命令行参数覆盖。","path":"~/.claude/agent-foreman/config.json","missing":"文件尚未创建，首次保存时会自动创建。"},
        "requests": [{"method":"dispatch-config.snapshot","payload":{}}],
        "stillAbsent": {"bytes":null,"files":null}
      }
    },
    {
      "id": "numbers-ui-syntax",
      "source": [{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"30-36"},{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"96-123"},{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"288-306"},{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"315-330"},{"file":"apps/desktop/src/renderer/src/i18n/messages/hierarchyShell.ts","lines":"245-260"}],
      "boundary": "负号、字母、空值先走格式校验。",
      "input": {
        "steps": [
          {"op":"open"},
          {"op":"change","field":"idleMin","value":"-1"},
          {"op":"change","field":"maxMin","value":"abc"},
          {"op":"change","field":"askWaitMin","value":""},
          {"op":"change","field":"retries","value":"-1"},
          {"op":"view","as":"uneditedErrors"},
          {"op":"save-button"},
          {"op":"view","as":"afterClick"},
          {"op":"requests","as":"requests"}
        ]
      },
      "expected": {
        "uneditedErrors": {"hasForm":true,"choices":{"agent":{"label":"默认执行者","value":"codex","shownLabel":"Codex","options":[{"value":"codex","label":"Codex"},{"value":"agy","label":"Antigravity"},{"value":"dsh","label":"DSH"}],"disabled":false,"readOnly":false,"hints":[]},"model":{"label":"默认模型","value":"sol","shownLabel":"sol","options":[{"value":"sol","label":"sol"},{"value":"best","label":"best"},{"value":"astra","label":"astra"},{"value":"terra","label":"terra"},{"value":"luna","label":"luna"}],"disabled":false,"readOnly":false,"hints":[]},"effort":{"label":"推理档位","value":"high","shownLabel":"高","options":[{"value":"low","label":"低"},{"value":"medium","label":"中"},{"value":"high","label":"高"},{"value":"xhigh","label":"超高"}],"disabled":false,"readOnly":false,"hints":[]}},"numbers":{"idleMin":{"label":"没动静判卡","value":"-1","error":null,"unit":"分钟"},"maxMin":{"label":"单轮最长","value":"abc","error":null,"unit":"分钟"},"askWaitMin":{"label":"等回答","value":"","error":null,"unit":"分钟"},"retries":{"label":"重试次数","value":"-1","error":null,"unit":null}},"alerts":[],"activeField":null,"save":{"label":"保存","disabled":false,"ariaDisabled":true,"status":null},"restoreLabel":"恢复默认","scope":"只影响新派的任务；单个任务仍可用命令行参数覆盖。","path":"~/.claude/agent-foreman/config.json","missing":"文件尚未创建，首次保存时会自动创建。"},
        "afterClick": {"hasForm":true,"choices":{"agent":{"label":"默认执行者","value":"codex","shownLabel":"Codex","options":[{"value":"codex","label":"Codex"},{"value":"agy","label":"Antigravity"},{"value":"dsh","label":"DSH"}],"disabled":false,"readOnly":false,"hints":[]},"model":{"label":"默认模型","value":"sol","shownLabel":"sol","options":[{"value":"sol","label":"sol"},{"value":"best","label":"best"},{"value":"astra","label":"astra"},{"value":"terra","label":"terra"},{"value":"luna","label":"luna"}],"disabled":false,"readOnly":false,"hints":[]},"effort":{"label":"推理档位","value":"high","shownLabel":"高","options":[{"value":"low","label":"低"},{"value":"medium","label":"中"},{"value":"high","label":"高"},{"value":"xhigh","label":"超高"}],"disabled":false,"readOnly":false,"hints":[]}},"numbers":{"idleMin":{"label":"没动静判卡","value":"-1","error":"请输入数字。","unit":"分钟"},"maxMin":{"label":"单轮最长","value":"abc","error":"请输入数字。","unit":"分钟"},"askWaitMin":{"label":"等回答","value":"","error":"请输入数字。","unit":"分钟"},"retries":{"label":"重试次数","value":"-1","error":"请输入整数。","unit":null}},"alerts":["请输入数字。","请输入数字。","请输入数字。","请输入整数。"],"activeField":"dispatch-idleMin","save":{"label":"保存","disabled":false,"ariaDisabled":true,"status":null},"restoreLabel":"恢复默认","scope":"只影响新派的任务；单个任务仍可用命令行参数覆盖。","path":"~/.claude/agent-foreman/config.json","missing":"文件尚未创建，首次保存时会自动创建。"},
        "requests": [{"method":"dispatch-config.snapshot","payload":{}}]
      }
    },
    {
      "id": "numbers-ui-zero-fraction",
      "source": [{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"30-36"},{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"96-123"},{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"288-306"},{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"315-330"},{"file":"apps/desktop/src/renderer/src/i18n/messages/hierarchyShell.ts","lines":"245-260"}],
      "boundary": "零是合法数字格式；该规则与重试可以保存为 0 并存。",
      "input": {
        "steps": [
          {"op":"open"},
          {"op":"change","field":"idleMin","value":"0"},
          {"op":"change","field":"maxMin","value":"181"},
          {"op":"change","field":"askWaitMin","value":"0"},
          {"op":"change","field":"retries","value":"1.5"},
          {"op":"save-button"},
          {"op":"view","as":"afterClick"},
          {"op":"requests","as":"requests"}
        ]
      },
      "expected": {
        "afterClick": {"hasForm":true,"choices":{"agent":{"label":"默认执行者","value":"codex","shownLabel":"Codex","options":[{"value":"codex","label":"Codex"},{"value":"agy","label":"Antigravity"},{"value":"dsh","label":"DSH"}],"disabled":false,"readOnly":false,"hints":[]},"model":{"label":"默认模型","value":"sol","shownLabel":"sol","options":[{"value":"sol","label":"sol"},{"value":"best","label":"best"},{"value":"astra","label":"astra"},{"value":"terra","label":"terra"},{"value":"luna","label":"luna"}],"disabled":false,"readOnly":false,"hints":[]},"effort":{"label":"推理档位","value":"high","shownLabel":"高","options":[{"value":"low","label":"低"},{"value":"medium","label":"中"},{"value":"high","label":"高"},{"value":"xhigh","label":"超高"}],"disabled":false,"readOnly":false,"hints":[]}},"numbers":{"idleMin":{"label":"没动静判卡","value":"0","error":"请输入 1–60 之间的数字。","unit":"分钟"},"maxMin":{"label":"单轮最长","value":"181","error":"请输入 5–180 之间的数字。","unit":"分钟"},"askWaitMin":{"label":"等回答","value":"0","error":"请输入 1–60 之间的数字。","unit":"分钟"},"retries":{"label":"重试次数","value":"1.5","error":"请输入整数。","unit":null}},"alerts":["请输入 1–60 之间的数字。","请输入 5–180 之间的数字。","请输入 1–60 之间的数字。","请输入整数。"],"activeField":"dispatch-idleMin","save":{"label":"保存","disabled":false,"ariaDisabled":true,"status":null},"restoreLabel":"恢复默认","scope":"只影响新派的任务；单个任务仍可用命令行参数覆盖。","path":"~/.claude/agent-foreman/config.json","missing":"文件尚未创建，首次保存时会自动创建。"},
        "requests": [{"method":"dispatch-config.snapshot","payload":{}}]
      }
    },
    {
      "id": "numbers-ui-long-input",
      "source": [{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"30-36"},{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"96-123"},{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"288-306"},{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"315-330"},{"file":"apps/desktop/src/renderer/src/i18n/messages/hierarchyShell.ts","lines":"245-260"}],
      "boundary": "没有文本长度上限；数字转换溢出与前导零行为不同。",
      "input": {
        "steps": [
          {"op":"open"},
          {"op":"change","field":"idleMin","value":"10000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"},
          {"op":"change","field":"maxMin","value":"00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000005"},
          {"op":"change","field":"retries","value":"10000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"},
          {"op":"save-button"},
          {"op":"view","as":"afterClick"},
          {"op":"requests","as":"requests"}
        ]
      },
      "expected": {
        "afterClick": {"hasForm":true,"choices":{"agent":{"label":"默认执行者","value":"codex","shownLabel":"Codex","options":[{"value":"codex","label":"Codex"},{"value":"agy","label":"Antigravity"},{"value":"dsh","label":"DSH"}],"disabled":false,"readOnly":false,"hints":[]},"model":{"label":"默认模型","value":"sol","shownLabel":"sol","options":[{"value":"sol","label":"sol"},{"value":"best","label":"best"},{"value":"astra","label":"astra"},{"value":"terra","label":"terra"},{"value":"luna","label":"luna"}],"disabled":false,"readOnly":false,"hints":[]},"effort":{"label":"推理档位","value":"high","shownLabel":"高","options":[{"value":"low","label":"低"},{"value":"medium","label":"中"},{"value":"high","label":"高"},{"value":"xhigh","label":"超高"}],"disabled":false,"readOnly":false,"hints":[]}},"numbers":{"idleMin":{"label":"没动静判卡","value":"10000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000","error":"请输入 1–60 之间的数字。","unit":"分钟"},"maxMin":{"label":"单轮最长","value":"00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000005","error":null,"unit":"分钟"},"askWaitMin":{"label":"等回答","value":"10","error":null,"unit":"分钟"},"retries":{"label":"重试次数","value":"10000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000","error":"请输入整数。","unit":null}},"alerts":["请输入 1–60 之间的数字。","请输入整数。"],"activeField":"dispatch-idleMin","save":{"label":"保存","disabled":false,"ariaDisabled":true,"status":null},"restoreLabel":"恢复默认","scope":"只影响新派的任务；单个任务仍可用命令行参数覆盖。","path":"~/.claude/agent-foreman/config.json","missing":"文件尚未创建，首次保存时会自动创建。"},
        "requests": [{"method":"dispatch-config.snapshot","payload":{}}]
      }
    },
    {
      "id": "numbers-ui-boundary",
      "source": [{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"30-36"},{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"96-123"},{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"288-306"},{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"315-330"},{"file":"apps/desktop/src/renderer/src/i18n/messages/hierarchyShell.ts","lines":"245-260"},{"file":"apps/runtime/src/rpc/runtime-rpc-router.ts","lines":"183-191"},{"file":"apps/runtime/src/rpc/dispatch-config.ts","lines":"38-55"},{"file":"packages/contracts/src/dispatch-config.ts","lines":"3-18"}],
      "boundary": "四字段两端均包含；第一组 idleMin 大于 maxMin 仍可保存。",
      "input": {
        "steps": [
          {"op":"open"},
          {"op":"change","field":"idleMin","value":"60"},
          {"op":"change","field":"maxMin","value":"5"},
          {"op":"change","field":"askWaitMin","value":"1"},
          {"op":"change","field":"retries","value":"0"},
          {"op":"save-button"},
          {"op":"snapshot","as":"first","payload":{}},
          {"op":"change","field":"idleMin","value":"1"},
          {"op":"change","field":"maxMin","value":"180"},
          {"op":"change","field":"askWaitMin","value":"60"},
          {"op":"change","field":"retries","value":"05.0"},
          {"op":"save-button"},
          {"op":"snapshot","as":"second","payload":{}},
          {"op":"view","as":"panel"},
          {"op":"requests","as":"requests"}
        ]
      },
      "expected": {
        "first": {"exists":true,"path":"$FIXTURE_HOME/.claude/agent-foreman/config.json","defaults":{"agent":"codex","model":"sol","effort":"high","idleMin":60,"maxMin":5,"askWaitMin":1,"retries":0}},
        "second": {"exists":true,"path":"$FIXTURE_HOME/.claude/agent-foreman/config.json","defaults":{"agent":"codex","model":"sol","effort":"high","idleMin":1,"maxMin":180,"askWaitMin":60,"retries":5}},
        "panel": {"hasForm":true,"choices":{"agent":{"label":"默认执行者","value":"codex","shownLabel":"Codex","options":[{"value":"codex","label":"Codex"},{"value":"agy","label":"Antigravity"},{"value":"dsh","label":"DSH"}],"disabled":false,"readOnly":false,"hints":[]},"model":{"label":"默认模型","value":"sol","shownLabel":"sol","options":[{"value":"sol","label":"sol"},{"value":"best","label":"best"},{"value":"astra","label":"astra"},{"value":"terra","label":"terra"},{"value":"luna","label":"luna"}],"disabled":false,"readOnly":false,"hints":[]},"effort":{"label":"推理档位","value":"high","shownLabel":"高","options":[{"value":"low","label":"低"},{"value":"medium","label":"中"},{"value":"high","label":"高"},{"value":"xhigh","label":"超高"}],"disabled":false,"readOnly":false,"hints":[]}},"numbers":{"idleMin":{"label":"没动静判卡","value":"1","error":null,"unit":"分钟"},"maxMin":{"label":"单轮最长","value":"180","error":null,"unit":"分钟"},"askWaitMin":{"label":"等回答","value":"60","error":null,"unit":"分钟"},"retries":{"label":"重试次数","value":"5","error":null,"unit":null}},"alerts":[],"activeField":null,"save":{"label":"保存","disabled":false,"ariaDisabled":true,"status":"已保存，新派任务生效"},"restoreLabel":"恢复默认","scope":"只影响新派的任务；单个任务仍可用命令行参数覆盖。","path":"~/.claude/agent-foreman/config.json","missing":null},
        "requests": [{"method":"dispatch-config.snapshot","payload":{}},{"method":"dispatch-config.save","payload":{"defaults":{"agent":"codex","model":"sol","effort":"high","idleMin":60,"maxMin":5,"askWaitMin":1,"retries":0}}},{"method":"dispatch-config.save","payload":{"defaults":{"agent":"codex","model":"sol","effort":"high","idleMin":1,"maxMin":180,"askWaitMin":60,"retries":5}}}]
      }
    },
    {
      "id": "numbers-ui-saved-bypass",
      "source": [{"file":"apps/runtime/src/rpc/runtime-rpc-router.ts","lines":"183-191"},{"file":"apps/runtime/src/rpc/dispatch-config.ts","lines":"15-35"},{"file":"packages/contracts/src/dispatch-config.ts","lines":"3-18"},{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"30-36"},{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"96-123"},{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"288-306"},{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"315-330"},{"file":"apps/desktop/src/renderer/src/i18n/messages/hierarchyShell.ts","lines":"245-260"},{"file":"apps/runtime/src/rpc/runtime-rpc-router.ts","lines":"183-191"},{"file":"apps/runtime/src/rpc/dispatch-config.ts","lines":"38-55"},{"file":"packages/contracts/src/dispatch-config.ts","lines":"3-18"}],
      "boundary": "原文等值检查早于格式检查，数值等值检查早于范围检查。",
      "input": {
        "file": {"version":1,"defaults":{"idleMin":1e-7,"maxMin":200,"askWaitMin":600}},
        "steps": [
          {"op":"open"},
          {"op":"view","as":"original"},
          {"op":"change","field":"maxMin","value":" 0200.0 "},
          {"op":"blur","field":"maxMin"},
          {"op":"change","field":"askWaitMin","value":"0600"},
          {"op":"enter","field":"askWaitMin"},
          {"op":"view","as":"equivalent"},
          {"op":"change","field":"retries","value":"2"},
          {"op":"save-button"},
          {"op":"snapshot","as":"saved","payload":{}},
          {"op":"requests","as":"requests"}
        ]
      },
      "expected": {
        "original": {"hasForm":true,"choices":{"agent":{"label":"默认执行者","value":"codex","shownLabel":"Codex","options":[{"value":"codex","label":"Codex"},{"value":"agy","label":"Antigravity"},{"value":"dsh","label":"DSH"}],"disabled":false,"readOnly":false,"hints":[]},"model":{"label":"默认模型","value":"sol","shownLabel":"sol","options":[{"value":"sol","label":"sol"},{"value":"best","label":"best"},{"value":"astra","label":"astra"},{"value":"terra","label":"terra"},{"value":"luna","label":"luna"}],"disabled":false,"readOnly":false,"hints":[]},"effort":{"label":"推理档位","value":"high","shownLabel":"高","options":[{"value":"low","label":"低"},{"value":"medium","label":"中"},{"value":"high","label":"高"},{"value":"xhigh","label":"超高"}],"disabled":false,"readOnly":false,"hints":[]}},"numbers":{"idleMin":{"label":"没动静判卡","value":"1e-7","error":null,"unit":"分钟"},"maxMin":{"label":"单轮最长","value":"200","error":null,"unit":"分钟"},"askWaitMin":{"label":"等回答","value":"600","error":null,"unit":"分钟"},"retries":{"label":"重试次数","value":"3","error":null,"unit":null}},"alerts":[],"activeField":null,"save":{"label":"保存","disabled":true,"ariaDisabled":true,"status":null},"restoreLabel":"恢复默认","scope":"只影响新派的任务；单个任务仍可用命令行参数覆盖。","path":"~/.claude/agent-foreman/config.json","missing":null},
        "equivalent": {"hasForm":true,"choices":{"agent":{"label":"默认执行者","value":"codex","shownLabel":"Codex","options":[{"value":"codex","label":"Codex"},{"value":"agy","label":"Antigravity"},{"value":"dsh","label":"DSH"}],"disabled":false,"readOnly":false,"hints":[]},"model":{"label":"默认模型","value":"sol","shownLabel":"sol","options":[{"value":"sol","label":"sol"},{"value":"best","label":"best"},{"value":"astra","label":"astra"},{"value":"terra","label":"terra"},{"value":"luna","label":"luna"}],"disabled":false,"readOnly":false,"hints":[]},"effort":{"label":"推理档位","value":"high","shownLabel":"高","options":[{"value":"low","label":"低"},{"value":"medium","label":"中"},{"value":"high","label":"高"},{"value":"xhigh","label":"超高"}],"disabled":false,"readOnly":false,"hints":[]}},"numbers":{"idleMin":{"label":"没动静判卡","value":"1e-7","error":null,"unit":"分钟"},"maxMin":{"label":"单轮最长","value":" 0200.0 ","error":null,"unit":"分钟"},"askWaitMin":{"label":"等回答","value":"0600","error":null,"unit":"分钟"},"retries":{"label":"重试次数","value":"3","error":null,"unit":null}},"alerts":[],"activeField":null,"save":{"label":"保存","disabled":true,"ariaDisabled":true,"status":null},"restoreLabel":"恢复默认","scope":"只影响新派的任务；单个任务仍可用命令行参数覆盖。","path":"~/.claude/agent-foreman/config.json","missing":null},
        "saved": {"exists":true,"path":"$FIXTURE_HOME/.claude/agent-foreman/config.json","defaults":{"agent":"codex","model":"sol","effort":"high","idleMin":1e-7,"maxMin":200,"askWaitMin":600,"retries":2}},
        "requests": [{"method":"dispatch-config.snapshot","payload":{}},{"method":"dispatch-config.save","payload":{"defaults":{"agent":"codex","model":"sol","effort":"high","idleMin":1e-7,"maxMin":200,"askWaitMin":600,"retries":2}}}]
      }
    },
    {
      "id": "choices-unlisted-model",
      "source": [{"file":"apps/runtime/src/rpc/runtime-rpc-router.ts","lines":"183-191"},{"file":"apps/runtime/src/rpc/dispatch-config.ts","lines":"15-35"},{"file":"packages/contracts/src/dispatch-config.ts","lines":"3-18"},{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"13-19"},{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"187-248"},{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"332-375"},{"file":"apps/desktop/src/renderer/src/i18n/messages/hierarchyShell.ts","lines":"229-244"},{"file":"apps/runtime/src/rpc/runtime-rpc-router.ts","lines":"183-191"},{"file":"apps/runtime/src/rpc/dispatch-config.ts","lines":"38-55"},{"file":"packages/contracts/src/dispatch-config.ts","lines":"3-18"}],
      "boundary": "合法自定义模型保留存储原值。",
      "input": {
        "file": {"version":1,"defaults":{"model":"custom/model"}},
        "steps": [
          {"op":"open"},
          {"op":"view","as":"before"},
          {"op":"change","field":"retries","value":"2"},
          {"op":"save-button"},
          {"op":"snapshot","as":"reread","payload":{}},
          {"op":"requests","as":"requests"}
        ]
      },
      "expected": {
        "before": {"hasForm":true,"choices":{"agent":{"label":"默认执行者","value":"codex","shownLabel":"Codex","options":[{"value":"codex","label":"Codex"},{"value":"agy","label":"Antigravity"},{"value":"dsh","label":"DSH"}],"disabled":false,"readOnly":false,"hints":[]},"model":{"label":"默认模型","value":"sol","shownLabel":"sol","options":[{"value":"sol","label":"sol"},{"value":"best","label":"best"},{"value":"astra","label":"astra"},{"value":"terra","label":"terra"},{"value":"luna","label":"luna"}],"disabled":false,"readOnly":false,"hints":["当前模型不在该执行者的预设列表中，这里显示默认项。直接保存会保留原模型值。"]},"effort":{"label":"推理档位","value":"high","shownLabel":"高","options":[{"value":"low","label":"低"},{"value":"medium","label":"中"},{"value":"high","label":"高"},{"value":"xhigh","label":"超高"}],"disabled":false,"readOnly":false,"hints":[]}},"numbers":{"idleMin":{"label":"没动静判卡","value":"10","error":null,"unit":"分钟"},"maxMin":{"label":"单轮最长","value":"45","error":null,"unit":"分钟"},"askWaitMin":{"label":"等回答","value":"10","error":null,"unit":"分钟"},"retries":{"label":"重试次数","value":"3","error":null,"unit":null}},"alerts":[],"activeField":null,"save":{"label":"保存","disabled":true,"ariaDisabled":true,"status":null},"restoreLabel":"恢复默认","scope":"只影响新派的任务；单个任务仍可用命令行参数覆盖。","path":"~/.claude/agent-foreman/config.json","missing":null},
        "reread": {"exists":true,"path":"$FIXTURE_HOME/.claude/agent-foreman/config.json","defaults":{"agent":"codex","model":"custom/model","effort":"high","idleMin":10,"maxMin":45,"askWaitMin":10,"retries":2}},
        "requests": [{"method":"dispatch-config.snapshot","payload":{}},{"method":"dispatch-config.save","payload":{"defaults":{"agent":"codex","model":"custom/model","effort":"high","idleMin":10,"maxMin":45,"askWaitMin":10,"retries":2}}}]
      }
    },
    {
      "id": "choices-unlisted-executor-effort",
      "source": [{"file":"apps/runtime/src/rpc/runtime-rpc-router.ts","lines":"183-191"},{"file":"apps/runtime/src/rpc/dispatch-config.ts","lines":"15-35"},{"file":"packages/contracts/src/dispatch-config.ts","lines":"3-18"},{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"13-19"},{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"187-248"},{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"332-375"},{"file":"apps/desktop/src/renderer/src/i18n/messages/hierarchyShell.ts","lines":"229-244"},{"file":"apps/runtime/src/rpc/runtime-rpc-router.ts","lines":"183-191"},{"file":"apps/runtime/src/rpc/dispatch-config.ts","lines":"38-55"},{"file":"packages/contracts/src/dispatch-config.ts","lines":"3-18"}],
      "boundary": "执行者、档位是全局枚举，自定义模型按格式校验。",
      "input": {
        "file": {"version":1,"defaults":{"agent":"future-executor","effort":"ultra","model":"custom/model"}},
        "steps": [
          {"op":"snapshot","as":"read","payload":{}},
          {"op":"open"},
          {"op":"view","as":"panel"},
          {"op":"rpc-save","as":"unknownExecutor","payload":{"defaults":{"agent":"future-executor","model":"sol","effort":"ultra","idleMin":10,"maxMin":45,"askWaitMin":10,"retries":3}}},
          {"op":"rpc-save","as":"unknownEffort","payload":{"defaults":{"agent":"codex","model":"sol","effort":"ultra","idleMin":10,"maxMin":45,"askWaitMin":10,"retries":3}}},
          {"op":"disk","as":"unchanged"}
        ]
      },
      "expected": {
        "read": {"exists":true,"path":"$FIXTURE_HOME/.claude/agent-foreman/config.json","defaults":{"agent":"codex","model":"custom/model","effort":"high","idleMin":10,"maxMin":45,"askWaitMin":10,"retries":3}},
        "panel": {"hasForm":true,"choices":{"agent":{"label":"默认执行者","value":"codex","shownLabel":"Codex","options":[{"value":"codex","label":"Codex"},{"value":"agy","label":"Antigravity"},{"value":"dsh","label":"DSH"}],"disabled":false,"readOnly":false,"hints":[]},"model":{"label":"默认模型","value":"sol","shownLabel":"sol","options":[{"value":"sol","label":"sol"},{"value":"best","label":"best"},{"value":"astra","label":"astra"},{"value":"terra","label":"terra"},{"value":"luna","label":"luna"}],"disabled":false,"readOnly":false,"hints":["当前模型不在该执行者的预设列表中，这里显示默认项。直接保存会保留原模型值。"]},"effort":{"label":"推理档位","value":"high","shownLabel":"高","options":[{"value":"low","label":"低"},{"value":"medium","label":"中"},{"value":"high","label":"高"},{"value":"xhigh","label":"超高"}],"disabled":false,"readOnly":false,"hints":[]}},"numbers":{"idleMin":{"label":"没动静判卡","value":"10","error":null,"unit":"分钟"},"maxMin":{"label":"单轮最长","value":"45","error":null,"unit":"分钟"},"askWaitMin":{"label":"等回答","value":"10","error":null,"unit":"分钟"},"retries":{"label":"重试次数","value":"3","error":null,"unit":null}},"alerts":[],"activeField":null,"save":{"label":"保存","disabled":true,"ariaDisabled":true,"status":null},"restoreLabel":"恢复默认","scope":"只影响新派的任务；单个任务仍可用命令行参数覆盖。","path":"~/.claude/agent-foreman/config.json","missing":null},
        "unknownExecutor": {"error":{"code":"INVALID_REQUEST","message":"Invalid dispatch config field: agent","retryable":false}},
        "unknownEffort": {"error":{"code":"INVALID_REQUEST","message":"Invalid dispatch config field: effort","retryable":false}},
        "unchanged": {"bytes":"{\"version\":1,\"defaults\":{\"agent\":\"future-executor\",\"effort\":\"ultra\",\"model\":\"custom/model\"}}","files":["config.json"]}
      }
    },
    {
      "id": "choices-invalid-wire-snapshot",
      "source": [{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"74-84"},{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"197-202"},{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"332-342"},{"file":"apps/desktop/src/renderer/src/i18n/messages/hierarchyShell.ts","lines":"225-228"}],
      "boundary": "仅测试面板防线；真实文件回读会先回落。",
      "input": {
        "steps": [
          {"op":"open","snapshotOverride":{"agent":"future-executor"}},
          {"op":"view","as":"unknownExecutor"},
          {"op":"close"},
          {"op":"open","snapshotOverride":{"effort":"ultra"}},
          {"op":"view","as":"unknownEffort"}
        ]
      },
      "expected": {
        "unknownExecutor": {"hasForm":false,"loadError":"无法读取派活默认值，请重试。","buttonLabels":["重试"]},
        "unknownEffort": {"hasForm":false,"loadError":"无法读取派活默认值，请重试。","buttonLabels":["重试"]}
      }
    },
    {
      "id": "choices-dsh-clamp",
      "source": [{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"13-19"},{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"187-248"},{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"332-375"},{"file":"apps/desktop/src/renderer/src/i18n/messages/hierarchyShell.ts","lines":"229-244"},{"file":"apps/runtime/src/rpc/runtime-rpc-router.ts","lines":"183-191"},{"file":"apps/runtime/src/rpc/dispatch-config.ts","lines":"38-55"},{"file":"packages/contracts/src/dispatch-config.ts","lines":"3-18"}],
      "boundary": "执行者的档位列表与全局允许保存的枚举不同。",
      "input": {
        "file": {"version":1,"defaults":{"agent":"dsh","model":"custom/model","effort":"xhigh"}},
        "steps": [
          {"op":"open"},
          {"op":"view","as":"before"},
          {"op":"change","field":"retries","value":"2"},
          {"op":"save-button"},
          {"op":"snapshot","as":"afterUnrelatedSave","payload":{}},
          {"op":"change","field":"effort","value":"low"},
          {"op":"save-button"},
          {"op":"snapshot","as":"afterExplicitChoice","payload":{}},
          {"op":"view","as":"panel"},
          {"op":"requests","as":"requests"}
        ]
      },
      "expected": {
        "before": {"hasForm":true,"choices":{"agent":{"label":"默认执行者","value":"dsh","shownLabel":"DSH","options":[{"value":"codex","label":"Codex"},{"value":"agy","label":"Antigravity"},{"value":"dsh","label":"DSH"}],"disabled":false,"readOnly":false,"hints":[]},"model":{"label":"默认模型","value":"deepseek-flash","shownLabel":"deepseek-flash","options":[],"disabled":false,"readOnly":true,"hints":["DSH 型号固定，配置文件里的 model 对它不生效"]},"effort":{"label":"推理档位","value":"high","shownLabel":"高","options":[{"value":"low","label":"低"},{"value":"high","label":"高"}],"disabled":false,"readOnly":false,"hints":["DSH 只支持低/高","当前档位为「超高」，DSH 按「高」显示。直接保存会保留原档位。"]}},"numbers":{"idleMin":{"label":"没动静判卡","value":"10","error":null,"unit":"分钟"},"maxMin":{"label":"单轮最长","value":"45","error":null,"unit":"分钟"},"askWaitMin":{"label":"等回答","value":"10","error":null,"unit":"分钟"},"retries":{"label":"重试次数","value":"3","error":null,"unit":null}},"alerts":[],"activeField":null,"save":{"label":"保存","disabled":true,"ariaDisabled":true,"status":null},"restoreLabel":"恢复默认","scope":"只影响新派的任务；单个任务仍可用命令行参数覆盖。","path":"~/.claude/agent-foreman/config.json","missing":null},
        "afterUnrelatedSave": {"exists":true,"path":"$FIXTURE_HOME/.claude/agent-foreman/config.json","defaults":{"agent":"dsh","model":"custom/model","effort":"xhigh","idleMin":10,"maxMin":45,"askWaitMin":10,"retries":2}},
        "afterExplicitChoice": {"exists":true,"path":"$FIXTURE_HOME/.claude/agent-foreman/config.json","defaults":{"agent":"dsh","model":"custom/model","effort":"low","idleMin":10,"maxMin":45,"askWaitMin":10,"retries":2}},
        "panel": {"hasForm":true,"choices":{"agent":{"label":"默认执行者","value":"dsh","shownLabel":"DSH","options":[{"value":"codex","label":"Codex"},{"value":"agy","label":"Antigravity"},{"value":"dsh","label":"DSH"}],"disabled":false,"readOnly":false,"hints":[]},"model":{"label":"默认模型","value":"deepseek-flash","shownLabel":"deepseek-flash","options":[],"disabled":false,"readOnly":true,"hints":["DSH 型号固定，配置文件里的 model 对它不生效"]},"effort":{"label":"推理档位","value":"low","shownLabel":"低","options":[{"value":"low","label":"低"},{"value":"high","label":"高"}],"disabled":false,"readOnly":false,"hints":["DSH 只支持低/高"]}},"numbers":{"idleMin":{"label":"没动静判卡","value":"10","error":null,"unit":"分钟"},"maxMin":{"label":"单轮最长","value":"45","error":null,"unit":"分钟"},"askWaitMin":{"label":"等回答","value":"10","error":null,"unit":"分钟"},"retries":{"label":"重试次数","value":"2","error":null,"unit":null}},"alerts":[],"activeField":null,"save":{"label":"保存","disabled":false,"ariaDisabled":true,"status":"已保存，新派任务生效"},"restoreLabel":"恢复默认","scope":"只影响新派的任务；单个任务仍可用命令行参数覆盖。","path":"~/.claude/agent-foreman/config.json","missing":null},
        "requests": [{"method":"dispatch-config.snapshot","payload":{}},{"method":"dispatch-config.save","payload":{"defaults":{"agent":"dsh","model":"custom/model","effort":"xhigh","idleMin":10,"maxMin":45,"askWaitMin":10,"retries":2}}},{"method":"dispatch-config.save","payload":{"defaults":{"agent":"dsh","model":"custom/model","effort":"low","idleMin":10,"maxMin":45,"askWaitMin":10,"retries":2}}}]
      }
    },
    {
      "id": "choices-orchestrator-switch",
      "source": [{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"152-162"},{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"13-19"},{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"187-248"},{"file":"apps/desktop/src/renderer/src/hierarchy/DispatchSettings.tsx","lines":"332-375"},{"file":"apps/desktop/src/renderer/src/i18n/messages/hierarchyShell.ts","lines":"229-244"},{"file":"apps/runtime/src/rpc/runtime-rpc-router.ts","lines":"183-191"},{"file":"apps/runtime/src/rpc/dispatch-config.ts","lines":"38-55"},{"file":"packages/contracts/src/dispatch-config.ts","lines":"3-18"}],
      "boundary": "切执行者取上次保存模型，未保存的 luna 不继承。",
      "input": {
        "file": {"version":1,"defaults":{"model":"astra","effort":"medium"}},
        "steps": [
          {"op":"open"},
          {"op":"change","field":"model","value":"luna"},
          {"op":"change","field":"agent","value":"agy"},
          {"op":"view","as":"switched"},
          {"op":"save-button"},
          {"op":"snapshot","as":"reread","payload":{}},
          {"op":"requests","as":"requests"}
        ]
      },
      "expected": {
        "switched": {"hasForm":true,"choices":{"agent":{"label":"默认执行者","value":"agy","shownLabel":"Antigravity","options":[{"value":"codex","label":"Codex"},{"value":"agy","label":"Antigravity"},{"value":"dsh","label":"DSH"}],"disabled":false,"readOnly":false,"hints":[]},"model":{"label":"默认模型","value":"gemini-pro-orchestrator","shownLabel":"Orchestrator（固定）","options":[{"value":"gemini-pro-orchestrator","label":"Orchestrator（固定）"},{"value":"hydrogen","label":"hydrogen"},{"value":"argon","label":"argon"},{"value":"carbon","label":"carbon"}],"disabled":false,"readOnly":false,"hints":["当前模型不在该执行者的预设列表中，这里显示默认项。直接保存会保留原模型值。"]},"effort":{"label":"推理档位","value":"medium","shownLabel":"中","options":[{"value":"low","label":"低"},{"value":"medium","label":"中"},{"value":"high","label":"高"},{"value":"xhigh","label":"超高"}],"disabled":true,"readOnly":false,"hints":["Orchestrator 没有独立的推理档位，沿用它的内置档位。"]}},"numbers":{"idleMin":{"label":"没动静判卡","value":"10","error":null,"unit":"分钟"},"maxMin":{"label":"单轮最长","value":"45","error":null,"unit":"分钟"},"askWaitMin":{"label":"等回答","value":"10","error":null,"unit":"分钟"},"retries":{"label":"重试次数","value":"3","error":null,"unit":null}},"alerts":[],"activeField":null,"save":{"label":"保存","disabled":false,"ariaDisabled":false,"status":null},"restoreLabel":"恢复默认","scope":"只影响新派的任务；单个任务仍可用命令行参数覆盖。","path":"~/.claude/agent-foreman/config.json","missing":null},
        "reread": {"exists":true,"path":"$FIXTURE_HOME/.claude/agent-foreman/config.json","defaults":{"agent":"agy","model":"astra","effort":"medium","idleMin":10,"maxMin":45,"askWaitMin":10,"retries":3}},
        "requests": [{"method":"dispatch-config.snapshot","payload":{}},{"method":"dispatch-config.save","payload":{"defaults":{"agent":"agy","model":"astra","effort":"medium","idleMin":10,"maxMin":45,"askWaitMin":10,"retries":3}}}]
      }
    },
    {
      "id": "choices-model-format",
      "source": [{"file":"apps/runtime/src/rpc/runtime-rpc-router.ts","lines":"183-191"},{"file":"apps/runtime/src/rpc/dispatch-config.ts","lines":"15-35"},{"file":"packages/contracts/src/dispatch-config.ts","lines":"3-18"},{"file":"apps/runtime/src/rpc/runtime-rpc-router.ts","lines":"183-191"},{"file":"apps/runtime/src/rpc/dispatch-config.ts","lines":"38-55"},{"file":"packages/contracts/src/dispatch-config.ts","lines":"3-18"}],
      "boundary": "模型清单成员资格不是存储约束，长度和空白是。",
      "input": {
        "steps": [
          {"op":"rpc-save","as":"maxLength","payload":{"defaults":{"agent":"codex","model":"mmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmm","effort":"high","idleMin":10,"maxMin":45,"askWaitMin":10,"retries":3}}},
          {"op":"rpc-save","as":"tooLong","payload":{"defaults":{"agent":"codex","model":"mmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmm","effort":"high","idleMin":10,"maxMin":45,"askWaitMin":10,"retries":3}}},
          {"op":"rpc-save","as":"whitespace","payload":{"defaults":{"agent":"codex","model":"custom model","effort":"high","idleMin":10,"maxMin":45,"askWaitMin":10,"retries":3}}},
          {"op":"snapshot","as":"reread","payload":{}}
        ]
      },
      "expected": {
        "maxLength": {"exists":true,"path":"$FIXTURE_HOME/.claude/agent-foreman/config.json","defaults":{"agent":"codex","model":"mmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmm","effort":"high","idleMin":10,"maxMin":45,"askWaitMin":10,"retries":3}},
        "tooLong": {"error":{"code":"INVALID_REQUEST","message":"Invalid dispatch config field: model","retryable":false}},
        "whitespace": {"error":{"code":"INVALID_REQUEST","message":"Invalid dispatch config field: model","retryable":false}},
        "reread": {"exists":true,"path":"$FIXTURE_HOME/.claude/agent-foreman/config.json","defaults":{"agent":"codex","model":"mmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmm","effort":"high","idleMin":10,"maxMin":45,"askWaitMin":10,"retries":3}}
      }
    }
  ]
}
"####;

#[derive(Deserialize)]
struct Fixture {
    version: u32,
    reference: String,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    id: String,
    source: Vec<Source>,
    boundary: String,
    input: Input,
    expected: Map<String, Value>,
}

#[derive(Deserialize)]
struct Source {
    file: String,
    lines: String,
}

#[derive(Deserialize)]
struct Input {
    file: Option<Value>,
    raw: Option<String>,
    steps: Vec<Step>,
}

#[derive(Deserialize)]
struct Step {
    op: String,
    #[serde(rename = "as")]
    observation: Option<String>,
}

impl Fixture {
    fn load() -> Self {
        serde_json::from_str(FIXTURE_JSON).expect("valid frozen dispatch fixture")
    }

    fn observations(&self, id: &str) -> &Map<String, Value> {
        &self
            .cases
            .iter()
            .find(|case| case.id == id)
            .expect(id)
            .expected
    }
}

#[test]
fn fixture_loads_fixed_inputs_and_every_named_observation() {
    let fixture = Fixture::load();
    assert_eq!(fixture.version, 1);
    assert_eq!(
        fixture.reference,
        "924de28da27999543aeef7a7c9797334fca45d97"
    );
    assert_eq!(fixture.cases.len(), 26);

    let mut ids = BTreeSet::new();
    for case in fixture.cases {
        assert!(ids.insert(case.id.clone()), "duplicate case {}", case.id);
        assert!(!case.boundary.is_empty(), "{}", case.id);
        assert!(!case.source.is_empty(), "{}", case.id);
        for source in case.source {
            assert!(source.file.starts_with("apps/") || source.file.starts_with("packages/"));
            let (start, end) = source
                .lines
                .split_once('-')
                .expect("inclusive source range");
            let start: usize = start.parse().expect("source line number");
            let end: usize = end.parse().expect("source line number");
            assert!(start > 0 && end >= start, "{}", case.id);
        }
        assert!(case.input.file.is_none() || case.input.raw.is_none());
        let mut names = Vec::new();
        for step in &case.input.steps {
            let produces_output = matches!(
                step.op.as_str(),
                "snapshot" | "rpc-save" | "disk" | "fresh-process" | "view" | "requests"
            );
            assert_eq!(step.observation.is_some(), produces_output, "{}", case.id);
            names.extend(step.observation.as_deref());
        }
        assert!(!names.is_empty(), "{}", case.id);
        let recorded: Vec<_> = case.expected.keys().map(String::as_str).collect();
        assert_eq!(names, recorded, "{}", case.id);
    }
}

#[test]
fn workspace_inputs_and_process_restart_have_the_frozen_readback() {
    let fixture = Fixture::load();
    let workspace = fixture.observations("workspace-global");
    assert_eq!(workspace["alpha"], workspace["beta"]);
    assert_eq!(workspace["alpha"]["defaults"]["model"], "astra");
    assert_eq!(workspace["alpha"]["defaults"]["idleMin"], 14);
    assert_eq!(
        workspace["panelRequests"],
        json!([{ "method": "dispatch-config.snapshot", "payload": {} }])
    );

    let first = fixture.observations("default-first-read");
    assert_eq!(
        first["first"],
        json!({
            "exists": false, "path": "$FIXTURE_HOME/.claude/agent-foreman/config.json",
            "defaults": {
                "agent": "codex", "model": "sol", "effort": "high",
                "idleMin": 10, "maxMin": 45, "askWaitMin": 10, "retries": 3
            }
        })
    );
    assert_eq!(
        first["stillAbsent"],
        json!({ "bytes": null, "files": null })
    );

    let saved = fixture.observations("save-reload-restart");
    assert_eq!(saved["reread"], saved["freshProcess"]);
    assert_eq!(saved["reread"]["defaults"]["retries"], 0);
    assert_eq!(saved["savedPanel"]["numbers"]["idleMin"]["value"], "1.5");
    assert_eq!(
        saved["savedPanel"]["save"]["status"],
        "已保存，新派任务生效"
    );
    assert_eq!(saved["remountedPanel"]["save"]["status"], Value::Null);
}

#[test]
fn rejected_numbers_and_write_failures_keep_the_original_configuration() {
    let fixture = Fixture::load();
    let rejected = fixture.observations("numbers-rpc-reject");
    assert_eq!(rejected["before"], rejected["after"]);
    assert_eq!(
        rejected["zero"]["error"],
        json!({
            "code": "INVALID_REQUEST", "message": "Invalid dispatch config field: idleMin",
            "retryable": false
        })
    );

    let long = fixture.observations("numbers-ui-long-input");
    assert_eq!(long["afterClick"]["activeField"], "dispatch-idleMin");
    assert_eq!(
        long["afterClick"]["alerts"],
        json!(["请输入 1–60 之间的数字。", "请输入整数。"])
    );
    assert_eq!(
        long["afterClick"]["numbers"]["maxMin"]["error"],
        Value::Null
    );
    assert_eq!(long["requests"].as_array().unwrap().len(), 1);

    let failed = fixture.observations("save-failure-retry");
    assert_eq!(failed["before"], failed["afterFailure"]);
    assert_eq!(
        failed["failedPanel"]["alerts"],
        json!(["保存失败，修改已保留。fixture rename failed"])
    );
}

#[test]
fn unlisted_values_distinguish_fallback_display_from_saved_values() {
    let fixture = Fixture::load();
    let model = fixture.observations("choices-unlisted-model");
    assert_eq!(model["before"]["choices"]["model"]["value"], "sol");
    assert_eq!(model["reread"]["defaults"]["model"], "custom/model");

    let choices = fixture.observations("choices-unlisted-executor-effort");
    assert_eq!(choices["read"]["defaults"]["agent"], "codex");
    assert_eq!(choices["read"]["defaults"]["effort"], "high");
    for (observation, field) in [("unknownExecutor", "agent"), ("unknownEffort", "effort")] {
        assert_eq!(
            choices[observation]["error"]["message"],
            format!("Invalid dispatch config field: {field}")
        );
    }

    let dsh = fixture.observations("choices-dsh-clamp");
    assert_eq!(dsh["before"]["choices"]["model"]["readOnly"], true);
    assert_eq!(dsh["before"]["choices"]["effort"]["value"], "high");
    assert_eq!(dsh["afterUnrelatedSave"]["defaults"]["effort"], "xhigh");
    assert_eq!(dsh["afterExplicitChoice"]["defaults"]["effort"], "low");
}
