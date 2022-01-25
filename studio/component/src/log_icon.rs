use crate::makepad_platform::*;

live_register!{
    use makepad_platform::shader::std::*;
    use makepad_component::theme::*;
    
    DrawLogIconQuad: {{DrawLogIconQuad}} {
        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.pos * vec2(10., 10.))
            match self.icon_type {
                LogIconType::Wait => {
                    sdf.circle(5., 5., 4.);
                    sdf.fill(COLOR_ICON_WAIT);
                    sdf.move_to(3., 5.);
                    sdf.line_to(3., 5.);
                    sdf.move_to(5., 5.);
                    sdf.line_to(5., 5.);
                    sdf.move_to(7., 5.);
                    sdf.line_to(7., 5.);
                    sdf.stroke(#0, 0.8);
                }
                LogIconType::Ok => {
                    sdf.circle(5., 5., 4.);
                    sdf.fill(COLOR_TEXT_META);
                    let sz = 1.;
                    sdf.move_to(5., 5.);
                    sdf.line_to(5., 5.);
                    sdf.stroke(#a, 0.8);
                }
                LogIconType::Error => {
                    sdf.circle(5., 5., 4.5);
                    sdf.fill(COLOR_ERROR);
                    let sz = 1.5;
                    sdf.move_to(5. - sz, 5. - sz);
                    sdf.line_to(5. + sz, 5. + sz);
                    sdf.move_to(5. - sz, 5. + sz);
                    sdf.line_to(5. + sz, 5. - sz);
                    sdf.stroke(#0, 0.8)
                }
                LogIconType::Warning => {
                    sdf.move_to(5., 1.);
                    sdf.line_to(9.25, 9.);
                    sdf.line_to(0.75, 9.);
                    sdf.close_path();
                    sdf.fill(COLOR_WARNING);
                    //  sdf.stroke(#be, 0.5);
                    sdf.move_to(5., 3.5);
                    sdf.line_to(5., 5.25);
                    sdf.stroke(#0, 1.0);
                    sdf.move_to(5., 7.25);
                    sdf.line_to(5., 7.5);
                    sdf.stroke(#0, 1.0);
                }
                LogIconType::Panic => {
                    sdf.move_to(5., 1.);
                    sdf.line_to(9., 9.);
                    sdf.line_to(1., 9.);
                    sdf.close_path();
                    sdf.fill(COLOR_ERROR);
                    let sz = 1.;
                    sdf.move_to(5. - sz, 6.25 - sz);
                    sdf.line_to(5. + sz, 6.25 + sz);
                    sdf.move_to(5. - sz, 6.25 + sz);
                    sdf.line_to(5. + sz, 6.25 - sz);
                    sdf.stroke(#0, 0.8);
                }
            }
            return sdf.result
        }
    }
}

#[derive(Live, LiveHook)]#[repr(C)]
pub struct DrawLogIconQuad {
    deref_target: DrawQuad,
    selected: f32,
    hover: f32,
    pub icon_type: LogIconType
}

#[derive(Live, LiveHook)]
#[repr(u32)]
pub enum LogIconType {
    Wait,
    #[pick] Ok,
    Error,
    Warning,
    Panic
}
