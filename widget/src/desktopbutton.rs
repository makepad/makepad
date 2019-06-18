use render::*;
use crate::button::*;

#[derive(Clone)]
pub struct DesktopButton {
    pub bg: Quad,
    pub animator: Animator,
    pub anim_over: Anim,
    pub anim_down: Anim,
    pub _bg_area: Area,
}

impl Style for DesktopButton {
    fn style(cx: &mut Cx) -> Self {
        Self {
            bg: Quad {
                shader: cx.add_shader(Self::def_bg_shader(), "Button.bg"),
                ..Style::style(cx)
            },
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
            _bg_area: Area::Empty,
        }
    }
}

pub enum DesktopButtonType {
    WindowsMin,
    WindowsMax,
    WindowsMaxToggled,
    WindowsClose,
    OSXMin,
    OSXMax,
    OSXClose
}

impl DesktopButtonType {
    fn shader_float(&self) -> f32 {
        match self {
            DesktopButtonType::WindowsMin => 1.,
            DesktopButtonType::WindowsMax => 2.,
            DesktopButtonType::WindowsMaxToggled => 3.,
            DesktopButtonType::WindowsClose => 4.,
            DesktopButtonType::OSXMin => 5.,
            DesktopButtonType::OSXMax => 6.,
            DesktopButtonType::OSXClose => 7.,
        }
    }
}

impl DesktopButton {
    pub fn def_bg_shader() -> ShaderGen {
        Quad::def_quad_shader().compose(shader_ast!({
            
            let hover: float<Instance>;
            let down: float<Instance>;
            let button_type: float<Instance>;
            
            fn pixel() -> vec4 {
                df_viewport(pos * vec2(w, h)); // );
                df_aa *= 3.0;
                let sz = 4.5;
                let c = vec2(w, h) * vec2(0.5, 0.5);
                // WindowsMin
                if abs(button_type - 1.) < 0.1 {
                    df_clear(mix(color("#3"), mix(color("#6"), color("#9"), down), hover));
                    df_move_to(c.x - sz, c.y);
                    df_line_to(c.x + sz, c.y);
                    df_stroke(color("white"), 0.5 + 0.5 * dpi_dilate);
                    return df_result;
                }
                // WindowsMax
                if abs(button_type - 2.) < 0.1 {
                    df_clear(mix(color("#3"), mix(color("#6"), color("#9"), down), hover));
                    df_rect(c.x - sz, c.y - sz, 2. * sz, 2. * sz);
                    df_stroke(color("white"), 0.5 + 0.5 * dpi_dilate);
                    return df_result;
                }
                // WindowsMaxToggled
                if abs(button_type - 3.) < 0.1 {
                    let clear = mix(color("#3"), mix(color("#6"), color("#9"), down), hover);
                    df_clear(clear);
                    let sz = 3.5;
                    df_rect(c.x - sz + 1., c.y - sz - 1., 2. * sz, 2. * sz);
                    df_stroke(color("white"), 0.5 + 0.5 * dpi_dilate);
                    df_rect(c.x - sz - 1., c.y - sz + 1., 2. * sz, 2. * sz);
                    df_fill_keep(clear);
                    df_stroke(color("white"), 0.5 + 0.5 * dpi_dilate);
                    
                    return df_result;
                }
                // WindowsClose
                if abs(button_type - 4.) < 0.1 { 
                    df_clear(mix(color("#3"), mix(color("#e00"), color("#c00"), down), hover));
                    df_move_to(c.x - sz, c.y - sz);
                    df_line_to(c.x + sz, c.y + sz);
                    df_move_to(c.x - sz, c.y + sz);
                    df_line_to(c.x + sz, c.y - sz);
                    df_stroke(color("white"), 0.5 + 0.5 * dpi_dilate);
                    return df_result;
                }
                
                return color("red")/*
                df_viewport(pos * vec2(w, h));
                df_box(0., 0., w, h, border_radius);
                df_shape += 3.;
                df_fill_keep(color);
                df_stroke_keep(border_color, border_width);
                df_blur = 2.;
                return df_glow(glow_color, glow_size);*/
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
            Event::FingerHover(fe) => match fe.hover_state {
                HoverState::In => if fe.any_down {
                    self.animator.play_anim(cx, self.anim_down.clone())
                }
                else {
                    self.animator.play_anim(cx, self.anim_over.clone())
                },
                HoverState::Out => self.animator.play_default(cx),
                _ => ()
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
    
    pub fn draw_desktop_button(&mut self, cx: &mut Cx, _ty: DesktopButtonType) {
        self.bg.color = self.animator.last_color("bg.color");
        let bg_inst = self.bg.draw_quad_walk(cx, Bounds::Fix(46.), Bounds::Fix(29.), Margin::zero());
        bg_inst.push_float(cx, self.animator.last_float("bg.hover"));
        bg_inst.push_float(cx, self.animator.last_float("bg.down"));
        bg_inst.push_float(cx, _ty.shader_float());
        self._bg_area = bg_inst.into_area();
        self.animator.update_area_refs(cx, self._bg_area); // if our area changed, update animation
    }
}
