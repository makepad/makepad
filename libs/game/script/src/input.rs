//! The `input` object handed to `on_tick`.
//!
//! `ScriptHost::tick` passes NIL in the second positional slot as a marker
//! meaning "build the snapshot here" — the object has to be constructed inside
//! the isolate, so it cannot be prepared before the call.
//!
//! Ported verbatim from gamemaker's `build_input_object`, expression order
//! included: `move_x`/`move_z` are the camera-relative axes every game walks
//! with, and changing how they are computed would change how every existing
//! game handles.

use makepad_game_sim::GameWorld;
use makepad_widgets::*;

/// Build one tick's input snapshot inside `vm`'s heap.
pub fn build_input_object(vm: &mut ScriptVm, world: &GameWorld) -> ScriptValue {
    let obj = vm.bx.heap.new_object();
    vm.bx.heap.set_object_storage_auto(obj);
    let trap = NoTrap;
    let key = |world: &GameWorld, name: LiveId| world.held.contains(&name);
    // Keyboard digital + gamepad analog, clamped — either device just works.
    let axis = ((key(world, live_id!(right)) as i8 - key(world, live_id!(left)) as i8) as f64
        + world.pad.axis_x)
        .clamp(-1.0, 1.0);
    let axis_z = ((key(world, live_id!(down)) as i8 - key(world, live_id!(up)) as i8) as f64
        + world.pad.axis_z)
        .clamp(-1.0, 1.0);
    let heap = &mut vm.bx.heap;
    for (name, held, pressed) in [
        (id!(left), live_id!(left), false),
        (id!(right), live_id!(right), false),
        (id!(up), live_id!(up), false),
        (id!(down), live_id!(down), false),
        (id!(jump), live_id!(jump), false),
        (id!(jump_pressed), live_id!(jump), true),
        (id!(shoot), live_id!(shoot), false),
        (id!(shoot_pressed), live_id!(shoot), true),
        (id!(grab), live_id!(grab), false),
        (id!(grab_pressed), live_id!(grab), true),
        (id!(reset), live_id!(reset), false),
        (id!(reset_pressed), live_id!(reset), true),
        (id!(back), live_id!(back), false),
        (id!(back_pressed), live_id!(back), true),
    ] {
        let v = if pressed {
            world.action_pressed(held)
        } else {
            world.action_held(held)
        };
        heap.set_value(obj, name.into(), ScriptValue::from_bool(v), trap);
    }
    // Mouse-drag deltas this tick (0 while not orbiting / under tapes):
    // chase cams use these to yield to the kid's hand.
    heap.set_value(obj, id!(look_dx).into(), ScriptValue::from_f64(world.look_dx), trap);
    heap.set_value(obj, id!(look_dy).into(), ScriptValue::from_f64(world.look_dy), trap);
    heap.set_value(obj, id!(axis_x).into(), ScriptValue::from_f64(axis), trap);
    heap.set_value(obj, id!(axis_z).into(), ScriptValue::from_f64(axis_z), trap);
    // Camera-relative movement: what the kid MEANS by "left" is screen-left.
    // Camera basis on the ground plane: forward = (sin y, -cos y), right =
    // (cos y, sin y) — so rotate the raw axes by +yaw. Scripts should walk
    // with these; the raw axes stay for side-scrollers and custom schemes.
    let yaw = world.cam_yaw;
    let move_x = axis * yaw.cos() as f64 - axis_z * yaw.sin() as f64;
    let move_z = axis * yaw.sin() as f64 + axis_z * yaw.cos() as f64;
    heap.set_value(obj, id!(move_x).into(), ScriptValue::from_f64(move_x), trap);
    heap.set_value(obj, id!(move_z).into(), ScriptValue::from_f64(move_z), trap);
    obj.into()
}
