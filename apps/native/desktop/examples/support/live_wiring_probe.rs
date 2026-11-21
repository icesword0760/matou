use super::*;
use crate::runtime_bridge::{ProbeReceiver, probe_channel};
use matou_native_desktop::{egress_panel::EgressAction, update_popover::UpdateAction};
use matou_native_runtime::{
    dsh_catalog::encode_project_path,
    egress_probe::{EgressSample, Family, HostPair, HostSample},
    host::{LaunchOptions, OfflineFixtures, RuntimeHost},
};
use std::{
    collections::{BTreeMap, VecDeque},
    ffi::OsString,
    fs,
    io::{Read, Write},
    net::{Ipv4Addr, TcpStream},
    os::unix::fs::PermissionsExt,
    path::Path,
    time::Instant,
};

const SOURCE: &str = "01a1ddc1-88fd-7fc1-8c66-ab8d38719da8";
const FORK: &str = "01a1ddc1-e832-7cf2-83b7-5adb2f12224b";
const CANARY: &str = "wave4a-fake-access-token";
const SAVED: &str = "dsh-saved";

struct Fixture {
    root: PathBuf,
    host: RuntimeHost,
    options: LaunchOptions,
    window: Handle,
    commands: ProbeReceiver,
    replies: Vec<Value>,
    identities: Vec<Value>,
}

impl Fixture {
    fn new(cx: &mut AsyncApp) -> Self {
        // Keep the absolute socket path under macOS's sockaddr_un length limit.
        let target = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../target")
            .canonicalize()
            .unwrap();
        let id = uuid::Uuid::new_v4().simple().to_string();
        let root = target.join(&id[..5]);
        fs::create_dir(&root).unwrap();
        let workspace = root.join("workspace");
        let shell_home = root.join("home");
        let data_root = root.clone();
        let codex_home = shell_home.join("codex");
        let claude_config = shell_home.join("claude");
        let dsh_history = root.join("dsh-history");
        for path in [
            &workspace,
            &shell_home,
            &data_root,
            &codex_home.join("sessions"),
            &claude_config.join("projects/fixture"),
        ] {
            fs::create_dir_all(path).unwrap();
        }
        let workspace = workspace.canonicalize().unwrap();
        fs::write(shell_home.join(".zshrc"), "PROMPT='FIXTURE> '\n").unwrap();
        fs::write(data_root.join("accounts.json"), json!({
            "currentId": "account-1",
            "accounts": [{
                "id":"account-1", "name":CANARY, "email":"private@example.test",
                "plan":"pro", "claudeAiOauth":{"accessToken":CANARY, "refreshToken":"wave4a-fake-refresh-token"}
            }]
        }).to_string()).unwrap();

        git(&workspace, &["init", "-b", "main"]);
        git(
            &workspace,
            &[
                "-c",
                "user.name=Probe",
                "-c",
                "user.email=probe@example.test",
                "commit",
                "--allow-empty",
                "-m",
                "offline probe",
            ],
        );
        let codex = root.join("codex-fixture");
        executable(
            &codex,
            r#"#!/bin/sh
name=source
for arg in "$@"; do
    [ "$arg" = fork ] && name=fork
done
printf '%s\n' "$@" > "$CODEX_HOME/$name-argv"
printf 'CODEX FIXTURE READY\n'
while IFS= read -r line; do
    printf '%s\n' "$line" >> "$CODEX_HOME/$name-input"
done
"#,
        );
        let claude = root.join("claude-fixture");
        executable(
            &claude,
            r#"#!/bin/sh
while [ "$#" -gt 0 ]; do
    if [ "$1" = --settings ]; then
        printf '%s\n' "$2" > "$HOME/claude-settings"
        break
    fi
    shift
done
printf 'CLAUDE FIXTURE READY\n'
exec /bin/cat
"#,
        );
        let dsh = root.join("dsh-fixture");
        executable(
            &dsh,
            r#"#!/bin/sh
if [ "${2:-}" = --dump-config ]; then
    printf '%s\n' '- id: dsh-tui' '  config:' '    backend: dsh'
    exit 0
fi
test "$1" = dsh-tui && test "$2" = --patch || exit 1
printf '%s\n' "$@" > "$HOME/dsh-argv"
/bin/cat "$3" > "$HOME/dsh-patch"
printf 'DSH FIXTURE READY\n'
exec /bin/cat
"#,
        );
        let browser = root.join("Browser.app/Contents/MacOS/browser");
        fs::create_dir_all(browser.parent().unwrap()).unwrap();
        executable(&browser, "#!/bin/sh\nexit 99\n");
        fs::write(root.join("Browser.app/Contents/Info.plist"), r#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>CFBundleShortVersionString</key><string>7.2.1</string></dict></plist>"#).unwrap();
        let saved = dsh_history
            .join(encode_project_path(workspace.to_str().unwrap()))
            .join(SAVED);
        fs::create_dir_all(&saved).unwrap();
        fs::write(saved.join("session.v4.jsonl"), format!("{}\n{}\n",
            json!({"type":"session","version":4,"id":SAVED,"cwd":workspace,"createdAt":1000}),
            json!({"type":"user/message","seq":1,"surfaceOp":"append","data":{"content":[{"text":"needle conversation"}]}}),
        )).unwrap();

        let environment: BTreeMap<OsString, OsString> = [
            ("PATH", "/usr/bin:/bin".into()),
            ("HOME", shell_home.clone().into_os_string()),
            ("CODEX_HOME", codex_home.into_os_string()),
            ("MATOU_CODEX_COMMAND", codex.into_os_string()),
            ("CLAUDE_CONFIG_DIR", claude_config.into_os_string()),
            ("MATOU_CLAUDE_COMMAND", claude.into_os_string()),
            ("MATOU_DSH_COMMAND", dsh.into_os_string()),
        ]
        .into_iter()
        .map(|(key, value)| (key.into(), value))
        .collect();
        let options = LaunchOptions {
            data_root,
            workspace,
            shell_home,
            shell: "/bin/zsh".into(),
        };
        let mut samples =
            VecDeque::from([sample("203.0.113.1", "US"), sample("203.0.113.2", "CN")]);
        let host = RuntimeHost::start_with_offline_fixtures(
            options.clone(),
            OfflineFixtures {
                provider_environment: Some(environment),
                dsh_session_root: Some(dsh_history),
                browser_executable: Some(browser),
                egress_sample: Some(Box::new(move || {
                    samples
                        .pop_front()
                        .ok_or_else(|| anyhow::anyhow!("Probe samples exhausted"))
                })),
                ..Default::default()
            },
        )
        .unwrap();
        let (window, commands) = cx
            .update(|cx| {
                let (commands, sink) = probe_channel();
                let (_, events) = async_channel::unbounded();
                let bounds = Bounds::centered(None, size(px(1260.), px(780.)), cx);
                let window = cx
                    .open_window(
                        WindowOptions {
                            show: false,
                            focus: false,
                            window_bounds: Some(WindowBounds::Windowed(bounds)),
                            ..Default::default()
                        },
                        |window, cx| cx.new(|cx| Workspace::new(commands, events, window, cx)),
                    )
                    .unwrap();
                window
                    .update(cx, |view, _, _| {
                        view.hide_wiring_children_for_probe();
                        view.locale = InterfaceLocale::English;
                    })
                    .unwrap();
                (window, sink)
            })
            .unwrap();
        Self {
            root,
            host,
            options,
            window,
            commands,
            replies: Vec::new(),
            identities: Vec::new(),
        }
    }

    fn apply(&self, cx: &mut AsyncApp, event: Event) {
        self.window
            .update(cx, |view, window, cx| {
                let graph = matches!(&event, Event::Graph(graph) if graph["type"].is_string() && graph["requestId"].is_u64())
                    .then(|| view.graph.clone());
                view.apply(
                    BridgeEvent::Runtime {
                        generation: 1,
                        event,
                    },
                    window,
                    cx,
                );
                if let Some(graph) = graph {
                    assert_eq!(view.graph, graph, "Runtime command replies must preserve the active graph");
                }
            })
            .unwrap();
    }

    fn send(&self, command: Command) {
        self.host.commands.send(command).unwrap();
    }

    async fn wait<T>(
        &mut self,
        cx: &mut AsyncApp,
        label: &str,
        mut read: impl FnMut(&Self, &mut AsyncApp) -> Option<T>,
    ) -> T {
        let deadline = Instant::now() + Duration::from_secs(12);
        loop {
            while let Ok(command) = self.commands.try_recv() {
                self.send(command);
            }
            let mut received = false;
            while let Ok(event) = self.host.events.try_recv() {
                received = true;
                if let Event::Graph(graph) = &event {
                    if let Some(identities) = graph["providerIdentityEvents"].as_array() {
                        self.identities.extend(identities.iter().cloned());
                    }
                    if graph["type"].is_string() {
                        if graph["type"] == "accountSummaryResult" {
                            let raw = graph.to_string();
                            assert!(!raw.contains(CANARY));
                            assert!(!raw.contains("wave4a-fake-refresh-token"));
                        }
                        self.replies.push(graph.clone());
                    }
                }
                if let Event::Diagnostic(message) = &event {
                    eprintln!("Offline Runtime diagnostic: {message}");
                }
                assert!(
                    !matches!(event, Event::Closed),
                    "Runtime closed before {label}"
                );
                self.apply(cx, event);
            }
            if let Some(result) = read(self, cx) {
                return result;
            }
            if received {
                draw(self.window.into(), cx);
            }
            assert!(Instant::now() < deadline, "Timed out waiting for {label}");
            cx.background_executor()
                .timer(Duration::from_millis(10))
                .await;
        }
    }

    async fn state<T>(
        &mut self,
        cx: &mut AsyncApp,
        label: &str,
        mut read: impl FnMut(&Workspace, &App) -> Option<T>,
    ) -> T {
        self.wait(cx, label, |fixture, cx| {
            fixture
                .window
                .update(cx, |view, _, cx| read(view, cx))
                .unwrap()
        })
        .await
    }

    async fn reply(&mut self, cx: &mut AsyncApp, kind: &str) -> Value {
        self.wait(cx, kind, |fixture, _| {
            fixture
                .replies
                .iter()
                .rev()
                .find(|value| value["type"] == kind)
                .cloned()
        })
        .await
    }

    fn update(
        &self,
        cx: &mut AsyncApp,
        action: impl FnOnce(&mut Workspace, &mut Window, &mut Context<Workspace>),
    ) {
        self.window.update(cx, action).unwrap();
    }

    fn input(&self, id: &str, bytes: &[u8]) {
        self.send(Command::Input {
            session_id: id.into(),
            bytes: bytes.into(),
            defer_ordering: false,
        });
    }

    fn codex_rollout(&self, id: &str) -> PathBuf {
        let path = self
            .options
            .shell_home
            .join(format!("codex/sessions/rollout-probe-{id}.jsonl"));
        fs::write(&path, format!("{}\n", json!({"type":"session_meta","payload":{"id":id,"cwd":self.options.workspace,"timestamp":"2026-01-01T12:00:00.000Z"}}))).unwrap();
        path
    }

    async fn codex_hook(&mut self, cx: &mut AsyncApp, stage: &str) -> String {
        let args = self.options.shell_home.join(format!("codex/{stage}-argv"));
        self.wait(cx, stage, |_, _| {
            let argv = fs::read_to_string(&args).ok()?;
            let (_, command) = argv.split_once("command = \"")?;
            let (command, _) = command.split_once('"')?;
            let shim = fs::read_to_string(command.trim_matches('\'')).unwrap();
            let (_, url) = shim.split_once("http://127.0.0.1:")?;
            let url = url
                .split(|ch: char| ch.is_whitespace() || matches!(ch, '\'' | '"' | '?'))
                .next()?;
            Some(format!("http://127.0.0.1:{url}"))
        })
        .await
    }

    async fn new_shell(&mut self, cx: &mut AsyncApp, source: &str) -> String {
        let scene = self
            .window
            .update(cx, |view, _, _| view.active_scene_id().unwrap().to_owned())
            .unwrap();
        self.send(Command::CreateShellSibling {
            scene_id: scene,
            source_session_id: source.into(),
            parent_session_id: None,
        });
        self.state(cx, "new shell", |view, _| {
            let id = view.level_focus()?;
            (id != source
                && view.graph_node(id)?["currentMode"] == "shell"
                && view.session_frames.contains(id))
            .then(|| id.to_owned())
        })
        .await
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.host.shutdown();
        fs::remove_dir_all(&self.root).unwrap();
    }
}

fn git(cwd: &Path, args: &[&str]) {
    let output = std::process::Command::new("/usr/bin/git")
        .current_dir(cwd)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn executable(path: &Path, script: &str) {
    fs::write(path, script).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

fn sample(ip: &str, country: &str) -> EgressSample {
    let family = Family::OpenAi;
    let (api, login) = family.hosts();
    let host = |name| HostSample {
        ip: Some(ip.into()),
        country: Some(country.into()),
        ..HostSample::new(name)
    };
    EgressSample::new(BTreeMap::from([(
        family,
        HostPair::new(family, host(api), host(login)),
    )]))
}

fn post(url: &str, payload: Value) {
    let (port, target) = url
        .strip_prefix("http://127.0.0.1:")
        .unwrap()
        .split_once('/')
        .unwrap();
    let mut socket =
        TcpStream::connect((Ipv4Addr::LOCALHOST, port.parse::<u16>().unwrap())).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let body = payload.to_string();
    write!(socket, "POST /{target} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
    let mut response = String::new();
    socket.read_to_string(&mut response).unwrap();
    assert!(
        response.starts_with("HTTP/1.1 200") || response.starts_with("HTTP/1.1 204"),
        "{response}"
    );
}

fn codex_event(name: &str, id: &str, cwd: &Path, rollout: &Path) -> Value {
    json!({"hook_event_name":name,"session_id":id,"cwd":cwd,"transcript_path":rollout,"turn_id":"probe-turn"})
}

fn submit_fork(fixture: &Fixture, cx: &mut AsyncApp, id: &str, action: CardMenuAction, name: &str) {
    fixture.update(cx, |view, _, _| {
        let menu = view.wiring_card_menu(id);
        let item = menu
            .items
            .iter()
            .find(|item| item.action == action)
            .unwrap();
        assert!(
            item.disabled_reason.is_none(),
            "Fork menu action {action:?}: {:?}",
            item.disabled_reason
        );
    });
    card_action(fixture.window, cx, id, action);
    fixture.update(cx, |view, window, cx| {
        let Some(TabDialog::Branch(state)) = &view.dialog else {
            panic!("fork dialog did not open")
        };
        state.input.update(cx, |input, cx| {
            input.replace_text_in_range(None, name, window, cx)
        });
        view.submit_fork(window, cx);
    });
}

pub(super) async fn verify(cx: &mut AsyncApp) {
    let mut fixture = Fixture::new(cx);
    let source = fixture
        .state(cx, "ready shell and startup exit status", |view, _| {
            let id = view.level_focus()?;
            (view.session_frames.contains(id) && !view.egress_for_probe().families.is_empty())
                .then(|| id.to_owned())
        })
        .await;
    fixture.input(&source, b"codex\r");
    let hook = fixture.codex_hook(cx, "source").await;
    let rollout = fixture.codex_rollout(SOURCE);
    post(
        &hook,
        codex_event("SessionStart", SOURCE, &fixture.options.workspace, &rollout),
    );
    fixture
        .wait(cx, "Codex source identity", |fixture, _| {
            fixture
                .identities
                .iter()
                .any(|event| {
                    event["sessionId"] == source
                        && event["providerSessionId"] == SOURCE
                        && event["result"] == "confirmed"
                })
                .then_some(())
        })
        .await;
    for name in ["UserPromptSubmit", "Stop"] {
        post(
            &hook,
            codex_event(name, SOURCE, &fixture.options.workspace, &rollout),
        );
    }
    fixture
        .state(cx, "Codex fork readiness", |view, _| {
            (view.graph_node(&source)?["canFork"] == true).then_some(())
        })
        .await;

    let egress = trigger_point_after_draw(fixture.window, cx, true);
    click(fixture.window.into(), cx, MouseButton::Left, egress);
    fixture.update(cx, |view, window, cx| {
        view.egress_action_for_probe(None, EgressAction::Recheck, window, cx)
    });
    fixture
        .state(cx, "first sample", |view, _| {
            view.egress_for_probe().checked_at.map(|_| ())
        })
        .await;
    fixture.update(cx, |view, window, cx| {
        view.egress_action_for_probe(None, EgressAction::Recheck, window, cx)
    });
    fixture
        .state(cx, "entry frozen by second sample", |view, _| {
            view.egress_frozen(&source).then_some(())
        })
        .await;
    let real_recheck = fixture.reply(cx, "egressRecheckResult").await;
    assert!(draw(fixture.window.into(), cx).contains_key(&format!("egress-banner-{source}")));
    fixture.update(cx, |view, window, cx| {
        let old_graph = view.graph.clone();
        let mut late = real_recheck.clone();
        late["sceneId"] = "old-reply-scene".into();
        late["result"]["state"] = "ok".into();
        view.apply(
            BridgeEvent::Runtime {
                generation: 1,
                event: Event::Graph(late),
            },
            window,
            cx,
        );
        assert_eq!(view.graph, old_graph);
        assert!(view.egress_frozen(&source));
        for action in [
            EgressAction::RequestOverride(EgressFamily::OpenAi),
            EgressAction::ConfirmOverride(EgressFamily::OpenAi),
        ] {
            view.egress_action_for_probe(Some(&source), action, window, cx);
        }
    });
    fixture
        .state(cx, "IP exemption", |view, _| {
            (!view.egress_frozen(&source)
                && view
                    .egress_for_probe()
                    .families
                    .iter()
                    .any(|row| row.family == EgressFamily::OpenAi && row.overridden))
            .then_some(())
        })
        .await;
    assert_eq!(
        fixture.reply(cx, "egressExemptionResult").await["result"]["families"][0]["overridden"],
        true
    );
    press(fixture.window, cx, "escape");
    println!(
        "live egress: status, two samples, frozen banner, stale reply isolation and current-IP exemption"
    );

    let update = trigger_point_after_draw(fixture.window, cx, false);
    click(fixture.window.into(), cx, MouseButton::Left, update);
    assert_eq!(
        fixture.reply(cx, "updateStatusResult").await["result"]["status"],
        "idle"
    );
    fixture.update(cx, |view, window, cx| {
        view.request_update_action_for_probe(UpdateAction::Check, window, cx);
        assert_eq!(view.update_view_for_probe().title, "Checking for updates");
    });
    let response = fixture.reply(cx, "updateCheckResult").await;
    fixture
        .state(cx, "update error", |view, _| {
            (view.update_view_for_probe().message.as_deref() == response["error"].as_str())
                .then_some(())
        })
        .await;
    fixture.update(cx, |view, window, cx| {
        view.accept_update_status(
            UpdateStatus::Available(UpdateRelease {
                version: "99.0.1".into(),
                install_mode: UpdateInstallMode::Manual,
                size_bytes: Some(100),
                release_day: None,
                notes: vec![],
            }),
            cx,
        );
        view.request_update_action_for_probe(UpdateAction::Download, window, cx);
        assert!(view.update_view_for_probe().progress.is_some());
    });
    let response = fixture.reply(cx, "updateDownloadResult").await;
    fixture
        .state(cx, "update download error", |view, _| {
            (view.update_view_for_probe().message.as_deref() == response["error"].as_str())
                .then_some(())
        })
        .await;
    press(fixture.window, cx, "escape");
    println!(
        "live update: idle, checking and download pending; no-feed check and download errors returned by Runtime"
    );

    submit_fork(
        &fixture,
        cx,
        &source,
        CardMenuAction::Fork,
        "Current probe fork",
    );
    let hook = fixture.codex_hook(cx, "fork").await;
    let rollout = fixture.codex_rollout(FORK);
    post(
        &hook,
        codex_event("SessionStart", FORK, &fixture.options.workspace, &rollout),
    );
    let forked = fixture
        .state(cx, "confirmed fork", |view, _| {
            session_levels::nodes(&view.graph)
                .iter()
                .find(|node| node["forkProgress"]["stage"] == "succeeded")
                .and_then(|node| node["sessionId"].as_str().map(str::to_owned))
        })
        .await;
    assert_ne!(forked, source);
    submit_fork(
        &fixture,
        cx,
        &source,
        CardMenuAction::ForkWorktree,
        "Isolated probe fork",
    );
    let failed = fixture
        .state(cx, "missing isolation root", |view, _| {
            session_levels::nodes(&view.graph)
                .iter()
                .find(|node| {
                    node["forkProgress"]["error"].as_str().is_some_and(|error| {
                        error.contains("An isolated worktree root is required")
                    })
                })
                .and_then(|node| node["sessionId"].as_str().map(str::to_owned))
        })
        .await;
    fixture.update(cx, |view, _, cx| {
        assert!(
            view.fork_repair_blocked_reason(&failed, view.graph_node(&failed).unwrap())
                .is_none()
        );
        view.retry_fork(&failed, cx);
    });
    fixture
        .state(cx, "failed fork retry reply", |view, _| {
            (!matches!(view.fork_retries.get(&failed), Some(ForkRetry::Pending))).then_some(())
        })
        .await;
    println!("live Codex menu: current fork confirmed, new worktree root error, and retry reply");

    let shell = fixture.new_shell(cx, &source).await;
    fixture.update(cx, |view, window, cx| {
        view.open_session_loader(&shell, window, cx);
        view.switch_loader_provider("dsh", cx);
    });
    fixture
        .state(cx, "DSH list and detail", |view, _| match &view.dialog {
            Some(TabDialog::SessionLoader(loader))
                if loader.selected.as_deref() == Some(SAVED)
                    && loader
                        .events
                        .iter()
                        .any(|event| event.text == "needle conversation") =>
            {
                Some(())
            }
            _ => None,
        })
        .await;
    fixture.update(cx, |view, window, cx| {
        let Some(TabDialog::SessionLoader(loader)) = &view.dialog else {
            panic!("DSH loader")
        };
        loader.content_input.update(cx, |input, cx| {
            input.replace_text_in_range(None, "needle", window, cx)
        });
        view.loader_search_changed(cx);
    });
    fixture
        .state(cx, "DSH search hits", |view, _| match &view.dialog {
            Some(TabDialog::SessionLoader(loader))
                if loader
                    .search_hits
                    .iter()
                    .any(|hit| hit.excerpt.contains("needle")) =>
            {
                Some(())
            }
            _ => None,
        })
        .await;
    fixture.update(cx, |view, _, cx| view.submit_session_loader(cx));
    assert_eq!(
        fixture.reply(cx, "dshSessionLoadResult").await["result"]["status"],
        "queued"
    );
    let patch_file = fixture.options.shell_home.join("dsh-patch");
    let patch = fixture
        .wait(cx, "DSH resume patch", |_, _| {
            fs::read_to_string(&patch_file)
                .ok()
                .filter(|text| text.contains("runId:") && text.contains("hookUrl:"))
        })
        .await;
    let field = |key: &str| -> String {
        serde_json::from_str(
            patch
                .lines()
                .find_map(|line| line.trim().strip_prefix(key))
                .unwrap(),
        )
        .unwrap()
    };
    let args = fs::read_to_string(fixture.options.shell_home.join("dsh-argv")).unwrap();
    assert!(args.contains("--resume\ndsh-saved"));
    post(
        &field("hookUrl: "),
        json!({
            "dsh_observation":1,"run_id":field("runId: "),"sequence":1,"generation":1,"session_id":SAVED,
            "foreground":true,"event_type":"ready","ready":true,"persisted":true,"cwd":fixture.options.workspace,"agent_status":"idle",
            "hud":{"title":"fixture DSH","contextPercent":18,"contextWindowSize":1000,"runningTools":[{"name":"Read"}]}
        }),
    );
    fixture
        .state(cx, "DSH provider identity and HUD", |view, _| {
            (view.dialog.is_none()
                && view.graph_node(&shell)?["currentMode"] == "dsh"
                && view.live_agent_hud(&shell)?["contextPercent"].as_f64() == Some(18.))
            .then_some(())
        })
        .await;
    assert!(fixture.identities.iter().any(|event| {
        event["provider"] == "dsh"
            && event["sessionId"] == shell
            && event["providerSessionId"] == SAVED
            && event["result"] == "confirmed"
    }));
    println!("live DSH: catalog, preview, content search, queued load, confirmed identity and HUD");

    let lead = fixture.new_shell(cx, &shell).await;
    let roll = fixture
        .options
        .shell_home
        .join(format!("claude/projects/fixture/{SOURCE}.jsonl"));
    fs::write(&roll, format!("{}\n{}\n",
        json!({"type":"user","toolUseResult":{"status":"teammate_spawned","teammate_id":"reviewer@team","name":"reviewer","team_name":"team","prompt":"Check the fixture"}}),
        json!({"type":"user","toolUseResult":{"listing":"Teammates (1):\n  reviewer [id]  ·  idle"}}),
    )).unwrap();
    fixture.input(&lead, b"claude\r");
    let settings_marker = fixture.options.shell_home.join("claude-settings");
    let settings = fixture
        .wait(cx, "Claude hook settings", |_, _| {
            fs::read_to_string(&settings_marker)
                .ok()
                .and_then(|path| fs::read(path.trim()).ok())
        })
        .await;
    let settings: Value = serde_json::from_slice(&settings).unwrap();
    let url = settings["hooks"]["SessionStart"][0]["hooks"][0]["url"]
        .as_str()
        .unwrap();
    post(
        url,
        json!({"hook_event_name":"SessionStart","session_id":SOURCE,"transcript_path":roll,"cwd":fixture.options.workspace}),
    );
    post(
        &format!("{url}?kind=statusline"),
        json!({"session_id":SOURCE,"context_window":{"used_percentage":21,"context_window_size":1000}}),
    );
    post(
        url,
        json!({"hook_event_name":"PreToolUse","session_id":SOURCE,"transcript_path":roll,"tool_name":"Read","tool_use_id":"probe-tool"}),
    );
    fixture
        .state(cx, "team and tools HUD", |view, _| {
            let hud = view.live_agent_hud(&lead)?;
            (hud["contextPercent"].as_f64() == Some(21.)
                && hud["runningTools"][0]["name"] == "Read"
                && view.render_live_teammates(&lead).is_some())
            .then_some(())
        })
        .await;
    assert!(
        draw(fixture.window.into(), cx).contains_key(&format!("team-member-{lead}-reviewer@team"))
    );
    println!("live team HUD: context meter, running tool and painted read-only teammate card");

    fixture.update(cx, |view, window, cx| {
        view.toggle_settings(window, cx);
        let (settings, _) = view.settings.as_ref().unwrap();
        settings.update(cx, |settings, cx| {
            settings.select_diagnostics_for_probe(window, cx)
        });
    });
    for kind in [
        "accountSummaryResult",
        "environmentDiagnosticsResult",
        "browserDiagnosticsResult",
    ] {
        assert!(fixture.reply(cx, kind).await.get("error").is_none());
    }
    fixture.update(cx, |view, _, cx| {
        let (settings, _) = view.settings.as_ref().unwrap();
        settings.update(cx, |settings, cx| {
            let account = settings.diagnostics_text_for_probe("accountSummaryResult");
            assert!(
                account.contains("pro")
                    && !account.contains(CANARY)
                    && !account.contains("private@")
            );
            assert!(
                settings
                    .diagnostics_text_for_probe("environmentDiagnosticsResult")
                    .contains("Ready")
            );
            assert!(
                settings
                    .diagnostics_text_for_probe("browserDiagnosticsResult")
                    .contains("7.2.1")
            );
            settings.cleanup_for_probe(cx);
        });
    });
    let receipt = fixture.reply(cx, "retentionCleanupResult").await;
    assert_eq!(receipt["error"], "Detached retention fixture unavailable");
    fixture.update(cx, |view, _, cx| {
        let (settings, _) = view.settings.as_ref().unwrap();
        assert_eq!(
            settings
                .read(cx)
                .diagnostics_text_for_probe("retentionCleanupResult"),
            "Detached retention fixture unavailable"
        );
    });
    fixture.commands.read_only_mode(1);
    fixture.update(cx, |view, _, cx| {
        let (settings, _) = view.settings.as_ref().unwrap();
        assert!(
            settings
                .read(cx)
                .diagnostics_text_for_probe("retentionCleanupResult")
                .contains("read-only")
        );
    });
    draw(fixture.window.into(), cx);
    fixture.update(cx, |_, window, _| window.remove_window());
    println!(
        "live settings: account canary redacted, Git and browser diagnostics, retention error and read-only guard"
    );
}

fn trigger_point_after_draw(window: Handle, cx: &mut AsyncApp, egress: bool) -> Point<Pixels> {
    draw(window.into(), cx);
    trigger_point(window, cx, egress)
}
