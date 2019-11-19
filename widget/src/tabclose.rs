use render::*;
use crate::buttonlogic::*;
use crate::widgettheme::*;

#[derive(Clone)]
pub struct TabClose {
    pub bg: Quad,
    pub animator: Animator,
    pub _bg_area: Area,
}

impl TabClose {
    pub fn proto(cx: &mut Cx) -> Self {
        Self {
            bg: Quad::proto_with_shader(cx, Self::def_bg_shader(), "TabClose.bg"),
            animator: Animator::default(),
            _bg_area: Area::Empty,
        }
    }
    
    pub fn walk()->WalkId{uid!()}
    pub fn instance_hover()->InstanceFloat{uid!()}
    pub fn instance_down()->InstanceFloat{uid!()}
    
    pub fn theme(cx: &mut Cx){
        Self::walk().set_base(cx, Walk {
            width: Width::Fix(10.),
            height: Height::Fix(10.),
            margin: Margin {l: -4., t: 0., r: 4., b: 0.}
        });
    }
    
    pub fn get_default_anim(cx:&Cx)->Anim{
        Anim::new(Play::Cut {duration: 0.2}, vec![
            Track::color(Quad::instance_color(), Ease::Lin, vec![(1.0, Theme::color_text_deselected_focus().base(cx))]),
            Track::float(Self::instance_hover(), Ease::Lin, vec![(1.0, 0.)]),
            Track::float(Self::instance_down(), Ease::Lin, vec![(1.0, 0.)]),
        ])
    }
    
    pub fn get_over_anim(cx:&Cx)->Anim{
        Anim::new(Play::Cut {duration: 0.2}, vec![
            Track::color(Quad::instance_color(), Ease::Lin, vec![(0.0, Theme::color_text_selected_focus().base(cx)), (1.0, Theme::color_text_selected_focus().base(cx))]),
            Track::float(Self::instance_hover(), Ease::Lin, vec![(1.0, 1.0)]),
            Track::float(Self::instance_down(), Ease::Lin, vec![(1.0, 0.)]),
        ])
    }
    
    pub fn get_down_anim(cx:&Cx)->Anim{
        Anim::new(Play::Cut {duration: 0.2}, vec![
            Track::color(Quad::instance_color(), Ease::Lin, vec![(0.0, Theme::color_text_selected_focus().base(cx)), (1.0, Theme::color_text_selected_focus().base(cx))]),
            Track::float(Self::instance_hover(), Ease::Lin, vec![(1.0, 1.0)]),
            Track::float(Self::instance_down(), Ease::OutExp, vec![(0.0, 0.0), (1.0, 3.1415 * 0.5)]),
        ])
    }

    pub fn def_bg_shader() -> ShaderGen {
        Quad::def_quad_shader().compose(shader_ast!({
            let hover: Self::instance_hover();
            let down: Self::instance_down();
            fn pixel() -> vec4 {
                df_viewport(pos * vec2(w, h));
                let hover_max: float = (hover * 0.2 + 0.8) * 0.5;
                let hover_min: float = 1. - hover_max;
                let c: vec2 = vec2(w, h) * 0.5;
                df_rotate(down, c.x, c.y);
                df_move_to(c.x * hover_min, c.y * hover_min);
                df_line_to(c.x + c.x * hover_max, c.y + c.y * hover_max);
                df_move_to(c.x + c.x * hover_max, c.y * hover_min);
                df_line_to(c.x * hover_min, c.y + c.y * hover_max);
                return df_stroke(color, 1. + down * 0.2);
                //return df_fill(color);
            }
        }))
    }
    
    pub fn handle_tab_close(&mut self, cx: &mut Cx, event: &mut Event) -> ButtonEvent {
        match event.hits(cx, self._bg_area, HitOpt {
            margin: Some(Margin {l: 5., t: 5., r: 5., b: 5.}),
            ..Default::default()
        }) {
            Event::Animate(ae) => self.animator.calc_area(cx, self._bg_area, ae.time),
            Event::FingerDown(_fe) => {
                self.animator.play_anim(cx, Self::get_down_anim(cx));
                cx.set_down_mouse_cursor(MouseCursor::Hand);
                return ButtonEvent::Down;
            },
            Event::FingerHover(fe) => {
                cx.set_hover_mouse_cursor(MouseCursor::Hand);
                match fe.hover_state {
                    HoverState::In => if fe.any_down {
                        self.animator.play_anim(cx, Self::get_down_anim(cx))
                    }
                    else {
                        self.animator.play_anim(cx, Self::get_over_anim(cx))
                    },
                    HoverState::Out => self.animator.play_anim(cx, Self::get_default_anim(cx)),
                    _ => ()
                }
            },
            Event::FingerUp(fe) => if fe.is_over {
                if !fe.is_touch {self.animator.play_anim(cx, Self::get_over_anim(cx))}
                else {self.animator.play_anim(cx, Self::get_default_anim(cx))}
                return ButtonEvent::Clicked;
            }
            else {
                self.animator.play_anim(cx, Self::get_default_anim(cx));
                return ButtonEvent::Up;
            }
            _ => ()
        };
        ButtonEvent::None
    }
    
    pub fn draw_tab_close(&mut self, cx: &mut Cx) {
        self.animator.init(cx, |cx| Self::get_default_anim(cx));
        self.bg.color = self.animator.last_color(cx, Quad::instance_color());
        let bg_inst = self.bg.draw_quad(cx, Self::walk().base(cx));
        bg_inst.push_last_float(cx, &self.animator, Self::instance_hover());
        bg_inst.push_last_float(cx, &self.animator, Self::instance_down());
        self._bg_area = bg_inst.into();
        self.animator.set_area(cx, self._bg_area); // if our area changed, update animation
    }
}
