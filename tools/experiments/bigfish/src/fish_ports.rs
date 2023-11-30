
use crate::makepad_micro_serde::*;

#[derive(Clone, Debug, SerRon, DeRon, Default)]
pub enum ConnectionType
{
    Audio,
    Control,
    MIDI,
    #[default] Unknown
}


#[derive(Clone, Debug, SerRon, DeRon, Default)]
pub struct FishInputPort
{
    pub id: i32,
   pub name: String,
   pub datatype: ConnectionType
}

#[derive(Clone, Debug, SerRon, DeRon, Default)]
pub struct FishOutputPort
{
    pub id: i32,
   pub name: String,
   pub datatype: ConnectionType
}