//! Theme choice: persisted next to the library, applied through imago's
//! palette table so Liber follows the same themes as Agenda and Cortex.

use std::path::{Path, PathBuf};

use gpui::{App, WindowAppearance};
use imago_gpui::palettes::THEMES;
use imago_gpui::theme::{self, Palette};
use std::sync::atomic::{AtomicUsize, Ordering};

static THEME: AtomicUsize = AtomicUsize::new(0);

/// Dark side of the selected theme: the reader's Gray/Night tones paint with it.
pub fn dark_palette() -> &'static Palette {
    &THEMES[THEME.load(Ordering::Relaxed).min(THEMES.len() - 1)].dark
}
use serde_json::{json, Map, Value};

fn path(root: &Path) -> PathBuf {
    root.join("appearance.json")
}

fn defaults() -> Map<String, Value> {
    let Value::Object(map) = json!({
        "schema_version": 1,
        "mode": "system",
        "light_theme": "default",
        "dark_theme": "default",
        "accent_source": "theme",
        "accent_color": null,
        "follow_apps": false,
        "material": "default",
        "font_family": "Inter",
        "font_size": 13.0,
        "revision": 0,
    }) else {
        unreachable!()
    };
    map
}

/// Saved settings laid over the defaults; unreadable files fall back to them.
pub fn load(root: &Path) -> Map<String, Value> {
    let mut settings = defaults();
    let saved = std::fs::read(path(root))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Map<String, Value>>(&bytes).ok());
    if let Some(saved) = saved {
        settings.extend(saved);
    }
    settings
}

pub fn save(root: &Path, settings: &Map<String, Value>) {
    if let Ok(bytes) = serde_json::to_vec_pretty(settings) {
        let _ = std::fs::create_dir_all(root);
        let _ = std::fs::write(path(root), bytes);
    }
}

/// Merge a patch from the appearance editor into `settings`.
pub fn merge(settings: &mut Map<String, Value>, patch: &Value) {
    if let Some(patch) = patch.as_object() {
        for (key, value) in patch {
            settings.insert(key.clone(), value.clone());
        }
    }
}

/// Host snapshot in the shape `AppearanceEditor::ingest` expects.
pub fn snapshot(settings: &Map<String, Value>) -> Value {
    json!({
        "settings": settings,
        "capabilities": { "materials": ["default", "opaque"], "wallpaper_accent": false },
    })
}

fn theme_index(key: &str) -> usize {
    THEMES.iter().position(|t| t.key == key).unwrap_or(0)
}

/// Resolve mode + theme and install the palette into gpui-component.
pub fn apply(settings: &Map<String, Value>, cx: &mut App) {
    let dark = match settings.get("mode").and_then(Value::as_str) {
        Some("light") => false,
        Some("dark") => true,
        _ => cx.window_appearance() != WindowAppearance::Light,
    };
    let key = settings
        .get(if dark { "dark_theme" } else { "light_theme" })
        .and_then(Value::as_str)
        .unwrap_or("default");
    let index = theme_index(key);
    THEME.store(index, Ordering::Relaxed);
    theme::set_mode(dark);
    theme::set_theme(index);
    theme::apply(cx);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_values_override_defaults_and_unknown_themes_fall_back() {
        let dir = std::env::temp_dir().join(format!("liber-appearance-{}", std::process::id()));
        let mut settings = load(&dir);
        assert_eq!(settings["mode"], "system");
        merge(&mut settings, &json!({"mode": "dark", "dark_theme": "claude"}));
        save(&dir, &settings);
        let again = load(&dir);
        assert_eq!(again["mode"], "dark");
        assert_eq!(again["dark_theme"], "claude");
        assert_eq!(theme_index("claude"), THEMES.iter().position(|t| t.key == "claude").unwrap());
        assert_eq!(theme_index("nope"), 0);
        let _ = std::fs::remove_dir_all(dir);
    }
}
