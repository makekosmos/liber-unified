//! Online book metadata: Open Library first, Google Books as a fallback
//! (better for Russian titles). Both work without an API key.

use std::collections::HashSet;
use std::time::Duration;

use serde_json::Value;

pub struct Found {
    pub title: String,
    pub author: String,
    /// PNG bytes of the edition cover, when one was available.
    pub cover: Option<Vec<u8>>,
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(12)))
        .user_agent("Liber/0.1 (local PDF library)")
        .build()
        .into()
}

fn get_json(agent: &ureq::Agent, url: &str) -> Option<Value> {
    let body = agent.get(url).call().ok()?.body_mut().read_to_string().ok()?;
    serde_json::from_str(&body).ok()
}

fn get_cover(agent: &ureq::Agent, url: &str) -> Option<Vec<u8>> {
    let bytes = agent.get(url).call().ok()?.body_mut().with_config().limit(8 << 20).read_to_vec().ok()?;
    let img = image::load_from_memory(&bytes).ok()?;
    // Placeholders are tiny; a real cover is at least this wide.
    if img.width() < 120 {
        return None;
    }
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).ok()?;
    Some(png)
}

fn words(s: &str) -> HashSet<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

/// A candidate title matches when almost all of its words occur in the query
/// (or the other way round): file names carry subtitles, catalogues shorten them.
fn same_title(query: &str, candidate: &str, loose: bool) -> bool {
    let (q, c) = (words(query), words(candidate));
    if q.is_empty() || c.is_empty() {
        return false;
    }
    let common = q.intersection(&c).count() as f32;
    common / q.len().min(c.len()) as f32 >= if loose { 0.6 } else { 0.8 } && common >= 2.0
}

fn same_author(query: &str, candidate: &str) -> bool {
    query.is_empty() || {
        let (q, c) = (words(query), words(candidate));
        q.intersection(&c).next().is_some()
    }
}

fn enc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn open_library(agent: &ureq::Agent, title: &str, author: &str) -> Option<Found> {
    let mut url = format!(
        "https://openlibrary.org/search.json?title={}&limit=8&fields=title,author_name,cover_i",
        enc(title)
    );
    if !author.is_empty() {
        url += &format!("&author={}", enc(author));
    }
    let json = get_json(agent, &url)?;
    for doc in json["docs"].as_array()? {
        let Some(found_title) = doc["title"].as_str() else { continue };
        let found_author = doc["author_name"][0].as_str().unwrap_or("");
        if !same_title(title, found_title, !author.is_empty()) || !same_author(author, found_author) {
            continue;
        }
        let cover = doc["cover_i"]
            .as_u64()
            .and_then(|id| get_cover(agent, &format!("https://covers.openlibrary.org/b/id/{id}-L.jpg?default=false")));
        return Some(Found { title: found_title.into(), author: found_author.into(), cover });
    }
    None
}

/// Anonymous Google Books quota is shared by every keyless client and is
/// often exhausted, so a personal (free) key is used when one is given:
/// `LIBER_GOOGLE_BOOKS_KEY` or a `google_books_key` file in the data dir.
fn google_key() -> Option<String> {
    std::env::var("LIBER_GOOGLE_BOOKS_KEY")
        .ok()
        .or_else(|| std::fs::read_to_string(crate::library::default_root().join("google_books_key")).ok())
        .map(|k| k.trim().to_owned())
        .filter(|k| !k.is_empty())
}

fn google_books(agent: &ureq::Agent, title: &str, author: &str) -> Option<Found> {
    // Plain text works better than `intitle:` operators, which 503 often.
    let head: Vec<&str> = title.split_whitespace().take(8).collect();
    let q = format!("{} {author}", head.join(" "));
    let url = format!("https://www.googleapis.com/books/v1/volumes?q={}&maxResults=8&printType=books", enc(&q));
    let url = match google_key() {
        Some(key) => format!("{url}&key={}", enc(&key)),
        None => url,
    };
    let json = get_json(agent, &url)?;
    for item in json["items"].as_array()? {
        let info = &item["volumeInfo"];
        let Some(base) = info["title"].as_str() else { continue };
        let mut found_title = base.to_owned();
        if let Some(sub) = info["subtitle"].as_str() {
            if !same_title(title, &found_title, false) {
                found_title = format!("{found_title}. {sub}");
            }
        }
        let found_author = info["authors"][0].as_str().unwrap_or("");
        if !same_title(title, &found_title, !author.is_empty()) || !same_author(author, found_author) {
            continue;
        }
        let cover = info["imageLinks"]["thumbnail"]
            .as_str()
            .map(|u| u.replacen("http://", "https://", 1).replace("zoom=1", "zoom=0"))
            .and_then(|u| get_cover(agent, &u));
        return Some(Found { title: found_title, author: found_author.into(), cover });
    }
    None
}

/// Blocking: call from a background thread.
pub fn lookup(title: &str, author: &str) -> Option<Found> {
    let agent = agent();
    open_library(&agent, title, author).or_else(|| google_books(&agent, title, author))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_match_despite_subtitles() {
        assert!(same_title(
            "JavaScript. Полное руководство справочник по самому популярному языку программирования",
            "JavaScript. Полное руководство",
            false
        ));
        assert!(!same_title("JavaScript. Полное руководство", "Python. Полное руководство", false));
        assert!(same_author("Дэвид Флэнаган", "Флэнаган Дэвид"));
        assert!(!same_author("Дэвид Флэнаган", "Иван Петров"));
    }

    #[test]
    #[ignore = "needs network"]
    fn real_lookup() {
        let f = lookup("JavaScript. Полное руководство справочник по самому популярному языку программирования", "Дэвид Флэнаган");
        let f = f.expect("found");
        println!("{} | {} | cover {}", f.title, f.author, f.cover.is_some());
    }
}
