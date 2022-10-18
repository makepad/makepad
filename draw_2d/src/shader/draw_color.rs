use {
    crate::{
        makepad_platform::*,
        shader::draw_quad::DrawQuad
    },
};

live_design!{
    DrawColor= {{DrawColor}} {
        fn pixel(self) -> vec4 {
            return vec4(self.color.rgb*self.color.a, self.color.a);
        }
    }
}

#[derive(Live, LiveHook)]
#[repr(C)]
pub struct DrawColor {
    #[live()] pub draw_super: DrawQuad,
    #[live()] pub color: Vec4
}