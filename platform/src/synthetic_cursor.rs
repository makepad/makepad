//! A pointer drawn INTO the window's frame, for captures of scripted runs.
//!
//! A hidden window, or an in-process recording, has no OS cursor in it, so a
//! demo made from injected input shows things happening with nothing
//! pointing at them. `/cursor?show=1` turns this on: the remote bridge moves
//! it with every injected mouse event and stamps each press, and the widgets
//! `Window` draws it (an arrow, and a ripple after a press) over everything
//! in its own pass, so grabs and captures include it. Off by default; while
//! off nothing reads it but one thread-local load per window draw.
//!
//! UI-thread state only: the bridge writes it while applying commands, the
//! window reads it while drawing.
use crate::cursor::MouseCursor;
use crate::makepad_math::Vec2d;
use std::cell::Cell;

/// How long a press ripple runs, in app-clock seconds.
pub const RIPPLE_SECONDS: f64 = 0.4;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SyntheticCursorStyle {
    /// Always this shape.
    Fixed(MouseCursor),
    /// Whatever the app asks for under the pointer (`Cx::mouse_cursor`).
    App,
}

impl SyntheticCursorStyle {
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name.to_ascii_lowercase().as_str() {
            "arrow" | "default" => Self::Fixed(MouseCursor::Arrow),
            "hand" | "pointer" => Self::Fixed(MouseCursor::Hand),
            "text" => Self::Fixed(MouseCursor::Text),
            "crosshair" => Self::Fixed(MouseCursor::Crosshair),
            "app" | "auto" => Self::App,
            _ => return None,
        })
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Fixed(MouseCursor::Hand) => "hand",
            Self::Fixed(MouseCursor::Text) => "text",
            Self::Fixed(MouseCursor::Crosshair) => "crosshair",
            Self::Fixed(_) => "arrow",
            Self::App => "app",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SyntheticCursor {
    pub style: SyntheticCursorStyle,
    /// The window the last injected pointer event went to; `None` until one
    /// has (the cursor is then drawn in every window, at `pos`).
    pub window_id: Option<usize>,
    /// Window-local layout points.
    pub pos: Vec2d,
    pub pressed: bool,
    /// App-clock time of the last press, for the ripple.
    pub press_time: Option<f64>,
    /// Bumped on every change, so a window redraws only when it must.
    pub generation: u64,
}

impl SyntheticCursor {
    /// How far the last press's ripple has run at `now`, `0..1`, or `None`
    /// once it is over.
    pub fn ripple_progress(&self, now: f64) -> Option<f64> {
        let progress = (now - self.press_time?) / RIPPLE_SECONDS;
        (0.0..1.0).contains(&progress).then_some(progress)
    }

    /// Is this cursor drawn in `window_id`?
    pub fn shows_in(&self, window_id: usize) -> bool {
        self.window_id.is_none_or(|id| id == window_id)
    }
}

thread_local! {
    static CURSOR: Cell<Option<SyntheticCursor>> = const { Cell::new(None) };
    static GENERATION: Cell<u64> = const { Cell::new(0) };
}

/// The cursor, while one is shown.
pub fn synthetic_cursor() -> Option<SyntheticCursor> {
    CURSOR.get()
}

fn next_generation() -> u64 {
    let generation = GENERATION.get() + 1;
    GENERATION.set(generation);
    generation
}

/// Show (or restyle) the cursor, keeping its position; `None` hides it.
pub fn set_synthetic_cursor(style: Option<SyntheticCursorStyle>) {
    let cursor = style.map(|style| {
        let mut cursor = CURSOR.get().unwrap_or(SyntheticCursor {
            style,
            window_id: None,
            pos: Vec2d::default(),
            pressed: false,
            press_time: None,
            generation: 0,
        });
        cursor.style = style;
        cursor.generation = next_generation();
        cursor
    });
    CURSOR.set(cursor);
}

/// An injected pointer event: move there, and press or release when `down`
/// says so. Nothing happens while the cursor is hidden.
pub fn note_synthetic_pointer(window_id: usize, pos: Vec2d, down: Option<bool>, time: f64) {
    let Some(mut cursor) = CURSOR.get() else {
        return;
    };
    let before = cursor;
    cursor.window_id = Some(window_id);
    cursor.pos = pos;
    match down {
        Some(true) => {
            cursor.pressed = true;
            cursor.press_time = Some(time);
        }
        Some(false) => cursor.pressed = false,
        None => {}
    }
    if cursor != before || down == Some(true) {
        cursor.generation = next_generation();
        CURSOR.set(Some(cursor));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::makepad_math::dvec2;

    #[test]
    fn hidden_cursor_ignores_pointer_events() {
        set_synthetic_cursor(None);
        note_synthetic_pointer(0, dvec2(10.0, 10.0), Some(true), 1.0);
        assert!(synthetic_cursor().is_none());
    }

    #[test]
    fn a_press_starts_a_ripple_that_ends() {
        set_synthetic_cursor(SyntheticCursorStyle::parse("arrow"));
        let shown = synthetic_cursor().unwrap();
        note_synthetic_pointer(1, dvec2(5.0, 6.0), None, 0.5);
        let moved = synthetic_cursor().unwrap();
        assert!(moved.generation > shown.generation);
        assert_eq!(moved.pos, dvec2(5.0, 6.0));
        assert!(moved.shows_in(1) && !moved.shows_in(0));
        assert_eq!(moved.ripple_progress(0.5), None);
        note_synthetic_pointer(1, dvec2(5.0, 6.0), Some(true), 1.0);
        let pressed = synthetic_cursor().unwrap();
        assert!(pressed.pressed);
        assert_eq!(pressed.ripple_progress(1.0), Some(0.0));
        assert!(pressed.ripple_progress(1.0 + RIPPLE_SECONDS * 0.5).is_some());
        assert_eq!(pressed.ripple_progress(1.01 + RIPPLE_SECONDS), None);
        // an unchanged move is not a change
        note_synthetic_pointer(1, dvec2(5.0, 6.0), None, 1.1);
        assert_eq!(synthetic_cursor().unwrap().generation, pressed.generation);
        set_synthetic_cursor(None);
    }

    #[test]
    fn styles_parse_and_name_themselves() {
        for name in ["arrow", "hand", "text", "crosshair", "app"] {
            assert_eq!(SyntheticCursorStyle::parse(name).unwrap().name(), name);
        }
        assert!(SyntheticCursorStyle::parse("wobble").is_none());
    }
}
