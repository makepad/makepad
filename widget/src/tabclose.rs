use makepad_render::*;
use crate::buttonlogic::*;

#[derive(Clone)]
pub struct TabClose {
    pub bg: Quad,
    pub animator: Animator,
    pub _bg_area: Area,
}

impl TabClose {
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            bg: Quad::new(cx),
            animator: Animator::default(),
            _bg_area: Area::Empty,
        }
    }
    
    pub fn style(cx: &mut Cx) {
        
        live!(cx, r#"
            self::color_selected_focus: #f;
            self::color_deselected_focus: #9d;
            
            self::walk: Walk {
                width: Fix(10.),
                height: Fix(10.),
                margin: {l: -4., t: 0., r: 4., b: 0.}
            }
            
            self::anim_default: Anim {
                play: Cut {duration: 0.2},
                tracks: [
                    Color {keys: {1.0: self::color_deselected_focus}, live_id: makepad_render::quad::shader::color},
                    Float {keys: {1.0: 0.0}, live_id: self::shader_bg::hover},
                    Float {keys: {1.0: 0.0}, live_id: self::shader_bg::down},
                ]
            }
            
            self::anim_over: Anim {
                play: Cut {duration: 0.2},
                tracks: [
                    Color {keys: {0.0: self::color_selected_focus}, live_id: makepad_render::quad::shader::color},
                    Float {keys: {1.0: 1.0}, live_id: self::shader_bg::hover},
                    Float {keys: {1.0: 0.0}, live_id: self::shader_bg::down},
                ]
            }
            
            self::anim_down: Anim {
                play: Cut {duration: 0.2}
                tracks: [
                    Color {keys: {0.0: self::color_selected_focus}, live_id: makepad_render::quad::shader::color},
                    Float {keys: {1.0: 1.0}, live_id: self::shader_bg::hover},
                    Float {keys: {0.0: 0.0, 1.0: 0.0}, live_id: self::shader_bg::down},
                ]
            }
            
            self::shader_bg: Shader {
                use makepad_render::quad::shader::*;
                
                instance hover: float;
                instance down: float;

                fn pixel() -> vec4 {
                    let cx = Df::viewport(pos * vec2(w, h));
                    let hover_max: float = (hover * 0.2 + 0.8) * 0.5;
                    let hover_min: float = 1. - hover_max;
                    let c: vec2 = vec2(w, h) * 0.5;
                    cx.rotate(down, c.x, c.y);
                    cx.move_to(c.x * hover_min, c.y * hover_min);
                    cx.line_to(c.x + c.x * hover_max, c.y + c.y * hover_max);
                    cx.move_to(c.x + c.x * hover_max, c.y * hover_min);
                    cx.line_to(c.x * hover_min, c.y + c.y * hover_max);
                    return cx.stroke(color, 1. + down * 0.2);
                    //return df_fill(color);
                }
            }
        "#);
    }
    
    pub fn handle_tab_close(&mut self, cx: &mut Cx, event: &mut Event) -> ButtonEvent {
        match event.hits(cx, self._bg_area, HitOpt {
            margin: Some(Margin {l: 5., t: 5., r: 5., b: 5.}),
            ..Default::default()
        }) {
            Event::Animate(ae) => self.animator.calc_area(cx, self._bg_area, ae.time),
            Event::FingerDown(_fe) => {
                self.animator.play_anim(cx, live_anim!(cx, self::anim_down));
                cx.set_down_mouse_cursor(MouseCursor::Hand);
                return ButtonEvent::Down;
            },
            Event::FingerHover(fe) => {
                cx.set_hover_mouse_cursor(MouseCursor::Hand);
                match fe.hover_state {
                    HoverState::In => if fe.any_down {
                        self.animator.play_anim(cx, live_anim!(cx, self::anim_down))
                    }
                    else {
                        self.animator.play_anim(cx, live_anim!(cx, self::anim_over))
                    },
                    HoverState::Out => self.animator.play_anim(cx, live_anim!(cx, self::anim_default)),
                    _ => ()
                }
            },
            Event::FingerUp(fe) => if fe.is_over {
                if !fe.is_touch {self.animator.play_anim(cx, live_anim!(cx, self::anim_over))}
                else {self.animator.play_anim(cx, live_anim!(cx, self::anim_default))}
                return ButtonEvent::Clicked;
            }
            else {
                self.animator.play_anim(cx, live_anim!(cx, self::anim_default));
                return ButtonEvent::Up;
            }
            _ => ()
        };
        ButtonEvent::None
    }
    
    pub fn draw_tab_close(&mut self, cx: &mut Cx) {
        self.animator.init(cx, | cx | live_anim!(cx, self::anim_default));
        self.bg.shader = live_shader!(cx, self::shader_bg);
        self.bg.color = self.animator.last_color(cx, live_id!(makepad_render::quad::shader::color));
        let bg_inst = self.bg.draw_quad(cx, live_walk!(cx, self::walk));
        bg_inst.push_last_float(cx, &self.animator, live_id!(self::shader_bg::hover));
        bg_inst.push_last_float(cx, &self.animator, live_id!(self::shader_bg::down));
        self._bg_area = bg_inst.into();
        self.animator.set_area(cx, self._bg_area); // if our area changed, update animation
    }
}
