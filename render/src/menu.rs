use crate::cx::*;

#[derive(PartialEq, Debug, Clone)]
pub enum Menu {
    Main {items:Vec<Menu>},
    Item {name: String, key: String, signal: Signal, value:usize, enabled:bool},
    Sub {name: String, key: String, items: Vec<Menu>},
    Line
}

impl Menu {
    pub fn main(items: Vec<Menu>)->Menu{
        Menu::Main{items:items}
    }
    
    pub fn sub(name: &str, key: &str, items: Vec<Menu>) -> Menu {
        Menu::Sub {
            name: name.to_string(),
            key: key.to_string(),
            items: items
        }
    }
    
    pub fn line() -> Menu {
        Menu::Line
    }
    pub fn item(name: &str, key: &str, enabled:bool, signal: Signal, value: usize) -> Menu {
        Menu::Item {
            name: name.to_string(),
            key: key.to_string(),
            signal: signal,
            value: value,
            enabled: enabled
        }
    }
}