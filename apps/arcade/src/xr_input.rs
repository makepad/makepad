//! XR controllers and hands → the same per-player input every other device
//! sends (game.md §"XR input → player input").
//!
//! An XR player is not a special case in the simulation: the headset fills a
//! [`PadState`] and a camera yaw, exactly as a gamepad or a phone would, and
//! the packet that goes on the wire is indistinguishable. That is what lets a
//! Quest in the corner race against a laptop without the sim knowing which is
//! which.
//!
//! Handedness follows the room, not the game: the left stick walks, the right
//! stick turns, and either hand's trigger shoots — so a left-handed player is
//! never worse off.

use makepad_widgets::makepad_platform::event::xr::{XrController, XrHand, XrState};
use makepad_widgets::*;

/// Sticks below this read as centred — Quest sticks rest a hair off zero and
/// would otherwise walk the player into a wall while nobody touches them.
const STICK_DEAD_ZONE: f32 = 0.15;

/// Trigger/grip pull that counts as a press.
const ANALOG_PRESS: f32 = 0.5;

/// One frame of XR intent, ready to merge into a player's input.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct XrIntent {
    /// Left stick, camera-relative walk axes (x = strafe, z = forward).
    pub axis_x: f64,
    pub axis_z: f64,
    /// Right stick x, applied to the player's camera yaw (snap or smooth is
    /// the shell's choice; this is the raw request).
    pub turn: f32,
    pub jump: bool,
    pub shoot: bool,
    pub grab: bool,
    pub reset: bool,
    /// Head yaw in tracking space: what "forward" means for this player this
    /// tick. Travels in their input packet, so movement resolves against
    /// their own view without replicating a camera.
    pub head_yaw: f32,
}

fn dead_zoned(v: f32) -> f64 {
    if v.abs() < STICK_DEAD_ZONE {
        0.0
    } else {
        // Rescale past the dead zone so the first live degree is gentle
        // rather than a jump to 0.15.
        let sign = if v < 0.0 { -1.0 } else { 1.0 };
        let scaled = (v.abs() - STICK_DEAD_ZONE) / (1.0 - STICK_DEAD_ZONE);
        (sign * scaled.min(1.0)) as f64
    }
}

/// Yaw of a pose's forward vector, projected onto the floor plane.
///
/// A head tipped to look at your feet still means "forward is that way", so
/// the y component is dropped rather than normalised into the answer.
pub fn flat_yaw(orientation: Quat) -> f32 {
    let f = orientation.rotate_vec3(&vec3f(0.0, 0.0, -1.0));
    if f.x.abs() < 1.0e-6 && f.z.abs() < 1.0e-6 {
        // Looking straight up or down: no usable heading, keep the old one.
        return 0.0;
    }
    // Matches the engine's camera convention: forward = (sin y, -cos y).
    f.x.atan2(-f.z)
}

fn controller_active(c: &XrController) -> bool {
    c.buttons & XrController::ACTIVE != 0
}

/// Map a headset state to game intent. Controllers win when held; hands
/// take over when they are the only thing tracked, so putting a controller
/// down mid-game does not freeze the player.
pub fn intent_from_xr(state: &XrState) -> XrIntent {
    let mut intent = XrIntent {
        head_yaw: flat_yaw(state.head_pose.orientation),
        ..Default::default()
    };

    let left = &state.left_controller;
    let right = &state.right_controller;
    let any_controller = controller_active(left) || controller_active(right);

    if any_controller {
        if controller_active(left) {
            intent.axis_x = dead_zoned(left.stick.x);
            // Stick forward is +y in XR, and forward is -z in the game.
            intent.axis_z = -dead_zoned(left.stick.y);
            intent.grab = left.grip >= ANALOG_PRESS;
            intent.shoot |= left.trigger >= ANALOG_PRESS;
            intent.jump |= left.buttons & XrController::CLICK_X != 0;
            intent.reset |= left.buttons & XrController::CLICK_Y != 0;
        }
        if controller_active(right) {
            intent.turn = if right.stick.x.abs() < STICK_DEAD_ZONE {
                0.0
            } else {
                right.stick.x
            };
            intent.grab |= right.grip >= ANALOG_PRESS;
            intent.shoot |= right.trigger >= ANALOG_PRESS;
            intent.jump |= right.buttons & XrController::CLICK_A != 0;
            intent.reset |= right.buttons & XrController::CLICK_B != 0;
        }
        return intent;
    }

    // Hands-only: pinch is the whole vocabulary. Index pinch is the trigger
    // (shoot), middle pinch is grab — cheap, reliable, and no gesture the
    // player has to learn beyond "pinch to act".
    let hand_shoot = |h: &XrHand| h.pinch_index();
    let hand_grab = |h: &XrHand| h.pinch_middle();
    intent.shoot = hand_shoot(&state.left_hand) || hand_shoot(&state.right_hand);
    intent.grab = hand_grab(&state.left_hand) || hand_grab(&state.right_hand);
    intent
}

/// Fold XR intent into the pad half of a player's input. Digital keys are
/// untouched, so a headset and a keyboard can drive the same player.
pub fn apply_intent_to_pad(intent: &XrIntent, pad: &mut PadState) {
    let was_jump = pad.jump;
    let was_shoot = pad.shoot;
    let was_grab = pad.grab;
    let was_reset = pad.reset;

    pad.axis_x = intent.axis_x;
    pad.axis_z = intent.axis_z;
    pad.jump = intent.jump;
    pad.shoot = intent.shoot;
    pad.grab = intent.grab;
    pad.reset = intent.reset;

    // Edges, so a held trigger fires once — same contract as the gamepad.
    pad.jump_pressed = intent.jump && !was_jump;
    pad.shoot_pressed = intent.shoot && !was_shoot;
    pad.grab_pressed = intent.grab && !was_grab;
    pad.reset_pressed = intent.reset && !was_reset;
}

pub use makepad_game_sim::PadState;

/// The stage this device starts in, from `ARCADE_XR`.
///
/// `mr` is the Quest default in game.md — passthrough on, world shrunk onto
/// the floor. `vr` is full-scale for headsets with no passthrough. Anything
/// else (including unset) stays flat, which is what desktop and mobile want.
pub fn stage_from_env() -> Stage {
    match std::env::var("ARCADE_XR").as_deref() {
        Ok("mr") | Ok("MR") => {
            let scale = std::env::var("ARCADE_XR_SCALE")
                .ok()
                .and_then(|s| s.parse::<f32>().ok())
                .filter(|s| *s > 0.0)
                .unwrap_or(DIORAMA_SCALE);
            // Planted on the floor, 1.5m in front of where the player starts.
            Stage::mr_diorama(vec3f(0.0, 0.0, -1.5), 0.0, scale)
        }
        Ok("vr") | Ok("VR") => Stage::vr_full_scale(),
        _ => Stage::flat(),
    }
}

use makepad_game_render::stage::{Stage, DIORAMA_SCALE};

#[cfg(test)]
mod tests {
    use super::*;

    fn active(mut c: XrController) -> XrController {
        c.buttons |= XrController::ACTIVE;
        c
    }

    fn state_with(left: XrController, right: XrController) -> XrState {
        XrState {
            left_controller: left,
            right_controller: right,
            ..Default::default()
        }
    }

    #[test]
    fn resting_sticks_do_not_walk_the_player() {
        let left = active(XrController {
            stick: vec2f(0.08, -0.10),
            ..Default::default()
        });
        let intent = intent_from_xr(&state_with(left, XrController::default()));
        assert_eq!(intent.axis_x, 0.0);
        assert_eq!(intent.axis_z, 0.0);
    }

    #[test]
    fn left_stick_walks_with_game_forward_sign() {
        let left = active(XrController {
            // Full forward push on the stick.
            stick: vec2f(0.0, 1.0),
            ..Default::default()
        });
        let intent = intent_from_xr(&state_with(left, XrController::default()));
        // Forward in the game is -z.
        assert!(intent.axis_z < -0.9, "{intent:?}");
        assert_eq!(intent.axis_x, 0.0);

        let left = active(XrController {
            stick: vec2f(1.0, 0.0),
            ..Default::default()
        });
        let intent = intent_from_xr(&state_with(left, XrController::default()));
        assert!(intent.axis_x > 0.9, "{intent:?}");
    }

    #[test]
    fn either_trigger_shoots_and_grips_grab() {
        let left = active(XrController {
            trigger: 1.0,
            ..Default::default()
        });
        assert!(intent_from_xr(&state_with(left, XrController::default())).shoot);

        let right = active(XrController {
            trigger: 1.0,
            grip: 1.0,
            ..Default::default()
        });
        let intent = intent_from_xr(&state_with(XrController::default(), right));
        assert!(intent.shoot && intent.grab);
    }

    #[test]
    fn face_buttons_jump_and_reset() {
        let right = active(XrController {
            buttons: XrController::CLICK_A,
            ..Default::default()
        });
        assert!(intent_from_xr(&state_with(XrController::default(), right)).jump);

        let right = active(XrController {
            buttons: XrController::CLICK_B,
            ..Default::default()
        });
        assert!(intent_from_xr(&state_with(XrController::default(), right)).reset);
    }

    #[test]
    fn hands_drive_when_no_controller_is_tracked() {
        let mut state = XrState::default();
        // Nothing is ACTIVE, so the hand path owns input. Pinch lives in the
        // flags bitfield — the runtime's own detection — not the raw
        // strength array.
        state.right_hand.flags |= XrHand::PINCH_INDEX;
        let intent = intent_from_xr(&state);
        assert!(intent.shoot, "index pinch should shoot: {intent:?}");
        assert!(!intent.grab);

        state.right_hand.flags &= !XrHand::PINCH_INDEX;
        state.left_hand.flags |= XrHand::PINCH_MIDDLE;
        assert!(intent_from_xr(&state).grab);
    }

    #[test]
    fn a_tracked_controller_beats_a_pinching_hand() {
        // Hands stay tracked while holding controllers; a curled finger on
        // the grip must not read as a pinch and fire the gun.
        let mut state = state_with(
            active(XrController::default()),
            active(XrController::default()),
        );
        state.right_hand.flags |= XrHand::PINCH_INDEX;
        assert!(!intent_from_xr(&state).shoot);
    }

    #[test]
    fn head_yaw_matches_the_engine_camera_convention() {
        // Facing -z (the default look direction) is yaw 0.
        let state = XrState::default();
        assert!(intent_from_xr(&state).head_yaw.abs() < 1.0e-5);

        // Quarter turn to face +x.
        let mut state = XrState::default();
        state.head_pose.orientation =
            Quat::from_axis_angle(vec3f(0.0, 1.0, 0.0), std::f32::consts::FRAC_PI_2);
        let yaw = intent_from_xr(&state).head_yaw;
        assert!(
            (yaw.abs() - std::f32::consts::FRAC_PI_2).abs() < 1.0e-4,
            "{yaw}"
        );
    }

    #[test]
    fn pad_edges_fire_once_per_press() {
        let mut pad = PadState::default();
        let held = XrIntent {
            shoot: true,
            ..Default::default()
        };
        apply_intent_to_pad(&held, &mut pad);
        assert!(pad.shoot && pad.shoot_pressed);
        // Still held next tick: no new edge.
        apply_intent_to_pad(&held, &mut pad);
        assert!(pad.shoot && !pad.shoot_pressed);
        // Released, then pressed again.
        apply_intent_to_pad(&XrIntent::default(), &mut pad);
        assert!(!pad.shoot && !pad.shoot_pressed);
        apply_intent_to_pad(&held, &mut pad);
        assert!(pad.shoot_pressed);
    }
}
