use makepad_render::*;

#[derive(Clone)]
pub struct ShaderView {
    quad: Quad,
    main_view: View,
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
            main_view: View::new(cx),
            finger_hover: Vec2::default(),
            finger_move: Vec2::default(),
            finger_down: 0.0
        }
    }
    
    pub fn handle_shader_view(&mut self, cx: &mut Cx, event: &mut Event) {
        match event.hits(cx, self.main_view.get_view_area(cx), HitOpt::default()) {
            Event::FingerMove(fm) => {
                self.finger_move = fm.rel;
                self.main_view.redraw_view_area(cx);
            },
            Event::FingerHover(fm) => {
                self.finger_hover = fm.rel;
                self.main_view.redraw_view_area(cx);
            },
            Event::FingerDown(_fd) =>{
                println!("FINGER DOWN");
                self.finger_down = 1.0;
                self.main_view.redraw_view_area(cx);
            },
            Event::FingerUp(_fu)=>{
                println!("FINGER UP");
                self.finger_down = 0.0;
                self.main_view.redraw_view_area(cx);
            },
            _ => ()
        }
    }
    
    pub fn draw_shader_view(&mut self, cx: &mut Cx) {
        if self.main_view.begin_view(cx, Layout::default()).is_ok() {
            
            self.quad.shader = Self::bg().get(cx);
            let k = self.quad.draw_quad_abs(cx, cx.get_turtle_rect());
            k.push_vec2(cx, self.finger_hover);
            k.push_vec2(cx, self.finger_move);
            k.push_float(cx, self.finger_down);
            self.main_view.end_view(cx);
        }
    }
}

