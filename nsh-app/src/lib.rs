//! Standalone NSH Windows reader fallback. The GPUI host is kept separate
//! until Liber's GPUI Kit fork can be migrated to the GPUI version NSH uses.
use hayro::hayro_interpret::util::TransformExt;
use hayro::hayro_interpret::{Context, InterpreterCache, InterpreterSettings, interpret_page};
use hayro::hayro_syntax::Pdf;
use hayro::kurbo::Rect;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{PixmapSettings, RenderCache, RenderSettings};
use nsh_base::{button, safe_area_view, theme};
use nsh_core::ui::prelude::*;

#[path = "../../src/pdf_text.rs"]
mod pdf_text;

fn page_data(path: &str, index: usize) -> Result<(usize, String, String), String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let pdf = Pdf::new(bytes).map_err(|e| format!("Invalid PDF: {e:?}"))?;
    let page = pdf
        .pages()
        .get(index)
        .ok_or("Page number is out of range")?;
    if page.raw().contains_key("Contents") && page.page_stream().is_none() {
        return Err("Page content could not be parsed".into());
    }
    let (width, height) = page.render_dimensions();
    let cache = RenderCache::new();
    let pixmap = hayro::render(
        page,
        &cache,
        &InterpreterSettings::default(),
        &RenderSettings::default(),
        &PixmapSettings {
            x_scale: 1.0,
            y_scale: 1.0,
            bg_color: WHITE,
        },
    );
    let image = image::RgbaImage::from_raw(
        u32::from(pixmap.width()),
        u32::from(pixmap.height()),
        pixmap.data_as_u8_slice().to_vec(),
    )
    .ok_or("Invalid renderer buffer")?;
    let image_path = std::env::temp_dir().join(format!("liber-nsh-page-{index}.png"));
    image.save(&image_path).map_err(|e| e.to_string())?;

    let cache = InterpreterCache::new();
    let mut context = Context::new(
        page.initial_transform(true).to_kurbo(),
        Rect::new(0.0, 0.0, f64::from(width), f64::from(height)),
        &cache,
        page.xref(),
        InterpreterSettings::default(),
    );
    let mut device = pdf_text::TextDevice::default();
    interpret_page(page, &mut context, &mut device);
    let text = pdf_text::to_lines(&device.glyphs);
    Ok((
        pdf.pages().len(),
        image_path.to_string_lossy().into_owned(),
        text,
    ))
}

#[native_module("liber/native")]
fn native(registry: &mut Registry) {
    registry.register::<App>();
}

#[component]
pub fn App() -> impl IntoElement {
    let path = signal(String::new());
    let page = signal(0usize);
    let count = signal(0usize);
    let image_path = signal(String::new());
    let extracted = signal(String::from("Enter a PDF path and press Open."));
    let error = signal(String::new());

    let load = std::rc::Rc::new(move |at: usize| {
        let source = path.get();
        match page_data(&source, at) {
            Ok((pages, image, text)) => {
                count.set(pages);
                page.set(at);
                image_path.set(image);
                extracted.set(if text.is_empty() {
                    "No text on this page.".into()
                } else {
                    text
                });
                error.set(String::new());
            }
            Err(message) => error.set(message),
        }
    });
    let load_open = load.clone();
    let load_previous = load.clone();
    let load_next = load.clone();

    view()
        .flex_1()
        .bg(move || theme().background)
        .text_color(move || theme().foreground)
        .child(
            safe_area_view().flex_1().child(
                view()
                    .flex_1()
                    .gap_3()
                    .p(px(16.0))
                    .child(
                        input()
                            .value(move || path.get())
                            .placeholder("C:\\path\\to\\book.pdf")
                            .on_change(move |value: &str| path.set(value.to_owned())),
                    )
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(
                                button()
                                    .on_press(move || load_open(0))
                                    .child(text("Open PDF")),
                            )
                            .child(
                                button()
                                    .on_press(move || {
                                        let at = page.get();
                                        if at > 0 {
                                            load_previous(at - 1);
                                        }
                                    })
                                    .child(text("Previous")),
                            )
                            .child(
                                button()
                                    .on_press(move || {
                                        let at = page.get();
                                        if at + 1 < count.get() {
                                            load_next(at + 1);
                                        }
                                    })
                                    .child(text("Next")),
                            )
                            .child(text(move || {
                                if count.get() == 0 {
                                    "No document".into()
                                } else {
                                    format!("Page {} / {}", page.get() + 1, count.get())
                                }
                            })),
                    )
                    .child(text(move || error.get()).text_color(move || theme().destructive))
                    .child(
                        scroll().flex_1().child(
                            view()
                                .gap_4()
                                .child(dynamic(move || {
                                    let src = image_path.get();
                                    (!src.is_empty())
                                        .then(|| image(src).w(px(680.0)).max_w(px(900.0)))
                                }))
                                .child(
                                    text(move || extracted.get())
                                        .text_size(px(16.0))
                                        .px(px(12.0))
                                        .py(px(12.0)),
                                ),
                        ),
                    ),
            ),
        )
}
