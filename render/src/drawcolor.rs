use crate::cx::*;
use crate::drawquad::DrawQuad;

live_register!{
    DrawColor: {{DrawColor}} {
        fn pixel(self) -> vec4 {
            return vec4(self.color.rgb*self.color.a, self.color.a);
        }
    }
}

#[derive(LiveComponent, LiveApply, LiveTraitCast)]
#[repr(C)]
pub struct DrawColor {
    #[live()] pub deref_target: DrawQuad,
    #[live()] pub color: Vec4
}