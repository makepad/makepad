use makepad_render::*;

#[derive(Clone)]
pub struct ShaderView {
    quad: Quad,
    main_view: View,
    mouse_xy: Vec2
}

impl ShaderView {
    pub fn bg() -> ShaderId {uid!()}
    pub fn mouse_xy() -> Vec2Id {uid!()}
    pub fn new(cx: &mut Cx) -> Self {
        Self::bg().set(cx, Quad::def_quad_shader().compose(shader!{"
            
            instance mouse_xy: Self::mouse_xy();
            
            fn pixel() -> vec4 {
                let df = Df::viewport(pos * vec2(w, h));
                df.circle(0.5 * w, 0.5 * h, 0.5 * w);
                return df.fill(color!(red));
            }
            
        "}));
        
        Self {
            quad: Quad::new(cx),
            main_view: View::new(cx),
            mouse_xy: Vec2::default()
        }
    }
    
    pub fn handle_shader_view(&mut self, cx: &mut Cx, event: &mut Event) {
        match event {
            Event::FingerMove(fm) => {
                self.mouse_xy = fm.rel;
                self.main_view.redraw_view_area(cx);
            },
            _ => ()
        }
    }
    
    pub fn draw_shader_view(&mut self, cx: &mut Cx) {
        if self.main_view.begin_view(cx, Layout::default()).is_ok() {
            
            self.quad.shader = Self::bg().get(cx);
            let k = self.quad.draw_quad_abs(cx, cx.get_turtle_rect());
            k.push_vec2(cx, self.mouse_xy);
            self.main_view.end_view(cx);
        }
    }
}

