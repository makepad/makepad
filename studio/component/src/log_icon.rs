use crate::makepad_render::*;

live_register!{
    use makepad_render::shader::std::*;
    use makepad_component::theme::*;
    
    DrawLogIconQuad: {{DrawLogIconQuad}} {
        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.pos * vec2(10., 10.))
            match self.icon_type {
                LogIconType::Wait => {
                    sdf.circle(5., 5., 4.);
                    sdf.fill_keep(#ffa500);
                    sdf.stroke(#be, 0.5);
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
                    sdf.fill_keep(#5);
                    sdf.stroke(#5, 0.5);
                    let sz = 1.;
                    sdf.move_to(5., 5.);
                    sdf.line_to(5., 5.);
                    sdf.stroke(#a, 0.8);
                }
                LogIconType::Error => {
                    sdf.circle(5., 5., 5.);
                    sdf.fill(#a00);
                    let sz = 1.6;
                    sdf.move_to(5. - sz, 5. - sz);
                    sdf.line_to(5. + sz, 5. + sz);
                    sdf.move_to(5. - sz, 5. + sz);
                    sdf.line_to(5. + sz, 5. - sz);
                    sdf.stroke(#0, 0.8)
                }
                LogIconType::Warning => {
                    sdf.move_to(5., 1.);
                    sdf.line_to(9.5, 9.);
                    sdf.line_to(0.5, 9.);
                    sdf.close_path();
                    sdf.fill(vec4(253.0 / 255.0, 205.0 / 255.0, 59.0 / 255.0, 1.0));
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
                    sdf.fill(#b00);
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
