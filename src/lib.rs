//! Liber desktop app. The native UI and PDF reader share one application root.
use liber_core::{
    import_pdf, load_books, render, save_books, search_books, search_pdf, set_progress,
};
use nsh_base::{button, safe_area_view};
use nsh_core::color::rgb;
use nsh_core::ui::prelude::*;
use std::path::PathBuf;

const LIBER_BG: u32 = 0xF5F5F7;
const LIBER_SIDEBAR: u32 = 0xEBEBED;
const LIBER_ACCENT: u32 = 0x007AFF;
const LIBER_INK: u32 = 0x202124;

fn appearance_path() -> PathBuf {
    liber_core::default_root().join("appearance.json")
}
fn load_dark() -> bool {
    std::fs::read(appearance_path())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
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

#[native_module("liber/native")]
fn native(registry: &mut Registry) {
    registry.assets(assets!());
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
        if let Ok(book) = import_pdf(&source) {
            let source = book.path.clone();
            if let Ok(first) = render(&source, 0, false) {
                let at = startup_books
                    .iter()
                    .position(|book| book.path == source)
                    .unwrap_or_else(|| {
                        startup_books.push(book.clone());
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
    let query = signal(String::new());
    let document_query = signal(String::new());
    let search_hits = signal(Vec::<liber_core::SearchHit>::new());
    let text_mode = signal(false);
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
                    if let Some(book) = list.get(i) {
                        let id = book.id.clone();
                        if id.is_empty() {
                            list[i].last_page = at;
                        } else {
                            set_progress(&mut list, &id, at);
                        }
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

    view().flex_1().font_family(asset!("fonts/Inter.ttf").family).bg(bg).text_color(fg).child(
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
                                        match import_pdf(&file) {
                                            Ok(book) => match render(&book.path, book.last_page, dark.get()) {
                                            Ok(result) => {
                                                let mut list = books.get();
                                                if !list.iter().any(|b| b.path == book.path) {
                                                    list.push(book.clone());
                                                }
                                                save_books(&list);
                                                books.set(list.clone());
                                                let i = list
                                                    .iter()
                                                    .position(|b| b.path == path.get())
                                                    .or_else(|| list.iter().position(|b| b.id == book.id))
                                                    .unwrap_or(0);
                                                path.set(book.path.clone());
                                                selected.set(Some(i));
                                                count.set(result.count);
                                                page.set(book.last_page);
                                                image_path.set(result.png);
                                                image_size.set(result.size);
                                                extracted.set(result.text);
                                                outline.set(result.toc);
                                                error.set(String::new());
                                            }
                                            Err(message) => error.set(message),
                                            },
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
                        let mut toc_entries = view().flex_col().gap_1();
                        for (title, at, depth) in outline.get() {
                            let jump = load_first.clone();
                            toc_entries = toc_entries.child(
                                button().on_press(move || jump(at)).child(text(format!(
                                    "{}{}",
                                    "  ".repeat(depth.min(8)),
                                    title
                                ))),
                            );
                        }
                        let mut result_entries = view().flex_col().gap_1();
                        for hit in search_hits.get() {
                            let jump = load_first.clone();
                            let at = hit.page;
                            result_entries = result_entries.child(
                                button().on_press(move || jump(at)).child(text(format!(
                                    "{} · {}",
                                    at + 1,
                                    hit.excerpt
                                )).text_size(px(11.0))),
                            );
                        }
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
                                                        .on_press(move || text_mode.set(!text_mode.get()))
                                                        .child(text(move || if text_mode.get() { "Страница" } else { "Текст" })),
                                                )
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
                                                .child(dynamic(move || {
                                                    if text_mode.get() {
                                                        scroll().flex_1().child(
                                                            view().w_full().flex().justify_center().py(px(36.0)).child(
                                                                text(move || extracted.get())
                                                                    .max_w(px(700.0))
                                                                    .font_family("Georgia")
                                                                    .text_size(px(19.0))
                                                                    .line_height(px(31.0)),
                                                            ),
                                                        ).into_element()
                                                    } else {
                                                        scroll().flex_1().child(
                                                            view()
                                                                .flex_col()
                                                                .items_center()
                                                                .py(px(22.0))
                                                                .child(dynamic(move || {
                                                                    let src = image_path.get();
                                                                    let (width, height) = image_size.get();
                                                                    (!src.is_empty()).then(|| {
                                                                        image(src)
                                                                            .w(px(680.0))
                                                                            .h(px(680.0 * height as f32 / width.max(1) as f32))
                                                                            .max_w(px(820.0))
                                                                    })
                                                                })),
                                                        ).into_element()
                                                    }
                                                }))
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
                                                        .child(toc_entries)
                                                        .child(
                                                            text("Поиск по книге")
                                                                .text_size(px(13.0))
                                                                .font_weight(FontWeight::SEMIBOLD),
                                                        )
                                                        .child(
                                                            input()
                                                                .value(move || document_query.get())
                                                                .placeholder("Найти в PDF")
                                                                .on_change(move |value: &str| {
                                                                    document_query.set(value.to_owned());
                                                                    match search_pdf(&path.get(), value) {
                                                                        Ok(found) => {
                                                                            search_hits.set(found);
                                                                            error.set(String::new());
                                                                        }
                                                                        Err(message) => error.set(message),
                                                                    }
                                                                }),
                                                        )
                                                        .child(result_entries)
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
                            let visible = search_books(&list, &query.get());
                            let message = if list.is_empty() {
                                "Добавьте PDF, чтобы начать читать"
                            } else if visible.is_empty() {
                                "Ничего не найдено"
                            } else {
                                ""
                            };
                            for book in visible {
                                let i = list
                                    .iter()
                                    .position(|candidate| candidate.path == book.path)
                                    .unwrap_or(0);
                                let name = book.title.clone();
                                let author = book.author.clone();
                                let page_count = book.pages;
                                let last_page = book.last_page;
                                let progress = book.progress();
                                let cover = book.cover.clone();
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
                                                        .child(dynamic(move || {
                                                            if cover.is_empty() {
                                                                view().h(px(226.0)).w(px(176.0)).flex_col().items_center().justify_center()
                                                                    .child(text("▤").text_size(px(48.0)).text_color(move || rgb(if dark.get() { 0xB7B7C0 } else { LIBER_ACCENT })))
                                                                    .child(text("PDF").text_size(px(11.0))).into_element()
                                                            } else {
                                                                image(cover.clone()).w(px(176.0)).h(px(226.0)).into_element()
                                                            }
                                                        })),
                                                ),
                                        )
                                        .child(text(name).text_size(px(14.0)))
                                        .child(text(author).text_size(px(12.0)).text_color(move || {
                                            rgb(if dark.get() { 0xA0A0A8 } else { 0x77777D })
                                        }))
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
                                .child(
                                    input()
                                        .value(move || query.get())
                                        .placeholder("Поиск по названию или автору")
                                        .on_change(move |value: &str| query.set(value.to_owned())),
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
