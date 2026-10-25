use gpui::{
    App, Bounds, Entity, Global, KeyBinding, Menu, MenuItem, Render, SharedString, Window,
    WindowBounds, WindowOptions, actions, div, prelude::*, px, rgb, size,
};

actions!(
    gpui_native_split_spike,
    [Quit, ToggleAppearance, ZoomIn, ZoomOut]
);

struct SpikeView {
    selected: SharedString,
    dark: bool,
    font_size: f32,
}

struct SpikeViewEntity(Entity<SpikeView>);

impl Global for SpikeViewEntity {}

impl SpikeView {
    fn toggle_appearance(&mut self) {
        self.dark = !self.dark;
        #[cfg(target_os = "macos")]
        NATIVE_SPLIT.with(|split| {
            if let Some(split) = split.borrow().as_ref() {
                split.set_dark_appearance(self.dark);
            }
        });
    }
}

impl Render for SpikeView {
    fn render(&mut self, _window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let (background, foreground, muted) = if self.dark {
            (0x171719, 0xf1efe8, 0xb5b2aa)
        } else {
            (0xf5f3ec, 0x282724, 0x716e66)
        };
        let selected = self.selected.clone();
        let font_size = self.font_size;

        div()
            .id("gpui-native-split-content")
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(background))
            .text_color(rgb(foreground))
            .child(
                div()
                    .flex()
                    .w_full()
                    .items_center()
                    .gap(px(16.0))
                    .px(px(32.0))
                    .pt(px(44.0))
                    .pb(px(22.0))
                    .border_b_1()
                    .border_color(rgb(if self.dark { 0x35353a } else { 0xd8d5cc }))
                    .child(
                        div()
                            .text_size(px(25.0))
                            .font_weight(gpui::FontWeight::BOLD)
                            .child(format!("GPUI content · {selected}")),
                    )
                    .child(
                        div()
                            .id("toggle-theme")
                            .cursor_pointer()
                            .rounded(px(8.0))
                            .border_1()
                            .border_color(rgb(if self.dark { 0x55555b } else { 0xc8c4ba }))
                            .px(px(12.0))
                            .py(px(7.0))
                            .text_size(px(13.0))
                            .child(if self.dark { "Light appearance" } else { "Dark appearance" })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.toggle_appearance();
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .id("gpui-scroll-area")
                    .flex_1()
                    .overflow_y_scroll()
                    .px(px(32.0))
                    .py(px(24.0))
                    .child(
                        div()
                            .max_w(px(860.0))
                            .text_size(px(font_size))
                            .line_height(px(font_size * 1.65))
                            .child(format!("Current GPUI view: {selected}"))
                            .child(
                                div()
                                    .mt(px(18.0))
                                    .text_size(px(14.0))
                                    .text_color(rgb(muted))
                                    .child("Resize this window, scroll this list, select a sidebar row, and toggle the AppKit sidebar from the unified toolbar."),
                            )
                            .children((1..=90).map(|index| {
                                div()
                                    .mt(px(14.0))
                                    .rounded(px(8.0))
                                    .bg(rgb(if self.dark { 0x222226 } else { 0xffffff }))
                                    .px(px(16.0))
                                    .py(px(13.0))
                                    .child(format!("Scrollable GPUI placeholder · {index:02}"))
                            })),
                    ),
            )
    }
}

#[cfg(target_os = "macos")]
thread_local! {
    static NATIVE_SPLIT: std::cell::RefCell<Option<imago_gpui_native_split::NativeSplit>> = const { std::cell::RefCell::new(None) };
}

fn main() {
    gpui::application().run(|cx: &mut App| {
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.on_action(|_: &ZoomIn, cx| {
            let entity = cx.global::<SpikeViewEntity>().0.clone();
            let _ = entity.update(cx, |view, cx| {
                view.font_size = (view.font_size + 2.0).min(32.0);
                cx.notify();
            });
        });
        cx.on_action(|_: &ZoomOut, cx| {
            let entity = cx.global::<SpikeViewEntity>().0.clone();
            let _ = entity.update(cx, |view, cx| {
                view.font_size = (view.font_size - 2.0).max(12.0);
                cx.notify();
            });
        });
        cx.on_action(|_: &ToggleAppearance, cx| {
            let entity = cx.global::<SpikeViewEntity>().0.clone();
            let _ = entity.update(cx, |view, cx| {
                view.toggle_appearance();
                cx.notify();
            });
        });
        cx.bind_keys([
            KeyBinding::new("cmd-+", ZoomIn, None),
            KeyBinding::new("cmd--", ZoomOut, None),
            KeyBinding::new("cmd-t", ToggleAppearance, None),
            KeyBinding::new("cmd-q", Quit, None),
        ]);
        cx.set_menus([Menu::new("Liber GPUI Split Spike").items([
            MenuItem::action("Zoom In", ZoomIn),
            MenuItem::action("Zoom Out", ZoomOut),
            MenuItem::action("Toggle Appearance", ToggleAppearance),
            MenuItem::separator(),
            MenuItem::action("Quit", Quit),
        ])]);

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(1180.0), px(780.0)),
                    cx,
                ))),
                ..Default::default()
            },
            |_window, cx| {
                let view = cx.new(|_| SpikeView {
                    selected: "Overview".into(),
                    dark: true,
                    font_size: 17.0,
                });
                cx.set_global(SpikeViewEntity(view.clone()));

                #[cfg(target_os = "macos")]
                {
                    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
                    let handle = _window.window_handle().expect("GPUI window handle");
                    if let RawWindowHandle::AppKit(handle) = handle.as_raw() {
                        let view_for_sidebar = view.clone();
                        let async_cx = cx.to_async();
                        // SAFETY: this callback runs while GPUI is creating the window on the AppKit main thread.
                        let split = unsafe {
                            imago_gpui_native_split::install(
                                handle,
                                vec![
                                    row("Overview", "Overview", "square.grid.2x2"),
                                    row("Bookmarks", "Bookmarks", "bookmark"),
                                    row("Notes", "Notes", "note.text"),
                                    row("Highlights", "Highlights", "highlighter"),
                                ],
                                move |id| {
                                    let _ = async_cx.update(|cx| {
                                        view_for_sidebar.update(cx, |view, cx| {
                                            view.selected = id.into();
                                            cx.notify();
                                        });
                                    });
                                },
                            )
                        };
                        split.set_dark_appearance(true);
                        NATIVE_SPLIT.with(|slot| *slot.borrow_mut() = Some(split));
                        _window.set_background_appearance(
                            gpui::WindowBackgroundAppearance::Transparent,
                        );
                    }
                }

                view
            },
        )
        .expect("open GPUI native split spike window");
        cx.activate(true);
    });
}

#[cfg(target_os = "macos")]
fn row(id: &'static str, label: &str, symbol: &'static str) -> imago_native_sidebar::Row {
    imago_native_sidebar::Row {
        id,
        label: label.into(),
        symbol: Some(symbol),
    }
}
