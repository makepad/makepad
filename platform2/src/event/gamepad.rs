use crate::makepad_live_id::*;
use std::sync::mpsc::{channel, Sender, Receiver};
use crate::makepad_math::Vec2;

#[derive(Clone, Debug, PartialEq)]
pub enum GamepadConnectedEvent {
    Connected(GamepadInfo),
    Disconnected(GamepadInfo)
}

#[derive(Clone, Debug, PartialEq)]
pub struct GamepadInfo{
    pub id: LiveId,
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GamepadState {
    pub a: f32,
    pub b: f32,
    pub x: f32,
    pub y: f32,
    
    pub left_shoulder: f32,
    pub right_shoulder: f32,
    pub left_trigger: f32,
    pub right_trigger: f32,
    
    pub select: f32,
    pub start: f32,
    pub left_thumb: f32,
    pub right_thumb: f32,
    
    pub dpad_up: f32,
    pub dpad_down: f32,
    pub dpad_left: f32,
    pub dpad_right: f32,
    
    pub left_stick: Vec2,
    pub right_stick: Vec2,
}

pub struct GamepadEventChannel {
    pub sender: Sender<GamepadConnectedEvent>,
    pub receiver: Receiver<GamepadConnectedEvent>,
}

impl Default for GamepadEventChannel {
    fn default() -> Self {
        let (sender, receiver) = channel();
        Self {
            sender,
            receiver
        }
    }
}
