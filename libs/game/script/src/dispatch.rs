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

/// Oscillator shape for `tone`/`beep`. Mirrors gamemaker's `synth::Wave`
/// including its parse fallbacks — a host maps this onto its own synth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToneWave {
    Sine,
    Square,
    Saw,
    Triangle,
    Noise,
}

impl ToneWave {
    pub fn parse(name: &str) -> ToneWave {
        match name {
            "sine" => ToneWave::Sine,
            "saw" => ToneWave::Saw,
            "triangle" | "tri" => ToneWave::Triangle,
            "noise" => ToneWave::Noise,
            _ => ToneWave::Square,
        }
    }
}

/// Audio is not a sim concern (M0 moved the synth host-side), so audio verbs
/// queue requests the host drains each tick.
///
/// Sustained tones need an id back *synchronously*, which a drained queue
/// cannot provide, so the ids are minted here and the host keeps the mapping
/// to its own voices. Script only ever treats the value as an opaque handle,
/// so this is observationally identical to gamemaker returning a synth id.
#[derive(Clone, Debug, PartialEq)]
pub enum AudioRequest {
    Sfx { name: String, pitch: f32 },
    Beep { freq: f32, to: f32, ms: f32, gain: f32 },
    Jingle { notes: String, ms: f32 },
    Tone { id: u64, freq: f32, wave: ToneWave, gain: f32 },
    ToneSet { id: u64, freq: Option<f32>, gain: Option<f32> },
    ToneStop { id: u64 },
    StopAllTones,
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
    /// Next sustained-tone handle. Host-owned state expressed as a hook, so
    /// the crate never reaches for a synth it cannot depend on.
    pub next_tone: Rc<Cell<u64>>,
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

// ── decoration: parts, beams ────────────────────────────────────────────

fn v_part(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let owner = arg_id(vm, args, 0);
    let Some(opts) = opts_of(vm, args, 1) else {
        return nil();
    };
    warn_unknown_keys(
        vm,
        &ctx.world,
        "part",
        opts,
        &[id!(pos), id!(size), id!(color), id!(glow), id!(rot_x), id!(rot_y), id!(rot_z), id!(shape)],
    );
    let offset = opt_vec3(vm, opts, id!(pos), vec3f(0.0, 0.0, 0.0));
    let size = opt_vec3(vm, opts, id!(size), vec3f(0.2, 0.2, 0.2));
    let color = opt_color(vm, opts, id!(color), vec4(0.1, 0.1, 0.12, 1.0));
    let glow = opt_f32(vm, opts, id!(glow), 0.0);
    let rot = vec3f(
        opt_f32(vm, opts, id!(rot_x), 0.0),
        opt_f32(vm, opts, id!(rot_y), 0.0),
        opt_f32(vm, opts, id!(rot_z), 0.0),
    );
    let half = vec3f(
        (size.x * 0.5).max(0.005),
        (size.y * 0.5).max(0.005),
        (size.z * 0.5).max(0.005),
    );
    let shape = match opt_string(vm, opts, id!(shape)) {
        Some(name) => Shape::parse(&name),
        None => Shape::Box,
    };
    let mut world = ctx.world.borrow_mut();
    if world.entity(owner).is_none() {
        return nil();
    }
    world.mark_render_dirty();
    world.next_id += 1;
    let id = world.next_id;
    world.parts.push(Part {
        id,
        owner,
        offset,
        rot,
        half,
        target_offset: offset,
        target_rot: rot,
        target_half: half,
        rate: 9.0,
        color,
        glow,
        shape,
        anim_active: false,
    });
    num(id as f64)
}

/// Animate a part: set lerp targets; the engine eases toward them at
/// `rate`/second. Only given keys move.
fn v_move_part(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let pid = arg_id(vm, args, 0);
    let Some(opts) = opts_of(vm, args, 1) else {
        return nil();
    };
    let pos_v = opts_value(vm, opts, id!(pos));
    let size_v = opts_value(vm, opts, id!(size));
    let rx_v = opts_value(vm, opts, id!(rot_x));
    let ry_v = opts_value(vm, opts, id!(rot_y));
    let rz_v = opts_value(vm, opts, id!(rot_z));
    let rate_v = opts_value(vm, opts, id!(rate));
    let pos = if pos_v.is_nil() { None } else { Some(value_vec3(vm, pos_v)) };
    let size = if size_v.is_nil() { None } else { Some(value_vec3(vm, size_v)) };
    let rx = if rx_v.is_nil() { None } else { Some(value_f32(vm, rx_v)) };
    let ry = if ry_v.is_nil() { None } else { Some(value_f32(vm, ry_v)) };
    let rz = if rz_v.is_nil() { None } else { Some(value_f32(vm, rz_v)) };
    let rate = if rate_v.is_nil() { None } else { Some(value_f32(vm, rate_v)) };
    let mut world = ctx.world.borrow_mut();
    if let Some(part) = world.parts.iter_mut().find(|p| p.id == pid) {
        if let Some(pos) = pos {
            part.target_offset = pos;
        }
        if let Some(size) = size {
            part.target_half = vec3f(
                (size.x * 0.5).max(0.005),
                (size.y * 0.5).max(0.005),
                (size.z * 0.5).max(0.005),
            );
        }
        if let Some(rx) = rx {
            part.target_rot.x = rx;
        }
        if let Some(ry) = ry {
            part.target_rot.y = ry;
        }
        if let Some(rz) = rz {
            part.target_rot.z = rz;
        }
        if let Some(rate) = rate {
            part.rate = rate.max(0.1);
        }
        part.anim_active = true;
    }
    // The part leaves the static slab while it animates.
    let owner = world.parts.iter().find(|p| p.id == pid).map(|p| p.owner);
    if let Some(owner) = owner {
        if world.is_static_visual(owner) {
            world.mark_render_dirty();
        }
    }
    nil()
}

fn v_beam(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let from_v = arg(vm, args, 0);
    let from = value_vec3(vm, from_v);
    let to_v = arg(vm, args, 1);
    let to = value_vec3(vm, to_v);
    let opts_v = arg(vm, args, 2);
    let mut size = 0.12f32;
    let mut color = vec4(0.9, 0.9, 0.95, 1.0);
    let mut glow = 0.0f32;
    if let Some(opts) = opts_v.as_object() {
        let size_v = opts_value(vm, opts, id!(size));
        let color_v = opts_value(vm, opts, id!(color));
        let glow_v = opts_value(vm, opts, id!(glow));
        if !size_v.is_nil() {
            size = value_f32(vm, size_v);
        }
        if !color_v.is_nil() {
            color = value_color(vm, color_v);
        }
        if !glow_v.is_nil() {
            glow = value_f32(vm, glow_v);
        }
    }
    ctx.world.borrow_mut().beams.push(Beam {
        from,
        to,
        size: size.clamp(0.01, 4.0),
        color,
        glow,
    });
    nil()
}

// ── attach / ride ───────────────────────────────────────────────────────

fn v_attach(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let rider = arg_id(vm, args, 0);
    let owner = arg_id(vm, args, 1);
    let extra = arg(vm, args, 2);
    // Third arg is either the legacy vec3 offset, or an options object
    // {pos, mode: "ride", spin}. A vec3 parses as a vec3 first.
    let ip = vm.bx.threads.cur_ref().trap.ip;
    let (offset, ride, spin) = match NumericValue::from_script_value_heap(&vm.bx.heap, extra, ip) {
        NumericValue::Vec3(v) => (v, false, 0.0),
        _ => {
            if let Some(opts) = extra.as_object() {
                let offset = opt_vec3(vm, opts, id!(pos), vec3f(0.0, 1.0, 0.0));
                let ride = match opt_string(vm, opts, id!(mode)) {
                    Some(mode) => mode == "ride",
                    None => false,
                };
                let spin = opt_f32(vm, opts, id!(spin), 0.0);
                (offset, ride, spin)
            } else {
                (vec3f(0.0, 1.0, 0.0), false, 0.0)
            }
        }
    };
    if let Some(e) = ctx.world.borrow_mut().entity_mut(rider) {
        e.attached_to = owner;
        e.attach_offset = offset;
        e.attach_ride = ride;
        e.attach_spin = spin;
        e.vel = vec3f(0.0, 0.0, 0.0);
    }
    nil()
}

fn v_detach(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let rider = arg_id(vm, args, 0);
    if let Some(e) = ctx.world.borrow_mut().entity_mut(rider) {
        e.attached_to = 0;
        e.attach_ride = false;
        e.attach_spin = 0.0;
    }
    nil()
}

fn v_speed_mult(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let id = arg_id(vm, args, 0);
    let f = arg_f32(vm, args, 1);
    if let Some(e) = ctx.world.borrow_mut().entity_mut(id) {
        e.speed_mult = f.clamp(0.0, 10.0);
    }
    nil()
}

// ── spatial queries ─────────────────────────────────────────────────────

fn v_raycast(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let from_v = arg(vm, args, 0);
    let from = value_vec3(vm, from_v);
    let dir_v = arg(vm, args, 1);
    let dir = value_vec3(vm, dir_v);
    let max = arg_f32(vm, args, 2).max(0.0);
    let hit = world_raycast(&ctx.world.borrow(), from, dir, max);
    match hit {
        Some((id, pos, normal, dist)) => {
            let obj = vm.bx.heap.new_object();
            vm.bx.heap.set_object_storage_auto(obj);
            // Terrain reports as hit: -1 (u64::MAX doesn't survive f64).
            let hit_id = if id == TERRAIN_ID { -1.0 } else { id as f64 };
            let pos_v = vec3_value(vm, pos);
            let normal_v = vec3_value(vm, normal);
            let heap = &mut vm.bx.heap;
            heap.set_value(obj, id!(hit).into(), ScriptValue::from_f64(hit_id), NoTrap);
            heap.set_value(obj, id!(pos).into(), pos_v, NoTrap);
            heap.set_value(obj, id!(normal).into(), normal_v, NoTrap);
            heap.set_value(obj, id!(dist).into(), ScriptValue::from_f64(dist as f64), NoTrap);
            obj.into()
        }
        None => nil(),
    }
}

fn v_overlap_sphere(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let center_v = arg(vm, args, 0);
    let center = value_vec3(vm, center_v);
    let r = arg_f32(vm, args, 1).max(0.0);
    let ids: Vec<u64> = ctx
        .world
        .borrow()
        .entities
        .iter()
        .filter(|e| {
            // Distance from sphere center to the entity's AABB.
            let dx = ((center.x - e.pos.x).abs() - e.half.x).max(0.0);
            let dy = ((center.y - e.pos.y).abs() - e.half.y).max(0.0);
            let dz = ((center.z - e.pos.z).abs() - e.half.z).max(0.0);
            dx * dx + dy * dy + dz * dz <= r * r
        })
        .map(|e| e.id)
        .collect();
    let array = vm.bx.heap.new_array();
    let trap = vm.bx.threads.cur().trap.pass();
    for id in ids {
        vm.bx.heap.array_push(array, ScriptValue::from_f64(id as f64), trap);
    }
    array.into()
}

fn v_ground_peak(vm: &mut ScriptVm, ctx: &Ctx, _args: ScriptObject) -> ScriptValue {
    // Highest terrain vertex, as vec3 — where the corpus puts the goal.
    let world = ctx.world.borrow();
    let Some(t) = world.terrain.as_ref() else {
        return nil();
    };
    let mut best = (0usize, f32::MIN);
    for (index, h) in t.heights.iter().enumerate() {
        if *h > best.1 {
            best = (index, *h);
        }
    }
    let ix = (best.0 % t.cells) as f32;
    let iz = (best.0 / t.cells) as f32;
    let pos = vec3f(
        t.origin + ix * t.cell_size,
        best.1,
        t.origin + iz * t.cell_size,
    );
    drop(world);
    vec3_value(vm, pos)
}

// ── raw input reads ─────────────────────────────────────────────────────

fn v_held(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let action = arg_string(vm, args, 0);
    let held = ctx.world.borrow().action_held(LiveId::from_str(&action));
    ScriptValue::from_bool(held)
}

fn v_pressed(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let action = arg_string(vm, args, 0);
    let pressed = ctx.world.borrow().action_pressed(LiveId::from_str(&action));
    ScriptValue::from_bool(pressed)
}

fn v_axis(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let neg = arg_string(vm, args, 0);
    let pos = arg_string(vm, args, 1);
    let world = ctx.world.borrow();
    let v = world.action_held(LiveId::from_str(&pos)) as i8 as f64
        - world.action_held(LiveId::from_str(&neg)) as i8 as f64;
    num(v)
}

fn v_player_input(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let id = PlayerId(arg_f32(vm, args, 0) as u32);
    let w = ctx.world.borrow();
    let (move_x, move_z) = w.player_move(id);
    let (axis_x, axis_z) = match w.players.get(id) {
        Some(p) if !id.is_local_slot() => p.input.axes(),
        _ => {
            let key = |name: LiveId| w.held.contains(&name);
            (
                ((key(live_id!(right)) as i8 - key(live_id!(left)) as i8) as f64 + w.pad.axis_x)
                    .clamp(-1.0, 1.0),
                ((key(live_id!(down)) as i8 - key(live_id!(up)) as i8) as f64 + w.pad.axis_z)
                    .clamp(-1.0, 1.0),
            )
        }
    };
    let actions = [
        (live_id!(left), id!(left_pressed)),
        (live_id!(right), id!(right_pressed)),
        (live_id!(up), id!(up_pressed)),
        (live_id!(down), id!(down_pressed)),
        (live_id!(jump), id!(jump_pressed)),
        (live_id!(shoot), id!(shoot_pressed)),
        (live_id!(grab), id!(grab_pressed)),
        (live_id!(reset), id!(reset_pressed)),
    ];
    let states: Vec<(LiveId, LiveId, bool, bool)> = actions
        .iter()
        .map(|(a, p)| (*a, *p, w.action_held_for(id, *a), w.action_pressed_for(id, *a)))
        .collect();
    drop(w);
    let obj = vm.bx.heap.new_object();
    vm.bx.heap.set_object_storage_auto(obj);
    let heap = &mut vm.bx.heap;
    for (action, pressed_key, is_held, was_pressed) in states {
        heap.set_value(obj, action.into(), ScriptValue::from_bool(is_held), NoTrap);
        heap.set_value(obj, pressed_key.into(), ScriptValue::from_bool(was_pressed), NoTrap);
    }
    heap.set_value(obj, id!(axis_x).into(), ScriptValue::from_f64(axis_x), NoTrap);
    heap.set_value(obj, id!(axis_z).into(), ScriptValue::from_f64(axis_z), NoTrap);
    heap.set_value(obj, id!(move_x).into(), ScriptValue::from_f64(move_x), NoTrap);
    heap.set_value(obj, id!(move_z).into(), ScriptValue::from_f64(move_z), NoTrap);
    obj.into()
}

// ── camera reads / writes ───────────────────────────────────────────────

fn v_cam_yaw(_vm: &mut ScriptVm, ctx: &Ctx, _args: ScriptObject) -> ScriptValue {
    num(ctx.world.borrow().cam_yaw as f64)
}

fn v_cam_pitch(_vm: &mut ScriptVm, ctx: &Ctx, _args: ScriptObject) -> ScriptValue {
    num(ctx.world.borrow().cam_pitch as f64)
}

fn v_cam_dist(_vm: &mut ScriptVm, ctx: &Ctx, _args: ScriptObject) -> ScriptValue {
    let world = ctx.world.borrow();
    let d = if world.cam_third != 0 { world.cam_boom } else { world.cam_distance };
    num(d as f64)
}

fn v_cam_fov(_vm: &mut ScriptVm, ctx: &Ctx, _args: ScriptObject) -> ScriptValue {
    num(ctx.world.borrow().cam_fov as f64)
}

fn v_cam_dragging(_vm: &mut ScriptVm, ctx: &Ctx, _args: ScriptObject) -> ScriptValue {
    ScriptValue::from_bool(ctx.world.borrow().cam_dragging)
}

fn v_cam_shake(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let amount = arg_f32(vm, args, 0).max(0.0);
    let mut world = ctx.world.borrow_mut();
    world.cam_shake = (world.cam_shake + amount).clamp(0.0, 1.5);
    nil()
}

fn v_set_cam_yaw(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let yaw = arg_f32(vm, args, 0);
    ctx.world.borrow_mut().cam_yaw_request = Some(yaw);
    nil()
}

fn v_set_cam_pitch(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let pitch = arg_f32(vm, args, 0).clamp(-1.2, 0.25);
    ctx.world.borrow_mut().cam_pitch_request = Some(pitch);
    nil()
}

fn v_set_cam_dist(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let d = arg_f32(vm, args, 0);
    let mut world = ctx.world.borrow_mut();
    if world.cam_third != 0 {
        world.cam_boom = d.clamp(1.0, 60.0);
    } else {
        world.cam_distance = d.clamp(0.5, 120.0);
    }
    nil()
}

fn v_set_cam_fov(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let fov = arg_f32(vm, args, 0).clamp(20.0, 120.0);
    ctx.world.borrow_mut().cam_fov = fov;
    nil()
}

// ── persistence ─────────────────────────────────────────────────────────

fn v_save(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let key = arg_string(vm, args, 0);
    let v = arg(vm, args, 1);
    // Strings first: the numeric cast coerces strings to NaN.
    let val = if let Some(text) = vm.bx.heap.string_with(v, |_, s| s.to_string()) {
        SaveVal::Str(text)
    } else {
        SaveVal::Num(value_f32(vm, v) as f64)
    };
    let mut world = ctx.world.borrow_mut();
    world.save_data.insert(key, val);
    world.save_dirty = true;
    nil()
}

fn v_load(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let key = arg_string(vm, args, 0);
    let world_ref = ctx.world.borrow();
    match world_ref.save_data.get(&key) {
        Some(SaveVal::Num(n)) => num(*n),
        Some(SaveVal::Str(text)) => {
            let text = text.clone();
            drop(world_ref);
            vm.bx.heap.new_string_from_str(&text)
        }
        None => {
            drop(world_ref);
            arg(vm, args, 1) // the default, or NIL
        }
    }
}

// ── sustained tones ─────────────────────────────────────────────────────

fn v_tone(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let mut freq = 220.0f32;
    let mut gain = 0.15f32;
    let mut wave = ToneWave::Saw;
    if let Some(opts) = opts_of(vm, args, 0) {
        warn_unknown_keys(vm, &ctx.world, "tone", opts, &[id!(freq), id!(gain), id!(wave)]);
        freq = opt_f32(vm, opts, id!(freq), freq);
        gain = opt_f32(vm, opts, id!(gain), gain);
        if let Some(name) = opt_string(vm, opts, id!(wave)) {
            wave = ToneWave::parse(&name);
        }
    }
    let id = ctx.next_tone.get().wrapping_add(1);
    ctx.next_tone.set(id);
    ctx.audio
        .borrow_mut()
        .push(AudioRequest::Tone { id, freq, wave, gain });
    num(id as f64)
}

fn v_tone_set(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let id = arg_id(vm, args, 0);
    let mut freq = None;
    let mut gain = None;
    if let Some(opts) = opts_of(vm, args, 1) {
        warn_unknown_keys(vm, &ctx.world, "tone_set", opts, &[id!(freq), id!(gain)]);
        let freq_v = opts_value(vm, opts, id!(freq));
        let gain_v = opts_value(vm, opts, id!(gain));
        if !freq_v.is_nil() {
            freq = Some(value_f32(vm, freq_v));
        }
        if !gain_v.is_nil() {
            gain = Some(value_f32(vm, gain_v));
        }
    }
    ctx.audio
        .borrow_mut()
        .push(AudioRequest::ToneSet { id, freq, gain });
    nil()
}

fn v_tone_stop(vm: &mut ScriptVm, ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let id = arg_id(vm, args, 0);
    ctx.audio.borrow_mut().push(AudioRequest::ToneStop { id });
    nil()
}

// ── HUD extras / introspection / lifecycle ──────────────────────────────

fn v_format(vm: &mut ScriptVm, _ctx: &Ctx, args: ScriptObject) -> ScriptValue {
    let value = arg_f32(vm, args, 0) as f64;
    let decimals = arg(vm, args, 1);
    let decimals = if decimals.is_nil() {
        1
    } else {
        (value_f32(vm, decimals) as usize).min(6)
    };
    let text = format!("{:.*}", decimals, value);
    vm.bx.heap.new_string_from_str(&text)
}

fn v_api(_vm: &mut ScriptVm, ctx: &Ctx, _args: ScriptObject) -> ScriptValue {
    // Introspection: dump the whole verb surface so the agent can lint
    // itself without six test cycles.
    let mut world = ctx.world.borrow_mut();
    world.log("game.* API:".to_string());
    for (verb, _, sig) in VERBS {
        world.log(format!("  game.{verb}{sig}"));
    }
    nil()
}

fn v_reset(_vm: &mut ScriptVm, ctx: &Ctx, _args: ScriptObject) -> ScriptValue {
    // Release this world's callback slots, then wipe.
    {
        let mut world = ctx.world.borrow_mut();
        let mut callbacks = ctx.callbacks.borrow_mut();
        if let Some(slot) = world.on_tick.take() {
            callbacks.free(slot);
        }
        if let Some(slot) = world.on_touch.take() {
            callbacks.free(slot);
        }
        for timer in world.timers.drain(..) {
            callbacks.free(timer.func);
        }
        world.reset_content();
    }
    ctx.blocks.borrow_mut().clear();
    ctx.audio.borrow_mut().push(AudioRequest::StopAllTones);
    nil()
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
    ("player_input", v_player_input, "(player) -> {left..back, *_pressed, axis_x, axis_z, move_x, move_z}"),
    ("bot", v_bot, "(name) -> player id"),
    ("part", v_part, "(owner, {pos, size, color, glow, rot_x, rot_y, rot_z, shape}) -> part id"),
    ("move_part", v_move_part, "(part, {pos, size, rot_x, rot_y, rot_z, rate}) — eases to targets"),
    ("beam", v_beam, "(from, to, {size, color, glow}) — immediate mode, re-issue each tick"),
    ("attach", v_attach, "(rider, owner, vec3 | {pos, mode: \"ride\", spin})"),
    ("detach", v_detach, "(rider)"),
    ("speed_mult", v_speed_mult, "(id, factor) — 0..10"),
    ("raycast", v_raycast, "(from, dir, max) -> {hit, pos, normal, dist} (hit -1 = terrain)"),
    ("overlap_sphere", v_overlap_sphere, "(center, radius) -> [ids]"),
    ("ground_peak", v_ground_peak, "() -> vec3 of the highest terrain vertex"),
    ("held", v_held, "(action) -> bool"),
    ("pressed", v_pressed, "(action) -> bool (this tick)"),
    ("axis", v_axis, "(neg_action, pos_action) -> -1..1"),
    ("cam_yaw", v_cam_yaw, "() -> yaw"),
    ("cam_pitch", v_cam_pitch, "() -> pitch"),
    ("cam_dist", v_cam_dist, "() -> boom in third person, else distance"),
    ("cam_fov", v_cam_fov, "() -> fov"),
    ("cam_dragging", v_cam_dragging, "() -> bool"),
    ("cam_shake", v_cam_shake, "(amount) — accumulates, clamped 0..1.5"),
    ("set_cam_yaw", v_set_cam_yaw, "(yaw)"),
    ("set_cam_pitch", v_set_cam_pitch, "(pitch) — clamped -1.2..0.25"),
    ("set_cam_dist", v_set_cam_dist, "(d) — boom 1..60 in third person, else distance 0.5..120"),
    ("set_cam_fov", v_set_cam_fov, "(fov) — clamped 20..120"),
    ("save", v_save, "(key, value) — numbers and strings"),
    ("load", v_load, "(key, default) -> value"),
    ("tone", v_tone, "({freq, gain, wave}) -> tone id — sustained"),
    ("tone_set", v_tone_set, "(tone id, {freq, gain})"),
    ("tone_stop", v_tone_stop, "(tone id)"),
    ("format", v_format, "(value, decimals) -> string"),
    ("api", v_api, "() — log every verb and signature"),
    ("reset", v_reset, "() — wipe world content, timers, callbacks, blocks, tones"),
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

    /// A VM with no Cx — enough to build args objects and call verbs
    /// directly, which is how every verb test below drives dispatch.
    struct Harness {
        std: usize,
        host: usize,
    }

    impl Harness {
        fn new() -> Self {
            Self { std: 0, host: 0 }
        }

        fn vm(&mut self) -> ScriptVm<'_> {
            ScriptVm {
                host: &mut self.host,
                std: &mut self.std,
                bx: Box::new(makepad_widgets::makepad_script::ScriptVmBase::new()),
            }
        }
    }

    fn ctx_with(world: GameWorld) -> Ctx {
        Ctx {
            world: Rc::new(RefCell::new(world)),
            blocks: Rc::new(RefCell::new(Blocks::new())),
            callbacks: Rc::new(RefCell::new(CallbackTable::default())),
            audio: Rc::new(RefCell::new(Vec::new())),
            eval_gen: Rc::new(Cell::new(1)),
            next_tone: Rc::new(Cell::new(0)),
        }
    }

    /// Build a positional args object, the shape a `game.verb(a, b, ...)`
    /// call hands the dispatcher.
    fn args_of(vm: &mut ScriptVm, values: &[ScriptValue]) -> ScriptObject {
        let obj = vm.bx.heap.new_object();
        vm.bx.heap.set_object_storage_vec2(obj);
        for v in values {
            vm.bx.heap.vec_push_unchecked(obj, NIL, *v);
        }
        obj
    }

    fn opts_of_pairs(vm: &mut ScriptVm, pairs: &[(LiveId, ScriptValue)]) -> ScriptValue {
        let obj = vm.bx.heap.new_object();
        vm.bx.heap.set_object_storage_auto(obj);
        for (k, v) in pairs {
            vm.bx.heap.set_value(obj, (*k).into(), *v, NoTrap);
        }
        obj.into()
    }

    fn call(
        verb: &str,
        vm: &mut ScriptVm,
        ctx: &Ctx,
        values: &[ScriptValue],
    ) -> ScriptValue {
        let f = verb_table()[&LiveId::from_str(verb)];
        let args = args_of(vm, values);
        f(vm, ctx, args)
    }

    /// A world with one static box at the origin, id returned.
    fn world_with_box() -> (GameWorld, u64) {
        let mut world = GameWorld::new();
        world.next_id += 1;
        let id = world.next_id;
        let mut e = Entity::default();
        e.id = id;
        e.kind = BodyKind::Static;
        e.half = vec3f(0.5, 0.5, 0.5);
        world.push_entity(e);
        (world, id)
    }

    #[test]
    fn part_appends_and_move_part_sets_targets() {
        let (world, owner) = world_with_box();
        let ctx = ctx_with(world);
        let mut h = Harness::new();
        let mut vm = h.vm();

        let opts = opts_of_pairs(
            &mut vm,
            &[
                (id!(size), ScriptValue::from_f64(1.0)),
                (id!(glow), ScriptValue::from_f64(0.5)),
                (id!(rot_y), ScriptValue::from_f64(0.25)),
            ],
        );
        let pid = call("part", &mut vm, &ctx, &[ScriptValue::from_f64(owner as f64), opts]);
        let pid = pid.as_f64().unwrap() as u64;
        {
            let w = ctx.world.borrow();
            assert_eq!(w.parts.len(), 1);
            let p = &w.parts[0];
            assert_eq!(p.owner, owner);
            assert_eq!(p.half, vec3f(0.5, 0.5, 0.5)); // size 1.0 -> half 0.5
            assert_eq!(p.glow, 0.5);
            assert_eq!(p.rot.y, 0.25);
            // Targets start equal to the pose: nothing animates yet.
            assert_eq!(p.target_offset, p.offset);
            assert!(!p.anim_active);
            assert_eq!(p.rate, 9.0);
        }

        // Ordering: a second part appends after the first.
        let opts2 = opts_of_pairs(&mut vm, &[(id!(glow), ScriptValue::from_f64(1.0))]);
        let pid2 = call("part", &mut vm, &ctx, &[ScriptValue::from_f64(owner as f64), opts2]);
        let pid2 = pid2.as_f64().unwrap() as u64;
        assert!(pid2 > pid);
        assert_eq!(ctx.world.borrow().parts[1].id, pid2);

        // move_part touches only the given keys, and arms the animation.
        let mopts = opts_of_pairs(
            &mut vm,
            &[
                (id!(rot_x), ScriptValue::from_f64(2.0)),
                (id!(rate), ScriptValue::from_f64(3.0)),
            ],
        );
        call("move_part", &mut vm, &ctx, &[ScriptValue::from_f64(pid as f64), mopts]);
        let w = ctx.world.borrow();
        let p = &w.parts[0];
        assert_eq!(p.target_rot.x, 2.0);
        assert_eq!(p.target_rot.y, 0.25); // untouched key keeps its value
        assert_eq!(p.rate, 3.0);
        assert!(p.anim_active);
    }

    #[test]
    fn part_on_a_missing_owner_is_a_no_op() {
        let ctx = ctx_with(GameWorld::new());
        let mut h = Harness::new();
        let mut vm = h.vm();
        let opts = opts_of_pairs(&mut vm, &[]);
        let r = call("part", &mut vm, &ctx, &[ScriptValue::from_f64(999.0), opts]);
        assert!(r.is_nil());
        assert!(ctx.world.borrow().parts.is_empty());
    }

    #[test]
    fn attach_takes_a_vec3_or_a_ride_options_object_and_detach_clears() {
        let (mut world, owner) = world_with_box();
        world.next_id += 1;
        let rider = world.next_id;
        let mut e = Entity::default();
        e.id = rider;
        e.kind = BodyKind::Mover;
        e.vel = vec3f(1.0, 2.0, 3.0);
        world.push_entity(e);
        let ctx = ctx_with(world);
        let mut h = Harness::new();
        let mut vm = h.vm();

        // Legacy vec3 form: plain offset, not a ride.
        let off = vec3_value(&mut vm, vec3f(0.0, 2.0, 0.0));
        call(
            "attach",
            &mut vm,
            &ctx,
            &[ScriptValue::from_f64(rider as f64), ScriptValue::from_f64(owner as f64), off],
        );
        {
            let w = ctx.world.borrow();
            let e = w.entity(rider).unwrap();
            assert_eq!(e.attached_to, owner);
            assert_eq!(e.attach_offset, vec3f(0.0, 2.0, 0.0));
            assert!(!e.attach_ride);
            // Attaching zeroes velocity so the rider stops fighting the pin.
            assert_eq!(e.vel, vec3f(0.0, 0.0, 0.0));
        }

        // Options form with ride + spin.
        let mode = vm.bx.heap.new_string_from_str("ride");
        let opts = opts_of_pairs(
            &mut vm,
            &[(id!(mode), mode), (id!(spin), ScriptValue::from_f64(4.0))],
        );
        call(
            "attach",
            &mut vm,
            &ctx,
            &[ScriptValue::from_f64(rider as f64), ScriptValue::from_f64(owner as f64), opts],
        );
        {
            let w = ctx.world.borrow();
            let e = w.entity(rider).unwrap();
            assert!(e.attach_ride);
            assert_eq!(e.attach_spin, 4.0);
            // No pos key -> the documented default, not the previous offset.
            assert_eq!(e.attach_offset, vec3f(0.0, 1.0, 0.0));
        }

        call("detach", &mut vm, &ctx, &[ScriptValue::from_f64(rider as f64)]);
        let w = ctx.world.borrow();
        let e = w.entity(rider).unwrap();
        assert_eq!(e.attached_to, 0);
        assert!(!e.attach_ride);
        assert_eq!(e.attach_spin, 0.0);
    }

    #[test]
    fn beam_pushes_an_immediate_mode_segment_with_clamped_size() {
        let ctx = ctx_with(GameWorld::new());
        let mut h = Harness::new();
        let mut vm = h.vm();
        let from = vec3_value(&mut vm, vec3f(0.0, 0.0, 0.0));
        let to = vec3_value(&mut vm, vec3f(0.0, 5.0, 0.0));
        let opts = opts_of_pairs(&mut vm, &[(id!(size), ScriptValue::from_f64(99.0))]);
        call("beam", &mut vm, &ctx, &[from, to, opts]);
        let w = ctx.world.borrow();
        assert_eq!(w.beams.len(), 1);
        assert_eq!(w.beams[0].to, vec3f(0.0, 5.0, 0.0));
        assert_eq!(w.beams[0].size, 4.0); // clamped 0.01..4.0
    }

    #[test]
    fn speed_mult_clamps_to_the_documented_range() {
        let (world, id) = world_with_box();
        let ctx = ctx_with(world);
        let mut h = Harness::new();
        let mut vm = h.vm();
        call(
            "speed_mult",
            &mut vm,
            &ctx,
            &[ScriptValue::from_f64(id as f64), ScriptValue::from_f64(50.0)],
        );
        assert_eq!(ctx.world.borrow().entity(id).unwrap().speed_mult, 10.0);
        call(
            "speed_mult",
            &mut vm,
            &ctx,
            &[ScriptValue::from_f64(id as f64), ScriptValue::from_f64(-3.0)],
        );
        assert_eq!(ctx.world.borrow().entity(id).unwrap().speed_mult, 0.0);
    }

    #[test]
    fn raycast_hits_known_geometry_and_misses_past_it() {
        let (mut world, id) = world_with_box();
        world.entity_mut(id).unwrap().pos = vec3f(0.0, 0.0, 10.0);
        let ctx = ctx_with(world);
        let mut h = Harness::new();
        let mut vm = h.vm();

        let from = vec3_value(&mut vm, vec3f(0.0, 0.0, 0.0));
        let dir = vec3_value(&mut vm, vec3f(0.0, 0.0, 1.0));
        let hit = call("raycast", &mut vm, &ctx, &[from, dir, ScriptValue::from_f64(50.0)]);
        let obj = hit.as_object().expect("ray should hit the box");
        let hit_id = vm.bx.heap.value(obj, id!(hit).into(), NoTrap);
        let dist = vm.bx.heap.value(obj, id!(dist).into(), NoTrap);
        assert_eq!(hit_id.as_f64().unwrap() as u64, id);
        // Box spans z 9.5..10.5. world_raycast marches in 0.15 steps and
        // reports the marched t, so the hit lands within one step past the
        // face — assert that contract rather than an analytic 9.5.
        let dist = dist.as_f64().unwrap();
        assert!(dist >= 9.5 && dist < 9.5 + 0.15, "hit at {dist}, want 9.5..9.65");

        // Too short to reach: a miss is NIL, not a zero-distance hit.
        let from = vec3_value(&mut vm, vec3f(0.0, 0.0, 0.0));
        let dir = vec3_value(&mut vm, vec3f(0.0, 0.0, 1.0));
        let miss = call("raycast", &mut vm, &ctx, &[from, dir, ScriptValue::from_f64(2.0)]);
        assert!(miss.is_nil());
    }

    #[test]
    fn overlap_sphere_selects_by_aabb_distance() {
        let (mut world, near) = world_with_box();
        world.next_id += 1;
        let far = world.next_id;
        let mut e = Entity::default();
        e.id = far;
        e.kind = BodyKind::Static;
        e.pos = vec3f(100.0, 0.0, 0.0);
        e.half = vec3f(0.5, 0.5, 0.5);
        world.push_entity(e);
        let ctx = ctx_with(world);
        let mut h = Harness::new();
        let mut vm = h.vm();

        let center = vec3_value(&mut vm, vec3f(0.0, 0.0, 0.0));
        let r = call("overlap_sphere", &mut vm, &ctx, &[center, ScriptValue::from_f64(2.0)]);
        let arr = r.as_array().expect("overlap_sphere returns an array");
        assert_eq!(vm.bx.heap.array_len(arr), 1);
        let first = vm.bx.heap.array_index(arr, 0, NoTrap);
        assert_eq!(first.as_f64().unwrap() as u64, near);
        assert_ne!(first.as_f64().unwrap() as u64, far);
    }

    #[test]
    fn save_and_load_round_trip_numbers_strings_and_defaults() {
        let ctx = ctx_with(GameWorld::new());
        let mut h = Harness::new();
        let mut vm = h.vm();

        let key = vm.bx.heap.new_string_from_str("best_lap");
        call("save", &mut vm, &ctx, &[key, ScriptValue::from_f64(12.5)]);
        assert!(ctx.world.borrow().save_dirty);

        let key = vm.bx.heap.new_string_from_str("best_lap");
        let got = call("load", &mut vm, &ctx, &[key]);
        assert_eq!(got.as_f64().unwrap(), 12.5);

        // Strings survive as strings — the numeric cast would NaN them.
        let key = vm.bx.heap.new_string_from_str("name");
        let val = vm.bx.heap.new_string_from_str("ada");
        call("save", &mut vm, &ctx, &[key, val]);
        let key = vm.bx.heap.new_string_from_str("name");
        let got = call("load", &mut vm, &ctx, &[key]);
        let text = vm.bx.heap.string_with(got, |_, s| s.to_string());
        assert_eq!(text.as_deref(), Some("ada"));

        // Missing key returns the supplied default.
        let key = vm.bx.heap.new_string_from_str("absent");
        let got = call("load", &mut vm, &ctx, &[key, ScriptValue::from_f64(7.0)]);
        assert_eq!(got.as_f64().unwrap(), 7.0);
    }

    #[test]
    fn camera_reads_and_writes_pair_up() {
        let ctx = ctx_with(GameWorld::new());
        let mut h = Harness::new();
        let mut vm = h.vm();

        // Writes land in the request mailbox, not the live value: the tick
        // drains them, so a read right after a write still sees the old pose.
        call("set_cam_yaw", &mut vm, &ctx, &[ScriptValue::from_f64(1.5)]);
        assert_eq!(ctx.world.borrow().cam_yaw_request, Some(1.5));

        call("set_cam_pitch", &mut vm, &ctx, &[ScriptValue::from_f64(-9.0)]);
        assert_eq!(ctx.world.borrow().cam_pitch_request, Some(-1.2)); // clamped

        call("set_cam_fov", &mut vm, &ctx, &[ScriptValue::from_f64(500.0)]);
        assert_eq!(ctx.world.borrow().cam_fov, 120.0); // clamped
        let fov = call("cam_fov", &mut vm, &ctx, &[]);
        assert_eq!(fov.as_f64().unwrap(), 120.0);

        // dist writes boom in third person, distance otherwise.
        call("set_cam_dist", &mut vm, &ctx, &[ScriptValue::from_f64(9.0)]);
        assert_eq!(ctx.world.borrow().cam_distance, 9.0);
        let d = call("cam_dist", &mut vm, &ctx, &[]);
        assert_eq!(d.as_f64().unwrap(), 9.0);
        ctx.world.borrow_mut().cam_third = 42;
        call("set_cam_dist", &mut vm, &ctx, &[ScriptValue::from_f64(1000.0)]);
        assert_eq!(ctx.world.borrow().cam_boom, 60.0); // clamped 1..60
        let d = call("cam_dist", &mut vm, &ctx, &[]);
        assert_eq!(d.as_f64().unwrap(), 60.0);

        // shake accumulates and clamps; dragging reads through.
        call("cam_shake", &mut vm, &ctx, &[ScriptValue::from_f64(1.0)]);
        call("cam_shake", &mut vm, &ctx, &[ScriptValue::from_f64(1.0)]);
        assert_eq!(ctx.world.borrow().cam_shake, 1.5);
        ctx.world.borrow_mut().cam_dragging = true;
        assert_eq!(call("cam_dragging", &mut vm, &ctx, &[]).as_bool(), Some(true));

        ctx.world.borrow_mut().cam_yaw = 0.75;
        assert_eq!(call("cam_yaw", &mut vm, &ctx, &[]).as_f64().unwrap(), 0.75);
    }

    #[test]
    fn tone_lifecycle_queues_requests_with_stable_handles() {
        let ctx = ctx_with(GameWorld::new());
        let mut h = Harness::new();
        let mut vm = h.vm();

        let wave = vm.bx.heap.new_string_from_str("sine");
        let opts = opts_of_pairs(
            &mut vm,
            &[(id!(freq), ScriptValue::from_f64(440.0)), (id!(wave), wave)],
        );
        let id_a = call("tone", &mut vm, &ctx, &[opts]).as_f64().unwrap() as u64;
        let opts = opts_of_pairs(&mut vm, &[]);
        let id_b = call("tone", &mut vm, &ctx, &[opts]).as_f64().unwrap() as u64;
        assert_ne!(id_a, id_b, "each tone gets its own handle");

        let opts = opts_of_pairs(&mut vm, &[(id!(gain), ScriptValue::from_f64(0.5))]);
        call("tone_set", &mut vm, &ctx, &[ScriptValue::from_f64(id_a as f64), opts]);
        call("tone_stop", &mut vm, &ctx, &[ScriptValue::from_f64(id_a as f64)]);

        let audio = ctx.audio.borrow();
        assert_eq!(
            audio[0],
            AudioRequest::Tone { id: id_a, freq: 440.0, wave: ToneWave::Sine, gain: 0.15 }
        );
        // No opts -> documented defaults, saw at 220.
        assert_eq!(
            audio[1],
            AudioRequest::Tone { id: id_b, freq: 220.0, wave: ToneWave::Saw, gain: 0.15 }
        );
        assert_eq!(
            audio[2],
            AudioRequest::ToneSet { id: id_a, freq: None, gain: Some(0.5) }
        );
        assert_eq!(audio[3], AudioRequest::ToneStop { id: id_a });
    }

    #[test]
    fn unknown_wave_names_fall_back_to_square() {
        assert_eq!(ToneWave::parse("sine"), ToneWave::Sine);
        assert_eq!(ToneWave::parse("tri"), ToneWave::Triangle);
        assert_eq!(ToneWave::parse("triangle"), ToneWave::Triangle);
        assert_eq!(ToneWave::parse("noise"), ToneWave::Noise);
        assert_eq!(ToneWave::parse("saw"), ToneWave::Saw);
        assert_eq!(ToneWave::parse("wobble"), ToneWave::Square);
    }

    #[test]
    fn format_rounds_and_defaults_to_one_decimal() {
        let ctx = ctx_with(GameWorld::new());
        let mut h = Harness::new();
        let mut vm = h.vm();
        let r = call("format", &mut vm, &ctx, &[ScriptValue::from_f64(3.14159)]);
        assert_eq!(vm.bx.heap.string_with(r, |_, s| s.to_string()).as_deref(), Some("3.1"));
        let r = call(
            "format",
            &mut vm,
            &ctx,
            &[ScriptValue::from_f64(3.14159), ScriptValue::from_f64(3.0)],
        );
        assert_eq!(vm.bx.heap.string_with(r, |_, s| s.to_string()).as_deref(), Some("3.142"));
        // Decimals cap at 6 rather than panicking on a silly precision.
        let r = call(
            "format",
            &mut vm,
            &ctx,
            &[ScriptValue::from_f64(1.0), ScriptValue::from_f64(99.0)],
        );
        assert_eq!(
            vm.bx.heap.string_with(r, |_, s| s.to_string()).as_deref(),
            Some("1.000000")
        );
    }

    #[test]
    fn api_logs_every_verb_exactly_once() {
        let ctx = ctx_with(GameWorld::new());
        let mut h = Harness::new();
        let mut vm = h.vm();
        call("api", &mut vm, &ctx, &[]);
        let world = ctx.world.borrow();
        // Header line plus one row per verb.
        assert_eq!(world.log_pending.len(), VERBS.len() + 1);
        // Alias rows document as "alias of box" rather than a signature, so
        // match the verb name itself, not a following paren.
        for (verb, _, _) in VERBS {
            let needle = format!("  game.{verb}");
            assert!(
                world.log_pending.iter().any(|l| l.starts_with(&needle)),
                "api() never mentioned game.{verb}"
            );
        }
    }

    #[test]
    fn reset_wipes_content_timers_and_tones() {
        let (mut world, _) = world_with_box();
        world.timers.push(GameTimer {
            id: 1,
            at_tick: 60,
            interval_ticks: 0,
            func: CallbackSlot(0),
        });
        let ctx = ctx_with(world);
        let mut h = Harness::new();
        let mut vm = h.vm();
        assert_eq!(ctx.world.borrow().entities.len(), 1);

        call("reset", &mut vm, &ctx, &[]);

        let w = ctx.world.borrow();
        assert!(w.entities.is_empty());
        assert!(w.timers.is_empty());
        assert!(w.on_tick.is_none());
        drop(w);
        assert_eq!(ctx.audio.borrow().last(), Some(&AudioRequest::StopAllTones));
    }

    #[test]
    fn ground_peak_and_input_reads_degrade_without_terrain_or_input() {
        let ctx = ctx_with(GameWorld::new());
        let mut h = Harness::new();
        let mut vm = h.vm();
        // No terrain: NIL rather than a bogus origin vec3.
        assert!(call("ground_peak", &mut vm, &ctx, &[]).is_nil());

        let a = vm.bx.heap.new_string_from_str("jump");
        assert_eq!(call("held", &mut vm, &ctx, &[a]).as_bool(), Some(false));
        let a = vm.bx.heap.new_string_from_str("jump");
        assert_eq!(call("pressed", &mut vm, &ctx, &[a]).as_bool(), Some(false));

        ctx.world.borrow_mut().held.insert(live_id!(right));
        let neg = vm.bx.heap.new_string_from_str("left");
        let pos = vm.bx.heap.new_string_from_str("right");
        assert_eq!(call("axis", &mut vm, &ctx, &[neg, pos]).as_f64().unwrap(), 1.0);
    }

    #[test]
    fn player_input_reports_the_local_players_state() {
        let ctx = ctx_with(GameWorld::new());
        ctx.world.borrow_mut().held.insert(live_id!(jump));
        let mut h = Harness::new();
        let mut vm = h.vm();
        let r = call("player_input", &mut vm, &ctx, &[ScriptValue::from_f64(0.0)]);
        let obj = r.as_object().expect("player_input returns an object");
        let jump = vm.bx.heap.value(obj, id!(jump).into(), NoTrap);
        assert_eq!(jump.as_bool(), Some(true));
        // The camera-relative pair is always present, even at rest.
        assert!(!vm.bx.heap.value(obj, id!(move_x).into(), NoTrap).is_nil());
        assert!(!vm.bx.heap.value(obj, id!(move_z).into(), NoTrap).is_nil());
    }

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

    /// Parity with gamemaker's binding layer, which this crate is meant to
    /// replace. gamemaker documents 102 rows (98 dispatch arms + 4 names that
    /// share an arm: block/box, teleport/set_pos, on_leave/on_join). If that
    /// count moves, the two implementations have drifted again.
    #[test]
    fn the_verb_surface_matches_gamemakers() {
        assert_eq!(
            VERBS.len(),
            102,
            "verb count changed — sync with examples/gamemaker GAME_API and splashgame.md"
        );
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
