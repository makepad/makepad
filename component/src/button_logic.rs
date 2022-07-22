use {
    crate::{
        makepad_platform::*,
        frame_component::*
    }
};

#[derive(Clone, PartialEq)]
pub enum ButtonState {
    Hover,
    Default,
    Pressed,
    None
}

impl Default for ButtonState {
    fn default() -> Self {Self::None}
}

#[derive(Clone, FrameAction)]
pub enum ButtonAction {
    None,
    WasClicked,
    IsPressed,
    IsUp
}

pub fn button_logic_handle_event(
    cx: &mut Cx,
    event: &mut Event,
    area: Area,
    dispatch_action: &mut dyn FnMut(&mut Cx, ButtonAction)
) -> ButtonState
{
    match event.hits(cx, area) {
        HitEvent::FingerDown(_fe) => {
            dispatch_action(cx, ButtonAction::IsPressed);
            return ButtonState::Pressed;
        },
        HitEvent::FingerHover(fe) => {
            cx.set_hover_mouse_cursor(MouseCursor::Hand);
            match fe.hover_state {
                HoverState::In => if fe.any_down {
                    return ButtonState::Pressed;
                }
                else {
                    return ButtonState::Hover;
                },
                HoverState::Out => return ButtonState::Default,
                _ => ()
            }
        },
        HitEvent::FingerUp(fe) => if fe.is_over {
            dispatch_action(cx, ButtonAction::WasClicked);
            if fe.input_type.has_hovers() {
                return ButtonState::Hover;
            }
            return ButtonState::Default;
        }
        else {
            dispatch_action(cx, ButtonAction::IsUp);
            return ButtonState::Default
        }
        _ => ()
    };
    ButtonState::Default
}