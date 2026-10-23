# Liber: performance and reader panes, 2026-10-14

Installed release: `/Users/user/Applications/Liber.app`. Both installed and bundled executables were replaced, ad-hoc signed and verified with `codesign --verify --deep --strict`. SHA-256 values are in `installed-binaries.json`; signing changes the executable hash from the unsigned build.

## Changes

- PDF pages use a small whole-page preview and visible device-pixel tiles (510px core plus 1px gutters). A byte-budget LRU limits retained CPU raster images to 64MiB; displaced and replaced images explicitly release their GPUI GPU atlas resources. This budget does not cap the entire process, GPU backing allocations or PDF parser memory.
- Viewport work replaces stale zoom/scroll work. Bounded result channels prevent decoded bitmaps accumulating behind the UI. Worker notifications replace a 16ms polling loop. Selection text is retained only near the viewport. Closing the reader releases its images.
- MuPDF display lists are limited to two pages. Shared resources are trimmed after inactivity. Documents are reopened after 16 distinct processed pages to bound document-owned parser/xref caches that store trimming alone cannot release. The small safe store-trim API addition is documented in `vendor/mupdf/LIBER-PATCH.md`.
- Reflow emits pages incrementally, persists text/metadata/index caches on disk, and uses a second incremental vocabulary-repair pass. Search scans shared text on a background executor with a coalescing delay. Visible content has priority; search and reflow share background work fairly. Early search/reflow waits for metadata before scheduling a whole-book pass.
- Reader sidebars mirror Agenda quick entry: 340pt default, POPOVER surface, divider, mirrored shadow, fixed inner shell clipped by a 200ms interruptible ease-out width animation. Left contains the outline, right contains notes; both resize and persist width. Detailed raster requests pause while geometry animates, retaining previews until final sharp tiles arrive.
- Native traffic lights retain the shared titlebar geometry. Redundant native position updates are avoided during scrolling/animations; fullscreen is exempt from manual alignment.

## Validation

Final source: 41 tests passed, 1 network test ignored. Environment-gated real-book tests in the default suite return early without a PDF; these are not counted as real-book runs. A separate final-source run of `real_book_opens_renders_and_has_text` with the actual 732-page PDF passed (metadata, rendering, text and links).

Tests cover clipped raster pixels in three tones, tile coverage/gutters, byte-budget eviction and replacement ownership, stale viewport replacement/shutdown, persistent text identity/repair, background search, metadata startup races and interruption of sidebar animation.

GUI checks on the installed final binary: immediate search during book opening produces results; both panes open/close; right pane resize and restore work; PDF scroll, zoom, reset and text-mode roundtrip work. Visible tiles were inspected after settling. Frame-time/FPS and GPU utilization were not instrumented. Fullscreen transitions were not validated in this run.

## Measurements and limits

Final installed PID 9579, macOS 27, Retina window 2560x1540 physical pixels. The session included search, both sidebars, resizing, reflow, scrolling and zoom. Caches were already populated. Five `ps` samples spaced approximately 2 seconds apart and a subsequent `vmmap -summary` were saved for each mode.

| Mode | Physical footprint | RSS sample range | CPU time delta over sample interval |
|---|---:|---:|---:|
| Reflow, both panes open | 192.6MiB | 251.0–251.1MiB | 0.06s / 8.05s, about 0.75% of one core |
| PDF, both panes open, after scroll/zoom/reset | 215.2MiB | 273.8–295.2MiB | 0.10s / 8.05s, about 1.24% of one core |

Observed process lifetime peak physical footprint: 315.3MiB. The final PDF snapshot attributes 50.7MiB resident memory to IOAccelerator graphics and 46.2MiB to IOSurface. RSS and physical footprint are different OS accounting metrics and should not be interchanged. CPU time is rounded to 0.01s, so small idle deltas have limited precision. These short samples establish observed idle behavior, not a sustained-load benchmark.

Five separate cold-cache worker runs on the 732-page, approximately 53MB book completed incremental extraction of 2,828,386 raw text bytes. Sampled peak RSS: 100.6, 139.3, 135.2, 100.1 and 98.2MiB; median 100.6MiB. This uses the optimized test profile, excludes GUI/GPU memory and samples RSS every 16 received pages, so it is not an exact peak. Some runs overlapped compilation; elapsed timings are not used to claim throughput improvements.

The original already-used process snapshot was approximately 1.1GiB, including roughly 600MiB graphics and 352MiB heap. It was not a matched cold/warm baseline with identical interactions. No precise before/after speedup, universal memory limit, maximum FPS or minimum CPU/GPU claim is made. `before-*` and `interaction-process.json` are diagnostic/intermediate evidence; `final-*` identify the installed final build measurements. `cold-worker-memory.json` records the five cold extraction checks before the final scheduling-only changes.

Source and original installed app were preserved under `target/perf-before/` for rollback/comparison.
