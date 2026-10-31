//! The host side of a hosted window's input: a view that shows a hosted
//! window hands its hits here. Positions are mapped to the window's points
//! by the caller (`local`: absolute position to window points, None when the
//! view has no rect yet). Focus, cursor and IME placement stay the view's.

use makepad_platform::*;
use makepad_studio_protocol::MouseButton;

use crate::HostedApp;

/// What `forward_hit` did with a hit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Forwarded {
    /// Not input for the child.
    No,
    /// Sent to the child.
    Yes,
    /// A press: sent, and the view should take the key focus.
    Press,
}

/// Send one hit of the view to its hosted window.
pub fn forward_hit(hosted: &mut HostedApp, hit: &Hit, local: impl Fn(Vec2d) -> Option<Vec2d>) -> Forwarded {
    fn button(device: &DigitDevice) -> MouseButton {
        device.mouse_button().unwrap_or(MouseButton::PRIMARY)
    }
    match hit {
        Hit::FingerDown(e) => {
            let Some(at) = local(e.abs) else { return Forwarded::No };
            hosted.mouse_down(at, button(&e.device), e.time, &e.modifiers);
            return Forwarded::Press;
        }
        Hit::FingerMove(e) => {
            let Some(at) = local(e.abs) else { return Forwarded::No };
            hosted.mouse_move(at, e.time, &e.modifiers);
        }
        Hit::FingerHoverIn(e) | Hit::FingerHoverOver(e) => {
            let Some(at) = local(e.abs) else { return Forwarded::No };
            hosted.hover_move(at, e.time, &e.modifiers);
        }
        Hit::FingerUp(e) => {
            hosted.mouse_up(local(e.abs), button(&e.device), e.time, &e.modifiers, e.cancelled);
        }
        Hit::FingerScroll(e) => {
            let Some(at) = local(e.abs) else { return Forwarded::No };
            // `FingerScroll` reports every scroll as a mouse device; a
            // trackpad gesture is the one that carries a phase.
            let is_mouse = e.device.is_mouse() && e.phase == makepad_platform::event::ScrollPhase::None;
            hosted.scroll(at, e.scroll, is_mouse, e.phase, e.time, &e.modifiers);
        }
        Hit::FingerPinch(e) => {
            let Some(at) = local(e.abs) else { return Forwarded::No };
            hosted.pinch(at, e.scale, e.phase, e.time, &e.modifiers);
        }
        Hit::TextInput(e) => hosted.text_input(e.clone()),
        Hit::KeyDown(e) => hosted.key_down(e.clone()),
        Hit::KeyUp(e) => hosted.key_up(e.clone()),
        Hit::TextCopy(_) => hosted.text_copy(),
        Hit::TextCut(_) => hosted.text_cut(),
        _ => return Forwarded::No,
    }
    Forwarded::Yes
}
