use crate::makepad_micro_serde::*;

#[derive(Clone, Debug, SerRon, DeRon, Default)]

pub struct FishConnection {
    pub id: u64,
    pub from_port: u64,
    pub from_block: u64,
    pub to_port: u64,
    pub to_block: u64,
}

impl FishConnection {
    pub fn reload_from_string(&mut self, serialized: &str) {
        *self = FishConnection::deserialize_ron(serialized).expect("deserialize a block");
    }
}
