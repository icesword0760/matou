//! Presentation for the exit-status chip, diagnostic panel, and card freeze banner.

use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    net::{IpAddr, Ipv4Addr},
    time::Duration,
};

use matou_native_core::sessions::InterfaceLocale;

pub const EGRESS_COPIED_DURATION: Duration = Duration::from_secs(2);
pub const EGRESS_RESUMED_DURATION: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EgressFamily {
    Anthropic,
    OpenAi,
    ChatGptSubscription,
    Google,
}

impl EgressFamily {
    pub const ALL: [Self; 4] = [
        Self::Anthropic,
        Self::OpenAi,
        Self::ChatGptSubscription,
        Self::Google,
    ];

    pub fn heading(self, locale: InterfaceLocale) -> &'static str {
        match self {
            Self::Anthropic => locale.text("Claude（Anthropic）", "Claude (Anthropic)"),
            Self::OpenAi => locale.text("Codex（OpenAI）", "Codex (OpenAI)"),
            Self::ChatGptSubscription => {
                locale.text("Claude（ChatGPT 订阅）", "Claude (ChatGPT subscription)")
            }
            Self::Google => locale.text("Google（通用出口检测）", "Google (generic exit)"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EgressState {
    Checking,
    Ok,
    Warning,
    Abnormal,
    Unknown,
}

impl EgressState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Checking => "checking",
            Self::Ok => "ok",
            Self::Warning => "warning",
            Self::Abnormal => "abnormal",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EgressRule {
    #[default]
    Country,
    Ip,
}

impl EgressRule {
    pub fn label(self, locale: InterfaceLocale) -> &'static str {
        match self {
            Self::Country => locale.text(
                "同一国家／地区（推荐）",
                "Same country/region (recommended)",
            ),
            Self::Ip => locale.text("固定 IP", "Fixed IP"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EgressViolation {
    Mainland,
    Rule,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EgressExit {
    pub ip: String,
    pub country: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EgressFamilyStatus {
    pub family: EgressFamily,
    pub state: EgressState,
    pub exit: Option<EgressExit>,
    pub baseline: Option<EgressExit>,
    pub violation: Option<EgressViolation>,
    pub overridden: bool,
    pub frozen_session_ids: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EgressCheckKind {
    Mainland,
    ExitRule,
    LoginApiMatch,
    AgentProxyPath,
    BrowserPath,
    TimeZone,
    Ipv6,
}

impl EgressCheckKind {
    fn title(self, locale: InterfaceLocale) -> &'static str {
        match self {
            Self::Mainland => locale.text("出口不在中国大陆", "Exit is outside mainland China"),
            Self::ExitRule => locale.text("出口符合出口规则", "Exit matches your rule"),
            Self::LoginApiMatch => {
                locale.text("登录与 API 同一出口", "Sign-in and API share one exit")
            }
            Self::AgentProxyPath => {
                locale.text("终端 agent 走代理", "Terminal agents use the proxy")
            }
            Self::BrowserPath => locale.text(
                "登录浏览器与终端同一出口",
                "Browser and terminal share one exit",
            ),
            Self::TimeZone => locale.text("没有暴露大陆时区", "No mainland China time zone"),
            Self::Ipv6 => locale.text("无 IPv6 绕过", "No IPv6 bypass"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EgressCheckResult {
    Pass,
    Warn,
    Fail,
    Unknown,
}

impl EgressCheckResult {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Warn => "warn",
            Self::Fail => "fail",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EgressCheck {
    pub kind: EgressCheckKind,
    pub family: Option<EgressFamily>,
    pub result: EgressCheckResult,
    pub facts: HashMap<String, String>,
}

impl EgressCheck {
    fn fact(&self, key: &str) -> &str {
        self.facts.get(key).map(String::as_str).unwrap_or_default()
    }

    fn visible(&self) -> bool {
        self.family != Some(EgressFamily::Google) || self.kind != EgressCheckKind::LoginApiMatch
    }

    fn view(&self, family_exit: Option<&EgressExit>, locale: InterfaceLocale) -> EgressCheckView {
        EgressCheckView {
            title: self.kind.title(locale),
            detail: self.detail(family_exit, locale),
            result: self.result,
        }
    }

    fn detail(&self, family_exit: Option<&EgressExit>, locale: InterfaceLocale) -> Option<String> {
        use EgressCheckKind::*;
        let zh = locale == InterfaceLocale::Chinese;
        let host = self.fact("host");
        if self.result == EgressCheckResult::Unknown {
            let error = self.fact("error");
            if error.is_empty() && self.fact("reason") == "notCollected" {
                return Some(locale.text("本次未采集", "Not collected in this check").into());
            }
            let target = if host.is_empty() {
                String::new()
            } else {
                format!("{host} ")
            };
            let reason = match error {
                "unreachable" => format!("{target}{}", locale.text("连不上", "unreachable")),
                "invalid" => format!("{target}{}", locale.text("返回异常", "invalid response")),
                _ if host.is_empty() => locale.text("检测超时", "check timed out").into(),
                _ => format!("{host} {}", locale.text("检测超时", "timed out")),
            };
            return Some(if zh {
                format!("无法确认（{reason}）")
            } else {
                format!("Couldn't confirm ({reason})")
            });
        }

        if self.result == EgressCheckResult::Pass {
            let slow_host = self.fact("slowHost");
            let slow = (!slow_host.is_empty()).then(|| {
                format!(
                    "{slow_host} {}",
                    locale.text("响应慢（>5 秒）", "is slow (>5s)")
                )
            });
            if self.kind == Mainland {
                let ip = family_exit.map_or_else(|| self.fact("ip"), |exit| &exit.ip);
                let code = family_exit.map_or_else(|| self.fact("country"), |exit| &exit.country);
                if !ip.is_empty() || !code.is_empty() {
                    let mut base = country_name(code, locale).into_owned();
                    if !ip.is_empty() {
                        base.push_str(&format!(" · {ip}"));
                    }
                    if let Some(slow) = slow {
                        base.push_str(&format!(" · {slow}"));
                    }
                    return Some(base);
                }
            }
            return slow;
        }

        Some(match self.kind {
            Mainland if zh => format!("{host} 的出口在中国大陆。检查代理是否开启；规则模式下确认该域名走代理。"),
            Mainland => format!("{host} exits from mainland China. Check that your proxy is on and that this domain is routed through it."),
            ExitRule => {
                let baseline = country_name(self.fact("baselineCountry"), locale);
                let current = country_name(self.fact("currentCountry"), locale);
                let ip = self.fact("currentIp");
                if zh {
                    format!("{baseline} → {current}（{ip}）。确认节点；如果是预期变化，点「以当前出口为准」。")
                } else {
                    format!("{baseline} → {current} ({ip}). Check your node; if this change is expected, choose \"Use current exit\".")
                }
            }
            LoginApiMatch => {
                let login_ip = self.fact("loginIp");
                let api_ip = self.fact("apiIp");
                if zh {
                    format!("登录 {login_ip}，API {api_ip}。多半是分流规则把两个域名分到了不同节点。")
                } else {
                    format!("Sign-in {login_ip}, API {api_ip}. A routing rule probably sends these domains through different nodes.")
                }
            }
            AgentProxyPath => locale.text(
                "已开系统代理，但终端里的 agent 不读系统代理。请在 Clash 开启 TUN，或在 shell 配置里设置 HTTPS_PROXY。",
                "A system proxy is on, but terminal agents ignore it. Turn on TUN mode in Clash, or set HTTPS_PROXY in your shell profile.",
            ).into(),
            BrowserPath => {
                let browser_ip = self.fact("browserIp");
                let agent_ip = self.fact("agentIp");
                if zh {
                    format!("浏览器 {browser_ip}，终端 {agent_ip}。授权登录和使用时 IP 不同。")
                } else {
                    format!("Browser {browser_ip}, terminal {agent_ip}. You would sign in and work from different IPs.")
                }
            }
            TimeZone => locale.text(
                "会话会使用大陆时区。到 设置 → Claude Code → 时区，选「自动」。",
                "Sessions would use a mainland China time zone. In Settings → Claude Code → Time zone, choose Automatic.",
            ).into(),
            Ipv6 => locale.text(
                "本机有公网 IPv6，部分 agent 可能经 IPv6 直连。可在 Clash 开启 IPv6 接管，或在系统里关闭 IPv6。",
                "This Mac has public IPv6, so some agents may connect directly over IPv6. Let Clash handle IPv6, or turn IPv6 off.",
            ).into(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EgressTransition {
    Frozen,
    Resumed { id: String, family: EgressFamily },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EgressStatus {
    pub state: EgressState,
    pub families: Vec<EgressFamilyStatus>,
    pub checks: Vec<EgressCheck>,
    pub rule_mode: EgressRule,
    pub protection_enabled: bool,
    pub session_families: HashMap<String, EgressFamily>,
    /// Full membership takes precedence, including explicitly empty entries.
    pub session_family_sets: HashMap<String, Vec<EgressFamily>>,
    /// Local time of the last check, as (hour, minute).
    pub checked_at: Option<(u8, u8)>,
    pub last_transition: Option<EgressTransition>,
}

impl EgressStatus {
    fn family(&self, family: EgressFamily) -> Option<&EgressFamilyStatus> {
        self.families.iter().find(|entry| entry.family == family)
    }

    fn membership(&self, session_id: &str) -> &[EgressFamily] {
        if let Some(set) = self.session_family_sets.get(session_id) {
            set
        } else {
            self.session_families
                .get(session_id)
                .map(std::slice::from_ref)
                .unwrap_or_default()
        }
    }

    fn session_family(&self, session_id: &str) -> Option<EgressFamily> {
        let membership = self.membership(session_id);
        membership
            .iter()
            .copied()
            .find(|&family| {
                self.family(family)
                    .is_some_and(|entry| !entry.overridden && entry.state == EgressState::Abnormal)
            })
            .or_else(|| {
                membership.iter().copied().find(|&family| {
                    self.family(family)
                        .is_some_and(|entry| is_frozen(entry, session_id))
                })
            })
            .or_else(|| membership.first().copied())
    }

    fn checks(&self, family: Option<EgressFamily>) -> impl Iterator<Item = &EgressCheck> {
        self.checks
            .iter()
            .filter(move |check| check.family == family && check.visible())
    }

    fn checked_time(&self) -> Option<String> {
        self.checked_at
            .map(|(hour, minute)| format!("{hour:02}:{minute:02}"))
    }
}

fn is_frozen(entry: &EgressFamilyStatus, session_id: &str) -> bool {
    !entry.overridden && entry.frozen_session_ids.iter().any(|id| id == session_id)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EgressTone {
    Neutral,
    Ok,
    Warn,
    Danger,
    DangerOutline,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EgressChipView {
    pub label: String,
    pub accessible_label: String,
    pub tone: EgressTone,
    pub open: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EgressReference {
    IpQuality,
    BrowserEnvironment,
}

impl EgressReference {
    pub fn url(self) -> &'static str {
        match self {
            Self::IpQuality => "https://ip.net.coffee/",
            Self::BrowserEnvironment => "https://fuck-claude.vercel.app/",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EgressAction {
    TogglePanel,
    DismissPanel,
    Recheck,
    SetRule(EgressRule),
    SetProtection(bool),
    AcceptCurrent(EgressFamily),
    RequestOverride(EgressFamily),
    CancelOverride,
    ConfirmOverride(EgressFamily),
    CopyReport(String),
    OpenReference(EgressReference),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EgressButton {
    pub label: &'static str,
    pub enabled: bool,
    pub action: EgressAction,
}

impl EgressButton {
    fn new(label: &'static str, action: EgressAction) -> Self {
        Self {
            label,
            enabled: true,
            action,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EgressCheckView {
    pub title: &'static str,
    pub detail: Option<String>,
    pub result: EgressCheckResult,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EgressGroupView {
    pub heading: &'static str,
    pub checks: Vec<EgressCheckView>,
    pub accept_current: Option<EgressButton>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EgressPanelView {
    pub title: &'static str,
    pub checked: String,
    pub close_label: &'static str,
    pub recheck: EgressButton,
    pub groups: Vec<EgressGroupView>,
    pub rule_title: &'static str,
    pub rules: [EgressButton; 2],
    pub rule_mode: EgressRule,
    pub protection: EgressButton,
    pub protection_enabled: bool,
    pub reference_title: &'static str,
    pub reference_links: [EgressButton; 2],
    pub reference_note: &'static str,
    pub sampling_note: &'static str,
    pub copy: EgressButton,
    pub copied: Option<&'static str>,
}

/// Popover-local UI state. Callers own the diagnostic snapshot and action effects.
#[derive(Clone, Debug, Default)]
pub struct EgressPanelState {
    pub open: bool,
    /// One recheck or protection change is in flight.
    pub rechecking: bool,
    copied_until: Option<Duration>,
}

impl EgressPanelState {
    pub fn copy_succeeded(&mut self, now: Duration) {
        self.copied_until = Some(now.saturating_add(EGRESS_COPIED_DURATION));
    }

    pub fn chip(
        &self,
        has_bridge: bool,
        status: Option<&EgressStatus>,
        session_id: &str,
        locale: InterfaceLocale,
    ) -> Option<EgressChipView> {
        if !has_bridge {
            return None;
        }
        let family = status.and_then(|status| status.session_family(session_id));
        let (label, tone) = match (status, family) {
            (Some(status), Some(family)) => family_chip(status, family, locale),
            _ => global_chip(status, locale),
        };
        Some(EgressChipView {
            accessible_label: if family.is_some() {
                label.clone()
            } else {
                panel_title(locale).into()
            },
            label,
            tone,
            open: self.open,
        })
    }

    pub fn view(
        &self,
        status: Option<&EgressStatus>,
        locale: InterfaceLocale,
        now: Duration,
    ) -> Option<EgressPanelView> {
        let status = status.filter(|_| self.open)?;
        let mut groups = Vec::new();
        for family in EgressFamily::ALL {
            let entry = status.family(family);
            let checks: Vec<_> = status
                .checks(Some(family))
                .map(|check| check.view(entry.and_then(|entry| entry.exit.as_ref()), locale))
                .collect();
            if entry.is_some() || !checks.is_empty() {
                groups.push(EgressGroupView {
                    heading: family.heading(locale),
                    checks,
                    accept_current: entry
                        .is_some_and(|entry| entry.violation == Some(EgressViolation::Rule))
                        .then(|| EgressButton {
                            enabled: !self.rechecking,
                            ..EgressButton::new(
                                locale.text("以当前出口为准", "Use current exit"),
                                EgressAction::AcceptCurrent(family),
                            )
                        }),
                });
            }
        }
        let local_checks: Vec<_> = status
            .checks(None)
            .map(|check| check.view(None, locale))
            .collect();
        if !local_checks.is_empty() {
            groups.push(EgressGroupView {
                heading: local_heading(locale),
                checks: local_checks,
                accept_current: None,
            });
        }
        Some(EgressPanelView {
            title: panel_title(locale),
            checked: status.checked_time()
                .map(|time| format!("{} {time}", locale.text("检测于", "Checked at")))
                .unwrap_or_else(|| locale.text("出口检查中…", "Checking exit…").into()),
            close_label: locale.text("关闭", "Close"),
            recheck: EgressButton { enabled: !self.rechecking, ..recheck(locale) },
            groups,
            rule_title: rule_title(locale),
            rules: [EgressRule::Country, EgressRule::Ip].map(|rule| EgressButton {
                enabled: !self.rechecking,
                ..EgressButton::new(rule.label(locale), EgressAction::SetRule(rule))
            }),
            rule_mode: status.rule_mode,
            protection: EgressButton {
                enabled: !self.rechecking,
                ..EgressButton::new(locale.text(
                    "出口保护：出口异常时暂停 Claude / Codex / Antigravity 会话",
                    "Exit protection: pause Claude / Codex / Antigravity sessions when the exit is wrong",
                ), EgressAction::SetProtection(!status.protection_enabled))
            },
            protection_enabled: status.protection_enabled,
            reference_title: locale.text("参考检查（不影响状态）", "Reference checks (do not affect status)"),
            reference_links: [
                EgressButton::new(locale.text("IP 质量 ↗", "IP quality ↗"), EgressAction::OpenReference(EgressReference::IpQuality)),
                EgressButton::new(locale.text("浏览器环境 ↗", "Browser environment ↗"), EgressAction::OpenReference(EgressReference::BrowserEnvironment)),
            ],
            reference_note: locale.text(
                "点击后才访问第三方站点，站点会收到你的出口 IP。",
                "Sites are only contacted when you click, and they will see your exit IP.",
            ),
            sampling_note: locale.text(
                "检测为每分钟抽查，无法保证每条连接都使用同一出口。Google 线路使用通用出口检测，不能保证 Google 实际看到的出口。",
                "Checks are sampled every minute and cannot guarantee every connection uses the same exit. Google uses a generic exit check, which cannot guarantee the exit Google actually sees.",
            ),
            copy: EgressButton::new(locale.text("复制报告", "Copy report"), EgressAction::CopyReport(build_egress_report(status, locale))),
            copied: self.copied_until.filter(|until| now < *until).map(|_| locale.text("已复制", "Copied")),
        })
    }
}

fn panel_title(locale: InterfaceLocale) -> &'static str {
    locale.text("网络检查", "Network check")
}

fn local_heading(locale: InterfaceLocale) -> &'static str {
    locale.text("本机", "This Mac")
}

fn rule_title(locale: InterfaceLocale) -> &'static str {
    locale.text("出口规则", "Exit rule")
}

fn recheck(locale: InterfaceLocale) -> EgressButton {
    EgressButton::new(locale.text("重新检查", "Recheck"), EgressAction::Recheck)
}

fn paused_copy(paused: usize, locale: InterfaceLocale) -> String {
    if paused == 0 {
        return locale.text("出口异常", "Exit problem").into();
    }
    match locale {
        InterfaceLocale::Chinese => format!("出口异常 · 已暂停 {paused} 个会话"),
        InterfaceLocale::English => format!("Exit problem · {paused} session(s) paused"),
    }
}

fn warnings_copy(warnings: usize, locale: InterfaceLocale) -> String {
    match locale {
        InterfaceLocale::Chinese => format!("{warnings} 项提醒"),
        InterfaceLocale::English => format!("{warnings} warning(s)"),
    }
}

fn global_chip(status: Option<&EgressStatus>, locale: InterfaceLocale) -> (String, EgressTone) {
    use EgressTone::*;
    let Some(status) = status else {
        return (locale.text("检查中…", "Checking…").into(), Neutral);
    };
    match status.state {
        EgressState::Checking => (locale.text("检查中…", "Checking…").into(), Neutral),
        EgressState::Abnormal => {
            let paused: HashSet<_> = status
                .families
                .iter()
                .flat_map(|entry| &entry.frozen_session_ids)
                .collect();
            (paused_copy(paused.len(), locale), Danger)
        }
        EgressState::Warning => {
            let count = status
                .checks
                .iter()
                .filter(|check| {
                    check.visible()
                        && matches!(
                            check.result,
                            EgressCheckResult::Warn | EgressCheckResult::Fail
                        )
                })
                .count();
            (warnings_copy(count.max(1), locale), Warn)
        }
        EgressState::Ok => (locale.text("出口正常", "Exit OK").into(), Ok),
        EgressState::Unknown => (locale.text("无法确认出口", "Exit unconfirmed").into(), Warn),
    }
}

fn family_chip(
    status: &EgressStatus,
    family: EgressFamily,
    locale: InterfaceLocale,
) -> (String, EgressTone) {
    use EgressTone::*;
    let entry = status
        .family(family)
        .filter(|_| status.state != EgressState::Checking && status.checked_at.is_some());
    let Some(entry) = entry else {
        return (locale.text("出口检查中…", "Checking exit…").into(), Neutral);
    };
    if entry.overridden {
        return (
            locale
                .text("出口异常 · 已手动继续", "Exit problem · Continued manually")
                .into(),
            DangerOutline,
        );
    }
    if entry.state == EgressState::Abnormal && !entry.frozen_session_ids.is_empty() {
        return (paused_copy(entry.frozen_session_ids.len(), locale), Danger);
    }
    if entry.state == EgressState::Unknown {
        return (locale.text("无法确认出口", "Exit unconfirmed").into(), Warn);
    }
    let region = country_label(entry.exit.as_ref().map_or("", |exit| &exit.country), locale);
    let warnings = status
        .checks
        .iter()
        .filter(|check| {
            check.visible()
                && (check.family.is_none() || check.family == Some(family))
                && check.result == EgressCheckResult::Warn
        })
        .count();
    if warnings > 0 {
        (
            format!("{region} · {}", warnings_copy(warnings, locale)),
            Warn,
        )
    } else {
        (
            format!("{region} · {}", locale.text("出口正常", "Exit OK")),
            Ok,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EgressConfirmView {
    pub title: &'static str,
    pub body: &'static str,
    pub cancel: EgressButton,
    pub confirm: EgressButton,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EgressBannerView {
    pub message: String,
    pub resumed: bool,
    pub recheck: Option<EgressButton>,
    pub override_button: Option<EgressButton>,
    pub confirm: Option<EgressConfirmView>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum BannerStatus {
    Hidden,
    Frozen {
        family: EgressFamily,
        violation: EgressViolation,
        baseline: String,
        current: String,
    },
    Resumed,
}

/// One instance per session card. Synchronize it with the current snapshot before handling clicks.
#[derive(Clone, Debug)]
pub struct EgressBannerState {
    status: BannerStatus,
    confirming: bool,
    rechecking: bool,
    overriding: bool,
    resumed: Option<(String, Duration)>,
}

impl Default for EgressBannerState {
    fn default() -> Self {
        Self {
            status: BannerStatus::Hidden,
            confirming: false,
            rechecking: false,
            overriding: false,
            resumed: None,
        }
    }
}

impl EgressBannerState {
    pub fn sync(&mut self, status: Option<&EgressStatus>, session_id: &str, now: Duration) {
        let next = self.select(status, session_id, now);
        if self.status != next {
            self.confirming = false;
        }
        self.status = next;
    }

    fn select(
        &mut self,
        status: Option<&EgressStatus>,
        session_id: &str,
        now: Duration,
    ) -> BannerStatus {
        let Some(status) = status else {
            return BannerStatus::Hidden;
        };
        let Some(family) = status.session_family(session_id) else {
            return BannerStatus::Hidden;
        };
        let Some(entry) = status.family(family) else {
            return BannerStatus::Hidden;
        };
        let frozen = status.membership(session_id).iter().copied().any(|family| {
            status
                .family(family)
                .is_some_and(|entry| is_frozen(entry, session_id))
        });
        if frozen {
            let violation = entry.violation.unwrap_or(EgressViolation::Mainland);
            let rule_check = status
                .checks(Some(family))
                .find(|check| check.kind == EgressCheckKind::ExitRule);
            let country = |exit: Option<&EgressExit>, fact| {
                exit.map_or_else(
                    || rule_check.map_or("", |check| check.fact(fact)),
                    |exit| &exit.country,
                )
                .to_owned()
            };
            return BannerStatus::Frozen {
                family,
                violation,
                baseline: country(entry.baseline.as_ref(), "baselineCountry"),
                current: country(entry.exit.as_ref(), "currentCountry"),
            };
        }
        if let Some(EgressTransition::Resumed { id, family }) = &status.last_transition
            && status.membership(session_id).contains(family)
        {
            if self
                .resumed
                .as_ref()
                .is_none_or(|(transition, _)| transition != id)
            {
                self.resumed = Some((id.clone(), now.saturating_add(EGRESS_RESUMED_DURATION)));
            }
            return BannerStatus::Resumed;
        }
        BannerStatus::Hidden
    }

    /// Return only actions that require a runtime request. Reject a stale confirmation.
    pub fn dispatch(&mut self, action: EgressAction) -> Option<EgressAction> {
        let BannerStatus::Frozen { family, .. } = self.status else {
            return None;
        };
        match action {
            EgressAction::Recheck if !self.rechecking && !self.overriding => {
                self.rechecking = true;
                return Some(EgressAction::Recheck);
            }
            EgressAction::RequestOverride(selected) if family == selected && !self.overriding => {
                self.confirming = true
            }
            EgressAction::CancelOverride => self.confirming = false,
            EgressAction::ConfirmOverride(selected)
                if family == selected && self.confirming && !self.overriding =>
            {
                self.confirming = false;
                self.overriding = true;
                return Some(EgressAction::ConfirmOverride(family));
            }
            _ => {}
        }
        None
    }

    pub fn recheck_finished(&mut self) {
        self.rechecking = false;
    }

    pub fn override_finished(&mut self) {
        self.overriding = false;
    }

    pub fn view(&self, locale: InterfaceLocale, now: Duration) -> Option<EgressBannerView> {
        match &self.status {
            BannerStatus::Resumed
                if self.resumed.as_ref().is_some_and(|(_, until)| now < *until) =>
            {
                Some(EgressBannerView {
                    message: locale
                        .text(
                            "出口已恢复，已自动继续",
                            "Exit restored; resumed automatically",
                        )
                        .into(),
                    resumed: true,
                    recheck: None,
                    override_button: None,
                    confirm: None,
                })
            }
            BannerStatus::Frozen {
                family,
                violation,
                baseline,
                current,
            } => {
                let message = match violation {
                    EgressViolation::Mainland => locale.text(
                        "出口异常（检测到中国大陆 IP），已暂停。网络恢复后自动继续。",
                        "Exit problem (mainland China IP detected). Paused; resumes automatically when the network recovers.",
                    ).into(),
                    EgressViolation::Rule => {
                        let baseline = country_name(baseline, locale);
                        let current = country_name(current, locale);
                        match locale {
                            InterfaceLocale::Chinese => format!("出口与规则不符（{baseline} → {current}），已暂停。网络恢复后自动继续。"),
                            InterfaceLocale::English => format!("Exit doesn't match your rule ({baseline} → {current}). Paused; resumes automatically when the network recovers."),
                        }
                    }
                };
                Some(EgressBannerView {
                    message, resumed: false,
                    recheck: Some(EgressButton { enabled: !self.rechecking && !self.overriding, ..recheck(locale) }),
                    override_button: Some(EgressButton {
                        enabled: !self.overriding,
                        ..EgressButton::new(locale.text("仍要继续", "Continue anyway"), EgressAction::RequestOverride(*family))
                    }),
                    confirm: self.confirming.then(|| EgressConfirmView {
                        title: locale.text("仍要继续？", "Continue anyway?"),
                        body: locale.text(
                            "出口仍然异常，继续后 agent 会用当前出口访问服务商。出口再次变化前不会再暂停。",
                            "The exit is still wrong. Agents will reach the provider through it, and they won't be paused again until the exit changes.",
                        ),
                        cancel: EgressButton::new(locale.text("取消", "Cancel"), EgressAction::CancelOverride),
                        confirm: EgressButton::new(locale.text("继续", "Continue"), EgressAction::ConfirmOverride(*family)),
                    }),
                })
            }
            _ => None,
        }
    }
}

pub fn build_egress_report(status: &EgressStatus, locale: InterfaceLocale) -> String {
    let checked = status.checked_time().unwrap_or_else(|| "—".into());
    let protection = if status.protection_enabled {
        locale.text("已开启", "On")
    } else {
        locale.text("已关闭", "Off")
    };
    let mut lines = vec![
        format!("{} ({checked})", panel_title(locale)),
        format!("{}: {}", rule_title(locale), status.rule_mode.label(locale)),
        format!(
            "{}: {protection}",
            locale.text("出口保护", "Exit protection")
        ),
    ];
    for family in EgressFamily::ALL {
        let Some(entry) = status.family(family) else {
            continue;
        };
        let summary = entry
            .exit
            .as_ref()
            .map(|exit| format!(" — {} · {}", country_name(&exit.country, locale), exit.ip))
            .unwrap_or_default();
        lines.push(String::new());
        lines.push(format!(
            "[{}] {}{summary}",
            family.heading(locale),
            entry.state.as_str()
        ));
        for check in status.checks(Some(family)) {
            lines.push(report_check(check, entry.exit.as_ref(), locale));
        }
    }

    let mut local = status.checks(None).peekable();
    if local.peek().is_some() {
        lines.push(String::new());
        lines.push(format!("[{}]", local_heading(locale)));
        lines.extend(local.map(|check| report_check(check, None, locale)));
    }

    redact_ips_in_text(&lines.join("\n"))
}

fn report_check(check: &EgressCheck, exit: Option<&EgressExit>, locale: InterfaceLocale) -> String {
    let view = check.view(exit, locale);
    let detail = view
        .detail
        .map(|detail| format!(" — {detail}"))
        .unwrap_or_default();
    format!("- [{}] {}{detail}", check.result.as_str(), view.title)
}

/// Mask IPv4 and IPv6 address literals, including compressed IPv6 and addresses with ports.
pub fn redact_ips_in_text(text: &str) -> String {
    let mut redacted = String::with_capacity(text.len());
    let mut start = 0;
    for (index, ch) in text.char_indices() {
        if !ch.is_ascii_alphanumeric() && ch != '.' && ch != ':' {
            push_redacted_token(&mut redacted, &text[start..index]);
            redacted.push(ch);
            start = index + ch.len_utf8();
        }
    }
    push_redacted_token(&mut redacted, &text[start..]);
    redacted
}

fn push_redacted_token(out: &mut String, token: &str) {
    let trimmed = token.trim_start_matches('.');
    let address = trimmed.trim_end_matches('.');
    let mask = |ip: IpAddr| match ip {
        IpAddr::V4(ip) => {
            let [a, b, ..] = ip.octets();
            format!("{a}.{b}.x.x")
        }
        IpAddr::V6(ip) => {
            let [a, b, ..] = ip.segments();
            format!("{a:x}:{b:x}:x:x")
        }
    };
    let replacement = address.parse().ok().map(mask).or_else(|| {
        let (ip, port) = address.rsplit_once(':')?;
        Some(format!(
            "{}:{port}",
            mask(IpAddr::V4(ip.parse::<Ipv4Addr>().ok()?))
        ))
    });
    if let Some(replacement) = replacement {
        out.push_str(&token[..token.len() - trimmed.len()]);
        out.push_str(&replacement);
        out.push_str(&trimmed[address.len()..]);
    } else {
        out.push_str(token);
    }
}

fn country_label(code: &str, locale: InterfaceLocale) -> String {
    let name = country_name(code, locale);
    if code.len() == 2 && code.bytes().all(|byte| byte.is_ascii_alphabetic()) {
        let flag: String = code
            .bytes()
            .map(|byte| {
                char::from_u32(0x1f1e6 + u32::from(byte.to_ascii_uppercase() - b'A')).unwrap()
            })
            .collect();
        format!("{flag} {name}")
    } else {
        name.into_owned()
    }
}

fn country_name(code: &str, locale: InterfaceLocale) -> Cow<'_, str> {
    if code.is_empty() {
        return locale.text("未知地区", "Unknown").into();
    }
    if code.len() != 2 || !code.bytes().all(|byte| byte.is_ascii_alphabetic()) {
        return code.into();
    }
    let upper = code.to_ascii_uppercase();
    match COUNTRY_NAMES.binary_search_by_key(&upper.as_str(), |(code, _, _)| code) {
        Ok(index) => {
            let (_, zh, en) = COUNTRY_NAMES[index];
            locale.text(zh, en).into()
        }
        Err(_) => upper.into(),
    }
}

// Region names from Intl.DisplayNames (zh-CN and en), matching the frozen renderer.
const COUNTRY_NAMES: &[(&str, &str, &str)] = &[
    ("AC", "阿森松岛", "Ascension Island"),
    ("AD", "安道尔", "Andorra"),
    ("AE", "阿拉伯联合酋长国", "United Arab Emirates"),
    ("AF", "阿富汗", "Afghanistan"),
    ("AG", "安提瓜和巴布达", "Antigua & Barbuda"),
    ("AI", "安圭拉", "Anguilla"),
    ("AL", "阿尔巴尼亚", "Albania"),
    ("AM", "亚美尼亚", "Armenia"),
    ("AN", "库拉索", "Curaçao"),
    ("AO", "安哥拉", "Angola"),
    ("AQ", "南极洲", "Antarctica"),
    ("AR", "阿根廷", "Argentina"),
    ("AS", "美属萨摩亚", "American Samoa"),
    ("AT", "奥地利", "Austria"),
    ("AU", "澳大利亚", "Australia"),
    ("AW", "阿鲁巴", "Aruba"),
    ("AX", "奥兰群岛", "Åland Islands"),
    ("AZ", "阿塞拜疆", "Azerbaijan"),
    ("BA", "波斯尼亚和黑塞哥维那", "Bosnia & Herzegovina"),
    ("BB", "巴巴多斯", "Barbados"),
    ("BD", "孟加拉国", "Bangladesh"),
    ("BE", "比利时", "Belgium"),
    ("BF", "布基纳法索", "Burkina Faso"),
    ("BG", "保加利亚", "Bulgaria"),
    ("BH", "巴林", "Bahrain"),
    ("BI", "布隆迪", "Burundi"),
    ("BJ", "贝宁", "Benin"),
    ("BL", "圣巴泰勒米", "St. Barthélemy"),
    ("BM", "百慕大", "Bermuda"),
    ("BN", "文莱", "Brunei"),
    ("BO", "玻利维亚", "Bolivia"),
    ("BQ", "荷属加勒比区", "Caribbean Netherlands"),
    ("BR", "巴西", "Brazil"),
    ("BS", "巴哈马", "Bahamas"),
    ("BT", "不丹", "Bhutan"),
    ("BU", "缅甸", "Myanmar (Burma)"),
    ("BV", "布韦岛", "Bouvet Island"),
    ("BW", "博茨瓦纳", "Botswana"),
    ("BY", "白俄罗斯", "Belarus"),
    ("BZ", "伯利兹", "Belize"),
    ("CA", "加拿大", "Canada"),
    ("CC", "科科斯（基林）群岛", "Cocos (Keeling) Islands"),
    ("CD", "刚果（金）", "Congo - Kinshasa"),
    ("CF", "中非共和国", "Central African Republic"),
    ("CG", "刚果（布）", "Congo - Brazzaville"),
    ("CH", "瑞士", "Switzerland"),
    ("CI", "科特迪瓦", "Côte d’Ivoire"),
    ("CK", "库克群岛", "Cook Islands"),
    ("CL", "智利", "Chile"),
    ("CM", "喀麦隆", "Cameroon"),
    ("CN", "中国", "China"),
    ("CO", "哥伦比亚", "Colombia"),
    ("CP", "克利珀顿岛", "Clipperton Island"),
    ("CQ", "CQ", "Sark"),
    ("CR", "哥斯达黎加", "Costa Rica"),
    ("CS", "塞尔维亚", "Serbia"),
    ("CU", "古巴", "Cuba"),
    ("CV", "佛得角", "Cape Verde"),
    ("CW", "库拉索", "Curaçao"),
    ("CX", "圣诞岛", "Christmas Island"),
    ("CY", "塞浦路斯", "Cyprus"),
    ("CZ", "捷克", "Czechia"),
    ("DD", "德国", "Germany"),
    ("DE", "德国", "Germany"),
    ("DG", "迪戈加西亚岛", "Diego Garcia"),
    ("DJ", "吉布提", "Djibouti"),
    ("DK", "丹麦", "Denmark"),
    ("DM", "多米尼克", "Dominica"),
    ("DO", "多米尼加共和国", "Dominican Republic"),
    ("DY", "贝宁", "Benin"),
    ("DZ", "阿尔及利亚", "Algeria"),
    ("EA", "休达及梅利利亚", "Ceuta & Melilla"),
    ("EC", "厄瓜多尔", "Ecuador"),
    ("EE", "爱沙尼亚", "Estonia"),
    ("EG", "埃及", "Egypt"),
    ("EH", "西撒哈拉", "Western Sahara"),
    ("ER", "厄立特里亚", "Eritrea"),
    ("ES", "西班牙", "Spain"),
    ("ET", "埃塞俄比亚", "Ethiopia"),
    ("EU", "欧盟", "European Union"),
    ("EZ", "欧元区", "Eurozone"),
    ("FI", "芬兰", "Finland"),
    ("FJ", "斐济", "Fiji"),
    ("FK", "福克兰群岛", "Falkland Islands"),
    ("FM", "密克罗尼西亚", "Micronesia"),
    ("FO", "法罗群岛", "Faroe Islands"),
    ("FR", "法国", "France"),
    ("FX", "法国", "France"),
    ("GA", "加蓬", "Gabon"),
    ("GB", "英国", "United Kingdom"),
    ("GD", "格林纳达", "Grenada"),
    ("GE", "格鲁吉亚", "Georgia"),
    ("GF", "法属圭亚那", "French Guiana"),
    ("GG", "根西岛", "Guernsey"),
    ("GH", "加纳", "Ghana"),
    ("GI", "直布罗陀", "Gibraltar"),
    ("GL", "格陵兰", "Greenland"),
    ("GM", "冈比亚", "Gambia"),
    ("GN", "几内亚", "Guinea"),
    ("GP", "瓜德罗普", "Guadeloupe"),
    ("GQ", "赤道几内亚", "Equatorial Guinea"),
    ("GR", "希腊", "Greece"),
    (
        "GS",
        "南乔治亚和南桑威奇群岛",
        "South Georgia & South Sandwich Islands",
    ),
    ("GT", "危地马拉", "Guatemala"),
    ("GU", "关岛", "Guam"),
    ("GW", "几内亚比绍", "Guinea-Bissau"),
    ("GY", "圭亚那", "Guyana"),
    ("HK", "中国香港特别行政区", "Hong Kong SAR China"),
    ("HM", "赫德岛和麦克唐纳群岛", "Heard & McDonald Islands"),
    ("HN", "洪都拉斯", "Honduras"),
    ("HR", "克罗地亚", "Croatia"),
    ("HT", "海地", "Haiti"),
    ("HU", "匈牙利", "Hungary"),
    ("HV", "布基纳法索", "Burkina Faso"),
    ("IC", "加纳利群岛", "Canary Islands"),
    ("ID", "印度尼西亚", "Indonesia"),
    ("IE", "爱尔兰", "Ireland"),
    ("IL", "以色列", "Israel"),
    ("IM", "马恩岛", "Isle of Man"),
    ("IN", "印度", "India"),
    ("IO", "英属印度洋领地", "British Indian Ocean Territory"),
    ("IQ", "伊拉克", "Iraq"),
    ("IR", "伊朗", "Iran"),
    ("IS", "冰岛", "Iceland"),
    ("IT", "意大利", "Italy"),
    ("JE", "泽西岛", "Jersey"),
    ("JM", "牙买加", "Jamaica"),
    ("JO", "约旦", "Jordan"),
    ("JP", "日本", "Japan"),
    ("KE", "肯尼亚", "Kenya"),
    ("KG", "吉尔吉斯斯坦", "Kyrgyzstan"),
    ("KH", "柬埔寨", "Cambodia"),
    ("KI", "基里巴斯", "Kiribati"),
    ("KM", "科摩罗", "Comoros"),
    ("KN", "圣基茨和尼维斯", "St. Kitts & Nevis"),
    ("KP", "朝鲜", "North Korea"),
    ("KR", "韩国", "South Korea"),
    ("KW", "科威特", "Kuwait"),
    ("KY", "开曼群岛", "Cayman Islands"),
    ("KZ", "哈萨克斯坦", "Kazakhstan"),
    ("LA", "老挝", "Laos"),
    ("LB", "黎巴嫩", "Lebanon"),
    ("LC", "圣卢西亚", "St. Lucia"),
    ("LI", "列支敦士登", "Liechtenstein"),
    ("LK", "斯里兰卡", "Sri Lanka"),
    ("LR", "利比里亚", "Liberia"),
    ("LS", "莱索托", "Lesotho"),
    ("LT", "立陶宛", "Lithuania"),
    ("LU", "卢森堡", "Luxembourg"),
    ("LV", "拉脱维亚", "Latvia"),
    ("LY", "利比亚", "Libya"),
    ("MA", "摩洛哥", "Morocco"),
    ("MC", "摩纳哥", "Monaco"),
    ("MD", "摩尔多瓦", "Moldova"),
    ("ME", "黑山", "Montenegro"),
    ("MF", "法属圣马丁", "St. Martin"),
    ("MG", "马达加斯加", "Madagascar"),
    ("MH", "马绍尔群岛", "Marshall Islands"),
    ("MK", "北马其顿", "North Macedonia"),
    ("ML", "马里", "Mali"),
    ("MM", "缅甸", "Myanmar (Burma)"),
    ("MN", "蒙古", "Mongolia"),
    ("MO", "中国澳门特别行政区", "Macao SAR China"),
    ("MP", "北马里亚纳群岛", "Northern Mariana Islands"),
    ("MQ", "马提尼克", "Martinique"),
    ("MR", "毛里塔尼亚", "Mauritania"),
    ("MS", "蒙特塞拉特", "Montserrat"),
    ("MT", "马耳他", "Malta"),
    ("MU", "毛里求斯", "Mauritius"),
    ("MV", "马尔代夫", "Maldives"),
    ("MW", "马拉维", "Malawi"),
    ("MX", "墨西哥", "Mexico"),
    ("MY", "马来西亚", "Malaysia"),
    ("MZ", "莫桑比克", "Mozambique"),
    ("NA", "纳米比亚", "Namibia"),
    ("NC", "新喀里多尼亚", "New Caledonia"),
    ("NE", "尼日尔", "Niger"),
    ("NF", "诺福克岛", "Norfolk Island"),
    ("NG", "尼日利亚", "Nigeria"),
    ("NH", "瓦努阿图", "Vanuatu"),
    ("NI", "尼加拉瓜", "Nicaragua"),
    ("NL", "荷兰", "Netherlands"),
    ("NO", "挪威", "Norway"),
    ("NP", "尼泊尔", "Nepal"),
    ("NR", "瑙鲁", "Nauru"),
    ("NU", "纽埃", "Niue"),
    ("NZ", "新西兰", "New Zealand"),
    ("OM", "阿曼", "Oman"),
    ("PA", "巴拿马", "Panama"),
    ("PE", "秘鲁", "Peru"),
    ("PF", "法属波利尼西亚", "French Polynesia"),
    ("PG", "巴布亚新几内亚", "Papua New Guinea"),
    ("PH", "菲律宾", "Philippines"),
    ("PK", "巴基斯坦", "Pakistan"),
    ("PL", "波兰", "Poland"),
    ("PM", "圣皮埃尔和密克隆群岛", "St. Pierre & Miquelon"),
    ("PN", "皮特凯恩群岛", "Pitcairn Islands"),
    ("PR", "波多黎各", "Puerto Rico"),
    ("PS", "巴勒斯坦领土", "Palestinian Territories"),
    ("PT", "葡萄牙", "Portugal"),
    ("PW", "帕劳", "Palau"),
    ("PY", "巴拉圭", "Paraguay"),
    ("QA", "卡塔尔", "Qatar"),
    ("QO", "大洋洲边远群岛", "Outlying Oceania"),
    ("RE", "留尼汪", "Réunion"),
    ("RH", "津巴布韦", "Zimbabwe"),
    ("RO", "罗马尼亚", "Romania"),
    ("RS", "塞尔维亚", "Serbia"),
    ("RU", "俄罗斯", "Russia"),
    ("RW", "卢旺达", "Rwanda"),
    ("SA", "沙特阿拉伯", "Saudi Arabia"),
    ("SB", "所罗门群岛", "Solomon Islands"),
    ("SC", "塞舌尔", "Seychelles"),
    ("SD", "苏丹", "Sudan"),
    ("SE", "瑞典", "Sweden"),
    ("SG", "新加坡", "Singapore"),
    ("SH", "圣赫勒拿", "St. Helena"),
    ("SI", "斯洛文尼亚", "Slovenia"),
    ("SJ", "斯瓦尔巴和扬马延", "Svalbard & Jan Mayen"),
    ("SK", "斯洛伐克", "Slovakia"),
    ("SL", "塞拉利昂", "Sierra Leone"),
    ("SM", "圣马力诺", "San Marino"),
    ("SN", "塞内加尔", "Senegal"),
    ("SO", "索马里", "Somalia"),
    ("SR", "苏里南", "Suriname"),
    ("SS", "南苏丹", "South Sudan"),
    ("ST", "圣多美和普林西比", "São Tomé & Príncipe"),
    ("SU", "俄罗斯", "Russia"),
    ("SV", "萨尔瓦多", "El Salvador"),
    ("SX", "荷属圣马丁", "Sint Maarten"),
    ("SY", "叙利亚", "Syria"),
    ("SZ", "斯威士兰", "Eswatini"),
    ("TA", "特里斯坦-达库尼亚群岛", "Tristan da Cunha"),
    ("TC", "特克斯和凯科斯群岛", "Turks & Caicos Islands"),
    ("TD", "乍得", "Chad"),
    ("TF", "法属南部领地", "French Southern Territories"),
    ("TG", "多哥", "Togo"),
    ("TH", "泰国", "Thailand"),
    ("TJ", "塔吉克斯坦", "Tajikistan"),
    ("TK", "托克劳", "Tokelau"),
    ("TL", "东帝汶", "Timor-Leste"),
    ("TM", "土库曼斯坦", "Turkmenistan"),
    ("TN", "突尼斯", "Tunisia"),
    ("TO", "汤加", "Tonga"),
    ("TP", "东帝汶", "Timor-Leste"),
    ("TR", "土耳其", "Türkiye"),
    ("TT", "特立尼达和多巴哥", "Trinidad & Tobago"),
    ("TV", "图瓦卢", "Tuvalu"),
    ("TW", "台湾", "Taiwan"),
    ("TZ", "坦桑尼亚", "Tanzania"),
    ("UA", "乌克兰", "Ukraine"),
    ("UG", "乌干达", "Uganda"),
    ("UK", "英国", "United Kingdom"),
    ("UM", "美国本土外小岛屿", "U.S. Outlying Islands"),
    ("UN", "联合国", "United Nations"),
    ("US", "美国", "United States"),
    ("UY", "乌拉圭", "Uruguay"),
    ("UZ", "乌兹别克斯坦", "Uzbekistan"),
    ("VA", "梵蒂冈", "Vatican City"),
    ("VC", "圣文森特和格林纳丁斯", "St. Vincent & Grenadines"),
    ("VD", "越南", "Vietnam"),
    ("VE", "委内瑞拉", "Venezuela"),
    ("VG", "英属维尔京群岛", "British Virgin Islands"),
    ("VI", "美属维尔京群岛", "U.S. Virgin Islands"),
    ("VN", "越南", "Vietnam"),
    ("VU", "瓦努阿图", "Vanuatu"),
    ("WF", "瓦利斯和富图纳", "Wallis & Futuna"),
    ("WS", "萨摩亚", "Samoa"),
    ("XA", "伪地区", "Pseudo-Accents"),
    ("XB", "伪双向语言地区", "Pseudo-Bidi"),
    ("XK", "科索沃", "Kosovo"),
    ("YD", "也门", "Yemen"),
    ("YE", "也门", "Yemen"),
    ("YT", "马约特", "Mayotte"),
    ("YU", "塞尔维亚", "Serbia"),
    ("ZA", "南非", "South Africa"),
    ("ZM", "赞比亚", "Zambia"),
    ("ZR", "刚果（金）", "Congo - Kinshasa"),
    ("ZW", "津巴布韦", "Zimbabwe"),
    ("ZZ", "未知地区", "Unknown Region"),
];

// Egress chip, diagnostics panel and freeze/resume banner, appended to the
// native egress module. Frozen source: egress/{EgressChip,EgressPanel,
// EgressFreezeBanner}.tsx + egress.css.

mod egress_ui {
    use super::*;
    use gpui::{prelude::*, *};
    use std::rc::Rc;

    #[derive(Clone, Copy)]
    struct EgressPalette {
        bg: Rgba,
        surface: Rgba,
        border: Rgba,
        text: Rgba,
        muted: Rgba,
        accent: Rgba,
        pass: Rgba,
        warn: Rgba,
        warn_bg: Rgba,
        danger: Rgba,
        danger_bg: Rgba,
    }

    impl EgressPalette {
        fn new(dark: bool) -> Self {
            if dark {
                Self {
                    bg: rgb(0x1e242d),
                    surface: rgb(0x252c37),
                    border: rgb(0x364152),
                    text: rgb(0xe1e6ed),
                    muted: rgb(0x9aa9be),
                    accent: rgb(0x58a6ff),
                    pass: rgb(0x3fb950),
                    warn: rgb(0xd29922),
                    warn_bg: rgba(0xd299221f),
                    danger: rgb(0xf47067),
                    danger_bg: rgba(0xe45d5524),
                }
            } else {
                Self {
                    bg: rgb(0xffffff),
                    surface: rgb(0xf7f9fc),
                    border: rgb(0xd9e3f1),
                    text: rgb(0x283446),
                    muted: rgb(0x687b96),
                    accent: rgb(0x287cdd),
                    pass: rgb(0x1f883d),
                    warn: rgb(0x9a6700),
                    warn_bg: rgb(0xfff8e8),
                    danger: rgb(0xc53929),
                    danger_bg: rgb(0xfff2f0),
                }
            }
        }
    }

    fn egress_shadow(color: u32, alpha: f32, y: f32, blur: f32) -> BoxShadow {
        let mut color: Hsla = rgb(color).into();
        color.a = alpha;
        BoxShadow {
            color,
            offset: point(px(0.), px(y)),
            blur_radius: px(blur),
            spread_radius: px(0.),
        }
    }

    fn egress_chip_colors(tone: EgressTone, dark: bool) -> (Rgba, Rgba, Rgba, bool) {
        match tone {
            EgressTone::Neutral => (
                if dark { rgb(0x8b949e) } else { rgb(0x606572) },
                rgba(0x00000000),
                rgba(0x00000000),
                false,
            ),
            EgressTone::Ok => (
                if dark { rgb(0x8b949e) } else { rgb(0x606572) },
                rgba(0x00000000),
                rgba(0x00000000),
                true,
            ),
            EgressTone::Warn => (
                if dark { rgb(0xd29922) } else { rgb(0x9a6700) },
                if dark {
                    rgba(0xd299221f)
                } else {
                    rgba(0xd2992224)
                },
                rgba(0x00000000),
                false,
            ),
            EgressTone::Danger => (
                if dark { rgb(0xf47067) } else { rgb(0xb42318) },
                if dark {
                    rgba(0xe45d5526)
                } else {
                    rgb(0xfeeceb)
                },
                rgba(0x00000000),
                false,
            ),
            EgressTone::DangerOutline => (
                if dark { rgb(0xf47067) } else { rgb(0xb42318) },
                rgba(0x00000000),
                if dark {
                    rgba(0xf47067a6)
                } else {
                    rgba(0xb423188c)
                },
                false,
            ),
        }
    }

    /// Bottom-bar egress chip. A `None` model means the bridge is absent, so the
    /// component renders nothing instead of inventing a placeholder.
    pub fn render_egress_chip(
        view: Option<&EgressChipView>,
        dark: bool,
        on_action: Rc<dyn Fn(EgressAction, &mut Window, &mut App)>,
    ) -> AnyElement {
        render_egress_chip_with_label(view, dark, view.map(|view| view.label.clone()), on_action)
    }

    pub fn render_egress_chip_with_label(
        view: Option<&EgressChipView>,
        dark: bool,
        label: Option<impl IntoElement>,
        on_action: Rc<dyn Fn(EgressAction, &mut Window, &mut App)>,
    ) -> AnyElement {
        let Some(view) = view else {
            return Empty.into_any_element();
        };
        let (text, background, border, ok_dot) = egress_chip_colors(view.tone, dark);
        let dot = if ok_dot { rgb(0x22c55e) } else { text };
        let on_action = on_action.clone();

        div()
            .id("egress-chip")
            .focusable()
            .flex()
            .items_center()
            .gap(px(5.))
            .px(px(7.))
            .py(px(2.))
            .rounded(px(5.))
            .border_1()
            .border_color(border)
            .bg(background)
            .text_color(text)
            .text_size(px(10.))
            .line_height(px(14.))
            .whitespace_nowrap()
            .max_w(px(220.))
            .cursor_pointer()
            .child(div().size(px(5.)).flex_none().rounded_full().bg(dot))
            .child(div().flex_1().min_w(px(0.)).truncate().children(label))
            .on_click(move |_, window, cx| {
                on_action(EgressAction::TogglePanel, window, cx);
            })
            .into_any_element()
    }

    /// The owner supplies the trigger bounds and focus handle for dismissal.
    pub fn render_egress_panel(
        view: Option<&EgressPanelView>,
        dark: bool,
        window: &Window,
        trigger_bounds: Option<Bounds<Pixels>>,
        focus: &FocusHandle,
        on_action: Rc<dyn Fn(EgressAction, &mut Window, &mut App)>,
    ) -> AnyElement {
        let Some(view) = view else {
            return Empty.into_any_element();
        };
        let viewport = window.viewport_size();
        let width = (f32::from(viewport.width) - 24.).clamp(120., 440.);
        let max_height = (f32::from(viewport.height) - 70.).max(120.);
        let palette = EgressPalette::new(dark);

        let header = div()
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(12.))
            .px(px(16.))
            .py(px(14.))
            .border_b(px(1.))
            .border_color(palette.border)
            .child(
                div()
                    .min_w(px(0.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .text_size(px(15.))
                            .font_weight(FontWeight(650.))
                            .text_color(palette.text)
                            .child(view.title),
                    )
                    .child(
                        div()
                            .flex_none()
                            .whitespace_nowrap()
                            .text_size(px(11.))
                            .text_color(palette.muted)
                            .child(view.checked.clone()),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(egress_button(&view.recheck, palette, on_action.clone()))
                    .child(
                        div()
                            .id("egress-panel-close")
                            .focusable()
                            .size(px(26.))
                            .rounded(px(6.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .text_size(px(18.))
                            .line_height(px(18.))
                            .text_color(palette.muted)
                            .hover(move |style| style.bg(rgba(0x7f91aa24)).text_color(palette.text))
                            .on_click({
                                let on_action = on_action.clone();
                                move |_, window, cx| {
                                    on_action(EgressAction::DismissPanel, window, cx);
                                }
                            })
                            .child("×"),
                    ),
            );

        let groups = view
            .groups
            .iter()
            .map(|group| egress_group(group, palette, on_action.clone()))
            .collect::<Vec<_>>();

        let controls = div()
            .pt(px(10.))
            .border_t(px(1.))
            .border_color(palette.border)
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .text_size(px(11.))
                    .font_weight(FontWeight(650.))
                    .text_color(palette.muted)
                    .mb(px(6.))
                    .child(view.rule_title),
            )
            .child(div().flex().flex_wrap().gap(px(14.)).children(
                view.rules.iter().map(|rule| {
                    egress_rule_radio(rule, view.rule_mode, palette, on_action.clone())
                }),
            ))
            .child(egress_protection_toggle(
                &view.protection,
                view.protection_enabled,
                palette,
                on_action.clone(),
            ));

        let reference = egress_reference(view, palette, on_action.clone());

        let scroll = div()
            .id("egress-panel-scroll")
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .gap(px(14.))
            .px(px(16.))
            .py(px(12.))
            .overflow_y_scroll()
            .scrollbar_width(px(4.))
            .children(groups)
            .child(controls)
            .child(reference);

        let footer = div()
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(10.))
            .px(px(16.))
            .py(px(10.))
            .border_t(px(1.))
            .border_color(palette.border)
            .bg(palette.surface)
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .text_size(px(10.))
                    .line_height(px(14.))
                    .text_color(palette.muted)
                    .child(view.sampling_note),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .when_some(view.copied, |wrap, copied| {
                        wrap.child(
                            div()
                                .text_size(px(11.))
                                .font_weight(FontWeight(600.))
                                .text_color(palette.pass)
                                .child(copied),
                        )
                    })
                    .child(egress_button(&view.copy, palette, on_action.clone())),
            );

        let escape_action = on_action.clone();

        div()
            .id("egress-panel")
            .track_focus(focus)
            .on_mouse_down_out(move |event, window, cx| {
                if !trigger_bounds.is_some_and(|bounds| bounds.contains(&event.position)) {
                    on_action(EgressAction::DismissPanel, window, cx);
                }
            })
            .on_key_down(move |event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    escape_action(EgressAction::DismissPanel, window, cx);
                    cx.stop_propagation();
                }
            })
            .relative()
            .occlude()
            .w(px(width))
            .max_h(px(max_height))
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(px(12.))
            .border_1()
            .border_color(palette.border)
            .bg(palette.bg)
            .text_color(palette.text)
            .text_size(px(12.))
            .line_height(px(18.))
            .shadow(vec![
                egress_shadow(0x263d61, if dark { 0.4 } else { 0.15 }, 16., 42.),
                egress_shadow(0x263d61, if dark { 0.1 } else { 0.05 }, 3., 10.),
            ])
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(header)
            .child(scroll)
            .child(footer)
            .into_any_element()
    }

    fn egress_group(
        group: &EgressGroupView,
        palette: EgressPalette,
        on_action: Rc<dyn Fn(EgressAction, &mut Window, &mut App)>,
    ) -> impl IntoElement {
        div()
            .id(group.heading)
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.))
                    .mb(px(6.))
                    .child(
                        div()
                            .text_size(px(11.))
                            .font_weight(FontWeight(650.))
                            .text_color(palette.muted)
                            .child(group.heading),
                    )
                    .when_some(group.accept_current.as_ref(), |header, button| {
                        header.child(egress_subtle_button(button, palette, on_action.clone()))
                    }),
            )
            .child(
                div().flex().flex_col().gap(px(6.)).children(
                    group
                        .checks
                        .iter()
                        .map(|check| egress_check(check, palette)),
                ),
            )
    }

    fn egress_check(check: &EgressCheckView, palette: EgressPalette) -> Div {
        let (glyph, badge_bg, row_bg, row_border, detail) = match check.result {
            EgressCheckResult::Pass => (
                "✓",
                rgba(0x3fb95024),
                palette.surface,
                palette.border,
                palette.muted,
            ),
            EgressCheckResult::Warn => (
                "!",
                rgba(0xd299222e),
                palette.warn_bg,
                rgba(0xd2992259),
                palette.warn,
            ),
            EgressCheckResult::Fail => (
                "✕",
                rgba(0xe45d5533),
                palette.danger_bg,
                rgba(0xe45d5561),
                palette.danger,
            ),
            EgressCheckResult::Unknown => (
                "?",
                rgba(0xd299222e),
                palette.surface,
                palette.border,
                palette.muted,
            ),
        };
        let badge_text = match check.result {
            EgressCheckResult::Pass => palette.pass,
            EgressCheckResult::Warn | EgressCheckResult::Unknown => palette.warn,
            EgressCheckResult::Fail => palette.danger,
        };

        div()
            .flex()
            .items_start()
            .gap(px(9.))
            .px(px(10.))
            .py(px(8.))
            .rounded(px(8.))
            .border_1()
            .border_color(row_border)
            .bg(row_bg)
            .child(
                div()
                    .mt(px(1.))
                    .size(px(17.))
                    .flex_none()
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(badge_bg)
                    .text_size(px(10.))
                    .font_weight(FontWeight(700.))
                    .text_color(badge_text)
                    .child(glyph),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight(600.))
                            .child(check.title),
                    )
                    .when_some(check.detail.clone(), |body, detail_text| {
                        body.child(
                            div()
                                .mt(px(2.))
                                .text_size(px(11.))
                                .line_height(px(15.95))
                                .text_color(detail)
                                .child(detail_text),
                        )
                    }),
            )
    }

    fn egress_button(
        button: &EgressButton,
        palette: EgressPalette,
        on_action: Rc<dyn Fn(EgressAction, &mut Window, &mut App)>,
    ) -> Stateful<Div> {
        let action = button.action.clone();
        let enabled = button.enabled;
        let mut element = div()
            .id(button.label)
            .px(px(10.))
            .py(px(4.))
            .rounded(px(6.))
            .border_1()
            .border_color(palette.border)
            .bg(palette.surface)
            .text_color(palette.text)
            .text_size(px(11.))
            .font_weight(FontWeight(500.))
            .whitespace_nowrap()
            .child(button.label)
            .when(!enabled, |element| element.opacity(0.55).cursor_default());

        if enabled {
            element = element
                .focusable()
                .cursor_pointer()
                .hover(move |style| {
                    style
                        .border_color(palette.accent)
                        .text_color(palette.accent)
                })
                .on_click(move |_, window, cx| {
                    on_action(action.clone(), window, cx);
                });
        }
        element
    }

    fn egress_subtle_button(
        button: &EgressButton,
        palette: EgressPalette,
        on_action: Rc<dyn Fn(EgressAction, &mut Window, &mut App)>,
    ) -> Stateful<Div> {
        let action = button.action.clone();
        let enabled = button.enabled;
        let mut element = div()
            .id(button.label)
            .px(px(8.))
            .py(px(2.))
            .rounded(px(6.))
            .border_1()
            .border_color(palette.danger)
            .bg(rgba(0x00000000))
            .text_color(palette.danger)
            .text_size(px(11.))
            .font_weight(FontWeight(500.))
            .whitespace_nowrap()
            .child(button.label)
            .when(!enabled, |element| element.opacity(0.55).cursor_default());

        if enabled {
            element = element
                .focusable()
                .cursor_pointer()
                .hover(move |style| style.bg(palette.danger_bg))
                .on_click(move |_, window, cx| {
                    on_action(action.clone(), window, cx);
                });
        }
        element
    }

    fn egress_rule_radio(
        button: &EgressButton,
        current: EgressRule,
        palette: EgressPalette,
        on_action: Rc<dyn Fn(EgressAction, &mut Window, &mut App)>,
    ) -> Stateful<Div> {
        let selected = match &button.action {
            EgressAction::SetRule(rule) => *rule == current,
            _ => false,
        };
        let action = button.action.clone();
        let enabled = button.enabled;

        let mut row = div()
            .id(button.label)
            .flex()
            .items_start()
            .gap(px(7.))
            .text_size(px(12.))
            .line_height(px(16.8))
            .text_color(palette.text)
            .child(
                div()
                    .mt(px(2.))
                    .size(px(13.))
                    .flex_none()
                    .rounded_full()
                    .border_1()
                    .border_color(if selected {
                        palette.accent
                    } else {
                        palette.muted
                    })
                    .bg(if selected {
                        palette.accent
                    } else {
                        rgba(0x00000000)
                    })
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(selected, |radio| {
                        radio.child(div().size(px(5.)).rounded_full().bg(palette.bg))
                    }),
            )
            .child(div().flex_1().min_w(px(0.)).child(button.label));

        if enabled {
            row = row
                .focusable()
                .cursor_pointer()
                .on_click(move |_, window, cx| {
                    on_action(action.clone(), window, cx);
                });
        } else {
            row = row.opacity(0.55);
        }
        row
    }

    fn egress_protection_toggle(
        button: &EgressButton,
        checked: bool,
        palette: EgressPalette,
        on_action: Rc<dyn Fn(EgressAction, &mut Window, &mut App)>,
    ) -> Stateful<Div> {
        let action = button.action.clone();
        let enabled = button.enabled;

        let mut row = div()
            .id(button.label)
            .flex()
            .items_start()
            .gap(px(7.))
            .text_size(px(12.))
            .line_height(px(16.8))
            .text_color(palette.text)
            .child(
                div()
                    .mt(px(2.))
                    .size(px(13.))
                    .flex_none()
                    .rounded(px(3.))
                    .border_1()
                    .border_color(if checked {
                        palette.accent
                    } else {
                        palette.muted
                    })
                    .bg(if checked {
                        palette.accent
                    } else {
                        rgba(0x00000000)
                    })
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(9.))
                    .font_weight(FontWeight(700.))
                    .text_color(rgb(0xffffff))
                    .when(checked, |checkbox| checkbox.child("✓")),
            )
            .child(div().flex_1().min_w(px(0.)).child(button.label));

        if enabled {
            row = row
                .focusable()
                .cursor_pointer()
                .on_click(move |_, window, cx| {
                    on_action(action.clone(), window, cx);
                });
        } else {
            row = row.opacity(0.55);
        }
        row
    }

    fn egress_reference(
        view: &EgressPanelView,
        palette: EgressPalette,
        on_action: Rc<dyn Fn(EgressAction, &mut Window, &mut App)>,
    ) -> Div {
        div()
            .pt(px(10.))
            .border_t(px(1.))
            .border_color(palette.border)
            .child(
                div()
                    .mb(px(6.))
                    .text_size(px(11.))
                    .font_weight(FontWeight(650.))
                    .text_color(palette.muted)
                    .child(view.reference_title),
            )
            .child(
                div().flex().flex_wrap().gap(px(10.)).mb(px(5.)).children(
                    view.reference_links
                        .iter()
                        .map(|link| egress_link_button(link, palette, on_action.clone())),
                ),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .line_height(px(15.95))
                    .text_color(palette.muted)
                    .child(view.reference_note),
            )
    }

    fn egress_link_button(
        button: &EgressButton,
        palette: EgressPalette,
        on_action: Rc<dyn Fn(EgressAction, &mut Window, &mut App)>,
    ) -> Stateful<Div> {
        let action = button.action.clone();
        let enabled = button.enabled;
        let mut label = button.label.to_owned();
        if !label.ends_with('↗') {
            label.push_str(" ↗");
        }

        let mut element = div()
            .id(button.label)
            .px(px(8.))
            .py(px(3.))
            .rounded(px(6.))
            .border_1()
            .border_color(palette.border)
            .bg(rgba(0x00000000))
            .text_color(palette.accent)
            .text_size(px(11.))
            .font_weight(FontWeight(500.))
            .whitespace_nowrap()
            .when(!enabled, |element| element.opacity(0.55).cursor_default())
            .child(label);

        if enabled {
            element = element
                .focusable()
                .cursor_pointer()
                .hover(move |style| style.bg(rgba(0x287cdd14)))
                .on_click(move |_, window, cx| {
                    on_action(action.clone(), window, cx);
                });
        }
        element
    }

    /// Card-top freeze/resume banner. The confirmation dialog is a deferred,
    /// full-window layer with its own occluding hitbox, so it is neither clipped by
    /// the terminal pane nor able to leak a click into the recheck/continue buttons.
    /// The owner focuses `confirmation_focus` on opening the confirmation.
    pub fn render_egress_banner(
        view: Option<&EgressBannerView>,
        dark: bool,
        confirmation_focus: &FocusHandle,
        on_action: Rc<dyn Fn(EgressAction, &mut Window, &mut App)>,
    ) -> AnyElement {
        let Some(view) = view else {
            return Empty.into_any_element();
        };

        let (text, background, border, button_bg, button_text, button_border) = if dark {
            if view.resumed {
                (
                    rgb(0x9be9a8),
                    rgb(0x14261a),
                    rgba(0x3fb9504d),
                    rgba(0x00000000),
                    rgb(0x9be9a8),
                    rgba(0x00000000),
                )
            } else {
                (
                    rgb(0xf0b1ad),
                    rgb(0x2b1919),
                    rgba(0xe45d554d),
                    rgba(0xe45d5514),
                    rgb(0xf2b7b3),
                    rgba(0xe45d5573),
                )
            }
        } else if view.resumed {
            (
                rgb(0x1f6f34),
                rgb(0xeffbf2),
                rgb(0xbfe6c7),
                rgba(0x00000000),
                rgb(0x1f6f34),
                rgba(0x00000000),
            )
        } else {
            (
                rgb(0x9b2f2a),
                rgb(0xfff3f2),
                rgb(0xedc2bf),
                rgb(0xffffff),
                rgb(0x9b2f2a),
                rgb(0xe4a39f),
            )
        };

        let mut banner = div()
            .id("egress-freeze-banner")
            .occlude()
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(12.))
            .px(px(10.))
            .py(px(8.))
            .border_t(px(1.))
            .border_b(px(1.))
            .border_color(border)
            .bg(background)
            .text_color(text)
            .text_size(px(11.))
            .line_height(px(15.4))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(div().flex_1().min_w(px(0.)).child(view.message.clone()));

        if !view.resumed {
            let recheck = view.recheck.as_ref().map(|button| {
                egress_banner_button(
                    button,
                    button_bg,
                    button_text,
                    button_border,
                    on_action.clone(),
                )
            });
            let override_button = view.override_button.as_ref().map(|button| {
                egress_banner_button(
                    button,
                    button_bg,
                    button_text,
                    button_border,
                    on_action.clone(),
                )
            });
            if recheck.is_some() || override_button.is_some() {
                banner = banner.child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .children(recheck)
                        .children(override_button),
                );
            }
        }

        if let Some(confirm) = view.confirm.as_ref() {
            banner = banner.child(
                EgressConfirmOverlay {
                    focus: confirmation_focus.clone(),
                    title: confirm.title,
                    body: confirm.body,
                    cancel_label: confirm.cancel.label,
                    cancel_enabled: confirm.cancel.enabled,
                    cancel_action: confirm.cancel.action.clone(),
                    confirm_label: confirm.confirm.label,
                    confirm_enabled: confirm.confirm.enabled,
                    confirm_action: confirm.confirm.action.clone(),
                    dark,
                    on_action,
                }
                .into_any_element(),
            );
        }

        banner.into_any_element()
    }

    fn egress_banner_button(
        button: &EgressButton,
        background: Rgba,
        text: Rgba,
        border: Rgba,
        on_action: Rc<dyn Fn(EgressAction, &mut Window, &mut App)>,
    ) -> Stateful<Div> {
        let action = button.action.clone();
        let enabled = button.enabled;
        let mut element = div()
            .id(button.label)
            .px(px(8.))
            .py(px(3.))
            .rounded(px(5.))
            .border_1()
            .border_color(border)
            .bg(background)
            .text_color(text)
            .text_size(px(11.))
            .whitespace_nowrap()
            .child(button.label)
            .when(!enabled, |element| element.opacity(0.55).cursor_default());

        if enabled {
            element = element
                .focusable()
                .cursor_pointer()
                .hover(move |style| style.bg(rgba(0xe45d5533)))
                .on_click(move |_, window, cx| {
                    on_action(action.clone(), window, cx);
                });
        }
        element
    }

    /// Full-window confirmation layer, sized to the current viewport.
    #[derive(IntoElement)]
    struct EgressConfirmOverlay {
        focus: FocusHandle,
        title: &'static str,
        body: &'static str,
        cancel_label: &'static str,
        cancel_enabled: bool,
        cancel_action: EgressAction,
        confirm_label: &'static str,
        confirm_enabled: bool,
        confirm_action: EgressAction,
        dark: bool,
        on_action: Rc<dyn Fn(EgressAction, &mut Window, &mut App)>,
    }

    impl RenderOnce for EgressConfirmOverlay {
        fn render(self, window: &mut Window, _cx: &mut App) -> impl IntoElement {
            let viewport = window.viewport_size();
            let dialog_width = (f32::from(viewport.width) - 40.).min(420.).max(120.);
            let dialog_bg = if self.dark {
                rgb(0x292929)
            } else {
                rgb(0xffffff)
            };
            let dialog_text = if self.dark {
                rgb(0xf4f4f4)
            } else {
                rgb(0x2f3547)
            };
            let body_text = if self.dark {
                rgba(0xffffffb3)
            } else {
                rgba(0x000000a6)
            };
            let footer_border = if self.dark {
                rgb(0x383838)
            } else {
                rgba(0x0000001f)
            };
            let footer_text = if self.dark {
                rgb(0xf2f2f2)
            } else {
                rgba(0x000000b3)
            };
            let close_color = if self.dark {
                rgba(0xffffff57)
            } else {
                rgb(0x8790a0)
            };

            let cancel_enabled = self.cancel_enabled;
            let confirm_enabled = self.confirm_enabled;
            let escape_action = self.cancel_action.clone();
            let escape_on_action = self.on_action.clone();
            let close_action = self.cancel_action.clone();
            let close_on_action = self.on_action.clone();
            let footer_cancel_action = self.cancel_action.clone();
            let footer_cancel_on_action = self.on_action.clone();
            let footer_confirm_action = self.confirm_action.clone();
            let footer_confirm_on_action = self.on_action.clone();
            let backdrop_action = self.cancel_action.clone();
            let backdrop_on_action = self.on_action.clone();

            let dialog = div()
                .id("egress-confirm-dialog")
                .track_focus(&self.focus)
                .on_key_down(move |event: &KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" {
                        escape_on_action(escape_action.clone(), window, cx);
                        cx.stop_propagation();
                    }
                })
                .relative()
                .w(px(dialog_width))
                .max_w(px((f32::from(viewport.width) - 40.).max(120.)))
                .flex()
                .flex_col()
                .overflow_hidden()
                .rounded(px(12.))
                .bg(dialog_bg)
                .text_color(dialog_text)
                .occlude()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .px(px(24.))
                        .pt(px(20.))
                        .pb(px(16.))
                        .child(
                            div()
                                .text_size(px(16.))
                                .font_weight(FontWeight(600.))
                                .text_color(dialog_text)
                                .child(self.title),
                        )
                        .child(
                            div()
                                .id("egress-confirm-close")
                                .focusable()
                                .size(px(28.))
                                .rounded(px(6.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .text_size(px(20.))
                                .line_height(px(20.))
                                .text_color(close_color)
                                .on_click(move |_, window, cx| {
                                    close_on_action(close_action.clone(), window, cx);
                                })
                                .child("×"),
                        ),
                )
                .child(
                    div()
                        .px(px(24.))
                        .pt(px(8.))
                        .pb(px(24.))
                        .text_size(px(14.))
                        .line_height(px(22.4))
                        .text_color(body_text)
                        .child(self.body),
                )
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(px(10.))
                        .px(px(24.))
                        .pt(px(16.))
                        .pb(px(24.))
                        .child(
                            div()
                                .id("egress-confirm-cancel")
                                .min_w(px(80.))
                                .h(px(32.))
                                .px(px(16.))
                                .rounded(px(8.))
                                .border_1()
                                .border_color(footer_border)
                                .bg(rgba(0x00000000))
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(14.))
                                .text_color(footer_text)
                                .when(!cancel_enabled, |button| {
                                    button.opacity(0.55).cursor_default()
                                })
                                .when(cancel_enabled, move |button| {
                                    button.focusable().cursor_pointer().on_click(
                                        move |_, window, cx| {
                                            footer_cancel_on_action(
                                                footer_cancel_action.clone(),
                                                window,
                                                cx,
                                            );
                                        },
                                    )
                                })
                                .child(self.cancel_label),
                        )
                        .child(
                            div()
                                .id("egress-confirm-primary")
                                .min_w(px(80.))
                                .h(px(32.))
                                .px(px(16.))
                                .rounded(px(8.))
                                .bg(rgb(0xc5443e))
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(14.))
                                .text_color(rgb(0xffffff))
                                .when(!confirm_enabled, |button| {
                                    button.opacity(0.55).cursor_default()
                                })
                                .when(confirm_enabled, move |button| {
                                    button.focusable().cursor_pointer().on_click(
                                        move |_, window, cx| {
                                            footer_confirm_on_action(
                                                footer_confirm_action.clone(),
                                                window,
                                                cx,
                                            );
                                        },
                                    )
                                })
                                .child(self.confirm_label),
                        ),
                );

            deferred(
                anchored().position(point(px(0.), px(0.))).child(
                    div()
                        .id("egress-confirm-backdrop")
                        .occlude()
                        .w(viewport.width)
                        .h(viewport.height)
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(rgba(0x00000094))
                        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                            backdrop_on_action(backdrop_action.clone(), window, cx);
                            cx.stop_propagation();
                        })
                        .child(dialog),
                ),
            )
            .with_priority(120)
        }
    }
}

pub use egress_ui::{
    render_egress_banner, render_egress_chip, render_egress_chip_with_label, render_egress_panel,
};

#[cfg(test)]
mod tests {
    use super::*;
    use EgressCheckKind::{BrowserPath, ExitRule, Ipv6, LoginApiMatch, Mainland};
    use EgressCheckResult::{Fail, Pass, Unknown, Warn};
    use EgressFamily::{Anthropic, ChatGptSubscription, Google, OpenAi};
    use InterfaceLocale::{Chinese, English};

    const NOW: Duration = Duration::from_secs(10);

    fn exit(ip: &str, country: &str) -> EgressExit {
        EgressExit {
            ip: ip.into(),
            country: country.into(),
        }
    }

    fn family(family: EgressFamily, ip: &str, country: &str) -> EgressFamilyStatus {
        EgressFamilyStatus {
            family,
            state: EgressState::Ok,
            exit: Some(exit(ip, country)),
            baseline: None,
            violation: None,
            overridden: false,
            frozen_session_ids: Vec::new(),
        }
    }

    fn status() -> EgressStatus {
        EgressStatus {
            state: EgressState::Ok,
            families: vec![
                family(Google, "198.51.100.111", "JP"),
                family(OpenAi, "198.51.100.50", "SG"),
                family(Anthropic, "203.0.113.77", "US"),
                family(ChatGptSubscription, "2001:db8::7", "GB"),
            ],
            checks: Vec::new(),
            rule_mode: EgressRule::Country,
            protection_enabled: true,
            session_families: [
                ("claude-card".into(), Anthropic),
                ("google-card".into(), Google),
            ]
            .into(),
            session_family_sets: HashMap::new(),
            checked_at: Some((9, 5)),
            last_transition: None,
        }
    }

    fn set_family(status: &mut EgressStatus, family: EgressFamily) -> &mut EgressFamilyStatus {
        status
            .families
            .iter_mut()
            .find(|entry| entry.family == family)
            .unwrap()
    }

    fn check(
        kind: EgressCheckKind,
        family: Option<EgressFamily>,
        result: EgressCheckResult,
        facts: &[(&str, &str)],
    ) -> EgressCheck {
        EgressCheck {
            kind,
            family,
            result,
            facts: facts
                .iter()
                .map(|&(key, value)| (key.into(), value.into()))
                .collect(),
        }
    }

    fn chip(
        status: Option<&EgressStatus>,
        session: &str,
        locale: InterfaceLocale,
    ) -> EgressChipView {
        EgressPanelState::default()
            .chip(true, status, session, locale)
            .unwrap()
    }

    #[test]
    fn global_shell_api_and_unmanaged_cards_have_global_copy() {
        let mut status = status();
        assert!(
            EgressPanelState::default()
                .chip(false, Some(&status), "shell", English)
                .is_none()
        );
        for (locale, title, checking, ok, warning, abnormal, unknown) in [
            (
                Chinese,
                "网络检查",
                "检查中…",
                "出口正常",
                "1 项提醒",
                "出口异常",
                "无法确认出口",
            ),
            (
                English,
                "Network check",
                "Checking…",
                "Exit OK",
                "1 warning(s)",
                "Exit problem",
                "Exit unconfirmed",
            ),
        ] {
            assert_eq!(chip(None, "shell", locale).label, checking);
            for (state, copy, tone) in [
                (EgressState::Checking, checking, EgressTone::Neutral),
                (EgressState::Ok, ok, EgressTone::Ok),
                (EgressState::Warning, warning, EgressTone::Warn),
                (EgressState::Abnormal, abnormal, EgressTone::Danger),
                (EgressState::Unknown, unknown, EgressTone::Warn),
            ] {
                status.state = state;
                for id in ["shell", "claude-api", "unmanaged-codex"] {
                    let view = chip(Some(&status), id, locale);
                    assert_eq!(
                        (&*view.label, &*view.accessible_label, view.tone),
                        (copy, title, tone)
                    );
                }
            }
        }
    }

    #[test]
    fn managed_chips_prioritize_blocked_families_and_scope_warnings() {
        let mut status = status();
        status
            .session_family_sets
            .insert("multi-card".into(), vec![Anthropic, Google]);
        status
            .session_family_sets
            .insert("google-card".into(), vec![]);
        for (locale, ok, warns, paused, global_paused, bypass, checking, unknown, cleared) in [
            (
                Chinese,
                "🇺🇸 美国 · 出口正常",
                "🇺🇸 美国 · 1 项提醒",
                "出口异常 · 已暂停 2 个会话",
                "出口异常 · 已暂停 3 个会话",
                "出口异常 · 已手动继续",
                "出口检查中…",
                "无法确认出口",
                "出口正常",
            ),
            (
                English,
                "🇺🇸 United States · Exit OK",
                "🇺🇸 United States · 1 warning(s)",
                "Exit problem · 2 session(s) paused",
                "Exit problem · 3 session(s) paused",
                "Exit problem · Continued manually",
                "Checking exit…",
                "Exit unconfirmed",
                "Exit OK",
            ),
        ] {
            assert_eq!(chip(Some(&status), "claude-card", locale).label, ok);
            assert_eq!(chip(Some(&status), "google-card", locale).label, cleared);
            status
                .checks
                .push(check(LoginApiMatch, Some(OpenAi), Warn, &[]));
            assert_eq!(chip(Some(&status), "claude-card", locale).label, ok);
            status.checks.push(check(Ipv6, None, Warn, &[]));
            let view = chip(Some(&status), "claude-card", locale);
            assert_eq!(
                (&*view.label, &*view.accessible_label, view.tone),
                (warns, warns, EgressTone::Warn)
            );
            let google = set_family(&mut status, Google);
            google.state = EgressState::Abnormal;
            google.frozen_session_ids = vec!["multi-card".into(), "google-other".into()];
            let openai = set_family(&mut status, OpenAi);
            openai.frozen_session_ids = vec!["multi-card".into(), "codex-other".into()];
            status.state = EgressState::Abnormal;
            assert_eq!(chip(Some(&status), "multi-card", locale).label, paused);
            assert_eq!(chip(Some(&status), "shell", locale).label, global_paused);
            set_family(&mut status, Google).overridden = true;
            assert_eq!(chip(Some(&status), "multi-card", locale).label, warns);
            set_family(&mut status, Anthropic).overridden = true;
            let view = chip(Some(&status), "claude-card", locale);
            assert_eq!(
                (&*view.label, view.tone),
                (bypass, EgressTone::DangerOutline)
            );
            let claude = set_family(&mut status, Anthropic);
            claude.overridden = false;
            claude.state = EgressState::Unknown;
            assert_eq!(chip(Some(&status), "claude-card", locale).label, unknown);
            status.checked_at = None;
            assert_eq!(chip(Some(&status), "claude-card", locale).label, checking);
            status = self::status();
            status
                .session_family_sets
                .insert("multi-card".into(), vec![Anthropic, Google]);
            status
                .session_family_sets
                .insert("google-card".into(), vec![]);
        }
    }

    #[test]
    fn uncollected_checks_are_distinguished_from_explicit_timeouts() {
        let mut diagnostic = check(BrowserPath, None, Unknown, &[("reason", "notCollected")]);
        assert_eq!(
            diagnostic.detail(None, Chinese).as_deref(),
            Some("本次未采集")
        );
        assert_eq!(
            diagnostic.detail(None, English).as_deref(),
            Some("Not collected in this check")
        );

        diagnostic.facts.insert("error".into(), "timeout".into());
        diagnostic.facts.insert("host".into(), "login.example".into());
        assert_eq!(
            diagnostic.detail(None, Chinese).as_deref(),
            Some("无法确认（login.example 检测超时）")
        );
        assert_eq!(
            diagnostic.detail(None, English).as_deref(),
            Some("Couldn't confirm (login.example timed out)")
        );
    }

    #[test]
    fn panel_groups_google_omits_login_and_localizes_slow_and_unreachable() {
        let mut state = EgressPanelState::default();
        let mut status = status();
        assert!(state.view(Some(&status), Chinese, NOW).is_none());
        state.open = true;
        assert!(state.view(None, English, NOW).is_none());

        status.checks = vec![
            check(
                Mainland,
                Some(Google),
                Pass,
                &[("slowHost", "google-exit.example")],
            ),
            check(LoginApiMatch, Some(Google), Warn, &[]),
            check(
                LoginApiMatch,
                Some(OpenAi),
                Unknown,
                &[("error", "unreachable"), ("host", "login.example")],
            ),
            check(Ipv6, None, Pass, &[]),
        ];
        set_family(&mut status, Anthropic).violation = Some(EgressViolation::Rule);
        for (locale, headings, title, checked, slow, outside, login, unreachable, accept) in [
            (
                Chinese,
                [
                    "Claude（Anthropic）",
                    "Codex（OpenAI）",
                    "Claude（ChatGPT 订阅）",
                    "Google（通用出口检测）",
                    "本机",
                ],
                "网络检查",
                "检测于 09:05",
                "日本 · 198.51.100.111 · google-exit.example 响应慢（>5 秒）",
                "出口不在中国大陆",
                "登录与 API 同一出口",
                "无法确认（login.example 连不上）",
                "以当前出口为准",
            ),
            (
                English,
                [
                    "Claude (Anthropic)",
                    "Codex (OpenAI)",
                    "Claude (ChatGPT subscription)",
                    "Google (generic exit)",
                    "This Mac",
                ],
                "Network check",
                "Checked at 09:05",
                "Japan · 198.51.100.111 · google-exit.example is slow (>5s)",
                "Exit is outside mainland China",
                "Sign-in and API share one exit",
                "Couldn't confirm (login.example unreachable)",
                "Use current exit",
            ),
        ] {
            let view = state.view(Some(&status), locale, NOW).unwrap();
            assert_eq!((view.title, &*view.checked), (title, checked));
            assert_eq!(
                view.groups
                    .iter()
                    .map(|group| group.heading)
                    .collect::<Vec<_>>(),
                headings
            );
            let claude = view.groups[0].accept_current.as_ref().unwrap();
            assert!(claude.enabled);
            assert_eq!(
                (claude.label, &claude.action),
                (accept, &EgressAction::AcceptCurrent(Anthropic))
            );
            assert_eq!(
                view.groups[3].checks,
                [EgressCheckView {
                    title: outside,
                    detail: Some(slow.into()),
                    result: Pass
                }]
            );
            assert_eq!(
                view.groups[1].checks,
                [EgressCheckView {
                    title: login,
                    detail: Some(unreachable.into()),
                    result: Unknown
                }]
            );
        }
    }

    #[test]
    fn protection_rule_switch_and_copy_feedback_have_independent_state() {
        let mut state = EgressPanelState {
            open: true,
            rechecking: true,
            ..EgressPanelState::default()
        };
        let mut status = status();
        status.rule_mode = EgressRule::Ip;
        status.checked_at = None;
        state.copy_succeeded(NOW);
        for (locale, checked, rules, protection, copy, copied, recheck) in [
            (
                Chinese,
                "出口检查中…",
                ["同一国家／地区（推荐）", "固定 IP"],
                "出口保护：出口异常时暂停 Claude / Codex / Antigravity 会话",
                "复制报告",
                "已复制",
                "重新检查",
            ),
            (
                English,
                "Checking exit…",
                ["Same country/region (recommended)", "Fixed IP"],
                "Exit protection: pause Claude / Codex / Antigravity sessions when the exit is wrong",
                "Copy report",
                "Copied",
                "Recheck",
            ),
        ] {
            let view = state.view(Some(&status), locale, NOW).unwrap();
            assert_eq!((&*view.checked, view.recheck.label), (checked, recheck));
            assert!(!view.recheck.enabled);
            assert!(view.rules.iter().all(|button| !button.enabled));
            assert!(!view.protection.enabled);
            assert_eq!(view.rules.each_ref().map(|button| button.label), rules);
            assert_eq!(
                view.rules.each_ref().map(|button| &button.action),
                [
                    &EgressAction::SetRule(EgressRule::Country),
                    &EgressAction::SetRule(EgressRule::Ip)
                ]
            );
            assert_eq!(view.protection.action, EgressAction::SetProtection(false));
            assert_eq!(view.protection.label, protection);
            assert!(view.protection_enabled);
            assert_eq!(view.rule_mode, EgressRule::Ip);
            assert_eq!(view.copy.label, copy);
            assert_eq!(
                view.copy.action,
                EgressAction::CopyReport(build_egress_report(&status, locale))
            );
            assert_eq!(view.copied, Some(copied));
            assert!(
                state
                    .view(Some(&status), locale, NOW + EGRESS_COPIED_DURATION)
                    .unwrap()
                    .copied
                    .is_none()
            );
        }
        status.protection_enabled = false;
        let view = state.view(Some(&status), English, NOW).unwrap();
        assert!(!view.protection_enabled);
        assert_eq!(view.protection.action, EgressAction::SetProtection(true));
    }

    #[test]
    fn frozen_banner_requires_confirmation_and_recovers_only_once_per_transition() {
        let mut status = status();
        let entry = set_family(&mut status, Anthropic);
        entry.state = EgressState::Abnormal;
        entry.violation = Some(EgressViolation::Rule);
        entry.baseline = Some(exit("198.51.100.50", "JP"));
        entry.frozen_session_ids = vec!["claude-card".into()];
        let mut state = EgressBannerState::default();
        state.sync(Some(&status), "claude-card", NOW);
        assert_eq!(
            state.dispatch(EgressAction::ConfirmOverride(Anthropic)),
            None
        );
        state.dispatch(EgressAction::RequestOverride(Anthropic));
        for (locale, message, override_label, title, body, cancel, confirm) in [
            (
                Chinese,
                "出口与规则不符（日本 → 美国），已暂停。网络恢复后自动继续。",
                "仍要继续",
                "仍要继续？",
                "出口仍然异常，继续后 agent 会用当前出口访问服务商。出口再次变化前不会再暂停。",
                "取消",
                "继续",
            ),
            (
                English,
                "Exit doesn't match your rule (Japan → United States). Paused; resumes automatically when the network recovers.",
                "Continue anyway",
                "Continue anyway?",
                "The exit is still wrong. Agents will reach the provider through it, and they won't be paused again until the exit changes.",
                "Cancel",
                "Continue",
            ),
        ] {
            let view = state.view(locale, NOW).unwrap();
            assert_eq!(view.message, message);
            assert_eq!(view.override_button.unwrap().label, override_label);
            let dialog = view.confirm.unwrap();
            assert_eq!(
                (
                    dialog.title,
                    dialog.body,
                    dialog.cancel.label,
                    dialog.confirm.label
                ),
                (title, body, cancel, confirm)
            );
        }
        assert_eq!(
            state.dispatch(EgressAction::Recheck),
            Some(EgressAction::Recheck)
        );
        assert!(!state.view(English, NOW).unwrap().recheck.unwrap().enabled);
        state.recheck_finished();
        state.dispatch(EgressAction::CancelOverride);
        assert!(state.view(English, NOW).unwrap().confirm.is_none());
        state.dispatch(EgressAction::RequestOverride(Anthropic));
        assert_eq!(state.dispatch(EgressAction::ConfirmOverride(Google)), None);
        assert_eq!(
            state.dispatch(EgressAction::ConfirmOverride(Anthropic)),
            Some(EgressAction::ConfirmOverride(Anthropic))
        );
        assert!(
            !state
                .view(English, NOW)
                .unwrap()
                .override_button
                .unwrap()
                .enabled
        );

        let entry = set_family(&mut status, Anthropic);
        entry.state = EgressState::Ok;
        entry.frozen_session_ids.clear();
        status.last_transition = Some(EgressTransition::Resumed {
            id: "resume-1".into(),
            family: Anthropic,
        });
        state.override_finished();
        state.sync(Some(&status), "claude-card", NOW);
        assert_eq!(
            state.view(Chinese, NOW).unwrap().message,
            "出口已恢复，已自动继续"
        );
        let resumed = state.view(English, NOW).unwrap();
        assert_eq!(resumed.message, "Exit restored; resumed automatically");
        assert!(resumed.resumed && resumed.recheck.is_none() && resumed.override_button.is_none());
        let later = NOW + EGRESS_RESUMED_DURATION;
        state.sync(Some(&status), "claude-card", later);
        assert!(state.view(English, later).is_none());

        let entry = set_family(&mut status, Anthropic);
        entry.violation = Some(EgressViolation::Mainland);
        entry.frozen_session_ids = vec!["claude-card".into()];
        state.sync(Some(&status), "claude-card", later);
        assert_eq!(
            state.view(Chinese, later).unwrap().message,
            "出口异常（检测到中国大陆 IP），已暂停。网络恢复后自动继续。"
        );
        assert_eq!(
            state.view(English, later).unwrap().message,
            "Exit problem (mainland China IP detected). Paused; resumes automatically when the network recovers."
        );
    }

    #[test]
    fn report_matches_reference_format_and_masks_addresses_in_details() {
        let mut status = status();
        status
            .families
            .retain(|entry| matches!(entry.family, Anthropic | ChatGptSubscription));
        set_family(&mut status, Anthropic).frozen_session_ids = vec!["private-session-id".into()];
        status.checks = vec![
            check(Mainland, Some(Anthropic), Pass, &[]),
            check(
                ExitRule,
                Some(Anthropic),
                Fail,
                &[
                    ("baselineCountry", "JP"),
                    ("currentCountry", "US"),
                    ("currentIp", "203.0.113.77"),
                ],
            ),
            check(Ipv6, None, Pass, &[("slowHost", "[2001:db8::7]:443")]),
        ];
        let chinese = build_egress_report(&status, Chinese);
        assert_eq!(
            chinese,
            concat!(
                "网络检查 (09:05)\n出口规则: 同一国家／地区（推荐）\n出口保护: 已开启\n\n",
                "[Claude（Anthropic）] ok — 美国 · 203.0.x.x\n",
                "- [pass] 出口不在中国大陆 — 美国 · 203.0.x.x\n",
                "- [fail] 出口符合出口规则 — 日本 → 美国（203.0.x.x）。确认节点；如果是预期变化，点「以当前出口为准」。\n\n",
                "[Claude（ChatGPT 订阅）] ok — 英国 · 2001:db8:x:x\n\n",
                "[本机]\n- [pass] 无 IPv6 绕过 — [2001:db8:x:x]:443 响应慢（>5 秒）",
            )
        );
        let english = build_egress_report(&status, English);
        assert_eq!(
            english,
            concat!(
                "Network check (09:05)\nExit rule: Same country/region (recommended)\nExit protection: On\n\n",
                "[Claude (Anthropic)] ok — United States · 203.0.x.x\n",
                "- [pass] Exit is outside mainland China — United States · 203.0.x.x\n",
                "- [fail] Exit matches your rule — Japan → United States (203.0.x.x). Check your node; if this change is expected, choose \"Use current exit\".\n\n",
                "[Claude (ChatGPT subscription)] ok — United Kingdom · 2001:db8:x:x\n\n",
                "[This Mac]\n- [pass] No IPv6 bypass — [2001:db8:x:x]:443 is slow (>5s)",
            )
        );
        for report in [chinese, english] {
            assert!(!report.contains("private-session-id"));
            assert!(!report.contains("203.0.113.77") && !report.contains("2001:db8::7"));
        }
        assert_eq!(
            redact_ips_in_text("IP 203.0.113.77:80. IPv6 ::1, checked at 09:05"),
            "IP 203.0.x.x:80. IPv6 0:0:x:x, checked at 09:05"
        );
    }
}
