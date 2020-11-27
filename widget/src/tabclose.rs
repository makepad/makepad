use makepad_render::*;
use crate::buttonlogic::*;

#[derive(Clone)]
pub struct TabClose {
    pub bg: DrawTabClose,
    pub animator: Animator,
}

#[derive(Clone, DrawQuad)]
#[repr(C)]
pub struct DrawTabClose {
    #[default_shader(self::shader_bg)]
    base: DrawColor,
    hover: f32,
    down: f32,
}

impl TabClose {
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            bg: DrawTabClose::new(cx, default_shader!())
                .with_draw_depth(1.3),
            animator: Animator::default(),
        }
    }
    
    pub fn style(cx: &mut Cx) {

        self::DrawTabClose::register_draw_input(cx);
        
        live_body!(cx, r#"
            self::color_selected_focus: #f;
            self::color_deselected_focus: #9d;
            
            self::walk: Walk { 
                width: Fix(20.), 
                height: Fix(20.),
                margin: {l: -4., t: 0., r: 4., b: 0.}
            }
            
            self::anim_default: Anim {
                play: Cut {duration: 0.2},
                tracks: [
                    Vec4 {keys: {1.0: self::color_deselected_focus}, bind_to: makepad_render::drawcolor::DrawColor::color},
                    Float {keys: {1.0: 0.0}, bind_to: self::DrawTabClose::hover},
                    Float {keys: {1.0: 0.0}, bind_to: self::DrawTabClose::down},
                ]
            }
            
            self::anim_over: Anim {
                play: Cut {duration: 0.1},
                tracks: [
                    Vec4 {keys: {0.0: self::color_selected_focus}, bind_to: makepad_render::drawcolor::DrawColor::color},
                    Float {keys: {1.0: 1.0}, bind_to: self::DrawTabClose::hover},
                    Float {keys: {1.0: 0.0}, bind_to: self::DrawTabClose::down},
                ]
            }
            
            self::anim_down: Anim {
                play: Cut {duration: 0.2}
                tracks: [
                    Vec4 {keys: {0.0: self::color_selected_focus}, bind_to: makepad_render::drawcolor::DrawColor::color},
                    Float {keys: {1.0: 1.0}, bind_to: self::DrawTabClose::hover},
                    Float {keys: {0.0: 0.0, 1.0: 0.0}, bind_to: self::DrawTabClose::down},
                ]
            }
            
            self::shader_bg: Shader {
                use makepad_render::drawquad::shader::*;
                
                draw_input: self::DrawTabClose;

                fn pixel() -> vec4 {
                    let cx = Df::viewport(pos * rect_size);
                    let hover_max: float = (hover * 0.5 + 0.3) * 0.5;
                    let hover_min: float = 1. - hover_max;
                    let c: vec2 = rect_size * 0.5;
                    cx.circle(c.x, c.y, 12.);
                    cx.stroke_keep(#4000,1.);
                    cx.fill(mix(#3332,#555f,hover));
                    cx.rotate(down, c.x, c.y);
                    cx.move_to(c.x * hover_min, c.y * hover_min);
                    cx.line_to(c.x + c.x * hover_max, c.y + c.y * hover_max);
                    cx.move_to(c.x + c.x * hover_max, c.y * hover_min);
                    cx.line_to(c.x * hover_min, c.y + c.y * hover_max);
                    return cx.stroke(color, 1. + hover*0.2);
                    //return df_fill(color);
                }
            }
        "#);
    }
    
    pub fn handle_tab_close(&mut self, cx: &mut Cx, event: &mut Event) -> ButtonEvent {

        if let Some(ae) = event.is_animate(cx, &self.animator) {
            self.bg.animate(cx, &mut self.animator, ae.time);
        }

        match event.hits(cx, self.bg.area(), HitOpt {
            margin: Some(Margin {l: 5., t: 5., r: 5., b: 5.}),
            ..Default::default()
        }) {
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
                if fe.input_type.has_hovers() {self.animator.play_anim(cx, live_anim!(cx, self::anim_over))}
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
        if self.animator.need_init(cx) {
            self.animator.init(cx, live_anim!(cx, self::anim_default));
            self.bg.last_animate(&self.animator);
        }

        self.bg.draw_quad_rel(cx, Rect{pos:vec2(1.0,1.0), size:vec2(25.0,25.0)});
    }
}
