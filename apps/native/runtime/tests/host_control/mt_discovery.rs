use super::*;
use crate::mt_cli::{Client, run_with};

fn cli(
    harness: &mut Harness,
    token: &str,
    args: &[&str],
    expected: (i32, &str, &str),
    label: &str,
) -> Result<()> {
    let endpoint = harness.server.endpoint.path.clone().into_os_string();
    let before = snapshot(&harness.observe, harness.root.path())?;
    let actual = harness.drive(|| {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = run_with(
            &args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>(),
            || (Some(endpoint), Some(token.to_owned())),
            &mut stdout,
            &mut stderr,
        );
        Ok((code, stdout, stderr))
    })?;
    assert_eq!(
        (actual.0, actual.1.as_slice(), actual.2.as_slice()),
        (expected.0, expected.1.as_bytes(), expected.2.as_bytes()),
        "{label}"
    );
    assert_eq!(snapshot(&harness.observe, harness.root.path())?, before);
    println!("PASS {label}");
    Ok(())
}

fn pretty_line(value: &Value) -> String {
    format!("{}\n", serde_json::to_string_pretty(value).unwrap())
}

#[test]
fn real_control_discovery_matches_frozen_text_and_ordered_json() -> Result<()> {
    let mut base = Harness::new(false, false, GENERATION, fixed_now, IDLE_TIMEOUT)?;
    let token = base.token(RUN, MANAGED_SCOPES, NOW)?;
    let identity = frozen_case("cli-identify-text");
    let text = identity["expected_response"]["stdout"][0].as_str().unwrap();
    cli(
        &mut base,
        &token,
        &["identify"],
        (0, &format!("{text}\n"), ""),
        "cli-identify-text",
    )?;
    let fixture = frozen_case("identify-caller");
    let identity = &fixture["expected_response"]["frames"][0]["result"];
    cli(
        &mut base,
        &token,
        &["identify", "--json"],
        (0, &pretty_line(identity), ""),
        "cli-identify-json",
    )?;

    let mut graph = Harness::new(true, false, GENERATION, fixed_now, IDLE_TIMEOUT)?;
    let token = graph.token(RUN, MANAGED_SCOPES, NOW)?;
    let fixture = frozen_case("list-scopes");
    let expected = &fixture["expected_response"]["frames"];
    let identity_on_graph = json!({
        "caller": identity["caller"],
        "target": expected[0]["result"]["targets"][0],
    });
    cli(
        &mut graph,
        &token,
        &["identify", "--json"],
        (0, &pretty_line(&identity_on_graph), ""),
        "cli-identify-json-on-two-layer-graph",
    )?;
    cli(
        &mut graph,
        &token,
        &["identify"],
        (0, &format!("{text}\n"), ""),
        "cli-identify-text-on-two-layer-graph",
    )?;
    for (label, flags, frame, text) in [
        (
            "cli-list-current",
            Vec::new(),
            0,
            "1. Shell [shell] /__fixture__/workspace\n2. Peer [shell] /__fixture__/workspace\n",
        ),
        (
            "cli-list-all",
            vec!["--all"],
            2,
            "1. Shell [shell] /__fixture__/workspace\n2. Peer [shell] /__fixture__/workspace\n1. Child [shell] /__fixture__/workspace\n1. Shell [shell] /__fixture__/workspace\n",
        ),
    ] {
        let mut args = vec!["list"];
        args.extend(flags);
        cli(
            &mut graph,
            &token,
            &args,
            (0, text, ""),
            &format!("{label}-text"),
        )?;
        args.push("--json");
        cli(
            &mut graph,
            &token,
            &args,
            (0, &pretty_line(&expected[frame]["result"]), ""),
            &format!("{label}-json-order-revision"),
        )?;
    }
    Ok(())
}

#[test]
fn real_control_lists_other_workspaces_and_denies_revoked_token() -> Result<()> {
    let mut graph = Harness::new(true, true, GENERATION, fixed_now, IDLE_TIMEOUT)?;
    let token = graph.token(RUN, MANAGED_SCOPES, NOW)?;
    let endpoint = graph.server.endpoint.path.clone();
    let all = graph.drive(|| {
        let result = Client::new(&endpoint, &token)
            .unwrap()
            .request("host.list", json!({"scope": "all"}))
            .unwrap();
        Ok(result.unwrap())
    })?;
    let targets = all["targets"].as_array().unwrap();
    assert_eq!(targets.len(), 5);
    assert_eq!(
        targets
            .iter()
            .filter(|target| target["workspace"]["name"] == "Secondary")
            .count(),
        1
    );
    cli(
        &mut graph,
        &token,
        &["list", "--all", "--json"],
        (0, &pretty_line(&all), ""),
        "cli-list-all-cross-workspace",
    )?;
    graph.server.runs().revoke(RUN);
    let fixture = frozen_case("wire-run-token-revoked");
    let error = &fixture["expected_response"]["frames"][1]["error"];
    cli(
        &mut graph,
        &token,
        &["list", "--all", "--json"],
        (4, "", &pretty_line(error)),
        "cli-token-revoked",
    )?;
    Ok(())
}

#[test]
fn real_control_round_trips_encoded_selectors_and_public_errors() -> Result<()> {
    let mut graph = Harness::new(true, false, GENERATION, fixed_now, IDLE_TIMEOUT)?;
    let main = graph.token(RUN, &[READ, "host.list"], NOW)?;
    let issue = |run: &str, id| {
        graph
            .server
            .state
            .tokens
            .issue(run, &fixture_id(id), &[READ, "host.list"], NOW)
    };
    let peer = issue("peer-run", 16)?;
    let child = issue("child-run", 25)?;
    let before = snapshot(&graph.observe, graph.root.path())?;
    let endpoint = graph.server.endpoint.path.clone();
    let mut resolve = |token: &str, target: &str| {
        graph.drive(|| {
            let client = Client::new(&endpoint, token).unwrap();
            let selector = client.terminal_selector(target).unwrap();
            Ok(client.request(READ, screen_params(selector)))
        })
    };
    for (token, input, expected) in [
        (&main, "self", "previous terminal output"),
        (&main, "right", "peer terminal output"),
        (&peer, "left", "previous terminal output"),
        (&child, "parent", "previous terminal output"),
        (&main, "child", "fixture child output"),
        (&main, "child:1", "fixture child output"),
        (&main, "sibling:2", "peer terminal output"),
        (&main, &fixture_id(25), "fixture child output"),
    ] {
        let result = resolve(token, input)?.unwrap().unwrap();
        assert_eq!(result["text"], expected);
        println!("PASS cli-selector-{input}");
    }
    let missing = frozen_case("selector-missing-id-name-position");
    let errors = &missing["expected_response"]["frames"];
    for (target, frame, name) in [
        (fixture_id(999), 0, "missing-id"),
        ("sibling:3".into(), 2, "sibling-out-of-range"),
    ] {
        let error = resolve(&main, &target)?.unwrap_err();
        assert_eq!(error.exit_code(), 3);
        assert_eq!(
            format!("{}\n", error.format(true)),
            pretty_line(&errors[frame]["error"])
        );
        assert_eq!(
            error.format(false),
            errors[frame]["error"]["message"].as_str().unwrap()
        );
        println!("PASS cli-selector-{name}");
    }
    let stale_fixture = frozen_case("stale-terminal-sibling");
    let stale = graph.drive(|| {
        let client = Client::new(&endpoint, &main).unwrap();
        let mut selector = client
            .terminal_selector(&format!("session:{}", fixture_id(25)))
            .unwrap();
        selector["projectionRevision"] = json!("older-revision");
        Ok(client.request(READ, screen_params(selector)).unwrap_err())
    })?;
    assert_eq!(stale.exit_code(), 3);
    assert_eq!(
        format!("{}\n", stale.format(true)),
        pretty_line(&stale_fixture["expected_response"]["frames"][0]["error"])
    );
    println!("PASS cli-selector-stale-ref");

    graph.server.projection_aliases = Some(|targets| {
        for target in targets {
            if [fixture_id(8), fixture_id(25)].contains(&target.session_id) {
                target.reference = "legacy:shared".into();
            }
        }
    });
    let ambiguous = graph.drive(|| {
        let client = Client::new(&endpoint, &main).unwrap();
        let selector = client.terminal_selector("legacy:shared").unwrap();
        Ok(client.request(READ, screen_params(selector)).unwrap_err())
    })?;
    let fixture = frozen_case("selector-legacy-ref-ambiguity");
    let expected = &fixture["expected_response"]["frames"][0]["error"];
    assert_eq!(ambiguous.exit_code(), 3);
    assert_eq!(
        format!("{}\n", ambiguous.format(true)),
        pretty_line(expected)
    );
    let paths = expected["details"]["candidates"].as_array().unwrap();
    assert_eq!(
        ambiguous.format(false),
        format!(
            "{}\n候选项：\n1. {}\n2. {}",
            expected["message"].as_str().unwrap(),
            paths[0]["humanPath"].as_str().unwrap(),
            paths[1]["humanPath"].as_str().unwrap()
        )
    );
    assert_eq!(snapshot(&graph.observe, graph.root.path())?, before);
    println!("PASS cli-selector-ambiguous-ref");
    Ok(())
}
