// Liber: local-first library and PDF reader.
mod app;
mod appearance;
mod ui;
mod assets;
mod flow;
mod library;
mod meta;
mod mac;
mod notes;
mod pdf;
mod reader;
mod raster;
mod reading_state;
mod syntax;

use gpui::{px, size, App, AppContext, Bounds, WindowBounds, WindowOptions};

fn main() {
    gpui::application().with_assets(assets::Assets).run(|cx: &mut App| {
        gpui_component::init(cx);
        cx.text_system()
            .add_fonts(assets::font_bytes())
            .expect("failed to load embedded fonts");
        app::install_menu(cx);
        appearance::apply(&appearance::load(&library::default_root()), cx);

        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(1280.), px(840.)),
                cx,
            ))),
            titlebar: Some(gpui::TitlebarOptions {
                title: None,
                appears_transparent: true,
                // Position once AppKit has created the buttons: their actual
                // height determines the inset into our shared titlebar grid.
                traffic_light_position: None,
            }),
            ..Default::default()
        };
        cx.open_window(options, |window, cx| {
            mac::align_traffic_lights(window);
            let liber = cx.new(|cx| app::Liber::new(window, cx));
            cx.new(|cx| gpui_component::Root::new(liber, window, cx))
        })
        .expect("open window");
        cx.activate(true);
    });
}
