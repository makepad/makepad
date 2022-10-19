use crate::{
    makepad_platform::*,
};

pub enum DataBinding{
    ToWidgets(Vec<LiveNode>),
    FromWidgets(Vec<LiveNode>)
}

impl DataBinding{
    pub fn new()->Self{
        Self::FromWidgets(Vec::new())
    }
    
    pub fn to_widgets(&mut self, nodes:Vec<LiveNode>){
        *self = Self::ToWidgets(nodes);
    }
    
    pub fn from_widgets(&self)->Option<&[LiveNode]>{
        match self{
            Self::FromWidgets(v) if v.len()>0=>Some(v),
            _=>None
        }
    }
}

