use makepad_render::*;
use crate::buttonlogic::*;
use crate::widgetstyle::*;

#[derive(Clone)]
pub struct NormalButton {
    pub button: ButtonLogic,
    pub bg: Quad,
    pub text: Text,
    pub animator: Animator,
    pub _bg_area: Area,
    pub _text_area: Area
}

impl NormalButton {
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            button: ButtonLogic::default(),
            bg: Quad::new(cx),
            text: Text::new(cx),
            animator: Animator::default(),
            _bg_area: Area::Empty,
            _text_area: Area::Empty,  
        }
    }
    
    pub fn layout_bg() -> LayoutId {uid!()}
    pub fn text_style_label() -> TextStyleId {uid!()}
    pub fn anim_default() -> AnimId {uid!()}
    pub fn anim_over() -> AnimId {uid!()}
    pub fn anim_down() -> AnimId {uid!()}
    pub fn shader_bg() -> ShaderId {uid!()}
    pub fn instance_hover() -> InstanceFloat {uid!()}
    pub fn instance_down() -> InstanceFloat {uid!()}
    
    pub fn style(cx: &mut Cx, _opt: &StyleOptions) {
        Self::layout_bg().set(cx, Layout {
            align: Align::center(),
            walk: Walk {
                width: Width::Compute,
                height: Height::Compute,
                margin: Margin::all(1.0),
            },
            padding: Padding {l: 16.0, t: 12.0, r: 16.0, b: 12.0},
            ..Default::default()
        });
        
        Self::text_style_label().set(cx, TextStyle {
            ..Theme::text_style_normal().get(cx)
        });
        
        Self::anim_default().set(cx,Anim::new(Play::Cut {duration: 0.1}, vec![
            Track::float(Self::instance_hover(), Ease::Lin, vec![(1., 0.)]),
            Track::float(Self::instance_down(), Ease::Lin, vec![(1.0, 0.)]),
            Track::color(Text::instance_color(), Ease::Lin, vec![(1., color("#9"))]),
        ]));
        
        Self::anim_over().set(cx, Anim::new(Play::Cut {duration: 0.1}, vec![
            Track::float(Self::instance_down(), Ease::Lin, vec![(0., 0.)]),
            Track::float(Self::instance_hover(), Ease::Lin, vec![(0.0, 1.0), (1.0, 1.0)]),
            Track::color(Text::instance_color(), Ease::Lin, vec![(0., color("#f"))]),
        ]));
        
        Self::anim_down().set(cx,Anim::new(Play::Cut {duration: 0.2}, vec![
            Track::float(Self::instance_down(), Ease::OutExp, vec![(0.0, 1.0), (1.0, 1.0)]),
            Track::float(Self::instance_hover(), Ease::Lin, vec![(1.0, 1.0)]),
            Track::color(Text::instance_color(), Ease::Lin, vec![(0., color("#c"))]),
        ]));
        
        // lets define the shader
        Self::shader_bg().set(cx, Quad::def_quad_shader().compose(shader_ast!({
            
            let hover: Self::instance_hover();
            let down: Self::instance_down();
            const shadow: float = 3.0;
            const border_radius: float = 2.5;
            fn pixel() -> vec4 {
                df_viewport(pos * vec2(w, h));
                df_box(shadow, shadow, w - shadow*(1.+down), h- shadow*(1.+down), border_radius);
                df_blur = 6.0;
                df_fill(mix(color("#0007"), color("#0"), hover));
                df_blur = 0.001;
                df_box(shadow, shadow, w - shadow*2., h - shadow*2., border_radius);
                return df_fill(mix(mix(color("#3"),color("#4"),hover), color("#2a2a2a"), down));
            }
        })));
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
            ButtonLogicEvent::Down => animator.play_anim(cx, Self::anim_down().get(cx)),
            ButtonLogicEvent::Default => animator.play_anim(cx, Self::anim_default().get(cx)),
            ButtonLogicEvent::Over => animator.play_anim(cx, Self::anim_over().get(cx))
        })
    }
    
    pub fn draw_normal_button(&mut self, cx: &mut Cx, label: &str) {
        self.bg.shader = Self::shader_bg().get(cx);
        
        self.animator.init(cx, | cx | Self::anim_default().get(cx));
        
        self.bg.color = self.animator.last_color(cx, Quad::instance_color());
        
        let bg_inst = self.bg.begin_quad(cx, Self::layout_bg().get(cx));
        
        bg_inst.push_last_float(cx, &self.animator, Self::instance_hover());
        bg_inst.push_last_float(cx, &self.animator, Self::instance_down());
        
        self.text.text_style = Self::text_style_label().get(cx);
        self.text.color = self.animator.last_color(cx, Text::instance_color());
        
        self._text_area = self.text.draw_text(cx, label);
        
        self._bg_area = self.bg.end_quad(cx, &bg_inst);
        self.animator.set_area(cx, self._bg_area);
    }
}
