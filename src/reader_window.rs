//! Which pages the reader keeps rendered for the visible range.

use std::ops::RangeInclusive;

/// The visible pages plus one neighbor on each side, clamped to the document.
pub fn window(first: usize, last: usize, count: usize) -> RangeInclusive<usize> {
    first.saturating_sub(1)..=last.saturating_add(1).min(count.saturating_sub(1))
}

/// Cached pages outside the window, to drop.
pub fn evict(cached: &[bool], window: &RangeInclusive<usize>) -> Vec<usize> {
    cached
        .iter()
        .enumerate()
        .filter(|(index, rendered)| **rendered && !window.contains(index))
        .map(|(index, _)| index)
        .collect()
}

/// Pages in the window that are neither rendered nor already queued.
pub fn to_request(window: &RangeInclusive<usize>, cached: &[bool], pending: &[usize]) -> Vec<usize> {
    window
        .clone()
        .filter(|index| !cached.get(*index).copied().unwrap_or(false) && !pending.contains(index))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rendered(count: usize, pages: impl IntoIterator<Item = usize>) -> Vec<bool> {
        let mut cached = vec![false; count];
        for page in pages {
            cached[page] = true;
        }
        cached
    }

    #[test]
    fn window_is_clamped_to_document() {
        assert_eq!(window(0, 0, 5), 0..=1);
        assert_eq!(window(3, 4, 5), 2..=4);
    }

    #[test]
    fn settled_window_requests_nothing() {
        let count = 30;
        let view = window(5, 7, count);
        let cached = rendered(count, view.clone());
        assert!(to_request(&view, &cached, &[]).is_empty());
        assert!(evict(&cached, &view).is_empty());
    }

    #[test]
    fn repeated_viewable_callback_does_not_request_again() {
        let count = 30;
        let view = window(5, 7, count);
        let mut cached = rendered(count, []);
        let mut requests = 0;
        for _ in 0..5 {
            let wanted = to_request(&view, &cached, &[]);
            requests += wanted.len();
            for page in wanted {
                cached[page] = true;
            }
        }
        assert_eq!(requests, view.clone().count());
    }

    #[test]
    fn pending_pages_are_not_requested_twice() {
        let view = window(5, 5, 30);
        let wanted = to_request(&view, &rendered(30, []), &[4, 5]);
        assert_eq!(wanted, vec![6]);
    }

    #[test]
    fn scrolling_renders_each_page_once_while_it_stays_in_window() {
        let count = 40;
        let mut cached = rendered(count, []);
        let mut renders = vec![0; count];
        for first in 0..count {
            let view = window(first, first, count);
            for page in evict(&cached, &view) {
                cached[page] = false;
            }
            for page in to_request(&view, &cached, &[]) {
                renders[page] += 1;
                cached[page] = true;
            }
        }
        assert!(renders.iter().all(|&n| n == 1), "renders per page: {renders:?}");
    }



}
