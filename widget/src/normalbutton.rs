use makepad_render::*;
use crate::buttonlogic::*;

live_body!{
    use makepad_render::shader_std::*;
    use makepad_render::drawquad::DrawQuad;
    use makepad_render::drawtext::DrawText;
    use makepad_render::turtle::*;
    
    NormalButton: Component {
        rust_type: {{NormalButton}}
        bg: DrawQuad {
            instance color: vec4 = #333;
            instance hover: float
            instance down: float
            
            const shadow: float = 3.0;
            const border_radius: float = 2.5;
            
            fn pixel(self) -> vec4 {
                let cx = Sdf2d::viewport(self.pos * self.rect_size);
                cx.box(shadow, shadow, self.rect_size.x - shadow * (1. + self.down), self.rect_size.y - shadow * (1. + self.down), border_radius);
                cx.blur = 6.0;
                cx.fill(mix(#0007, #0, self.hover));
                cx.blur = 0.001;
                cx.box(shadow, shadow, self.rect_size.x - shadow * 2., self.rect_size.y - shadow * 2., border_radius);
                return cx.fill(mix(mix(#3, #4, self.hover), #2a, self.down));
            }
            /*fn pixel(self) -> vec4 {
                return self.color;
            }*/
        }
        text: DrawText{}
        layout: Layout {
            align: Align{fx:0.5, fy:0.5},
            walk: Walk{
                width: Width::Compute,
                height: Height::Compute,
                margin: Margin{l:100.0,r:1.0,t:100.0,b:1.0},
            }
            padding: Padding{l:16.0,t:12.0,r:16.0,b:12.0}
        }
        
    }
}

#[derive(Live, LiveUpdateHooks)]
pub struct NormalButton {
    #[hidden()] pub button: ButtonLogic,
    #[live()] pub bg: DrawQuad,
    #[live()] pub text: DrawText,
    #[live()] pub layout: Layout
}

impl NormalButton {
    
    pub fn handle_normal_button(&mut self, cx: &mut Cx, event: &mut Event) -> ButtonEvent {
        //let mut ret_event = ButtonEvent::None;
        /*let animator = &mut self.animator;
        if let Some(ae) = event.is_animate(cx, animator) {
            self.bg.animate(cx, animator, ae.time);
            self.text.animate(cx, animator, ae.time);
        }
        self.button.handle_button_logic(cx, event, self.bg.area(), | cx, logic_event, _ | match logic_event {
            ButtonLogicEvent::Down => animator.play_anim(cx, live_anim!(cx, self::anim_down)),
            ButtonLogicEvent::Default => animator.play_anim(cx, live_anim!(cx, self::anim_default)),
            ButtonLogicEvent::Over => animator.play_anim(cx, live_anim!(cx, self::anim_over))
        })*/
        ButtonEvent::None
    }
    
    pub fn draw_normal_button(&mut self, cx: &mut Cx, label: &str) {
        /*
        if self.animator.need_init(cx) {
            self.animator.init(cx, live_anim!(cx, self::anim_default));
            self.bg.last_animate(&self.animator);
            self.text.last_animate(&self.animator);
        }*/
        
        self.bg.begin_quad(cx, self.layout);
        
        //self.text.text_style = live_text_style!(cx, self::text_style_label);
        self.text.draw_text_walk(cx, label);
        
        self.bg.end_quad(cx);
    }
}

/*
    pub fn style(cx: &mut Cx) {
        self::DrawNormalButton::register_draw_input(cx);
        
        live_body!(cx, {
            self::layout_bg: Layout {
                align: all(0.5),
                walk: Walk {
                    width: Compute,
                    height: Compute,
                    margin: all(1.0),
                },
                padding: {l: 16.0, t: 12.0, r: 16.0, b: 12.0},
            }
            
            self::text_style_label: TextStyle {
                ..crate::widgetstyle::text_style_normal
            }
            
            self::anim_default: Anim {
                play: Cut {duration: 0.1}
                tracks: [
                    Float {keys: {1.0: 0.0}, bind_to: self::DrawNormalButton::hover}
                    Float {keys: {1.0: 0.0}, bind_to: self::DrawNormalButton::down}
                    Vec4 {keys: {1.0: #9}, bind_to: makepad_render::drawtext::DrawText::color}
                ]
            }
            
            self::anim_over: Anim {
                play: Cut {duration: 0.1},
                tracks: [
                    Float {keys: {0.0: 1.0, 1.0: 1.0}, bind_to: self::DrawNormalButton::hover},
                    Float {keys: {1.0: 0.0}, bind_to: self::DrawNormalButton::down},
                    Vec4 {keys: {1.0: #f}, bind_to: makepad_render::drawtext::DrawText::color}
                ]
            }
            
            self::anim_down: Anim {
                play: Cut {duration: 0.2},
                tracks: [
                    Float {keys: {0.0: 1.0, 1.0: 1.0}, bind_to: self::DrawNormalButton::down},
                    Float {keys: {1.0: 1.0}, bind_to: self::DrawNormalButton::hover},
                    Vec4 {keys: {0.0: #c}, bind_to: makepad_render::drawtext::DrawText::color},
                ]
            }
            
            self::shader_bg: Shader {
                use makepad_render::drawquad::shader::*;
                
                draw_input: self::DrawNormalButton;
                
                const shadow: float = 3.0;
                const border_radius: float = 2.5;
                
                fn pixel() -> vec4 {
                    let cx = Df::viewport(pos * rect_size);
                    cx.box(shadow, shadow, rect_size.x - shadow * (1. + down), rect_size.y - shadow * (1. + down), border_radius);
                    cx.blur = 6.0;
                    cx.fill(mix(#0007, #0, hover));
                    cx.blur = 0.001;
                    cx.box(shadow, shadow, rect_size.x - shadow * 2., rect_size.y - shadow * 2., border_radius);
                    return cx.fill(mix(mix(#3, #4, hover), #2a, down));
                }
            }
        })*/