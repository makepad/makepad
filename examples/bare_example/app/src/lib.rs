use makepad_render::*;

#[derive(DrawQuad)]
#[repr(C)]
struct ButtonQuad {
    #[shader(self::bg_shader)]
    base: DrawQuad,
    #[instance(self::bg_shader::counter)] 
    counter: f32, 
}

/*
impl ButtonQuad {
    pub fn new(cx: &mut Cx) -> Self {
        Self::with_shader(cx, live_shader!(cx, self::bg_shader), 0)
    }
    
    pub fn with_shader(cx: &mut Cx, shader: Shader, slots: usize) -> Self {
        Self {
            blarp: 1,
            base: DrawQuad::with_shader(cx, shader, slots + 1),
            counter: 0.0,
        }
    }
    
    pub fn begin_quad(&mut self, cx: &mut Cx, layout: Layout) {
        self.base.begin_quad(cx, layout)
    }
    
    pub fn end_quad(&mut self, cx: &mut Cx) { 
        self.base.end_quad(cx) 
    }
    
    pub fn draw_quad(&mut self, cx: &mut Cx, walk: Walk) {
        self.base.draw_quad(cx, walk);
    }
    
    pub fn draw_quad_rel(&mut self, cx: &mut Cx, rect: Rect) {
        self.base.draw_quad_rel(cx, rect)
    }
    
    pub fn draw_quad_abs(&mut self, cx: &mut Cx, rect: Rect) {
        self.base.draw_quad_abs(cx, rect)
    }
    
    pub fn lock_quad(&mut self, cx: &mut Cx) {
        self.base.lock_quad(cx)
    }
    
    pub fn add_quad(&mut self, rect: Rect) {
        self.base.add_quad(rect);
    }
    
    pub fn unlock_quad(&mut self, cx: &mut Cx) {
        self.base.unlock_quad(cx)
    }
    
    pub fn animate(&mut self, animator: &mut Animator, time: f64) {
    }
    
    pub fn last_animate(&mut self, animator: &Animator) {
    }
}*/

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
            quad: ButtonQuad::new(cx),
            main_view: View::new(cx),
            count: 0.
        }
    }
    
    pub fn style(cx: &mut Cx) {
        live_body!(cx, r#"
            
            self::quad_shader: Shader {
                
                use makepad_render::shader_std::prelude::*;
                
                default_geometry: makepad_render::shader_std::quad_2d;
                geometry geom: vec2;
                
                varying pos: vec2;
                
                instance x: float;
                instance y: float;
                instance w: float;
                instance h: float;
                instance z: float;
                
                //let dpi_dilate: float<Uniform>;
                fn scroll() -> vec2 {
                    return draw_scroll.xy;
                }
                
                fn vertex() -> vec4 {
                    let scr = scroll();
                    
                    let clipped: vec2 = clamp(
                        geom * vec2(w, h) + vec2(x, y) - scr,
                        draw_clip.xy,
                        draw_clip.zw
                    );
                    pos = (clipped + scr - vec2(x, y)) / vec2(w, h);
                    // only pass the clipped position forward
                    return camera_projection * (camera_view * (view_transform * vec4(clipped.x, clipped.y, z + draw_zbias, 1.)));
                }
                
                fn pixel() -> vec4 {
                    return #0f0;
                }
            }
            
            self::bg_color: #f00;
            self::bg_color2: #00f;
            self::bg_shader: Shader {
                use self::quad_shader::*;
                
                instance counter: float;
                
                fn pixel() -> vec4 {
                    let df = Df::viewport(pos * vec2(w, h));
                    df.circle(0.5 * w, 0.5 * h, 0.5 * w);
                    return df.fill(mix(self::bg_color2, self::bg_color, abs(sin(counter))));
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
            for i in 0..250000 {
                let v = 0.3 * (i as f32);
                self.quad.counter += 0.01; //= (i as f32).sin();
                self.quad.add_quad(Rect {
                    x: 300. + (v + self.count).sin() * 100.,
                    y: 300. + (v + self.count * 8.).cos() * 100.,
                    w: 10.,
                    h: 10.
                });
            }
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
