//! What one window holds down on the desktop it drives: the keys (modifiers too) and pointer
//! buttons whose down went out and whose up has not. When the window stops driving that
//! desktop (it lost the focus, its tab changed, the history paused it, it closed, the app quit)
//! the ups go out for all of them, so nothing stays pressed on the other side: no stuck Cmd
//! after a Cmd-Tab, no game that keeps driving.

use makepad_desk_link::{self as proto, ClientMsg};
use makepad_widgets::KeyCode;

#[derive(Debug, Default)]
pub struct Held {
    /// The connection the downs went to and its session (`Connection::session`): a session that
    /// ended took its downs with it (the desktop let them go).
    pub on: Option<(usize, u64)>,
    /// Keys (makepad `KeyCode` index), in the order they went down.
    keys: Vec<u32>,
    /// Buttons, a bit per `proto::button`.
    buttons: u8,
}

/// The `proto::mods` bit of a modifier key (0: not one).
fn modifier(keycode: u32) -> u8 {
    match KeyCode::from_index(keycode) {
        KeyCode::Shift => proto::mods::SHIFT,
        KeyCode::Control => proto::mods::CONTROL,
        KeyCode::Alt => proto::mods::ALT,
        KeyCode::Logo => proto::mods::LOGO,
        _ => 0,
    }
}

impl Held {
    pub fn key_down(&mut self, keycode: u32) {
        if !self.keys.contains(&keycode) {
            self.keys.push(keycode);
        }
    }

    /// The key went up: whether its down had gone out (only then does its up).
    pub fn key_up(&mut self, keycode: u32) -> bool {
        let before = self.keys.len();
        self.keys.retain(|k| *k != keycode);
        self.keys.len() != before
    }

    pub fn button_down(&mut self, button: u8) {
        if button < 8 {
            self.buttons |= 1 << button;
        }
    }

    /// The button went up: whether its down had gone out.
    pub fn button_up(&mut self, button: u8) -> bool {
        let held = button < 8 && self.buttons & (1 << button) != 0;
        if held {
            self.buttons &= !(1 << button);
        }
        held
    }

    /// Any button held (a drag keeps driving the picture past its edge).
    pub fn dragging(&self) -> bool {
        self.buttons != 0
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty() && self.buttons == 0
    }

    /// The ups for everything held, and nothing held any more (`on` forgotten too): buttons
    /// first, then the other keys, newest first, with the modifiers still down, then the
    /// modifiers themselves, so the desktop never sees a key go up under modifiers it no
    /// longer has.
    pub fn release(&mut self) -> Vec<ClientMsg> {
        let mut ups = Vec::new();
        for b in 0..8u8 {
            if self.buttons & (1 << b) != 0 {
                ups.push(ClientMsg::Button { down: false, button: b });
            }
        }
        let keys = std::mem::take(&mut self.keys);
        let mut mods = keys.iter().fold(0u8, |m, k| m | modifier(*k));
        for k in keys.iter().rev().filter(|k| modifier(**k) == 0) {
            ups.push(ClientMsg::Key { down: false, keycode: *k, mods });
        }
        for k in keys.iter().rev().filter(|k| modifier(**k) != 0) {
            mods &= !modifier(*k);
            ups.push(ClientMsg::Key { down: false, keycode: *k, mods });
        }
        self.buttons = 0;
        self.on = None;
        ups
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(k: KeyCode) -> u32 {
        k.to_index()
    }

    fn up(k: KeyCode, mods: u8) -> ClientMsg {
        ClientMsg::Key { down: false, keycode: code(k), mods }
    }

    #[test]
    fn release_lets_go_of_everything_once() {
        let mut h = Held::default();
        h.on = Some((1, 7));
        h.key_down(code(KeyCode::KeyW));
        h.key_down(code(KeyCode::KeyW)); // a repeat: still one key
        h.button_down(proto::button::LEFT);
        h.button_down(proto::button::RIGHT);
        assert!(h.dragging());
        let ups = h.release();
        assert_eq!(ups, vec![
            ClientMsg::Button { down: false, button: proto::button::LEFT },
            ClientMsg::Button { down: false, button: proto::button::RIGHT },
            up(KeyCode::KeyW, 0),
        ]);
        assert!(h.is_empty() && h.on.is_none() && !h.dragging());
        assert!(h.release().is_empty(), "a second release sends nothing");
    }

    #[test]
    fn modifiers_go_up_last_and_the_mods_shrink() {
        // Cmd-Tab with Shift: Shift, Cmd, then Tab went down; the window lost the focus
        let mut h = Held::default();
        h.key_down(code(KeyCode::Shift));
        h.key_down(code(KeyCode::Logo));
        h.key_down(code(KeyCode::Tab));
        let ups = h.release();
        let both = proto::mods::SHIFT | proto::mods::LOGO;
        assert_eq!(ups, vec![up(KeyCode::Tab, both), up(KeyCode::Logo, proto::mods::SHIFT), up(KeyCode::Shift, 0)]);
    }

    #[test]
    fn an_up_goes_out_only_after_its_down() {
        let mut h = Held::default();
        // pressed before the window had the keyboard: its up is not the desktop's
        assert!(!h.key_up(code(KeyCode::KeyA)));
        assert!(!h.button_up(proto::button::LEFT));
        h.key_down(code(KeyCode::KeyA));
        h.button_down(proto::button::MIDDLE);
        assert!(h.key_up(code(KeyCode::KeyA)));
        assert!(!h.key_up(code(KeyCode::KeyA)), "one up per down");
        assert!(h.button_up(proto::button::MIDDLE));
        assert!(h.is_empty());
        // released on a focus loss: the late up from the OS is dropped
        h.key_down(code(KeyCode::Control));
        assert_eq!(h.release().len(), 1);
        assert!(!h.key_up(code(KeyCode::Control)));
    }

    #[test]
    fn out_of_range_buttons_are_ignored() {
        let mut h = Held::default();
        h.button_down(9);
        assert!(h.is_empty());
        assert!(!h.button_up(9));
    }
}
