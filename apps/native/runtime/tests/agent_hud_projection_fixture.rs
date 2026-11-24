use matou_native_core::sessions::InterfaceLocale;
use matou_native_runtime::agent_hud_projection::{
    AgentHudProjection, HudProvider, TaskStatus, project_teammates,
};
use serde_json::{Value, json};

// The same frozen bytes loaded by tests/provider_events_fixture.rs.
const FIXTURE: &str =
    include_str!("../../../../tooling/native-parity/fixtures/provider-events-reference.json");

#[test]
fn provider_hook_fixture_replays_every_step_at_the_hud_ingress_boundary() {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    let mut cases = 0;
    let mut steps = 0;
    for case in fixture["cases"].as_array().unwrap() {
        let input = &case["input"];
        if input["operation"] != "provider-hook" {
            continue;
        }
        cases += 1;
        let provider = match input["provider"].as_str().unwrap() {
            "claude-code" => HudProvider::ClaudeCode,
            "codex" => HudProvider::Codex,
            "antigravity" => HudProvider::Antigravity,
            "dsh" => HudProvider::Dsh,
            provider => panic!("Unexpected provider {provider}"),
        };
        let mut now = input["now"].as_f64().unwrap();
        let mut projection = AgentHudProjection::new(
            input["registration"]["sessionId"].as_str().unwrap(),
            provider,
            now,
        );
        let mut transcripts = serde_json::Map::new();
        for (index, (step, expected)) in input["steps"]
            .as_array()
            .unwrap()
            .iter()
            .zip(case["expected"]["steps"].as_array().unwrap())
            .enumerate()
        {
            steps += 1;
            now += step["advanceMs"].as_f64().unwrap_or_default();
            if let Some(next) = step["transcripts"].as_object() {
                transcripts.extend(next.clone());
            }
            let before = projection.snapshot(now);
            if expected["hudPublishes"] == 1 {
                let payload = &step["payload"];
                assert!(payload.is_object());
                if provider == HudProvider::Dsh {
                    // The hook's separately captured work status reaches the registry
                    // as a normalized payload after the DSH reducer accepts the event.
                    let normalized = json!({
                        "dsh_session_id": payload["session_id"],
                        "dsh_hud": payload["hud"],
                        "dsh_work_status": expected["providerWorkStatuses"][0],
                    });
                    projection.ingest_provider(&normalized, now);
                } else {
                    projection.ingest_provider(payload, now);
                    let path = payload["transcript_path"]
                        .as_str()
                        .or(payload["transcriptPath"].as_str());
                    if let Some(rows) = path.and_then(|path| transcripts.get(path)?.as_array()) {
                        projection.refresh_transcript(rows);
                    }
                }
            } else {
                assert_eq!(expected["hudPublishes"], 0);
                assert_eq!(projection.snapshot(now), before);
            }
            assert_eq!(
                serde_json::to_value(projection.snapshot(now).task_status).unwrap(),
                expected["hudTaskStatus"],
                "case {} step {}",
                case["id"],
                index + 1,
            );
        }
    }
    assert!(
        cases >= 20 && steps >= 50,
        "{cases} cases and {steps} steps"
    );
}

#[test]
fn claude_hook_projects_complete_requested_snapshot_and_tool_transitions() {
    let mut projection = AgentHudProjection::new("s", HudProvider::ClaudeCode, 1000.);
    projection.seed_settings(Some("/old"), Some("auto"), Some("sonnet"), None, false);
    projection.update_permission("plan", true);
    projection.ingest_provider(&json!({"permission_mode":"auto"}), 1001.);
    assert_eq!(projection.snapshot(1001.).permission_mode, Some("plan"));
    let payloads = [
        json!({"cwd":"/new", "model":{"display_name":"Sample Sonnet", "id":"fallback"},
            "permissionMode":"plan_mode", "context_window":{"used_percentage":19.5,"context_window_size":1000}}),
        json!({"hook_event_name":"PreToolUse","tool_name":"TodoWrite","tool_use_id":"todo",
            "tool_input":{"todos":[{"content":" first ","status":"running"},{"subject":"Done","status":"done"},null,{"content":" "}]}}),
        json!({"hook_event_name":"PostToolUse","tool_name":"TodoWrite","tool_use_id":"todo"}),
        json!({"hookEventName":"PreToolUse","toolName":"TaskCreate","toolUseId":"create","toolInput":{"description":"new"}}),
        json!({"hookEventName":"PostToolUse","toolName":"TaskCreate","toolUseId":"create"}),
        json!({"hook_event_name":"PreToolUse","tool_name":"TaskUpdate","tool_use_id":"update",
            "tool_input":{"task_id":"1","subject":"first revised","status":"completed"}}),
        json!({"hook_event_name":"PostToolUse","tool_name":"TaskUpdate","tool_use_id":"update"}),
        json!({"hook_event_name":"PreToolUse","tool_name":"mcp__sample__open","tool_use_id":"mcp"}),
        json!({"hook_event_name":"PostToolUseFailure","tool_name":"mcp__sample__open","tool_use_id":"mcp"}),
        json!({"hook_event_name":"PreToolUse","tool_name":"Agent","tool_use_id":"agent","tool_input":{"name":" Reviewer "}}),
        json!({"hook_event_name":"PreToolUse","tool_name":"Agent","tool_use_id":"agent","tool_input":{"name":" Reviewer "}}),
        json!({"hook_event_name":"PreToolUse","toolCall":{"id":"read","name":"Read","args":{"file_path":" /new/a "}}}),
    ];
    for payload in payloads {
        projection.ingest_provider(&payload, 1002.);
    }
    assert_eq!(
        serde_json::to_value(projection.snapshot(1002.)).unwrap(),
        json!({
            "sessionId":"s", "mode":"agent", "provider":"claude-code", "startedAt":1000.0,
            "taskStatus":"running", "resumable":false, "cwd":"/new", "permissionMode":"plan",
            "modelStrategy":"claude-sonnet-4-6", "model":"Sample Sonnet",
            "contextPercent":20.0, "contextWindowSize":1000.0,
            "runningTools":[{"name":"Agent"},{"name":"Read","target":"/new/a"}],
            "toolCounts":[{"name":"TodoWrite","count":1},{"name":"TaskCreate","count":1},
                {"name":"TaskUpdate","count":1},{"name":"mcp__sample__open","count":1}],
            "lastTool":{"name":"Read","target":"/new/a","status":"running"},
            "mcpErrors":["sample"],
            "todos":[{"content":"first revised","status":"completed"},{"content":"Done","status":"completed"},
                {"content":"new","status":"pending"}],
            "agents":[{"id":"agent","label":"Reviewer","source":"claude","state":"running","startedAt":1002.0}],
            "subagentCount":1, "subagents":["Reviewer"]
        })
    );
    projection.ingest_provider(
        &json!({"hook_event_name":"PostToolUseFailure","tool_name":"Read","tool_use_id":"read"}),
        1003.,
    );
    assert_eq!(
        serde_json::to_value(projection.snapshot(1003.).last_tool).unwrap(),
        json!({"name":"Read","target":"/new/a","status":"error"})
    );
    projection.ingest_provider(&json!({"hook_event_name":"SessionEnd"}), 1004.);
    let hud = projection.snapshot(1004.);
    assert_eq!(hud.task_status, TaskStatus::Idle);
    assert!(hud.running_tools.unwrap().is_empty());
    assert!(hud.agents.unwrap().is_empty());
    assert_eq!(hud.subagent_count, Some(0));
    assert!(hud.subagents.unwrap().is_empty());
}

#[test]
fn claude_transcript_deduplicates_completions_and_projects_async_agent_status() {
    const NOW: f64 = 1_792_033_200_000.;
    let stamp = "2026-10-15T03:00:00.000Z";
    let mut projection = AgentHudProjection::new("lead", HudProvider::ClaudeCode, NOW);
    let spawn = json!({"type":"assistant","timestamp":stamp,"message":{"model":"sample",
        "content":[{"type":"tool_use","id":"a","name":"Agent","input":{"description":"review"}}]}});
    let result = json!({"type":"user","timestamp":stamp,"toolUseResult":{"isAsync":true,"agentId":"worker"},
        "message":{"content":[{"type":"tool_result","tool_use_id":"a"}]}});
    let failed = json!({"type":"user","timestamp":stamp,"message":{"content":[{
        "type":"text","text":"<task-notification><tool-use-id>a</tool-use-id><status>failed</status><summary>failure\nnext line</summary></task-notification>"
    }]}});
    let wake = json!({"type":"assistant","timestamp":"2026-10-15T03:00:10.000Z","message":{
        "content":[{"type":"tool_use","id":"send","name":"SendMessage","input":{"to":"worker"}}]
    }});
    let cancelled = json!({"type":"user","message":{"content":
        "<task-notification><tool-use-id>a</tool-use-id><status>cancelled</status></task-notification>"
    }});
    let mut rows = vec![spawn, result.clone(), result, failed];
    projection.refresh_transcript(&rows);
    let hud = projection.snapshot(NOW);
    assert_eq!(
        serde_json::to_value(hud.tool_counts).unwrap(),
        json!([{"name":"Agent","count":1}])
    );
    assert_eq!(
        serde_json::to_value(hud.agents).unwrap(),
        json!([
            {"id":"a","label":"review","source":"claude","state":"error","startedAt":NOW,"detail":"failure"}
        ])
    );
    assert_eq!(projection.snapshot(NOW + 300_000.).subagent_count, Some(0));
    rows.push(wake);
    projection.refresh_transcript(&rows);
    assert_eq!(
        serde_json::to_value(projection.snapshot(NOW + 10_000.).agents).unwrap(),
        json!([
            {"id":"a","label":"review","source":"claude","state":"running","startedAt":NOW+10_000.}
        ])
    );
    rows.push(cancelled);
    projection.refresh_transcript(&rows);
    assert_eq!(projection.snapshot(NOW + 10_000.).subagent_count, Some(0));
}

#[test]
fn teammate_transcript_projects_full_rows_and_deduplicates_repeat_observations() {
    // Frozen provider-hook-server.test.ts:602–646, with duplicate rows and both message shapes.
    let spawn = json!({"type":"user","toolUseResult":{
        "status":"teammate_spawned","teammate_id":"MATOU_QA_TEAMMATE@session-real",
        "agent_id":"MATOU_QA_TEAMMATE@session-real", "name":"MATOU_QA_TEAMMATE",
        "team_name":"session-real","prompt":"Reply exactly TEAMMATE_REAL_READY"
    }});
    let message = json!({"type":"queue-operation","operation":"enqueue",
        "content":"<agent-message from=\"MATOU_QA_TEAMMATE\">\nTEAMMATE_REAL_READY\n</agent-message>"
    });
    let listing = json!({"type":"user","toolUseResult":{
        "listing":"Teammates (1):\n  MATOU_QA_TEAMMATE [96bcfc]  ·  idle  ·  started 8s ago"
    }});
    let mut rows = vec![spawn.clone(), spawn, message.clone(), message, listing];
    let project = |rows: &[Value], locale| {
        serde_json::to_value(project_teammates("run-team", "session-1", rows, locale)).unwrap()
    };
    assert_eq!(
        project(&rows[..2], InterfaceLocale::English)[0]["workStatus"],
        "running"
    );
    let mut expected = json!([{
        "runId":"run-team", "leadSessionId":"session-1", "teammateId":"MATOU_QA_TEAMMATE@session-real",
        "teamId":"session-real", "name":"MATOU_QA_TEAMMATE", "workStatus":"idle",
        "latestLines":["Reply exactly TEAMMATE_REAL_READY","TEAMMATE_REAL_READY"]
    }]);
    assert_eq!(project(&rows, InterfaceLocale::English), expected);
    assert_eq!(project(&rows, InterfaceLocale::English), expected);
    rows.push(json!({"message":{"content":[{"text":concat!(
        "<teammate-message teammate_id=\"MATOU_QA_TEAMMATE@session-real\">third</teammate-message>",
        "<teammate-message teammate_id=\"MATOU_QA_TEAMMATE@session-real\">fourth</teammate-message>",
        "<teammate-message teammate_id=\"MATOU_QA_TEAMMATE@session-real\">{\"type\":\"idle_notification\"}</teammate-message>"
    )}]}}));
    expected[0]["latestLines"] = json!([
        "TEAMMATE_REAL_READY",
        "third",
        "fourth",
        "A teammate finished the current task"
    ]);
    assert_eq!(project(&rows, InterfaceLocale::English), expected);
    expected[0]["latestLines"][3] = json!("队友已完成当前任务");
    assert_eq!(project(&rows, InterfaceLocale::Chinese), expected);
}

#[test]
fn antigravity_hud_projects_model_quota_hook_and_transcript_activity() {
    let now = 1_795_262_400_000.;
    let mut projection = AgentHudProjection::new("agy", HudProvider::Antigravity, now);
    assert!(projection.snapshot(now).usage_windows.is_none());
    projection.ingest_provider(
        &json!({
            "model":{"display_name":"Gemini Pro", "id":"gemini-pro"}, "cycle_mode":"ask",
            "quota":{
                "gemini-5h":{"remaining_fraction":0.75,"reset_time":(now + 3_600_000.) / 1000.},
                "gemini-weekly":{"remainingFraction":1.0},
                "pro-weekly":{"remainingFraction":0.6},
                "3p-5h":{"remaining_fraction":0.5},
                "invalid":{"reset_time":"bad"}
            }
        }),
        now,
    );
    let hud = serde_json::to_value(projection.snapshot(now)).unwrap();
    assert_eq!(hud["permissionMode"], "ask");
    assert_eq!(
        hud["usageWindows"],
        json!([
            {"id":"gemini-5h","label":"5h","usedPercent":25.0,"resetText":"1h 0m"},
            {"id":"pro-weekly","label":"Weekly","usedPercent":40.0,"resetText":null}
        ])
    );
    assert_eq!(hud["allUsageWindows"].as_array().unwrap().len(), 4);
    projection.ingest_provider(
        &json!({"model":{"id":"claude-sonnet"},"cycle_mode":"plan"}),
        now,
    );
    let hud = projection.snapshot(now);
    assert_eq!(hud.usage_windows.unwrap()[0].id, "3p-5h");
    assert_eq!(hud.all_usage_windows.unwrap().len(), 4);

    projection.refresh_transcript(&[json!({
        "created_at": now, "type":"PLANNER_RESPONSE","source":"MODEL","tool_calls":[
            {"id":"plan","name":"update_plan","arguments":"{\"plan\":[{\"step\":\"Collect evidence\",\"status\":\"completed\"}]}"},
            {"id":"task","name":"TaskCreate","arguments":{"subject":"Review"}},
            {"id":"task-update","name":"TaskUpdate","arguments":{"taskId":"2","status":"in_progress"}},
            {"id":"agent","name":"spawn_agent","arguments":{"description":"Reviewer"}}
        ]
    })]);
    let hud = serde_json::to_value(projection.snapshot(now)).unwrap();
    assert_eq!(
        hud["todos"],
        json!([
            {"content":"Collect evidence","status":"completed"},
            {"content":"Review","status":"in_progress"}
        ])
    );
    assert_eq!(hud["agents"][0]["label"], "Reviewer");
    projection.ingest_provider(&json!({
        "hookEventName":"PostToolUseFailure","toolCall":{"name":"spawn_agent","id":"agent"},"error":"unavailable"
    }), now + 1.);
    let hud = serde_json::to_value(projection.snapshot(now + 1.)).unwrap();
    assert_eq!(hud["agents"][0]["state"], "error");
    projection.ingest_provider(
        &json!({
            "hookEventName":"PreToolUse","toolCall":{"id":"todos","name":"TodoWrite","args":{
                "todos":[{"content":"Replacement","status":"pending"}]
            }}
        }),
        now + 2.,
    );
    let hud = serde_json::to_value(projection.snapshot(now + 2.)).unwrap();
    assert_eq!(hud["todos"][0]["content"], "Replacement");
}

#[test]
fn claude_status_line_accepts_rate_limits_and_explicit_usage_windows() {
    let mut projection = AgentHudProjection::new("claude", HudProvider::ClaudeCode, 0.);
    projection.ingest_provider(
        &json!({"rateLimits":{
            "fiveHour":{"usedPercentage":0},
            "seven_day":{"used_percentage":25},
            "modelScoped":[{"displayName":"Opus","utilization":58}]
        }}),
        0.,
    );
    let windows = projection.snapshot(0.).usage_windows.unwrap();
    assert_eq!(
        windows
            .iter()
            .map(|window| window.used_percent)
            .collect::<Vec<_>>(),
        [0., 25., 58.]
    );
    projection.ingest_provider(
        &json!({"usageWindows":[
            {"id":"id","label":"Provider window","usedPercent":12,"resetText":"7m"}
        ]}),
        0.,
    );
    let windows = projection.snapshot(0.).usage_windows.unwrap();
    assert_eq!(windows[0].id, "id");
    assert_eq!(windows[0].reset_text.as_deref(), Some("7m"));
}

#[test]
fn dsh_hud_projects_external_foreman_agents_only_when_present() {
    let mut projection = AgentHudProjection::new("dsh", HudProvider::Dsh, 0.);
    let idle = projection.snapshot(1000.);
    assert!(idle.agents.is_none());
    assert!(idle.subagent_count.is_none());

    projection.set_external_agents(vec![
        matou_native_runtime::agent_hud_projection::AgentActivity {
            id: "codex-demo-01".into(),
            label: "demo-01".into(),
            state: matou_native_runtime::agent_hud_projection::AgentState::Running,
            source: matou_native_runtime::agent_hud_projection::AgentSource::External,
            started_at: Some(1000.),
            detail: Some("sol · high".into()),
        },
    ]);
    let active = projection.snapshot(2000.);
    assert_eq!(active.subagent_count, Some(1));
    assert_eq!(active.agents.as_ref().map(Vec::len), Some(1));
    assert_eq!(active.agents.as_ref().unwrap()[0].id, "codex-demo-01");

    projection.set_external_agents(Vec::new());
    let cleared = projection.snapshot(3000.);
    assert!(cleared.agents.is_none());
    assert!(cleared.subagent_count.is_none());
}
