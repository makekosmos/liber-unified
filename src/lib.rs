//! Liber desktop app. The native UI and PDF reader share one application root.
use liber_core::{
    PageTone, READING_WIDTH_PT, extract_reading_blocks, import_pdf_with_first_page, load_books,
    render_with_tone, save_books, search_books, search_pdf, set_progress,
};
#[cfg(target_os = "macos")]
pub mod page_view;
pub mod reader_pages;
pub mod reader_window;
pub mod sidebar;

use nsh_base::{button, safe_area_view};
use nsh_core::color::rgb;
use nsh_core::desktop::TitleBarStyle;
use nsh_core::ui::desktop::{set_title_bar, title_bar};
use nsh_core::ui::prelude::*;
use serde_json::{Value as JsonValue, json};
use sidebar::{Page, native_sidebar};
use std::path::PathBuf;
use std::sync::Mutex;

const LIBER_BG: u32 = 0xF5F5F7;
const LIBER_SIDEBAR: u32 = 0xEBEBED;
const LIBER_ACCENT: u32 = 0x007AFF;
const LIBER_INK: u32 = 0x202124;
const LIBER_BG_DARK: u32 = 0x17171A;
const LIBER_SURFACE_DARK: u32 = 0x202024;
const LIBER_TEXT_DARK: u32 = 0xF2F2F7;
const LIBER_MUTED_LIGHT: u32 = 0x77777D;
const LIBER_MUTED_DARK: u32 = 0xA0A0A8;
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
const TITLE_CONTROLS_WIDTH: f32 = 116.0;
const READER_BAR_HEIGHT: f32 = 48.0;
/// Height of the tone that fades from the top of a reader window under its native toolbar.
const READER_DIM_HEIGHT: f32 = 120.0;
/// The share of that height that is solid tone before it starts to fade out.
const READER_DIM_SOLID: f32 = 0.12;
/// The tones the reader's settings popover offers, in its segment order.
const READER_TONES: [PageTone; 4] = [
    PageTone::Day,
    PageTone::Sepia,
    PageTone::Gray,
    PageTone::Night,
];

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

fn sidebar_frame(selected: Signal<Option<usize>>, open: Signal<bool>, dark: Signal<bool>) -> View {
    view()
        .w(move || {
            px(
                if sidebar::system() || (selected.get().is_some() && !open.get()) {
                    0.0
                } else {
                    SIDEBAR_WIDTH
                },
            )
        })
        .min_w_0()
        .overflow_hidden()
        .flex_col()
        .bg(move || {
            rgb(if dark.get() {
                LIBER_SURFACE_DARK
            } else {
                LIBER_SIDEBAR
            })
        })
        .p(move || {
            px(
                if sidebar::system() || selected.get().is_some() || sidebar::native_registered() {
                    0.0
                } else {
                    SPACE_16
                },
            )
        })
        .gap_3()
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

/// The library window's app.
#[component]
pub fn App() -> impl IntoElement {
    window_app(false)
}

/// The app of a reader window: the book in [`READER_BOOK`] and nothing else.
#[component]
pub fn ReaderApp() -> impl IntoElement {
    window_app(true)
}

/// The book a reader window shows, set before the window opens.
static READER_BOOK: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Opens `source` in a reader window titled `title`.
pub fn open_reader_window(source: &str, title: &str) {
    #[cfg(target_os = "macos")]
    {
        *READER_BOOK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = Some(PathBuf::from(source));
        imago_window_macos::open_reader(title, source);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (source, title);
}

/// Opens the book named by `LIBER_OPEN_PDF`, imported if it is new, in a reader window.
pub fn open_startup_reader() {
    let Some(source) = std::env::var_os("LIBER_OPEN_PDF").map(PathBuf::from) else {
        return;
    };
    match import_pdf_with_first_page(&source) {
        Ok((book, _)) => {
            open_reader_window(&book.path, &book.title);
        }
        Err(error) => eprintln!("Liber: could not open {}: {error}", source.display()),
    }
}

fn reader_tone_color(tone: PageTone) -> u32 {
    tone.palette().paper
}

#[derive(Clone, Copy, PartialEq)]
struct SelectionDrag {
    page: usize,
    start_surface: (f32, f32),
    end_surface: (f32, f32),
}

impl SelectionDrag {
    fn is_drag(&self) -> bool {
        let dx = self.end_surface.0 - self.start_surface.0;
        let dy = self.end_surface.1 - self.start_surface.1;
        dx * dx + dy * dy >= 9.0
    }
}

fn window_app(reader_window: bool) -> impl IntoElement {
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
    let mut startup_error = None;
    let reader_source = reader_window
        .then(|| {
            READER_BOOK
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .clone()
        })
        .flatten();
    if let Some(source) = reader_source {
        match import_pdf_with_first_page(&source) {
            Err(error) => startup_error = Some(format!("Не удалось открыть книгу: {error}")),
            Ok((book, first_page)) => {
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
                    first_page.or_else(|| render_with_tone(&source, 0, 1.0, initial_tone).ok())
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
    }
    let reader_error = signal(startup_error);
    let initial_notes_book = startup_selected
        .and_then(|index| startup_books.get(index))
        .map(|book| book.id.clone());
    let initial_notes = initial_notes_book
        .as_deref()
        .map(|book_id| liber_core::notes::Notes::open(&liber_core::default_root(), book_id).items)
        .unwrap_or_default();
    let notes_snapshot_book = signal(initial_notes_book);
    let notes_snapshot = signal(initial_notes);
    save_books(&startup_books);
    let books = signal(startup_books);
    let selected = signal(startup_selected);
    let section = signal(Page::Home);
    let path = signal(startup_path);
    let page = signal(startup_page);
    // The virtual list reads this only when its mode is created. The live page
    // signal changes while scrolling and must not rebuild the list on every
    // page boundary.
    let scroll_restore_page = signal(startup_page);
    // Ignore pre-restoration visibility from a newly created list so row 0
    // cannot overwrite the saved progress before initial_scroll_index applies.
    let restore_page_pending = signal(true);
    let count = signal(startup_count);
    let record_page = std::rc::Rc::new(move |at: usize| {
        if count.get() == 0 {
            return;
        }
        let at = at.min(count.get() - 1);
        if page.get() != at {
            page.set(at);
        }
        let Some(index) = selected.get() else {
            return;
        };
        let mut list = books.get();
        let Some(book) = list.get(index) else {
            return;
        };
        if book.last_page == at {
            return;
        }
        let id = book.id.clone();
        if id.is_empty() {
            list[index].last_page = at;
        } else {
            set_progress(&mut list, &id, at);
        }
        save_books(&list);
    });
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
    let rendered_pages: Signal<Vec<Signal<Option<liber_core::Page>>>> =
        signal(initial_pages.into_iter().map(signal).collect());
    let reading_pages = signal(vec![None::<Vec<liber_core::ReadingBlock>>; startup_count]);
    let pending_text_pages = signal(Vec::<usize>::new());
    let error = signal(String::new());
    let query = signal(String::new());
    let document_query = signal(String::new());
    let search_hits = signal(Vec::<liber_core::SearchHit>::new());
    let search_generation = signal(0u64);
    let text_mode = signal(false);
    let zoom = signal(1.0f32);
    let gesture_zoom = animated_value(1.0f32);
    let reader_viewport_width = signal(1000.0f32);
    // Shown while the pointer is on the reader window's top edge.
    let tone = signal(initial_tone);
    let dark = signal(load_dark());
    let loading = signal(false);
    let reader_scroll_ref = element_ref();
    let library_sidebar_open = signal(false);
    let outline_sidebar_open = signal(true);
    let notes_sidebar_open = signal(false);
    let selection = signal(None::<SelectionDrag>);
    let note_comment = signal(String::new());
    let note_draft = signal(None::<liber_core::notes::Highlight>);
    let note_action_enabled = std::rc::Rc::new(move || {
        !text_mode.get()
            && selection.get().is_some_and(|drag| {
                drag.is_drag()
                    && rendered_pages
                        .get()
                        .get(drag.page)
                        .and_then(|slot| slot.get())
                        .is_some_and(|page| !page.page_text.chars.is_empty())
            })
    });
    let materialize_selection = std::rc::Rc::new(move |tint, open_note: bool| {
        let Some(drag) = selection.get() else {
            return;
        };
        if !drag.is_drag() {
            return;
        }
        let Some(rendered) = rendered_pages
            .get()
            .get(drag.page)
            .and_then(|slot| slot.get())
        else {
            return;
        };
        let page_text = rendered.page_text.clone();
        let scale = READING_WIDTH_PT * zoom.get() / rendered.points.0;
        if scale <= 0.0 {
            return;
        }
        let Some(book_id) = books
            .get()
            .iter()
            .find(|book| book.path == path.get())
            .map(|book| book.id.clone())
        else {
            return;
        };
        let root = liber_core::default_root();
        spawn(async move {
            let Some(highlight) = page_text.highlight(
                drag.page as u32,
                (drag.start_surface.0 / scale, drag.start_surface.1 / scale),
                (drag.end_surface.0 / scale, drag.end_surface.1 / scale),
            ) else {
                return;
            };
            let mut highlight = highlight;
            highlight.tint = tint;
            if open_note {
                note_comment.set(String::new());
                note_draft.set(Some(highlight));
            } else {
                let mut notes = liber_core::notes::Notes::open(&root, &book_id);
                notes.add(highlight);
                notes_snapshot.set(notes.items);
                notes_snapshot_book.set(Some(book_id));
                selection.set(None);
            }
        });
    });
    let save_note_draft = std::rc::Rc::new(move || {
        let Some(mut highlight) = note_draft.get() else {
            return;
        };
        let Some(book_id) = books
            .get()
            .iter()
            .find(|book| book.path == path.get())
            .map(|book| book.id.clone())
        else {
            return;
        };
        highlight.note = note_comment.get();
        let mut notes = liber_core::notes::Notes::open(&liber_core::default_root(), &book_id);
        notes.add(highlight);
        notes_snapshot.set(notes.items);
        notes_snapshot_book.set(Some(book_id));
        note_comment.set(String::new());
        note_draft.set(None);
        selection.set(None);
    });

    let pending_pages = signal(Vec::<usize>::new());
    let reader_pages = reader_pages::ReaderPages {
        rendered: rendered_pages,
        pending: pending_pages,
        path,
        count,
        zoom,
        tone,
        in_flight: Default::default(),
        last_visible: Default::default(),
        last_visible_change: std::rc::Rc::new(std::cell::Cell::new(std::time::Instant::now())),
        render_preview: |path, at, zoom, tone, cancel| {
            liber_core::render_preview_cancellable(path, at, zoom, tone, cancel)
        },
        render: |path, at, zoom, tone, cancel| {
            liber_core::render_cancellable(path, at, zoom, tone, cancel)
        },
        owner: Owner::current(),
    };
    let pages_for_view = reader_pages.clone();
    let request_page = std::rc::Rc::new(move |at: usize| reader_pages.request(at));
    let text_owner = Owner::current();
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
        text_owner.run(|| {
            spawn(async move {
                let page_path = source.clone();
                let result = blocking(move || extract_reading_blocks(page_path, at)).await;
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
            })
        });
    });
    let load_generation = signal(0u64);
    let preload_after_jump = request_page.clone();
    let load = std::rc::Rc::new(move |at: usize| {
        if let Some(book_id) = selected
            .get()
            .and_then(|index| books.get().get(index).map(|book| book.id.clone()))
            .filter(|book_id| !book_id.is_empty())
        {
            if notes_snapshot_book.get().as_deref() != Some(book_id.as_str()) {
                notes_snapshot.set(
                    liber_core::notes::Notes::open(&liber_core::default_root(), &book_id).items,
                );
                notes_snapshot_book.set(Some(book_id));
            }
        }
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
                    rendered_pages.set(cached.into_iter().map(signal).collect());
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

    if !sidebar::system() {
        set_title_bar(TitleBarStyle::Hidden, Some(px(40.0)));
    }
    let import_pdf = std::rc::Rc::new(move || {
        if let Some(file) = rfd::FileDialog::new()
            .add_filter("PDF", &["pdf"])
            .pick_file()
        {
            let source = file.to_path_buf();
            loading.set(true);
            load_generation.set(load_generation.get().wrapping_add(1));
            let selected_tone = tone.get();
            let books = books;
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
                    let rendered = if book.last_page == 0 {
                        if selected_tone == PageTone::Day {
                            first_page.or_else(|| {
                                render_with_tone(&book.path, 0, 1.0, selected_tone).ok()
                            })
                        } else {
                            render_with_tone(&book.path, 0, 1.0, selected_tone).ok()
                        }
                        .ok_or_else(|| "Не удалось отрисовать первую страницу".to_owned())?
                    } else {
                        render_with_tone(&book.path, book.last_page, 1.0, selected_tone)?
                    };
                    Ok((book, rendered))
                })
                .await;
                match result {
                    Ok(Ok((book, rendered))) => {
                        loading.set(false);
                        let mut list = books.get();
                        if !list.iter().any(|b| b.path == book.path) {
                            list.push(book.clone());
                        }
                        save_books(&list);
                        books.set(list);
                        open_reader_window(&book.path, &book.title);
                        notes_snapshot.set(
                            liber_core::notes::Notes::open(&liber_core::default_root(), &book.id)
                                .items,
                        );
                        notes_snapshot_book.set(Some(book.id.clone()));
                        path.set(book.path);
                        count.set(rendered.count);
                        page.set(book.last_page);
                        let mut cache = vec![None; rendered.count];
                        cache[book.last_page] = Some(rendered.clone());
                        rendered_pages.set(cache.into_iter().map(signal).collect());
                        reading_pages.set(vec![None; rendered.count]);
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
                    Ok(Err(message)) => {
                        loading.set(false);
                        error.set(message);
                    }
                    Err(message) => {
                        loading.set(false);
                        error.set(format!("Не удалось открыть PDF: {message}"));
                    }
                }
            });
        }
    });
    let sidebar_press: std::rc::Rc<dyn Fn(&str)> = {
        let import_pdf = import_pdf.clone();
        let reload = load.clone();
        std::rc::Rc::new(move |id: &str| match id {
            "home" => {
                selected.set(None);
                books.set(load_books());
                section.set(Page::Home);
            }
            "all" => {
                selected.set(None);
                books.set(load_books());
                section.set(Page::All);
            }
            "settings" => {
                selected.set(None);
                books.set(load_books());
                section.set(Page::Settings);
            }
            "theme" => {
                let next = !dark.get();
                dark.set(next);
                save_dark(next);
                if count.get() > 0 {
                    reload(page.get());
                }
            }
            "import" => import_pdf(),
            _ => {}
        })
    };
    let sidebar_caption = move || {
        format!(
            "Книг: {} · v{}",
            books.get().len(),
            env!("CARGO_PKG_VERSION")
        )
    };
    let sidebar_hidden = signal(false);
    #[cfg(target_os = "macos")]
    if reader_window {
        // The contents popover's picks and the toolbar's requests reach the reader here, a few times a second.
        nsh_core::ui::ambient::set_interval(std::time::Duration::from_millis(30), {
            let open_page = load_first.clone();
            let request_text = request_text_page.clone();
            move || {
                for page in imago_window_macos::reader_presses() {
                    reader_scroll_ref.scroll_to_index(
                        page as u32,
                        nsh_core::geometry::Percentage(0.0),
                        px(0.0),
                        true,
                    );
                    open_page(page);
                }
                for command in imago_window_macos::reader_commands() {
                    match command {
                        imago_window_macos::ReaderCommand::Zoom(up) => {
                            if text_mode.get() {
                                continue;
                            }
                            let next = if up {
                                (zoom.get() + 0.25).min(3.0)
                            } else {
                                (zoom.get() - 0.25).max(0.5)
                            };
                            if (next - zoom.get()).abs() > 0.001 {
                                zoom.set(next);
                                rendered_pages
                                    .set((0..count.get()).map(|_| signal(None)).collect());
                                pending_pages.set(Vec::new());
                                open_page(page.get());
                            }
                        }
                        imago_window_macos::ReaderCommand::Tone(index) => {
                            if let Some(next) = READER_TONES.get(index).copied() {
                                tone.set(next);
                                save_tone(next);
                                rendered_pages
                                    .set((0..count.get()).map(|_| signal(None)).collect());
                                pending_pages.set(Vec::new());
                                open_page(page.get());
                            }
                        }
                        imago_window_macos::ReaderCommand::TextMode => {
                            let enabled = !text_mode.get();
                            scroll_restore_page.set(page.get());
                            restore_page_pending.set(true);
                            text_mode.set(enabled);
                            if enabled {
                                request_text(page.get());
                            }
                        }
                        imago_window_macos::ReaderCommand::Search(query) => {
                            document_query.set(query.clone());
                            let generation = search_generation.get().wrapping_add(1);
                            search_generation.set(generation);
                            if query.is_empty() {
                                search_hits.set(Vec::new());
                                imago_window_macos::set_search_results(Vec::new());
                                continue;
                            }
                            let source = path.get();
                            spawn(async move {
                                let result = blocking(move || search_pdf(source, &query)).await;
                                if search_generation.get() != generation {
                                    return;
                                }
                                match result {
                                    Ok(Ok(found)) => {
                                        imago_window_macos::set_search_results(
                                            found
                                                .iter()
                                                .map(|hit| imago_window_macos::Contents {
                                                    title: hit.excerpt.clone(),
                                                    page: hit.page,
                                                    depth: 0,
                                                })
                                                .collect(),
                                        );
                                        search_hits.set(found);
                                        error.set(String::new());
                                    }
                                    Ok(Err(message)) => error.set(message),
                                    Err(message) => {
                                        error.set(format!("Не удалось выполнить поиск: {message}"))
                                    }
                                }
                            });
                        }
                    }
                }
                imago_window_macos::set_reader_state(imago_window_macos::ReaderState {
                    zoom_percent: (zoom.get() * 100.0).round() as u32,
                    zoom_enabled: !text_mode.get(),
                    tones: READER_TONES
                        .iter()
                        .map(|tone| tone.label().to_owned())
                        .collect(),
                    tone: READER_TONES
                        .iter()
                        .position(|candidate| *candidate == tone.get())
                        .unwrap_or(0),
                    text_mode: text_mode.get(),
                });
            }
        });
    }
    #[cfg(target_os = "macos")]
    if !reader_window && sidebar::system() {
        // The system sidebar's picks and visibility reach the app here, a few times a second.
        nsh_core::ui::ambient::set_interval(std::time::Duration::from_millis(30), {
            let press = sidebar_press.clone();
            move || {
                for id in imago_window_macos::sidebar_presses() {
                    press(&id);
                }
                let hidden = imago_window_macos::sidebar_collapsed();
                if sidebar_hidden.get() != hidden {
                    sidebar_hidden.set(hidden);
                }
            }
        });
    }
    view().flex_1().font_family(asset!("fonts/Inter.ttf").family).bg(bg).text_color(fg).child(
        safe_area_view().flex_1().child(
            view().flex_1().flex_col().bg(bg)
                .child(view().h(move || px(if sidebar::system() { 0.0 } else { 40.0 })).overflow_hidden().flex_none().flex_row().items_center()
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
                .child((!reader_window).then(|| {
                    let frame = sidebar_frame(selected, library_sidebar_open, dark);
                    if sidebar::native_registered() {
                        frame.child(native_sidebar(dark, section, selected, sidebar_caption, sidebar_press.clone()))
                    } else {
                        frame
                            .child(
                                button()
                                .on_press({ let press = sidebar_press.clone(); move || press("home") })
                                    .child(view().flex_row().items_center().gap_2().child(icon(asset!("icons/book-open.svg").uri, 16.0, dark)).child(text("Главная"))),
                            )
                            .child(
                                button()
                                .on_press({ let press = sidebar_press.clone(); move || press("all") })
                                    .child(view().flex_row().items_center().gap_2().child(icon(asset!("icons/book-open.svg").uri, 16.0, dark)).child(text("Все книги"))),
                            )
                            .child(
                                button()
                                .on_press({ let press = sidebar_press.clone(); move || press("settings") })
                                    .child(view().flex_row().items_center().gap_2().child(icon(asset!("icons/settings.svg").uri, 16.0, dark)).child(text("Настройки"))),
                            )
                            .child(
                                button()
                                .on_press({ let press = sidebar_press.clone(); move || press("theme") })
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
                                .on_press({ let press = sidebar_press.clone(); move || press("import") })
                                    .child(view().flex_row().items_center().gap_2().child(icon(asset!("icons/plus.svg").uri, 16.0, dark)).child(text("Добавить PDF")))
                            )
                            .child(text(sidebar_caption()).text_size(px(11.0)))
                    }
                }))
                .child(
                    view()
                        .flex_1()
                        .min_w_0()
                        .flex_col()
                        .child((!reader_window).then(||
                            view()
                                .h(px(LIBRARY_BAR_HEIGHT))
                                .flex_row()
                                .items_center()
                                .pl(move || px(if sidebar_hidden.get() { SPACE_28 + TITLE_CONTROLS_WIDTH } else { SPACE_28 }))
                                .pr(px(SPACE_28))
                                .gap_3()
                                .child(
                                    button()
                                        .on_press(move || {
                                            selected.set(None);
                                            books.set(load_books());
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
                        ))
                        .child(dynamic(move || {
                            let request_text_for_reader = request_text_page.clone();
                            let pages_for_reader = pages_for_view.clone();
                            #[cfg(target_os = "macos")]
                            if reader_window {
                                // The reader window's contents popover lists the open book's contents.
                                imago_window_macos::set_contents(
                                    outline
                                        .get()
                                        .into_iter()
                                        .map(|(title, page, depth)| imago_window_macos::Contents { title, page, depth })
                                        .collect(),
                                );
                            }
                            if let Some(i) = selected.get() {
                                let list = books.get();
                                if let Some(book) = list.get(i) {
                                    let previous_page = load_prev.clone();
                                    let next_page = load_next.clone();
                                    let highlights = notes_snapshot.get();
                                    #[cfg(target_os = "macos")]
                                    if reader_window {
                                        // The reader window's notes popover lists the highlights, like its contents.
                                        imago_window_macos::set_notes(
                                            highlights
                                                .iter()
                                                .map(|highlight| imago_window_macos::Contents {
                                                    title: highlight.text.clone(),
                                                    page: highlight.page as usize,
                                                    depth: 0,
                                                })
                                                .collect(),
                                        );
                                    }
                                    let panel_highlights = highlights.clone();
                                    let panel_open = notes_sidebar_open;
                                    let panel_dark = dark;
                                    let panel_scroll = reader_scroll_ref;
                                    let panel_jump = load_first.clone();
                                    let notes_panel = dynamic(move || {
                                        if !panel_open.get() {
                                            return view().w(px(0.0)).min_w(px(0.0)).into_element();
                                        }
                                        let mut note_entries = view().flex_col().gap_2();
                                        if panel_highlights.is_empty() {
                                            note_entries = note_entries.child(
                                                text("Заметок пока нет")
                                                    .text_size(px(13.0))
                                                    .text_color(move || rgb(if panel_dark.get() { LIBER_MUTED_DARK } else { LIBER_MUTED_LIGHT })),
                                            );
                                        }
                                        for highlight in panel_highlights.iter().cloned() {
                                            let jump = panel_jump.clone();
                                            let scroll_ref = panel_scroll;
                                            let at = highlight.page as usize;
                                            note_entries = note_entries.child(
                                                button().on_press(move || {
                                                    scroll_ref.scroll_to_index(
                                                        at as u32,
                                                        nsh_core::geometry::Percentage(0.0),
                                                        px(0.0),
                                                        true,
                                                    );
                                                    jump(at);
                                                }).child(
                                                    view()
                                                        .w_full()
                                                        .p(px(10.0))
                                                        .rounded(px(6.0))
                                                        .flex_col()
                                                        .gap_1()
                                                        .child(
                                                            text(format!("стр. {}", at + 1))
                                                                .text_size(px(11.0))
                                                                .text_color(move || rgb(if panel_dark.get() { LIBER_MUTED_DARK } else { LIBER_MUTED_LIGHT })),
                                                        )
                                                        .child(
                                                            text(highlight.text.clone())
                                                                .text_size(px(13.0))
                                                                .text_color(move || rgb(if panel_dark.get() { LIBER_TEXT_DARK } else { LIBER_INK })),
                                                        )
                                                        .child(
                                                            text(highlight.note.clone())
                                                                .text_size(px(12.0))
                                                                .text_color(move || rgb(if panel_dark.get() { LIBER_TEXT_DARK } else { LIBER_INK })),
                                                        ),
                                                ),
                                            );
                                        }
                                        view()
                                            .w(px(READER_PANEL_WIDTH))
                                            .min_w(px(0.0))
                                            .overflow_hidden()
                                            .bg(move || rgb(if panel_dark.get() { LIBER_SURFACE_DARK } else { LIBER_PANEL_LIGHT }))
                                            .flex_col()
                                            .child(view()
                                                .w(px(READER_PANEL_WIDTH))
                                                .flex_1()
                                                .flex_col()
                                                .p(px(20.0))
                                                .gap_3()
                                                .child(view()
                                                        .flex_row()
                                                        .items_center()
                                                        .justify_between()
                                                        .child(text("Заметки").text_size(px(16.0)).font_weight(FontWeight::SEMIBOLD))
                                                        .child(
                                                            button()
                                                                .on_press(move || {
                                                                    animate_layout(nsh_core::LayoutAnimation::EASE_IN_EASE_OUT);
                                                                    panel_open.set(false);
                                                                })
                                                                .child(icon(asset!("icons/status-x.svg").uri, 16.0, panel_dark)),
                                                        ))
                                                .child(scroll().flex_1().child(note_entries)))
                                            .into_element()
                                    });
            let bar_load = load_first.clone();
            let bar_request = request_text_for_reader.clone();
            let bar_previous = previous_page.clone();
            let bar_next = next_page.clone();
            let bar_title = book.title.clone();
            let bar_row = std::rc::Rc::new(move || {
                let bar_previous = bar_previous.clone();
                let bar_next = bar_next.clone();
                view()
                    .flex_row()
                    .items_center()
                    .px(px(SPACE_20))
                    .pl(px(SPACE_20))
                    .gap_3()
                                                .child((!reader_window).then(|| view().window_drag(false).child(button().on_press(move || {
                                                    animate_layout(nsh_core::LayoutAnimation::EASE_IN_EASE_OUT);
                                                    outline_sidebar_open.set(!outline_sidebar_open.get());
                                                }).child(icon(asset!("icons/sidebar-left.svg").uri, 16.0, dark)))))
                                                .child((!reader_window).then(|| button()
                                                        .on_press(move || {
                                                            selected.set(None);
                                                            books.set(load_books());
                                                        })
                                                        .child(view().flex_row().items_center().gap_2().child(icon(asset!("icons/arrow-left.svg").uri, 16.0, dark)).child(text("Библиотека")))))
                                                .child((!reader_window).then(|| text(bar_title.clone()).text_size(px(14.0))))
                                                .child(view().flex_1())
                                                .child(button().on_press({
                                                    let reopen = bar_load.clone();
                                                    move || {
                                                        let next = tone.get().next();
                                                        tone.set(next);
                                                        save_tone(next);
                                                        rendered_pages.set((0..count.get()).map(|_| signal(None)).collect());
                                                        pending_pages.set(Vec::new());
                                                        reopen(page.get());
                                                    }
                                                }).child(text(move || tone.get().label())))
                                                .child(button().disabled(move || text_mode.get()).on_press({
                                                    let reopen = bar_load.clone();
                                                    move || {
                                                        let next = (zoom.get() - 0.25).max(0.5);
                                                        if (next - zoom.get()).abs() > 0.001 {
                                                            zoom.set(next);
                                                            rendered_pages.set((0..count.get()).map(|_| signal(None)).collect());
                                                            pending_pages.set(Vec::new());
                                                            reopen(page.get());
                                                        }
                                                    }
                                                }).child(text("−")))
                                                .child(when(move || !text_mode.get(), move || text(move || format!("{}%", (zoom.get() * 100.0).round() as u32)).text_size(px(12.0))))
                                                .child(button().disabled(move || text_mode.get()).on_press({
                                                    let reopen = bar_load.clone();
                                                    move || {
                                                        let next = (zoom.get() + 0.25).min(3.0);
                                                        if (next - zoom.get()).abs() > 0.001 {
                                                            zoom.set(next);
                                                            rendered_pages.set((0..count.get()).map(|_| signal(None)).collect());
                                                            pending_pages.set(Vec::new());
                                                            reopen(page.get());
                                                        }
                                                    }
                                                }).child(text("+")))
                                                .child(
                                                    button()
                                                        .on_press({
                                                            let request = bar_request.clone();
                                                            move || {
                                                                let enabled = !text_mode.get();
                                                                scroll_restore_page.set(page.get());
                                                                restore_page_pending.set(true);
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
                                                                bar_previous(at - 1);
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
                                                                bar_next(at + 1);
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
                                                )
                                                .child(button().on_press(move || {
                                                    animate_layout(nsh_core::LayoutAnimation::EASE_IN_EASE_OUT);
                                                    notes_sidebar_open.set(!notes_sidebar_open.get());
                                                }).child(icon(asset!("icons/inbox.svg").uri, 16.0, dark)))
            });
                                    if let Some(message) = reader_error.get() {
                                        return view()
                                            .flex_1()
                                            .items_center()
                                            .justify_center()
                                            .child(text(message))
                                            .into_element();
                                    }
                                    let note_action_enabled = note_action_enabled.clone();
                                    let materialize_selection = materialize_selection.clone();
                                    return view()
                                        .flex_1()
                                        .relative()
                                        .flex_col()
                                        .bg(move || rgb(reader_tone_color(tone.get())))
                                        .child(reader_window.then(|| view()
                                            .absolute()
                                            .top(px(0.0))
                                            .w_full()
                                            .z_index(1)
                                            .h(px(READER_DIM_HEIGHT))
                                            .bg_linear_to_b()
                                            .gradient_from(move || rgb(reader_tone_color(tone.get())))
                                            .gradient_via(move || rgb(reader_tone_color(tone.get())))
                                            .gradient_via_position(Percentage(READER_DIM_SOLID))
                                            .window_drag(true)))
                                        .child(view()
                                                .h(move || px(if reader_window { 0.0 } else { READER_BAR_HEIGHT }))
                                                .overflow_hidden()
                                                .child((*bar_row)()))
                                        .child(
                                            view()
                                                .flex_1()
                                                .flex_row_reverse()
                                                .min_h_0()
                                                .child(notes_panel)
                                                .child(dynamic({
                                                    let record_page = record_page.clone();
                                                    move || {
                                                    let note_action_enabled = note_action_enabled.clone();
                                                    let materialize_selection = materialize_selection.clone();
                                                    if text_mode.get() {
                                                        virtual_list(move || count.get(), move |position| {
                                                            let position = position;
                                                            dynamic(move || {
                                                                let index = position.index();
                                                                let blocks = reading_pages.get().get(index).cloned().flatten();
                                                                let Some(blocks) = blocks else {
                                                                    return view().w_full().h(px(220.0)).items_center().justify_center()
                                                                        .child(text("Загрузка текста…").text_color(rgb(tone.get().palette().muted_ink))).into_element();
                                                                };
                                                                let mut content = view().w_full().max_w(px(700.0)).flex().flex_col().gap(px(18.0));
                                                                for paragraph in blocks {
                                                                    let code = paragraph.code;
                                                                    let paragraph_text = paragraph.text;
                                                                    let syntax = paragraph.syntax;
                                                                    let mut block = text(if code { String::new() } else { paragraph_text.clone() })
                                                                        .font_family(if code { "Menlo" } else { "Georgia" })
                                                                        .text_size(px(if code { 16.0 } else { 19.0 }))
                                                                        .line_height(px(if code { 25.0 } else { 31.0 }))
                                                                        .app_class(if code { Some("whitespace-nowrap".into()) } else { None })
                                                                        .selectable(true)
                                                                        .text_color(move || rgb(tone.get().palette().ink))
                                                                        .selection_color(rgb(tone.get().palette().accent));
                                                                    if code {
                                                                        let mut cursor = 0;
                                                                        for span in syntax {
                                                                            if span.start < cursor
                                                                                || span.end > paragraph_text.len()
                                                                                || !paragraph_text.is_char_boundary(span.start)
                                                                                || !paragraph_text.is_char_boundary(span.end)
                                                                            {
                                                                                continue;
                                                                            }
                                                                            if cursor < span.start {
                                                                                block = block.child(text(paragraph_text[cursor..span.start].to_owned())
                                                                                    .app_class("whitespace-nowrap"));
                                                                            }
                                                                            block = block.child(text(paragraph_text[span.start..span.end].to_owned())
                                                                                .app_class("whitespace-nowrap")
                                                                                .text_color(rgb(tone.get().palette().code_ink(span.token))));
                                                                            cursor = span.end;
                                                                        }
                                                                        if cursor < paragraph_text.len() {
                                                                            block = block.child(text(paragraph_text[cursor..].to_owned())
                                                                                .app_class("whitespace-nowrap"));
                                                                        }
                                                                    }
                                                                    content = content.child(
                                                                        view().w_full()
                                                                            .when(code, |block| block
                                                                                .min_w_0()
                                                                                .app_class("overflow-x-auto")
                                                                                .bg(rgb(tone.get().palette().code_paper))
                                                                                .p(px(12.0)))
                                                                            .child(block),
                                                                    );
                                                                }
                                                                view().w_full().flex_row().justify_center().py(px(SPACE_36)).selection_area(true)
                                                                    .child(content).into_element()
                                                            })
                                                        })
                                                        .estimated_item_size(px(700.0))
                                                        .initial_scroll_index(scroll_restore_page.get())
                                                        .element_ref(&reader_scroll_ref)
                                                        .on_viewable_items_change({
                                                            let request = request_text_for_reader.clone();
                                                            let record_page = record_page.clone();
                                                            let restore_pending = restore_page_pending;
                                                            let restore_page = scroll_restore_page;
                                                            move |items: &ViewableItems| {
                                                                if let Some((first, last)) = items.viewable {
                                                                    if restore_pending.get() {
                                                                        if first != restore_page.get() { return; }
                                                                        restore_pending.set(false);
                                                                    }
                                                                    record_page(first);
                                                                    for index in first.saturating_sub(1)..=last.saturating_add(1).min(count.get().saturating_sub(1)) {
                                                                        request(index);
                                                                    }
                                                                }
                                                            }
                                                        })
                                                        .flex_1()
                                                        .into_element()
                                                    } else {
                                                        scroll().flex_1().w_full().horizontal().center_content(true)
                                                            .on_layout(move |bounds: &Bounds<Points>| {
                                                                let width = bounds.size.width.0;
                                                                if (reader_viewport_width.get() - width).abs() > 1.0 {
                                                                    reader_viewport_width.set(width);
                                                                }
                                                            })
                                                            .pinch_value(&gesture_zoom)
                                                            .on_pinch_end({
                                                                let gesture_zoom = gesture_zoom;
                                                                let zoom = zoom;
                                                                move |event: &GestureEvent| {
                                                                    zoom.set((zoom.get() * event.value).clamp(0.5, 3.0));
                                                                    gesture_zoom.set(1.0);
                                                                }
                                                            })
                                                            .child(
                                                            view().bind_scale(&gesture_zoom).w_full().min_w(px(680.0 * zoom.get())).flex_row().justify_center().child(
                                                        virtual_list(move || count.get(), move |position| {
                                                            let position = position;
                                                            let note_action_enabled = note_action_enabled.clone();
                                                            let materialize_selection = materialize_selection.clone();
                                                            dynamic(move || {
                                                                let index = position.index();
                                                                let rendered = rendered_pages.get().get(index).and_then(|slot| slot.get());
                                                                let Some(rendered) = rendered else {
                                                                    let scale = zoom.get();
                                                                    let viewport_width = reader_viewport_width.get();
                                                                    let display_width = 680.0 * scale;
                                                                    let row_width = viewport_width.max(display_width);
                                                                    let left_inset = ((viewport_width - display_width) / 2.0).max(0.0);
                                                                    return view().relative().w(px(row_width)).h(px(980.0 * scale)).child(
                                                                        view().absolute().left(px(left_inset)).top(px(0.0))
                                                                            .w(px(display_width)).h(px(980.0 * scale))
                                                                            .items_center().justify_center()
                                                                            .child(text("Загрузка страницы…").text_color(rgb(LIBER_MUTED_LIGHT)))
                                                                    ).into_element();
                                                                };
                                                                let (width, height) = rendered.size;
                                                                if width == 0 { return view().h(px(980.0)).into_element(); }
                                                                // MuPDF text geometry uses page points; the tiles are rendered at device density for this width.
                                                                let display_width = READING_WIDTH_PT * zoom.get();
                                                                let viewport_width = reader_viewport_width.get();
                                                                let row_width = viewport_width.max(display_width);
                                                                let left_inset = ((viewport_width - display_width) / 2.0).max(0.0);
                                                                let scale = display_width / rendered.points.0;
                                                                let image_height = display_width * height as f32 / width as f32;
                                                                let saved_highlights = notes_snapshot.get();
                                                                let mut highlight_overlay = view().absolute().inset_0()
                                                                    .pointer_events(PointerEvents::None);
                                                                for highlight in saved_highlights.iter().filter(|highlight| highlight.page == index as u32) {
                                                                    for rect in &highlight.rects {
                                                                        highlight_overlay = highlight_overlay.child(view().absolute()
                                                                            .left(px(rect[0] * scale))
                                                                            .top(px(rect[1] * scale))
                                                                            .w(px((rect[2] - rect[0]) * scale))
                                                                            .h(px((rect[3] - rect[1]) * scale))
                                                                            .bg(rgb(highlight.tint.hex()).opacity(0.28)));
                                                                    }
                                                                }
                                                                if let Some(drag) = selection.get().filter(|drag| {
                                                                    drag.page == index && drag.is_drag()
                                                                }) {
                                                                    let start = rendered.page_text.hit(
                                                                        drag.start_surface.0 / scale,
                                                                        drag.start_surface.1 / scale,
                                                                    );
                                                                    let end = rendered.page_text.hit(
                                                                        drag.end_surface.0 / scale,
                                                                        drag.end_surface.1 / scale,
                                                                    );
                                                                    if let (Some(start), Some(end)) = (start, end) {
                                                                        for rect in rendered.page_text.rects(start, end) {
                                                                            highlight_overlay = highlight_overlay.child(view().absolute()
                                                                                .left(px(rect[0] * scale))
                                                                                .top(px(rect[1] * scale))
                                                                                .w(px((rect[2] - rect[0]) * scale))
                                                                                .h(px((rect[3] - rect[1]) * scale))
                                                                                .bg(rgb(LIBER_ACCENT).opacity(0.28)));
                                                                        }
                                                                    }
                                                                }
                                                                let body = view().relative().w(px(display_width)).h(px(image_height));
                                                                #[cfg(target_os = "macos")]
                                                                let body = {
                                                                    let selection_text = selection.get()
                                                                        .filter(|drag| drag.page == index && drag.is_drag())
                                                                        .and_then(|drag| {
                                                                            let start = rendered.page_text.hit(
                                                                                drag.start_surface.0 / scale,
                                                                                drag.start_surface.1 / scale,
                                                                            )?;
                                                                            let end = rendered.page_text.hit(
                                                                                drag.end_surface.0 / scale,
                                                                                drag.end_surface.1 / scale,
                                                                            )?;
                                                                            Some(rendered.page_text.text(start, end))
                                                                        })
                                                                        .unwrap_or_default();
                                                                    let tiles: Vec<JsonValue> = rendered
                                                                        .tiles
                                                                        .iter()
                                                                        .map(|tile| {
                                                                            json!({
                                                                                "png": tile.png,
                                                                                "x": tile.x as f32 / tile.scale,
                                                                                "y": tile.y as f32 / tile.scale,
                                                                                "width": tile.width as f32 / tile.scale,
                                                                                "height": tile.height as f32 / tile.scale,
                                                                            })
                                                                        })
                                                                        .collect();
                                                                    let event_selection = selection;
                                                                    let event_materialize = materialize_selection.clone();
                                                                    let native_page = nsh_core::ui::prelude::native(page_view::NAME)
                                                                            .absolute()
                                                                            .inset_0()
                                                                            .prop("preview", json!(rendered.png))
                                                                            .prop("size", json!([rendered.points.0, rendered.points.1]))
                                                                            .prop("tiles", JsonValue::Array(tiles))
                                                                            .prop("menuEnabled", json!(note_action_enabled()))
                                                                            .prop("selectionText", json!(selection_text));
                                                                    let native_page = native_page.on_native_event(move |event: &NativeEvent| {
                                                                        let point = || {
                                                                            let values = event.data.as_array()?;
                                                                            Some((values.first()?.as_f64()? as f32, values.get(1)?.as_f64()? as f32))
                                                                        };
                                                                        match event.name.as_ref() {
                                                                            "selectionStart" => {
                                                                                if let Some((x, y)) = point() {
                                                                                    event_selection.set(Some(SelectionDrag {
                                                                                        page: index,
                                                                                        start_surface: (x, y),
                                                                                        end_surface: (x, y),
                                                                                    }));
                                                                                }
                                                                            }
                                                                            "selectionMove" | "selectionEnd" => {
                                                                                if let (Some((x, y)), Some(mut drag)) = (point(), event_selection.get()) {
                                                                                    if drag.page == index {
                                                                                        drag.end_surface = (x, y);
                                                                                        event_selection.set(Some(drag));
                                                                                    }
                                                                                }
                                                                            }
                                                                            "selectionAction" => match event.data.as_str() {
                                                                                Some("yellow") => event_materialize(liber_core::notes::Tint::Yellow, false),
                                                                                Some("green") => event_materialize(liber_core::notes::Tint::Green, false),
                                                                                Some("blue") => event_materialize(liber_core::notes::Tint::Blue, false),
                                                                                Some("pink") => event_materialize(liber_core::notes::Tint::Pink, false),
                                                                                Some("note") => event_materialize(liber_core::notes::Tint::Yellow, true),
                                                                                _ => {}
                                                                            },
                                                                            _ => {}
                                                                        }
                                                                    });
                                                                    body.child(native_page)
                                                                };
                                                                #[cfg(not(target_os = "macos"))]
                                                                let body = {
                                                                    let mut tiles = view().absolute().inset_0();
                                                                    for tile in &rendered.tiles {
                                                                        let tile_scale = scale / tile.scale;
                                                                        tiles = tiles.child(view().absolute()
                                                                            .left(px(tile.x as f32 * tile_scale))
                                                                            .top(px(tile.y as f32 * tile_scale))
                                                                            .w(px(tile.width as f32 * tile_scale))
                                                                            .h(px(tile.height as f32 * tile_scale))
                                                                            .child(image(tile.png.clone()).w_full().h_full()));
                                                                    }
                                                                    body.child(image(rendered.png.clone()).w(px(display_width)).h(px(image_height))).child(tiles)
                                                                };
                                                                view().relative().w(px(row_width)).h(px(image_height))
                                                                    .child(body.absolute().left(px(left_inset)).top(px(0.0))
                                                                        .child(highlight_overlay))
                                                                    .into_element()
                                                            })
                                                        })
                                                        .estimated_item_size(move || px(980.0 * zoom.get()))
                                                        .row_gap(px(SPACE_22))
                                                        .content_padding_y(px(SPACE_22))
                                                        .initial_scroll_index(scroll_restore_page.get())
                                                        .element_ref(&reader_scroll_ref)
                                                        .on_viewable_items_change({
                                                            let pages = pages_for_reader.clone();
                                                            let record_page = record_page.clone();
                                                            let restore_pending = restore_page_pending;
                                                            let restore_page = scroll_restore_page;
                                                            move |items: &ViewableItems| {
                                                                if let Some((first, last)) = items.viewable {
                                                                    if restore_pending.get() {
                                                                        if first != restore_page.get() { return; }
                                                                        restore_pending.set(false);
                                                                    }
                                                                    record_page(first);
                                                                    pages.show(first, last);
                                                                }
                                                            }
                                                        })
                                                        .flex_1()
                                                            )
                                                        ).into_element()
                                                    }
                                                } }))
                                                .child({
                                                    let outline_panel_load = load_first.clone();
                                                    let outline_panel_outline = outline;
                                                    let outline_panel_hits = search_hits;
                                                    when(
                                                    move || !reader_window && !sidebar::system() && outline_sidebar_open.get(),
                                                    move || {
                                                        let mut toc_entries = view().flex_col().gap_1();
                                                        for (title, at, depth) in outline_panel_outline.get() {
                                                            let jump = outline_panel_load.clone();
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
                                                                }).child(
                                                                    view()
                                                                        .w_full()
                                                                        .py(px(8.0))
                                                                        .pl(px(12.0 + 14.0 * depth.min(8) as f32))
                                                                        .pr(px(12.0))
                                                                        .rounded(px(6.0))
                                                                        .child(
                                                                            text(title)
                                                                                .text_size(px(13.0))
                                                                                .font_weight(if depth == 0 { FontWeight::MEDIUM } else { FontWeight::NORMAL })
                                                                                .text_color(move || rgb(if dark.get() { LIBER_TEXT_DARK } else { LIBER_INK })),
                                                                        ),
                                                                ),
                                                            );
                                                        }
                                                        if outline_panel_outline.get().is_empty() {
                                                            toc_entries = toc_entries.child(
                                                                text("В документе нет оглавления")
                                                                    .text_size(px(13.0))
                                                                    .text_color(move || rgb(if dark.get() { LIBER_MUTED_DARK } else { LIBER_MUTED_LIGHT })),
                                                            );
                                                        }
                                                        let mut result_entries = view().flex_col().gap_1();
                                                        for hit in outline_panel_hits.get() {
                                                            let jump = outline_panel_load.clone();
                                                            let at = hit.page;
                                                            let scroll_ref = reader_scroll_ref;
                                                            result_entries = result_entries.child(
                                                                button().on_press(move || {
                                                                    scroll_ref.scroll_to_index(at as u32, nsh_core::geometry::Percentage(0.0), px(0.0), true);
                                                                    jump(at);
                                                                }).child(text(format!("{} · {}", at + 1, hit.excerpt)).text_size(px(11.0))),
                                                            );
                                                        }
                                                        view().w(px(READER_PANEL_WIDTH))
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
                                                            view()
                                                                .flex_row()
                                                                .items_center()
                                                                .justify_between()
                                                                .child(
                                                                    text("Содержание")
                                                                        .text_size(px(16.0))
                                                                        .font_weight(FontWeight::SEMIBOLD),
                                                                )
                                                                .child(
                                                                    button()
                                                                        .on_press(move || {
                                                                            animate_layout(nsh_core::LayoutAnimation::EASE_IN_EASE_OUT);
                                                                            outline_sidebar_open.set(false);
                                                                        })
                                                                        .child(icon(asset!("icons/status-x.svg").uri, 16.0, dark)),
                                                                ),
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
                                                    }
                                                )})
                                                .child({
                                                    let save_note_draft = save_note_draft.clone();
                                                    when(
                                                    move || note_draft.get().is_some(),
                                                    move || {
                                                        let save = save_note_draft.clone();
                                                        #[cfg(target_os = "macos")]
                                                        let cancel_button = nsh_core::ui::prelude::native(page_view::NOTE_BUTTON)
                                                            .w(px(92.0)).h(px(32.0))
                                                            .prop("title", json!("Отмена"))
                                                            .prop("action", json!("cancel"))
                                                            .on_native_event(move |event: &NativeEvent| {
                                                                if event.data.as_str() == Some("cancel") {
                                                                    note_draft.set(None);
                                                                }
                                                            });
                                                        #[cfg(not(target_os = "macos"))]
                                                        let cancel_button = button()
                                                            .on_press(move || note_draft.set(None))
                                                            .child(text("Отмена"));
                                                        #[cfg(target_os = "macos")]
                                                        let save_button = nsh_core::ui::prelude::native(page_view::NOTE_BUTTON)
                                                            .w(px(72.0)).h(px(32.0))
                                                            .prop("title", json!("OK"))
                                                            .prop("action", json!("save"))
                                                            .on_native_event(move |event: &NativeEvent| {
                                                                if event.data.as_str() == Some("save") {
                                                                    save();
                                                                }
                                                            });
                                                        #[cfg(not(target_os = "macos"))]
                                                        let save_button = button()
                                                            .on_press(move || save())
                                                            .child(text("OK"));
                                                        view()
                                                            .absolute()
                                                            .inset_0()
                                                            .z_index(10)
                                                            .items_center()
                                                            .justify_center()
                                                            .bg(rgb(0x000000).opacity(0.62))
                                                            .child(
                                                                view()
                                                                    .w(px(720.0))
                                                                    .h(px(480.0))
                                                                    .flex_col()
                                                                    .gap_4()
                                                                    .p(px(24.0))
                                                                    .rounded(px(18.0))
                                                                    .bg(move || rgb(if dark.get() { 0x1C1C1E } else { 0xF5F5F7 }))
                                                                    .child(text("Заметка").text_size(px(18.0)).font_weight(FontWeight::SEMIBOLD))
                                                                    .child(
                                                                        view()
                                                                            .flex_row()
                                                                            .gap_3()
                                                                            .p(px(12.0))
                                                                            .rounded(px(8.0))
                                                                            .bg(move || rgb(if dark.get() { 0x252529 } else { 0xE7E7EA }))
                                                                            .child(view().w(px(3.0)).rounded(px(2.0)).bg(move || rgb(note_draft.get().map_or(liber_core::notes::Tint::Yellow, |draft| draft.tint).hex())))
                                                                            .child(text(move || note_draft.get().map_or_else(String::new, |draft| draft.text)).text_size(px(14.0)).line_height(px(21.0)).text_color(rgb(LIBER_MUTED_DARK))),
                                                                    )
                                                                    .child(
                                                                        input()
                                                                            .multiline(true)
                                                                            .h(px(180.0))
                                                                            .value(move || note_comment.get())
                                                                            .placeholder("Добавить заметку…")
                                                                            .on_change(move |value: &str| note_comment.set(value.to_owned())),
                                                                    )
                                                                    .child(view().flex_1())
                                                                    .child(
                                                                        view().flex_row().justify_end().gap_2()
                                                                            .child(cancel_button)
                                                                            .child(save_button),
                                                                    ),
                                                            )
                                                    },
                                                )})
                                        ).into_element();
                                }
                            }
                            if section.get() == Page::Settings {
                                let press = sidebar_press.clone();
                                return view()
                                    .flex_1()
                                    .px(px(SPACE_32))
                                    .py(px(SPACE_24))
                                    .gap_6()
                                    .child(
                                        text("Настройки")
                                            .text_size(px(20.0))
                                            .font_weight(FontWeight::SEMIBOLD),
                                    )
                                    .child(
                                        button()
                                            .on_press(move || press("theme"))
                                            .child(text(move || if dark.get() { "Тема: тёмная" } else { "Тема: светлая" })),
                                    )
                                    .into_element();
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
                                let name = book.title.clone();
                                let reader_title = name.clone();
                                let author = book.author.clone();
                                let page_count = book.pages;
                                let last_page = book.last_page;
                                let progress = book.progress();
                                let cover = book.cover.clone();
                                let source = book.path.clone();
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
                                                    pages_signal.set((0..page_count).map(|_| signal(None)).collect());
                                                    reading_signal.set(vec![None; page_count]);
                                                    pending_signal.set(Vec::new());
                                                    pending_text_signal.set(Vec::new());
                                                    search_generation.set(search_generation.get().wrapping_add(1));
                                                    document_query.set(String::new());
                                                    search_hits.set(Vec::new());
                                                    open_reader_window(&source, &reader_title);
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
                                    text(if section.get() == Page::All { "Все книги" } else { "Читаю сейчас" })
                                        .text_size(px(20.0))
                                        .font_weight(FontWeight::SEMIBOLD),
                                )
                                .child(
                                    input()
                                        .value(move || query.get())
                                        .placeholder("Поиск по названию или автору")
                                        .on_change(move |value: &str| query.set(value.to_owned())),
                                )
                                .child(text(if section.get() == Page::All { "" } else { "Недавно добавленные" }).text_size(px(13.0)).text_color(
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
