Vendored from crates.io mupdf 0.8.0. License and upstream sources retained.

Local API addition: Context::trim_store(percent), a safe wrapper around
fz_shrink_store. It exposes eviction of unreferenced entries in the shared
MuPDF store without depending on private Rust object layouts or reopening
PDF documents. No MuPDF C implementation or rendering behavior is changed.
