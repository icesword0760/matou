use super::*;
use crate::runtime_bridge::{ProbeReceiver, probe_channel};
use matou_native_desktop::{
    egress_panel::{EgressAction, EgressRule},
    update_popover::UpdateAction,
    workspace_board::BoardStatus,
};
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
const AGY_OTHER: &str = "agy-other";
const AGY_FORK: &str = "agy-fork";
const AGY_TITLE: &str = "Antigravity fixture title";
const AGY_MESSAGE: &str = "unique transcript needle";

pub(super) fn prepare() -> PathBuf {
    // Keep the absolute socket path under macOS's sockaddr_un length limit.
    let target = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../target")
        .canonicalize()
        .unwrap();
    let id = uuid::Uuid::new_v4().simple().to_string();
    let root = target.join(&id[..5]);
    fs::create_dir(&root).unwrap();
    // The Claude catalog reads the process environment. Set it before starting
    // GPUI or Runtime so that neither opens the user's own Claude catalog.
    unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", root.join("home/claude")) };
    root
}

struct Fixture {
    root: PathBuf,
    host: RuntimeHost,
    options: LaunchOptions,
    environment: BTreeMap<OsString, OsString>,
    window: Handle,
    commands: ProbeReceiver,
    replies: Vec<Value>,
    identities: Vec<Value>,
    frame_sequences: HashMap<String, u64>,
    generation: u64,
}

impl Fixture {
    fn new(cx: &mut AsyncApp, root: PathBuf) -> Self {
        let workspace = root.join("workspace");
        let shell_home = root.join("home");
        let data_root = root.clone();
        let codex_home = shell_home.join("codex");
        let claude_config = shell_home.join("claude");
        let dsh_history = root.join("dsh-history");
        let agy_runs = root.join("agy-runs");
        for path in [
            &workspace,
            &shell_home,
            &data_root,
            &codex_home.join("sessions"),
            &claude_config.join("projects/fixture"),
            &agy_runs,
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

        let agy = root.join("agy-fixture");
        executable(
            &agy,
            r#"#!/bin/sh
set -eu
run="$AGY_FIXTURE_RUNS/$MATOU_PROVIDER_HOOK_TOKEN"
mkdir "$run"
printf '%s\n' "$@" > "$run/args"
printf '%s' "$MATOU_PROVIDER_HOOK_URL" > "$run/url"
/bin/stty -echo
touch "$run/ready"
while IFS= read -r line; do
    printf '%s\n' "$line" >> "$run/input"
    [ "$line" != /exit ] || exit 17
    if { [ "$line" = /fork ] && [ -f "$run/emit-fork-output" ]; } || [ "$line" = /output ]; then
        printf 'ANTIGRAVITY FIXTURE OUTPUT\n'
    fi
done
"#,
        );
        let agy_history = shell_home.join(".gemini/antigravity-cli");
        fs::create_dir_all(agy_history.join("cache")).unwrap();
        fs::write(
            agy_history.join("cache/conversation_metadata.json"),
            json!({"conversations": {
                SOURCE: {"last_modified_time": 200.5, "summary": {
                    "AppDataDir": "antigravity-cli", "WorkspaceURIs": [&workspace],
                    "Title": AGY_TITLE, "NumSteps": 2,
                }},
                AGY_OTHER: {"last_modified_time": 100, "summary": {
                    "AppDataDir": "antigravity-cli", "WorkspaceURIs": [&workspace],
                    "Title": "Another saved conversation",
                }},
            }})
            .to_string(),
        )
        .unwrap();
        let transcript = agy_history.join(format!("brain/{SOURCE}/.system_generated/logs"));
        fs::create_dir_all(&transcript).unwrap();
        fs::write(
            transcript.join("transcript.jsonl"),
            format!(
                "{}\n{}\n",
                json!({"source":"USER_EXPLICIT","content":AGY_MESSAGE}),
                json!({"type":"PLANNER_RESPONSE","content":"Antigravity reply"}),
            ),
        )
        .unwrap();
        // The catalog uses a path-encoded bucket. Keep the same provider ID
        // across fixtures to catch replies routed by ID alone.
        let claude_bucket: String = workspace
            .to_str()
            .unwrap()
            .chars()
            .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
            .collect();
        let claude_bucket = claude_config.join("projects").join(claude_bucket);
        fs::create_dir_all(&claude_bucket).unwrap();
        fs::write(
            claude_bucket.join(format!("{SOURCE}.jsonl")),
            format!(
                "{}\n",
                json!({"cwd":workspace,"timestamp":300,"type":"user",
                    "message":{"role":"user","content":"Claude catalog fixture"}}),
            ),
        )
        .unwrap();

        let environment: BTreeMap<OsString, OsString> = [
            ("PATH", "/usr/bin:/bin".into()),
            ("HOME", shell_home.clone().into_os_string()),
            ("CODEX_HOME", codex_home.into_os_string()),
            ("MATOU_CODEX_COMMAND", codex.into_os_string()),
            ("CLAUDE_CONFIG_DIR", claude_config.into_os_string()),
            ("MATOU_CLAUDE_COMMAND", claude.into_os_string()),
            ("MATOU_DSH_COMMAND", dsh.into_os_string()),
            ("MATOU_ANTIGRAVITY_COMMAND", agy.into_os_string()),
            ("AGY_FIXTURE_RUNS", agy_runs.into_os_string()),
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
        let mut samples = VecDeque::from([
            sample("203.0.113.1", "US"),
            sample("203.0.113.2", "US"),
            sample("203.0.113.3", "CN"),
        ]);
        let host = RuntimeHost::start_with_offline_fixtures(
            options.clone(),
            OfflineFixtures {
                provider_environment: Some(environment.clone()),
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
            environment,
            window,
            commands,
            replies: Vec::new(),
            identities: Vec::new(),
            frame_sequences: HashMap::new(),
            generation: 1,
        }
    }

    fn apply(&self, cx: &mut AsyncApp, event: Event) {
        self.window
            .update(cx, |view, window, cx| {
                let graph = matches!(&event, Event::Graph(graph) if graph["type"].is_string() && graph["requestId"].is_u64())
                    .then(|| view.graph.clone());
                view.apply(
                    BridgeEvent::Runtime {
                        generation: self.generation,
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
                    assert!(
                        !message.to_ascii_lowercase().contains("sequence"),
                        "Offline Runtime diagnostic: {message}"
                    );
                    eprintln!("Offline Runtime diagnostic: {message}");
                }
                if let Event::Frame {
                    session_id,
                    sequence,
                    ..
                } = &event
                {
                    self.frame_sequences.insert(session_id.clone(), *sequence);
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

    async fn agy_run(&mut self, cx: &mut AsyncApp, count: usize) -> AgyRun {
        let runs = self.root.join("agy-runs");
        self.wait(cx, &format!("Antigravity run {count}"), |_, _| {
            let mut ready: Vec<_> = fs::read_dir(&runs)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .filter(|run| run.join("ready").is_file())
                .collect();
            if ready.len() < count {
                return None;
            }
            assert_eq!(ready.len(), count);
            ready.sort_by_key(|run| fs::metadata(run.join("ready")).unwrap().modified().unwrap());
            let path = ready.pop().unwrap();
            Some(AgyRun {
                url: fs::read_to_string(path.join("url")).unwrap(),
                argv: fs::read_to_string(path.join("args")).unwrap(),
                path,
            })
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

struct AgyRun {
    url: String,
    argv: String,
    path: PathBuf,
}

impl AgyRun {
    fn status(&self, id: &str) {
        post(
            &format!("{}?kind=statusline", self.url),
            json!({"conversation_id":id,"agent_state":"idle","cycle_mode":"plan"}),
        );
    }

    fn hook(&self, event: &str, id: &str) {
        post(
            &self.url,
            json!({"hookEventName":event,"conversationId":id,"lastAssistantMessage":"Probe reply"}),
        );
    }

    fn resume_arg(&self) -> Option<&str> {
        let (_, args) = self.argv.split_once("--conversation\n")?;
        args.lines().next()
    }

    fn received_fork(&self) -> bool {
        fs::read_to_string(self.path.join("input"))
            .is_ok_and(|text| text.lines().any(|line| line == "/fork"))
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

pub(super) async fn verify(cx: &mut AsyncApp, root: PathBuf) {
    let mut fixture = Fixture::new(cx, root);
    let source = fixture
        .state(cx, "ready shell and startup exit status", |view, _| {
            let id = view.level_focus()?;
            (view.session_frames.contains(id)
                && !view.recovery_blocked(id)
                && !view.egress_for_probe().families.is_empty())
            .then(|| id.to_owned())
        })
        .await;
    card_action(fixture.window, cx, &source, CardMenuAction::OpenDetached);
    let child = fixture
        .window
        .read_with(cx, |view, _| view.wiring_child_for_probe(&source).unwrap())
        .unwrap();
    draw(child, cx);
    assert!(dispatch_key(child, cx, "cmd-w"));
    assert!(child.update(cx, |_, _, _| ()).is_err());
    draw(fixture.window.into(), cx);
    fixture.update(cx, |view, window, cx| {
        assert!(view.wiring_child_for_probe(&source).is_none());
        assert!(view.terminals[&source].read(cx).focus.is_focused(window));
    });
    const DETACHED_OUTPUT: &str = "detachedcmdwok";
    for key in ["e", "c", "h", "o", "space"] {
        assert!(dispatch_key(fixture.window.into(), cx, key));
    }
    for character in DETACHED_OUTPUT.chars() {
        assert!(dispatch_key(
            fixture.window.into(),
            cx,
            &character.to_string()
        ));
    }
    assert!(dispatch_key(fixture.window.into(), cx, "enter"));
    fixture
        .state(cx, "shell output after Cmd+W return", |view, _| {
            view.graph_node(&source)?["latestLines"]
                .as_array()?
                .iter()
                .any(|line| {
                    line.as_str()
                        .is_some_and(|line| line.trim() == DETACHED_OUTPUT)
                })
                .then_some(())
        })
        .await;
    println!("detached Cmd+W: original offline shell executed typed input ({DETACHED_OUTPUT})");

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
    let panel_action = |fixture: &Fixture, cx: &mut AsyncApp, action| {
        fixture.update(cx, |view, window, cx| {
            view.egress_action_for_probe(None, action, window, cx)
        });
    };
    panel_action(&fixture, cx, EgressAction::Recheck);
    fixture
        .state(cx, "different IP in the original country", |view, _| {
            view.egress_for_probe()
                .families
                .iter()
                .find(|row| row.family == EgressFamily::OpenAi)?
                .exit
                .as_ref()
                .filter(|exit| exit.ip == "203.0.113.2")
                .map(|_| ())
        })
        .await;
    panel_action(&fixture, cx, EgressAction::SetRule(EgressRule::Ip));
    panel_action(&fixture, cx, EgressAction::SetRule(EgressRule::Ip));
    let strict = fixture.reply(cx, "egressRuleModeResult").await;
    assert_eq!(strict["result"]["ruleMode"], "ip");
    fixture.update(cx, |view, _, _| assert!(view.egress_frozen(&source)));
    panel_action(&fixture, cx, EgressAction::SetProtection(false));
    fixture
        .state(cx, "protection off and card thawed", |view, _| {
            (!view.egress_for_probe().protection_enabled && !view.egress_frozen(&source))
                .then_some(())
        })
        .await;
    panel_action(&fixture, cx, EgressAction::SetProtection(true));
    fixture
        .state(cx, "protection on and card frozen", |view, _| {
            (view.egress_for_probe().protection_enabled && view.egress_frozen(&source))
                .then_some(())
        })
        .await;
    panel_action(
        &fixture,
        cx,
        EgressAction::AcceptCurrent(EgressFamily::OpenAi),
    );
    let accepted = fixture.reply(cx, "egressBaselineResult").await;
    assert_eq!(
        accepted["result"]["families"][0]["baseline"]["ip"],
        "203.0.113.2"
    );
    fixture.update(cx, |view, _, _| assert!(!view.egress_frozen(&source)));

    fixture.update(cx, |view, window, cx| {
        view.egress_action_for_probe(None, EgressAction::Recheck, window, cx)
    });
    fixture
        .state(cx, "entry frozen by mainland sample", |view, _| {
            view.egress_frozen(&source).then_some(())
        })
        .await;
    let real_recheck = fixture.reply(cx, "egressRecheckResult").await;
    assert!(draw(fixture.window.into(), cx).contains_key(&format!("egress-banner-{source}")));
    assert_eq!(
        fixture
            .replies
            .iter()
            .filter(|reply| reply["type"] == "egressRuleModeResult")
            .count(),
        1
    );
    // A click already queued when a new sample arrives can target a now-invalid exit.
    panel_action(
        &fixture,
        cx,
        EgressAction::AcceptCurrent(EgressFamily::OpenAi),
    );
    fixture
        .state(cx, "baseline validation message from Runtime", |view, _| {
            (view.egress_frozen(&source)
                && view
                    .task_toast
                    .as_deref()
                    .is_some_and(|message| message.contains("mainland check")))
            .then_some(())
        })
        .await;
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
        "live egress: rule switch, duplicate guard, protection off/on, accepted baseline, Runtime validation error, frozen banner, stale reply isolation and current-IP exemption"
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
    // A hook response acknowledges receipt. Wait for the start event to be
    // applied before sending observations that need the active run.
    fixture
        .state(
            cx,
            "Claude HUD and teammate after session start",
            |view, _| {
                (view.live_agent_hud(&lead).is_some()
                    && view.render_live_teammates(&lead).is_some())
                .then_some(())
            },
        )
        .await;
    post(
        &format!("{url}?kind=statusline"),
        json!({"session_id":SOURCE,"context_window":{"used_percentage":21,"context_window_size":1000}}),
    );
    fixture
        .state(cx, "Claude context meter", |view, _| {
            (view.live_agent_hud(&lead)?["contextPercent"].as_f64() == Some(21.)).then_some(())
        })
        .await;
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

    let agy = verify_antigravity(&mut fixture, cx, [&source, &shell, &lead]).await;

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
    verify_antigravity_read_only(&mut fixture, cx, &agy).await;
    draw(fixture.window.into(), cx);
    fixture.update(cx, |_, window, _| window.remove_window());
    println!(
        "live settings: account canary redacted, Git and browser diagnostics, retention error and read-only guard"
    );
}

async fn verify_antigravity(
    fixture: &mut Fixture,
    cx: &mut AsyncApp,
    siblings: [&str; 3],
) -> String {
    let target = fixture.new_shell(cx, siblings[2]).await;
    fixture.update(cx, |view, window, cx| {
        view.open_session_loader(&target, window, cx)
    });
    for (provider, id, preview) in [
        ("claude-code", SOURCE, Some("Claude catalog fixture")),
        ("codex", SOURCE, None),
        ("dsh", SAVED, Some("needle conversation")),
    ] {
        fixture.update(cx, |view, _, cx| view.switch_loader_provider(provider, cx));
        fixture
            .state(cx, provider, |view, _| {
                let Some(TabDialog::SessionLoader(loader)) = &view.dialog else {
                    return None;
                };
                // Codex has multiple rows after forking. Check membership and
                // require the initial preview to finish before switching.
                (loader.provider == provider
                    && !loader.loading_list
                    && !loader.loading_detail
                    && loader.sessions.iter().any(|row| row.id == id)
                    && preview
                        .is_none_or(|text| loader.events.iter().any(|event| event.text == text))
                    && loader.events.iter().all(|event| event.text != AGY_MESSAGE))
                .then_some(())
            })
            .await;
    }
    fixture.update(cx, |view, window, cx| {
        view.switch_loader_provider("antigravity", cx);
        let Some(TabDialog::SessionLoader(loader)) = &view.dialog else {
            panic!("Antigravity loader");
        };
        let request_id = loader.pending_list.as_ref().unwrap().id;
        let graph = view.graph.clone();
        for (generation, kind, id) in [
            (1, "antigravitySessionListResult", request_id - 1),
            (1, "antigravitySessionDetailResult", request_id),
            (1, "dshSessionListResult", request_id),
            (0, "antigravitySessionListResult", request_id),
        ] {
            view.apply(
                BridgeEvent::Runtime {
                    generation,
                    event: Event::Graph(json!({
                        "type":kind,"requestId":id,"error":"stale fixture reply","nodes":[],
                    })),
                },
                window,
                cx,
            );
        }
        let Some(TabDialog::SessionLoader(loader)) = &view.dialog else {
            panic!("A stale receipt closed the modal");
        };
        assert_eq!(loader.pending_list.as_ref().unwrap().id, request_id);
        assert!(loader.error.is_none());
        assert_eq!(view.graph, graph);
    });
    fixture
        .state(cx, "Antigravity catalog and preview", |view, _| {
            let Some(TabDialog::SessionLoader(loader)) = &view.dialog else {
                return None;
            };
            (loader.sessions.len() == 2
                && loader.selected.as_deref() == Some(SOURCE)
                && loader.events.iter().any(|event| event.text == AGY_MESSAGE))
            .then_some(())
        })
        .await;
    fixture.update(cx, |view, window, cx| {
        let Some(TabDialog::SessionLoader(loader)) = &view.dialog else {
            panic!("Antigravity loader");
        };
        loader.input.update(cx, |input, cx| {
            input.replace_text_in_range(None, "FIXTURE TITLE", window, cx)
        });
        view.loader_filter_changed(cx);
    });
    fixture
        .state(cx, "Antigravity title filter", |view, _| {
            let Some(TabDialog::SessionLoader(loader)) = &view.dialog else {
                return None;
            };
            (loader.sessions.len() == 1 && loader.sessions[0].title == AGY_TITLE).then_some(())
        })
        .await;
    fixture.update(cx, |view, window, cx| {
        let Some(TabDialog::SessionLoader(loader)) = &view.dialog else {
            panic!("Antigravity loader");
        };
        loader.content_input.update(cx, |input, cx| {
            input.replace_text_in_range(None, "TRANSCRIPT NEEDLE", window, cx)
        });
        view.loader_search_changed(cx);
    });
    let hit = fixture
        .state(cx, "Antigravity content search", |view, _| {
            let Some(TabDialog::SessionLoader(loader)) = &view.dialog else {
                return None;
            };
            let [hit] = loader.search_hits.as_slice() else {
                return None;
            };
            assert!(hit.excerpt.contains("transcript needle"));
            Some(hit.index)
        })
        .await;
    fixture.update(cx, |view, _, cx| view.select_loader_hit(hit, cx));
    fixture
        .state(cx, "Antigravity search result preview", |view, _| {
            let Some(TabDialog::SessionLoader(loader)) = &view.dialog else {
                return None;
            };
            (loader.pending_detail.is_none()
                && loader
                    .events
                    .iter()
                    .any(|event| event.index as i64 == hit && event.text == AGY_MESSAGE))
            .then_some(())
        })
        .await;
    fixture.update(cx, |view, _, cx| view.submit_session_loader(cx));
    let receipt = fixture.reply(cx, "antigravitySessionLoadResult").await;
    assert_eq!(receipt["result"]["status"], "restoring", "{receipt}");
    assert_eq!(receipt["result"]["sessionId"], target);

    let first = fixture.agy_run(cx, 1).await;
    assert_eq!(first.resume_arg(), Some(SOURCE));
    fixture
        .state(cx, "Antigravity restoring banner", |view, _| {
            let node = view.graph_node(&target)?;
            matches!(
                view.provider_restore_banner(&target, node),
                Some(ProviderRestoreBanner::Restoring { .. })
            )
            .then_some(())
        })
        .await;
    assert!(
        !fixture
            .identities
            .iter()
            .any(|event| { event["sessionId"] == target && event["result"] == "confirmed" })
    );
    first.status(AGY_OTHER);
    restore_from_failed_card(fixture, cx, &target, false).await;
    let recovered = fixture.agy_run(cx, 2).await;
    assert_eq!(recovered.resume_arg(), Some(SOURCE));
    recovered.status(SOURCE);
    recovered.hook("PreInvocation", SOURCE);
    recovered.hook("Stop", SOURCE);
    fixture
        .state(cx, "Antigravity confirmed and ready to fork", |view, _| {
            let node = view.graph_node(&target)?;
            (view.dialog.is_none()
                && node["canFork"] == true
                && view.provider_restore_banner(&target, node).is_none()
                && !view.recovery_blocked(&target))
            .then_some(())
        })
        .await;
    for (id, provider, provider_id) in [
        (siblings[0], "codex", SOURCE),
        (siblings[1], "dsh", SAVED),
        (siblings[2], "claude-code", SOURCE),
        (&target, "antigravity", SOURCE),
    ] {
        fixture.update(cx, |view, _, _| {
            assert_eq!(view.graph_node(id).unwrap()["currentMode"], provider)
        });
        assert!(
            fixture.identities.iter().any(|event| {
                event["sessionId"] == id
                    && event["provider"] == provider
                    && event["providerSessionId"] == provider_id
                    && event["result"] == "confirmed"
            }),
            "Missing confirmed {provider} identity"
        );
    }
    println!(
        "live Antigravity loader: four-provider isolation, stale reply guards, title filter, content search, preview, restoring receipt and confirmed retry"
    );

    fixture.update(cx, |view, _, _| {
        let menu = view.wiring_card_menu(&target);
        assert!(
            menu.items
                .iter()
                .any(|item| item.action == CardMenuAction::ForkWorktree
                    && item.disabled_reason.is_some())
        );
    });
    submit_fork(
        fixture,
        cx,
        &target,
        CardMenuAction::Fork,
        "Antigravity probe fork",
    );
    let cwd = fixture.options.workspace.to_str().unwrap().to_owned();
    let forked = fixture
        .state(cx, "Antigravity fork receipt", |view, _| {
            let child = session_levels::nodes(&view.graph)
                .iter()
                .find(|node| node["title"] == "Antigravity probe fork")?;
            assert_eq!(child["cwd"], cwd);
            view.dialog
                .is_none()
                .then(|| child["sessionId"].as_str().unwrap().to_owned())
        })
        .await;
    let run = fixture.agy_run(cx, 3).await;
    assert_eq!(run.resume_arg(), Some(SOURCE));
    fs::write(run.path.join("emit-fork-output"), "").unwrap();
    run.status(SOURCE);
    fixture
        .wait(cx, "Antigravity fork input", |_, _| {
            run.received_fork().then_some(())
        })
        .await;
    let previous_sequence = fixture
        .wait(cx, "output frame from the first fork run", |fixture, _| {
            fixture
                .frame_sequences
                .get(&forked)
                .copied()
                .filter(|&seq| seq > 0)
        })
        .await;
    fixture.input(&forked, b"/exit\r");
    fixture
        .state(cx, "Antigravity fork failure menu", |view, _| {
            let menu = view.wiring_card_menu(&forked);
            (menu.can_open()
                && menu.items.iter().any(|item| {
                    item.action == CardMenuAction::RetryFork && item.disabled_reason.is_none()
                }))
            .then_some(())
        })
        .await;
    card_action(fixture.window, cx, &forked, CardMenuAction::RetryFork);
    let retry = fixture.agy_run(cx, 4).await;
    assert_ne!(retry.url, run.url);
    assert_eq!(retry.resume_arg(), Some(SOURCE));
    fixture
        .wait(
            cx,
            "zero-credit layout frame from the quiet retry",
            |fixture, _| (fixture.frame_sequences.get(&forked) == Some(&0)).then_some(()),
        )
        .await;
    fixture.send(Command::Applied {
        session_id: forked.clone(),
        sequence: previous_sequence,
    });
    retry.status(SOURCE);
    fixture
        .wait(cx, "Antigravity retry fork input", |_, _| {
            retry.received_fork().then_some(())
        })
        .await;
    retry.status(AGY_FORK);
    fixture
        .state(cx, "Antigravity fork retry confirmed", |view, _| {
            let child = view.graph_node(&forked)?;
            (child["forkProgress"]["stage"] == "succeeded"
                && !view.fork_retries.contains_key(&forked))
            .then_some(())
        })
        .await;
    assert!(fixture.identities.iter().any(|event| {
        event["sessionId"] == forked
            && event["providerSessionId"] == AGY_FORK
            && event["result"] == "confirmed"
    }));
    fixture.input(&forked, b"/output\r");
    fixture
        .wait(
            cx,
            "new output credit after rejecting the old frame acknowledgement",
            |fixture, _| {
                (fixture.frame_sequences.get(&forked).copied()? > previous_sequence).then_some(())
            },
        )
        .await;
    println!(
        "live Antigravity menu: current-worktree fork, controlled failure, right-click retry, confirmed child identity and old acknowledgement rejected"
    );

    fixture.update(cx, |view, window, cx| {
        view.open_session_loader(&target, window, cx)
    });
    fixture
        .state(cx, "Antigravity loader reopen", |view, _| {
            let Some(TabDialog::SessionLoader(loader)) = &view.dialog else {
                return None;
            };
            (loader.provider == "antigravity" && loader.selected.as_deref() == Some(SOURCE))
                .then_some(())
        })
        .await;
    fixture.update(cx, |view, _, cx| {
        view.submit_session_loader(cx);
        if matches!(&view.dialog, Some(TabDialog::SessionLoader(loader)) if loader.confirm_running)
        {
            view.submit_session_loader(cx);
        }
    });
    fixture.agy_run(cx, 5).await.status(AGY_OTHER);
    restore_from_failed_card(fixture, cx, &target, true).await;
    let fresh = fixture.agy_run(cx, 6).await;
    assert_eq!(fresh.resume_arg(), None);
    fresh.status(AGY_OTHER);
    fixture
        .wait(cx, "Antigravity fresh identity", |fixture, _| {
            fixture
                .identities
                .iter()
                .any(|event| {
                    event["sessionId"] == target
                        && event["providerSessionId"] == AGY_OTHER
                        && event["result"] == "confirmed"
                })
                .then_some(())
        })
        .await;
    println!(
        "live Antigravity recovery: failed restore can start a new conversation on the same card"
    );
    target
}

async fn restore_from_failed_card(
    fixture: &mut Fixture,
    cx: &mut AsyncApp,
    target: &str,
    fresh: bool,
) {
    fixture
        .state(cx, "Antigravity failed restore actions", |view, _| {
            let node = view.graph_node(target)?;
            matches!(
                view.provider_restore_banner(target, node),
                Some(ProviderRestoreBanner::Failed {
                    pending: false,
                    retry_blocked: None,
                    fresh_blocked: None,
                    ..
                })
            )
            .then_some(())
        })
        .await;
    fixture.update(cx, |view, window, cx| {
        view.run_provider_restore(target, fresh, window, cx)
    });
}

async fn verify_antigravity_read_only(fixture: &mut Fixture, cx: &mut AsyncApp, target: &str) {
    use matou_native_runtime::host::STORAGE_READ_ONLY;

    fixture.host.shutdown().unwrap();
    let output = std::process::Command::new("/usr/bin/sqlite3")
        .env_clear()
        .arg(fixture.root.join("matou.sqlite"))
        .arg("PRAGMA wal_checkpoint(TRUNCATE); INSERT INTO schema_migrations (version, name, checksum, applied_at) VALUES (9999, 'probe-newer', 'probe-newer', 5);")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    fixture.host = RuntimeHost::start_with_offline_fixtures(
        fixture.options.clone(),
        OfflineFixtures {
            provider_environment: Some(fixture.environment.clone()),
            ..Default::default()
        },
    )
    .unwrap();
    fixture.replies.clear();
    fixture
        .state(cx, "Runtime read-only mode", |view, _| {
            view.read_only.as_ref().map(|_| ())
        })
        .await;
    let cwd = fixture.options.workspace.to_str().unwrap().to_owned();
    fixture.send(Command::ListAntigravitySessions {
        request_id: 101,
        cwd,
        query: AGY_TITLE.into(),
        full_text: false,
        offset: None,
        limit: None,
    });
    let list = fixture.reply(cx, "antigravitySessionListResult").await;
    assert_eq!(list["result"]["sessions"][0]["providerSessionId"], SOURCE);
    let load = || Command::LoadAntigravitySession {
        request_id: 102,
        session_id: target.into(),
        provider_session_id: SOURCE.into(),
    };
    fixture.update(cx, |view, window, cx| {
        assert_eq!(
            view.commands.send_at(1, load()),
            Err(crate::runtime_bridge::SendError::StorageReadOnly)
        );
        view.open_session_loader(target, window, cx);
        assert!(view.dialog.is_none());
        assert!(!view.wiring_card_menu(target).can_open());
    });
    fixture.send(load());
    let rejected = fixture.reply(cx, "antigravitySessionLoadResult").await;
    assert_eq!(rejected["requestId"], 102);
    assert_eq!(rejected["code"], STORAGE_READ_ONLY);
    fixture.send(Command::SetTaskStatus {
        request_id: 103,
        workspace_id: "unused".into(),
        task_id: "unused".into(),
        status: "completed".into(),
        before_task_id: None,
    });
    let rejected = fixture.reply(cx, "taskStatusResult").await;
    assert_eq!(rejected["requestId"], 103);
    assert_eq!(rejected["code"], STORAGE_READ_ONLY);
    assert_eq!(
        fs::read_dir(fixture.root.join("agy-runs")).unwrap().count(),
        6
    );
    println!(
        "live Antigravity read-only: Runtime can list, Desktop blocks writes and Runtime rejects a direct load without starting a process"
    );
}

pub(super) async fn verify_board(cx: &mut AsyncApp, root: PathBuf) {
    let mut fixture = Fixture::new(cx, root);
    let (workspace, task) = fixture
        .state(cx, "initial board task", |view, _| {
            let task = view.hierarchy.as_ref()?.task.as_ref()?;
            view.domain_ready
                .then(|| (task.workspace_id.clone(), task.id.clone()))
        })
        .await;
    fixture.send(Command::CreateCanvas {
        task_id: task.clone(),
    });
    fixture
        .state(cx, "two canvases for one task", |view, _| {
            (view.scenes.len() == 2 && view.graph["workspaceSessionCountByTask"][&task] == 2)
                .then_some(())
        })
        .await;
    fixture.update(cx, |view, window, cx| view.toggle_board(window, cx));
    let card = |id: &str| format!("board-card:{workspace}:{id}");
    let header =
        |status: BoardStatus| format!("board-column-header:{workspace}:{}", status.as_str());
    let window = fixture.window;
    let read_board = |cx: &mut AsyncApp| {
        window
            .read_with(cx, |view, _| view.board_view_for_probe().unwrap())
            .unwrap()
    };
    let assert_column = |cx: &mut AsyncApp, id: &str, status: BoardStatus| {
        let bounds = draw_board(window, cx);
        let header = bounds[&header(status)];
        let card = bounds[&card(id)];
        assert!(
            header.left() < card.center().x && card.center().x < header.right(),
            "Task {id} must be painted in {}",
            status.as_str()
        );
    };
    assert_column(cx, task.as_str(), BoardStatus::Planned);
    assert_eq!(
        read_board(cx).columns[0].cards[0].session_count,
        "2 sessions"
    );
    assert_eq!(
        window
            .read_with(cx, |view, _| session_levels::nodes(&view.graph).len())
            .unwrap(),
        1
    );

    let mut previous = None;
    for (from, to, feedback) in [
        (
            BoardStatus::Planned,
            BoardStatus::Active,
            "Task moved to \"Active\"",
        ),
        (
            BoardStatus::Active,
            BoardStatus::Completed,
            "Task moved to \"Done\"",
        ),
    ] {
        drag_board_card(window, cx, &card(&task), &header(to));
        assert_column(cx, &task, to);
        assert!(read_board(cx).feedback.is_none());
        fixture.update(cx, |view, _, _| {
            assert_eq!(
                view.tasks
                    .iter()
                    .find(|item| item.id == task)
                    .unwrap()
                    .status,
                from.as_str()
            );
        });
        if let Some(request_id) = previous {
            fixture.apply(cx, Event::Graph(json!({
                "type": "taskStatusResult", "requestId": request_id, "error": "Late failed reply"
            })));
            assert_column(cx, &task, to);
            assert!(read_board(cx).feedback.is_none());
        }
        fixture.replies.clear();
        let reply = fixture.reply(cx, "taskStatusResult").await;
        assert_eq!(reply["result"][0]["status"], to.as_str());
        previous = reply["requestId"].as_u64();
        assert_eq!(read_board(cx).feedback.as_deref(), Some(feedback));
        assert!(draw_board(window, cx).contains_key("board-feedback"));
    }

    fixture.send(Command::CreateTask {
        workspace_id: workspace.clone(),
    });
    let other = fixture
        .state(cx, "second board task", |view, _| {
            view.tasks
                .iter()
                .find(|item| item.id != task)
                .map(|item| item.id.clone())
        })
        .await;
    // Drop on a card to insert before it, then reorder inside the same column.
    for (source, target) in [(&other, &task), (&task, &other)] {
        drag_board_card(window, cx, &card(source), &card(target));
        assert_eq!(read_board(cx).columns[3].cards[0].id, *source);
        fixture.replies.clear();
        let reply = fixture.reply(cx, "taskStatusResult").await;
        assert_eq!(reply["result"][0]["id"], *source);
        assert_eq!(reply["result"][0]["sortKey"], "a00000000");
        assert_eq!(read_board(cx).columns[3].cards[0].id, *source);
    }

    fixture.host.shutdown().unwrap();
    fixture.generation += 1;
    let generation = fixture.generation;
    fixture.commands.generation(generation);
    fixture.host = RuntimeHost::start_with_offline_fixtures(
        fixture.options.clone(),
        OfflineFixtures {
            provider_environment: Some(fixture.environment.clone()),
            dsh_session_root: Some(fixture.root.join("dsh-history")),
            ..Default::default()
        },
    )
    .unwrap();
    fixture
        .state(cx, "reopened board Runtime", |view, _| {
            (view.runtime_generation == generation && view.domain_ready).then_some(())
        })
        .await;
    assert_eq!(
        read_board(cx).columns[3]
            .cards
            .iter()
            .map(|card| card.id.as_str())
            .collect::<Vec<_>>(),
        [task.as_str(), other.as_str()]
    );
    assert_column(cx, &task, BoardStatus::Completed);
    assert_eq!(
        read_board(cx).columns[3].cards[0].session_count,
        "2 sessions"
    );

    drag_board_card(window, cx, &card(&other), &header(BoardStatus::Blocked));
    fixture.replies.clear();
    assert!(fixture.reply(cx, "taskStatusResult").await["result"].is_array());
    // The destination moves in Runtime while the board still shows the old column.
    const COMPETING_MOVE: u64 = 10_000;
    fixture.send(Command::SetTaskStatus {
        request_id: COMPETING_MOVE,
        workspace_id: workspace.clone(),
        task_id: other.clone(),
        status: "active".into(),
        before_task_id: None,
    });
    drag_board_card(window, cx, &card(&task), &card(&other));
    assert_column(cx, &task, BoardStatus::Blocked);
    fixture.replies.clear();
    let rejected = fixture
        .wait(cx, "stale board destination", |fixture, _| {
            fixture
                .replies
                .iter()
                .find(|reply| {
                    reply["type"] == "taskStatusResult" && reply["requestId"] != COMPETING_MOVE
                })
                .cloned()
        })
        .await;
    assert_eq!(
        rejected["error"],
        "Board destination must belong to the target column"
    );
    assert_column(cx, &task, BoardStatus::Completed);
    assert_column(cx, &other, BoardStatus::Active);
    assert_eq!(
        read_board(cx).feedback.as_deref(),
        Some("Could not save the status. Try again.")
    );
    assert!(draw_board(window, cx).contains_key("board-feedback"));
    fixture.update(cx, |_, window, _| window.remove_window());
    fixture.host.shutdown().unwrap();
    println!(
        "live board: hidden drag, saved status and order after reopen, cross-canvas counts, stale reply and rollback"
    );
}

fn trigger_point_after_draw(window: Handle, cx: &mut AsyncApp, egress: bool) -> Point<Pixels> {
    draw(window.into(), cx);
    trigger_point(window, cx, egress)
}
