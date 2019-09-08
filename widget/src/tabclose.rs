use render::*;
use crate::button::*;

#[derive(Clone)]
pub struct TabClose {
    pub bg: Quad,
    pub text: Text,
    pub animator: Animator,
    pub anim_over: Anim,
    pub anim_down: Anim,
    pub margin: Margin,
    pub _bg_area: Area,
}

impl TabClose {
    pub fn style(cx: &mut Cx) -> Self {
        Self {
            bg: Quad {
                shader: cx.add_shader(Self::def_bg_shader(), "TabClose.bg"),
                ..Quad::style(cx)
            },
            text: Text::style(cx),
            animator: Animator::new(Anim::new(Play::Cut {duration: 0.2}, vec![
                Track::color("bg.color", Ease::Lin, vec![(1.0, color("#a"))]),
                Track::float("bg.hover", Ease::Lin, vec![(1.0, 0.)]),
                Track::float("bg.down", Ease::Lin, vec![(1.0, 0.)]),
            ])),
            anim_over: Anim::new(Play::Cut {duration: 0.2}, vec![
                Track::color("bg.color", Ease::Lin, vec![(0.0, color("#f")), (1.0, color("#f"))]),
                Track::float("bg.down", Ease::Lin, vec![(1.0, 0.)]),
                Track::float("bg.hover", Ease::Lin, vec![(0.0, 1.0), (1.0, 1.0)]),
            ]),
            anim_down: Anim::new(Play::Cut {duration: 0.2}, vec![
                Track::color("bg.color", Ease::Lin, vec![(0.0, color("#f55")), (1.0, color("#f55"))]),
                Track::float("bg.hover", Ease::Lin, vec![(1.0, 1.0)]),
                Track::float("bg.down", Ease::OutExp, vec![(0.0, 0.0), (1.0, 3.1415 * 0.5)]),
            ]),
            margin: Margin::zero(),
            _bg_area: Area::Empty,
        }
    }

    pub fn def_bg_shader() -> ShaderGen {
        Quad::def_quad_shader().compose(shader_ast!({
            let hover: float<Instance>;
            let down: float<Instance>;
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
            Event::Animate(ae) => self.animator.write_area(cx, self._bg_area, "bg.", ae.time),
            Event::FingerDown(_fe) => {
                self.animator.play_anim(cx, self.anim_down.clone());
                return ButtonEvent::Down;
            },
            Event::FingerHover(fe) => {
                cx.set_hover_mouse_cursor(MouseCursor::Default);
                match fe.hover_state {
                    HoverState::In => if fe.any_down {
                        self.animator.play_anim(cx, self.anim_down.clone())
                    }
                    else {
                        self.animator.play_anim(cx, self.anim_over.clone())
                    },
                    HoverState::Out => self.animator.play_default(cx),
                    _ => ()
                }
            },
            Event::FingerUp(fe) => if fe.is_over {
                if !fe.is_touch {self.animator.play_anim(cx, self.anim_over.clone())}
                else {self.animator.play_default(cx)}
                return ButtonEvent::Clicked;
            }
            else {
                self.animator.play_default(cx);
                return ButtonEvent::Up;
            }
            _ => ()
        };
        ButtonEvent::None
    }
    
    pub fn draw_tab_close(&mut self, cx: &mut Cx) {
        self.bg.color = self.animator.last_color("bg.color");
        let bg_inst = self.bg.draw_quad_walk(cx, Bounds::Fix(10.), Bounds::Fix(10.), self.margin);
        bg_inst.push_float(cx, self.animator.last_float("bg.hover"));
        bg_inst.push_float(cx, self.animator.last_float("bg.down"));
        self._bg_area = bg_inst.into_area();
        self.animator.update_area_refs(cx, self._bg_area); // if our area changed, update animation
    }
}
