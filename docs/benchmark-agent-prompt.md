# Task: GUI benchmark, Zotero vs Liber (computer use)

Goal: measure how fast and how heavy two PDF readers are when opening the same book in their real UI.
Do not modify the user's real libraries. Use the isolated copies below.

## Setup
- Book: `~/Downloads/JavaScript. Полное руководство справочник по самому популярному языку программирования (Дэвид Флэнаган) (z-library.sk, 1lib.sk, z-lib.sk).pdf` (732 pages, 450 TOC entries).
- Liber: `~/Applications/Liber.app`. Isolated data: `LIBER_DATA_DIR=$(mktemp -d)` and launch
  `LIBER_DATA_DIR=... ~/Applications/Liber.app/Contents/MacOS/Liber`. Import the book by drag-and-drop.
- Zotero 7: `/Applications/Zotero.app`. Isolated profile with its own data dir; never touch
  `~/Library/Application Support/Zotero`. Launch with `-profile <tmp> -no-remote`.
  The user's own Zotero may be running: ignore it, don't quit it, and exclude its PIDs from numbers.

## Measure (3 runs each, report median; cold = just after app launch, warm = second open)
1. Launch to usable window (stopwatch from launch command to the window accepting input).
2. Import the book into the library (drag-and-drop), time until it appears with a cover/title.
3. Double-click to open: time until page 1 is visible and readable.
4. Jump to page 400 via the contents/outline: time until the page is sharp.
5. Hold arrow-down/scroll for 10 s through the book: note stutter (subjective 1-5) and CPU peak.
6. Zoom to 300%: time until the page is sharp.
7. Text search for the word "function" (Liber: Cmd+F or the magnifier in the top bar; Zotero: Cmd+F in the reader): time to first result.
8. Memory: RSS (sum over the app's whole process tree, `ps -axo pid,ppid,rss,pcpu,comm`) at: idle empty library, book open idle (30 s after open), after scrolling the whole book, peak.
9. CPU: average % over 30 s idle with the book open.

Use screen recording or a frame-by-frame screenshot loop for timings (>= 30 fps), not eyeballing.

## Report
A markdown table (metric | Zotero | Liber), then notes on anything unfair (Zotero indexing/recognition running
in the background, first-run effects). Write it to `liber-gpui/docs/benchmark-gui-results.md`.
Already known (headless, no need to repeat): Liber library page open 23-73 ms; Zotero RSS peaks 1.3 GB while importing/opening this book.
