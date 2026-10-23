//! UI-independent PDF, library, and reading progress services used by Liber.
use hayro::hayro_interpret::util::TransformExt;
use hayro::hayro_interpret::{Context, InterpreterCache, InterpreterSettings, interpret_page};
use hayro::hayro_syntax::Pdf;
use hayro::kurbo::Rect;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{PixmapSettings, RenderCache, RenderSettings};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

mod pdf_nav;
mod pdf_text;

const INDEX: &str = "library.json";

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

#[derive(Debug)]
pub struct Page {
    pub count: usize,
    pub png: String,
    pub text: String,
    pub toc: Vec<(String, usize, usize)>,
    pub size: (u32, u32),
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

/// Copy a PDF into the local library, create its cover, and persist its record.
/// Identical file bytes resolve to the existing book.
pub fn import_pdf(source: &Path) -> Result<Book, String> {
    let bytes =
        std::fs::read(source).map_err(|error| format!("Не удалось прочитать файл: {error}"))?;
    let id = hash(&bytes);
    let mut books = load_books();
    if let Some(book) = books.iter().find(|book| book.id == id) {
        return Ok(book.clone());
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
    std::fs::copy(page.png, &cover_path).map_err(|e| e.to_string())?;
    let document = Pdf::new(std::fs::read(&book_path).map_err(|e| e.to_string())?)
        .map_err(|e| format!("Не удалось прочитать PDF metadata: {e:?}"))?;
    let fallback = source
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Без названия")
        .to_owned();
    let embedded = document
        .metadata()
        .title
        .as_deref()
        .map(pdf_nav::decode_text)
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    let author = document
        .metadata()
        .author
        .as_deref()
        .map(pdf_nav::decode_text)
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_default();
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
    Ok(book)
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

pub fn render(path: impl AsRef<Path>, index: usize, dark: bool) -> Result<Page, String> {
    let path = path.as_ref();
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let pdf = Pdf::new(bytes).map_err(|e| format!("Не удалось открыть PDF: {e:?}"))?;
    let page = pdf.pages().get(index).ok_or("Страница не найдена")?;
    if page.raw().contains_key("Contents") && page.page_stream().is_none() {
        return Err("Не удалось разобрать содержимое страницы".into());
    }
    let toc = pdf_nav::outline(&pdf)
        .into_iter()
        .filter_map(|entry| entry.page.map(|at| (entry.title, at, entry.depth)))
        .collect();
    let (width, height) = page.render_dimensions();
    let cache = RenderCache::new();
    let pixmap = hayro::render(
        page,
        &cache,
        &InterpreterSettings::default(),
        &RenderSettings::default(),
        &PixmapSettings {
            x_scale: 1.5,
            y_scale: 1.5,
            bg_color: WHITE,
        },
    );
    let size = (u32::from(pixmap.width()), u32::from(pixmap.height()));
    let mut image = image::RgbaImage::from_raw(size.0, size.1, pixmap.data_as_u8_slice().to_vec())
        .ok_or("Ошибка буфера hayro")?;
    if dark {
        for pixel in image.pixels_mut() {
            pixel.0 = [255 - pixel[0], 255 - pixel[1], 255 - pixel[2], pixel[3]];
        }
    }
    let name = hash(path.to_string_lossy().as_bytes());
    let temp = std::env::temp_dir().join(format!("liber-nsh-{name}-{index}.png"));
    image.save(&temp).map_err(|e| e.to_string())?;
    let cache = InterpreterCache::new();
    let mut context = Context::new(
        page.initial_transform(true).to_kurbo(),
        Rect::new(0.0, 0.0, f64::from(width), f64::from(height)),
        &cache,
        page.xref(),
        InterpreterSettings::default(),
    );
    let mut device = pdf_text::TextDevice::default();
    interpret_page(page, &mut context, &mut device);
    Ok(Page {
        count: pdf.pages().len(),
        png: temp.to_string_lossy().into_owned(),
        text: pdf_text::to_lines(&device.glyphs),
        toc,
        size,
    })
}

#[cfg(test)]
mod tests {
    use super::{Book, search_books};
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
}
