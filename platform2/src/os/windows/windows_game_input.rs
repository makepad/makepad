use {
    crate::{
        event::{
            game_input::*,
        },
        makepad_live_id::*,
        makepad_math::Vec2,
        windows::Win32::UI::Input::XboxController::*,
        windows::Gaming::Input::{RacingWheel},
    }
};

pub struct WindowsGameInput {
    pub gamepads: Vec<GameInputInfo>,
    pub states: Vec<GameInputState>,
    pub racing_wheels: Vec<(LiveId, RacingWheel)>,
    pub next_wheel_id: u64,
}

impl WindowsGameInput {
    pub fn new() -> Self {
        Self {
            gamepads: Vec::new(),
            states: Vec::new(),
            racing_wheels: Vec::new(),
            next_wheel_id: 128,
        }
    }

    pub fn init() -> Self {
        Self::new()
    }

    pub fn poll<F>(&mut self, mut callback: F)
    where
        F: FnMut(GameInputConnectedEvent),
    {
        for i in 0..4 {
            let mut state = XINPUT_STATE::default();
            let result = unsafe { XInputGetState(i, &mut state) };
            
            // Construct a stable ID for this XInput slot
            let id = LiveId(i as u64);

            if result == 0 { // ERROR_SUCCESS
                // Connected
                let info = GameInputInfo {
                    id,
                    name: format!("Xbox Controller {}", i + 1),
                };

                // Check if we already know about this gamepad
                let index = self.gamepads.iter().position(|g| g.id == id);
                
                if index.is_none() {
                    // New connection
                    self.gamepads.push(info.clone());
                    // Default to Gamepad variant
                    self.states.push(GameInputState::Gamepad(GamepadState::default()));
                    callback(GameInputConnectedEvent::Connected(info));
                }
                
                // Update state
                if let Some(index) = self.gamepads.iter().position(|g| g.id == id) {
                     if let GameInputState::Gamepad(gp_state) = &mut self.states[index] {
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
                        fn normalize_axis(val: i16) -> f32 {
                             val as f32 / 32768.0
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
                }
            } else {
                 // Disconnected
                 // Only disconnect if it was an XInput device (id < 128)
                 if let Some(index) = self.gamepads.iter().position(|g| g.id == id) {
                     let info = self.gamepads[index].clone();
                     self.gamepads.remove(index);
                     self.states.remove(index);
                     callback(GameInputConnectedEvent::Disconnected(info));
                 }
            }
        }
        
        // Racing Wheel Polling
        if let Ok(wheels) = RacingWheel::RacingWheels() {
            if let Ok(count) = wheels.Size() {
                let mut active_wheel_ids = Vec::new();
                for i in 0..count {
                     if let Ok(wheel) = wheels.GetAt(i) {
                         // Find if we have this wheel
                         let mut existing_id = None;
                         for (id, w) in &self.racing_wheels {
                             if w == &wheel {
                                 existing_id = Some(*id);
                                 break;
                             }
                         }
                         
                         let id = if let Some(id) = existing_id {
                             id
                         } else {
                             // New wheel
                             let id_val = self.next_wheel_id;
                             self.next_wheel_id += 1;
                             let new_id = LiveId(id_val);
                             
                             self.racing_wheels.push((new_id, wheel.clone()));
                             
                             let info = GameInputInfo {
                                 id: new_id,
                                 name: "Racing Wheel".to_string(), 
                             };
                             self.gamepads.push(info.clone());
                             self.states.push(GameInputState::Wheel(WheelState::default()));
                             callback(GameInputConnectedEvent::Connected(info));
                             
                             new_id
                         };
                         
                         active_wheel_ids.push(id);
                         
                         if let Ok(reading) = wheel.GetCurrentReading() {
                             if let Some(index) = self.gamepads.iter().position(|g| g.id == id) {
                                  if let GameInputState::Wheel(wh_state) = &mut self.states[index] {
                                      wh_state.steering = reading.Wheel as f32;
                                      wh_state.throttle = reading.Throttle as f32;
                                      wh_state.brake = reading.Brake as f32;
                                      wh_state.clutch = reading.Clutch as f32; 
                                  }
                             }
                         }
                     }
                }
                
                // Cleanup disconnected wheels
                let mut i = 0;
                while i < self.racing_wheels.len() {
                    let (id, _) = self.racing_wheels[i];
                    if !active_wheel_ids.contains(&id) {
                         self.racing_wheels.remove(i);
                         if let Some(index) = self.gamepads.iter().position(|g| g.id == id) {
                             let info = self.gamepads[index].clone();
                             self.gamepads.remove(index);
                             self.states.remove(index);
                             callback(GameInputConnectedEvent::Disconnected(info));
                         }
                    } else {
                        i += 1;
                    }
                }
            }
        }
    }
}
