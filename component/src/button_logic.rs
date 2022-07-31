use {
    crate::{
        makepad_derive_frame::*,
        makepad_platform::*,
        frame::*
    }
};

#[derive(Clone, PartialEq)]
pub enum ButtonState {
    Hover,
    Default,
    Pressed,
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
    event: &Event,
    area: Area,
    dispatch_action: &mut dyn FnMut(&mut Cx, ButtonAction)
) -> Option<ButtonState>
{
    match event.hits(cx, area) {
        Hit::FingerDown(_fe) => {
            dispatch_action(cx, ButtonAction::IsPressed);
            return Some(ButtonState::Pressed);
        },
        Hit::FingerHoverIn(_) => {
            cx.set_cursor(MouseCursor::Hand);
            return Some(ButtonState::Hover);
        }
        Hit::FingerHoverOut(_) => {
            return Some(ButtonState::Default)
        }
        Hit::FingerUp(fe) => if fe.is_over {
            dispatch_action(cx, ButtonAction::WasClicked);
            if fe.digit.has_hovers() {
                return Some(ButtonState::Hover);
            }
            return Some(ButtonState::Default)
        }
        else {
            dispatch_action(cx, ButtonAction::IsUp);
            return Some(ButtonState::Default)
        }
        _ => ()
    };
    None
}