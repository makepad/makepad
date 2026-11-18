//! Platforms without Audio Units: nothing is installed and nothing loads.

use crate::{ComponentDescription, ComponentInfo, Error, MidiMessage};

pub(crate) fn components(_desc: &ComponentDescription) -> Vec<ComponentInfo> {
    Vec::new()
}

pub(crate) enum Unit {}

impl Unit {
    pub(crate) fn new(_desc: &ComponentDescription, _rate: f64, _max_frames: u32) -> Result<(Unit, ComponentInfo), Error> {
        Err(Error::unsupported())
    }

    pub(crate) fn set_offline(&mut self, _offline: bool) -> Result<(), Error> {
        match *self {}
    }

    pub(crate) fn latency_seconds(&self) -> f64 {
        match *self {}
    }

    pub(crate) fn midi(&mut self, _message: MidiMessage, _offset: u32) -> Result<(), Error> {
        match *self {}
    }

    pub(crate) fn render(&mut self, _left: &mut [f32], _right: &mut [f32]) -> Result<(), Error> {
        match *self {}
    }

    pub(crate) fn parameters(&self) -> Vec<crate::ParameterInfo> {
        match *self {}
    }

    pub(crate) fn parameter(&self, _id: u32) -> Result<f32, Error> {
        match *self {}
    }

    pub(crate) fn set_parameter(&mut self, _id: u32, _value: f32) -> Result<(), Error> {
        match *self {}
    }

    pub(crate) fn parameter_text(&self, _id: u32, _value: f32) -> Option<String> {
        match *self {}
    }

    pub(crate) fn factory_presets(&self) -> Vec<(i32, String)> {
        match *self {}
    }

    pub(crate) fn set_factory_preset(&mut self, _number: i32) -> Result<(), Error> {
        match *self {}
    }

    pub(crate) fn reset(&mut self) -> Result<(), Error> {
        match *self {}
    }
}
