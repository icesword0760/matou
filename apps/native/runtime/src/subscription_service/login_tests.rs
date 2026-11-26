use std::{
    fs,
    io::{Read, Write},
    net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream},
    os::unix::fs::PermissionsExt,
};

use matou_native_core::provider_config::ProviderConfigLocale;

use super::*;

struct Fixture {
    service: SubscriptionService,
    root: tempfile::TempDir,
    ports: [u16; 2],
}

impl Fixture {
    fn new(mode: &str, timeout: Duration) -> Result<Self> {
        let root = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR"))?;
        let binary = root.path().join("router-fixture");
        fs::write(
            &binary,
            include_str!("../../tests/fixtures/subscription-router.py"),
        )?;
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700))?;
        let router = root.path().join("model-router");
        fs::create_dir(&router)?;
        fs::write(router.join("fixture-mode"), mode)?;
        let (_, first) = callback::bind(0)?;
        let (_, second) = callback::bind(0)?;
        let ports = [
            first[0].local_addr()?.port(),
            second[0].local_addr()?.port(),
        ];
        fs::write(
            router.join("fixture-oauth-ports"),
            json!({"codex": ports[0], "antigravity": ports[1]}).to_string(),
        )?;
        let store = Arc::new(ProviderConfigStore::new(
            root.path(),
            ProviderConfigLocale::En,
        ));
        let mut service = SubscriptionService::new(
            root.path().into(),
            store,
            BTreeMap::from([
                ("MATOU_MODEL_ROUTER_BIN".into(), binary.into()),
                ("NO_PROXY".into(), "*".into()),
            ]),
        )?;
        service.login_options = LoginOptions {
            ports: Some(ports),
            timeout,
            open: |url, system| {
                assert!(
                    url.starts_with("https://auth.openai.com/")
                        || url.starts_with("https://accounts.google.com/")
                );
                if system {
                    Ok(())
                } else {
                    Err(BrowserOpenFailure::Controlled)
                }
            },
        };
        Ok(Self {
            service,
            root,
            ports,
        })
    }

    fn request(
        &self,
        kind: SubscriptionProvider,
        ipv6: bool,
        method: &str,
        path: &str,
    ) -> Result<String> {
        let port = self.ports[usize::from(kind == SubscriptionProvider::Antigravity)];
        let address = if ipv6 {
            SocketAddr::from((Ipv6Addr::LOCALHOST, port))
        } else {
            SocketAddr::from((Ipv4Addr::LOCALHOST, port))
        };
        let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(2))?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        write!(
            stream,
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
        )?;
        let mut response = String::new();
        stream.read_to_string(&mut response)?;
        Ok(response)
    }

    fn state(&self, id: &str) -> String {
        self.service.login_attempts.lock().unwrap()[id]
            .state
            .clone()
    }

    fn authorize(
        &self,
        owner: u64,
        kind: SubscriptionProvider,
        code: &str,
    ) -> Result<SubscriptionLoginView> {
        let start = self.service.start_login(owner, kind, false)?;
        let state = self.state(&start.attempt_id);
        let response = self.request(
            kind,
            false,
            "GET",
            &format!(
                "{}?state={state}&code={code}",
                callback::callback_endpoint(kind).1
            ),
        )?;
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        self.service.login_status(owner, &start.attempt_id)
    }

    fn credential(&self, name: &str) -> PathBuf {
        self.root.path().join("model-router/credentials").join(name)
    }

    fn usage_reads(&self) -> Result<usize> {
        Ok(
            fs::read_to_string(self.root.path().join("model-router/fixture-events.jsonl"))?
                .lines()
                .filter(|line| line.contains("/wham/usage"))
                .count(),
        )
    }

    fn catalog_mode(&self, mode: &str) -> Result<()> {
        fs::write(
            self.root.path().join("model-router/fixture-catalog-mode"),
            mode,
        )?;
        Ok(())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.service.close_all_settings();
        self.service.shutdown();
    }
}

fn wait_for(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !ready() {
        assert!(Instant::now() < deadline);
        thread::sleep(POLL);
    }
}

fn input(view: &SubscriptionLoginView) -> Value {
    let mut draft = view.draft.clone().expect("authenticated setup draft");
    draft.as_object_mut().unwrap().remove("subscription");
    draft
}

fn preferences() -> Result<Connection> {
    let db = Connection::open_in_memory()?;
    db.execute_batch(
        "CREATE TABLE preferences (key TEXT PRIMARY KEY, value_json TEXT, updated_at INTEGER);",
    )?;
    Ok(db)
}

fn credential_state(path: &std::path::Path) -> Result<Value> {
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

#[test]
fn reports_system_browser_failure_after_automatic_fallback() -> Result<()> {
    let mut fixture = Fixture::new("", LOGIN_TIMEOUT)?;
    let service = &mut fixture.service;
    service.login_options.open = |url, system| {
        let tried_controlled = std::cell::Cell::new(false);
        open_subscription_browser(
            url,
            system,
            |_| {
                tried_controlled.set(true);
                false
            },
            |_| {
                assert!(
                    tried_controlled.get(),
                    "the controlled browser is tried first"
                );
                false
            },
        )
    };
    browser_integration::set_controlled_browser_preference(&service.root, true)?;
    service.begin_settings(1)?;
    let start = service.start_login(1, SubscriptionProvider::ChatGpt, false)?;
    wait_for(|| {
        service
            .login_status(1, &start.attempt_id)
            .unwrap()
            .can_reopen
    });
    let view = service.login_status(1, &start.attempt_id)?;
    assert_eq!(view.status, Status::Waiting);
    assert!(
        !view.system_browser_available,
        "the system browser already failed",
    );
    assert_eq!(
        view.reason.as_deref(),
        Some("Could not open the system browser. Check your browser and try again."),
    );
    Ok(())
}

#[test]
fn codex_callback_draft_relogin_rollback_and_delete() -> Result<()> {
    let fixture = Fixture::new("", LOGIN_TIMEOUT)?;
    let service = &fixture.service;
    browser_integration::set_controlled_browser_preference(&service.root, true)?;
    let kind = SubscriptionProvider::ChatGpt;
    service.begin_settings(1)?;
    service.begin_settings(2)?;
    let first = service.start_login(1, kind, false)?;
    assert_eq!(first.status, Status::Waiting);
    assert!(
        service
            .start_login(2, kind, false)
            .unwrap_err()
            .to_string()
            .contains("another window")
    );
    assert!(service.login_status(2, &first.attempt_id).is_err());
    wait_for(|| {
        service
            .login_status(1, &first.attempt_id)
            .unwrap()
            .system_browser_available
    });
    service.reopen_login(1, &first.attempt_id, true)?;
    wait_for(|| {
        let view = service.login_status(1, &first.attempt_id).unwrap();
        view.can_reopen && !view.system_browser_available
    });
    let state = fixture.state(&first.attempt_id);
    for (method, path, status) in [
        (
            "POST",
            format!("/auth/callback?state={state}&code=first"),
            "400",
        ),
        ("GET", format!("/wrong?state={state}&code=first"), "404"),
        ("GET", "/auth/callback?state=wrong&code=first".into(), "400"),
        (
            "GET",
            format!("/auth/callback?state={state}&state={state}&code=first"),
            "400",
        ),
        (
            "GET",
            format!("/auth/callback?state={state}&code=first&code=second"),
            "400",
        ),
        (
            "GET",
            format!("/auth/callback?state={state}&code=first&error=denied"),
            "400",
        ),
    ] {
        assert!(
            fixture
                .request(kind, false, method, &path)?
                .starts_with(&format!("HTTP/1.1 {status}"))
        );
    }
    let path = format!("/auth/callback?state={state}&code=first");
    let response = fixture.request(kind, false, "GET", &path)?;
    assert!(response.starts_with("HTTP/1.1 200") && response.contains("cache-control: no-store"));
    assert!(!response.contains(&state) && !response.contains("code=first"));
    assert!(
        fixture
            .request(kind, false, "GET", &path)?
            .starts_with("HTTP/1.1 409")
    );
    let ready = service.login_status(1, &first.attempt_id)?;
    assert_eq!(ready.status, Status::Ready);
    let public = serde_json::to_string(&ready)?;
    assert!(
        !public.contains(&state)
            && !public.contains("credentialRef")
            && !public.contains("fixture-secret-token")
    );
    assert!(!service.store.has_subscription("codex")?);
    assert_eq!(
        credential_state(&fixture.credential("codex-first.json"))?["disabled"],
        true
    );

    let mut draft = input(&ready);
    draft["name"] = json!("My Codex account");
    draft["modelMapping"]["fable"] = json!("fixture-fable");
    draft["effortMapping"]["fable"] = json!("high");
    draft["contextWindowMode"] = json!("maximum");
    let mut invalid = draft.clone();
    invalid["effortMapping"]["fable"] = json!("max");
    let mut db = preferences()?;
    assert!(
        service
            .commit_draft(1, &ready.attempt_id, &invalid, false, &mut db, 1)
            .is_err()
    );
    let saved = service.commit_draft(1, &ready.attempt_id, &draft, false, &mut db, 1)?;
    let id = saved["id"].as_str().unwrap();
    assert_eq!(saved["modelMapping"]["fable"], "fixture-fable");
    assert_eq!(saved["contextWindowMode"], "maximum");
    let bound = service.prepare_model_target(&PreparedSubscriptionTarget {
        provider_id: id.into(),
        provider: "codex",
        model: Some("fixture-fable".into()),
    })?;
    let relogin = fixture.authorize(1, kind, "second")?;
    let mut draft = input(&relogin);
    assert_eq!(draft["name"], "My Codex account");
    assert_eq!(draft["modelMapping"]["fable"], "fixture-fable");
    assert_eq!(draft["effortMapping"]["fable"], "high");
    draft["contextWindowMode"] = json!("512k");
    let mut broken_db = Connection::open_in_memory()?;
    assert!(
        service
            .commit_draft(1, &relogin.attempt_id, &draft, false, &mut broken_db, 2)
            .is_err()
    );
    assert!(bound.is_revoked());
    assert_eq!(
        service
            .store
            .subscription(id)?
            .unwrap()
            .identity
            .credential_name,
        "codex-first.json"
    );
    assert_eq!(
        credential_state(&fixture.credential("codex-first.json"))?["disabled"],
        false
    );
    assert_eq!(
        credential_state(&fixture.credential("codex-second.json"))?["disabled"],
        true
    );
    service.commit_draft(1, &relogin.attempt_id, &draft, false, &mut db, 2)?;
    assert_eq!(
        service
            .store
            .subscription(id)?
            .unwrap()
            .identity
            .credential_name,
        "codex-second.json"
    );
    let bound = service.prepare_model_target(&PreparedSubscriptionTarget {
        provider_id: id.into(),
        provider: "codex",
        model: Some("fixture-fable".into()),
    })?;
    service.delete("claude-code", id)?;
    assert!(bound.is_revoked());
    assert_eq!(
        credential_state(&fixture.credential("codex-second.json"))?["disabled"],
        true
    );
    assert!(service.store.subscription(id)?.is_none());
    Ok(())
}

#[test]
fn antigravity_risk_ipv6_cancel_and_unambiguous_baseline() -> Result<()> {
    let fixture = Fixture::new("", LOGIN_TIMEOUT)?;
    let service = &fixture.service;
    let kind = SubscriptionProvider::Antigravity;
    service.begin_settings(10)?;
    assert!(
        service
            .start_login(10, kind, false)
            .unwrap_err()
            .to_string()
            .contains("risk confirmation")
    );
    service.store.confirm_antigravity_risk()?;
    let start = service.start_login(10, kind, false)?;
    let state = fixture.state(&start.attempt_id);
    let response = fixture.request(
        kind,
        true,
        "GET",
        &format!("/oauth-callback?state={state}&error=%3Cscript%3E"),
    )?;
    assert!(response.starts_with("HTTP/1.1 400") && !response.contains("<script>"));
    assert_eq!(
        service.login_status(10, &start.attempt_id)?.status,
        Status::Failed
    );
    let ready = fixture.authorize(10, kind, "first")?;
    assert_eq!(ready.status, Status::Ready);
    assert_eq!(
        ready.draft.as_ref().unwrap()["modelMapping"]["fable"],
        "claude-fable-5-2"
    );
    assert_eq!(
        ready.draft.as_ref().unwrap()["subscription"]["quota"][0]["usedPercent"],
        75.0
    );
    let saved = service.commit_draft(
        10,
        &ready.attempt_id,
        &input(&ready),
        true,
        &mut preferences()?,
        1,
    )?;
    let id = saved["id"].as_str().unwrap();
    assert_eq!(
        service.store.snapshot()?["activeProviderIds"]["claude-code"],
        id
    );
    let draft = fixture.authorize(10, kind, "second")?;
    assert_eq!(draft.status, Status::Ready);
    assert!(fixture.credential("antigravity-second.json").exists());
    service.close_settings(10);
    service.cancel_settings_logins(10)?;
    assert!(!fixture.credential("antigravity-second.json").exists());
    service.begin_settings(11)?;
    let rejected = fixture.authorize(11, kind, "ambiguous")?;
    assert_eq!(rejected.status, Status::Failed);
    assert_eq!(
        service
            .store
            .subscription(id)?
            .unwrap()
            .identity
            .credential_name,
        "antigravity-first.json"
    );
    Ok(())
}

#[test]
fn login_timeout_busy_port_and_untrusted_redirect() -> Result<()> {
    let mut fixture = Fixture::new("", Duration::from_millis(100))?;
    let kind = SubscriptionProvider::ChatGpt;
    let use_codex_port = |fixture: &mut Fixture, port| -> Result<()> {
        fixture.ports[0] = port;
        fs::write(
            fixture.root.path().join("model-router/fixture-oauth-ports"),
            json!({"codex": port, "antigravity": fixture.ports[1]}).to_string(),
        )?;
        fixture.service.login_options.ports = Some(fixture.ports);
        Ok(())
    };
    fixture.service.begin_settings(1)?;
    let (port, busy) = callback::bind(0)?;
    use_codex_port(&mut fixture, port)?;
    assert!(
        fixture
            .service
            .start_login(1, kind, false)
            .unwrap_err()
            .to_string()
            .contains("port is in use")
    );
    drop(busy);
    let mut view = fixture.service.start_login(1, kind, false);
    // Other local fixtures can claim an ephemeral port during the handoff.
    // Retry only an OS bind conflict, with a freshly reserved port.
    for _ in 0..2 {
        if !view
            .as_ref()
            .is_err_and(|error| error.to_string().contains("port is in use"))
        {
            break;
        }
        let (port, reservation) = callback::bind(0)?;
        use_codex_port(&mut fixture, port)?;
        drop(reservation);
        view = fixture.service.start_login(1, kind, false);
    }
    let view = view?;
    let service = &fixture.service;
    thread::sleep(Duration::from_millis(125));
    service.expire_logins();
    assert_eq!(
        service.login_status(1, &view.attempt_id)?.status,
        Status::Expired
    );

    let fixture = Fixture::new("invalid-auth-url", LOGIN_TIMEOUT)?;
    fixture.service.begin_settings(1)?;
    assert!(
        fixture
            .service
            .start_login(1, kind, false)
            .unwrap_err()
            .to_string()
            .contains("address was rejected")
    );
    let log = fs::read_to_string(
        fixture
            .root
            .path()
            .join("model-router/fixture-events.jsonl"),
    )?;
    assert!(log.contains("oauth/session"));
    Ok(())
}

#[test]
fn settings_cache_uses_current_credential_and_catalog_refresh_is_explicit() -> Result<()> {
    let fixture = Fixture::new("", LOGIN_TIMEOUT)?;
    let service = &fixture.service;
    service.begin_settings(1)?;
    let view = fixture.authorize(1, SubscriptionProvider::ChatGpt, "first")?;
    let saved = service.commit_draft(
        1,
        &view.attempt_id,
        &input(&view),
        false,
        &mut preferences()?,
        1,
    )?;
    let id = saved["id"].as_str().unwrap();
    let name = "codex-first.json";
    assert_eq!(fixture.usage_reads()?, 1);
    service.activate(id)?;
    assert_eq!(fixture.usage_reads()?, 1);
    service.close_settings(1);
    service.begin_settings(2)?;
    let settings = service.read_settings(2)?;
    assert_eq!(settings.accounts[id].account["plan"], "Pro 200");
    assert_eq!(settings.accounts[id].quota[0].used_percent, 35.0);
    assert_eq!(fixture.usage_reads()?, 1);
    service.close_settings(2);

    service
        .account_details
        .lock()
        .unwrap()
        .get_mut(name)
        .unwrap()
        .checked = Instant::now() - settings::DETAILS_TTL;
    service.begin_settings(3)?;
    service.read_settings(3)?;
    assert_eq!(fixture.usage_reads()?, 2);
    let mut file = credential_state(&fixture.credential(name))?;
    file["updated_at"] = json!("new-fingerprint");
    fs::write(fixture.credential(name), file.to_string())?;
    service.close_settings(3);
    service.begin_settings(4)?;
    service.read_settings(4)?;
    assert_eq!(fixture.usage_reads()?, 3);
    let before = service.store.subscription(id)?.unwrap().model_metadata;
    fixture.catalog_mode("failure")?;
    assert!(service.refresh_models(4, id).is_err());
    assert_eq!(
        service.store.subscription(id)?.unwrap().model_metadata,
        before
    );
    fixture.catalog_mode("updated")?;
    let refreshed = service.refresh_models(4, id)?;
    assert!(
        refreshed
            .details
            .models
            .iter()
            .any(|model| model["id"] == "fixture-updated-model")
    );
    assert_eq!(refreshed.details.quota[0].used_percent, 35.0);
    assert_eq!(fixture.usage_reads()?, 3);
    fixture.catalog_mode("unauthorized")?;
    assert!(
        service
            .refresh_models(4, id)
            .unwrap_err()
            .is::<account_details::SignInRequired>()
    );
    assert_eq!(
        service.store.subscription(id)?.unwrap().account["status"],
        "needs-sign-in"
    );
    service.close_settings(4);
    wait_for(|| service.manager.current_connection().is_none());
    Ok(())
}
