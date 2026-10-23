//! Native window chrome tweaks gpui does not expose.

use gpui::Window;

/// Keep native buttons on the same vertical grid as every app titlebar.
/// GPUI's position is a button *inset*, not a center: its macOS backend
/// makes the native titlebar `button_height + 2 * inset` tall. Read the
/// live AppKit button instead of assuming a size across macOS versions.
/// Using GPUI's setter also preserves its resize/fullscreen restoration.
#[cfg(target_os = "macos")]
pub fn align_traffic_lights(window: &Window) {
    use gpui::{point, px};
    use objc2_app_kit::{NSView, NSWindowButton};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return;
    };
    // SAFETY: GPUI owns this live content view and renders on the main thread.
    let view: &NSView = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
    let Some(ns_window) = view.window() else {
        return;
    };
    let Some(button) = ns_window.standardWindowButton(NSWindowButton::CloseButton) else {
        return;
    };
    if window.is_fullscreen() {
        return;
    }
    let height = button.frame().size.height as f32;
    if height > 0.0 {
        let inset = (crate::ui::TITLEBAR_H - height) / 2.0;
        // Scrolling and animations render frequently. Avoid rebuilding native
        // tracking areas when the AppKit grid is already correct.
        let aligned = unsafe { button.superview().and_then(|view| view.superview()) }.is_some_and(
            |container| {
                let frame = container.frame();
                frame.size.height == f64::from(crate::ui::TITLEBAR_H)
                    && frame.origin.y
                        == ns_window.frame().size.height - f64::from(crate::ui::TITLEBAR_H)
                    && button.frame().origin.x == 18.0
                    && button.frame().origin.y == f64::from(inset)
            },
        );
        if !aligned {
            window.set_traffic_light_position(point(px(18.), px(inset)));
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn align_traffic_lights(_window: &Window) {}

/// Fade the macOS traffic-light buttons (`1.0` visible, `0.0` hidden).
#[cfg(target_os = "macos")]
pub fn set_traffic_lights_alpha(window: &Window, alpha: f64) {
    use objc2_app_kit::{NSView, NSWindowButton};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return;
    };
    // SAFETY: the handle's `ns_view` is the live content view of this window,
    // and gpui calls us on the main thread.
    let view: &NSView = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
    let Some(ns_window) = view.window() else {
        return;
    };
    for kind in [
        NSWindowButton::CloseButton,
        NSWindowButton::MiniaturizeButton,
        NSWindowButton::ZoomButton,
    ] {
        if let Some(button) = ns_window.standardWindowButton(kind) {
            button.setAlphaValue(alpha);
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn set_traffic_lights_alpha(_window: &Window, _alpha: f64) {}

/// AppKit also lays out its titlebar without a GPUI render (focus changes,
/// sharing indicators, native window state). Enforce the grid when those
/// native frames change, rather than polling or waiting for navigation.
#[cfg(target_os = "macos")]
pub struct WindowChrome {
    observers: Vec<
        objc2::rc::Retained<objc2::runtime::ProtocolObject<dyn objc2_foundation::NSObjectProtocol>>,
    >,
}

#[cfg(target_os = "macos")]
impl WindowChrome {
    pub fn new(window: &Window) -> Self {
        use block2::RcBlock;
        use objc2_app_kit::{
            NSView, NSViewFrameDidChangeNotification, NSWindowButton,
            NSWindowDidEnterFullScreenNotification, NSWindowDidExitFullScreenNotification,
            NSWindowStyleMask, NSWindowWillEnterFullScreenNotification,
        };
        use objc2_foundation::{NSNotification, NSNotificationCenter, NSOperationQueue, NSPoint};
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        use std::{cell::Cell, ptr::NonNull, rc::Rc};

        let mut chrome = Self {
            observers: Vec::new(),
        };
        let Ok(handle) = HasWindowHandle::window_handle(window) else {
            return chrome;
        };
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return chrome;
        };
        // SAFETY: this is GPUI's live main-thread AppKit content view.
        let view = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
        let Some(native) = view.window() else {
            return chrome;
        };
        let Some(close) = native.standardWindowButton(NSWindowButton::CloseButton) else {
            return chrome;
        };
        let Some(minimize) = native.standardWindowButton(NSWindowButton::MiniaturizeButton) else {
            return chrome;
        };
        // Capture spacing before AppKit starts updating individual button frames.
        let gap = minimize.frame().origin.x - close.frame().origin.x;
        // SAFETY: all views belong to this main-thread window.
        let Some(container) = (unsafe { close.superview().and_then(|v| v.superview()) }) else {
            return chrome;
        };
        let mut views = vec![container.clone()];
        for kind in [
            NSWindowButton::CloseButton,
            NSWindowButton::MiniaturizeButton,
            NSWindowButton::ZoomButton,
        ] {
            if let Some(button) = native.standardWindowButton(kind) {
                views.push(objc2::rc::Retained::into_super(
                    objc2::rc::Retained::into_super(button),
                ));
            }
        }
        let updating = Rc::new(Cell::new(false));
        let entering_fullscreen = Rc::new(Cell::new(false));
        let center = NSNotificationCenter::defaultCenter();
        for (name, entering) in unsafe {
            [
                (NSWindowWillEnterFullScreenNotification, true),
                (NSWindowDidEnterFullScreenNotification, false),
                (NSWindowDidExitFullScreenNotification, false),
            ]
        } {
            let state = entering_fullscreen.clone();
            let callback = RcBlock::new(move |_: NonNull<NSNotification>| state.set(entering));
            // SAFETY: window notifications are delivered on the main thread.
            chrome.observers.push(unsafe {
                center.addObserverForName_object_queue_usingBlock(
                    Some(name),
                    Some(&native),
                    None,
                    &callback,
                )
            });
        }
        for observed in views {
            observed.setPostsFrameChangedNotifications(true);
            let native = native.clone();
            let container = container.clone();
            let updating = updating.clone();
            let entering_fullscreen = entering_fullscreen.clone();
            let callback = RcBlock::new(move |_: NonNull<NSNotification>| {
                if updating.get()
                    || entering_fullscreen.get()
                    || native.styleMask().contains(NSWindowStyleMask::FullScreen)
                {
                    return;
                }
                updating.set(true);
                let native = native.clone();
                let container = container.clone();
                let updating = updating.clone();
                let entering_fullscreen = entering_fullscreen.clone();
                // Native notifications may arrive inside GPUI's window lock.
                // Correct the completed AppKit layout on the next main-queue
                // turn, coalescing all button/container notifications.
                let layout = RcBlock::new(move || {
                    if entering_fullscreen.get()
                        || native.styleMask().contains(NSWindowStyleMask::FullScreen)
                    {
                        updating.set(false);
                        return;
                    }
                    let kinds = [
                        NSWindowButton::CloseButton,
                        NSWindowButton::MiniaturizeButton,
                        NSWindowButton::ZoomButton,
                    ];
                    let buttons: Vec<_> = kinds
                        .into_iter()
                        .filter_map(|k| native.standardWindowButton(k))
                        .collect();
                    if buttons.len() == 3 {
                        let h = f64::from(crate::ui::TITLEBAR_H);
                        let mut frame = container.frame();
                        frame.size.height = h;
                        frame.origin.y = native.frame().size.height - h;
                        if container.frame() != frame {
                            container.setFrame(frame);
                        }
                        for (i, button) in buttons.iter().enumerate() {
                            let origin = NSPoint::new(
                                18.0 + i as f64 * gap,
                                (h - button.frame().size.height) / 2.0,
                            );
                            if button.frame().origin != origin {
                                button.setFrameOrigin(origin);
                                button.updateTrackingAreas();
                            }
                        }
                        container.updateTrackingAreas();
                    }
                    updating.set(false);
                });
                // SAFETY: this queue runs the captured AppKit objects only on
                // the main thread, after the current native layout returns.
                unsafe { NSOperationQueue::mainQueue().addOperationWithBlock(&layout) };
            });
            // SAFETY: observations are restricted to main-thread AppKit views;
            // delivery is synchronous on that thread. Drop unregisters tokens.
            let token = unsafe {
                center.addObserverForName_object_queue_usingBlock(
                    Some(NSViewFrameDidChangeNotification),
                    Some(&observed),
                    None,
                    &callback,
                )
            };
            chrome.observers.push(token);
        }
        chrome
    }
}

#[cfg(target_os = "macos")]
impl Drop for WindowChrome {
    fn drop(&mut self) {
        let center = objc2_foundation::NSNotificationCenter::defaultCenter();
        for observer in &self.observers {
            // SAFETY: these tokens were registered by this guard and remain live.
            unsafe { center.removeObserver((**observer).as_ref()) };
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub struct WindowChrome;

#[cfg(not(target_os = "macos"))]
impl WindowChrome {
    pub fn new(_window: &Window) -> Self {
        Self
    }
}
