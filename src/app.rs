//! Application shell: sidebar, library grid, import, and the reader screen.

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    actions, deferred, div, img, linear_color_stop, linear_gradient, px, uniform_list, App, AppContext, ClickEvent, Context,
    Entity, ExternalPaths, InteractiveElement, IntoElement, KeyBinding, KeyDownEvent, Menu, MenuItem, MouseButton,
    MouseDownEvent, ObjectFit, ParentElement, PathPromptOptions, Render, SharedString, StatefulInteractiveElement,
    Styled, StyledImage, Subscription, Window, WindowControlArea,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::Icon;
use imago_gpui::appearance_editor::AppearanceEditor;
use imago_gpui::button;
use imago_gpui::settings::UiStyle;
use imago_gpui::chrome::{self, sidebar_item};
use imago_gpui::theme::{c, rgba, ACCENT, BG, BORDER, DESTRUCTIVE, FG, MUTED_FG, POPOVER};

use crate::library::{self, Book, Imported, Library};
use crate::reader::{Reader, ReaderEvent};

actions!(liber, [Quit, Hide, Import, Search]);

const CARD_W: f32 = 160.0;
const COVER_H: f32 = 224.0;
const CARD_GAP: f32 = 24.0;
const ROW_H: f32 = 316.0;
/// Page gutter shared by every library page (title and content line up).
const PAD: f32 = 32.0;

pub fn install_menu(cx: &mut App) {
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-h", Hide, None),
        KeyBinding::new("cmd-o", Import, None),
        KeyBinding::new("cmd-k", Search, None),
    ]);
    cx.set_menus([
        Menu::new("Liber").items([
            MenuItem::action("Скрыть Liber", Hide),
            MenuItem::separator(),
            MenuItem::action("Завершить Liber", Quit),
        ]),
        Menu::new("Файл").items([MenuItem::action("Добавить книги…", Import), MenuItem::action("Поиск", Search)]),
    ]);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Filter {
    Home,
    All,
    Settings,
}

enum Screen {
    Library,
    Reader(Entity<Reader>),
}

pub struct Liber {
    library: Library,
    covers: HashSet<String>,
    screen: Screen,
    filter: Filter,
    search: Entity<InputState>,
    appearance: Entity<AppearanceEditor>,
    query: String,
    search_open: bool,
    /// Book context menu: id and window position.
    menu: Option<(String, f32, f32)>,
    /// Book shown in the details modal.
    detail: Option<String>,
    click_gen: u64,
    shown: Vec<String>,
    status: Option<SharedString>,
    importing: usize,
    save_gen: u64,
    _subs: Vec<Subscription>,
    // Kept separately: reader subscriptions are truncated on navigation.
    _window_chrome: crate::mac::WindowChrome,
}

impl Liber {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let window_chrome = crate::mac::WindowChrome::new(window);
        let library = Library::open(library::default_root());
        let covers = library
            .books()
            .iter()
            .filter(|b| library.cover_path(&b.id).exists())
            .map(|b| b.id.clone())
            .collect();
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Поиск по названию и автору"));
        let sub = cx.subscribe(&search, |this, state, ev: &InputEvent, cx| match ev {
            InputEvent::Change => {
                this.query = state.read(cx).value().to_string().to_lowercase();
                cx.notify();
            }
            InputEvent::PressEnter { .. } => {
                if let Some(id) = this.search_results().first().map(|b| b.id.clone()) {
                    this.show_detail(id, cx);
                }
            }
            _ => {}
        });
        let root = library.root().to_path_buf();
        let settings = Rc::new(RefCell::new(crate::appearance::load(&root)));
        let snapshot = crate::appearance::snapshot(&settings.borrow());
        let appearance = {
            cx.new(|cx| {
                let mut editor = AppearanceEditor::new(cx, move |patch, cx| {
                    let mut settings = settings.borrow_mut();
                    crate::appearance::merge(&mut settings, &patch);
                    crate::appearance::save(&root, &settings);
                    crate::appearance::apply(&settings, cx);
                    cx.refresh_windows();
                });
                editor.ingest(&snapshot);
                editor
            })
        };
        let status = library
            .recovered_from
            .as_ref()
            .map(|p| format!("Индекс библиотеки был повреждён, копия: {}", p.display()).into());
        Self {
            library,
            covers,
            screen: Screen::Library,
            filter: Filter::Home,
            search,
            appearance,
            query: String::new(),
            search_open: false,
            menu: None,
            detail: None,
            click_gen: 0,
            shown: Vec::new(),
            status,
            importing: 0,
            save_gen: 0,
            _subs: vec![sub],
            _window_chrome: window_chrome,
        }
    }

    // --- import ---------------------------------------------------------

    fn pick_files(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Добавить".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                let _ = this.update(cx, |this, cx| this.import(paths, cx));
            }
        })
        .detach();
    }

    fn import(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        for path in paths.into_iter().filter(|p| {
            p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
        }) {
            let root = self.library.root().to_path_buf();
            let known = self.library.hashes();
            self.importing += 1;
            let work = cx
                .background_executor()
                .spawn(async move { library::prepare_import(&root, &path, &known) });
            cx.spawn(async move |this, cx| {
                let outcome = work.await;
                let _ = this.update(cx, |this, cx| {
                    this.importing -= 1;
                    match outcome {
                        Ok(Imported::Added(book)) => {
                            let id = book.id.clone();
                            if this.library.cover_path(&id).exists() {
                                this.covers.insert(id.clone());
                            }
                            if this.library.add(book) {
                                this.lookup_meta(id, cx);
                            }
                        }
                        Ok(Imported::Duplicate(_)) => {
                            this.status = Some("Эта книга уже есть в библиотеке".into())
                        }
                        Err(e) => this.status = Some(format!("Не удалось добавить: {e}").into()),
                    }
                    cx.notify();
                });
            })
            .detach();
        }
        cx.notify();
    }

    // --- reader ---------------------------------------------------------

    fn open_book(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(book) = self.library.get(id) else { return };
        let (title, start) = (book.title.clone(), book.last_page);
        let path = self.library.book_path(id);
        self.library.mark_opened(id);
        let id = id.to_owned();
        let root = self.library.root().to_path_buf();
        let reader = cx.new(|cx| Reader::new(id, title, path, start, &root, window, cx));
        let sub = cx.subscribe_in(&reader, window, |this, reader, ev: &ReaderEvent, window, cx| match ev {
            ReaderEvent::Page(page) => {
                let id = reader.read(cx).book_id.clone();
                this.save_page_soon(id, *page, cx);
            }
            ReaderEvent::Close => this.close_reader(window, cx),
        });
        self._subs.truncate(1);
        self._subs.push(sub);
        self.screen = Screen::Reader(reader);
        cx.notify();
    }

    fn close_reader(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Screen::Reader(reader) = &self.screen {
            reader.update(cx, |reader, _| {
                reader.flush_view();
                reader.release_images(window);
            });
        }
        crate::mac::set_traffic_lights_alpha(window, 1.0);
        self._subs.truncate(1);
        self.screen = Screen::Library;
        cx.notify();
    }

    /// Persist reading progress a moment after scrolling settles.
    fn save_page_soon(&mut self, id: String, page: u32, cx: &mut Context<Self>) {
        self.save_gen += 1;
        let generation = self.save_gen;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(800)).await;
            let _ = this.update(cx, |this, _| {
                if this.save_gen == generation {
                    this.library.set_page(&id, page);
                }
            });
        })
        .detach();
    }

    // --- overlays ---------------------------------------------------------

    /// Fetch title/author/cover online in the background and apply a confident match.
    fn lookup_meta(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(book) = self.library.get(&id) else { return };
        let (title, author) = (book.title.clone(), book.author.clone());
        let work = cx.background_executor().spawn(async move { crate::meta::lookup(&title, &author) });
        cx.spawn(async move |this, cx| {
            let found = work.await;
            let _ = this.update(cx, |this, cx| {
                if this.library.get(&id).is_none() {
                    return;
                }
                if let Some(png) = found.as_ref().and_then(|f| f.cover.as_ref()) {
                    let path = library::online_cover_file(this.library.root(), &id);
                    if std::fs::write(&path, png).is_ok() {
                        this.covers.insert(id.clone());
                    }
                }
                this.library.set_meta(&id, found.map(|f| (f.title, f.author)));
                cx.notify();
            });
        })
        .detach();
    }

    fn close_overlays(&mut self, cx: &mut Context<Self>) {
        self.search_open = false;
        self.menu = None;
        self.detail = None;
        cx.notify();
    }

    fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        self.detail = None;
        self.search_open = true;
        self.search.update(cx, |s, cx| {
            s.set_value("", window, cx);
            s.focus(window, cx);
        });
        cx.notify();
    }

    fn show_detail(&mut self, id: String, cx: &mut Context<Self>) {
        self.search_open = false;
        self.menu = None;
        self.detail = Some(id);
        cx.notify();
    }

    fn delete_book(&mut self, id: &str, cx: &mut Context<Self>) {
        self.library.remove(id);
        self.covers.remove(id);
        self.close_overlays(cx);
    }

    /// Quick-search matches; with an empty query, the recently read books.
    fn search_results(&self) -> Vec<&Book> {
        let mut books: Vec<&Book> = self.library.books().iter().collect();
        if self.query.is_empty() {
            books.retain(|b| b.last_opened.is_some());
            books.sort_by_key(|b| std::cmp::Reverse(b.last_opened));
        } else {
            books.retain(|b| {
                b.title.to_lowercase().contains(&self.query) || b.author.to_lowercase().contains(&self.query)
            });
            books.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
        }
        books.truncate(7);
        books
    }

    // --- library view ---------------------------------------------------

    fn recompute_shown(&mut self) {
        let mut books: Vec<&Book> = self.library.books().iter().collect();
        books.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
        self.shown = books.into_iter().map(|b| b.id.clone()).collect();
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let item = |id: &'static str, icon: &'static str, label: &'static str, f: Filter, cx: &mut Context<Self>| {
            sidebar_item(id, Icon::empty().path(icon), label, self.filter == f).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.filter = f;
                    cx.notify();
                },
            ))
        };
        chrome::sidebar()
            .child(chrome::sidebar_titlebar().window_control_area(WindowControlArea::Drag))
            .child(
                chrome::sidebar_body()
                    .child(
                        sidebar_item("nav-search", Icon::empty().path("icons/search.svg"), "Поиск", false)
                            .on_click(cx.listener(|this, _, window, cx| this.open_search(window, cx))),
                    )
                    .child(div().h(px(8.)))
                    .child(item("nav-home", "icons/book-open.svg", "Главная", Filter::Home, cx))
                    .child(item("nav-all", "icons/folder.svg", "Все книги", Filter::All, cx))
                    .child(item("nav-settings", "icons/settings.svg", "Настройки", Filter::Settings, cx)),
            )
            .child(
                chrome::sidebar_footer()
                    .text_size(px(12.))
                    .text_color(rgba(FG(), 0.5))
                    .child(format!(
                        "Книг: {} · v{}",
                        self.library.books().len(),
                        env!("CARGO_PKG_VERSION")
                    )),
            )
    }

    /// Page heading inside the content area (the title bar stays empty).
    fn page_head(&self, title: &'static str, with_add: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let mut row = div()
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .px(px(PAD))
            .pt(px(8.))
            .pb(px(20.))
            .child(div().text_size(px(22.)).font_weight(gpui::FontWeight::SEMIBOLD).child(title));
        if with_add {
            row = row.child(
                button::secondary("import")
                    .icon(Icon::empty().path("icons/plus.svg"))
                    .label("Добавить")
                    .on_click(cx.listener(|this, _, _, cx| this.pick_files(cx))),
            );
        }
        row
    }

    /// A book cover with a hardback binding drawn over its left edge.
    fn cover(&self, book: &Book, w: f32, h: f32) -> gpui::Div {
        let art = if self.covers.contains(&book.id) {
            img(self.library.cover_path(&book.id)).size_full().object_fit(ObjectFit::Cover).into_any_element()
        } else {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(h / 5.))
                .text_color(rgba(FG(), 0.3))
                .child(book.title.chars().next().map(String::from).unwrap_or_default())
                .into_any_element()
        };
        let k = h / COVER_H;
        div()
            .relative()
            .flex_none()
            .w(px(w))
            .h(px(h))
            .rounded_r(px(5. * k))
            .rounded_l(px(2. * k))
            .overflow_hidden()
            .bg(rgba(FG(), 0.07))
            .shadow_lg()
            .child(art)
            // Spine shadow, hinge groove and highlight.
            .child(
                div().absolute().top_0().bottom_0().left_0().w(px(14. * k)).bg(linear_gradient(
                    90.,
                    linear_color_stop(rgba(0x000000, 0.32), 0.),
                    linear_color_stop(rgba(0x000000, 0.0), 1.),
                )),
            )
            .child(div().absolute().top_0().bottom_0().left(px(6. * k)).w(px(1.)).bg(rgba(0x000000, 0.28)))
            .child(div().absolute().top_0().bottom_0().left(px(7. * k)).w(px(1.)).bg(rgba(0xffffff, 0.20)))
            // Soft gloss from the top.
            .child(div().absolute().inset_0().bg(linear_gradient(
                180.,
                linear_color_stop(rgba(0xffffff, 0.10), 0.),
                linear_color_stop(rgba(0xffffff, 0.0), 0.5),
            )))
    }

    fn percent(book: &Book) -> String {
        format!("{}%", (book.progress() * 100.0).round() as u32)
    }

    fn card(&self, book: &Book, cx: &mut Context<Self>) -> impl IntoElement {
        let (click_id, menu_id) = (book.id.clone(), book.id.clone());
        div()
            .id(SharedString::from(format!("book-{}", book.id)))
            .role(gpui::Role::Button)
            .aria_label(book.title.clone())
            .w(px(CARD_W))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(10.))
            .cursor_pointer()
            .child(self.cover(book, CARD_W, COVER_H))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(13.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(book.title.clone()),
                    )
                    .child(div().text_size(px(12.)).text_color(c(MUTED_FG())).child(Self::percent(book))),
            )
            .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                this.menu = None;
                this.click_gen += 1;
                if ev.click_count() >= 2 {
                    this.open_book(&click_id, window, cx);
                    return;
                }
                // A single click waits briefly in case it becomes a double click.
                let (generation, id) = (this.click_gen, click_id.clone());
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(Duration::from_millis(250)).await;
                    let _ = this.update(cx, |this, cx| {
                        if this.click_gen == generation {
                            this.show_detail(id, cx);
                        }
                    });
                })
                .detach();
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                    this.click_gen += 1;
                    this.menu = Some((menu_id.clone(), f32::from(ev.position.x), f32::from(ev.position.y)));
                    cx.notify();
                }),
            )
    }

    /// Apple Books-style home: "Читаю сейчас" and "Недавно добавленные" shelves.
    fn home_body(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut reading: Vec<&Book> = self
            .library
            .books()
            .iter()
            .filter(|b| b.last_opened.is_some() && b.progress() < 0.999)
            .collect();
        reading.sort_by_key(|b| std::cmp::Reverse(b.last_opened));
        let mut added: Vec<&Book> = self.library.books().iter().collect();
        added.sort_by_key(|b| std::cmp::Reverse(b.added_at));

        let mut page = div()
            .id("home")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(PAD))
            .pb_6()
            .flex()
            .flex_col()
            .gap_6();
        for (title, books) in [("Читаю сейчас", reading), ("Недавно добавленные", added)] {
            if books.is_empty() {
                continue;
            }
            let mut shelf = div().flex().flex_wrap().gap(px(CARD_GAP));
            for b in books.into_iter().take(12) {
                shelf = shelf.child(self.card(b, cx));
            }
            page = page.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .text_size(px(15.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(rgba(FG(), 0.85))
                            .child(title),
                    )
                    .child(shelf),
            );
        }
        page
    }

    fn library_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.recompute_shown();
        let sidebar_w = imago_gpui::chrome::SIDEBAR_W;
        let avail = f32::from(window.viewport_size().width) - sidebar_w - 2.0 * PAD;
        let cols = (((avail + CARD_GAP) / (CARD_W + CARD_GAP)).floor() as usize).max(1);
        let rows = self.shown.len().div_ceil(cols);
        let empty = self.library.books().is_empty();

        let (title, with_add) = match self.filter {
            Filter::Home => ("Главная", true),
            Filter::All => ("Все книги", true),
            Filter::Settings => ("", false),
        };
        let body = if self.filter == Filter::Settings {
            self.appearance
                .update(cx, |editor, _| editor.configure(UiStyle::default(), false, true));
            div()
                .id("settings")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(
                    div()
                        .mx_auto()
                        .w_full()
                        .max_w(px(imago_gpui::settings::PAGE_WIDTH))
                        .px(px(PAD))
                        .pb_6()
                        .child(div().pt(px(8.)).pb(px(20.)).text_size(px(22.)).font_weight(gpui::FontWeight::SEMIBOLD).child("Настройки"))
                        .child(self.appearance.clone()),
                )
                .into_any_element()
        } else if empty {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(14.))
                .text_color(rgba(FG(), 0.5))
                .child("Перетащите PDF сюда или нажмите «Добавить»")
                .into_any_element()
        } else if self.filter == Filter::Home {
            self.home_body(cx).into_any_element()
        } else {
            uniform_list(
                "books",
                rows,
                cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                    range
                        .map(|row| {
                            let ids: Vec<String> = this.shown.iter().skip(row * cols).take(cols).cloned().collect();
                            let mut line = div().h(px(ROW_H)).flex().gap(px(CARD_GAP)).px(px(PAD)).items_start();
                            for id in ids {
                                if let Some(book) = this.library.get(&id).cloned() {
                                    line = line.child(this.card(&book, cx));
                                }
                            }
                            line
                        })
                        .collect()
                }),
            )
            .flex_1()
            .into_any_element()
        };

        let mut col = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .bg(c(BG()))
            .child(chrome::titlebar().window_control_area(WindowControlArea::Drag));
        if self.filter != Filter::Settings {
            col = col.child(self.page_head(title, with_add, cx));
        }
        if let Some(status) = self.status.clone() {
            col = col.child(
                div()
                    .id("status")
                    .mx(px(PAD))
                    .mb_2()
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .bg(rgba(FG(), 0.08))
                    .text_size(px(12.))
                    .cursor_pointer()
                    .child(status)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.status = None;
                        cx.notify();
                    })),
            );
        }
        if self.importing > 0 {
            col = col.child(
                div()
                    .px(px(PAD))
                    .pb_2()
                    .text_size(px(12.))
                    .text_color(rgba(FG(), 0.55))
                    .child(format!("Добавляю книг: {}…", self.importing)),
            );
        }
        div()
            .size_full()
            .relative()
            .flex()
            .child(self.sidebar(cx))
            .child(col.child(body))
            .children(self.overlays(window, cx))
    }

    fn overlays(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let mut out = Vec::new();
        if self.search_open {
            out.push(self.search_overlay(cx).into_any_element());
        }
        if let Some(id) = self.detail.clone() {
            if let Some(book) = self.library.get(&id).cloned() {
                out.push(self.detail_overlay(&book, cx).into_any_element());
            }
        }
        if let Some((id, x, y)) = self.menu.clone() {
            out.push(self.menu_overlay(&id, x, y, window, cx).into_any_element());
        }
        out
    }

    fn backdrop(&self, id: &'static str, dim: f32, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        div().id(id).absolute().inset_0().bg(rgba(0x000000, dim)).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, _, cx| {
                this.close_overlays(cx);
                cx.stop_propagation();
            }),
        )
    }

    fn panel(&self) -> gpui::Div {
        div()
            .flex()
            .flex_col()
            .rounded(px(14.))
            .border_1()
            .border_color(c(BORDER()))
            .bg(c(POPOVER()))
            .shadow_lg()
            // Presses inside must not reach the dismissing backdrop.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
    }

    fn search_overlay(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let results: Vec<Book> = self.search_results().into_iter().cloned().collect();
        let mut list = div().flex().flex_col().p_1();
        if results.is_empty() {
            list = list.child(
                div()
                    .h(px(64.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(13.))
                    .text_color(c(MUTED_FG()))
                    .child(if self.query.is_empty() { "Начните вводить название или автора" } else { "Ничего не найдено" }),
            );
        }
        for book in results {
            let id = book.id.clone();
            list = list.child(
                div()
                    .id(SharedString::from(format!("sr-{}", book.id)))
                    .h(px(48.))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_3()
                    .rounded(px(8.))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgba(FG(), 0.07)))
                    .child(self.cover(&book, 26., 36.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_size(px(13.))
                                    .child(book.title.clone()),
                            )
                            .child(
                                div()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_size(px(12.))
                                    .text_color(c(MUTED_FG()))
                                    .child(if book.author.is_empty() { format!("{} стр.", book.pages) } else { book.author.clone() }),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.show_detail(id.clone(), cx))),
            );
        }
        div()
            .absolute()
            .inset_0()
            .child(self.backdrop("search-backdrop", 0.35, cx))
            .child(
                div().absolute().top(px(88.)).left_0().right_0().flex().justify_center().child(
                    self.panel()
                        .w(px(560.))
                        .overflow_hidden()
                        .child(
                            div()
                                .flex_none()
                                .p_2()
                                .border_b_1()
                                .border_color(c(BORDER()))
                                .child(Input::new(&self.search).cleanable(true)),
                        )
                        .child(list),
                ),
            )
    }

    fn detail_overlay(&mut self, book: &Book, cx: &mut Context<Self>) -> impl IntoElement {
        let id = book.id.clone();
        let progress = book.progress().clamp(0.0, 1.0);
        let bar = div()
            .h(px(4.))
            .w_full()
            .rounded_full()
            .bg(rgba(FG(), 0.12))
            .child(div().h_full().w(gpui::relative(progress)).rounded_full().bg(c(ACCENT())));
        let mut info = div().flex_1().min_w_0().flex().flex_col().gap(px(6.)).child(
            div().font_family("Georgia").text_size(px(22.)).line_height(px(27.)).font_weight(gpui::FontWeight::SEMIBOLD).child(book.title.clone()),
        );
        if !book.author.is_empty() {
            info = info.child(div().text_size(px(14.)).text_color(rgba(FG(), 0.75)).child(book.author.clone()));
        }
        info = info
            .child(div().text_size(px(13.)).text_color(c(MUTED_FG())).child(format!("{} стр.", book.pages)))
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(bar)
                    .child(div().text_size(px(12.)).text_color(c(MUTED_FG())).child(if book.last_opened.is_none() {
                        "Ещё не открыта".to_string()
                    } else {
                        format!("Прочитано {} · стр. {} из {}", Self::percent(book), book.last_page + 1, book.pages)
                    }))
                    .child(
                        div().pt(px(6.)).child(
                            button::primary("read")
                                .label("Читать")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.close_overlays(cx);
                                    this.open_book(&id, window, cx);
                                })),
                        ),
                    ),
            );
        div()
            .absolute()
            .inset_0()
            .child(self.backdrop("detail-backdrop", 0.45, cx))
            .child(
                div().absolute().inset_0().flex().items_center().justify_center().child(
                    self.panel()
                        .w(px(680.))
                        .overflow_hidden()
                        // The modal's own title bar: round close button at the left, as in Apple Books.
                        .child(
                            div().flex_none().h(px(56.)).px(px(14.)).flex().items_center().child(
                                div()
                                    .id("detail-close")
                                    .role(gpui::Role::Button)
                                    .aria_label("Закрыть")
                                    .size(px(32.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_full()
                                    .cursor_pointer()
                                    .bg(rgba(FG(), 0.10))
                                    .hover(|s| s.bg(rgba(FG(), 0.18)))
                                    .child(gpui::svg().path("icons/status-x.svg").size(px(14.)).text_color(rgba(FG(), 0.8)))
                                    .on_click(cx.listener(|this, _, _, cx| this.close_overlays(cx))),
                            ),
                        )
                        .child(
                            div()
                                .flex()
                                .gap(px(28.))
                                .px(px(32.))
                                .pt(px(8.))
                                .pb(px(32.))
                                .child(self.cover(book, 180., 252.))
                                .child(info),
                        ),
                ),
            )
    }

    fn menu_overlay(&mut self, id: &str, x: f32, y: f32, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        let (x, y) = (
            x.min(f32::from(viewport.width) - 188.).max(8.),
            y.min(f32::from(viewport.height) - 140.).max(8.),
        );
        let row = |key: &'static str, label: &'static str, destructive: bool| {
            div()
                .id(key)
                .h_7()
                .w_full()
                .px_2()
                .flex()
                .items_center()
                .rounded(px(5.))
                .cursor_pointer()
                .text_size(px(13.))
                .text_color(if destructive { c(DESTRUCTIVE()) } else { c(FG()) })
                .hover(|s| s.bg(rgba(FG(), 0.07)))
                .child(label)
        };
        let (read_id, info_id, del_id, meta_id) = (id.to_owned(), id.to_owned(), id.to_owned(), id.to_owned());
        let menu = div()
            .absolute()
            .left(px(x))
            .top(px(y))
            .min_w(px(180.))
            .p_1()
            .flex()
            .flex_col()
            .rounded_lg()
            .border_1()
            .border_color(c(BORDER()))
            .bg(c(POPOVER()))
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(row("menu-read", "Читать", false).on_click(cx.listener(move |this, _, window, cx| {
                this.close_overlays(cx);
                this.open_book(&read_id, window, cx);
            })))
            .child(row("menu-info", "Подробнее", false).on_click(cx.listener(move |this, _, _, cx| {
                this.show_detail(info_id.clone(), cx);
            })))
            .child(row("menu-meta", "Обновить метаданные", false).on_click(cx.listener(move |this, _, _, cx| {
                this.close_overlays(cx);
                this.lookup_meta(meta_id.clone(), cx);
            })))
            .child(div().h(px(1.)).my_1().bg(c(BORDER())))
            .child(row("menu-delete", "Удалить", true).on_click(cx.listener(move |this, _, _, cx| {
                this.delete_book(&del_id, cx);
            })));
        div()
            .absolute()
            .inset_0()
            .child(
                div()
                    .id("menu-backdrop")
                    .size_full()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, _, cx| {
                            this.menu = None;
                            cx.notify();
                            cx.stop_propagation();
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|this, _: &MouseDownEvent, _, cx| {
                            this.menu = None;
                            cx.notify();
                        }),
                    ),
            )
            .child(deferred(menu))
    }
}

impl Render for Liber {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::mac::align_traffic_lights(window);
        let screen = match &self.screen {
            Screen::Library => self.library_page(window, cx).into_any_element(),
            Screen::Reader(reader) => reader.clone().into_any_element(),
        };
        div()
            .size_full()
            .font_family("Inter")
            .text_color(c(FG()))
            .bg(c(BG()))
            .on_action(cx.listener(|this, _: &Import, _, cx| this.pick_files(cx)))
            .on_action(cx.listener(|this, _: &Search, window, cx| {
                if matches!(this.screen, Screen::Library) {
                    this.open_search(window, cx);
                }
            }))
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                if ev.keystroke.key == "escape" && (this.search_open || this.menu.is_some() || this.detail.is_some()) {
                    this.close_overlays(cx);
                }
            }))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                this.import(paths.paths().to_vec(), cx);
            }))
            .child(screen)
    }
}
