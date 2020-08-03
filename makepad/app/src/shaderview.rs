use makepad_render::*;

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
        Self::bg().set(cx, Quad::def_quad_shader().compose(shader!{"
            
            instance finger_hover: Self::finger_hover();
            instance finger_move: Self::finger_move();
            instance finger_down: Self::finger_down();
            
            fn pixel() -> vec4 {
                let df = Df::viewport(pos * vec2(w, h));
                df.circle(finger_hover.x, finger_hover.y, 100.);
                return df.fill(mix(color!(blue),color!(red), finger_down)); 
            }
            
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
            Event::FingerDown(_fd) =>{
                println!("{:?}", cx.captured_fingers);
                self.finger_down = 1.0;
                cx.redraw_child_area(self.area);
            },
            Event::FingerUp(_fu)=>{
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
        let new_area = k.into();
        cx.update_area_refs(self.area, new_area);
        self.area = new_area;
    }
}

