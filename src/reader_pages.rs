//! The open book's rendered pages: renders the pages around the visible ones and drops the rest.

use crate::reader_window;
use liber_core::{Page, PageTone};
use nsh_core::ui::prelude::*;
use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub type Render = fn(&str, usize, f32, PageTone, &Arc<AtomicBool>) -> Result<Page, String>;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Demand {
    Visible,
    Prefetch,
}

#[derive(Clone)]
pub struct ReaderPages {
    pub rendered: Signal<Vec<Signal<Option<Page>>>>,
    pub pending: Signal<Vec<usize>>,
    pub path: Signal<String>,
    pub count: Signal<usize>,
    pub zoom: Signal<f32>,
    pub tone: Signal<PageTone>,
    /// Render jobs still queued or running, each with the flag that cancels it once its page leaves the window.
    pub(crate) in_flight: Rc<
        RefCell<
            Vec<(
                usize,
                Rc<Cell<Demand>>,
                Rc<Cell<Option<std::time::Instant>>>,
                Arc<AtomicBool>,
            )>,
        >,
    >,
    pub(crate) last_visible: Rc<Cell<Option<(usize, usize)>>>,
    pub(crate) last_visible_change: Rc<Cell<std::time::Instant>>,
    pub render_preview: Render,
    pub render: Render,
    /// Owns the render tasks: the scope that asks for a page, such as the list's viewability listener, is rebuilt while they run.
    pub owner: Owner,
}

impl ReaderPages {
    pub fn request(&self, at: usize) {
        self.request_at(at, Demand::Visible);
    }

    fn request_at(&self, at: usize, demand: Demand) {
        let pending = self.pending;
        if at >= self.count.get() {
            return;
        }
        let mut pending_list = pending.get();
        if pending_list.contains(&at) {
            let active = self
                .in_flight
                .borrow()
                .iter()
                .find(|(index, _, _, _)| *index == at)
                .map(|(_, queued, visible_since, _)| (queued.clone(), visible_since.clone()));
            if let Some((queued, visible_since)) = active {
                if demand == Demand::Visible && queued.get() == Demand::Prefetch {
                    queued.set(Demand::Visible);
                    visible_since.set(Some(std::time::Instant::now()));
                    #[cfg(test)]
                    if self
                        .rendered
                        .get()
                        .get(at)
                        .and_then(|slot| slot.get())
                        .is_some_and(|page| !page.complete)
                    {
                        record_preview_latency(at, std::time::Duration::ZERO);
                    }
                }
                return;
            }
            pending_list.retain(|index| *index != at);
            pending.set(pending_list);
        }
        let requested_zoom = self.zoom.get();
        let requested_tone = self.tone.get();
        if self
            .rendered
            .get()
            .get(at)
            .and_then(|slot| slot.get())
            .as_ref()
            .is_some_and(|cached| {
                cached.complete
                    && (cached.zoom - requested_zoom).abs() < 0.001
                    && cached.tone == requested_tone
            })
        {
            return;
        }
        pending_list = pending.get();
        pending_list.push(at);
        pending.set(pending_list);
        for (index, _, _, previous) in self.in_flight.borrow().iter() {
            if *index == at {
                previous.store(true, Ordering::Relaxed);
            }
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let queued = Rc::new(Cell::new(demand));
        let visible_since = Rc::new(Cell::new(
            (demand == Demand::Visible).then(std::time::Instant::now),
        ));
        self.in_flight.borrow_mut().push((
            at,
            queued.clone(),
            visible_since.clone(),
            cancel.clone(),
        ));
        let source = self.path.get();
        let (pages, count, current_path, current_zoom, current_tone) =
            (self.rendered, self.count, self.path, self.zoom, self.tone);
        let in_flight = self.in_flight.clone();
        let request_demand = queued;
        let last_visible = self.last_visible.clone();
        let last_visible_change = self.last_visible_change.clone();
        #[cfg(test)]
        let preview_visible_since = visible_since;
        let render_preview = self.render_preview;
        let render = self.render;
        #[cfg(test)]
        let request_started = std::time::Instant::now();
        self.owner.run(|| {
            spawn(async move {
                let cached_preview = pages
                    .get()
                    .get(at)
                    .and_then(|slot| slot.get())
                    .as_ref()
                    .is_some_and(|page| {
                        (page.zoom - requested_zoom).abs() < 0.001
                            && page.tone == requested_tone
                            && Path::new(&page.png).exists()
                    });
                if !cached_preview {
                    let page_path = source.clone();
                    let job_cancel = cancel.clone();
                    let preview = blocking(move || {
                        render_preview(&page_path, at, requested_zoom, requested_tone, &job_cancel)
                    })
                    .await;
                    #[cfg(test)]
                    if request_demand.get() == Demand::Visible
                        && preview.as_ref().is_ok_and(|result| result.is_ok())
                    {
                        record_preview_latency(
                            at,
                            preview_visible_since
                                .get()
                                .unwrap_or(request_started)
                                .elapsed(),
                        );
                    }
                    let expected_zoom = current_zoom.get();
                    let current = current_path.get() == source
                        && (expected_zoom - requested_zoom).abs() <= 0.001
                        && current_tone.get() == requested_tone;
                    if !current {
                        cancel.store(true, Ordering::Relaxed);
                    }
                    if current && !cancel.load(Ordering::Relaxed) {
                        if let Ok(Ok(preview)) = preview {
                            count.set(preview.count);
                            let slots = pages.get();
                            if let Some(slot) = slots.get(at) {
                                slot.set(Some(preview));
                            }
                        }
                    }
                } else {
                    #[cfg(test)]
                    if request_demand.get() == Demand::Visible {
                        record_preview_latency(
                            at,
                            preview_visible_since
                                .get()
                                .unwrap_or(request_started)
                                .elapsed(),
                        );
                    }
                }
                if cancel.load(Ordering::Relaxed) {
                    in_flight
                        .borrow_mut()
                        .retain(|(_, _, _, flag)| !Arc::ptr_eq(flag, &cancel));
                    return;
                }
                if request_demand.get() == Demand::Prefetch {
                    in_flight
                        .borrow_mut()
                        .retain(|(_, _, _, flag)| !Arc::ptr_eq(flag, &cancel));
                    let mut pending_list = pending.get();
                    pending_list.retain(|index| *index != at);
                    pending.set(pending_list);
                    return;
                }
                loop {
                    if cancel.load(Ordering::Relaxed)
                        || !last_visible
                            .get()
                            .is_some_and(|(first, last)| (first..=last).contains(&at))
                    {
                        in_flight
                            .borrow_mut()
                            .retain(|(_, _, _, flag)| !Arc::ptr_eq(flag, &cancel));
                        let mut pending_list = pending.get();
                        pending_list.retain(|index| *index != at);
                        pending.set(pending_list);
                        return;
                    }
                    let delay = std::time::Duration::from_millis(300)
                        .saturating_sub(last_visible_change.get().elapsed());
                    if delay.is_zero() {
                        break;
                    }
                    let _ = blocking(move || std::thread::sleep(delay)).await;
                }
                let page_path = source.clone();
                let job_cancel = cancel.clone();
                let result = blocking(move || {
                    render(&page_path, at, requested_zoom, requested_tone, &job_cancel)
                })
                .await;
                in_flight
                    .borrow_mut()
                    .retain(|(_, _, _, flag)| !Arc::ptr_eq(flag, &cancel));
                // A cancelled job already left `pending`, and the page may have been requested again since.
                if !cancel.load(Ordering::Relaxed) {
                    let mut pending_list = pending.get();
                    pending_list.retain(|index| *index != at);
                    pending.set(pending_list);
                }
                let expected_zoom = current_zoom.get();
                if current_path.get() != source
                    || (expected_zoom - requested_zoom).abs() > 0.001
                    || current_tone.get() != requested_tone
                {
                    return;
                }
                if let Ok(Ok(rendered)) = result {
                    count.set(rendered.count);
                    let slots = pages.get();
                    if let Some(slot) = slots.get(at) {
                        slot.set(Some(rendered));
                    }
                }
            })
        });
    }

    /// Pages `first..=last` are on screen: cancels and drops pages outside the window and requests the missing ones.
    pub fn show(&self, first: usize, last: usize) {
        let window = reader_window::window(first, last, self.count.get());
        let visible = (first, last);
        let previous = self.last_visible.replace(Some(visible));
        if previous != Some(visible) {
            self.last_visible_change.set(std::time::Instant::now());
        }
        let previous_anchor = previous.map(|(old_first, old_last)| (old_first + old_last) / 2);
        let current_anchor = (first + last) / 2;
        let forward = previous_anchor.is_none_or(|anchor| current_anchor >= anchor);
        self.in_flight.borrow_mut().retain(|(index, _, _, cancel)| {
            let keep = window.contains(index);
            if !keep {
                cancel.store(true, Ordering::Relaxed);
            }
            keep
        });
        let pending = self.pending;
        pending.set(
            pending
                .get()
                .into_iter()
                .filter(|index| window.contains(index))
                .collect(),
        );
        let slots = self.rendered.get();
        let present: Vec<bool> = slots.iter().map(|slot| slot.get().is_some()).collect();
        let complete: Vec<bool> = slots
            .iter()
            .map(|slot| slot.get().is_some_and(|page| page.complete))
            .collect();
        let evicted = reader_window::evict(&present, &window);
        if !evicted.is_empty() {
            for index in evicted {
                if let Some(slot) = slots.get(index) {
                    slot.set(None);
                }
            }
        }
        let pending = pending.get();
        for index in first..=last {
            if !complete.get(index).copied().unwrap_or(false) && !pending.contains(&index) {
                self.request(index);
            }
        }
        let prefetch = if forward {
            [last.saturating_add(1), first.saturating_sub(1)]
        } else {
            [first.saturating_sub(1), last.saturating_add(1)]
        };
        for index in prefetch {
            if window.contains(&index)
                && !present.get(index).copied().unwrap_or(false)
                && !pending.contains(&index)
            {
                self.request_at(index, Demand::Prefetch);
            }
        }
    }
}

#[cfg(test)]
static PREVIEW_LATENCIES: std::sync::Mutex<Vec<(usize, std::time::Duration)>> =
    std::sync::Mutex::new(Vec::new());

#[cfg(test)]
fn record_preview_latency(page: usize, latency: std::time::Duration) {
    if let Ok(mut samples) = PREVIEW_LATENCIES.lock() {
        samples.push((page, latency));
    }
}

#[cfg(test)]
fn take_preview_latencies() -> Vec<(usize, std::time::Duration)> {
    PREVIEW_LATENCIES
        .lock()
        .map(|mut samples| std::mem::take(&mut *samples))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use nsh_core::ui::Root;
    use nsh_engine::EngineConfig;
    use nsh_testing::Headless;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    const PAGES: usize = 40;
    const PAGE_HEIGHT: f32 = 980.0;
    static LIST_BUILDS: AtomicUsize = AtomicUsize::new(0);

    fn quick_render(
        _: &str,
        _: usize,
        zoom: f32,
        tone: PageTone,
        cancel: &Arc<AtomicBool>,
    ) -> Result<Page, String> {
        std::thread::sleep(Duration::from_millis(700));
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        Ok(Page {
            count: PAGES,
            png: String::new(),
            text: String::new(),
            text_runs: Vec::new(),
            page_text: liber_core::notes::PageText::default(),
            toc: Vec::new(),
            size: (680, 980),
            points: (680.0, 980.0),
            zoom,
            tiles: Vec::new(),
            tone,
            complete: true,
        })
    }

    fn quick_preview(
        _: &str,
        _: usize,
        zoom: f32,
        tone: PageTone,
        cancel: &Arc<AtomicBool>,
    ) -> Result<Page, String> {
        std::thread::sleep(Duration::from_millis(25));
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        Ok(Page {
            count: PAGES,
            png: String::new(),
            text: String::new(),
            text_runs: Vec::new(),
            page_text: liber_core::notes::PageText::default(),
            toc: Vec::new(),
            size: (680, 980),
            points: (680.0, 980.0),
            zoom,
            tiles: Vec::new(),
            tone,
            complete: false,
        })
    }

    fn real_render(
        path: &str,
        at: usize,
        zoom: f32,
        tone: PageTone,
        cancel: &Arc<AtomicBool>,
    ) -> Result<Page, String> {
        liber_core::render_cancellable(path, at, zoom, tone, cancel)
    }

    fn real_preview_render(
        path: &str,
        at: usize,
        zoom: f32,
        tone: PageTone,
        cancel: &Arc<AtomicBool>,
    ) -> Result<Page, String> {
        liber_core::render_preview_cancellable(path, at, zoom, tone, cancel)
    }

    /// Page slots update independently so scrolling does not rebuild the list.
    fn reader(
        path: String,
        count: usize,
        render: Render,
        render_preview: Render,
    ) -> impl IntoElement {
        let shown = signal((0usize, 0usize));
        let pages = ReaderPages {
            rendered: signal((0..count).map(|_| signal(None)).collect()),
            pending: signal(Vec::new()),
            path: signal(path),
            count: signal(count),
            zoom: signal(1.0),
            tone: signal(PageTone::Day),
            in_flight: Default::default(),
            last_visible: Default::default(),
            last_visible_change: Rc::new(Cell::new(std::time::Instant::now())),
            render_preview,
            render,
            owner: Owner::current(),
        };
        let (rendered, pending) = (pages.rendered, pages.pending);
        let missing_previews = move || {
            let (first, last) = shown.get();
            let cache = rendered.get();
            reader_window::window(first, last, count)
                .filter(|index| cache.get(*index).is_none_or(|slot| slot.get().is_none()))
                .collect::<Vec<_>>()
        };
        let missing_details = move || {
            let (first, last) = shown.get();
            let cache = rendered.get();
            (first..=last)
                .filter(|index| {
                    !cache
                        .get(*index)
                        .and_then(|slot| slot.get())
                        .is_some_and(|page| page.complete)
                })
                .collect::<Vec<_>>()
        };
        view()
            .flex_1()
            .child(text(move || {
                format!("preview missing {:?}", missing_previews())
            }))
            .child(text(move || {
                format!("detail missing {:?}", missing_details())
            }))
            .child(text(move || format!("pending {:?}", pending.get())))
            .child(dynamic(move || {
                LIST_BUILDS.fetch_add(1, Ordering::Relaxed);
                let pages = pages.clone();
                virtual_list(
                    move || count,
                    move |position| {
                        let position = position;
                        dynamic(move || {
                            let index = position.index();
                            let label = if rendered
                                .get()
                                .get(index)
                                .is_some_and(|slot| slot.get().is_some())
                            {
                                format!("page {index}")
                            } else {
                                format!("loading {index}")
                            };
                            view().h(px(PAGE_HEIGHT)).child(text(label)).into_element()
                        })
                    },
                )
                .estimated_item_size(px(PAGE_HEIGHT))
                .initial_scroll_index(0)
                .test_id("pages")
                .on_viewable_items_change(move |items: &ViewableItems| {
                    if let Some((first, last)) = items.viewable {
                        shown.set((first, last));
                        pages.show(first, last);
                    }
                })
                .flex_1()
                .into_element()
            }))
    }

    #[component]
    fn QuickReader() -> impl IntoElement {
        reader(String::new(), PAGES, quick_render, quick_preview)
    }

    #[component]
    fn PdfReader() -> impl IntoElement {
        let path = std::env::var("LIBER_TEST_PDF").unwrap_or_default();
        let count = liber_core::render(&path, 0, false)
            .map(|page| page.count)
            .unwrap_or(0);
        reader(path, count, real_render, real_preview_render)
    }

    fn scroll_then_settle(root: Root, steps: usize, bound: Duration) {
        LIST_BUILDS.store(0, Ordering::Relaxed);
        let mut app = Headless::start(EngineConfig::new(root)).unwrap();
        app.first_render().unwrap();
        app.wait_until("first window previews", bound, |screen| {
            screen.shows("preview missing []")
        })
        .unwrap();
        for _ in 0..steps {
            app.scroll("id:pages", px(PAGE_HEIGHT)).unwrap();
        }
        app.wait_until("the visible window previews", bound, |screen| {
            screen.shows("preview missing []")
        })
        .unwrap();
        let settled = app.wait_until("the visible window details", bound, |screen| {
            screen.shows("detail missing []")
        });
        let builds = LIST_BUILDS.load(Ordering::Relaxed);
        assert!(
            settled.is_ok(),
            "{} (page list built {builds} times)",
            settled.unwrap_err()
        );
        assert_eq!(
            builds, 1,
            "page list rebuilt during scroll ({builds} builds)"
        );
        assert!(
            app.screen().shows("page 12"),
            "recycled virtual-list cell did not display page 12"
        );
    }

    #[test]
    fn scrolled_window_renders() {
        scroll_then_settle(Root::of::<QuickReader>(), 12, Duration::from_secs(5));
    }

    #[test]
    fn preview_is_visible_before_detail_render_finishes() {
        let mut app = Headless::start(EngineConfig::new(Root::of::<QuickReader>())).unwrap();
        let started = std::time::Instant::now();
        app.first_render().unwrap();
        app.wait_until(
            "preview appears before detail",
            Duration::from_secs(2),
            |screen| screen.shows("preview missing []"),
        )
        .unwrap();
        assert!(
            started.elapsed() <= Duration::from_millis(300),
            "preview exceeded 300 ms: {:?}",
            started.elapsed()
        );
        assert!(
            !app.screen().shows("detail missing []"),
            "detail finished before preview was observed"
        );
        app.wait_until(
            "detail eventually replaces preview",
            Duration::from_secs(5),
            |screen| screen.shows("detail missing []"),
        )
        .unwrap();
    }

    #[test]
    #[ignore = "needs LIBER_TEST_PDF"]
    fn scrolled_window_renders_from_pdf() {
        scroll_then_settle(Root::of::<PdfReader>(), 24, Duration::from_secs(30));
        let mut samples = take_preview_latencies();
        samples.sort_unstable_by_key(|(_, latency)| *latency);
        let p50 = samples
            .get(samples.len() / 2)
            .map(|(_, latency)| *latency)
            .unwrap_or_default();
        let p95 = samples
            .get(
                samples
                    .len()
                    .saturating_mul(95)
                    .div_ceil(100)
                    .saturating_sub(1),
            )
            .map(|(_, latency)| *latency)
            .unwrap_or_default();
        let max = samples
            .last()
            .map(|(_, latency)| *latency)
            .unwrap_or_default();
        eprintln!(
            "preview request-to-result: samples={samples:?} p50={p50:?} p95={p95:?} max={max:?}"
        );
        assert!(
            max <= Duration::from_millis(300),
            "preview exceeded 300 ms: p50={p50:?}, p95={p95:?}, max={max:?}"
        );
    }
}
