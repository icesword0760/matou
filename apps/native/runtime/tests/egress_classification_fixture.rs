use std::{
    collections::{BTreeMap, BTreeSet},
    convert::Infallible,
    future::{Future, ready},
};

use matou_native_runtime::egress_probe::{
    Check, CheckId, DEFAULT_TIMEOUT_MS, EgressClock, EgressCopy, EgressMonitor, EgressRule,
    EgressSample, EgressSampler, Exit, FROZEN_RECHECK_MS, Family, HostError, HostPair, HostSample,
    InterfaceLocale, LocalePreference, OverrideDecision, OverridePrompt, RECOVERY_CONFIRM_MS,
    SLOW_THRESHOLD_MS, SamplingPlan, SessionEffect, Status, SubscriptionRoute, TimeZonePreference,
    TraceEvent, TraceProbe, TraceRequestError, evaluate_egress, launch_plan,
    resolve_interface_locale,
};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};

// Shared offline contract for the later native egress gates. The Rust tests can
// read these cases without a local evidence directory or a TypeScript toolchain.
const FIXTURE_JSON: &str = r#"{
  "source": {"commit": "924de28da27999543aeef7a7c9797334fca45d97", "inventory": ["NET-06", "NET-07", "NET-08", "REC-09"]},
  "constants": {"epochMs": 1700000000000, "slowThresholdMs": 5000, "defaultTimeoutMs": 10000, "frozenRecheckMs": 15000, "confirmDelayMs": 5000, "baselineExit": {"ip": "192.0.2.10", "country": "US"}},
  "cases": [
    {
      "id": "fast-below-5000",
      "description": "5 秒下方，成功",
      "source": [
        "apps/runtime/src/egress/trace-probe.ts:7-8 default timeout / slow threshold",
        "apps/runtime/src/egress/trace-probe.ts:53-81 probeTrace",
        "apps/runtime/src/egress/trace-probe.ts:104-197 defaultTraceRequester / classifyRequestError",
        "apps/runtime/src/egress/egress-evaluator.ts:80-117 evaluateFamily",
        "apps/runtime/src/egress/egress-evaluator.ts:132-174 unknownCheck / slowFacts / evaluateMainland",
        "apps/desktop/src/renderer/src/egress/egress-copy.ts:149-203 checkCopy"
      ],
      "input": {"operation": "trace", "eventAtMs": 4999, "event": "response", "body": "ip=192.0.2.10\nloc=US"},
      "expected": {"classification": "fast", "requestTimeoutMs": 10000, "settledAtMs": 4999, "sample": {"host": "api.anthropic.com", "ip": "192.0.2.10", "country": "US"}, "familyState": "ok", "state": "ok", "checks": [{"id": "mainland", "result": "pass"}, {"id": "exit-rule", "result": "pass"}, {"id": "login-api-match", "result": "pass"}], "detail": ["美国 · 192.0.2.10", "United States · 192.0.2.10"]}
    },
    {
      "id": "fast-at-5000",
      "description": "恰好 5 秒，严格大于才标慢",
      "source": [
        "apps/runtime/src/egress/trace-probe.ts:7-8 default timeout / slow threshold",
        "apps/runtime/src/egress/trace-probe.ts:53-81 probeTrace",
        "apps/runtime/src/egress/trace-probe.ts:104-197 defaultTraceRequester / classifyRequestError",
        "apps/runtime/src/egress/egress-evaluator.ts:80-117 evaluateFamily",
        "apps/runtime/src/egress/egress-evaluator.ts:132-174 unknownCheck / slowFacts / evaluateMainland",
        "apps/desktop/src/renderer/src/egress/egress-copy.ts:149-203 checkCopy"
      ],
      "input": {"operation": "trace", "eventAtMs": 5000, "event": "response", "body": "ip=192.0.2.10\nloc=US"},
      "expected": {"classification": "fast", "requestTimeoutMs": 10000, "settledAtMs": 5000, "sample": {"host": "api.anthropic.com", "ip": "192.0.2.10", "country": "US"}, "familyState": "ok", "state": "ok", "checks": [{"id": "mainland", "result": "pass"}, {"id": "exit-rule", "result": "pass"}, {"id": "login-api-match", "result": "pass"}], "detail": ["美国 · 192.0.2.10", "United States · 192.0.2.10"]}
    },
    {
      "id": "slow-above-5000",
      "description": "5 秒上方，慢提示和出口同时保留",
      "source": [
        "apps/runtime/src/egress/trace-probe.ts:7-8 default timeout / slow threshold",
        "apps/runtime/src/egress/trace-probe.ts:53-81 probeTrace",
        "apps/runtime/src/egress/trace-probe.ts:104-197 defaultTraceRequester / classifyRequestError",
        "apps/runtime/src/egress/egress-evaluator.ts:80-117 evaluateFamily",
        "apps/runtime/src/egress/egress-evaluator.ts:132-174 unknownCheck / slowFacts / evaluateMainland",
        "apps/desktop/src/renderer/src/egress/egress-copy.ts:149-203 checkCopy"
      ],
      "input": {"operation": "trace", "eventAtMs": 5001, "event": "response", "body": "ip=192.0.2.10\nloc=US"},
      "expected": {"classification": "slow", "requestTimeoutMs": 10000, "settledAtMs": 5001, "sample": {"host": "api.anthropic.com", "ip": "192.0.2.10", "country": "US", "slow": true}, "familyState": "ok", "state": "ok", "checks": [{"id": "mainland", "result": "pass", "facts": {"slowHost": "api.anthropic.com"}}, {"id": "exit-rule", "result": "pass", "facts": {"slowHost": "api.anthropic.com"}}, {"id": "login-api-match", "result": "pass", "facts": {"slowHost": "api.anthropic.com"}}], "detail": ["美国 · 192.0.2.10 · api.anthropic.com 响应慢（>5 秒）", "United States · 192.0.2.10 · api.anthropic.com is slow (>5s)"]}
    },
    {
      "id": "slow-before-10000",
      "description": "截止前 1 ms 收到完整响应",
      "source": [
        "apps/runtime/src/egress/trace-probe.ts:7-8 default timeout / slow threshold",
        "apps/runtime/src/egress/trace-probe.ts:53-81 probeTrace",
        "apps/runtime/src/egress/trace-probe.ts:104-197 defaultTraceRequester / classifyRequestError",
        "apps/runtime/src/egress/egress-evaluator.ts:80-117 evaluateFamily",
        "apps/runtime/src/egress/egress-evaluator.ts:132-174 unknownCheck / slowFacts / evaluateMainland",
        "apps/desktop/src/renderer/src/egress/egress-copy.ts:149-203 checkCopy"
      ],
      "input": {"operation": "trace", "eventAtMs": 9999, "event": "response", "body": "ip=192.0.2.10\nloc=US"},
      "expected": {"classification": "slow", "requestTimeoutMs": 10000, "settledAtMs": 9999, "sample": {"host": "api.anthropic.com", "ip": "192.0.2.10", "country": "US", "slow": true}, "familyState": "ok", "state": "ok", "checks": [{"id": "mainland", "result": "pass", "facts": {"slowHost": "api.anthropic.com"}}, {"id": "exit-rule", "result": "pass", "facts": {"slowHost": "api.anthropic.com"}}, {"id": "login-api-match", "result": "pass", "facts": {"slowHost": "api.anthropic.com"}}], "detail": ["美国 · 192.0.2.10 · api.anthropic.com 响应慢（>5 秒）", "United States · 192.0.2.10 · api.anthropic.com is slow (>5s)"]}
    },
    {
      "id": "slow-at-10000-response-first",
      "description": "同刻响应先完成并清除超时计时器",
      "source": [
        "apps/runtime/src/egress/trace-probe.ts:7-8 default timeout / slow threshold",
        "apps/runtime/src/egress/trace-probe.ts:53-81 probeTrace",
        "apps/runtime/src/egress/trace-probe.ts:104-197 defaultTraceRequester / classifyRequestError",
        "apps/runtime/src/egress/egress-evaluator.ts:80-117 evaluateFamily",
        "apps/runtime/src/egress/egress-evaluator.ts:132-174 unknownCheck / slowFacts / evaluateMainland",
        "apps/desktop/src/renderer/src/egress/egress-copy.ts:149-203 checkCopy"
      ],
      "input": {"operation": "trace", "eventAtMs": 10000, "event": "response", "body": "ip=192.0.2.10\nloc=US", "tie": "response-first"},
      "expected": {"classification": "slow", "requestTimeoutMs": 10000, "settledAtMs": 10000, "sample": {"host": "api.anthropic.com", "ip": "192.0.2.10", "country": "US", "slow": true}, "familyState": "ok", "state": "ok", "checks": [{"id": "mainland", "result": "pass", "facts": {"slowHost": "api.anthropic.com"}}, {"id": "exit-rule", "result": "pass", "facts": {"slowHost": "api.anthropic.com"}}, {"id": "login-api-match", "result": "pass", "facts": {"slowHost": "api.anthropic.com"}}], "detail": ["美国 · 192.0.2.10 · api.anthropic.com 响应慢（>5 秒）", "United States · 192.0.2.10 · api.anthropic.com is slow (>5s)"]}
    },
    {
      "id": "timeout-at-10000-timer-first",
      "description": "同刻超时先结算",
      "source": [
        "apps/runtime/src/egress/trace-probe.ts:7-8 default timeout / slow threshold",
        "apps/runtime/src/egress/trace-probe.ts:53-81 probeTrace",
        "apps/runtime/src/egress/trace-probe.ts:104-197 defaultTraceRequester / classifyRequestError",
        "apps/runtime/src/egress/egress-evaluator.ts:80-117 evaluateFamily",
        "apps/runtime/src/egress/egress-evaluator.ts:132-174 unknownCheck / slowFacts / evaluateMainland",
        "apps/desktop/src/renderer/src/egress/egress-copy.ts:149-203 checkCopy"
      ],
      "input": {"operation": "trace", "eventAtMs": 10000, "event": "response", "body": "ip=192.0.2.10\nloc=US", "tie": "timer-first"},
      "expected": {"classification": "timeout", "requestTimeoutMs": 10000, "settledAtMs": 10000, "sample": {"host": "api.anthropic.com", "error": "timeout"}, "familyState": "unknown", "state": "warning", "checks": [{"id": "mainland", "result": "unknown", "facts": {"error": "timeout", "host": "api.anthropic.com"}}, {"id": "exit-rule", "result": "unknown", "facts": {"error": "timeout", "host": "api.anthropic.com"}}, {"id": "login-api-match", "result": "unknown", "facts": {"error": "timeout", "host": "api.anthropic.com"}}], "detail": ["无法确认（api.anthropic.com 检测超时）", "Couldn't confirm (api.anthropic.com timed out)"]}
    },
    {
      "id": "timeout-response-after-10000",
      "description": "晚到 1 ms，先在 10 秒结算",
      "source": [
        "apps/runtime/src/egress/trace-probe.ts:7-8 default timeout / slow threshold",
        "apps/runtime/src/egress/trace-probe.ts:53-81 probeTrace",
        "apps/runtime/src/egress/trace-probe.ts:104-197 defaultTraceRequester / classifyRequestError",
        "apps/runtime/src/egress/egress-evaluator.ts:80-117 evaluateFamily",
        "apps/runtime/src/egress/egress-evaluator.ts:132-174 unknownCheck / slowFacts / evaluateMainland",
        "apps/desktop/src/renderer/src/egress/egress-copy.ts:149-203 checkCopy"
      ],
      "input": {"operation": "trace", "eventAtMs": 10001, "event": "response", "body": "ip=192.0.2.10\nloc=US"},
      "expected": {"classification": "timeout", "requestTimeoutMs": 10000, "settledAtMs": 10000, "sample": {"host": "api.anthropic.com", "error": "timeout"}, "familyState": "unknown", "state": "warning", "checks": [{"id": "mainland", "result": "unknown", "facts": {"error": "timeout", "host": "api.anthropic.com"}}, {"id": "exit-rule", "result": "unknown", "facts": {"error": "timeout", "host": "api.anthropic.com"}}, {"id": "login-api-match", "result": "unknown", "facts": {"error": "timeout", "host": "api.anthropic.com"}}], "detail": ["无法确认（api.anthropic.com 检测超时）", "Couldn't confirm (api.anthropic.com timed out)"]}
    },
    {
      "id": "unreachable-below-5000",
      "description": "连接拒绝发生在慢阈值以下",
      "source": [
        "apps/runtime/src/egress/trace-probe.ts:7-8 default timeout / slow threshold",
        "apps/runtime/src/egress/trace-probe.ts:53-81 probeTrace",
        "apps/runtime/src/egress/trace-probe.ts:104-197 defaultTraceRequester / classifyRequestError",
        "apps/runtime/src/egress/egress-evaluator.ts:80-117 evaluateFamily",
        "apps/runtime/src/egress/egress-evaluator.ts:132-174 unknownCheck / slowFacts / evaluateMainland",
        "apps/desktop/src/renderer/src/egress/egress-copy.ts:149-203 checkCopy"
      ],
      "input": {"operation": "trace", "eventAtMs": 4999, "event": "error", "code": "ECONNREFUSED"},
      "expected": {"classification": "unreachable", "requestTimeoutMs": 10000, "settledAtMs": 4999, "sample": {"host": "api.anthropic.com", "error": "unreachable"}, "familyState": "unknown", "state": "warning", "checks": [{"id": "mainland", "result": "unknown", "facts": {"error": "unreachable", "host": "api.anthropic.com"}}, {"id": "exit-rule", "result": "unknown", "facts": {"error": "unreachable", "host": "api.anthropic.com"}}, {"id": "login-api-match", "result": "unknown", "facts": {"error": "unreachable", "host": "api.anthropic.com"}}], "detail": ["无法确认（api.anthropic.com 连不上）", "Couldn't confirm (api.anthropic.com unreachable)"]}
    },
    {
      "id": "unreachable-at-5000",
      "description": "连接拒绝恰好 5 秒",
      "source": [
        "apps/runtime/src/egress/trace-probe.ts:7-8 default timeout / slow threshold",
        "apps/runtime/src/egress/trace-probe.ts:53-81 probeTrace",
        "apps/runtime/src/egress/trace-probe.ts:104-197 defaultTraceRequester / classifyRequestError",
        "apps/runtime/src/egress/egress-evaluator.ts:80-117 evaluateFamily",
        "apps/runtime/src/egress/egress-evaluator.ts:132-174 unknownCheck / slowFacts / evaluateMainland",
        "apps/desktop/src/renderer/src/egress/egress-copy.ts:149-203 checkCopy"
      ],
      "input": {"operation": "trace", "eventAtMs": 5000, "event": "error", "code": "ECONNREFUSED"},
      "expected": {"classification": "unreachable", "requestTimeoutMs": 10000, "settledAtMs": 5000, "sample": {"host": "api.anthropic.com", "error": "unreachable"}, "familyState": "unknown", "state": "warning", "checks": [{"id": "mainland", "result": "unknown", "facts": {"error": "unreachable", "host": "api.anthropic.com"}}, {"id": "exit-rule", "result": "unknown", "facts": {"error": "unreachable", "host": "api.anthropic.com"}}, {"id": "login-api-match", "result": "unknown", "facts": {"error": "unreachable", "host": "api.anthropic.com"}}], "detail": ["无法确认（api.anthropic.com 连不上）", "Couldn't confirm (api.anthropic.com unreachable)"]}
    },
    {
      "id": "unreachable-above-5000",
      "description": "超过慢阈值的连接拒绝仍为不可达",
      "source": [
        "apps/runtime/src/egress/trace-probe.ts:7-8 default timeout / slow threshold",
        "apps/runtime/src/egress/trace-probe.ts:53-81 probeTrace",
        "apps/runtime/src/egress/trace-probe.ts:104-197 defaultTraceRequester / classifyRequestError",
        "apps/runtime/src/egress/egress-evaluator.ts:80-117 evaluateFamily",
        "apps/runtime/src/egress/egress-evaluator.ts:132-174 unknownCheck / slowFacts / evaluateMainland",
        "apps/desktop/src/renderer/src/egress/egress-copy.ts:149-203 checkCopy"
      ],
      "input": {"operation": "trace", "eventAtMs": 5001, "event": "error", "code": "ECONNREFUSED"},
      "expected": {"classification": "unreachable", "requestTimeoutMs": 10000, "settledAtMs": 5001, "sample": {"host": "api.anthropic.com", "error": "unreachable"}, "familyState": "unknown", "state": "warning", "checks": [{"id": "mainland", "result": "unknown", "facts": {"error": "unreachable", "host": "api.anthropic.com"}}, {"id": "exit-rule", "result": "unknown", "facts": {"error": "unreachable", "host": "api.anthropic.com"}}, {"id": "login-api-match", "result": "unknown", "facts": {"error": "unreachable", "host": "api.anthropic.com"}}], "detail": ["无法确认（api.anthropic.com 连不上）", "Couldn't confirm (api.anthropic.com unreachable)"]}
    },
    {
      "id": "unreachable-before-10000",
      "description": "10 秒前连接失败",
      "source": [
        "apps/runtime/src/egress/trace-probe.ts:7-8 default timeout / slow threshold",
        "apps/runtime/src/egress/trace-probe.ts:53-81 probeTrace",
        "apps/runtime/src/egress/trace-probe.ts:104-197 defaultTraceRequester / classifyRequestError",
        "apps/runtime/src/egress/egress-evaluator.ts:80-117 evaluateFamily",
        "apps/runtime/src/egress/egress-evaluator.ts:132-174 unknownCheck / slowFacts / evaluateMainland",
        "apps/desktop/src/renderer/src/egress/egress-copy.ts:149-203 checkCopy"
      ],
      "input": {"operation": "trace", "eventAtMs": 9999, "event": "error", "code": "ENETUNREACH"},
      "expected": {"classification": "unreachable", "requestTimeoutMs": 10000, "settledAtMs": 9999, "sample": {"host": "api.anthropic.com", "error": "unreachable"}, "familyState": "unknown", "state": "warning", "checks": [{"id": "mainland", "result": "unknown", "facts": {"error": "unreachable", "host": "api.anthropic.com"}}, {"id": "exit-rule", "result": "unknown", "facts": {"error": "unreachable", "host": "api.anthropic.com"}}, {"id": "login-api-match", "result": "unknown", "facts": {"error": "unreachable", "host": "api.anthropic.com"}}], "detail": ["无法确认（api.anthropic.com 连不上）", "Couldn't confirm (api.anthropic.com unreachable)"]}
    },
    {
      "id": "unreachable-at-10000-error-first",
      "description": "同刻普通连接错误先结算",
      "source": [
        "apps/runtime/src/egress/trace-probe.ts:7-8 default timeout / slow threshold",
        "apps/runtime/src/egress/trace-probe.ts:53-81 probeTrace",
        "apps/runtime/src/egress/trace-probe.ts:104-197 defaultTraceRequester / classifyRequestError",
        "apps/runtime/src/egress/egress-evaluator.ts:80-117 evaluateFamily",
        "apps/runtime/src/egress/egress-evaluator.ts:132-174 unknownCheck / slowFacts / evaluateMainland",
        "apps/desktop/src/renderer/src/egress/egress-copy.ts:149-203 checkCopy"
      ],
      "input": {"operation": "trace", "eventAtMs": 10000, "event": "error", "code": "ENETUNREACH"},
      "expected": {"classification": "unreachable", "requestTimeoutMs": 10000, "settledAtMs": 10000, "sample": {"host": "api.anthropic.com", "error": "unreachable"}, "familyState": "unknown", "state": "warning", "checks": [{"id": "mainland", "result": "unknown", "facts": {"error": "unreachable", "host": "api.anthropic.com"}}, {"id": "exit-rule", "result": "unknown", "facts": {"error": "unreachable", "host": "api.anthropic.com"}}, {"id": "login-api-match", "result": "unknown", "facts": {"error": "unreachable", "host": "api.anthropic.com"}}], "detail": ["无法确认（api.anthropic.com 连不上）", "Couldn't confirm (api.anthropic.com unreachable)"]}
    },
    {
      "id": "timeout-after-10000-late-error",
      "description": "晚到的连接失败无法覆盖超时",
      "source": [
        "apps/runtime/src/egress/trace-probe.ts:7-8 default timeout / slow threshold",
        "apps/runtime/src/egress/trace-probe.ts:53-81 probeTrace",
        "apps/runtime/src/egress/trace-probe.ts:104-197 defaultTraceRequester / classifyRequestError",
        "apps/runtime/src/egress/egress-evaluator.ts:80-117 evaluateFamily",
        "apps/runtime/src/egress/egress-evaluator.ts:132-174 unknownCheck / slowFacts / evaluateMainland",
        "apps/desktop/src/renderer/src/egress/egress-copy.ts:149-203 checkCopy"
      ],
      "input": {"operation": "trace", "eventAtMs": 10001, "event": "error", "code": "ENETUNREACH"},
      "expected": {"classification": "timeout", "requestTimeoutMs": 10000, "settledAtMs": 10000, "sample": {"host": "api.anthropic.com", "error": "timeout"}, "familyState": "unknown", "state": "warning", "checks": [{"id": "mainland", "result": "unknown", "facts": {"error": "timeout", "host": "api.anthropic.com"}}, {"id": "exit-rule", "result": "unknown", "facts": {"error": "timeout", "host": "api.anthropic.com"}}, {"id": "login-api-match", "result": "unknown", "facts": {"error": "timeout", "host": "api.anthropic.com"}}], "detail": ["无法确认（api.anthropic.com 检测超时）", "Couldn't confirm (api.anthropic.com timed out)"]}
    },
    {
      "id": "timeout-before-deadline-socket",
      "description": "请求本身先发出 timeout 事件",
      "source": [
        "apps/runtime/src/egress/trace-probe.ts:7-8 default timeout / slow threshold",
        "apps/runtime/src/egress/trace-probe.ts:53-81 probeTrace",
        "apps/runtime/src/egress/trace-probe.ts:104-197 defaultTraceRequester / classifyRequestError",
        "apps/runtime/src/egress/egress-evaluator.ts:80-117 evaluateFamily",
        "apps/runtime/src/egress/egress-evaluator.ts:132-174 unknownCheck / slowFacts / evaluateMainland",
        "apps/desktop/src/renderer/src/egress/egress-copy.ts:149-203 checkCopy"
      ],
      "input": {"operation": "trace", "eventAtMs": 9999, "event": "socket-timeout"},
      "expected": {"classification": "timeout", "requestTimeoutMs": 10000, "settledAtMs": 9999, "sample": {"host": "api.anthropic.com", "error": "timeout"}, "familyState": "unknown", "state": "warning", "checks": [{"id": "mainland", "result": "unknown", "facts": {"error": "timeout", "host": "api.anthropic.com"}}, {"id": "exit-rule", "result": "unknown", "facts": {"error": "timeout", "host": "api.anthropic.com"}}, {"id": "login-api-match", "result": "unknown", "facts": {"error": "timeout", "host": "api.anthropic.com"}}], "detail": ["无法确认（api.anthropic.com 检测超时）", "Couldn't confirm (api.anthropic.com timed out)"]}
    },
    {
      "id": "timeout-before-deadline-code",
      "description": "ETIMEDOUT 自身决定分类",
      "source": [
        "apps/runtime/src/egress/trace-probe.ts:7-8 default timeout / slow threshold",
        "apps/runtime/src/egress/trace-probe.ts:53-81 probeTrace",
        "apps/runtime/src/egress/trace-probe.ts:104-197 defaultTraceRequester / classifyRequestError",
        "apps/runtime/src/egress/egress-evaluator.ts:80-117 evaluateFamily",
        "apps/runtime/src/egress/egress-evaluator.ts:132-174 unknownCheck / slowFacts / evaluateMainland",
        "apps/desktop/src/renderer/src/egress/egress-copy.ts:149-203 checkCopy"
      ],
      "input": {"operation": "trace", "eventAtMs": 4999, "event": "error", "code": "ETIMEDOUT"},
      "expected": {"classification": "timeout", "requestTimeoutMs": 10000, "settledAtMs": 4999, "sample": {"host": "api.anthropic.com", "error": "timeout"}, "familyState": "unknown", "state": "warning", "checks": [{"id": "mainland", "result": "unknown", "facts": {"error": "timeout", "host": "api.anthropic.com"}}, {"id": "exit-rule", "result": "unknown", "facts": {"error": "timeout", "host": "api.anthropic.com"}}, {"id": "login-api-match", "result": "unknown", "facts": {"error": "timeout", "host": "api.anthropic.com"}}], "detail": ["无法确认（api.anthropic.com 检测超时）", "Couldn't confirm (api.anthropic.com timed out)"]}
    },
    {
      "id": "timeout-injected-deadline",
      "description": "显式注入 4000 ms 请求时限",
      "source": [
        "apps/runtime/src/egress/trace-probe.ts:7-8 default timeout / slow threshold",
        "apps/runtime/src/egress/trace-probe.ts:53-81 probeTrace",
        "apps/runtime/src/egress/trace-probe.ts:104-197 defaultTraceRequester / classifyRequestError",
        "apps/runtime/src/egress/egress-evaluator.ts:80-117 evaluateFamily",
        "apps/runtime/src/egress/egress-evaluator.ts:132-174 unknownCheck / slowFacts / evaluateMainland",
        "apps/desktop/src/renderer/src/egress/egress-copy.ts:149-203 checkCopy"
      ],
      "input": {"operation": "trace", "eventAtMs": 4001, "event": "response", "body": "ip=192.0.2.10\nloc=US", "timeoutMs": 4000},
      "expected": {"classification": "timeout", "requestTimeoutMs": 4000, "settledAtMs": 4000, "sample": {"host": "api.anthropic.com", "error": "timeout"}, "familyState": "unknown", "state": "warning", "checks": [{"id": "mainland", "result": "unknown", "facts": {"error": "timeout", "host": "api.anthropic.com"}}, {"id": "exit-rule", "result": "unknown", "facts": {"error": "timeout", "host": "api.anthropic.com"}}, {"id": "login-api-match", "result": "unknown", "facts": {"error": "timeout", "host": "api.anthropic.com"}}], "detail": ["无法确认（api.anthropic.com 检测超时）", "Couldn't confirm (api.anthropic.com timed out)"]}
    },
    {
      "id": "invalid-after-slow-threshold",
      "description": "无效 IP 优先于慢提示",
      "source": [
        "apps/runtime/src/egress/trace-probe.ts:7-8 default timeout / slow threshold",
        "apps/runtime/src/egress/trace-probe.ts:53-81 probeTrace",
        "apps/runtime/src/egress/trace-probe.ts:104-197 defaultTraceRequester / classifyRequestError",
        "apps/runtime/src/egress/egress-evaluator.ts:80-117 evaluateFamily",
        "apps/runtime/src/egress/egress-evaluator.ts:132-174 unknownCheck / slowFacts / evaluateMainland",
        "apps/desktop/src/renderer/src/egress/egress-copy.ts:149-203 checkCopy"
      ],
      "input": {"operation": "trace", "eventAtMs": 5001, "event": "response", "body": "ip=invalid\nloc=US"},
      "expected": {"classification": "invalid", "requestTimeoutMs": 10000, "settledAtMs": 5001, "sample": {"host": "api.anthropic.com", "error": "invalid"}, "familyState": "unknown", "state": "warning", "checks": [{"id": "mainland", "result": "unknown", "facts": {"error": "invalid", "host": "api.anthropic.com"}}, {"id": "exit-rule", "result": "unknown", "facts": {"error": "invalid", "host": "api.anthropic.com"}}, {"id": "login-api-match", "result": "unknown", "facts": {"error": "invalid", "host": "api.anthropic.com"}}], "detail": ["无法确认（api.anthropic.com 返回异常）", "Couldn't confirm (api.anthropic.com invalid response)"]}
    },
    {
      "id": "launch-claude-default",
      "description": "空地址按官方端点处理",
      "source": [
        "apps/runtime/src/egress/egress-monitor.ts:41-101 matchesOfficialHost / egressFamilyForLaunch / egressPlanForLaunch",
        "apps/runtime/src/runtime-server.ts:2251-2265 launch context",
        "apps/runtime/src/runtime-server.ts:2804-2808 registerSession"
      ],
      "input": {"operation": "launch", "profile": "claude-code", "env": {}},
      "expected": {"hideTimeZone": true, "family": "anthropic", "exitFamily": "anthropic"}
    },
    {
      "id": "launch-claude-official-host",
      "description": "去空白后按 hostname 判定，大小写与路径无影响",
      "source": [
        "apps/runtime/src/egress/egress-monitor.ts:41-101 matchesOfficialHost / egressFamilyForLaunch / egressPlanForLaunch",
        "apps/runtime/src/runtime-server.ts:2251-2265 launch context",
        "apps/runtime/src/runtime-server.ts:2804-2808 registerSession"
      ],
      "input": {"operation": "launch", "profile": "claude-code", "env": {"ANTHROPIC_BASE_URL": " https://API.ANTHROPIC.COM/edge "}},
      "expected": {"hideTimeZone": true, "family": "anthropic", "exitFamily": "anthropic"}
    },
    {
      "id": "launch-claude-custom",
      "description": "看似官方的后缀域名不匹配",
      "source": [
        "apps/runtime/src/egress/egress-monitor.ts:41-101 matchesOfficialHost / egressFamilyForLaunch / egressPlanForLaunch",
        "apps/runtime/src/runtime-server.ts:2251-2265 launch context",
        "apps/runtime/src/runtime-server.ts:2804-2808 registerSession"
      ],
      "input": {"operation": "launch", "profile": "claude-code", "env": {"ANTHROPIC_BASE_URL": "https://api.anthropic.com.fixture.invalid"}},
      "expected": {"hideTimeZone": false}
    },
    {
      "id": "launch-codex-official",
      "description": "官方 hostname 匹配，端口无影响",
      "source": [
        "apps/runtime/src/egress/egress-monitor.ts:41-101 matchesOfficialHost / egressFamilyForLaunch / egressPlanForLaunch",
        "apps/runtime/src/runtime-server.ts:2251-2265 launch context",
        "apps/runtime/src/runtime-server.ts:2804-2808 registerSession"
      ],
      "input": {"operation": "launch", "profile": "codex", "env": {"OPENAI_BASE_URL": "https://api.openai.com:8443/v1"}},
      "expected": {"hideTimeZone": true, "family": "openai", "exitFamily": "openai"}
    },
    {
      "id": "launch-codex-invalid-url",
      "description": "非法 URL 不适用",
      "source": [
        "apps/runtime/src/egress/egress-monitor.ts:41-101 matchesOfficialHost / egressFamilyForLaunch / egressPlanForLaunch",
        "apps/runtime/src/runtime-server.ts:2251-2265 launch context",
        "apps/runtime/src/runtime-server.ts:2804-2808 registerSession"
      ],
      "input": {"operation": "launch", "profile": "codex", "env": {"OPENAI_BASE_URL": "not a URL"}},
      "expected": {"hideTimeZone": false}
    },
    {
      "id": "launch-shell",
      "description": "普通终端无受管 family",
      "source": [
        "apps/runtime/src/egress/egress-monitor.ts:41-101 matchesOfficialHost / egressFamilyForLaunch / egressPlanForLaunch",
        "apps/runtime/src/runtime-server.ts:2251-2265 launch context",
        "apps/runtime/src/runtime-server.ts:2804-2808 registerSession"
      ],
      "input": {"operation": "launch", "profile": "zsh", "env": {}},
      "expected": {"hideTimeZone": false}
    },
    {
      "id": "launch-antigravity",
      "description": "独立 Antigravity 归 Google 通用出口",
      "source": [
        "apps/runtime/src/egress/egress-monitor.ts:41-101 matchesOfficialHost / egressFamilyForLaunch / egressPlanForLaunch",
        "apps/runtime/src/runtime-server.ts:2251-2265 launch context",
        "apps/runtime/src/runtime-server.ts:2804-2808 registerSession"
      ],
      "input": {"operation": "launch", "profile": "antigravity", "env": {}},
      "expected": {"hideTimeZone": true, "family": "google", "exitFamily": "google"}
    },
    {
      "id": "launch-claude-codex-subscription",
      "description": "订阅身份优先于 Anthropic 地址",
      "source": [
        "apps/runtime/src/egress/egress-monitor.ts:41-101 matchesOfficialHost / egressFamilyForLaunch / egressPlanForLaunch",
        "apps/runtime/src/runtime-server.ts:2251-2265 launch context",
        "apps/runtime/src/runtime-server.ts:2804-2808 registerSession"
      ],
      "input": {"operation": "launch", "profile": "claude-code", "env": {"ANTHROPIC_BASE_URL": "https://fixture.invalid"}, "subscription": "codex"},
      "expected": {"hideTimeZone": true, "family": "chatgpt-subscription", "exitFamily": "chatgpt-subscription"}
    },
    {
      "id": "launch-claude-antigravity-subscription",
      "description": "Claude 走 Antigravity 订阅时仅应用时区策略",
      "source": [
        "apps/runtime/src/egress/egress-monitor.ts:41-101 matchesOfficialHost / egressFamilyForLaunch / egressPlanForLaunch",
        "apps/runtime/src/runtime-server.ts:2251-2265 launch context",
        "apps/runtime/src/runtime-server.ts:2804-2808 registerSession"
      ],
      "input": {"operation": "launch", "profile": "claude-code", "env": {}, "subscription": "antigravity"},
      "expected": {"hideTimeZone": true}
    },
    {
      "id": "channel-general-sample",
      "description": "通用采样含 Google 组，登录样本标不适用，禁用的订阅无组",
      "source": [
        "apps/runtime/src/egress/egress-sampler.ts:63-144 parseFixtureHostSample / sampleFullFromFixture",
        "apps/runtime/src/egress/egress-sampler.ts:193-308 createEgressSampler / sampleFull / sampleLight",
        "apps/runtime/src/egress/egress-evaluator.ts:45-55 evaluateEgress",
        "apps/runtime/src/egress/egress-evaluator.ts:80-117 evaluateFamily",
        "apps/desktop/src/renderer/src/egress/EgressPanel.tsx:390-455 EgressPanel family sections",
        "apps/desktop/src/renderer/src/egress/EgressPanel.tsx:540-545 EgressPanel footer"
      ],
      "input": {"operation": "channel", "routerEnabled": false},
      "expected": {"state": "ok", "families": [["anthropic", "ok", ["mainland", "exit-rule", "login-api-match"]], ["openai", "ok", ["mainland", "exit-rule", "login-api-match"]], ["google", "ok", ["mainland", "exit-rule"]]], "googleHosts": [{"host": "www.cloudflare.com", "ip": "192.0.2.10", "country": "US"}, {"host": "accounts.google.com", "notApplicable": true}], "fullProbeHosts": ["api.anthropic.com", "api.openai.com", "chatgpt.com", "claude.ai", "www.cloudflare.com"], "routerLight": {"host": "chatgpt.com", "error": "unreachable"}, "headings": ["Claude（Anthropic）", "Codex（OpenAI）", "Google（通用出口检测）", "本机"], "note": "检测为每分钟抽查，无法保证每条连接都使用同一出口。Google 线路使用通用出口检测，不能保证 Google 实际看到的出口。"}
    },
    {
      "id": "channel-subscription-enabled",
      "description": "启用订阅后增加订阅组与两个采样，Google 仍然只采通用主机",
      "source": [
        "apps/runtime/src/egress/egress-sampler.ts:63-144 parseFixtureHostSample / sampleFullFromFixture",
        "apps/runtime/src/egress/egress-sampler.ts:193-308 createEgressSampler / sampleFull / sampleLight",
        "apps/runtime/src/egress/egress-evaluator.ts:45-55 evaluateEgress",
        "apps/runtime/src/egress/egress-evaluator.ts:80-117 evaluateFamily",
        "apps/desktop/src/renderer/src/egress/EgressPanel.tsx:390-455 EgressPanel family sections",
        "apps/desktop/src/renderer/src/egress/EgressPanel.tsx:540-545 EgressPanel footer"
      ],
      "input": {"operation": "channel", "routerEnabled": true},
      "expected": {"state": "ok", "families": [["anthropic", "ok", ["mainland", "exit-rule", "login-api-match"]], ["openai", "ok", ["mainland", "exit-rule", "login-api-match"]], ["chatgpt-subscription", "ok", ["mainland", "exit-rule", "login-api-match"]], ["google", "ok", ["mainland", "exit-rule"]]], "googleHosts": [{"host": "www.cloudflare.com", "ip": "192.0.2.10", "country": "US"}, {"host": "accounts.google.com", "notApplicable": true}], "fullProbeHosts": ["api.anthropic.com", "api.openai.com", "auth.openai.com", "chatgpt.com", "chatgpt.com", "claude.ai", "www.cloudflare.com"], "routerLight": {"host": "chatgpt.com", "ip": "192.0.2.10", "country": "US"}, "headings": ["Claude（Anthropic）", "Codex（OpenAI）", "Claude（ChatGPT 订阅）", "Google（通用出口检测）", "本机"], "note": "检测为每分钟抽查，无法保证每条连接都使用同一出口。Google 线路使用通用出口检测，不能保证 Google 实际看到的出口。"}
    },
    {
      "id": "channel-no-google-sample",
      "description": "使用冻结的文件入口，省略 Google family 后整组也省略",
      "source": [
        "apps/runtime/src/egress/egress-sampler.ts:63-144 parseFixtureHostSample / sampleFullFromFixture",
        "apps/runtime/src/egress/egress-sampler.ts:193-308 createEgressSampler / sampleFull / sampleLight",
        "apps/runtime/src/egress/egress-evaluator.ts:45-55 evaluateEgress",
        "apps/runtime/src/egress/egress-evaluator.ts:80-117 evaluateFamily",
        "apps/desktop/src/renderer/src/egress/EgressPanel.tsx:390-455 EgressPanel family sections",
        "apps/desktop/src/renderer/src/egress/EgressPanel.tsx:540-545 EgressPanel footer"
      ],
      "input": {"operation": "channel", "fixtureHosts": {"anthropic": {"api": {"ip": "192.0.2.10", "country": "US"}, "login": {"ip": "192.0.2.10", "country": "US"}}, "openai": {"api": {"ip": "192.0.2.10", "country": "US"}, "login": {"ip": "192.0.2.10", "country": "US"}}}},
      "expected": {"state": "ok", "families": [["anthropic", "ok", ["mainland", "exit-rule", "login-api-match"]], ["openai", "ok", ["mainland", "exit-rule", "login-api-match"]]], "googleHosts": null, "fullProbeHosts": [], "routerLight": {"host": "chatgpt.com", "error": "timeout"}, "headings": ["Claude（Anthropic）", "Codex（OpenAI）", "本机"], "note": "检测为每分钟抽查，无法保证每条连接都使用同一出口。Google 线路使用通用出口检测，不能保证 Google 实际看到的出口。"}
    },
    {
      "id": "channel-google-login-ignored",
      "description": "固定数据含 Google 登录 CN，采样仍把它标为不适用且不触发大陆异常",
      "source": [
        "apps/runtime/src/egress/egress-sampler.ts:63-144 parseFixtureHostSample / sampleFullFromFixture",
        "apps/runtime/src/egress/egress-sampler.ts:193-308 createEgressSampler / sampleFull / sampleLight",
        "apps/runtime/src/egress/egress-evaluator.ts:45-55 evaluateEgress",
        "apps/runtime/src/egress/egress-evaluator.ts:80-117 evaluateFamily",
        "apps/desktop/src/renderer/src/egress/EgressPanel.tsx:390-455 EgressPanel family sections",
        "apps/desktop/src/renderer/src/egress/EgressPanel.tsx:540-545 EgressPanel footer"
      ],
      "input": {"operation": "channel", "fixtureHosts": {"anthropic": {"api": {"ip": "192.0.2.10", "country": "US"}, "login": {"ip": "192.0.2.10", "country": "US"}}, "openai": {"api": {"ip": "192.0.2.10", "country": "US"}, "login": {"ip": "192.0.2.10", "country": "US"}}, "google": {"api": {"ip": "192.0.2.10", "country": "US"}, "login": {"ip": "198.51.100.20", "country": "CN"}}}},
      "expected": {"state": "ok", "families": [["anthropic", "ok", ["mainland", "exit-rule", "login-api-match"]], ["openai", "ok", ["mainland", "exit-rule", "login-api-match"]], ["google", "ok", ["mainland", "exit-rule"]]], "googleHosts": [{"host": "www.cloudflare.com", "ip": "192.0.2.10", "country": "US"}, {"host": "accounts.google.com", "notApplicable": true}], "fullProbeHosts": [], "routerLight": {"host": "chatgpt.com", "error": "timeout"}, "headings": ["Claude（Anthropic）", "Codex（OpenAI）", "Google（通用出口检测）", "本机"], "note": "检测为每分钟抽查，无法保证每条连接都使用同一出口。Google 线路使用通用出口检测，不能保证 Google 实际看到的出口。"}
    },
    {
      "id": "protection-snapshot-switch-recovery",
      "description": "两张 Google 卡随同族冻结，切卡、注册新卡、重检失败、两次成功恢复。恢复计时器分别比较到期前 1 ms 和恰好到期。",
      "source": [
        "apps/runtime/src/egress/egress-monitor.ts:149-167 registerSession",
        "apps/runtime/src/egress/egress-monitor.ts:192-255 status",
        "apps/runtime/src/egress/egress-monitor.ts:325-403 runFullCheck / applySample",
        "apps/runtime/src/egress/egress-monitor.ts:419-482 reconcileFamily",
        "apps/runtime/src/egress/egress-monitor.ts:623-647 syncRecoveryTimer",
        "apps/desktop/src/renderer/src/egress/use-egress-status.ts:154-237 session selectors",
        "apps/desktop/src/renderer/src/egress/EgressFreezeBanner.tsx:36-108 EgressFreezeBanner",
        "apps/desktop/src/renderer/src/egress/egress-notifications.ts:14-50 egressNotification",
        "apps/desktop/src/renderer/src/hierarchy/TerminalPane.tsx:474-487 inputDisabled"
      ],
      "input": {
        "operation": "protection",
        "sessions": {"a": "google", "b": "google", "c": "openai", "shell": null},
        "steps": [
          {"atMs": 0, "action": "snapshot"},
          {"atMs": 10, "action": "check", "hosts": {"openai": {"ip": "192.0.2.10", "country": "US"}, "google": {"ip": "192.0.2.10", "country": "US"}}},
          {"atMs": 20, "action": "check", "hosts": {"openai": {"ip": "192.0.2.10", "country": "US"}, "google": {"ip": "198.51.100.20", "country": "CN"}}},
          {"atMs": 21, "action": "view", "id": "b"},
          {"atMs": 22, "action": "register", "id": "late", "family": "google"},
          {"atMs": 15021, "action": "advance", "hosts": {"openai": {"ip": "192.0.2.10", "country": "US"}, "google": "timeout"}},
          {"atMs": 15022, "action": "advance"},
          {"atMs": 30022, "action": "advance", "hosts": {"openai": {"ip": "192.0.2.10", "country": "US"}, "google": {"ip": "192.0.2.10", "country": "US"}}},
          {"atMs": 35021, "action": "advance"},
          {"atMs": 35022, "action": "advance"}
        ]
      },
      "expected": [
        {"state": "checking", "checkedAtMs": null, "protectionEnabled": true, "ruleMode": "country", "families": {"anthropic": ["unknown", null, [], false, false], "openai": ["unknown", null, [], false, false], "google": ["unknown", null, [], false, false]}, "cards": {"a": ["google", false, "none", null], "b": ["google", false, "none", null], "c": ["openai", false, "none", null], "shell": [null, false, "none", null]}, "effects": [], "nextCheckInMs": null, "fullChecks": 0, "transition": null, "notification": null},
        {"state": "ok", "checkedAtMs": 10, "protectionEnabled": true, "ruleMode": "country", "families": {"openai": ["ok", null, [], false, false], "google": ["ok", null, [], false, false]}, "cards": {"a": ["google", false, "none", null], "b": ["google", false, "none", null], "c": ["openai", false, "none", null], "shell": [null, false, "none", null]}, "effects": [], "nextCheckInMs": null, "fullChecks": 1, "transition": null, "notification": null},
        {"state": "abnormal", "checkedAtMs": 20, "protectionEnabled": true, "ruleMode": "country", "families": {"openai": ["ok", null, [], false, false], "google": ["abnormal", "mainland", ["a", "b"], true, false]}, "cards": {"a": ["google", true, "frozen", "出口异常（检测到中国大陆 IP），已暂停。网络恢复后自动继续。"], "b": ["google", true, "frozen", "出口异常（检测到中国大陆 IP），已暂停。网络恢复后自动继续。"], "c": ["openai", false, "none", null], "shell": [null, false, "none", null]}, "effects": ["freeze:a", "freeze:b"], "nextCheckInMs": 15000, "fullChecks": 2, "transition": ["frozen", "google", 2, 20, "mainland"], "notification": "出口异常，已暂停 2 个会话"},
        {"state": "abnormal", "checkedAtMs": 20, "protectionEnabled": true, "ruleMode": "country", "families": {"openai": ["ok", null, [], false, false], "google": ["abnormal", "mainland", ["a", "b"], true, false]}, "cards": {"a": ["google", true, "frozen", "出口异常（检测到中国大陆 IP），已暂停。网络恢复后自动继续。"], "b": ["google", true, "frozen", "出口异常（检测到中国大陆 IP），已暂停。网络恢复后自动继续。"], "c": ["openai", false, "none", null], "shell": [null, false, "none", null]}, "effects": [], "nextCheckInMs": 14999, "fullChecks": 2, "transition": ["frozen", "google", 2, 20, "mainland"], "notification": null, "actionResult": ["google", true, "frozen", "出口异常（检测到中国大陆 IP），已暂停。网络恢复后自动继续。"]},
        {"state": "abnormal", "checkedAtMs": 20, "protectionEnabled": true, "ruleMode": "country", "families": {"openai": ["ok", null, [], false, false], "google": ["abnormal", "mainland", ["a", "b", "late"], true, false]}, "cards": {"a": ["google", true, "frozen", "出口异常（检测到中国大陆 IP），已暂停。网络恢复后自动继续。"], "b": ["google", true, "frozen", "出口异常（检测到中国大陆 IP），已暂停。网络恢复后自动继续。"], "c": ["openai", false, "none", null], "shell": [null, false, "none", null], "late": ["google", true, "frozen", "出口异常（检测到中国大陆 IP），已暂停。网络恢复后自动继续。"]}, "effects": [], "nextCheckInMs": 15000, "fullChecks": 2, "transition": ["frozen", "google", 2, 20, "mainland"], "notification": null, "actionResult": {"bornFrozen": true}},
        {"state": "abnormal", "checkedAtMs": 20, "protectionEnabled": true, "ruleMode": "country", "families": {"openai": ["ok", null, [], false, false], "google": ["abnormal", "mainland", ["a", "b", "late"], true, false]}, "cards": {"a": ["google", true, "frozen", "出口异常（检测到中国大陆 IP），已暂停。网络恢复后自动继续。"], "b": ["google", true, "frozen", "出口异常（检测到中国大陆 IP），已暂停。网络恢复后自动继续。"], "c": ["openai", false, "none", null], "shell": [null, false, "none", null], "late": ["google", true, "frozen", "出口异常（检测到中国大陆 IP），已暂停。网络恢复后自动继续。"]}, "effects": [], "nextCheckInMs": 1, "fullChecks": 2, "transition": ["frozen", "google", 2, 20, "mainland"], "notification": null},
        {"state": "warning", "checkedAtMs": 15022, "protectionEnabled": true, "ruleMode": "country", "families": {"openai": ["ok", null, [], false, false], "google": ["unknown", null, ["a", "b", "late"], true, false]}, "cards": {"a": ["google", true, "frozen", "出口异常（检测到中国大陆 IP），已暂停。网络恢复后自动继续。"], "b": ["google", true, "frozen", "出口异常（检测到中国大陆 IP），已暂停。网络恢复后自动继续。"], "c": ["openai", false, "none", null], "shell": [null, false, "none", null], "late": ["google", true, "frozen", "出口异常（检测到中国大陆 IP），已暂停。网络恢复后自动继续。"]}, "effects": [], "nextCheckInMs": 15000, "fullChecks": 3, "transition": ["frozen", "google", 2, 20, "mainland"], "notification": null},
        {"state": "ok", "checkedAtMs": 30022, "protectionEnabled": true, "ruleMode": "country", "families": {"openai": ["ok", null, [], false, false], "google": ["ok", null, ["a", "b", "late"], true, false]}, "cards": {"a": ["google", true, "frozen", "出口异常（检测到中国大陆 IP），已暂停。网络恢复后自动继续。"], "b": ["google", true, "frozen", "出口异常（检测到中国大陆 IP），已暂停。网络恢复后自动继续。"], "c": ["openai", false, "none", null], "shell": [null, false, "none", null], "late": ["google", true, "frozen", "出口异常（检测到中国大陆 IP），已暂停。网络恢复后自动继续。"]}, "effects": [], "nextCheckInMs": 5000, "fullChecks": 4, "transition": ["frozen", "google", 2, 20, "mainland"], "notification": null},
        {"state": "ok", "checkedAtMs": 30022, "protectionEnabled": true, "ruleMode": "country", "families": {"openai": ["ok", null, [], false, false], "google": ["ok", null, ["a", "b", "late"], true, false]}, "cards": {"a": ["google", true, "frozen", "出口异常（检测到中国大陆 IP），已暂停。网络恢复后自动继续。"], "b": ["google", true, "frozen", "出口异常（检测到中国大陆 IP），已暂停。网络恢复后自动继续。"], "c": ["openai", false, "none", null], "shell": [null, false, "none", null], "late": ["google", true, "frozen", "出口异常（检测到中国大陆 IP），已暂停。网络恢复后自动继续。"]}, "effects": [], "nextCheckInMs": 1, "fullChecks": 4, "transition": ["frozen", "google", 2, 20, "mainland"], "notification": null},
        {"state": "ok", "checkedAtMs": 35022, "protectionEnabled": true, "ruleMode": "country", "families": {"openai": ["ok", null, [], false, false], "google": ["ok", null, [], false, false]}, "cards": {"a": ["google", false, "resumed", "出口已恢复，已自动继续"], "b": ["google", false, "resumed", "出口已恢复，已自动继续"], "c": ["openai", false, "none", null], "shell": [null, false, "none", null], "late": ["google", false, "resumed", "出口已恢复，已自动继续"]}, "effects": ["thaw:a", "thaw:b", "thaw:late"], "nextCheckInMs": null, "fullChecks": 5, "transition": ["resumed", "google", 3, 35022, null], "notification": "出口已恢复，3 个会话已继续"}
      ]
    },
    {
      "id": "protection-rule-and-toggle",
      "description": "同国换 IP、切换 IP 规则、关闭再开启保护、采纳非大陆基线及拒绝采纳大陆出口。",
      "source": [
        "apps/runtime/src/egress/egress-monitor.ts:149-167 registerSession",
        "apps/runtime/src/egress/egress-monitor.ts:192-255 status",
        "apps/runtime/src/egress/egress-monitor.ts:325-403 runFullCheck / applySample",
        "apps/runtime/src/egress/egress-monitor.ts:419-482 reconcileFamily",
        "apps/runtime/src/egress/egress-monitor.ts:623-647 syncRecoveryTimer",
        "apps/desktop/src/renderer/src/egress/use-egress-status.ts:154-237 session selectors",
        "apps/desktop/src/renderer/src/egress/EgressFreezeBanner.tsx:36-108 EgressFreezeBanner",
        "apps/desktop/src/renderer/src/egress/egress-notifications.ts:14-50 egressNotification",
        "apps/desktop/src/renderer/src/hierarchy/TerminalPane.tsx:474-487 inputDisabled",
        "apps/runtime/src/egress/egress-evaluator.ts:154-210 evaluateMainland / evaluateExitRule",
        "apps/runtime/src/egress/egress-monitor.ts:263-305 setRuleMode / acceptCurrent / setProtection"
      ],
      "input": {
        "operation": "protection",
        "sessions": {"a": "google", "shell": null},
        "steps": [
          {"atMs": 10, "action": "check", "hosts": {"google": {"ip": "192.0.2.10", "country": "US"}}},
          {"atMs": 20, "action": "check", "hosts": {"google": {"ip": "192.0.2.11", "country": "US"}}},
          {"atMs": 21, "action": "rule", "mode": "ip"},
          {"atMs": 22, "action": "protect", "enabled": false},
          {"atMs": 23, "action": "protect", "enabled": true},
          {"atMs": 24, "action": "accept", "family": "google"},
          {"atMs": 25, "action": "check", "hosts": {"google": {"ip": "198.51.100.20", "country": "CN"}}},
          {"atMs": 26, "action": "accept", "family": "google"}
        ]
      },
      "expected": [
        {"state": "ok", "checkedAtMs": 10, "protectionEnabled": true, "ruleMode": "country", "families": {"google": ["ok", null, [], false, false]}, "cards": {"a": ["google", false, "none", null], "shell": [null, false, "none", null]}, "effects": [], "nextCheckInMs": null, "fullChecks": 1, "transition": null, "notification": null},
        {"state": "ok", "checkedAtMs": 20, "protectionEnabled": true, "ruleMode": "country", "families": {"google": ["ok", null, [], false, false]}, "cards": {"a": ["google", false, "none", null], "shell": [null, false, "none", null]}, "effects": [], "nextCheckInMs": null, "fullChecks": 2, "transition": null, "notification": null},
        {"state": "abnormal", "checkedAtMs": 20, "protectionEnabled": true, "ruleMode": "ip", "families": {"google": ["abnormal", "rule", ["a"], true, false]}, "cards": {"a": ["google", true, "frozen", "出口与规则不符（美国 → 美国），已暂停。网络恢复后自动继续。"], "shell": [null, false, "none", null]}, "effects": ["freeze:a"], "nextCheckInMs": 15000, "fullChecks": 2, "transition": ["frozen", "google", 1, 21, "rule"], "notification": "出口异常，已暂停 1 个会话"},
        {"state": "abnormal", "checkedAtMs": 20, "protectionEnabled": false, "ruleMode": "ip", "families": {"google": ["abnormal", "rule", [], false, false]}, "cards": {"a": ["google", false, "none", null], "shell": [null, false, "none", null]}, "effects": ["thaw:a"], "nextCheckInMs": null, "fullChecks": 2, "transition": ["frozen", "google", 1, 21, "rule"], "notification": null},
        {"state": "abnormal", "checkedAtMs": 20, "protectionEnabled": true, "ruleMode": "ip", "families": {"google": ["abnormal", "rule", ["a"], true, false]}, "cards": {"a": ["google", true, "frozen", "出口与规则不符（美国 → 美国），已暂停。网络恢复后自动继续。"], "shell": [null, false, "none", null]}, "effects": ["freeze:a"], "nextCheckInMs": 15000, "fullChecks": 2, "transition": ["frozen", "google", 1, 23, "rule"], "notification": "出口异常，已暂停 1 个会话"},
        {"state": "ok", "checkedAtMs": 20, "protectionEnabled": true, "ruleMode": "ip", "families": {"google": ["ok", null, [], false, false]}, "cards": {"a": ["google", false, "resumed", "出口已恢复，已自动继续"], "shell": [null, false, "none", null]}, "effects": ["thaw:a"], "nextCheckInMs": null, "fullChecks": 2, "transition": ["resumed", "google", 1, 24, null], "notification": "出口已恢复，1 个会话已继续"},
        {"state": "abnormal", "checkedAtMs": 25, "protectionEnabled": true, "ruleMode": "ip", "families": {"google": ["abnormal", "mainland", ["a"], true, false]}, "cards": {"a": ["google", true, "frozen", "出口异常（检测到中国大陆 IP），已暂停。网络恢复后自动继续。"], "shell": [null, false, "none", null]}, "effects": ["freeze:a"], "nextCheckInMs": 15000, "fullChecks": 3, "transition": ["frozen", "google", 1, 25, "mainland"], "notification": "出口异常，已暂停 1 个会话"},
        {"state": "abnormal", "checkedAtMs": 25, "protectionEnabled": true, "ruleMode": "ip", "families": {"google": ["abnormal", "mainland", ["a"], true, false]}, "cards": {"a": ["google", true, "frozen", "出口异常（检测到中国大陆 IP），已暂停。网络恢复后自动继续。"], "shell": [null, false, "none", null]}, "effects": [], "nextCheckInMs": 14999, "fullChecks": 3, "transition": ["frozen", "google", 1, 25, "mainland"], "notification": null, "actionResult": "Current API exit for google does not pass the mainland check"}
      ]
    },
    {
      "id": "protection-multi-channel",
      "description": "共享 Claude 卡挂两个 family，切换阻塞原因；只恢复 Google 时只解锁 Google 单路卡。",
      "source": [
        "apps/runtime/src/egress/egress-monitor.ts:149-167 registerSession",
        "apps/runtime/src/egress/egress-monitor.ts:192-255 status",
        "apps/runtime/src/egress/egress-monitor.ts:325-403 runFullCheck / applySample",
        "apps/runtime/src/egress/egress-monitor.ts:419-482 reconcileFamily",
        "apps/runtime/src/egress/egress-monitor.ts:623-647 syncRecoveryTimer",
        "apps/desktop/src/renderer/src/egress/use-egress-status.ts:154-237 session selectors",
        "apps/desktop/src/renderer/src/egress/EgressFreezeBanner.tsx:36-108 EgressFreezeBanner",
        "apps/desktop/src/renderer/src/egress/egress-notifications.ts:14-50 egressNotification",
        "apps/desktop/src/renderer/src/hierarchy/TerminalPane.tsx:474-487 inputDisabled",
        "apps/runtime/src/egress/egress-monitor.ts:526-531 sessionCanThaw"
      ],
      "input": {
        "operation": "protection",
        "sessions": {"a": ["openai", "google"], "b": "google"},
        "steps": [
          {"atMs": 10, "action": "check", "hosts": {"openai": {"ip": "192.0.2.10", "country": "US"}, "google": {"ip": "192.0.2.10", "country": "US"}}},
          {"atMs": 20, "action": "check", "hosts": {"openai": {"ip": "192.0.2.10", "country": "US"}, "google": {"ip": "198.51.100.20", "country": "CN"}}},
          {"atMs": 30, "action": "check", "hosts": {"openai": {"ip": "198.51.100.20", "country": "CN"}, "google": {"ip": "198.51.100.20", "country": "CN"}}},
          {"atMs": 40, "action": "check", "hosts": {"openai": {"ip": "198.51.100.20", "country": "CN"}, "google": {"ip": "192.0.2.10", "country": "US"}}},
          {"atMs": 50, "action": "check", "hosts": {"openai": {"ip": "198.51.100.20", "country": "CN"}, "google": {"ip": "192.0.2.10", "country": "US"}}}
        ]
      },
      "expected": [
        {"state": "ok", "checkedAtMs": 10, "protectionEnabled": true, "ruleMode": "country", "families": {"openai": ["ok", null, [], false, false], "google": ["ok", null, [], false, false]}, "cards": {"a": ["openai", false, "none", null], "b": ["google", false, "none", null]}, "effects": [], "nextCheckInMs": null, "fullChecks": 1, "transition": null, "notification": null},
        {"state": "abnormal", "checkedAtMs": 20, "protectionEnabled": true, "ruleMode": "country", "families": {"openai": ["ok", null, ["a"], false, false], "google": ["abnormal", "mainland", ["a", "b"], true, false]}, "cards": {"a": ["google", true, "frozen", "出口异常（检测到中国大陆 IP），已暂停。网络恢复后自动继续。"], "b": ["google", true, "frozen", "出口异常（检测到中国大陆 IP），已暂停。网络恢复后自动继续。"]}, "effects": ["freeze:a", "freeze:b"], "nextCheckInMs": 15000, "fullChecks": 2, "transition": ["frozen", "google", 2, 20, "mainland"], "notification": "出口异常，已暂停 2 个会话"},
        {"state": "abnormal", "checkedAtMs": 30, "protectionEnabled": true, "ruleMode": "country", "families": {"openai": ["abnormal", "mainland", ["a"], true, false], "google": ["abnormal", "mainland", ["a", "b"], true, false]}, "cards": {"a": ["openai", true, "frozen", "出口异常（检测到中国大陆 IP），已暂停。网络恢复后自动继续。"], "b": ["google", true, "frozen", "出口异常（检测到中国大陆 IP），已暂停。网络恢复后自动继续。"]}, "effects": [], "nextCheckInMs": 15000, "fullChecks": 3, "transition": ["frozen", "google", 2, 20, "mainland"], "notification": null},
        {"state": "abnormal", "checkedAtMs": 40, "protectionEnabled": true, "ruleMode": "country", "families": {"openai": ["abnormal", "mainland", ["a"], true, false], "google": ["ok", null, ["a", "b"], true, false]}, "cards": {"a": ["openai", true, "frozen", "出口异常（检测到中国大陆 IP），已暂停。网络恢复后自动继续。"], "b": ["google", true, "frozen", "出口异常（检测到中国大陆 IP），已暂停。网络恢复后自动继续。"]}, "effects": [], "nextCheckInMs": 5000, "fullChecks": 4, "transition": ["frozen", "google", 2, 20, "mainland"], "notification": null},
        {"state": "abnormal", "checkedAtMs": 50, "protectionEnabled": true, "ruleMode": "country", "families": {"openai": ["abnormal", "mainland", ["a"], true, false], "google": ["ok", null, ["a"], true, false]}, "cards": {"a": ["openai", true, "frozen", "出口异常（检测到中国大陆 IP），已暂停。网络恢复后自动继续。"], "b": ["google", false, "resumed", "出口已恢复，已自动继续"]}, "effects": ["thaw:b"], "nextCheckInMs": 5000, "fullChecks": 5, "transition": ["resumed", "google", 1, 50, null], "notification": "出口已恢复，1 个会话已继续"}
      ]
    },
    {
      "id": "protection-override-confirm-cancel",
      "description": "取消只关弹窗，确认当场解锁该 family，同 IP 保持豁免，换 IP 后再次冻结。",
      "source": [
        "apps/desktop/src/renderer/src/egress/EgressFreezeBanner.tsx:47-108 EgressFreezeBanner controls / confirmation",
        "apps/desktop/src/renderer/src/hierarchy/ConfirmDialog.tsx:59-72 ConfirmDialog buttons",
        "apps/runtime/src/egress/egress-monitor.ts:308-322 override",
        "apps/runtime/src/egress/egress-monitor.ts:405-416 updateOverrideState",
        "apps/runtime/src/egress/egress-monitor.ts:419-451 reconcileFamily",
        "apps/desktop/src/renderer/src/egress/use-egress-status.ts:184-237 session selectors"
      ],
      "input": {"operation": "override", "sessions": {"a": "google", "b": "openai"}, "hosts": {"openai": {"ip": "192.0.2.10", "country": "US"}, "google": {"ip": "198.51.100.20", "country": "CN"}}, "nextHosts": {"openai": {"ip": "192.0.2.10", "country": "US"}, "google": {"ip": "198.51.100.21", "country": "CN"}}},
      "expected": {"initialButtons": ["重新检查", "仍要继续"], "confirmation": ["仍要继续？", "出口仍然异常，继续后 agent 会用当前出口访问服务商。出口再次变化前不会再暂停。", "取消", "继续"], "canceled": {"dialogClosed": true, "overrideCalls": [], "cards": {"a": ["google", true, "frozen", "出口异常（检测到中国大陆 IP），已暂停。网络恢复后自动继续。"], "b": ["openai", false, "none", null]}, "effects": ["freeze:a"], "overridden": []}, "confirmed": {"dialogClosed": true, "overrideCalls": ["google"], "cards": {"a": ["google", false, "none", null], "b": ["openai", false, "none", null]}, "effects": ["thaw:a"], "overridden": ["google"]}, "sameExit": {"cards": {"a": ["google", false, "none", null], "b": ["openai", false, "none", null]}, "effects": [], "overridden": ["google"]}, "changedExit": {"cards": {"a": ["google", true, "frozen", "出口异常（检测到中国大陆 IP），已暂停。网络恢复后自动继续。"], "b": ["openai", false, "none", null]}, "effects": ["freeze:a"], "overridden": []}}
    },
    {
      "id": "timezone-save-explicit-claude",
      "description": "Claude 自动选台北，显式 UTC 只在后续启动读取",
      "source": [
        "apps/runtime/src/session/claude-settings.ts:17-25 isClaudeTimeZone",
        "apps/runtime/src/session/claude-settings.ts:36-80 snapshot / set / environment",
        "apps/runtime/src/egress/time-zone-policy.ts:3-45 isMainlandTimeZone / agentTimeZone",
        "packages/domain/src/agent-manifest.ts:139,199,274 claudeTimeZone capabilities",
        "apps/runtime/src/rpc/runtime-rpc-router.ts:196-201 claude-settings.set",
        "apps/runtime/src/runtime-server.ts:2562-2568 launch env",
        "apps/runtime/src/index.ts:269-275 agentTimeZoneMainland",
        "apps/runtime/src/egress/egress-evaluator.ts:57-75 evaluateEgress",
        "apps/desktop/src/renderer/src/hierarchy/ClaudeTimeZoneSettings.tsx:46-54,70-78 save / scope"
      ],
      "input": {"operation": "timeZone", "profile": "claude-code", "systemTimeZone": "Asia/Shanghai", "exitCountry": "TW", "savedZone": "UTC"},
      "expected": {"activeEnv": {"TZ": "Asia/Taipei"}, "saveResult": "UTC", "sameStatusOnSave": true, "checksOnSave": 0, "nextLaunchEnv": {"TZ": "UTC"}, "timeZoneCheck": "pass", "state": "ok", "frozen": false, "effects": []}
    },
    {
      "id": "timezone-save-explicit-antigravity",
      "description": "Antigravity 接受保存的共享值；后续启动按 manifest 继续使用自动台北时区",
      "source": [
        "apps/runtime/src/session/claude-settings.ts:17-25 isClaudeTimeZone",
        "apps/runtime/src/session/claude-settings.ts:36-80 snapshot / set / environment",
        "apps/runtime/src/egress/time-zone-policy.ts:3-45 isMainlandTimeZone / agentTimeZone",
        "packages/domain/src/agent-manifest.ts:139,199,274 claudeTimeZone capabilities",
        "apps/runtime/src/rpc/runtime-rpc-router.ts:196-201 claude-settings.set",
        "apps/runtime/src/runtime-server.ts:2562-2568 launch env",
        "apps/runtime/src/index.ts:269-275 agentTimeZoneMainland",
        "apps/runtime/src/egress/egress-evaluator.ts:57-75 evaluateEgress",
        "apps/desktop/src/renderer/src/hierarchy/ClaudeTimeZoneSettings.tsx:46-54,70-78 save / scope"
      ],
      "input": {"operation": "timeZone", "profile": "antigravity", "systemTimeZone": "Asia/Shanghai", "exitCountry": "TW", "savedZone": "UTC"},
      "expected": {"activeEnv": {"TZ": "Asia/Taipei"}, "saveResult": "UTC", "sameStatusOnSave": true, "checksOnSave": 0, "nextLaunchEnv": {"TZ": "Asia/Taipei"}, "timeZoneCheck": "pass", "state": "ok", "frozen": false, "effects": []}
    },
    {
      "id": "timezone-save-explicit-codex",
      "description": "Codex 保存后继续按 manifest 使用自动新加坡时区",
      "source": [
        "apps/runtime/src/session/claude-settings.ts:17-25 isClaudeTimeZone",
        "apps/runtime/src/session/claude-settings.ts:36-80 snapshot / set / environment",
        "apps/runtime/src/egress/time-zone-policy.ts:3-45 isMainlandTimeZone / agentTimeZone",
        "packages/domain/src/agent-manifest.ts:139,199,274 claudeTimeZone capabilities",
        "apps/runtime/src/rpc/runtime-rpc-router.ts:196-201 claude-settings.set",
        "apps/runtime/src/runtime-server.ts:2562-2568 launch env",
        "apps/runtime/src/index.ts:269-275 agentTimeZoneMainland",
        "apps/runtime/src/egress/egress-evaluator.ts:57-75 evaluateEgress",
        "apps/desktop/src/renderer/src/hierarchy/ClaudeTimeZoneSettings.tsx:46-54,70-78 save / scope"
      ],
      "input": {"operation": "timeZone", "profile": "codex", "systemTimeZone": "Asia/Shanghai", "exitCountry": "US", "savedZone": "UTC"},
      "expected": {"activeEnv": {"TZ": "Asia/Singapore"}, "saveResult": "UTC", "sameStatusOnSave": true, "checksOnSave": 0, "nextLaunchEnv": {"TZ": "Asia/Singapore"}, "timeZoneCheck": "pass", "state": "ok", "frozen": false, "effects": []}
    },
    {
      "id": "timezone-save-system",
      "description": "探测使用 Claude 策略，保存跟随大陆系统时区后待下次检测产生提醒；Antigravity 仍自动取新加坡，卡继续运行",
      "source": [
        "apps/runtime/src/session/claude-settings.ts:17-25 isClaudeTimeZone",
        "apps/runtime/src/session/claude-settings.ts:36-80 snapshot / set / environment",
        "apps/runtime/src/egress/time-zone-policy.ts:3-45 isMainlandTimeZone / agentTimeZone",
        "packages/domain/src/agent-manifest.ts:139,199,274 claudeTimeZone capabilities",
        "apps/runtime/src/rpc/runtime-rpc-router.ts:196-201 claude-settings.set",
        "apps/runtime/src/runtime-server.ts:2562-2568 launch env",
        "apps/runtime/src/index.ts:269-275 agentTimeZoneMainland",
        "apps/runtime/src/egress/egress-evaluator.ts:57-75 evaluateEgress",
        "apps/desktop/src/renderer/src/hierarchy/ClaudeTimeZoneSettings.tsx:46-54,70-78 save / scope"
      ],
      "input": {"operation": "timeZone", "profile": "antigravity", "systemTimeZone": "Asia/Shanghai", "exitCountry": "US", "savedZone": "system"},
      "expected": {"activeEnv": {"TZ": "Asia/Singapore"}, "saveResult": "system", "sameStatusOnSave": true, "checksOnSave": 0, "nextLaunchEnv": {"TZ": "Asia/Singapore"}, "timeZoneCheck": "warn", "state": "warning", "frozen": false, "effects": []}
    },
    {
      "id": "timezone-save-invalid",
      "description": "非法时区保留原值与运行态",
      "source": [
        "apps/runtime/src/session/claude-settings.ts:17-25 isClaudeTimeZone",
        "apps/runtime/src/session/claude-settings.ts:36-80 snapshot / set / environment",
        "apps/runtime/src/egress/time-zone-policy.ts:3-45 isMainlandTimeZone / agentTimeZone",
        "packages/domain/src/agent-manifest.ts:139,199,274 claudeTimeZone capabilities",
        "apps/runtime/src/rpc/runtime-rpc-router.ts:196-201 claude-settings.set",
        "apps/runtime/src/runtime-server.ts:2562-2568 launch env",
        "apps/runtime/src/index.ts:269-275 agentTimeZoneMainland",
        "apps/runtime/src/egress/egress-evaluator.ts:57-75 evaluateEgress",
        "apps/desktop/src/renderer/src/hierarchy/ClaudeTimeZoneSettings.tsx:46-54,70-78 save / scope"
      ],
      "input": {"operation": "timeZone", "profile": "antigravity", "systemTimeZone": "Asia/Shanghai", "exitCountry": "US", "savedZone": "Mars/Olympus"},
      "expected": {"activeEnv": {"TZ": "Asia/Singapore"}, "saveResult": "Invalid Claude Code time zone", "sameStatusOnSave": true, "checksOnSave": 0, "nextLaunchEnv": {"TZ": "Asia/Singapore"}, "timeZoneCheck": "pass", "state": "ok", "frozen": false, "effects": []}
    },
    {
      "id": "timezone-non-mainland-auto",
      "description": "本机非大陆，自动模式不注入 TZ",
      "source": [
        "apps/runtime/src/session/claude-settings.ts:17-25 isClaudeTimeZone",
        "apps/runtime/src/session/claude-settings.ts:36-80 snapshot / set / environment",
        "apps/runtime/src/egress/time-zone-policy.ts:3-45 isMainlandTimeZone / agentTimeZone",
        "packages/domain/src/agent-manifest.ts:139,199,274 claudeTimeZone capabilities",
        "apps/runtime/src/rpc/runtime-rpc-router.ts:196-201 claude-settings.set",
        "apps/runtime/src/runtime-server.ts:2562-2568 launch env",
        "apps/runtime/src/index.ts:269-275 agentTimeZoneMainland",
        "apps/runtime/src/egress/egress-evaluator.ts:57-75 evaluateEgress",
        "apps/desktop/src/renderer/src/hierarchy/ClaudeTimeZoneSettings.tsx:46-54,70-78 save / scope"
      ],
      "input": {"operation": "timeZone", "profile": "antigravity", "systemTimeZone": "America/New_York", "exitCountry": "TW", "savedZone": "auto"},
      "expected": {"activeEnv": {}, "saveResult": "auto", "sameStatusOnSave": true, "checksOnSave": 0, "nextLaunchEnv": {}, "timeZoneCheck": "pass", "state": "ok", "frozen": false, "effects": []}
    },
    {
      "id": "locale-change-while-frozen",
      "description": "语言切换使同一冻结卡改用英文提示，保留冻结状态",
      "source": [
        "apps/desktop/src/main/locale-store.ts:19-43 LocaleStore",
        "apps/desktop/src/main/index.ts:537-546,687-692 locale broadcast and setter",
        "apps/desktop/src/renderer/src/i18n/LocaleProvider.tsx:9-24 LocaleProvider",
        "apps/desktop/src/renderer/src/egress/use-egress-status.ts:264-278 formatBannerFreezeMessage",
        "apps/desktop/src/main/runtime-host.ts:246-259 runtime spawn env",
        "apps/runtime/src/i18n/locale.ts:9-12 runtimeLocale"
      ],
      "input": {"operation": "locale", "system": "zh-CN", "preference": "en", "exit": {"ip": "198.51.100.20", "country": "CN"}},
      "expected": {"initial": "zh-CN", "stored": "en", "effective": "en", "changed": ["en"], "sameStatus": true, "checksOnSave": 0, "frozen": true, "bannerBefore": "出口异常（检测到中国大陆 IP），已暂停。网络恢复后自动继续。", "bannerAfter": "Exit problem (mainland China IP detected). Paused; resumes automatically when the network recovers.", "effects": []}
    },
    {
      "id": "locale-forced-while-frozen",
      "description": "保存英文偏好，环境覆盖优先，冻结状态与中文提示持续",
      "source": [
        "apps/desktop/src/main/locale-store.ts:19-43 LocaleStore",
        "apps/desktop/src/main/index.ts:537-546,687-692 locale broadcast and setter",
        "apps/desktop/src/renderer/src/i18n/LocaleProvider.tsx:9-24 LocaleProvider",
        "apps/desktop/src/renderer/src/egress/use-egress-status.ts:264-278 formatBannerFreezeMessage",
        "apps/desktop/src/main/runtime-host.ts:246-259 runtime spawn env",
        "apps/runtime/src/i18n/locale.ts:9-12 runtimeLocale"
      ],
      "input": {"operation": "locale", "system": "zh-CN", "preference": "en", "exit": {"ip": "198.51.100.20", "country": "CN"}, "forced": "zh-CN"},
      "expected": {"initial": "zh-CN", "stored": "en", "effective": "zh-CN", "changed": ["zh-CN"], "sameStatus": true, "checksOnSave": 0, "frozen": true, "bannerBefore": "出口异常（检测到中国大陆 IP），已暂停。网络恢复后自动继续。", "bannerAfter": "出口异常（检测到中国大陆 IP），已暂停。网络恢复后自动继续。", "effects": []}
    }
  ]
}"#;

#[derive(Deserialize)]
struct Fixture {
    source: Source,
    constants: Value,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Source {
    commit: String,
    inventory: Vec<String>,
}

#[derive(Deserialize)]
struct Case {
    id: String,
    description: String,
    source: Vec<String>,
    input: Value,
    expected: Value,
}

fn fixture() -> Fixture {
    serde_json::from_str(FIXTURE_JSON).expect("valid frozen egress fixture")
}

#[test]
fn frozen_egress_cases_can_be_loaded_by_native_tests() {
    let fixture = fixture();
    assert_eq!(
        fixture.source.commit,
        "924de28da27999543aeef7a7c9797334fca45d97"
    );
    assert_eq!(
        fixture.source.inventory,
        ["NET-06", "NET-07", "NET-08", "REC-09"]
    );
    assert!(fixture.constants["epochMs"].is_u64());
    let mut ids = BTreeSet::new();
    let mut operations = BTreeSet::new();
    for case in &fixture.cases {
        assert!(!case.id.is_empty() && ids.insert(case.id.as_str()));
        assert!(!case.description.is_empty() && !case.source.is_empty());
        let operation = case.input["operation"].as_str().expect("input operation");
        operations.insert(operation);
        if operation == "protection" {
            let steps = case.input["steps"].as_array().expect("protection steps");
            let expected = case.expected.as_array().expect("expected snapshots");
            assert!(
                !steps.is_empty() && steps.len() == expected.len(),
                "{}",
                case.id
            );
        } else {
            assert!(case.expected.is_object(), "{}", case.id);
        }
    }
    assert_eq!(
        operations,
        BTreeSet::from([
            "channel",
            "launch",
            "locale",
            "override",
            "protection",
            "timeZone",
            "trace"
        ])
    );
}

#[test]
fn frozen_egress_cases_drive_native_probe() {
    let fixture = fixture();
    let constants = &fixture.constants;
    assert_eq!(constants["slowThresholdMs"], SLOW_THRESHOLD_MS);
    assert_eq!(constants["defaultTimeoutMs"], DEFAULT_TIMEOUT_MS);
    assert_eq!(constants["frozenRecheckMs"], FROZEN_RECHECK_MS);
    assert_eq!(constants["confirmDelayMs"], RECOVERY_CONFIRM_MS);
    assert_eq!(fixture.cases.len(), 42);
    let epoch = millis(constants, "epochMs");
    let baseline: Exit = decode(&constants["baselineExit"]);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();

    for case in &fixture.cases {
        let input = &case.input;
        let actual = match string(input, "operation") {
            "trace" => run_trace(input, &baseline),
            "launch" => {
                let plan = launch_plan(
                    string(input, "profile"),
                    &decode(&input["env"]),
                    input["subscription"].as_str(),
                );
                json!(plan)
            }
            "channel" => run_channel(input, &baseline),
            "protection" => runtime.block_on(run_protection(input, epoch)),
            "override" => run_override(input, epoch),
            "timeZone" => runtime.block_on(run_time_zone(input, epoch, &baseline)),
            "locale" => runtime.block_on(run_locale(input, epoch)),
            operation => panic!("unsupported operation {operation}"),
        };
        if let (Some(actual_steps), Some(expected_steps)) =
            (actual.as_array(), case.expected.as_array())
        {
            assert_eq!(actual_steps.len(), expected_steps.len(), "{}", case.id);
            for (index, (actual_step, expected_step)) in
                actual_steps.iter().zip(expected_steps).enumerate()
            {
                assert_fixture_value(
                    actual_step,
                    expected_step,
                    &format!("{} step {}", case.id, index + 1),
                );
            }
        } else {
            assert_fixture_value(&actual, &case.expected, &case.id);
        }
    }
}

fn assert_fixture_value(actual: &Value, expected: &Value, context: &str) {
    let expected = expected.as_object().unwrap();
    let actual = actual.as_object().unwrap();
    assert_eq!(
        actual.keys().collect::<Vec<_>>(),
        expected.keys().collect::<Vec<_>>(),
        "{context} fields"
    );
    for (key, expected) in expected {
        assert_eq!(&actual[key], expected, "{context} / {key}");
    }
}

fn decode<T: DeserializeOwned>(value: &Value) -> T {
    serde_json::from_value(value.clone()).unwrap()
}

fn string<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key]
        .as_str()
        .unwrap_or_else(|| panic!("missing string {key}"))
}

fn millis(value: &Value, key: &str) -> u64 {
    value[key]
        .as_u64()
        .unwrap_or_else(|| panic!("missing milliseconds {key}"))
}

fn region_name(code: &str, locale: InterfaceLocale) -> Option<String> {
    // The fixture uses US for all localized region names. In the desktop the
    // same lookup is supplied by Intl.DisplayNames.
    (code == "US").then(|| locale.text("美国", "United States").into())
}

fn copy(locale: InterfaceLocale) -> EgressCopy<fn(&str, InterfaceLocale) -> Option<String>> {
    EgressCopy::new(locale, region_name)
}

fn zh() -> EgressCopy<fn(&str, InterfaceLocale) -> Option<String>> {
    copy(InterfaceLocale::Chinese)
}

fn check_without_family(check: &Check) -> Value {
    let mut value = json!(check);
    value.as_object_mut().unwrap().remove("family");
    value
}

fn run_trace(input: &Value, baseline: &Exit) -> Value {
    let family = Family::Anthropic;
    let (api, login) = family.hosts();
    let mut probe = TraceProbe::new(api, input["timeoutMs"].as_u64());
    let timeout = probe.timeout_ms();
    let event_at = millis(input, "eventAtMs");
    let event = match string(input, "event") {
        "response" => TraceEvent::Response {
            status: 200,
            body: string(input, "body").as_bytes(),
        },
        "socket-timeout" => TraceEvent::Timeout,
        "error" => TraceEvent::Error(TraceRequestError {
            code: input["code"].as_str(),
            ..TraceRequestError::default()
        }),
        name => panic!("unknown trace event {name}"),
    };
    if input["tie"] == "timer-first" {
        probe.observe(timeout, TraceEvent::Timeout);
    }
    let result = probe.observe(event_at, event).clone();
    if event_at == timeout {
        assert_eq!(probe.observe(timeout, TraceEvent::Timeout), &result);
    }

    // The second endpoint fails in error scenarios, so the family cannot rely
    // on a login fallback and the API cause is the first unknown fact.
    let login = if let Some(error) = result.sample.error {
        HostSample::failed(login, error)
    } else {
        HostSample::success(login, baseline.clone())
    };
    let sample = EgressSample::new(BTreeMap::from([(
        family,
        HostPair::new(family, result.sample.clone(), login),
    )]));
    let rule = EgressRule {
        baselines: BTreeMap::from([(family, baseline.clone())]),
        ..EgressRule::default()
    };
    let evaluation = evaluate_egress(&sample, &rule);
    let entry = &evaluation.families[0];
    let checks: Vec<_> = evaluation
        .checks
        .iter()
        .filter(|check| check.family == Some(family))
        .collect();
    let mainland = checks
        .iter()
        .find(|check| check.id == CheckId::Mainland)
        .unwrap();
    let details = [InterfaceLocale::Chinese, InterfaceLocale::English]
        .map(|locale| copy(locale).check(mainland, entry.exit.as_ref()).detail);
    json!({
        "classification": result.classification(),
        "requestTimeoutMs": timeout,
        "settledAtMs": result.elapsed_ms,
        "sample": result.sample,
        "familyState": entry.state,
        "state": evaluation.state,
        "checks": checks.into_iter().map(check_without_family).collect::<Vec<_>>(),
        "detail": details,
    })
}

fn host_sample(host: &str, input: &Value) -> HostSample {
    if input.is_string() {
        return HostSample::failed(host, decode(input));
    }
    if let Some(error) = input.get("error") {
        return HostSample::failed(host, decode(error));
    }
    let ip = input["ip"]
        .as_str()
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let country = input["country"]
        .as_str()
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    if ip.is_none() && country.is_none() {
        return HostSample::failed(host, HostError::Timeout);
    }
    HostSample {
        ip,
        country,
        ..HostSample::new(host)
    }
}

fn host_map(
    hosts: &Value,
    pair_inputs: impl Fn(&Value) -> (&Value, &Value),
) -> BTreeMap<Family, HostPair> {
    hosts
        .as_object()
        .unwrap()
        .iter()
        .map(|(family, input)| {
            let family: Family = decode(&json!(family));
            let (api, login) = family.hosts();
            let (api_input, login_input) = pair_inputs(input);
            (
                family,
                HostPair::new(
                    family,
                    host_sample(api, api_input),
                    host_sample(login, login_input),
                ),
            )
        })
        .collect()
}

fn protection_sample(hosts: &Value) -> EgressSample {
    EgressSample::new(host_map(hosts, |input| (input, input)))
}

fn run_channel(input: &Value, baseline: &Exit) -> Value {
    let mut full_probe_hosts = Vec::new();
    let (sample, router_light) = if let Some(hosts) = input.get("fixtureHosts") {
        let sample =
            EgressSample::from_recording(host_map(hosts, |input| (&input["api"], &input["login"])));
        let light = sample.recorded_light(Family::ChatgptSubscription);
        (sample, light)
    } else {
        let plan = SamplingPlan {
            subscription: if input["routerEnabled"].as_bool().unwrap() {
                SubscriptionRoute::Enabled
            } else {
                SubscriptionRoute::Disabled
            },
        };
        let samples = plan
            .full_targets()
            .map(|target| {
                full_probe_hosts.push(target.host);
                (target, HostSample::success(target.host, baseline.clone()))
            })
            .collect();
        let light = plan
            .light_target(Family::ChatgptSubscription)
            .map(|target| HostSample::success(target.host, baseline.clone()))
            .unwrap_or_else(|failure| failure);
        (plan.collect_full(samples), light)
    };
    full_probe_hosts.sort_unstable();
    let mut monitor = EgressMonitor::default();
    monitor.apply_full_sample(sample, 0);
    let status = monitor.status();
    let families: Vec<_> = status
        .families
        .iter()
        .map(|entry| {
            let family = entry.evaluation.family;
            let checks = status
                .checks
                .iter()
                .filter(|check| check.family == Some(family))
                .map(|check| check.id)
                .collect::<Vec<_>>();
            (family, entry.evaluation.state, checks)
        })
        .collect();
    let google_hosts = status
        .families
        .iter()
        .find(|entry| entry.evaluation.family == Family::Google)
        .map(|entry| &entry.hosts);
    let locale = zh();
    let mut headings = status
        .families
        .iter()
        .map(|entry| locale.family_heading(entry.evaluation.family))
        .collect::<Vec<_>>();
    headings.push(locale.local_heading());
    json!({
        "state": status.state,
        "families": families,
        "googleHosts": google_hosts,
        "fullProbeHosts": full_probe_hosts,
        "routerLight": router_light,
        "headings": headings,
        "note": locale.sampling_note(),
    })
}

struct FixtureClock(u64);

impl EgressClock for FixtureClock {
    fn now_ms(&self) -> u64 {
        self.0
    }
}

#[derive(Default)]
struct FixtureSampler {
    current: EgressSample,
    full_checks: usize,
}

impl EgressSampler for FixtureSampler {
    type Error = Infallible;

    fn sample_full(&mut self) -> impl Future<Output = Result<EgressSample, Self::Error>> + Send {
        self.full_checks += 1;
        ready(Ok(self.current.clone()))
    }

    fn sample_light(
        &mut self,
        family: Family,
    ) -> impl Future<Output = Result<HostSample, Self::Error>> + Send {
        ready(Ok(self.current.recorded_light(family)))
    }
}

fn register_sessions(monitor: &mut EgressMonitor, sessions: &Value, now: u64) {
    for (id, value) in sessions.as_object().unwrap() {
        let families = match value {
            Value::Null => Vec::new(),
            Value::Array(_) => decode(value),
            _ => vec![decode(value)],
        };
        assert!(!monitor.register_session(id.clone(), &families, now));
    }
}

fn card(status: &Status, id: &str) -> Value {
    let view = zh().card(status, id);
    json!([view.family, view.frozen, view.kind, view.message])
}

fn cards(status: &Status) -> BTreeMap<&str, Value> {
    status
        .session_families
        .keys()
        .map(|id| (id.as_str(), card(status, id)))
        .collect()
}

fn effects(effects: &[SessionEffect]) -> Vec<String> {
    effects
        .iter()
        .map(|effect| match effect {
            SessionEffect::Freeze(id) => format!("freeze:{id}"),
            SessionEffect::Thaw(id) => format!("thaw:{id}"),
        })
        .collect()
}

async fn run_protection(input: &Value, epoch: u64) -> Value {
    let mut clock = FixtureClock(epoch);
    let mut sampler = FixtureSampler::default();
    let mut monitor = EgressMonitor::default();
    register_sessions(&mut monitor, &input["sessions"], epoch);
    let mut snapshots = Vec::new();
    for step in input["steps"].as_array().unwrap() {
        clock.0 = epoch + millis(step, "atMs");
        let now = clock.now_ms();
        if let Some(hosts) = step.get("hosts") {
            sampler.current = protection_sample(hosts);
        }
        let previous = monitor.status();
        let mut action_result = None;
        let actions = match string(step, "action") {
            "snapshot" => Vec::new(),
            "view" => {
                action_result = Some(card(&monitor.status(), string(step, "id")));
                Vec::new()
            }
            "register" => {
                let born_frozen = monitor.register_session(
                    string(step, "id").into(),
                    &[decode(&step["family"])],
                    now,
                );
                action_result = Some(json!({ "bornFrozen": born_frozen }));
                Vec::new()
            }
            "check" => monitor.recheck(&clock, &mut sampler).await.unwrap(),
            "advance" => monitor
                .recheck_if_due(&clock, &mut sampler)
                .await
                .unwrap()
                .unwrap_or_default(),
            "rule" => monitor.set_rule_mode(decode(&step["mode"]), now),
            "protect" => monitor.set_protection(step["enabled"].as_bool().unwrap(), now),
            "accept" => match monitor.accept_current(decode(&step["family"]), now) {
                Ok(actions) => actions,
                Err(error) => {
                    action_result = Some(json!(error));
                    Vec::new()
                }
            },
            action => panic!("unsupported action {action}"),
        };
        let status = monitor.status();
        let families: BTreeMap<_, _> = status
            .families
            .iter()
            .map(|entry| {
                (
                    entry.evaluation.family,
                    json!([
                        entry.evaluation.state,
                        entry.evaluation.violation,
                        entry.frozen_session_ids,
                        entry.recovering,
                        entry.overridden,
                    ]),
                )
            })
            .collect();
        let transition = status.last_transition.as_ref().map(|transition| {
            json!([
                transition.kind,
                transition.family,
                transition.session_count,
                transition.at - epoch,
                transition.reason
            ])
        });
        let mut snapshot = json!({
            "state": status.state,
            "checkedAtMs": status.checked_at.map(|time| time - epoch),
            "protectionEnabled": status.protection_enabled,
            "ruleMode": status.rule_mode,
            "families": families,
            "cards": cards(&status),
            "effects": effects(&actions),
            "nextCheckInMs": monitor.recovery_deadline().map(|deadline| deadline - now),
            "fullChecks": sampler.full_checks,
            "transition": transition,
            "notification": zh().notification(&previous, &status),
        });
        if let Some(action_result) = action_result {
            snapshot["actionResult"] = action_result;
        }
        snapshots.push(snapshot);
    }
    json!(snapshots)
}

fn override_snapshot(status: &Status, actions: &[SessionEffect]) -> Value {
    let overridden: Vec<_> = status
        .families
        .iter()
        .filter(|entry| entry.overridden)
        .map(|entry| entry.evaluation.family)
        .collect();
    json!({ "cards": cards(status), "effects": effects(actions), "overridden": overridden })
}

fn run_override(input: &Value, epoch: u64) -> Value {
    let mut monitor = EgressMonitor::default();
    register_sessions(&mut monitor, &input["sessions"], epoch);
    let sample = protection_sample(&input["hosts"]);
    let mut actions = monitor.apply_full_sample(sample.clone(), epoch);
    let locale = zh();
    let family = locale.card(&monitor.status(), "a").family.unwrap();
    let mut dialog = Some(OverridePrompt::new(family));
    let canceled_outcome = dialog
        .take()
        .unwrap()
        .respond(OverrideDecision::Cancel, &mut monitor, epoch + 1)
        .unwrap();
    actions.extend(canceled_outcome.effects);
    let mut canceled = override_snapshot(&monitor.status(), &actions);
    canceled["dialogClosed"] = json!(dialog.is_none());
    canceled["overrideCalls"] = json!(
        canceled_outcome
            .applied_family
            .into_iter()
            .collect::<Vec<_>>()
    );

    let mut dialog = Some(OverridePrompt::new(family));
    let confirmed_outcome = dialog
        .take()
        .unwrap()
        .respond(OverrideDecision::Confirm, &mut monitor, epoch + 2)
        .unwrap();
    let mut confirmed = override_snapshot(&monitor.status(), &confirmed_outcome.effects);
    confirmed["dialogClosed"] = json!(dialog.is_none());
    confirmed["overrideCalls"] = json!(
        confirmed_outcome
            .applied_family
            .into_iter()
            .collect::<Vec<_>>()
    );

    let actions = monitor.apply_full_sample(sample, epoch + 3);
    let same_exit = override_snapshot(&monitor.status(), &actions);
    let actions = monitor.apply_full_sample(protection_sample(&input["nextHosts"]), epoch + 4);
    let changed_exit = override_snapshot(&monitor.status(), &actions);
    json!({
        "initialButtons": locale.freeze_buttons(),
        "confirmation": locale.override_confirmation(),
        "canceled": canceled,
        "confirmed": confirmed,
        "sameExit": same_exit,
        "changedExit": changed_exit,
    })
}

fn time_zone_environment(zone: Option<&str>) -> BTreeMap<&str, &str> {
    zone.map(|zone| BTreeMap::from([("TZ", zone)]))
        .unwrap_or_default()
}

async fn run_time_zone(input: &Value, epoch: u64, baseline: &Exit) -> Value {
    let mut monitor = EgressMonitor::default();
    let profile = string(input, "profile");
    let system = string(input, "systemTimeZone");
    let country = string(input, "exitCountry");
    let plan = launch_plan(profile, &BTreeMap::new(), None);
    let family = plan.family.unwrap();
    let mut preference = TimeZonePreference::default();
    let (api, login) = family.hosts();
    let exit = Exit {
        country: country.into(),
        ip: baseline.ip.clone(),
    };
    let mut sample = EgressSample::new(BTreeMap::from([(
        family,
        HostPair::new(
            family,
            HostSample::success(api, exit.clone()),
            HostSample::success(login, exit),
        ),
    )]));
    sample.agent_time_zone_mainland = preference.mainland_warning(system);
    let mut sampler = FixtureSampler {
        current: sample,
        ..FixtureSampler::default()
    };
    monitor.register_session("session".into(), &[family], epoch);
    monitor
        .recheck(&FixtureClock(epoch), &mut sampler)
        .await
        .unwrap();

    let active =
        time_zone_environment(preference.launch_zone(profile, system, Some(country), plan));
    let active = json!(active);
    let previous = monitor.status();
    let before_save = sampler.full_checks;
    let result = preference
        .set(string(input, "savedZone"))
        .unwrap_or_else(|error| error)
        .to_owned();
    let same_status = previous == monitor.status();
    let checks_on_save = sampler.full_checks - before_save;
    let next = time_zone_environment(preference.launch_zone(profile, system, Some(country), plan));

    sampler.current.agent_time_zone_mainland = preference.mainland_warning(system);
    let actions = monitor
        .recheck(&FixtureClock(epoch + 1), &mut sampler)
        .await
        .unwrap();
    let status = monitor.status();
    let check = status
        .checks
        .iter()
        .find(|check| check.id == CheckId::TimeZone)
        .unwrap();
    json!({
        "activeEnv": active,
        "saveResult": result,
        "sameStatusOnSave": same_status,
        "checksOnSave": checks_on_save,
        "nextLaunchEnv": next,
        "timeZoneCheck": check.result,
        "state": status.state,
        "frozen": zh().card(&status, "session").frozen,
        "effects": effects(&actions),
    })
}

async fn run_locale(input: &Value, epoch: u64) -> Value {
    let mut monitor = EgressMonitor::default();
    monitor.register_session("a".into(), &[Family::Google], epoch);
    let mut sampler = FixtureSampler {
        current: protection_sample(&json!({ "google": input["exit"] })),
        ..FixtureSampler::default()
    };
    monitor
        .recheck(&FixtureClock(epoch), &mut sampler)
        .await
        .unwrap();
    let previous = monitor.status();
    let checks = sampler.full_checks;
    let system = input["system"].as_str();
    let forced = input["forced"].as_str();
    let initial = resolve_interface_locale(LocalePreference::System, forced, system);
    let stored = LocalePreference::from_value(string(input, "preference")).unwrap();
    let effective = resolve_interface_locale(stored, forced, system);
    let next = monitor.status();
    let view = copy(effective).card(&next, "a");
    json!({
        "initial": initial.value(),
        "stored": stored.value(),
        "effective": effective.value(),
        "changed": [effective.value()],
        "sameStatus": previous == next,
        "checksOnSave": sampler.full_checks - checks,
        "frozen": view.frozen,
        "bannerBefore": copy(initial).card(&previous, "a").message,
        "bannerAfter": view.message,
        "effects": Vec::<String>::new(),
    })
}
