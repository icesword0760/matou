//! Visible, data-free GPUI lifecycle diagnostic; never opens Matou storage or a PTY.
#[cfg(target_os = "macos")]
mod mac {
    use gpui::{prelude::*, *};
    use matou_native_desktop::{
        recovery_water_macos as cg,
        recovery_water_view::RecoveryWater,
        rename_input::{RenameInput, RenameInputEvent},
    };
    use matou_native_terminal::Theme;
    use std::{
        sync::Arc,
        time::{Duration, Instant},
    };

    // Bounded example-only samples; exported at phase boundaries, never per frame I/O.
    static EVENTS: std::sync::Mutex<Vec<serde_json::Value>> = std::sync::Mutex::new(Vec::new());
    fn record(row: serde_json::Value) {
        let mut events = EVENTS.lock().unwrap();
        if events.len() < 32_768 {
            events.push(row);
        }
    }
    fn export(cx: &mut App, name: &str) {
        let water = matou_native_desktop::recovery_water_view::debug_diagnostics();
        let events = EVENTS.lock().unwrap().clone();
        let target = std::path::Path::new(
            "artifacts/native-parity/native/evidence/recovery-water-scheduling",
        )
        .join(format!("{name}.json"));
        cx.background_executor().spawn(async move {
            let value=serde_json::json!({"water":water,"example":events,"physical_presentation":false,"terminal_input_latency":false});
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            std::fs::write(&target,serde_json::to_vec(&value).unwrap()).unwrap();
            eprintln!("Diagnostic snapshot: {}",target.display());
        }).detach();
    }
    actions!(water_diagnostic, [Quit]);
    struct Probe {
        cards: [Option<Entity<RecoveryWater>>; 2],
        input: Entity<RenameInput>,
        control: Arc<RenderImage>,
        small: bool,
        clip_second: bool,
        phase: String,
        started: Instant,
        _input_subscription: Subscription,
    }
    impl Probe {
        fn new(cx: &mut Context<Self>) -> Self {
            let input = cx.new(|cx| {
                let mut input = RenameInput::new(String::new(), cx);
                input.placeholder = "点击输入，动画不抢焦点".into();
                input
            });
            let subscription = cx.subscribe(&input, |this, input, event, cx| {
                if matches!(event,RenameInputEvent::Changed) {
                    record(serde_json::json!({"event":"input_changed","at_ms":this.started.elapsed().as_secs_f64()*1000.,"text":input.read(cx).text}));
                    cx.notify();
                }
            });
            let control = cg::diagnostic(1., 0., (0., 0.)).render_image();
            let released = control.clone();
            cx.on_release(move |_, cx| cx.defer(move |cx| cx.drop_image(released, None)))
                .detach();
            Self {
                cards: [
                    Some(cx.new(|cx| RecoveryWater::new(101, Theme::LIGHT, false, cx))),
                    Some(cx.new(|cx| RecoveryWater::new(201, Theme::LIGHT, false, cx))),
                ],
                input,
                control,
                small: false,
                clip_second: false,
                phase: if std::env::args().any(|arg| arg == "--manual-only") {
                    "手动补证：短文本输入与隐藏恢复"
                } else {
                    "双卡完整周期测量"
                }
                .into(),
                started: Instant::now(),
                _input_subscription: subscription,
            }
        }
        fn phase(&mut self, step: usize, cx: &mut Context<Self>) {
            match step {
                1 => {
                    self.small = true;
                    self.phase = "尺寸改变".into();
                }
                2 => {
                    self.clip_second = true;
                    self.phase = "B移出内容mask".into();
                }
                3 => {
                    self.clip_second = false;
                    self.phase = "B返回内容mask".into();
                }
                4 => {
                    self.cards[0]
                        .as_ref()
                        .unwrap()
                        .update(cx, |water, cx| water.set_theme(Theme::DARK, cx));
                    self.cards[1]
                        .as_ref()
                        .unwrap()
                        .update(cx, |water, cx| water.set_generation(202, cx));
                    self.phase = "A换主题，B新恢复代".into();
                }
                5 => {
                    self.cards[0]
                        .as_ref()
                        .unwrap()
                        .update(cx, |water, cx| water.set_visible(false, cx));
                    self.phase = "A暂停".into();
                }
                6 => {
                    self.cards[0]
                        .as_ref()
                        .unwrap()
                        .update(cx, |water, cx| water.set_visible(true, cx));
                    self.phase = "A恢复".into();
                }
                7 => {
                    self.cards[1] =
                        Some(cx.new(|cx| RecoveryWater::new(203, Theme::LIGHT, true, cx)));
                    self.phase = "B减少动态效果：单次静态".into();
                }
                8 => {
                    self.small = false;
                    self.phase = "静态B尺寸改变".into();
                }
                9 => {
                    self.cards[1]
                        .as_ref()
                        .unwrap()
                        .update(cx, |water, cx| water.set_visible(false, cx));
                    self.phase = "静态B暂停".into();
                }
                10 => {
                    self.cards[1]
                        .as_ref()
                        .unwrap()
                        .update(cx, |water, cx| water.set_visible(true, cx));
                    self.phase = "静态B恢复：只重画一次".into();
                }
                11 => {
                    self.cards[1] =
                        Some(cx.new(|cx| RecoveryWater::new(204, Theme::LIGHT, false, cx)));
                    self.phase = "自动序列完成，可进行真实输入/隐藏/移除".into();
                }
                _ => unreachable!(),
            }
            record(
                serde_json::json!({"event":"phase","step":step,"at_ms":self.started.elapsed().as_secs_f64()*1000.,"phase":self.phase}),
            );
            cx.notify();
        }
        fn hide(&mut self, window: &mut Window, cx: &mut Context<Self>) {
            record(
                serde_json::json!({"event":"hide","at_ms":self.started.elapsed().as_secs_f64()*1000.}),
            );
            if let Err(error) = matou_native_desktop::macos::hide_window(window) {
                eprintln!("Hide diagnostic: {error:#}");
                return;
            }
            cx.spawn_in(window,async move |weak,cx| {
                cx.background_executor().timer(Duration::from_secs(8)).await;
                let _=cx.update(|window,cx| {
                    window.activate_window();
                    window.refresh();
                    let _=weak.update(cx,|this,cx| {
                        record(serde_json::json!({"event":"show","at_ms":this.started.elapsed().as_secs_f64()*1000.}));cx.notify();
                    });
                });
            }).detach();
        }
    }
    impl Render for Probe {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let (width, height) = if self.small {
                (520., 560.)
            } else {
                (560., 600.)
            };
            let mut bodies = div()
                .relative()
                .w_full()
                .h(px(height + 42.))
                .overflow_hidden();
            for index in 0..2 {
                let x = if index == 1 && self.clip_second {
                    2000.
                } else {
                    index as f32 * (width + 12.)
                };
                let card = div()
                    .absolute()
                    .left(px(x))
                    .top_0()
                    .w(px(width))
                    .h(px(height + 42.))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .h(px(42.))
                            .flex_none()
                            .bg(rgb(0x26384a))
                            .text_color(rgb(0xffffff))
                            .p_2()
                            .child(format!(
                                "诊断卡 {} · 独立42px标题",
                                if index == 0 { "A" } else { "B" }
                            )),
                    )
                    .child(
                        div()
                            .w_full()
                            .h(px(height))
                            .flex_none()
                            .children(self.cards[index].clone()),
                    );
                bodies = bodies.child(card);
            }
            let feedback = self.input.read(cx).text.clone();
            let stamp = self.started.elapsed().as_secs_f64() * 1000.;
            div().size_full().bg(rgb(0xf0f2f5)).text_color(rgb(0x24364a)).p_3().flex().flex_col().gap_2()
                .child(div().flex().gap_3().items_center()
                    .child(div().w(px(350.)).child(self.input.clone()))
                    .child(div().id("hide-diagnostic").p_2().bg(rgb(0xd9e7f4)).cursor_pointer().on_click(cx.listener(|this,_,window,cx|this.hide(window,cx))).child("隐藏8秒并恢复"))
                    .child(div().id("finish-diagnostic").p_2().bg(rgb(0xd9e7f4)).cursor_pointer().on_click(cx.listener(|this,_,_,cx| {
                        this.cards=[None,None];
                        cx.spawn(async move |_,cx| {cx.background_executor().timer(Duration::from_secs(2)).await;let _=cx.update(|cx|export(cx,"removed"));}).detach();
                        this.phase="两张恢复Entity已移除；等待后台收尾".into();
                        record(serde_json::json!({"event":"remove_cards","at_ms":this.started.elapsed().as_secs_f64()*1000.}));cx.notify();
                    })).child("完成并移除双卡")))
                .child(div().flex().items_center().gap_3()
                    .child(canvas(move |_,_,_|(),{let image=self.control.clone();move |bounds,_,window,_| {
                        window.paint_image(bounds,Corners::default(),image,0,false).expect("Diagnostic color upload");
                    }}).w(px(128.)).h(px(96.)).bg(rgb(0xffffff)))
                    .child(div().flex().flex_col().gap_1().child("CG半透明控制片：顶部偏红，底部偏蓝（白底）")
                        .child(format!("输入反馈：{feedback}"))
                        .child(self.phase.clone()))
                    .child(canvas(move |_,_,_|(),move |_,_,_,_| {
                        record(serde_json::json!({"event":"feedback_paint","render_at_ms":stamp,"text":feedback}));
                    }).w(px(1.)).h(px(1.))))
                .child(bodies)
        }
    }
    pub fn run() {
        Application::new().run(|cx| {
            cx.on_action(|_: &Quit, cx| cx.quit());
            cx.on_app_quit(|cx| {
                // The CUA sequence removes cards before Quit, so all frame/jobs are already quiet.
                let water = matou_native_desktop::recovery_water_view::debug_diagnostics();
                let events = EVENTS.lock().unwrap().clone();
                let path = std::path::Path::new(
                    "artifacts/native-parity/native/evidence/recovery-water-scheduling/quit.json",
                );
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(
                    path,
                    serde_json::to_vec(
                        &serde_json::json!({"water":water,"example":events,"normal_quit":true}),
                    )
                    .unwrap(),
                )
                .unwrap();
                let _ = cx;
                async {}
            })
            .detach();
            cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
            cx.set_menus(vec![Menu {
                name: "Recovery Water Diagnostic".into(),
                items: vec![MenuItem::action("退出诊断", Quit)],
            }]);
            let bounds = Bounds::centered(None, size(px(1160.), px(848.)), cx);
            let window = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(bounds)),
                        titlebar: Some(TitlebarOptions {
                            title: Some("Matou Recovery Water — 独立诊断（无数据）".into()),
                            ..Default::default()
                        }),
                        app_id: Some("com.matou.native-parity.recovery-water-diagnostic".into()),
                        ..Default::default()
                    },
                    |_, cx| cx.new(Probe::new),
                )
                .expect("Open independent diagnostic");
            cx.on_window_closed(|cx| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            cx.activate(true);
            if !std::env::args().any(|arg| arg == "--manual-only") {
                cx.spawn(async move |cx| {
                    let start = Instant::now();
                    for (step, at_ms) in [
                        (1, 8500),
                        (2, 10500),
                        (3, 11800),
                        (4, 13500),
                        (5, 15000),
                        (6, 16200),
                        (7, 18000),
                        (8, 19500),
                        (9, 20800),
                        (10, 22000),
                        (11, 24000),
                    ] {
                        cx.background_executor()
                            .timer(Duration::from_millis(at_ms).saturating_sub(start.elapsed()))
                            .await;
                        let _ = window.update(cx, |this, _, cx| this.phase(step, cx));
                        if step == 11 {
                            let _ = cx.update(|cx| export(cx, "automatic"));
                        }
                    }
                })
                .detach();
            }
            cx.spawn(async move |cx| {
                let mut previous = Instant::now();
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(16))
                        .await;
                    let now = Instant::now();
                    let interval = now.duration_since(previous).as_secs_f64() * 1000.;
                    previous = now;
                    if cx
                        .update(|_| {
                            record(serde_json::json!({"event":"heartbeat","interval_ms":interval}))
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .detach();
            record(
                serde_json::json!({"event":"launched","pid":std::process::id(),"isolated":true}),
            );
        });
    }
}
#[cfg(target_os = "macos")]
fn main() {
    mac::run();
}
#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("Recovery water view diagnostic is Mac-only; no Windows renderer accepted");
}
