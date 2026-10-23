//! Reader view. Two ways to read the same book:
//!
//! * **PDF** — continuous vertical scroll of rendered pages, virtualized.
//!   MuPDF renders on a worker thread, newest request first; a page keeps its
//!   previous bitmap until the sharper one arrives, so zooming and fast
//!   scrolling never flash blank paper. Text selection, links and highlights
//!   live on top.
//! * **Reading mode** — the text re-set as a book (paragraphs, serif type,
//!   themes), in a variable-height virtualized list.
//!
//! Chrome follows Apple Books: a slim top bar that appears when the pointer
//! nears the top edge, an "Aa" popover for appearance, the page counter at the
//! bottom, contents on the left and notes on the right.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::ui::{icon_button, text_button};
use gpui::prelude::FluentBuilder;
use gpui::{
    div, img, list, point, px, rgb, AppContext, Context, Entity, EventEmitter,
    FocusHandle, Focusable, FontWeight, HighlightStyle, InteractiveElement, IntoElement,
    KeyDownEvent, ListAlignment, ListOffset, ListState, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ObjectFit, ParentElement, Pixels, Point, Render, RenderImage,
    ScrollHandle, SharedString, StatefulInteractiveElement, Styled, StyledImage, StyledText,
    Subscription, Window, WindowControlArea,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::Icon;
use imago_gpui::button;
use imago_gpui::theme::{c, rgba, ACCENT, FG, POPOVER, SIDEBAR_BG, SIDEBAR_DIVIDER};
use imago_gpui::toggle::toggle;
use serde::{Deserialize, Serialize};
use smallvec::smallvec;

use crate::flow::{self, Kind, Para};
use crate::library;
use crate::mac;
use crate::notes::{Highlight, LinkTarget, Notes, PageText, Tint};
use crate::pdf::{Opened, RasterKey, Renderer, TocEntry, Tone};
use crate::raster::{self, RasterCache, IMAGE_BUDGET, PREVIEW_PIXELS};
use crate::reading_state::ReadingState;

const TOP: f32 = 64.0;
const MARGIN: f32 = 24.0;
const GAP: f32 = 14.0;
const PANE_W: f32 = 340.0;
use crate::ui::TITLEBAR_H as BAR_H;
/// Reading mode is a list of paragraphs; only this much is laid out beyond the viewport.
const FLOW_OVERDRAW: f32 = 400.0;
const ZOOM_MIN: f32 = 0.05;
const ZOOM_MAX: f32 = 4.0;
const FONT_MIN: f32 = 13.0;
const FONT_MAX: f32 = 32.0;
const COLUMN_W: f32 = 700.0;
/// Pages kept decoded beyond the visible range.
const KEEP_SLACK: usize = 2;
/// Time the top bar (and traffic lights) take to fade.
const BAR_FADE_MS: f32 = 220.0;
const MAX_HITS: usize = 120;

struct Hit {
    page: u32,
    chapter: String,
    before: String,
    found: String,
    after: String,
}

pub enum ReaderEvent {
    Page(u32),
    Close,
}

/// Reader settings shared by all books.
#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
#[serde(default)]
struct Prefs {
    tone: Tone,
    font_size: f32,
    serif: bool,
    reflow: bool,
    toc_width: f32,
    notes_width: f32,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            tone: Tone::Day,
            font_size: 19.0,
            serif: true,
            reflow: false,
            toc_width: PANE_W,
            notes_width: PANE_W,
        }
    }
}

impl Prefs {
    fn path(root: &Path) -> PathBuf {
        root.join("prefs.json")
    }

    fn load(root: &Path) -> Self {
        std::fs::read(Self::path(root))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn save(&self, root: &Path) {
        if let Ok(json) = serde_json::to_vec_pretty(self) {
            let _ = std::fs::create_dir_all(root);
            let _ = std::fs::write(Self::path(root), json);
        }
    }
}

enum Load {
    Loading,
    Ready,
    Failed(String),
}

/// Text selection on one page: glyph indices `a..=b` (either order).
#[derive(Clone, Copy)]
struct Sel {
    page: u32,
    a: usize,
    b: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct FlowPoint {
    page: usize,
    paragraph: usize,
    byte: usize,
}

#[derive(Clone, Copy)]
struct FlowSel { anchor: FlowPoint, cursor: FlowPoint }

impl FlowSel {
    fn range(&self, page: usize, paragraph: usize, len: usize) -> Option<std::ops::Range<usize>> {
        let (a, b) = (self.anchor.min(self.cursor), self.anchor.max(self.cursor));
        if (page, paragraph) < (a.page, a.paragraph) || (page, paragraph) > (b.page, b.paragraph) {
            return None;
        }
        let start = if (page, paragraph) == (a.page, a.paragraph) { a.byte.min(len) } else { 0 };
        let end = if (page, paragraph) == (b.page, b.paragraph) { b.byte.min(len) } else { len };
        (start < end).then_some(start..end)
    }
}

/// StyledText requires disjoint runs. The newest mark (selection last) wins
/// where saved highlights overlap, without repeating or dropping any glyphs.
fn disjoint_highlights(ranges: Vec<(std::ops::Range<usize>, HighlightStyle)>)
    -> Vec<(std::ops::Range<usize>, HighlightStyle)> {
    let mut events: Vec<_> = ranges.iter().enumerate().filter(|(_, (r, _))| r.start < r.end)
        .flat_map(|(i, (r, _))| [(r.start, i, true), (r.end, i, false)]).collect();
    events.sort_unstable_by_key(|e| e.0);
    let mut active = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    let mut previous = 0;
    let mut i = 0;
    while i < events.len() {
        let offset = events[i].0;
        if previous < offset && !active.is_empty() {
            let style = active.iter().fold(HighlightStyle::default(), |style, index: &usize|
                style.highlight(ranges[*index].1));
            out.push((previous..offset, style));
        }
        while i < events.len() && events[i].0 == offset {
            let (_, index, start) = events[i];
            if start { active.insert(index); } else { active.remove(&index); }
            i += 1;
        }
        previous = offset;
    }
    out
}

fn syntax_style(token: crate::syntax::Token, tone: Tone) -> HighlightStyle {
    use crate::syntax::Token;
    let light = matches!(tone, Tone::Day | Tone::Sepia);
    let color = match (token, light) {
        (Token::Comment, false) => 0x8F9EAC, (Token::Comment, true) => 0x586575,
        (Token::String, false) => 0xA6D79A, (Token::String, true) => 0x286A31,
        (Token::Number, false) => 0xE9B47C, (Token::Number, true) => 0x985119,
        (Token::Keyword, false) => 0xC7A5EF, (Token::Keyword, true) => 0x7940A4,
        (Token::Type, false) => 0xE6CF8B, (Token::Type, true) => 0x785D13,
        (Token::Function, false) => 0x8DBEF2, (Token::Function, true) => 0x215D9C,
        (Token::Operator, false) => 0x84CDD3, (Token::Operator, true) => 0x17656E,
    };
    HighlightStyle::color(rgb(color).into())
}

/// Page geometry for one viewport width and zoom.
struct Layout {
    tops: Vec<f32>,
    heights: Vec<f32>,
    width: f32,
    total: f32,
    /// Left edge of the page column inside the scrolled content.
    left: f32,
}

impl Layout {
    fn new(sizes: &[(f32, f32)], viewport_w: f32, zoom: f32) -> Self {
        let width = ((viewport_w - 2.0 * MARGIN) * zoom).max(80.0);
        let mut tops = Vec::with_capacity(sizes.len());
        let mut heights = Vec::with_capacity(sizes.len());
        let mut y = TOP;
        for (pw, ph) in sizes {
            let h = width * ph / pw;
            tops.push(y);
            heights.push(h);
            y += h + GAP;
        }
        Self {
            tops,
            heights,
            width,
            total: y - GAP + MARGIN,
            left: MARGIN,
        }
    }

    /// Page containing document-space `y` (the last page above it).
    fn page_at(&self, y: f32) -> usize {
        self.tops
            .partition_point(|top| *top <= y)
            .saturating_sub(1)
            .min(self.tops.len().saturating_sub(1))
    }

    /// Inclusive range of pages intersecting `[y0, y1]`.
    fn visible(&self, y0: f32, y1: f32) -> (usize, usize) {
        let first = self.page_at(y0);
        let mut last = self.page_at(y1);
        if self.tops[last] > y1 {
            last = first;
        }
        (first, last.max(first))
    }
}

type FlowPages = Rc<RefCell<Vec<Option<Vec<Para>>>>>;
type FlowLayouts = Rc<RefCell<HashMap<(usize, usize), (gpui::TextLayout, f32)>>>;

pub struct Reader {
    pub book_id: String,
    title: SharedString,
    root: PathBuf,
    renderer: Renderer,
    state: Load,
    sizes: Vec<(f32, f32)>,
    toc: Arc<Vec<TocEntry>>,
    show_toc: bool,
    toc_motion: crate::ui::PaneMotion,
    notes_motion: crate::ui::PaneMotion,
    pane_frame_pending: bool,
    pane_widths: (f32, f32),
    toc_width: f32,
    notes_width: f32,
    pane_resize: Option<(bool, f32, f32)>,
    zoom: f32,
    prefs: Prefs,
    scroll: ScrollHandle,
    toc_scroll: ListState,
    focus: FocusHandle,
    images: RasterCache<Arc<RenderImage>>,
    failed: HashSet<RasterKey>,
    page: u32,
    viewport_w: f32,
    layout: Option<Layout>,
    /// Page + fraction to scroll to once the layout exists.
    anchor: Option<(u32, f32)>,
    resume: Option<ReadingState>,
    saved_view: Option<ReadingState>,
    observed_view: Option<ReadingState>,
    save_pending: bool,
    fit_page: bool,
    project_flow: bool,
    mode_return: Option<ReadingState>,
    mode_entry: Option<ReadingState>,
    flow_target: Option<FlowPoint>,
    flow_layouts: FlowLayouts,
    code_scroll: Rc<RefCell<HashMap<(usize, usize), ScrollHandle>>>,
    notes: Notes,
    texts: HashMap<u32, PageText>,
    sel: Option<Sel>,
    flow_sel: Option<FlowSel>,
    dragging: bool,
    word_selection: bool,
    copied_at: Option<Instant>,
    /// Highlight whose note is being edited.
    active: Option<String>,
    show_notes: bool,
    note_input: Entity<InputState>,
    /// Top bar visibility (pointer near the top edge).
    bar: bool,
    /// Bar fade progress, `0.0` hidden .. `1.0` shown.
    bar_t: f32,
    bar_animating: bool,
    search_refresh_pending: bool,
    search_revision: u64,
    appearance: bool,
    show_search: bool,
    search_input: Entity<InputState>,
    /// Plain text per page, filled by the worker once search is first opened.
    index: Vec<Option<Arc<str>>>,
    index_requested: bool,
    query: String,
    hits: Vec<Hit>,
    flow: FlowPages,
    flow_list: ListState,
    flow_requested: bool,
    _subs: Vec<Subscription>,
}

/// Opening a tool before metadata arrives must not mark an empty pass as
/// requested: the next ready frame will schedule the real page range.
fn begin_bulk(total: usize, start: u32, requested: &mut bool) -> Vec<u32> {
    if *requested || total == 0 {
        return Vec::new();
    }
    *requested = true;
    let total = total as u32;
    let start = start.min(total - 1);
    (start..total).chain(0..start).collect()
}

fn scan_hits(index: &[Option<Arc<str>>], toc: &[TocEntry], query: &str) -> Vec<Hit> {
    let mut hits = Vec::new();
    let needle: Vec<char> = query.trim().chars().flat_map(char::to_lowercase).collect();
    if needle.is_empty() {
        return hits;
    }
    'pages: for (page, text) in index.iter().enumerate() {
        let Some(text) = text else { continue };
        let chars: Vec<char> = text.chars().collect();
        let lower: Vec<char> = chars
            .iter()
            .map(|c| c.to_lowercase().next().unwrap_or(*c))
            .collect();
        let mut i = 0;
        while i + needle.len() <= lower.len() {
            if lower[i..i + needle.len()] == needle[..] {
                let from = i.saturating_sub(36);
                let to = (i + needle.len() + 60).min(chars.len());
                let collect = |a: usize, b: usize| chars[a..b].iter().collect::<String>();
                let chapter = toc
                    .iter()
                    .filter(|t| t.page.is_some_and(|p| p as usize <= page))
                    .next_back()
                    .map(|t| t.title.clone())
                    .unwrap_or_default();
                hits.push(Hit {
                    page: page as u32,
                    chapter,
                    before: format!("{}{}", if from > 0 { "…" } else { "" }, collect(from, i)),
                    found: collect(i, i + needle.len()),
                    after: format!(
                        "{}{}",
                        collect(i + needle.len(), to),
                        if to < chars.len() { "…" } else { "" }
                    ),
                });
                if hits.len() >= MAX_HITS {
                    break 'pages;
                }
                // One hit per page keeps the list scannable.
                continue 'pages;
            }
            i += 1;
        }
    }
    hits
}

impl EventEmitter<ReaderEvent> for Reader {}

/// Whole-page fallback only. Full-resolution content is rasterized in tiles,
/// so zoom no longer allocates a huge bitmap or sacrifices text sharpness.
const MAX_PAGE_PX: f32 = PREVIEW_PIXELS;

fn capped_scale(pw: f32, ph: f32, want: f32) -> f32 {
    let limit = (MAX_PAGE_PX / (pw * ph).max(1.0)).sqrt();
    want.min(limit)
}

fn scale_key(scale: f32) -> u32 {
    (scale * 50.0).round() as u32
}

impl Reader {
    pub fn new(
        book_id: String,
        title: String,
        path: PathBuf,
        start_page: u32,
        root: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let notes = Notes::open(root, &book_id);
        let note_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Заметка к выделению"));
        let sub = cx.subscribe(&note_input, |this, state, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change) {
                if let Some(id) = this.active.clone() {
                    let text = state.read(cx).value().to_string();
                    this.notes.update(&id, |h| h.note = text);
                    cx.notify();
                }
            }
        });
        let search_input = cx.new(|cx| InputState::new(window, cx).placeholder("Поиск в книге"));
        let search_sub = cx.subscribe(&search_input, |this, state, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change) {
                this.query = state.read(cx).value().to_string();
                this.queue_search(cx);
            }
        });
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let mut prefs = Prefs::load(root);
        let resume = ReadingState::load(root, &book_id);
        let start_page = resume.map_or(start_page, |s| s.page);
        if let Some(state) = resume {
            prefs.reflow = state.reflow;
        }
        let renderer = Renderer::spawn(path);
        let wake = renderer.notifications();
        cx.spawn_in(window, async move |this, cx| {
            while wake.recv().await.is_ok() {
                if this
                    .update_in(cx, |this, window, cx| this.drain(window, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        let release = cx.on_release_in(window, |this, window, _| {
            this.flush_view();
            this.release_images(window);
        });
        let quit = cx.on_app_quit(|this, _| {
            this.flush_view();
            async {}
        });
        Self {
            book_id,
            title: title.into(),
            root: root.to_path_buf(),
            renderer,
            state: Load::Loading,
            sizes: Vec::new(),
            toc: Arc::new(Vec::new()),
            show_toc: false,
            toc_motion: crate::ui::PaneMotion::new(),
            notes_motion: crate::ui::PaneMotion::new(),
            pane_frame_pending: false,
            pane_widths: (0.0, 0.0),
            toc_width: prefs.toc_width.clamp(240.0, 600.0),
            notes_width: prefs.notes_width.clamp(240.0, 600.0),
            pane_resize: None,
            zoom: resume.map_or(1.0, |s| s.zoom),
            prefs,
            scroll: ScrollHandle::new(),
            toc_scroll: ListState::new(0, ListAlignment::Top, px(160.)),
            focus,
            images: RasterCache::new(IMAGE_BUDGET),
            failed: HashSet::new(),
            page: start_page,
            viewport_w: 0.0,
            layout: None,
            anchor: Some((start_page, resume.map_or(0.0, |s| s.fraction))),
            resume,
            saved_view: resume,
            observed_view: resume,
            save_pending: false,
            fit_page: resume.is_none(),
            project_flow: false,
            mode_return: None,
            mode_entry: None,
            flow_target: None,
            code_scroll: Rc::new(RefCell::new(HashMap::new())),
            flow_layouts: Rc::new(RefCell::new(HashMap::new())),
            notes,
            texts: HashMap::new(),
            sel: None,
            flow_sel: None,
            dragging: false,
            word_selection: false,
            copied_at: None,
            active: None,
            show_notes: false,
            note_input,
            bar: true,
            bar_t: 1.0,
            bar_animating: false,
            search_refresh_pending: false,
            search_revision: 0,
            appearance: false,
            show_search: false,
            search_input,
            index: Vec::new(),
            index_requested: false,
            query: String::new(),
            hits: Vec::new(),
            flow: Rc::new(RefCell::new(Vec::new())),
            flow_list: ListState::new(0, ListAlignment::Top, px(FLOW_OVERDRAW)),
            flow_requested: false,
            _subs: vec![sub, search_sub, release, quit],
        }
    }

    fn viewport_state(&self) -> Option<ReadingState> {
        if !matches!(self.state, Load::Ready) || self.resume.is_some() || self.flow_target.is_some() {
            return None;
        }
        let mut state = ReadingState { zoom: self.zoom, reflow: self.prefs.reflow,
            ..Default::default() };
        if self.prefs.reflow {
            let top = self.flow_list.logical_scroll_top();
            let pages = self.flow.borrow();
            let (page, paragraph) = flow_locate(&pages, top.item_ix);
            // Do not replace a real paragraph with a temporary loading placeholder.
            let paras = pages.get(page)?.as_ref()?;
            state.page = page as u32;
            state.paragraph = paragraph;
            state.paragraph_offset = f32::from(top.offset_in_item);
            let total: usize = paras.iter().map(|p| p.text.len().max(1)).sum();
            let before: usize = paras.iter().take(paragraph).map(|p| p.text.len().max(1)).sum();
            state.fraction = before as f32 / total.max(1) as f32;
            if let Some((layout, inset)) = self.flow_layouts.borrow().get(&(page, paragraph)) {
                let bounds = layout.bounds();
                let pos = point(bounds.left(), bounds.top() + px((state.paragraph_offset - inset).max(0.0)));
                state.text_byte = layout.index_for_position(pos).unwrap_or_else(|i| i);
                if let Some(y) = paras.get(paragraph).and_then(|p| p.source_y(state.text_byte)) {
                    state.fraction = y / self.sizes.get(page).map_or(1.0, |s| s.1);
                }
            }
        } else {
            let lay = self.layout.as_ref()?;
            if let Some((page, fraction)) = self.anchor {
                state.page = page;
                state.fraction = fraction;
            } else {
                let page = lay.page_at(self.doc_y());
                state.page = page as u32;
                state.fraction = (self.doc_y() - lay.tops[page]) / lay.heights[page];
            }
            let range = (lay.width + 2.0 * MARGIN - self.viewport_w).max(0.0);
            state.horizontal = if range > 0.0 {
                (-f32::from(self.scroll.offset().x) / range).clamp(0.0, 1.0)
            } else { 0.0 };
        }
        Some(state)
    }

    /// One pending save regardless of wheel event rate; no idle polling or frame writes.
    fn save_view_soon(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.viewport_state() else { return };
        if self.mode_return.is_some() && self.mode_entry.is_none() {
            self.mode_entry = Some(state);
        }
        if self.observed_view == Some(state) { return; }
        self.observed_view = Some(state);
        if self.save_pending { return; }
        self.save_pending = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(350)).await;
            let _ = this.update(cx, |this, _| {
                this.save_pending = false;
                this.flush_view();
            });
        }).detach();
    }

    pub fn flush_view(&mut self) {
        let Some(state) = self.viewport_state() else { return };
        if self.saved_view != Some(state) {
            match state.save(&self.root, &self.book_id) {
                Ok(()) => self.saved_view = Some(state),
                Err(err) => eprintln!("Liber: could not save reading position: {err}"),
            }
        }
    }

    fn save_prefs(&self) {
        self.prefs.save(&self.root);
    }

    fn bar_wanted(&self) -> bool {
        self.bar || self.appearance || self.show_search
    }

    fn drain(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut changed = false;
        let mut indexed = false;
        while let Ok((page, text)) = self.renderer.index.try_recv() {
            if let Some(slot) = self.index.get_mut(page as usize) {
                *slot = Some(Arc::from(text));
                indexed = true;
            }
        }
        if indexed {
            changed = true;
            if !self.query.is_empty() {
                self.queue_search(cx);
            }
        }
        if matches!(self.state, Load::Loading) {
            if let Ok(opened) = self.renderer.opened.try_recv() {
                match opened {
                    Ok(Opened { sizes, toc }) => {
                        let n = sizes.len();
                        self.index = vec![None; n];
                        self.flow = Rc::new(RefCell::new(vec![None; n]));
                        self.flow_list = ListState::new(n, ListAlignment::Top, px(FLOW_OVERDRAW));
                        // Resume where the book was left, not at the first page.
                        self.page = self.page.min(n.saturating_sub(1) as u32);
                        self.flow_list.scroll_to(ListOffset {
                            item_ix: self.page as usize,
                            offset_in_item: px(0.),
                        });
                        self.sizes = sizes;
                        self.toc_scroll = ListState::new(toc.len(), ListAlignment::Top, px(160.));
                        self.toc = Arc::new(toc);
                        self.state = Load::Ready;
                        if self.prefs.reflow {
                            self.enter_reflow();
                        }
                    }
                    Err(e) => self.state = Load::Failed(e),
                }
                changed = true;
            }
        }
        while let Ok((page, text)) = self.renderer.texts.try_recv() {
            self.texts.insert(page, text);
            changed = true;
        }
        while let Ok((page, paras)) = self.renderer.flows.try_recv() {
            let ix = page as usize;
            let (start, old, new) = {
                let mut pages = self.flow.borrow_mut();
                if ix >= pages.len() {
                    continue;
                }
                let start = flow_start(&pages, ix);
                let old = flow_len(&pages[ix]);
                pages[ix] = Some(paras);
                (start, old, flow_len(&pages[ix]))
            };
            splice_flow_page(&self.flow_list, start, old, new);
            changed = true;
        }
        self.restore_flow_position();
        while let Ok(reply) = self.renderer.replies.try_recv() {
            let key = reply.key();
            if self.prefs.reflow || !self.images.wanted(&key) || key.tone != self.prefs.tone {
                continue;
            }
            match reply.image {
                Ok(r) => {
                    let bytes = r.bgra.len();
                    if let Some(buf) = image::RgbaImage::from_raw(r.width, r.height, r.bgra) {
                        let image = Arc::new(RenderImage::new(smallvec![image::Frame::new(buf)]));
                        for displaced in self.images.insert(key, image, bytes) {
                            let _ = window.drop_image(displaced);
                        }
                    }
                }
                Err(_) => {
                    self.failed.insert(key);
                }
            }
            changed = true;
        }
        if changed {
            cx.notify();
        }
    }

    fn bar_eased(&self) -> f32 {
        let t = self.bar_t;
        t * t * (3.0 - 2.0 * t)
    }

    // --- search ---------------------------------------------------------------

    fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_search = true;
        self.appearance = false;
        self.ensure_index();
        self.renderer.set_activity(self.prefs.reflow, true);
        let focus = self.search_input.focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    fn ensure_index(&mut self) {
        for page in begin_bulk(self.sizes.len(), self.page, &mut self.index_requested) {
            self.renderer.request_index(page);
        }
    }

    fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_search = false;
        self.renderer.set_activity(self.prefs.reflow, false);
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Coalesce typing/index updates and scan immutable text snapshots off
    /// the UI thread. A superseded query can never overwrite newer results.
    fn queue_search(&mut self, cx: &mut Context<Self>) {
        self.search_revision += 1;
        if self.search_refresh_pending {
            return;
        }
        self.search_refresh_pending = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(50))
                .await;
            let Ok((revision, index, toc, query)) = this.update(cx, |this, _| {
                (
                    this.search_revision,
                    this.index.clone(),
                    this.toc.clone(),
                    this.query.clone(),
                )
            }) else {
                return;
            };
            let task_query = query.clone();
            let hits = cx
                .background_executor()
                .spawn(async move { scan_hits(&index, &toc, &task_query) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.search_refresh_pending = false;
                if this.query == query {
                    this.hits = hits;
                    cx.notify();
                }
                if this.search_revision != revision {
                    this.queue_search(cx);
                }
            });
        })
        .detach();
    }

    // --- navigation ---------------------------------------------------------

    fn doc_y(&self) -> f32 {
        -f32::from(self.scroll.offset().y)
    }

    fn scroll_to_doc_y(&self, y: f32) {
        let max = self.layout.as_ref().map_or(0.0, |l| l.total);
        let x = self.scroll.offset().x;
        self.scroll.set_offset(point(x, px(-y.clamp(0.0, max))));
    }

    fn go_to_page(&mut self, page: u32, cx: &mut Context<Self>) {
        self.resume = None;
        self.flow_target = None;
        let last = self.sizes.len().saturating_sub(1) as u32;
        let page = page.min(last);
        if self.prefs.reflow {
            let at = flow_start(&self.flow.borrow(), page as usize);
            self.flow_list.scroll_to(ListOffset {
                item_ix: at,
                offset_in_item: px(0.),
            });
        } else {
            self.anchor = Some((page, 0.0));
        }
        self.page = page;
        cx.emit(ReaderEvent::Page(page));
        cx.notify();
    }

    fn set_zoom(&mut self, zoom: f32, cx: &mut Context<Self>) {
        self.fit_page = false;
        let zoom = zoom.clamp(ZOOM_MIN, ZOOM_MAX);
        if (zoom - self.zoom).abs() < 1e-3 {
            return;
        }
        if let Some(lay) = &self.layout {
            let p = lay.page_at(self.doc_y());
            let frac = (self.doc_y() - lay.tops[p]) / lay.heights[p];
            self.anchor = Some((p as u32, frac));
        }
        self.zoom = zoom;
        cx.notify();
    }

    /// Larger/smaller: type size in reading mode, zoom on PDF pages.
    fn scale_content(&mut self, up: bool, cx: &mut Context<Self>) {
        if self.prefs.reflow {
            let step = if up { 1.0 } else { -1.0 };
            self.prefs.font_size = (self.prefs.font_size + step).clamp(FONT_MIN, FONT_MAX);
            self.flow_list.remeasure();
            self.save_prefs();
            cx.notify();
        } else {
            let f = if up { 1.15 } else { 1.0 / 1.15 };
            self.set_zoom(self.zoom * f, cx);
        }
    }

    fn scroll_by(&mut self, dy: f32, cx: &mut Context<Self>) {
        if self.prefs.reflow {
            self.flow_list.scroll_by(px(dy));
        } else {
            self.scroll_to_doc_y(self.doc_y() + dy);
        }
        cx.notify();
    }

    fn viewport_h(&self) -> f32 {
        if self.prefs.reflow {
            f32::from(self.flow_list.viewport_bounds().size.height).max(400.0)
        } else {
            f32::from(self.scroll.bounds().size.height)
        }
    }

    fn set_tone(&mut self, tone: Tone, cx: &mut Context<Self>) {
        if tone == self.prefs.tone {
            return;
        }
        self.prefs.tone = tone;
        // Old-tone bitmaps stay on screen until the new ones arrive.
        self.failed.clear();
        self.save_prefs();
        cx.notify();
    }

    fn toggle_invert(&mut self, cx: &mut Context<Self>) {
        let dark = matches!(self.prefs.tone, Tone::Gray | Tone::Night);
        self.set_tone(if dark { Tone::Day } else { Tone::Night }, cx);
    }

    fn set_reflow(&mut self, on: bool, cx: &mut Context<Self>) {
        if on == self.prefs.reflow {
            return;
        }
        // ListState and ScrollHandle are the live source of truth. `self.page`
        // is a display counter from an earlier render and can lag wheel input.
        let source = self.viewport_state().or(self.resume).or(self.mode_return).unwrap_or(ReadingState {
            page: self.page, zoom: self.zoom, reflow: self.prefs.reflow, ..Default::default()
        });
        let roundtrip = self.mode_entry.is_some_and(|entry|
            entry.page == source.page && entry.paragraph == source.paragraph
            && (entry.paragraph_offset - source.paragraph_offset).abs() < 1.0
            && (entry.fraction - source.fraction).abs() < 0.0001
            && (entry.horizontal - source.horizontal).abs() < 0.0001
            && entry.zoom == source.zoom);
        let target = if roundtrip { self.mode_return.unwrap_or(source) } else { source };
        self.mode_return = Some(source);
        self.mode_entry = None;
        self.flow_target = None;
        self.page = target.page.min(self.sizes.len().saturating_sub(1) as u32);
        self.prefs.reflow = on;
        self.resume = Some(ReadingState { page: self.page, reflow: on, ..target });
        self.project_flow = on && !roundtrip;
        self.renderer.set_activity(on, self.show_search);
        self.save_prefs();
        if on {
            self.enter_reflow();
            self.restore_flow_position();
        } else {
            self.anchor = Some((self.page, target.fraction));
        }
        self.sel = None;
        self.flow_sel = None;
        self.active = None;
        cx.emit(ReaderEvent::Page(self.page));
        cx.notify();
    }

    fn restore_flow_position(&mut self) {
        if !self.prefs.reflow { return; }
        let Some(state) = self.resume else { return };
        let pages = self.flow.borrow();
        let page = (state.page as usize).min(pages.len().saturating_sub(1));
        let Some(Some(paras)) = pages.get(page) else { return };
        let y = state.fraction * self.sizes.get(page).map_or(1.0, |s| s.1);
        let paragraph = if self.project_flow {
            paras.iter().enumerate().flat_map(|(i, para)| para.source.iter().map(move |line| {
                let distance = (line.y0 - y).max(0.0).max(y - line.y1);
                (i, distance)
            })).min_by(|a, b| a.1.total_cmp(&b.1)).map_or(0, |p| p.0)
        } else { state.paragraph.min(paras.len().saturating_sub(1)) };
        if self.project_flow && !paras.is_empty() {
            self.flow_target = Some(FlowPoint { page, paragraph,
                byte: paras.get(paragraph).map_or(0, |p| p.byte_at_y(y)) });
        }
        self.flow_list.scroll_to(ListOffset { item_ix: flow_start(&pages, page) + paragraph,
            offset_in_item: px(if self.project_flow { 0.0 } else { state.paragraph_offset }) });
        self.resume = None;
        self.project_flow = false;
        self.anchor = None;
    }

    /// Ask the worker for every page's paragraphs, current page first.
    fn enter_reflow(&mut self) {
        for page in begin_bulk(self.sizes.len(), self.page, &mut self.flow_requested) {
            self.renderer.request_flow(page);
        }
    }

    fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        let vh = self.viewport_h();
        let cmd = k.modifiers.platform || k.modifiers.control;
        if self.search_input.focus_handle(cx).is_focused(window) {
            if k.key == "escape" {
                self.close_search(window, cx);
            }
            return;
        }
        if self.note_input.focus_handle(cx).is_focused(window) {
            if k.key == "escape" {
                window.focus(&self.focus, cx);
            }
            return;
        }
        match (k.key.as_str(), cmd) {
            ("pagedown", _) | ("space", false) => self.scroll_by(vh * 0.9, cx),
            ("pageup", _) => self.scroll_by(-vh * 0.9, cx),
            ("down", false) => self.scroll_by(60.0, cx),
            ("up", false) => self.scroll_by(-60.0, cx),
            ("home", _) => self.go_to_page(0, cx),
            ("end", _) => self.go_to_page(u32::MAX, cx),
            ("right", false) => self.go_to_page(self.page + 1, cx),
            ("left", false) => self.go_to_page(self.page.saturating_sub(1), cx),
            ("=", true) | ("+", true) => self.scale_content(true, cx),
            ("-", true) => self.scale_content(false, cx),
            ("0", true) if !self.prefs.reflow => self.set_zoom(1.0, cx),
            ("f", true) => self.open_search(window, cx),
            ("r", false) => self.set_reflow(!self.prefs.reflow, cx),
            ("i", false) => self.toggle_invert(cx),
            ("escape", _) if self.appearance => {
                self.appearance = false;
                cx.notify();
            }
            ("c", true) => self.copy_selection(cx),
            ("escape", _) if self.sel.is_some() || self.flow_sel.is_some() || self.active.is_some() => {
                self.sel = None;
                self.flow_sel = None;
                self.active = None;
                cx.notify();
            }
            ("escape", _) => cx.emit(ReaderEvent::Close),
            _ => {}
        }
    }

    // --- selection and highlights ------------------------------------------

    fn copy_selection(&mut self, cx: &mut Context<Self>) {
        let text = if self.prefs.reflow {
            self.flow_selection().0
        } else {
            self.sel.and_then(|s| self.texts.get(&s.page).map(|t| t.text(s.a, s.b)))
                .unwrap_or_default()
        };
        let text = if text.is_empty() {
            self.active.as_ref().and_then(|id| self.notes.items.iter().find(|h| &h.id == id))
                .map_or(String::new(), |h| h.text.clone())
        } else { text };
        if text.is_empty() { return; }
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
        self.sel = None;
        self.flow_sel = None;
        self.active = None;
        self.dragging = false;
        let stamp = Instant::now();
        self.copied_at = Some(stamp);
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(1340)).await;
            let _ = this.update(cx, |_, cx| cx.notify());
            cx.background_executor().timer(Duration::from_millis(160)).await;
            let _ = this.update(cx, |this, cx| {
                if this.copied_at == Some(stamp) { this.copied_at = None; cx.notify(); }
            });
        }).detach();
    }

    /// Window position -> page index and page-space (points) coordinates.
    /// With `only`, the point is clamped onto that page.
    fn to_page(&self, pos: Point<Pixels>, only: Option<u32>) -> Option<(u32, f32, f32)> {
        let lay = self.layout.as_ref()?;
        let b = self.scroll.bounds();
        let off = self.scroll.offset();
        let dx = f32::from(pos.x - b.origin.x - off.x);
        let dy = f32::from(pos.y - b.origin.y - off.y);
        let page = only.map_or_else(|| lay.page_at(dy), |p| p as usize);
        let (pw, ph) = *self.sizes.get(page)?;
        let x = ((dx - lay.left) / lay.width * pw).clamp(0.0, pw);
        let y = ((dy - lay.tops[page]) / lay.heights[page] * ph).clamp(0.0, ph);
        Some((page as u32, x, y))
    }

    fn on_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        self.appearance = false;
        self.show_search = false;
        self.sel = None;
        self.word_selection = ev.click_count >= 2;
        let Some((page, x, y)) = self.to_page(ev.position, None) else {
            return;
        };
        if let Some(i) = self.texts.get(&page).and_then(|t| t.hit(x, y)) {
            let (a, b) = if self.word_selection {
                let t = &self.texts[&page];
                let line = t.chars[i].line;
                let start = (0..=i).rev().take_while(|&j| t.chars[j].line == line).last().unwrap_or(i);
                let end = (i..t.chars.len()).take_while(|&j| t.chars[j].line == line).last().unwrap_or(i) + 1;
                let text: String = t.chars[start..end].iter().map(|g| g.c).collect();
                let byte: usize = t.chars[start..i].iter().map(|g| g.c.len_utf8()).sum();
                let range = word_range(&text, byte);
                (start + text[..range.start].chars().count(), start + text[..range.end].chars().count().saturating_sub(1))
            } else { (i, i) };
            self.sel = Some(Sel { page, a, b });
            self.dragging = !self.word_selection;
        }
        cx.notify();
    }

    fn on_move(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
        if !self.dragging {
            return;
        }
        let Some(sel) = self.sel else { return };
        if let Some((_, x, y)) = self.to_page(ev.position, Some(sel.page)) {
            if let Some(i) = self.texts.get(&sel.page).and_then(|t| t.hit(x, y)) {
                if i != sel.b {
                    self.sel = Some(Sel { b: i, ..sel });
                    cx.notify();
                }
            }
        }
    }

    fn on_up(&mut self, ev: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        let was_dragging = std::mem::take(&mut self.dragging);
        if !was_dragging {
            return;
        }
        if self.sel.is_some_and(|s| s.a != s.b) {
            return;
        }
        // A plain click: follow a link, else open the highlight under the cursor.
        self.sel = None;
        if let Some((page, x, y)) = self.to_page(ev.position, None) {
            match self.texts.get(&page).and_then(|t| t.link_at(x, y)).cloned() {
                Some(LinkTarget::Page(p)) => {
                    self.go_to_page(p, cx);
                    return;
                }
                Some(LinkTarget::Uri(uri)) => {
                    let _ = std::process::Command::new("open").arg(uri).spawn();
                    return;
                }
                None => {}
            }
        }
        let hit = self.to_page(ev.position, None).and_then(|(page, x, y)| {
            self.notes
                .on_page(page)
                .find(|h| h.contains(x, y))
                .map(|h| h.id.clone())
        });
        match hit {
            Some(id) => self.activate(&id, window, cx),
            None => self.active = None,
        }
        cx.notify();
    }

    fn activate(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(note) = self
            .notes
            .items
            .iter()
            .find(|h| h.id == id)
            .map(|h| h.note.clone())
        else {
            return;
        };
        self.active = Some(id.to_owned());
        self.show_notes = true;
        self.note_input
            .update(cx, |s, cx| s.set_value(note, window, cx));
    }

    fn highlight_selection(&mut self, tint: Tint, window: &mut Window, cx: &mut Context<Self>) {
        if self.prefs.reflow {
            let (text, flow) = self.flow_selection();
            let Some(first) = flow.first() else { return };
            let page = first.page;
            let id = uuid::Uuid::new_v4().to_string();
            self.notes.add(Highlight { id: id.clone(), page, rects: Vec::new(), text,
                tint, note: String::new(), created: library::now(), flow });
            self.flow_sel = None;
            self.activate(&id, window, cx);
            cx.notify();
            return;
        }
        let Some(sel) = self.sel.take() else { return };
        let Some(text) = self.texts.get(&sel.page) else {
            return;
        };
        let rects = text.rects(sel.a, sel.b);
        if rects.is_empty() {
            return;
        }
        let id = format!(
            "{:x}",
            library::now() * 1000 + self.notes.items.len() as u64
        );
        self.notes.add(Highlight {
            id: id.clone(),
            page: sel.page,
            rects,
            text: text.text(sel.a, sel.b),
            tint,
            note: String::new(),
            created: library::now(),
            flow: Vec::new(),
        });
        self.activate(&id, window, cx);
        cx.notify();
    }

    // --- chrome -------------------------------------------------------------

    fn action_bar(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let selecting = self.sel.is_some_and(|s| s.a != s.b || self.word_selection)
            || self.flow_sel.is_some_and(|s| s.anchor != s.cursor);
        let active = self.active.clone();
        if !selecting && active.is_none() {
            return None;
        }
        let mut row = div().flex().items_center().gap_2();
        for (i, tint) in Tint::ALL.into_iter().enumerate() {
            let id = active.clone();
            row = row.child(
                div()
                    .id(("tint", i))
                    .size(px(22.))
                    .rounded_full()
                    .bg(rgb(tint.hex()))
                    .cursor_pointer()
                    .hover(|s| s.opacity(0.8))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if selecting {
                            this.highlight_selection(tint, window, cx);
                        } else if let Some(id) = &id {
                            this.notes.update(id, |h| h.tint = tint);
                            cx.notify();
                        }
                    })),
            );
        }
        let mut row = div().flex().flex_col().gap_2().p_3().rounded_lg()
            .bg(c(POPOVER())).border_1().border_color(c(SIDEBAR_DIVIDER())).shadow_lg()
            .child(row)
            .child(button::ghost("copy-selection").w_full()
                .icon(Icon::empty().path("icons/copy.svg"))
                .accessibility_label("Скопировать")
                .on_click(cx.listener(|this, _, _, cx| this.copy_selection(cx))));
        if !selecting {
            if let Some(id) = active {
                row = row.child(
                    button::ghost("hl-del")
                        .icon(Icon::empty().path("icons/delete.svg"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.notes.remove(&id);
                            this.active = None;
                            cx.notify();
                        })),
                );
            }
        }
        Some(
            div()
                .absolute()
                .top(px(BAR_H + 12.))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(row.occlude())
                .into_any_element(),
        )
    }

    fn notes_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut list = div()
            .id("notes-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_2()
            .flex()
            .flex_col()
            .gap_2();
        if self.notes.items.is_empty() {
            list = list.child(
                div()
                    .p_2()
                    .text_size(px(13.))
                    .text_color(rgba(FG(), 0.5))
                    .child("Выделите текст мышью и выберите цвет маркера"),
            );
        }
        for (i, h) in self.notes.items.iter().enumerate() {
            let id = h.id.clone();
            let page = h.page;
            let is_active = self.active.as_deref() == Some(h.id.as_str());
            let mut card = div()
                .id(("note", i))
                .p_2()
                .rounded_md()
                .flex()
                .flex_col()
                .gap_1()
                .cursor_pointer()
                .bg(if is_active {
                    rgba(FG(), 0.10)
                } else {
                    rgba(FG(), 0.04)
                })
                .hover(|s| s.bg(rgba(FG(), 0.08)))
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .items_center()
                        .child(div().size(px(8.)).rounded_full().bg(rgb(h.tint.hex())))
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(rgba(FG(), 0.5))
                                .child(format!("стр. {}", h.page + 1)),
                        ),
                )
                .child(
                    div()
                        .max_h(px(66.))
                        .overflow_hidden()
                        .text_size(px(13.))
                        .child(h.text.clone()),
                );
            if !h.note.is_empty() && !is_active {
                card = card.child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgba(FG(), 0.65))
                        .child(h.note.clone()),
                );
            }
            if is_active {
                card = card.child(div().mt_1().child(Input::new(&self.note_input)));
            }
            list = list.child(card.on_click(cx.listener(move |this, _, window, cx| {
                this.activate(&id, window, cx);
                if !this.prefs.reflow {
                    this.go_to_page(page, cx);
                }
            })));
        }
        div()
            .relative()
            .w_full()
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .pt(px(BAR_H))
            .bg(c(POPOVER()))
            .child(
                div()
                    .h(px(36.))
                    .flex_none()
                    .px_3()
                    .flex()
                    .items_center()
                    .text_size(px(13.))
                    .text_color(rgba(FG(), 0.7))
                    .justify_between()
                    .child(format!("Заметки · {}", self.notes.items.len()))
                    .child(
                        icon_button(
                            "notes-close",
                            "icons/status-x.svg",
                            "Закрыть заметки",
                            false,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.show_notes = false;
                            window.focus(&this.focus, cx);
                            cx.notify();
                        })),
                    ),
            )
            .child(list)
            .child(self.pane_resize_handle(true, cx))
    }

    fn top_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        if self.bar_t <= 0.01 {
            // Text scrolls under the bar area, so no resting title in either mode.
            return div();
        }
        // Native window controls sit at the left on macOS; elsewhere they are on the right.
        let (pad_l, pad_r) = if cfg!(target_os = "macos") {
            (88., 12.)
        } else {
            (12., 140.)
        };
        let left = div()
            .flex_1()
            .flex()
            .pl(px(pad_l))
            .items_center()
            .gap_1()
            .child(
                icon_button("back", "icons/arrow-left.svg", "Библиотека", false)
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(ReaderEvent::Close))),
            )
            .child(
                icon_button("toc", "icons/sidebar-left.svg", "Содержание", self.show_toc).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.show_toc = !this.show_toc;
                        cx.notify();
                    }),
                ),
            );
        let right = div()
            .flex_1()
            .flex()
            .pr(px(pad_r))
            .items_center()
            .justify_end()
            .gap_1()
            .child(
                icon_button("search", "icons/search.svg", "Поиск", self.show_search).on_click(
                    cx.listener(|this, _, window, cx| {
                        if this.show_search {
                            this.close_search(window, cx);
                        } else {
                            this.open_search(window, cx);
                        }
                    }),
                ),
            )
            .child(
                text_button("aa", "Aa", "Оформление", self.appearance).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.appearance = !this.appearance;
                        this.show_search = false;
                        cx.notify();
                    },
                )),
            )
            .child(
                icon_button("notes", "icons/inbox.svg", "Заметки", self.show_notes).on_click(
                    cx.listener(|this, _, window, cx| {
                        this.show_notes = !this.show_notes;
                        if !this.show_notes {
                            window.focus(&this.focus, cx);
                        }
                        cx.notify();
                    }),
                ),
            );
        div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(px(BAR_H))
            .flex()
            .items_center()
            .gap_3()
            .bg(c(SIDEBAR_BG()))
            .border_b_1()
            .border_color(c(SIDEBAR_DIVIDER()))
            .opacity(self.bar_eased())
            .window_control_area(WindowControlArea::Drag)
            .child(left)
            .child(
                div()
                    .flex_none()
                    .max_w(gpui::relative(0.5))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgba(FG(), 0.8))
                    .child(self.title.clone()),
            )
            .child(right)
    }

    /// The "Aa" popover: size, themes, typeface and reading mode.
    fn appearance_popover(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let reflow = self.prefs.reflow;
        let tone = self.prefs.tone;
        let size_label = if reflow {
            format!("{} pt", self.prefs.font_size.round() as i32)
        } else {
            format!("{}%", (self.zoom * 100.0).round() as i32)
        };
        let row = || div().flex().items_center().justify_between().gap_2();
        let caption = |t: &'static str| {
            div()
                .text_size(px(12.))
                .text_color(rgba(FG(), 0.55))
                .child(t)
        };

        let mut themes = div().flex().gap_2();
        for (i, t) in Tone::ALL.into_iter().enumerate() {
            let on = t == tone;
            themes = themes.child(
                div()
                    .id(("theme", i))
                    .flex_1()
                    .h(px(44.))
                    .rounded_lg()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .bg(rgb(t.paper()))
                    .text_color(rgb(t.ink()))
                    .text_size(px(16.))
                    .font_family("Georgia")
                    .border_2()
                    .border_color(if on {
                        c(ACCENT())
                    } else {
                        c(SIDEBAR_DIVIDER())
                    })
                    .child("Aa")
                    .on_click(cx.listener(move |this, _, _, cx| this.set_tone(t, cx))),
            );
        }
        let mut pop = div()
            .absolute()
            .top(px(BAR_H + 6.))
            .right(px(12.))
            .w(px(300.))
            .p_3()
            .flex()
            .flex_col()
            .gap_3()
            .rounded_xl()
            .bg(c(POPOVER()))
            .border_1()
            .border_color(c(SIDEBAR_DIVIDER()))
            .shadow_lg()
            .occlude()
            .child(
                row()
                    .child(
                        button::secondary("size-down")
                            .label("A")
                            .accessibility_label("Меньше")
                            .on_click(cx.listener(|this, _, _, cx| this.scale_content(false, cx))),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(rgba(FG(), 0.7))
                            .child(size_label),
                    )
                    .child(
                        button::secondary("size-up")
                            .label("A+")
                            .accessibility_label("Больше")
                            .on_click(cx.listener(|this, _, _, cx| this.scale_content(true, cx))),
                    ),
            )
            .child(themes)
            .child(
                row()
                    .child(caption("Режим чтения (текст как в книге)"))
                    .child(toggle("reflow-toggle", reflow, cx, |this, on, cx| {
                        this.set_reflow(on, cx)
                    })),
            );
        if reflow {
            let serif = self.prefs.serif;
            pop = pop.child(
                row()
                    .child(
                        if serif {
                            button::secondary("font-serif")
                        } else {
                            button::ghost("font-serif")
                        }
                        .label("Georgia")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.prefs.serif = true;
                            this.flow_list.remeasure();
                            this.save_prefs();
                            cx.notify();
                        })),
                    )
                    .child(
                        if serif {
                            button::ghost("font-sans")
                        } else {
                            button::secondary("font-sans")
                        }
                        .label("Inter")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.prefs.serif = false;
                            this.flow_list.remeasure();
                            this.save_prefs();
                            cx.notify();
                        })),
                    ),
            );
        }
        pop
    }

    fn search_popover(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut results = div()
            .id("search-results")
            .max_h(px(440.))
            .overflow_y_scroll()
            .flex()
            .flex_col();
        if self.query.trim().is_empty() {
            let indexed = self.index.iter().filter(|t| t.is_some()).count();
            let note = if indexed < self.index.len() {
                format!("Индексирую книгу: {} из {}…", indexed, self.index.len())
            } else {
                "Введите слово или фразу".to_string()
            };
            results = results.child(
                div()
                    .p_3()
                    .text_size(px(12.))
                    .text_color(rgba(FG(), 0.5))
                    .child(note),
            );
        } else if self.hits.is_empty() {
            let indexing = self.index.iter().any(|t| t.is_none());
            results = results.child(
                div()
                    .p_3()
                    .text_size(px(12.))
                    .text_color(rgba(FG(), 0.5))
                    .child(if indexing {
                        "Ничего не найдено (индексация ещё идёт)"
                    } else {
                        "Ничего не найдено"
                    }),
            );
        }
        for (i, hit) in self.hits.iter().enumerate() {
            let page = hit.page;
            let text = format!("{}{}{}", hit.before, hit.found, hit.after);
            let start = hit.before.len();
            let mark = HighlightStyle {
                font_weight: Some(FontWeight::BOLD),
                ..Default::default()
            };
            let snippet =
                StyledText::new(text).with_highlights([(start..start + hit.found.len(), mark)]);
            results = results.child(
                div()
                    .id(("hit", i))
                    .px_3()
                    .py_2()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .cursor_pointer()
                    .border_b_1()
                    .border_color(rgba(FG(), 0.06))
                    .hover(|s| s.bg(rgba(FG(), 0.07)))
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .gap_2()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(
                                div()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(if hit.chapter.is_empty() {
                                        "Без главы".to_string()
                                    } else {
                                        hit.chapter.clone()
                                    }),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(rgba(FG(), 0.55))
                                    .child((page + 1).to_string()),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .line_height(px(16.))
                            .text_color(rgba(FG(), 0.75))
                            .child(snippet),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.go_to_page(page, cx);
                        this.close_search(window, cx);
                    })),
            );
        }
        div()
            .absolute()
            .top(px(BAR_H + 6.))
            .right(px(12.))
            .w(px(360.))
            .flex()
            .flex_col()
            .rounded_xl()
            .overflow_hidden()
            .bg(c(POPOVER()))
            .border_1()
            .border_color(c(SIDEBAR_DIVIDER()))
            .shadow_lg()
            .occlude()
            .child(
                div()
                    .p_2()
                    .child(Input::new(&self.search_input).cleanable(true)),
            )
            .child(results)
    }

    fn toc_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let list = if self.toc.is_empty() {
            div()
                .p_3()
                .text_size(px(13.))
                .text_color(rgba(FG(), 0.5))
                .child("В документе нет оглавления")
                .into_any_element()
        } else {
            let entries = self.toc.clone();
            let weak = cx.weak_entity();
            list(self.toc_scroll.clone(), move |ix, _, _| {
                let entry = &entries[ix];
                let page = entry.page;
                let top = entry.depth == 0;
                let weak = weak.clone();
                div().w_full().px_1().child(
                    div().id(("toc-item", ix)).w_full().min_h(px(32.))
                        .py_2().pl(px(12.0 + 14.0 * f32::from(entry.depth.min(8)))).pr_3()
                        .rounded_md().text_size(px(13.)).line_height(px(18.))
                        .font_weight(if top { FontWeight::MEDIUM } else { FontWeight::NORMAL })
                        .text_color(rgba(FG(), if top { 0.9 } else { 0.7 }))
                        .whitespace_normal().cursor_pointer()
                        .hover(|s| s.bg(rgba(FG(), 0.08)))
                        .child(entry.title.clone())
                        .on_click(move |_, _, cx| {
                            let _ = weak.update(cx, |this, cx| {
                                if let Some(p) = page { this.go_to_page(p, cx); }
                            });
                        })
                ).into_any_element()
            }).size_full().into_any_element()
        };
        div()
            .relative()
            .w_full()
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .pt(px(BAR_H))
            .bg(c(POPOVER()))
            .child(
                div()
                    .h(px(36.))
                    .flex_none()
                    .px_3()
                    .flex()
                    .items_center()
                    .text_size(px(13.))
                    .text_color(rgba(FG(), 0.7))
                    .justify_between()
                    .child("Содержание")
                    .child(
                        icon_button(
                            "toc-close",
                            "icons/status-x.svg",
                            "Закрыть содержание",
                            false,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_toc = false;
                            cx.notify();
                        })),
                    ),
            )
            .child(div().flex_1().min_h_0().child(list))
            .child(self.pane_resize_handle(false, cx))
    }

    fn pane_resize_handle(&self, right: bool, cx: &mut Context<Self>) -> impl IntoElement {
        use gpui::prelude::FluentBuilder;
        div()
            .id(if right { "notes-resize" } else { "toc-resize" })
            .absolute()
            .top(px(BAR_H))
            .bottom_0()
            .w(px(8.))
            .when(right, |el| el.left_0())
            .when(!right, |el| el.right_0())
            .cursor_col_resize()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.pane_resize = Some((
                        right,
                        f32::from(ev.position.x),
                        if right {
                            this.notes_width
                        } else {
                            this.toc_width
                        },
                    ));
                }),
            )
    }

    /// Limit both CPU pixels and their GPU copies; all displaced images have a
    /// single release path, including replacements and reader teardown.
    pub fn release_images(&mut self, window: &mut Window) {
        self.renderer.set_view(&[], &[], &[]);
        for image in self.images.clear() {
            let _ = window.drop_image(image);
        }
        self.texts.clear();
        self.failed.clear();
    }

    fn schedule(
        &mut self,
        lay: &Layout,
        first: usize,
        last: usize,
        scale_factor: f32,
        viewport_h: f32,
    ) {
        let tone = self.prefs.tone;
        let vw = self.viewport_w.max(1.0);
        let vh = viewport_h.max(1.0);
        // Fit the visible detailed raster plus a small scroll margin in 48 MiB.
        // At ordinary Retina window sizes this keeps the full device scale.
        let dpr = scale_factor
            .min((48.0 * 1024.0 * 1024.0 / (vw * vh * 4.0 * 1.8)).sqrt())
            .max(0.5);
        let y = self.doc_y();
        let x = -f32::from(self.scroll.offset().x);
        let content_w = vw.max(lay.width + 2.0 * MARGIN);
        let left = (content_w - lay.width) / 2.0;
        let mut wanted = Vec::new();
        let mut detail = Vec::new();
        for page in first..=last {
            let (pw, ph) = self.sizes[page];
            let factor = lay.width / pw * dpr;
            let preview = (capped_scale(pw, ph, factor) * 50.0).floor().max(1.0) as u32;
            wanted.push(RasterKey {
                page: page as u32,
                scale: preview,
                tone,
                tile: None,
            });
            let scale = scale_key(factor).max(1);
            if scale <= preview
                || self.toc_motion.animating()
                || self.notes_motion.animating()
                || self.pane_resize.is_some()
            {
                continue;
            }
            let to_pdf = pw / lay.width;
            // One tile of movement beyond the viewport, rather than decoding
            // entire neighboring pages at full Retina resolution.
            let margin = 160.0;
            let rect = [
                (x - left - margin) * to_pdf,
                (y - lay.tops[page] - margin) * to_pdf,
                (x - left + vw + margin) * to_pdf,
                (y + vh - lay.tops[page] + margin) * to_pdf,
            ];
            detail.extend(
                raster::tiles(pw, ph, scale as f32 / 50.0, rect)
                    .into_iter()
                    .map(|tile| RasterKey {
                        page: page as u32,
                        scale,
                        tone,
                        tile: Some(tile),
                    }),
            );
        }
        // Center tiles first so sharpening follows the reading position.
        detail.sort_by(|a, b| {
            let distance = |key: &RasterKey| {
                let tile = key.tile.unwrap();
                let cy = lay.tops[key.page as usize]
                    + (tile.y as f32 + tile.height as f32 / 2.0)
                        / key.factor()
                        / self.sizes[key.page as usize].0
                        * lay.width;
                (cy - (y + vh / 2.0)).abs()
            };
            distance(a).total_cmp(&distance(b))
        });
        let mut planned_bytes: usize = wanted
            .iter()
            .map(|key| {
                let (pw, ph) = self.sizes[key.page as usize];
                (pw * key.factor()).ceil() as usize * (ph * key.factor()).ceil() as usize * 4
            })
            .sum();
        for key in detail {
            let tile = key.tile.unwrap();
            let bytes = tile.width as usize * tile.height as usize * 4;
            if planned_bytes + bytes <= IMAGE_BUDGET {
                planned_bytes += bytes;
                wanted.push(key);
            }
        }
        self.images.set_wanted(&wanted);
        self.failed.retain(|key| wanted.contains(key));
        let missing: Vec<_> = wanted
            .iter()
            .copied()
            .filter(|key| !self.images.contains(key) && !self.failed.contains(key))
            .collect();
        let lo = first.saturating_sub(KEEP_SLACK) as u32;
        let hi = (last + KEEP_SLACK) as u32;
        self.texts.retain(|page, _| *page >= lo && *page <= hi);
        let text_pages: Vec<_> = (first..=last)
            .map(|p| p as u32)
            .filter(|p| !self.texts.contains_key(p))
            .collect();
        self.renderer.set_view(&wanted, &missing, &text_pages);
        if std::env::var_os("LIBER_PERF_LOG").is_some() && !missing.is_empty() {
            eprintln!(
                "LIBER_PERF images={} cpu_pixels_bytes={} wanted={} missing={}",
                self.images.len(),
                self.images.bytes(),
                wanted.len(),
                missing.len()
            );
        }
    }
}

/// Reading mode lists paragraphs: a page that is not extracted yet is one
/// placeholder row, an extracted one is one row per paragraph.
fn flow_len(page: &Option<Vec<Para>>) -> usize {
    page.as_ref().map_or(1, |p| p.len().max(1))
}

/// ListState resets scroll offsets inside a replaced range. A repair of the
/// currently visible page must preserve its paragraph and intra-row offset.
fn splice_flow_page(list: &ListState, start: usize, old: usize, new: usize) {
    let top = list.logical_scroll_top();
    list.splice(start..start + old, new);
    let item_ix = if start + old <= top.item_ix {
        top.item_ix + new - old
    } else if start <= top.item_ix {
        start + (top.item_ix - start).min(new.saturating_sub(1))
    } else { top.item_ix };
    list.scroll_to(ListOffset { item_ix, offset_in_item: top.offset_in_item });
}

/// Row index of the first row of `page`.
fn flow_start(pages: &[Option<Vec<Para>>], page: usize) -> usize {
    pages.iter().take(page).map(flow_len).sum()
}

/// (page, paragraph) shown in row `ix`.
fn flow_locate(pages: &[Option<Vec<Para>>], mut ix: usize) -> (usize, usize) {
    for (page, p) in pages.iter().enumerate() {
        let n = flow_len(p);
        if ix < n {
            return (page, ix);
        }
        ix -= n;
    }
    (pages.len().saturating_sub(1), 0)
}

impl Render for Reader {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        mac::align_traffic_lights(window);
        if self.show_search {
            self.ensure_index();
        }
        self.renderer
            .set_activity(self.prefs.reflow, self.show_search);
        if self.prefs.reflow && self.images.len() > 0 {
            self.release_images(window);
        }
        let target = if self.bar_wanted() { 1.0 } else { 0.0 };
        if (self.bar_t - target).abs() > f32::EPSILON && !self.bar_animating {
            self.bar_animating = true;
            let started = Instant::now();
            cx.on_next_frame(window, move |this, window, cx| {
                this.bar_animating = false;
                let target = if this.bar_wanted() { 1.0 } else { 0.0 };
                let step = started.elapsed().as_secs_f32() * 1000.0 / BAR_FADE_MS;
                this.bar_t = if target > this.bar_t {
                    (this.bar_t + step).min(target)
                } else {
                    (this.bar_t - step).max(target)
                };
                mac::set_traffic_lights_alpha(window, f64::from(this.bar_eased()));
                cx.notify();
            });
        }
        let now = Instant::now();
        let toc_p = self.toc_motion.progress(self.show_toc, now);
        let notes_p = self.notes_motion.progress(self.show_notes, now);
        self.pane_widths = (self.toc_width * toc_p, self.notes_width * notes_p);
        if (self.toc_motion.animating() || self.notes_motion.animating())
            && !self.pane_frame_pending
        {
            self.pane_frame_pending = true;
            cx.on_next_frame(window, |this, _, cx| {
                this.pane_frame_pending = false;
                cx.notify();
            });
        }
        let weak = cx.weak_entity();
        let tone = self.prefs.tone;
        let body = match &self.state {
            Load::Loading => centered("Открываю…"),
            Load::Failed(e) => centered(format!("Не удалось открыть книгу: {e}")),
            Load::Ready if self.prefs.reflow => self.reflow_view(window, cx).into_any_element(),
            Load::Ready => self.pages(window, cx).into_any_element(),
        };
        if self.prefs.reflow && self.flow_target.is_some() {
            cx.on_next_frame(window, |this, _, cx| {
                if let Some(target) = this.flow_target {
                    let ready = this.flow_layouts.borrow().get(&(target.page, target.paragraph))
                        .and_then(|(layout, inset)| layout.position_for_index(target.byte)
                            .map(|pos| f32::from(pos.y - layout.bounds().top()) + inset));
                    if let Some(offset) = ready {
                        let item_ix = flow_start(&this.flow.borrow(), target.page) + target.paragraph;
                        this.flow_list.scroll_to(ListOffset { item_ix, offset_in_item: px(offset) });
                        this.flow_target = None;
                    }
                    cx.notify();
                }
            });
        }
        self.save_view_soon(cx);
        let mut row = div().size_full().flex();
        if toc_p > 0.0 {
            row = row.child(crate::ui::reader_pane(
                self.toc_width,
                toc_p,
                false,
                self.toc_panel(cx),
            ));
        }
        row = row.child(div().flex_1().min_w_0().h_full().child(body));
        if notes_p > 0.0 {
            row = row.child(crate::ui::reader_pane(
                self.notes_width,
                notes_p,
                true,
                self.notes_panel(cx),
            ));
        }

        let total = self.sizes.len();
        let mut root = div()
            .id("reader")
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(rgb(tone.surround()))
            .text_color(c(FG()))
            .track_focus(&self.focus)
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !*hovered && this.bar {
                    this.bar = false;
                    cx.notify();
                }
            }))
            .on_key_down(move |ev, window, cx| {
                let _ = weak.update(cx, |this, cx| this.on_key(ev, window, cx));
            })
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, window, cx| {
                if let Some((right, start_x, width)) = this.pane_resize {
                    let delta = f32::from(ev.position.x) - start_x;
                    let maximum = (f32::from(window.viewport_size().width) * 0.45)
                        .min(600.0)
                        .max(240.0);
                    let width = (width + if right { -delta } else { delta }).clamp(240.0, maximum);
                    if right {
                        this.notes_width = width;
                    } else {
                        this.toc_width = width;
                    }
                    cx.notify();
                }
                let y = f32::from(ev.position.y);
                let want = if y < BAR_H + 24.0 {
                    Some(true)
                } else if y > BAR_H + 90.0 {
                    Some(false)
                } else {
                    None
                };
                if let Some(v) = want {
                    if v != this.bar {
                        this.bar = v;
                        cx.notify();
                    }
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.pane_resize.take().is_some() {
                        this.prefs.toc_width = this.toc_width;
                        this.prefs.notes_width = this.notes_width;
                        this.save_prefs();
                        cx.notify();
                    }
                }),
            )
            .child(row);
        if total > 0 {
            root = root.child(
                div()
                    .absolute()
                    .bottom(px(10.))
                    .left_0()
                    .right_0()
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(tone.ink()))
                            .opacity(0.5)
                            .child(format!("{}", self.page + 1)),
                    ),
            );
        }
        root = root.child(self.top_bar(cx));
        if self.show_search {
            root = root.child(self.search_popover(cx));
        }
        if self.appearance {
            root = root.child(self.appearance_popover(cx));
        }
        if let Some(bar) = self.action_bar(cx) {
            root = root.child(bar);
        }
        if let Some(stamp) = self.copied_at {
            let age = stamp.elapsed().as_secs_f32();
            let alpha = (age / 0.16).min((1.5 - age) / 0.16).clamp(0.0, 1.0);
            if age < 0.16 || age >= 1.34 {
                cx.on_next_frame(window, |_, _, cx| cx.notify());
            }
            root = root.child(div().absolute().bottom(px(28.0 - (1.0 - alpha) * 10.0))
                .left_0().right_0().flex().justify_center().child(
                    div().px_4().py_2().rounded_lg().shadow_lg().bg(c(POPOVER()))
                        .text_color(c(FG())).opacity(alpha).child("Скопировано")));
        }
        root
    }
}

fn centered(text: impl Into<SharedString>) -> gpui::AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(14.))
        .text_color(rgba(FG(), 0.6))
        .child(text.into())
        .into_any_element()
}

impl Reader {
    fn flow_selection(&self) -> (String, Vec<crate::notes::FlowRange>) {
        let Some(selection) = self.flow_sel else { return (String::new(), Vec::new()) };
        let mut text = String::new();
        let mut ranges = Vec::new();
        let pages = self.flow.borrow();
        let first = selection.anchor.page.min(selection.cursor.page);
        let last = selection.anchor.page.max(selection.cursor.page);
        for page in first..=last {
            let Some(Some(paras)) = pages.get(page) else { continue };
            for (paragraph, para) in paras.iter().enumerate() {
                if let Some(range) = selection.range(page, paragraph, para.text.len()) {
                    if let Some(part) = para.text.get(range.clone()) {
                        if !text.is_empty() { text.push('\n'); }
                        text.push_str(part);
                        ranges.push(crate::notes::FlowRange { page: page as u32, paragraph,
                            start: range.start, end: range.end });
                    }
                }
            }
        }
        (text, ranges)
    }

    /// Reading mode: paragraphs of every page in one virtualized list.
    fn reflow_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let item = {
            let pages = self.flow.borrow();
            flow_locate(&pages, self.flow_list.logical_scroll_top().item_ix).0 as u32
        };
        if item != self.page && !self.sizes.is_empty() {
            self.page = item.min(self.sizes.len() as u32 - 1);
            cx.emit(ReaderEvent::Page(self.page));
        }
        if self
            .flow
            .borrow()
            .get(item as usize)
            .is_some_and(|page| page.is_none())
        {
            self.renderer.prioritize_flow(item);
        }
        let tone = self.prefs.tone;
        let size = self.prefs.font_size;
        let mono_id = window.text_system().resolve_font(&gpui::font("Menlo"));
        let mono_advance = f32::from(window.text_system().layout_width(mono_id, px(size * 0.85), '0'));
        let family = if self.prefs.serif { "Georgia" } else { "Inter" };
        let flow = self.flow.clone();
        self.flow_layouts.borrow_mut().clear();
        let layouts = self.flow_layouts.clone();
        let code_scroll = self.code_scroll.clone();
        let weak = cx.weak_entity();
        let scroll_notify = weak.clone();
        self.flow_list.set_scroll_handler(move |_, _, cx| {
            let _ = scroll_notify.update(cx, |_, cx| cx.notify());
        });
        let selection = self.flow_sel;
        let marks: Vec<_> = self.notes.items.iter().flat_map(|note| {
            note.flow.iter().map(move |range| (range.clone(), note.tint, note.id.clone()))
        }).collect();
        let state = list(self.flow_list.clone(), move |ix, _, _| {
            let pages = flow.borrow();
            let (page, i) = flow_locate(&pages, ix);
            let is_last =
                page + 1 >= pages.len() && pages.get(page).is_some_and(|p| i + 1 >= flow_len(p));
            let col = div()
                .w_full()
                .max_w(px(COLUMN_W))
                .px_6()
                .flex()
                .flex_col()
                .pt(px(if ix == 0 { 96.0 } else { 0.0 }))
                .pb(px(if is_last { 140.0 } else { 0.0 }));
            let col = match pages.get(page).and_then(|p| p.as_ref()) {
                None => col.h(px(160.)),
                Some(paras) => match paras.get(i) {
                    None => col.h(px(8.)),
                    Some(para) => {
                        let prev_last = page
                            .checked_sub(1)
                            .and_then(|p| pages[p].as_ref())
                            .and_then(|p| p.last());
                        let joined = i == 0 && prev_last.is_some_and(|p| flow::continues(p, para));
                        let (scale, weight, mt) = match para.kind {
                            Kind::Heading(1) => (1.802, FontWeight::BOLD, 2.4),
                            Kind::Heading(2) => (1.602, FontWeight::SEMIBOLD, 1.9),
                            Kind::Heading(_) => (1.424, FontWeight::SEMIBOLD, 1.5),
                            Kind::Body => {
                                (1.0, FontWeight::NORMAL, if joined { 0.0 } else { 0.85 })
                            }
                            Kind::Code => (0.85, FontWeight::NORMAL, 0.85),
                        };
                        let mut highlights: Vec<_> = para.syntax.iter()
                            .map(|s| (s.start..s.end, syntax_style(s.token, tone))).collect();
                        highlights.extend(marks.iter()
                            .filter(|(r, _, _)| r.page == page as u32 && r.paragraph == i)
                            .filter(|(r, _, _)| para.text.get(r.start..r.end).is_some())
                            .map(|(r, tint, _)| {
                                let mut color: gpui::Hsla = rgb(tint.hex()).into();
                                color.a = 0.35;
                                (r.start..r.end, HighlightStyle { background_color: Some(color), ..Default::default() })
                            }));
                        if let Some(range) = selection.and_then(|s| s.range(page, i, para.text.len())) {
                            let mut color: gpui::Hsla = rgb(0x0A84FF).into();
                            color.a = 0.35;
                            highlights.push((range, HighlightStyle { background_color: Some(color), ..Default::default() }));
                        }
                        let text = StyledText::new(para.text.clone()).with_highlights(disjoint_highlights(highlights));
                        let down_layout = text.layout().clone();
                        layouts.borrow_mut().insert((page, i), (down_layout.clone(),
                            size * mt + if ix == 0 { 96.0 } else { 0.0 }
                            + if para.kind == Kind::Code { 12.0 } else { 0.0 }));
                        let move_layout = down_layout.clone();
                        let down_text = para.text.clone();
                        let down = weak.clone();
                        let motion = weak.clone();
                        let up = weak.clone();
                        let handle = if para.kind == Kind::Code {
                            let mut handles = code_scroll.borrow_mut();
                            if handles.len() > 512 { handles.clear(); }
                            Some(handles.entry((page, i)).or_default().clone())
                        } else { None };
                        let wheel = weak.clone();
                        let paragraph = div()
                                .id(("flow-paragraph", ix))
                                .mt(px(size * mt))
                                .when(para.kind == Kind::Code, |el| el
                                    .w_full().min_w_0()
                                    .font_family("Menlo")
                                    .bg(rgb(if matches!(tone, Tone::Day | Tone::Sepia) { 0xE9E6DF } else { 0x181820 }))
                                    .p_3().rounded_md().whitespace_nowrap().overflow_x_scroll().restrict_scroll_to_axis())
                                .when_some(handle.clone(), |el, handle| {
                                    let scroller = handle.clone();
                                    el.track_scroll(&handle).on_scroll_wheel(move |ev, _, cx| {
                                        let delta = ev.delta.pixel_delta(px(20.));
                                        let x = if ev.modifiers.shift { delta.y + delta.x } else { delta.x };
                                        if x != px(0.) {
                                            let offset = scroller.offset();
                                            scroller.set_offset(point(offset.x + x, offset.y));
                                            cx.stop_propagation();
                                            let _ = wheel.update(cx, |_, cx| cx.notify());
                                        }
                                    })
                                })
                                .text_size(px(size * scale))
                                .line_height(px(size
                                    * scale
                                    * if scale > 1.0 { 1.3 } else { 1.62 }))
                                .font_weight(weight)
                                .cursor(gpui::CursorStyle::IBeam)
                                .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
                                    let byte = down_layout.index_for_position(ev.position).unwrap_or_else(|i| i);
                                    let _ = down.update(cx, |this, cx| {
                                        window.focus(&this.focus, cx);
                                        let point = FlowPoint { page, paragraph: i, byte };
                                        let range = if ev.click_count >= 2 { word_range(&down_text, byte) } else { byte..byte };
                                        this.flow_sel = Some(FlowSel {
                                            anchor: FlowPoint { byte: range.start, ..point },
                                            cursor: FlowPoint { byte: range.end, ..point },
                                        });
                                        this.dragging = ev.click_count < 2;
                                        this.active = None;
                                        this.appearance = false;
                                        cx.notify();
                                    });
                                })
                                .on_mouse_move(move |ev, _, cx| {
                                    let _ = motion.update(cx, |this, cx| {
                                        if this.dragging {
                                            if let Some(sel) = &mut this.flow_sel {
                                                let byte = move_layout.index_for_position(ev.position).unwrap_or_else(|i| i);
                                                let cursor = FlowPoint { page, paragraph: i, byte };
                                                if cursor != sel.cursor { sel.cursor = cursor; cx.notify(); }
                                            }
                                        }
                                    });
                                })
                                .on_mouse_up(MouseButton::Left, move |_, window, cx| {
                                    let _ = up.update(cx, |this, cx| {
                                        this.dragging = false;
                                        if let Some(sel) = this.flow_sel.filter(|s| s.anchor == s.cursor) {
                                            let hit = this.notes.items.iter().find(|n| n.flow.iter().any(|r|
                                                r.page == page as u32 && r.paragraph == i
                                                && r.start <= sel.cursor.byte && sel.cursor.byte < r.end))
                                                .map(|n| n.id.clone());
                                            if let Some(id) = hit { this.activate(&id, window, cx); }
                                            this.flow_sel = None;
                                        }
                                        cx.notify();
                                    });
                                })
                                .child(div().when(para.kind == Kind::Code, |el| el
                                    .min_w(px(para.text.lines().map(|l| l.chars().count()).max().unwrap_or(0) as f32 * mono_advance + 2.0))
                                    .flex_shrink_0()).child(text));
                        col.child(div().relative().child(paragraph).when_some(handle, |el, handle| el.child(
                            gpui_component::scroll::Scrollbar::horizontal(&handle)
                                .id(("code-scrollbar", ix))
                                .mode(gpui_component::scroll::ScrollbarMode::Always))))
                    }
                },
            };
            div()
                .w_full()
                .flex()
                .justify_center()
                .child(col)
                .into_any_element()
        })
        .size_full();
        div()
            .id("flow")
            .size_full()
            .bg(rgb(tone.paper()))
            .text_color(rgb(tone.ink()))
            .font_family(family)
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _, _, _| this.dragging = false))
            .child(state)
    }

    fn pages(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tone = self.prefs.tone;
        // Bounds are those of the previous frame; fall back until measured.
        let measured_h = f32::from(self.scroll.bounds().size.height);
        let viewport_h = if measured_h > 1.0 {
            measured_h
        } else {
            f32::from(window.viewport_size().height)
        };
        let side = self.pane_widths.0 + self.pane_widths.1;
        let fallback = f32::from(window.viewport_size().width) - side;
        let vw = fallback.max(1.0);
        if self.fit_page && !self.sizes.is_empty() {
            let page = (self.page as usize).min(self.sizes.len() - 1);
            let (pw, ph) = self.sizes[page];
            let width = (vw - 2.0 * MARGIN).max(80.0);
            self.zoom = ((viewport_h - TOP - MARGIN).max(1.0) * pw / ph / width)
                .min(1.0).clamp(ZOOM_MIN, ZOOM_MAX);
            self.fit_page = false;
        }
        if (vw - self.viewport_w).abs() > 0.5 {
            // Width changed (resize, panel toggle): keep the reading position.
            if let Some(old) = &self.layout {
                let p = old.page_at(self.doc_y());
                let frac = (self.doc_y() - old.tops[p]) / old.heights[p];
                self.anchor.get_or_insert((p as u32, frac));
            }
            self.viewport_w = vw;
        }

        let width = ((vw - 2.0 * MARGIN) * self.zoom).max(80.0);
        let mut lay = self
            .layout
            .take()
            .filter(|lay| (lay.width - width).abs() < 0.01)
            .unwrap_or_else(|| Layout::new(&self.sizes, vw, self.zoom));
        if let Some((page, frac)) = self.anchor.take() {
            let p = (page as usize).min(lay.tops.len().saturating_sub(1));
            let y = lay.tops[p] + frac * lay.heights[p] - if frac == 0.0 { MARGIN } else { 0.0 };
            self.scroll_to_doc_y_with(&lay, y);
        }
        if let Some(state) = self.resume.take() {
            let range = (lay.width + 2.0 * MARGIN - vw).max(0.0);
            let y = self.scroll.offset().y;
            self.scroll.set_offset(point(px(-state.horizontal * range), y));
        }
        let y0 = self.doc_y();
        let (first, last) = lay.visible(y0 - viewport_h * 0.5, y0 + viewport_h * 1.5);
        self.schedule(&lay, first, last, window.scale_factor(), viewport_h);

        let current = lay.page_at(y0 + viewport_h * 0.35) as u32;
        if current != self.page {
            self.page = current;
            cx.emit(ReaderEvent::Page(current));
        }

        let content_w = vw.max(lay.width + 2.0 * MARGIN);
        let left = (content_w - lay.width) / 2.0;
        lay.left = left;
        let mut canvas = div().relative().w(px(content_w)).h(px(lay.total));
        for page in first..=last {
            let mut paper = div()
                .absolute()
                .left(px(left))
                .top(px(lay.tops[page]))
                .w(px(lay.width))
                .h(px(lay.heights[page]))
                .bg(rgb(tone.paper()))
                .overflow_hidden()
                .shadow_md();
            let (pw, ph) = self.sizes[page];
            let mut layers: Vec<_> = self
                .images
                .entries()
                .filter(|(key, _)| key.page == page as u32)
                .collect();
            let display_tone = if layers.iter().any(|(key, _)| key.tone == tone) {
                tone
            } else {
                layers
                    .iter()
                    .max_by_key(|(_, e)| e.used)
                    .map_or(tone, |(key, _)| key.tone)
            };
            layers.retain(|(key, _)| key.tone == display_tone);
            layers.sort_by_key(|(key, entry)| (key.tile.is_some(), key.scale, entry.used));
            for (key, cached) in layers {
                if let Some(tile) = key.tile {
                    let logical = lay.width / pw / key.factor();
                    paper = paper.child(
                        div()
                            .absolute()
                            .left(px(tile.x as f32 * logical))
                            .top(px(tile.y as f32 * logical))
                            .w(px(tile.width as f32 * logical))
                            .h(px(tile.height as f32 * logical))
                            .child(
                                img(cached.value.clone())
                                    .size_full()
                                    .object_fit(ObjectFit::Fill),
                            ),
                    );
                } else {
                    paper = paper.child(
                        img(cached.value.clone())
                            .absolute()
                            .size_full()
                            .object_fit(ObjectFit::Fill),
                    );
                }
            }
            let (sx, sy) = (lay.width / pw, lay.heights[page] / ph);
            let overlay = |r: &[f32; 4], color: gpui::Hsla| {
                div()
                    .absolute()
                    .left(px(r[0] * sx))
                    .top(px(r[1] * sy))
                    .w(px((r[2] - r[0]) * sx))
                    .h(px((r[3] - r[1]) * sy))
                    .rounded_sm()
                    .bg(color)
            };
            for h in self.notes.on_page(page as u32) {
                let mut color: gpui::Hsla = rgb(h.tint.hex()).into();
                color.a = if self.active.as_deref() == Some(h.id.as_str()) {
                    0.55
                } else {
                    0.38
                };
                for r in &h.rects {
                    paper = paper.child(overlay(r, color));
                }
            }
            if let Some(sel) = self.sel.filter(|s| s.page == page as u32) {
                if let Some(text) = self.texts.get(&sel.page) {
                    let mut color: gpui::Hsla = rgb(0x0A84FF).into();
                    color.a = 0.35;
                    for r in text.rects(sel.a, sel.b) {
                        paper = paper.child(overlay(&r, color));
                    }
                }
            }
            canvas = canvas.child(paper);
        }
        self.layout = Some(lay);

        div()
            .id("pages")
            .size_full()
            .bg(rgb(tone.surround()))
            .overflow_scroll()
            .track_scroll(&self.scroll)
            .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev, window, cx| this.on_down(ev, window, cx)),
            )
            .on_mouse_move(cx.listener(|this, ev, _, cx| this.on_move(ev, cx)))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, ev, window, cx| this.on_up(ev, window, cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, ev, window, cx| this.on_up(ev, window, cx)),
            )
            .child(canvas)
    }

    fn scroll_to_doc_y_with(&self, lay: &Layout, y: f32) {
        let x = self.scroll.offset().x;
        self.scroll
            .set_offset(point(x, px(-y.clamp(0.0, lay.total))));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asynchronous_text_repair_does_not_reset_the_visible_paragraph() {
        let list = ListState::new(20, ListAlignment::Top, px(400.0));
        list.scroll_to(ListOffset { item_ix: 8, offset_in_item: px(37.0) });
        splice_flow_page(&list, 5, 7, 7);
        assert_eq!(list.logical_scroll_top().item_ix, 8);
        assert_eq!(list.logical_scroll_top().offset_in_item, px(37.0));
        splice_flow_page(&list, 0, 1, 4);
        assert_eq!(list.logical_scroll_top().item_ix, 11);
        assert_eq!(list.logical_scroll_top().offset_in_item, px(37.0));
    }

    #[test]
    fn reflow_selection_crosses_paragraphs_in_both_directions() {
        let a = FlowPoint { page: 2, paragraph: 3, byte: 4 };
        let b = FlowPoint { page: 3, paragraph: 1, byte: 6 };
        for selection in [FlowSel { anchor: a, cursor: b }, FlowSel { anchor: b, cursor: a }] {
            assert_eq!(selection.range(2, 2, 20), None);
            assert_eq!(selection.range(2, 3, 20), Some(4..20));
            assert_eq!(selection.range(3, 0, 30), Some(0..30));
            assert_eq!(selection.range(3, 1, 20), Some(0..6));
            assert_eq!(selection.range(3, 2, 20), None);
        }
        let p = FlowPoint { page: 0, paragraph: 0, byte: 2 };
        let q = FlowPoint { byte: 8, ..p };
        let text = "Привет";
        let range = FlowSel { anchor: p, cursor: q }.range(0, 0, text.len()).unwrap();
        assert_eq!(&text[range], "рив");
    }

    #[test]
    fn overlapping_reflow_marks_do_not_duplicate_glyph_runs() {
        let result = disjoint_highlights(vec![(0..12, HighlightStyle::default()),
            (4..8, HighlightStyle::default()), (6..16, HighlightStyle::default())]);
        assert_eq!(result.iter().map(|(r, _)| r.len()).sum::<usize>(), 16);
        for pair in result.windows(2) { assert_eq!(pair[0].0.end, pair[1].0.start); }
    }

    #[test]
    fn syntax_foreground_survives_selection_background() {
        let foreground = rgb(0xE9B47C).into();
        let background = rgb(0x224466).into();
        let runs = disjoint_highlights(vec![(0..10, HighlightStyle::color(foreground)),
            (2..4, HighlightStyle { background_color: Some(background), ..Default::default() })]);
        assert!(runs.iter().all(|(_, style)| style.color == Some(foreground)));
        assert!(runs.iter().any(|(r, style)| *r == (2..4) && style.background_color == Some(background)));
    }

    #[test]
    fn layout_stacks_pages_and_finds_visible_range() {
        let sizes = vec![(100.0, 200.0); 10];
        let lay = Layout::new(&sizes, 100.0 + 2.0 * MARGIN, 1.0);
        assert_eq!(lay.width, 100.0);
        assert_eq!(lay.heights[0], 200.0);
        assert_eq!(lay.tops[1], TOP + 200.0 + GAP);
        assert_eq!(lay.page_at(0.0), 0);
        assert_eq!(lay.page_at(lay.tops[4] + 1.0), 4);
        // A window spanning the gap between pages 2 and 3 sees both.
        let (a, b) = lay.visible(lay.tops[2] + 150.0, lay.tops[3] + 10.0);
        assert_eq!((a, b), (2, 3));
        // Past the end clamps to the last page.
        assert_eq!(lay.page_at(1e9), 9);
    }

    #[test]
    fn zoom_scales_page_width_with_a_floor() {
        let sizes = vec![(100.0, 100.0)];
        assert_eq!(Layout::new(&sizes, 548.0, 2.0).width, 1000.0);
        assert_eq!(Layout::new(&sizes, 10.0, 1.0).width, 80.0);
    }

    #[test]
    fn early_search_or_reflow_waits_for_metadata_and_runs_once() {
        let mut requested = false;
        assert!(begin_bulk(0, 49, &mut requested).is_empty());
        assert!(!requested);
        assert_eq!(begin_bulk(3, 2, &mut requested), vec![2, 0, 1]);
        assert!(requested);
        assert!(begin_bulk(3, 2, &mut requested).is_empty());
    }

    #[test]
    fn background_search_keeps_unicode_excerpt_and_chapter() {
        let index = vec![
            None,
            Some(Arc::<str>::from(
                "Текст: Переменные JavaScript. Переменные ещё раз.",
            )),
        ];
        let toc = vec![TocEntry {
            title: "Глава".into(),
            page: Some(1),
            depth: 0,
        }];
        let hits = scan_hits(&index, &toc, "ПЕРЕМЕННЫЕ");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].page, 1);
        assert_eq!(hits[0].found, "Переменные");
        assert_eq!(hits[0].chapter, "Глава");
        assert!(scan_hits(&index, &toc, "  ").is_empty());
        let many = vec![Some(Arc::<str>::from("match match")); MAX_HITS + 10];
        assert_eq!(scan_hits(&many, &[], "match").len(), MAX_HITS);
    }

    #[test]
    fn page_bitmaps_are_capped() {
        let s = capped_scale(595.0, 842.0, 16.0);
        assert!(595.0 * s * 842.0 * s <= MAX_PAGE_PX * 1.01);
        assert_eq!(capped_scale(595.0, 842.0, 1.0), 1.0);
    }

    #[test]
    fn scale_keys_quantize_to_two_percent() {
        assert_eq!(scale_key(2.0), 100);
        assert_eq!(scale_key(2.004), 100);
        assert_ne!(scale_key(2.0), scale_key(2.04));
    }
}

fn word_range(text: &str, byte: usize) -> std::ops::Range<usize> {
    if text.is_empty() { return 0..0; }
    let at = byte.min(text.len().saturating_sub(1));
    let glyphs: Vec<_> = text.char_indices().collect();
    let i = glyphs.partition_point(|(offset, _)| *offset <= at).saturating_sub(1);
    let class = |c: char| if c.is_alphanumeric() || c == '_' || c == '$' { 0 } else if c.is_whitespace() { 1 } else { 2 };
    let category = class(glyphs[i].1);
    let mut a = i;
    let mut b = i + 1;
    if category != 2 {
        while a > 0 && class(glyphs[a - 1].1) == category { a -= 1; }
        while b < glyphs.len() && class(glyphs[b].1) == category { b += 1; }
    }
    glyphs[a].0..glyphs.get(b).map_or(text.len(), |g| g.0)
}

#[test]
fn double_click_word_boundaries() {
    for (text, byte, expected) in [("привет мир", 4, "привет"), ("0b10101 // hi", 3, "0b10101"), ("const foo_bar = 1", 9, "foo_bar"), ("foo.bar", 3, "."), ("a b", 1, " ")] {
        assert_eq!(&text[word_range(text, byte)], expected);
    }
}
