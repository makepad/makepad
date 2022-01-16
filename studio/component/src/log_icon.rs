use crate::makepad_render::*;

live_register!{
    use makepad_render::shader::std::*;
    use makepad_component::theme::*;
    
    DrawLogIconQuad: {{DrawLogIconQuad}} {
        fn pixel() -> vec4 {
            let sdf = Sdf2d::viewport(self.pos * vec2(10., 10.))
            match 
            if abs(icon_type - 5.) < 0.1 { //Wait
                let df = Df::viewport(pos * vec2(10., 10.)); // * vec2(w, h));
                df.circle(5., 5., 4.);
                df.fill_keep(#ffa500);
                df.stroke(#be, 0.5);
                df.move_to(3., 5.);
                df.line_to(3., 5.);
                df.move_to(5., 5.);
                df.line_to(5., 5.);
                df.move_to(7., 5.);
                df.line_to(7., 5.);
                df.stroke(#0, 0.8);
                return df.result;
            }
            if abs(icon_type - 4.) < 0.1 { //OK
                let df = Df::viewport(pos * vec2(10., 10.)); // * vec2(w, h));
                df.circle(5., 5., 4.);
                df.fill_keep(#5);
                df.stroke(#5, 0.5);
                let sz = 1.;
                df.move_to(5., 5.);
                df.line_to(5., 5.);
                df.stroke(#a, 0.8);
                return df.result;
            }
            else if abs(icon_type - 3.) < 0.1 { // Error
                let df = Df::viewport(pos * vec2(10., 10.)); // * vec2(w, h));
                df.circle(5., 5., 4.);
                df.fill_keep(#c00);
                df.stroke(#be, 0.5);
                let sz = 1.;
                df.move_to(5. - sz, 5. - sz);
                df.line_to(5. + sz, 5. + sz);
                df.move_to(5. - sz, 5. + sz);
                df.line_to(5. + sz, 5. - sz);
                df.stroke(#0, 0.8);
                return df.result;
            }
            else if abs(icon_type - 2.) < 0.1 { // Warning
                let df = Df::viewport(pos * vec2(10., 10.)); // * vec2(w, h));
                df.move_to(5., 1.);
                df.line_to(9., 9.);
                df.line_to(1., 9.);
                df.close_path();
                df.fill_keep(vec4(253.0 / 255.0, 205.0 / 255.0, 59.0 / 255.0, 1.0));
                df.stroke(#be, 0.5);
                df.move_to(5., 3.5);
                df.line_to(5., 5.25);
                df.stroke(#0, 0.8);
                df.move_to(5., 7.25);
                df.line_to(5., 7.5);
                df.stroke(#0, 0.8);
                return df.result;
            }
            else { // Panic
                let df = Df::viewport(pos * vec2(10., 10.)); // * vec2(w, h));
                df.move_to(5., 1.);
                df.line_to(9., 9.);
                df.line_to(1., 9.);
                df.close_path();
                df.fill_keep(#c00);
                df.stroke(#be, 0.5);
                let sz = 1.;
                df.move_to(5. - sz, 6.25 - sz);
                df.line_to(5. + sz, 6.25 + sz);
                df.move_to(5. - sz, 6.25 + sz);
                df.line_to(5. + sz, 6.25 - sz);
                df.stroke(#f, 0.8);
                
                return df.result;
            }
        }
    }
}

#[derive(Live, LiveHook)]#[repr(C)]
pub struct DrawLogIconQuad {
    deref_target: DrawQuad,
    selected: f32,
    hover: f32,
    icon:LogIcon
}

#[derive(Live, LiveHook)]
#[repr(u32)]
pub enum LogIcon {
    Wait,
    #[pick] Ok,
    Error,
    Warning,
    Panic
}
