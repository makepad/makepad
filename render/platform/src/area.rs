pub use {
    std::{
        rc::Rc,
        cell::RefCell
    },
    makepad_math::{
        LiveId,
        Vec2
    },
};

#[derive(Clone, Default, Hash, Ord, PartialOrd, Eq,Debug, PartialEq, Copy)]
pub struct InstanceArea{
    pub view_id:usize,
    pub draw_item_id:usize,
    pub instance_offset:usize,
    pub instance_count:usize,
    pub redraw_id:u64
}

#[derive(Clone, Default, Hash, Ord, PartialOrd, Eq,Debug, PartialEq, Copy)]
pub struct ViewArea{
    pub view_id:usize,
    pub redraw_id:u64 
}

#[derive(Clone, Debug, Hash, PartialEq, Ord, PartialOrd, Eq, Copy)]
pub enum Area{
    Empty,
    Instance(InstanceArea),
    View(ViewArea)
}

impl Default for Area{
    fn default()->Area{
        Area::Empty
    } 
}  

pub struct DrawReadRef<'a>{
    pub repeat: usize,
    pub stride: usize,
    pub buffer:&'a [f32]
}

pub struct DrawWriteRef<'a>{
    pub repeat: usize,
    pub stride: usize,
    pub buffer:&'a mut [f32]
}

impl Into<Area> for InstanceArea{
    fn into(self)->Area{
        Area::Instance(self)
    }
}
