//! Local book store: PDFs copied under the data dir plus a JSON index.
//! Nothing here talks to any other service; the data dir is the whole truth.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::pdf;

const INDEX: &str = "library.json";
const COVER_WIDTH_PX: f32 = 360.0;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Book {
    pub id: String,
    pub title: String,
    pub author: String,
    pub pages: u32,
    /// FNV-1a of the file bytes; used to refuse importing the same file twice.
    pub hash: String,
    pub added_at: u64,
    #[serde(default)]
    pub last_opened: Option<u64>,
    /// Zero-based page the reader resumes on.
    #[serde(default)]
    pub last_page: u32,
    #[serde(default)]
    pub favorite: bool,
    /// Online metadata lookup already ran (successfully or not).
    #[serde(default)]
    pub looked_up: bool,
}

impl Book {
    /// Reading progress in `0.0..=1.0`.
    pub fn progress(&self) -> f32 {
        if self.pages <= 1 {
            return 0.0;
        }
        (self.last_page.min(self.pages - 1) as f32) / (self.pages - 1) as f32
    }
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `LIBER_DATA_DIR`, else the platform data dir (`…/Liber`).
pub fn default_root() -> PathBuf {
    if let Some(dir) = std::env::var_os("LIBER_DATA_DIR") {
        return PathBuf::from(dir);
    }
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("Liber")
}

/// "Title (Author) (z-library.sk, …)" → ("Title", "Author"): files from book
/// sites carry the author in a trailing parenthesis and a source tag after it.
fn split_file_name(stem: &str) -> (String, String) {
    let mut rest = stem.trim().to_owned();
    let mut groups: Vec<String> = Vec::new();
    while rest.ends_with(')') {
        let Some(open) = rest.rfind('(') else { break };
        groups.push(rest[open + 1..rest.len() - 1].trim().to_owned());
        rest.truncate(open);
        rest = rest.trim_end().to_owned();
    }
    groups.retain(|g| !g.to_lowercase().contains("lib"));
    match (rest.is_empty(), groups.last()) {
        (false, Some(author)) => (rest, author.clone()),
        (false, None) => (rest, String::new()),
        (true, _) => (stem.trim().to_owned(), String::new()),
    }
}

fn fnv1a(bytes: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

pub enum Imported {
    Added(Book),
    /// Same bytes are already in the library.
    Duplicate(String),
}

/// Copy `src` into the store and build its record. Heavy (reads, hashes and
/// renders the file): run it off the UI thread; it does not touch the index.
pub fn prepare_import(root: &Path, src: &Path, known: &HashSet<String>) -> Result<Imported, String> {
    let bytes = fs::read(src).map_err(|e| format!("не удалось прочитать файл: {e}"))?;
    let hash = fnv1a(&bytes);
    if known.contains(&hash) {
        return Ok(Imported::Duplicate(hash));
    }
    let id = uuid::Uuid::new_v4().to_string();
    let dest = book_file(root, &id);
    let cover = cover_file(root, &id);
    for dir in [dest.parent(), cover.parent()].into_iter().flatten() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    fs::write(&dest, &bytes).map_err(|e| e.to_string())?;
    let built = pdf::probe(&dest).and_then(|meta| {
        // A missing cover is cosmetic; the card falls back to a placeholder.
        let _ = pdf::save_cover(&dest, &cover, COVER_WIDTH_PX);
        Ok(meta)
    });
    let meta = match built {
        Ok(meta) => meta,
        Err(e) => {
            let _ = fs::remove_file(&dest);
            return Err(e);
        }
    };
    let (fallback_title, file_author) = src
        .file_stem()
        .map(|s| split_file_name(&s.to_string_lossy()))
        .unwrap_or_else(|| ("Без названия".into(), String::new()));
    Ok(Imported::Added(Book {
        id,
        title: meta.title.unwrap_or(fallback_title),
        author: meta.author.unwrap_or(file_author),
        pages: meta.pages,
        hash,
        added_at: now(),
        last_opened: None,
        last_page: 0,
        favorite: false,
        looked_up: false,
    }))
}

pub fn book_file(root: &Path, id: &str) -> PathBuf {
    root.join("books").join(format!("{id}.pdf"))
}

/// Edition cover fetched online; preferred over the rendered first page.
pub fn online_cover_file(root: &Path, id: &str) -> PathBuf {
    root.join("covers").join(format!("{id}.online.png"))
}

pub fn cover_file(root: &Path, id: &str) -> PathBuf {
    root.join("covers").join(format!("{id}.png"))
}

pub struct Library {
    root: PathBuf,
    books: Vec<Book>,
    /// Set when the index on disk could not be parsed and was moved aside.
    pub recovered_from: Option<PathBuf>,
}

impl Library {
    pub fn open(root: PathBuf) -> Self {
        let index = root.join(INDEX);
        let mut recovered_from = None;
        let books = match fs::read(&index) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|_| {
                let aside = root.join(format!("{INDEX}.corrupt-{}", now()));
                if fs::rename(&index, &aside).is_ok() {
                    recovered_from = Some(aside);
                }
                Vec::new()
            }),
            Err(_) => Vec::new(),
        };
        Self { root, books, recovered_from }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn books(&self) -> &[Book] {
        &self.books
    }

    pub fn get(&self, id: &str) -> Option<&Book> {
        self.books.iter().find(|b| b.id == id)
    }

    pub fn hashes(&self) -> HashSet<String> {
        self.books.iter().map(|b| b.hash.clone()).collect()
    }

    pub fn book_path(&self, id: &str) -> PathBuf {
        book_file(&self.root, id)
    }

    pub fn cover_path(&self, id: &str) -> PathBuf {
        let online = online_cover_file(&self.root, id);
        if online.exists() {
            return online;
        }
        cover_file(&self.root, id)
    }

    /// Add a prepared book. Returns false (and discards its files) when the
    /// same content was added in the meantime.
    pub fn add(&mut self, book: Book) -> bool {
        if self.books.iter().any(|b| b.hash == book.hash) {
            let _ = fs::remove_file(self.book_path(&book.id));
            let _ = fs::remove_file(self.cover_path(&book.id));
            return false;
        }
        self.books.push(book);
        self.persist();
        true
    }

    pub fn remove(&mut self, id: &str) {
        self.books.retain(|b| b.id != id);
        let _ = fs::remove_file(self.book_path(id));
        let _ = fs::remove_file(cover_file(&self.root, id));
        let _ = fs::remove_file(online_cover_file(&self.root, id));
        let _ = fs::remove_file(crate::notes::notes_file(&self.root, id));
        self.persist();
    }

    pub fn mark_opened(&mut self, id: &str) {
        self.update(id, |b| b.last_opened = Some(now()));
    }

    pub fn set_page(&mut self, id: &str, page: u32) {
        self.update(id, |b| b.last_page = page.min(b.pages.saturating_sub(1)));
    }

    pub fn toggle_favorite(&mut self, id: &str) {
        self.update(id, |b| b.favorite = !b.favorite);
    }

    /// Store a lookup result; `None` just records that the lookup ran.
    pub fn set_meta(&mut self, id: &str, found: Option<(String, String)>) {
        self.update(id, |b| {
            b.looked_up = true;
            // The catalogue's title may name another edition; keep the file's own.
            if let Some((_, author)) = found {
                if !author.is_empty() {
                    b.author = author;
                }
            }
        });
    }

    fn update(&mut self, id: &str, f: impl FnOnce(&mut Book)) {
        if let Some(book) = self.books.iter_mut().find(|b| b.id == id) {
            f(book);
            self.persist();
        }
    }

    /// Atomic write: a crash mid-save leaves the previous index intact.
    fn persist(&self) {
        let Ok(json) = serde_json::to_vec_pretty(&self.books) else { return };
        if fs::create_dir_all(&self.root).is_err() {
            return;
        }
        let tmp = self.root.join(format!("{INDEX}.tmp"));
        if fs::write(&tmp, json).is_ok() {
            let _ = fs::rename(&tmp, self.root.join(INDEX));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdf::test_pdf;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("liber-lib-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn source(dir: &Path, file: &str, title: &str, pages: usize) -> PathBuf {
        let path = dir.join(file);
        fs::write(&path, test_pdf::minimal(title, pages)).unwrap();
        path
    }

    fn added(outcome: Result<Imported, String>) -> Book {
        match outcome.unwrap() {
            Imported::Added(book) => book,
            Imported::Duplicate(_) => panic!("unexpected duplicate"),
        }
    }

    #[test]
    fn import_copies_file_and_survives_reopen() {
        let dir = scratch("reopen");
        let root = dir.join("data");
        let src = source(&dir, "a.pdf", "Alpha", 5);
        let mut lib = Library::open(root.clone());
        let book = added(prepare_import(&root, &src, &lib.hashes()));
        assert_eq!((book.title.as_str(), book.pages), ("Alpha", 5));
        let id = book.id.clone();
        assert!(lib.add(book));
        assert!(lib.book_path(&id).exists());
        assert!(lib.cover_path(&id).exists());

        lib.set_page(&id, 3);
        lib.toggle_favorite(&id);
        let again = Library::open(root);
        let book = again.get(&id).unwrap();
        assert_eq!((book.last_page, book.favorite), (3, true));
    }

    #[test]
    fn same_bytes_are_not_imported_twice() {
        let dir = scratch("dup");
        let root = dir.join("data");
        let src = source(&dir, "a.pdf", "Alpha", 1);
        let mut lib = Library::open(root.clone());
        assert!(lib.add(added(prepare_import(&root, &src, &lib.hashes()))));
        assert!(matches!(
            prepare_import(&root, &src, &lib.hashes()).unwrap(),
            Imported::Duplicate(_)
        ));
        assert_eq!(lib.books().len(), 1);
    }

    #[test]
    fn title_falls_back_to_file_name_and_junk_is_rejected() {
        let dir = scratch("fallback");
        let root = dir.join("data");
        let blank = dir.join("Мой файл.pdf");
        let mut bytes = test_pdf::minimal("", 1);
        bytes.extend_from_slice(b"\n");
        fs::write(&blank, bytes).unwrap();
        assert_eq!(added(prepare_import(&root, &blank, &HashSet::new())).title, "Мой файл");

        let junk = dir.join("junk.pdf");
        fs::write(&junk, b"nope").unwrap();
        assert!(prepare_import(&root, &junk, &HashSet::new()).is_err());
        let leftovers = fs::read_dir(root.join("books")).unwrap().count();
        assert_eq!(leftovers, 1, "failed import must not leave its copy behind");
    }

    #[test]
    fn remove_deletes_files_and_record() {
        let dir = scratch("remove");
        let root = dir.join("data");
        let src = source(&dir, "a.pdf", "Alpha", 1);
        let mut lib = Library::open(root.clone());
        let book = added(prepare_import(&root, &src, &lib.hashes()));
        let id = book.id.clone();
        lib.add(book);
        lib.remove(&id);
        assert!(lib.get(&id).is_none());
        assert!(!lib.book_path(&id).exists());
        assert!(Library::open(root).books().is_empty());
    }

    #[test]
    fn corrupt_index_is_moved_aside_not_overwritten() {
        let dir = scratch("corrupt");
        let root = dir.join("data");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join(INDEX), b"{ broken").unwrap();
        let lib = Library::open(root.clone());
        assert!(lib.books().is_empty());
        let aside = lib.recovered_from.expect("index should be preserved");
        assert_eq!(fs::read(aside).unwrap(), b"{ broken");
    }

    #[test]
    fn progress_is_clamped() {
        let mut book = Book {
            id: "x".into(),
            title: String::new(),
            author: String::new(),
            pages: 11,
            hash: String::new(),
            added_at: 0,
            last_opened: None,
            last_page: 5,
            favorite: false,
            looked_up: false,
        };
        assert_eq!(book.progress(), 0.5);
        book.last_page = 99;
        assert_eq!(book.progress(), 1.0);
        book.pages = 1;
        assert_eq!(book.progress(), 0.0);
    }
}
