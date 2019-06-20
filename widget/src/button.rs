use render::*;

#[derive(Clone)]
pub struct Button {
    pub bg: Quad,
    pub bg_layout: Layout,
    pub text: Text,
    pub animator: Animator,
    pub anim_over: Anim,
    pub anim_down: Anim,
    pub _bg_area: Area,
}

impl Style for Button {
    fn style(cx: &mut Cx) -> Self {
        Self {
            bg: Quad {
                shader: cx.add_shader(Self::def_bg_shader(), "Button.bg"),
                ..Style::style(cx)
            },
            bg_layout: Layout {
                align: Align::center(),
                width: Bounds::Compute,
                height: Bounds::Compute,
                margin: Margin::all(1.0),
                padding: Padding {l: 16.0, t: 14.0, r: 16.0, b: 14.0},
                ..Default::default()
            },
            text: Text::style(cx),
            animator: Animator::new(Anim::new(Play::Cut {duration: 0.5}, vec![
                Track::color("bg.color", Ease::Lin, vec![(1., cx.color("bg_normal"))]),
                Track::float("bg.glow_size", Ease::Lin, vec![(1., 0.0)]),
                Track::color("bg.border_color", Ease::Lin, vec![(1., color("#6"))]),
                Track::color("text.color", Ease::Lin, vec![(1., color("white"))]),
                Track::color("icon.color", Ease::Lin, vec![(1., color("white"))])
            ])),
            anim_over: Anim::new(Play::Cut {duration: 0.05}, vec![
                Track::color("bg.color", Ease::Lin, vec![(1., color("#999"))]),
                Track::color("bg.border_color", Ease::Lin, vec![(1., color("white"))]),
                Track::float("bg.glow_size", Ease::Lin, vec![(1., 1.0)])
            ]),
            anim_down: Anim::new(Play::Cut {duration: 0.2}, vec![
                Track::color("bg.border_color", Ease::Lin, vec![(0.0, color("white")), (1.0, color("white"))]),
                Track::color("bg.color", Ease::Lin, vec![(0.0, color("#f")), (1.0, color("#6"))]),
                Track::float("bg.glow_size", Ease::Lin, vec![(0.0, 1.0), (1.0, 1.0)]),
                Track::color("icon.color", Ease::Lin, vec![(0.0, color("#0")), (1.0, color("#f")),]),
            ]),
            _bg_area: Area::Empty,
        }
    }
}

#[derive(Clone, PartialEq)]
pub enum ButtonEvent {
    None,
    Clicked,
    Down,
    Up
}

impl Button {
    pub fn def_bg_shader() -> ShaderGen {
        Quad::def_quad_shader().compose(shader_ast!({
            
            let border_color: vec4<Instance>;
            let glow_size: float<Instance>;
            
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
        match event.hits(cx, self._bg_area, HitOpt::default()) {
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
    
    pub fn draw_button_with_label(&mut self, cx: &mut Cx, label: &str) {
        self.bg.color = self.animator.last_color("bg.color");
        let bg_inst = self.bg.begin_quad(cx, &self.bg_layout);
        bg_inst.push_color(cx, self.animator.last_color("bg.border_color"));
        bg_inst.push_float(cx, self.animator.last_float("bg.glow_size"));
        self.text.draw_text(cx, label);
        self._bg_area = self.bg.end_quad(cx, &bg_inst);
        self.animator.update_area_refs(cx, self._bg_area);
    }
}
