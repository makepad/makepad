use render::*;
use crate::button::*;

#[derive(Clone)]
pub struct DesktopButton {
    pub bg: Quad,
    pub animator: Animator,
    pub _bg_area: Area,
}

pub enum DesktopButtonType {
    WindowsMin,
    WindowsMax,
    WindowsMaxToggled,
    WindowsClose,
    VRMode,
}

impl DesktopButtonType {
    fn shader_float(&self) -> f32 {
        match self {
            DesktopButtonType::WindowsMin => 1.,
            DesktopButtonType::WindowsMax => 2.,
            DesktopButtonType::WindowsMaxToggled => 3.,
            DesktopButtonType::WindowsClose => 4.,
            DesktopButtonType::VRMode => 5.,
        }
    }
}

impl DesktopButton {
    pub fn style(cx: &mut Cx) -> Self {
        Self {
            bg: Quad {
                shader: cx.add_shader(Self::def_bg_shader(), "Button.bg"),
                ..Quad::style(cx)
            },
            animator: Animator::new(Self::get_default_anim(cx)),
            _bg_area: Area::Empty,
        }
    }
    
    pub fn get_default_anim(cx:&Cx)->Anim{
        Anim::new(Play::Cut {duration: 0.2}, vec![
            Track::color(cx.id("bg.color"), Ease::Lin, vec![(1.0, color("#a"))]),
            Track::float(cx.id("bg.hover"), Ease::Lin, vec![(1.0, 0.)]),
            Track::float(cx.id("bg.down"), Ease::Lin, vec![(1.0, 0.)]),
        ])
    }
    
    pub fn get_over_anim(cx:&Cx)->Anim{
        Anim::new(Play::Cut {duration: 0.2}, vec![
            Track::color(cx.id("bg.color"), Ease::Lin, vec![(0.0, color("#f")), (1.0, color("#f"))]),
            Track::float(cx.id("bg.down"), Ease::Lin, vec![(1.0, 0.)]),
            Track::float(cx.id("bg.hover"), Ease::Lin, vec![(0.0, 1.0), (1.0, 1.0)]),
        ])
    }
    
    pub fn get_down_anim(cx:&Cx)->Anim{
        Anim::new(Play::Cut {duration: 0.2}, vec![
            Track::color(cx.id("bg.color"), Ease::Lin, vec![(0.0, color("#f55")), (1.0, color("#f55"))]),
            Track::float(cx.id("bg.down"), Ease::OutExp, vec![(0.0, 0.0), (1.0, 3.1415 * 0.5)]),
            Track::float(cx.id("bg.hover"), Ease::Lin, vec![(1.0, 1.0)]),
        ])
    }

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
                // VRMode
                if abs(button_type - 5.) < 0.1 {
                    df_clear(mix(color("#3"), mix(color("#0aa"), color("#077"), down), hover));
                    let w = 12.;
                    let h = 8.;
                    df_box(c.x - w, c.y - h, 2. * w, 2. * h, 2.);
                    // subtract 2 eyes
                    df_circle(c.x - 5.5,c.y,3.5);
                    df_subtract();
                    df_circle(c.x + 5.5,c.y,3.5);
                    df_subtract();
                    df_circle(c.x, c.y + h-0.75,2.5);
                    df_subtract();
                    df_fill(color("#8"));
                    
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
            Event::Animate(ae) => self.animator.write_area2(cx, self._bg_area, "bg.", ae.time),
            Event::AnimEnded(_) => self.animator.end(),
            Event::FingerDown(_fe) => {
                self.animator.play_anim(cx, Self::get_down_anim(cx));
                return ButtonEvent::Down;
            },
            Event::FingerHover(fe) => {
                cx.set_hover_mouse_cursor(MouseCursor::Default);
                match fe.hover_state {
                    HoverState::In => if fe.any_down {
                        self.animator.play_anim(cx, Self::get_down_anim(cx))
                    }
                    else {
                        self.animator.play_anim(cx, Self::get_over_anim(cx))
                    },
                    HoverState::Out => {
                        self.animator.play_anim(cx, Self::get_default_anim(cx))
                    }
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
    
    pub fn draw_desktop_button(&mut self, cx: &mut Cx, ty: DesktopButtonType) {
        self.bg.color = self.animator.last_color(cx.id("bg.color"));
        
        let (w,h) = match ty {
            DesktopButtonType::WindowsMin 
            | DesktopButtonType::WindowsMax 
            | DesktopButtonType::WindowsMaxToggled 
            | DesktopButtonType::WindowsClose => (46.,29.),
            DesktopButtonType::VRMode => (50.,36.),
        };

        let bg_inst = self.bg.draw_quad_walk(cx, Bounds::Fix(w), Bounds::Fix(h), Margin::zero());
        bg_inst.push_last_float(cx, &self.animator, "bg.hover");
        bg_inst.push_last_float(cx, &self.animator, "bg.down");
        bg_inst.push_float(cx, ty.shader_float());
        self._bg_area = bg_inst.into_area();
        self.animator.update_area_refs(cx, self._bg_area); // if our area changed, update animation
    }
}
