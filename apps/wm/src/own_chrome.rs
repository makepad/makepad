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
    /// Maximise in place (fill the space's usable area, the centre band while
    /// something is parked) or restore; never the whole screen.
    Maximise(bool),
    /// Leave a fullscreen space (the desk's own fullscreen paths enter one).
    LeaveFullscreen,
    /// Fullscreen in a space of its own on the window's screen (bar and dock covered): a
    /// double click on the caption of a window that declares it (`drag_double::FULLSCREEN`).
    EnterFullscreen,
}

/// A double click on the app's caption: what the window declared in its drag answer
/// (`HostedDragAnswer::double_click`) -- maximise in place, or fullscreen on its screen --
/// toggled; a fullscreen window leaves fullscreen (back to the exact rect it had).
pub fn caption_double_click(double_click: u8, state: WindowState) -> WindowAction {
    if state.fullscreen {
        WindowAction::LeaveFullscreen
    } else if double_click == drag_double::FULLSCREEN {
        WindowAction::EnterFullscreen
    } else {
        WindowAction::Maximise(!state.maximised)
    }
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
    fn a_caption_double_click_does_what_the_window_declared() {
        let st = |maximised, fullscreen| WindowState { maximised, fullscreen };
        // The default: maximise in place, again restores.
        assert_eq!(caption_double_click(drag_double::MAXIMISE, st(false, false)), WindowAction::Maximise(true));
        assert_eq!(caption_double_click(drag_double::MAXIMISE, st(true, false)), WindowAction::Maximise(false));
        // A presenter's output: fullscreen on its screen, again back.
        assert_eq!(caption_double_click(drag_double::FULLSCREEN, st(false, false)), WindowAction::EnterFullscreen);
        assert_eq!(caption_double_click(drag_double::FULLSCREEN, st(true, false)), WindowAction::EnterFullscreen);
        assert_eq!(caption_double_click(drag_double::FULLSCREEN, st(false, true)), WindowAction::LeaveFullscreen);
        assert_eq!(caption_double_click(drag_double::MAXIMISE, st(false, true)), WindowAction::LeaveFullscreen);
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
