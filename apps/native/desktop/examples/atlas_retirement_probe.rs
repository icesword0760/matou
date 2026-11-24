//! Automatic real Window/Metal contract. No user input, storage or PTY.
#[cfg(target_os = "macos")]
mod mac {
    use gpui::{prelude::*, *};
    use std::{cell::RefCell, rc::Rc, sync::Arc, time::Instant};

    const PENDING: u8 = 255;

    struct Progress {
        exit: u8,
        phase: u8,
        painted: u8,
        teardown_painted: bool,
        teardown_completed: bool,
        started: Instant,
    }
    type SharedProgress = Rc<RefCell<Progress>>;

    fn finish(progress: &SharedProgress, exit: u8, category: &str, detail: &str, cx: &mut App) {
        let mut state = progress.borrow_mut();
        if state.exit != PENDING {
            return;
        }
        state.exit = exit;
        if exit == 0 {
            eprintln!(
                "ATLAS_PROBE COMPLETE phases=7 painted_mask=0x7f teardown_painted=true teardown_completed=true legal_retire=true normal_quit_requested=true"
            );
        } else {
            eprintln!(
                "ATLAS_PROBE result=FAIL category={category} exit={exit} phase={} painted_mask={:#04x} elapsed_ms={:.3} detail={detail}",
                state.phase,
                state.painted,
                state.started.elapsed().as_secs_f64() * 1000.,
            );
        }
        drop(state);
        if exit != 0 {
            // AppKit terminate normally exits with zero without returning to Rust.
            // Diagnostic failures must preserve their explicit nonzero exit code.
            std::process::exit(i32::from(exit));
        }
        cx.quit();
    }

    struct Probe {
        images: Vec<Arc<RenderImage>>,
        phase: u8,
        frame_queued: bool,
        progress: SharedProgress,
    }

    impl Probe {
        fn after_paint(&mut self, phase: u8, window: &mut Window, cx: &mut Context<Self>) {
            if self.progress.borrow().exit != PENDING {
                return;
            }
            if self.phase != phase || !self.frame_queued {
                finish(
                    &self.progress,
                    2,
                    "fixture_order",
                    "unexpected frame event",
                    cx,
                );
                return;
            }
            if phase == 7 {
                eprintln!("ATLAS_PROBE teardown_after_frame");
            } else {
                eprintln!("ATLAS_PROBE after_frame phase={phase}");
            }
            if phase == 6 {
                eprintln!(
                    "ATLAS_PROBE phases_complete painted_mask={:#04x}",
                    self.progress.borrow().painted
                );
                self.phase = 7;
                self.frame_queued = false;
                self.progress.borrow_mut().phase = 7;
                cx.notify();
                return;
            }
            if phase == 7 {
                if self.progress.borrow().painted != 0x7f
                    || !self.progress.borrow().teardown_painted
                {
                    finish(
                        &self.progress,
                        2,
                        "fixture_order",
                        "missing actual paint phase",
                        cx,
                    );
                    return;
                }
                for image in &self.images {
                    for _ in 0..2 {
                        if let Err(error) = window.drop_image(image.clone()) {
                            finish(
                                &self.progress,
                                3,
                                "render_api_error",
                                &error.to_string(),
                                cx,
                            );
                            return;
                        }
                    }
                }
                self.progress.borrow_mut().teardown_completed = true;
                finish(&self.progress, 0, "teardown_complete", "", cx);
                return;
            }
            let next = phase + 1;
            let indices: &[usize] = match next {
                1 | 3 => &[1],
                4 => &[2],
                5 => &[3],
                6 => &[0],
                _ => &[],
            };
            for index in indices {
                eprintln!("ATLAS_PROBE drop_twice phase={next} image={index}");
                for _ in 0..2 {
                    if let Err(error) = window.drop_image(self.images[*index].clone()) {
                        finish(
                            &self.progress,
                            3,
                            "render_api_error",
                            &error.to_string(),
                            cx,
                        );
                        return;
                    }
                }
            }
            self.phase = next;
            self.frame_queued = false;
            self.progress.borrow_mut().phase = next;
            cx.notify();
        }
    }

    impl Render for Probe {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let phase = self.phase;
            let indices: &[usize] = match phase {
                0 | 2 | 6 => &[0, 1],
                1 | 5 => &[0],
                3 => &[0, 2, 3],
                4 => &[0, 3],
                7 => &[], // Empty teardown scene retires all sprites before final drop.
                _ => unreachable!(),
            };
            let images = indices
                .iter()
                .map(|i| self.images[*i].clone())
                .collect::<Vec<_>>();
            let weak = cx.weak_entity();
            let progress = self.progress.clone();
            canvas(
                |_, _, _| (),
                move |bounds, _, window, cx| {
                    if progress.borrow().exit != PENDING {
                        return;
                    }
                    for (index, image) in images.iter().enumerate() {
                        if let Err(error) = window.paint_image(
                            Bounds::new(
                                bounds.origin + point(px(index as f32 * 24.), px(0.)),
                                size(px(16.), px(16.)),
                            ),
                            Corners::default(),
                            image.clone(),
                            0,
                            false,
                        ) {
                            finish(&progress, 3, "render_api_error", &error.to_string(), cx);
                            return;
                        }
                    }
                    if let Err(error) = weak.update(cx, |view, _| {
                        if view.frame_queued {
                            return;
                        }
                        view.frame_queued = true;
                        if phase == 7 {
                            view.progress.borrow_mut().teardown_painted = true;
                            eprintln!("ATLAS_PROBE teardown_paint image_count=0");
                        } else {
                            view.progress.borrow_mut().painted |= 1 << phase;
                            eprintln!(
                                "ATLAS_PROBE paint phase={phase} image_count={}",
                                images.len()
                            );
                        }
                        let weak = weak.clone();
                        let progress = progress.clone();
                        // GPUI submits the current scene after paint. Advance only
                        // on the subsequent frame event, never a wall-clock guess.
                        window.on_next_frame(move |window, cx| {
                            if let Err(error) =
                                weak.update(cx, |view, cx| view.after_paint(phase, window, cx))
                            {
                                finish(&progress, 2, "fixture_unavailable", &error.to_string(), cx);
                            }
                        });
                    }) {
                        finish(&progress, 2, "fixture_unavailable", &error.to_string(), cx);
                    }
                },
            )
            .size_full()
        }
    }

    pub fn run() -> u8 {
        let progress = Rc::new(RefCell::new(Progress {
            exit: PENDING,
            phase: 0,
            painted: 0,
            teardown_painted: false,
            teardown_completed: false,
            started: Instant::now(),
        }));
        let observed = progress.clone();
        Application::new().run(move |cx| {
            eprintln!("ATLAS_PROBE start deadline_owned_by_runner=true activation_once=true");
            let view_progress = progress.clone();
            let opened = cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(80.), px(80.)),
                        size(px(180.), px(96.)),
                    ))),
                    focus: false,
                    show: true,
                    titlebar: Some(TitlebarOptions {
                        title: Some("Matou Atlas Retirement — 自动诊断".into()),
                        ..Default::default()
                    }),
                    app_id: Some("com.matou.native-parity.atlas-retirement-probe".into()),
                    ..Default::default()
                },
                |_, cx| {
                    cx.new(|_| Probe {
                        images: (0..4)
                            .map(|i| {
                                Arc::new(RenderImage::new(vec![image::Frame::new(
                                    image::RgbaImage::from_pixel(
                                        16,
                                        16,
                                        image::Rgba([30 + i * 40, 80, 180 - i * 30, 255]),
                                    ),
                                )]))
                            })
                            .collect(),
                        phase: 0,
                        frame_queued: false,
                        progress: view_progress,
                    })
                },
            );
            let window = match opened {
                Ok(window) => window,
                Err(error) => {
                    finish(&progress, 2, "window_open_error", &error.to_string(), cx);
                    return;
                }
            };
            // Startup activation only: later user focus changes are never reversed.
            cx.activate(true);
            if let Err(error) = window.update(cx, |_, window, _| window.activate_window()) {
                finish(
                    &progress,
                    2,
                    "window_activation_error",
                    &error.to_string(),
                    cx,
                );
                return;
            }
        });
        let state = observed.borrow();
        if state.exit == PENDING {
            eprintln!(
                "ATLAS_PROBE result=FAIL category=incomplete_application_exit phase={} painted_mask={:#04x}",
                state.phase, state.painted
            );
            2
        } else {
            state.exit
        }
    }
}

#[cfg(target_os = "macos")]
fn main() -> std::process::ExitCode {
    std::process::ExitCode::from(mac::run())
}
#[cfg(not(target_os = "macos"))]
fn main() -> std::process::ExitCode {
    eprintln!("ATLAS_PROBE result=UNSUPPORTED category=platform Metal contract is Mac-only");
    std::process::ExitCode::from(2)
}
