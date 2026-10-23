//! Text extraction through hayro's public `Device` trait. Only glyph runs are
//! recorded; every drawing call is ignored.

use hayro::hayro_interpret::font::{Glyph as FontGlyph, GlyphRun};
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Device, DrawMode, DrawProps, Image, ImageDrawProps, SoftMask,
};
use hayro::hayro_interpret::hayro_cmap::BfString;
use hayro::kurbo::BezPath;

#[derive(Debug, Clone)]
pub struct Glyph {
    /// Unicode text from ToUnicode (or glyph-name fallback). Empty when unknown.
    pub text: String,
    /// Device-space origin in points, y grows downwards (top-left page origin).
    pub x: f64,
    pub y: f64,
    /// Effective font size in points (length of the glyph transform's x axis).
    pub size: f64,
    /// Advance width in points (horizontal distance to the next glyph origin).
    pub advance: f64,
}

#[derive(Default)]
pub struct TextDevice {
    pub glyphs: Vec<Glyph>,
    /// Glyphs whose Unicode mapping could not be determined.
    pub unmapped: usize,
    /// Glyphs drawn with text render mode 3 (invisible; typical for OCR layers).
    pub invisible: usize,
}

impl<'a> Device<'a> for TextDevice {
    fn draw_glyph_run(&mut self, run: &GlyphRun<'_, 'a>, props: DrawProps<'a>, mode: &DrawMode) {
        // Fill-and-stroke text reaches this method twice. Keep one copy per glyph.
        let invisible = matches!(mode, DrawMode::Invisible);
        if !invisible && !matches!(mode, DrawMode::Fill(_)) {
            return;
        }
        for g in run.glyphs() {
            let t = props.transform * g.transform();
            let [a, b, _, _, e, f] = t.as_coeffs();
            let raw = (a * a + b * b).sqrt();
            // Font widths are in 1/1000 em, and `raw` is font size / 1000 in device units.
            let width = match &**g {
                FontGlyph::Outline(o) => o.advance_width(),
                FontGlyph::Type3(_) => None,
            };
            let advance = width.map_or(0.0, |w| f64::from(w) * raw);
            let text = match g.as_unicode() {
                Some(BfString::Char(c)) => c.to_string(),
                Some(BfString::String(s)) => s,
                None => {
                    self.unmapped += 1;
                    String::new()
                }
            };
            if invisible {
                self.invisible += 1;
            }
            self.glyphs.push(Glyph {
                text,
                x: e,
                y: f,
                // hayro scales glyph outlines by font_size / 1000 (font units per em),
                // so the x-axis length is ~0.001 * font size. Scale back to points.
                size: raw * 1000.0,
                advance,
            });
        }
    }

    fn draw_path(&mut self, _: &BezPath, _: DrawProps<'a>, _: &DrawMode) {}
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn draw_image(&mut self, _: Image<'a, '_>, _: ImageDrawProps<'a>) {}
    fn pop_clip(&mut self) {}
    fn pop_transparency_group(&mut self) {}
}

/// Group glyphs into lines (by baseline) and join them left-to-right.
pub fn to_lines(glyphs: &[Glyph]) -> String {
    let mut sorted: Vec<&Glyph> = glyphs.iter().filter(|g| !g.text.is_empty()).collect();
    sorted.sort_by(|a, b| {
        let tol = a.size.max(1.0) * 0.5;
        if (a.y - b.y).abs() <= tol {
            a.x.total_cmp(&b.x)
        } else {
            a.y.total_cmp(&b.y)
        }
    });
    let mut out = String::new();
    let mut last: Option<&Glyph> = None;
    for g in sorted {
        if let Some(prev) = last {
            let tol = prev.size.max(1.0) * 0.5;
            if (g.y - prev.y).abs() > tol {
                out.push('\n');
            } else if g.x - (prev.x + prev.advance) > prev.size * 0.2 {
                out.push(' ');
            }
        }
        out.push_str(&g.text);
        last = Some(g);
    }
    out
}
