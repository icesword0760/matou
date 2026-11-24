//! Editable project instruction file dialog based on the frozen TerminalHud
//! InstructionFileDialog and hierarchy.css instruction-file styles.
use super::*;
use std::{
    io::{self, Read as _, Write as _},
    path::Path,
};

/// Frozen light entry: the screenshot's neutral dark label on a light gray
/// pill. The pill is the existing `#00000014` light-theme hover wash and the
/// label is its `#2f3547` text; hover deepens the same wash one step.
const ENTRY_COLOR: u32 = 0x2f3547;
const ENTRY_BG: u32 = 0x00000014;
/// `.hierarchy-shell[data-theme="light"] .is-clickable:hover`.
const ENTRY_HOVER_COLOR: u32 = 0x2f3547;
const ENTRY_HOVER_BG: u32 = 0x0000001f;
const ENTRY_PADDING_X: f32 = 6.;
const ENTRY_PADDING_Y: f32 = 1.;
const ENTRY_RADIUS: f32 = 4.;

const PANEL_WIDTH: f32 = 760.;
const PANEL_HEIGHT: f32 = 640.;
const VIEWPORT_MARGIN_X: f32 = 64.;
const VIEWPORT_MARGIN_Y: f32 = 80.;
const PANEL_PADDING: f32 = 32.;
const PANEL_RADIUS: f32 = 14.;
const HEADER_HEIGHT: f32 = 64.;
const FOOTER_HEIGHT: f32 = 54.;
const PANEL_BG: u32 = 0xffffff;
const PANEL_BORDER: u32 = 0xd0d7de;
const PANEL_TEXT: u32 = 0x24292f;
const MUTED_TEXT: u32 = 0x6e7781;
const SECTION_BORDER: u32 = 0xd8dee4;
const BODY_BG: u32 = 0xf6f8fa;
const OVERLAY_BG: u32 = 0x1f23283d;
const ERROR_TEXT: u32 = 0xffa198;
const ERROR_BORDER: u32 = 0xf8514940;
const ERROR_BG: u32 = 0xf8514914;
const BUTTON_HOVER_BG: u32 = 0xeef1f4;

const CLOSE_LABEL: &str = "×";
const MONO_LINE_HEIGHT: f32 = 1.6;
const MONO_SIZE: f32 = 13.;
use crate::rename_input::INSTRUCTIONS_MAX_BYTES as LIMIT;

#[derive(Clone, Copy)]
struct Colors {
    panel: Rgba,
    border: Rgba,
    text: Rgba,
    muted: Rgba,
    section: Rgba,
    body: Rgba,
    hover: Rgba,
}

impl Colors {
    fn for_theme(dark: bool) -> Self {
        let (panel, border, text, muted, section, body, hover) = if dark {
            (
                0x1c2128, 0x30363d, 0xe6edf3, 0x8b949e, 0x30363d, 0x0d1117, 0x30363d,
            )
        } else {
            (
                PANEL_BG,
                PANEL_BORDER,
                PANEL_TEXT,
                MUTED_TEXT,
                SECTION_BORDER,
                BODY_BG,
                BUTTON_HOVER_BG,
            )
        };
        Self {
            panel: rgb(panel),
            border: rgb(border),
            text: rgb(text),
            muted: rgb(muted),
            section: rgb(section),
            body: rgb(body),
            hover: rgb(hover),
        }
    }
}

/// Frozen `.status-field.status-instructions.is-clickable` entry. The caller
/// supplies the label element and wires `Workspace::button` around it, which
/// owns the shared focus outline and activation.
pub(super) fn entry(label: impl IntoElement, dark: bool) -> Stateful<Div> {
    div()
        .id("footer-instructions")
        .flex_none()
        .flex()
        .items_center()
        .px(px(ENTRY_PADDING_X))
        .py(px(ENTRY_PADDING_Y))
        .rounded(px(ENTRY_RADIUS))
        .bg(rgba(if dark { 0xffffff15 } else { ENTRY_BG }))
        .text_color(rgb(if dark { 0xc9d1d9 } else { ENTRY_COLOR }))
        .font_weight(FontWeight::SEMIBOLD)
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(move |style| {
            style
                .text_color(rgb(if dark { 0xe6edf3 } else { ENTRY_HOVER_COLOR }))
                .bg(rgba(if dark { 0xffffff24 } else { ENTRY_HOVER_BG }))
        })
        .child(label)
}

/// Frozen `.instruction-file-dialog header button` with the light-theme colors.
fn close_button(colors: Colors) -> Stateful<Div> {
    div()
        .id("project-instructions-close")
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .size(px(30.))
        .rounded(px(6.))
        .text_size(px(22.))
        .text_color(colors.muted)
        .cursor_pointer()
        .hover(move |style| style.text_color(colors.text).bg(colors.hover))
        .child(ui_text(CLOSE_LABEL).normal_line_height())
}

fn action_button(
    id: &'static str,
    label: &'static str,
    primary: bool,
    disabled: bool,
    colors: Colors,
) -> Stateful<Div> {
    let background = if primary { rgb(0x347fd6) } else { colors.body };
    let hover = if primary { rgb(0x2f72c1) } else { colors.hover };
    div()
        .id(id)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .min_w(px(68.))
        .h(px(30.))
        .px(px(12.))
        .rounded(px(6.))
        .border_1()
        .border_color(if primary { background } else { colors.border })
        .text_size(px(11.))
        .text_color(if primary { rgb(0xffffff) } else { colors.text })
        .bg(background)
        .when(disabled, |button| {
            button
                .opacity(0.55)
                .cursor(CursorStyle::OperationNotAllowed)
        })
        .when(!disabled, |button| {
            button.cursor_pointer().hover(move |style| style.bg(hover))
        })
        .child(ui_text(label).normal_line_height())
}

fn editor_surface(
    input: Option<&Entity<RenameInput>>,
    focus: &FocusHandle,
    loading: bool,
    locale: InterfaceLocale,
    colors: Colors,
) -> Stateful<Div> {
    let surface = div()
        .id("project-instructions-preview")
        .relative()
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .min_h_0()
        .w_full()
        .bg(colors.body)
        .text_color(colors.text)
        .children(footer_bounds_probe("instructions-editor"));
    if let Some(input) = input {
        surface
            .font_family(monospace_family())
            .text_size(px(MONO_SIZE))
            .line_height(relative(MONO_LINE_HEIGHT))
            .child(input.clone())
    } else {
        surface
            .track_focus(focus)
            .items_center()
            .justify_center()
            .text_size(px(12.))
            .children(loading.then(|| ui_text(locale.text("正在读取…", "Loading…"))))
    }
}

struct Dialog {
    path: PathBuf,
    body: Stateful<Div>,
    error: Option<String>,
    close: Stateful<Div>,
    cancel: Stateful<Div>,
    save: Stateful<Div>,
    colors: Colors,
}

fn panel(viewport: Size<Pixels>, dialog: Dialog) -> Stateful<Div> {
    let Dialog {
        path,
        body,
        error,
        close,
        cancel,
        save,
        colors,
    } = dialog;
    let width = PANEL_WIDTH.min((f32::from(viewport.width) - VIEWPORT_MARGIN_X).max(0.));
    let height = PANEL_HEIGHT.min((f32::from(viewport.height) - VIEWPORT_MARGIN_Y).max(0.));
    let filename = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();

    let header = div()
        .flex_none()
        .h(px(HEADER_HEIGHT))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.))
        .px(px(18.))
        .border_b_1()
        .border_color(colors.section)
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .max_w(px(620.))
                .child(
                    div()
                        .whitespace_nowrap()
                        .text_size(px(15.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(colors.text)
                        .child(ui_text(filename).normal_line_height()),
                )
                .child(
                    div()
                        .mt(px(4.))
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_size(px(11.))
                        .text_color(colors.muted)
                        .child(ui_text(path.display().to_string()).normal_line_height()),
                ),
        )
        .child(close);

    let error = error.map(|error| {
        div()
            .flex_none()
            .px(px(18.))
            .py(px(8.))
            .border_t_1()
            .border_color(rgba(ERROR_BORDER))
            .bg(rgba(ERROR_BG))
            .text_color(rgb(ERROR_TEXT))
            .text_size(px(12.))
            .child(ui_text(error.to_owned()))
    });

    let footer = div()
        .flex_none()
        .h(px(FOOTER_HEIGHT))
        .flex()
        .items_center()
        .justify_end()
        .gap(px(8.))
        .pl(px(18.))
        .pr(px(14.))
        .border_t_1()
        .border_color(colors.section)
        .text_color(colors.muted)
        .text_size(px(11.))
        .child(cancel)
        .child(save);

    div()
        .id("project-instructions-dialog")
        .flex()
        .flex_col()
        .w(px(width))
        .h(px(height))
        .overflow_hidden()
        .rounded(px(PANEL_RADIUS))
        .border_1()
        .border_color(colors.border)
        .bg(colors.panel)
        .text_color(colors.text)
        .shadow(vec![BoxShadow {
            color: rgba(0x1f232842).into(),
            offset: point(px(0.), px(24.)),
            blur_radius: px(70.),
            spread_radius: px(0.),
        }])
        .child(header)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .flex()
                .flex_col()
                .overflow_hidden()
                .child(body),
        )
        .children(error)
        .child(footer)
}

/// Frozen `.instruction-file-overlay` light backdrop. `occlude` keeps the
/// workspace underneath inert; the caller attaches the outside-click close and
/// keeps panel events from bubbling here.
fn overlay(panel: Stateful<Div>) -> Stateful<Div> {
    div()
        .id("project-instructions-overlay")
        .absolute()
        .inset_0()
        .occlude()
        .flex()
        .items_center()
        .justify_center()
        .p(px(PANEL_PADDING))
        .bg(rgba(OVERLAY_BG))
        .child(panel)
}

fn monospace_family() -> &'static str {
    if cfg!(target_os = "macos") {
        "SF Mono"
    } else {
        "Consolas"
    }
}

#[derive(PartialEq, Eq)]
struct Target {
    session: String,
    cwd: PathBuf,
    mode: String,
}

pub(super) struct Instructions {
    target: Target,
    pub(super) path: Option<PathBuf>,
    pub(super) task: Option<gpui::Task<()>>,
    pub(super) preview: Option<Preview>,
}

pub(super) struct Preview {
    pub(super) original: Option<String>,
    pub(super) input: Option<Entity<RenameInput>>,
    pub(super) error: Option<String>,
    saving: bool,
    _input_events: Option<Subscription>,
}

impl Instructions {
    pub(super) fn filename_for(&self, session: Option<&str>) -> Option<String> {
        (session == Some(self.target.session.as_str())).then_some(())?;
        Some(
            self.path
                .as_ref()?
                .file_name()?
                .to_string_lossy()
                .into_owned(),
        )
    }
}

fn filenames(mode: &str) -> &'static [&'static str] {
    match mode {
        "claude-code" => &["CLAUDE.md"],
        "antigravity" => &["AGENTS.md", "GEMINI.md"],
        "codex" => &["AGENTS.md"],
        _ => &[],
    }
}

fn discover(cwd: &Path, mode: &str) -> Option<PathBuf> {
    let names = filenames(mode);
    names
        .iter()
        .map(|name| cwd.join(name))
        .find(|path| match std::fs::metadata(path) {
            Ok(metadata) => metadata.is_file() || names.len() == 1,
            Err(error) => error.kind() != io::ErrorKind::NotFound,
        })
}

fn read(path: &Path) -> io::Result<String> {
    let metadata = std::fs::metadata(path)?;
    if !metadata.is_file() {
        return Err(io::Error::other("Expected a regular file"));
    }
    if metadata.len() > LIMIT as u64 {
        return Err(io::Error::other("Instruction file exceeds 2 MB"));
    }
    let mut text = String::new();
    std::fs::File::open(path)?
        .take(LIMIT as u64 + 1)
        .read_to_string(&mut text)?;
    if text.len() > LIMIT {
        return Err(io::Error::other("Instruction file exceeds 2 MB"));
    }
    Ok(text)
}

fn save(path: &Path, original: &str, text: &str) -> io::Result<()> {
    if text.len() > LIMIT {
        return Err(io::Error::other("Instruction file exceeds 2 MB"));
    }
    let destination = path.canonicalize()?;
    let permissions = std::fs::metadata(&destination)?.permissions();
    let temp =
        destination.with_file_name(format!(".matou-instructions-{}.tmp", fork_submission_key()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(&temp)?;
    let result = (|| {
        file.set_permissions(permissions)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        if destination != path.canonicalize()? || read(&destination)? != original {
            return Err(io::Error::other(
                "The instruction file changed on disk. Reopen it before saving.",
            ));
        }
        #[cfg(windows)]
        return replace_preserving_dacl(&temp, &destination);
        #[cfg(not(windows))]
        std::fs::rename(&temp, &destination)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

#[cfg(windows)]
fn replace_preserving_dacl(temp: &Path, destination: &Path) -> io::Result<()> {
    use std::{ffi::c_void, os::windows::ffi::OsStrExt, ptr::null_mut};

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn ReplaceFileW(
            replaced: *const u16,
            replacement: *const u16,
            backup: *const u16,
            flags: u32,
            exclude: *mut c_void,
            reserved: *mut c_void,
        ) -> i32;
        fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
    }

    let backup = temp.with_extension("bak");
    let wide = |path: &Path| {
        path.as_os_str()
            .encode_wide()
            .chain([0])
            .collect::<Vec<_>>()
    };
    let replaced = wide(destination);
    let replacement = wide(temp);
    let backup_name = wide(&backup);
    // Use no ignore flags so Windows must preserve the destination's DACL.
    // Supply a backup name so a failed rename cannot discard the original.
    let ok = unsafe {
        ReplaceFileW(
            replaced.as_ptr(),
            replacement.as_ptr(),
            backup_name.as_ptr(),
            0,
            null_mut(),
            null_mut(),
        )
    };
    if ok != 0 {
        let _ = std::fs::remove_file(&backup);
        return Ok(());
    }
    let error = io::Error::last_os_error();
    const ERROR_UNABLE_TO_MOVE_REPLACEMENT_2: i32 = 1177;
    // On this failure Windows has already moved the original to the backup.
    // Restore it only if no other writer has since created the destination.
    if error.raw_os_error() == Some(ERROR_UNABLE_TO_MOVE_REPLACEMENT_2)
        && unsafe { MoveFileExW(backup_name.as_ptr(), replaced.as_ptr(), 0) } == 0
    {
        return Err(io::Error::other(format!(
            "{error}. Original instruction file retained at {} ({})",
            backup.display(),
            io::Error::last_os_error()
        )));
    }
    Err(error)
}

impl Workspace {
    pub(super) fn refresh_project_instructions(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let handle = window.window_handle();
        let target = self.footer_session(window).and_then(|id| {
            let mode = self.hud_node(id)?["currentMode"].as_str()?;
            if filenames(mode).is_empty() || self.is_stopped_session(id) {
                return None;
            }
            let node = self.hud_node(id)?;
            let cwd = ["environment", "worktree"]
                .into_iter()
                .find_map(|key| trimmed_graph_text(&node[key]["path"]))
                .or_else(|| {
                    self.detached_session_for_window(handle)
                        .is_none()
                        .then_some(())
                        .and(self.hierarchy.as_ref())
                        .and_then(|hierarchy| hierarchy.execution_context.as_ref())
                        .map(|context| context.cwd.as_str())
                })
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .or_else(|| self.footer_cwd(id))?;
            Some(Target {
                session: id.into(),
                cwd,
                mode: mode.into(),
            })
        });
        if self
            .project_instructions
            .get(&handle)
            .map(|state| &state.target)
            == target.as_ref()
        {
            return;
        }
        let was_open = self
            .project_instructions
            .remove(&handle)
            .is_some_and(|state| state.preview.is_some());
        if was_open {
            if let Some(terminal) = self
                .footer_session(window)
                .and_then(|id| self.terminals.get(id))
            {
                window.focus(&terminal.read(cx).focus);
            } else {
                window.blur();
            }
        }
        let Some(target) = target else {
            return;
        };
        let cwd = target.cwd.clone();
        let mode = target.mode.clone();
        let lookup = cx
            .background_executor()
            .spawn(async move { discover(&cwd, &mode) });
        let task = cx.spawn(async move |this, cx| {
            let path = lookup.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(state) = this.project_instructions.get_mut(&handle) {
                    state.path = path;
                    state.task = None;
                    cx.notify();
                }
            });
        });
        self.project_instructions.insert(
            handle,
            Instructions {
                target,
                path: None,
                task: Some(task),
                preview: None,
            },
        );
    }

    pub(super) fn open_project_instructions(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let handle = window.window_handle();
        let Some(path) = self
            .project_instructions
            .get(&handle)
            .filter(|state| state.preview.is_none())
            .and_then(|state| state.path.clone())
        else {
            return;
        };
        self.dismiss_footer_popover(window, cx);
        self.dismiss_permission_menu(window, cx);
        self.dismiss_hud_wiring_overlay(window, cx);
        window.focus(&self.control_focus["project-instructions-preview"]);
        let read = cx.background_executor().spawn(async move { read(&path) });
        let task = cx.spawn(async move |this, cx| {
            let result = read.await;
            let _ = this.update(cx, |this, cx| {
                if !this
                    .project_instructions
                    .get(&handle)
                    .is_some_and(|state| state.preview.is_some())
                {
                    return;
                }
                let focus = this.control_focus["project-instructions-preview"].clone();
                let error_prefix = this.locale.text("读取失败", "Could not read file");
                let state = this.project_instructions.get_mut(&handle).unwrap();
                let preview = state.preview.as_mut().unwrap();
                match result {
                    Ok(content) => {
                        let original = content.clone();
                        let input = cx.new(|cx| RenameInput::instructions(content, focus, cx));
                        let events = cx.subscribe(&input, move |_, _, event, cx| match event {
                            RenameInputEvent::Changed => cx.notify(),
                            RenameInputEvent::Confirm | RenameInputEvent::Cancel => {
                                let owner = cx.weak_entity();
                                let save = matches!(event, RenameInputEvent::Confirm);
                                cx.defer(move |cx| {
                                    let _ = handle.update(cx, |_, window, cx| {
                                        let _ = owner.update(cx, |this, cx| {
                                            if save {
                                                this.save_project_instructions(window, cx);
                                            } else {
                                                this.close_project_instructions(window, cx);
                                            }
                                        });
                                    });
                                });
                            }
                        });
                        preview.original = Some(original);
                        preview.input = Some(input);
                        preview._input_events = Some(events);
                    }
                    Err(error) => preview.error = Some(format!("{error_prefix}：{error}")),
                }
                state.task = None;
                cx.notify();
            });
        });
        let state = self.project_instructions.get_mut(&handle).unwrap();
        state.preview = Some(Preview {
            original: None,
            input: None,
            error: None,
            saving: false,
            _input_events: None,
        });
        state.task = Some(task);
        cx.notify();
    }

    fn save_project_instructions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let handle = window.window_handle();
        if self.read_only.is_some() {
            return;
        }
        let Some(state) = self.project_instructions.get_mut(&handle) else {
            return;
        };
        let Some((path, preview)) = state.path.clone().zip(state.preview.as_mut()) else {
            return;
        };
        let Some((input, original)) = preview.input.as_ref().zip(preview.original.as_ref()) else {
            return;
        };
        let text = input.read(cx).text.clone();
        if preview.saving || &text == original || input.read(cx).composing() {
            return;
        }
        let original = original.clone();
        preview.error = None;
        preview.saving = true;
        input.update(cx, |input, cx| input.set_document_read_only(true, cx));
        let save = cx
            .background_executor()
            .spawn(async move { save(&path, &original, &text) });
        state.task = Some(cx.spawn(async move |this, cx| {
            let result = save.await;
            let _ = handle.update(cx, |_, window, cx| {
                let _ = this.update(cx, |this, cx| {
                    let Some(state) = this.project_instructions.get_mut(&handle) else {
                        return;
                    };
                    state.task = None;
                    let Some(preview) = state.preview.as_mut() else {
                        return;
                    };
                    preview.saving = false;
                    if let Err(error) = result {
                        preview.error = Some(format!(
                            "{} {error}",
                            this.locale.text("保存失败。", "Could not save.")
                        ));
                        if let Some(input) = &preview.input {
                            input.update(cx, |input, cx| input.set_document_read_only(false, cx));
                        }
                        window.focus(&this.control_focus["project-instructions-preview"]);
                        cx.notify();
                    } else {
                        this.close_project_instructions(window, cx);
                    }
                });
            });
        }));
        cx.notify();
    }

    pub(super) fn close_project_instructions(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.project_instructions.get_mut(&window.window_handle()) else {
            return;
        };
        if state.preview.as_ref().is_none_or(|preview| preview.saving) {
            return;
        }
        state.preview = None;
        state.task = None;
        if state.path.is_some()
            && self.footer_session(window)
                == self
                    .project_instructions
                    .get(&window.window_handle())
                    .map(|s| s.target.session.as_str())
        {
            window.focus(&self.control_focus["project-instructions"]);
        } else if let Some(terminal) = self
            .footer_session(window)
            .and_then(|id| self.terminals.get(id))
        {
            window.focus(&terminal.read(cx).focus);
        } else {
            window.blur();
        }
        cx.notify();
    }

    pub(super) fn project_instructions_open(&self, window: &Window) -> bool {
        self.project_instructions
            .get(&window.window_handle())
            .is_some_and(|state| state.preview.is_some())
    }

    pub(super) fn project_instructions_overlay(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<Stateful<Div>> {
        let state = self.project_instructions.get(&window.window_handle())?;
        let preview = state.preview.as_ref()?;
        let path = state.path.clone()?;
        let colors = Colors::for_theme(self.theme.effective() == AppTheme::Dark);
        let locale = self.locale;
        let body = editor_surface(
            preview.input.as_ref(),
            &self.control_focus["project-instructions-preview"],
            preview.input.is_none() && preview.error.is_none(),
            locale,
            colors,
        );
        let cancel = action_button(
            "project-instructions-cancel",
            locale.text("取消", "Cancel"),
            false,
            preview.saving,
            colors,
        )
        .children(footer_bounds_probe("instructions-cancel"));
        let close = close_button(colors).children(footer_bounds_probe("instructions-close"));
        let disabled = preview.saving
            || self.read_only.is_some()
            || preview
                .input
                .as_ref()
                .zip(preview.original.as_ref())
                .is_none_or(|(input, original)| &input.read(cx).text == original);
        let save = action_button(
            "project-instructions-save",
            if preview.saving {
                locale.text("正在保存…", "Saving…")
            } else {
                locale.text("保存", "Save")
            },
            true,
            disabled,
            colors,
        )
        .children(footer_bounds_probe("instructions-save"));
        let close = if preview.saving {
            close.opacity(0.55)
        } else {
            self.button(
                close,
                &self.control_focus["project-instructions-close"],
                Self::close_project_instructions,
                cx,
            )
        };
        let cancel = if preview.saving {
            cancel
        } else {
            self.button(
                cancel,
                &self.control_focus["project-instructions-cancel"],
                Self::close_project_instructions,
                cx,
            )
        };
        let save = if disabled {
            save
        } else {
            self.button(
                save,
                &self.control_focus["project-instructions-save"],
                Self::save_project_instructions,
                cx,
            )
        };
        let panel = panel(
            window.viewport_size(),
            Dialog {
                path,
                body,
                error: preview.error.clone(),
                close,
                cancel,
                save,
                colors,
            },
        )
        .children(footer_bounds_probe("instructions-dialog"))
        .on_any_mouse_down(|_, _, cx| cx.stop_propagation());
        Some(
            overlay(panel).on_any_mouse_down(cx.listener(|this, _, window, cx| {
                this.close_project_instructions(window, cx);
                cx.stop_propagation();
            })),
        )
    }

    pub(super) fn project_instructions_key(
        &mut self,
        stroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(preview) = self
            .project_instructions
            .get(&window.window_handle())
            .and_then(|state| state.preview.as_ref())
        else {
            return false;
        };
        let editor = &self.control_focus["project-instructions-preview"];
        if preview.saving {
            return true;
        }
        if stroke.key == "escape"
            && preview
                .input
                .as_ref()
                .is_none_or(|input| !input.read(cx).composing())
        {
            self.close_project_instructions(window, cx);
            return true;
        }
        if stroke.key == "tab"
            && !stroke.modifiers.alt
            && !stroke.modifiers.control
            && !stroke.modifiers.platform
        {
            if preview
                .input
                .as_ref()
                .is_some_and(|input| input.read(cx).composing())
            {
                return false;
            }
            let mut controls = vec![
                &self.control_focus["project-instructions-close"],
                editor,
                &self.control_focus["project-instructions-cancel"],
            ];
            if self.read_only.is_none()
                && preview
                    .input
                    .as_ref()
                    .zip(preview.original.as_ref())
                    .is_some_and(|(input, original)| &input.read(cx).text != original)
            {
                controls.push(&self.control_focus["project-instructions-save"]);
            }
            let current = controls.iter().position(|focus| focus.is_focused(window));
            let next = match (current, stroke.modifiers.shift) {
                (Some(i), true) => (i + controls.len() - 1) % controls.len(),
                (Some(i), false) => (i + 1) % controls.len(),
                (None, true) => controls.len() - 1,
                (None, false) => 0,
            };
            window.focus(controls[next]);
            self.keyboard_focus = true;
            cx.notify();
            return true;
        }
        !editor.is_focused(window) && !matches!(stroke.key.as_str(), "enter" | "space")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn provider_discovery_and_file_save_preserve_text_and_reject_stale_edits() {
        let root =
            std::env::temp_dir().join(format!("matou-hud-instructions-{}", fork_submission_key()));
        std::fs::create_dir(&root).unwrap();
        struct RemoveFixture(PathBuf);
        impl Drop for RemoveFixture {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _fixture = RemoveFixture(root.clone());
        let agents = root.join("AGENTS.md");
        let gemini = root.join("GEMINI.md");
        let claude = root.join("CLAUDE.md");

        std::fs::write(&gemini, "# Gemini\n").unwrap();
        assert_eq!(discover(&root, "antigravity"), Some(gemini));
        assert!(discover(&root, "codex").is_none());
        std::fs::write(&claude, "# Claude\n").unwrap();
        std::fs::write(&agents, "# 指令\r\n\t正文\r\n").unwrap();
        assert_eq!(discover(&root, "claude-code"), Some(claude));
        assert_eq!(discover(&root, "antigravity"), Some(agents.clone()));
        assert_eq!(discover(&root, "codex"), Some(agents.clone()));
        assert!(discover(&root, "shell").is_none());
        assert!(discover(&root, "dsh").is_none());

        let original = read(&agents).unwrap();
        assert_eq!(original, "# 指令\r\n\t正文\r\n");
        let updated = "# 新指令\r\n\t编辑成功\r\n";
        save(&agents, &original, updated).unwrap();
        assert_eq!(read(&agents).unwrap(), updated);
        assert!(save(&agents, &original, "stale save").is_err());
        assert_eq!(read(&agents).unwrap(), updated);
        assert_eq!(std::fs::read_dir(root).unwrap().count(), 3);
    }

    #[::core::prelude::v1::test]
    fn instruction_dialog_uses_distinct_light_and_dark_surfaces() {
        let light = Colors::for_theme(false);
        let dark = Colors::for_theme(true);
        assert_eq!(light.panel, rgb(0xffffff));
        assert_eq!(light.body, rgb(0xf6f8fa));
        assert_eq!(dark.panel, rgb(0x1c2128));
        assert_eq!(dark.body, rgb(0x0d1117));
        assert_ne!(dark.text, light.text);
    }
}
