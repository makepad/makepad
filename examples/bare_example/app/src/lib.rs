use makepad_render::*;

#[derive(Clone, DrawQuad)]
#[repr(C)]
struct ButtonQuad {
    #[shader(self::shader_button_quad)]
    base: DrawQuad,
    #[instance(self::shader_button_quad::counter)]
    counter: f32,
}

#[derive(Clone, DrawText)]
#[repr(C)]
struct ButtonText {
    #[shader(self::shader_button_text)]
    base: DrawText,
    #[instance(self::shader_button_text::counter)]
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
            quad: ButtonQuad::new(cx),
            text: ButtonText::new(cx),
            main_view: View::new(cx),
            count: 0.
        }
    }
    
    pub fn style(cx: &mut Cx) {
        live_body!(cx, r#"
            self::shader_button_quad: Shader {
                use makepad_render::drawquad::shader::*;
                instance counter: float;
                fn pixel() -> vec4 {
                    return mix(#f00, #0f0, counter);
                }
            }
            
            self::shader_button_text: Shader {
                use makepad_render::drawtext::shader::*;
                instance counter: float;
                fn get_color() -> vec4 {
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
        self.pass.add_color_texture(cx, self.color_texture, ClearColor::ClearWith(Color::rgb(32, 0, 0)));
        if self.main_view.begin_view(cx, Layout::default()).is_ok() {
             cx.profile_start(1);
            
            //let x = 1.0f32;
            //let y = x.sin();
            self.quad.counter = 0.;
            self.quad.lock_quad(cx);
            //self.quad.base.shader = live_shader!(cx, self::bg_shader);
            //println!("{}", self.quad.base.slots);
            self.text.counter += 0.01;
            
            //self.text.lock_text(cx);
            let msg = format!("HELLO WORLD");
            for i in 0..20000 {
                let v = 0.3 * (i as f32);
                self.quad.counter += 0.01; //= (i as f32).sin();
                self.quad.add_quad(Rect {
                    x: 300. + (v + self.count).sin() * 100.,
                    y: 300. + (v + self.count * 8.).cos() * 100.,
                    w: 10.,
                    h: 10.
                });
                self.text.draw_text(
                    cx,
                    &msg
                );
            }
           // self.text.unlock_text(cx);
            self.quad.unlock_quad(cx);
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
            
            self.main_view.redraw_view_area(cx);
            self.main_view.end_view(cx);
        }
        self.pass.end_pass(cx);
        self.window.end_window(cx);
    }
}
