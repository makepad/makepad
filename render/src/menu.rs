use crate::cx::*;
use std::any::TypeId;

impl Cx{
    pub fn command_quit()->CommandId{uid!()}
    pub fn command_undo()->CommandId{uid!()}
    pub fn command_redo()->CommandId{uid!()}
    pub fn command_cut()->CommandId{uid!()}
    pub fn command_copy()->CommandId{uid!()}
    pub fn command_paste()->CommandId{uid!()}
    pub fn command_zoom_in()->CommandId{uid!()}
    pub fn command_zoom_out()->CommandId{uid!()}
    pub fn command_minimize()->CommandId{uid!()}
    pub fn command_zoom()->CommandId{uid!()}
    pub fn command_select_all()->CommandId{uid!()}
    
    pub fn command_default_keymap(&mut self){
        Cx::command_quit().set_key(self, KeyCode::KeyQ);
        Cx::command_undo().set_key(self, KeyCode::KeyZ);
        Cx::command_redo().set_key_shift(self, KeyCode::KeyZ);
        Cx::command_cut().set_key(self, KeyCode::KeyX);
        Cx::command_copy().set_key(self, KeyCode::KeyC);
        Cx::command_paste().set_key(self, KeyCode::KeyV);
        Cx::command_select_all().set_key(self, KeyCode::KeyA);
        Cx::command_zoom_out().set_key(self, KeyCode::Minus);
        Cx::command_zoom_in().set_key(self, KeyCode::Equals);
        Cx::command_minimize().set_key(self, KeyCode::KeyM);
    }
}


// Command


#[derive(PartialEq, Copy, Clone, Hash, Eq, Debug)]
pub struct CommandId(pub TypeId);

impl CommandId{
    pub fn set_enabled(&self, cx:&mut Cx, enabled:bool)->Self{
        let mut s = if let Some(s) = cx.command_settings.get(self){*s}else{CxCommandSetting::default()};
        s.enabled = enabled;
        cx.command_settings.insert(*self, s);
        *self
    }

    pub fn set_key(&self, cx:&mut Cx, key_code:KeyCode)->Self{
        let mut s = if let Some(s) = cx.command_settings.get(self){*s}else{CxCommandSetting::default()};
        s.shift = false;
        s.key_code = key_code;
        cx.command_settings.insert(*self, s);
        *self
    }
    
    pub fn set_key_shift(&self, cx:&mut Cx, key_code:KeyCode)->Self{
        let mut s = if let Some(s) = cx.command_settings.get(self){*s}else{CxCommandSetting::default()};
        s.shift = true;
        s.key_code = key_code;
        cx.command_settings.insert(*self, s);
        *self
    }
}

impl Into<CommandId> for TypeId {
    fn into(self) -> CommandId {CommandId(self)}
}


#[derive(PartialEq, Clone)]
pub enum Menu {
    Main {items:Vec<Menu>},
    Item {name: String, command:CommandId},
    Sub {name: String, items: Vec<Menu>},
    Line
}

impl Menu {
    pub fn main(items: Vec<Menu>)->Menu{
        Menu::Main{items:items}
    }
    
    pub fn sub(name: &str, items: Vec<Menu>) -> Menu {
        Menu::Sub {
            name: name.to_string(),
            items: items
        }
    }
    
    pub fn line() -> Menu {
        Menu::Line
    }
    
    pub fn item(name: &str, command: CommandId) -> Menu {
        Menu::Item {
            name: name.to_string(),
            command: command
        }
    }
}