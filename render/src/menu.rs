use crate::cx::*;
 
pub enum Menu{
    Item{name:String, key:String, signal:Signal},
    Sub{name:String, key:String, menu:Box<Menu>},
    Line
}

