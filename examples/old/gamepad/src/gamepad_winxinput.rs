#![allow(non_snake_case)]
#![allow(non_camel_case_types)]
#![allow(non_upper_case_globals)]

use winapi::shared::minwindef::{DWORD, TRUE};
use winapi::um::xinput;
use render::*;

pub const GamepadButtonDpadUp:u32 = 0x1;
pub const GamepadButtonDpadDown:u32 = 0x2;
pub const GamepadButtonDpadLeft:u32 = 0x4;
pub const GamepadButtonDpadRight:u32 = 0x8;
pub const GamepadButtonStart:u32 = 0x10;
pub const GamepadButtonBack:u32 = 0x20;
pub const GamepadButtonLeftThumb:u32 = 0x40;
pub const GamepadButtonRightThumb:u32 = 0x80;
pub const GamepadButtonLeftShoulder:u32 = 0x100;
pub const GamepadButtonRightShoulder:u32 = 0x200;
pub const GamepadButtonA:u32 = 0x1000;
pub const GamepadButtonB:u32 = 0x2000;
pub const GamepadButtonX:u32 = 0x4000;
pub const GamepadButtonY:u32 = 0x8000;

#[derive(Default)]
pub struct Gamepad {
    pub initialized: bool,
    pub thumb_deadzone: f32,
    pub player: usize,
    pub packet_number: u64,
    pub buttons: u32,
    pub last_buttons: u32, 
    pub buttons_up_edge: u32,
    pub buttons_down_edge: u32,
    pub left_trigger: f32,
    pub right_trigger: f32,
    pub left_thumb: Vec2,
    pub right_thumb: Vec2,
    pub platform: Option<GamepadPlatform>
}

pub struct GamepadPlatform {
}

impl Gamepad {
    
    pub fn init(&mut self, player: usize, thumb_deadzone:f32) -> bool {
        self.thumb_deadzone = thumb_deadzone;
        self.player = player;
        self.lazy_init();
        return true;
    }
    
    fn lazy_init(&mut self){
        if!self.initialized{ 
            unsafe{xinput::XInputEnable(TRUE)};
            self.initialized = true;
        }
    }
    
    pub fn poll(&mut self) {
        unsafe{
            self.lazy_init();
            let mut state = std::mem::uninitialized();
            xinput::XInputGetState(self.player as DWORD, &mut state);
            self.packet_number = state.dwPacketNumber as u64;
            self.last_buttons = self.buttons;
            self.buttons = state.Gamepad.wButtons as u32;
            self.buttons_down_edge = self.buttons & !self.last_buttons;
            self.buttons_up_edge = !self.buttons & self.last_buttons;
            self.left_trigger = state.Gamepad.bLeftTrigger as f32 / 255.0;
            self.right_trigger = state.Gamepad.bRightTrigger  as f32 / 255.0;
            fn thumb_map(inp:i16, deadzone:f32)->f32{
                if inp == -32768{return -1.0;}
                let ret = inp as f32 / 32767.0;
                if ret.abs() < deadzone{
                    return 0.0;
                }
                return ret;
            }
            self.left_thumb = Vec2{
                x:thumb_map(state.Gamepad.sThumbLX, self.thumb_deadzone),
                y:thumb_map(state.Gamepad.sThumbLY, self.thumb_deadzone)
            };
            self.right_thumb = Vec2{
                x:thumb_map(state.Gamepad.sThumbRX, self.thumb_deadzone),
                y:thumb_map(state.Gamepad.sThumbRY, self.thumb_deadzone)
            };
        }
    }
}
