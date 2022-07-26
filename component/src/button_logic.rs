use {
    crate::{
        makepad_derive_frame::*,
        makepad_platform::*,
        frame_traits::*
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
    event: &mut Event,
    area: Area,
    dispatch_action: &mut dyn FnMut(&mut Cx, ButtonAction)
) -> Option<ButtonState>
{
    match event.hits(cx, area) {
        Hit::FingerDown(_fe) => {
            dispatch_action(cx, ButtonAction::IsPressed);
            return Some(ButtonState::Pressed);
        },
        Hit::FingerHoverIn(fe) => {
            cx.set_hover_cursor(MouseCursor::Hand);
            if fe.any_down {
                return Some(ButtonState::Pressed);
            }
            else {
                return Some(ButtonState::Hover);
            }
        }
        Hit::FingerHoverOut(_) => {
            return Some(ButtonState::Default)
        }
        Hit::FingerUp(fe) => if fe.is_over {
            dispatch_action(cx, ButtonAction::WasClicked);
            if fe.input_type.has_hovers() {
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