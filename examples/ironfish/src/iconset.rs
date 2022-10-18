
use crate::makepad_draw_2d::*;

live_design!{
    import makepad_draw_2d::shader::std::*;
    import makepad_widgets::theme::*;
    
    DrawLogIconQuad= {{DrawLogIconQuad}} {
        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.pos * vec2(10., 10.))
            match self.icon_type {
                LogIconType::Pause => {
                    sdf.circle(5., 5., 4.);
                    sdf.fill(COLOR_TEXT_META);
                    sdf.move_to(3., 5.);
                    sdf.line_to(3., 5.);
                    sdf.move_to(5., 5.);
                    sdf.line_to(5., 5.);
                    sdf.move_to(7., 5.);
                    sdf.line_to(7., 5.);
                    sdf.stroke(#0, 0.8);
                }
            }
            return sdf.result
        }
    }
}

#[derive(Live, LiveHook)]#[repr(C)]
pub struct DrawLogIconQuad {
    draw_super: DrawQuad,
    selected: f32,
    hover: f32,
    pub icon_type: LogIconType
}

#[derive(Live, LiveHook)]
#[repr(u32)]
pub enum LogIconType {
    Pause = shader_enum(1),
}