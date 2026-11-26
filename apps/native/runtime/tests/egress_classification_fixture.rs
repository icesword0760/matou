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
const FIXTURE_JSON: &str = include_str!("fixtures/egress-classification-reference.json");

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
