//! Text layer geometry and per-book highlights with notes.
//! Coordinates are PDF points, origin top-left of the page.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One glyph of the text layer.
#[derive(Clone, Debug, PartialEq)]
pub struct Ch {
    pub c: char,
    /// `[x0, y0, x1, y1]`
    pub r: [f32; 4],
    pub line: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LinkTarget {
    /// Zero-based page inside the document.
    Page(u32),
    Uri(String),
}

#[derive(Clone, Debug)]
pub struct PageLink {
    /// `[x0, y0, x1, y1]` in page points.
    pub r: [f32; 4],
    pub target: LinkTarget,
}

#[derive(Clone, Debug, Default)]
pub struct PageText {
    pub chars: Vec<Ch>,
    pub links: Vec<PageLink>,
}

fn inside(r: &[f32; 4], x: f32, y: f32) -> bool {
    x >= r[0] && x <= r[2] && y >= r[1] && y <= r[3]
}

fn dist_to_rect(r: &[f32; 4], x: f32, y: f32) -> f32 {
    let dx = (r[0] - x).max(0.0).max(x - r[2]);
    let dy = (r[1] - y).max(0.0).max(y - r[3]);
    // Same-line glyphs win over glyphs that are merely close vertically.
    (dx * dx + dy * dy * 4.0).sqrt()
}

impl PageText {
    /// Index of the glyph nearest to the point.
    pub fn hit(&self, x: f32, y: f32) -> Option<usize> {
        self.chars
            .iter()
            .enumerate()
            .map(|(i, ch)| (i, dist_to_rect(&ch.r, x, y)))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    }

    pub fn link_at(&self, x: f32, y: f32) -> Option<&LinkTarget> {
        self.links
            .iter()
            .find(|l| inside(&l.r, x, y))
            .map(|l| &l.target)
    }

    /// One rectangle per line covering glyphs `a..=b` (order-insensitive).
    pub fn rects(&self, a: usize, b: usize) -> Vec<[f32; 4]> {
        let (a, b) = (a.min(b), a.max(b));
        let mut out: Vec<[f32; 4]> = Vec::new();
        let mut line = None;
        for ch in self.chars.iter().skip(a).take(b + 1 - a) {
            if ch.c.is_whitespace() && line != Some(ch.line) {
                continue;
            }
            match (line == Some(ch.line), out.last_mut()) {
                (true, Some(r)) => {
                    r[0] = r[0].min(ch.r[0]);
                    r[1] = r[1].min(ch.r[1]);
                    r[2] = r[2].max(ch.r[2]);
                    r[3] = r[3].max(ch.r[3]);
                }
                _ => {
                    out.push(ch.r);
                    line = Some(ch.line);
                }
            }
        }
        out
    }

    pub fn text(&self, a: usize, b: usize) -> String {
        let (a, b) = (a.min(b), a.max(b));
        let mut s = String::new();
        let mut prev = None;
        for ch in self.chars.iter().skip(a).take(b + 1 - a) {
            if prev.is_some_and(|l| l != ch.line) && !s.ends_with(' ') && !s.ends_with('-') {
                s.push(' ');
            }
            prev = Some(ch.line);
            s.push(ch.c);
        }
        s.trim().to_owned()
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Tint {
    Yellow,
    Green,
    Blue,
    Pink,
}

impl Tint {
    pub const ALL: [Tint; 4] = [Tint::Yellow, Tint::Green, Tint::Blue, Tint::Pink];

    /// sRGB hex, used for both swatches and the page overlay.
    pub fn hex(self) -> u32 {
        match self {
            Tint::Yellow => 0xFFD60A,
            Tint::Green => 0x34C759,
            Tint::Blue => 0x0A84FF,
            Tint::Pink => 0xFF4FA3,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct FlowRange {
    pub page: u32,
    pub paragraph: usize,
    pub start: usize,
    pub end: usize,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Highlight {
    pub id: String,
    pub page: u32,
    pub rects: Vec<[f32; 4]>,
    pub text: String,
    pub tint: Tint,
    #[serde(default)]
    pub note: String,
    pub created: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flow: Vec<FlowRange>,
}

impl Highlight {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        self.rects
            .iter()
            .any(|r| x >= r[0] && x <= r[2] && y >= r[1] && y <= r[3])
    }
}

pub fn notes_file(root: &Path, book_id: &str) -> PathBuf {
    root.join("notes").join(format!("{book_id}.json"))
}

pub struct Notes {
    path: PathBuf,
    pub items: Vec<Highlight>,
}

impl Notes {
    pub fn open(root: &Path, book_id: &str) -> Self {
        let path = notes_file(root, book_id);
        let items = fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self { path, items }
    }

    pub fn add(&mut self, h: Highlight) {
        self.items.push(h);
        self.items
            .sort_by(|a, b| a.page.cmp(&b.page).then(a.created.cmp(&b.created)));
        self.save();
    }

    pub fn remove(&mut self, id: &str) {
        self.items.retain(|h| h.id != id);
        self.save();
    }

    pub fn update(&mut self, id: &str, f: impl FnOnce(&mut Highlight)) {
        if let Some(h) = self.items.iter_mut().find(|h| h.id == id) {
            f(h);
            self.save();
        }
    }

    pub fn on_page(&self, page: u32) -> impl Iterator<Item = &Highlight> {
        self.items.iter().filter(move |h| h.page == page)
    }

    fn save(&self) {
        let Ok(json) = serde_json::to_vec_pretty(&self.items) else {
            return;
        };
        if let Some(dir) = self.path.parent() {
            if fs::create_dir_all(dir).is_err() {
                return;
            }
        }
        let tmp = self.path.with_extension("json.tmp");
        if fs::write(&tmp, json).is_ok() {
            let _ = fs::rename(&tmp, &self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, line: u32, y: f32) -> Vec<Ch> {
        text.chars()
            .enumerate()
            .map(|(i, c)| Ch {
                c,
                r: [i as f32 * 10.0, y, i as f32 * 10.0 + 10.0, y + 10.0],
                line,
            })
            .collect()
    }

    fn page() -> PageText {
        let mut chars = line("hello world", 0, 0.0);
        chars.extend(line("second", 1, 20.0));
        PageText {
            chars,
            links: Vec::new(),
        }
    }

    #[test]
    fn hit_finds_containing_or_nearest_glyph() {
        let p = page();
        assert_eq!(p.hit(15.0, 5.0), Some(1));
        // Right of the first line's end still maps to its last glyph.
        assert_eq!(p.hit(500.0, 4.0), Some(10));
        // Between lines, the closer line wins.
        assert_eq!(p.hit(5.0, 18.0), Some(11));
    }

    #[test]
    fn rects_merge_per_line() {
        let p = page();
        let r = p.rects(6, 13);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0], [60.0, 0.0, 110.0, 10.0]);
        assert_eq!(r[1], [0.0, 20.0, 30.0, 30.0]);
        assert_eq!(p.rects(13, 6), r);
    }

    #[test]
    fn text_joins_lines_with_a_space() {
        let p = page();
        assert_eq!(p.text(6, 13), "world sec");
        assert_eq!(p.text(0, 4), "hello");
    }

    #[test]
    fn notes_roundtrip_and_remove() {
        let dir = std::env::temp_dir().join(format!("liber-notes-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut n = Notes::open(&dir, "b1");
        n.add(Highlight {
            id: "h1".into(),
            page: 2,
            rects: vec![[0.0, 0.0, 10.0, 10.0]],
            text: "x".into(),
            tint: Tint::Green,
            note: String::new(),
            created: 1,
            flow: Vec::new(),
        });
        n.update("h1", |h| h.note = "заметка".into());
        let again = Notes::open(&dir, "b1");
        assert_eq!(again.items.len(), 1);
        assert_eq!(again.items[0].note, "заметка");
        assert!(again.items[0].contains(5.0, 5.0) && !again.items[0].contains(50.0, 5.0));
        let mut again = again;
        again.remove("h1");
        assert!(Notes::open(&dir, "b1").items.is_empty());
    }
}
