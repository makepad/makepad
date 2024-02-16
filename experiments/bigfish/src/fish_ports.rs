use crate::makepad_micro_serde::*;

#[derive(Copy, Clone, Debug, SerRon, DeRon, Default)]
pub enum ConnectionType {
    Audio,
    Control,
    MIDI,
    Gate,
    #[default]
    Unknown,
}

#[derive(Clone, Debug, SerRon, DeRon, Default)]
pub struct FishInputPort {
    pub id: u64,
    pub name: String,
    pub datatype: ConnectionType,
}

#[derive(Clone, Debug, SerRon, DeRon, Default)]
pub struct FishOutputPort {
    pub id: u64,
    pub name: String,
    pub datatype: ConnectionType,
}

#[derive(Clone, Debug, SerRon, DeRon, Default)]
pub struct FishInputPortInstance {
    pub id: u64,
    pub source_id: u64,
    pub name: String,
    pub display_x: i32,
    pub display_y: i32,
    pub datatype: ConnectionType,
}

#[derive(Clone, Debug, SerRon, DeRon, Default)]
pub struct FishOutputPortInstance {
    pub id: u64,
    pub source_id: u64,
    pub name: String,
    pub display_x: i32,
    pub display_y: i32,
    pub datatype: ConnectionType,
}
