use {
    crate::{
        makepad_platform::*,
        draw_list_2d::ManyInstances,
        geometry::GeometryQuad2D,
        cx_2d::Cx2d,
        turtle::{Walk, Layout},
        DrawQuad
    },
};

live_design! {
    import makepad_draw::shader::std::*;
    DrawLine= {{DrawLine}} {
       
        fn stroke(self, side:float, progress: float) -> vec4{
            return self.color;
        }

        fn pixel(self) -> vec4 {
            let p = self.pos * self.rect_size;
            let b = self.line_end;
            let a = self.line_start;
            
            let ba = b-a;
            let pa = p-a;
            let h = clamp( dot(pa,ba)/dot(ba,ba), 0.0, 1.0 );
            let dist= length(pa-h*ba)
            
            let linemult = smoothstep(self.width-1., self.width, dist);
            let C = self.stroke(dist, h);
            return vec4(C.xyz*(1.-linemult),(1.0-linemult)*C.a);
        }
    }



}





#[derive(Live, LiveHook)]
#[repr(C)]
pub struct DrawLine {
    #[deref]  pub draw_super: DrawQuad,
    #[calc]   pub line_start: Vec2,
    #[calc]   pub line_end: Vec2,
    #[calc]   pub width: f32,
    #[calc]   pub color: Vec4,    
}