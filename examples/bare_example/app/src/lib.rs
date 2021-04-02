use makepad_render::*;

#[derive(Clone, DrawQuad)]
#[repr(C)]
struct ButtonQuad {
    #[default_shader(self::shader_quad)]
    some: f32,
    base: DrawQuad,
    counter: f32,
}

#[derive(Clone, DrawText)]
#[repr(C)]
struct ButtonText {
    #[default_shader(self::shader_text)]
    base: DrawText,
    counter: f32,
}

pub struct BareExampleApp {
    window: Window,
    pass: Pass,
    color_texture: Texture,
    main_view: View,
    quad: ButtonQuad,
    count: f32
}

impl BareExampleApp {
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            window: Window::new(cx),
            pass: Pass::default(),
            color_texture: Texture::new(cx),
            quad: ButtonQuad::new(cx, default_shader!()),
            main_view: View::new(),
            count: 0.
        }
    }
    
    pub fn style(cx: &mut Cx) {
        
        ButtonQuad::register_draw_input(cx);
        ButtonText::register_draw_input(cx);
        
        live_body!(cx, {
            self::shader_quad: Shader {
                use makepad_render::drawquad::shader::*;
                draw_input: self::ButtonQuad;
                fn pixel() -> vec4 {
                    return mix(#f00, #0f0, abs(sin(counter + some)));
                }
            }
            
            self::shader_text: Shader {
                use makepad_render::drawtext::shader::*;
                draw_input: self::ButtonText;
                fn get_color() -> vec4 {
                    //return #f;
                    return mix(#f00, #0f0, abs(sin(counter + char_offset * 0.2)));
                }
            }
        });
    }
    
    pub fn handle_app(&mut self, _cx: &mut Cx, event: &mut Event) {
        match event {
            Event::Construct => {
                
            },
            Event::FingerMove(fm) => {
                self.count = fm.abs.x * 0.01;
            },
            _ => ()
        }
    }
    
    pub fn draw_app(&mut self, cx: &mut Cx) {

        self.window.begin_window(cx);
        self.pass.begin_pass(cx);
        self.pass.add_color_texture(cx, self.color_texture, ClearColor::ClearWith(Vec4::color("300")));
        if self.main_view.begin_view(cx, Layout::default()).is_ok() {

            self.quad.counter = 0.;
            self.quad.begin_many(cx);
            
            self.quad.counter = 0.;
            self.quad.some = 0.;
            
            for i in 0..1000 {  
                let v = 0.5 * (i as f32);
                self.quad.counter += 0.01; //= (i as f32).sin();
                let x = 400. + (v + self.count).sin() * 400.;
                let y = 400. + (v * 1.12 + self.count * 18.).cos() * 400.;
                self.quad.draw_quad_abs(cx, Rect {pos: vec2(x, y), size: vec2(10., 10.0)});
            }

            self.quad.end_many(cx);
            self.count += 0.001;
            
            self.main_view.redraw_view(cx);
            self.main_view.end_view(cx);
        }
        self.pass.end_pass(cx);
        self.window.end_window(cx);
    }
}
