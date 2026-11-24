#[cfg(target_os = "macos")]
mod probe {
    // Use the production element's shape, measured layout, prepaint, and paint paths.
    include!("../src/ui_text.rs");

    const BODY: &str =
        "移出 \"项目甲\" 会关闭该空间下的事项和终端会话，本地文件保持原样。 是否继续？";

    struct Probe;

    impl Render for Probe {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            assert_eq!(
                window.scale_factor(),
                2.,
                "reference geometry was captured at DPR 2"
            );
            window.with_text_style(
                Some(TextStyleRefinement {
                    font_family: Some(".AppleSystemUIFont".into()),
                    font_size: Some(px(14.).into()),
                    line_height: Some(relative(1.6)),
                    ..Default::default()
                }),
                |window| {
                    verify_body(window, cx);
                    verify_whitespace(window, cx);
                },
            );
            div()
                .w(px(372.))
                .font_family(".AppleSystemUIFont")
                .text_size(px(14.))
                .line_height(relative(1.6))
                .child(ui_text(BODY).pre_line())
        }
    }

    fn visual_text<'a>(line: &ParagraphLine<'a>) -> &'a str {
        let runs = &line.source.unwrapped_layout.runs;
        let boundary_index = |edge: WrapBoundary| runs[edge.run_ix].glyphs[edge.glyph_ix].index;
        let start = line.start.map_or(0, boundary_index);
        let end = line.end.map_or(line.source.text.len(), boundary_index);
        &line.source.text[start..end]
    }

    fn verify_body(window: &mut Window, cx: &mut App) {
        let mut label = ui_text(BODY)
            .pre_line()
            .render(window, cx)
            .into_any_element();
        let element = label.downcast_mut::<PreLineText>().unwrap();
        let (node, mut state) = element.request_layout(None, None, window, cx);
        window.compute_layout(
            node,
            size(
                AvailableSpace::Definite(px(372.)),
                AvailableSpace::MaxContent,
            ),
            cx,
        );
        let mut bounds = window.layout_bounds(node);
        assert_eq!(bounds.size.width, px(372.));
        let layout = element.prepaint(None, None, bounds, &mut state, window, cx);
        assert_eq!(layout.metrics.height, px(22.3984375));
        assert_eq!(layout.metrics.baseline_from_top, px(32.));
        assert_eq!(layout.size().height, px(44.796875));

        bounds.origin.x = px(414.);
        for top in [355.6015625, 355.5] {
            bounds.origin.y = px(top);
            let lines: Vec<_> = layout.visual_lines(bounds, TextAlign::Left).collect();
            let text: Vec<_> = lines.iter().map(visual_text).collect();
            assert_eq!(
                text,
                [
                    "移出 \"项目甲\" 会关闭该空间下的事项和终端会话，本地文件",
                    "保持原样。 是否继续？",
                ],
            );
            let baselines: Vec<_> = lines.iter().map(|line| line.origin.y * 2.).collect();
            assert_eq!(baselines, [px(743.), px(788.)]);
            assert!(lines.iter().all(|line| line.origin.x == px(414.)));
            println!("Body top={top}: {text:?}, device baselines={baselines:?}");
        }
    }

    fn verify_whitespace(window: &mut Window, cx: &mut App) {
        let input = "\t甲   乙\u{000c}\t \r\n \t\r  丙\u{00a0}  丁 \t\n";
        let mut label = ui_text(input)
            .pre_line()
            .render(window, cx)
            .into_any_element();
        let element = label.downcast_mut::<PreLineText>().unwrap();
        let (node, mut state) = element.request_layout(None, None, window, cx);
        window.compute_layout(
            node,
            size(
                AvailableSpace::Definite(px(372.)),
                AvailableSpace::MaxContent,
            ),
            cx,
        );
        let bounds = window.layout_bounds(node);
        let layout = element.prepaint(None, None, bounds, &mut state, window, cx);
        let lines: Vec<_> = layout.visual_lines(bounds, TextAlign::Left).collect();
        let text: Vec<_> = lines.iter().map(visual_text).collect();
        assert_eq!(text, ["甲 乙", "", "丙\u{00a0} 丁"]);
        assert_eq!(layout.size().height, px(67.1953125));
        println!(
            "Pre-line whitespace: {text:?}, height={:?}",
            layout.size().height
        );
    }

    pub fn run() {
        Application::new().run(|cx| {
            cx.open_window(
                WindowOptions {
                    show: false,
                    focus: false,
                    ..Default::default()
                },
                |_, cx| cx.new(|_| Probe),
            )
            .expect("paragraph probe window");
            cx.quit();
        });
    }
}

fn main() {
    #[cfg(target_os = "macos")]
    probe::run();
}
