use makepad_render::*;
use crate::buttonlogic::*;

#[derive(Clone, DrawQuad)]
#[repr(C)]
struct NormalButtonBg {
    #[default_shader(self::shader_bg)],

    bla: f32,
    
    base: DrawQuad,

    hover: f32,
    down: f32,
}

#[derive(Clone)]
pub struct NormalButton {
    pub button: ButtonLogic,
    pub bg: NormalButtonBg,
    pub text: Text,
    pub animator: Animator,
    pub _bg_area: Area,
    pub _text_area: Area
}

impl NormalButton {
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            button: ButtonLogic::default(),
            bg: NormalButtonBg::new(cx, live_default_shader!(cx, self::shader_bg)),
            text: Text{
                shader: live_shader!(cx, makepad_render::text::shader),
                ..Text::new(cx)
            },
            animator: Animator::default(),
            _bg_area: Area::Empty,
            _text_area: Area::Empty,
        }
    }
    pub fn style(cx: &mut Cx) {
        live_draw_input!(cx, self::NormalButtonBg);
        live_body!(cx, r#"
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
                tracks:[
                    Float {keys:{1.0: 0.0}, bind_to: self::NormalButtonBg::hover}
                    Float {keys:{1.0: 0.0}, bind_to: self::NormalButtonBg::down}
                    Color {keys:{1.0: #9}, bind_to: makepad_render::drawwtext::DrawText::color}
                ]
            }
            
            self::anim_over: Anim {
                play: Cut {duration: 0.1},
                tracks:[
                    Float {keys:{0.0: 1.0, 1.0: 1.0}, bind_to: self::shader_bg::hover},
                    Float {keys:{1.0: 0.0}, bind_to: self::shader_bg::down},
                    Color {keys:{0.0: #f}, bind_to: makepad_render::text::shader::color}
                ]
            }
            
            self::anim_down: Anim {
                play: Cut {duration: 0.2},
                tracks:[
                    Float {keys:{0.0: 1.0, 1.0: 1.0}, bind_to: self::shader_bg::down},
                    Float {keys:{1.0: 1.0}, bind_to: self::shader_bg::hover},
                    Color {keys:{0.0: #c}, bind_to: makepad_render::text::shader::color},
                ]
            }
            
            self::shader_bg: Shader {
                
                use makepad_render::quad::shader::*;
                
                draw_input: self::NormalButtonBg;
                
                const shadow: float = 3.0;
                const border_radius: float = 2.5;
                
                fn pixel() -> vec4 {
                    let cx = Df::viewport(pos * vec2(w, h));
                    cx.box(shadow, shadow, w - shadow * (1. + down), h - shadow * (1. + down), border_radius);
                    cx.blur = 6.0;
                    cx.fill(mix(#0007, #0, hover));
                    cx.blur = 0.001;
                    cx.box(shadow, shadow, w - shadow * 2., h - shadow * 2., border_radius);
                    return cx.fill(mix(mix(#3, #4, hover), #2a, down));
                }
            }
        "#);
    }
    
    pub fn handle_normal_button(&mut self, cx: &mut Cx, event: &mut Event) -> ButtonEvent {
        //let mut ret_event = ButtonEvent::None;
        let animator = &mut self.animator;
        let text_area = self._text_area;
        self.button.handle_button_logic(cx, event, self._bg_area, | cx, logic_event, area | match logic_event {
            ButtonLogicEvent::Animate(ae) => {
                animator.calc_area(cx, area, ae.time);
                animator.calc_area(cx, text_area, ae.time);
            },
            ButtonLogicEvent::AnimEnded(_) => animator.end(),
            ButtonLogicEvent::Down => animator.play_anim(cx, live_anim!(cx, self::anim_down)),
            ButtonLogicEvent::Default => animator.play_anim(cx, live_anim!(cx, self::anim_default)),
            ButtonLogicEvent::Over => animator.play_anim(cx, live_anim!(cx, self::anim_over))
        })
    }
    
    pub fn draw_normal_button(&mut self, cx: &mut Cx, label: &str) {
        
        self.bg.shader = live_shader!(cx, self::shader_bg);
        
        self.animator.init(cx, | cx | live_anim!(cx, self::anim_default));
        
        let bg_inst = self.bg.begin_quad(cx, live_layout!(cx, self::layout_bg));
        
        bg_inst.push_last_float(cx, &self.animator, live_item_id!(self::shader_bg::hover));
        bg_inst.push_last_float(cx, &self.animator, live_item_id!(self::shader_bg::down));
        
        self.text.text_style = live_text_style!(cx, self::text_style_label);
        self.text.color = self.animator.last_color(cx, live_item_id!(makepad_render::text::shader::color));
        self._text_area = self.text.draw_text(cx, label);
        
        self._bg_area = self.bg.end_quad(cx, bg_inst);
        self.animator.set_area(cx, self._bg_area);
        
        
        
        //---- IS NOW ----
            self.animator.init(cx, | cx | live_anim!(cx, self::anim_default));
        
        self.bg.last_animator(&self.animator);
        self.bg.begin_quad(cx, live_layout!(cx, self::layout_bg));
        
        self.text.text_style = live_text_style!(cx, self::text_style_label);
        self.text.last_animator(&self.animator);
        self.text.draw_text(cx, label);
        
        self.bg.end_quad(cx);
        self.animator.set_area(cx, self.bg.area());
    }
}
