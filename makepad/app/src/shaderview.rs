use makepad_render::*;

// Shader code itself

fn shader() -> ShaderGen {Quad::def_quad_shader().compose(shader!{"
    
    fn pixel() -> vec4 {
        let df = Df::viewport(pos * vec2(w, h));
        df.circle(0.5*w,0.5*h, 200.);
        return df.fill(mix(
            color!(#60f),
            color!(red),
            finger_down
        ));
    }
    
"})}

// Makepad UI structure to render shader

#[derive(Clone)]
pub struct ShaderView {
    quad: Quad,
    area: Area,
    finger_hover: Vec2,
    finger_move: Vec2,
    finger_down: f32
}

impl ShaderView {
    pub fn bg() -> ShaderId {uid!()}
    pub fn finger_hover() -> Vec2Id {uid!()}
    pub fn finger_move() -> Vec2Id {uid!()}
    pub fn finger_down() -> FloatId {uid!()}
    pub fn new(cx: &mut Cx) -> Self {
        
        Self::bg().set(cx, shader().compose(shader!{"
            instance finger_hover: ShaderView::finger_hover();
            instance finger_move: ShaderView::finger_move();
            instance finger_down: ShaderView::finger_down();
            
        "}));
        
        Self {
            quad: Quad::new(cx),
            area: Area::default(),
            finger_hover: Vec2::default(),
            finger_move: Vec2::default(),
            finger_down: 0.0
        }
    }
    
    pub fn handle_shader_view(&mut self, cx: &mut Cx, event: &mut Event) {
        match event.hits(cx, self.area, HitOpt::default()) {
            Event::FingerMove(fm) => {
                self.finger_move = fm.rel;
                cx.redraw_child_area(self.area);
            },
            Event::FingerHover(fm) => {
                self.finger_hover = fm.rel;
                cx.redraw_child_area(self.area);
            },
            Event::FingerDown(_fd) => {
                self.finger_down = 1.0;
                cx.redraw_child_area(self.area);
            },
            Event::FingerUp(_fu) => {
                self.finger_down = 0.0;
                cx.redraw_child_area(self.area);
            },
            _ => ()
        }
    }
    
    pub fn draw_shader_view(&mut self, cx: &mut Cx) {
        self.quad.shader = Self::bg().get(cx);
        let k = self.quad.draw_quad_abs(cx, cx.get_turtle_rect());
        k.push_vec2(cx, self.finger_hover);
        k.push_vec2(cx, self.finger_move);
        k.push_float(cx, self.finger_down);
        self.area = cx.update_area_refs(self.area, k.into());
    }
}

