// Frozen `AgentTeamMemberSummary.tsx:4-24` and the
// `.agent-team-member-summary*` rules in `hierarchy.css:330-339` / `718-720`,
// ported to GPUI as a standalone, read-only teammate card.
//
// Frozen copy:
// - `hierarchyTerminal.ts:142-156` (zh-CN) and `:333-347` (en).
// - `TerminalPane.tsx:345` renders the session title verbatim and allows an
//   empty title; `:479-481` mounts the summary instead of a terminal surface.
// - The card activation seam follows `SessionCarousel.tsx`'s `onActivate`
//   and `TerminalPane.tsx`'s pane `onPointerDown` -> `onActivate(session.id)`
//   plus `ChildSessionBadge.tsx:26-46`'s `onOpen()`. The return seam follows
//   `SessionBreadcrumb.tsx:10-24`'s `onReturnParent`; the visible return
//   control stays in the surrounding workspace breadcrumb, because the frozen
//   summary panel itself contains no button or navigation copy.
//
// The card is read-only: it never accepts terminal input and never mutates
// session data.

use std::rc::Rc;

use gpui::{prelude::*, *};
use matou_native_core::sessions::InterfaceLocale;
use matou_native_terminal::Theme;

use crate::ui_text::ui_text;

/// Frozen `SessionGraphNodeView['workStatus']`
/// (`hierarchy-types.ts:39`) in declaration order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TeamMemberWorkStatus {
    Starting,
    Idle,
    Running,
    NeedsInput,
    Error,
    Interrupted,
    Exited,
}

impl TeamMemberWorkStatus {
    /// Every value the frozen work status can hold.
    pub const ALL: [Self; 7] = [
        Self::Starting,
        Self::Idle,
        Self::Running,
        Self::NeedsInput,
        Self::Error,
        Self::Interrupted,
        Self::Exited,
    ];

    /// Parse the wire value used by the session graph. Unknown values are
    /// reported instead of being silently folded into a frozen state.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "starting" => Some(Self::Starting),
            "idle" => Some(Self::Idle),
            "running" => Some(Self::Running),
            "needs-input" => Some(Self::NeedsInput),
            "error" => Some(Self::Error),
            "interrupted" => Some(Self::Interrupted),
            "exited" => Some(Self::Exited),
            _ => None,
        }
    }

    pub fn wire(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Idle => "idle",
            Self::Running => "running",
            Self::NeedsInput => "needs-input",
            Self::Error => "error",
            Self::Interrupted => "interrupted",
            Self::Exited => "exited",
        }
    }

    /// `running` and `starting` share the frozen active badge treatment
    /// (`hierarchy.css:334-335`).
    pub fn is_active(self) -> bool {
        matches!(self, Self::Starting | Self::Running)
    }

    /// `exited` is the frozen ended state; it keeps the neutral badge
    /// treatment because `hierarchy.css` has no `exited` override.
    pub fn is_ended(self) -> bool {
        matches!(self, Self::Exited)
    }

    pub fn tone(self) -> TeamMemberStatusTone {
        match self {
            Self::Starting | Self::Running => TeamMemberStatusTone::Active,
            Self::Error | Self::Interrupted => TeamMemberStatusTone::Error,
            Self::Idle | Self::NeedsInput | Self::Exited => TeamMemberStatusTone::Neutral,
        }
    }

    /// Frozen `hierarchyTerminal.teamMember.workStatus[status]`.
    pub fn label(self, locale: InterfaceLocale) -> &'static str {
        use TeamMemberWorkStatus::*;
        match (self, locale) {
            (Starting | Running, InterfaceLocale::Chinese) => "运行中",
            (Starting | Running, InterfaceLocale::English) => "Running",
            (Idle, InterfaceLocale::Chinese) => "空闲",
            (Idle, InterfaceLocale::English) => "Idle",
            (NeedsInput, InterfaceLocale::Chinese) => "待输入",
            (NeedsInput, InterfaceLocale::English) => "Needs input",
            (Error | Interrupted, InterfaceLocale::Chinese) => "异常",
            (Error | Interrupted, InterfaceLocale::English) => "Error",
            (Exited, InterfaceLocale::Chinese) => "已结束",
            (Exited, InterfaceLocale::English) => "Ended",
        }
    }
}

/// Badge tint groups from `hierarchy.css:333-337`. The light theme does not
/// override the badge, so these colors are theme-independent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TeamMemberStatusTone {
    Neutral,
    Active,
    Error,
}

/// Frozen `hierarchyTerminal.teamMember` copy resolved for one locale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TeamMemberMessages {
    pub summary: &'static str,
    pub heading: &'static str,
    pub waiting: &'static str,
    pub hint: &'static str,
    pub status: &'static str,
}

pub fn team_member_messages(
    locale: InterfaceLocale,
    status: TeamMemberWorkStatus,
) -> TeamMemberMessages {
    TeamMemberMessages {
        summary: locale.text("队友会话摘要", "Teammate session summary"),
        heading: locale.text("Claude Code 队友", "Claude Code teammate"),
        waiting: locale.text("等待队友更新…", "Waiting for the teammate to update…"),
        hint: locale.text(
            "队友会话由 Claude Code 团队管理，在此查看状态与最新摘要。",
            "The Claude Code team manages this teammate session. Its status and latest summary appear here.",
        ),
        status: status.label(locale),
    }
}

/// Plain presentation model for one teammate. No GPUI or JSON types are
/// required to build or test it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TeamMemberCardModel {
    title: String,
    work_status: TeamMemberWorkStatus,
    latest_lines: Vec<String>,
}

impl TeamMemberCardModel {
    pub fn new(
        title: impl Into<String>,
        work_status: TeamMemberWorkStatus,
        latest_lines: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            title: title.into(),
            work_status,
            latest_lines: latest_lines.into_iter().map(Into::into).collect(),
        }
    }

    /// The frozen pane title, preserved verbatim. It may be empty:
    /// `TerminalPane.tsx:345` renders the raw title with no fallback.
    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn work_status(&self) -> TeamMemberWorkStatus {
        self.work_status
    }

    pub fn latest_lines(&self) -> &[String] {
        &self.latest_lines
    }

    /// The frozen teammate pane replaces the terminal surface and has no
    /// terminal input (`TerminalPane.tsx:479-481`).
    pub fn accepts_terminal_input(&self) -> bool {
        false
    }

    /// Resolve every frozen string and the summary fallback for `locale`.
    pub fn resolve(&self, locale: InterfaceLocale) -> ResolvedTeamMemberCard {
        let messages = team_member_messages(locale, self.work_status);
        // Frozen `AgentTeamMemberSummary.tsx:18`: only a genuinely empty
        // array falls back to the waiting copy. Blank lines are preserved.
        let lines = if self.latest_lines.is_empty() {
            vec![messages.waiting.to_owned()]
        } else {
            self.latest_lines.clone()
        };
        ResolvedTeamMemberCard {
            raw_title: self.title.clone(),
            summary_label: messages.summary,
            heading: messages.heading,
            status_label: messages.status,
            hint: messages.hint,
            status: self.work_status,
            tone: self.work_status.tone(),
            is_active: self.work_status.is_active(),
            is_ended: self.work_status.is_ended(),
            lines,
            accepts_terminal_input: false,
        }
    }
}

/// Fully resolved copy for rendering or for tests. The title remains the
/// original string; width ellipsis is a render-time concern only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedTeamMemberCard {
    pub raw_title: String,
    pub summary_label: &'static str,
    pub heading: &'static str,
    pub status_label: &'static str,
    pub hint: &'static str,
    pub status: TeamMemberWorkStatus,
    pub tone: TeamMemberStatusTone,
    pub is_active: bool,
    pub is_ended: bool,
    pub lines: Vec<String>,
    pub accepts_terminal_input: bool,
}

impl ResolvedTeamMemberCard {
    pub fn title(&self) -> &str {
        &self.raw_title
    }
}

/// Navigation seams outside the read-only summary body. The card itself does
/// not grow a button or new copy for either action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TeamMemberAction {
    /// Enter / focus the teammate card, matching `SessionCarousel`'s and
    /// `TerminalPane`'s `onActivate` and the child-session badge's `onOpen()`.
    Activate,
    /// Return to the main card, matching `SessionBreadcrumb`'s
    /// `onReturnParent`. The visible breadcrumb stays in the workspace footer.
    ReturnToMain,
}

/// Window-free callback router plus an optional GPUI effect. `dispatch` is
/// callable from unit tests; the renderer uses `invoke` from the card's
/// activation handler so a workspace can also update and notify its entity.
#[derive(Clone, Default)]
pub struct TeamMemberActions {
    activate: Option<Rc<dyn Fn()>>,
    return_to_main: Option<Rc<dyn Fn()>>,
    effect: Option<Rc<dyn Fn(TeamMemberAction, &mut Window, &mut App)>>,
}

impl TeamMemberActions {
    pub fn none() -> Self {
        Self::default()
    }

    pub fn with_activate(mut self, callback: impl Fn() + 'static) -> Self {
        self.activate = Some(Rc::new(callback));
        self
    }

    pub fn with_return_to_main(mut self, callback: impl Fn() + 'static) -> Self {
        self.return_to_main = Some(Rc::new(callback));
        self
    }

    /// Optional GPUI-native effect for workspace wiring. The renderer calls it
    /// with `Window`/`App` after a plain callback, so the workspace can update
    /// an entity and notify it. This hook is deliberately separate from
    /// [`Self::dispatch`], which stays unit-testable without a window.
    pub fn with_effect(
        mut self,
        effect: impl Fn(TeamMemberAction, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.effect = Some(Rc::new(effect));
        self
    }

    pub fn is_wired(&self, action: TeamMemberAction) -> bool {
        let plain = match action {
            TeamMemberAction::Activate => self.activate.is_some(),
            TeamMemberAction::ReturnToMain => self.return_to_main.is_some(),
        };
        plain || self.effect.is_some()
    }

    /// Invoke the window-free plain callback for `action`. Returns whether a
    /// plain callback ran. An effect-only registration is reported by
    /// [`Self::is_wired`] but is intentionally not executed here.
    pub fn dispatch(&self, action: TeamMemberAction) -> bool {
        let callback = match action {
            TeamMemberAction::Activate => &self.activate,
            TeamMemberAction::ReturnToMain => &self.return_to_main,
        };
        if let Some(callback) = callback {
            callback();
            true
        } else {
            false
        }
    }

    /// Invoke the plain callback and, when present, the GPUI effect. Use this
    /// from an existing outer control (such as the workspace breadcrumb) when
    /// the action is not rendered by this card.
    pub fn invoke(&self, action: TeamMemberAction, window: &mut Window, cx: &mut App) -> bool {
        let plain = self.dispatch(action);
        if let Some(effect) = &self.effect {
            effect(action, window, cx);
            true
        } else {
            plain
        }
    }
}

#[derive(Clone, Copy)]
struct TeamMemberColors {
    surface: u32,
    chrome: u32,
    text: u32,
    title: u32,
    hint: u32,
    lines_border: u32,
    lines_background: u32,
}

impl TeamMemberColors {
    fn for_theme(theme: Theme) -> Self {
        if theme.background == Theme::LIGHT.background {
            // `hierarchy.css:718-720` light overrides. The heading badge is
            // intentionally not overridden by the frozen light theme.
            Self {
                surface: 0xf7f8fa,
                chrome: 0xffffff,
                text: 0x303746,
                title: 0x303744ff,
                hint: 0x78808c,
                lines_border: 0xd9dee8ff,
                lines_background: 0xffffffff,
            }
        } else {
            // Frozen dark summary and session-card header.
            Self {
                surface: theme.background,
                chrome: 0x1d2025,
                text: 0xd9dce2,
                title: 0xffffffdb,
                hint: 0x7f8793,
                lines_border: 0x8ba0bf33,
                lines_background: 0xffffff06,
            }
        }
    }
}

fn badge_colors(tone: TeamMemberStatusTone) -> (u32, u32) {
    match tone {
        TeamMemberStatusTone::Neutral => (0xa9c8f1, 0x4f89cd2e),
        TeamMemberStatusTone::Active => (0x9ed6b0, 0x45975f2e),
        TeamMemberStatusTone::Error => (0xefaaa1, 0xbc483d2e),
    }
}

/// `TerminalPane.tsx:345` keeps the full title in the native title attribute.
/// GPUI shows it through a tooltip; only control characters are normalized
/// for the single-line title itself.
fn single_line_title(raw: &str) -> SharedString {
    if raw
        .chars()
        .any(|character| matches!(character, '\r' | '\n' | '\t'))
    {
        SharedString::from(raw.replace(['\r', '\n', '\t'], " "))
    } else {
        SharedString::from(raw.to_owned())
    }
}

struct TeamMemberTitleTooltip(SharedString);

impl Render for TeamMemberTitleTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .max_w(px(460.))
            .px(px(9.))
            .py(px(7.))
            .border_1()
            .border_color(rgb(0xd6dae1))
            .rounded(px(7.))
            .bg(rgb(0xffffff))
            .text_color(rgb(0x333b47))
            .text_size(px(10.))
            .line_height(px(15.))
            .whitespace_normal()
            .child(ui_text(self.0.clone()))
    }
}

fn title_element(id: &str, full_title: String, color: u32) -> Stateful<Div> {
    let title_id = SharedString::from(format!("{id}:title"));
    let mut title = div()
        .id(title_id)
        .min_w_0()
        .max_w(relative(0.46))
        .whitespace_nowrap()
        .line_clamp(1)
        .text_ellipsis()
        .text_size(px(12.))
        .font_weight(FontWeight::BOLD)
        .text_color(rgba(color))
        .child(ui_text(single_line_title(&full_title)));
    if !full_title.is_empty() {
        title = title.tooltip(move |_, cx| {
            cx.new(|_| TeamMemberTitleTooltip(SharedString::from(full_title.clone())))
                .into()
        });
    }
    title
}

/// Render the frozen read-only teammate card. The caller supplies its unique
/// element id and connects navigation to the surrounding session canvas.
pub fn team_member_card(
    id: impl Into<SharedString>,
    model: &TeamMemberCardModel,
    locale: InterfaceLocale,
    theme: Theme,
    actions: &TeamMemberActions,
) -> Stateful<Div> {
    let id = id.into();
    let resolved = model.resolve(locale);
    let colors = TeamMemberColors::for_theme(theme);
    let (badge_text, badge_background) = badge_colors(resolved.tone);

    // The session canvas overrides the pane header (`session-canvas.css:35-37,86-87`).
    let header = div()
        .flex_none()
        .h(px(42.))
        .w_full()
        .min_w_0()
        .px(px(12.))
        .flex()
        .items_center()
        .bg(rgb(colors.chrome))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .child(title_element(id.as_ref(), resolved.raw_title, colors.title)),
        );

    // `.agent-team-member-summary__heading` (`hierarchy.css:331-337`).
    let heading = div()
        .flex_none()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.))
        .child(
            div()
                .min_w_0()
                .text_size(px(15.))
                .font_weight(FontWeight::BOLD)
                .child(ui_text(resolved.heading)),
        )
        .child(
            div()
                .flex_none()
                .px(px(8.))
                .py(px(3.))
                .rounded(px(999.))
                .bg(rgba(badge_background))
                .text_color(rgb(badge_text))
                .text_size(px(11.))
                .line_height(relative(1.2))
                .whitespace_nowrap()
                .child(ui_text(resolved.status_label)),
        );

    // `.agent-team-member-summary__lines` (`hierarchy.css:338`).
    let lines_id = SharedString::from(format!("{id}:lines"));
    let mut lines = div()
        .id(lines_id)
        .w_full()
        .min_h_0()
        .flex()
        .flex_col()
        .gap(px(8.))
        .p(px(14.))
        .border_1()
        .border_color(rgba(colors.lines_border))
        .rounded(px(8.))
        .bg(rgba(colors.lines_background))
        .text_color(rgb(colors.text))
        .text_size(px(12.))
        .font_family("SFMono-Regular")
        .line_height(relative(1.55))
        .overflow_scroll();
    for line in &resolved.lines {
        lines = lines.child(
            div()
                .flex_none()
                .w_full()
                .min_w_0()
                .whitespace_normal()
                .child(ui_text(line.clone())),
        );
    }

    // `.agent-team-member-summary small` (`hierarchy.css:339`).
    let hint = div()
        .flex_none()
        .mt_auto()
        .text_color(rgb(colors.hint))
        .text_size(px(13.))
        .line_height(relative(1.4))
        .whitespace_normal()
        .child(ui_text(resolved.hint));

    // `.agent-team-member-summary` (`hierarchy.css:330`): 28px padding,
    // 16px gap, terminal background, plus the light overrides at `:718-720`.
    let body = div()
        .flex_1()
        .min_w_0()
        .min_h_0()
        .flex()
        .flex_col()
        .gap(px(16.))
        .p(px(28.))
        .bg(rgb(colors.surface))
        .text_color(rgb(colors.text))
        .child(heading)
        .child(lines)
        .child(hint);

    let mut card = div()
        .id(id)
        .size_full()
        .min_w_0()
        .min_h_0()
        .flex()
        .flex_col()
        .overflow_hidden()
        .bg(rgb(colors.surface))
        .text_color(rgb(colors.text))
        .child(header)
        .child(body);

    // Match the pane's pointer-down activation.
    if actions.is_wired(TeamMemberAction::Activate) {
        let actions = actions.clone();
        card = card.on_mouse_down(MouseButton::Left, move |_, window, cx| {
            if actions.invoke(TeamMemberAction::Activate, window, cx) {
                window.refresh();
            }
        });
    }

    card
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui::{Styled, TextOverflow, WhiteSpace, relative};
    use matou_native_core::sessions::InterfaceLocale;

    use super::{
        TeamMemberAction, TeamMemberActions, TeamMemberCardModel, TeamMemberStatusTone,
        TeamMemberWorkStatus, single_line_title, team_member_messages, title_element,
    };

    fn statuses() -> [(TeamMemberWorkStatus, &'static str, &'static str); 7] {
        [
            (TeamMemberWorkStatus::Starting, "运行中", "Running"),
            (TeamMemberWorkStatus::Idle, "空闲", "Idle"),
            (TeamMemberWorkStatus::Running, "运行中", "Running"),
            (TeamMemberWorkStatus::NeedsInput, "待输入", "Needs input"),
            (TeamMemberWorkStatus::Error, "异常", "Error"),
            (TeamMemberWorkStatus::Interrupted, "异常", "Error"),
            (TeamMemberWorkStatus::Exited, "已结束", "Ended"),
        ]
    }

    #[test]
    fn work_status_covers_every_frozen_value_and_wire_name() {
        assert_eq!(TeamMemberWorkStatus::ALL.len(), statuses().len());
        for (status, _, _) in statuses() {
            assert_eq!(TeamMemberWorkStatus::from_wire(status.wire()), Some(status));
            assert!(TeamMemberWorkStatus::ALL.contains(&status));
        }
        assert_eq!(TeamMemberWorkStatus::from_wire("paused"), None);
        assert_eq!(TeamMemberWorkStatus::from_wire(""), None);
    }

    #[test]
    fn status_labels_match_frozen_chinese_and_english_copy() {
        for (status, chinese, english) in statuses() {
            assert_eq!(status.label(InterfaceLocale::Chinese), chinese);
            assert_eq!(status.label(InterfaceLocale::English), english);
        }
    }

    #[test]
    fn active_and_ended_classification_matches_frozen_presentation() {
        for (status, _, _) in statuses() {
            assert_eq!(
                status.is_active(),
                matches!(
                    status,
                    TeamMemberWorkStatus::Starting | TeamMemberWorkStatus::Running
                )
            );
            assert_eq!(status.is_ended(), status == TeamMemberWorkStatus::Exited);
            assert_eq!(
                status.tone(),
                match status {
                    TeamMemberWorkStatus::Starting | TeamMemberWorkStatus::Running =>
                        TeamMemberStatusTone::Active,
                    TeamMemberWorkStatus::Error | TeamMemberWorkStatus::Interrupted =>
                        TeamMemberStatusTone::Error,
                    TeamMemberWorkStatus::Idle
                    | TeamMemberWorkStatus::NeedsInput
                    | TeamMemberWorkStatus::Exited => TeamMemberStatusTone::Neutral,
                }
            );
        }
        assert!(TeamMemberWorkStatus::Running.is_active());
        assert!(TeamMemberWorkStatus::Starting.is_active());
        assert!(!TeamMemberWorkStatus::Exited.is_active());
        assert!(TeamMemberWorkStatus::Exited.is_ended());
        assert!(!TeamMemberWorkStatus::Idle.is_ended());
    }

    #[test]
    fn messages_match_frozen_locale_copy() {
        let chinese = team_member_messages(InterfaceLocale::Chinese, TeamMemberWorkStatus::Running);
        assert_eq!(chinese.summary, "队友会话摘要");
        assert_eq!(chinese.heading, "Claude Code 队友");
        assert_eq!(chinese.waiting, "等待队友更新…");
        assert_eq!(
            chinese.hint,
            "队友会话由 Claude Code 团队管理，在此查看状态与最新摘要。"
        );
        assert_eq!(chinese.status, "运行中");

        let english = team_member_messages(InterfaceLocale::English, TeamMemberWorkStatus::Exited);
        assert_eq!(english.summary, "Teammate session summary");
        assert_eq!(english.heading, "Claude Code teammate");
        assert_eq!(english.waiting, "Waiting for the teammate to update…");
        assert_eq!(
            english.hint,
            "The Claude Code team manages this teammate session. Its status and latest summary appear here."
        );
        assert_eq!(english.status, "Ended");
    }

    #[test]
    fn empty_lines_use_the_frozen_waiting_fallback_only() {
        let empty =
            TeamMemberCardModel::new("teammate", TeamMemberWorkStatus::Idle, Vec::<String>::new());
        let chinese = empty.resolve(InterfaceLocale::Chinese);
        assert_eq!(chinese.lines, vec!["等待队友更新…"]);
        let english = empty.resolve(InterfaceLocale::English);
        assert_eq!(english.lines, vec!["Waiting for the teammate to update…"]);

        // A blank line is content: the frozen ternary checks `.length > 0`.
        let blank = TeamMemberCardModel::new("teammate", TeamMemberWorkStatus::Idle, [""]);
        assert_eq!(blank.resolve(InterfaceLocale::Chinese).lines, vec![""]);

        let lines = TeamMemberCardModel::new(
            "teammate",
            TeamMemberWorkStatus::Running,
            ["first", "second"],
        );
        assert_eq!(
            lines.resolve(InterfaceLocale::Chinese).lines,
            vec!["first", "second"]
        );
    }

    #[test]
    fn title_is_preserved_verbatim_and_may_be_empty() {
        let long_title = "a very long teammate title ".repeat(40);
        let model = TeamMemberCardModel::new(
            long_title.clone(),
            TeamMemberWorkStatus::Running,
            Vec::<String>::new(),
        );
        assert_eq!(model.title(), long_title);
        assert_eq!(model.resolve(InterfaceLocale::English).title(), long_title);

        let empty =
            TeamMemberCardModel::new("", TeamMemberWorkStatus::Exited, Vec::<String>::new());
        assert_eq!(empty.title(), "");
        assert_eq!(empty.resolve(InterfaceLocale::Chinese).title(), "");

        let mut rendered_title = title_element("teammate", long_title, 0xffffffff);
        assert_eq!(
            rendered_title.style().max_size.width,
            Some(relative(0.46).into())
        );
        let style = rendered_title.text_style().as_ref().unwrap();
        assert_eq!(style.white_space, Some(WhiteSpace::Nowrap));
        assert_eq!(style.line_clamp, Some(1));
        assert_eq!(
            style.text_overflow,
            Some(TextOverflow::Truncate("…".into()))
        );
    }

    #[test]
    fn teammate_card_never_accepts_terminal_input() {
        let model =
            TeamMemberCardModel::new("teammate", TeamMemberWorkStatus::NeedsInput, ["line"]);
        assert!(!model.accepts_terminal_input());
        assert!(
            !model
                .resolve(InterfaceLocale::Chinese)
                .accepts_terminal_input
        );
        assert!(
            !model
                .resolve(InterfaceLocale::English)
                .accepts_terminal_input
        );
    }

    #[test]
    fn actions_route_without_a_window() {
        let activated = Rc::new(Cell::new(0));
        let returned = Rc::new(Cell::new(0));
        let activated_counter = activated.clone();
        let returned_counter = returned.clone();

        let actions = TeamMemberActions::none()
            .with_activate(move || activated_counter.set(activated_counter.get() + 1))
            .with_return_to_main(move || returned_counter.set(returned_counter.get() + 1));

        assert!(actions.is_wired(TeamMemberAction::Activate));
        assert!(actions.is_wired(TeamMemberAction::ReturnToMain));
        assert!(actions.dispatch(TeamMemberAction::Activate));
        assert_eq!(activated.get(), 1);
        assert_eq!(returned.get(), 0);
        assert!(actions.dispatch(TeamMemberAction::ReturnToMain));
        assert_eq!(activated.get(), 1);
        assert_eq!(returned.get(), 1);

        let unwired = TeamMemberActions::none();
        assert!(!unwired.is_wired(TeamMemberAction::Activate));
        assert!(!unwired.dispatch(TeamMemberAction::Activate));
        assert!(!unwired.dispatch(TeamMemberAction::ReturnToMain));

        // One plain seam can be wired without inventing the other.
        let activate_only = TeamMemberActions::none().with_activate(|| {});
        assert!(activate_only.is_wired(TeamMemberAction::Activate));
        assert!(!activate_only.is_wired(TeamMemberAction::ReturnToMain));
        assert!(!activate_only.dispatch(TeamMemberAction::ReturnToMain));

        // The GPUI effect is optional real-window wiring; it does not make the
        // window-free dispatch pretend to have run a plain callback.
        let effect_only = TeamMemberActions::none().with_effect(|_, _, _| {});
        assert!(effect_only.is_wired(TeamMemberAction::Activate));
        assert!(effect_only.is_wired(TeamMemberAction::ReturnToMain));
        assert!(!effect_only.dispatch(TeamMemberAction::Activate));
    }

    #[test]
    fn single_line_title_only_normalizes_control_characters() {
        assert_eq!(single_line_title("plain").as_ref(), "plain");
        assert_eq!(single_line_title("a\nb\tc").as_ref(), "a b c");
        assert_eq!(single_line_title("").as_ref(), "");

        // Width ellipsis happens in the element layout; the model and the
        // tooltip text never impose a character limit.
        let long = "x".repeat(500);
        assert_eq!(single_line_title(&long).as_ref(), long);
    }
}
