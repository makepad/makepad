use crate::cx::*;
use crate::drawquad::DrawQuad;

live_register!{
    DrawColor: {{DrawColor}} {
        fn pixel(self) -> vec4 {
            return self.color;
        }
    }
}

#[derive(LiveComponent, LiveApply, LiveCast)]
#[repr(C)]
pub struct DrawColor {
    #[live()] pub deref_target: DrawQuad,
    #[live()] pub color: Vec4
}