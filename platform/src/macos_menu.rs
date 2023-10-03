use {
    crate::{
        makepad_live_id::LiveId,
        event::KeyCode
    },
};

/*
#[derive(Clone, Copy, Default)]
pub struct CxCommandSetting {
    pub shift: bool,
    pub key_code: KeyCode,
    pub enabled: bool
}*/

#[derive(Debug, PartialEq, Clone)]
pub enum MacosMenu {
    Main {items:Vec<MacosMenu>},
    Item {name: String, command:LiveId, shift:bool, key:KeyCode, enabled: bool},
    Sub {name: String, items: Vec<MacosMenu>},
    Line
}