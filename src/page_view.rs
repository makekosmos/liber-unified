//! The reader page as a native component: an NSView that draws the page
//! preview and its tiles from the PNG files `liber-core` renders. AppKit
//! passes each draw the dirty rect, so only the tiles in the visible part of
//! the page are drawn.

use nsh_platform_macos::{
    AppView, ComponentContext, NativeComponent,
    objc2::{
        AllocAnyThread, DefinedClass, MainThreadOnly, define_class, msg_send, rc::Retained, sel,
    },
    objc2_app_kit::{
        NSBezelStyle, NSButton, NSCompositingOperation, NSEvent, NSEventModifierFlags, NSImage,
        NSMenu, NSMenuItem, NSPasteboard, NSPasteboardTypeString, NSResponder, NSView,
    },
    objc2_foundation::{MainThreadMarker, NSObject, NSPoint, NSRect, NSSize, NSString},
};
use serde_json::Value as JsonValue;
use std::cell::RefCell;
use std::rc::Rc;

/// The name `native("liber.page")` uses in the app.
pub const NAME: &str = "liber.page";
/// Native AppKit button used in the note editor.
pub const NOTE_BUTTON: &str = "liber.note-button";

/// Decoded PNGs kept at once; recently used tiles stay hot when the reader reverses direction.
const IMAGE_LIMIT: usize = 96;

thread_local! {
    static IMAGES: RefCell<Vec<(String, Retained<NSImage>)>> = const { RefCell::new(Vec::new()) };
}

/// Registers the page component with `view`.
pub fn register(view: &AppView) {
    #[cfg(debug_assertions)]
    view.set_fps_monitor(true);
    view.register_component(
        NAME,
        Rc::new(|context| {
            let mtm = MainThreadMarker::new().expect("components are made on the main thread");
            Box::new(Page {
                view: PageView::new(mtm, context),
            })
        }),
    );
    view.register_component(
        NOTE_BUTTON,
        Rc::new(|context| {
            let mtm = MainThreadMarker::new().expect("components are made on the main thread");
            Box::new(NoteButton::new(mtm, context))
        }),
    );
}

#[derive(Default)]
struct PageState {
    preview: Option<String>,
    /// Page size in points: the preview and the tiles are laid over it.
    size: (f64, f64),
    tiles: Vec<Tile>,
    context: Option<ComponentContext>,
    menu_enabled: bool,
    selection_text: String,
}

struct Tile {
    png: String,
    /// In page points, from the top left of the page.
    rect: NSRect,
}

define_class!(
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = RefCell<PageState>]
    struct PageView;

    impl PageView {
        #[unsafe(method(drawRect:))]
        fn draw(&self, dirty: NSRect) {
            let state = self.ivars().borrow();
            let (page_width, page_height) = state.size;
            if page_width <= 0.0 || page_height <= 0.0 {
                return;
            }
            let bounds = self.bounds();
            let factor = bounds.size.width / page_width;
            // The view is not flipped: page y = 0 is the top edge.
            let to_view = |rect: NSRect| {
                NSRect::new(
                    NSPoint::new(
                        rect.origin.x * factor,
                        bounds.size.height - (rect.origin.y + rect.size.height) * factor,
                    ),
                    NSSize::new(rect.size.width * factor, rect.size.height * factor),
                )
            };
            if let Some(preview) = state.preview.as_deref().and_then(image) {
                draw_image(&preview, bounds);
            }
            for tile in &state.tiles {
                let rect = to_view(tile.rect);
                if !intersects(rect, dirty) {
                    continue;
                }
                if let Some(image) = image(&tile.png) {
                    draw_image(&image, rect);
                }
            }
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool { true }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            if let Some(window) = self.window() { window.makeFirstResponder(Some(self)); }
            self.emit_point("selectionStart", event);
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) { self.emit_point("selectionMove", event); }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) { self.emit_point("selectionEnd", event); }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            let command = event.modifierFlags().contains(NSEventModifierFlags::Command);
            let copy = event.charactersIgnoringModifiers()
                .is_some_and(|key| key.to_string().eq_ignore_ascii_case("c"));
            if command && copy {
                if !self.copy_selection() {
                    unsafe { msg_send![super(self), keyDown: event] }
                }
            } else {
                unsafe { msg_send![super(self), keyDown: event] }
            }
        }

        #[unsafe(method_id(menuForEvent:))]
        fn menu_for_event(&self, _event: &NSEvent) -> Option<Retained<NSMenu>> {
            if self.ivars().borrow().menu_enabled {
                MainThreadMarker::new().map(|mtm| {
                    let menu = NSMenu::new(mtm);
                    menu.setAutoenablesItems(false);
                    self.add_menu_item(&menu, mtm, "🟡  Жёлтый", sel!(selectYellow:));
                    self.add_menu_item(&menu, mtm, "🟢  Зелёный", sel!(selectGreen:));
                    self.add_menu_item(&menu, mtm, "🔵  Синий", sel!(selectBlue:));
                    self.add_menu_item(&menu, mtm, "🩷  Розовый", sel!(selectPink:));
                    menu.addItem(&NSMenuItem::separatorItem(mtm));
                    self.add_menu_item(&menu, mtm, "Добавить заметку", sel!(addNote:));
                    self.add_menu_item(&menu, mtm, "Копировать", sel!(copySelection:));
                    menu
                })
            } else {
                None
            }
        }

        #[unsafe(method(selectYellow:))]
        fn select_yellow(&self, _sender: &NSMenuItem) { self.emit_action("yellow"); }
        #[unsafe(method(selectGreen:))]
        fn select_green(&self, _sender: &NSMenuItem) { self.emit_action("green"); }
        #[unsafe(method(selectBlue:))]
        fn select_blue(&self, _sender: &NSMenuItem) { self.emit_action("blue"); }
        #[unsafe(method(selectPink:))]
        fn select_pink(&self, _sender: &NSMenuItem) { self.emit_action("pink"); }
        #[unsafe(method(addNote:))]
        fn add_note(&self, _sender: &NSMenuItem) { self.emit_action("note"); }
        #[unsafe(method(copySelection:))]
        fn copy_selection_menu(&self, _sender: &NSMenuItem) { self.copy_selection(); }
    }
);

impl PageView {
    fn new(mtm: MainThreadMarker, context: ComponentContext) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(RefCell::new(PageState::default()));
        // SAFETY: `init` on a freshly allocated instance of this class.
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        this.ivars().borrow_mut().context = Some(context);
        this
    }

    fn update(&self, name: &str, value: Option<&JsonValue>) {
        {
            let mut state = self.ivars().borrow_mut();
            match name {
                "preview" => state.preview = value.and_then(JsonValue::as_str).map(str::to_owned),
                "size" => state.size = size(value).unwrap_or_default(),
                "tiles" => {
                    state.tiles = value
                        .and_then(JsonValue::as_array)
                        .map(|tiles| tiles.iter().filter_map(tile).collect())
                        .unwrap_or_default();
                }
                "menuEnabled" => {
                    state.menu_enabled = value.and_then(JsonValue::as_bool).unwrap_or(false)
                }
                "selectionText" => {
                    state.selection_text = value
                        .and_then(JsonValue::as_str)
                        .unwrap_or_default()
                        .to_owned()
                }
                _ => return,
            }
        }
        self.setNeedsDisplay(true);
    }

    fn emit_point(&self, name: &str, event: &NSEvent) {
        let point = self.convertPoint_fromView(event.locationInWindow(), None);
        let y = self.bounds().size.height - point.y;
        if let Some(context) = self.ivars().borrow().context.as_ref() {
            context.emit(name, serde_json::json!([point.x, y]));
        }
    }

    fn emit_action(&self, action: &str) {
        if let Some(context) = self.ivars().borrow().context.as_ref() {
            context.emit("selectionAction", serde_json::json!(action));
        }
    }

    fn copy_selection(&self) -> bool {
        let text = self.ivars().borrow().selection_text.clone();
        if text.is_empty() {
            eprintln!("Liber selection copy: no selected text");
            return false;
        }
        let pasteboard = NSPasteboard::generalPasteboard();
        pasteboard.clearContents();
        // SAFETY: this is AppKit's immutable global string pasteboard type.
        let copied = unsafe {
            pasteboard.setString_forType(&NSString::from_str(&text), NSPasteboardTypeString)
        };
        eprintln!(
            "Liber selection copy: {} chars, pasteboard accepted={copied}",
            text.chars().count()
        );
        copied
    }

    fn add_menu_item(
        &self,
        menu: &NSMenu,
        mtm: MainThreadMarker,
        title: &str,
        action: objc2::runtime::Sel,
    ) {
        // SAFETY: each selector is implemented by this view below.
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(title),
                Some(action),
                &NSString::new(),
            )
        };
        // SAFETY: AppKit invokes this selector on the retained target.
        unsafe {
            item.setTarget(Some(self));
            item.setAction(Some(action));
        }
        menu.addItem(&item);
    }
}

#[derive(Default)]
struct ButtonTargetState {
    context: Option<ComponentContext>,
    action: String,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = RefCell<ButtonTargetState>]
    struct ButtonTarget;

    impl ButtonTarget {
        #[unsafe(method(press:))]
        fn press(&self, _sender: &NSButton) {
            let state = self.ivars().borrow();
            if let Some(context) = state.context.as_ref() {
                context.emit("press", serde_json::json!(state.action));
            }
        }
    }
);

impl ButtonTarget {
    fn new(mtm: MainThreadMarker, context: ComponentContext) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(RefCell::new(ButtonTargetState {
            context: Some(context),
            action: String::new(),
        }));
        // SAFETY: `init` on a freshly allocated instance of this class.
        unsafe { msg_send![super(this), init] }
    }
}

struct NoteButton {
    button: Retained<NSButton>,
    target: Retained<ButtonTarget>,
}

impl NoteButton {
    fn new(mtm: MainThreadMarker, context: ComponentContext) -> Self {
        let target = ButtonTarget::new(mtm, context);
        // SAFETY: the target is retained by `Self` and implements the action selector.
        let button = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::new(),
                Some(&target),
                Some(sel!(press:)),
                mtm,
            )
        };
        button.setBezelStyle(NSBezelStyle::Rounded);
        Self { button, target }
    }
}

impl NativeComponent for NoteButton {
    fn view(&self) -> Retained<NSView> {
        Retained::into_super(Retained::into_super(self.button.clone()))
    }

    fn set_prop(&mut self, name: &str, value: Option<&JsonValue>) {
        match name {
            "title" => self.button.setTitle(&NSString::from_str(
                value.and_then(JsonValue::as_str).unwrap_or_default(),
            )),
            "action" => {
                self.target.ivars().borrow_mut().action = value
                    .and_then(JsonValue::as_str)
                    .unwrap_or_default()
                    .to_owned();
            }
            _ => {}
        }
    }
}

/// The component nsh drives: it forwards every prop to its view.
struct Page {
    view: Retained<PageView>,
}

impl NativeComponent for Page {
    fn view(&self) -> Retained<NSView> {
        Retained::into_super(self.view.clone())
    }

    fn set_prop(&mut self, name: &str, value: Option<&JsonValue>) {
        self.view.update(name, value);
    }
}

fn size(value: Option<&JsonValue>) -> Option<(f64, f64)> {
    let values = value?.as_array()?;
    Some((values.first()?.as_f64()?, values.get(1)?.as_f64()?))
}

fn tile(value: &JsonValue) -> Option<Tile> {
    Some(Tile {
        png: value.get("png")?.as_str()?.to_owned(),
        rect: NSRect::new(
            NSPoint::new(value.get("x")?.as_f64()?, value.get("y")?.as_f64()?),
            NSSize::new(
                value.get("width")?.as_f64()?,
                value.get("height")?.as_f64()?,
            ),
        ),
    })
}

/// The decoded PNG at `path`, decoded on first use.
fn image(path: &str) -> Option<Retained<NSImage>> {
    IMAGES.with(|images| {
        let mut images = images.borrow_mut();
        if let Some(index) = images.iter().position(|(cached, _)| cached == path) {
            let (cached, image) = images.remove(index);
            let copy = image.clone();
            images.push((cached, image));
            return Some(copy);
        }
        let decoded = NSImage::initWithContentsOfFile(NSImage::alloc(), &NSString::from_str(path))?;
        if images.len() >= IMAGE_LIMIT {
            images.remove(0);
        }
        images.push((path.to_owned(), decoded.clone()));
        Some(decoded)
    })
}

fn draw_image(image: &NSImage, rect: NSRect) {
    image.drawInRect_fromRect_operation_fraction(
        rect,
        NSRect::ZERO,
        NSCompositingOperation::SourceOver,
        1.0,
    );
}

fn intersects(a: NSRect, b: NSRect) -> bool {
    a.origin.x < b.origin.x + b.size.width
        && b.origin.x < a.origin.x + a.size.width
        && a.origin.y < b.origin.y + b.size.height
        && b.origin.y < a.origin.y + a.size.height
}
