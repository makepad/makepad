use crate::cx::*;

pub trait LivePickGet{
    fn get(&self, cx:&Cx)->Color;
}

impl LivePickGet for LivePick{
    fn get(&self, _cx:&Cx)->Color{
        self.color
    }    
}


pub trait LiveSlideGet{
    fn get(&self, cx:&Cx)->f32;
}

impl LiveSlideGet for LiveSlide{
    fn get(&self, _cx:&Cx)->f32{
        self.value
    }    
}
