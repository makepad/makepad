use render::*;
use crate::buttonlogic::*;
use crate::widgettheme::*;

#[derive(Clone)]
pub struct NormalButton {
    pub class: ClassId,
    pub button: ButtonLogic,
    pub bg: Quad,
    //pub bg_layout: LayoutId,
    pub text: Text,
    pub animator: Animator,
    pub _bg_area: Area,
}

instance_color!(NormalButton_border_color);
instance_float!(NormalButton_glow_size);

impl NormalButton {
    pub fn style(cx: &mut Cx) -> Self {
        Self {
            class:ThemeBase::id(),
            //bg_layout: NormalButton_layout_bg::id(),
            button: ButtonLogic::default(),
            bg: Quad::style_with_shader(cx, Self::def_bg_shader(), "Button.bg"),
            text: Text::style(cx, TextStyle_normal::id()),
            animator: Animator::new(Self::get_default_anim(cx)),
            _bg_area: Area::Empty,
        }
    }
    
    pub fn get_default_anim(_cx: &Cx) -> Anim {
        Anim::new(Play::Cut {duration: 0.5}, vec![
            Track::color_id(Quad_color::id(), Ease::Lin, vec![(1., Color_bg_normal::id())]),
            Track::float(NormalButton_glow_size::id(), Ease::Lin, vec![(1., 0.0)]),
            Track::color(NormalButton_border_color::id(), Ease::Lin, vec![(1., color("#6"))]),
        ])
    }
    
    pub fn get_over_anim(_cx: &Cx) -> Anim {
        Anim::new(Play::Cut {duration: 0.05}, vec![
            Track::color(Quad_color::id(), Ease::Lin, vec![(1., color("#999"))]),
            Track::float(NormalButton_glow_size::id(), Ease::Lin, vec![(1., 1.0)]),
            Track::color(NormalButton_border_color::id(), Ease::Lin, vec![(1., color("white"))]),
        ])
    }
    
    pub fn get_down_anim(_cx: &Cx) -> Anim {
        Anim::new(Play::Cut {duration: 0.2}, vec![
            Track::color(Quad_color::id(), Ease::Lin, vec![(0.0, color("#f")), (1.0, color("#6"))]),
            Track::float(NormalButton_glow_size::id(), Ease::Lin, vec![(0.0, 1.0), (1.0, 1.0)]),
            Track::color(NormalButton_border_color::id(), Ease::Lin, vec![(0.0, color("white")), (1.0, color("white"))]),
        ])
    }
    
    pub fn def_bg_shader() -> ShaderGen {
        Quad::def_quad_shader().compose(shader_ast!({
            
            let border_color: NormalButton_border_color;
            let glow_size: NormalButton_glow_size;
            
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
        }))
    }
    
    pub fn handle_button(&mut self, cx: &mut Cx, event: &mut Event) -> ButtonEvent {
        //let mut ret_event = ButtonEvent::None;
        let animator = &mut self.animator;
        let class = self.class;
        self.button.handle_button_logic(cx, event, self._bg_area, | cx, logic_event, area | match logic_event {
            ButtonLogicEvent::Animate(ae) => animator.write_area(cx, class, area, ae.time),
            ButtonLogicEvent::AnimEnded(_) => animator.end(),
            ButtonLogicEvent::Down => animator.play_anim(cx, Self::get_down_anim(cx)),
            ButtonLogicEvent::Default => animator.play_anim(cx, Self::get_default_anim(cx)),
            ButtonLogicEvent::Over => animator.play_anim(cx, Self::get_over_anim(cx))
        })
    }
    
    pub fn draw_button(&mut self, cx: &mut Cx, label: &str) {
        self.bg.color = self.animator.last_color(cx, self.class, Quad_color::id());
        
        let bg_inst = self.bg.begin_quad(cx, NormalButton_layout_bg::get_class(cx, self.class));
        
        bg_inst.push_last_color(cx, self.class, &self.animator, NormalButton_border_color::id());
        bg_inst.push_last_float(cx, &self.animator, NormalButton_glow_size::id());

        self.text.draw_text(cx, label);

        self._bg_area = self.bg.end_quad(cx, &bg_inst);
        self.animator.update_area_refs(cx, self._bg_area);
    }
}
