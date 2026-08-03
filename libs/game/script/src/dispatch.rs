//! The `game.*` verb table.
//!
//! game.md calls for table-driven dispatch rather than gamemaker's 84-arm
//! `x if x == live_id!(..)` chain: verbs register into a `HashMap<LiveId,
//! VerbFn>` once at handle-registration time, so a call is one hash lookup
//! instead of up to 84 compares on the hot path.

use crate::build::{spawn_entity, spawn_terrain};
use crate::callbacks::CallbackTable;
use crate::value::*;
use makepad_game_blocks::{
    Blocks, Brain, BrainKind, Car, CarConfig, Character, CharacterConfig, ControlSource, Plane,
    PlaneConfig,
};
use makepad_game_sim::*;
use makepad_widgets::*;
use makepad_widgets::makepad_script::numeric::NumericValue;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

/// Audio is not a sim concern (M0 moved the synth host-side), so audio verbs
/// queue requests the host drains each tick.
#[derive(Clone, Debug, PartialEq)]
pub enum AudioRequest {
    Sfx { name: String, pitch: f32 },
    Beep { freq: f32, to: f32, ms: f32, gain: f32 },
    Jingle { notes: String, ms: f32 },
}

/// Everything a verb may touch. Cloned handles, so verbs can borrow narrowly
/// and drop before calling back into the heap (the re-entrancy rule that
/// gamemaker's `game.load`/`ground_peak` learned the hard way).
pub struct Ctx {
    pub world: Rc<RefCell<GameWorld>>,
    pub blocks: Rc<RefCell<Blocks>>,
    pub callbacks: Rc<RefCell<CallbackTable>>,
    pub audio: Rc<RefCell<Vec<AudioRequest>>>,
    pub eval_gen: Rc<Cell<u64>>,
}

pub type VerbFn = fn(&mut ScriptVm, &Ctx, ScriptObject) -> ScriptValue;

/// Typo guard: a misspelled option used to be silently ignored and cost whole
/// agent test cycles. Every options-taking verb checks its keys.
pub fn warn_unknown_keys(
    vm: &mut ScriptVm,
    world: &Rc<RefCell<GameWorld>>,
    verb: &str,
    opts: ScriptObject,
    allowed: &[LiveId],
) {
    let len = vm.bx.heap.iter_len(opts);
    for index in 0..len {
        let kv = vm.bx.heap.iter_key_value(opts, index, NoTrap);
        let Some(key) = kv.key.as_id() else {
            continue;
        };
        if !allowed.contains(&key) {
            world
                .borrow_mut()
                .log(format!("game.{verb}: unknown option `{key}` (ignored)"));
        }
    }
}

fn opts_of(vm: &mut ScriptVm, args: ScriptObject, index: usize) -> Option<ScriptObject> {
    arg(vm, args, index).as_object()
}

fn nil() -> ScriptValue {
    NIL
}

fn num(v: f64) -> ScriptValue {
    ScriptValue::from_f64(v)
}

// ── world ───────────────────────────────────────────────────────────────

fn v_box(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    spawn_entity(vm, &ctx.world, args, BodyKind::Static)
}

fn v_mover(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    spawn_entity(vm, &ctx.world, args, BodyKind::Mover)
}

fn v_terrain(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let Some(opts) = opts_of(vm, args, 0) else {
        return nil();
    };
    spawn_terrain(vm, &ctx.world, opts)
}

fn v_sky(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let mut sky = SkyConfig::default();
    if let Some(opts) = opts_of(vm, args, 0) {
        warn_unknown_keys(
            vm,
            &ctx.world,
            "sky",
            opts,
            &[id!(top), id!(horizon), id!(ground), id!(ground_bottom), id!(fog)],
        );
        sky.top = opt_color(vm, opts, id!(top), sky.top);
        sky.horizon = opt_color(vm, opts, id!(horizon), sky.horizon);
        sky.ground = opt_color(vm, opts, id!(ground), sky.ground);
        sky.ground_bottom = opt_color(vm, opts, id!(ground_bottom), sky.ground_bottom);
        sky.fog = opt_f32(vm, opts, id!(fog), sky.fog);
    }
    let mut world = ctx.world.borrow_mut();
    world.sky = Some(sky);
    world.mark_render_dirty();
    nil()
}

fn v_gravity(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let g = arg_f32(vm, args, 0);
    ctx.world.borrow_mut().gravity = g;
    nil()
}

fn v_remove(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let id = arg_id(vm, args, 0);
    let mut world = ctx.world.borrow_mut();
    world.entities.retain(|e| e.id != id);
    world.parts.retain(|p| p.owner != id);
    world.labels.retain(|l| l.owner != id);
    world.mark_render_dirty();
    nil()
}

// ── entity transform / query ────────────────────────────────────────────

fn v_pos(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let id = arg_id(vm, args, 0);
    let pos = ctx.world.borrow().entity(id).map(|e| e.pos);
    match pos {
        Some(p) => vec3_value(vm, p),
        None => nil(),
    }
}

fn v_vel(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let id = arg_id(vm, args, 0);
    let vel = ctx.world.borrow().entity(id).map(|e| e.vel);
    match vel {
        Some(v) => vec3_value(vm, v),
        None => nil(),
    }
}

fn v_set_pos(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let id = arg_id(vm, args, 0);
    let p_v = arg(vm, args, 1);
    let p = value_vec3(vm, p_v);
    let mut world = ctx.world.borrow_mut();
    if let Some(e) = world.entity_mut(id) {
        e.pos = p;
        e.vel = vec3f(0.0, 0.0, 0.0);
    }
    world.mark_render_dirty();
    nil()
}

fn v_set_vel(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let id = arg_id(vm, args, 0);
    let v_v = arg(vm, args, 1);
    let v = value_vec3(vm, v_v);
    if let Some(e) = ctx.world.borrow_mut().entity_mut(id) {
        e.vel = v;
    }
    nil()
}

fn v_push(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let id = arg_id(vm, args, 0);
    let v_v = arg(vm, args, 1);
    let v = value_vec3(vm, v_v);
    if let Some(e) = ctx.world.borrow_mut().entity_mut(id) {
        e.vel += v;
    }
    nil()
}

fn v_walk(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let id = arg_id(vm, args, 0);
    let vx = arg_f32(vm, args, 1);
    let vz = arg_f32(vm, args, 2);
    if let Some(e) = ctx.world.borrow_mut().entity_mut(id) {
        e.vel.x = vx;
        e.vel.z = vz;
    }
    nil()
}

fn v_jump(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let id = arg_id(vm, args, 0);
    let v = arg_f32(vm, args, 1);
    let mut world = ctx.world.borrow_mut();
    if let Some(e) = world.entity_mut(id) {
        if e.on_floor {
            e.vel.y = v;
        }
    }
    nil()
}

fn v_on_floor(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let id = arg_id(vm, args, 0);
    let on = ctx
        .world
        .borrow()
        .entity(id)
        .map(|e| e.on_floor)
        .unwrap_or(false);
    ScriptValue::from_bool(on)
}

fn v_face(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let id = arg_id(vm, args, 0);
    let yaw = arg_f32(vm, args, 1);
    let mut world = ctx.world.borrow_mut();
    if let Some(e) = world.entity_mut(id) {
        e.yaw = yaw;
        e.auto_face = false;
    }
    world.mark_render_dirty();
    nil()
}

fn v_yaw(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let id = arg_id(vm, args, 0);
    num(ctx.world.borrow().entity(id).map(|e| e.yaw).unwrap_or(0.0) as f64)
}

fn v_tag(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let id = arg_id(vm, args, 0);
    let tag = ctx
        .world
        .borrow()
        .entity(id)
        .map(|e| e.tag.clone())
        .unwrap_or_default();
    vm.bx.heap.new_string_from_str(&tag)
}

fn v_find(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let want = arg_string(vm, args, 0);
    let id = ctx
        .world
        .borrow()
        .entities
        .iter()
        .find(|e| e.tag == want)
        .map(|e| e.id)
        .unwrap_or(0);
    num(id as f64)
}

fn v_distance(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let a_v = arg(vm, args, 0);
    let b_v = arg(vm, args, 1);
    let resolve = |vm: &mut ScriptVm, v: ScriptValue| -> Option<Vec3f> {
        let ip = vm.bx.threads.cur_ref().trap.ip;
        match NumericValue::from_script_value_heap(&vm.bx.heap, v, ip) {
            NumericValue::Vec3(p) => Some(p),
            NumericValue::F64(f) => ctx.world.borrow().entity(f as u64).map(|e| e.pos),
            _ => None,
        }
    };
    let (Some(a), Some(b)) = (resolve(vm, a_v), resolve(vm, b_v)) else {
        return num(0.0);
    };
    let d = a - b;
    num(makepad_game_sim::math::sqrt(d.x * d.x + d.y * d.y + d.z * d.z) as f64)
}

fn v_set_color(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let id = arg_id(vm, args, 0);
    let c_v = arg(vm, args, 1);
    let c = value_color(vm, c_v);
    let mut world = ctx.world.borrow_mut();
    if let Some(e) = world.entity_mut(id) {
        e.color = c;
    }
    world.mark_render_dirty();
    nil()
}

fn v_glow(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let id = arg_id(vm, args, 0);
    let g = arg_f32(vm, args, 1);
    let mut world = ctx.world.borrow_mut();
    if let Some(e) = world.entity_mut(id) {
        e.glow = g;
    }
    world.mark_render_dirty();
    nil()
}

fn v_scale(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let id = arg_id(vm, args, 0);
    let s = arg_f32(vm, args, 1);
    let mut world = ctx.world.borrow_mut();
    if let Some(e) = world.entity_mut(id) {
        e.scale_target = vec3f(s, s, s);
    }
    world.mark_render_dirty();
    nil()
}

// ── camera / HUD ────────────────────────────────────────────────────────

fn v_camera(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let Some(opts) = opts_of(vm, args, 0) else {
        return nil();
    };
    warn_unknown_keys(
        vm,
        &ctx.world,
        "camera",
        opts,
        &[
            id!(third_person),
            id!(follow),
            id!(chase),
            id!(side),
            id!(height),
            id!(boom),
            id!(distance),
            id!(pitch),
            id!(fov),
            id!(lag),
            id!(recenter),
            id!(speed_tighten),
            id!(target),
        ],
    );
    let third = opts_value(vm, opts, id!(third_person));
    let follow = opts_value(vm, opts, id!(follow));
    let chase = opts_value(vm, opts, id!(chase));
    let side = opt_bool(vm, opts, id!(side), false);
    let height = opts_value(vm, opts, id!(height));
    let boom = opts_value(vm, opts, id!(boom));
    let distance = opts_value(vm, opts, id!(distance));
    let pitch = opts_value(vm, opts, id!(pitch));
    let fov = opts_value(vm, opts, id!(fov));
    let lag = opts_value(vm, opts, id!(lag));
    let recenter = opts_value(vm, opts, id!(recenter));
    let tighten = opts_value(vm, opts, id!(speed_tighten));
    let target = opts_value(vm, opts, id!(target));

    let third_id = if third.is_nil() { None } else { Some(value_f32(vm, third) as u64) };
    let follow_id = if follow.is_nil() { None } else { Some(value_f32(vm, follow) as u64) };
    let chase_id = if chase.is_nil() { None } else { Some(value_f32(vm, chase) as u64) };
    let height_v = if height.is_nil() { None } else { Some(value_f32(vm, height)) };
    let boom_v = if boom.is_nil() { None } else { Some(value_f32(vm, boom)) };
    let dist_v = if distance.is_nil() { None } else { Some(value_f32(vm, distance)) };
    let pitch_v = if pitch.is_nil() { None } else { Some(value_f32(vm, pitch)) };
    let fov_v = if fov.is_nil() { None } else { Some(value_f32(vm, fov)) };
    let lag_v = if lag.is_nil() { None } else { Some(value_f32(vm, lag)) };
    let recenter_v = if recenter.is_nil() { None } else { Some(value_f32(vm, recenter)) };
    let tighten_v = if tighten.is_nil() { None } else { Some(value_f32(vm, tighten)) };
    let target_v = if target.is_nil() { None } else { Some(value_vec3(vm, target)) };

    let mut world = ctx.world.borrow_mut();
    if let Some(id) = third_id {
        world.cam_third = id;
    }
    if let Some(id) = follow_id {
        world.cam_follow = id;
    }
    if let Some(id) = chase_id {
        world.cam_chase = id;
    }
    world.cam_side = side;
    if let Some(v) = height_v {
        world.cam_height = v;
    }
    if let Some(v) = boom_v {
        world.cam_boom = v;
    }
    if let Some(v) = dist_v {
        world.cam_distance = v;
    }
    if let Some(v) = pitch_v {
        world.cam_pitch_request = Some(v);
    }
    if let Some(v) = fov_v {
        world.cam_fov = v;
    }
    if let Some(v) = lag_v {
        world.cam_lag = v;
    }
    if let Some(v) = recenter_v {
        world.cam_recenter = v;
    }
    if let Some(v) = tighten_v {
        world.cam_speed_tighten = v;
    }
    if let Some(v) = target_v {
        world.cam_target = v;
    }
    nil()
}

fn v_text(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    // game.text(msg) or game.text(slot, msg, {anchor, color, size})
    let first = arg(vm, args, 0);
    let second = arg(vm, args, 1);
    let (slot, msg) = if second.is_nil() {
        ("_".to_string(), value_string(vm, first))
    } else {
        (value_string(vm, first), value_string(vm, second))
    };
    let mut anchor = HudAnchor::TopLeft;
    let mut color = vec4(1.0, 1.0, 1.0, 1.0);
    let mut size = 1.0f32;
    if let Some(opts) = opts_of(vm, args, 2) {
        warn_unknown_keys(vm, &ctx.world, "text", opts, &[id!(anchor), id!(color), id!(size)]);
        if let Some(a) = opt_string(vm, opts, id!(anchor)) {
            anchor = anchor_from_str(&a);
        }
        color = opt_color(vm, opts, id!(color), color);
        size = opt_f32(vm, opts, id!(size), size);
    }
    let mut world = ctx.world.borrow_mut();
    if let Some(existing) = world.hud_slot_mut(&slot) {
        existing.text = msg;
        existing.anchor = anchor;
        existing.color = color;
        existing.size = size;
    } else {
        world.hud_slots.push((
            slot,
            HudSlot {
                text: msg,
                anchor,
                color,
                size,
            },
        ));
    }
    nil()
}

fn anchor_from_str(s: &str) -> HudAnchor {
    match s {
        "top" | "top_center" => HudAnchor::Top,
        "top_right" => HudAnchor::TopRight,
        "bottom_left" => HudAnchor::BottomLeft,
        "center" => HudAnchor::Center,
        "bottom_right" => HudAnchor::BottomRight,
        "bottom" | "bottom_center" => HudAnchor::Bottom,
        _ => HudAnchor::TopLeft,
    }
}

fn v_bar(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let name = arg_string(vm, args, 0);
    let frac = arg_f32(vm, args, 1);
    let mut color = vec4(0.4, 0.8, 0.4, 1.0);
    let mut anchor = HudAnchor::TopLeft;
    if let Some(opts) = opts_of(vm, args, 2) {
        warn_unknown_keys(vm, &ctx.world, "bar", opts, &[id!(color), id!(anchor)]);
        color = opt_color(vm, opts, id!(color), color);
        if let Some(a) = opt_string(vm, opts, id!(anchor)) {
            anchor = anchor_from_str(&a);
        }
    }
    let mut world = ctx.world.borrow_mut();
    // Negative fraction removes the bar (gamemaker semantics).
    if frac < 0.0 {
        world.hud_bars.retain(|b| b.name != name);
        return nil();
    }
    if let Some(bar) = world.hud_bars.iter_mut().find(|b| b.name == name) {
        bar.fraction = frac;
        bar.color = color;
        bar.anchor = anchor;
    } else {
        world.hud_bars.push(HudBar {
            name,
            fraction: frac,
            color,
            anchor,
        });
    }
    nil()
}

fn v_crosshair(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let on_v = arg(vm, args, 0);
    let on = value_bool(vm, on_v);
    ctx.world.borrow_mut().crosshair = on;
    nil()
}

fn v_label(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let owner = arg_id(vm, args, 0);
    let text = arg_string(vm, args, 1);
    let mut height = 1.2f32;
    let mut color = vec4(1.0, 1.0, 1.0, 1.0);
    let mut size = 1.0f32;
    if let Some(opts) = opts_of(vm, args, 2) {
        warn_unknown_keys(vm, &ctx.world, "label", opts, &[id!(height), id!(color), id!(size)]);
        height = opt_f32(vm, opts, id!(height), height);
        color = opt_color(vm, opts, id!(color), color);
        size = opt_f32(vm, opts, id!(size), size);
    }
    let mut world = ctx.world.borrow_mut();
    let id = world.next_id;
    world.next_id += 1;
    world.labels.push(LabelDef {
        lid: id,
        owner,
        text,
        height,
        color,
        size,
        default: false,
    });
    world.mark_render_dirty();
    num(id as f64)
}

fn v_label_text(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let id = arg_id(vm, args, 0);
    let text = arg_string(vm, args, 1);
    let mut world = ctx.world.borrow_mut();
    if let Some(l) = world.labels.iter_mut().find(|l| l.lid == id) {
        l.text = text;
    }
    world.mark_render_dirty();
    nil()
}

// ── callbacks / timers ──────────────────────────────────────────────────

fn register_callback(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> Option<CallbackSlot> {
    let func_v = arg(vm, args, 0);
    let func = fn_ref(vm, func_v)?;
    Some(ctx.callbacks.borrow_mut().alloc(ctx.eval_gen.get(), func))
}

fn v_on_tick(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    if let Some(slot) = register_callback(vm, ctx, args) {
        let old = ctx.world.borrow_mut().on_tick.replace(slot);
        if let Some(old) = old {
            ctx.callbacks.borrow_mut().free(old);
        }
    }
    nil()
}

fn v_on_touch(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    if let Some(slot) = register_callback(vm, ctx, args) {
        let old = ctx.world.borrow_mut().on_touch.replace(slot);
        if let Some(old) = old {
            ctx.callbacks.borrow_mut().free(old);
        }
    }
    nil()
}

fn v_on_join(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    if let Some(slot) = register_callback(vm, ctx, args) {
        let old = ctx.world.borrow_mut().on_join.replace(slot);
        if let Some(old) = old {
            ctx.callbacks.borrow_mut().free(old);
        }
    }
    nil()
}

fn v_on_leave(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    if let Some(slot) = register_callback(vm, ctx, args) {
        let old = ctx.world.borrow_mut().on_leave.replace(slot);
        if let Some(old) = old {
            ctx.callbacks.borrow_mut().free(old);
        }
    }
    nil()
}

fn add_timer(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject, repeat: bool) -> ScriptValue {
    let secs = arg_f32(vm, args, 0).max(0.0);
    let func_v = arg(vm, args, 1);
    let Some(func) = fn_ref(vm, func_v) else {
        return num(0.0);
    };
    let slot = ctx.callbacks.borrow_mut().alloc(ctx.eval_gen.get(), func);
    let mut world = ctx.world.borrow_mut();
    let id = world.next_id;
    world.next_id += 1;
    // Timers are tick-based so replays land on the same tick every run.
    let ticks = ((secs / TICK_DT).round() as u64).max(1);
    let at_tick = world.tick + ticks;
    world.timers.push(GameTimer {
        id,
        at_tick,
        interval_ticks: if repeat { ticks } else { 0 },
        func: slot,
    });
    num(id as f64)
}

fn v_after(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    add_timer(vm, ctx, args, false)
}

fn v_every(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    add_timer(vm, ctx, args, true)
}

fn v_cancel(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let id = arg_id(vm, args, 0);
    let mut world = ctx.world.borrow_mut();
    if let Some(pos) = world.timers.iter().position(|t| t.id == id) {
        let timer = world.timers.remove(pos);
        ctx.callbacks.borrow_mut().free(timer.func);
    }
    nil()
}

// ── misc ────────────────────────────────────────────────────────────────

fn v_time(_vm: &mut ScriptVm, ctx: &Ctx, _args: ScriptObject) -> ScriptValue {
    num(ctx.world.borrow().time)
}

fn v_rand(_vm: &mut ScriptVm, ctx: &Ctx, _args: ScriptObject) -> ScriptValue {
    num(ctx.world.borrow_mut().rand())
}

fn v_rand_range(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let lo = arg_f64(vm, args, 0);
    let hi = arg_f64(vm, args, 1);
    let r = ctx.world.borrow_mut().rand();
    num(lo + (hi - lo) * r)
}

fn v_log(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let msg = arg_string(vm, args, 0);
    ctx.world.borrow_mut().log(msg);
    nil()
}

fn v_ground_y(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let x = arg_f32(vm, args, 0);
    let z = arg_f32(vm, args, 1);
    let y = ctx
        .world
        .borrow()
        .terrain
        .as_ref()
        .and_then(|t| t.height_at(x, z))
        .unwrap_or(0.0);
    num(y as f64)
}

fn v_ground_normal(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let x = arg_f32(vm, args, 0);
    let z = arg_f32(vm, args, 1);
    let n = ctx
        .world
        .borrow()
        .terrain
        .as_ref()
        .and_then(|t| t.normal_at(x, z))
        .unwrap_or(vec3f(0.0, 1.0, 0.0));
    vec3_value(vm, n)
}

// ── audio (queued for the host) ─────────────────────────────────────────

fn v_sfx(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let name = arg_string(vm, args, 0);
    let pitch_v = arg(vm, args, 1);
    let pitch = if pitch_v.is_nil() { 1.0 } else { value_f32(vm, pitch_v) };
    ctx.audio.borrow_mut().push(AudioRequest::Sfx { name, pitch });
    nil()
}

fn v_beep(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let (mut freq, mut to, mut ms, mut gain) = (440.0f32, 0.0f32, 120.0f32, 0.3f32);
    if let Some(opts) = opts_of(vm, args, 0) {
        warn_unknown_keys(vm, &ctx.world, "beep", opts, &[id!(freq), id!(to), id!(ms), id!(gain), id!(wave)]);
        freq = opt_f32(vm, opts, id!(freq), freq);
        to = opt_f32(vm, opts, id!(to), to);
        ms = opt_f32(vm, opts, id!(ms), ms);
        gain = opt_f32(vm, opts, id!(gain), gain);
    }
    ctx.audio
        .borrow_mut()
        .push(AudioRequest::Beep { freq, to, ms, gain });
    nil()
}

fn v_jingle(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let notes = arg_string(vm, args, 0);
    let ms_v = arg(vm, args, 1);
    let ms = if ms_v.is_nil() { 140.0 } else { value_f32(vm, ms_v) };
    ctx.audio
        .borrow_mut()
        .push(AudioRequest::Jingle { notes, ms });
    nil()
}

// ── blocks: car / character / plane ─────────────────────────────────────

fn block_common(
    vm: &mut ScriptVm,
    ctx: &Ctx,
    args: ScriptObject,
    allowed: &[LiveId],
    verb: &str,
) -> Option<(u64, ScriptObject, PlayerId)> {
    let opts = opts_of(vm, args, 0)?;
    warn_unknown_keys(vm, &ctx.world, verb, opts, allowed);
    // Spawn the body through the same path as game.mover so every option
    // (pos/size/color/tag/...) behaves identically.
    let kind = if verb == "car" || verb == "plane" {
        BodyKind::Rigid
    } else {
        BodyKind::Mover
    };
    let id_v = spawn_entity_from_opts(vm, ctx, opts, kind);
    let id = value_f32(vm, id_v) as u64;
    let owner = PlayerId(opt_f32(vm, opts, id!(player), 0.0) as u32);
    Some((id, opts, owner))
}

/// `spawn_entity` takes the args vec; block verbs already unwrapped the opts
/// object, so re-wrap it in a one-element vec for the shared path.
fn spawn_entity_from_opts(
    vm: &mut ScriptVm,
    ctx: &Ctx,
    opts: ScriptObject,
    kind: BodyKind,
) -> ScriptValue {
    let wrapper = vm.bx.heap.new_object();
    vm.bx.heap.set_object_storage_vec2(wrapper);
    vm.bx.heap.vec_push_unchecked(wrapper, NIL, opts.into());
    let out = spawn_entity(vm, &ctx.world, wrapper, kind);
    vm.release_transient(wrapper.into());
    out
}

fn v_car(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let allowed = &[
        id!(pos), id!(size), id!(color), id!(tag), id!(player), id!(top_speed),
        id!(accel), id!(braking), id!(grip), id!(steer_rate), id!(seats),
        id!(density), id!(friction), id!(restitution), id!(rot_y),
    ];
    let Some((id, opts, owner)) = block_common(vm, ctx, args, allowed, "car") else {
        return nil();
    };
    let mut config = CarConfig::default();
    config.top_speed = opt_f32(vm, opts, id!(top_speed), config.top_speed);
    config.accel = opt_f32(vm, opts, id!(accel), config.accel);
    config.braking = opt_f32(vm, opts, id!(braking), config.braking);
    config.grip = opt_f32(vm, opts, id!(grip), config.grip);
    config.steer_rate = opt_f32(vm, opts, id!(steer_rate), config.steer_rate);
    config.seats = opt_f32(vm, opts, id!(seats), config.seats as f32) as u32;
    // Player 0 is this device; anyone else is driven by their own packets
    // (Blocks::player_inputs) through the same Player source.
    let control = ControlSource::Player;
    let mut car = Car::new(id, config, control);
    car.owner = owner;
    ctx.blocks.borrow_mut().cars.push(car);
    num(id as f64)
}

fn v_character(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let allowed = &[
        id!(pos), id!(size), id!(color), id!(tag), id!(player), id!(model),
        id!(speed), id!(jump), id!(view), id!(gravity), id!(rot_y),
    ];
    let Some((id, opts, owner)) = block_common(vm, ctx, args, allowed, "character") else {
        return nil();
    };
    let model = opt_string(vm, opts, id!(model));
    let mut config = CharacterConfig::default();
    config.speed = opt_f32(vm, opts, id!(speed), config.speed);
    config.jump = opt_f32(vm, opts, id!(jump), config.jump);
    let control = ControlSource::Player;
    let mut ch = Character::new(id, config, control, model);
    ch.owner = owner;
    ctx.blocks.borrow_mut().characters.push(ch);
    num(id as f64)
}

fn v_plane(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let allowed = &[
        id!(pos), id!(size), id!(color), id!(tag), id!(player), id!(thrust),
        id!(top_speed), id!(lift_speed), id!(auto_level), id!(density), id!(rot_y),
    ];
    let Some((id, opts, owner)) = block_common(vm, ctx, args, allowed, "plane") else {
        return nil();
    };
    let mut config = PlaneConfig::default();
    config.thrust = opt_f32(vm, opts, id!(thrust), config.thrust);
    config.top_speed = opt_f32(vm, opts, id!(top_speed), config.top_speed);
    config.lift_speed = opt_f32(vm, opts, id!(lift_speed), config.lift_speed);
    config.auto_level = opt_f32(vm, opts, id!(auto_level), config.auto_level);
    let mut plane = Plane::new(id, config, ControlSource::Player);
    plane.owner = owner;
    ctx.blocks.borrow_mut().planes.push(plane);
    num(id as f64)
}

fn v_drive(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let id = arg_id(vm, args, 0);
    let Some(opts) = opts_of(vm, args, 1) else {
        return nil();
    };
    warn_unknown_keys(
        vm,
        &ctx.world,
        "drive",
        opts,
        &[id!(steer), id!(throttle), id!(brake), id!(handbrake), id!(pitch), id!(roll), id!(move_x), id!(move_z), id!(jump)],
    );
    let steer = opt_f32(vm, opts, id!(steer), 0.0);
    let throttle = opt_f32(vm, opts, id!(throttle), 0.0);
    let brake = opt_f32(vm, opts, id!(brake), 0.0);
    let handbrake = opt_f32(vm, opts, id!(handbrake), 0.0);
    let pitch = opt_f32(vm, opts, id!(pitch), 0.0);
    let roll = opt_f32(vm, opts, id!(roll), 0.0);
    let move_x = opt_f32(vm, opts, id!(move_x), 0.0);
    let move_z = opt_f32(vm, opts, id!(move_z), 0.0);
    let jump = opt_bool(vm, opts, id!(jump), false);
    ctx.blocks.borrow_mut().drive(id, |d| {
        d.steer = steer;
        d.throttle = throttle;
        d.brake = brake;
        d.handbrake = handbrake;
        d.pitch = pitch;
        d.roll = roll;
        d.move_x = move_x;
        d.move_z = move_z;
        d.jump = jump;
    });
    nil()
}

fn v_autodrive(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let id = arg_id(vm, args, 0);
    let Some(opts) = opts_of(vm, args, 1) else {
        return nil();
    };
    warn_unknown_keys(vm, &ctx.world, "autodrive", opts, &[id!(points), id!(pace)]);
    let points = opt_points(vm, opts, id!(points));
    let pace = opt_f32(vm, opts, id!(pace), 1.0);
    let mut blocks = ctx.blocks.borrow_mut();
    if let Some(car) = blocks.car_mut(id) {
        car.route = points;
        car.route_at = 0;
        car.route_pace = pace;
        // A car with a route drives itself; Script control lets the route
        // writer (or game.drive) own the intent.
        car.control = ControlSource::Script;
    }
    nil()
}

fn v_speed(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let id = arg_id(vm, args, 0);
    let mut blocks = ctx.blocks.borrow_mut();
    if let Some(car) = blocks.car_mut(id) {
        return num(car.speed as f64);
    }
    if let Some(plane) = blocks.plane_mut(id) {
        return num(plane.airspeed as f64);
    }
    drop(blocks);
    let v = ctx.world.borrow().entity(id).map(|e| e.vel).unwrap_or(vec3f(0.0, 0.0, 0.0));
    num(makepad_game_sim::math::sqrt(v.x * v.x + v.z * v.z) as f64)
}

// ── brains ──────────────────────────────────────────────────────────────

fn v_wander(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let id = arg_id(vm, args, 0);
    let Some(opts) = opts_of(vm, args, 1) else {
        return nil();
    };
    warn_unknown_keys(vm, &ctx.world, "wander", opts, &[id!(home), id!(range), id!(speed), id!(pause)]);
    let default_home = ctx.world.borrow().entity(id).map(|e| e.pos).unwrap_or(vec3f(0.0, 0.0, 0.0));
    let home = opt_vec3(vm, opts, id!(home), default_home);
    let range = opt_f32(vm, opts, id!(range), 8.0);
    let speed = opt_f32(vm, opts, id!(speed), 2.0);
    let pause = opt_f32(vm, opts, id!(pause), 1.0);
    push_brain(ctx, id, BrainKind::Wander { home, range, speed, pause });
    nil()
}

fn v_chase(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let id = arg_id(vm, args, 0);
    let Some(opts) = opts_of(vm, args, 1) else {
        return nil();
    };
    warn_unknown_keys(vm, &ctx.world, "chase", opts, &[id!(tag), id!(target), id!(range), id!(catch), id!(speed)]);
    let tag = opt_string(vm, opts, id!(tag)).unwrap_or_default();
    let target = opt_f32(vm, opts, id!(target), 0.0) as u64;
    let range = opt_f32(vm, opts, id!(range), 20.0);
    let catch = opt_f32(vm, opts, id!(catch), 1.2);
    let speed = opt_f32(vm, opts, id!(speed), 3.0);
    push_brain(ctx, id, BrainKind::Chase { tag, target, range, catch, speed });
    nil()
}

fn v_patrol(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let id = arg_id(vm, args, 0);
    let Some(opts) = opts_of(vm, args, 1) else {
        return nil();
    };
    warn_unknown_keys(vm, &ctx.world, "patrol", opts, &[id!(points), id!(speed), id!(loop)]);
    let points = opt_points(vm, opts, id!(points));
    let speed = opt_f32(vm, opts, id!(speed), 2.5);
    let looping = opt_bool(vm, opts, id!(loop), true);
    push_brain(ctx, id, BrainKind::Patrol { points, speed, looping });
    nil()
}

fn push_brain(ctx: &Ctx, entity: u64, kind: BrainKind) {
    let mut blocks = ctx.blocks.borrow_mut();
    blocks.brains.retain(|b| b.entity != entity);
    blocks.brains.push(Brain::new(entity, kind));
}

fn v_caught(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let id = arg_id(vm, args, 0);
    let caught = ctx
        .blocks
        .borrow()
        .brains
        .iter()
        .find(|b| b.entity == id)
        .map(|b| b.caught)
        .unwrap_or(0);
    num(caught as f64)
}

// ── race kit ────────────────────────────────────────────────────────────

fn v_spawnpoint(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let Some(opts) = opts_of(vm, args, 0) else {
        return num(0.0);
    };
    warn_unknown_keys(vm, &ctx.world, "spawnpoint", opts, &[id!(pos), id!(yaw)]);
    let pos = opt_vec3(vm, opts, id!(pos), vec3f(0.0, 0.0, 0.0));
    let yaw = opt_f32(vm, opts, id!(yaw), 0.0);
    let slot = ctx.blocks.borrow_mut().race.add_spawn(pos, yaw);
    num(slot as f64)
}

fn v_checkpoint(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let Some(opts) = opts_of(vm, args, 0) else {
        return num(0.0);
    };
    warn_unknown_keys(vm, &ctx.world, "checkpoint", opts, &[id!(pos), id!(size)]);
    let pos = opt_vec3(vm, opts, id!(pos), vec3f(0.0, 0.0, 0.0));
    let half = opt_vec3(vm, opts, id!(size), vec3f(4.0, 3.0, 4.0));
    let index = ctx.blocks.borrow_mut().race.add_checkpoint(pos, half, 0);
    num(index as f64)
}

fn v_place(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let id = arg_id(vm, args, 0);
    let slot = arg_f32(vm, args, 1) as usize;
    let mut world = ctx.world.borrow_mut();
    ctx.blocks.borrow_mut().race.place(&mut world, slot, id);
    nil()
}

fn v_race(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let mut laps = 3u32;
    if let Some(opts) = opts_of(vm, args, 0) {
        warn_unknown_keys(vm, &ctx.world, "race", opts, &[id!(laps)]);
        laps = opt_f32(vm, opts, id!(laps), laps as f32) as u32;
    }
    ctx.blocks.borrow_mut().race.start(laps);
    nil()
}

fn v_standings(vm: &mut ScriptVm, ctx: &Ctx, _args: ScriptObject) -> ScriptValue {
    let order = ctx.blocks.borrow().race.order();
    let out = vm.bx.heap.new_object();
    vm.bx.heap.set_object_storage_vec2(out);
    for s in order {
        let row = vm.bx.heap.new_object();
        vm.bx.heap.set_object_storage_auto(row);
        let trap = NoTrap;
        vm.bx.heap.set_value(row, id!(entity).into(), num(s.entity as f64), trap);
        vm.bx.heap.set_value(row, id!(lap).into(), num(s.lap as f64), trap);
        vm.bx.heap.set_value(row, id!(checkpoint).into(), num(s.checkpoint as f64), trap);
        vm.bx.heap.set_value(row, id!(finished).into(), ScriptValue::from_bool(s.finished), trap);
        vm.bx.heap.set_value(row, id!(score).into(), num(s.score as f64), trap);
        vm.bx.heap.vec_push_unchecked(out, NIL, row.into());
    }
    out.into()
}

fn v_lap(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let id = arg_id(vm, args, 0);
    let lap = ctx.blocks.borrow().race.standing_of(id).map(|s| s.lap).unwrap_or(0);
    num(lap as f64)
}

fn v_rank(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let id = arg_id(vm, args, 0);
    num(ctx.blocks.borrow().race.rank_of(id) as f64)
}

fn v_finished(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let id = arg_id(vm, args, 0);
    let f = ctx.blocks.borrow().race.standing_of(id).map(|s| s.finished).unwrap_or(false);
    ScriptValue::from_bool(f)
}

fn v_score(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let id = arg_id(vm, args, 0);
    let points = arg_f32(vm, args, 1) as i32;
    ctx.blocks.borrow_mut().race.add_score(id, points);
    nil()
}

fn v_score_of(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let id = arg_id(vm, args, 0);
    let s = ctx.blocks.borrow().race.standing_of(id).map(|s| s.score).unwrap_or(0);
    num(s as f64)
}

// ── players ─────────────────────────────────────────────────────────────

fn v_players(vm: &mut ScriptVm, ctx: &Ctx, _args: ScriptObject) -> ScriptValue {
    let ids: Vec<PlayerId> = ctx.world.borrow().players.iter().map(|p| p.id).collect();
    let out = vm.bx.heap.new_object();
    vm.bx.heap.set_object_storage_vec2(out);
    for id in ids {
        vm.bx.heap.vec_push_unchecked(out, NIL, num(id.0 as f64));
    }
    out.into()
}

fn v_player_name(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let id = PlayerId(arg_f32(vm, args, 0) as u32);
    let name = ctx
        .world
        .borrow()
        .players
        .get(id)
        .map(|p| p.name.clone())
        .unwrap_or_default();
    vm.bx.heap.new_string_from_str(&name)
}

fn v_player_entity(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let id = PlayerId(arg_f32(vm, args, 0) as u32);
    let e = ctx
        .world
        .borrow()
        .players
        .get(id)
        .map(|p| p.entity)
        .unwrap_or(0);
    num(e as f64)
}

fn v_bot(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let name = arg_string(vm, args, 0);
    let id = ctx.world.borrow_mut().players.add(name, PlayerSource::Bot);
    num(id.0 as f64)
}

// ── the table ───────────────────────────────────────────────────────────

/// `(name, fn, signature)` — the signature strings are what `game.api()`
/// prints and what splashgame.md documents.
pub const VERBS: &[(&str, VerbFn, &str)] = &[
    ("box", v_box, "({pos, size, color, tag, sensor, collide, body, glow, shape, rot_y, density, friction, restitution}) -> id"),
    ("block", v_box, "alias of box"),
    ("mover", v_mover, "({...box opts, gravity, vel, turn_rate, face}) -> id"),
    ("spawn", v_mover, "({...mover opts, vel, life, hits}) -> id — projectile"),
    ("terrain", v_terrain, "({size, cells, base, amp, seed, freq, offset, step, min, max, plaza, bands, heights, colors, color, tag, smooth, water}) "),
    ("sky", v_sky, "({top, bottom, fog, fog_density})"),
    ("gravity", v_gravity, "(g)"),
    ("remove", v_remove, "(id)"),
    ("pos", v_pos, "(id) -> vec3"),
    ("vel", v_vel, "(id) -> vec3"),
    ("set_pos", v_set_pos, "(id, vec3)"),
    ("teleport", v_set_pos, "alias of set_pos"),
    ("set_vel", v_set_vel, "(id, vec3)"),
    ("push", v_push, "(id, vec3) — add velocity (impulse on rigids)"),
    ("walk", v_walk, "(id, vx, vz)"),
    ("jump", v_jump, "(id, v)"),
    ("on_floor", v_on_floor, "(id) -> bool"),
    ("face", v_face, "(id, yaw)"),
    ("yaw", v_yaw, "(id) -> yaw"),
    ("tag", v_tag, "(id) -> tag"),
    ("find", v_find, "(tag) -> id (0 = none)"),
    ("distance", v_distance, "(a, b) -> distance; ids or vec3"),
    ("set_color", v_set_color, "(id, color)"),
    ("glow", v_glow, "(id, amount)"),
    ("scale", v_scale, "(id, factor)"),
    ("camera", v_camera, "({third_person, follow, chase, side, height, boom, distance, pitch, fov, lag, recenter, speed_tighten, target})"),
    ("text", v_text, "(msg) | (slot, msg, {anchor, color, size})"),
    ("bar", v_bar, "(name, frac, {color, anchor}) — negative frac removes"),
    ("crosshair", v_crosshair, "(bool)"),
    ("label", v_label, "(id, text, {height, color, size}) -> label id"),
    ("label_text", v_label_text, "(label_id, text)"),
    ("on_tick", v_on_tick, "(fn(dt, input))"),
    ("on_touch", v_on_touch, "(fn(a, b))"),
    ("on_join", v_on_join, "(fn(player))"),
    ("on_leave", v_on_leave, "(fn(player))"),
    ("after", v_after, "(secs, fn) -> timer id"),
    ("every", v_every, "(secs, fn) -> timer id"),
    ("cancel", v_cancel, "(timer id)"),
    ("time", v_time, "() -> seconds since eval"),
    ("rand", v_rand, "() -> 0..1"),
    ("rand_range", v_rand_range, "(lo, hi) -> value"),
    ("log", v_log, "(msg)"),
    ("ground_y", v_ground_y, "(x, z) -> terrain height"),
    ("ground_normal", v_ground_normal, "(x, z) -> vec3"),
    ("sfx", v_sfx, "(name, pitch)"),
    ("beep", v_beep, "({freq, to, ms, gain})"),
    ("jingle", v_jingle, "(notes, ms)"),
    ("car", v_car, "({pos, size, color, tag, player, top_speed, accel, braking, grip, steer_rate, seats}) -> id"),
    ("character", v_character, "({pos, size, color, tag, player, model, speed, jump, view}) -> id"),
    ("plane", v_plane, "({pos, size, color, tag, player, thrust, top_speed, lift_speed, auto_level}) -> id"),
    ("drive", v_drive, "(id, {steer, throttle, brake, handbrake, pitch, roll, move_x, move_z, jump})"),
    ("autodrive", v_autodrive, "(id, {points, pace})"),
    ("speed", v_speed, "(id) -> speed"),
    ("wander", v_wander, "(id, {home, range, speed, pause})"),
    ("chase", v_chase, "(id, {tag, target, range, catch, speed})"),
    ("patrol", v_patrol, "(id, {points, speed, loop})"),
    ("caught", v_caught, "(id) -> entity caught this tick (0 = none)"),
    ("spawnpoint", v_spawnpoint, "({pos, yaw}) -> slot"),
    ("checkpoint", v_checkpoint, "({pos, size}) -> index"),
    ("place", v_place, "(id, slot)"),
    ("race", v_race, "({laps})"),
    ("standings", v_standings, "() -> [{entity, lap, checkpoint, finished, score}]"),
    ("lap", v_lap, "(id) -> laps"),
    ("rank", v_rank, "(id) -> 1-based position"),
    ("finished", v_finished, "(id) -> bool"),
    ("score", v_score, "(id, points)"),
    ("score_of", v_score_of, "(id) -> score"),
    ("players", v_players, "() -> [player ids]"),
    ("player_name", v_player_name, "(player) -> name"),
    ("player_entity", v_player_entity, "(player) -> entity id"),
    ("bot", v_bot, "(name) -> player id"),
];

/// Built once per isolate: LiveId -> verb. Replaces gamemaker's linear chain.
pub fn verb_table() -> HashMap<LiveId, VerbFn> {
    VERBS
        .iter()
        .map(|(name, f, _)| (LiveId::from_str(name), *f))
        .collect()
}

/// Nearest known verb for an unknown call (edit distance 1-2), so a typo
/// reports "did you mean" instead of just failing.
pub fn suggest(name: &str) -> Option<&'static str> {
    let mut best: Option<(usize, &'static str)> = None;
    for (verb, _, _) in VERBS {
        let d = edit_distance(name, verb);
        if d <= 2 && best.map(|(bd, _)| d < bd).unwrap_or(true) {
            best = Some((d, verb));
        }
    }
    best.map(|(_, v)| v)
}

fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_verb_is_uniquely_named_and_documented() {
        let mut seen = std::collections::HashSet::new();
        for (name, _, doc) in VERBS {
            assert!(seen.insert(*name), "duplicate verb {name}");
            assert!(!doc.is_empty(), "{name} has no signature");
        }
        // The table is the dispatch: a LiveId collision would silently shadow.
        assert_eq!(verb_table().len(), VERBS.len());
    }

    #[test]
    fn typos_suggest_the_real_verb() {
        assert_eq!(suggest("chekpoint"), Some("checkpoint"));
        assert_eq!(suggest("cammera"), Some("camera"));
        assert_eq!(suggest("zzzzzzzzzz"), None);
    }

    #[test]
    fn racing_fixture_verbs_all_exist() {
        // The 72-line fixture is the acceptance bar for script mode.
        let used = [
            "text", "race", "place", "box", "terrain", "tag", "standings", "speed",
            "spawnpoint", "sky", "rank", "on_tick", "label", "jingle", "finished",
            "checkpoint", "car", "camera", "bar", "autodrive",
        ];
        let table = verb_table();
        for verb in used {
            assert!(
                table.contains_key(&LiveId::from_str(verb)),
                "racing.splash uses game.{verb}, which the table lacks"
            );
        }
    }
}
