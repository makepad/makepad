use render::*;

#[derive(Default, Clone)]
pub struct ButtonLogic {
}

#[derive(Clone, PartialEq)]
pub enum ButtonLogicEvent {
    Animate(AnimateEvent),
    AnimEnded(AnimateEvent),
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

impl ButtonLogic {
    
    pub fn handle_button_logic<F>(&mut self, cx: &mut Cx, event: &mut Event, area:Area, mut cb:F) -> ButtonEvent
    where F: FnMut(&mut Cx, ButtonLogicEvent, Area)
    {
        match event.hits(cx, area, HitOpt::default()) {
            Event::Animate(ae) => cb(cx, ButtonLogicEvent::Animate(ae), area),
            Event::AnimEnded(ae) => cb(cx, ButtonLogicEvent::AnimEnded(ae), area),
            Event::FingerDown(_fe) => {
                cb(cx, ButtonLogicEvent::Down, area);
                return ButtonEvent::Down;
            },
            Event::FingerHover(fe) => {
                cx.set_hover_mouse_cursor(MouseCursor::Default);
                match fe.hover_state {
                    HoverState::In => if fe.any_down {
                        cb(cx, ButtonLogicEvent::Down, area);
                    }
                    else {
                        cb(cx, ButtonLogicEvent::Over, area);
                    },
                    HoverState::Out => cb(cx, ButtonLogicEvent::Default, area),
                    _ => ()
                }
            },
            Event::FingerUp(fe) => if fe.is_over {
                if !fe.is_touch {cb(cx, ButtonLogicEvent::Over, area)}
                else {cb(cx, ButtonLogicEvent::Default, area)}
                return ButtonEvent::Clicked;
            }
            else {
                cb(cx, ButtonLogicEvent::Default, area);
                return ButtonEvent::Up;
            }
            _ => ()
        };
        ButtonEvent::None
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
