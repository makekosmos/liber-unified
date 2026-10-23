//! PDF access through hayro: metadata probe, cover thumbnails, page rendering,
//! text extraction and a worker that talks to the UI over channels.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::thread;

use hayro::hayro_interpret::util::TransformExt;
use hayro::hayro_interpret::{interpret_page, Context, InterpreterCache, InterpreterSettings};
use hayro::hayro_syntax::Pdf as Document;
use hayro::kurbo::Rect;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{PixmapSettings, RenderCache, RenderSettings};

#[path = "pdf_nav.rs"]
mod nav;
#[path = "pdf_text.rs"]
mod text;

use crate::flow::{self, Para, RawBlock};
use crate::notes::{Ch, LinkTarget, PageLink, PageText};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Meta {
    pub title: Option<String>,
    pub author: Option<String>,
    pub pages: u32,
}

fn clean(text: String) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

fn open(path: &Path) -> Result<Document, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    Document::new(bytes).map_err(|e| format!("не удалось разобрать PDF: {e:?}"))
}

pub fn probe(path: &Path) -> Result<Meta, String> {
    let doc = open(path)?;
    let pages = doc.pages().len();
    if pages == 0 {
        return Err("в файле нет страниц".into());
    }
    Ok(Meta {
        title: doc
            .metadata()
            .title
            .as_deref()
            .map(nav::decode_text)
            .and_then(clean),
        author: doc
            .metadata()
            .author
            .as_deref()
            .map(nav::decode_text)
            .and_then(clean),
        pages: pages as u32,
    })
}

/// Return PDF page labels (`i`, `1`, `A-1`, …), with one-based numeric labels
/// when the document does not define a label tree.
pub fn page_labels(path: &Path) -> Result<Vec<String>, String> {
    let doc = open(path)?;
    Ok(nav::page_labels(&doc))
}

/// Render page 0 as a PNG that fits `width_px` pixels across.
pub fn save_cover(pdf: &Path, png: &Path, width_px: f32) -> Result<(), String> {
    let doc = open(pdf)?;
    let page = doc.pages().first().ok_or("в PDF нет страниц")?;
    let (w, _) = page.render_dimensions();
    let scale = width_px / f32::from(w).max(1.0);
    let cache = RenderCache::new();
    let pixmap = hayro::render(
        page,
        &cache,
        &InterpreterSettings::default(),
        &RenderSettings::default(),
        &PixmapSettings {
            x_scale: scale,
            y_scale: scale,
            bg_color: WHITE,
        },
    );
    let image = image::RgbaImage::from_raw(
        u32::from(pixmap.width()),
        u32::from(pixmap.height()),
        pixmap.data_as_u8_slice().to_vec(),
    )
    .ok_or("hayro вернул некорректный буфер")?;
    image.save(png).map_err(|e| e.to_string())
}

/// Reading theme: how page bitmaps are recoloured and what the reflowed
/// text looks like.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
pub enum Tone {
    #[default]
    Day,
    Sepia,
    Gray,
    Night,
}

impl Tone {
    pub const ALL: [Tone; 4] = [Tone::Day, Tone::Sepia, Tone::Gray, Tone::Night];

    pub fn label(self) -> &'static str {
        match self {
            Tone::Day => "Белая",
            Tone::Sepia => "Сепия",
            Tone::Gray => "Серая",
            Tone::Night => "Ночь",
        }
    }

    /// Page / reading surface colour. Gray and Night come from the dark side
    /// of the selected theme.
    pub fn paper(self) -> u32 {
        match self {
            Tone::Day => 0xFFFFFF,
            Tone::Sepia => 0xF4ECD8,
            Tone::Gray => crate::appearance::dark_palette().card,
            Tone::Night => crate::appearance::dark_palette().bg,
        }
    }

    /// Text colour on `paper`.
    pub fn ink(self) -> u32 {
        match self {
            Tone::Day => 0x1D1D1F,
            Tone::Sepia => 0x4B3A2A,
            Tone::Gray | Tone::Night => crate::appearance::dark_palette().fg,
        }
    }

    /// Colour around the pages.
    pub fn surround(self) -> u32 {
        match self {
            Tone::Day => 0xE9E9EB,
            Tone::Sepia => 0xE6DCC2,
            Tone::Gray | Tone::Night => crate::appearance::dark_palette().sidebar_bg,
        }
    }
}

/// Page bitmap in BGRA order, the layout GPUI's `RenderImage` expects.
pub struct Rendered {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

fn to_bgra(width: usize, height: usize, samples: &[u8], tone: Tone) -> Rendered {
    let mut bgra = Vec::with_capacity(width * height * 4);
    for px in samples.chunks_exact(4).take(width * height) {
        let (r, g, b) = (px[0], px[1], px[2]);
        let (r, g, b) = match tone {
            Tone::Day => (r, g, b),
            // Multiply by the paper colour: white becomes cream, ink stays dark.
            Tone::Sepia => (mul(r, 0xF4), mul(g, 0xEC), mul(b, 0xD8)),
            Tone::Night => (mul(255 - r, 0xDD), mul(255 - g, 0xDD), mul(255 - b, 0xD6)),
            // Inverted, lifted off black onto the gray paper.
            Tone::Gray => (lift(255 - r), lift(255 - g), lift(255 - b)),
        };
        bgra.extend_from_slice(&[b, g, r, 255]);
    }
    Rendered {
        width: width as u32,
        height: height as u32,
        bgra,
    }
}

fn lift(v: u8) -> u8 {
    (0x3A + (u16::from(v) * (0xE5 - 0x3A)) / 255) as u8
}

fn mul(v: u8, k: u8) -> u8 {
    ((u16::from(v) * u16::from(k)) / 255) as u8
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TocEntry {
    pub title: String,
    pub page: Option<u32>,
    pub depth: u8,
}

/// Everything the reader needs before the first page is drawn.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Opened {
    /// Page size in PDF points, one per page.
    pub sizes: Vec<(f32, f32)>,
    pub toc: Vec<TocEntry>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Tile {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Quantized device-pixel scale and optional bounded tile. None is a small
/// whole-page preview, retained underneath detailed tiles during navigation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RasterKey {
    pub page: u32,
    pub scale: u32,
    pub tone: Tone,
    pub tile: Option<Tile>,
}

impl RasterKey {
    pub fn factor(self) -> f32 {
        self.scale as f32 / 50.0
    }
}

pub struct Reply {
    pub page: u32,
    pub tone: Tone,
    pub scale: f32,
    pub tile: Option<Tile>,
    pub image: Result<Rendered, String>,
}

impl Reply {
    pub fn key(&self) -> RasterKey {
        RasterKey {
            page: self.page,
            scale: (self.scale * 50.0).round() as u32,
            tone: self.tone,
            tile: self.tile,
        }
    }
}

enum Request {
    Render(RasterKey),
    Text(u32),
    Flow(u32),
    Index(u32),
    Trim,
}

#[derive(Default)]
struct Work {
    closed: bool,
    wanted: std::collections::HashSet<RasterKey>,
    delivered: std::collections::HashSet<RasterKey>,
    in_flight: Option<RasterKey>,
    text_in_flight: Option<u32>,
    renders: VecDeque<RasterKey>,
    texts: VecDeque<u32>,
    flows: VecDeque<u32>,
    indices: VecDeque<u32>,
    repairs: VecDeque<u32>,
    flow_active: bool,
    search_active: bool,
    idle_trimmed: bool,
    index_turn: bool,
}

type SharedWork = std::sync::Arc<(std::sync::Mutex<Work>, std::sync::Condvar)>;

/// Results are bounded, requests are a replaceable viewport snapshot. The UI
/// never blocks sending work; at most one obsolete tile can be in progress.
pub struct Renderer {
    work: SharedWork,
    notify: async_channel::Receiver<()>,
    pub opened: Receiver<Result<Opened, String>>,
    pub replies: Receiver<Reply>,
    pub texts: Receiver<(u32, PageText)>,
    pub flows: Receiver<(u32, Vec<Para>)>,
    pub index: Receiver<(u32, String)>,
}

struct Output<T> {
    tx: mpsc::SyncSender<T>,
    wake: async_channel::Sender<()>,
}

impl<T> Output<T> {
    fn send(&self, value: T) -> Result<(), mpsc::SendError<T>> {
        self.tx.send(value)?;
        let _ = self.wake.try_send(());
        Ok(())
    }
}

fn output<T>(capacity: usize, wake: &async_channel::Sender<()>) -> (Output<T>, Receiver<T>) {
    let (tx, rx) = mpsc::sync_channel(capacity);
    (
        Output {
            tx,
            wake: wake.clone(),
        },
        rx,
    )
}

impl Renderer {
    pub fn spawn(path: PathBuf) -> Self {
        let work = SharedWork::default();
        let (wake, notify) = async_channel::bounded(1);
        let (opened_tx, opened) = output(1, &wake);
        let (reply_tx, replies) = output(3, &wake);
        let (text_tx, texts) = output(3, &wake);
        let (flow_tx, flows) = output(4, &wake);
        let (index_tx, index) = output(8, &wake);
        let thread_work = work.clone();
        thread::Builder::new()
            .name("liber-render".into())
            .spawn(move || {
                worker(
                    path,
                    thread_work,
                    opened_tx,
                    reply_tx,
                    text_tx,
                    flow_tx,
                    index_tx,
                )
            })
            .expect("spawn render thread");
        Self {
            work,
            notify,
            opened,
            replies,
            texts,
            flows,
            index,
        }
    }

    pub fn notifications(&self) -> async_channel::Receiver<()> {
        self.notify.clone()
    }

    /// Also used by the renderer tests, where there is no viewport scheduler.
    #[cfg(test)]
    pub fn request(&self, page: u32, scale: f32, tone: Tone) {
        let key = RasterKey {
            page,
            scale: (scale * 50.0).round() as u32,
            tone,
            tile: None,
        };
        let mut work = self.work.0.lock().unwrap();
        work.wanted.insert(key);
        work.renders.push_back(key);
        self.work.1.notify_one();
    }

    pub fn set_view(&self, wanted: &[RasterKey], missing: &[RasterKey], text_pages: &[u32]) {
        let mut work = self.work.0.lock().unwrap();
        work.wanted = wanted.iter().copied().collect();
        let desired = work.wanted.clone();
        work.delivered.retain(|key| desired.contains(key));
        work.renders = missing
            .iter()
            .copied()
            .filter(|key| work.in_flight != Some(*key) && !work.delivered.contains(key))
            .collect();
        work.texts = text_pages
            .iter()
            .copied()
            .filter(|page| work.text_in_flight != Some(*page))
            .collect();
        self.work.1.notify_one();
    }

    pub fn set_activity(&self, flow: bool, search: bool) {
        let mut work = self.work.0.lock().unwrap();
        if work.flow_active == flow && work.search_active == search {
            return;
        }
        work.flow_active = flow;
        work.search_active = search;
        self.work.1.notify_one();
    }

    pub fn request_flow(&self, page: u32) {
        let mut work = self.work.0.lock().unwrap();
        if !work.flows.contains(&page) {
            work.flows.push_back(page);
        }
        work.flow_active = true;
        self.work.1.notify_one();
    }

    pub fn prioritize_flow(&self, page: u32) {
        let mut work = self.work.0.lock().unwrap();
        if let Some(at) = work.flows.iter().position(|p| *p == page) {
            work.flows.remove(at);
            work.flows.push_front(page);
        } else if let Some(at) = work.repairs.iter().position(|p| *p == page) {
            work.repairs.remove(at);
            work.repairs.push_front(page);
        }
        self.work.1.notify_one();
    }

    pub fn request_index(&self, page: u32) {
        let mut work = self.work.0.lock().unwrap();
        if !work.indices.contains(&page) {
            work.indices.push_back(page);
        }
        work.search_active = true;
        self.work.1.notify_one();
    }

    #[cfg(test)]
    pub fn request_text(&self, page: u32) {
        let mut work = self.work.0.lock().unwrap();
        if !work.texts.contains(&page) {
            work.texts.push_back(page);
        }
        self.work.1.notify_one();
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        let mut work = self.work.0.lock().unwrap();
        work.closed = true;
        self.work.1.notify_one();
        // Receivers drop next, releasing a worker blocked on a bounded result.
    }
}

fn next_work(shared: &SharedWork) -> Option<Request> {
    let mut work = shared.0.lock().unwrap();
    loop {
        if work.closed {
            return None;
        }
        if let Some(page) = work.texts.pop_front() {
            work.text_in_flight = Some(page);
            work.idle_trimmed = false;
            return Some(Request::Text(page));
        }
        if let Some(key) = work.renders.pop_front() {
            work.in_flight = Some(key);
            work.idle_trimmed = false;
            return Some(Request::Render(key));
        }
        // Search and text preparation share the background budget fairly;
        // opening search during first-time reflow must not wait for the book.
        if work.search_active && work.index_turn {
            if let Some(page) = work.indices.pop_front() {
                work.index_turn = false;
                work.idle_trimmed = false;
                return Some(Request::Index(page));
            }
        }
        if work.flow_active {
            if let Some(page) = work.flows.pop_front().or_else(|| work.repairs.pop_front()) {
                work.index_turn = true;
                work.idle_trimmed = false;
                return Some(Request::Flow(page));
            }
        }
        if work.search_active {
            if let Some(page) = work.indices.pop_front() {
                work.idle_trimmed = false;
                return Some(Request::Index(page));
            }
        }
        if work.idle_trimmed {
            work = shared.1.wait(work).unwrap();
        } else {
            let (next, timeout) = shared
                .1
                .wait_timeout(work, std::time::Duration::from_secs(2))
                .unwrap();
            work = next;
            if timeout.timed_out() {
                work.idle_trimmed = true;
                return Some(Request::Trim);
            }
        }
    }
}

fn load_opened(doc: &Document) -> Result<Opened, String> {
    let sizes = doc
        .pages()
        .iter()
        .map(|p| {
            let (w, h) = p.render_dimensions();
            (f32::from(w).max(1.0), f32::from(h).max(1.0))
        })
        .collect();
    let toc = nav::outline(doc)
        .into_iter()
        .map(|e| TocEntry {
            title: e.title,
            page: e.page.map(|p| p as u32),
            depth: e.depth.min(u8::MAX as usize) as u8,
        })
        .collect();
    Ok(Opened { sizes, toc })
}

fn extract_text(doc: &Document, page: u32) -> Result<PageText, String> {
    let page_ref = doc
        .pages()
        .get(page as usize)
        .ok_or("страница вне диапазона")?;
    let (w, h) = page_ref.render_dimensions();
    let cache = InterpreterCache::new();
    let transform = page_ref.initial_transform(true).to_kurbo();
    let mut ctx = Context::new(
        transform,
        Rect::new(0.0, 0.0, f64::from(w), f64::from(h)),
        &cache,
        page_ref.xref(),
        InterpreterSettings::default(),
    );
    let mut device = text::TextDevice::default();
    interpret_page(page_ref, &mut ctx, &mut device);
    let mut chars = Vec::new();
    let mut line_no = 0u32;
    let mut last_y = None;
    for g in &device.glyphs {
        if g.text.is_empty() {
            continue;
        }
        if last_y.is_some_and(|y: f64| (g.y - y).abs() > g.size.max(1.0) * 0.5) {
            line_no += 1;
        }
        last_y = Some(g.y);
        let width = g.advance.max(g.size * 0.25);
        for c in g.text.chars() {
            chars.push(Ch {
                c,
                r: [
                    g.x as f32,
                    (g.y - g.size * 0.8) as f32,
                    (g.x + width) as f32,
                    g.y as f32,
                ],
                line: line_no,
            });
        }
    }
    let links = nav::links(doc, page as usize)
        .into_iter()
        .filter_map(|l| {
            let target = match l.target {
                nav::LinkTarget::Page(p) => LinkTarget::Page(p as u32),
                nav::LinkTarget::Uri(s) => LinkTarget::Uri(s),
                nav::LinkTarget::Unresolved => return None,
            };
            Some(PageLink {
                r: [
                    l.rect[0] as f32,
                    l.rect[1] as f32,
                    l.rect[2] as f32,
                    l.rect[3] as f32,
                ],
                target,
            })
        })
        .collect();
    Ok(PageText { chars, links })
}

fn extract_plain(doc: &Document, page: u32) -> Result<String, String> {
    let page_ref = doc
        .pages()
        .get(page as usize)
        .ok_or("страница вне диапазона")?;
    let (w, h) = page_ref.render_dimensions();
    let cache = InterpreterCache::new();
    let mut ctx = Context::new(
        page_ref.initial_transform(true).to_kurbo(),
        Rect::new(0.0, 0.0, f64::from(w), f64::from(h)),
        &cache,
        page_ref.xref(),
        InterpreterSettings::default(),
    );
    let mut device = text::TextDevice::default();
    interpret_page(page_ref, &mut ctx, &mut device);
    Ok(text::to_lines(&device.glyphs))
}

/// hayro owns the active document data; there is no process-global store to trim.
/// The worker calls this after dropping/reopening documents to keep its lifecycle explicit.
fn relieve_memory() {
    // TODO: measure hayro's retained allocations on long documents before adding cache controls.
}

fn extract_flow(doc: &Document, page: u32) -> Result<Vec<Para>, String> {
    let page_ref = doc
        .pages()
        .get(page as usize)
        .ok_or("страница вне диапазона")?;
    let (w, h) = page_ref.render_dimensions();
    let height = f32::from(h);
    let cache = InterpreterCache::new();
    let mut ctx = Context::new(
        page_ref.initial_transform(true).to_kurbo(),
        Rect::new(0.0, 0.0, f64::from(w), f64::from(h)),
        &cache,
        page_ref.xref(),
        InterpreterSettings::default(),
    );
    let mut device = text::TextDevice::default();
    interpret_page(page_ref, &mut ctx, &mut device);
    if device.glyphs.iter().all(|g| g.text.is_empty()) {
        return Ok(Vec::new());
    }
    let mut rows: Vec<Vec<&text::Glyph>> = Vec::new();
    let mut glyphs: Vec<_> = device
        .glyphs
        .iter()
        .filter(|g| !g.text.is_empty())
        .collect();
    glyphs.sort_by(|a, b| a.y.total_cmp(&b.y).then(a.x.total_cmp(&b.x)));
    for g in glyphs {
        if let Some(row) = rows
            .last_mut()
            .filter(|row| (row[0].y - g.y).abs() < g.size.max(1.0) * 0.5)
        {
            row.push(g)
        } else {
            rows.push(vec![g])
        }
    }
    let lines: Vec<String> = rows
        .iter()
        .map(|row| {
            let refs: Vec<_> = row.iter().map(|g| (*g).clone()).collect();
            text::to_lines(&refs)
        })
        .collect();
    let size = device.glyphs.iter().map(|g| g.size as f32).sum::<f32>()
        / device.glyphs.len().max(1) as f32;
    let blocks = vec![RawBlock {
        lines,
        size,
        y0: 0.0,
        y1: height,
        line_bounds: Vec::new(),
        mono: false,
        indents: Vec::new(),
    }];
    let mut paras = flow::paragraphs(&blocks, height);
    for para in &mut paras {
        para.repair(&flow::Vocab::default());
    }
    Ok(paras)
}

/// Reflow text of every page, with stray spaces inside words repaired from
/// the book's own vocabulary. May swap `doc` for a fresh handle to shed caches.
#[cfg(test)]
fn repaired_flow(doc: &mut Document, path: &Path) -> Vec<Vec<Para>> {
    let pages = doc.pages().len() as u32;
    let mut raw = Vec::with_capacity(pages as usize);
    for page in 0..pages {
        raw.push(extract_flow(doc, page).unwrap_or_default());
        if (page + 1) % 25 == 0 {
            if let Ok(fresh) = open(path) {
                *doc = fresh;
                relieve_memory();
            }
        }
    }
    let vocab = flow::Vocab::build(raw.iter().flatten().map(|p| p.text.as_str()));
    for para in raw.iter_mut().flatten() {
        para.repair(&vocab);
    }
    raw
}

/// A small per-document cache of parsed drawing commands. Tiles reuse these
/// instead of parsing the same PDF page for every rectangle.
struct Rasterizer;

impl Rasterizer {
    fn render(&mut self, doc: &Document, key: RasterKey) -> Result<Rendered, String> {
        let page = doc
            .pages()
            .get(key.page as usize)
            .ok_or("страница вне диапазона")?;
        if page.raw().contains_key("Contents") && page.page_stream().is_none() {
            return Err("page content stream could not be parsed".into());
        }
        let scale = key.factor();
        let cache = RenderCache::new();
        let pixmap = hayro::render(
            page,
            &cache,
            &InterpreterSettings::default(),
            &RenderSettings::default(),
            &PixmapSettings {
                x_scale: scale,
                y_scale: scale,
                bg_color: WHITE,
            },
        );
        let (w, h) = (u32::from(pixmap.width()), u32::from(pixmap.height()));
        let rgba = pixmap.data_as_u8_slice();
        let full = to_bgra(w as usize, h as usize, rgba, key.tone);
        if let Some(t) = key.tile {
            let x = t.x.min(w);
            let y = t.y.min(h);
            let tw = t.width.min(w - x);
            let th = t.height.min(h - y);
            let mut bgra = Vec::with_capacity(tw as usize * th as usize * 4);
            for row in y..y + th {
                let start = ((row * w + x) * 4) as usize;
                bgra.extend_from_slice(&full.bgra[start..start + (tw * 4) as usize]);
            }
            Ok(Rendered {
                width: tw,
                height: th,
                bgra,
            })
        } else {
            Ok(full)
        }
    }
}

/// Disk entries are keyed by source identity and transformation version.
/// Only one page and the vocabulary need to be resident on the worker.
struct TextCache {
    dir: Option<PathBuf>,
    vocab: flow::Vocab,
    known: std::collections::HashSet<u32>,
    ready: bool,
}

impl TextCache {
    fn new(path: &Path) -> Self {
        use std::hash::{Hash, Hasher};
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        path.hash(&mut hash);
        if let Ok(meta) = std::fs::metadata(path) {
            meta.len().hash(&mut hash);
            meta.modified().ok().hash(&mut hash);
        }
        let root = std::env::var_os("LIBER_PDF_CACHE_DIR")
            .map(PathBuf::from)
            .or_else(|| dirs::cache_dir().map(|root| root.join("Liber/pdf-v7")));
        let dir = root
            .map(|root| root.join(format!("{:016x}", hash.finish())))
            .filter(|dir| std::fs::create_dir_all(dir).is_ok());
        let vocab = dir
            .as_ref()
            .and_then(|p| std::fs::read(p.join("vocab.json")).ok())
            .and_then(|data| serde_json::from_slice(&data).ok());
        let ready = vocab.is_some();
        Self {
            dir,
            vocab: vocab.unwrap_or_default(),
            known: Default::default(),
            ready,
        }
    }

    fn read<T: serde::de::DeserializeOwned>(&self, name: &str) -> Option<T> {
        let data = std::fs::read(self.dir.as_ref()?.join(name)).ok()?;
        serde_json::from_slice(&data).ok()
    }

    fn write<T: serde::Serialize>(&self, name: &str, value: &T) {
        let Some(dir) = &self.dir else { return };
        let Ok(data) = serde_json::to_vec(value) else {
            return;
        };
        let path = dir.join(name);
        let tmp = path.with_extension(format!(
            "{}.{}.tmp",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        if std::fs::write(&tmp, data).is_ok() {
            let _ = std::fs::rename(tmp, path);
        }
    }

    fn flow(&mut self, doc: &Document, page: u32) -> Vec<Para> {
        let name = format!("flow-{page}.json");
        let mut raw: Vec<Para> = self.read(&name).unwrap_or_else(|| {
            let raw = extract_flow(doc, page).unwrap_or_default();
            self.write(&name, &raw);
            raw
        });
        if !self.ready && self.known.insert(page) {
            for para in &raw {
                self.vocab.extend(&para.text);
            }
        }
        // Early pages appear immediately. Once the whole vocabulary is ready,
        // a second incremental pass repairs them using the same book-wide rules.
        if self.ready {
            for para in &mut raw {
                para.repair(&self.vocab);
            }
        }
        raw
    }

    fn plain(&self, doc: &Document, page: u32) -> String {
        let name = format!("index-{page}.json");
        self.read(&name).unwrap_or_else(|| {
            let text = extract_plain(doc, page).unwrap_or_default();
            self.write(&name, &text);
            text
        })
    }
}

fn worker(
    path: PathBuf,
    shared: SharedWork,
    opened: Output<Result<Opened, String>>,
    replies: Output<Reply>,
    texts: Output<(u32, PageText)>,
    flows: Output<(u32, Vec<Para>)>,
    index: Output<(u32, String)>,
) {
    struct Cleanup;
    impl Drop for Cleanup {
        fn drop(&mut self) {
            relieve_memory();
        }
    }
    let _cleanup = Cleanup;
    let mut doc = match open(&path) {
        Ok(doc) => doc,
        Err(e) => {
            let _ = opened.send(Err(e));
            return;
        }
    };
    let mut cache = TextCache::new(&path);
    let info = match cache.read::<Opened>("opened.json") {
        Some(info) => Ok(info),
        None => {
            let info = load_opened(&doc);
            if let Ok(info) = &info {
                cache.write("opened.json", info);
            }
            // This initial metadata walk touches every page dictionary. Release
            // those document-owned objects once, then reuse the reading handle.
            if let Ok(fresh) = open(&path) {
                doc = fresh;
            }
            info
        }
    };
    let pages = info.as_ref().map_or(0, |i| i.sizes.len());
    if opened.send(info).is_err() {
        return;
    }
    let mut rasterizer = Rasterizer;
    let mut touched = std::collections::HashSet::new();
    while let Some(request) = next_work(&shared) {
        if let Some(page) = match &request {
            Request::Render(key) => Some(key.page),
            Request::Text(page) | Request::Flow(page) | Request::Index(page) => Some(*page),
            Request::Trim => None,
        } {
            touched.insert(page);
        }
        // PDF xref/object caches belong to Document, not the shared store.
        // Bound both layers. Repeated tiles of the same page do not rotate it.
        if touched.len() >= 16 {
            if let Ok(fresh) = open(&path) {
                doc = fresh;
                touched.clear();
                relieve_memory();
            }
        }
        match request {
            Request::Render(key) => {
                let image = rasterizer.render(&doc, key);
                let wanted = {
                    let mut work = shared.0.lock().unwrap();
                    work.in_flight = None;
                    let wanted = !work.closed && work.wanted.contains(&key);
                    if wanted {
                        work.delivered.insert(key);
                    }
                    wanted
                };
                if wanted
                    && replies
                        .send(Reply {
                            page: key.page,
                            scale: key.factor(),
                            tone: key.tone,
                            tile: key.tile,
                            image,
                        })
                        .is_err()
                {
                    return;
                }
            }
            Request::Text(page) => {
                let text = extract_text(&doc, page).unwrap_or_default();
                shared.0.lock().unwrap().text_in_flight = None;
                if texts.send((page, text)).is_err() {
                    return;
                }
            }
            Request::Flow(page) => {
                let paras = cache.flow(&doc, page);
                if flows.send((page, paras)).is_err() {
                    return;
                }
                if !cache.ready && cache.known.len() == pages {
                    cache.ready = true;
                    cache.write("vocab.json", &cache.vocab);
                    shared.0.lock().unwrap().repairs = (0..pages as u32).collect();
                }
            }
            Request::Index(page) => {
                if index.send((page, cache.plain(&doc, page))).is_err() {
                    return;
                }
            }
            Request::Trim => {
                relieve_memory();
            }
        }
    }
    drop(rasterizer);
    drop(doc);
}

#[cfg(test)]
pub(crate) mod test_pdf {
    /// One page showing `text` in Helvetica at (20, 250).
    pub fn with_text(text: &str) -> Vec<u8> {
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        let stream = format!("BT /F1 24 Tf 20 250 Td ({text}) Tj ET");
        let bodies = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_string(),
            format!("<< /Length {} >>\nstream\n{stream}\nendstream", stream.len()),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        ];
        for (i, body) in bodies.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes(),
        );
        for o in &offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                offsets.len() + 1
            )
            .as_bytes(),
        );
        out
    }

    /// A valid one-page PDF with the given title, built by hand so tests do
    /// not depend on fixtures.
    pub fn minimal(title: &str, pages: usize) -> Vec<u8> {
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        let mut obj = |out: &mut Vec<u8>, body: String| {
            offsets.push(out.len());
            out.extend_from_slice(
                format!("{} 0 obj\n{}\nendobj\n", offsets.len(), body).as_bytes(),
            );
        };
        obj(&mut out, "<< /Type /Catalog /Pages 2 0 R >>".into());
        let kids: String = (0..pages).map(|i| format!("{} 0 R ", 4 + i)).collect();
        obj(
            &mut out,
            format!("<< /Type /Pages /Kids [{kids}] /Count {pages} >>"),
        );
        obj(
            &mut out,
            format!("<< /Title ({title}) /Author (Test Author) >>"),
        );
        for _ in 0..pages {
            obj(
                &mut out,
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 300] >>".into(),
            );
        }
        let xref = out.len();
        out.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes(),
        );
        for o in &offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R /Info 3 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                offsets.len() + 1
            )
            .as_bytes(),
        );
        out
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn real_book_source_lines_and_technical_text() {
        let Some(path) = std::env::var_os("LIBER_TEST_PDF").map(std::path::PathBuf::from) else {
            return;
        };
        let doc = super::open(&path).unwrap();
        let paras = super::extract_flow(&doc, 48).unwrap();
        assert!(paras.iter().any(|p| p.text.contains("IEEE 754")));
        assert!(paras.iter().any(|p| p.text.contains("var")));
        for p in &paras {
            for line in &p.source {
                assert!(
                    p.text.is_char_boundary(line.start),
                    "bad start {line:?}: {}",
                    p.text
                );
                assert!(
                    p.text.is_char_boundary(line.end),
                    "bad end {line:?}: {}",
                    p.text
                );
            }
        }
        let codes: Vec<_> = (49..53)
            .flat_map(|page| super::extract_flow(&doc, page).unwrap())
            .filter(|p| p.kind == crate::flow::Kind::Code)
            .collect();
        let numeric_page = super::extract_flow(&doc, 49).unwrap();
        assert!(numeric_page
            .iter()
            .any(|p| p.text.starts_with("3.2.1.") && p.kind == crate::flow::Kind::Heading(3)));
        assert!(!codes.is_empty());
        assert!(
            codes.iter().any(|p| p.text.contains("0b10101")),
            "binary OCR must agree with => 21"
        );
        assert!(
            codes.iter().any(|p| p.text.contains("0o377")),
            "octal OCR must agree with => 255"
        );
        assert!(
            codes.iter().any(|p| p.text.starts_with("0\n3\n")),
            "numeric OCR column must stay multiline"
        );
        assert!(codes.iter().any(|p| p
            .syntax
            .iter()
            .any(|s| s.token == crate::syntax::Token::Comment)));
        println!(
            "Technical page: {} paragraphs, {} source lines; {} code blocks on pages 50–53",
            paras.len(),
            paras.iter().map(|p| p.source.len()).sum::<usize>(),
            codes.len()
        );
    }
    use super::*;

    fn temp_pdf(name: &str, pages: usize) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("liber-pdf-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("book.pdf");
        std::fs::write(&path, test_pdf::minimal("Хоббит", pages)).unwrap();
        path
    }

    #[test]
    fn probe_reads_metadata_and_page_count() {
        let path = temp_pdf("probe", 3);
        let meta = probe(&path).unwrap();
        assert_eq!(meta.pages, 3);
        assert_eq!(meta.title.as_deref(), Some("Хоббит"));
        assert_eq!(meta.author.as_deref(), Some("Test Author"));
    }

    #[test]
    fn probe_rejects_non_pdf() {
        let path = std::env::temp_dir().join(format!("liber-pdf-{}-junk.pdf", std::process::id()));
        std::fs::write(&path, b"not a pdf at all").unwrap();
        assert!(probe(&path).is_err());
    }

    #[test]
    fn renderer_rejects_unresolvable_content_instead_of_showing_white_page() {
        let path = std::env::temp_dir().join(format!(
            "liber-pdf-{}-broken-content.pdf",
            std::process::id()
        ));
        let mut bytes = test_pdf::with_text("should not render");
        let needle = b"/Contents 4 0 R";
        let at = bytes
            .windows(needle.len())
            .position(|w| w == needle)
            .unwrap();
        bytes[at + needle.len() - 5] = b'9';
        std::fs::write(&path, bytes).unwrap();
        let renderer = Renderer::spawn(path);
        renderer.opened.recv().unwrap().unwrap();
        renderer.request(0, 1.0, Tone::Day);
        assert!(renderer.replies.recv().unwrap().image.is_err());
    }

    #[test]
    fn renderer_reports_sizes_and_renders_bgra() {
        let path = temp_pdf("render", 2);
        let renderer = Renderer::spawn(path);
        let opened = renderer.opened.recv().unwrap().unwrap();
        assert_eq!(opened.sizes, vec![(200.0, 300.0); 2]);
        renderer.request(1, 2.0, Tone::Day);
        let reply = renderer.replies.recv().unwrap();
        assert_eq!((reply.page, reply.scale), (1, 2.0));
        let image = reply.image.unwrap();
        assert_eq!((image.width, image.height), (400, 600));
        assert_eq!(image.bgra.len(), 400 * 600 * 4);
        // A blank page is white and opaque.
        assert_eq!(&image.bgra[..4], &[255, 255, 255, 255]);
    }

    #[test]
    fn text_layer_gives_glyph_boxes_in_page_space() {
        let dir = std::env::temp_dir().join(format!("liber-pdf-{}-text", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.pdf");
        std::fs::write(&path, test_pdf::with_text("Hello world")).unwrap();
        let renderer = Renderer::spawn(path);
        renderer.opened.recv().unwrap().unwrap();
        renderer.request_text(0);
        let (page, text) = renderer.texts.recv().unwrap();
        assert_eq!(page, 0);
        assert_eq!(text.text(0, text.chars.len() - 1), "Hello world");
        let h = &text.chars[0];
        // Baseline at y=250 from the bottom of a 300pt page: glyph sits near y=50 from the top.
        assert!(h.r[0] >= 19.0 && h.r[0] < 25.0, "{:?}", h.r);
        assert!(h.r[1] > 30.0 && h.r[3] < 60.0, "{:?}", h.r);
        let hit = text.hit(h.r[0] + 1.0, (h.r[1] + h.r[3]) / 2.0);
        assert_eq!(hit, Some(0));
    }

    #[test]
    fn night_and_sepia_tones_recolour_paper() {
        let path = temp_pdf("tone", 1);
        let renderer = Renderer::spawn(path);
        renderer.opened.recv().unwrap().unwrap();
        renderer.request(0, 1.0, Tone::Night);
        let night = renderer.replies.recv().unwrap().image.unwrap();
        assert!(
            night.bgra[0] < 60 && night.bgra[2] < 60,
            "{:?}",
            &night.bgra[..4]
        );
        renderer.request(0, 1.0, Tone::Sepia);
        let sepia = renderer.replies.recv().unwrap().image.unwrap();
        assert_eq!(&sepia.bgra[..4], &[0xD8, 0xEC, 0xF4, 255]);
    }

    /// Run with `LIBER_TEST_PDF=/path/to/book.pdf cargo test real_book -- --nocapture`.
    #[test]
    fn real_book_opens_renders_and_has_text() {
        let Some(path) = std::env::var_os("LIBER_TEST_PDF").map(PathBuf::from) else {
            return;
        };
        let meta = probe(&path).unwrap();
        println!("{meta:?}");
        let renderer = Renderer::spawn(path);
        let opened = renderer.opened.recv().unwrap().unwrap();
        println!(
            "pages={} toc={} with_page={} first={:?}",
            opened.sizes.len(),
            opened.toc.len(),
            opened.toc.iter().filter(|t| t.page.is_some()).count(),
            &opened.toc[..4.min(opened.toc.len())]
        );
        let t = std::time::Instant::now();
        renderer.request(40, 2.0, Tone::Day);
        renderer.request_text(40);
        let (_, text) = renderer.texts.recv().unwrap();
        let img = renderer.replies.recv().unwrap().image.unwrap();
        println!(
            "page40 {}x{} chars={} in {:?}",
            img.width,
            img.height,
            text.chars.len(),
            t.elapsed()
        );
        assert!(!text.chars.is_empty());
        renderer.request_text(5);
        let (_, toc_page) = renderer.texts.recv().unwrap();
        println!(
            "page5 links={} first={:?}",
            toc_page.links.len(),
            toc_page.links.first()
        );
    }

    fn rss_mb() -> f64 {
        let out = std::process::Command::new("ps")
            .args(["-o", "rss=", "-p", &std::process::id().to_string()])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout)
            .trim()
            .parse::<f64>()
            .unwrap_or(0.0)
            / 1024.0
    }

    #[test]
    fn real_book_reflow_is_repaired() {
        let Some(path) = std::env::var_os("LIBER_TEST_PDF").map(PathBuf::from) else {
            return;
        };
        let mut doc = open(&path).unwrap();
        let book = repaired_flow(&mut doc, &path);
        let all: String = book
            .iter()
            .flatten()
            .map(|p| p.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write("/tmp/liber-fixed.txt", &all).unwrap();
        for bad in ["n u ll", "O bject", "сим вол", "S tr in g"] {
            println!("REPAIR {bad}: {}", all.matches(bad).count());
        }
    }

    /// `LIBER_TEST_PDF=… cargo test --release bench_real_book -- --nocapture`
    #[test]
    fn bench_real_book() {
        use std::time::Instant;
        let Some(path) = std::env::var_os("LIBER_TEST_PDF").map(PathBuf::from) else {
            return;
        };
        let base = rss_mb();
        let t = Instant::now();
        let meta = probe(&path).unwrap();
        println!(
            "BENCH import_probe_ms={:.0} pages={}",
            t.elapsed().as_secs_f64() * 1e3,
            meta.pages
        );
        let cover = std::env::temp_dir().join("liber-bench-cover.png");
        let t = Instant::now();
        save_cover(&path, &cover, 360.0).unwrap();
        println!("BENCH cover_ms={:.0}", t.elapsed().as_secs_f64() * 1e3);
        let t = Instant::now();
        let renderer = Renderer::spawn(path);
        let opened = renderer.opened.recv().unwrap().unwrap();
        println!(
            "BENCH open_ms={:.0} toc={}",
            t.elapsed().as_secs_f64() * 1e3,
            opened.toc.len()
        );
        let t = Instant::now();
        renderer.request(0, 2.0, Tone::Day);
        renderer.replies.recv().unwrap().image.unwrap();
        println!(
            "BENCH first_page_ms={:.0} rss={:.0}",
            t.elapsed().as_secs_f64() * 1e3,
            rss_mb()
        );
        let n = 60u32;
        let t = Instant::now();
        for p in 100..100 + n {
            renderer.request(p, 2.0, Tone::Night);
            renderer.replies.recv().unwrap().image.unwrap();
        }
        println!(
            "BENCH page_render_ms_avg={:.1} rss={:.0}",
            t.elapsed().as_secs_f64() * 1e3 / f64::from(n),
            rss_mb()
        );
        let t = Instant::now();
        for p in 0..opened.sizes.len() as u32 {
            renderer.request_flow(p);
        }
        for _ in 0..opened.sizes.len() {
            renderer.flows.recv().unwrap();
        }
        println!(
            "BENCH reflow_all_pages_ms={:.0}",
            t.elapsed().as_secs_f64() * 1e3
        );
        renderer.set_activity(false, true);
        while renderer.flows.try_recv().is_ok() {}
        let t = Instant::now();
        for p in 0..opened.sizes.len() as u32 {
            renderer.request_index(p);
        }
        let mut chars = 0;
        for _ in 0..opened.sizes.len() {
            chars += renderer.index.recv().unwrap().1.len();
        }
        println!(
            "BENCH index_all_pages_ms={:.0} text_kb={}",
            t.elapsed().as_secs_f64() * 1e3,
            chars / 1024
        );
        println!("BENCH rss_before_mb={base:.0} rss_after_mb={:.0}", rss_mb());
    }

    /// Run explicitly on the user's book. Consumes bounded results as the GUI
    /// does, and records sampled worker RSS through the entire extraction pass.
    #[test]
    fn real_book_incremental_flow_memory() {
        let Some(path) = std::env::var_os("LIBER_TEST_PDF").map(PathBuf::from) else {
            return;
        };
        let renderer = Renderer::spawn(path);
        let opened = renderer.opened.recv().unwrap().unwrap();
        let start = std::time::Instant::now();
        for page in 0..opened.sizes.len() as u32 {
            renderer.request_flow(page);
        }
        let mut sampled_peak = rss_mb();
        let mut bytes = 0usize;
        for ix in 0..opened.sizes.len() {
            let (_, paragraphs) = renderer
                .flows
                .recv_timeout(std::time::Duration::from_secs(20))
                .unwrap();
            bytes += paragraphs.iter().map(|p| p.text.len()).sum::<usize>();
            if ix % 16 == 0 {
                sampled_peak = sampled_peak.max(rss_mb());
            }
        }
        assert!(bytes > 100_000, "the full book must actually be extracted");
        println!("STREAM_FLOW pages={} text_bytes={} elapsed_ms={:.0} sampled_peak_rss_mib={sampled_peak:.1}",
            opened.sizes.len(), bytes, start.elapsed().as_secs_f64()*1000.0);
        assert!(
            sampled_peak < 600.0,
            "worker RSS exceeded guardrail: {sampled_peak:.1} MiB"
        );
    }

    #[test]
    fn clipped_tiles_match_full_page_pixels_and_bound_allocation() {
        let path = std::env::temp_dir().join(format!("liber-tile-{}.pdf", std::process::id()));
        std::fs::write(&path, test_pdf::with_text("Hello tiled PDF")).unwrap();
        let doc = open(&path).unwrap();
        let mut raster = Rasterizer;
        let full_key = RasterKey {
            page: 0,
            scale: 100,
            tone: Tone::Day,
            tile: None,
        };
        for tone in [Tone::Day, Tone::Night, Tone::Sepia] {
            let full = raster.render(&doc, RasterKey { tone, ..full_key }).unwrap();
            let tile = Tile {
                x: 25,
                y: 35,
                width: 175,
                height: 181,
            };
            let clipped = raster
                .render(
                    &doc,
                    RasterKey {
                        tone,
                        tile: Some(tile),
                        ..full_key
                    },
                )
                .unwrap();
            assert_eq!(
                clipped.bgra.len(),
                tile.width as usize * tile.height as usize * 4
            );
            for row in 0..tile.height as usize {
                let start = ((tile.y as usize + row) * full.width as usize + tile.x as usize) * 4;
                let len = tile.width as usize * 4;
                assert_eq!(
                    &clipped.bgra[row * len..(row + 1) * len],
                    &full.bgra[start..start + len],
                    "tone {tone:?}, row {row}"
                );
            }
        }
    }

    #[test]
    fn viewport_snapshot_replaces_stale_zoom_and_worker_shuts_down() {
        let renderer = Renderer::spawn(temp_pdf("cancel", 2));
        renderer.opened.recv().unwrap().unwrap();
        let tile = Tile {
            x: 0,
            y: 0,
            width: 128,
            height: 128,
        };
        for scale in 100..300 {
            let key = RasterKey {
                page: 0,
                scale,
                tone: Tone::Day,
                tile: Some(tile),
            };
            renderer.set_view(&[key], &[key], &[]);
        }
        let final_key = RasterKey {
            page: 1,
            scale: 300,
            tone: Tone::Night,
            tile: Some(tile),
        };
        renderer.set_view(&[final_key], &[final_key], &[]);
        {
            let work = renderer.work.0.lock().unwrap();
            assert!(work.renders.iter().all(|key| *key == final_key));
            assert_eq!(work.wanted.len(), 1);
        }
        loop {
            let reply = renderer
                .replies
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            if reply.key() == final_key {
                assert!(reply.image.is_ok());
                break;
            }
        }
        let shared = renderer.work.clone();
        drop(renderer);
        assert!(shared.0.lock().unwrap().closed);
    }

    #[test]
    fn disk_text_cache_preserves_repaired_paragraphs_and_source_identity() {
        let path = std::env::temp_dir().join(format!("liber-disk-{}.pdf", std::process::id()));
        std::fs::write(&path, test_pdf::with_text("Hello world")).unwrap();
        let doc = open(&path).unwrap();
        let mut cache = TextCache::new(&path);
        let first = cache.flow(&doc, 0);
        let text = cache.plain(&doc, 0);
        assert!(text.contains("Hello world"));
        cache.ready = true;
        cache.write("vocab.json", &cache.vocab);
        let mut reopened = TextCache::new(&path);
        assert!(reopened.ready);
        assert_eq!(first, reopened.flow(&doc, 0));
        let old_dir = reopened.dir.clone();
        std::fs::write(&path, test_pdf::with_text("Changed document content")).unwrap();
        assert_ne!(TextCache::new(&path).dir, old_dir);
    }

    #[test]
    fn cover_is_written_as_png() {
        let path = temp_pdf("cover", 1);
        let png = path.with_extension("png");
        save_cover(&path, &png, 120.0).unwrap();
        assert_eq!(&std::fs::read(&png).unwrap()[1..4], b"PNG");
    }
}
