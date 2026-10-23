//! Embedded icons and fonts; everything else falls through to imago's set.
use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

macro_rules! embed {
    ($($path:literal),* $(,)?) => {
        fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
            match path {
                $( $path => Ok(Some(Cow::Borrowed(include_bytes!(concat!("../assets/", $path)) as &[u8]))), )*
                _ => imago_gpui::assets::Assets.load(path),
            }
        }
        fn list(&self, path: &str) -> Result<Vec<SharedString>> {
            let mut paths = imago_gpui::assets::Assets.list(path)?;
            paths.extend([$(SharedString::new_static($path)),*].into_iter()
                .filter(|name| name.starts_with(path)));
            paths.sort();
            paths.dedup();
            Ok(paths)
        }
    };
}

pub struct Assets;

impl AssetSource for Assets {
    embed!(
        "icons/arrow-left.svg",
        "icons/book-open.svg",
        "icons/book-open-fill.svg",
        "icons/clock-01.svg",
        "icons/delete.svg",
        "icons/folder.svg",
        "icons/help-circle.svg",
        "icons/inbox.svg",
        "icons/more-h.svg",
        "icons/plus.svg",
        "icons/search.svg",
        "icons/settings.svg",
        "icons/sidebar-left.svg",
        "icons/star.svg",
        "icons/status-x.svg",
        "icons/window-max.svg",
        "icons/window-min.svg",
        "icons/window-restore.svg",
    );
}

pub fn font_bytes() -> Vec<Cow<'static, [u8]>> {
    vec![Cow::Borrowed(include_bytes!("../assets/fonts/Inter.ttf") as &[u8])]
}
