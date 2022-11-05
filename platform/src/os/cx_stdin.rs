#![allow(dead_code)]
use {
    std::cell::Cell,
    crate::{
        cx::Cx,
        makepad_micro_serde::*,
        makepad_math::{dvec2},
        window::CxWindowPool,
        area::Area,
        event::{
            MouseDownEvent,
            MouseUpEvent,
            MouseMoveEvent,
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
pub struct StdinMouseDown{
   pub button: usize,
   pub x: f64,
   pub y: f64,
   pub time: f64,
}

impl From<StdinMouseDown> for MouseDownEvent {
    fn from(v: StdinMouseDown) -> Self {
        Self{
            abs: dvec2(v.x, v.y),
            button: v.button,
            window_id: CxWindowPool::id_zero(),
            modifiers: Default::default(),
            time: v.time,
            handled: Cell::new(Area::Empty),
            sweep_lock: Cell::new(Area::Empty),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, SerBin, DeBin, SerJson, DeJson, PartialEq)]
pub struct StdinMouseMove{
   pub time: f64,
   pub x: f64,
   pub y: f64
}

impl From<StdinMouseMove> for MouseMoveEvent {
    fn from(v: StdinMouseMove) -> Self {
        Self{
            abs: dvec2(v.x, v.y),
            window_id: CxWindowPool::id_zero(),
            modifiers: Default::default(),
            time: v.time,
            handled: Cell::new(Area::Empty),
            sweep_lock: Cell::new(Area::Empty),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, SerBin, DeBin, SerJson, DeJson, PartialEq)]
pub struct StdinMouseUp{
   pub time: f64,
   pub button: usize,
   pub x: f64,
   pub y: f64
}

impl From<StdinMouseUp> for MouseUpEvent {
    fn from(v: StdinMouseUp) -> Self {
        Self{
            abs: dvec2(v.x, v.y),
            button: v.button,
            window_id: CxWindowPool::id_zero(),
            modifiers: Default::default(),
            time: v.time,
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
    MouseDown(StdinMouseDown),
    MouseUp(StdinMouseUp),
    MouseMove(StdinMouseMove)
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
