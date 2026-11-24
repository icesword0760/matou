//! Workspace board presentation and optimistic drop calculation.
//! Task ownership comes from the hierarchy projection, independent of canvas mounts.

use std::collections::HashMap;
use std::time::Duration;

use matou_native_core::{
    hierarchy::{Session, Task},
    sessions::InterfaceLocale,
};

pub const BOARD_FEEDBACK_DURATION: Duration = Duration::from_millis(1_600);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoardStatus {
    Planned,
    Active,
    Blocked,
    Completed,
}

impl BoardStatus {
    pub const ALL: [Self; 4] = [Self::Planned, Self::Active, Self::Blocked, Self::Completed];

    pub fn from_task_status(status: &str) -> Self {
        match status {
            "active" => Self::Active,
            "blocked" => Self::Blocked,
            "completed" => Self::Completed,
            _ => Self::Planned,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Active => "active",
            Self::Blocked => "blocked",
            Self::Completed => "completed",
        }
    }

    fn title(self, locale: InterfaceLocale) -> &'static str {
        match self {
            Self::Planned => locale.text("就绪", "Ready"),
            Self::Active => locale.text("运行中", "Active"),
            Self::Blocked => locale.text("阻塞", "Blocked"),
            Self::Completed => locale.text("完成", "Done"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct BoardTask {
    id: String,
    title: String,
    status: BoardStatus,
    sort_key: String,
    created_at: i64,
    sessions: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoardMoveRequest {
    pub workspace_id: String,
    pub task_id: String,
    pub status: BoardStatus,
    /// Insert ahead of a card in the destination, or append when absent.
    pub before_task_id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoardDropError {
    WrongWorkspace,
    MissingTask,
    SelfDrop,
    InvalidDestination,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoardFeedback {
    Moved(BoardStatus),
    SaveFailed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceBoard {
    workspace_id: String,
    workspace_name: String,
    active_task_id: Option<String>,
    tasks: Vec<BoardTask>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoardView {
    pub workspace_id: String,
    pub title: String,
    pub task_count: String,
    pub drag_hint: &'static str,
    pub empty: &'static str,
    pub columns: [BoardColumn; 4],
    pub feedback: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoardColumn {
    pub status: BoardStatus,
    pub title: &'static str,
    pub label: String,
    pub count: String,
    pub cards: Vec<BoardCard>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoardCard {
    pub id: String,
    pub title: String,
    pub session_count: String,
    pub current: Option<&'static str>,
}

impl WorkspaceBoard {
    /// Use the full workspace projection, including tasks not placed in the current window.
    pub fn from_projection(
        workspace_id: &str,
        workspace_name: &str,
        active_task_id: Option<&str>,
        tasks: &[Task],
        sessions: &[Session],
    ) -> Self {
        Self::from_session_tasks(
            workspace_id,
            workspace_name,
            active_task_id,
            tasks,
            sessions.iter().map(|session| session.task_id.as_str()),
        )
    }

    pub fn from_session_tasks<'a>(
        workspace_id: &str,
        workspace_name: &str,
        active_task_id: Option<&str>,
        tasks: &[Task],
        session_tasks: impl IntoIterator<Item = &'a str>,
    ) -> Self {
        let mut counts = HashMap::new();
        for task_id in session_tasks {
            *counts.entry(task_id).or_insert(0) += 1;
        }
        Self::from_session_counts(
            workspace_id,
            workspace_name,
            active_task_id,
            tasks,
            |task| counts.get(task).copied().unwrap_or(0),
        )
    }

    pub fn from_session_counts(
        workspace_id: &str,
        workspace_name: &str,
        active_task_id: Option<&str>,
        tasks: &[Task],
        session_count: impl Fn(&str) -> usize,
    ) -> Self {
        let mut tasks: Vec<_> = tasks
            .iter()
            .filter(|task| task.workspace_id == workspace_id)
            .map(|task| BoardTask {
                id: task.id.clone(),
                title: task.title.clone(),
                status: BoardStatus::from_task_status(&task.status),
                sort_key: task.sort_key.clone(),
                created_at: task.created_at,
                sessions: session_count(&task.id),
            })
            .collect();
        tasks.sort_by(|left, right| {
            (&left.sort_key, left.created_at, &left.id).cmp(&(
                &right.sort_key,
                right.created_at,
                &right.id,
            ))
        });

        Self {
            workspace_id: workspace_id.into(),
            workspace_name: workspace_name.into(),
            active_task_id: active_task_id.map(str::to_owned),
            tasks,
        }
    }

    /// Compute the next preview from the caller's current snapshot. The caller owns
    /// persistence and restores its latest authoritative snapshot on save failure.
    pub fn after_drop(&self, request: &BoardMoveRequest) -> Result<Self, BoardDropError> {
        if self.workspace_id != request.workspace_id {
            return Err(BoardDropError::WrongWorkspace);
        }
        let source = self
            .tasks
            .iter()
            .find(|task| task.id == request.task_id)
            .ok_or(BoardDropError::MissingTask)?;
        if request.before_task_id.as_deref() == Some(source.id.as_str()) {
            return Err(BoardDropError::SelfDrop);
        }
        let mut destination: Vec<_> = self
            .tasks
            .iter()
            .filter(|task| task.status == request.status && task.id != source.id)
            .cloned()
            .collect();
        let index = match &request.before_task_id {
            Some(before) => destination
                .iter()
                .position(|task| &task.id == before)
                .ok_or(BoardDropError::InvalidDestination)?,
            None => destination.len(),
        };
        destination.insert(
            index,
            BoardTask {
                status: request.status,
                ..source.clone()
            },
        );
        for (index, task) in destination.iter_mut().enumerate() {
            task.sort_key = format!("a{index:08}");
        }

        let mut next = self.clone();
        next.tasks
            .retain(|task| task.status != request.status && task.id != source.id);
        next.tasks.extend(destination);
        Ok(next)
    }

    pub fn view(&self, locale: InterfaceLocale, feedback: Option<BoardFeedback>) -> BoardView {
        BoardView {
            workspace_id: self.workspace_id.clone(),
            title: format!("{} {}", self.workspace_name, locale.text("看板", "board")),
            task_count: count(locale, self.tasks.len(), "事项", "task"),
            drag_hint: locale.text("拖动事项以更新状态", "Drag a task to update its status"),
            empty: locale.text("暂无事项", "No tasks yet"),
            columns: BoardStatus::ALL.map(|status| {
                let title = status.title(locale);
                let cards: Vec<_> = self
                    .tasks
                    .iter()
                    .filter(|task| task.status == status)
                    .map(|task| BoardCard {
                        id: task.id.clone(),
                        title: task.title.clone(),
                        session_count: count(locale, task.sessions, "会话", "session"),
                        current: (self.active_task_id.as_ref() == Some(&task.id))
                            .then(|| locale.text("当前", "Current")),
                    })
                    .collect();
                let (label, count) = match locale {
                    InterfaceLocale::Chinese => {
                        (format!("{title}列"), format!("（{}）", cards.len()))
                    }
                    InterfaceLocale::English => {
                        (format!("{title} column"), format!("({})", cards.len()))
                    }
                };
                BoardColumn {
                    status,
                    title,
                    label,
                    count,
                    cards,
                }
            }),
            feedback: feedback.map(|feedback| match feedback {
                BoardFeedback::Moved(status) => {
                    let title = status.title(locale);
                    match locale {
                        InterfaceLocale::Chinese => format!("事项已移至「{title}」"),
                        InterfaceLocale::English => format!("Task moved to \"{title}\""),
                    }
                }
                BoardFeedback::SaveFailed => locale
                    .text(
                        "状态保存失败，请重试",
                        "Could not save the status. Try again.",
                    )
                    .into(),
            }),
        }
    }
}

fn count(locale: InterfaceLocale, count: usize, chinese: &str, english: &str) -> String {
    match locale {
        InterfaceLocale::Chinese => format!("{count} 个{chinese}"),
        InterfaceLocale::English if count == 1 => format!("1 {english}"),
        InterfaceLocale::English => format!("{count} {english}s"),
    }
}

// Workspace kanban board surface, appended to the native workspace_board module.
// Frozen source: hierarchy/WorkspaceKanbanBoard.tsx + hierarchy.css 1068-1104.

mod workspace_board_ui {
    use super::*;
    use gpui::{prelude::*, *};
    use std::rc::Rc;

    const BOARD_GAP: f32 = 11.;
    const BOARD_MIN_WIDTH: f32 = 840.;
    const BOARD_COLUMN_MIN_WIDTH: f32 = 192.;

    #[cfg(debug_assertions)]
    thread_local! {
        static BOARD_BOUNDS: std::cell::RefCell<Option<HashMap<String, Bounds<Pixels>>>> = const {
            std::cell::RefCell::new(None)
        };
    }

    #[cfg(debug_assertions)]
    pub fn capture_board_bounds(draw: impl FnOnce()) -> HashMap<String, Bounds<Pixels>> {
        BOARD_BOUNDS.with_borrow_mut(|bounds| *bounds = Some(HashMap::new()));
        draw();
        BOARD_BOUNDS.with_borrow_mut(|bounds| bounds.take().unwrap_or_default())
    }

    fn board_probe_bounds(_id: &str) -> Option<Canvas<()>> {
        #[cfg(debug_assertions)]
        if BOARD_BOUNDS.with_borrow(Option::is_some) {
            let id = _id.to_owned();
            return Some(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, _, _| {
                        BOARD_BOUNDS.with_borrow_mut(|capture| {
                            if let Some(capture) = capture {
                                capture.insert(id.clone(), bounds);
                            }
                        });
                    },
                )
                .absolute()
                .inset_0()
                .size_full(),
            );
        }
        None
    }

    /// Typed drag payload. The board only accepts payloads from its own workspace.
    struct BoardDrag {
        task_id: String,
        workspace_id: String,
        title: String,
    }

    /// The source card is dimmed while its own drag is in flight. GPUI only exposes
    /// `has_active_drag` publicly, so the id is kept in a small app global.
    struct BoardDragState {
        workspace_id: String,
        source_task_id: String,
        preview: WeakEntity<BoardDragPreview>,
    }

    impl Global for BoardDragState {}

    /// A standalone Render so the drag image never borrows the board element tree.
    struct BoardDragPreview {
        title: String,
        dark: bool,
    }

    impl Render for BoardDragPreview {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let palette = BoardPalette::new(self.dark);
            div()
                .w(px(196.))
                .p(px(11.))
                .rounded(px(9.))
                .border_1()
                .border_color(palette.accent)
                .bg(palette.panel)
                .shadow(vec![board_shadow(
                    0x000000,
                    if self.dark { 0.34 } else { 0.14 },
                    9.,
                    24.,
                )])
                .text_size(px(12.))
                .font_weight(FontWeight(600.))
                .line_height(px(16.8))
                .text_color(palette.card_title)
                .child(self.title.clone())
        }
    }

    #[derive(Clone, Copy)]
    struct BoardPalette {
        canvas: Rgba,
        panel: Rgba,
        panel_muted: Rgba,
        line: Rgba,
        line_strong: Rgba,
        text: Rgba,
        muted: Rgba,
        faint: Rgba,
        accent: Rgba,
        accent_soft: Rgba,
        accent_line: Rgba,
        card_title: Rgba,
        column_header: Rgba,
        drop_target: Rgba,
        feedback_text: Rgba,
        feedback_bg: Rgba,
    }

    impl BoardPalette {
        fn new(dark: bool) -> Self {
            if dark {
                Self {
                    canvas: rgb(0x181a1e),
                    panel: rgba(0x2c2f35f5),
                    panel_muted: rgba(0x23262be0),
                    line: rgba(0xffffff16),
                    line_strong: rgba(0xffffff26),
                    text: rgb(0xe1e5eb),
                    muted: rgb(0x8d96a2),
                    faint: rgb(0x626b77),
                    accent: rgb(0x67a6ef),
                    accent_soft: rgba(0x4a91e224),
                    accent_line: rgba(0x5b9eeb47),
                    card_title: rgb(0xe1e5eb),
                    column_header: rgb(0xd2d7de),
                    drop_target: rgb(0x202e3d),
                    feedback_text: rgb(0xd6dbe2),
                    feedback_bg: rgba(0x272a2ff5),
                }
            } else {
                Self {
                    canvas: rgb(0xf7f8fa),
                    panel: rgb(0xffffff),
                    panel_muted: rgb(0xfbfcfd),
                    line: rgba(0x424b581a),
                    line_strong: rgba(0x424b5829),
                    text: rgb(0x303744),
                    muted: rgb(0x7e8794),
                    faint: rgb(0xa3aab4),
                    accent: rgb(0x347fd6),
                    accent_soft: rgba(0x347fd61c),
                    accent_line: rgba(0x347fd647),
                    card_title: rgb(0x3b4350),
                    column_header: rgb(0x4f5966),
                    drop_target: rgba(0xf1f7fec7),
                    feedback_text: rgb(0x596372),
                    feedback_bg: rgba(0xfffffff0),
                }
            }
        }

        fn tone(&self, status: BoardStatus) -> Rgba {
            match status {
                BoardStatus::Planned => rgb(0x8e98a5),
                BoardStatus::Active => rgb(0xd29a35),
                BoardStatus::Blocked => rgb(0xd26762),
                BoardStatus::Completed => rgb(0x589a70),
            }
        }
    }

    fn board_shadow(color: u32, alpha: f32, y: f32, blur: f32) -> BoxShadow {
        let mut color: Hsla = rgb(color).into();
        color.a = alpha;
        BoxShadow {
            color,
            offset: point(px(0.), px(y)),
            blur_radius: px(blur),
            spread_radius: px(0.),
        }
    }

    /// Public board entry point. The caller owns all state and reacts to move requests.
    type MoveHandler = Rc<dyn Fn(BoardMoveRequest, &mut Window, &mut App)>;
    type SessionRows = Rc<dyn Fn(&str) -> Vec<AnyElement>>;

    pub fn render_workspace_board(
        view: &BoardView,
        dark: bool,
        on_move: MoveHandler,
    ) -> impl IntoElement {
        render_board(view, dark, Some(on_move), None)
    }

    pub fn render_workspace_board_with_sessions(
        view: &BoardView,
        dark: bool,
        on_move: MoveHandler,
        session_rows: SessionRows,
    ) -> impl IntoElement {
        render_board(view, dark, Some(on_move), Some(session_rows))
    }

    fn render_board(
        view: &BoardView,
        dark: bool,
        on_move: Option<MoveHandler>,
        session_rows: Option<SessionRows>,
    ) -> impl IntoElement {
        let palette = BoardPalette::new(dark);
        let columns = view
            .columns
            .iter()
            .map(|column| {
                board_column(
                    column,
                    &view.workspace_id,
                    dark,
                    view.empty,
                    on_move.clone(),
                    session_rows.as_ref(),
                )
                .into_any_element()
            })
            .collect::<Vec<_>>();

        let header = div()
            .h(px(68.))
            .flex_none()
            .px(px(18.))
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(9.))
                    .min_w(px(0.))
                    .child(
                        div()
                            .min_w(px(0.))
                            .truncate()
                            .text_size(px(17.))
                            .font_weight(FontWeight(680.))
                            .text_color(palette.text)
                            .child(view.title.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(11.))
                            .text_color(palette.muted)
                            .child(view.task_count.clone()),
                    ),
            )
            .when(on_move.is_some(), |header| {
                header.child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .text_size(px(11.))
                        .text_color(palette.muted)
                        .child(board_guide_icon())
                        .child(view.drag_hint),
                )
            });

        let scroll = div()
            .id("workspace-board-scroll")
            .flex_1()
            .min_h(px(0.))
            .min_w(px(0.))
            .px(px(18.))
            .pb(px(16.))
            .overflow_x_scroll()
            .scrollbar_width(px(4.))
            .child(
                div()
                    .flex()
                    .gap(px(BOARD_GAP))
                    .min_w(px(BOARD_MIN_WIDTH))
                    .h_full()
                    .children(columns),
            );

        div()
            .id("workspace-board")
            .relative()
            .size_full()
            .min_w(px(0.))
            .min_h(px(0.))
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(palette.canvas)
            .text_color(palette.text)
            .text_size(px(12.))
            .child(header)
            .child(scroll)
            .when_some(view.feedback.clone(), |board, feedback| {
                board.child(
                    div()
                        .absolute()
                        .left(px(0.))
                        .right(px(0.))
                        .bottom(px(18.))
                        .flex()
                        .justify_center()
                        .child(
                            div()
                                .relative()
                                .children(board_probe_bounds("board-feedback"))
                                .px(px(11.))
                                .py(px(8.))
                                .rounded(px(8.))
                                .border_1()
                                .border_color(palette.line)
                                .bg(palette.feedback_bg)
                                .text_size(px(11.))
                                .text_color(palette.feedback_text)
                                .child(feedback),
                        )
                        .with_animation(
                            "workspace-board-feedback-in",
                            appearance_animation(160),
                            |toast, progress| {
                                toast.opacity(progress).bottom(px(12. + 6. * progress))
                            },
                        ),
                )
            })
            .with_animation(
                "workspace-board-in",
                appearance_animation(180),
                |board, progress| board.opacity(progress).top(px(3. * (1. - progress))),
            )
    }

    fn appearance_animation(millis: u64) -> Animation {
        #[cfg(target_os = "macos")]
        let millis = if crate::macos::reduced_motion() {
            1
        } else {
            millis
        };
        Animation::new(Duration::from_millis(millis)).with_easing(ease_out_quint())
    }

    fn board_guide_icon() -> Div {
        let row = || {
            div()
                .flex()
                .items_center()
                .gap(px(3.))
                .child(div().size(px(1.6)).rounded_full().bg(rgb(0x738194)))
                .child(div().h(px(1.4)).w(px(8.4)).rounded_full().bg(rgb(0x738194)))
        };
        div()
            .size(px(14.))
            .flex_none()
            .flex()
            .flex_col()
            .justify_between()
            .py(px(2.3))
            .child(row())
            .child(row())
            .child(row())
    }

    fn board_column(
        column: &BoardColumn,
        workspace_id: &str,
        dark: bool,
        empty_text: &'static str,
        on_move: Option<MoveHandler>,
        session_rows: Option<&SessionRows>,
    ) -> impl IntoElement {
        let palette = BoardPalette::new(dark);
        let tone = palette.tone(column.status);
        let column_status = column.status;
        let status_key = column.status.as_str();
        let column_id = SharedString::from(format!("board-column:{workspace_id}:{status_key}"));
        let header_id =
            SharedString::from(format!("board-column-header:{workspace_id}:{status_key}"));
        let tail_id = SharedString::from(format!("board-column-tail:{workspace_id}:{status_key}"));

        let mut halo = tone;
        halo.a = 0.13;

        let mut header = div()
            .children(board_probe_bounds(&header_id))
            .id(header_id)
            .relative()
            .h(px(42.))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(7.))
            .px(px(11.))
            .border_b(px(1.))
            .border_color(palette.line)
            .text_color(palette.column_header)
            .child(
                div()
                    .size(px(13.))
                    .flex_none()
                    .rounded_full()
                    .bg(halo)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(div().size(px(7.)).rounded_full().bg(tone)),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .font_weight(FontWeight(650.))
                    .child(column.title),
            )
            .child(
                div()
                    .text_size(px(10.))
                    .text_color(palette.muted)
                    .child(column.count.clone()),
            );

        // Dropping on the header is the same append operation as the column tail.
        // Header and tail are siblings, never nested, so one drop dispatches once.
        {
            let workspace_id = workspace_id.to_owned();
            let drag_workspace = workspace_id.clone();
            let on_move = on_move.clone();
            header = header
                .can_drop({
                    let workspace_id = workspace_id.clone();
                    move |value, _, _| {
                        value
                            .downcast_ref::<BoardDrag>()
                            .is_some_and(|drag| drag.workspace_id == workspace_id)
                    }
                })
                .drag_over::<BoardDrag>(move |style, drag, _, _| {
                    if drag.workspace_id == drag_workspace {
                        style.bg(palette.drop_target)
                    } else {
                        style
                    }
                })
                .on_drop(move |drag: &BoardDrag, window, cx| {
                    board_emit_move(
                        &workspace_id,
                        drag,
                        column_status,
                        None,
                        &on_move,
                        window,
                        cx,
                    );
                });
        }

        let cards = column
            .cards
            .iter()
            .map(|card| {
                BoardCardComponent {
                    workspace_id: workspace_id.to_owned(),
                    card: card.clone(),
                    session_rows: session_rows.map_or_else(Vec::new, |rows| rows(&card.id)),
                    status: column_status,
                    dark,
                    on_move: on_move.clone(),
                }
                .into_any_element()
            })
            .collect::<Vec<_>>();

        let mut tail = div()
            .id(tail_id)
            .flex_grow()
            .min_h(px(if column.cards.is_empty() { 104. } else { 24. }))
            .flex()
            .flex_col()
            .when(column.cards.is_empty(), |tail| {
                tail.pt(px(22.))
                    .items_center()
                    .text_size(px(10.))
                    .text_color(palette.faint)
                    .child(empty_text)
            });

        // The append target is a sibling of every card target. Putting `on_drop` on
        // the column as well would dispatch twice for a card: GPUI keeps every
        // hovered hitbox in the drop path and does not require the event to bubble.
        {
            let workspace_id = workspace_id.to_owned();
            let drag_workspace = workspace_id.clone();
            let on_move = on_move.clone();
            tail = tail
                .can_drop({
                    let workspace_id = workspace_id.clone();
                    move |value, _, _| {
                        value
                            .downcast_ref::<BoardDrag>()
                            .is_some_and(|drag| drag.workspace_id == workspace_id)
                    }
                })
                .drag_over::<BoardDrag>(move |style, drag, _, _| {
                    if drag.workspace_id == drag_workspace {
                        style.bg(palette.drop_target)
                    } else {
                        style
                    }
                })
                .on_drop(move |drag: &BoardDrag, window, cx| {
                    board_emit_move(
                        &workspace_id,
                        drag,
                        column_status,
                        None,
                        &on_move,
                        window,
                        cx,
                    );
                });
        }

        // GPUI consumes the active payload at the first matching hitbox. In the
        // gaps around a card, the column is the first match and appends it.
        let drop_workspace_id = workspace_id.to_owned();
        div()
            .id(column_id)
            .on_drop(move |drag: &BoardDrag, window, cx| {
                board_emit_move(
                    &drop_workspace_id,
                    drag,
                    column_status,
                    None,
                    &on_move,
                    window,
                    cx,
                );
            })
            .flex_1()
            .min_w(px(BOARD_COLUMN_MIN_WIDTH))
            .h_full()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(px(11.))
            .border_1()
            .border_color(palette.line)
            .bg(palette.panel_muted)
            .child(header)
            .child(
                div()
                    .id(SharedString::from(format!(
                        "board-column-cards:{workspace_id}:{status_key}"
                    )))
                    .flex_1()
                    .min_h(px(150.))
                    .p(px(8.))
                    .flex()
                    .flex_col()
                    .overflow_y_scroll()
                    .scrollbar_width(px(4.))
                    .children(cards)
                    .child(tail),
            )
    }

    fn board_emit_move(
        workspace_id: &str,
        drag: &BoardDrag,
        status: BoardStatus,
        before_task_id: Option<String>,
        on_move: &Option<MoveHandler>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(on_move) = on_move else { return };
        if drag.workspace_id != workspace_id {
            return;
        }
        if before_task_id
            .as_deref()
            .is_some_and(|before| before == drag.task_id)
        {
            return;
        }
        on_move(
            BoardMoveRequest {
                workspace_id: workspace_id.to_owned(),
                task_id: drag.task_id.clone(),
                status,
                before_task_id,
            },
            window,
            cx,
        );
    }

    #[derive(IntoElement)]
    struct BoardCardComponent {
        workspace_id: String,
        card: BoardCard,
        session_rows: Vec<AnyElement>,
        status: BoardStatus,
        dark: bool,
        on_move: Option<MoveHandler>,
    }

    impl RenderOnce for BoardCardComponent {
        fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
            let palette = BoardPalette::new(self.dark);
            let tone = palette.tone(self.status);
            let card_id =
                SharedString::from(format!("board-card:{}:{}", self.workspace_id, self.card.id));
            let dragged = cx.has_active_drag()
                && cx.try_global::<BoardDragState>().is_some_and(|state| {
                    state.workspace_id == self.workspace_id
                        && state.source_task_id == self.card.id
                        && state.preview.upgrade().is_some()
                });

            let insert_before = self.card.id.clone();
            let hover_id = self.card.id.clone();
            let drop_workspace = self.workspace_id.clone();
            let move_workspace = self.workspace_id.clone();
            let drag_title = self.card.title.clone();
            let drop_on_move = self.on_move.clone();
            let drag_dark = self.dark;
            let drag_status = self.status;

            let card = div()
                .children(board_probe_bounds(&card_id))
                .id(card_id)
                .relative()
                .mb(px(7.))
                .p(px(11.))
                .overflow_hidden()
                .rounded(px(9.))
                .border_1()
                .border_color(palette.line)
                .bg(palette.panel)
                .shadow(vec![board_shadow(0x223244, 0.035, 1., 2.)])
                .when(self.on_move.is_some(), |card| {
                    card.cursor(CursorStyle::OpenHand)
                })
                .when(self.card.current.is_some(), |card| {
                    card.border_color(palette.accent_line).child(
                        div()
                            .absolute()
                            .left(px(0.))
                            .top(px(0.))
                            .bottom(px(0.))
                            .w(px(2.))
                            .bg(palette.accent),
                    )
                })
                .when(dragged, |card| {
                    card.opacity(0.38).cursor(CursorStyle::ClosedHand)
                })
                .when(self.card.current.is_none(), |card| {
                    card.hover(|style| style.border_color(palette.line_strong))
                })
                .child(
                    div()
                        .mb(px(12.))
                        .text_size(px(12.))
                        .font_weight(FontWeight(600.))
                        .line_height(px(16.8))
                        .text_color(palette.card_title)
                        .child(self.card.title.clone()),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .text_size(px(10.))
                        .text_color(palette.muted)
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(5.))
                                .child(div().size(px(5.)).flex_none().rounded_full().bg(tone))
                                .child(self.card.session_count.clone()),
                        )
                        .when_some(self.card.current, |footer, current| {
                            footer.child(
                                div()
                                    .px(px(5.))
                                    .py(px(2.))
                                    .rounded(px(5.))
                                    .bg(palette.accent_soft)
                                    .text_size(px(9.))
                                    .text_color(palette.accent)
                                    .child(current),
                            )
                        }),
                )
                .children(self.session_rows)
                .when(self.on_move.is_some(), |card| {
                    card.on_drag(
                        BoardDrag {
                            task_id: self.card.id,
                            workspace_id: self.workspace_id,
                            title: drag_title,
                        },
                        move |info, _, _, cx| {
                            let preview = cx.new(|_| BoardDragPreview {
                                title: info.title.clone(),
                                dark: drag_dark,
                            });
                            cx.set_global(BoardDragState {
                                workspace_id: info.workspace_id.clone(),
                                source_task_id: info.task_id.clone(),
                                preview: preview.downgrade(),
                            });
                            preview
                        },
                    )
                })
                .on_drop(move |drag: &BoardDrag, window, cx| {
                    board_emit_move(
                        &move_workspace,
                        drag,
                        drag_status,
                        Some(insert_before.clone()),
                        &drop_on_move,
                        window,
                        cx,
                    );
                })
                .drag_over::<BoardDrag>({
                    let drag_workspace = drop_workspace;
                    move |style, drag, _, _| {
                        if drag.workspace_id == drag_workspace && drag.task_id != hover_id {
                            style.border_color(palette.accent).bg(palette.accent_soft)
                        } else {
                            style
                        }
                    }
                });

            card
        }
    }
}

#[cfg(debug_assertions)]
pub use workspace_board_ui::capture_board_bounds;
pub use workspace_board_ui::{render_workspace_board, render_workspace_board_with_sessions};

#[cfg(test)]
mod tests {
    use super::*;
    use InterfaceLocale::{Chinese, English};

    fn task(id: &str, status: &str, sort_key: &str) -> Task {
        Task {
            id: id.into(),
            workspace_id: "w".into(),
            parent_task_id: None,
            title: id.into(),
            status: status.into(),
            execution_context_id: "ctx".into(),
            sort_key: sort_key.into(),
            is_pinned: false,
            pin_sort_key: String::new(),
            last_opened_at: 0,
            archived_at: None,
            created_at: 0,
            updated_at: 0,
            version: 1,
        }
    }

    fn session(id: &str, task_id: &str) -> Session {
        Session {
            id: id.into(),
            task_id: task_id.into(),
            execution_context_id: "ctx".into(),
            kind: "shell".into(),
            status: "created".into(),
            title: id.into(),
            title_source: None,
            provider_title: None,
            cwd: "/project".into(),
            created_at: 0,
            updated_at: 0,
            last_activity_at: 0,
            archived_at: None,
            version: 1,
        }
    }

    fn request(task_id: &str, status: BoardStatus, before: Option<&str>) -> BoardMoveRequest {
        BoardMoveRequest {
            workspace_id: "w".into(),
            task_id: task_id.into(),
            status,
            before_task_id: before.map(str::to_owned),
        }
    }

    fn ids(view: &BoardView, column: usize) -> Vec<&str> {
        view.columns[column]
            .cards
            .iter()
            .map(|card| card.id.as_str())
            .collect()
    }

    #[test]
    fn counts_three_distinct_sessions_mounted_across_two_canvases() {
        let mut outside = task("outside", "active", "a0");
        outside.workspace_id = "other".into();
        let tasks = [
            task("first", "active", "a0"),
            task("second", "", "a0"),
            outside,
        ];
        let sessions = [
            session("one", "first"),
            session("two", "first"),
            session("three", "first"),
            session("outside-shell", "outside"),
        ];
        let canvas_mounts = [["one", "two"], ["one", "three"]];
        assert_eq!(canvas_mounts.concat().len(), 4);
        assert!(
            canvas_mounts
                .iter()
                .flatten()
                .all(|id| sessions.iter().any(|session| session.id == *id))
        );

        let board =
            WorkspaceBoard::from_projection("w", "Studio", Some("first"), &tasks, &sessions);
        let english = board.view(English, None);
        assert_eq!(english.task_count, "2 tasks");
        assert_eq!(english.columns[1].cards[0].session_count, "3 sessions");
        assert_eq!(english.columns[0].cards[0].session_count, "0 sessions");
        let chinese = board.view(Chinese, None);
        assert_eq!(chinese.columns[1].cards[0].session_count, "3 个会话");
        assert_eq!(chinese.columns[1].cards[0].current, Some("当前"));
    }

    #[test]
    fn drop_moves_across_columns_before_a_card_and_reorders_within_column() {
        let board = WorkspaceBoard::from_projection(
            "w",
            "Studio",
            None,
            &[
                task("third", "active", "a2"),
                task("first", "planned", "a0"),
                task("second", "active", "a1"),
            ],
            &[],
        );
        let cross = board
            .after_drop(&request("first", BoardStatus::Active, Some("second")))
            .unwrap();
        assert!(ids(&cross.view(English, None), 0).is_empty());
        assert_eq!(
            ids(&cross.view(English, None), 1),
            ["first", "second", "third"]
        );
        let append = cross
            .after_drop(&request("first", BoardStatus::Active, None))
            .unwrap();
        assert_eq!(
            ids(&append.view(English, None), 1),
            ["second", "third", "first"]
        );
        let empty = append
            .after_drop(&request("third", BoardStatus::Completed, None))
            .unwrap();
        assert_eq!(ids(&empty.view(English, None), 3), ["third"]);
        assert_eq!(ids(&board.view(English, None), 0), ["first"]);
    }

    #[test]
    fn rejects_stale_self_and_cross_workspace_targets() {
        let board = WorkspaceBoard::from_projection(
            "w",
            "Studio",
            None,
            &[
                task("first", "planned", "a0"),
                task("second", "active", "a1"),
            ],
            &[],
        );
        let cases = [
            (
                BoardMoveRequest {
                    workspace_id: "other".into(),
                    ..request("first", BoardStatus::Active, None)
                },
                BoardDropError::WrongWorkspace,
            ),
            (
                request("missing", BoardStatus::Active, None),
                BoardDropError::MissingTask,
            ),
            (
                request("first", BoardStatus::Planned, Some("first")),
                BoardDropError::SelfDrop,
            ),
            (
                request("first", BoardStatus::Blocked, Some("second")),
                BoardDropError::InvalidDestination,
            ),
            (
                request("first", BoardStatus::Blocked, Some("gone")),
                BoardDropError::InvalidDestination,
            ),
        ];
        for (request, error) in cases {
            assert_eq!(board.after_drop(&request), Err(error));
        }
    }

    #[test]
    fn localized_board_labels_empty_and_move_feedback_match_reference() {
        let board = WorkspaceBoard::from_projection(
            "w",
            "Studio",
            Some("task"),
            &[task("task", "archived", "")],
            &[session("one", "task")],
        );
        let moved = Some(BoardFeedback::Moved(BoardStatus::Completed));
        let zh = board.view(Chinese, moved);
        let en = board.view(English, moved);
        assert_eq!(
            zh.columns.each_ref().map(|column| column.title),
            ["就绪", "运行中", "阻塞", "完成"]
        );
        assert_eq!(
            en.columns.each_ref().map(|column| column.title),
            ["Ready", "Active", "Blocked", "Done"]
        );
        assert_eq!(
            (
                &*zh.title,
                &*zh.task_count,
                zh.drag_hint,
                zh.empty,
                zh.feedback.as_deref()
            ),
            (
                "Studio 看板",
                "1 个事项",
                "拖动事项以更新状态",
                "暂无事项",
                Some("事项已移至「完成」")
            )
        );
        assert_eq!(
            (
                &*en.title,
                &*en.task_count,
                en.drag_hint,
                en.empty,
                en.feedback.as_deref()
            ),
            (
                "Studio board",
                "1 task",
                "Drag a task to update its status",
                "No tasks yet",
                Some("Task moved to \"Done\"")
            )
        );
        assert_eq!(
            (&*zh.columns[0].label, &*zh.columns[0].count),
            ("就绪列", "（1）")
        );
        assert_eq!(
            (&*en.columns[0].label, &*en.columns[0].count),
            ("Ready column", "(1)")
        );
        assert_eq!(en.columns[0].cards[0].session_count, "1 session");
        assert_eq!(en.columns[0].cards[0].current, Some("Current"));
        assert_eq!(
            board
                .view(Chinese, Some(BoardFeedback::SaveFailed))
                .feedback
                .as_deref(),
            Some("状态保存失败，请重试")
        );
        assert_eq!(
            board
                .view(English, Some(BoardFeedback::SaveFailed))
                .feedback
                .as_deref(),
            Some("Could not save the status. Try again.")
        );
    }
}
