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
    text: ButtonText,
    count: f32
}

impl BareExampleApp {
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            window: Window::new(cx),
            pass: Pass::default(),
            color_texture: Texture::new(cx),
            quad: ButtonQuad::new(cx, default_shader!()),
            text: ButtonText::new(cx, default_shader!()),
            main_view: View::new(),
            count: 0.
        }
    }
    
    pub fn style(cx: &mut Cx) {
        
        ButtonQuad::register_draw_input(cx);
        ButtonText::register_draw_input(cx);
        
        live_body!(cx, r#"
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
        "#);
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
            cx.profile_start(1);
            
            //let x = 1.0f32;
            //let y = x.sin();
            self.quad.counter = 0.;
            
            self.quad.begin_many(cx);
            //self.quad.base.shader = live_shader!(cx, self::bg_shader);
            //println!("{}", self.quad.base.slots);
            self.text.counter += 0.01;
            
            //self.text.begin_many(cx);
            self.quad.counter = 0.;
            self.quad.some += 1.1;
            let msg = format!("HELLO WORLD");
            
            for i in 0..1000000 {
                let v = 0.5 * (i as f32);
                self.quad.counter += 0.01; //= (i as f32).sin();
                let x = 0.0;//400. + (v + self.count).sin() * 400.;
                let y = 0.0;//400. + (v * 1.12 + self.count * 18.).cos() * 400.;
                self.quad.draw_quad_abs(cx, Rect {pos: vec2(x, y), size: vec2(10., 10.0)});
                
                //self.text.draw_text_abs(cx, vec2(x, y), &msg);
            }
            //self.text.end_many(cx);
            self.quad.end_many(cx);
            self.count += 0.001;
            
            cx.profile_end(1);
            
            /*
            cx.profile_start(2);
            
            for i in 0..2500000 {
                let v = 0.3 * (i as f32);
                self.quad.draw_quad_scratch(Rect {
                    x: 300. + (v + self.count).sin() * 100.,
                    y: 300. + (v + self.count * 8.).cos() * 100.,
                    w: 10.,
                    h: 10.
                });
                self.quad.scratch[9] = v * 2. + self.count * 10.;
                self.quad.draw_quad_scratch_final(cx, 10);
            }
            self.count += 0.001;
            cx.profile_end(2);
            
            cx.profile_start(3);
            
            let inst = cx.new_instance(self.quad.shader, None, 1);
            let mut data = Vec::new();
            for i in 0..2500000 {
                let inst_array = inst.get_instance_array(cx);
                std::mem::swap(&mut data, inst_array);
                let v = 0.3 * (i as f32);
                self.quad.draw_quad_scratch(Rect {
                    x: 300. + (v + self.count).sin() * 100.,
                    y: 300. + (v + self.count * 8.).cos() * 100.,
                    w: 10.,
                    h: 10.
                });
                self.quad.scratch[9] = v * 2. + self.count * 10.;
                data.extend_from_slice(&self.quad.scratch[0..10]);
                std::mem::swap(inst_array, &mut data);
            }
            self.count += 0.001;
            cx.profile_end(3);
            */
            /*
            cx.profile_start(4);
            let inst = cx.new_instance(self.quad3.shader, None, 1);
            let inst_array = inst.get_instance_array(cx);
            for i in 0..2500000 {
                let v = 0.3 * (i as f32);
                self.quad3.rect = Rect {
                    x: 300. + (v + self.count).sin() * 100.,
                    y: 300. + (v + self.count * 8.).cos() * 100.,
                    w: 10.,
                    h: 10.
                };
                self.quad3.count = v * 2. + self.count * 10.;
                self.quad3.draw_quad_direct(inst_array);
                //inst_array.push();
            }
            self.count += 0.001;
            cx.profile_end(4);
            */
            
            self.main_view.redraw_view(cx);
            self.main_view.end_view(cx);
        }
        self.pass.end_pass(cx);
        self.window.end_window(cx);
    }
}
