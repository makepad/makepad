use crate::{
    makepad_platform::*,
};

pub enum DataBinding{
    ToWidgets(Vec<LiveNode>),
    FromWidgets(Vec<LiveNode>)
}

impl DataBinding{
    pub fn new()->Self{
        let mut nodes = Vec::new();
        nodes.open();
        nodes.close();
        Self::FromWidgets(nodes)
    }
    
    pub fn to_widgets(&mut self, nodes:Vec<LiveNode>){
        *self = Self::ToWidgets(nodes);
    }
    
    pub fn from_widgets(&self)->Option<&[LiveNode]>{
        match self{
            Self::FromWidgets(v)=>Some(v),
            _=>None
        }
    }
    
    pub fn nodes(&self)->&[LiveNode]{
        match self{
            Self::FromWidgets(v)=>v,
            Self::ToWidgets(v)=>v
        }
    }
}

