use render::*;
use crate::buttonlogic::*;
use crate::widgettheme::*;

#[derive(Clone)]
pub struct NormalButton {
    pub class: ClassId,
    pub button: ButtonLogic,
    pub bg: Quad,
    pub text: Text,
    pub animator: Animator,
    pub _bg_area: Area,
}

impl NormalButton {
    pub fn proto(cx: &mut Cx) -> Self {
        Self {
            class: ClassId::base(),
            button: ButtonLogic::default(),
            bg: Quad::proto(cx),
            text: Text::proto(cx),
            animator: Animator::default(),
            _bg_area: Area::Empty,
        }
    }
    
    pub fn layout_bg() -> LayoutId {uid!()}
    pub fn text_style_label() -> TextStyleId {uid!()}
    pub fn anim_default() -> AnimId {uid!()}
    pub fn anim_over() -> AnimId {uid!()}
    pub fn anim_down() -> AnimId {uid!()}
    pub fn shader_bg() -> ShaderId {uid!()}
    pub fn instance_border_color() -> InstanceColor {uid!()}
    pub fn instance_glow_size() -> InstanceFloat {uid!()}
    
    pub fn theme(cx: &mut Cx) { 
        Self::layout_bg().set_base(cx, Layout {
            align: Align::center(),
            walk: Walk {
                width: Width::Compute,
                height: Height::Compute,
                margin: Margin::all(1.0),
            },
            padding: Padding {l: 16.0, t: 14.0, r: 16.0, b: 14.0},
            ..Default::default()
        });
        
        Self::text_style_label().set_base(cx, Theme::text_style_normal().base(cx));
        
        Self::anim_default().set_base(cx, Anim::new(Play::Cut {duration: 0.5}, vec![
            Track::color(Quad::instance_color(), Ease::Lin, vec![(1., Theme::color_bg_normal().base(cx))]),
            Track::float(Self::instance_glow_size(), Ease::Lin, vec![(1., 0.0)]),
            Track::color(Self::instance_border_color(), Ease::Lin, vec![(1., color("#6"))]),
        ]));
        
        Self::anim_over().set_base(cx, Anim::new(Play::Cut {duration: 0.05}, vec![
            Track::color(Quad::instance_color(), Ease::Lin, vec![(1., color("#999"))]),
            Track::float(Self::instance_glow_size(), Ease::Lin, vec![(1., 1.0)]),
            Track::color(Self::instance_border_color(), Ease::Lin, vec![(1., color("white"))]),
        ]));
        
        Self::anim_down().set_base(cx, Anim::new(Play::Cut {duration: 0.2}, vec![
            Track::color(Quad::instance_color(), Ease::Lin, vec![(0.0, color("#f")), (1.0, color("#6"))]),
            Track::float(Self::instance_glow_size(), Ease::Lin, vec![(0.0, 1.0), (1.0, 1.0)]),
            Track::color(Self::instance_border_color(), Ease::Lin, vec![(0.0, color("white")), (1.0, color("white"))]),
        ]));
        
        // lets define the shader
        Self::shader_bg().set_base(cx, Quad::def_quad_shader().compose(shader_ast!({
            
            let border_color: Self::instance_border_color();
            let glow_size: Self::instance_glow_size();
            
            const glow_color: vec4 = color("#30f");
            const border_radius: float = 6.5;
            const border_width: float = 1.0;
            
            fn pixel() -> vec4 {
                df_viewport(pos * vec2(w, h));
                df_box(0., 0., w, h, border_radius);
                df_shape += 3.;
                df_fill_keep(color);
                df_stroke_keep(border_color, border_width);
                df_blur = 2.;
                return df_glow(glow_color, glow_size);
            }
        })));
    }
    
    pub fn handle_button(&mut self, cx: &mut Cx, event: &mut Event) -> ButtonEvent {
        //let mut ret_event = ButtonEvent::None;
        let animator = &mut self.animator;
        let class = self.class;
        self.button.handle_button_logic(cx, event, self._bg_area, | cx, logic_event, area | match logic_event {
            ButtonLogicEvent::Animate(ae) => animator.calc_area(cx, area, ae.time),
            ButtonLogicEvent::AnimEnded(_) => animator.end(),
            ButtonLogicEvent::Down => animator.play_anim(cx, Self::anim_down().class(cx, class)),
            ButtonLogicEvent::Default => animator.play_anim(cx, Self::anim_default().class(cx, class)),
            ButtonLogicEvent::Over => animator.play_anim(cx, Self::anim_over().class(cx, class))
        })
    }
    
    pub fn draw_button(&mut self, cx: &mut Cx, label: &str) {
        let class = self.class;
        
        self.bg.shader = Self::shader_bg().class(cx, class);
        
        self.animator.init(cx, | cx | Self::anim_default().class(cx, class));

        self.bg.color = self.animator.last_color(cx, Quad::instance_color());
        
        let bg_inst = self.bg.begin_quad(cx, Self::layout_bg().class(cx, class));

        bg_inst.push_last_color(cx, &self.animator, Self::instance_border_color());
        bg_inst.push_last_float(cx, &self.animator, Self::instance_glow_size());
        
        self.text.text_style = Self::text_style_label().class(cx, class);
        self.text.draw_text(cx, label);
        
        self._bg_area = self.bg.end_quad(cx, &bg_inst);
        self.animator.set_area(cx, self._bg_area);
    }
}
