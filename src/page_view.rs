//! The reader page as a native component: an NSView that draws the page
//! preview and its tiles from the PNG files `liber-core` renders. AppKit
//! passes each draw the dirty rect, so only the tiles in the visible part of
//! the page are drawn.

use nsh_platform_macos::{
    AppView, NativeComponent,
    objc2::{
        AllocAnyThread, DefinedClass, MainThreadOnly, define_class, msg_send, rc::Retained,
    },
    objc2_app_kit::{NSCompositingOperation, NSImage, NSResponder, NSView},
    objc2_foundation::{MainThreadMarker, NSObject, NSPoint, NSRect, NSSize, NSString},
};
use serde_json::Value as JsonValue;
use std::cell::RefCell;
use std::rc::Rc;

/// The name `native("liber.page")` uses in the app.
pub const NAME: &str = "liber.page";

/// Decoded PNGs kept at once; recently used tiles stay hot when the reader reverses direction.
const IMAGE_LIMIT: usize = 96;

thread_local! {
    static IMAGES: RefCell<Vec<(String, Retained<NSImage>)>> = const { RefCell::new(Vec::new()) };
}

/// Registers the page component with `view`.
pub fn register(view: &AppView) {
    view.set_fps_monitor(true);
    view.register_component(
        NAME,
        Rc::new(|_| {
            let mtm = MainThreadMarker::new().expect("components are made on the main thread");
            Box::new(Page {
                view: PageView::new(mtm),
            })
        }),
    );
}

#[derive(Default)]
struct PageState {
    preview: Option<String>,
    /// Page size in points: the preview and the tiles are laid over it.
    size: (f64, f64),
    tiles: Vec<Tile>,
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
    }
);

impl PageView {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(RefCell::new(PageState::default()));
        // SAFETY: `init` on a freshly allocated instance of this class.
        unsafe { msg_send![super(this), init] }
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
                _ => return,
            }
        }
        self.setNeedsDisplay(true);
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
            NSSize::new(value.get("width")?.as_f64()?, value.get("height")?.as_f64()?),
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
        let decoded = NSImage::initWithContentsOfFile(
            NSImage::alloc(),
            &NSString::from_str(path),
        )?;
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
