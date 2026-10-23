//! Small shared widgets: borderless icon buttons for bars, as in Agenda/Cortex.

use gpui::{
    div, px, svg, InteractiveElement, ParentElement, Role, SharedString, Stateful,
    StatefulInteractiveElement, Styled,
};
use imago_gpui::theme::{rgba, FG};

// Library, settings, reader and native window buttons share this grid.
pub use imago_gpui::chrome::TITLEBAR_H;

/// 28pt square, no border; faint fill on hover, stronger when `active`.
pub fn icon_button(
    id: &'static str,
    path: &'static str,
    label: &'static str,
    active: bool,
) -> Stateful<gpui::Div> {
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(label)
        .size(px(28.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .cursor_pointer()
        .bg(rgba(FG(), if active { 0.12 } else { 0.0 }))
        .hover(|s| s.bg(rgba(FG(), 0.08)))
        .child(
            svg()
                .path(path)
                .size(px(16.))
                .text_color(rgba(FG(), if active { 0.95 } else { 0.65 })),
        )
}

/// Same, with a text label ("Aa").
pub fn text_button(
    id: &'static str,
    text: &'static str,
    label: &'static str,
    active: bool,
) -> Stateful<gpui::Div> {
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(label)
        .h(px(28.))
        .px(px(8.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .cursor_pointer()
        .text_size(px(13.))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(rgba(FG(), if active { 0.95 } else { 0.65 }))
        .bg(rgba(FG(), if active { 0.12 } else { 0.0 }))
        .hover(|s| s.bg(rgba(FG(), 0.08)))
        .child(SharedString::from(text))
}

/// Agenda's interruptible quick-entry pane tween (200 ms, CSS ease-out).
/// Only a running transition requests frames; there is no idle animation loop.
pub struct PaneMotion {
    open: bool,
    tween: Option<(f32, f32, std::time::Instant)>,
}

impl PaneMotion {
    pub fn new() -> Self {
        Self {
            open: false,
            tween: None,
        }
    }
    fn value(&self, now: std::time::Instant) -> f32 {
        let Some((from, to, started)) = self.tween else {
            return if self.open { 1.0 } else { 0.0 };
        };
        let t = now.duration_since(started).as_secs_f32() / 0.2;
        if t >= 1.0 {
            return to;
        }
        from + (to - from) * imago_gpui::theme::cubic_bezier(0.0, 0.0, 0.58, 1.0, t)
    }
    pub fn progress(&mut self, open: bool, now: std::time::Instant) -> f32 {
        if self.open != open {
            let from = self.value(now);
            self.open = open;
            self.tween = Some((from, if open { 1.0 } else { 0.0 }, now));
        }
        let value = self.value(now);
        if self
            .tween
            .is_some_and(|(_, _, start)| now.duration_since(start).as_secs_f32() >= 0.2)
        {
            self.tween = None;
        }
        value
    }
    pub fn animating(&self) -> bool {
        self.tween.is_some()
    }
}

/// Same fixed-width inner shell and animated clipping column as Agenda's
/// qe_panel. POPOVER surface, 10% white mix divider, mirrored 32px shadow.
pub fn reader_pane(
    width: f32,
    progress: f32,
    right: bool,
    content: impl gpui::IntoElement,
) -> gpui::Div {
    use gpui::prelude::FluentBuilder;
    use imago_gpui::theme::{c, mix, POPOVER};
    let inner = div()
        .relative()
        .w(px(width))
        .h_full()
        .flex_none()
        .flex()
        .flex_col()
        .bg(c(POPOVER()))
        .when(right, |el| el.ml_auto().border_l_1())
        .when(!right, |el| el.mr_auto().border_r_1())
        .border_color(mix(0xffffff, 0.10, POPOVER()))
        .shadow(vec![gpui::BoxShadow {
            color: rgba(0x000000, 0.20),
            offset: gpui::point(px(if right { -8.0 } else { 8.0 }), px(0.0)),
            blur_radius: px(32.0),
            spread_radius: px(0.0),
            inset: false,
        }])
        .child(content);
    div()
        .w(px(width * progress))
        .h_full()
        .flex_none()
        .overflow_hidden()
        .child(inner)
}

#[cfg(test)]
mod pane_tests {
    use super::*;
    #[test]
    fn reversing_agenda_pane_animation_has_no_width_jump_and_stops() {
        let start = std::time::Instant::now();
        let mut pane = PaneMotion::new();
        assert_eq!(pane.progress(true, start), 0.0);
        let midway = start + std::time::Duration::from_millis(80);
        let before = pane.progress(true, midway);
        let reversed = pane.progress(false, midway);
        assert!((before - reversed).abs() < 0.0001);
        assert!(pane.animating());
        assert_eq!(
            pane.progress(false, midway + std::time::Duration::from_millis(201)),
            0.0
        );
        assert!(!pane.animating());
    }
}
