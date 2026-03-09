use crate::{cx::Cx, event::game_input::GameInputState};

pub trait CxGameInputApi {
    fn game_input_state(&mut self, index: usize) -> Option<&GameInputState>;
    fn game_input_state_mut(&mut self, index: usize) -> Option<&mut GameInputState>;
    fn game_input_states(&mut self) -> &[GameInputState];
    fn game_input_states_mut(&mut self) -> &mut [GameInputState];
}

#[cfg(not(any(
    target_os = "windows",
    target_os = "macos",
    target_os = "ios",
    target_os = "tvos"
)))]
impl CxGameInputApi for Cx {
    fn game_input_state(&mut self, _index: usize) -> Option<&GameInputState> {
        None
    }

    fn game_input_state_mut(&mut self, _index: usize) -> Option<&mut GameInputState> {
        None
    }

    fn game_input_states(&mut self) -> &[GameInputState] {
        &[]
    }

    fn game_input_states_mut(&mut self) -> &mut [GameInputState] {
        &mut []
    }
}
