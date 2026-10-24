//! Liber desktop app. The native UI and PDF reader share one application root.
use liber_core::{
    PageTone, extract_page_text, import_pdf_with_first_page, load_books, render, render_with_tone,
    save_books, search_books, search_pdf, set_progress,
};
use nsh_base::{button, safe_area_view};
use nsh_core::color::rgb;
use nsh_core::desktop::TitleBarStyle;
use nsh_core::ui::desktop::{set_title_bar, title_bar};
use nsh_core::ui::prelude::*;
use std::path::PathBuf;

const LIBER_BG: u32 = 0xF5F5F7;
const LIBER_SIDEBAR: u32 = 0xEBEBED;
const LIBER_ACCENT: u32 = 0x007AFF;
const LIBER_INK: u32 = 0x202124;
const LIBER_BG_DARK: u32 = 0x17171A;
const LIBER_SURFACE_DARK: u32 = 0x202024;
const LIBER_TEXT_DARK: u32 = 0xF2F2F7;
const LIBER_MUTED_LIGHT: u32 = 0x77777D;
const LIBER_MUTED_DARK: u32 = 0xA0A0A8;
const LIBER_READER_LIGHT: u32 = 0xE9E9EB;
const LIBER_READER_DARK: u32 = 0x29292D;
const LIBER_PANEL_LIGHT: u32 = 0xF0F0F2;
const LIBER_COVER_LIGHT: u32 = 0xD9D9DE;
const LIBER_COVER_DARK: u32 = 0x3A3A40;
const LIBER_PROGRESS_LIGHT: u32 = 0xD5D5DA;
const LIBER_PROGRESS_DARK: u32 = 0x45454C;
const LIBER_ERROR_LIGHT: u32 = 0xC62828;
const LIBER_ERROR_DARK: u32 = 0xFF6961;

const SIDEBAR_WIDTH: f32 = 232.0;
const READER_PANEL_WIDTH: f32 = 300.0;
const LIBRARY_BAR_HEIGHT: f32 = 64.0;
const READER_BAR_HEIGHT: f32 = 48.0;
const BOOK_CARD_WIDTH: f32 = 176.0;
const BOOK_CARD_HEIGHT: f32 = 226.0;
const SPACE_16: f32 = 16.0;
const SPACE_20: f32 = 20.0;
const SPACE_22: f32 = 22.0;
const SPACE_24: f32 = 24.0;
const SPACE_28: f32 = 28.0;
const SPACE_32: f32 = 32.0;
const SPACE_36: f32 = 36.0;

fn icon(path: &'static str, size: f32, dark: Signal<bool>) -> impl IntoElement {
    image(path).w(px(size)).h(px(size)).tint_color(move || {
        rgb(if dark.get() {
            LIBER_TEXT_DARK
        } else {
            LIBER_INK
        })
    })
}

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

fn tone_path() -> PathBuf {
    liber_core::default_root().join("reader-tone.json")
}

fn load_tone() -> PageTone {
    std::fs::read(tone_path())
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save_tone(tone: PageTone) {
    if let Ok(bytes) = serde_json::to_vec(&tone) {
        let path = tone_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
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
    let initial_tone = load_tone();
    let mut startup_books = load_books();
    let mut startup_selected = None;
    let mut startup_path = String::new();
    let mut startup_page = 0;
    let mut startup_count = 0;
    let mut startup_image = String::new();
    let mut startup_text = String::new();
    let mut startup_runs = Vec::<liber_core::TextRun>::new();
    let mut startup_rendered = None;
    let mut startup_outline = Vec::new();
    let mut startup_size = (1, 1);
    if let Some(source) = std::env::var_os("LIBER_OPEN_PDF").map(PathBuf::from) {
        if let Ok((book, first_page)) = import_pdf_with_first_page(&source) {
            let source = book.path.clone();
            let at = startup_books
                .iter()
                .position(|candidate| candidate.path == source)
                .unwrap_or_else(|| {
                    startup_books.push(book.clone());
                    startup_books.len() - 1
                });
            startup_path = source.clone();
            startup_selected = Some(at);
            startup_count = book.pages;
            startup_page = startup_books[at]
                .last_page
                .min(book.pages.saturating_sub(1));
            let loaded = if startup_page == 0 && initial_tone == PageTone::Day {
                first_page.or_else(|| render(&source, 0, false).ok())
            } else {
                render_with_tone(&source, startup_page, 1.0, initial_tone).ok()
            };
            if let Some(data) = loaded {
                startup_rendered = Some((startup_page, data.clone()));
                startup_image = data.png;
                startup_text = data.text;
                startup_runs = data.text_runs;
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
    let extracted = signal(startup_text.clone());
    let outline = signal(startup_outline);
    let image_size = signal(startup_size);
    let text_runs = signal(startup_runs);
    let mut initial_pages = vec![None; startup_count];
    if let Some((index, page)) = startup_rendered {
        if let Some(slot) = initial_pages.get_mut(index) {
            *slot = Some(page);
        }
    }
    let rendered_pages = signal(initial_pages);
    let mut initial_reading_pages = vec![None; startup_count];
    if let Some(slot) = initial_reading_pages.get_mut(startup_page) {
        *slot = Some(startup_text.clone());
    }
    let reading_pages = signal(initial_reading_pages);
    let pending_text_pages = signal(Vec::<usize>::new());
    let error = signal(String::new());
    let query = signal(String::new());
    let document_query = signal(String::new());
    let search_hits = signal(Vec::<liber_core::SearchHit>::new());
    let search_generation = signal(0u64);
    let text_mode = signal(false);
    let zoom = signal(1.0f32);
    let tone = signal(initial_tone);
    let dark = signal(load_dark());
    let loading = signal(false);
    let reader_scroll_ref = element_ref();
    let library_sidebar_open = signal(false);
    let outline_sidebar_open = signal(true);

    let pending_pages = signal(Vec::<usize>::new());
    let visible_pages = signal((startup_page, startup_page));
    let request_page = std::rc::Rc::new(move |at: usize| {
        if at >= count.get() || pending_pages.get().contains(&at) {
            return;
        }
        let requested_zoom = if at == page.get() { zoom.get() } else { 1.0 };
        let requested_tone = tone.get();
        if rendered_pages
            .get()
            .get(at)
            .and_then(Option::as_ref)
            .is_some_and(|cached| {
                (cached.zoom - requested_zoom).abs() < 0.001 && cached.tone == requested_tone
            })
        {
            return;
        }
        let mut pending = pending_pages.get();
        pending.push(at);
        pending_pages.set(pending);
        let source = path.get();
        let pages = rendered_pages;
        let pending = pending_pages;
        let count = count;
        let current_path = path;
        let visible = visible_pages;
        let current_zoom = zoom;
        let current_page = page;
        let current_tone = tone;
        spawn(async move {
            let page_path = source.clone();
            let result =
                blocking(move || render_with_tone(page_path, at, requested_zoom, requested_tone))
                    .await;
            let mut pending_list = pending.get();
            pending_list.retain(|index| *index != at);
            pending.set(pending_list);
            let expected_zoom = if at == current_page.get() {
                current_zoom.get()
            } else {
                1.0
            };
            if current_path.get() != source
                || (expected_zoom - requested_zoom).abs() > 0.001
                || current_tone.get() != requested_tone
            {
                return;
            }
            if let Ok(Ok(rendered)) = result {
                let (first, last) = visible.get();
                if at < first.saturating_sub(2) || at > last.saturating_add(2) {
                    return;
                }
                count.set(rendered.count);
                let mut cached = pages.get();
                if cached.len() < rendered.count {
                    cached.resize(rendered.count, None);
                }
                cached[at] = Some(rendered);
                pages.set(cached);
            }
        });
    });
    let request_text_page = std::rc::Rc::new(move |at: usize| {
        if at >= count.get()
            || reading_pages.get().get(at).is_some_and(Option::is_some)
            || pending_text_pages.get().contains(&at)
        {
            return;
        }
        let mut pending = pending_text_pages.get();
        pending.push(at);
        pending_text_pages.set(pending);
        let source = path.get();
        let current_path = path;
        let pages = reading_pages;
        let pending = pending_text_pages;
        spawn(async move {
            let page_path = source.clone();
            let result = blocking(move || extract_page_text(page_path, at)).await;
            let mut loading = pending.get();
            loading.retain(|index| *index != at);
            pending.set(loading);
            if current_path.get() != source {
                return;
            }
            if let Ok(Ok(text)) = result {
                let mut cache = pages.get();
                if cache.len() <= at {
                    cache.resize(at + 1, None);
                }
                cache[at] = Some(text);
                pages.set(cache);
            }
        });
    });
    let load_generation = signal(0u64);
    let preload_after_jump = request_page.clone();
    let load = std::rc::Rc::new(move |at: usize| {
        let generation = load_generation.get().wrapping_add(1);
        load_generation.set(generation);
        page.set(at);
        loading.set(true);
        let source = path.get();
        let requested_zoom = zoom.get();
        let requested_tone = tone.get();
        let generation_signal = load_generation;
        let image_path = image_path;
        let extracted = extracted;
        let outline = outline;
        let image_size = image_size;
        let text_runs = text_runs;
        let rendered_pages = rendered_pages;
        let pending_pages = pending_pages;
        let error = error;
        let count = count;
        let selected = selected;
        let books = books;
        let request = preload_after_jump.clone();
        spawn(async move {
            let result =
                blocking(move || render_with_tone(source, at, requested_zoom, requested_tone))
                    .await;
            if generation_signal.get() != generation {
                return;
            }
            loading.set(false);
            match result {
                Ok(Ok(result)) => {
                    count.set(result.count);
                    let mut cached = vec![None; result.count];
                    cached[at] = Some(result.clone());
                    rendered_pages.set(cached);
                    pending_pages.set(Vec::new());
                    image_path.set(result.png);
                    extracted.set(if result.text.is_empty() {
                        "На странице нет текстового слоя".into()
                    } else {
                        result.text
                    });
                    outline.set(result.toc);
                    image_size.set(result.size);
                    text_runs.set(result.text_runs);
                    error.set(String::new());
                    request(at);
                    request(at.saturating_add(1));
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
                Ok(Err(message)) => error.set(message),
                Err(message) => error.set(format!("Не удалось отрисовать PDF: {message}")),
            }
        });
    });
    let load_first = load.clone();
    let load_prev = load.clone();
    let load_next = load.clone();
    let request_visible = request_page.clone();
    let request_import = request_page.clone();
    if selected.get().is_some() {
        request_page(startup_page);
    }
    let bg = move || rgb(if dark.get() { LIBER_BG_DARK } else { LIBER_BG });
    let fg = move || {
        rgb(if dark.get() {
            LIBER_TEXT_DARK
        } else {
            LIBER_INK
        })
    };

    set_title_bar(TitleBarStyle::Hidden, Some(px(40.0)));
    view().flex_1().font_family(asset!("fonts/Inter.ttf").family).bg(bg).text_color(fg).child(
        safe_area_view().flex_1().child(
            view().flex_1().flex_col().bg(bg)
                .child(view().h(px(40.0)).flex_none().flex_row().items_center()
                    .window_drag(true)
                    .pl(move || title_bar().left + px(12.0))
                    .pr(move || title_bar().right + px(12.0))
                    .gap_3()
                    .child(view().window_drag(false).child(button().on_press(move || {
                        animate_layout(nsh_core::LayoutAnimation::EASE_IN_EASE_OUT);
                        library_sidebar_open.set(!library_sidebar_open.get());
                    }).child(icon(asset!("icons/sidebar-left.svg").uri, 16.0, dark))))
                    .child(text(move || if selected.get().is_some() { "Liber · Книга" } else { "Liber · Библиотека" })
                        .text_size(px(13.0)).font_weight(FontWeight::MEDIUM))
                    .child(view().flex_1().window_drag(true))
                )
                .child(view()
                .flex_1()
                .flex_row()
                .bg(bg)
                .child(
                    view()
                        .w(move || px(if selected.get().is_some() && !library_sidebar_open.get() { 0.0 } else { SIDEBAR_WIDTH }))
                        .min_w_0()
                        .overflow_hidden()
                        .flex_col()
                        .bg(move || rgb(if dark.get() { LIBER_SURFACE_DARK } else { LIBER_SIDEBAR }))
                        .p(move || px(if selected.get().is_some() { 0.0 } else { SPACE_16 }))
                        .gap_3()
                        .child(
                            text("Liber")
                                .text_size(px(24.0))
                                .font_weight(FontWeight::BOLD),
                        )
                        .child(
                            text("БИБЛИОТЕКА").text_size(px(11.0)).text_color(move || {
                                rgb(if dark.get() { LIBER_MUTED_DARK } else { LIBER_MUTED_LIGHT })
                            }),
                        )
                        .child(
                            button()
                                .on_press(move || {
                                    selected.set(None);
                                })
                                .child(view().flex_row().items_center().gap_2().child(icon(asset!("icons/book-open.svg").uri, 16.0, dark)).child(text("Главная"))),
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
                                .child(view().flex_row().items_center().gap_2()
                                    .child(icon(asset!("icons/settings.svg").uri, 16.0, dark))
                                    .child(text(move || {
                                    if dark.get() {
                                        "Светлая тема"
                                    } else {
                                        "Тёмная тема"
                                    }
                                }))),
                        )
                                        .child(view().flex_1())
                        .child(
                            button()
                                .on_press(move || {
                                    if let Some(file) = rfd::FileDialog::new()
                                        .add_filter("PDF", &["pdf"])
                                        .pick_file()
                                    {
                                        let source = file.to_path_buf();
                                        loading.set(true);
                                        load_generation.set(load_generation.get().wrapping_add(1));
                                        let selected_tone = tone.get();
                                        let books = books;
                                        let selected = selected;
                                        let path = path;
                                        let count = count;
                                        let page = page;
                                        let image_path = image_path;
                                        let image_size = image_size;
                                        let extracted = extracted;
                                        let outline = outline;
                                        let text_runs = text_runs;
                                        let rendered_pages = rendered_pages;
                                        let reading_pages = reading_pages;
                                        let pending_pages = pending_pages;
                                        let pending_text_pages = pending_text_pages;
                                        let request_import = request_import.clone();
                                        let error = error;
                                        spawn(async move {
                                            let result = blocking(move || -> Result<_, String> {
                                                let (book, first_page) = import_pdf_with_first_page(&source)?;
                                                let rendered = if book.last_page == 0 && selected_tone == PageTone::Day {
                                                    first_page.or_else(|| render(&book.path, 0, false).ok())
                                                        .ok_or_else(|| "Не удалось отрисовать первую страницу".to_owned())?
                                                } else {
                                                    render_with_tone(&book.path, book.last_page, 1.0, selected_tone)?
                                                };
                                                Ok((book, rendered))
                                            }).await;
                                            match result {
                                                Ok(Ok((book, rendered))) => {
                                                    loading.set(false);
                                                    let mut list = books.get();
                                                    if !list.iter().any(|b| b.path == book.path) {
                                                        list.push(book.clone());
                                                    }
                                                    save_books(&list);
                                                    let index = list.iter().position(|b| b.path == book.path).unwrap_or(0);
                                                    books.set(list);
                                                    path.set(book.path);
                                                    selected.set(Some(index));
                                                    count.set(rendered.count);
                                                    page.set(book.last_page);
                                                    let mut cache = vec![None; rendered.count];
                                                    cache[book.last_page] = Some(rendered.clone());
                                                    rendered_pages.set(cache);
                                                    let mut text_cache = vec![None; rendered.count];
                                                    text_cache[book.last_page] = Some(rendered.text.clone());
                                                    reading_pages.set(text_cache);
                                                    pending_pages.set(Vec::new());
                                                    pending_text_pages.set(Vec::new());
                                                    request_import(book.last_page);
                                                    image_path.set(rendered.png.clone());
                                                    image_size.set(rendered.size);
                                                    extracted.set(rendered.text);
                                                    outline.set(rendered.toc);
                                                    text_runs.set(rendered.text_runs);
                                                    error.set(String::new());
                                                }
                                                Ok(Err(message)) => { loading.set(false); error.set(message); },
                                                Err(message) => { loading.set(false); error.set(format!("Не удалось открыть PDF: {message}")); },
                                            }
                                        });
                                    }
                                })
                                .child(view().flex_row().items_center().gap_2().child(icon(asset!("icons/plus.svg").uri, 16.0, dark)).child(text("Добавить PDF"))),
                        ),
                )
                .child(
                    view()
                        .flex_1()
                        .min_w_0()
                        .flex_col()
                        .child(
                            view()
                                .h(px(LIBRARY_BAR_HEIGHT))
                                .flex_row()
                                .items_center()
                                .px(px(SPACE_28))
                                .gap_3()
                                .child(
                                    button()
                                        .on_press(move || {
                                            selected.set(None);
                                        })
                                        .child(view().flex_row().items_center().gap_2().child(icon(asset!("icons/book-open.svg").uri, 16.0, dark)).child(text("Библиотека"))),
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
                                        .child(view().flex_row().items_center().gap_2().child(icon(asset!("icons/book-open.svg").uri, 16.0, dark)).child(text("Открыть книгу"))),
                                )
                                .child(
                                    text(move || {
                                        if loading.get() {
                                            "Загрузка PDF…".to_owned()
                                        } else if count.get() > 0 {
                                            format!("{} / {}", page.get() + 1, count.get())
                                        } else {
                                            format!("{} книг", books.get().len())
                                        }
                                    })
                                    .text_color(move || {
                                        rgb(if dark.get() { LIBER_MUTED_DARK } else { LIBER_MUTED_LIGHT })
                                    }),
                                ),
                        )
                        .child(dynamic(move || {
                            let request_for_reader = request_visible.clone();
                            let request_text_for_reader = request_text_page.clone();
                            if let Some(i) = selected.get() {
                                let list = books.get();
                                if let Some(book) = list.get(i) {
                                    let open_page = load_first.clone();
                                    let previous_page = load_prev.clone();
                                    let next_page = load_next.clone();
                        let mut toc_entries = view().flex_col().gap_1();
                        for (title, at, depth) in outline.get() {
                            let jump = load_first.clone();
                            let scroll_ref = reader_scroll_ref;
                            toc_entries = toc_entries.child(
                                button().on_press(move || {
                                    scroll_ref.scroll_to_index(
                                        at as u32,
                                        nsh_core::geometry::Percentage(0.0),
                                        px(0.0),
                                        true,
                                    );
                                    jump(at);
                                }).child(text(format!(
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
                            let scroll_ref = reader_scroll_ref;
                            result_entries = result_entries.child(
                                button().on_press(move || {
                                    scroll_ref.scroll_to_index(at as u32, nsh_core::geometry::Percentage(0.0), px(0.0), true);
                                    jump(at);
                                }).child(text(format!(
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
                                            rgb(match tone.get() {
                                                PageTone::Day => if dark.get() { LIBER_READER_DARK } else { LIBER_READER_LIGHT },
                                                PageTone::Sepia => 0xE6DCC2,
                                                PageTone::Gray => 0x29292D,
                                                PageTone::Night => 0x17171A,
                                            })
                                        })
                                        .child(
                                            view()
                                                .h(px(READER_BAR_HEIGHT))
                                                .flex_row()
                                                .items_center()
                                                .px(px(SPACE_20))
                                                .gap_3()
                                                .child(view().window_drag(false).child(button().on_press(move || {
                                                    animate_layout(nsh_core::LayoutAnimation::EASE_IN_EASE_OUT);
                                                    outline_sidebar_open.set(!outline_sidebar_open.get());
                                                }).child(icon(asset!("icons/sidebar-left.svg").uri, 16.0, dark))))
                                                .child(
                                                    button()
                                                        .on_press(move || {
                                                            selected.set(None);
                                                        })
                                                        .child(view().flex_row().items_center().gap_2().child(icon(asset!("icons/arrow-left.svg").uri, 16.0, dark)).child(text("Библиотека"))),
                                                )
                                                .child(text(book.title.clone()).text_size(px(14.0)))
                                                .child(view().flex_1())
                                                .child(button().on_press({
                                                    let reopen = load_first.clone();
                                                    move || {
                                                        let next = tone.get().next();
                                                        tone.set(next);
                                                        save_tone(next);
                                                        rendered_pages.set(vec![None; count.get()]);
                                                        pending_pages.set(Vec::new());
                                                        reopen(page.get());
                                                    }
                                                }).child(text(move || tone.get().label())))
                                                .child(button().on_press({
                                                    let reopen = load_first.clone();
                                                    move || {
                                                        let next = (zoom.get() - 0.25).max(0.5);
                                                        if (next - zoom.get()).abs() > 0.001 {
                                                            zoom.set(next);
                                                            rendered_pages.set(vec![None; count.get()]);
                                                            pending_pages.set(Vec::new());
                                                            reopen(page.get());
                                                        }
                                                    }
                                                }).child(text("−")))
                                                .child(text(move || format!("{}%", (zoom.get() * 100.0).round() as u32)).text_size(px(12.0)))
                                                .child(button().on_press({
                                                    let reopen = load_first.clone();
                                                    move || {
                                                        let next = (zoom.get() + 0.25).min(3.0);
                                                        if (next - zoom.get()).abs() > 0.001 {
                                                            zoom.set(next);
                                                            rendered_pages.set(vec![None; count.get()]);
                                                            pending_pages.set(Vec::new());
                                                            reopen(page.get());
                                                        }
                                                    }
                                                }).child(text("+")))
                                                .child(
                                                    button()
                                                        .on_press({
                                                            let request = request_text_for_reader.clone();
                                                            move || {
                                                                let enabled = !text_mode.get();
                                                                text_mode.set(enabled);
                                                                if enabled {
                                                                    request(page.get());
                                                                }
                                                            }
                                                        })
                                                        .child(text(move || if text_mode.get() { "Страница" } else { "Текст" })),
                                                )
                                                .child(
                                                    button()
                                                        .on_press(move || {
                                                            let at = page.get();
                                                            if at > 0 {
                                                                reader_scroll_ref.scroll_to_index(at as u32 - 1, nsh_core::geometry::Percentage(0.0), px(0.0), true);
                                                                previous_page(at - 1);
                                                            }
                                                        })
                                                        .child(icon(asset!("icons/arrow-left.svg").uri, 16.0, dark)),
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
                                                                reader_scroll_ref.scroll_to_index(at as u32 + 1, nsh_core::geometry::Percentage(0.0), px(0.0), true);
                                                                next_page(at + 1);
                                                            }
                                                        })
                                                        .child(icon(asset!("icons/arrow-right.svg").uri, 16.0, dark)),
                                                )
                                                .child(
                                                    button()
                                                        .on_press(move || set_clipboard(extracted.get()))
                                                        .child(
                                                            view()
                                                                .flex_row()
                                                                .items_center()
                                                                .gap_2()
                                                                .child(icon(asset!("icons/copy.svg").uri, 16.0, dark))
                                                                .child(text("Копировать")),
                                                        ),
                                                ),
                                        )
                                        .child(
                                            view()
                                                .flex_1()
                                                .flex_row()
                                                .min_h_0()
                                                .child(dynamic(move || {
                                                    if text_mode.get() {
                                                        virtual_list(move || count.get(), move |position| {
                                                            let index = position.index();
                                                            dynamic(move || {
                                                                let body = reading_pages.get().get(index).cloned().flatten();
                                                                let Some(body) = body else {
                                                                    return view().w_full().h(px(220.0)).items_center().justify_center()
                                                                        .child(text("Загрузка текста…").text_color(rgb(LIBER_MUTED_LIGHT))).into_element();
                                                                };
                                                                view().w_full().flex().justify_center().py(px(SPACE_36)).selection_area(true).child(
                                                                    text(body)
                                                                        .max_w(px(700.0))
                                                                        .font_family("Georgia")
                                                                        .text_size(px(19.0))
                                                                        .line_height(px(31.0))
                                                                        .selectable(true)
                                                                        .selection_color(rgb(LIBER_ACCENT)),
                                                                ).into_element()
                                                            })
                                                        })
                                                        .estimated_item_size(px(700.0))
                                                        .initial_scroll_index(page.get())
                                                        .on_viewable_items_change({
                                                            let request = request_text_for_reader.clone();
                                                            let current_page = page;
                                                            move |items: &ViewableItems| {
                                                                if let Some((first, last)) = items.viewable {
                                                                    current_page.set(first);
                                                                    for index in first.saturating_sub(1)..=last.saturating_add(1).min(count.get().saturating_sub(1)) {
                                                                        request(index);
                                                                    }
                                                                }
                                                            }
                                                        })
                                                        .flex_1()
                                                        .into_element()
                                                    } else {
                                                        scroll().flex_1().horizontal().child(
                                                            view().w_full().min_w(px(680.0 * zoom.get())).flex().justify_center().child(
                                                        virtual_list(move || count.get(), move |position| {
                                                            let index = position.index();
                                                            dynamic(move || {
                                                                let rendered = rendered_pages.get().get(index).cloned().flatten();
                                                                let Some(rendered) = rendered else {
                                                                    let scale = zoom.get();
                                                                    return view().w(px(680.0 * scale)).h(px(980.0 * scale)).items_center().justify_center()
                                                                        .child(text("Загрузка страницы…").text_color(rgb(LIBER_MUTED_LIGHT))).into_element();
                                                                };
                                                                let (width, height) = rendered.size;
                                                                if width == 0 { return view().h(px(980.0)).into_element(); }
                                                                // MuPDF text geometry uses page points while the bitmap is rendered at 1.25x.
                                                                let display_width = 680.0 * zoom.get();
                                                                let scale = display_width / rendered.points.0;
                                                                let image_height = display_width * height as f32 / width as f32;
                                                                let mut selectable = view().absolute().inset_0();
                                                                for run in rendered.text_runs {
                                                                    let font_size = (run.size as f32 * scale).clamp(6.0, 48.0);
                                                                    selectable = selectable.child(text(run.text).absolute()
                                                                        .left(px(run.x as f32 * scale))
                                                                        .top(px((run.y as f32 - run.size as f32 * 0.82) * scale))
                                                                        .text_size(px(font_size)).text_color(rgb(LIBER_INK).opacity(0.0))
                                                                        .selectable(true).selection_color(rgb(LIBER_ACCENT)));
                                                                }
                                                                let mut tiles = view().absolute().inset_0();
                                                                for tile in rendered.tiles {
                                                                    let tile_scale = scale / tile.scale;
                                                                    tiles = tiles.child(view().absolute()
                                                                        .left(px(tile.x as f32 * tile_scale))
                                                                        .top(px(tile.y as f32 * tile_scale))
                                                                        .w(px(tile.width as f32 * tile_scale))
                                                                        .h(px(tile.height as f32 * tile_scale))
                                                                        .child(image(tile.png).w_full().h_full()));
                                                                }
                                                                view().w_full().flex().justify_center().child(
                                                                    view().relative().selection_area(true).w(px(display_width)).h(px(image_height))
                                                                        .child(image(rendered.png).w(px(display_width)).h(px(image_height)))
                                                                        .child(tiles).child(selectable)
                                                                ).into_element()
                                                            })
                                                        })
                                                        .estimated_item_size(move || px(980.0 * zoom.get()))
                                                        .row_gap(px(SPACE_22))
                                                        .content_padding_y(px(SPACE_22))
                                                        .initial_scroll_index(page.get())
                                                        .element_ref(&reader_scroll_ref)
                                                        .on_viewable_items_change({
                                                            let request = request_for_reader.clone();
                                                            let pages = rendered_pages;
                                                            let visible = visible_pages;
                                                            let count = count;
                                                            let current_page = page;
                                                            move |items: &ViewableItems| {
                                                                if let Some((first, last)) = items.viewable {
                                                                    let start = first.saturating_sub(1);
                                                                    let end = last.saturating_add(1).min(count.get().saturating_sub(1));
                                                                    visible.set((first, last));
                                                                    current_page.set(first);
                                                                    let mut cache = pages.get();
                                                                    let mut changed = false;
                                                                    for (index, page) in cache.iter_mut().enumerate() {
                                                                                if index != first {
                                                                                    if let Some(page) = page {
                                                                                        if !page.tiles.is_empty() {
                                                                                            page.tiles.clear();
                                                                                            changed = true;
                                                                                        }
                                                                                    }
                                                                                }
                                                                        if (index < start || index > end) && page.is_some() {
                                                                            *page = None;
                                                                            changed = true;
                                                                        }
                                                                    }
                                                                    if changed {
                                                                        pages.set(cache);
                                                                    }
                                                                    for index in start..=end {
                                                                        request(index);
                                                                    }
                                                                }
                                                            }
                                                        })
                                                        .flex_1()
                                                            )
                                                        ).into_element()
                                                    }
                                                }))
                                                .child(view().w(move || px(if outline_sidebar_open.get() { READER_PANEL_WIDTH } else { 0.0 }))
                                                        .overflow_hidden()
                                                        .bg(move || {
                                                            rgb(if dark.get() {
                                                                LIBER_SURFACE_DARK
                                                            } else {
                                                                LIBER_PANEL_LIGHT
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
                                                                    let query = value.to_owned();
                                                                    document_query.set(query.clone());
                                                                    let generation = search_generation.get().wrapping_add(1);
                                                                    search_generation.set(generation);
                                                                    let generation_state = search_generation;
                                                                    let source = path.get();
                                                                    spawn(async move {
                                                                        let result = blocking(move || search_pdf(source, &query)).await;
                                                                        if generation_state.get() != generation {
                                                                            return;
                                                                        }
                                                                        match result {
                                                                            Ok(Ok(found)) => {
                                                                                search_hits.set(found);
                                                                                error.set(String::new());
                                                                            }
                                                                            Ok(Err(message)) => error.set(message),
                                                                            Err(message) => error.set(format!("Не удалось выполнить поиск: {message}")),
                                                                        }
                                                                    });
                                                                }),
                                                        )
                                                        .child(result_entries)
                                                        .child(
                                                            text("Текст страницы")
                                                                .text_size(px(12.0))
                                                                .text_color(move || {
                                                                    rgb(if dark.get() {
                                                                        LIBER_MUTED_DARK
                                                                    } else {
                                                                        LIBER_MUTED_LIGHT
                                                                    })
                                                                }),
                                                        )
                                                        .child(
                                                            scroll().flex_1().child(
                                                                text(move || extracted.get())
                                                                    .text_size(px(14.0))
                                                                    .line_height(px(22.0)),
                                                            ),
                                                        ).into_element()
                                                )
                                        ).into_element();
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
                                let page_count_signal = count;
                                let pages_signal = rendered_pages;
                                let reading_signal = reading_pages;
                                let pending_signal = pending_pages;
                                let pending_text_signal = pending_text_pages;
                                shelf = shelf.child(
                                    view()
                                        .w(px(BOOK_CARD_WIDTH))
                                        .flex_col()
                                        .gap_2()
                                        .child(
                                            button()
                                                .on_press(move || {
                                                    path.set(source.clone());
                                                    page_count_signal.set(page_count);
                                                    pages_signal.set(vec![None; page_count]);
                                                    reading_signal.set(vec![None; page_count]);
                                                    pending_signal.set(Vec::new());
                                                    pending_text_signal.set(Vec::new());
                                                    search_generation.set(search_generation.get().wrapping_add(1));
                                                    document_query.set(String::new());
                                                    search_hits.set(Vec::new());
                                                    selected.set(Some(i));
                                                    open_book(last_page);
                                                })
                                                .child(
                                                    view()
                                                        .h(px(BOOK_CARD_HEIGHT))
                                                        .w(px(BOOK_CARD_WIDTH))
                                                        .rounded(px(8.0))
                                                        .bg(move || {
                                                            rgb(if dark.get() {
                                                                LIBER_COVER_DARK
                                                            } else {
                                                                LIBER_COVER_LIGHT
                                                            })
                                                        })
                                                        .flex_col()
                                                        .items_center()
                                                        .justify_center()
                                                        .child(dynamic(move || {
                                                            if cover.is_empty() {
                                                                view().h(px(BOOK_CARD_HEIGHT)).w(px(BOOK_CARD_WIDTH)).flex_col().items_center().justify_center()
                                                                    .child(icon(asset!("icons/book-open.svg").uri, 48.0, dark))
                                                                    .child(text("PDF").text_size(px(11.0))).into_element()
                                                            } else {
                                                                image(cover.clone()).w(px(BOOK_CARD_WIDTH)).h(px(BOOK_CARD_HEIGHT)).into_element()
                                                            }
                                                        })),
                                                ),
                                        )
                                        .child(text(name).text_size(px(14.0)))
                                        .child(text(author).text_size(px(12.0)).text_color(move || {
                                            rgb(if dark.get() { LIBER_MUTED_DARK } else { LIBER_MUTED_LIGHT })
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
                                                .w(px(BOOK_CARD_WIDTH))
                                                .rounded_full()
                                                .bg(rgb(if dark.get() {
                                                    LIBER_PROGRESS_DARK
                                                } else {
                                                    LIBER_PROGRESS_LIGHT
                                                }))
                                                .child(
                                                    view()
                                                        .h(px(3.0))
                                                        .w(px(BOOK_CARD_WIDTH * progress))
                                                        .rounded_full()
                                                        .bg(rgb(LIBER_ACCENT)),
                                                ),
                                        ),
                                );
                            }
                            view()
                                .flex_1()
                                .px(px(SPACE_32))
                                .py(px(SPACE_24))
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
                                    move || rgb(if dark.get() { LIBER_MUTED_DARK } else { LIBER_MUTED_LIGHT }),
                                ))
                                .child(text(message).text_color(move || {
                                    rgb(if dark.get() { LIBER_MUTED_DARK } else { LIBER_MUTED_LIGHT })
                                }))
                                .child(shelf)
                                .child(text(move || error.get()).text_color(move || {
                                    rgb(if dark.get() { LIBER_ERROR_DARK } else { LIBER_ERROR_LIGHT })
                                }))
                                .into_element()
                        })),
                ),
                ),
        ),
    )
}
