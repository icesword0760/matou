//! One recovery's native animation owner. Windows deliberately has no raster fallback.
#[cfg(target_os = "macos")]
use crate::{recovery_water::Fluid, recovery_water_macos as cg};
use gpui::{prelude::*, *};
use matou_native_terminal::Theme;
#[cfg(target_os = "macos")]
use std::{sync::Arc, time::Instant};

impl RecoveryWater {
    pub const TOP: Pixels = px(32.);

    /// Append after the title and body of the card's relative container.
    pub fn overlay(entity: Entity<Self>) -> Div {
        div()
            .absolute()
            .top(Self::TOP)
            .left_0()
            .right_0()
            .bottom_0()
            .overflow_hidden()
            .child(entity)
    }
}

// Gate arguments as well as collection: release builds construct no diagnostic JSON.
#[cfg(target_os = "macos")]
macro_rules! trace {
    ($($argument:tt)*) => {
        #[cfg(debug_assertions)]
        trace($($argument)*);
    };
}
#[cfg(all(target_os = "macos", debug_assertions))]
#[derive(Default)]
struct DiagnosticBuffer {
    rows: Vec<serde_json::Value>,
    dropped: usize,
}
#[cfg(all(target_os = "macos", debug_assertions))]
static DIAGNOSTICS: std::sync::OnceLock<std::sync::Mutex<DiagnosticBuffer>> =
    std::sync::OnceLock::new();
#[cfg(target_os = "macos")]
fn trace(id: u64, event: &str, fields: serde_json::Value) {
    #[cfg(debug_assertions)]
    {
        static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        if *ENABLED.get_or_init(|| std::env::var_os("MATOU_RECOVERY_WATER_TRACE").is_some()) {
            let row = serde_json::json!({"id":id,"event":event,"at_ms":START.get_or_init(Instant::now).elapsed().as_secs_f64()*1000.,"data":fields});
            let mut buffer = DIAGNOSTICS.get_or_init(Default::default).lock().unwrap();
            if buffer.rows.len() < 65_536 {
                buffer.rows.push(row);
            } else {
                buffer.dropped += 1;
            }
        }
    }
    #[cfg(not(debug_assertions))]
    let _ = (id, event, fields);
}
/// Read-only debug observation for the independent diagnostic example. No I/O in frame callbacks.
#[cfg(all(target_os = "macos", debug_assertions))]
pub fn debug_diagnostics() -> serde_json::Value {
    let buffer = DIAGNOSTICS.get_or_init(Default::default).lock().unwrap();
    serde_json::json!({"limit":65_536,"dropped":buffer.dropped,"rows":buffer.rows})
}
#[cfg(target_os = "macos")]
#[derive(Clone, Copy, PartialEq)]
struct Spec {
    width: f64,
    height: f64,
    dpr: f64,
}
#[cfg(target_os = "macos")]
#[derive(Default)]
struct Images {
    background: Option<Arc<RenderImage>>,
    dynamic: Option<Arc<RenderImage>>,
    edges: Option<Arc<RenderImage>>,
}
#[cfg(target_os = "macos")]
impl Images {
    fn drain(&mut self) -> impl Iterator<Item = Arc<RenderImage>> {
        [
            self.background.take(),
            self.dynamic.take(),
            self.edges.take(),
        ]
        .into_iter()
        .flatten()
    }
}
#[cfg(target_os = "macos")]
struct Output {
    fluid: Fluid,
    images: Images,
    sampled_ms: f64,
}

pub struct RecoveryWater {
    generation: u64,
    theme: Theme,
    reduced_motion: bool,
    requested_visible: bool,
    #[cfg(target_os = "macos")]
    bottom_radius: Pixels,
    #[cfg(target_os = "macos")]
    state: MacState,
}
#[cfg(target_os = "macos")]
struct MacState {
    id: u64,
    origin: Instant,
    fluid: Option<Fluid>,
    revision: u64,
    sequence: u64,
    spec: Option<Spec>,
    images_spec: Option<Spec>,
    effective_visible: bool,
    reset_previous: Option<f64>,
    needs_frame: bool,
    inflight: bool,
    images: Images,
    painted_sequence: u64,
    error: Option<String>,
    observers: Vec<Subscription>,
}
impl RecoveryWater {
    pub fn new(
        generation: u64,
        theme: Theme,
        reduced_motion: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        #[cfg(target_os = "macos")]
        cx.on_release(|this, cx| {
            let state = &mut this.state;
            trace!(state.id, "release", serde_json::json!({"inflight":state.inflight,"generation":this.generation}));
            let images = state.images.drain().collect::<Vec<_>>();
            let id = state.id;
            // Release can run while its Window is borrowed; defer until it is back in App.
            cx.defer(move |cx| for image in images {
                trace!(id, "image_drop", serde_json::json!({"image":format!("{:?}", image.id),"reason":"entity_release"}));
                cx.drop_image(image, None);
            });
        }).detach();
        #[cfg(not(target_os = "macos"))]
        let _ = cx;
        Self {
            generation,
            theme,
            reduced_motion,
            requested_visible: true,
            #[cfg(target_os = "macos")]
            bottom_radius: Pixels::ZERO,
            #[cfg(target_os = "macos")]
            state: MacState {
                id: cx.entity_id().as_u64(),
                origin: Instant::now(),
                fluid: Some(Fluid::default()),
                revision: 1,
                sequence: 0,
                spec: None,
                images_spec: None,
                effective_visible: false,
                reset_previous: Some(0.),
                needs_frame: true,
                inflight: false,
                images: Images::default(),
                painted_sequence: 0,
                error: None,
                observers: Vec::new(),
            },
        }
    }
    #[cfg(target_os = "macos")]
    pub fn with_bottom_radius(mut self, radius: Pixels) -> Self {
        self.bottom_radius = radius;
        self
    }
    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.requested_visible == visible {
            return;
        }
        self.requested_visible = visible;
        #[cfg(target_os = "macos")]
        self.invalidate(false, cx);
        cx.notify();
    }
    #[cfg(target_os = "macos")]
    pub fn unmount(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_visible(false, cx);
        let state = &mut self.state;
        for image in state.images.drain() {
            trace!(
                state.id,
                "image_drop",
                serde_json::json!({"image":format!("{:?}",image.id),"reason":"unmount"}),
            );
            if let Err(error) = window.drop_image(image) {
                eprintln!("Recovery image release: {error:#}");
            }
        }
    }
    pub fn set_theme(&mut self, theme: Theme, cx: &mut Context<Self>) {
        if self.theme == theme {
            return;
        }
        self.theme = theme;
        #[cfg(target_os = "macos")]
        self.invalidate(true, cx);
        cx.notify();
    }
    pub fn set_generation(&mut self, generation: u64, cx: &mut Context<Self>) {
        if self.generation == generation {
            return;
        }
        self.generation = generation;
        #[cfg(target_os = "macos")]
        // Reconnecting and starting the replacement are one visible recovery.
        // Drop stale jobs, keeping the last painted water and its running clock.
        self.invalidate(false, cx);
        cx.notify();
    }
    #[cfg(target_os = "macos")]
    fn invalidate(&mut self, clear: bool, cx: &mut Context<Self>) {
        let s = &mut self.state;
        s.revision += 1;
        s.needs_frame = true;
        s.error = None;
        trace!(
            s.id,
            "revision",
            serde_json::json!({"revision":s.revision,"generation":self.generation,"inflight":s.inflight,"clear":clear}),
        );
        if clear {
            s.images_spec = None;
            let images = s.images.drain().collect::<Vec<_>>();
            let id = s.id;
            cx.defer(move |cx| {
                for image in images {
                    trace!(
                        id,
                        "image_drop",
                        serde_json::json!({"image":format!("{:?}",image.id),"reason":"revision"}),
                    );
                    cx.drop_image(image, None);
                }
            });
        }
    }
    #[cfg(target_os = "macos")]
    fn visibility(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.state.effective_visible == visible {
            return;
        }
        self.state.effective_visible = visible;
        if visible {
            self.state.reset_previous = Some(self.state.origin.elapsed().as_secs_f64() * 1000.);
        }
        let error = self.state.error.take();
        self.invalidate(false, cx);
        self.state.error = error;
        trace!(
            self.state.id,
            if visible { "resume" } else { "pause" },
            serde_json::json!({"elapsed_ms":self.state.origin.elapsed().as_secs_f64()*1000.}),
        );
    }
    #[cfg(target_os = "macos")]
    fn native_visible(&mut self, window: &Window) -> bool {
        match crate::macos::is_window_visible(window) {
            Ok(value) => value,
            Err(error) => {
                self.fail(format!("Window visibility: {error:#}"));
                false
            }
        }
    }
    #[cfg(target_os = "macos")]
    fn fail(&mut self, error: String) {
        eprintln!("Recovery water failed: {error}");
        self.state.error = Some(error);
    }
    #[cfg(target_os = "macos")]
    fn layout(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let mask = bounds.intersect(&window.content_mask().bounds);
        let spec = Spec {
            width: f64::from(f32::from(bounds.size.width)).max(0.),
            height: f64::from(f32::from(bounds.size.height)).max(0.),
            dpr: f64::from(window.scale_factor()).min(2.),
        };
        let in_mask = self.requested_visible
            && mask.size.width > px(0.)
            && mask.size.height > px(0.)
            && spec.width > 0.
            && spec.height > 0.;
        if self.state.spec != Some(spec) {
            self.state.spec = Some(spec);
            // Card activation can change the bounds every display frame.
            // Paint results at the current bounds while the next size renders.
            // Cancelling every in-flight job here can starve the first image.
            self.state.needs_frame = true;
            trace!(
                self.state.id,
                "size",
                serde_json::json!({"x":f32::from(bounds.left()),"y":f32::from(bounds.top()),"width":spec.width,"height":spec.height,"dpr":spec.dpr,"background":format!("{:06x}",self.background_color()),"bottom_radius":f32::from(self.bottom_radius)}),
            );
        }
        let visible = in_mask && self.native_visible(window);
        self.visibility(visible, cx);
    }
    #[cfg(target_os = "macos")]
    fn background_color(&self) -> u32 {
        cg::water_base_color(self.theme)
    }
    #[cfg(target_os = "macos")]
    fn schedule(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let visible =
            self.requested_visible && self.state.effective_visible && self.native_visible(window);
        self.visibility(visible, cx);
        if !visible
            || self.state.inflight
            || self.state.error.is_some()
            || (self.reduced_motion && !self.state.needs_frame)
        {
            return;
        }
        // This call already follows painting in the current GPUI frame. A second
        // next-frame callback would add an unnecessary idle interval per result.
        self.start(window, cx);
    }
    #[cfg(target_os = "macos")]
    fn start(&mut self, window: &Window, cx: &mut Context<Self>) {
        let s = &mut self.state;
        if s.inflight || s.error.is_some() {
            return;
        }
        let Some(spec) = s.spec else {
            return;
        };
        let Some(mut fluid) = s.fluid.take() else {
            return;
        };
        s.inflight = true;
        s.sequence += 1;
        let (id, sequence, revision, origin) = (s.id, s.sequence, s.revision, s.origin);
        let reset_previous = std::mem::take(&mut s.reset_previous);
        let statics = s.images_spec != Some(spec);
        let (theme, reduced) = (self.theme, self.reduced_motion);
        trace!(
            id,
            "request",
            serde_json::json!({"sequence":sequence,"revision":revision,"generation":self.generation,"elapsed_ms":origin.elapsed().as_secs_f64()*1000.,"statics":statics,"width":spec.width,"height":spec.height,"dpr":spec.dpr,"reduced":reduced}),
        );
        let job = cx.background_executor().spawn(async move {
            let start = Instant::now();
            let sampled_ms = origin.elapsed().as_secs_f64()*1000.;
            if let Some(previous_ms) = reset_previous { fluid.reset_previous_frame(previous_ms); }
            let frame = fluid.sample(sampled_ms,spec.width,spec.height,spec.dpr,reduced);
            let math_ms = start.elapsed().as_secs_f64()*1000.;
            let dynamic = cg::draw_fluid(&frame).map(|bitmap| bitmap.render_image());
            let draw_ms = start.elapsed().as_secs_f64()*1000. - math_ms;
            let static_start = Instant::now();
            let mut images = Images::default();
            if let Ok(dynamic) = dynamic.as_ref() { images.dynamic = Some(dynamic.clone()); }
            if dynamic.is_ok() && statics {
                images.background = Some(cg::draw_background(spec.width,spec.height,spec.dpr,theme).render_image());
                images.edges = Some(cg::draw_edges(spec.width,spec.height,spec.dpr).render_image());
            }
            for image in [&images.background,&images.dynamic,&images.edges].into_iter().flatten() {
                trace!(id,"image_create",serde_json::json!({"sequence":sequence,"image":format!("{:?}",image.id)}));
            }
            trace!(id,"finish",serde_json::json!({"sequence":sequence,"revision":revision,"sampled_ms":sampled_ms,"previous_reset_ms":reset_previous,"frame_scale":frame["frameScale"],"math_ms":math_ms,"dynamic_ms":draw_ms,"static_ms":static_start.elapsed().as_secs_f64()*1000.,"total_ms":start.elapsed().as_secs_f64()*1000.}));
            (Output {fluid,images,sampled_ms}, dynamic.err().map(|error|format!("{error:#}")))
        });
        // Detached continuation owns the real job until it finishes, even if Entity is removed.
        cx.spawn_in(window, async move |weak, cx| {
            let (output,error) = job.await;
            let mut output = Some(output);
            let updated = cx.update(|window,cx| weak.update(cx, |this,cx| {
                let mut output = output.take().unwrap();
                this.state.inflight = false;
                this.state.fluid = Some(std::mem::take(&mut output.fluid));
                let visible = this.requested_visible && this.state.effective_visible && this.native_visible(window);
                this.visibility(visible,cx);
                if this.state.revision == revision && visible {
                    if let Some(error) = error { this.fail(error); }
                    else {
                        if statics { this.state.images_spec = Some(spec); }
                        for (old,new) in [(&mut this.state.images.background,&mut output.images.background),(&mut this.state.images.dynamic,&mut output.images.dynamic),(&mut this.state.images.edges,&mut output.images.edges)] {
                            if new.is_some() {
                                if let Some(image) = old.take() {
                                    trace!(id,"image_drop",serde_json::json!({"image":format!("{:?}",image.id),"reason":"replace"}));
                                    if let Err(error) = window.drop_image(image) { eprintln!("Recovery image release: {error:#}"); }
                                }
                                *old = new.take();
                            }
                        }
                        this.state.needs_frame = this.state.spec != Some(spec);
                        this.state.painted_sequence = sequence;
                        trace!(id,"accept",serde_json::json!({"sequence":sequence,"sampled_ms":output.sampled_ms}));
                    }
                } else { trace(id,"stale",serde_json::json!({"sequence":sequence,"revision":revision,"current_revision":this.state.revision})); }
                for image in output.images.drain() {
                    trace!(id,"image_drop",serde_json::json!({"image":format!("{:?}",image.id),"reason":"unuploaded_stale"}));
                }
                cx.notify();
            }));
            if let Some(mut output) = output {
                for image in output.images.drain() {
                    trace!(id,"image_drop",serde_json::json!({"image":format!("{:?}",image.id),"reason":"unuploaded_released"}));
                }
            }
            if !matches!(updated,Ok(Ok(()))) { trace(id,"orphan_finish",serde_json::json!({"sequence":sequence})); }
        }).detach();
    }
}
impl Render for RecoveryWater {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(target_os = "macos")]
        {
            if self.state.observers.is_empty() {
                self.state
                    .observers
                    .push(cx.observe_window_activation(window, |_, _, cx| cx.notify()));
                self.state
                    .observers
                    .push(cx.observe_window_bounds(window, |_, _, cx| cx.notify()));
            }
            let pre = cx.weak_entity();
            let paint = pre.clone();
            let corners = Corners {
                bottom_left: self.bottom_radius,
                bottom_right: self.bottom_radius,
                ..Corners::default()
            };
            div().size_full().relative().rounded_b(self.bottom_radius).bg(rgb(self.background_color())).child(canvas(
                move |bounds,window,cx| { let _ = pre.update(cx,|this,cx| this.layout(bounds,window,cx)); },
                move |bounds,_,window,cx| { let _ = paint.update(cx,|this,cx| {
                    if this.state.effective_visible {
                        let paint_started = Instant::now();
                        for image in [this.state.images.background.clone(),this.state.images.dynamic.clone(),this.state.images.edges.clone()].into_iter().flatten() {
                            if let Err(error) = window.paint_image(bounds,corners.clone(),image,0,false) { this.fail(format!("Image upload: {error:#}")); break; }
                        }
                        trace!(this.state.id,"paint",serde_json::json!({"sequence":this.state.painted_sequence,"paint_ok":this.state.error.is_none(),"callback_ms":paint_started.elapsed().as_secs_f64()*1000.,"images":[this.state.images.background.as_ref().map(|x|format!("{:?}",x.id)),this.state.images.dynamic.as_ref().map(|x|format!("{:?}",x.id)),this.state.images.edges.as_ref().map(|x|format!("{:?}",x.id))]}));
                        this.schedule(window,cx);
                    }
                }); },
            ).size_full()).when_some(self.state.error.clone(),|el,error| el.child(div().absolute().top_0().left_0().text_color(rgb(0xa82222)).child(error))).into_any_element()
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (window, cx, self.reduced_motion);
            div()
                .child("Native recovery rendering is not implemented on this platform")
                .into_any_element()
        }
    }
}
