//! Runs the Rust application directly on the machine's native backend.

#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

#[cfg(target_os = "linux")]
use nsh_platform_linux as platform;
#[cfg(target_os = "macos")]
use nsh_platform_macos as platform;
#[cfg(target_os = "windows")]
use nsh_platform_windows as platform;

/// macOS: the window is AppKit's split view with the library's sidebar.
#[cfg(target_os = "macos")]
fn main() -> impl std::process::Termination {
    env_logger::init();
    liber::sidebar::SYSTEM.store(true, std::sync::atomic::Ordering::Relaxed);
    liber::open_startup_reader();
    imago_window_macos::run(
        "Liber",
        (1200.0, 820.0),
        liber::sidebar::system_rows(),
        || platform::Root::of::<liber::App>(),
        || platform::Root::of::<liber::ReaderApp>(),
    )
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn main() -> impl std::process::Termination {
    env_logger::init();
    platform::run_app(
        platform::AppSource::Rust(platform::Root::of::<liber::App>()),
        platform::AppOptions {
            title: "Liber".to_owned(),
            app_id: "com.kosmos.liber".to_owned(),
            size: (1200, 820),
            ..Default::default()
        },
    )
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
compile_error!(
    "Run this binary on macOS, Windows or Linux; use the TSX entry with nsh run for iOS and Android."
);
