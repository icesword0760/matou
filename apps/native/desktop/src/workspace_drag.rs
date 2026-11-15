use crate::ui_text::ui_text;
use gpui::{prelude::*, *};

#[derive(Clone)]
pub struct WorkspaceDragSnapshot {
    pub name: String,
    pub active: bool,
    pub collapsed: bool,
    pub is_default: bool,
    pub is_pinned: bool,
    pub invalid: bool,
    pub actions_visible: bool,
    pub header_hovered: bool,
    pub tasks: Vec<WorkspaceDragTask>,
}

#[derive(Clone)]
pub struct WorkspaceDragTask {
    pub title: String,
    pub active: bool,
    pub is_pinned: bool,
    pub menu_visible: bool,
    pub hovered: bool,
}

pub fn active_marker(scale: f32) -> Div {
    let s = |value: f32| px(value * scale);
    let mut glow: Hsla = rgb(0x2d86ff).into();
    glow.a = 0.32;

    div()
        .absolute()
        .top(s(5.))
        .bottom(s(5.))
        .left_0()
        .w(s(2.))
        .rounded(s(99.))
        .overflow_hidden()
        .shadow(vec![BoxShadow {
            color: glow,
            offset: point(px(0.), px(0.)),
            blur_radius: s(10.),
            spread_radius: px(0.),
        }])
        .child(
            div()
                .absolute()
                .top_0()
                .h(relative(0.42))
                .rounded_t(s(1.))
                .w_full()
                .bg(linear_gradient(
                    180.,
                    linear_color_stop(rgb(0x67b9ff), 0.),
                    linear_color_stop(rgb(0x287cff), 1.),
                )),
        )
        .child(
            div()
                .absolute()
                .top(relative(0.42))
                .bottom_0()
                .rounded_b(s(1.))
                .w_full()
                .bg(linear_gradient(
                    180.,
                    linear_color_stop(rgb(0x287cff), 0.),
                    linear_color_stop(rgb(0x71bfff), 1.),
                )),
        )
}

pub fn group(snapshot: &WorkspaceDragSnapshot, width: Pixels, scale: f32) -> Div {
    let s = |value: f32| px(value * scale);

    let toggle = div()
        .h(s(34.))
        .flex_1()
        .min_w_0()
        .flex()
        .items_center()
        .pl(s(7.))
        .pr(s(5.))
        .gap(s(7.))
        .child(icon(
            if snapshot.collapsed {
                "chevron-right"
            } else {
                "chevron-down"
            },
            14. * scale,
            0x777777,
        ))
        .child(icon(
            "folder",
            16. * scale,
            if snapshot.active { 0x4078f2 } else { 0x656d76 },
        ))
        .child(
            div()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_size(s(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(if snapshot.active { 0x2f3547 } else { 0x656d76 }))
                .child(ui_text(snapshot.name.clone()).normal_line_height()),
        )
        .child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(s(6.))
                .when(snapshot.is_default, |status| {
                    status.child(
                        div()
                            .flex_none()
                            .px(s(5.))
                            .py(s(1.))
                            .rounded(s(5.))
                            .bg(rgb(0xeceef3))
                            .text_color(rgb(0x91949e))
                            .text_size(s(9.))
                            .line_height(s(12.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(ui_text("默认")),
                    )
                })
                .when(snapshot.is_pinned, |status| {
                    status.child(icon("pin", 13. * scale, 0x81848d))
                })
                .when(snapshot.invalid, |status| {
                    status.child(
                        div()
                            .flex_none()
                            .px(s(4.))
                            .py(s(1.))
                            .rounded(s(4.))
                            .bg(rgba(0xd99d3c1a))
                            .text_color(rgb(0xd99d3c))
                            .text_size(s(9.))
                            .line_height(s(12.5))
                            .child(ui_text("路径失效")),
                    )
                }),
        );

    let add_el = div()
        .size(s(26.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(s(6.))
        .opacity(if snapshot.invalid {
            0.28
        } else if snapshot.actions_visible {
            1.
        } else {
            0.
        })
        .child(icon("plus", 14. * scale, 0x81848d));

    let more_el = div()
        .size(s(26.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(s(6.))
        .opacity(if snapshot.actions_visible { 1. } else { 0. })
        .child(icon("ellipsis", 14. * scale, 0x81848d));

    let header = div()
        .min_h(s(34.))
        .flex()
        .items_center()
        .rounded(s(8.))
        .when(!snapshot.active && snapshot.header_hovered, |header| {
            header.bg(rgba(0x2f35470f))
        })
        .child(toggle)
        .child(add_el)
        .when(snapshot.invalid && !snapshot.is_default, |header| {
            header.child(
                div()
                    .h(s(24.))
                    .flex_none()
                    .px(s(7.))
                    .rounded(s(6.))
                    .border(s(1.))
                    .border_color(rgba(0x9a681840))
                    .bg(rgb(0xfff8e8))
                    .text_color(rgb(0x9a6818))
                    .text_size(s(9.))
                    .line_height(s(12.5))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(ui_text("恢复目录")),
            )
        })
        .child(more_el);

    div()
        .relative()
        .w(width * scale)
        .font_family(".AppleSystemUIFont")
        .text_color(rgb(0x303744))
        .when(snapshot.active, |group| group.child(active_marker(scale)))
        .child(header)
        .when(!snapshot.collapsed, |group| {
            group.child(
                div()
                    .ml(s(17.))
                    .pt(s(2.))
                    .flex()
                    .flex_col()
                    .gap(s(1.))
                    .children(snapshot.tasks.iter().map(|task| task_row(task, scale))),
            )
        })
}

fn task_row(task: &WorkspaceDragTask, scale: f32) -> Div {
    let s = |value: f32| px(value * scale);
    let color = if task.active { 0x2f3547 } else { 0x6f7683 };
    div()
        .h(s(34.))
        .flex_none()
        .rounded(s(8.))
        .flex()
        .items_center()
        .px(s(7.))
        .py(s(6.))
        .gap(s(7.))
        .text_size(s(13.))
        .line_height(s(19.5))
        .text_color(rgb(color))
        .bg(if task.hovered {
            rgba(0x57657f12)
        } else if task.active {
            rgba(0x57657f18)
        } else {
            rgba(0x00000000)
        })
        .child(icon("task", 12. * scale, color))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .line_height(s(19.5))
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .font_weight(FontWeight::MEDIUM)
                .child(ui_text(task.title.clone())),
        )
        .when(task.is_pinned, |row| {
            row.child(icon("pin", 13. * scale, 0x81848d))
        })
        .child(
            div()
                .size(s(22.))
                .flex_none()
                .rounded(s(4.))
                .flex()
                .items_center()
                .justify_center()
                .opacity(if task.menu_visible { 1. } else { 0. })
                .child(icon("ellipsis", 14. * scale, 0x656d76)),
        )
}

fn icon(name: &str, size: f32, color: u32) -> Svg {
    svg()
        .path(SharedString::from(format!("icons/{name}.svg")))
        .size(px(size))
        .flex_none()
        .text_color(rgb(color))
}
