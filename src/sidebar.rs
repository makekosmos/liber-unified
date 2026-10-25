//! The library sidebar: Liber's pages and theme over the shared
//! `imago-native-sidebar` component. Where that host registers the native
//! sidebar it is an AppKit source list; elsewhere `lib.rs` draws the same rows
//! with NSH.

use imago_native_sidebar::{Row, SidebarSpec};
use nsh_core::ui::NativeView;
use nsh_core::ui::prelude::*;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

pub use imago_native_sidebar::native_registered;

/// A page of the library, chosen from the sidebar when no book is open.
#[derive(Clone, Copy, PartialEq)]
pub enum Page {
    Home,
    All,
    Settings,
}

impl Page {
    /// The sidebar row id of the page.
    pub fn id(self) -> &'static str {
        match self {
            Page::Home => "home",
            Page::All => "all",
            Page::Settings => "settings",
        }
    }
}

/// The sidebar as the native component: its rows follow `dark` and `page`
/// (unless a book is open, which `selected` says); `caption` is the line under
/// the footer. A press reports the row's id to `press`.
pub fn native_sidebar(
    dark: Signal<bool>,
    page: Signal<Page>,
    selected: Signal<Option<usize>>,
    caption: impl Fn() -> String + 'static,
    press: Rc<dyn Fn(&str)>,
) -> NativeView {
    imago_native_sidebar::native_sidebar(SidebarSpec {
        items: Rc::new(move || items(dark.get())),
        footer: vec![Row { id: "import", label: "Добавить PDF".into(), symbol: Some("plus") }],
        selected: Rc::new(move || {
            (selected.get().is_none()).then(|| page.get().id())
        }),
        caption: Rc::new(caption),
        dark: Rc::new(move || dark.get()),
        press,
    })
}

fn items(dark: bool) -> Vec<Row> {
    vec![
        Row { id: "home", label: "Главная".into(), symbol: Some("house") },
        Row { id: "all", label: "Все книги".into(), symbol: Some("books.vertical") },
        Row { id: "settings", label: "Настройки".into(), symbol: Some("gearshape") },
        Row {
            id: "theme",
            label: if dark { "Светлая тема" } else { "Тёмная тема" }.into(),
            symbol: Some(if dark { "sun.max" } else { "moon" }),
        },
    ]
}

/// Whether the window draws the sidebar itself (`imago_window_macos`): the nsh
/// sidebar column is then zero wide.
pub static SYSTEM: AtomicBool = AtomicBool::new(false);

/// Whether [`SYSTEM`] is set.
pub fn system() -> bool {
    SYSTEM.load(Ordering::Relaxed)
}

/// The rows of the system sidebar. Its theme and import picks go through the
/// same ids as the nsh sidebar's.
#[cfg(target_os = "macos")]
pub fn system_rows() -> Vec<Row> {
    vec![
        Row { id: "home", label: "Главная".into(), symbol: Some("house") },
        Row { id: "all", label: "Все книги".into(), symbol: Some("books.vertical") },
        Row { id: "settings", label: "Настройки".into(), symbol: Some("gearshape") },
        Row { id: "import", label: "Добавить PDF".into(), symbol: Some("plus") },
    ]
}

/// The factory registrations the macOS app passes in its options.
#[cfg(target_os = "macos")]
pub fn components() -> nsh_platform_macos::Components {
    imago_native_sidebar::components()
}
