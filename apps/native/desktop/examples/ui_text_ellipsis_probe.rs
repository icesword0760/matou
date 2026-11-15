#[cfg(target_os = "macos")]
mod probe {
    // Exercise the private element while keeping its internals out of the desktop API.
    include!("../src/ui_text.rs");

    struct Probe;

    impl Render for Probe {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            window.with_text_style(
                Some(TextStyleRefinement {
                    font_family: Some(".AppleSystemUIFont".into()),
                    font_size: Some(px(13.).into()),
                    line_height: Some(px(20.).into()),
                    white_space: Some(WhiteSpace::Nowrap),
                    text_overflow: Some(TextOverflow::Truncate("…".into())),
                    ..Default::default()
                }),
                |window| {
                    verify_latin(window, cx);
                    verify_truncated_normal_line(window, cx);
                    for (title, expected, next) in [
                        ("Dev项目任务资料", "Dev项目…", "Dev项目任…"),
                        ("ci👩‍💻release branch", "ci👩‍💻…", "ci👩‍💻r…"),
                        ("e\u{301}diting branch", "e\u{301}…", "e\u{301}d…"),
                    ] {
                        verify_mixed(title, expected, next, window, cx);
                    }
                    // Medium has a corrected native face on Mac, including ASCII
                    // titles; the measured element must use that face's advances.
                    window.with_text_style(
                        Some(TextStyleRefinement {
                            font_weight: Some(FontWeight::MEDIUM),
                            ..Default::default()
                        }),
                        |window| verify_mixed("workspace release", "work…", "works…", window, cx),
                    );
                },
            );
            verify_baselines(window, cx);
            div()
        }
    }

    fn verify_latin(window: &mut Window, cx: &mut App) {
        let title = "workspace release branch";
        let available = size(
            AvailableSpace::Definite(px(42.)),
            AvailableSpace::MaxContent,
        );
        let mut label = ui_text(title).render(window, cx).into_any_element();
        let element = label.downcast_mut::<SingleLineText>().unwrap();
        let (node, mut layout) = element.request_layout(None, None, window, cx);
        window.compute_layout(node, available, cx);
        let bounds = window.layout_bounds(node);
        let line = element
            .prepaint(None, None, bounds, &mut layout, window, cx)
            .line
            .unwrap();

        let stock = StyledText::new(title);
        let stock_layout = stock.layout().clone();
        let mut stock = stock.into_any();
        stock.layout_as_root(available, window, cx);
        stock.prepaint(window, cx);
        assert_eq!(line.text.as_ref(), "work…");
        assert_eq!(line.text.as_ref(), stock_layout.text());
        println!("Latin: {}", line.text);
    }

    fn verify_truncated_normal_line(window: &mut Window, cx: &mut App) {
        let title = "workspace release branch 中文";
        let mut rendered = ui_text(title)
            .normal_line_height()
            .render(window, cx)
            .into_any_element();
        let element = rendered.downcast_mut::<SingleLineText>().unwrap();
        let (node, mut layout) = element.request_layout(None, None, window, cx);
        window.compute_layout(
            node,
            size(
                AvailableSpace::Definite(px(42.)),
                AvailableSpace::MaxContent,
            ),
            cx,
        );
        let bounds = window.layout_bounds(node);
        let measured = element.prepaint(None, None, bounds, &mut layout, window, cx);
        let line = measured.line.unwrap();
        assert_eq!(line.text.as_ref(), "work…");
        assert_eq!(bounds.size.height, px(18.5));
        assert_eq!(measured.metrics.height, px(18.5));
        assert_eq!(measured.metrics.baseline_from_top, px(28.));
        println!(
            "{title:?} -> {:?} retains normal height 18.5px and device baseline offset 28px",
            line.text
        );
    }

    fn verify_mixed(title: &str, expected: &str, next: &str, window: &mut Window, cx: &mut App) {
        let mut rendered = ui_text(title.to_owned())
            .render(window, cx)
            .into_any_element();
        let element = rendered.downcast_mut::<SingleLineText>().unwrap();
        let shape_width = |text: &str| {
            element
                .0
                .shape(
                    text.to_owned().into(),
                    &element.0.fonts.runs(text, window),
                    window,
                )
                .unwrap()
                .unwrapped_layout
                .width
        };
        let exact = shape_width(expected);
        let next_width = shape_width(next);
        let suffix_width = shape_width("…");
        let fit = element.0.measure(Some(exact), window).line.unwrap();
        assert_eq!(fit.text.as_ref(), expected);
        assert_eq!(fit.unwrapped_layout.width, exact);
        assert!(next_width > exact);
        assert_eq!(
            element.0.measure(None, window).line.unwrap().text.as_ref(),
            title
        );
        assert_eq!(
            element
                .0
                .measure(Some(suffix_width), window)
                .line
                .unwrap()
                .text
                .as_ref(),
            "…"
        );
        assert!(element.0.measure(Some(px(0.)), window).line.is_none());

        // Use the element's real measured node and final bounds, including GPUI's rounding.
        let (node, mut layout) = element.request_layout(None, None, window, cx);
        window.compute_layout(
            node,
            size(
                AvailableSpace::Definite(exact.ceil()),
                AvailableSpace::MaxContent,
            ),
            cx,
        );
        let bounds = window.layout_bounds(node);
        let painted = element
            .prepaint(None, None, bounds, &mut layout, window, cx)
            .line
            .unwrap();
        assert_eq!(painted.text.as_ref(), expected);
        assert!(painted.unwrapped_layout.width <= bounds.size.width);
        assert!(next_width > bounds.size.width);
        assert_eq!(bounds.size.height, px(20.));
        println!("{title:?} -> {expected:?} fits {exact:?}, next candidate {next_width:?}");
    }

    fn verify_baselines(window: &mut Window, cx: &mut App) {
        assert_eq!(
            window.scale_factor(),
            2.,
            "reference geometry was captured at DPR 2"
        );
        // DOM origins and drawTextBlob baselines captured in the reference.
        // Also check the corresponding device-rounded origins for CJK and task.
        for (title, weight, normal, height, origins, baseline) in [
            (
                "workspace",
                FontWeight::SEMIBOLD,
                true,
                15.,
                &[66.5][..],
                158.,
            ),
            (
                "项目甲",
                FontWeight::SEMIBOLD,
                true,
                18.5,
                &[106.75, 107.][..],
                242.,
            ),
            (
                "项目资料与交互验收长标题测试甲乙丙丁",
                FontWeight::MEDIUM,
                false,
                19.5,
                &[142.25, 142.5][..],
                314.,
            ),
        ] {
            window.with_text_style(
                Some(TextStyleRefinement {
                    font_family: Some(".AppleSystemUIFont".into()),
                    font_size: Some(px(13.).into()),
                    font_weight: Some(weight),
                    line_height: Some(px(if normal { 20. } else { height }).into()),
                    white_space: Some(WhiteSpace::Nowrap),
                    text_overflow: (!normal).then(|| TextOverflow::Truncate("…".into())),
                    ..Default::default()
                }),
                |window| {
                    let mut rendered = ui_text(title)
                        .when(normal, UiText::normal_line_height)
                        .render(window, cx)
                        .into_any_element();
                    let element = rendered.downcast_mut::<SingleLineText>().unwrap();
                    if normal {
                        assert_eq!(element.0.measure(Some(px(1.)), window).line.unwrap().text.as_ref(), title);
                    }
                    let (node, mut layout) = element.request_layout(None, None, window, cx);
                    window.compute_layout(
                        node,
                        size(AvailableSpace::Definite(px(170.)), AvailableSpace::MaxContent),
                        cx,
                    );
                    let mut bounds = window.layout_bounds(node);
                    assert_eq!(bounds.size.height, px(height));
                    for &origin_y in origins {
                        bounds.origin.y = px(origin_y);
                        let measured = element.prepaint(None, None, bounds, &mut layout, window, cx);
                        let metrics = measured.metrics;
                        let line = measured.line.unwrap();
                        let origin = metrics.text_origin(bounds.origin, &line);
                        let layout = &line.unwrapped_layout;
                        let painted_baseline =
                            origin.y + ((metrics.height - layout.ascent - layout.descent) / 2. + layout.ascent);
                        assert_eq!(painted_baseline * window.scale_factor(), px(baseline));
                        println!("{title:?}: origin y={origin_y}, height={height}, device baseline={baseline}");
                    }
                },
            );
        }
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
            .expect("font probe window");
            cx.quit();
        });
    }
}

fn main() {
    #[cfg(target_os = "macos")]
    probe::run();
}
