//! Reflowed ("reading mode") text: turn a page's text blocks into paragraphs.
//! Pure functions over plain data so the heuristics are testable without MuPDF.

use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Kind {
    Body,
    Code,
    /// 1 = chapter title, 2 = section title.
    Heading(u8),
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Para {
    pub text: String,
    pub kind: Kind,
    #[serde(default)]
    pub source: Vec<SourceLine>,
    #[serde(default)]
    pub syntax: Vec<crate::syntax::Span>,
}

/// Compact correspondence between reflow byte ranges and native PDF lines.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SourceLine {
    pub start: usize,
    pub end: usize,
    pub y0: f32,
    pub y1: f32,
}

impl Para {
    pub fn source_y(&self, byte: usize) -> Option<f32> {
        self.source.iter().find(|line| byte < line.end)
            .or_else(|| self.source.last()).map(|line| line.y0)
    }

    pub fn byte_at_y(&self, y: f32) -> usize {
        self.source.iter().min_by(|a, b| {
            let distance = |l: &SourceLine| (l.y0 - y).max(0.0).max(y - l.y1);
            distance(a).total_cmp(&distance(b))
        }).map_or(0, |line| line.start)
    }

    /// Repairs only whitespace and known OCR confusions; remap source offsets
    /// by character correspondence so layout changes cannot move the anchor.
    pub fn repair(&mut self, vocab: &Vocab) {
        if self.kind == Kind::Code { return; }
        let repaired = repair_technical_text(&vocab.repair(&self.text));
        if repaired == self.text { return; }
        let old: Vec<_> = self.text.char_indices().collect();
        let new: Vec<_> = repaired.char_indices().collect();
        let mut mapping = vec![repaired.len(); self.text.len() + 1];
        let mut j = 0;
        for (i, c) in old {
            if c.is_whitespace() {
                mapping[i] = new.get(j).map_or(repaired.len(), |(i, _)| *i);
                continue;
            }
            // Space repair preserves non-space characters. For an OCR
            // substitution, retain the nearest offset rather than guessing far ahead.
            while new.get(j).is_some_and(|(_, c)| c.is_whitespace()) { j += 1; }
            if new.get(j).is_some_and(|(_, ch)| *ch != c) {
                if let Some(ahead) = new.iter().skip(j).take(8).position(|(_, ch)| *ch == c) {
                    j += ahead;
                }
            }
            mapping[i] = new.get(j).map_or(repaired.len(), |(i, _)| *i);
            j += usize::from(j < new.len());
        }
        for line in &mut self.source {
            line.start = mapping[line.start.min(mapping.len() - 1)];
            line.end = mapping[line.end.min(mapping.len() - 1)];
        }
        self.text = repaired;
    }
}

/// One text block as MuPDF reports it: its lines, average glyph size and
/// vertical extent on the page.
pub struct RawBlock {
    pub lines: Vec<String>,
    pub size: f32,
    pub y0: f32,
    pub y1: f32,
    pub line_bounds: Vec<(f32, f32)>,
    pub mono: bool,
    pub indents: Vec<usize>,
}

/// Conservative structural detection: monospace or several syntax-bearing
/// lines. A number alone is code only in a multi-line numeric example.
fn is_code(b: &RawBlock) -> bool {
    let lines: Vec<_> = b.lines.iter().map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
    if lines.is_empty() { return false; }
    let syntax = |s: &&str| ["//", "/*", "const ", "let ", "var ", "function ", "class ", "import "].iter().any(|prefix| s.starts_with(prefix))
        || s.ends_with([';', '{', '}']) || s.contains("=>") || s.contains(" = ");
    (b.mono && (lines.iter().any(syntax) || lines.len() > 1))
        || numeric_example(&b.lines)
        || (lines.len() >= 2 && lines.iter().filter(|s| syntax(s)).count() * 2 >= lines.len())
}

fn numeric_example(lines: &[String]) -> bool {
    let lines: Vec<_> = lines.iter().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
    lines.len() >= 2 && lines.iter().any(|l| l.chars().any(|c| c.is_ascii_digit()))
        && lines.iter().all(|line| line.chars().all(|c| c.is_ascii_digit() || matches!(c, 'O' | 'О' | 'о')))
}

fn code_rows(b: &RawBlock) -> (Vec<String>, Vec<(f32, f32)>) {
    let mut fragments: Vec<_> = b.lines.iter().enumerate().map(|(i, text)| {
        let bounds = b.line_bounds.get(i).copied()
            .unwrap_or((b.y0 + i as f32 * b.size, b.y0 + (i + 1) as f32 * b.size));
        (bounds, b.indents.get(i).copied().unwrap_or(0).min(80), text)
    }).collect();
    fragments.sort_by(|a, b| a.0.0.total_cmp(&b.0.0).then(a.1.cmp(&b.1)));
    let mut lines: Vec<String> = Vec::new();
    let mut bounds: Vec<(f32, f32)> = Vec::new();
    let numeric = numeric_example(&b.lines);
    for ((y0, y1), column, text) in fragments {
        let numeric_text;
        let text = if numeric {
            numeric_text = text.trim().replace(['O', 'О', 'о'], "0");
            &numeric_text
        } else { text };
        if let Some(last) = bounds.last_mut().filter(|line| (line.0 - y0).abs() < b.size * 0.3) {
            let row = lines.last_mut().unwrap();
            let gap = column.saturating_sub(row.chars().count()).max(1);
            row.push_str(&" ".repeat(gap.min(80)));
            row.push_str(text);
            last.1 = last.1.max(y1);
        } else {
            lines.push(format!("{}{}", " ".repeat(column), text));
            bounds.push((y0, y1));
        }
    }
    for row in &mut lines { *row = repair_numeric_example(row); }
    (lines, bounds)
}

/// Correct mixed-script Latin identifiers, while leaving Russian prose alone.
/// Pure Cyrillic words are never transliterated. The IEEE correction requires
/// the adjacent standard number, so a lookalike elsewhere is untouched.
pub fn repair_technical_text(text: &str) -> String {
    let corrected = text.replace("ШЕЕ 754", "IEEE 754").replace("ІЕЕЕ 754", "IEEE 754");
    corrected.split_inclusive(|c: char| !c.is_alphanumeric() && c != '_').map(|token| {
        let latin = token.chars().filter(|c| c.is_ascii_alphabetic()).count();
        let cyrillic = token.chars().filter(|c| ('\u{0400}'..='\u{04ff}').contains(c)).count();
        if latin < 2 || cyrillic > 2 || cyrillic >= latin { return token.to_owned(); }
        token.chars().map(|c| match c {
            'А' => 'A', 'В' => 'B', 'Е' => 'E', 'К' => 'K', 'М' => 'M',
            'Н' => 'H', 'О' => 'O', 'Р' => 'P', 'С' => 'C', 'Т' => 'T',
            'Х' => 'X', 'а' => 'a', 'е' => 'e', 'о' => 'o', 'р' => 'p',
            'с' => 'c', 'х' => 'x', 'у' => 'y', 'І' => 'I', 'і' => 'i',
            _ => c,
        }).collect::<String>()
    }).collect()
}

fn join_lines(lines: &[String]) -> String {
    let mut out = String::new();
    for line in lines {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if out.is_empty() {
            out.push_str(line);
            continue;
        }
        let soft_hyphen = out.ends_with(['-', '\u{ad}'])
            && out.chars().rev().nth(1).is_some_and(char::is_alphabetic)
            && line.chars().next().is_some_and(char::is_lowercase);
        if soft_hyphen {
            out.pop();
        } else {
            out.push(' ');
        }
        out.push_str(line);
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Most common glyph size (rounded to 0.5pt) weighted by text length.
fn body_size(blocks: &[RawBlock]) -> f32 {
    let mut counts: Vec<(i32, usize)> = Vec::new();
    for b in blocks {
        let key = (b.size * 2.0).round() as i32;
        let n: usize = b.lines.iter().map(|l| l.chars().count()).sum();
        match counts.iter_mut().find(|(k, _)| *k == key) {
            Some(e) => e.1 += n,
            None => counts.push((key, n)),
        }
    }
    counts.into_iter().max_by_key(|(_, n)| *n).map_or(10.0, |(k, _)| k as f32 / 2.0)
}

/// Build paragraphs for one page of height `page_h`. Running headers,
/// footers and bare page numbers are dropped.
pub fn paragraphs(blocks: &[RawBlock], page_h: f32) -> Vec<Para> {
    let body = body_size(blocks);
    let mut out = Vec::new();
    for b in blocks {
        let code = is_code(b);
        let (code_lines, code_bounds) = if code { code_rows(b) } else { (Vec::new(), Vec::new()) };
        let lines = if code { &code_lines } else { &b.lines };
        let text = if code { lines.join("\n") } else { join_lines(lines) };
        if text.is_empty() {
            continue;
        }
        let in_margin = b.y1 < page_h * 0.07 || b.y0 > page_h * 0.93;
        let bare_number = text.chars().count() <= 4 && text.chars().all(|c| c.is_ascii_digit());
        if (bare_number && in_margin) || (in_margin && text.chars().count() < 80) {
            continue;
        }
        let ratio = b.size / body;
        let numbered = numbered_heading(&text);
        let kind = if code { Kind::Code } else if text.chars().count() < 160 && (ratio > 1.15 || numbered.is_some()) {
            Kind::Heading(numbered.unwrap_or(if ratio > 1.45 { 1 } else if ratio > 1.25 { 2 } else { 3 }))
        } else {
            Kind::Body
        };
        let mut source: Vec<SourceLine> = Vec::new();
        let mut prefix = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            let start = if prefix.is_empty() { 0 } else {
                let s = if code { prefix.join("\n") } else { join_lines(&prefix) };
                if !code && s.ends_with(['-', '\u{ad}']) && line.chars().next().is_some_and(char::is_lowercase) {
                    let removed = s.chars().last().unwrap().len_utf8();
                    if let Some(prev) = source.last_mut() { prev.end = prev.end.saturating_sub(removed); }
                    s.len() - removed
                } else { s.len() + 1 }
            };
            prefix.push(line.clone());
            let end = if code { prefix.join("\n").len() } else { join_lines(&prefix).len() };
            let bounds = if code { &code_bounds } else { &b.line_bounds };
            let (y0, y1) = bounds.get(i).copied().unwrap_or((b.y0, b.y1));
            source.push(SourceLine { start: start.min(end), end, y0, y1 });
        }
        for line in &mut source {
            line.start = line.start.min(text.len());
            line.end = line.end.min(text.len());
            while !text.is_char_boundary(line.start) { line.start -= 1; }
            while !text.is_char_boundary(line.end) { line.end -= 1; }
        }
        let syntax = if code { crate::syntax::lex(&text) } else { Vec::new() };
        out.push(Para { text, kind, source, syntax });
    }
    out
}

/// Split a token into (leading punctuation, word, trailing punctuation);
/// `None` unless it has exactly one word core.
fn split_token(tok: &str) -> Option<(&str, &str, &str)> {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let start = tok.find(is_word)?;
    let end = tok.rfind(is_word).map(|i| i + tok[i..].chars().next().map_or(0, char::len_utf8))?;
    let core = &tok[start..end];
    core.chars().all(is_word).then(|| (&tok[..start], core, &tok[end..]))
}

/// Cyrillic one-letter words: a pair of real words is not a broken word.
fn is_word_fragment(w: &str, count: u32, merged: u32) -> bool {
    let mut cs = w.chars();
    let single = cs.next().is_some() && cs.next().is_none();
    let real_single = single && "вксоуияаВКСОУИЯА".contains(w);
    (!single || real_single) && count * 4 >= merged
}

/// Rejoin Latin identifiers set one letter per token. Cyrillic one-letter
/// words are valid prose and must never be consumed by these runs.
fn glue_letter_runs(text: &str) -> String {
    fn single(t: &str) -> Option<(&str, &str, &str)> {
        split_token(t).filter(|(_, core, _)| core.chars().count() == 1)
    }
    let toks: Vec<&str> = text.split_whitespace().collect();
    let mut out: Vec<String> = Vec::with_capacity(toks.len());
    let mut i = 0;
    while i < toks.len() {
        let mut end = i;
        while let Some((lead, core, trail)) = toks.get(end).and_then(|t| single(t)) {
            // Russian one-letter words surrounding an English identifier are
            // separate words, even when the entire run has many letters.
            if !core.chars().all(|c| c.is_ascii_alphabetic()) { break; }
            if end > i && !lead.is_empty() {
                break;
            }
            end += 1;
            if !trail.is_empty() {
                break;
            }
        }
        let run = &toks[i..end];
        if run.len() >= 3 {
            out.push(run.concat());
            i = end;
        } else {
            out.push(toks[i].to_string());
            i += 1;
        }
    }
    out.join(" ")
}

/// Word frequencies of one book. Some PDFs (OCR output, odd embedded fonts)
/// carry stray spaces inside words ("O bject", "n u ll"); the book itself
/// shows the right spelling elsewhere, so a run of fragments whose join is a
/// frequent word gets glued back together.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct Vocab(HashMap<String, u32>);

impl Vocab {
    #[cfg(test)]
    pub fn build<'a>(texts: impl IntoIterator<Item = &'a str>) -> Self {
        let mut counts = HashMap::new();
        for text in texts {
            for tok in text.split_whitespace() {
                if let Some((_, core, _)) = split_token(tok) {
                    *counts.entry(core.to_lowercase()).or_insert(0) += 1;
                }
            }
        }
        Vocab(counts)
    }

    pub fn extend(&mut self, text: &str) {
        for tok in text.split_whitespace() {
            if let Some((_, core, _)) = split_token(tok) {
                *self.0.entry(core.to_lowercase()).or_insert(0) += 1;
            }
        }
    }

    fn count(&self, word: &str) -> u32 {
        self.0.get(&word.to_lowercase()).copied().unwrap_or(0)
    }

    pub fn repair(&self, text: &str) -> String {
        let text = glue_letter_runs(text);
        let toks: Vec<&str> = text.split_whitespace().collect();
        let mut out: Vec<String> = Vec::with_capacity(toks.len());
        let mut i = 0;
        'next: while i < toks.len() {
            for n in (2..=6).rev() {
                let Some(run) = toks.get(i..i + n) else { continue };
                let Some(parts) = run.iter().map(|t| split_token(t)).collect::<Option<Vec<_>>>() else { continue };
                let inner_clean = parts[1..n - 1].iter().all(|p| p.0.is_empty() && p.2.is_empty());
                if !inner_clean || !parts[0].2.is_empty() || parts[1..].iter().any(|p| !p.0.is_empty()) {
                    continue;
                }
                let merged: String = parts.iter().map(|p| p.1).collect();
                let keyword = matches!(merged.as_str(), "var" | "let" | "null" | "const" | "typeof" | "instanceof" | "undefined")
                    && parts.iter().any(|p| p.1.len() == 1);
                let m = self.count(&merged);
                let all_words = parts.iter().all(|p| is_word_fragment(p.1, self.count(p.1), m));
                let long_enough = merged.chars().count() >= 4 || m >= 100;
                if keyword || (m >= 4 && long_enough && !all_words) {
                    out.push(format!("{}{}{}", parts[0].0, merged, parts[n - 1].2));
                    i += n;
                    continue 'next;
                }
            }
            out.push(toks[i].to_string());
            i += 1;
        }
        out.join(" ")
    }
}

/// Whether `next` continues the unfinished sentence that ends `prev`
/// (a paragraph split by a page break).
pub fn continues(prev: &Para, next: &Para) -> bool {
    prev.kind == Kind::Body
        && next.kind == Kind::Body
        && !prev.text.ends_with(['.', '!', '?', ':', ';', '…', '»', '"', ')'])
        && next.text.chars().next().is_some_and(char::is_lowercase)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_lines_survive_joining_and_whitespace_repair() {
        let mut b = block(&["Это длин-", "ный текст с l e t", "и JavaSсript."], 10.0, 100.0, 140.0);
        b.line_bounds = vec![(100.0, 110.0), (115.0, 125.0), (130.0, 140.0)];
        let mut p = paragraphs(&[b], 800.0).remove(0);
        p.repair(&Vocab::default());
        assert_eq!(p.text, "Это длинный текст с let и JavaScript.");
        for line in &p.source {
            assert!(p.text.is_char_boundary(line.start));
            assert!(p.text.is_char_boundary(line.end));
        }
        assert_eq!(p.source_y(p.byte_at_y(132.0)), Some(130.0));
        assert!(p.text[p.byte_at_y(132.0)..].starts_with("и JavaScript"));
    }

    #[test]
    fn mixed_language_repairs_preserve_prose_and_use_standard_context() {
        assert_eq!(repair_technical_text("JavaSсript и стандарт ШЕЕ 754; JavaScript-функция; ШЕЕ отдельно"),
            "JavaScript и стандарт IEEE 754; JavaScript-функция; ШЕЕ отдельно");
    }

    #[test]
    fn code_keeps_lines_indentation_and_operators() {
        let mut b = block(&["const total = 1;", "return total + 2;"], 10.0, 100.0, 140.0);
        b.mono = true;
        b.indents = vec![0, 4];
        let mut p = paragraphs(&[b], 800.0).remove(0);
        assert_eq!(p.kind, Kind::Code);
        assert_eq!(p.text, "const total = 1;\n    return total + 2;");
        p.repair(&Vocab::default());
        assert_eq!(p.text, "const total = 1;\n    return total + 2;");
        let numeric = paragraphs(&[block(&["0", "3", "1000000"], 10.0, 100.0, 150.0)], 800.0);
        assert_eq!(numeric[0].kind, Kind::Code);
        assert_eq!(numeric[0].text, "0\n3\n1000000");
        let ocr_numeric = paragraphs(&[block(&["О", "3", "10000000 "], 10.0, 100.0, 150.0)], 800.0);
        assert_eq!(ocr_numeric[0].kind, Kind::Code);
        assert_eq!(ocr_numeric[0].text, "0\n3\n10000000");
        assert_eq!(ocr_numeric[0].syntax.iter().filter(|s| s.token == crate::syntax::Token::Number).count(), 3);
        let mut aligned = block(&["0xff", "// => 255", "0xBADCAFE", "// => 195939070"], 10.0, 100.0, 140.0);
        aligned.line_bounds = vec![(100.0, 110.0), (100.0, 110.0), (120.0, 130.0), (120.0, 130.0)];
        aligned.indents = vec![0, 12, 0, 12];
        let code = paragraphs(&[aligned], 800.0).remove(0);
        assert_eq!(code.text.lines().count(), 2);
        assert!(code.text.lines().next().unwrap().contains("0xff        // => 255"));
    }

    fn block(lines: &[&str], size: f32, y0: f32, y1: f32) -> RawBlock {
        RawBlock { lines: lines.iter().map(|s| s.to_string()).collect(), size, y0, y1,
            line_bounds: Vec::new(), mono: false, indents: Vec::new() }
    }

    #[test]
    fn lines_join_and_soft_hyphens_vanish() {
        let p = paragraphs(&[block(&["Это длин-", "ный текст", "в три строки."], 10.0, 100.0, 140.0)], 800.0);
        assert_eq!(p[0].text, "Это длинный текст в три строки.");
    }

    #[test]
    fn real_hyphens_before_capitals_stay() {
        let p = paragraphs(&[block(&["как JavaScript-", "Разработчик"], 10.0, 100.0, 120.0)], 800.0);
        assert_eq!(p[0].text, "как JavaScript- Разработчик");
    }

    #[test]
    fn headings_by_size_and_margins_dropped() {
        let blocks = [
            block(&["Глава 13"], 20.0, 100.0, 130.0),
            block(&["Асинхронный JavaScript"], 14.0, 140.0, 160.0),
            block(&["Основной текст абзаца, достаточно длинный, чтобы быть телом."], 10.0, 200.0, 220.0),
            block(&["Основной текст второго абзаца, тоже достаточно длинный текст."], 10.0, 230.0, 250.0),
            block(&["Книга"], 8.0, 10.0, 30.0),
            block(&["375"], 10.0, 760.0, 775.0),
        ];
        let p = paragraphs(&blocks, 800.0);
        assert_eq!(p.len(), 4);
        assert_eq!(p[0].kind, Kind::Heading(1));
        assert_eq!(p[1].kind, Kind::Heading(2));
        assert_eq!(p[2].kind, Kind::Body);
    }

    #[test]
    fn stray_spaces_inside_words_are_glued_by_book_vocabulary() {
        let book = "Object Object Object Object null null null null и n u ll и O bject. так же так же также также также также";
        let vocab = Vocab::build([book]);
        assert_eq!(vocab.repair("значение n u ll и O bject."), "значение null и Object.");
        assert_eq!(vocab.repair("всё так же хорошо"), "всё так же хорошо");
    }

    #[test]
    fn page_break_continuation() {
        let a = Para { text: "и тогда программа".into(), kind: Kind::Body, source: Vec::new(), syntax: Vec::new() };
        let b = Para { text: "продолжает работу.".into(), kind: Kind::Body, source: Vec::new(), syntax: Vec::new() };
        let c = Para { text: "Новый абзац.".into(), kind: Kind::Body, source: Vec::new(), syntax: Vec::new() };
        assert!(continues(&a, &b));
        assert!(!continues(&a, &c));
        assert!(!continues(&b, &b));
    }
}

/// Repair only stand-alone numeric examples whose adjacent result proves the value.
fn repair_numeric_example(row: &str) -> String {
    let Some((code, comment)) = row.split_once("//") else { return row.into() };
    let literal = code.trim();
    let Some(result) = comment.trim_start().strip_prefix("=>") else { return row.into() };
    let digits: String = result.trim_start().chars().take_while(char::is_ascii_digit).collect();
    let Ok(expected) = digits.parse::<u64>() else { return row.into() };
    let candidate = if let Some(tail) = literal.strip_prefix("0Ы") { format!("0b1{tail}") }
        else if let Some(tail) = literal.strip_prefix("0о").or_else(|| literal.strip_prefix("0О")) { format!("0o{tail}") }
        else if let Some(tail) = literal.strip_prefix("Ox").or_else(|| literal.strip_prefix("Оx")) { format!("0x{tail}") }
        else { return row.into() };
    let radix = match candidate.as_bytes()[1] { b'b' => 2, b'o' => 8, _ => 16 };
    if u64::from_str_radix(&candidate[2..], radix) != Ok(expected) { return row.into(); }
    let at = code.find(literal).unwrap();
    format!("{}{}{}", &row[..at], candidate, &row[at + literal.len()..])
}

#[test]
fn numeric_ocr_requires_matching_result() {
    assert_eq!(repair_numeric_example("0Ы0101 // => 21: sum"), "0b10101 // => 21: sum");
    assert_eq!(repair_numeric_example("0о377 // => 255"), "0o377 // => 255");
    assert_eq!(repair_numeric_example("0Ы0101 // => 22"), "0Ы0101 // => 22");
    assert_eq!(repair_numeric_example("0Ы0101"), "0Ы0101");
}

fn numbered_heading(text: &str) -> Option<u8> {
    let mut words = text.split_whitespace();
    let token = words.next()?;
    if !token.ends_with('.') || !words.next()?.chars().next()?.is_uppercase() { return None; }
    let prefix = token.trim_end_matches('.');
    let parts: Vec<_> = prefix.split('.').collect();
    (parts.len() <= 3 && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())))
        .then_some(parts.len() as u8)
}

#[test]
fn numbered_headings_keep_hierarchy() {
    assert_eq!(numbered_heading("3. Типы"), Some(1));
    assert_eq!(numbered_heading("3.2. Числа"), Some(2));
    assert_eq!(numbered_heading("3.2.1. Целочисленные литералы"), Some(3));
    assert_eq!(numbered_heading("IEEE 754"), None);
}
