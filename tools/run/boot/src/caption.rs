//! The caption strip of the boot window and of the halted view over the app: the window's
//! platform caption, mirrored from the widgets' stock caption (widgets/core/src/window.rs), since
//! Relax draws with fonts and shaders only. Where the window system draws the window buttons
//! (macOS: the traffic lights, reported by the HAL's `Chrome` event) the strip keeps their room and
//! is centred on them (height = twice their top inset + their height); where the app draws its
//! own (`info_flags::CLIENT_CHROME`: Windows, Wayland's client-side decorations) they are
//! minimise / maximise / close, 46 x 29 points each, at the strip's right. The title is centred
//! in the strip when the buttons leave room for it, else in the widest gap they leave, cut with
//! an ellipsis. The strip is the window's drag region (`Boot::drag_answer`).
use crate::{
    document::Line,
    text::{Shaped, Text},
    Action, TextLine,
};
use makepad_hal::window_state;
use std::sync::Arc;

/// The size of one app-drawn window button, in points (widgets' `DesktopButton`).
pub const BUTTON: [f32; 2] = [46.0, 29.0];
/// The space kept between the title and the buttons (widgets' `CAPTION_TITLE_SPACING`).
pub const TITLE_SPACING: f32 = 6.0;
/// The title's colour: the field's soft ink, as the stock caption's label is its theme's.
pub const TITLE: [f32; 4] = [0.706, 0.671, 0.839, 1.0];
/// The space between the title's parts, in points.
const PART_GAP: f32 = 6.0;

/// What the platform reports about the window's chrome.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Chrome {
    /// The native window buttons' bounds (x, y, w, h, window points; zero: none).
    pub native: [f32; 4],
    /// The app draws its own window buttons.
    pub client: bool,
    /// The window's `window_state`.
    pub state: u32,
}

/// The caption strip at the window's top.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Caption {
    /// Zero: the window has no caption of ours (touch, fullscreen, a system-drawn title bar).
    pub height: f32,
    /// The span the title may use (x from, x to).
    pub gap: [f32; 2],
    /// The app-drawn window buttons: their action and rect.
    pub buttons: Vec<(Action, [f32; 4])>,
}

impl Chrome {
    pub fn caption(&self, width: f32, touch: bool) -> Caption {
        if touch || self.state == window_state::FULLSCREEN || width <= 0.0 {
            return Caption::default();
        }
        if self.client {
            let row = BUTTON[0] * 3.0;
            let x0 = (width - row).max(0.0);
            let buttons = [Action::Minimize, Action::Maximize, Action::Close]
                .into_iter()
                .enumerate()
                .map(|(i, action)| (action, [x0 + i as f32 * BUTTON[0], 0.0, BUTTON[0], BUTTON[1]]))
                .collect();
            return Caption { height: BUTTON[1], gap: [0.0, x0], buttons };
        }
        let [x, y, w, h] = self.native;
        if !(w > 0.0 && h > 0.0) {
            return Caption::default();
        }
        // the buttons sit at one end of the strip (the left on macOS, the right in a
        // right-to-left layout): the title has the rest
        let gap = if x + w * 0.5 < width * 0.5 { [x + w, width] } else { [0.0, x] };
        Caption { height: (y.max(0.0) * 2.0 + h).round(), gap, buttons: Vec::new() }
    }
}

impl Caption {
    /// Where a title `width` points wide starts in a window `window` points wide, and the width
    /// it may take: centred in the window when that keeps it inside the gap, else at the
    /// gap's start (it is then cut to the gap).
    pub fn title_at(&self, width: f32, window: f32) -> (f32, f32) {
        let left = self.gap[0] + TITLE_SPACING;
        let right = self.gap[1] - TITLE_SPACING;
        let room = (right - left).max(0.0);
        let centred = (window - width) * 0.5;
        let x = if centred >= left && centred + width <= right {
            centred
        } else {
            // as close to the centre as the gap allows
            centred.clamp(left, (right - width).max(left))
        };
        (x, room)
    }

    /// The window button at `point`, if any.
    pub fn button_at(&self, point: [f32; 2]) -> Option<Action> {
        self.buttons.iter().find(|(_, r)| crate::contains(*r, point)).map(|(a, _)| *a)
    }
}

/// The caption's title as text lines fixed to the window: `parts` follow each other on one
/// line, `PART_GAP` apart (the app's name, then what the view shows), centred on the strip's
/// middle. Parts that
/// do not fit are left out from the last; a first part that alone does not fit is cut with an
/// ellipsis.
pub fn title(text: &mut Text, caption: &Caption, window: f32, parts: &[(String, Line)]) -> Vec<TextLine> {
    if caption.height <= 0.0 || parts.is_empty() {
        return Vec::new();
    }
    let room = (caption.gap[1] - caption.gap[0] - 2.0 * TITLE_SPACING).max(0.0);
    let mut shaped: Vec<(Arc<Shaped>, Line)> = Vec::new();
    let mut total = 0.0;
    for (i, (value, style)) in parts.iter().enumerate() {
        let run = text.shape(value, style.size, 1.0e4, style.height);
        let gap = if i == 0 { 0.0 } else { PART_GAP };
        if total + gap + run.width > room {
            if i == 0 {
                let mut chars: Vec<char> = value.chars().collect();
                while !chars.is_empty() {
                    chars.pop();
                    let cut: String = chars.iter().collect::<String>().trim_end().to_string() + "…";
                    let run = text.shape(&cut, style.size, 1.0e4, style.height);
                    if run.width <= room {
                        total = run.width;
                        shaped.push((run, *style));
                        break;
                    }
                }
            }
            break;
        }
        total += gap + run.width;
        shaped.push((run, *style));
    }
    let (mut x, _) = caption.title_at(total, window);
    let middle = caption.height * 0.5;
    shaped
        .into_iter()
        .map(|(run, style)| {
            // the capitals' middle on the strip's middle
            let baseline = (middle + style.size * 0.36).round();
            let line = TextLine {
                position: [x, baseline - text.ascender(style.size)],
                run: run.clone(),
                style: Line { align: 0.0, ..style },
                fixed: true,
            };
            x += run.width + PART_GAP;
            line
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traffic_lights_keep_their_room_and_centre_the_strip() {
        let chrome = Chrome { native: [7.0, 6.0, 54.0, 16.0], client: false, state: 0 };
        let caption = chrome.caption(900.0, false);
        assert_eq!(caption.height, 28.0);
        assert_eq!(caption.gap, [61.0, 900.0]);
        assert!(caption.buttons.is_empty());
        // a short title is centred in the window, a long one starts after the buttons
        assert_eq!(caption.title_at(100.0, 900.0).0, 400.0);
        assert_eq!(caption.title_at(850.0, 900.0).0, 67.0);
    }

    #[test]
    fn app_drawn_buttons_sit_at_the_right() {
        let chrome = Chrome { native: [0.0; 4], client: true, state: 0 };
        let caption = chrome.caption(800.0, false);
        assert_eq!(caption.height, 29.0);
        assert_eq!(caption.buttons.len(), 3);
        assert_eq!(caption.buttons[2], (Action::Close, [754.0, 0.0, 46.0, 29.0]));
        assert_eq!(caption.button_at([760.0, 10.0]), Some(Action::Close));
        assert_eq!(caption.button_at([600.0, 10.0]), None);
    }

    #[test]
    fn no_caption_on_touch_fullscreen_or_without_chrome() {
        let lights = Chrome { native: [7.0, 6.0, 54.0, 16.0], client: false, state: 0 };
        assert_eq!(lights.caption(900.0, true).height, 0.0);
        let full = Chrome { state: window_state::FULLSCREEN, ..lights };
        assert_eq!(full.caption(900.0, false).height, 0.0);
        assert_eq!(Chrome::default().caption(900.0, false).height, 0.0);
    }
}
