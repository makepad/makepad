use crate::cx::*;
use std::any::TypeId;


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
        
    pub fn set_shift(&self, cx:&mut Cx, shift:bool)->Self{
        let mut s = if let Some(s) = cx.command_settings.get(self){*s}else{CxCommandSetting::default()};
        s.shift = shift;
        cx.command_settings.insert(*self, s);
        *self
    }


    pub fn set_key(&self, cx:&mut Cx, key_code:KeyCode)->Self{
        let mut s = if let Some(s) = cx.command_settings.get(self){*s}else{CxCommandSetting::default()};
        s.key_code = key_code;
        cx.command_settings.insert(*self, s);
        *self
    }
}

impl Into<CommandId> for UniqueId {
    fn into(self) -> CommandId {CommandId(self.0)}
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