//! Liber desktop app. The native UI and PDF reader share one application root.
use hayro::hayro_interpret::util::TransformExt;
use hayro::hayro_interpret::{Context, InterpreterCache, InterpreterSettings, interpret_page};
use hayro::hayro_syntax::Pdf;
use hayro::kurbo::Rect;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{PixmapSettings, RenderCache, RenderSettings};
use nsh_base::{button, safe_area_view};
use nsh_core::color::rgb;
use nsh_core::ui::prelude::*;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[path = "pdf_nav.rs"]
mod pdf_nav;
#[path = "pdf_text.rs"]
mod pdf_text;

const LIBER_BG: u32 = 0xF5F5F7;
const LIBER_SIDEBAR: u32 = 0xEBEBED;
const LIBER_ACCENT: u32 = 0x007AFF;
const LIBER_INK: u32 = 0x202124;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
struct Book {
    title: String,
    path: String,
    pages: usize,
    last_page: usize,
}

fn index_path() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("Liber")
        .join("library.json")
}
fn load_books() -> Vec<Book> {
    std::fs::read(index_path())
        .ok()
        .and_then(|v| serde_json::from_slice(&v).ok())
        .unwrap_or_default()
}
fn save_books(books: &[Book]) {
    let path = index_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(bytes) = serde_json::to_vec_pretty(books) {
        let _ = std::fs::write(path, bytes);
    }
}
fn appearance_path() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("Liber")
        .join("appearance.json")
}
fn load_dark() -> bool {
    std::fs::read(appearance_path())
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or(false)
}
fn save_dark(dark: bool) {
    let path = appearance_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(bytes) = serde_json::to_vec(&dark) {
        let _ = std::fs::write(path, bytes);
    }
}

struct Page {
    count: usize,
    png: String,
    text: String,
    toc: Vec<(String, usize)>,
    size: (u32, u32),
}
fn render(path: &str, index: usize, dark: bool) -> Result<Page, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let pdf = Pdf::new(bytes).map_err(|e| format!("Не удалось открыть PDF: {e:?}"))?;
    let page = pdf.pages().get(index).ok_or("Страница не найдена")?;
    if page.raw().contains_key("Contents") && page.page_stream().is_none() {
        return Err("Не удалось разобрать содержимое страницы".into());
    }
    let toc = pdf_nav::outline(&pdf)
        .into_iter()
        .filter_map(|entry| entry.page.map(|at| (entry.title, at)))
        .collect();
    let (width, height) = page.render_dimensions();
    let cache = RenderCache::new();
    let pixmap = hayro::render(
        page,
        &cache,
        &InterpreterSettings::default(),
        &RenderSettings::default(),
        &PixmapSettings {
            x_scale: 1.5,
            y_scale: 1.5,
            bg_color: WHITE,
        },
    );
    let size = (u32::from(pixmap.width()), u32::from(pixmap.height()));
    let mut image = image::RgbaImage::from_raw(
        u32::from(pixmap.width()),
        u32::from(pixmap.height()),
        pixmap.data_as_u8_slice().to_vec(),
    )
    .ok_or("Ошибка буфера hayro")?;
    if dark {
        for pixel in image.pixels_mut() {
            pixel.0 = [255 - pixel[0], 255 - pixel[1], 255 - pixel[2], pixel[3]];
        }
    }
    let temp = std::env::temp_dir().join(format!("liber-nsh-page-{index}.png"));
    image.save(&temp).map_err(|e| e.to_string())?;
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
    Ok(Page {
        count: pdf.pages().len(),
        png: temp.to_string_lossy().into_owned(),
        text: pdf_text::to_lines(&device.glyphs),
        toc,
        size,
    })
}
fn title(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Без названия")
        .to_owned()
}

#[native_module("liber/native")]
fn native(registry: &mut Registry) {
    registry.register::<App>();
}

#[component]
pub fn App() -> impl IntoElement {
    let mut startup_books = load_books();
    let mut startup_selected = None;
    let mut startup_path = String::new();
    let mut startup_page = 0;
    let mut startup_count = 0;
    let mut startup_image = String::new();
    let mut startup_text = String::new();
    let mut startup_outline = Vec::new();
    let mut startup_size = (1, 1);
    if let Some(source) = std::env::var_os("LIBER_OPEN_PDF").map(PathBuf::from) {
        let source = source.to_string_lossy().into_owned();
        if let Ok(first) = render(&source, 0, false) {
            let at = startup_books
                .iter()
                .position(|book| book.path == source)
                .unwrap_or_else(|| {
                    startup_books.push(Book {
                        title: title(&source),
                        path: source.clone(),
                        pages: first.count,
                        last_page: 0,
                    });
                    startup_books.len() - 1
                });
            startup_path = source;
            startup_selected = Some(at);
            startup_count = first.count;
            startup_page = startup_books[at]
                .last_page
                .min(first.count.saturating_sub(1));
            let loaded = if startup_page == 0 {
                Some(first)
            } else {
                render(&startup_path, startup_page, false).ok()
            };
            if let Some(data) = loaded {
                startup_image = data.png;
                startup_text = data.text;
                startup_outline = data.toc;
                startup_size = data.size;
            }
        }
    }
    save_books(&startup_books);
    let books = signal(startup_books);
    let selected = signal(startup_selected);
    let path = signal(startup_path);
    let page = signal(startup_page);
    let count = signal(startup_count);
    let image_path = signal(startup_image);
    let extracted = signal(startup_text);
    let outline = signal(startup_outline);
    let image_size = signal(startup_size);
    let error = signal(String::new());
    let dark = signal(load_dark());

    let load = std::rc::Rc::new(move |at: usize| {
        let source = path.get();
        match render(&source, at, dark.get()) {
            Ok(result) => {
                count.set(result.count);
                page.set(at);
                image_path.set(result.png);
                extracted.set(if result.text.is_empty() {
                    "На странице нет текстового слоя".into()
                } else {
                    result.text
                });
                outline.set(result.toc);
                image_size.set(result.size);
                error.set(String::new());
                if let Some(i) = selected.get() {
                    let mut list = books.get();
                    if let Some(book) = list.get_mut(i) {
                        book.last_page = at;
                        save_books(&list);
                        books.set(list);
                    }
                }
            }
            Err(message) => error.set(message),
        }
    });
    let load_first = load.clone();
    let load_prev = load.clone();
    let load_next = load.clone();
    let bg = move || rgb(if dark.get() { 0x17171A } else { LIBER_BG });
    let fg = move || rgb(if dark.get() { 0xF2F2F7 } else { LIBER_INK });

    view().flex_1().bg(bg).text_color(fg).child(
        safe_area_view().flex_1().child(
            view()
                .flex_1()
                .flex_row()
                .bg(bg)
                .child(
                    view()
                        .w(px(232.0))
                        .flex_col()
                        .bg(move || rgb(if dark.get() { 0x202024 } else { LIBER_SIDEBAR }))
                        .p(px(16.0))
                        .gap_3()
                        .child(
                            text("Liber")
                                .text_size(px(24.0))
                                .font_weight(FontWeight::BOLD),
                        )
                        .child(
                            text("БИБЛИОТЕКА").text_size(px(11.0)).text_color(move || {
                                rgb(if dark.get() { 0xA0A0A8 } else { 0x77777D })
                            }),
                        )
                        .child(
                            button()
                                .on_press(move || {
                                    selected.set(None);
                                })
                                .child(text("⌂   Главная")),
                        )
                        .child(
                            button()
                                .on_press({
                                    let reload = load.clone();
                                    move || {
                                        let next = !dark.get();
                                        dark.set(next);
                                        save_dark(next);
                                        if count.get() > 0 {
                                            reload(page.get());
                                        }
                                    }
                                })
                                .child(text(move || {
                                    if dark.get() {
                                        "☼   Светлая тема"
                                    } else {
                                        "☾   Тёмная тема"
                                    }
                                })),
                        )
                        .child(view().flex_1())
                        .child(
                            button()
                                .on_press(move || {
                                    if let Some(file) = rfd::FileDialog::new()
                                        .add_filter("PDF", &["pdf"])
                                        .pick_file()
                                    {
                                        let source = file.to_string_lossy().into_owned();
                                        path.set(source.clone());
                                        match render(&source, 0, dark.get()) {
                                            Ok(result) => {
                                                let mut list = books.get();
                                                let book = Book {
                                                    title: title(&source),
                                                    path: source,
                                                    pages: result.count,
                                                    last_page: 0,
                                                };
                                                if !list.iter().any(|b| b.path == book.path) {
                                                    list.push(book);
                                                    save_books(&list);
                                                    books.set(list.clone());
                                                }
                                                let i = list
                                                    .iter()
                                                    .position(|b| b.path == path.get())
                                                    .unwrap_or(0);
                                                selected.set(Some(i));
                                                count.set(result.count);
                                                page.set(0);
                                                image_path.set(result.png);
                                                image_size.set(result.size);
                                                extracted.set(result.text);
                                                outline.set(result.toc);
                                                error.set(String::new());
                                            }
                                            Err(message) => error.set(message),
                                        }
                                    }
                                })
                                .child(text("＋   Добавить PDF")),
                        ),
                )
                .child(
                    view()
                        .flex_1()
                        .min_w_0()
                        .flex_col()
                        .child(
                            view()
                                .h(px(64.0))
                                .flex_row()
                                .items_center()
                                .px(px(28.0))
                                .gap_3()
                                .child(
                                    button()
                                        .on_press(move || {
                                            selected.set(None);
                                        })
                                        .child(text("Библиотека")),
                                )
                                .child(view().flex_1())
                                .child(
                                    button()
                                        .on_press({
                                            let load_open = load_first.clone();
                                            move || {
                                                if let Some(i) = selected.get() {
                                                    let list = books.get();
                                                    if let Some(book) = list.get(i) {
                                                        path.set(book.path.clone());
                                                        load_open(book.last_page);
                                                    }
                                                }
                                            }
                                        })
                                        .child(text("Открыть книгу  ›")),
                                )
                                .child(
                                    text(move || {
                                        if count.get() > 0 {
                                            format!("{} / {}", page.get() + 1, count.get())
                                        } else {
                                            format!("{} книг", books.get().len())
                                        }
                                    })
                                    .text_color(move || {
                                        rgb(if dark.get() { 0xA0A0A8 } else { 0x77777D })
                                    }),
                                ),
                        )
                        .child(dynamic(move || {
                            if let Some(i) = selected.get() {
                                let list = books.get();
                                if let Some(book) = list.get(i) {
                                    let open_page = load_first.clone();
                                    let previous_page = load_prev.clone();
                                    let next_page = load_next.clone();
                                    return view()
                                        .flex_1()
                                        .flex_col()
                                        .bg(move || {
                                            rgb(if dark.get() { 0x29292D } else { 0xE9E9EB })
                                        })
                                        .child(
                                            view()
                                                .h(px(48.0))
                                                .flex_row()
                                                .items_center()
                                                .px(px(20.0))
                                                .gap_3()
                                                .child(
                                                    button()
                                                        .on_press(move || {
                                                            selected.set(None);
                                                        })
                                                        .child(text("‹  Библиотека")),
                                                )
                                                .child(text(book.title.clone()).text_size(px(14.0)))
                                                .child(view().flex_1())
                                                .child(
                                                    button()
                                                        .on_press(move || {
                                                            let at = page.get();
                                                            if at > 0 {
                                                                previous_page(at - 1);
                                                            }
                                                        })
                                                        .child(text("‹")),
                                                )
                                                .child(
                                                    text(move || {
                                                        format!(
                                                            "{} / {}",
                                                            page.get() + 1,
                                                            count.get()
                                                        )
                                                    })
                                                    .text_size(px(13.0)),
                                                )
                                                .child(
                                                    button()
                                                        .on_press(move || {
                                                            let at = page.get();
                                                            if at + 1 < count.get() {
                                                                next_page(at + 1);
                                                            }
                                                        })
                                                        .child(text("›")),
                                                ),
                                        )
                                        .child(
                                            view()
                                                .flex_1()
                                                .flex_row()
                                                .min_h_0()
                                                .child(
                                                    scroll().flex_1().child(
                                                        view()
                                                            .flex_col()
                                                            .items_center()
                                                            .py(px(22.0))
                                                            .child(dynamic(move || {
                                                                let src = image_path.get();
                                                                let (width, height) =
                                                                    image_size.get();
                                                                (!src.is_empty()).then(|| {
                                                                    image(src)
                                                                        .w(px(680.0))
                                                                        .h(px(680.0
                                                                            * height as f32
                                                                            / width.max(1) as f32))
                                                                        .max_w(px(820.0))
                                                                })
                                                            })),
                                                    ),
                                                )
                                                .child(
                                                    view()
                                                        .w(px(300.0))
                                                        .bg(move || {
                                                            rgb(if dark.get() {
                                                                0x202024
                                                            } else {
                                                                0xF0F0F2
                                                            })
                                                        })
                                                        .flex_col()
                                                        .p(px(20.0))
                                                        .gap_3()
                                                        .child(
                                                            text("Содержание")
                                                                .text_size(px(16.0))
                                                                .font_weight(FontWeight::SEMIBOLD),
                                                        )
                                                        .child(
                                                            text(move || {
                                                                outline
                                                                    .get()
                                                                    .iter()
                                                                    .map(|(title, page)| {
                                                                        format!(
                                                                            "{}  ·  стр. {}",
                                                                            title,
                                                                            page + 1
                                                                        )
                                                                    })
                                                                    .collect::<Vec<_>>()
                                                                    .join("\n")
                                                            })
                                                            .text_size(px(12.0)),
                                                        )
                                                        .child(
                                                            text("Текст страницы")
                                                                .text_size(px(12.0))
                                                                .text_color(move || {
                                                                    rgb(if dark.get() {
                                                                        0xA0A0A8
                                                                    } else {
                                                                        0x77777D
                                                                    })
                                                                }),
                                                        )
                                                        .child(
                                                            scroll().flex_1().child(
                                                                text(move || extracted.get())
                                                                    .text_size(px(14.0))
                                                                    .line_height(px(22.0)),
                                                            ),
                                                        ),
                                                ),
                                        )
                                        .into_element();
                                }
                            }
                            let list = books.get();
                            let mut shelf = view().flex_row().flex_wrap().gap_5();
                            for (i, book) in list.iter().enumerate() {
                                let name = book.title.clone();
                                let page_count = book.pages;
                                let last_page = book.last_page;
                                let progress = if page_count <= 1 {
                                    0.0
                                } else {
                                    last_page.min(page_count - 1) as f32 / (page_count - 1) as f32
                                };
                                let source = book.path.clone();
                                let open_book = load_first.clone();
                                shelf = shelf.child(
                                    view()
                                        .w(px(176.0))
                                        .flex_col()
                                        .gap_2()
                                        .child(
                                            button()
                                                .on_press(move || {
                                                    path.set(source.clone());
                                                    selected.set(Some(i));
                                                    open_book(last_page);
                                                })
                                                .child(
                                                    view()
                                                        .h(px(226.0))
                                                        .w(px(176.0))
                                                        .rounded(px(8.0))
                                                        .bg(move || {
                                                            rgb(if dark.get() {
                                                                0x3A3A40
                                                            } else {
                                                                0xD9D9DE
                                                            })
                                                        })
                                                        .flex_col()
                                                        .items_center()
                                                        .justify_center()
                                                        .child(
                                                            text("▤")
                                                                .text_size(px(48.0))
                                                                .text_color(move || {
                                                                    rgb(if dark.get() {
                                                                        0xB7B7C0
                                                                    } else {
                                                                        LIBER_ACCENT
                                                                    })
                                                                }),
                                                        )
                                                        .child(text("PDF").text_size(px(11.0))),
                                                ),
                                        )
                                        .child(text(name).text_size(px(14.0)))
                                        .child(
                                            text(format!(
                                                "{} страниц · стр. {}",
                                                page_count,
                                                last_page + 1
                                            ))
                                            .text_size(px(12.0)),
                                        )
                                        .child(
                                            view()
                                                .h(px(3.0))
                                                .w(px(176.0))
                                                .rounded_full()
                                                .bg(rgb(if dark.get() {
                                                    0x45454C
                                                } else {
                                                    0xD5D5DA
                                                }))
                                                .child(
                                                    view()
                                                        .h(px(3.0))
                                                        .w(px(176.0 * progress))
                                                        .rounded_full()
                                                        .bg(rgb(LIBER_ACCENT)),
                                                ),
                                        ),
                                );
                            }
                            let message = if list.is_empty() {
                                "Добавьте PDF, чтобы начать читать"
                            } else {
                                ""
                            };
                            view()
                                .flex_1()
                                .px(px(32.0))
                                .py(px(24.0))
                                .gap_6()
                                .child(
                                    text("Читаю сейчас")
                                        .text_size(px(20.0))
                                        .font_weight(FontWeight::SEMIBOLD),
                                )
                                .child(text("Недавно добавленные").text_size(px(13.0)).text_color(
                                    move || rgb(if dark.get() { 0xA0A0A8 } else { 0x77777D }),
                                ))
                                .child(text(message).text_color(move || {
                                    rgb(if dark.get() { 0xA0A0A8 } else { 0x77777D })
                                }))
                                .child(shelf)
                                .child(text(move || error.get()).text_color(move || {
                                    rgb(if dark.get() { 0xFF6961 } else { 0xC62828 })
                                }))
                                .into_element()
                        })),
                ),
        ),
    )
}
