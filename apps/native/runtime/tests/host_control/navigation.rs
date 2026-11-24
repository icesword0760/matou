use super::*;

#[test]
fn navigation_rejects_wrong_scope_expired_token_and_stale_projection() -> Result<()> {
    const FOCUS: &str = "navigation.focus.session";
    let mut harness = Harness::new(false, false, GENERATION, fixed_now, IDLE_TIMEOUT)?;
    let read_token = harness.token(RUN, &["host.identify", READ], NOW + 1000)?;
    let expired = harness.token(RUN, &[FOCUS], NOW - 1)?;
    let focus_token = harness.token(RUN, &[FOCUS], NOW + 1000)?;
    for (label, method, token, error) in [
        ("navigation-scope", FOCUS, &read_token, "CAPABILITY_DENIED"),
        ("navigation-expired", FOCUS, &expired, "CAPABILITY_DENIED"),
        (
            "navigation-method-scope",
            "navigation.switch.task",
            &focus_token,
            "CAPABILITY_DENIED",
        ),
    ] {
        harness.verify(
            label,
            request(label, method, token, json!({"target": {"kind": "self"}})),
            label,
            Some(error),
            0,
            0,
        )?;
    }
    for (target, code, topology_reads) in [
        (
            json!({"kind":"relative","direction":"left","projectionRevision":"stale"}),
            "STALE_PROJECTION",
            1,
        ),
        (json!({"kind":"relative","direction":"left"}), INVALID, 0),
        (
            json!({"kind":"wrong","sessionId":fixture_id(8)}),
            INVALID,
            0,
        ),
    ] {
        harness.verify(
            "navigation-selector-validation",
            request("selector", FOCUS, &focus_token, json!({"target":target})),
            "selector",
            Some(code),
            topology_reads,
            0,
        )?;
    }
    Ok(())
}
