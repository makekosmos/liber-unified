//! UI-independent PDF, library, and reading progress services used by Liber.
use mupdf::{
    Colorspace, Document, Matrix, MetadataName, TextBlockContent, TextExtractOptions, TextPageFlags,
};
pub mod notes;
mod syntax;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender},
    },
};

const INDEX: &str = "library.json";
const PAGE_CACHE_LIMIT: usize = 16;
const DOCUMENT_PAGE_LIMIT: usize = 16;
const DISPLAY_LIST_CACHE_LIMIT: usize = 2;
const MAX_RENDER_PIXELS: f64 = 12_000_000.0;
const PREVIEW_RENDER_PIXELS: f64 = 1_000_000.0;
const TILE_PIXELS: u32 = 510;
const RECENT_PAGE_FILES: usize = 16;
const RECENT_PREVIEW_FILES: usize = 48;
/// Width of the reading column in points at zoom 1, as the UI lays it out.
pub const READING_WIDTH_PT: f32 = 680.0;
/// Device pixels per point on Retina displays; tiles are rendered at this density.
const RETINA_SCALE: f64 = 2.0;
type PageCacheKey = (String, usize, u64, u128, u32, u8);
static PAGE_CACHE: OnceLock<Mutex<PageCache>> = OnceLock::new();
static RECENT_PAGE_FILES_LIST: Mutex<VecDeque<Vec<PathBuf>>> = Mutex::new(VecDeque::new());
static RECENT_PREVIEW_FILES_LIST: Mutex<VecDeque<PathBuf>> = Mutex::new(VecDeque::new());
type DocumentCacheKey = (String, u64, u128);

struct RenderRequest {
    path: PathBuf,
    index: usize,
    zoom: f32,
    tone: PageTone,
    key: DocumentCacheKey,
    cancel: Arc<AtomicBool>,
    preview_only: bool,
    reply: SyncSender<Result<Page, String>>,
}

static PDF_RENDERER: OnceLock<Sender<RenderRequest>> = OnceLock::new();
static PDF_PREVIEW_RENDERER: OnceLock<Sender<RenderRequest>> = OnceLock::new();

#[derive(Default)]
struct PageCache {
    pages: HashMap<PageCacheKey, Page>,
    order: VecDeque<PageCacheKey>,
}

impl PageCache {
    fn get(&mut self, key: &PageCacheKey) -> Option<Page> {
        let page = self.pages.get(key)?.clone();
        self.order.retain(|cached| cached != key);
        self.order.push_back(key.clone());
        Some(page)
    }

    fn insert(&mut self, key: PageCacheKey, page: Page) {
        self.order.retain(|cached| cached != &key);
        self.pages.insert(key.clone(), page);
        self.order.push_back(key);
        while self.order.len() > PAGE_CACHE_LIMIT {
            if let Some(oldest) = self.order.pop_front() {
                self.pages.remove(&oldest);
            }
        }
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize, Debug)]
pub struct Book {
    #[serde(default)]
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub author: String,
    pub path: String,
    pub pages: usize,
    pub last_page: usize,
    #[serde(default)]
    pub cover: String,
}

impl Book {
    pub fn progress(&self) -> f32 {
        if self.pages <= 1 {
            0.0
        } else {
            self.last_page.min(self.pages - 1) as f32 / (self.pages - 1) as f32
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Page {
    pub count: usize,
    pub png: String,
    pub text: String,
    pub text_runs: Vec<TextRun>,
    pub page_text: notes::PageText,
    pub toc: Vec<(String, usize, usize)>,
    pub size: (u32, u32),
    /// Original PDF page dimensions in points, used to position selectable text.
    pub points: (f32, f32),
    pub zoom: f32,
    pub tiles: Vec<PageTile>,
    pub tone: PageTone,
    /// False while the low-resolution image is visible and detail rendering continues.
    pub complete: bool,
}

// Flexoki reading colors adapted from https://stephango.com/flexoki (MIT).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PageTone {
    #[default]
    Day,
    Sepia,
    Gray,
    Night,
}

impl PageTone {
    pub fn next(self) -> Self {
        match self {
            Self::Day => Self::Sepia,
            Self::Sepia => Self::Gray,
            Self::Gray => Self::Night,
            Self::Night => Self::Day,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Day => "Светлая",
            Self::Sepia => "Тёплая",
            Self::Gray => "Тёмная",
            Self::Night => "Чернильная",
        }
    }

    pub fn palette(self) -> ReadingPalette {
        match self {
            Self::Day => ReadingPalette {
                paper: 0xFFFCF0,
                code_paper: 0xF2F0E5,
                ink: 0x100F0F,
                muted_ink: 0x6F6E69,
                accent: 0x205EA6,
                dark: false,
            },
            Self::Sepia => ReadingPalette {
                paper: 0xF2F0E5,
                code_paper: 0xE6E4D9,
                ink: 0x282726,
                muted_ink: 0x6F6E69,
                accent: 0x205EA6,
                dark: false,
            },
            Self::Gray => ReadingPalette {
                paper: 0x1C1B1A,
                code_paper: 0x100F0F,
                ink: 0xCECDC3,
                muted_ink: 0x878580,
                accent: 0x4385BE,
                dark: true,
            },
            Self::Night => ReadingPalette {
                paper: 0x100F0F,
                code_paper: 0x1C1B1A,
                ink: 0xCECDC3,
                muted_ink: 0x878580,
                accent: 0x4385BE,
                dark: true,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadingPalette {
    pub paper: u32,
    pub code_paper: u32,
    pub ink: u32,
    pub muted_ink: u32,
    pub accent: u32,
    pub dark: bool,
}

impl ReadingPalette {
    pub fn code_ink(self, token: CodeToken) -> u32 {
        use CodeToken::*;
        match (token, self.dark) {
            (Comment, false) => 0x6F6E69,
            (Comment, true) => 0x878580,
            (String, false) => 0x66800B,
            (String, true) => 0x879A39,
            (Number, false) => 0xBC5215,
            (Number, true) => 0xDA702C,
            (Keyword, false) => 0x5E409D,
            (Keyword, true) => 0x8B7EC8,
            (Type, false) => 0xAD8301,
            (Type, true) => 0xD0A215,
            (Function, false) => 0x205EA6,
            (Function, true) => 0x4385BE,
            (Operator, false) => 0x24837B,
            (Operator, true) => 0x3AA99F,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PageTile {
    pub png: String,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub scale: f32,
}

/// A selectable text line positioned over the rendered PDF page.
#[derive(Clone, Debug, PartialEq)]
pub struct TextRun {
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub size: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchHit {
    pub page: usize,
    pub excerpt: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReadingBlock {
    pub text: String,
    pub code: bool,
    #[serde(default)]
    pub syntax: Vec<CodeSpan>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CodeToken {
    Comment,
    String,
    Number,
    Keyword,
    Type,
    Function,
    Operator,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeSpan {
    pub start: usize,
    pub end: usize,
    pub token: CodeToken,
}

pub fn default_root() -> PathBuf {
    if let Some(path) = std::env::var_os("LIBER_DATA_DIR") {
        return PathBuf::from(path);
    }
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("Liber")
}

pub fn load_books() -> Vec<Book> {
    std::fs::read(default_root().join(INDEX))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub fn save_books(books: &[Book]) {
    let root = default_root();
    if std::fs::create_dir_all(&root).is_err() {
        return;
    }
    if let Ok(bytes) = serde_json::to_vec_pretty(books) {
        let temp = root.join(format!("{INDEX}.tmp"));
        if std::fs::write(&temp, bytes).is_ok() {
            let _ = std::fs::rename(temp, root.join(INDEX));
        }
    }
}

fn hash(bytes: &[u8]) -> String {
    let mut value = 0xcbf29ce484222325u64;
    for byte in bytes {
        value ^= u64::from(*byte);
        value = value.wrapping_mul(0x100000001b3);
    }
    format!("{value:016x}")
}

fn document_cache_key(path: &Path, metadata: &std::fs::Metadata) -> DocumentCacheKey {
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |duration| duration.as_nanos());
    (
        path.to_string_lossy().into_owned(),
        metadata.len(),
        modified,
    )
}

/// Copy a PDF into the local library, create its cover, and persist its record.
/// Identical file bytes resolve to the existing book.
pub fn import_pdf(source: &Path) -> Result<Book, String> {
    import_pdf_with_first_page(source).map(|(book, _)| book)
}

/// Import a PDF and return its first rendered page when the file is new.
/// Reusing that page avoids rendering the cover and reader page twice.
pub fn import_pdf_with_first_page(source: &Path) -> Result<(Book, Option<Page>), String> {
    let bytes =
        std::fs::read(source).map_err(|error| format!("Не удалось прочитать файл: {error}"))?;
    let id = hash(&bytes);
    let mut books = load_books();
    if let Some(book) = books.iter().find(|book| book.id == id) {
        return Ok((book.clone(), None));
    }
    let root = default_root();
    let book_path = root.join("books").join(format!("{id}.pdf"));
    let cover_path = root.join("covers").join(format!("{id}.png"));
    std::fs::create_dir_all(book_path.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(cover_path.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(&book_path, bytes).map_err(|e| e.to_string())?;
    let page = match render(&book_path, 0, false) {
        Ok(page) => page,
        Err(error) => {
            let _ = std::fs::remove_file(book_path);
            return Err(error);
        }
    };
    std::fs::copy(&page.png, &cover_path).map_err(|e| e.to_string())?;
    let document_path = book_path.to_string_lossy();
    let document = Document::open(document_path.as_ref())
        .map_err(|e| format!("Не удалось прочитать PDF metadata: {e}"))?;
    let fallback = source
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Без названия")
        .to_owned();
    let embedded = document
        .metadata(MetadataName::Title)
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    let author = document
        .metadata(MetadataName::Author)
        .unwrap_or_default()
        .trim()
        .to_owned();
    let book = Book {
        id,
        title: embedded.unwrap_or(fallback),
        author,
        path: book_path.to_string_lossy().into_owned(),
        pages: page.count,
        last_page: 0,
        cover: cover_path.to_string_lossy().into_owned(),
    };
    books.push(book.clone());
    save_books(&books);
    Ok((book, Some(page)))
}

pub fn search_books<'a>(books: &'a [Book], query: &str) -> Vec<&'a Book> {
    let needle = query.trim().to_lowercase();
    books
        .iter()
        .filter(|book| {
            needle.is_empty()
                || book.title.to_lowercase().contains(&needle)
                || book.author.to_lowercase().contains(&needle)
        })
        .collect()
}

pub fn set_progress(books: &mut [Book], id: &str, page: usize) {
    if let Some(book) = books.iter_mut().find(|book| book.id == id) {
        book.last_page = page.min(book.pages.saturating_sub(1));
        save_books(books);
    }
}

pub fn render(path: impl AsRef<Path>, index: usize, _dark: bool) -> Result<Page, String> {
    render_zoom(path, index, 1.0)
}

pub fn render_zoom(path: impl AsRef<Path>, index: usize, zoom: f32) -> Result<Page, String> {
    render_with_tone(path, index, zoom, PageTone::Day)
}

pub fn render_with_tone(
    path: impl AsRef<Path>,
    index: usize,
    zoom: f32,
    tone: PageTone,
) -> Result<Page, String> {
    render_cancellable(path, index, zoom, tone, &Arc::new(AtomicBool::new(false)))
}

/// Checks `cancel` before queued work and between rendering stages.
/// An active MuPDF raster pass cannot be interrupted.
pub fn render_cancellable(
    path: impl AsRef<Path>,
    index: usize,
    zoom: f32,
    tone: PageTone,
    cancel: &Arc<AtomicBool>,
) -> Result<Page, String> {
    render_cancellable_quality(path, index, zoom, tone, cancel, false)
}

/// Renders only the low-resolution page image so the reader can display it
/// while detail tiles and selectable text are prepared on the detail worker.
pub fn render_preview_cancellable(
    path: impl AsRef<Path>,
    index: usize,
    zoom: f32,
    tone: PageTone,
    cancel: &Arc<AtomicBool>,
) -> Result<Page, String> {
    render_cancellable_quality(path, index, zoom, tone, cancel, true)
}

fn render_cancellable_quality(
    path: impl AsRef<Path>,
    index: usize,
    zoom: f32,
    tone: PageTone,
    cancel: &Arc<AtomicBool>,
    preview_only: bool,
) -> Result<Page, String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("страница больше не нужна".into());
    }
    let path = path.as_ref();
    let zoom = zoom.clamp(0.25, 4.0);
    let metadata = std::fs::metadata(path).map_err(|e| e.to_string())?;
    let document_key = document_cache_key(path, &metadata);
    let cache_key = (
        document_key.0.clone(),
        index,
        document_key.1,
        document_key.2,
        (zoom * 100.0).round() as u32,
        tone as u8,
    );
    let page_cache = PAGE_CACHE.get_or_init(|| Mutex::new(PageCache::default()));
    if let Some(cached) = page_cache
        .lock()
        .ok()
        .and_then(|mut cache| cache.get(&cache_key))
        .filter(page_files_exist)
    {
        if !preview_only || cached.complete {
            return Ok(cached);
        }
    }

    let worker = if preview_only {
        &PDF_PREVIEW_RENDERER
    } else {
        &PDF_RENDERER
    };
    let renderer = worker.get_or_init(|| {
        let (sender, receiver) = mpsc::channel();
        std::thread::Builder::new()
            .name(if preview_only {
                "liber-mupdf-preview".into()
            } else {
                "liber-mupdf-detail".into()
            })
            .spawn(move || pdf_render_worker(receiver))
            .expect("failed to start MuPDF worker");
        sender
    });
    let (reply, result) = mpsc::sync_channel(1);
    renderer
        .send(RenderRequest {
            path: path.to_path_buf(),
            index,
            zoom,
            tone,
            key: document_key,
            cancel: Arc::clone(cancel),
            preview_only,
            reply,
        })
        .map_err(|e| format!("MuPDF worker остановлен: {e}"))?;
    let rendered = result
        .recv()
        .map_err(|e| format!("MuPDF worker не вернул страницу: {e}"))??;
    if cancel.load(Ordering::Relaxed) {
        return Err("страница больше не нужна".into());
    }
    if preview_only {
        keep_recent_preview_file(&rendered.png);
    } else {
        if let Ok(mut cache) = page_cache.lock() {
            cache.insert(cache_key, rendered.clone());
        }
        keep_recent_page_files(&rendered);
    }
    Ok(rendered)
}

fn page_files_exist(page: &Page) -> bool {
    std::iter::once(&page.png)
        .chain(page.tiles.iter().map(|tile| &tile.png))
        .all(|path| Path::new(path).exists())
}

fn keep_recent_page_files(page: &Page) {
    let files: Vec<PathBuf> = page
        .tiles
        .iter()
        .map(|tile| &tile.png)
        .map(PathBuf::from)
        .collect();
    let Ok(mut recent) = RECENT_PAGE_FILES_LIST.lock() else {
        return;
    };
    recent.push_back(files);
    while recent.len() > RECENT_PAGE_FILES {
        let Some(evicted) = recent.pop_front() else {
            break;
        };
        for path in evicted {
            if !recent.iter().any(|kept| kept.contains(&path)) {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

fn keep_recent_preview_file(path: &str) {
    let Ok(mut recent) = RECENT_PREVIEW_FILES_LIST.lock() else {
        return;
    };
    let path = PathBuf::from(path);
    recent.retain(|cached| cached != &path);
    recent.push_back(path);
    while recent.len() > RECENT_PREVIEW_FILES {
        if let Some(evicted) = recent.pop_front() {
            let _ = std::fs::remove_file(evicted);
        }
    }
}

fn pdf_render_worker(receiver: Receiver<RenderRequest>) {
    let mut opened: Option<(DocumentCacheKey, Document, Vec<(String, usize, usize)>)> = None;
    let mut display_lists: VecDeque<(usize, mupdf::DisplayList)> = VecDeque::new();
    let mut touched_pages = std::collections::HashSet::new();
    loop {
        let request = match receiver.recv_timeout(std::time::Duration::from_secs(2)) {
            Ok(request) => request,
            Err(RecvTimeoutError::Timeout) => {
                display_lists.clear();
                let _ = mupdf::Context::get().trim_store(0);
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };
        if request.cancel.load(Ordering::Relaxed) {
            let _ = request.reply.send(Err("страница больше не нужна".into()));
            continue;
        }
        if opened
            .as_ref()
            .is_none_or(|(key, _, _)| key != &request.key)
        {
            display_lists.clear();
            touched_pages.clear();
            let document_path = request.path.to_string_lossy();
            let result = Document::open(document_path.as_ref())
                .map_err(|e| format!("Не удалось открыть PDF: {e}"))
                .and_then(|document| {
                    let toc = document.outlines().unwrap_or_default();
                    let mut flattened = Vec::new();
                    flatten_outline(&toc, 0, &mut flattened);
                    Ok((document, flattened))
                });
            match result {
                Ok((document, toc)) => opened = Some((request.key.clone(), document, toc)),
                Err(error) => {
                    let _ = request.reply.send(Err(error));
                    continue;
                }
            }
        }

        let result = match opened.as_ref() {
            Some((_, document, toc)) => render_mupdf_page(
                document,
                toc,
                &request.key,
                request.index,
                request.zoom,
                request.tone,
                &mut display_lists,
                &request.cancel,
                request.preview_only,
            ),
            None => Err("Документ PDF не открыт".into()),
        };
        let _ = request.reply.send(result);
        touched_pages.insert(request.index);
        if touched_pages.len() >= DOCUMENT_PAGE_LIMIT {
            let path = opened.as_ref().map(|(key, _, _)| PathBuf::from(&key.0));
            if let Some(path) = path {
                if let Ok(document) = Document::open(path.to_string_lossy().as_ref()) {
                    let key = opened.as_ref().map(|(key, _, _)| key.clone()).unwrap();
                    let toc = opened.as_ref().map(|(_, _, toc)| toc.clone()).unwrap();
                    opened = Some((key, document, toc));
                    display_lists.clear();
                    touched_pages.clear();
                    let _ = mupdf::Context::get().trim_store(25);
                }
            }
        }
    }
}

fn flatten_outline(
    entries: &[mupdf::Outline],
    depth: usize,
    out: &mut Vec<(String, usize, usize)>,
) {
    if depth > 64 || out.len() >= 20_000 {
        return;
    }
    for entry in entries {
        if out.len() >= 20_000 {
            break;
        }
        if let Some(destination) = &entry.dest {
            out.push((
                entry.title.clone(),
                destination.loc.page_number as usize,
                depth,
            ));
        }
        flatten_outline(&entry.down, depth + 1, out);
    }
}

fn render_mupdf_page(
    document: &Document,
    toc: &[(String, usize, usize)],
    key: &DocumentCacheKey,
    index: usize,
    zoom: f32,
    tone: PageTone,
    display_lists: &mut VecDeque<(usize, mupdf::DisplayList)>,
    cancel: &AtomicBool,
    preview_only: bool,
) -> Result<Page, String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("страница больше не нужна".into());
    }
    let count = document
        .page_count()
        .map_err(|e| format!("Не удалось получить количество страниц PDF: {e}"))?
        .max(0) as usize;
    if index >= count || index > i32::MAX as usize {
        return Err("Страница не найдена".into());
    }
    let needs_display_list = !display_lists.iter().any(|(page, _)| *page == index);
    if needs_display_list {
        let page = document
            .load_page(index as i32)
            .map_err(|e| format!("Не удалось открыть страницу PDF: {e}"))?;
        let display_list = page
            .to_display_list(true)
            .map_err(|e| format!("Не удалось подготовить страницу PDF: {e}"))?;
        display_lists.push_back((index, display_list));
        while display_lists.len() > DISPLAY_LIST_CACHE_LIMIT {
            display_lists.pop_front();
        }
    }
    let list_index = display_lists
        .iter()
        .position(|(page, _)| *page == index)
        .unwrap();
    let display_list = display_lists.remove(list_index).unwrap();
    display_lists.push_back(display_list);
    let display_list = &display_lists.back().unwrap().1;
    let bounds = display_list.bounds();
    let point_width = (bounds.x1 - bounds.x0).max(1.0);
    let point_height = (bounds.y1 - bounds.y0).max(1.0);
    let identity = format!("{}:{}:{}", key.0, key.1, key.2);
    let name = hash(identity.as_bytes());
    let area = f64::from(point_width * point_height).max(1.0);
    let preview_scale = (1.25f64).min((PREVIEW_RENDER_PIXELS / area).sqrt()) as f32;
    let fit_scale =
        f64::from(READING_WIDTH_PT) * RETINA_SCALE * f64::from(zoom) / f64::from(point_width);
    let detail_scale = fit_scale.min((MAX_RENDER_PIXELS / area).sqrt()) as f32;
    let zoom_key = (zoom * 100.0).round() as u32;
    let tone_key = tone as u8;
    let preview = std::env::temp_dir().join(format!(
        "liber-mupdf-v5-{name}-{index}-t{tone_key}-preview.png"
    ));
    if image::image_dimensions(&preview).is_err() {
        if cancel.load(Ordering::Relaxed) {
            return Err("страница больше не нужна".into());
        }
        let mut matrix = Matrix::new_scale(preview_scale, preview_scale);
        matrix.pre_translate(-bounds.x0, -bounds.y0);
        let mut pixmap = display_list
            .to_pixmap(&matrix, &Colorspace::device_rgb(), false)
            .map_err(|e| format!("Не удалось отрисовать страницу PDF: {e}"))?;
        apply_tone(&mut pixmap, tone);
        pixmap
            .save_as(
                preview.to_str().ok_or("Некорректный путь временного PNG")?,
                mupdf::ImageFormat::PNG,
            )
            .map_err(|e| format!("Не удалось сохранить страницу PDF: {e}"))?;
    }
    let size = image::image_dimensions(&preview).map_err(|e| e.to_string())?;
    if preview_only {
        return Ok(Page {
            count,
            png: preview.to_string_lossy().into_owned(),
            text: String::new(),
            text_runs: Vec::new(),
            page_text: notes::PageText::default(),
            toc: toc.to_vec(),
            size,
            points: (point_width, point_height),
            zoom,
            tiles: Vec::new(),
            tone,
            complete: false,
        });
    }
    let mut tiles = Vec::new();
    let mut detail_pixmap: Option<mupdf::Pixmap> = None;
    if detail_scale > preview_scale + 0.01 {
        let pixel_width = (point_width * detail_scale).ceil().max(1.0) as u32;
        let pixel_height = (point_height * detail_scale).ceil().max(1.0) as u32;
        let mut matrix = Matrix::new_scale(detail_scale, detail_scale);
        matrix.pre_translate(-bounds.x0, -bounds.y0);
        for y in (0..pixel_height).step_by(TILE_PIXELS as usize) {
            for x in (0..pixel_width).step_by(TILE_PIXELS as usize) {
                if cancel.load(Ordering::Relaxed) {
                    return Err("страница больше не нужна".into());
                }
                let left = x.saturating_sub(1);
                let top = y.saturating_sub(1);
                let tile_width = (x + TILE_PIXELS + 1).min(pixel_width) - left;
                let tile_height = (y + TILE_PIXELS + 1).min(pixel_height) - top;
                let tile_path = std::env::temp_dir().join(format!(
                    "liber-mupdf-v5-{name}-{index}-t{tone_key}-z{zoom_key}-{left}-{top}.png"
                ));
                if image::image_dimensions(&tile_path).is_err() {
                    if detail_pixmap.is_none() {
                        let mut detail = mupdf::Pixmap::new(
                            &Colorspace::device_rgb(),
                            0,
                            0,
                            pixel_width as i32,
                            pixel_height as i32,
                            false,
                        )
                        .map_err(|e| format!("Не удалось создать изображение страницы PDF: {e}"))?;
                        detail.clear_with(255).map_err(|e| e.to_string())?;
                        {
                            let device = mupdf::Device::from_pixmap(&detail).map_err(|e| {
                                format!("Не удалось создать устройство рендера: {e}")
                            })?;
                            display_list
                                .run(
                                    &device,
                                    &matrix,
                                    mupdf::Rect::new(
                                        0.0,
                                        0.0,
                                        pixel_width as f32,
                                        pixel_height as f32,
                                    ),
                                )
                                .map_err(|e| format!("Не удалось отрисовать страницу PDF: {e}"))?;
                        }
                        if cancel.load(Ordering::Relaxed) {
                            return Err("страница больше не нужна".into());
                        }
                        apply_tone(&mut detail, tone);
                        detail_pixmap = Some(detail);
                    }
                    let detail = detail_pixmap.as_ref().unwrap();
                    let mut pixmap = mupdf::Pixmap::new(
                        &Colorspace::device_rgb(),
                        left as i32,
                        top as i32,
                        tile_width as i32,
                        tile_height as i32,
                        false,
                    )
                    .map_err(|e| format!("Не удалось создать тайл PDF: {e}"))?;
                    let components = detail.n() as usize;
                    if components != pixmap.n() as usize {
                        return Err("Несовпадение цветовых компонентов тайла PDF".into());
                    }
                    let source_stride = detail.stride().max(0) as usize;
                    let tile_stride = pixmap.stride().max(0) as usize;
                    let row_bytes = tile_width as usize * components;
                    let source_samples = detail.samples();
                    let tile_samples = pixmap.samples_mut();
                    for row in 0..tile_height as usize {
                        let source_start =
                            (top as usize + row) * source_stride + left as usize * components;
                        let tile_start = row * tile_stride;
                        tile_samples[tile_start..tile_start + row_bytes].copy_from_slice(
                            &source_samples[source_start..source_start + row_bytes],
                        );
                    }
                    pixmap
                        .save_as(
                            tile_path
                                .to_str()
                                .ok_or("Некорректный путь временного PNG")?,
                            mupdf::ImageFormat::PNG,
                        )
                        .map_err(|e| format!("Не удалось сохранить тайл PDF: {e}"))?;
                }
                tiles.push(PageTile {
                    png: tile_path.to_string_lossy().into_owned(),
                    x: left,
                    y: top,
                    width: tile_width,
                    height: tile_height,
                    scale: detail_scale,
                });
            }
        }
    }
    drop(detail_pixmap);
    let text_page = display_list
        .to_text_page(TextPageFlags::empty())
        .map_err(|e| format!("Не удалось извлечь текст страницы PDF: {e}"))?;
    let structured = text_page.structured();
    let mut text = String::new();
    let mut text_runs = Vec::new();
    let mut page_text = notes::PageText::default();
    let mut text_line = 0u32;
    for block in structured.blocks {
        if let TextBlockContent::Text { lines } = block.content {
            for line in lines {
                let value = line.text.trim_end().to_owned();
                if value.is_empty() {
                    continue;
                }
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&value);
                let font_size = line
                    .chars
                    .iter()
                    .map(|character| character.size)
                    .fold(0.0f32, f32::max)
                    .max(line.bounds.height()) as f64;
                text_runs.push(TextRun {
                    text: value,
                    x: f64::from(line.bounds.x0 - bounds.x0),
                    y: f64::from(line.bounds.y1 - bounds.y0),
                    size: font_size,
                });
                page_text.chars.extend(line.chars.iter().map(|character| {
                    let rect = mupdf::Rect::from(character.quad.clone());
                    notes::Ch {
                        c: character.ch,
                        r: [
                            rect.x0 - bounds.x0,
                            rect.y0 - bounds.y0,
                            rect.x1 - bounds.x0,
                            rect.y1 - bounds.y0,
                        ],
                        line: text_line,
                    }
                }));
                text_line = text_line.wrapping_add(1);
            }
        }
    }
    Ok(Page {
        count,
        png: preview.to_string_lossy().into_owned(),
        text,
        text_runs,
        page_text,
        toc: toc.to_vec(),
        size,
        points: (point_width, point_height),
        zoom,
        tiles,
        tone,
        complete: true,
    })
}

fn apply_tone(pixmap: &mut mupdf::Pixmap, tone: PageTone) {
    if tone == PageTone::Day {
        return;
    }
    let width = pixmap.width().max(0) as usize;
    let height = pixmap.height().max(0) as usize;
    let components = pixmap.n().max(0) as usize;
    let stride = pixmap.stride().max(0) as usize;
    if components < 3 || stride == 0 {
        return;
    }
    let palette = tone.palette();
    for row in pixmap.samples_mut().chunks_mut(stride).take(height) {
        for pixel in row.chunks_mut(components).take(width) {
            let (r, g, b) = (pixel[0], pixel[1], pixel[2]);
            let light = ((u16::from(r) * 77 + u16::from(g) * 150 + u16::from(b) * 29) / 256) as u8;
            let (r, g, b) = paper_tone(
                light,
                rgb_channels(palette.ink),
                rgb_channels(palette.paper),
            );
            pixel[0] = r;
            pixel[1] = g;
            pixel[2] = b;
        }
    }
}

fn rgb_channels(color: u32) -> [u8; 3] {
    [
        ((color >> 16) & 0xff) as u8,
        ((color >> 8) & 0xff) as u8,
        (color & 0xff) as u8,
    ]
}

/// Maps a pixel to ink on paper by its brightness: white stays paper, black becomes ink.
fn paper_tone(light: u8, ink: [u8; 3], paper: [u8; 3]) -> (u8, u8, u8) {
    let mix = |i: u8, p: u8| {
        ((u16::from(i) * u16::from(255 - light) + u16::from(p) * u16::from(light)) / 255) as u8
    };
    (
        mix(ink[0], paper[0]),
        mix(ink[1], paper[1]),
        mix(ink[2], paper[2]),
    )
}

/// Search each page's text layer and return one short excerpt per matching page.
pub fn search_pdf(path: impl AsRef<Path>, query: &str) -> Result<Vec<SearchHit>, String> {
    let needle: Vec<char> = query.trim().chars().flat_map(char::to_lowercase).collect();
    if needle.is_empty() {
        return Ok(Vec::new());
    }
    let source = path.as_ref();
    let metadata = std::fs::metadata(source).map_err(|e| e.to_string())?;
    let cache_dir = text_cache_dir(source, &metadata);
    let _ = std::fs::create_dir_all(&cache_dir);
    let path = source.to_string_lossy();
    let document =
        Document::open(path.as_ref()).map_err(|e| format!("Не удалось открыть PDF: {e}"))?;
    let count = document
        .page_count()
        .map_err(|e| format!("Не удалось получить количество страниц PDF: {e}"))?
        .max(0) as usize;
    let mut hits = Vec::new();
    for index in 0..count {
        let cached_text = cache_dir.join(format!("{index}.txt"));
        let text = if let Ok(text) = std::fs::read_to_string(&cached_text) {
            text
        } else {
            let page = document
                .load_page(index as i32)
                .map_err(|e| format!("Не удалось прочитать страницу PDF: {e}"))?;
            let text = page
                .text(TextExtractOptions::default())
                .map_err(|e| format!("Не удалось извлечь текст PDF: {e}"))?;
            let _ = std::fs::write(cached_text, &text);
            text
        };
        let Some(excerpt) = find_excerpt(&text, &needle) else {
            continue;
        };
        hits.push(SearchHit {
            page: index,
            excerpt,
        });
        if hits.len() >= 120 {
            break;
        }
    }
    Ok(hits)
}

fn text_cache_dir(path: &Path, metadata: &std::fs::Metadata) -> PathBuf {
    let identity = document_cache_key(path, metadata);
    let cache_id = hash(format!("{}:{}:{}", identity.0, identity.1, identity.2).as_bytes());
    default_root().join("search-cache-v1").join(cache_id)
}

/// Extract one page for continuous text reading, reusing the persistent search
/// index so switching between PDF and reading mode does not re-parse pages.
pub fn extract_page_text(path: impl AsRef<Path>, index: usize) -> Result<String, String> {
    let path = path.as_ref();
    let metadata = std::fs::metadata(path).map_err(|e| e.to_string())?;
    let cache_dir = text_cache_dir(path, &metadata);
    let _ = std::fs::create_dir_all(&cache_dir);
    let cached = cache_dir.join(format!("{index}.txt"));
    if let Ok(text) = std::fs::read_to_string(&cached) {
        return Ok(text);
    }
    let path = path.to_string_lossy();
    let document =
        Document::open(path.as_ref()).map_err(|e| format!("Не удалось открыть PDF: {e}"))?;
    if index >= document.page_count().unwrap_or(0).max(0) as usize || index > i32::MAX as usize {
        return Err("Страница не найдена".into());
    }
    let page = document
        .load_page(index as i32)
        .map_err(|e| format!("Не удалось прочитать страницу PDF: {e}"))?;
    let text = page
        .text(TextExtractOptions::default())
        .map_err(|e| format!("Не удалось извлечь текст PDF: {e}"))?;
    let _ = std::fs::write(cached, &text);
    Ok(text)
}

pub fn extract_reading_blocks(
    path: impl AsRef<Path>,
    index: usize,
) -> Result<Vec<ReadingBlock>, String> {
    let path = path.as_ref();
    let metadata = std::fs::metadata(path).map_err(|e| e.to_string())?;
    let cache_dir = text_cache_dir(path, &metadata);
    let _ = std::fs::create_dir_all(&cache_dir);
    let cached = cache_dir.join(format!("{index}.reading-v34.json"));
    if let Ok(bytes) = std::fs::read(&cached)
        && let Ok(blocks) = serde_json::from_slice(&bytes)
    {
        return Ok(blocks);
    }
    let path = path.to_string_lossy();
    let document =
        Document::open(path.as_ref()).map_err(|e| format!("Не удалось открыть PDF: {e}"))?;
    if index >= document.page_count().unwrap_or(0).max(0) as usize || index > i32::MAX as usize {
        return Err("Страница не найдена".into());
    }
    let page = document
        .load_page(index as i32)
        .map_err(|e| format!("Не удалось прочитать страницу PDF: {e}"))?;
    let page_bounds = page.bounds().map_err(|e| e.to_string())?;
    let text_page = page
        .to_text_page(TextPageFlags::PRESERVE_LIGATURES | TextPageFlags::PRESERVE_WHITESPACE)
        .map_err(|e| e.to_string())?;
    let mut blocks = Vec::new();
    for block in text_page.blocks() {
        let mut lines = Vec::new();
        let mut char_count = 0usize;
        let mut size_sum = 0.0f32;
        let mut indent_columns = Vec::new();
        let mut line_bounds = Vec::new();
        let bounds = block.bounds();
        for line in block.lines() {
            let mut value = String::new();
            for character in line.chars() {
                let Some(ch) = character.char() else { continue };
                value.push(ch);
                if !ch.is_whitespace() {
                    char_count += 1;
                    size_sum += character.size();
                }
            }
            let line_bounds_value = line.bounds();
            line_bounds.push((
                line_bounds_value.y0 - page_bounds.y0,
                line_bounds_value.y1 - page_bounds.y0,
            ));
            indent_columns.push(line_bounds_value.x0);
            lines.push(value);
        }
        if lines.is_empty() || char_count == 0 {
            continue;
        }
        let average_size = size_sum / char_count as f32;
        let column_width = (average_size * 0.6).max(1.0);
        for x in &mut indent_columns {
            *x = ((*x - bounds.x0).max(0.0) / column_width).round();
        }
        let repaired_lines: Vec<_> = lines
            .iter()
            .map(|line| repair_code_letter_runs(line))
            .collect();
        let nonempty: Vec<_> = repaired_lines
            .iter()
            .map(|line| line.trim())
            .filter(|line| !line.is_empty())
            .collect();
        let syntax_line = |line: &&str| {
            is_code_line(line) || line.ends_with('}') || line.contains(" = ") || line.contains("=>")
        };
        let syntax_count = nonempty.iter().filter(|line| syntax_line(line)).count();
        let code = nonempty.len() >= 2 && syntax_count * 2 >= nonempty.len();
        let mut positioned_code_rows = Vec::new();
        let text = if code {
            let mut fragments: Vec<_> = lines
                .iter()
                .enumerate()
                .filter_map(|(i, line)| {
                    (!line.trim().is_empty()).then_some((
                        line_bounds[i].0,
                        line_bounds[i].1,
                        indent_columns[i] as usize,
                        line.as_str(),
                    ))
                })
                .collect();
            fragments.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.2.cmp(&b.2)));
            let mut rows: Vec<(f32, f32, Vec<(usize, &str)>)> = Vec::new();
            for (y0, y1, column, line) in fragments {
                if let Some((_row_start, row_end, row)) = rows
                    .last_mut()
                    .filter(|(row_start, _, _)| (*row_start - y0).abs() < average_size * 0.3)
                {
                    row.push((column, line));
                    *row_end = row_end.max(y1);
                } else {
                    rows.push((y0, y1, vec![(column, line)]));
                }
            }
            let rows: Vec<_> = rows
                .into_iter()
                .map(|(y0, y1, mut fragments)| {
                    fragments.sort_by(|a, b| a.0.cmp(&b.0));
                    let mut row = String::new();
                    for (column, line) in fragments {
                        let gap = column.saturating_sub(row.chars().count());
                        row.push_str(&" ".repeat(gap.min(80)));
                        row.push_str(line.trim());
                    }
                    (y0, y1, row)
                })
                .collect::<Vec<_>>();
            positioned_code_rows = rows.clone();
            rows.into_iter()
                .map(|(_, _, line)| repair_code_letter_runs(&line))
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            let mut joined = String::new();
            for line in &lines {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if joined.is_empty() {
                    joined.push_str(line);
                    continue;
                }
                let hyphen = joined.ends_with(['-', '\u{ad}'])
                    && joined.chars().rev().nth(1).is_some_and(char::is_alphabetic)
                    && line.chars().next().is_some_and(char::is_lowercase);
                if hyphen {
                    joined.pop();
                } else {
                    joined.push(' ');
                }
                joined.push_str(line);
            }
            repair_ascii_letter_runs(&joined)
        };
        let in_margin = bounds.y1 < page_bounds.y0 + page_bounds.height() * 0.07
            || bounds.y0 > page_bounds.y0 + page_bounds.height() * 0.93;
        if in_margin && text.chars().count() < 80 {
            continue;
        }
        if !text.trim().is_empty() {
            if code {
                for (row_y0, row_y1, line) in positioned_code_rows {
                    blocks.push((
                        ReadingBlock {
                            text: line,
                            code: true,
                            syntax: Vec::new(),
                        },
                        row_y0,
                        row_y1,
                        average_size,
                        bounds.x0,
                    ));
                }
            } else {
                for (i, line) in lines.iter().enumerate() {
                    let line = repair_code_letter_runs(line.trim());
                    if line.is_empty() {
                        continue;
                    }
                    let (line_y0, line_y1) = line_bounds[i];
                    let line_x0 = bounds.x0 + indent_columns[i] * column_width;
                    if let Some(at) = code_start(&line) {
                        let prose = line[..at].trim();
                        if !prose.is_empty() {
                            blocks.push((
                                ReadingBlock {
                                    text: prose.to_owned(),
                                    code: false,
                                    syntax: Vec::new(),
                                },
                                line_y0,
                                line_y1,
                                average_size,
                                line_x0,
                            ));
                        }
                        blocks.push((
                            ReadingBlock {
                                text: line[at..].to_owned(),
                                code: true,
                                syntax: Vec::new(),
                            },
                            line_y0,
                            line_y1,
                            average_size,
                            line_x0,
                        ));
                    } else if is_code_line(&line) {
                        blocks.push((
                            ReadingBlock {
                                text: line,
                                code: true,
                                syntax: Vec::new(),
                            },
                            line_y0,
                            line_y1,
                            average_size,
                            line_x0,
                        ));
                    } else {
                        blocks.push((
                            ReadingBlock {
                                text: line,
                                code: false,
                                syntax: Vec::new(),
                            },
                            line_y0,
                            line_y1,
                            average_size,
                            line_x0,
                        ));
                    }
                }
            }
        }
    }
    blocks.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.4.total_cmp(&b.4)));
    let mut aligned_rows = Vec::with_capacity(blocks.len());
    let mut at = 0;
    while at < blocks.len() {
        let row_y0 = blocks[at].1;
        let row_size = blocks[at].3;
        let mut end = at + 1;
        while end < blocks.len()
            && (blocks[end].1 - row_y0).abs() < row_size.max(blocks[end].3) * 0.3
        {
            end += 1;
        }
        let has_code = blocks[at..end].iter().any(|(block, ..)| block.code);
        let mergeable = has_code
            && blocks[at..end]
                .iter()
                .all(|(block, ..)| block.code || block.text.lines().count() == 1);
        if mergeable {
            let mut row = blocks[at..end].iter().collect::<Vec<_>>();
            row.sort_by(|a, b| a.4.total_cmp(&b.4));
            let base_x0 = row
                .iter()
                .find(|(block, ..)| block.code)
                .map_or(row[0].4, |item| item.4);
            let size = row.iter().map(|item| item.3).fold(0.0f32, f32::max);
            let column_width = (size * 0.6).max(1.0);
            let mut text = String::new();
            let mut has_comment = false;
            for &(block, _, _, _, x0) in &row {
                if !text.is_empty() {
                    let target_column = ((*x0 - base_x0).max(0.0) / column_width).round() as usize;
                    let width = text.chars().count();
                    let min_gap = if block.text.trim_start().starts_with("//") {
                        2
                    } else {
                        1
                    };
                    let gap = target_column.saturating_sub(width).max(min_gap).min(80);
                    if block.code || block.text.trim_start().starts_with("//") {
                        text.push_str(&" ".repeat(gap));
                        text.push_str(&block.text);
                    } else {
                        if !has_comment {
                            let text_end = text.trim_end().len();
                            if text[..text_end].ends_with("//") {
                                text.truncate(text_end - 2);
                                text.push_str(" // ");
                            } else {
                                text.push_str(" // ");
                            }
                            has_comment = true;
                        } else {
                            text.push(' ');
                        }
                        text.push_str(block.text.trim());
                    }
                } else {
                    text.push_str(&block.text);
                }
            }
            let y1 = row.iter().map(|item| item.2).fold(row[0].2, f32::max);
            let text = text.replace("// //", "//").replace("////", "//");
            aligned_rows.push((
                ReadingBlock {
                    text,
                    code: true,
                    syntax: Vec::new(),
                },
                row_y0,
                y1,
                size,
                base_x0,
            ));
        } else {
            aligned_rows.extend(blocks[at..end].iter().cloned());
        }
        at = end;
    }
    let mut blocks: Vec<(ReadingBlock, f32, f32, f32, f32)> =
        Vec::with_capacity(aligned_rows.len());
    for (block, y0, y1, size, x0) in aligned_rows {
        if !block.code {
            if let Some((previous, previous_y0, previous_y1, previous_size, previous_x0)) =
                blocks.last_mut()
            {
                let vertical_gap = y0 - *previous_y1;
                let same_column = (x0 - *previous_x0).abs() <= size.max(*previous_size) * 0.6;
                if !previous.code
                    && same_column
                    && (0.0..=size.max(*previous_size) * 1.8).contains(&vertical_gap)
                {
                    previous.text = join_reading_prose(&[previous.text.clone(), block.text]);
                    *previous_y1 = y1.max(*previous_y1);
                    *previous_size = size.max(*previous_size);
                    *previous_y0 = previous_y0.min(y0);
                    continue;
                }
            }
        }
        blocks.push((block, y0, y1, size, x0));
    }
    let mut merged: Vec<(ReadingBlock, f32, f32, f32, f32)> = Vec::with_capacity(blocks.len());
    for (block, y0, y1, size, x0) in blocks {
        if block.code {
            if let Some((previous, previous_y0, previous_y1, previous_size, base_x0)) =
                merged.last_mut()
            {
                let same_row = (y0 - *previous_y0).abs() <= size.max(*previous_size) * 0.35;
                let vertical_gap = y0 - *previous_y1;
                if previous.code
                    && (same_row
                        || (-*previous_size * 0.5..=size.max(*previous_size) * 1.8)
                            .contains(&vertical_gap))
                {
                    if same_row {
                        let existing_width = previous
                            .text
                            .rsplit('\n')
                            .next()
                            .unwrap_or_default()
                            .chars()
                            .count();
                        let target_column =
                            ((x0 - *base_x0).max(0.0) / (size * 0.6).max(1.0)).round() as usize;
                        previous.text.push_str(
                            &" ".repeat(
                                target_column.saturating_sub(existing_width).max(1).min(80),
                            ),
                        );
                    } else {
                        previous.text.push('\n');
                        let indent =
                            ((x0 - *base_x0).max(0.0) / (size * 0.6).max(1.0)).round() as usize;
                        previous.text.push_str(&" ".repeat(indent.min(80)));
                        *previous_y0 = y0;
                    }
                    previous.text.push_str(&block.text);
                    *previous_y1 = previous_y1.max(y1);
                    *previous_size = previous_size.max(size);
                    continue;
                }
            }
        }
        merged.push((block, y0, y1, size, x0));
    }
    let blocks: Vec<_> = merged
        .into_iter()
        .map(|(mut block, _, _, _, _)| {
            if block.code {
                block.text = block
                    .text
                    .lines()
                    .map(repair_code_letter_runs)
                    .filter_map(|line| {
                        let trimmed = line.trim_end();
                        if trimmed == "//" {
                            None
                        } else if let Some(code) = trimmed.strip_suffix("//") {
                            (!code.trim().is_empty()).then(|| code.trim_end().to_owned())
                        } else {
                            Some(if let Some((code, comment)) = line.split_once("//") {
                                if code.trim().is_empty() {
                                    line
                                } else {
                                    format!("{} // {}", code.trim_end(), comment.trim_start())
                                }
                            } else {
                                line
                            })
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                block.text = block.text.replace("// //", "//").replace("////", "//");
                block.syntax = syntax::lex(&block.text);
            }
            block
        })
        .collect();
    if let Ok(bytes) = serde_json::to_vec(&blocks) {
        let _ = std::fs::write(cached, bytes);
    }
    Ok(blocks)
}

/// MuPDF can expose a PDF word as a run of individually spaced glyphs.
/// Rejoin ASCII identifier fragments in the same way as the established GPUI reader.
fn repair_ascii_letter_runs(text: &str) -> String {
    let tokens: Vec<_> = text.split_whitespace().collect();
    let mut out = Vec::with_capacity(tokens.len());
    let mut i = 0;
    while i < tokens.len() {
        let mut end = i;
        while let Some(token) = tokens.get(end) {
            if token.len() != 1 || !token.as_bytes()[0].is_ascii_alphabetic() {
                break;
            }
            end += 1;
        }
        if end - i >= 3 {
            out.push(tokens[i..end].concat());
            i = end;
        } else {
            out.push(tokens[i].to_owned());
            i += 1;
        }
    }
    out.join(" ")
}

fn code_start(line: &str) -> Option<usize> {
    let starts = [
        "class ",
        "constructor(",
        "function ",
        "return ",
        "const ",
        "let ",
        "var ",
        "this.",
    ];
    starts
        .iter()
        .filter_map(|needle| line.find(needle))
        .filter(|at| {
            let prefix = line[..*at].trim_end();
            prefix.is_empty() || prefix.ends_with(':')
        })
        .min()
}

fn is_code_line(line: &str) -> bool {
    let line = line.trim();
    [
        "class ",
        "constructor(",
        "function ",
        "return ",
        "const ",
        "let ",
        "var ",
        "this.",
        "distance",
        "//",
        "}",
        "{",
        "for(",
        "for (",
        "if(",
        "if (",
        "else ",
        "while(",
        "while (",
    ]
    .iter()
    .any(|prefix| line.starts_with(prefix))
        || line.ends_with([';', '{', '}'])
        || [
            "++", "--", "+=", "-=", "*=", "/=", "%=", "===", "!==", ">=", "<=", "&&", "||",
        ]
        .iter()
        .any(|operator| line.contains(operator))
}

fn join_reading_prose(lines: &[String]) -> String {
    let mut joined = String::new();
    for line in lines {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if joined.is_empty() {
            joined.push_str(line);
            continue;
        }
        let hyphen = joined.ends_with(['-', '\u{ad}'])
            && joined.chars().rev().nth(1).is_some_and(char::is_alphabetic)
            && line.chars().next().is_some_and(char::is_lowercase);
        if hyphen {
            joined.pop();
        } else {
            joined.push(' ');
        }
        joined.push_str(line);
    }
    repair_ascii_letter_runs(&joined)
}

fn repair_code_letter_runs(text: &str) -> String {
    const WORDS: &[&str] = &[
        "abstract",
        "arguments",
        "array",
        "async",
        "await",
        "boolean",
        "break",
        "case",
        "catch",
        "class",
        "const",
        "constructor",
        "continue",
        "debugger",
        "default",
        "delete",
        "distance",
        "do",
        "else",
        "enum",
        "eval",
        "export",
        "extends",
        "false",
        "factorial",
        "finally",
        "for",
        "function",
        "if",
        "implements",
        "import",
        "in",
        "instanceof",
        "interface",
        "let",
        "Math",
        "new",
        "null",
        "of",
        "package",
        " Point",
        "private",
        "product",
        "protected",
        "public",
        "return",
        "static",
        "super",
        "switch",
        "this",
        "throw",
        "true",
        "try",
        "typeof",
        "undefined",
        "abs",
        "var",
        "void",
        "while",
        "with",
        "yield",
        "sqrt",
        "sum",
        "Point",
    ];
    let tokens: Vec<_> = text
        .split_whitespace()
        .map(|token| {
            let start = token.as_ptr() as usize - text.as_ptr() as usize;
            (start, start + token.len())
        })
        .collect();
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    let mut i = 0;
    while i < tokens.len() {
        let (token_start, token_end) = tokens[i];
        out.push_str(&text[cursor..token_start]);
        let mut merged = String::new();
        let mut found = None;
        for end in i..tokens.len().min(i + 12) {
            let (start, finish) = tokens[end];
            if end > i && text[tokens[end - 1].1..start].chars().count() > 1 {
                break;
            }
            let token = &text[start..finish];
            let Some(start) = token.find(|c: char| c.is_ascii_alphabetic()) else {
                break;
            };
            let finish = token[start..]
                .find(|c: char| !c.is_ascii_alphabetic())
                .map_or(token.len(), |offset| start + offset);
            let prefix = &token[..start];
            let part = &token[start..finish];
            if !part.chars().all(|c| c.is_ascii_alphabetic()) || (end > i && !prefix.is_empty()) {
                break;
            }
            merged.push_str(part);
            if end > i && WORDS.iter().any(|word| word.eq_ignore_ascii_case(&merged)) {
                let leading = text[token_start..token_end]
                    .find(|c: char| c.is_ascii_alphabetic())
                    .unwrap();
                found = Some((
                    end + 1,
                    tokens[end].1,
                    format!(
                        "{}{}{}",
                        &text[token_start..token_start + leading],
                        merged,
                        &token[finish..]
                    ),
                ));
            }
        }
        if let Some((next, end, word)) = found {
            out.push_str(&word);
            cursor = end;
            i = next;
        } else {
            out.push_str(&text[token_start..token_end]);
            cursor = token_end;
            i += 1;
        }
    }
    out.push_str(&text[cursor..]);
    out.replace("/ /", "//")
        .replace("this,", "this.")
        .replace(" (", "(")
}

fn find_excerpt(text: &str, needle: &[char]) -> Option<String> {
    if needle.is_empty() {
        return None;
    }
    let chars: Vec<char> = text.chars().collect();
    let lower: Vec<char> = chars
        .iter()
        .map(|c| c.to_lowercase().next().unwrap_or(*c))
        .collect();
    let at = lower
        .windows(needle.len())
        .position(|window| window == needle)?;
    let from = at.saturating_sub(60);
    let to = (at + needle.len() + 100).min(chars.len());
    Some(chars[from..to].iter().collect())
}

#[cfg(test)]
mod tests {
    use super::{
        Book, PageTone, extract_reading_blocks, find_excerpt, render_cancellable, render_with_tone,
        search_books,
    };

    /// Regression for a mixed code and explanation page in the Russian JavaScript book.
    /// Run with `LIBER_READER_LAYOUT_PDF=/path/book.pdf cargo test -p liber-core reader_keeps_explanations_out_of_code -- --ignored`.
    #[test]
    #[ignore = "requires the JavaScript reader-layout PDF"]
    fn reader_aligns_explanations_with_their_code_rows() {
        let path = std::env::var("LIBER_READER_LAYOUT_PDF").expect("set LIBER_READER_LAYOUT_PDF");
        let blocks = extract_reading_blocks(path, 31).expect("extract page 32");
        assert!(blocks.iter().any(|block| {
            block.code
                && block
                    .text
                    .lines()
                    .any(|line| line.contains("let") && line.contains("Начать с произведения 1"))
        }));
        assert!(blocks.iter().any(|block| {
            block.code
                && block.text.lines().any(|line| {
                    line.contains("let") && line.contains("; // Начать с произведения 1")
                })
        }));
        assert!(blocks.iter().any(|block| {
            block.code
                && block
                    .text
                    .lines()
                    .any(|line| line.contains("while") && line.contains("Повторять операторы"))
        }));
        assert!(blocks.iter().any(|block| {
            block.code
                && block
                    .text
                    .lines()
                    .any(|line| line.trim_start().starts_with("// пока выражение"))
        }));
        assert!(
            blocks.iter().any(|block| block.code
                && block
                    .text
                    .lines()
                    .any(|line| line.contains("for(let x of array)")
                        && line.contains("в цикле по массиву")))
        );
        assert!(
            blocks
                .iter()
                .any(|block| block.code && block.text.contains("product *= n;"))
        );
        assert!(
            blocks
                .iter()
                .any(|block| block.code && block.text.contains("product *= i"))
        );
        assert!(
            blocks
                .iter()
                .any(|block| block.code && block.text.contains("factorial(5)"))
        );
        let code = blocks
            .iter()
            .filter(|block| block.code)
            .map(|block| block.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!code.lines().any(|line| line.trim() == "//"));
        assert!(
            !code
                .lines()
                .any(|line| line.trim_start().starts_with("// }"))
        );
        assert!(code.contains("abs("));
        assert!(!code.contains("a b s"));
    }

    #[test]
    fn progress_is_clamped_and_handles_single_page_documents() {
        let mut book = Book {
            id: "a".into(),
            title: "A".into(),
            author: String::new(),
            path: String::new(),
            pages: 5,
            last_page: 2,
            cover: String::new(),
        };
        assert_eq!(book.progress(), 0.5);
        book.last_page = usize::MAX;
        assert_eq!(book.progress(), 1.0);
        book.pages = 1;
        assert_eq!(book.progress(), 0.0);
    }

    #[test]
    fn library_search_matches_titles_and_authors_without_case() {
        let books = vec![
            Book {
                id: "a".into(),
                title: "Frontend Guide".into(),
                author: "Иван Иванов".into(),
                path: String::new(),
                pages: 2,
                last_page: 0,
                cover: String::new(),
            },
            Book {
                id: "b".into(),
                title: "Rust".into(),
                author: "Ada".into(),
                path: String::new(),
                pages: 1,
                last_page: 0,
                cover: String::new(),
            },
        ];
        assert_eq!(search_books(&books, "FRONT").len(), 1);
        assert_eq!(search_books(&books, "ИВАН").len(), 1);
        assert!(search_books(&books, "missing").is_empty());
    }

    #[test]
    fn pdf_search_excerpt_finds_unicode_case_insensitively() {
        let query: Vec<char> = "ПЕРЕМЕННЫЕ".chars().flat_map(char::to_lowercase).collect();
        let excerpt = find_excerpt("Текст книги: Переменные в JavaScript", &query).unwrap();
        assert!(excerpt.contains("Переменные"));
        assert!(find_excerpt("нет совпадения", &query).is_none());
        assert!(find_excerpt("любой текст", &[]).is_none());
    }

    /// Renders pages from several threads at once, the way the reader does
    /// when it scrolls. Run with `LIBER_TEST_PDF=/path/book.pdf cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn concurrent_page_requests_all_finish() {
        let path = std::env::var("LIBER_TEST_PDF").expect("set LIBER_TEST_PDF");
        let pages: Vec<usize> = (6..=15).collect();
        let (sender, receiver) = std::sync::mpsc::channel();
        for index in pages.clone() {
            let path = path.clone();
            let sender = sender.clone();
            std::thread::spawn(move || {
                let started = std::time::Instant::now();
                let result = render_with_tone(&path, index, 1.0, PageTone::Day).map(|_| ());
                let _ = sender.send((index, started.elapsed(), result));
            });
        }
        drop(sender);
        let mut finished = Vec::new();
        while let Ok((index, elapsed, result)) =
            receiver.recv_timeout(std::time::Duration::from_secs(120))
        {
            eprintln!("page {index}: {elapsed:?} {result:?}");
            finished.push(index);
        }
        assert_eq!(
            finished.len(),
            pages.len(),
            "pages that never finished: {:?}",
            pages
                .iter()
                .filter(|i| !finished.contains(i))
                .collect::<Vec<_>>()
        );
    }

    /// Simulates scrolling down a document: each step requests a window of pages
    /// around the viewport from separate threads, as the reader does. Every request
    /// must finish. Run with `LIBER_TEST_PDF=/path/book.pdf cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn scrolling_requests_all_finish() {
        let path = std::env::var("LIBER_TEST_PDF").expect("set LIBER_TEST_PDF");
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut requested = std::collections::BTreeSet::new();
        for first in 0..24usize {
            for index in first.saturating_sub(1)..=first + 2 {
                if !requested.insert(index) {
                    continue;
                }
                let path = path.clone();
                let sender = sender.clone();
                std::thread::spawn(move || {
                    let started = std::time::Instant::now();
                    let result = render_with_tone(&path, index, 1.0, PageTone::Day).map(|_| ());
                    let _ = sender.send((index, started.elapsed(), result));
                });
            }
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
        drop(sender);
        let mut finished = std::collections::BTreeSet::new();
        while let Ok((index, elapsed, result)) =
            receiver.recv_timeout(std::time::Duration::from_secs(60))
        {
            eprintln!("page {index}: {elapsed:?} {result:?}");
            finished.insert(index);
        }
        let stuck: Vec<usize> = requested.difference(&finished).copied().collect();
        assert!(stuck.is_empty(), "pages that never finished: {stuck:?}");
    }

    /// Replays the reader's scroll: a step every 300 ms, each requesting the window around
    /// the top page. Returns how long the final window took to render after the last step,
    /// or `None` when it did not finish within the deadline.
    fn replay_scroll(path: &str, cancel_offscreen: bool) -> Option<std::time::Duration> {
        use std::collections::{BTreeSet, HashMap};
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::{Arc, mpsc};
        use std::time::{Duration, Instant};

        type Live = HashMap<usize, (u64, Arc<AtomicBool>)>;
        fn record(done: &mut BTreeSet<usize>, live: &mut Live, index: usize, job: u64, ok: bool) {
            if ok {
                done.insert(index);
            }
            if live.get(&index).is_some_and(|(current, _)| *current == job) {
                live.remove(&index);
            }
        }

        let (sender, receiver) = mpsc::channel::<(usize, u64, bool)>();
        let mut live: Live = HashMap::new();
        let mut done = BTreeSet::new();
        let mut next_job = 0u64;
        let mut final_window = 0..=0;
        for first in 0..24usize {
            let window = first.saturating_sub(1)..=first + 2;
            if cancel_offscreen {
                live.retain(|index, (_, cancel)| {
                    let keep = window.contains(index);
                    if !keep {
                        cancel.store(true, Ordering::Relaxed);
                    }
                    keep
                });
            }
            for index in window.clone() {
                if done.contains(&index) || live.contains_key(&index) {
                    continue;
                }
                next_job += 1;
                let cancel = Arc::new(AtomicBool::new(false));
                live.insert(index, (next_job, cancel.clone()));
                let (path, sender, job) = (path.to_owned(), sender.clone(), next_job);
                std::thread::spawn(move || {
                    let ok = render_cancellable(&path, index, 1.0, PageTone::Day, &cancel).is_ok();
                    let _ = sender.send((index, job, ok));
                });
            }
            while let Ok((index, job, ok)) = receiver.try_recv() {
                record(&mut done, &mut live, index, job, ok);
            }
            final_window = window;
            std::thread::sleep(Duration::from_millis(300));
        }

        let started = Instant::now();
        let deadline = started + Duration::from_secs(120);
        while !final_window.clone().all(|index| done.contains(&index)) {
            if Instant::now() >= deadline {
                return None;
            }
            if let Ok((index, job, ok)) = receiver.recv_timeout(Duration::from_millis(200)) {
                record(&mut done, &mut live, index, job, ok);
            }
        }
        Some(started.elapsed())
    }

    #[test]
    #[ignore = "needs LIBER_TEST_PDF; renders a 24-step scroll through MuPDF"]
    fn scroll_renders_the_final_window_promptly() {
        let path = std::env::var("LIBER_TEST_PDF").expect("set LIBER_TEST_PDF");
        let waited = replay_scroll(&path, true);
        eprintln!("final window rendered after {waited:?}");
        assert!(
            waited.is_some_and(|waited| waited < std::time::Duration::from_secs(25)),
            "final window took {waited:?}"
        );
    }
}
