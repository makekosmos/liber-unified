# Zotero vs Liber — GUI measurements

Date: 2026-10-14 (Europe/Moscow). Same Flanagan PDF, 732 pages, approximately 56 MB. Installed applications: **Zotero 10.0.6**, **Liber 0.1.0**. Zotero 7 is not installed at the requested path.

The user authorized a best-effort Computer benchmark after the initial targeting blocker. The following are measured numbers, with the method and limits distinguished explicitly. Liber used less memory at settled idle, but its observed zoom-related RSS peak exceeded Zotero's observed peak. These runs do not establish a precise rendering-speed winner.

| Metric | Zotero | Liber |
|---|---:|---:|
| Empty-library RSS, median of samples from one fresh profile | 215.2 MiB | 93.2 MiB |
| Settled open-book RSS after interactions, median of three idle endpoints | 842.8 MiB | 177.9 MiB |
| Open-book idle CPU, median of three 30 s intervals | 2.20% | 1.23% |
| CPU interval results, one core = 100% | 2.23 / 2.16 / 2.20% | 1.23 / 1.27 / 1.17% |
| RSS peak during one 10 s scroll burst | 1993.7 MiB | 691.0 MiB |
| RSS immediately after that scroll burst | 1717.3 MiB | 680.0 MiB |
| Maximum sampled `ps %cpu` during that scroll burst | 153.7% | 102.6% |
| Highest RSS observed in the isolated session | 1993.7 MiB (scroll) | 2732.6 MiB (zoom sequence) |
| Repeated open: median Computer action + observation cycle, three repetitions | 1.823 s (0.879–1.946) | 0.611 s (0.589–0.676) |
| Repeated search “function”: median Computer cycle, three repetitions | 1.986 s (1.950–1.989), includes Return | 0.600 s (0.594–0.602), live results |
| Page 400: median Computer cycle, three repetitions, page-number field | 0.477 s (0.449–1.253) | Not measured |
| Page 732: median Computer cycle, three repetitions, Home → End | Not measured | 0.420 s (0.414–0.532) |
| Fresh-library GUI import → cover/title confirmed, one import, file picker | Timing lost on UI-change exception; import succeeded | Within 0.985 s including Computer overhead |
| First open → readable cover, one observation sequence | Blank at 1.110 s; readable confirmed by 6.931 s | Readable confirmed within 0.688 s |
| Zoom action → observation cycle, one attempt | 0.555 s, one Zoom In step; 300% not verified | 0.416 s, eighth 15% step after reset (~306%); exact label not verified |
| Launch → usable window | Not timed reliably | Not timed reliably |
| RSS after scrolling every page | Not measured | Not measured |
| Subjective stutter score, 1–5 | Not scored reliably | Not scored reliably |

## What the timing numbers mean

The repeated timing rows measure `Date.now()` immediately before a Computer action through completion of its AX/screenshot observation. They include tool dispatch, the click/key action, accessibility collection and screenshot capture. They are **automation-cycle durations**, not frame-resolved rendering latencies. A readable screenshot gives a conservative upper bound where explicitly stated. Differences in accessibility-tree size and tool overhead can account for some of the gap; do not interpret 1.823 / 0.611 as a renderer speedup.

Liber repeated opens visibly retained the cover. Zotero's first open initially returned a blank reader, so the 1.110 s observation is not reported as successful rendering. Both readers returned actual search results: Liber showed snippets with “function”; Zotero showed 315 matches and a highlighted occurrence. Search is not semantically identical: Liber lists matching pages and snippets; Zotero navigates individual occurrences. The initial Zotero `setValue` observation had no result and was excluded from successful-search timing.

A desktop video was captured at 30 fps (44,661 frames, 1488.7 s). Inspection showed the foreground Codex interface and Computer previews, rather than a continuous full-resolution view of the background reader windows. Therefore it is **not** used to claim 33 ms timing precision or score stutter. Temporary recording: `/tmp/liber-gui-benchmark/gui-session.mp4`.

## Isolation and application identity

Copied installed bundles to `/tmp/liber-gui-benchmark/Liber Benchmark.app` and `Zotero Benchmark.app`, changing only the copied bundles' display names and identifiers so Computer could target each independently. Liber ran with `LIBER_DATA_DIR=/tmp/liber-gui-benchmark/liber-data`. Zotero ran with `-profile /tmp/liber-gui-benchmark/zotero-profile -no-remote`; its profile explicitly selected `/tmp/liber-gui-benchmark/zotero-data`. Both empty libraries were confirmed before import.

The copied Zotero bundle initially exited with code 137 after its metadata changed. Applying an ad-hoc signature to **that temporary copy only** allowed it to run. The original installed bundle was not modified. Liber executable SHA-256 matches the installed executable. Zotero's complete executable hash changed with signing; all arm64 `__TEXT` section hashes match. This is an instrumentation difference from running the untouched installed bundle.

Benchmark roots were Liber PID 91092 and Zotero PID 91172. Every resource sample recursively sums each root's descendants. Existing user instances, PID 88620 and PID 81989 and their children, are excluded. The user's initially visible reader positions were briefly exercised during preflight, then restored to Liber page 83 and Zotero page 91. No real-library items were imported or deleted; neither user app was quit.

## Resource method and fairness

- RSS is summed from `ps -axo pid,ppid,rss,pcpu,comm`, sampled approximately every 0.5 s. MiB = RSS KiB / 1024. Peaks are sampled maxima; a briefer peak may be missed.
- Empty-library medians cover one launch per application, not three launches: 80 Liber samples and 787 Zotero samples. Sample windows differ and include first-run settling.
- Idle CPU uses the change in summed cumulative `ps time` across each actual 30 s interval, divided by wall time. The three intervals lasted approximately 30.04 s each. No reader input or screen recording occurred during these intervals. They are repeated intervals of one session, not independent fresh launches. Liber had one process and Zotero six at this stage.
- The final idle measurement follows search, scrolling, zoom and reset to the cover. It is not the original task's exact “30 s after first open” condition. A separate quiet Liber sample window about 30 s after first open was 199.0 MiB; Zotero's equivalent period overlapped repeated opens, so no idle comparison is claimed for it.
- Scrolling used repeated Computer scroll commands for 10 s, starting at page 1: Liber issued 145 commands and reached about page 61; Zotero issued 79 and reached page 29. Input rates, viewport sizes and traversed pages differ. These are workload observations, not matched scrolling-throughput results. `ps %cpu` is a smoothed process statistic, not an instantaneous CPU peak.
- The Liber zoom sequence coincided with a 2732.6 MiB RSS sample. After resetting the zoom and settling, RSS fell to approximately 178 MiB. No memory leak or cause is inferred from a single sequence.
- Zotero first-run notifications and partial indexing were observed. Defaults were retained; metadata/indexing was not disabled. Other user applications stayed running. Initial host load was 3.04 / 3.94 / 6.62, and desktop recording added load during the interaction measurements.
- Import used each app's file picker, rather than drag-and-drop. Opens/searches/page jumps have three repetitions; import, first-open and scrolling each have one. No cold/warm OS-cache separation, matched exact 300% zoom, complete-book scroll, or frame-based latency claim is made.

## Evidence

Raw isolated process samples: [isolated-process-samples.jsonl](benchmark-gui-evidence/isolated-process-samples.jsonl). Three idle intervals: [idle.json](benchmark-gui-evidence/idle.json). Per-action durations and excluded observations: [gui-timings.json](benchmark-gui-evidence/gui-timings.json). Aggregated resources: [summary.json](benchmark-gui-evidence/summary.json). Binary checks: [binary-hashes.json](benchmark-gui-evidence/binary-hashes.json), [Zotero code sections](benchmark-gui-evidence/zotero-code-hashes.json). Sampling scripts are saved beside these files.
