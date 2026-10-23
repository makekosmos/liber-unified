# Zotero vs Liber — headless comparison

Machine: Apple M1, macOS 27.0. Zotero 7 (Firefox ESR 140.17), Liber 0.1.0 (release).
Test book: Flanagan, *JavaScript: The Definitive Guide* — 732 pages, 450 TOC entries.
Liber figures: `LIBER_TEST_PDF=<book> cargo test --release bench_real_book -- --nocapture`.
Zotero figures: fresh isolated profile (`-profile … -no-remote -headless`), RSS summed over the process tree, sampled with `ps`.

| Metric | Zotero | Liber |
|---|---|---|
| App size on disk | 399 MB | 29 MB |
| Idle RSS, empty library | 286–358 MB (2 processes) | 99 MB (1 process) |
| Idle CPU, empty library | 13–17 % shortly after start, 0 % once settled | 0.6 % |
| Import of the 732-page PDF (read + hash + metadata) | not measurable headless | 18 ms probe |
| Cover thumbnail | not measurable headless | 48 ms |
| Open book (732 pages, TOC) | not measurable headless | 23–73 ms |
| First page at 2× | not measurable headless | 21 ms |
| Page render at 2×, average of 60 pages | not measurable headless | 101 ms (inverted/dark tone) |
| RSS after rendering 60 pages | — | 302 MB (MuPDF cache, capped) |
| Text reflow of all 732 pages | no equivalent | 2.5 s, RSS 400 MB |

## Zotero with its GUI (isolated profile, book imported via connector API)

| Metric | Zotero |
|---|---|
| Launch to connector ready | about 7 s |
| Peak RSS while importing and opening the book | 1293 MB |
| RSS once settled, book open | about 680 MB |
| Time until CPU settles after open | about 25 s (indexing and metadata recognition) |

Liber after the memory fix: 191 MB after rendering, 199 MB after reflowing and indexing the whole book.

## Both apps driven through the GUI (computer-use agent, isolated copies)

Full method, raw samples and caveats: `benchmark-gui-results.md`. Timings are automation-cycle
durations (action + accessibility/screenshot observation), not frame latencies.

| Metric | Zotero 10.0.6 | Liber 0.1.0 |
|---|---:|---:|
| RSS, empty library | 215 MiB | 93 MiB |
| RSS, book open, settled | 843 MiB | 178 MiB |
| CPU, book open, idle | 2.20% | 1.23% |
| RSS peak, 10 s scroll burst | 1994 MiB | 691 MiB |
| Open book (cycle) | 1.82 s | 0.61 s |
| Search "function" (cycle) | 1.99 s | 0.60 s |
| Jump to page 400 / 732 (cycle) | 0.48 s | 0.42 s |
| First open to readable cover | about 6.9 s (blank at 1.1 s) | within 0.7 s |
| RSS peak, zoom to about 300% | not measured | **2733 MiB (before fix)** |

Subjective stutter and launch-to-window time were not measured reliably (the screen recording
only captured the agent UI). Scroll workloads differed between apps, so they are not a throughput comparison.

## Limits of this comparison

- Zotero has no headless PDF-open or render entry point; its reader runs inside the app UI,
  so open/render latency was not measured for it. Only process-level numbers are comparable.
- Zotero's idle RSS is for an empty profile; a user instance with a real library measured 377 MB.
- Liber's page-render time is a CPU bitmap render on the worker thread; display latency adds
  at most one 16 ms poll tick.
- Single run each, no warm/cold cache separation: treat as order-of-magnitude.

## Finding fixed during benchmarking

Reflowing the whole book originally pushed Liber to 2.1 GB RSS (MuPDF caches kept growing).
The worker now reopens the document every 12 renders / 25 bulk pages and returns freed memory to the OS:
RSS after the same run is about 200 MB.

The GUI run also exposed a zoom spike: at 300-400% on a retina display each visible page was rasterized
at up to about 100 Mpx (400 MB), pushing Liber to 2.7 GB. Page bitmaps are now capped at 12 Mpx
(`MAX_PAGE_PX` in `src/reader.rs`); beyond that the bitmap is upscaled on screen, slightly softer
but bounded at about 48 MB per page. Re-measure the zoom sequence with the installed build to confirm.
