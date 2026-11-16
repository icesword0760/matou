//! Check the macOS fallback style through the GPUI font, shape, and paint APIs.
#[cfg(target_os = "macos")]
mod probe {
    use gpui::{prelude::*, *};
    use matou_native_desktop::assets::NativeAssets;

    const LABEL: &str = "下方还有更新记录";

    fn run_style(font: Font, text: &str) -> TextRun {
        TextRun {
            len: text.len(),
            font,
            color: rgb(0x242b37).into(),
            background_color: None,
            underline: None,
            strikethrough: None,
        }
    }

    fn shape(
        window: &Window,
        text: &'static str,
        runs: &[TextRun],
        font_size: Pixels,
    ) -> WrappedLine {
        // Matou's UI labels use shape_text to preserve adjacent font runs.
        window
            .text_system()
            .shape_text(text.into(), font_size, runs, None, None)
            .expect("shape the probe line")
            .into_iter()
            .next()
            .expect("one line")
    }

    fn metrics(line: &WrappedLine) -> (Pixels, Pixels, Pixels) {
        let layout = &line.unwrapped_layout;
        (layout.width, layout.ascent, layout.descent)
    }

    struct Probe;

    impl Render for Probe {
        fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let text_system = window.text_system();
            let pingfang = font("PingFang SC");
            let italic = pingfang.clone().italic();
            let normal_id = text_system.resolve_font(&pingfang);
            let alias_id = text_system.resolve_font(&italic);
            assert_ne!(alias_id, normal_id);
            assert_eq!(text_system.resolve_font(&italic), alias_id);
            assert_eq!(
                text_system.resolve_font(&Font {
                    style: FontStyle::Oblique,
                    ..pingfang.clone()
                }),
                alias_id,
                "oblique requests that match the same upright face reuse its alias"
            );

            let font_size = px(18.);
            let ordinary_line = shape(
                window,
                LABEL,
                &[run_style(pingfang.clone(), LABEL)],
                font_size,
            );
            let italic_line = shape(
                window,
                LABEL,
                &[run_style(italic.clone(), LABEL)],
                font_size,
            );
            assert_eq!(italic_line.unwrapped_layout.runs.len(), 1);
            assert_eq!(italic_line.unwrapped_layout.runs[0].font_id, alias_id);
            assert_eq!(ordinary_line.unwrapped_layout.runs[0].font_id, normal_id);
            assert_eq!(metrics(&italic_line), metrics(&ordinary_line));

            let mixed = shape(
                window,
                LABEL,
                &[
                    run_style(pingfang.clone(), "下方"),
                    run_style(italic.clone(), "还有更新"),
                    run_style(pingfang.clone(), "记录"),
                ],
                font_size,
            );
            assert_eq!(
                mixed
                    .unwrapped_layout
                    .runs
                    .iter()
                    .map(|run| run.font_id)
                    .collect::<Vec<_>>(),
                [normal_id, alias_id, normal_id]
            );
            assert_eq!(
                mixed
                    .unwrapped_layout
                    .runs
                    .iter()
                    .flat_map(|run| run.glyphs.iter().map(|glyph| glyph.index))
                    .collect::<Vec<_>>(),
                LABEL
                    .char_indices()
                    .map(|(index, _)| index)
                    .collect::<Vec<_>>()
            );
            assert_eq!(metrics(&mixed), metrics(&ordinary_line));
            let emoji_italic = shape(window, "下🙂方", &[run_style(italic, "下🙂方")], font_size);
            let emoji_upright = shape(
                window,
                "下🙂方",
                &[run_style(pingfang, "下🙂方")],
                font_size,
            );
            let emoji_face = |line: &WrappedLine| {
                line.unwrapped_layout
                    .runs
                    .iter()
                    .find(|run| run.glyphs.iter().any(|glyph| glyph.is_emoji))
                    .map(|run| run.font_id)
                    .unwrap()
            };
            assert_eq!(emoji_face(&emoji_italic), emoji_face(&emoji_upright));
            assert_ne!(emoji_face(&emoji_italic), alias_id);
            println!(
                "PingFang SC: upright {normal_id:?}, reusable alias {alias_id:?}; mixed glyph IDs/UTF-8 indices and width/ascent/descent pass {:?}; emoji fallback unchanged.",
                metrics(&mixed)
            );

            let menlo = font("Menlo");
            let menlo_italic = menlo.clone().italic();
            let menlo_italic_id = text_system.resolve_font(&menlo_italic);
            let sample = "Menlo italic";
            let menlo_line = shape(
                window,
                sample,
                &[run_style(menlo_italic, sample)],
                font_size,
            );
            assert_ne!(text_system.resolve_font(&menlo), menlo_italic_id);
            assert_eq!(menlo_line.unwrapped_layout.runs[0].font_id, menlo_italic_id);
            assert_eq!(
                menlo_line.width(),
                shape(window, sample, &[run_style(menlo, sample)], font_size).width()
            );
            println!("Menlo resolves a distinct italic face with the existing advance.");

            div().size_full().child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, cx| {
                        for (y, line) in [(16., &ordinary_line), (56., &italic_line), (96., &mixed)] {
                            line.paint(
                                bounds.origin + point(px(16.25), px(y)),
                                px(30.),
                                TextAlign::Left,
                                None,
                                window,
                                cx,
                            )
                            .expect("rasterize and paint the shaped PingFang glyphs");
                        }
                        println!("Upright, synthetic italic and mixed PingFang lines rasterized in the hidden window.");
                    },
                )
                .size_full(),
            )
        }
    }

    pub fn run() {
        Application::new().with_assets(NativeAssets).run(|cx| {
            cx.open_window(
                WindowOptions {
                    show: false,
                    focus: false,
                    ..Default::default()
                },
                |_, cx| cx.new(|_| Probe),
            )
            .expect("open hidden synthetic italic probe");
            cx.quit();
        });
    }
}

fn main() {
    #[cfg(target_os = "macos")]
    probe::run();
}
