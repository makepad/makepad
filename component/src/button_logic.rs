use{
    crate::{
        makepad_platform::*,
        frame_registry::*
    }
};

#[derive(Default, Clone)]
pub struct ButtonLogic {
}

impl Default for ButtonAction {
    fn default() -> Self {Self::None}
}

#[derive(Copy, Debug, Clone, PartialEq)]
pub enum ButtonState {
    Hover,
    Default,
    Pressed,
    None
}

impl Default for ButtonState {
    fn default() -> Self {Self::None}
}

#[derive(Copy, Clone, PartialEq, IntoFrameComponentAction)]
pub enum ButtonAction {
    None,
    WasClicked,
    IsPressed,
    IsUp
}

#[derive(Copy, Clone, Default)]
pub struct ButtonHandleResult {
    pub action: ButtonAction,
    pub state: ButtonState
}

impl ButtonLogic {
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event, area: Area) -> ButtonHandleResult
    {
        match event.hits(cx, area) {
            HitEvent::FingerDown(_fe) => {
                return ButtonHandleResult {
                    action: ButtonAction::IsPressed,
                    state: ButtonState::Pressed
                };
            },
            HitEvent::FingerHover(fe) => {
                cx.set_hover_mouse_cursor(MouseCursor::Hand);
                match fe.hover_state {
                    HoverState::In => if fe.any_down {
                        return ButtonHandleResult {
                            action: ButtonAction::None,
                            state: ButtonState::Pressed
                        };
                    }
                    else {
                        return ButtonHandleResult {
                            action: ButtonAction::None,
                            state: ButtonState::Hover
                        };
                    },
                    HoverState::Out => return ButtonHandleResult {
                        action: ButtonAction::None,
                        state: ButtonState::Default
                    },
                    _ => ()
                }
            },
            HitEvent::FingerUp(fe) => if fe.is_over {
                if fe.input_type.has_hovers() {
                    return ButtonHandleResult {
                        action: ButtonAction::WasClicked,
                        state: ButtonState::Hover
                    };
                }
                return ButtonHandleResult {
                    action: ButtonAction::WasClicked,
                    state: ButtonState::Default
                };
            }
            else {
                return ButtonHandleResult {
                    action: ButtonAction::IsUp,
                    state: ButtonState::Default
                };
            }
            _ => ()
        };
        ButtonHandleResult::default()
    }
}
