//! Session-0 pointer gestures. The renderer supplies the displayed hit rectangles;
//! this host maps a completed tap to the same actions as the keyboard.
use makepad_hal::{buffers::read_items, input::touch, HalEvent, TouchPoint};
use relax_boot::{Action, Boot};

struct Press {
    finger: Option<u32>,
    start: [f32; 2],
    last: [f32; 2],
    action: Option<Action>,
    revision: u64,
    scroll: bool,
    dragged: bool,
}

#[derive(Default)]
pub struct Input {
    press: Option<Press>,
}

impl Input {
    pub fn cancel(&mut self, host: &mut Boot) {
        self.press = None;
        host.press(None);
    }

    fn down(&mut self, host: &mut Boot, finger: Option<u32>, point: [f32; 2]) {
        if self.press.is_some() {
            self.cancel(host);
            return;
        }
        let action = host.hit_test(point);
        self.press = Some(Press {
            finger,
            start: point,
            last: point,
            action,
            revision: host.layout_revision(),
            scroll: host.scrollable_at(point),
            dragged: false,
        });
        host.press(action);
    }

    fn moved(&mut self, host: &mut Boot, finger: Option<u32>, point: [f32; 2]) {
        if finger.is_none() {
            host.hover(Some(point));
        }
        let Some(press) = &mut self.press else { return };
        if press.finger != finger {
            return;
        }
        let delta = [point[0] - press.start[0], point[1] - press.start[1]];
        // Gesture slop is in logical dp, just like the event positions.
        if delta[0] * delta[0] + delta[1] * delta[1] > 8.0 * 8.0 {
            press.dragged = true;
        }
        if press.revision != host.layout_revision() {
            press.action = None;
        }
        if press.dragged {
            host.press(None);
            if press.scroll {
                host.scroll_points(press.last[1] - point[1]);
            }
        } else {
            let action = press.action.filter(|a| host.hit_test(point) == Some(*a));
            host.press(action);
        }
        press.last = point;
    }

    fn up(&mut self, host: &mut Boot, finger: Option<u32>, point: [f32; 2]) -> Option<Action> {
        self.moved(host, finger, point);
        if self.press.as_ref()?.finger != finger {
            return None;
        }
        let press = self.press.take()?;
        host.press(None);
        if press.dragged || press.revision != host.layout_revision() {
            return None;
        }
        press
            .action
            .filter(|action| host.hit_test(point) == Some(*action))
    }

    pub fn event(&mut self, host: &mut Boot, event: HalEvent<'_>) -> Option<Action> {
        match event {
            HalEvent::MouseDown {
                x, y, button: 1, ..
            } => self.down(host, None, [x as f32, y as f32]),
            HalEvent::MouseMove { x, y, .. } => self.moved(host, None, [x as f32, y as f32]),
            HalEvent::MouseUp {
                x, y, button: 1, ..
            } => return self.up(host, None, [x as f32, y as f32]),
            HalEvent::MouseLeave { .. } | HalEvent::Focus { focused: false, .. } => {
                host.hover(None);
                self.cancel(host)
            }
            HalEvent::Scroll { x, y, dy, .. } => {
                self.cancel(host);
                if host.scrollable_at([x as f32, y as f32]) {
                    host.scroll_points(dy as f32);
                }
            }
            HalEvent::Touch { count, touches, .. } => {
                // A second contact cancels the pending tap. Never activate more
                // than one action from a multi-touch packet.
                if read_items::<TouchPoint>(touches, count)
                    .filter(|p| p.phase != touch::STOP)
                    .count()
                    > 1
                {
                    self.cancel(host);
                    return None;
                }
                for point in read_items::<TouchPoint>(touches, count) {
                    let pos = [point.x, point.y];
                    match point.phase {
                        touch::START => self.down(host, Some(point.id), pos),
                        touch::MOVE => self.moved(host, Some(point.id), pos),
                        touch::STOP => {
                            if let Some(action) = self.up(host, Some(point.id), pos) {
                                return Some(action);
                            }
                        }
                        touch::STABLE => {}
                        _ => self.cancel(host),
                    }
                }
            }
            _ => {}
        }
        None
    }
}
