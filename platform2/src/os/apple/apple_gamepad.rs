use {
    crate::{
        makepad_live_id::*,
        makepad_objc_sys::{
            objc_block,
            runtime::{ObjcId, nil, Sel, BOOL, YES},
            msg_send,
            sel,
            sel_impl,
            class,
        },
        event::{
            gamepad::*,
        },
        gamepad::CxGamepadApi,
        cx::Cx,
        os::{
            apple::apple_sys::*,
            apple::apple_util::*,
        },
        makepad_math::Vec2,
    }
};

pub struct AppleGamepad {
    pub gamepads: Vec<GamepadInfo>,
    pub controllers: Vec<ObjcId>,
    pub states: Vec<GamepadState>,
}

impl AppleGamepad {
    pub fn new() -> Self {
        Self {
            gamepads: Vec::new(),
            controllers: Vec::new(),
            states: Vec::new(),
        }
    }

    pub fn init<F>(callback: F) -> Self
    where
        F: Fn(GamepadConnectedEvent) + 'static + Clone,
    {
        unsafe {
             // Enable background monitoring for macOS 11.3+
             // GCController.shouldMonitorBackgroundEvents = true
             // This is a class property
             let gc_controller_class = class!(GCController);
             let sel_monitor = Sel::register("setShouldMonitorBackgroundEvents:");
             if msg_send![gc_controller_class, respondsToSelector: sel_monitor] {
                 let () = msg_send![gc_controller_class, setShouldMonitorBackgroundEvents: YES];
             }

            let center: ObjcId = msg_send![class!(NSNotificationCenter), defaultCenter];
            let callback_clone = callback.clone();
            
            let block = objc_block!(move | note: ObjcId | {
                let controller: ObjcId = msg_send![note, object];
                let _: ObjcId = msg_send![controller, retain];
                let vendor_name: ObjcId = msg_send![controller, vendorName];
                let name = nsstring_to_string(vendor_name);
                
                let ptr = controller as u64;
                let id = LiveId(ptr); 
                
                let info = GamepadInfo {
                    id,
                    name,
                };
                callback_clone(GamepadConnectedEvent::Connected(info));
            });
            
            let () = msg_send![center, addObserverForName: GCControllerDidConnectNotification object: nil queue: nil usingBlock: block];

            let callback_clone = callback.clone();
            let block = objc_block!(move | note: ObjcId | {
                let controller: ObjcId = msg_send![note, object];
                let _: () = msg_send![controller, release];
                let vendor_name: ObjcId = msg_send![controller, vendorName];
                let name = nsstring_to_string(vendor_name);
                
                let ptr = controller as u64;
                let id = LiveId(ptr); 
                
                let info = GamepadInfo {
                    id,
                    name,
                };
                callback_clone(GamepadConnectedEvent::Disconnected(info));
            });
             let () = msg_send![center, addObserverForName: GCControllerDidDisconnectNotification object: nil queue: nil usingBlock: block];

        }
        
        Self::new()
    }

    pub fn on_connected(&mut self, info: &GamepadInfo) {
        let ptr = info.id.0 as ObjcId;
        self.gamepads.push(info.clone());
        self.controllers.push(ptr);
        self.states.push(GamepadState::default());
        crate::log!("Gamepad connected: {}", info.name);
    }

    pub fn on_disconnected(&mut self, info: &GamepadInfo) {
        if let Some(index) = self.gamepads.iter().position(|g| g.id == info.id) {
            self.gamepads.remove(index);
            self.controllers.remove(index);
            self.states.remove(index);
            crate::log!("Gamepad disconnected: {}", info.name);
        }
    }

    pub fn poll(&mut self) {
        for (i, controller) in self.controllers.iter().enumerate() {
            unsafe {
                let extended_gamepad: ObjcId = msg_send![*controller, extendedGamepad];
                if extended_gamepad != nil {
                    let state = &mut self.states[i];
                    
                    let get_val = |btn: ObjcId| -> f32 {
                        if btn != nil {
                            let val: f32 = msg_send![btn, value];
                            if val == 0.0 {
                                let pressed: BOOL = msg_send![btn, isPressed];
                                if pressed == YES {
                                    return 1.0;
                                }
                            }
                            val
                        } else {
                            0.0
                        }
                    };

                    let get_axis = |input: ObjcId| -> f32 {
                         if input != nil {
                             let val: f32 = msg_send![input, value];
                             val
                         } else { 0.0 }
                    };
                    
                    state.a = get_val(msg_send![extended_gamepad, buttonA]);
                    state.b = get_val(msg_send![extended_gamepad, buttonB]);
                    state.x = get_val(msg_send![extended_gamepad, buttonX]);
                    state.y = get_val(msg_send![extended_gamepad, buttonY]);
                    
                    state.left_shoulder = get_val(msg_send![extended_gamepad, leftShoulder]);
                    state.right_shoulder = get_val(msg_send![extended_gamepad, rightShoulder]);
                    
                    state.left_trigger = get_val(msg_send![extended_gamepad, leftTrigger]);
                    state.right_trigger = get_val(msg_send![extended_gamepad, rightTrigger]);
                    
                    state.select = get_val(msg_send![extended_gamepad, buttonOptions]);
                    state.start = get_val(msg_send![extended_gamepad, buttonMenu]);
                    
                    state.left_thumb = get_val(msg_send![extended_gamepad, leftThumbstickButton]);
                    state.right_thumb = get_val(msg_send![extended_gamepad, rightThumbstickButton]);
                    
                    let dpad: ObjcId = msg_send![extended_gamepad, dpad];
                    if dpad != nil {
                        state.dpad_up = get_axis(msg_send![dpad, up]);
                        state.dpad_down = get_axis(msg_send![dpad, down]);
                        state.dpad_left = get_axis(msg_send![dpad, left]);
                        state.dpad_right = get_axis(msg_send![dpad, right]);
                    }
                    
                    let left_stick: ObjcId = msg_send![extended_gamepad, leftThumbstick];
                    if left_stick != nil {
                        state.left_stick = Vec2 {
                            x: get_axis(msg_send![left_stick, xAxis]),
                            y: get_axis(msg_send![left_stick, yAxis]),
                        };
                    }
                    
                    let right_stick: ObjcId = msg_send![extended_gamepad, rightThumbstick];
                    if right_stick != nil {
                        state.right_stick = Vec2 {
                            x: get_axis(msg_send![right_stick, xAxis]),
                            y: get_axis(msg_send![right_stick, yAxis]),
                        };
                    }
                }
            }
        }
    }
}

impl CxGamepadApi for Cx {
    fn gamepad_state(&mut self, index: usize) -> Option<&GamepadState> {
         if let Some(gamepad) = &self.os.apple_gamepad {
             if index < gamepad.states.len() {
                 return Some(&gamepad.states[index]);
             }
         }
         None
    }
}
