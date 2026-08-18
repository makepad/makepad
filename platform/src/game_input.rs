use crate::event::game_input::GameInputState;

#[cfg(any(
    headless,
    not(any(
        target_os = "windows",
        target_os = "macos",
        target_os = "ios",
        target_os = "tvos"
    ))
))]
use crate::cx::Cx;

pub trait CxGameInputApi {
    fn game_input_state(&mut self, index: usize) -> Option<&GameInputState>;
    fn game_input_state_mut(&mut self, index: usize) -> Option<&mut GameInputState>;
    fn game_input_states(&mut self) -> &[GameInputState];
    fn game_input_states_mut(&mut self) -> &mut [GameInputState];
}

#[cfg(any(
    headless,
    not(any(
        target_os = "windows",
        target_os = "macos",
        target_os = "ios",
        target_os = "tvos"
    ))
))]
/// These platforms have no native game-input backend, but an app hosted by
/// Studio still gets controllers: Studio reads them and forwards the state, so
/// a Linux app run from Studio on a machine whose Studio can see a pad works
/// even though the same app run standalone would see nothing. The headless
/// backend also has no native game input and uses the same remote fallback.
impl CxGameInputApi for Cx {
    fn game_input_state(&mut self, index: usize) -> Option<&GameInputState> {
        self.game_input_remote.get(index)
    }

    fn game_input_state_mut(&mut self, index: usize) -> Option<&mut GameInputState> {
        self.game_input_remote.get_mut(index)
    }

    fn game_input_states(&mut self) -> &[GameInputState] {
        &self.game_input_remote
    }

    fn game_input_states_mut(&mut self) -> &mut [GameInputState] {
        &mut self.game_input_remote
    }
}
