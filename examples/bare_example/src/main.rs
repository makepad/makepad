use makepad_render::*;

struct App {
    window: Window,
    pass: Pass,
    color_texture: Texture,
    main_view: View,
    quad: Quad,
    count: f32
}

main_app!(App);

impl App {
    pub fn bg() -> ShaderId {uid!()}
    pub fn counter() -> InstanceFloat {uid!()}
    pub fn new(cx: &mut Cx) -> Self {
        
        Self::bg().set(cx, Quad::def_quad_shader().compose(shader_ast!({
            let counter: Self::counter();
            fn pixel() -> vec4 {
                df_viewport(pos * vec2(w, h));
                df_circle(0.5 * w, 0.5 * h, 0.5 * w);
                //return df_fill(color("green"));
                return df_fill(mix(color("green"), color("blue"), abs(sin(counter))));
            }
        })));
        
        Self {
            window: Window::new(cx),
            pass: Pass::default(),
            color_texture: Texture::default(),
            quad: Quad::new(cx),
            main_view: View::new(cx),
            count: 0.
        }
    }
    
    fn handle_app(&mut self, _cx: &mut Cx, event: &mut Event) {
        match event {
            Event::Construct => {
                
            },
            Event::FingerMove(fm)=>{
                self.count = fm.abs.x*0.01;
            },
            _ => ()
        }
    }
    
    fn draw_app(&mut self, cx: &mut Cx) {
        self.window.begin_window(cx);
        self.pass.begin_pass(cx);
        self.pass.add_color_texture(cx, &mut self.color_texture, ClearColor::ClearWith(color256(32, 0, 0)));
        if self.main_view.begin_view(cx, Layout::default()).is_ok() {
            
            self.quad.shader = Self::bg().get(cx);
            let k = self.quad.draw_quad_abs(cx, Rect{x:100.,y:100.,w:200.,h:200.});
            k.push_float(cx,10.);
            
            for i in 0..2500 {
                let v = 0.3 * (i as f32);
                let k = self.quad.draw_quad_abs(cx, Rect {
                    x: 300. + (v + self.count).sin() * 100.,
                    y: 300. + (v + self.count*8.).cos() * 100.,
                    w: 10., 
                    h: 10.
                }); 
                k.push_float(cx, v*2.+self.count*10.);
            }
            self.count += 0.001;
            
            self.main_view.redraw_view_area(cx);
            self.main_view.end_view(cx);
        }
        self.pass.end_pass(cx);
        self.window.end_window(cx);
    }
}
