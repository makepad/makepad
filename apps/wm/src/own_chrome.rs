//! Own chrome: the window manager's half of the contract with an app that
//! draws its own caption (`HostedChrome`, `HostedDragQuery`,
//! `HostedDragAnswer` in `platform/src/ime.rs`).
//!
//! The app says once that it draws its own chrome; the desk then draws no
//! frame or title bar for it (the resize edges stay the desk's, at the window
//! border). A primary press inside the window goes to the app as usual and,
//! with it, the app's own window drag query is asked where the press landed.
//! The answer decides: `Caption` makes the desk move the window (the press is
//! taken away from the app with a cancel), anything else was the app's press.
use makepad_widgets::makepad_platform::ime::{drag_answer, window_op, HostedDragAnswer, HostedDragQuery, HostedWindowOp};
use makepad_widgets::*;

/// What the desk does for a window op the app asked for (`HostedWindowOp`):
/// the same paths as the desk's own caption buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowAction {
    /// The desk's close (`request_close`).
    Close,
    /// Minimise to the dock.
    Minimize,
    /// Maximise in place (fill the space's usable area, the centre band while
    /// something is parked) or restore; never the whole screen.
    Maximise(bool),
    /// Leave a fullscreen space (the desk's own fullscreen paths enter one).
    LeaveFullscreen,
}

/// Where a window stands, which the app cannot see.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WindowState {
    pub maximised: bool,
    pub fullscreen: bool,
}

/// Routes an app's window op. The app cannot see the window's state, so its
/// maximise button toggles; a restore only leaves a maximised or fullscreen
/// window and is nothing otherwise.
pub fn route_window_op(op: &HostedWindowOp, state: WindowState) -> Option<WindowAction> {
    match op.op {
        window_op::CLOSE => Some(WindowAction::Close),
        window_op::MINIMIZE => Some(WindowAction::Minimize),
        window_op::MAXIMIZE if state.fullscreen => Some(WindowAction::LeaveFullscreen),
        window_op::MAXIMIZE => Some(WindowAction::Maximise(!state.maximised)),
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
}

/// The pending press of one hosted window.
#[derive(Debug, Default)]
pub struct ChromePresses {
    next: u32,
    pending: Option<ChromePress>,
}

impl ChromePresses {
    /// A press at `abs` (desk coordinates), `local` in the window's points:
    /// the query to send the app.
    pub fn begin(&mut self, abs: Vec2d, local: Vec2d) -> HostedDragQuery {
        self.next = self.next.wrapping_add(1);
        self.pending = Some(ChromePress { id: self.next, abs });
        HostedDragQuery { id: self.next, x: local.x, y: local.y }
    }
    /// The answer: where the window drag starts when the press landed on the
    /// app's caption, `None` for the app's own press, a stale answer, or
    /// one for a press that is over.
    pub fn answer(&mut self, answer: &HostedDragAnswer) -> Option<Vec2d> {
        let press = self.pending.filter(|p| p.id == answer.id)?;
        self.pending = None;
        (answer.response == drag_answer::CAPTION).then_some(press.abs)
    }
    /// The press ended: a later answer is stale.
    pub fn end(&mut self) {
        self.pending = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ans(id: u32, response: u8) -> HostedDragAnswer {
        HostedDragAnswer { id, response }
    }

    #[test]
    fn an_apps_window_ops_take_the_desks_own_paths() {
        let op = |op| HostedWindowOp { op };
        let st = |maximised, fullscreen| WindowState { maximised, fullscreen };
        let plain = st(false, false);
        assert_eq!(route_window_op(&op(window_op::CLOSE), plain), Some(WindowAction::Close));
        assert_eq!(route_window_op(&op(window_op::CLOSE), st(true, false)), Some(WindowAction::Close));
        assert_eq!(route_window_op(&op(window_op::MINIMIZE), plain), Some(WindowAction::Minimize));
        // The maximise button toggles in place: it never asks for fullscreen.
        assert_eq!(route_window_op(&op(window_op::MAXIMIZE), plain), Some(WindowAction::Maximise(true)));
        assert_eq!(route_window_op(&op(window_op::MAXIMIZE), st(true, false)), Some(WindowAction::Maximise(false)));
        assert_eq!(route_window_op(&op(window_op::MAXIMIZE), st(false, true)), Some(WindowAction::LeaveFullscreen));
        assert_eq!(route_window_op(&op(window_op::RESTORE), st(true, false)), Some(WindowAction::Maximise(false)));
        assert_eq!(route_window_op(&op(window_op::RESTORE), st(false, true)), Some(WindowAction::LeaveFullscreen));
        assert_eq!(route_window_op(&op(window_op::RESTORE), plain), None);
        assert_eq!(route_window_op(&op(200), plain), None);
    }

    #[test]
    fn a_caption_answer_starts_the_drag_where_the_press_landed() {
        let mut p = ChromePresses::default();
        let q = p.begin(dvec2(300.0, 200.0), dvec2(40.0, 12.0));
        assert_eq!((q.x, q.y), (40.0, 12.0));
        assert_eq!(p.answer(&ans(q.id, drag_answer::CAPTION)), Some(dvec2(300.0, 200.0)));
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
    fn a_stale_answer_or_one_after_the_release_does_nothing() {
        let mut p = ChromePresses::default();
        let old = p.begin(dvec2(1.0, 1.0), dvec2(1.0, 1.0));
        let new = p.begin(dvec2(2.0, 2.0), dvec2(2.0, 2.0));
        assert_eq!(p.answer(&ans(old.id, drag_answer::CAPTION)), None);
        p.end();
        assert_eq!(p.answer(&ans(new.id, drag_answer::CAPTION)), None);
    }
}
