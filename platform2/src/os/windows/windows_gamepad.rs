use {
    crate::{
        event::{
            gamepad::*,
        },
        makepad_live_id::*,
        makepad_math::Vec2,
        windows::Win32::UI::Input::XboxController::*,
    }
};

pub struct WindowsGamepad {
    pub gamepads: Vec<GamepadInfo>,
    pub states: Vec<GamepadState>,
}

impl WindowsGamepad {
    pub fn new() -> Self {
        Self {
            gamepads: Vec::new(),
            states: Vec::new(),
        }
    }

    pub fn init() -> Self {
        Self::new()
    }

    pub fn poll<F>(&mut self, mut callback: F)
    where
        F: FnMut(GamepadConnectedEvent),
    {
        for i in 0..4 {
            let mut state = XINPUT_STATE::default();
            let result = unsafe { XInputGetState(i, &mut state) };
            
            // Construct a stable ID for this XInput slot
            // XInput doesn't give us a unique serial number, so we use the index.
            // This matches how XInput works (controller in slot 0 is always slot 0).
            let id = LiveId(i as u64);

            if result == 0 { // ERROR_SUCCESS
                // Connected
                let info = GamepadInfo {
                    id,
                    name: format!("Xbox Controller {}", i + 1),
                };

                // Check if we already know about this gamepad
                let index = self.gamepads.iter().position(|g| g.id == id);
                
                if index.is_none() {
                    // New connection
                    self.gamepads.push(info.clone());
                    self.states.push(GamepadState::default());
                    callback(GamepadConnectedEvent::Connected(info));
                }
                
                // Update state
                if let Some(index) = self.gamepads.iter().position(|g| g.id == id) {
                    let gp_state = &mut self.states[index];
                    let x_state = state.Gamepad;
                    
                    // Buttons
                    let z =  windows::Win32::UI::Input::XboxController::XINPUT_GAMEPAD_BUTTON_FLAGS(0);
                    gp_state.dpad_up = if (x_state.wButtons & XINPUT_GAMEPAD_DPAD_UP) != z { 1.0 } else { 0.0 };
                    gp_state.dpad_down = if (x_state.wButtons & XINPUT_GAMEPAD_DPAD_DOWN) != z { 1.0 } else { 0.0 };
                    gp_state.dpad_left = if (x_state.wButtons & XINPUT_GAMEPAD_DPAD_LEFT) != z { 1.0 } else { 0.0 };
                    gp_state.dpad_right = if (x_state.wButtons & XINPUT_GAMEPAD_DPAD_RIGHT) != z { 1.0 } else { 0.0 };
                    
                    gp_state.start = if (x_state.wButtons & XINPUT_GAMEPAD_START) != z { 1.0 } else { 0.0 };
                    gp_state.select = if (x_state.wButtons & XINPUT_GAMEPAD_BACK) != z { 1.0 } else { 0.0 };
                    
                    gp_state.left_thumb = if (x_state.wButtons & XINPUT_GAMEPAD_LEFT_THUMB) != z { 1.0 } else { 0.0 };
                    gp_state.right_thumb = if (x_state.wButtons & XINPUT_GAMEPAD_RIGHT_THUMB) != z { 1.0 } else { 0.0 };
                    
                    gp_state.left_shoulder = if (x_state.wButtons & XINPUT_GAMEPAD_LEFT_SHOULDER) != z { 1.0 } else { 0.0 };
                    gp_state.right_shoulder = if (x_state.wButtons & XINPUT_GAMEPAD_RIGHT_SHOULDER) != z { 1.0 } else { 0.0 };
                    
                    gp_state.a = if (x_state.wButtons & XINPUT_GAMEPAD_A) != z { 1.0 } else { 0.0 };
                    gp_state.b = if (x_state.wButtons & XINPUT_GAMEPAD_B) != z { 1.0 } else { 0.0 };
                    gp_state.x = if (x_state.wButtons & XINPUT_GAMEPAD_X) != z { 1.0 } else { 0.0 };
                    gp_state.y = if (x_state.wButtons & XINPUT_GAMEPAD_Y) != z { 1.0 } else { 0.0 };
                    
                    // Triggers (0-255 -> 0.0-1.0)
                    gp_state.left_trigger = x_state.bLeftTrigger as f32 / 255.0;
                    gp_state.right_trigger = x_state.bRightTrigger as f32 / 255.0;
                    
                    // Thumbsticks (-32768 to 32767 -> -1.0 to 1.0)
                    // Apply deadzone handling if needed, but for raw state we often just normalize
                    
                    fn normalize_axis(val: i16) -> f32 {
                         let ret = val as f32 / 32768.0;
                         ret
                    }

                    gp_state.left_stick = Vec2 {
                        x: normalize_axis(x_state.sThumbLX),
                        y: normalize_axis(x_state.sThumbLY),
                    };
                    
                    gp_state.right_stick = Vec2 {
                        x: normalize_axis(x_state.sThumbRX),
                        y: normalize_axis(x_state.sThumbRY),
                    };
                }
            } else {
                 // Disconnected
                 // If we had it, remove it
                 if let Some(index) = self.gamepads.iter().position(|g| g.id == id) {
                     let info = self.gamepads[index].clone();
                     self.gamepads.remove(index);
                     self.states.remove(index);
                     callback(GamepadConnectedEvent::Disconnected(info));
                 }
            }
        }
    }
}
