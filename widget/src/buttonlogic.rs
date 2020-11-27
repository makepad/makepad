use makepad_render::*;

#[derive(Default, Clone)]
pub struct ButtonLogic {
}

#[derive(Clone, PartialEq)]
pub enum ButtonLogicEvent {
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
            Event::FingerDown(_fe) => {
                cb(cx, ButtonLogicEvent::Down, area);
                return ButtonEvent::Down;
            },
            Event::FingerHover(fe) => {
                cx.set_hover_mouse_cursor(MouseCursor::Hand);
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
                if fe.input_type.has_hovers() {cb(cx, ButtonLogicEvent::Over, area)}
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

}
