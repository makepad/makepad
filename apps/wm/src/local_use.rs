//! Local use of this machine, as the WM sees it, for its AI hub's admission. On a node the
//! WM owns the screen, so no OS probe sees its games: the WM tells its hub (`POST
//! /control/local-use`, through `hub_link`). In the Automatic compute mode the hub then
//! closes admission (the fleet routes jobs to other nodes), cancels running jobs as
//! `local-use: gaming` so their clients reroute, and unloads its models.
//!
//! A game is a game app in focus (the Sandbox), a fullscreen space, or gamepad input. Play
//! ends once none has been seen for [`REOPEN_IDLE_S`]: a pause between rounds does not
//! reload a 27B model.
//! Plain state over tick times, so it is tested without a hub, a GPU or a pad.

/// Seconds without a game signal before the hub takes compute jobs again.
pub const REOPEN_IDLE_S: f64 = 120.0;

/// The registry ids of the apps that are games.
const GAME_APPS: &[&str] = &["sandbox"];

pub fn is_game(app: &str) -> bool {
    GAME_APPS.contains(&app)
}

#[derive(Default)]
pub struct LocalUse {
    last_signal: Option<f64>,
    /// What the last tick saw (logged when it changes).
    seen: Option<String>,
    pads: Vec<makepad_widgets::makepad_platform::event::game_input::GameInputState>,
}

impl LocalUse {
    /// A game is being played: a signal within the last [`REOPEN_IDLE_S`].
    pub fn gaming(&self, now: f64) -> bool {
        self.last_signal.is_some_and(|at| now - at < REOPEN_IDLE_S)
    }

    /// One tick: `play` (a game in focus, or a fullscreen space: what) and the pads' state now. Answers
    /// whether a game is being played. Any change of a pad's state is input; connected,
    /// untouched pads are not.
    pub fn tick(&mut self, now: f64, play: Option<String>, pads: &[makepad_widgets::makepad_platform::event::game_input::GameInputState]) -> bool {
        let pad_input = pads.len() == self.pads.len() && pads.iter().zip(&self.pads).any(|(a, b)| a != b);
        if pads != self.pads.as_slice() {
            self.pads = pads.to_vec();
        }
        if play != self.seen {
            makepad_widgets::log!("wm: local use: {}", play.as_deref().unwrap_or("no game on screen"));
            self.seen = play.clone();
        }
        if play.is_some() || pad_input {
            self.last_signal = Some(now);
        }
        self.gaming(now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_widgets::makepad_platform::event::game_input::{GameInputState, GamepadState};

    #[test]
    fn fullscreen_or_pad_input_is_play_and_it_ends_after_the_idle_threshold() {
        let mut l = LocalUse::default();
        let idle = [GameInputState::Gamepad(GamepadState::default())];
        assert!(!l.tick(0.0, None, &idle), "a connected pad nobody touches is not play");
        assert!(!l.tick(1.0, None, &idle));
        assert!(l.tick(2.0, Some("sandbox in focus".into()), &idle), "a game in focus");
        assert!(l.tick(2.0 + REOPEN_IDLE_S - 1.0, None, &idle), "a pause between rounds");
        assert!(!l.tick(2.0 + REOPEN_IDLE_S, None, &idle), "play ended");
        let pressed = [GameInputState::Gamepad(GamepadState { a: 1.0, ..Default::default() })];
        assert!(l.tick(500.0, None, &pressed), "a button press");
        assert!(l.tick(501.0, None, &pressed));
        assert!(!l.tick(501.0 + REOPEN_IDLE_S, None, &pressed), "a held, unchanged state is not new input");
        assert!(is_game("sandbox") && !is_game("terminal"));
    }
}
