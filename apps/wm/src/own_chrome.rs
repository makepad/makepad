//! Own chrome: the window manager's half of the contract with an app that
//! draws its own caption (`HostedChrome`, `HostedDragQuery`,
//! `HostedDragAnswer`: `makepad_studio_protocol`, the contract in `platform/src/ime.rs`).
//!
//! The app says once that it draws its own chrome; the desk then draws no
//! frame or title bar for it (the resize edges stay the desk's, at the window
//! border). A primary press inside the window goes to the app as usual and,
//! with it, the app's own window drag query is asked where the press landed.
//! The answer decides: `Caption` makes the desk move the window (the press is
//! taken away from the app with a cancel), anything else was the app's press.
use makepad_hal::window_controls::WINDOW_OP_FULLSCREEN;
use makepad_studio_protocol::{drag_answer, drag_double, window_op, HostedDragAnswer, HostedDragQuery, HostedWindowOp};
use makepad_widgets::*;

/// What the desk does for a window op the app asked for (`HostedWindowOp`):
/// the same paths as the desk's own caption buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowAction {
    /// The desk's close (`request_close`).
    Close,
    /// Minimise to the dock.
    Minimize,
    /// Back from the dock (or wherever it was), raised and focused: a tray app opening its
    /// window again.
    Show,
    /// Maximise in place (fill the space's usable area, the centre band while
    /// something is parked) or restore; never the whole screen.
    Maximise(bool),
    /// Leave a fullscreen space, back to the exact rect the window had.
    LeaveFullscreen,
    /// Fullscreen in a space of its own on the window's screen (bar and dock covered).
    EnterFullscreen,
}

/// Where a window stands, which the app cannot see.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WindowState {
    pub maximised: bool,
    pub fullscreen: bool,
}

/// The green button, on every window (an app-drawn caption's maximise, the desk's
/// own): into a full-screen space of its own, or out of it again.
pub fn green_button(state: WindowState) -> WindowAction {
    if state.fullscreen {
        WindowAction::LeaveFullscreen
    } else {
        WindowAction::EnterFullscreen
    }
}

/// A double click on a caption, on every window (the app's own, the desk's): maximise
/// in place, again restores; a full-screen window leaves full screen.
pub fn caption_double_click(state: WindowState) -> WindowAction {
    if state.fullscreen {
        WindowAction::LeaveFullscreen
    } else {
        WindowAction::Maximise(!state.maximised)
    }
}

/// A double click where an app's drag answer says the window drags from: a caption's, unless
/// the window has no caption and drags by its content (a picture window: Amp's party window,
/// a visualiser's output) and declared its own double click there
/// (`HostedDragAnswer::double_click`, `drag_double::FULLSCREEN`, which an app sends only for
/// such a window): full screen on its screen, and a double click there leaves it again.
pub fn drag_double_click(double_click: u8, state: WindowState) -> WindowAction {
    if double_click == drag_double::FULLSCREEN && !state.fullscreen {
        WindowAction::EnterFullscreen
    } else {
        caption_double_click(state)
    }
}

/// Routes an app's window op. The app cannot see the window's state, so its
/// green button toggles full screen; its own full-screen request (`Window::fullscreen`)
/// enters it; a restore only leaves a maximised or fullscreen window and is nothing
/// otherwise.
pub fn route_window_op(op: &HostedWindowOp, state: WindowState) -> Option<WindowAction> {
    match op.op {
        window_op::CLOSE => Some(WindowAction::Close),
        window_op::MINIMIZE => Some(WindowAction::Minimize),
        window_op::SHOW => Some(WindowAction::Show),
        window_op::MAXIMIZE => Some(green_button(state)),
        WINDOW_OP_FULLSCREEN => Some(WindowAction::EnterFullscreen),
        window_op::RESTORE if state.fullscreen => Some(WindowAction::LeaveFullscreen),
        window_op::RESTORE => state.maximised.then_some(WindowAction::Maximise(false)),
        _ => None,
    }
}

/// One press waiting for the app's answer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChromePress {
    id: u32,
    /// Where the press landed, in the desk's coordinates.
    abs: Vec2d,
    /// Released before the answer came (a click): no drag, but it still counts toward a
    /// double click on the caption.
    released: bool,
}

/// What a caption answer comes to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CaptionPress {
    /// The press is still down: the desk moves the window from `abs`.
    Drag(Vec2d),
    /// It was released already: a click on the caption at `abs`.
    Click(Vec2d),
}

impl CaptionPress {
    pub fn abs(self) -> Vec2d {
        match self {
            CaptionPress::Drag(abs) | CaptionPress::Click(abs) => abs,
        }
    }
}

/// The pending press of one hosted window.
#[derive(Debug, Default)]
pub struct ChromePresses {
    next: u32,
    /// Presses still waiting for their answer, oldest first: a quick double click sends
    /// both presses before the first answer is back.
    pending: Vec<ChromePress>,
}

impl ChromePresses {
    /// A press at `abs` (desk coordinates), `local` in the window's points:
    /// the query to send the app.
    pub fn begin(&mut self, abs: Vec2d, local: Vec2d) -> HostedDragQuery {
        // Ids unique across every window of the WM: an app's windows share one channel,
        // and the answer comes back on it without saying which window it is for.
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
        self.next = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // An older press can no longer start a drag; it may still count as a click.
        for press in &mut self.pending {
            press.released = true;
        }
        if self.pending.len() >= 4 {
            self.pending.remove(0);
        }
        self.pending.push(ChromePress { id: self.next, abs, released: false });
        HostedDragQuery { id: self.next, x: local.x, y: local.y }
    }
    /// The answer: a press on the app's caption (a drag while it is held and the newest, a
    /// click otherwise), `None` for the app's own press or an answer already taken.
    pub fn answer(&mut self, answer: &HostedDragAnswer) -> Option<CaptionPress> {
        let at = self.pending.iter().position(|p| p.id == answer.id)?;
        let press = self.pending[at];
        self.pending.drain(..=at);
        (answer.response == drag_answer::CAPTION)
            .then_some(if press.released { CaptionPress::Click(press.abs) } else { CaptionPress::Drag(press.abs) })
    }
    /// The press ended: an answer still to come is a click, never a drag.
    pub fn end(&mut self) {
        for press in &mut self.pending {
            press.released = true;
        }
    }
    /// The app stopped drawing its chrome: no answer counts.
    pub fn clear(&mut self) {
        self.pending.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ans(id: u32, response: u8) -> HostedDragAnswer {
        HostedDragAnswer { id, response, double_click: drag_double::MAXIMISE }
    }

    /// The rule on every window: the green button goes full screen (and out again), a
    /// double click on the caption maximises in place (and out of full screen).
    #[test]
    fn the_green_button_goes_full_screen_and_a_double_click_maximises() {
        let op = |op| HostedWindowOp { op };
        let st = |maximised, fullscreen| WindowState { maximised, fullscreen };
        let plain = st(false, false);
        assert_eq!(route_window_op(&op(window_op::CLOSE), plain), Some(WindowAction::Close));
        assert_eq!(route_window_op(&op(window_op::CLOSE), st(true, false)), Some(WindowAction::Close));
        assert_eq!(route_window_op(&op(window_op::MINIMIZE), plain), Some(WindowAction::Minimize));
        assert_eq!(route_window_op(&op(window_op::SHOW), plain), Some(WindowAction::Show));
        // The green button: full screen, a maximised window too; again leaves it.
        assert_eq!(route_window_op(&op(window_op::MAXIMIZE), plain), Some(WindowAction::EnterFullscreen));
        assert_eq!(route_window_op(&op(window_op::MAXIMIZE), st(true, false)), Some(WindowAction::EnterFullscreen));
        assert_eq!(route_window_op(&op(window_op::MAXIMIZE), st(false, true)), Some(WindowAction::LeaveFullscreen));
        // The app's own full-screen request enters it, and a restore leaves it.
        assert_eq!(route_window_op(&op(WINDOW_OP_FULLSCREEN), plain), Some(WindowAction::EnterFullscreen));
        assert_eq!(route_window_op(&op(WINDOW_OP_FULLSCREEN), st(false, true)), Some(WindowAction::EnterFullscreen));
        assert_eq!(route_window_op(&op(window_op::RESTORE), st(true, false)), Some(WindowAction::Maximise(false)));
        assert_eq!(route_window_op(&op(window_op::RESTORE), st(false, true)), Some(WindowAction::LeaveFullscreen));
        assert_eq!(route_window_op(&op(window_op::RESTORE), plain), None);
        assert_eq!(route_window_op(&op(200), plain), None);
        // A double click on the caption: maximise in place, again restores; out of full screen.
        assert_eq!(caption_double_click(plain), WindowAction::Maximise(true));
        assert_eq!(caption_double_click(st(true, false)), WindowAction::Maximise(false));
        assert_eq!(caption_double_click(st(false, true)), WindowAction::LeaveFullscreen);
        assert_eq!(green_button(plain), WindowAction::EnterFullscreen);
        assert_eq!(green_button(st(false, true)), WindowAction::LeaveFullscreen);
    }

    /// A window that drags by its content and declared full screen for its double click
    /// (a picture window) gets it, and a double click leaves it again; any other drag answer
    /// is a caption's, which maximises.
    #[test]
    fn a_picture_window_keeps_its_declared_double_click() {
        let st = |maximised, fullscreen| WindowState { maximised, fullscreen };
        assert_eq!(drag_double_click(drag_double::FULLSCREEN, st(false, false)), WindowAction::EnterFullscreen);
        assert_eq!(drag_double_click(drag_double::FULLSCREEN, st(true, false)), WindowAction::EnterFullscreen);
        assert_eq!(drag_double_click(drag_double::FULLSCREEN, st(false, true)), WindowAction::LeaveFullscreen);
        assert_eq!(drag_double_click(drag_double::MAXIMISE, st(false, false)), WindowAction::Maximise(true));
        assert_eq!(drag_double_click(drag_double::MAXIMISE, st(true, false)), WindowAction::Maximise(false));
        assert_eq!(drag_double_click(drag_double::MAXIMISE, st(false, true)), WindowAction::LeaveFullscreen);
    }

    #[test]
    fn a_caption_answer_starts_the_drag_where_the_press_landed() {
        let mut p = ChromePresses::default();
        let q = p.begin(dvec2(300.0, 200.0), dvec2(40.0, 12.0));
        assert_eq!((q.x, q.y), (40.0, 12.0));
        assert_eq!(p.answer(&ans(q.id, drag_answer::CAPTION)), Some(CaptionPress::Drag(dvec2(300.0, 200.0))));
        assert_eq!(p.answer(&ans(q.id, drag_answer::CAPTION)), None, "taken once");
    }

    #[test]
    fn client_no_answer_and_buttons_stay_the_apps_press() {
        let mut p = ChromePresses::default();
        for r in [drag_answer::CLIENT, drag_answer::NO_ANSWER, drag_answer::SYS_MENU] {
            let q = p.begin(dvec2(1.0, 1.0), dvec2(1.0, 1.0));
            assert_eq!(p.answer(&ans(q.id, r)), None);
        }
    }

    #[test]
    fn presses_answered_late_are_clicks_and_only_the_newest_held_one_drags() {
        let mut p = ChromePresses::default();
        // A double click: both presses go out before the first answer comes back.
        let old = p.begin(dvec2(1.0, 1.0), dvec2(1.0, 1.0));
        p.end();
        let new = p.begin(dvec2(2.0, 2.0), dvec2(2.0, 2.0));
        assert_eq!(p.answer(&ans(old.id, drag_answer::CAPTION)), Some(CaptionPress::Click(dvec2(1.0, 1.0))));
        assert_eq!(p.answer(&ans(old.id, drag_answer::CAPTION)), None, "taken once");
        p.end();
        assert_eq!(p.answer(&ans(new.id, drag_answer::CAPTION)), Some(CaptionPress::Click(dvec2(2.0, 2.0))));
        // A newer press answered first makes the older answer stale.
        let a = p.begin(dvec2(5.0, 5.0), dvec2(5.0, 5.0));
        let b = p.begin(dvec2(6.0, 6.0), dvec2(6.0, 6.0));
        assert_eq!(p.answer(&ans(b.id, drag_answer::CAPTION)), Some(CaptionPress::Drag(dvec2(6.0, 6.0))));
        assert_eq!(p.answer(&ans(a.id, drag_answer::CAPTION)), None);
        let q = p.begin(dvec2(3.0, 3.0), dvec2(3.0, 3.0));
        p.clear();
        assert_eq!(p.answer(&ans(q.id, drag_answer::CAPTION)), None);
    }
}
