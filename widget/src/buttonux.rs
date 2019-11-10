use render::*;

#[derive(Clone)]
pub struct ButtonUx {
}

#[derive(Clone, PartialEq)]
pub enum ButtonUxEvent {
    Animate,
    AnimEnded,
    Over,
    Default,
    Down,
}

#[derive(Clone, PartialEq)]
pub enum ButtonEvent {
    None,
    Clicked,
    Down,
    Up 
}

impl ButtonUx {
    
    pub fn handle_button(&mut self, _cx: &mut Cx, _event: &mut Event) -> ButtonEvent {
        ButtonEvent::None
        //let mut ret_event = ButtonEvent::None;
        /*
        match event.hits(cx, self._bg_area, HitOpt::default()) {
            Event::Animate(ae) => self.animator.write_area(cx, self._bg_area, "bg.", ae.time),
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
        */
    }
    
    pub fn draw_button(&mut self, _cx: &mut Cx, _label: &str) {
        /*
        self.bg.color = self.animator.last_color(cx.id("bg.color"));

        let bg_inst = self.bg.begin_quad(cx, &self.bg_layout);

        bg_inst.push_last_color(cx, &self.animator, "bg.border_color");
        bg_inst.push_last_float(cx, &self.animator, "bg.glow_size");

        self.text.draw_text(cx, label);
        self._bg_area = self.bg.end_quad(cx, &bg_inst);
        self.animator.update_area_refs(cx, self._bg_area);
        */
    }
}
