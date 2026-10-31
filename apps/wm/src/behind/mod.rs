//! Drag from behind. A press on a window that is not in front goes to that
//! client at once (so it can start its own drag-and-drop source) but does
//! not raise or focus it. The raise happens on button-up, and only when the
//! pointer stayed within [`CLICK_SLOP`] of where it went down. The keyboard
//! stays with the previous window until then. A title-bar press never comes
//! through here: it begins a window drag, which raises on down.
use makepad_app_host::ClientId;
use crate::App;
use makepad_widgets::*;

/// A press that moves less than this (logical px) is a click.
pub const CLICK_SLOP: f64 = 4.0;

pub fn is_click(down: Vec2d, now: Vec2d) -> bool {
    (now.x - down.x).abs() < CLICK_SLOP && (now.y - down.y).abs() < CLICK_SLOP
}

struct Pending {
    client: ClientId,
    down: Vec2d,
    moved: bool,
}

#[derive(Default)]
pub struct Behind {
    /// Where the mouse button went down, until it lifts.
    press: Option<Vec2d>,
    pending: Option<Pending>,
}

impl App {
    /// Every pointer event, before the tiles see it.
    pub(crate) fn behind_observe(&mut self, cx: &mut Cx, event: &Event) {
        match event {
            Event::MouseDown(e) => {
                let b = &mut self.state_mut().behind;
                b.press = Some(e.abs);
                b.pending = None;
            }
            Event::MouseMove(e) => {
                if let Some(p) = self.state_mut().behind.pending.as_mut() {
                    p.moved |= !is_click(p.down, e.abs);
                }
            }
            Event::MouseUp(e) => {
                let b = &mut self.state_mut().behind;
                b.press = None;
                if let Some(p) = b.pending.take() {
                    if !p.moved && is_click(p.down, e.abs) {
                        self.focus_client(cx, p.client);
                    }
                }
            }
            _ => {}
        }
    }

    /// A tile reports a press on `client`. True when the raise is deferred
    /// to button-up (the caller then does nothing).
    pub(crate) fn behind_deferred(&mut self, cx: &mut Cx, client: ClientId) -> bool {
        if self.ai_bus.is_pane(client) {
            return false;
        }
        let state = self.state_mut();
        let Some(down) = state.behind.press else { return false };
        let front = state.layout().focused_client();
        if front == Some(client) || front.is_none() {
            return false;
        }
        state.behind.pending = Some(Pending { client, down, moved: false });
        // The tile took the keyboard on the press; give it back.
        if let Some(front) = front {
            self.desk(cx).borrow_mut::<crate::desk::WmDesk>().map(|mut d| d.with_tile(cx, front, |cx, v| v.focus_keyboard(cx)));
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_click_stays_under_four_pixels() {
        let d = dvec2(100.0, 100.0);
        assert!(is_click(d, d));
        assert!(is_click(d, dvec2(103.0, 97.5)));
        assert!(!is_click(d, dvec2(104.0, 100.0)));
        assert!(!is_click(d, dvec2(100.0, 95.0)));
    }
}
