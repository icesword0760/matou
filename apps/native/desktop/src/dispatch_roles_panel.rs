//! Unified Matou-local dispatch settings and the live Foreman instruction preview.

use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::Rc,
    time::Duration,
};

use gpui::{prelude::*, *};
use matou_native_core::{
    dispatch_model_catalog::{DispatchModelCatalog, probe_local_cli_models},
    dispatch_roles::{
        AGY_EFFORTS, AgentRoleConfig, BUILTIN_AGENTS, CODEX_EFFORTS, DSH_EFFORTS, DispatchRoleMode,
        DispatchRolesConfig, DispatchRolesSnapshot, DispatchRolesStore, ROLE_MAX_CHARS,
        agent_display_name, agy_effort_suffix, default_role_text, effort_is_fixed, effort_label_zh,
        format_roles_markdown, speed_options,
    },
    sessions::InterfaceLocale,
};

use crate::{
    rename_input::{RenameInput, RenameInputEvent},
    settings_shell::{FocusRingStyle, focus_ring_with, icon, shadow},
    theme_tokens::{AppTheme, WorkspacePalette},
    workspace::CardTooltip,
};

const NUMBER_LIMITS: [(f64, f64); 4] = [(1., 60.), (5., 180.), (1., 60.), (0., 5.)];
const CHOICES: usize = 1 + BUILTIN_AGENTS.len() * 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Choice {
    Default,
    Mode(usize),
    Model(usize),
    Effort(usize),
    Speed(usize),
}

impl Choice {
    fn slot(self) -> usize {
        match self {
            Self::Default => 0,
            Self::Mode(index) => 1 + 4 * index,
            Self::Model(index) => 2 + 4 * index,
            Self::Effort(index) => 3 + 4 * index,
            Self::Speed(index) => 4 + 4 * index,
        }
    }
}

const CHOICE_ORDER: [Choice; CHOICES] = [
    Choice::Default,
    Choice::Mode(0),
    Choice::Model(0),
    Choice::Effort(0),
    Choice::Speed(0),
    Choice::Mode(1),
    Choice::Model(1),
    Choice::Effort(1),
    Choice::Speed(1),
    Choice::Mode(2),
    Choice::Model(2),
    Choice::Effort(2),
    Choice::Speed(2),
];

struct ChoiceItem {
    value: String,
    label: String,
    group: String,
}

impl ChoiceItem {
    fn new(value: &str, label: &str) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            group: String::new(),
        }
    }
}

struct Menu {
    choice: Choice,
    active: usize,
}

#[derive(Clone, Copy)]
struct Colors {
    palette: WorkspacePalette,
    card: u32,
    control: u32,
    popup: u32,
    error: u32,
    success: u32,
}

impl Colors {
    fn new(theme: AppTheme) -> Self {
        let palette = theme.palette().workspace;
        if theme == AppTheme::Dark {
            Self {
                palette,
                card: 0x171b22ff,
                control: 0x232936ff,
                popup: 0x171b22ff,
                error: 0xff9b94ff,
                success: 0x72bd83ff,
            }
        } else {
            Self {
                palette,
                card: 0xffffffff,
                control: 0xffffffff,
                popup: 0xffffffff,
                error: 0xb34d47ff,
                success: 0x16794bff,
            }
        }
    }

    fn card(self) -> Div {
        div()
            .flex()
            .flex_col()
            .min_w_0()
            .gap(px(7.))
            .px(px(12.))
            .py(px(10.))
            .rounded(px(8.))
            .border_1()
            .border_color(rgba(self.palette.line))
            .bg(rgba(self.card))
    }

    fn field(self) -> Div {
        div()
            .relative()
            .h(px(34.))
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(9.))
            .min_w_0()
            .rounded(px(6.))
            .border_1()
            .border_color(rgba(self.palette.line_strong))
            .bg(rgba(self.control))
            .text_size(px(12.))
    }
}

pub struct DispatchRolesPanel {
    store: Rc<DispatchRolesStore>,
    data_root: PathBuf,
    saved: DispatchRolesConfig,
    draft: DispatchRolesConfig,
    catalog: DispatchModelCatalog,
    numbers: [Entity<RenameInput>; 4],
    roles: [Entity<RenameInput>; 3],
    search: Entity<RenameInput>,
    locale: InterfaceLocale,
    theme: AppTheme,
    choice_focus: [FocusHandle; CHOICES],
    anchor_bounds: Rc<RefCell<[Option<Bounds<Pixels>>; CHOICES]>>,
    menu: Option<Menu>,
    menu_scroll: ScrollHandle,
    agent_scroll: ScrollHandle,
    pane_width: Rc<Cell<Pixels>>,
    refresh_focus: FocusHandle,
    preview_focus: FocusHandle,
    sub_dispatch_focus: FocusHandle,
    enabled_focus: [FocusHandle; 3],
    role_reset_focus: [FocusHandle; 3],
    restore_focus: FocusHandle,
    save_focus: FocusHandle,
    preview: bool,
    refreshing: bool,
    refresh_status: Option<&'static str>,
    refresh_generation: u64,
    save_status: Option<Result<(), String>>,
    _subscriptions: Vec<Subscription>,
}

impl DispatchRolesPanel {
    pub fn new(
        store: Rc<DispatchRolesStore>,
        snapshot: DispatchRolesSnapshot,
        locale: InterfaceLocale,
        theme: AppTheme,
        cx: &mut Context<Self>,
    ) -> Self {
        let data_root = snapshot
            .path
            .parent()
            .expect("dispatch roles data root")
            .to_path_buf();
        let catalog = DispatchModelCatalog::load(&data_root);
        let saved = snapshot.config;
        let mut subscriptions = Vec::new();
        let numbers = std::array::from_fn(|index| {
            let input = cx.new(|cx| Self::compact_input(Self::number_text(&saved, index), cx));
            subscriptions.push(cx.subscribe(&input, move |this, input, event, cx| {
                if matches!(event, RenameInputEvent::Changed) && !input.read(cx).composing() {
                    this.number_edited(index, cx);
                }
            }));
            input
        });
        let roles = std::array::from_fn(|index| {
            let focus = cx.focus_handle().tab_stop(true);
            let input = cx.new(|cx| {
                let mut input =
                    RenameInput::instructions(saved.agents[index].role.clone(), focus, cx);
                input.set_inline_document_layout(px(8.));
                input
            });
            subscriptions.push(cx.subscribe(&input, move |this, input, event, cx| {
                if matches!(event, RenameInputEvent::Changed)
                    && !input.read(cx).composing()
                    && this.draft.agents[index].role != input.read(cx).text
                {
                    this.draft.agents[index]
                        .role
                        .clone_from(&input.read(cx).text);
                    this.changed(cx);
                }
            }));
            input
        });
        let search = cx.new(|cx| Self::compact_input(String::new(), cx));
        subscriptions.push(cx.subscribe(&search, |this, _, event, cx| {
            if matches!(event, RenameInputEvent::Changed)
                && let Some(menu) = &mut this.menu
            {
                menu.active = 0;
                this.menu_scroll.set_offset(point(px(0.), px(0.)));
                cx.notify();
            }
        }));
        let mut panel = Self {
            store,
            data_root,
            draft: saved.clone(),
            saved,
            catalog,
            numbers,
            roles,
            search,
            locale,
            theme,
            choice_focus: std::array::from_fn(|_| cx.focus_handle().tab_stop(true)),
            anchor_bounds: Rc::default(),
            menu: None,
            menu_scroll: ScrollHandle::new(),
            agent_scroll: ScrollHandle::new(),
            pane_width: Rc::default(),
            refresh_focus: cx.focus_handle().tab_stop(true),
            preview_focus: cx.focus_handle().tab_stop(true),
            sub_dispatch_focus: cx.focus_handle().tab_stop(true),
            enabled_focus: std::array::from_fn(|_| cx.focus_handle().tab_stop(true)),
            role_reset_focus: std::array::from_fn(|_| cx.focus_handle().tab_stop(true)),
            restore_focus: cx.focus_handle().tab_stop(true),
            save_focus: cx.focus_handle().tab_stop(true),
            preview: false,
            refreshing: false,
            refresh_status: None,
            refresh_generation: 0,
            save_status: None,
            _subscriptions: subscriptions,
        };
        panel.configure(locale, theme, cx);
        if panel.catalog.is_stale() {
            panel.refresh(false, cx);
        }
        panel
    }

    pub fn configure(&mut self, locale: InterfaceLocale, theme: AppTheme, cx: &mut Context<Self>) {
        let changed = self.locale != locale || self.theme != theme;
        self.locale = locale;
        self.theme = theme;
        let colors = Colors::new(theme).palette;
        for input in self.numbers.iter().chain(&self.roles).chain([&self.search]) {
            let placeholder = if *input == self.search {
                self.text("搜索模型…", "Search models…")
            } else {
                ""
            };
            let current = input.read(cx);
            if current.text_color != Some(rgba(colors.text))
                || current.placeholder_color != Some(rgba(colors.muted))
                || current.placeholder != placeholder
            {
                input.update(cx, |input, cx| {
                    input.text_color = Some(rgba(colors.text));
                    input.placeholder_color = Some(rgba(colors.muted));
                    input.placeholder = placeholder.into();
                    cx.notify();
                });
            }
        }
        if changed {
            cx.notify();
        }
    }

    fn compact_input(text: String, cx: &mut Context<RenameInput>) -> RenameInput {
        let mut input = RenameInput::new(text, cx);
        input.search_style = true;
        input.placeholder.clear();
        input
    }

    fn text(&self, zh: &'static str, en: &'static str) -> &'static str {
        self.locale.text(zh, en)
    }

    fn agent_label(&self, id: &str) -> String {
        if id == "agy" {
            self.text("Antigravity（反重力）", "Antigravity").to_owned()
        } else {
            agent_display_name(id).to_owned()
        }
    }

    fn number_text(config: &DispatchRolesConfig, index: usize) -> String {
        match index {
            0 => config.idle_min.to_string(),
            1 => config.max_min.to_string(),
            2 => config.ask_wait_min.to_string(),
            _ => config.retries.to_string(),
        }
    }

    fn parsed_number(&self, index: usize, cx: &App) -> Option<f64> {
        let input = self.numbers[index].read(cx);
        let raw = input.text.trim();
        if index == 3 {
            return raw
                .parse::<u8>()
                .ok()
                .filter(|value| *value <= 5)
                .map(f64::from);
        }
        let (min, max) = NUMBER_LIMITS[index];
        raw.parse::<f64>()
            .ok()
            .filter(|value| (min..=max).contains(value))
    }

    fn number_edited(&mut self, index: usize, cx: &mut Context<Self>) {
        let canonical_text = Self::number_text(&self.draft, index);
        if let Some(value) = self.parsed_number(index, cx) {
            match index {
                0 => self.draft.idle_min = value,
                1 => self.draft.max_min = value,
                2 => self.draft.ask_wait_min = value,
                _ => self.draft.retries = value as u8,
            }
        }
        if self.numbers[index].read(cx).text == canonical_text {
            cx.notify();
        } else {
            self.changed(cx);
        }
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        self.save_status = None;
        cx.notify();
    }

    fn enabled_agent_exists(&self) -> bool {
        self.draft.agents[..BUILTIN_AGENTS.len()]
            .iter()
            .any(|agent| agent.enabled)
    }

    fn can_save(&self, cx: &App) -> bool {
        self.draft != self.saved
            && self.enabled_agent_exists()
            && (0..4).all(|index| self.parsed_number(index, cx).is_some())
            && self.roles.iter().all(|input| {
                let input = input.read(cx);
                !input.composing()
                    && input.text.replace("\r\n", "\n").chars().count() <= ROLE_MAX_CHARS
            })
            && self.numbers.iter().all(|input| !input.read(cx).composing())
    }

    fn replace_input(input: &Entity<RenameInput>, text: &str, window: &mut Window, cx: &mut App) {
        let current = input.read(cx);
        if current.text == text {
            return;
        }
        let length = current.text.encode_utf16().count();
        input.update(cx, |editor, cx| {
            editor.replace_text_in_range(Some(0..length), text, window, cx)
        });
    }

    fn sync_inputs(&self, window: &mut Window, cx: &mut Context<Self>) {
        for (index, input) in self.numbers.iter().enumerate() {
            Self::replace_input(input, &Self::number_text(&self.draft, index), window, cx);
        }
        for (index, input) in self.roles.iter().enumerate() {
            Self::replace_input(input, &self.draft.agents[index].role, window, cx);
        }
    }

    fn restore(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.draft = DispatchRolesConfig::default();
        self.sync_inputs(window, cx);
        self.changed(cx);
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_save(cx) {
            return;
        }
        match self.store.save(self.draft.clone()) {
            Ok(snapshot) => {
                self.saved = snapshot.config.clone();
                self.draft = snapshot.config;
                self.sync_inputs(window, cx);
                self.save_status = Some(Ok(()));
            }
            Err(error) => self.save_status = Some(Err(error.to_string())),
        }
        cx.notify();
    }

    fn refresh(&mut self, show_feedback: bool, cx: &mut Context<Self>) {
        if self.refreshing {
            return;
        }
        self.refreshing = true;
        self.refresh_status = None;
        self.refresh_generation += 1;
        let generation = self.refresh_generation;
        let root = self.data_root.clone();
        let probe = cx
            .background_executor()
            .spawn(async move { probe_local_cli_models(&root) });
        cx.spawn(async move |this, cx| {
            let result = probe.await;
            if this
                .update(cx, |this, cx| {
                    this.catalog = result.catalog;
                    this.refreshing = false;
                    this.refresh_status = show_feedback.then_some(
                        match (
                            result.codex_updated,
                            result.agy_updated,
                            result.cache_error.is_some(),
                        ) {
                            (false, false, _) => "failed",
                            (_, _, true) => "cache",
                            (true, true, false) => "ok",
                            (true, false, false) => "codex",
                            (false, true, false) => "agy",
                        },
                    );
                    if let Some(menu) = &mut this.menu {
                        menu.active = 0;
                        this.menu_scroll.set_offset(point(px(0.), px(0.)));
                    }
                    cx.notify();
                })
                .is_err()
                || !show_feedback
            {
                return;
            }
            cx.background_executor().timer(Duration::from_secs(4)).await;
            let _ = this.update(cx, |this, cx| {
                if this.refresh_generation == generation {
                    this.refresh_status = None;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn mode_label(&self, mode: DispatchRoleMode) -> &'static str {
        match mode {
            DispatchRoleMode::Auto => self.text("按职责自动派", "Auto by responsibility"),
            DispatchRoleMode::Explicit => self.text("仅点名时派", "Only when named"),
        }
    }

    fn effort_label(&self, effort: &str) -> String {
        if self.locale == InterfaceLocale::Chinese {
            return effort_label_zh(effort).to_owned();
        }
        match effort {
            "off" => "Off",
            "none" => "None",
            "minimal" => "Minimal",
            "low" => "Low",
            "medium" => "Medium",
            "high" => "High",
            "xhigh" => "Extra high",
            "max" => "Maximum",
            other => other,
        }
        .to_owned()
    }

    fn efforts(&self, agent: &AgentRoleConfig) -> Vec<String> {
        if effort_is_fixed(&agent.id, &agent.model) {
            return Vec::new();
        }
        if agent.id == "agy"
            && let Some(effort) = agy_effort_suffix(&agent.model)
        {
            return vec![effort.into()];
        }
        if let Some(model) = self
            .catalog
            .models(&agent.id)
            .iter()
            .find(|model| model.value == agent.model)
        {
            return model.efforts.clone();
        }
        let efforts: &[&str] = match agent.id.as_str() {
            "codex" => &CODEX_EFFORTS,
            "dsh" => &DSH_EFFORTS,
            _ => &AGY_EFFORTS,
        };
        efforts.iter().map(|effort| (*effort).to_owned()).collect()
    }

    fn selected(&self, choice: Choice) -> &str {
        match choice {
            Choice::Default => &self.draft.default_agent,
            Choice::Mode(index) => self.draft.agents[index].mode.as_str(),
            Choice::Model(index) => &self.draft.agents[index].model,
            Choice::Effort(index) => {
                let agent = &self.draft.agents[index];
                if agent.id == "agy" {
                    agy_effort_suffix(&agent.model).unwrap_or(&agent.effort)
                } else {
                    &agent.effort
                }
            }
            Choice::Speed(index) => self.draft.agents[index]
                .speed
                .as_deref()
                .unwrap_or("default"),
        }
    }

    fn model_label(&self, value: &str, label: &str) -> String {
        if self.locale == InterfaceLocale::Chinese {
            return label.into();
        }
        if let Some((_, suffix)) = label.split_once("（当前 ") {
            return format!(
                "{value} · Latest (currently {})",
                suffix.trim_end_matches('）')
            );
        }
        label
            .replace("主力系列最新", "Latest main series")
            .replace("跨系列最高", "Best series")
            .replace("前沿系列最新", "Latest frontier series")
            .replace("均衡系列最新", "Latest balanced series")
            .replace("快速系列最新", "Latest fast series")
            .replace("系列最新", "Latest series")
            .replace("默认", "Default")
            .replace('（', " (")
            .replace('）', ")")
    }

    fn group_label(&self, group: &str) -> String {
        if self.locale == InterfaceLocale::Chinese || group.is_empty() {
            return group.into();
        }
        if group.starts_with("系列") {
            "Dynamic series"
        } else if group.starts_with("本机") {
            "Installed models"
        } else if group.starts_with("Orchestrator") {
            "Orchestrators · No separate reasoning level"
        } else if group.starts_with("Gemini") {
            "Gemini series by reasoning level"
        } else {
            "Cross-ecosystem and fixed models"
        }
        .into()
    }

    fn choices(&self, choice: Choice) -> Vec<ChoiceItem> {
        match choice {
            Choice::Default => self.draft.agents[..3]
                .iter()
                .filter(|agent| agent.enabled)
                .map(|agent| ChoiceItem::new(&agent.id, &self.agent_label(&agent.id)))
                .collect(),
            Choice::Mode(_) => [DispatchRoleMode::Auto, DispatchRoleMode::Explicit]
                .into_iter()
                .map(|mode| ChoiceItem::new(mode.as_str(), self.mode_label(mode)))
                .collect(),
            Choice::Model(index) => {
                let agent = &self.draft.agents[index];
                let entries = self.catalog.models(&agent.id);
                let mut options = Vec::with_capacity(entries.len() + 1);
                if !entries.iter().any(|entry| entry.value == agent.model) {
                    options.push(ChoiceItem::new(
                        &agent.model,
                        &format!(
                            "{}{}",
                            agent.model,
                            self.text("（当前已保存）", " (Current selection)")
                        ),
                    ));
                }
                options.extend(entries.iter().map(|entry| ChoiceItem {
                    value: entry.value.clone(),
                    label: self.model_label(&entry.value, &entry.label_for_effort(&agent.effort)),
                    group: self.group_label(&entry.group),
                }));
                options
            }
            Choice::Effort(index) => self
                .efforts(&self.draft.agents[index])
                .into_iter()
                .map(|effort| ChoiceItem::new(&effort, &self.effort_label(&effort)))
                .collect(),
            Choice::Speed(_) => speed_options("codex")
                .iter()
                .zip([
                    "Normal (follow environment)",
                    "Fast",
                    "Ultra fast",
                    "Priority",
                    "Flex",
                ])
                .map(|((value, zh), en)| {
                    let zh = if *value == "default" {
                        "普通（跟随环境变量）"
                    } else {
                        zh
                    };
                    ChoiceItem::new(value, self.text(zh, en))
                })
                .collect(),
        }
    }

    fn choice_disabled(&self, choice: Choice, options: &[ChoiceItem]) -> bool {
        match choice {
            Choice::Mode(index) => self.draft.default_agent == self.draft.agents[index].id,
            Choice::Effort(_) => options.len() <= 1,
            _ => options.is_empty(),
        }
    }

    fn filtered_choices(&self, choice: Choice, cx: &App) -> Vec<ChoiceItem> {
        let options = self.choices(choice);
        if !matches!(choice, Choice::Model(_)) {
            return options;
        }
        let query = self.search.read(cx).text.trim().to_lowercase();
        options
            .into_iter()
            .filter(|option| {
                option.value.to_lowercase().contains(&query)
                    || option.label.to_lowercase().contains(&query)
                    || option.group.to_lowercase().contains(&query)
            })
            .collect()
    }

    fn open_menu(&mut self, choice: Choice, window: &mut Window, cx: &mut Context<Self>) {
        let options = self.choices(choice);
        if self.choice_disabled(choice, &options) {
            return;
        }
        if self.menu.as_ref().is_some_and(|menu| menu.choice == choice) {
            self.close_menu(window, cx);
            return;
        }
        self.menu = None;
        self.search
            .update(cx, |input, cx| input.clear_sensitive(cx));
        let active = options
            .iter()
            .position(|item| item.value == self.selected(choice))
            .unwrap_or(0);
        self.menu = Some(Menu { choice, active });
        self.menu_scroll.set_offset(point(px(0.), px(0.)));
        if active > 0 {
            self.menu_scroll.scroll_to_item(active);
        }
        if matches!(choice, Choice::Model(_)) {
            window.focus(&self.search.read(cx).focus);
        } else {
            window.focus(&self.choice_focus[choice.slot()]);
        }
        cx.notify();
    }

    pub fn close_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(menu) = self.menu.take() else {
            return false;
        };
        window.focus(&self.choice_focus[menu.choice.slot()]);
        cx.notify();
        true
    }

    pub fn composing(&self, cx: &App) -> bool {
        self.numbers
            .iter()
            .chain(&self.roles)
            .any(|input| input.read(cx).composing())
            || (self.menu.is_some() && self.search.read(cx).composing())
    }

    fn apply_choice(
        &mut self,
        choice: Choice,
        value: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_menu(window, cx);
        match choice {
            Choice::Default => {
                self.draft.default_agent = value;
                self.draft.normalize_default_agent();
            }
            Choice::Mode(index) => {
                self.draft.agents[index].mode = if value == "auto" {
                    DispatchRoleMode::Auto
                } else {
                    DispatchRoleMode::Explicit
                };
            }
            Choice::Model(index) => {
                let agent = &mut self.draft.agents[index];
                if agent.model == value {
                    return;
                }
                agent.model = value;
                let efforts = self.efforts(&self.draft.agents[index]);
                let agent = &mut self.draft.agents[index];
                if !efforts.contains(&agent.effort) {
                    if let Some(effort) = efforts
                        .iter()
                        .find(|effort| *effort == "high")
                        .or_else(|| efforts.last())
                    {
                        agent.effort.clone_from(effort);
                    }
                }
            }
            Choice::Effort(index) => self.draft.agents[index].effort = value,
            Choice::Speed(index) => self.draft.agents[index].speed = Some(value),
        }
        self.changed(cx);
    }

    fn menu_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.menu.as_ref() else {
            return;
        };
        let choice = menu.choice;
        let search = matches!(choice, Choice::Model(_));
        if search && self.search.read(cx).composing() {
            return;
        }
        let key = event.keystroke.key.as_str();
        if event.keystroke.modifiers.modified() && key != "tab" {
            return;
        }
        let options = self.filtered_choices(choice, cx);
        let len = options.len();
        let active = menu.active.min(len.saturating_sub(1));
        match key {
            "escape" => {
                self.close_menu(window, cx);
            }
            "tab" => {
                self.close_menu(window, cx);
                if event.keystroke.modifiers.shift {
                    window.focus_prev();
                } else {
                    window.focus_next();
                }
            }
            "enter" | "space" if key == "enter" || !search => {
                if let Some(option) = options.into_iter().nth(active) {
                    self.apply_choice(choice, option.value, window, cx);
                }
            }
            "up" | "down" | "home" | "end"
                if len > 0 && (!search || matches!(key, "up" | "down")) =>
            {
                let next = match key {
                    "up" => (active + len - 1) % len,
                    "down" => (active + 1) % len,
                    "home" => 0,
                    _ => len - 1,
                };
                self.menu.as_mut().unwrap().active = next;
                if next == 0 {
                    self.menu_scroll.set_offset(point(px(0.), px(0.)));
                } else {
                    self.menu_scroll.scroll_to_item(next);
                }
                cx.notify();
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    fn button(
        id: impl Into<ElementId>,
        base: Div,
        focus: &FocusHandle,
        enabled: bool,
        action: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let button = base.id(id).relative();
        if !enabled {
            return button.opacity(0.55);
        }
        let action = Rc::new(action);
        let key_action = action.clone();
        let mouse_focus = focus.clone();
        button
            .track_focus(focus)
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                window.focus(&mouse_focus);
                cx.stop_propagation();
            })
            .on_click(cx.listener(move |this, event, window, cx| {
                if !matches!(event, ClickEvent::Keyboard(_)) {
                    action(this, window, cx);
                }
                cx.stop_propagation();
            }))
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space")
                    && !event.keystroke.modifiers.modified()
                {
                    key_action(this, window, cx);
                    cx.stop_propagation();
                }
            }))
            .child(focus_ring_with(
                focus.clone(),
                6.,
                true,
                FocusRingStyle::Shell,
            ))
    }

    fn checkbox(checked: bool, colors: Colors) -> Div {
        div()
            .flex_none()
            .size(px(15.))
            .rounded(px(3.))
            .border_1()
            .flex()
            .items_center()
            .justify_center()
            .border_color(rgba(if checked {
                colors.palette.accent
            } else {
                colors.palette.line_strong
            }))
            .bg(rgba(if checked {
                colors.palette.accent
            } else {
                colors.control
            }))
            .text_color(rgb(0xffffff))
            .text_size(px(11.))
            .line_height(relative(1.))
            .when(checked, |el| el.child("✓"))
    }

    fn action(colors: Colors, label: &'static str, primary: bool) -> Div {
        div()
            .h(px(28.))
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .px(px(11.))
            .rounded(px(6.))
            .border_1()
            .font_weight(FontWeight(600.))
            .text_size(px(11.))
            .border_color(rgba(if primary {
                colors.palette.accent
            } else {
                colors.palette.line_strong
            }))
            .text_color(rgba(if primary {
                0xffffffff
            } else {
                colors.palette.text
            }))
            .bg(rgba(if primary {
                colors.palette.accent
            } else {
                colors.control
            }))
            .child(label)
    }

    fn label(colors: Colors, label: impl Into<SharedString>) -> Div {
        div()
            .text_size(px(11.))
            .line_height(px(15.))
            .text_color(rgba(colors.palette.muted))
            .child(label.into())
    }

    fn trigger(
        &self,
        choice: Choice,
        options: &[ChoiceItem],
        colors: Colors,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let selected = self.selected(choice);
        let disabled = self.choice_disabled(choice, options);
        let open = self.menu.as_ref().is_some_and(|menu| menu.choice == choice);
        let label = if let Choice::Effort(index) = choice {
            if effort_is_fixed(
                &self.draft.agents[index].id,
                &self.draft.agents[index].model,
            ) {
                self.text("固定（无独立档位）", "Fixed (no separate level)")
                    .to_owned()
            } else if options.is_empty() {
                self.text("无可选档位", "No available level").to_owned()
            } else {
                self.effort_label(selected)
            }
        } else {
            options
                .iter()
                .find(|option| option.value == selected)
                .map(|option| option.label.clone())
                .unwrap_or_else(|| self.text("无可用执行者", "No enabled agent").to_owned())
        };
        let slot = choice.slot();
        let bounds = self.anchor_bounds.clone();
        let base = colors
            .field()
            .w_full()
            .when(open, |el| el.border_color(rgba(colors.palette.accent)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(label),
            )
            .child(icon("chevron-down", 12., colors.palette.muted >> 8))
            .when(!disabled, |el| {
                el.child(
                    canvas(
                        |_, _, _| (),
                        move |rect, _, window, _| {
                            let previous = bounds.borrow_mut()[slot].replace(rect);
                            if open && previous != Some(rect) {
                                window.request_animation_frame();
                            }
                        },
                    )
                    .absolute()
                    .inset_0()
                    .size_full(),
                )
            });
        Self::button(
            ("dispatch-choice", slot),
            base,
            &self.choice_focus[slot],
            !disabled,
            move |this, window, cx| this.open_menu(choice, window, cx),
            cx,
        )
        .when(!disabled, |el| {
            el.hover(move |style| style.border_color(rgba(colors.palette.accent)))
                .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                    if event.keystroke.key == "down" && !event.keystroke.modifiers.modified() {
                        this.open_menu(choice, window, cx);
                        cx.stop_propagation();
                    }
                }))
        })
    }

    fn render_menu(
        &self,
        viewport: Size<Pixels>,
        colors: Colors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let menu = self.menu.as_ref()?;
        let choice = menu.choice;
        let anchor = self.anchor_bounds.borrow()[choice.slot()]?;
        let options = self.filtered_choices(choice, cx);
        let empty = options.is_empty();
        let selected = self.selected(choice);
        let search = matches!(choice, Choice::Model(_));
        let margin = px(8.);
        let gap = px(4.);
        let width = anchor
            .size
            .width
            .max(px(if search { 380. } else { 160. }))
            .min(viewport.width - margin * 2.);
        let preferred_height = px(if search { 438. } else { 230. });
        let below = viewport.height - anchor.bottom() - margin - gap;
        let above = anchor.top() - margin - gap;
        let open_above = below < preferred_height && above > below;
        let height = preferred_height.min(if open_above { above } else { below });
        let x = anchor
            .left()
            .clamp(margin, (viewport.width - margin - width).max(margin));
        let (corner, y) = if open_above {
            (Corner::BottomLeft, anchor.top() - gap)
        } else {
            (Corner::TopLeft, anchor.bottom() + gap)
        };
        let mut list = div()
            .id("dispatch-menu-list")
            .min_h_0()
            .max_h((height - px(if search { 47. } else { 10. })).max(px(0.)))
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .track_scroll(&self.menu_scroll)
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation());
        let mut group = String::new();
        for (index, option) in options.into_iter().enumerate() {
            let heading = if !option.group.is_empty() && group != option.group {
                group.clone_from(&option.group);
                Some(Self::label(colors, group.clone()).px(px(7.)).pt(px(4.)))
            } else {
                None
            };
            let selected = option.value == selected;
            let active = index == menu.active;
            let accent = colors.palette.accent_soft;
            let row = div()
                .id(("dispatch-menu-item", index))
                .h(px(28.))
                .px(px(7.))
                .rounded(px(5.))
                .flex()
                .items_center()
                .gap(px(6.))
                .cursor_pointer()
                .text_size(px(12.))
                .when(selected || active, |el| el.bg(rgba(accent)))
                .when(selected, |el| el.text_color(rgba(colors.palette.accent)))
                .hover(move |el| el.bg(rgba(accent)))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.apply_choice(choice, option.value.clone(), window, cx);
                    cx.stop_propagation();
                }))
                .child(
                    div()
                        .w(px(13.))
                        .flex_none()
                        .child(if selected { "✓" } else { "" }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(option.label),
                );
            list = list.child(div().flex_none().children(heading).child(row));
        }
        if empty {
            list = list.child(
                Self::label(colors, self.text("没有匹配的模型", "No matching models")).p(px(8.)),
            );
        }

        let popup = div()
            .id("dispatch-menu")
            .w(width)
            .max_h(height)
            .flex()
            .flex_col()
            .gap(px(3.))
            .p(px(4.))
            .rounded(px(8.))
            .border_1()
            .border_color(rgba(colors.palette.line_strong))
            .bg(rgba(colors.popup))
            .text_color(rgba(colors.palette.text))
            .shadow(vec![shadow(0x0e1627, 0.25, 8., 24.)])
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .when(search, |el| {
                el.child(colors.field().flex_none().child(self.search.clone()))
            })
            .child(list);
        Some(
            deferred(
                anchored()
                    .position(point(x, y))
                    .anchor(corner)
                    .snap_to_window_with_margin(margin)
                    .child(popup),
            )
            .with_priority(101)
            .into_any_element(),
        )
    }

    fn backdrop(&self, viewport: Size<Pixels>, cx: &mut Context<Self>) -> AnyElement {
        deferred(
            anchored().position(point(px(0.), px(0.))).child(
                div()
                    .id("dispatch-menu-backdrop")
                    .w(viewport.width)
                    .h(viewport.height)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            let target = CHOICE_ORDER.into_iter().find(|choice| {
                                this.anchor_bounds.borrow()[choice.slot()]
                                    .is_some_and(|bounds| bounds.contains(&event.position))
                                    && !this.choice_disabled(*choice, &this.choices(*choice))
                            });
                            if let Some(choice) = target {
                                this.open_menu(choice, window, cx);
                            } else {
                                this.close_menu(window, cx);
                            }
                            cx.stop_propagation();
                        }),
                    )
                    .on_scroll_wheel(|_, _, cx| cx.stop_propagation()),
            ),
        )
        .with_priority(100)
        .into_any_element()
    }

    fn header(&self, colors: Colors, cx: &mut Context<Self>) -> Div {
        let label = if self.refreshing {
            self.text("正在刷新…", "Refreshing…")
        } else {
            self.text("↻ 刷新本机模型", "↻ Refresh local models")
        };
        let feedback = match self.refresh_status {
            Some("failed") => self.text(
                "刷新失败，沿用已有目录",
                "Refresh failed. Kept the existing catalog.",
            ),
            Some("cache") => self.text(
                "已更新可用结果，缓存写入失败",
                "Updated available results. Could not write the cache.",
            ),
            Some("ok") => self.text("已刷新本机模型", "Local models refreshed"),
            Some("codex") => self.text(
                "Codex 已刷新，反重力沿用已有目录",
                "Codex refreshed. Kept the Antigravity catalog.",
            ),
            Some("agy") => self.text(
                "反重力已刷新，Codex 沿用已有目录",
                "Antigravity refreshed. Kept the Codex catalog.",
            ),
            _ => "",
        };
        let refresh = Self::button(
            "dispatch-refresh",
            Self::action(colors, label, false),
            &self.refresh_focus,
            !self.refreshing,
            |this, _, cx| this.refresh(true, cx),
            cx,
        );
        div().flex().flex_col().gap(px(3.)).flex_none()
            .child(div().flex().items_start().justify_between().gap(px(8.)).pr(px(36.))
                .child(div().text_size(px(19.)).line_height(px(23.)).font_weight(FontWeight(680.))
                    .child(self.text("派活", "Dispatch")))
                .child(refresh))
            .child(Self::label(colors, self.text(
                "为码头托管会话统一设置各执行者的启停、默认模型、生态位职责与监督策略。",
                "Set agent availability, default models, responsibilities and supervision for Matou-managed sessions.",
            )))
            .when(!feedback.is_empty(), |el| el.child(Self::label(colors, feedback)))
    }

    fn number_field(&self, index: usize, colors: Colors, cx: &mut Context<Self>) -> Div {
        let (zh, en) = [
            ("没动静判卡", "Idle timeout"),
            ("单轮最长", "Round limit"),
            ("等回答", "Answer wait"),
            ("重试次数", "Retries"),
        ][index];
        let input = self.numbers[index].clone();
        let focus = input.read(cx).focus.clone();
        let invalid = self.parsed_number(index, cx).is_none();
        let (min, max) = NUMBER_LIMITS[index];
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                div()
                    .text_size(px(11.))
                    .font_weight(FontWeight(600.))
                    .child(self.text(zh, en)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .child(
                        colors
                            .field()
                            .id(("dispatch-number", index))
                            .w(px(54.))
                            .flex_none()
                            .h(px(30.))
                            .when(invalid, |el| el.border_color(rgba(colors.error)))
                            .child(input)
                            .child(focus_ring_with(focus, 6., true, FocusRingStyle::Shell)),
                    )
                    .child(Self::label(
                        colors,
                        if index == 3 {
                            self.text("次", "tries")
                        } else {
                            self.text("分钟", "min")
                        },
                    )),
            )
            .when(invalid, |el| {
                el.child(
                    Self::label(
                        colors,
                        if index == 3 {
                            format!("{} {min}–{max}", self.text("请输入整数", "Whole number"))
                        } else {
                            format!("{} {min}–{max}", self.text("范围", "Range"))
                        },
                    )
                    .text_color(rgba(colors.error)),
                )
            })
    }

    fn policy(
        &self,
        narrow: bool,
        compact: bool,
        viewport: Size<Pixels>,
        colors: Colors,
        cx: &mut Context<Self>,
    ) -> Div {
        let targets = self
            .draft
            .sub_dispatch_targets()
            .map(|agent| self.agent_label(&agent.id))
            .collect::<Vec<_>>();
        let inactive = self.draft.allow_sub_dispatch && targets.is_empty();
        let description = if !self.draft.allow_sub_dispatch {
            self.text(
                "已关闭跨执行者二次转包，所有任务仅允许由顶层监工直接派活。",
                "Sub-dispatch is off. Only the top-level supervisor can dispatch work.",
            )
            .to_owned()
        } else if targets.is_empty() {
            if let Some(agent) = self
                .draft
                .agents
                .iter()
                .find(|agent| agent.enabled && agent.mode == DispatchRoleMode::Auto)
            {
                let name = self.agent_label(&agent.id);
                match self.locale {
                    InterfaceLocale::Chinese => format!(
                        "当前仅 {name} 处于「按职责自动派」，暂无其他自动派协作者可接收转包；仅由顶层监工直接派活。"
                    ),
                    InterfaceLocale::English => format!(
                        "Only {name} is set to Auto by responsibility. With no eligible peer, the top-level supervisor dispatches all work."
                    ),
                }
            } else {
                self.text(
                    "暂无可接收二次转包的协作者，仅由顶层监工直接派活。",
                    "There are no eligible peers for sub-dispatch. Only the supervisor can dispatch work.",
                ).to_owned()
            }
        } else {
            let names = targets.join(self.text("、", ", "));
            match self.locale {
                InterfaceLocale::Chinese => format!(
                    "开启后，执行者干活时遇到其他「按职责自动派」协作者（当前：{names}）擅长的部分，可跨执行者转派 1 层子任务。"
                ),
                InterfaceLocale::English => format!(
                    "An agent can delegate one level of work to other agents set to Auto by responsibility when it matches their role (currently {names})."
                ),
            }
        };
        let default = self.choices(Choice::Default);
        let fallback = div()
            .flex()
            .items_center()
            .gap(px(10.))
            .min_w_0()
            .child(Self::label(colors, self.text("默认兜底执行者", "Default agent")).flex_none())
            .child(div().flex_1().min_w_0().child(self.trigger(
                Choice::Default,
                &default,
                colors,
                cx,
            )));
        let toggle = Self::button(
            "dispatch-sub-dispatch",
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(px(11.))
                .font_weight(FontWeight(600.))
                .child(Self::checkbox(self.draft.allow_sub_dispatch, colors))
                .child(self.text(
                    "允许执行者跨职责二次转包（1 层）",
                    "Allow 1-level cross-agent sub-dispatch",
                )),
            &self.sub_dispatch_focus,
            true,
            |this, _, cx| {
                this.draft.allow_sub_dispatch = !this.draft.allow_sub_dispatch;
                this.changed(cx);
            },
            cx,
        );
        let number_row = |range: std::ops::Range<usize>, cx: &mut Context<Self>| {
            div()
                .flex()
                .gap(px(8.))
                .children(range.map(|index| self.number_field(index, colors, cx)))
        };
        let numbers = if compact {
            div()
                .flex()
                .flex_col()
                .gap(px(7.))
                .child(number_row(0..2, cx))
                .child(number_row(2..4, cx))
        } else {
            number_row(0..4, cx)
        };
        colors
            .card()
            .flex_none()
            .child(
                div()
                    .flex()
                    .items_start()
                    .justify_between()
                    .gap(px(8.))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight(700.))
                            .child(self.text("全局兜底与监督策略", "Fallback and supervision")),
                    )
                    .child(Self::button(
                        "dispatch-preview-toggle",
                        Self::label(
                            colors,
                            if self.preview {
                                self.text("收起自动注入文本", "Hide injected text")
                            } else {
                                self.text("查看自动注入文本", "View injected text")
                            },
                        )
                        .text_color(rgba(colors.palette.accent)),
                        &self.preview_focus,
                        true,
                        |this, _, cx| {
                            this.preview = !this.preview;
                            cx.notify();
                        },
                        cx,
                    )),
            )
            .child(
                div()
                    .flex()
                    .gap(px(12.))
                    .when(narrow, |el| el.flex_col().gap(px(6.)))
                    .child(fallback.when(!narrow, |el| el.w(px(332.)).flex_none()))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(3.))
                            .child(toggle)
                            .child(
                                Self::label(colors, description)
                                    .pl(px(21.))
                                    .when(inactive, |el| {
                                        el.text_color(rgba(colors.palette.accent))
                                    }),
                            ),
                    ),
            )
            .child(numbers)
            .when(self.preview, |el| {
                el.child(
                    div()
                        .id("dispatch-markdown-preview")
                        .max_h((viewport.height / 3.).min(px(300.)))
                        .overflow_y_scroll()
                        .p(px(9.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(rgba(colors.palette.line))
                        .bg(rgba(colors.control))
                        .text_size(px(11.))
                        .line_height(px(17.))
                        .whitespace_normal()
                        .font_family(if cfg!(windows) { "Consolas" } else { "Menlo" })
                        .child(format_roles_markdown(&self.draft)),
                )
            })
    }

    fn agent_card(
        &self,
        index: usize,
        narrow: bool,
        compact: bool,
        colors: Colors,
        cx: &mut Context<Self>,
    ) -> Div {
        let agent = &self.draft.agents[index];
        let fallback = self.draft.default_agent == agent.id && agent.enabled;
        let fixed = effort_is_fixed(&agent.id, &agent.model);
        let modes = self.choices(Choice::Mode(index));
        let customized_role = agent.role != default_role_text(&agent.id);
        let role_len = self.roles[index]
            .read(cx)
            .text
            .replace("\r\n", "\n")
            .chars()
            .count();
        let role_input = self.roles[index].clone();
        let mut form = div()
            .flex()
            .flex_col()
            .gap(px(5.))
            .min_w_0()
            .when(!narrow, |el| el.w(px(332.)).flex_none());
        for (choice, label) in [
            (Choice::Model(index), self.text("默认模型", "Default model")),
            (Choice::Effort(index), self.text("推理档位", "Reasoning")),
            (Choice::Speed(index), self.text("推理速度", "Speed")),
        ] {
            if matches!(choice, Choice::Speed(_)) && agent.id != "codex" {
                continue;
            }
            let options = self.choices(choice);
            form = form.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .child(Self::label(colors, label).w(px(67.)).flex_none())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(self.trigger(choice, &options, colors, cx)),
                    ),
            );
        }
        if fixed {
            form = form.child(
                Self::label(
                    colors,
                    self.text(
                        "Orchestrator 可自派子代理并行干活",
                        "Orchestrator can run its own subagents in parallel",
                    ),
                )
                .pl(px(72.)),
            );
        }
        colors
            .card()
            .flex_none()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(6.))
                    .when(compact, |el| el.flex_col().items_start())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(7.))
                            .min_w_0()
                            .child(Self::button(
                                ("dispatch-enabled", index),
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(7.))
                                    .text_size(px(12.))
                                    .font_weight(FontWeight(700.))
                                    .child(Self::checkbox(agent.enabled, colors))
                                    .child(self.agent_label(&agent.id)),
                                &self.enabled_focus[index],
                                true,
                                move |this, _, cx| {
                                    let agent = &mut this.draft.agents[index];
                                    agent.enabled = !agent.enabled;
                                    this.draft.normalize_default_agent();
                                    this.changed(cx);
                                },
                                cx,
                            ))
                            .when(fallback, |el| {
                                el.child(
                                    Self::label(
                                        colors,
                                        self.text("默认兜底主力", "Default fallback"),
                                    )
                                    .text_size(px(10.))
                                    .px(px(5.))
                                    .rounded(px(5.))
                                    .bg(rgba(colors.palette.accent_soft))
                                    .text_color(rgba(colors.palette.accent)),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .flex_none()
                            .when(compact, |el| el.w_full())
                            .child(
                                Self::label(colors, self.text("调度方式", "Dispatch mode"))
                                    .flex_none(),
                            )
                            .child(
                                div()
                                    .when(compact, |el| el.flex_1().min_w_0())
                                    .when(!compact, |el| {
                                        el.w(px(if self.locale == InterfaceLocale::English {
                                            162.
                                        } else {
                                            146.
                                        }))
                                    })
                                    .child(self.trigger(Choice::Mode(index), &modes, colors, cx)),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap(px(16.))
                    .when(narrow, |el| el.flex_col().gap(px(8.)))
                    .child(form)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap(px(6.))
                                    .child(Self::label(
                                        colors,
                                        format!(
                                            "{}  {role_len} {}",
                                            self.text("生态位与职责", "Responsibilities"),
                                            self.text("字", "characters")
                                        ),
                                    ))
                                    .child(Self::button(
                                        ("dispatch-reset-role", index),
                                        Self::label(
                                            colors,
                                            self.text("恢复默认文案", "Restore default text"),
                                        )
                                        .text_color(rgba(
                                            if customized_role {
                                                colors.palette.accent
                                            } else {
                                                colors.palette.muted
                                            },
                                        )),
                                        &self.role_reset_focus[index],
                                        customized_role,
                                        move |this, window, cx| {
                                            let agent = &mut this.draft.agents[index];
                                            let role = default_role_text(&agent.id);
                                            agent.role = role.into();
                                            Self::replace_input(
                                                &this.roles[index],
                                                role,
                                                window,
                                                cx,
                                            );
                                            this.changed(cx);
                                        },
                                        cx,
                                    )),
                            )
                            .child(
                                div()
                                    .h(px(84.))
                                    .min_w_0()
                                    .flex()
                                    .rounded(px(6.))
                                    .border_1()
                                    .border_color(rgba(if role_len > ROLE_MAX_CHARS {
                                        colors.error
                                    } else {
                                        colors.palette.line_strong
                                    }))
                                    .bg(rgba(colors.control))
                                    .child(role_input),
                            )
                            .when(role_len > ROLE_MAX_CHARS, |el| {
                                el.child(
                                    Self::label(
                                        colors,
                                        format!(
                                            "{} {ROLE_MAX_CHARS}",
                                            self.text("职责文案字数上限", "Maximum characters"),
                                        ),
                                    )
                                    .text_color(rgba(colors.error)),
                                )
                            }),
                    ),
            )
    }

    fn footer(&self, compact: bool, colors: Colors, cx: &mut Context<Self>) -> Div {
        let notice = if !self.enabled_agent_exists() {
            Some((
                self.text("至少保留一个启用的执行者", "Enable at least one agent")
                    .to_owned(),
                colors.error,
            ))
        } else {
            self.save_status.as_ref().map(|status| match status {
                Ok(()) => (self.text("已保存", "Saved").to_owned(), colors.success),
                Err(error) => (
                    format!("{} {error}", self.text("保存失败。", "Save failed.")),
                    colors.error,
                ),
            })
        };
        let path = self.store.path().display().to_string();
        let visible_path = if compact {
            self.store
                .path()
                .file_name()
                .map(|name| format!("…/{}", name.to_string_lossy()))
                .unwrap_or_else(|| path.clone())
        } else {
            path.clone()
        };
        let path = SharedString::from(path);
        let file = Self::label(
            colors,
            format!(
                "{} {visible_path}",
                self.text("配置文件：", "Configuration:")
            ),
        )
        .id("dispatch-config-file")
        .min_w_0()
        .whitespace_nowrap()
        .overflow_hidden()
        .text_ellipsis()
        .tooltip(move |_, cx| cx.new(|_| CardTooltip(path.clone())).into());
        let scope = Self::label(
            colors,
            self.text(
                "（仅作用于码头内会话，不修改外部全局配置）",
                "(Applies to Matou-managed sessions only)",
            ),
        );
        div()
            .flex()
            .flex_none()
            .justify_between()
            .items_center()
            .gap(px(8.))
            .pt(px(8.))
            .border_t_1()
            .border_color(rgba(colors.palette.line))
            .when(compact, |el| el.flex_col().items_start().gap(px(4.)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .when(compact, |el| el.w_full())
                    .child(
                        div()
                            .flex()
                            .gap(px(5.))
                            .min_w_0()
                            .when(compact, |el| el.flex_col().gap(px(1.)))
                            .child(file)
                            .child(scope),
                    )
                    .children(notice.map(|(message, color)| {
                        Self::label(colors, message).text_color(rgba(color))
                    })),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .gap(px(8.))
                    .when(compact, |el| el.w_full().justify_end())
                    .child(Self::button(
                        "dispatch-restore",
                        Self::action(
                            colors,
                            self.text("恢复全部默认", "Restore all defaults"),
                            false,
                        ),
                        &self.restore_focus,
                        true,
                        Self::restore,
                        cx,
                    ))
                    .child(Self::button(
                        "dispatch-save",
                        Self::action(colors, self.text("保存", "Save"), true),
                        &self.save_focus,
                        self.can_save(cx),
                        Self::save,
                        cx,
                    )),
            )
    }

    #[cfg(debug_assertions)]
    pub fn probe_agent_scroll(&self) -> (Bounds<Pixels>, Pixels) {
        (self.agent_scroll.bounds(), self.agent_scroll.offset().y)
    }

    #[cfg(debug_assertions)]
    pub fn probe_focus_model(&self, index: usize, window: &mut Window) {
        window.focus(&self.choice_focus[Choice::Model(index).slot()]);
    }

    #[cfg(debug_assertions)]
    pub fn probe_focus_number(&self, index: usize, window: &mut Window, cx: &App) {
        window.focus(&self.numbers[index].read(cx).focus);
    }

    #[cfg(debug_assertions)]
    pub fn probe_number_focused(&self, index: usize, window: &Window, cx: &App) -> bool {
        self.numbers[index].read(cx).focus.is_focused(window)
    }

    #[cfg(debug_assertions)]
    pub fn probe_focus_save(&self, window: &mut Window) {
        window.focus(&self.save_focus);
    }
}

impl Render for DispatchRolesPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        let pane_width = self.pane_width.get();
        let narrow = viewport.width < px(760.) || (pane_width > px(0.) && pane_width < px(620.));
        let compact = if pane_width > px(0.) {
            pane_width < px(380.)
        } else {
            narrow
        };
        let width = self.pane_width.clone();
        let colors = Colors::new(self.theme);
        // Previous-frame anchors are read by the menu. Each enabled trigger
        // publishes its new bounds in the current frame.
        let menu = self.render_menu(viewport, colors, cx);
        let backdrop = menu.as_ref().map(|_| self.backdrop(viewport, cx));
        div()
            .id("dispatch-unified")
            .flex()
            .flex_col()
            .h_full()
            .min_h_0()
            .w_full()
            .gap(px(10.))
            .text_color(rgba(colors.palette.text))
            .capture_key_down(cx.listener(Self::menu_key))
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        if width.replace(bounds.size.width) != bounds.size.width {
                            window.request_animation_frame();
                        }
                    },
                )
                .absolute()
                .inset_0()
                .size_full(),
            )
            .child(self.header(colors, cx))
            .child(self.policy(narrow, compact, viewport, colors, cx))
            .child(
                div()
                    .id("dispatch-agent-scroll")
                    .flex()
                    .flex_col()
                    .min_h_0()
                    .gap(px(10.))
                    .overflow_y_scroll()
                    .track_scroll(&self.agent_scroll)
                    .children(
                        (0..3).map(|index| self.agent_card(index, narrow, compact, colors, cx)),
                    ),
            )
            .child(self.footer(compact, colors, cx))
            .children(backdrop)
            .children(menu)
    }
}

#[cfg(test)]
mod tests {
    use super::{AppTheme, Colors};

    #[test]
    fn dark_dispatch_surfaces_are_opaque_and_themed() {
        let light = Colors::new(AppTheme::Light);
        let dark = Colors::new(AppTheme::Dark);
        assert_eq!(light.card, 0xffffffff);
        assert_eq!(light.popup, 0xffffffff);
        for surface in [dark.card, dark.control, dark.popup] {
            assert_eq!(surface & 0xff, 0xff);
            assert_ne!(surface, 0xffffffff);
        }
    }
}
