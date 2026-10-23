//! Per-book viewport, separate from shared appearance settings and library metadata.
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReadingState {
    pub page: u32,
    pub zoom: f32,
    /// Position of the viewport top relative to the page height.
    pub fraction: f32,
    /// Horizontal position as a fraction of the available scroll range.
    pub horizontal: f32,
    pub reflow: bool,
    pub paragraph: usize,
    pub paragraph_offset: f32,
    pub text_byte: usize,
}

impl Default for ReadingState {
    fn default() -> Self {
        Self { page: 0, zoom: 1.0, fraction: 0.0, horizontal: 0.0,
            reflow: false, paragraph: 0, paragraph_offset: 0.0, text_byte: 0 }
    }
}

impl ReadingState {
    fn path(root: &Path, id: &str) -> PathBuf {
        root.join("reading").join(format!("{id}.json"))
    }

    pub fn load(root: &Path, id: &str) -> Option<Self> {
        let bytes = std::fs::read(Self::path(root, id)).ok()?;
        let state: Self = serde_json::from_slice(&bytes).ok()?;
        if ![state.zoom, state.fraction, state.horizontal, state.paragraph_offset]
            .iter().all(|v| v.is_finite()) {
            return None;
        }
        Some(Self { zoom: state.zoom.clamp(0.05, 4.0),
            horizontal: state.horizontal.clamp(0.0, 1.0),
            fraction: state.fraction.clamp(-1.0, 1.0), ..state })
    }

    pub fn save(&self, root: &Path, id: &str) -> std::io::Result<()> {
        let path = Self::path(root, id);
        std::fs::create_dir_all(path.parent().unwrap())?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(self)?)?;
        std::fs::rename(tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_survives_reopening_and_books_are_independent() {
        let root = std::env::temp_dir().join(format!("liber-resume-{}", uuid::Uuid::new_v4()));
        let pdf = ReadingState { page: 51, zoom: 2.3, fraction: 0.637,
            horizontal: 0.71, ..Default::default() };
        let flow = ReadingState { page: 27, reflow: true, paragraph: 8,
            paragraph_offset: 36.5, ..Default::default() };
        assert_eq!(ReadingState::load(&root, "old-library-book"), None);
        pdf.save(&root, "pdf").unwrap();
        flow.save(&root, "flow").unwrap();
        assert_eq!(ReadingState::load(&root, "pdf"), Some(pdf));
        assert_eq!(ReadingState::load(&root, "flow"), Some(flow));
        flow.save(&root, "pdf").unwrap();
        assert_eq!(ReadingState::load(&root, "pdf"), Some(flow));
        std::fs::write(ReadingState::path(&root, "pdf"), b"broken").unwrap();
        assert_eq!(ReadingState::load(&root, "pdf"), None);
        std::fs::remove_dir_all(root).unwrap();
    }
}
