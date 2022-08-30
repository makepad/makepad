#![allow(dead_code)]
use {
    crate::{
        cx::Cx,
        makepad_micro_serde::*,
    }
};

#[derive(Clone, Copy, Debug, Default, SerBin, DeBin, SerJson, DeJson, PartialEq)]
pub struct StdinWindowSize{
    pub width: f64,
    pub height: f64,
    pub dpi_factor: f64,
}

#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson)]
pub enum HostToStdin{
    WindowSize(StdinWindowSize),
    Signal(u64),
    Tick{
        frame: u64,
        time: f64,
    }
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
