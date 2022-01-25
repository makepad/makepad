use {
    crate::{
        makepad_derive_live::*,
        cx::Cx,
        live_traits::*,
        draw_2d::draw_quad::DrawQuad
    },
};

live_register!{
    DrawColor: {{DrawColor}} {
        fn pixel(self) -> vec4 {
            return vec4(self.color.rgb*self.color.a, self.color.a);
        }
    }
}

#[derive(Live, LiveHook)]
#[repr(C)]
pub struct DrawColor {
    #[live()] pub deref_target: DrawQuad,
    #[live()] pub color: Vec4
}