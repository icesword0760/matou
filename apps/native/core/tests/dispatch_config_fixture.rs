use std::{collections::BTreeSet, fs, path::Path, process::Command};

use matou_native_core::dispatch_config::{
    DispatchConfig, DispatchConfigError, DispatchConfigSnapshot, DispatchDefaults,
    form::{
        ChoiceField, DispatchForm, MISSING_NOTE, NumberField, RETRY_LABEL, SAVED_MESSAGE,
        SCOPE_NOTE,
    },
};
use serde::Deserialize;
use serde_json::{Map, Value, json};

// Offline capture from the frozen renderer and RPC. The gate runner checks that
// these bytes match a fresh replay. The core test needs neither Node nor the
// local evidence directory. Replay below drives the native implementation.
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
    #[serde(default)]
    payload: Value,
    field: Option<String>,
    value: Option<String>,
    #[serde(default, rename = "failRename")]
    fail_rename: bool,
    #[serde(default, rename = "snapshotOverride")]
    snapshot_override: Map<String, Value>,
}

impl Fixture {
    fn load() -> Self {
        serde_json::from_str(FIXTURE_JSON).expect("valid frozen dispatch fixture")
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

macro_rules! replay_cases {
    ($($test:ident),+ $(,)?) => {
        $(
            #[test]
            fn $test() {
                replay(&stringify!($test).replace('_', "-")).unwrap();
            }
        )+
    }
}

replay_cases!(
    workspace_global,
    workspace_without_global,
    workspace_save_is_global,
    default_first_read,
    version_defaults_missing,
    unsupported_version,
    malformed_file,
    save_reload_restart,
    cancel_and_restore,
    save_failure_retry,
    numbers_file_fallback,
    numbers_file_overflow_zero,
    numbers_rpc_reject,
    numbers_rpc_boundary,
    numbers_ui_ranges,
    numbers_ui_syntax,
    numbers_ui_zero_fraction,
    numbers_ui_long_input,
    numbers_ui_boundary,
    numbers_ui_saved_bypass,
    choices_unlisted_model,
    choices_unlisted_executor_effort,
    choices_invalid_wire_snapshot,
    choices_dsh_clamp,
    choices_orchestrator_switch,
    choices_model_format,
);

fn replay(id: &str) -> anyhow::Result<()> {
    let case = Fixture::load()
        .cases
        .into_iter()
        .find(|case| case.id == id)
        .expect(id);
    let home = tempfile::tempdir_in(std::env::current_dir()?)?;
    let config = DispatchConfig::for_home(home.path());
    let path = config.path();
    assert_eq!(path, home.path().join(".claude/agent-foreman/config.json"));
    let initial = case
        .input
        .raw
        .or_else(|| case.input.file.map(|value| value.to_string()));
    if let Some(initial) = initial {
        fs::create_dir_all(path.parent().unwrap())?;
        fs::write(path, initial)?;
    }
    let mut form = None;
    let mut first_error = None;
    let mut requests = Vec::new();
    let mut seen = BTreeSet::new();
    for step in case.input.steps {
        let actual = match step.op.as_str() {
            // The native entry point has no workspace input.
            "snapshot" => Some(snapshot_json(config.snapshot()?, path)),
            "fresh-process" => Some(fresh_process(home.path())?),
            "rpc-save" => Some(match config.save(&step.payload["defaults"]) {
                Ok(snapshot) => snapshot_json(snapshot, path),
                Err(error @ DispatchConfigError::InvalidField(_)) => json!({
                    "error": {"code": "INVALID_REQUEST", "message": error.to_string(), "retryable": false}
                }),
                Err(error) => return Err(error.into()),
            }),
            "disk" => Some(disk(path)?),
            "requests" => Some(json!(requests)),
            "open" => {
                requests.push(json!({"method": "dispatch-config.snapshot", "payload": {}}));
                let mut snapshot = config.snapshot()?;
                for (key, value) in step.snapshot_override {
                    let value = value.as_str().unwrap().into();
                    match key.as_str() {
                        "agent" => snapshot.defaults.agent = value,
                        "effort" => snapshot.defaults.effort = value,
                        _ => panic!("unsupported snapshot override {key}"),
                    }
                }
                form = Some(DispatchForm::new(snapshot));
                first_error = None;
                None
            }
            "close" => {
                form = None;
                None
            }
            "view" => Some(form_json(form.as_ref().unwrap(), first_error)),
            "change" => {
                let form = form.as_mut().unwrap().as_mut().unwrap();
                let key = step.field.as_deref().unwrap();
                let value = step.value.unwrap();
                if let Some(field) = ChoiceField::ALL
                    .into_iter()
                    .find(|field| field.key() == key)
                {
                    form.choose(field, &value);
                } else {
                    form.edit_number(number_field(key), value);
                }
                None
            }
            "enter" | "blur" => {
                form.as_mut()
                    .unwrap()
                    .as_mut()
                    .unwrap()
                    .commit_number(number_field(step.field.as_deref().unwrap()));
                None
            }
            "restore-button" => {
                form.as_mut().unwrap().as_mut().unwrap().restore_defaults();
                None
            }
            "save-button" => {
                first_error = form.as_mut().unwrap().as_mut().unwrap().save(|defaults| {
                    requests.push(json!({
                        "method": "dispatch-config.save", "payload": {"defaults": &defaults}
                    }));
                    if step.fail_rename {
                        assert_rename_fails(&config, defaults);
                        Err("fixture rename failed".into())
                    } else {
                        config
                            .save_defaults(defaults)
                            .map_err(|error| error.to_string())
                    }
                });
                None
            }
            op => panic!("unknown fixture step {op}"),
        };
        if let Some(name) = step.observation {
            assert_eq!(actual.as_ref(), case.expected.get(&name), "{id}/{name}");
            assert!(seen.insert(name), "{id}: duplicate observation");
        } else {
            assert!(
                actual.is_none(),
                "{id}/{} had an unexpected observation",
                step.op
            );
        }
    }
    assert_eq!(
        seen.len(),
        case.expected.len(),
        "{id}: missing observations"
    );
    Ok(())
}

fn number_field(key: &str) -> NumberField {
    NumberField::ALL
        .into_iter()
        .find(|field| field.key() == key)
        .unwrap_or_else(|| panic!("unknown number field {key}"))
}

fn snapshot_json(snapshot: DispatchConfigSnapshot, path: &Path) -> Value {
    assert_eq!(snapshot.path, path);
    json!({
        "exists": snapshot.exists,
        "path": "$FIXTURE_HOME/.claude/agent-foreman/config.json",
        "defaults": snapshot.defaults,
    })
}

fn disk(path: &Path) -> anyhow::Result<Value> {
    let file = match fs::read_to_string(path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let names = match fs::read_dir(path.parent().unwrap()) {
        Ok(entries) => {
            let mut names = entries
                .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
                .collect::<std::io::Result<Vec<_>>>()?;
            names.sort();
            Some(names)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    Ok(json!({"bytes": file, "files": names}))
}

fn form_json(form: &Result<DispatchForm, &'static str>, active: Option<NumberField>) -> Value {
    let form = match form {
        Ok(form) => form,
        Err(error) => {
            return json!({"hasForm": false, "loadError": error, "buttonLabels": [RETRY_LABEL]});
        }
    };
    let choices: Map<_, _> = ChoiceField::ALL
        .into_iter()
        .map(|field| {
            let choice = form.choice(field);
            let options: Vec<_> = choice
                .options
                .iter()
                .map(|option| json!({"value": option.value, "label": option.label}))
                .collect();
            (
                field.key().into(),
                json!({
                    "label": field.label(), "value": choice.value, "shownLabel": choice.shown_label,
                    "options": options, "disabled": choice.disabled, "readOnly": choice.read_only,
                    "hints": choice.hints,
                }),
            )
        })
        .collect();
    let numbers: Map<_, _> = NumberField::ALL.into_iter().map(|field| {
        (field.key().into(), json!({
            "label": field.label(), "value": form.number(field), "error": form.number_error(field),
            "unit": (!field.unit().is_empty()).then_some(field.unit()),
        }))
    }).collect();
    let alerts: Vec<_> = NumberField::ALL
        .into_iter()
        .filter_map(|field| form.number_error(field))
        .chain(form.save_error())
        .collect();
    json!({
        "hasForm": true, "choices": choices, "numbers": numbers, "alerts": alerts,
        "activeField": active.map(|field| format!("dispatch-{}", field.key())),
        "save": {
            "label": "保存", "disabled": !form.is_dirty() && !form.saved(),
            "ariaDisabled": !form.can_save(), "status": form.saved().then_some(SAVED_MESSAGE),
        },
        "restoreLabel": "恢复默认", "scope": SCOPE_NOTE, "path": form.config_path(),
        "missing": (!form.exists()).then_some(MISSING_NOTE),
    })
}

fn fresh_process(home: &Path) -> anyhow::Result<Value> {
    let child = Command::new(std::env::current_exe()?)
        .args([
            "--exact",
            "dispatch_fixture_child",
            "--ignored",
            "--nocapture",
        ])
        .env("MATOU_DISPATCH_FIXTURE_HOME", home)
        .output()?;
    let stdout = String::from_utf8(child.stdout)?;
    assert!(
        child.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&child.stderr)
    );
    let snapshot = stdout
        .lines()
        .find_map(|line| line.strip_prefix("DISPATCH_FIXTURE_SNAPSHOT "))
        .expect("child snapshot on stdout");
    Ok(serde_json::from_str(snapshot)?)
}

#[test]
#[ignore = "invoked in a new process by save_reload_restart"]
fn dispatch_fixture_child() -> anyhow::Result<()> {
    let home = std::env::var_os("MATOU_DISPATCH_FIXTURE_HOME").expect("isolated fixture home");
    let config = DispatchConfig::for_home(home);
    println!(
        "DISPATCH_FIXTURE_SNAPSHOT {}",
        snapshot_json(config.snapshot()?, config.path())
    );
    Ok(())
}

#[cfg(target_os = "macos")]
fn assert_rename_fails(config: &DispatchConfig, defaults: DispatchDefaults) {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};

    let path = CString::new(config.path().as_os_str().as_bytes()).unwrap();
    let before = disk(config.path()).unwrap();
    // SAFETY: path is NUL-terminated and names this case's own temporary file.
    assert_eq!(
        unsafe { libc::chflags(path.as_ptr(), libc::UF_IMMUTABLE) },
        0
    );
    let result = config.save_defaults(defaults);
    // SAFETY: clear the flag on the same file before any assertions or cleanup.
    assert_eq!(unsafe { libc::chflags(path.as_ptr(), 0) }, 0);
    assert!(matches!(result, Err(DispatchConfigError::Io(_))));
    assert_eq!(disk(config.path()).unwrap(), before);
}

#[cfg(not(target_os = "macos"))]
fn assert_rename_fails(config: &DispatchConfig, defaults: DispatchDefaults) {
    let path = config.path();
    let before = disk(path).unwrap();
    // A regular file cannot be renamed over a directory. Keep a directory entry
    // for the original while exercising the real native writer.
    let original = path.with_extension("original");
    fs::rename(path, &original).unwrap();
    fs::create_dir(path).unwrap();
    let result = config.save_defaults(defaults);
    fs::remove_dir(path).unwrap();
    fs::rename(original, path).unwrap();
    assert!(matches!(result, Err(DispatchConfigError::Io(_))));
    assert_eq!(disk(path).unwrap(), before);
}
