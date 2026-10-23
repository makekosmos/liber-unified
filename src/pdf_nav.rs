//! Navigation data that hayro 0.8 does not expose: outline (TOC), page labels,
//! link annotations and named destinations. Written by hand on top of the
//! object API of `hayro_syntax`.

use std::collections::HashMap;

use hayro::hayro_syntax::Pdf;
use hayro::hayro_syntax::object::{Array, Dict, MaybeRef, Name, ObjRef, Object, ObjectIdentifier};

/// Upper bound on outline nodes, so a cyclic or hostile outline cannot hang the walk.
const MAX_OUTLINE: usize = 20_000;

#[derive(Debug, Clone)]
pub struct TocEntry {
    pub title: String,
    pub page: Option<usize>,
    pub depth: usize,
}

#[derive(Debug, Clone)]
pub enum LinkTarget {
    Page(usize),
    Uri(String),
    Unresolved,
}

#[derive(Debug, Clone)]
pub struct Link {
    pub rect: [f64; 4],
    pub target: LinkTarget,
}

/// Maps the object identity of every page dictionary to its index.
fn page_index(pdf: &Pdf) -> HashMap<ObjectIdentifier, usize> {
    pdf.pages()
        .iter()
        .enumerate()
        .filter_map(|(i, page)| page.raw().obj_id().map(|id| (id, i)))
        .collect()
}

fn ident(r: ObjRef) -> ObjectIdentifier {
    ObjectIdentifier::new(r.obj_number, r.gen_number)
}

/// Decode a PDF text string: UTF-16BE with BOM, otherwise a Latin-1 approximation
/// of PDFDocEncoding (exact for ASCII, which is enough for this report).
pub fn decode_text(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let units: Vec<u16> = rest.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
        return String::from_utf16_lossy(&units);
    }
    if let Ok(text) = std::str::from_utf8(bytes) {
        return text.to_owned();
    }
    bytes.iter().map(|&b| b as char).collect()
}

fn root(pdf: &Pdf) -> Option<Dict<'_>> {
    pdf.xref().get::<Dict<'_>>(pdf.xref().root_id())
}

/// Resolve a destination (explicit array, or a name looked up in the name tree)
/// to a page index.
fn dest_page(pdf: &Pdf, index: &HashMap<ObjectIdentifier, usize>, dest: Object<'_>) -> Option<usize> {
    match dest {
        Object::Array(arr) => explicit_page(index, &arr),
        Object::Dict(d) => d.get::<Array<'_>>("D").and_then(|a| explicit_page(index, &a)),
        Object::Name(n) => named_dest(pdf, index, n.as_ref()),
        Object::String(s) => named_dest(pdf, index, s.as_bytes()),
        _ => None,
    }
}

fn explicit_page(index: &HashMap<ObjectIdentifier, usize>, arr: &Array<'_>) -> Option<usize> {
    match arr.raw_iter().next()? {
        MaybeRef::Ref(r) => index.get(&ident(r)).copied(),
        MaybeRef::NotRef(_) => None,
    }
}

fn named_dest(pdf: &Pdf, index: &HashMap<ObjectIdentifier, usize>, key: &[u8]) -> Option<usize> {
    let root = root(pdf)?;
    // PDF 1.1 style: /Dests is a dictionary of destinations.
    if let Some(dests) = root.get::<Dict<'_>>("Dests") {
        if let Some(found) = dests.get::<Object<'_>>(key) {
            return dest_page(pdf, index, found);
        }
    }
    // PDF 1.2+ style: /Names /Dests is a name tree.
    let tree = root.get::<Dict<'_>>("Names")?.get::<Dict<'_>>("Dests")?;
    let found = name_tree_lookup(&tree, key, 0)?;
    dest_page(pdf, index, found)
}

fn name_tree_lookup<'a>(node: &Dict<'a>, key: &[u8], depth: usize) -> Option<Object<'a>> {
    if depth > 32 {
        return None;
    }
    if let Some(names) = node.get::<Array<'a>>("Names") {
        let items: Vec<Object<'a>> = names.iter::<Object<'a>>().collect();
        for pair in items.chunks_exact(2) {
            let k = match &pair[0] {
                Object::String(s) => s.as_bytes(),
                Object::Name(n) => n.as_ref(),
                _ => continue,
            };
            if k == key {
                return Some(pair[1].clone());
            }
        }
    }
    if let Some(kids) = node.get::<Array<'a>>("Kids") {
        for kid in kids.iter::<Dict<'a>>() {
            if let Some(found) = name_tree_lookup(&kid, key, depth + 1) {
                return Some(found);
            }
        }
    }
    None
}

/// Debug helper: describes the raw /Dest (or /A) of the first outline item.
pub fn debug_first_dest(pdf: &Pdf) -> String {
    let Some(root) = root(pdf) else { return "no root".into() };
    let Some(outlines) = root.get::<Dict<'_>>("Outlines") else { return "no outlines".into() };
    let Some(first) = outlines.get::<Dict<'_>>("First") else { return "no First".into() };
    let kind = |o: &Object<'_>| match o {
        Object::Array(_) => "array",
        Object::Dict(_) => "dict",
        Object::Name(_) => "name",
        Object::String(_) => "string",
        _ => "other",
    };
    let dest = first.get::<Object<'_>>("Dest").map(|d| kind(&d).to_owned());
    let action_d = first
        .get::<Dict<'_>>("A")
        .and_then(|a| a.get::<Object<'_>>("D"))
        .map(|d| kind(&d).to_owned());
    let names_dests = root.get::<Dict<'_>>("Names").and_then(|n| n.get::<Dict<'_>>("Dests")).is_some();
    let legacy = root.get::<Dict<'_>>("Dests").is_some();
    let ids = pdf.pages().iter().take(3).map(|p| p.raw().obj_id().is_some()).collect::<Vec<_>>();
    let array_first = first.get::<Object<'_>>("Dest").or_else(|| {
        first.get::<Dict<'_>>("A")?.get::<Object<'_>>("D")
    });
    let arr_kind = match array_first {
        Some(Object::Array(a)) => format!("{:?}", a.raw_iter().next().map(|m| matches!(m, MaybeRef::Ref(_)))),
        _ => "-".into(),
    };
    format!("Dest={dest:?} A.D={action_d:?} NamesDests={names_dests} LegacyDests={legacy} page_ids={ids:?} firstElemIsRef={arr_kind}")
}

pub fn outline(pdf: &Pdf) -> Vec<TocEntry> {
    let index = page_index(pdf);
    let Some(root) = root(pdf) else { return Vec::new() };
    let Some(outlines) = root.get::<Dict<'_>>("Outlines") else { return Vec::new() };
    let mut out = Vec::new();
    if let Some(first) = outlines.get::<Dict<'_>>("First") {
        walk(pdf, &index, first, 0, &mut out);
    }
    out
}

fn walk(
    pdf: &Pdf,
    index: &HashMap<ObjectIdentifier, usize>,
    mut node: Dict<'_>,
    depth: usize,
    out: &mut Vec<TocEntry>,
) {
    let mut seen = 0usize;
    loop {
        seen += 1;
        if out.len() >= MAX_OUTLINE || seen > MAX_OUTLINE || depth > 64 {
            return;
        }
        let title = node
            .get::<hayro::hayro_syntax::object::String<'_>>("Title")
            .map(|s| decode_text(s.as_bytes()))
            .unwrap_or_default();
        let page = node
            .get::<Object<'_>>("Dest")
            .and_then(|d| dest_page(pdf, index, d))
            .or_else(|| {
                let action = node.get::<Dict<'_>>("A")?;
                let dest = action.get::<Object<'_>>("D")?;
                dest_page(pdf, index, dest)
            });
        out.push(TocEntry { title: title.trim().to_owned(), page, depth });
        if let Some(child) = node.get::<Dict<'_>>("First") {
            walk(pdf, index, child, depth + 1, out);
        }
        match node.get::<Dict<'_>>("Next") {
            Some(next) => node = next,
            None => return,
        }
    }
}

/// Labels for every page (e.g. "i", "ii", "1", "A-3"), with the page index as
/// fallback when the document defines no labels.
pub fn page_labels(pdf: &Pdf) -> Vec<String> {
    let count = pdf.pages().len();
    let mut ranges: Vec<(usize, Dict<'_>)> = Vec::new();
    if let Some(root) = root(pdf) {
        if let Some(tree) = root.get::<Dict<'_>>("PageLabels") {
            collect_label_ranges(&tree, &mut ranges, 0);
        }
    }
    ranges.sort_by_key(|(start, _)| *start);
    (0..count)
        .map(|i| {
            let Some((start, dict)) = ranges.iter().rev().find(|(s, _)| *s <= i) else {
                return (i + 1).to_string();
            };
            let offset = i - start;
            let first = dict.get::<f32>("St").map_or(1, |v| v as usize);
            let number = first + offset;
            let prefix = dict
                .get::<hayro::hayro_syntax::object::String<'_>>("P")
                .map(|s| decode_text(s.as_bytes()))
                .unwrap_or_default();
            let style = dict.get::<Name<'_>>("S").map(|n| n.as_ref().to_vec());
            let body = match style.as_deref() {
                Some(b"D") => number.to_string(),
                Some(b"R") => roman(number).to_uppercase(),
                Some(b"r") => roman(number),
                Some(b"A") => letters(number).to_uppercase(),
                Some(b"a") => letters(number),
                _ => String::new(),
            };
            format!("{prefix}{body}")
        })
        .collect()
}

fn collect_label_ranges<'a>(node: &Dict<'a>, out: &mut Vec<(usize, Dict<'a>)>, depth: usize) {
    if depth > 32 {
        return;
    }
    if let Some(nums) = node.get::<Array<'a>>("Nums") {
        let items: Vec<Object<'a>> = nums.iter::<Object<'a>>().collect();
        for pair in items.chunks_exact(2) {
            if let (Some(n), Some(d)) = (pair[0].clone().into_number(), pair[1].clone().into_dict()) {
                out.push((n.as_f32().max(0.0) as usize, d));
            }
        }
    }
    if let Some(kids) = node.get::<Array<'a>>("Kids") {
        for kid in kids.iter::<Dict<'a>>() {
            collect_label_ranges(&kid, out, depth + 1);
        }
    }
}

fn roman(mut n: usize) -> String {
    const TABLE: [(usize, &str); 13] = [
        (1000, "m"), (900, "cm"), (500, "d"), (400, "cd"), (100, "c"), (90, "xc"),
        (50, "l"), (40, "xl"), (10, "x"), (9, "ix"), (5, "v"), (4, "iv"), (1, "i"),
    ];
    let mut out = String::new();
    for (value, sym) in TABLE {
        while n >= value {
            out.push_str(sym);
            n -= value;
        }
    }
    out
}

fn letters(n: usize) -> String {
    let n = n.max(1) - 1;
    let letter = (b'a' + (n % 26) as u8) as char;
    std::iter::repeat_n(letter, n / 26 + 1).collect()
}

/// Link annotations of one page. Only links with a page destination or a URI are kept.
pub fn links(pdf: &Pdf, page: usize) -> Vec<Link> {
    let index = page_index(pdf);
    let Some(p) = pdf.pages().get(page) else { return Vec::new() };
    let Some(annots) = p.raw().get::<Array<'_>>("Annots") else { return Vec::new() };
    let mut out = Vec::new();
    for annot in annots.iter::<Dict<'_>>() {
        if annot.get::<Name<'_>>("Subtype").map(|n| n.as_ref().to_vec()).as_deref() != Some(b"Link") {
            continue;
        }
        let Some(rect) = annot.get::<hayro::hayro_syntax::object::Rect>("Rect") else { continue };
        let target = if let Some(dest) = annot.get::<Object<'_>>("Dest") {
            dest_page(pdf, &index, dest).map_or(LinkTarget::Unresolved, LinkTarget::Page)
        } else if let Some(action) = annot.get::<Dict<'_>>("A") {
            match action.get::<Name<'_>>("S").map(|n| n.as_ref().to_vec()).as_deref() {
                Some(b"URI") => action
                    .get::<hayro::hayro_syntax::object::String<'_>>("URI")
                    .map_or(LinkTarget::Unresolved, |s| LinkTarget::Uri(decode_text(s.as_bytes()))),
                Some(b"GoTo") => action
                    .get::<Object<'_>>("D")
                    .and_then(|d| dest_page(pdf, &index, d))
                    .map_or(LinkTarget::Unresolved, LinkTarget::Page),
                _ => LinkTarget::Unresolved,
            }
        } else {
            LinkTarget::Unresolved
        };
        out.push(Link { rect: [rect.x0, rect.y0, rect.x1, rect.y1], target });
    }
    out
}
