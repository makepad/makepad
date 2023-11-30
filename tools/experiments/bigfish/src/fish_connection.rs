
use crate::makepad_micro_serde::*;


#[derive(Clone, Debug, SerRon, DeRon, Default)]

pub struct FishConnection{
    pub id: i32,
    pub from_port: i32,
    pub from_block: i32,
    pub to_port: i32,
    pub to_block: i32
}