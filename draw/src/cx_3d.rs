use {
    std::{
        ops::Deref,
        ops::DerefMut,
    },
    crate::{
        cx_2d::{Cx2d},
    }
};
    
pub struct Cx3d<'a> {
    pub cx: &'a mut Cx2d<'a>,
}

impl<'a> Deref for Cx3d<'a> {type Target = Cx2d<'a>; fn deref(&self) -> &Self::Target {self.cx}}
impl<'a> DerefMut for Cx3d<'a> {fn deref_mut(&mut self) -> &mut Self::Target {self.cx}}