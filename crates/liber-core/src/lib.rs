//! UI-independent PDF, library, and reading progress services used by Liber.
use mupdf::{
    Colorspace, Document, Matrix, MetadataName, TextBlockContent, TextExtractOptions, TextPageFlags,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Mutex, OnceLock,
        mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender},
    },
};

const INDEX: &str = "library.json";
const PAGE_CACHE_LIMIT: usize = 6;
const DOCUMENT_PAGE_LIMIT: usize = 16;
const DISPLAY_LIST_CACHE_LIMIT: usize = 2;
const MAX_RENDER_PIXELS: f64 = 12_000_000.0;
const PREVIEW_RENDER_PIXELS: f64 = 1_000_000.0;
const TILE_PIXELS: u32 = 510;
type PageCacheKey = (String, usize, u64, u128, u32, u8);
static PAGE_CACHE: OnceLock<Mutex<PageCache>> = OnceLock::new();
type DocumentCacheKey = (String, u64, u128);

struct RenderRequest {
    path: PathBuf,
    index: usize,
    zoom: f32,
    tone: PageTone,
    key: DocumentCacheKey,
    reply: SyncSender<Result<Page, String>>,
}

static PDF_RENDERER: OnceLock<Sender<RenderRequest>> = OnceLock::new();

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
    pub toc: Vec<(String, usize, usize)>,
    pub size: (u32, u32),
    /// Original PDF page dimensions in points, used to position selectable text.
    pub points: (f32, f32),
    pub zoom: f32,
    pub tiles: Vec<PageTile>,
    pub tone: PageTone,
}

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
            Self::Day => "День",
            Self::Sepia => "Сепия",
            Self::Gray => "Серая",
            Self::Night => "Ночь",
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
    {
        return Ok(cached);
    }

    let renderer = PDF_RENDERER.get_or_init(|| {
        let (sender, receiver) = mpsc::channel();
        std::thread::Builder::new()
            .name("liber-mupdf".into())
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
            reply,
        })
        .map_err(|e| format!("MuPDF worker остановлен: {e}"))?;
    let rendered = result
        .recv()
        .map_err(|e| format!("MuPDF worker не вернул страницу: {e}"))??;
    if let Ok(mut cache) = page_cache.lock() {
        cache.insert(cache_key, rendered.clone());
    }
    Ok(rendered)
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
) -> Result<Page, String> {
    let count = document
        .page_count()
        .map_err(|e| format!("Не удалось получить количество страниц PDF: {e}"))?
        .max(0) as usize;
    if index >= count || index > i32::MAX as usize {
        return Err("Страница не найдена".into());
    }
    if !display_lists.iter().any(|(page, _)| *page == index) {
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
    let detail_scale = (1.25f64 * f64::from(zoom)).min((MAX_RENDER_PIXELS / area).sqrt()) as f32;
    let zoom_key = (zoom * 100.0).round() as u32;
    let tone_key = tone as u8;
    let preview = std::env::temp_dir().join(format!(
        "liber-mupdf-v4-{name}-{index}-t{tone_key}-preview.png"
    ));
    if image::image_dimensions(&preview).is_err() {
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
    let mut tiles = Vec::new();
    if detail_scale > preview_scale + 0.01 {
        let pixel_width = (point_width * detail_scale).ceil().max(1.0) as u32;
        let pixel_height = (point_height * detail_scale).ceil().max(1.0) as u32;
        let mut matrix = Matrix::new_scale(detail_scale, detail_scale);
        matrix.pre_translate(-bounds.x0, -bounds.y0);
        for y in (0..pixel_height).step_by(TILE_PIXELS as usize) {
            for x in (0..pixel_width).step_by(TILE_PIXELS as usize) {
                let left = x.saturating_sub(1);
                let top = y.saturating_sub(1);
                let tile_width = (x + TILE_PIXELS + 1).min(pixel_width) - left;
                let tile_height = (y + TILE_PIXELS + 1).min(pixel_height) - top;
                let tile_path = std::env::temp_dir().join(format!(
                    "liber-mupdf-v4-{name}-{index}-t{tone_key}-z{zoom_key}-{left}-{top}.png"
                ));
                if image::image_dimensions(&tile_path).is_err() {
                    let mut pixmap = mupdf::Pixmap::new(
                        &Colorspace::device_rgb(),
                        left as i32,
                        top as i32,
                        tile_width as i32,
                        tile_height as i32,
                        false,
                    )
                    .map_err(|e| format!("Не удалось создать тайл PDF: {e}"))?;
                    pixmap.clear_with(255).map_err(|e| e.to_string())?;
                    {
                        let device = mupdf::Device::from_pixmap(&pixmap)
                            .map_err(|e| format!("Не удалось создать устройство рендера: {e}"))?;
                        let area = mupdf::Rect::new(
                            left as f32,
                            top as f32,
                            (left + tile_width) as f32,
                            (top + tile_height) as f32,
                        );
                        display_list
                            .run(&device, &matrix, area)
                            .map_err(|e| format!("Не удалось отрисовать тайл PDF: {e}"))?;
                    }
                    apply_tone(&mut pixmap, tone);
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
    let text_page = display_list
        .to_text_page(TextPageFlags::empty())
        .map_err(|e| format!("Не удалось извлечь текст страницы PDF: {e}"))?;
    let structured = text_page.structured();
    let mut text = String::new();
    let mut text_runs = Vec::new();
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
            }
        }
    }
    Ok(Page {
        count,
        png: preview.to_string_lossy().into_owned(),
        text,
        text_runs,
        toc: toc.to_vec(),
        size,
        points: (point_width, point_height),
        zoom,
        tiles,
        tone,
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
    for row in pixmap.samples_mut().chunks_mut(stride).take(height) {
        for pixel in row.chunks_mut(components).take(width) {
            let (r, g, b) = (pixel[0], pixel[1], pixel[2]);
            let (r, g, b) = match tone {
                PageTone::Day => (r, g, b),
                PageTone::Sepia => (
                    ((u16::from(r) * 0xF4) / 255) as u8,
                    ((u16::from(g) * 0xEC) / 255) as u8,
                    ((u16::from(b) * 0xD8) / 255) as u8,
                ),
                PageTone::Gray => (lift(255 - r), lift(255 - g), lift(255 - b)),
                PageTone::Night => (
                    ((u16::from(255 - r) * 0xDD) / 255) as u8,
                    ((u16::from(255 - g) * 0xDD) / 255) as u8,
                    ((u16::from(255 - b) * 0xD6) / 255) as u8,
                ),
            };
            pixel[0] = r;
            pixel[1] = g;
            pixel[2] = b;
        }
    }
}

fn lift(value: u8) -> u8 {
    (0x3A + (u16::from(value) * (0xE5 - 0x3A)) / 255) as u8
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
    use super::{Book, find_excerpt, search_books};
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
}
