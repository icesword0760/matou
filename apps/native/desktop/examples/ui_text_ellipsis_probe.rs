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
        let layout = label.downcast_mut::<StyledText>().unwrap().layout().clone();
        label.layout_as_root(available, window, cx);
        label.prepaint(window, cx);

        let stock = StyledText::new(title);
        let stock_layout = stock.layout().clone();
        let mut stock = stock.into_any();
        stock.layout_as_root(available, window, cx);
        stock.prepaint(window, cx);
        assert!(layout.text().ends_with('…'));
        assert_eq!(layout.text(), stock_layout.text());
        println!("Latin: {}", layout.text());
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
        let fit = element.0.fit(Some(exact), window).unwrap();
        assert_eq!(fit.text.as_ref(), expected);
        assert_eq!(fit.unwrapped_layout.width, exact);
        assert!(next_width > exact);
        assert_eq!(element.0.fit(None, window).unwrap().text.as_ref(), title);
        assert_eq!(
            element
                .0
                .fit(Some(suffix_width), window)
                .unwrap()
                .text
                .as_ref(),
            "…"
        );
        assert!(element.0.fit(Some(px(0.)), window).is_none());

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
            .unwrap();
        assert_eq!(painted.text.as_ref(), expected);
        assert!(painted.unwrapped_layout.width <= bounds.size.width);
        assert!(next_width > bounds.size.width);
        assert_eq!(bounds.size.height, px(20.));
        println!("{title:?} -> {expected:?} fits {exact:?}, next candidate {next_width:?}");
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
