use crate::event::gamepad::GamepadState;

pub trait CxGamepadApi {
    fn gamepad_state(&mut self, index: usize) -> Option<&GamepadState>;
}

