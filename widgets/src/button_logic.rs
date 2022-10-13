use {
    crate::{
        makepad_derive_widget::*,
        makepad_platform::*,
        widget::*
    }
};

#[derive(Clone, PartialEq)]
pub enum ButtonState {
    Hover,
    Default,
    Pressed,
}

#[derive(Clone, WidgetAction)]
pub enum ButtonAction {
    None,
    Click,
    Press,
    Release
}

impl ButtonAction {
    pub fn is_click(&self) -> bool {
        match self {
            Self::Click => true,
            _ => false
        }
    }
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
            dispatch_action(cx, ButtonAction::Press);
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
            dispatch_action(cx, ButtonAction::Click);
            if fe.digit.has_hovers() {
                return Some(ButtonState::Hover);
            }
            return Some(ButtonState::Default)
        }
        else {
            dispatch_action(cx, ButtonAction::Release);
            return Some(ButtonState::Default)
        }
        _ => ()
    };
    None
}