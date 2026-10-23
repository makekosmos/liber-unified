//! Bounded image ownership and viewport tile geometry, independent of GPUI.
use crate::pdf::{RasterKey, Tile};
use std::collections::{HashMap, HashSet};

// 510px core + a 1px gutter on each side fits a 512px atlas slot.
pub const TILE_PX: u32 = 510;
pub const IMAGE_BUDGET: usize = 64 * 1024 * 1024;
pub const PREVIEW_PIXELS: f32 = 1_000_000.0;

pub struct Entry<T> {
    pub value: T,
    pub bytes: usize,
    pub used: u64,
}

pub struct RasterCache<T> {
    entries: HashMap<RasterKey, Entry<T>>,
    wanted: HashSet<RasterKey>,
    bytes: usize,
    clock: u64,
    budget: usize,
}

impl<T> RasterCache<T> {
    pub fn new(budget: usize) -> Self {
        Self {
            entries: HashMap::new(),
            wanted: HashSet::new(),
            bytes: 0,
            clock: 0,
            budget,
        }
    }
    pub fn contains(&self, key: &RasterKey) -> bool {
        self.entries.contains_key(key)
    }
    pub fn wanted(&self, key: &RasterKey) -> bool {
        self.wanted.contains(key)
    }
    pub fn entries(&self) -> impl Iterator<Item = (&RasterKey, &Entry<T>)> {
        self.entries.iter()
    }
    pub fn bytes(&self) -> usize {
        self.bytes
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn set_wanted(&mut self, keys: &[RasterKey]) {
        self.clock += 1;
        self.wanted = keys.iter().copied().collect();
        for key in keys {
            if let Some(entry) = self.entries.get_mut(key) {
                entry.used = self.clock;
            }
        }
    }

    /// Return ownership of every displaced image so its GPU allocation can be
    /// explicitly released together with its CPU pixels.
    pub fn insert(&mut self, key: RasterKey, value: T, bytes: usize) -> Vec<T> {
        let mut removed = Vec::new();
        if bytes > self.budget {
            removed.push(value);
            return removed;
        }
        if let Some(old) = self.entries.remove(&key) {
            self.bytes -= old.bytes;
            removed.push(old.value);
        }
        while self.bytes + bytes > self.budget {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(key, entry)| {
                    (self.wanted.contains(key), entry.used, key.page, key.scale)
                })
                .map(|(key, _)| *key);
            let Some(oldest) = oldest else { break };
            let old = self.entries.remove(&oldest).unwrap();
            self.bytes -= old.bytes;
            removed.push(old.value);
        }
        self.clock += 1;
        self.entries.insert(
            key,
            Entry {
                value,
                bytes,
                used: self.clock,
            },
        );
        self.bytes += bytes;
        removed
    }

    pub fn clear(&mut self) -> Vec<T> {
        self.bytes = 0;
        self.wanted.clear();
        self.entries.drain().map(|(_, entry)| entry.value).collect()
    }
}

/// Device-space tiles intersecting a clipped page-local viewport. All origins
/// lie on a stable grid, so scrolling reuses tiles instead of re-rasterizing.
pub fn tiles(page_width: f32, page_height: f32, factor: f32, rect: [f32; 4]) -> Vec<Tile> {
    let width = (page_width * factor).ceil().max(1.0) as u32;
    let height = (page_height * factor).ceil().max(1.0) as u32;
    let x0 = (rect[0] * factor).floor().max(0.0).min(width as f32) as u32;
    let y0 = (rect[1] * factor).floor().max(0.0).min(height as f32) as u32;
    let x1 = (rect[2] * factor).ceil().max(0.0).min(width as f32) as u32;
    let y1 = (rect[3] * factor).ceil().max(0.0).min(height as f32) as u32;
    if x0 >= x1 || y0 >= y1 {
        return Vec::new();
    }
    let mut result = Vec::new();
    for y in (y0 / TILE_PX * TILE_PX..y1).step_by(TILE_PX as usize) {
        for x in (x0 / TILE_PX * TILE_PX..x1).step_by(TILE_PX as usize) {
            let left = x.saturating_sub(1);
            let top = y.saturating_sub(1);
            result.push(Tile {
                x: left,
                y: top,
                width: (x + TILE_PX + 1).min(width) - left,
                height: (y + TILE_PX + 1).min(height) - top,
            });
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdf::Tone;
    fn key(page: u32) -> RasterKey {
        RasterKey {
            page,
            scale: 50,
            tone: Tone::Day,
            tile: None,
        }
    }
    #[test]
    fn eviction_returns_cpu_and_gpu_owners_and_honors_bytes() {
        let mut cache = RasterCache::new(100);
        assert!(cache.insert(key(1), "one", 60).is_empty());
        assert_eq!(cache.insert(key(2), "two", 60), vec!["one"]);
        assert_eq!(cache.bytes(), 60);
        assert_eq!(cache.insert(key(2), "replacement", 50), vec!["two"]);
        assert_eq!(cache.clear(), vec!["replacement"]);
        assert_eq!(cache.bytes(), 0);
    }
    #[test]
    fn viewport_images_survive_before_cold_images() {
        let mut cache = RasterCache::new(100);
        cache.insert(key(1), 1, 30);
        cache.insert(key(2), 2, 30);
        cache.set_wanted(&[key(1), key(3)]);
        assert_eq!(cache.insert(key(3), 3, 60), vec![2]);
        assert!(cache.contains(&key(1)));
        assert!(cache.bytes() <= 100);
    }
    #[test]
    fn tile_gutters_overlap_without_wasting_atlas_slots() {
        let result = tiles(600.0, 900.0, 4.0, [0.0, 0.0, 600.0, 900.0]);
        assert!(result
            .iter()
            .all(|tile| tile.width <= 512 && tile.height <= 512));
        let a = result[0];
        let b = result[1];
        assert_eq!(a.x + a.width - b.x, 2);
        assert_eq!(a.y, b.y);
    }

    #[test]
    fn tile_grid_covers_fractional_viewport_without_whole_page_allocation() {
        let result = tiles(600.0, 900.0, 8.0, [123.5, 345.25, 300.0, 440.0]);
        assert!(result.len() <= 9);
        assert!(result
            .iter()
            .all(|t| t.width <= TILE_PX + 2 && t.height <= TILE_PX + 2));
        for y in [2762, 3000, 3519] {
            for x in [988, 1600, 2399] {
                assert!(result
                    .iter()
                    .any(|t| x >= t.x && x < t.x + t.width && y >= t.y && y < t.y + t.height));
            }
        }
        assert!(tiles(600.0, 900.0, 2.0, [-10.0, -10.0, -1.0, -1.0]).is_empty());
    }
}
