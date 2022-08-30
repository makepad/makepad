#![allow(dead_code)]
use {
    std::cell::Cell,
    crate::{
        cx::Cx,
        makepad_live_id::*,
        makepad_micro_serde::*,
        makepad_math::{DVec2},
        window::CxWindowPool,
        area::Area,
        event::{
            DigitInfo,
            DigitDevice,
            CxFingers,
            KeyModifiers,
            FingerDownEvent,
            FingerUpEvent,
            FingerMoveEvent,
        }
    }
};

#[derive(Clone, Copy, Debug, Default, SerBin, DeBin, SerJson, DeJson, PartialEq)]
pub struct StdinWindowSize{
    pub width: f64,
    pub height: f64,
    pub dpi_factor: f64,
}

#[derive(Clone, Copy, Debug, Default, SerBin, DeBin, SerJson, DeJson, PartialEq)]
pub struct StdinFingerDown{
   pub time: f64,
   pub digit_id: u64,
   pub mouse_button: Option<usize>,
   pub x: f64,
   pub y: f64
}

#[derive(Clone, Copy, Debug, Default, SerBin, DeBin, SerJson, DeJson, PartialEq)]
pub struct StdinFingerMove{
   pub time: f64,
   pub mouse_button: Option<usize>,
   pub digit_id: u64,
   pub x: f64,
   pub y: f64
}


#[derive(Clone, Copy, Debug, Default, SerBin, DeBin, SerJson, DeJson, PartialEq)]
pub struct StdinFingerUp{
   pub time: f64,
   pub mouse_button: Option<usize>,
   pub digit_id: u64,
   pub x: f64,
   pub y: f64
}

impl StdinFingerDown {
    pub fn into_finger_down_event(self, fingers: &CxFingers) -> FingerDownEvent {
        let digit_id = LiveId(self.digit_id).into();
        FingerDownEvent {
            window_id: CxWindowPool::id_zero(),
            abs: DVec2 {x: self.x, y: self.y},
            handled: Cell::new(Area::Empty),
            digit: DigitInfo {
                id: digit_id,
                index: fingers.get_digit_index(digit_id),
                count: fingers.get_digit_count(),
                device: if let Some(mb) = self.mouse_button{
                    DigitDevice::Mouse(mb)
                } else{
                    DigitDevice::Touch(self.digit_id)
                }
            },
            sweep_lock: Cell::new(Area::Empty),
            modifiers: KeyModifiers::default(),
            time: self.time,
            tap_count: fingers.get_tap_count(digit_id)
        }
    }
}

impl StdinFingerMove {
    pub fn into_finger_move_event(self, fingers: &CxFingers) -> FingerMoveEvent {
        let digit_id = LiveId(self.digit_id).into();
        FingerMoveEvent {
            window_id: CxWindowPool::id_zero(),
            abs: DVec2 {x: self.x, y: self.y},
            tap_count: fingers.get_tap_count(digit_id),
            handled: Cell::new(Area::Empty),
            sweep_lock: Cell::new(Area::Empty),
            digit: DigitInfo {
                id: digit_id,
                index: fingers.get_digit_index(digit_id),
                count: fingers.get_digit_count(),
                device: if let Some(mb) = self.mouse_button{
                    DigitDevice::Mouse(mb)
                } else{
                    DigitDevice::Touch(self.digit_id)
                }
            },
            hover_last: fingers.get_hover_area(digit_id),
            //captured: fingers.get_captured_area(digit_id),
            modifiers: KeyModifiers::default(),
            time: self.time,
        }
    }
}


impl StdinFingerUp {
    pub fn into_finger_up_event(self, fingers: &CxFingers) -> FingerUpEvent {
        let digit_id = LiveId(self.digit_id).into();
        FingerUpEvent {
            window_id: CxWindowPool::id_zero(),
            abs: DVec2 {x: self.x, y: self.y},
            tap_count: fingers.get_tap_count(digit_id),
            digit: DigitInfo {
                id: digit_id,
                index: fingers.get_digit_index(digit_id),
                count: fingers.get_digit_count(),
                device: if let Some(mb) = self.mouse_button{
                    DigitDevice::Mouse(mb)
                } else{
                    DigitDevice::Touch(self.digit_id)
                }
            },
            capture_time: fingers.get_capture_time(digit_id),
            captured: fingers.get_captured_area(digit_id),
            modifiers: KeyModifiers::default(),
            time: self.time,
        }
    }
}


#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson)]
pub enum HostToStdin{
    WindowSize(StdinWindowSize),
    Signal(u64),
    Tick{
        frame: u64,
        time: f64,
    },
    FingerDown(StdinFingerDown),
    FingerUp(StdinFingerUp),
    FingerMove(StdinFingerMove)
}

#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson)]
pub enum StdinToHost{
    ReadyToStart,
    DrawComplete
}

impl StdinToHost{
    pub fn to_json(&self)->String{
        let mut json = self.serialize_json();
        json.push('\n');
        json
    }
}

impl HostToStdin{
    pub fn to_json(&self)->String{
        let mut json = self.serialize_json();
        json.push('\n');
        json
    }
}

impl Cx {
    
}
