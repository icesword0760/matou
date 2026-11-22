//! Provider context-window form and controls. The host supplies the saved
//! value and owns persistence, including the meaning of a cleared override.

use gpui::prelude::*;
use gpui::{
    App, BoxShadow, Context, Div, Entity, FocusHandle, FontWeight, HighlightStyle, StyledText,
    Subscription, Window, div, px, rgb, rgba,
};
use serde_json::Value;

use crate::{
    claude_role_assignments::catalog,
    rename_input::{RenameInput, RenameInputEvent},
};

use matou_native_core::sessions::InterfaceLocale;

/// The frozen launch path rejects context windows smaller than 100K.
pub const MIN_CONTEXT_TOKENS: u64 = 100_000;
/// Largest integer that can round-trip through the Electron JSON number model.
pub const MAX_CONTEXT_TOKENS: u64 = 9_007_199_254_740_991;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextWindowValidation {
    NotAnInteger,
    BelowMinimum,
    AboveMaximum,
}

impl ContextWindowValidation {
    pub fn message(self, locale: InterfaceLocale) -> &'static str {
        match self {
            Self::NotAnInteger => {
                locale.text("请输入整数 token 数。", "Enter a whole number of tokens.")
            }
            Self::BelowMinimum => locale.text(
                "上下文窗口至少需要 100,000 tokens。",
                "The context window must be at least 100,000 tokens.",
            ),
            Self::AboveMaximum => locale.text(
                "上下文窗口不能超过 9,007,199,254,740,991 tokens。",
                "The context window cannot exceed 9,007,199,254,740,991 tokens.",
            ),
        }
    }
}

/// A blank field removes the override. A custom value is a base-10 integer
/// in tokens, with optional surrounding whitespace.
pub fn parse_context_window(value: &str) -> Result<Option<u64>, ContextWindowValidation> {
    use ContextWindowValidation::{AboveMaximum, BelowMinimum, NotAnInteger};

    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(NotAnInteger);
    }
    let tokens = value.parse::<u64>().map_err(|_| AboveMaximum)?;
    match tokens {
        ..MIN_CONTEXT_TOKENS => Err(BelowMinimum),
        ..=MAX_CONTEXT_TOKENS => Ok(Some(tokens)),
        _ => Err(AboveMaximum),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SaveError {
    Validation(ContextWindowValidation),
    Write(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextWindowForm {
    saved: Option<u64>,
    draft: String,
    error: Option<SaveError>,
}

impl ContextWindowForm {
    pub fn new(saved: Option<u64>) -> Self {
        Self {
            saved,
            draft: saved.map(|tokens| tokens.to_string()).unwrap_or_default(),
            error: None,
        }
    }

    pub fn saved(&self) -> Option<u64> {
        self.saved
    }

    pub fn draft(&self) -> &str {
        &self.draft
    }

    pub fn edit(&mut self, draft: impl Into<String>) {
        let draft = draft.into();
        if draft != self.draft {
            self.draft = draft;
            self.error = None;
        }
    }

    pub fn badge(&self, locale: InterfaceLocale) -> String {
        context_window_badge(self.saved, locale)
    }

    pub fn error(&self, locale: InterfaceLocale) -> Option<&str> {
        self.error.as_ref().map(|error| match error {
            SaveError::Validation(error) => error.message(locale),
            SaveError::Write(message) => message.as_str(),
        })
    }

    /// Commits a changed value only after the write callback succeeds. On
    /// failure, the draft stays editable and the badge reflects the prior save.
    pub fn save(&mut self, write: impl FnOnce(Option<u64>) -> Result<(), String>) -> bool {
        let next = match parse_context_window(&self.draft) {
            Ok(next) => next,
            Err(error) => {
                self.error = Some(SaveError::Validation(error));
                return false;
            }
        };
        if next != self.saved {
            if let Err(message) = write(next) {
                self.error = Some(SaveError::Write(message));
                return false;
            }
            self.saved = next;
        }
        self.error = None;
        true
    }
}

/// The default label and compact token formatting follow
/// `context-window-ui.tsx:53–72`.
pub fn context_window_badge(tokens: Option<u64>, locale: InterfaceLocale) -> String {
    let Some(tokens) = tokens else {
        return locale.text("默认窗口", "Default window").into();
    };
    format!(
        "{}{}",
        compact_tokens(tokens),
        locale.text(" 窗口", " window")
    )
}

/// `formatCompactTokens` from `context-window-ui.tsx:53–57`.
pub fn compact_tokens(tokens: u64) -> String {
    if tokens.is_multiple_of(1_000_000) {
        format!("{}M", tokens / 1_000_000)
    } else if tokens.is_multiple_of(1_000) {
        format!("{}K", tokens / 1_000)
    } else {
        format_tokens(tokens)
    }
}

/// `toLocaleString('en-US')` without pulling a locale database into the UI.
pub fn format_tokens(tokens: u64) -> String {
    let digits = tokens.to_string();
    let mut grouped = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

pub const TARGET_512K_TOKENS: u64 = 512_000;

/// Frozen `ContextWindowPicker` defaults: a saved mode wins, a Codex
/// subscription defaults to the fixed 512K request, and everything else keeps
/// the model/CLI default.
pub fn provider_context_mode(provider: &Value) -> &str {
    provider["contextWindowMode"].as_str().unwrap_or_else(|| {
        if provider["kind"] == "subscription" && provider["subscription"]["provider"] == "codex" {
            "512k"
        } else {
            "default"
        }
    })
}

/// `providerRoleModelIds` from `context-window-ui.tsx:47–51`. Subscription
/// providers resolve through their role mapping only; API providers also count
/// the default model.
pub fn provider_context_model_ids(provider: &Value) -> Vec<String> {
    let mapped = matou_native_core::claude_role_budget::CLAUDE_ROLES
        .into_iter()
        .filter_map(|role| provider["modelMapping"][role].as_str())
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if provider["kind"] == "subscription" {
        mapped
    } else {
        let model = provider["model"].as_str().unwrap_or_default().trim();
        std::iter::once(model)
            .chain(mapped.iter().map(String::as_str))
            .filter(|id| !id.is_empty() && *id != "CLI 默认" && *id != "__cli_default__")
            .map(str::to_owned)
            .collect()
    }
}

fn mode_tokens(mode: &str, row: &Value) -> Option<u64> {
    let default = row["contextWindow"].as_u64().filter(|tokens| *tokens > 0);
    let maximum = row["maxContextWindow"]
        .as_u64()
        .filter(|tokens| *tokens > 0);
    if mode == "maximum" {
        maximum.or(default)
    } else {
        default
    }
}

/// `resolveProviderWindow` from `context-window-ui.tsx:19–45`: only a window
/// known for every selected model is usable; otherwise the CLI default remains.
pub fn resolve_context_tokens(mode: &str, models: &[Value], selected: &[String]) -> Option<u64> {
    if mode == "512k" {
        return Some(TARGET_512K_TOKENS);
    }
    let mut ids = Vec::new();
    for id in selected {
        let id = id.trim();
        if !id.is_empty() && id != "CLI 默认" && id != "__cli_default__" && !ids.contains(&id) {
            ids.push(id);
        }
    }
    let pick = |id: &str| models.iter().find(|row| row["id"] == id);
    if !ids.is_empty() {
        let resolved = ids
            .iter()
            .map(|id| pick(id).and_then(|row| mode_tokens(mode, row)))
            .collect::<Vec<_>>();
        return resolved
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .map(|tokens| tokens.into_iter().min().unwrap_or_default());
    }
    let fallback = models
        .iter()
        .find(|row| row["id"] == "default")
        .or_else(|| models.iter().find(|row| row["id"] == "opus"))
        .or_else(|| models.first());
    fallback.and_then(|row| mode_tokens(mode, row))
}

/// `formatContextWindowBadge` from `context-window-ui.tsx:59–73`.
pub fn format_context_window_badge(
    mode: &str,
    models: &[Value],
    selected: &[String],
    locale: InterfaceLocale,
) -> String {
    let zh = locale == InterfaceLocale::Chinese;
    if mode == "512k" {
        return if zh { "512K 窗口" } else { "512K window" }.into();
    }
    let tokens = resolve_context_tokens(mode, models, selected);
    if mode == "maximum" {
        return match tokens {
            Some(tokens) => {
                if zh {
                    format!("最大 {} 窗口", compact_tokens(tokens))
                } else {
                    format!("Max {}", compact_tokens(tokens))
                }
            }
            None if zh => "最大可扩展".into(),
            None => "Max extensible".into(),
        };
    }
    match tokens {
        Some(tokens) => {
            if zh {
                format!("默认 {} 窗口", compact_tokens(tokens))
            } else {
                format!("Default {}", compact_tokens(tokens))
            }
        }
        None if zh => "默认窗口".into(),
        None => "Default window".into(),
    }
}

/// Card badge that reads the concrete mode plus the provider's model directory.
/// Rows written before the mode selector existed keep their numeric capacity so
/// old saved data does not silently lose its label.
pub fn provider_window_badge_label(
    provider: &Value,
    native_catalog: &[Value],
    locale: InterfaceLocale,
) -> String {
    if provider["contextWindowMode"].as_str().is_none()
        && let Some(tokens) = provider["contextWindow"]
            .as_u64()
            .filter(|tokens| *tokens > 0)
    {
        return format!(
            "{}{}",
            locale.text("最大 ", "Max "),
            context_window_badge(Some(tokens), locale)
        );
    }
    let models = provider["modelMetadata"]
        .as_array()
        .map(Vec::as_slice)
        .or_else(|| catalog(provider, native_catalog))
        .unwrap_or_default();
    let selected = provider_context_model_ids(provider);
    format_context_window_badge(provider_context_mode(provider), models, &selected, locale)
}

pub fn context_mode_title(mode: &str, locale: InterfaceLocale) -> &'static str {
    match mode {
        "512k" => "512K",
        "maximum" => locale.text("最大可扩展", "Max extensible"),
        _ => locale.text("默认窗口", "Default"),
    }
}

pub fn context_mode_subtitle(mode: &str, locale: InterfaceLocale) -> &'static str {
    match mode {
        "512k" => locale.text("固定请求；需通道支持", "Fixed request; channel-dependent"),
        "maximum" => locale.text("使用模型已知最大上限", "Largest known model window"),
        _ => locale.text("沿用模型与 CLI 默认", "Model & CLI default"),
    }
}

pub fn context_mode_tokens(
    mode: &str,
    models: &[Value],
    selected: &[String],
    locale: InterfaceLocale,
) -> String {
    match resolve_context_tokens(mode, models, selected) {
        Some(tokens) => format!("{} tokens", format_tokens(tokens)),
        None if mode == "maximum" => locale
            .text("未提供（沿用默认）", "Not provided (uses default)")
            .into(),
        None => locale.text("由 CLI 决定", "Decided by CLI").into(),
    }
}

pub fn context_unknown_hint(locale: InterfaceLocale) -> &'static str {
    locale.text(
        "服务未提供完整的窗口信息，将沿用 CLI 默认设置。",
        "The service did not provide complete window information; CLI default settings will be used.",
    )
}

pub fn context_multi_model_hint(locale: InterfaceLocale) -> &'static str {
    locale.text(
        "映射多个模型时，仅当所有模型都有窗口数据才取最小值；任一未知时保留 CLI 默认。",
        "When multiple models are mapped, the smallest known window applies only if all models provide data.",
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderSwitchImpact {
    pub title: String,
    pub body: &'static str,
}

/// Frozen `i18n/messages/hierarchyShell.ts:189–191,462–464`.
pub fn provider_switch_impact(locale: InterfaceLocale, app_name: &str) -> ProviderSwitchImpact {
    ProviderSwitchImpact {
        title: match locale {
            InterfaceLocale::Chinese => format!("切换会影响所有 {app_name} 窗口。"),
            InterfaceLocale::English => format!("Switching affects every {app_name} window."),
        },
        body: locale.text(
            "所有 Claude Code 会话跟随全局供应商；暂缓会话就绪后自动切换，旧会话再次打开时也会应用，历史保留。上下文窗口可在「编辑」中设置。",
            "All Claude Code sessions follow the global provider. Deferred sessions switch when ready, and reopened sessions apply it while preserving history. Configure context windows in Edit.",
        ),
    }
}

/// Frozen `hierarchy.css:1135–1136` and `provider-setup.css:118`.
/// The last card has no row border. Active cards in other positions
/// override the color of the existing 1 px bottom divider.
pub const ACTIVE_PROVIDER_BOTTOM_EDGE_RGB: u32 = 0x6aa1ee;
pub const PROVIDER_BOTTOM_EDGE_WIDTH: f32 = 1.;

pub fn active_provider_bottom_edge(active: bool, is_last: bool) -> Option<u32> {
    (active && !is_last).then_some(ACTIVE_PROVIDER_BOTTOM_EDGE_RGB)
}

/// One reusable editor row for a provider's custom context window.
///
/// `saved` is the currently persisted override; `on_save` receives `None` when
/// the field is empty and `Some(tokens)` for a valid custom value. The callback
/// is invoked only for a changed value, and only its `Ok` result updates the
/// badge. A rejected save keeps the draft and shows the form's message.
///
/// Place [`provider_switch_impact_row`] below this row to show the frozen
/// provider-switch impact copy.
pub struct ProviderContextEditorRow {
    form: ContextWindowForm,
    input: Entity<RenameInput>,
    dark: bool,
    locale: InterfaceLocale,
    on_save: Box<dyn FnMut(Option<u64>, &mut Window, &mut App) -> Result<(), String>>,
    confirm_write: bool,
    save_pending: bool,
    save_focus: FocusHandle,
    input_focused: bool,
    _subscriptions: Vec<Subscription>,
}

impl ProviderContextEditorRow {
    /// `window` is explicit because the row subscribes to the focused input's
    /// focus/blur events and to the input's `RenameInputEvent` stream.
    pub fn new(
        saved: Option<u64>,
        dark: bool,
        locale: InterfaceLocale,
        on_save: impl FnMut(Option<u64>, &mut Window, &mut App) -> Result<(), String> + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let form = ContextWindowForm::new(saved);
        let draft = form.draft().to_owned();
        let input = cx.new(move |cx| {
            let mut input = if dark {
                let mut input = RenameInput::session_loader_search("", cx);
                input.text = draft;
                input
            } else {
                let mut input = RenameInput::new(draft, cx);
                input.search_style = true;
                input
            };
            input.placeholder.clear();
            input
        });
        let input_focus = input.read(cx).focus.clone();
        let save_focus = cx.focus_handle().tab_stop(true);
        let mut subscriptions = Vec::with_capacity(3);
        subscriptions.push(cx.subscribe_in(
            &input,
            window,
            |this, _, event, window, cx| match event {
                RenameInputEvent::Changed => this.changed(cx),
                RenameInputEvent::Confirm => this.commit(window, cx),
                RenameInputEvent::Cancel => {}
            },
        ));
        subscriptions.push(cx.on_focus(&input_focus, window, |this, _, cx| {
            this.input_focused = true;
            cx.notify();
        }));
        subscriptions.push(cx.on_blur(&input_focus, window, |this, _, cx| {
            this.input_focused = false;
            cx.notify();
        }));

        Self {
            form,
            input,
            dark,
            locale,
            on_save: Box::new(on_save),
            confirm_write: false,
            save_pending: false,
            save_focus,
            input_focused: false,
            _subscriptions: subscriptions,
        }
    }

    /// Keep the saved badge until the runtime confirms the queued write.
    pub fn with_runtime_confirmation(mut self) -> Self {
        self.confirm_write = true;
        self
    }

    pub fn saved(&self) -> Option<u64> {
        self.form.saved()
    }

    pub fn set_locale(&mut self, locale: InterfaceLocale, cx: &mut Context<Self>) {
        if self.locale != locale {
            self.locale = locale;
            cx.notify();
        }
    }

    pub fn complete_save(&mut self, result: Result<Option<u64>, String>, cx: &mut Context<Self>) {
        if !std::mem::take(&mut self.save_pending) {
            return;
        }
        match result {
            Ok(tokens) => {
                self.form.saved = tokens;
                self.form.error = None;
            }
            Err(message) => self.form.error = Some(SaveError::Write(message)),
        }
        cx.notify();
    }

    pub fn focus_order(&self, cx: &App) -> [FocusHandle; 2] {
        [self.input.read(cx).focus.clone(), self.save_focus.clone()]
    }

    #[cfg(debug_assertions)]
    pub fn probe_focus_input(&self, window: &mut Window, cx: &App) {
        window.focus(&self.input.read(cx).focus);
    }

    #[cfg(debug_assertions)]
    pub fn probe_status(&self) -> (String, Option<&str>, bool) {
        (
            self.form.badge(self.locale),
            self.form.error(self.locale),
            self.save_pending,
        )
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        let text = self.input.read(cx).text.clone();
        if text.as_str() != self.form.draft() {
            self.form.edit(text);
            cx.notify();
        }
    }

    fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.save_pending {
            return;
        }
        let input_focus = self.input.read(cx).focus.clone();
        let before = self.form.saved;
        let Self { form, on_save, .. } = self;
        let saved = form.save(|value| on_save(value, window, cx));
        if self.confirm_write && form.saved != before {
            form.saved = before;
            self.save_pending = true;
        }
        if !saved {
            // Keep the caret in the field so the retained draft is editable.
            window.focus(&input_focus);
        }
        cx.notify();
    }
}

impl Render for ProviderContextEditorRow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = ContextRowColors::new(self.dark);
        let locale = self.locale;
        let error = self.form.error(locale).map(str::to_owned);

        let title = div()
            .text_size(px(13.))
            .font_weight(FontWeight(600.))
            .text_color(rgb(colors.title))
            .whitespace_nowrap()
            .child(crate::ui_text::ui_text(
                locale.text("上下文窗口长度", "Context window"),
            ));

        let label = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(5.))
            .child(title)
            .child(provider_context_window_badge(self.form.saved(), locale));

        let input_line = if self.input_focused {
            colors.input_focus_line
        } else {
            colors.input_border
        };
        let mut input_shell = div()
            .flex_1()
            .min_w(px(0.))
            .h(px(34.))
            .flex()
            .items_center()
            .rounded(px(7.))
            .border_1()
            .border_color(rgba(input_line))
            .bg(rgba(colors.input_background))
            .px(px(10.))
            .child(self.input.clone());
        if self.input_focused {
            input_shell = input_shell.shadow(vec![focus_ring(colors.input_focus_ring, 3.)]);
        }

        let save_focus = self.save_focus.clone();
        let save_pending = self.save_pending;
        let save = div()
            .id(("provider-context-save", self.input.entity_id()))
            .flex_none()
            .h(px(31.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(7.))
            .border_1()
            .border_color(rgb(colors.save))
            .bg(rgb(colors.save))
            .px(px(12.))
            .text_size(px(10.))
            .font_weight(FontWeight(650.))
            .text_color(rgb(0xffffff))
            .when(save_pending, |button| button.opacity(0.55))
            .when(!save_pending, |button| button.cursor_pointer())
            .track_focus(&save_focus)
            .focus(move |style| style.shadow(vec![focus_ring(colors.input_focus_line, 2.)]))
            .hover(move |style| {
                style
                    .bg(rgb(colors.save_hover))
                    .border_color(rgb(colors.save_hover))
            })
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.modifiers.modified() {
                    return;
                }
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    cx.stop_propagation();
                    this.commit(window, cx);
                }
            }))
            .on_click(cx.listener(|this, event: &gpui::ClickEvent, window, cx| {
                if matches!(event, gpui::ClickEvent::Keyboard(_)) {
                    return;
                }
                this.commit(window, cx);
            }))
            .child(crate::ui_text::ui_text(if save_pending {
                locale.text("正在保存…", "Saving…")
            } else {
                locale.text("保存", "Save")
            }));

        let mut row = div().flex().flex_col().gap(px(6.)).w_full().child(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .w_full()
                .child(label)
                .child(input_shell)
                .child(save),
        );
        if let Some(error) = error {
            row = row.child(
                div()
                    .text_size(px(10.))
                    .line_height(gpui::relative(1.5))
                    .text_color(rgb(colors.error))
                    .whitespace_normal()
                    .child(crate::ui_text::ui_text(error)),
            );
        }
        row
    }
}

/// Badge shared by the editor and provider cards. Frozen `provider-setup.css:159`
/// applies the same colors in both themes.
pub fn provider_context_window_badge(tokens: Option<u64>, locale: InterfaceLocale) -> Div {
    context_window_badge_view(context_window_badge(tokens, locale))
}

pub fn context_window_badge_view(label: String) -> Div {
    div()
        .flex_none()
        .h(px(18.))
        .flex()
        .items_center()
        .rounded(px(6.))
        .border_1()
        .border_color(rgba(0x347fd638))
        .bg(rgba(0x347fd617))
        .px(px(6.))
        .text_size(px(10.))
        .font_weight(FontWeight(600.))
        .text_color(rgb(0x276cc0))
        .whitespace_nowrap()
        .child(crate::ui_text::ui_text(label))
}

/// Provider-switch note with inline emphasis, as in `hierarchy.css:1162–1163,
/// 1207` and the provider settings size override in `provider-setup.css:131`.
pub fn provider_switch_impact_row(locale: InterfaceLocale, app_name: &str, dark: bool) -> Div {
    let impact = provider_switch_impact(locale, app_name);
    let colors = ContextRowColors::new(dark);
    let title_length = impact.title.len();
    let message = StyledText::new(format!("{} {}", impact.title, impact.body)).with_highlights([(
        0..title_length,
        HighlightStyle {
            color: Some(rgb(colors.impact_title).into()),
            font_weight: Some(FontWeight::BOLD),
            ..Default::default()
        },
    )]);
    div()
        .flex()
        .items_center()
        .gap(px(5.))
        .mt(px(11.))
        .mx(px(2.))
        .text_size(px(11.))
        .line_height(gpui::relative(1.7))
        .text_color(rgb(colors.muted))
        .child(div().flex_none().child(crate::ui_text::ui_text("ⓘ")))
        .child(div().min_w_0().flex_1().whitespace_normal().child(message))
}

fn focus_ring(color: u32, spread: f32) -> BoxShadow {
    BoxShadow {
        color: rgba(color).into(),
        offset: gpui::point(px(0.), px(0.)),
        blur_radius: px(0.),
        spread_radius: px(spread),
    }
}

#[derive(Clone, Copy)]
struct ContextRowColors {
    title: u32,
    muted: u32,
    impact_title: u32,
    input_border: u32,
    input_background: u32,
    input_focus_line: u32,
    input_focus_ring: u32,
    error: u32,
    save: u32,
    save_hover: u32,
}

impl ContextRowColors {
    const fn new(dark: bool) -> Self {
        if dark {
            Self {
                title: 0xdde2e8,
                muted: 0x8d96a2,
                impact_title: 0xa3abb5,
                input_border: 0xffffff26,
                input_background: 0xffffff0b,
                input_focus_line: 0x5b9eeb47,
                input_focus_ring: 0x4a91e224,
                // hierarchy.css has no dark override for the dialog error;
                // reuse the existing dark error text from hierarchy.css:931.
                error: 0xff9b94,
                save: 0x67a6ef,
                save_hover: 0x5b9eeb,
            }
        } else {
            Self {
                title: 0x344257,
                muted: 0x7e8794,
                impact_title: 0x6a7481,
                input_border: 0x424b5829,
                input_background: 0xffffffe0,
                input_focus_line: 0x347fd647,
                input_focus_ring: 0x347fd61c,
                error: 0xa64d47,
                save: 0x347fd6,
                save_hover: 0x2f79df,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use InterfaceLocale::{Chinese, English};
    use serde_json::json;

    #[test]
    fn saves_custom_window_and_clears_to_default_without_duplicate_writes() {
        let mut form = ContextWindowForm::new(None);
        let mut writes = Vec::new();
        let mut write = |value| {
            writes.push(value);
            Ok(())
        };
        assert_eq!(form.badge(Chinese), "默认窗口");

        form.edit(" 512000 ");
        assert_eq!(form.badge(Chinese), "默认窗口");
        assert!(form.save(&mut write));
        assert_eq!(form.saved(), Some(512_000));
        assert_eq!(form.badge(Chinese), "512K 窗口");
        assert_eq!(form.badge(English), "512K window");
        assert!(form.save(&mut write));

        form.edit("");
        assert!(form.save(&mut write));
        assert_eq!(form.saved(), None);
        assert_eq!(form.badge(Chinese), "默认窗口");
        assert_eq!(form.badge(English), "Default window");
        assert_eq!(writes, [Some(512_000), None]);
    }

    #[test]
    fn validates_boundaries_and_leaves_invalid_drafts_unsaved() {
        use ContextWindowValidation::*;

        assert_eq!(parse_context_window("  "), Ok(None));
        assert_eq!(parse_context_window("100000"), Ok(Some(100_000)));
        assert_eq!(
            parse_context_window("9007199254740991"),
            Ok(Some(MAX_CONTEXT_TOKENS))
        );
        let mut form = ContextWindowForm::new(Some(200_000));
        for (draft, error) in [
            ("99999", BelowMinimum),
            ("0", BelowMinimum),
            ("9007199254740992", AboveMaximum),
            ("18446744073709551616", AboveMaximum),
            ("512K", NotAnInteger),
            ("-100000", NotAnInteger),
            ("100000.0", NotAnInteger),
            ("1e6", NotAnInteger),
            ("1,000,000", NotAnInteger),
        ] {
            assert_eq!(parse_context_window(draft), Err(error));
            form.edit(draft);
            assert!(!form.save(|_| panic!("invalid input called the writer")));
            assert_eq!(form.error(Chinese), Some(error.message(Chinese)));
            assert_eq!(form.saved(), Some(200_000));
            assert_eq!(form.draft(), draft);
        }
    }

    #[test]
    fn write_failure_preserves_the_saved_value_and_allows_retry() {
        let mut form = ContextWindowForm::new(Some(300_000));
        assert_eq!(form.draft(), "300000");
        form.edit("123456");
        assert!(!form.save(|_| Err("write failed".into())));
        assert_eq!(form.error(English), Some("write failed"));
        assert_eq!(form.draft(), "123456");
        assert_eq!(form.badge(English), "300K window");
        assert!(form.save(|value| {
            assert_eq!(value, Some(123_456));
            Ok(())
        }));
        assert_eq!(form.badge(English), "123,456 window");
        assert_eq!(form.error(English), None);
    }

    #[test]
    fn mode_badges_and_picker_tokens_follow_the_frozen_resolution() {
        let models = vec![
            json!({"id": "opus-a", "contextWindow": 272_000, "maxContextWindow": 872_000}),
            json!({"id": "haiku-a", "contextWindow": 272_000, "maxContextWindow": 400_000}),
            json!({"id": "unknown-a"}),
        ];
        let pair = vec!["opus-a".to_owned(), "haiku-a".to_owned()];
        assert_eq!(
            resolve_context_tokens("default", &models, &pair),
            Some(272_000)
        );
        assert_eq!(
            resolve_context_tokens("maximum", &models, &pair),
            Some(400_000)
        );
        assert_eq!(
            resolve_context_tokens("512k", &models, &pair),
            Some(512_000)
        );
        assert_eq!(
            resolve_context_tokens("maximum", &models, &["opus-a".into(), "unknown-a".into()]),
            None
        );
        assert_eq!(
            format_context_window_badge("512k", &models, &["opus-a".into()], Chinese),
            "512K 窗口"
        );
        assert_eq!(
            format_context_window_badge("default", &models, &["opus-a".into()], Chinese),
            "默认 272K 窗口"
        );
        assert_eq!(
            format_context_window_badge("maximum", &models, &pair, Chinese),
            "最大 400K 窗口"
        );
        assert_eq!(
            format_context_window_badge("default", &models, &["unknown-a".into()], English),
            "Default window"
        );
        assert_eq!(
            context_mode_tokens("default", &models, &pair, Chinese),
            "272,000 tokens"
        );
        assert_eq!(
            context_mode_tokens("maximum", &models, &["unknown-a".into()], Chinese),
            "未提供（沿用默认）"
        );
    }

    #[test]
    fn official_provider_badge_uses_native_windows_with_metadata_and_endpoint_guards() {
        let mut provider = json!({
            "builtIn": true, "endpoint": "https://api.anthropic.com/",
            "model": "__cli_default__",
        });
        let native = [json!({
            "id": "default", "contextWindow": 1_000_000, "maxContextWindow": 1_000_000,
        })];
        assert_eq!(
            provider_window_badge_label(&provider, &native, Chinese),
            "默认 1M 窗口"
        );
        assert_eq!(
            provider_window_badge_label(&provider, &native, English),
            "Default 1M"
        );
        assert_eq!(
            provider_window_badge_label(&provider, &[], Chinese),
            "默认窗口"
        );
        provider["endpoint"] = json!("https://proxy.example.invalid");
        assert_eq!(
            provider_window_badge_label(&provider, &native, Chinese),
            "默认窗口"
        );
        provider["endpoint"] = json!("https://api.anthropic.com");
        provider["modelMetadata"] = json!([{"id": "default", "contextWindow": 200_000}]);
        assert_eq!(
            provider_window_badge_label(&provider, &native, Chinese),
            "默认 200K 窗口"
        );
        provider["modelMetadata"] = json!([]);
        assert_eq!(
            provider_window_badge_label(&provider, &native, Chinese),
            "默认窗口"
        );
        assert_eq!(compact_tokens(1_048_576), "1,048,576");
    }

    #[test]
    fn provider_mode_and_role_ids_match_the_frozen_defaults() {
        let codex = json!({
            "kind": "subscription", "model": "CLI 默认",
            "subscription": {"provider": "codex"},
            "modelMapping": {"opus": "a", "fable": "d", "sonnet": "b", "haiku": "c"}
        });
        assert_eq!(provider_context_mode(&codex), "512k");
        assert_eq!(provider_context_model_ids(&codex), ["a", "d", "b", "c"]);
        let antigravity = json!({
            "kind": "subscription", "subscription": {"provider": "antigravity"},
            "modelMapping": {"opus": "a"}
        });
        assert_eq!(provider_context_mode(&antigravity), "default");
        let api = json!({
            "model": "default-model", "contextWindowMode": "maximum",
            "modelMapping": {"opus": "role-model", "sonnet": "", "haiku": "CLI 默认"}
        });
        assert_eq!(provider_context_mode(&api), "maximum");
        assert_eq!(
            provider_context_model_ids(&api),
            ["default-model", "role-model"]
        );
    }

    #[test]
    fn impact_copy_and_active_card_edge_match_the_frozen_source() {
        let frozen_messages =
            include_str!("../../../desktop/src/renderer/src/i18n/messages/hierarchyShell.ts");
        for locale in [Chinese, English] {
            let impact = provider_switch_impact(locale, "${app}");
            assert!(frozen_messages.contains(&format!("`{}`", impact.title)));
            assert!(frozen_messages.contains(&format!("'{}'", impact.body)));
        }

        assert_eq!(active_provider_bottom_edge(false, false), None);
        assert_eq!(active_provider_bottom_edge(true, true), None);
        let edge = active_provider_bottom_edge(true, false).unwrap();
        let css = include_str!("../../../desktop/src/renderer/src/hierarchy/provider-setup.css");
        assert!(css.lines().any(|line| {
            line.starts_with(".provider-settings .model-provider.is-current ")
                && line.contains(&format!("border-color: #{edge:06x}"))
        }));
        let css = include_str!("../../../desktop/src/renderer/src/hierarchy/hierarchy.css");
        assert!(css.lines().any(|line| {
            line.starts_with(".model-provider {")
                && line.contains(&format!("border-bottom: {PROVIDER_BOTTOM_EDGE_WIDTH}px"))
        }));
        assert!(css.contains(".model-provider:last-child { border-bottom: 0; }"));
    }
}
