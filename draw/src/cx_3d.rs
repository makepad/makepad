use {
    std::{
        ops::Deref,
        ops::DerefMut,
    },
    crate::{
        cx_draw::{CxDraw},
    }
};

pub struct Cx3d<'a, 'b> {
    pub cx: &'b mut CxDraw<'a>,
}

impl<'a, 'b> Deref for Cx3d<'a,'b> {type Target = CxDraw<'a>; fn deref(&self) -> &Self::Target {self.cx}}
impl<'a, 'b> DerefMut for Cx3d<'a,'b> {fn deref_mut(&mut self) -> &mut Self::Target {self.cx}}

impl<'a, 'b> Cx3d<'a, 'b> {
    pub fn new(cx: &'b mut CxDraw<'a>)->Self{
        Self {
            cx: cx,
        }
    }
}