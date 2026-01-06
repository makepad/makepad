use crate::event::game_input::GameInputState;

pub trait CxGameInputApi {
    fn game_input_state(&mut self, index: usize) -> Option<&GameInputState>;
    fn game_input_states(&mut self) -> &[GameInputState];
}
