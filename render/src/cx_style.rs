use crate::cx::*;

impl Cx{
    pub fn style(&mut self){
        crate::shader_std::define_shader_stdlib(self);
        crate::fonts::TrapezoidText::style(self);
        crate::drawquad::DrawQuad::style(self);
        crate::drawtext::DrawText::style(self);
        crate::drawcolor::DrawColor::style(self);
        crate::drawimage::DrawImage::style(self);
        crate::drawcube::DrawCube::style(self);
    }
}

