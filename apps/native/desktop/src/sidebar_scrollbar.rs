use std::time::Duration;

use gpui::{prelude::*, *};

use crate::workspace::Workspace;

const INITIAL_HOLD_DELAY: Duration = Duration::from_millis(500);
const REPEAT_HOLD_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Clone, Copy)]
struct ThumbDrag {
    start_mouse_y: f32,
    start_offset: f32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TrackDirection {
    Up,
    Down,
}

#[derive(Clone, Copy)]
struct TrackHold {
    direction: TrackDirection,
    pointer: Point<Pixels>,
    track_bounds: Bounds<Pixels>,
    scale_factor: f32,
    generation: u64,
    interactive: bool,
}

#[derive(Clone, Copy, Default)]
pub struct SidebarScrollbarState {
    pub(crate) thumb_hovered: bool,
    dragging: Option<ThumbDrag>,
    track_hold: Option<TrackHold>,
    hold_generation: u64,
}

impl SidebarScrollbarState {
    pub fn is_dragging(&self) -> bool {
        self.dragging.is_some()
    }

    pub fn is_holding(&self) -> bool {
        self.track_hold.is_some()
    }

    pub fn clear_drag(&mut self) -> bool {
        self.dragging.take().is_some()
    }

    pub fn clear_track_hold(&mut self) -> bool {
        if self.track_hold.take().is_some() {
            self.hold_generation = self.hold_generation.wrapping_add(1);
            true
        } else {
            false
        }
    }

    pub fn cancel_interaction(&mut self) -> bool {
        let drag = self.clear_drag();
        let hold = self.clear_track_hold();
        drag || hold
    }

    pub fn reset(&mut self) {
        self.thumb_hovered = false;
        self.dragging = None;
        self.clear_track_hold();
    }

    fn begin_track_hold(
        &mut self,
        direction: TrackDirection,
        pointer: Point<Pixels>,
        track_bounds: Bounds<Pixels>,
        scale_factor: f32,
    ) -> u64 {
        self.hold_generation = self.hold_generation.wrapping_add(1);
        let generation = self.hold_generation;
        self.track_hold = Some(TrackHold {
            direction,
            pointer,
            track_bounds,
            scale_factor,
            generation,
            interactive: true,
        });
        generation
    }

    fn step_track_hold(
        &mut self,
        generation: u64,
        scroll: &mut ScrollHandle,
        cx: &mut Context<Workspace>,
    ) -> bool {
        let Some(hold) = self.track_hold else {
            return false;
        };
        if hold.generation != generation || !hold.interactive {
            if hold.generation == generation {
                self.clear_track_hold();
            }
            return false;
        }
        let Some(current) = ThumbMetrics::compute(hold.track_bounds, scroll, hold.scale_factor)
        else {
            self.clear_track_hold();
            return false;
        };
        if current.thumb_bounds.contains(&hold.pointer) {
            // Keep the press until mouse-up: moving farther along the track
            // while still holding must resume paging.
            if !self.thumb_hovered {
                self.thumb_hovered = true;
                cx.notify();
            }
            return true;
        }
        let on_pressed_side = hold.track_bounds.contains(&hold.pointer)
            && match hold.direction {
                TrackDirection::Up => hold.pointer.y < current.thumb_bounds.top(),
                TrackDirection::Down => hold.pointer.y >= current.thumb_bounds.bottom(),
            };
        if !on_pressed_side {
            return true;
        }
        let page_step = (current.viewport * 0.875).floor();
        let next_offset = match hold.direction {
            TrackDirection::Up => (current.offset - page_step).max(0.0),
            TrackDirection::Down => (current.offset + page_step).min(current.max_offset),
        };
        if (next_offset - current.offset).abs() > 0.01 {
            let scroll_x = scroll.offset().x;
            scroll.set_offset(point(scroll_x, px(-next_offset)));
            cx.notify();
            self.thumb_hovered =
                ThumbMetrics::compute(hold.track_bounds, scroll, hold.scale_factor)
                    .is_some_and(|next| next.thumb_bounds.contains(&hold.pointer));
        }
        true
    }
}

#[derive(Clone, Copy)]
struct ThumbMetrics {
    viewport: f32,
    max_offset: f32,
    offset: f32,
    thumb_height: f32,
    thumb_bounds: Bounds<Pixels>,
    travel: f32,
}

impl ThumbMetrics {
    fn compute(
        track_bounds: Bounds<Pixels>,
        scroll: &ScrollHandle,
        scale_factor: f32,
    ) -> Option<Self> {
        let viewport = f32::from(scroll.bounds().size.height);
        let max_offset = f32::from(scroll.max_offset().height).max(0.0);
        let track = f32::from(track_bounds.size.height);
        if viewport <= 0.0 || max_offset <= 0.5 || track <= 0.0 {
            return None;
        }
        let content = viewport + max_offset;
        // Browser scrollbar geometry is quantized in device pixels. Keeping
        // that grid matters for odd-sized thumbs on Retina displays.
        let thumb_height = (viewport * viewport / content * scale_factor)
            .round()
            .clamp(1.0, track * scale_factor)
            / scale_factor;
        let offset = (-f32::from(scroll.offset().y)).clamp(0.0, max_offset);
        let travel = (track - thumb_height).max(0.0);
        let device_top = offset / max_offset * travel * scale_factor;
        let thumb_top = if device_top > 0.0 && device_top < 1.0 {
            1.0
        } else {
            device_top.floor()
        }
        .clamp(0.0, travel * scale_factor)
            / scale_factor;
        let thumb_bounds = Bounds::new(
            point(track_bounds.left(), track_bounds.top() + px(thumb_top)),
            size(track_bounds.size.width, px(thumb_height)),
        );
        Some(Self {
            viewport,
            max_offset,
            offset,
            thumb_height,
            thumb_bounds,
            travel,
        })
    }
}

pub fn scrollbar(
    scroll: ScrollHandle,
    state: SidebarScrollbarState,
    interactive: bool,
    view: WeakEntity<Workspace>,
) -> Stateful<Div> {
    let hover_view = view.clone();
    let up_out_view = view.clone();
    div()
        .id("sidebar-scrollbar")
        .w(px(4.))
        .h_full()
        .flex_none()
        .cursor_default()
        .on_hover(move |hovered, _, cx| {
            if !hovered {
                let _ = hover_view.update(cx, |this, cx| {
                    if this.sidebar_scrollbar.thumb_hovered && !this.sidebar_scrollbar.is_dragging()
                    {
                        this.sidebar_scrollbar.thumb_hovered = false;
                        cx.notify();
                    }
                });
            }
        })
        .on_mouse_up_out(MouseButton::Left, move |_, _, cx| {
            let _ = up_out_view.update(cx, |this, cx| {
                if this.sidebar_scrollbar.cancel_interaction() {
                    cx.notify();
                }
            });
        })
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, cx| {
                    let scale_factor = window.scale_factor();
                    if !interactive && (state.is_dragging() || state.is_holding()) {
                        let cancel_view = view.clone();
                        cx.spawn(async move |cx| {
                            let _ = cancel_view.update(cx, |this, cx| {
                                if this.sidebar_scrollbar.cancel_interaction() {
                                    cx.notify();
                                }
                            });
                        })
                        .detach();
                    }

                    if let Some(metrics) = ThumbMetrics::compute(bounds, &scroll, scale_factor) {
                        let color = if state.is_dragging() || state.thumb_hovered {
                            rgb(0x8b949e)
                        } else {
                            rgb(0xd0d7de)
                        };
                        let radius = px(10.0_f32.min(f32::from(bounds.size.width) / 2.0));
                        window.paint_quad(fill(metrics.thumb_bounds, color).corner_radii(radius));
                    }

                    let down_view = view.clone();
                    window.on_mouse_event(move |event: &MouseDownEvent, phase, _, cx| {
                        if phase != DispatchPhase::Bubble
                            || !interactive
                            || event.button != MouseButton::Left
                            || !bounds.contains(&event.position)
                        {
                            return;
                        }
                        let pos = event.position;
                        let option_click = cfg!(target_os = "macos") && event.modifiers.alt;
                        let handled = down_view.update(cx, |this, cx| {
                            let Some(current) =
                                ThumbMetrics::compute(bounds, &this.sidebar_scroll, scale_factor)
                            else {
                                return false;
                            };
                            this.sidebar_scrollbar.clear_track_hold();
                            if current.thumb_bounds.contains(&pos) {
                                this.sidebar_scrollbar.dragging = Some(ThumbDrag {
                                    start_mouse_y: f32::from(pos.y),
                                    start_offset: current.offset,
                                });
                                this.sidebar_scrollbar.thumb_hovered = true;
                            } else if option_click {
                                let desired_thumb_top = (f32::from(pos.y)
                                    - f32::from(bounds.top())
                                    - current.thumb_height * 0.5)
                                    .clamp(0.0, current.travel);
                                let next_offset = if current.travel > 0.0 {
                                    (desired_thumb_top / current.travel * current.max_offset)
                                        .clamp(0.0, current.max_offset)
                                } else {
                                    0.0
                                };
                                let scroll_x = this.sidebar_scroll.offset().x;
                                this.sidebar_scroll
                                    .set_offset(point(scroll_x, px(-next_offset)));
                                this.sidebar_scrollbar.dragging = Some(ThumbDrag {
                                    start_mouse_y: f32::from(pos.y),
                                    start_offset: next_offset,
                                });
                                this.sidebar_scrollbar.thumb_hovered = true;
                            } else {
                                let direction = if pos.y < current.thumb_bounds.top() {
                                    TrackDirection::Up
                                } else {
                                    TrackDirection::Down
                                };
                                let page_step = (current.viewport * 0.875).floor();
                                let next_offset = match direction {
                                    TrackDirection::Up => (current.offset - page_step).max(0.0),
                                    TrackDirection::Down => {
                                        (current.offset + page_step).min(current.max_offset)
                                    }
                                };
                                let scroll_x = this.sidebar_scroll.offset().x;
                                this.sidebar_scroll
                                    .set_offset(point(scroll_x, px(-next_offset)));
                                let generation = this.sidebar_scrollbar.begin_track_hold(
                                    direction,
                                    pos,
                                    bounds,
                                    scale_factor,
                                );
                                cx.spawn(async move |this, cx| {
                                    let mut delay = INITIAL_HOLD_DELAY;
                                    loop {
                                        cx.background_executor().timer(delay).await;
                                        let keep_going = this.update(cx, |this, cx| {
                                            this.sidebar_scrollbar.step_track_hold(
                                                generation,
                                                &mut this.sidebar_scroll,
                                                cx,
                                            )
                                        });
                                        match keep_going {
                                            Ok(true) => delay = REPEAT_HOLD_INTERVAL,
                                            _ => break,
                                        }
                                    }
                                })
                                .detach();
                            }
                            cx.notify();
                            true
                        });
                        if matches!(handled, Ok(true)) {
                            cx.stop_propagation();
                        }
                    });

                    let move_view = view.clone();
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                        if phase != DispatchPhase::Capture {
                            return;
                        }
                        let pos = event.position;
                        let dragging_button = event.dragging();
                        let stop = move_view.update(cx, |this, cx| {
                            let mut changed = false;
                            let mut active_drag = false;
                            if let Some(drag) = this.sidebar_scrollbar.dragging {
                                if !dragging_button {
                                    this.sidebar_scrollbar.dragging = None;
                                    changed = true;
                                } else {
                                    active_drag = true;
                                    if let Some(current) = ThumbMetrics::compute(
                                        bounds,
                                        &this.sidebar_scroll,
                                        scale_factor,
                                    ) && current.travel > 0.0
                                    {
                                        let delta_y = f32::from(pos.y) - drag.start_mouse_y;
                                        let next_offset = (drag.start_offset
                                            + delta_y / current.travel * current.max_offset)
                                            .clamp(0.0, current.max_offset);
                                        let scroll_x = this.sidebar_scroll.offset().x;
                                        this.sidebar_scroll
                                            .set_offset(point(scroll_x, px(-next_offset)));
                                        changed = true;
                                    }
                                }
                            }
                            if let Some(hold) = &mut this.sidebar_scrollbar.track_hold {
                                if !dragging_button {
                                    this.sidebar_scrollbar.clear_track_hold();
                                } else {
                                    hold.pointer = pos;
                                    hold.track_bounds = bounds;
                                    hold.scale_factor = scale_factor;
                                    hold.interactive = interactive;
                                }
                            }
                            let hovered = interactive
                                && bounds.contains(&pos)
                                && ThumbMetrics::compute(
                                    bounds,
                                    &this.sidebar_scroll,
                                    scale_factor,
                                )
                                .is_some_and(|current| current.thumb_bounds.contains(&pos));
                            if this.sidebar_scrollbar.thumb_hovered != hovered {
                                this.sidebar_scrollbar.thumb_hovered = hovered;
                                changed = true;
                            }
                            if changed {
                                cx.notify();
                            }
                            active_drag
                        });
                        if matches!(stop, Ok(true)) {
                            cx.stop_propagation();
                        }
                    });

                    let up_view = view.clone();
                    window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                        if phase != DispatchPhase::Capture || event.button != MouseButton::Left {
                            return;
                        }
                        let pos = event.position;
                        let stop = up_view.update(cx, |this, cx| {
                            let was_drag = this.sidebar_scrollbar.clear_drag();
                            let was_hold = this.sidebar_scrollbar.clear_track_hold();
                            if was_drag || was_hold {
                                this.sidebar_scrollbar.thumb_hovered = interactive
                                    && bounds.contains(&pos)
                                    && ThumbMetrics::compute(
                                        bounds,
                                        &this.sidebar_scroll,
                                        scale_factor,
                                    )
                                    .is_some_and(|current| current.thumb_bounds.contains(&pos));
                                cx.notify();
                                true
                            } else {
                                false
                            }
                        });
                        if matches!(stop, Ok(true)) {
                            cx.stop_propagation();
                        }
                    });

                    let wheel_view = view.clone();
                    window.on_mouse_event(move |event: &ScrollWheelEvent, phase, _, cx| {
                        if phase != DispatchPhase::Capture
                            || !interactive
                            || !bounds.contains(&event.position)
                            || event.modifiers.control
                            || event.modifiers.platform
                        {
                            return;
                        }
                        let delta_y = f32::from(event.delta.pixel_delta(px(20.)).y);
                        if delta_y == 0.0 {
                            return;
                        }
                        let _ = wheel_view.update(cx, |this, cx| {
                            let max_offset =
                                f32::from(this.sidebar_scroll.max_offset().height).max(0.0);
                            if max_offset <= 0.5 {
                                return;
                            }
                            let current = this.sidebar_scroll.offset();
                            let next_y = (f32::from(current.y) + delta_y).clamp(-max_offset, 0.0);
                            this.sidebar_scroll.set_offset(point(current.x, px(next_y)));
                            this.sidebar_scrollbar.thumb_hovered =
                                ThumbMetrics::compute(bounds, &this.sidebar_scroll, scale_factor)
                                    .is_some_and(|next| {
                                        next.thumb_bounds.contains(&event.position)
                                    });
                            cx.notify();
                        });
                        cx.stop_propagation();
                    });
                },
            )
            .size_full(),
        )
}
