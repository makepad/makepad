//! World-building verbs: `game.box/mover/spawn` and `game.terrain`.
//!
//! Ported verbatim from gamemaker's game_view.rs (M4). These are pure
//! `fn(vm, world, opts)` over the sim — no host state — and the terrain noise
//! shaping in particular must stay bit-identical so a fixture authored against
//! gamemaker builds the same world here.

use crate::dispatch::warn_unknown_keys;
use crate::value::*;
use makepad_game_sim::*;
use makepad_widgets::*;
use std::cell::RefCell;
use std::rc::Rc;

pub(crate) fn spawn_entity(
    vm: &mut ScriptVm,
    world: &Rc<RefCell<GameWorld>>,
    args: ScriptObject,
    kind: BodyKind,
) -> ScriptValue {
    spawn_entity_inner(vm, world, args, kind, true)
}

/// Block verbs (car/character/plane) reuse this body-spawning path but have
/// already validated their own, larger option set — re-checking here would
/// flag every block option (`top_speed`, `player`, ...) as an unknown box
/// option. A typo warning that cries wolf is worse than none.
pub(crate) fn spawn_entity_unchecked(
    vm: &mut ScriptVm,
    world: &Rc<RefCell<GameWorld>>,
    args: ScriptObject,
    kind: BodyKind,
) -> ScriptValue {
    spawn_entity_inner(vm, world, args, kind, false)
}

fn spawn_entity_inner(
    vm: &mut ScriptVm,
    world: &Rc<RefCell<GameWorld>>,
    args: ScriptObject,
    kind: BodyKind,
    warn: bool,
) -> ScriptValue {
    let opts_val = arg(vm, args, 0);
    let Some(opts) = opts_val.as_object() else {
        return NIL;
    };
    if warn {
    warn_unknown_keys(
        vm,
        world,
        "box/mover/spawn",
        opts,
        &[
            id!(pos),
            id!(size),
            id!(color),
            id!(tag),
            id!(sensor),
            id!(collide),
            id!(body),
            id!(gravity),
            id!(vel),
            id!(life),
            id!(hits),
            id!(glow),
            id!(face),
            id!(rot_y),
            id!(turn_rate),
            id!(shape),
            id!(density),
            id!(friction),
            id!(restitution),
        ],
    );
    }
    let pos_v = opts_value(vm, opts, id!(pos));
    let size_v = opts_value(vm, opts, id!(size));
    let color_v = opts_value(vm, opts, id!(color));
    let tag_v = opts_value(vm, opts, id!(tag));
    let sensor_v = opts_value(vm, opts, id!(sensor));
    let body_v = opts_value(vm, opts, id!(body));
    let gravity_v = opts_value(vm, opts, id!(gravity));
    let vel_v = opts_value(vm, opts, id!(vel));
    let life_v = opts_value(vm, opts, id!(life));
    let hits_v = opts_value(vm, opts, id!(hits));
    let glow_v = opts_value(vm, opts, id!(glow));
    let face_v = opts_value(vm, opts, id!(face));
    let rot_y_v = opts_value(vm, opts, id!(rot_y));
    let collide_v = opts_value(vm, opts, id!(collide));
    let turn_v = opts_value(vm, opts, id!(turn_rate));

    let pos = if pos_v.is_nil() { vec3f(0.0, 0.0, 0.0) } else { value_vec3(vm, pos_v) };
    let size = if size_v.is_nil() { vec3f(1.0, 1.0, 1.0) } else { value_vec3(vm, size_v) };
    let color = if color_v.is_nil() { vec4(0.75, 0.75, 0.8, 1.0) } else { value_color(vm, color_v) };
    let tag = if tag_v.is_nil() {
        String::new()
    } else {
        vm.bx.heap.temp_string_with(|heap, out| {
            heap.cast_to_string(tag_v, out);
            out.to_string()
        })
    };
    let sensor = sensor_v.as_bool().unwrap_or(false);
    let gravity_scale = if gravity_v.is_nil() {
        1.0
    } else {
        let ip = vm.bx.threads.cur_ref().trap.ip;
        vm.bx.heap.cast_to_f64(gravity_v, ip) as f32
    };

    // `body: "kinematic"` upgrades a box to a script-driven platform.
    let kind = if body_v.is_nil() {
        kind
    } else {
        let body = vm.bx.heap.temp_string_with(|heap, out| {
            heap.cast_to_string(body_v, out);
            out.to_string()
        });
        match body.as_str() {
            "kinematic" => BodyKind::Kinematic,
            "mover" => BodyKind::Mover,
            // Full box3d dynamics (M1a): stacks, tumbles, impulses. Shared
            // replication tier; collides with statics/kinematics/rigids,
            // not with movers.
            "rigid" => BodyKind::Rigid,
            _ => kind,
        }
    };

    let vel = if vel_v.is_nil() { vec3f(0.0, 0.0, 0.0) } else { value_vec3(vm, vel_v) };
    let life = if life_v.is_nil() {
        0.0
    } else {
        let ip = vm.bx.threads.cur_ref().trap.ip;
        (vm.bx.heap.cast_to_f64(life_v, ip) as f32).max(0.0)
    };
    let hits = hits_v.as_bool().unwrap_or(false);
    let ip = vm.bx.threads.cur_ref().trap.ip;
    let glow = if glow_v.is_nil() { 0.0 } else { vm.bx.heap.cast_to_f64(glow_v, ip) as f32 };
    // `rot_y:` is the natural spelling for placed geometry (a rotated road
    // slab); `face:` reads better for characters. Same visual yaw either way.
    let yaw = if !rot_y_v.is_nil() {
        vm.bx.heap.cast_to_f64(rot_y_v, ip) as f32
    } else if !face_v.is_nil() {
        vm.bx.heap.cast_to_f64(face_v, ip) as f32
    } else {
        0.0
    };
    let collide = collide_v.as_bool().unwrap_or(true);
    let turn_rate = if turn_v.is_nil() { 7.0 } else { vm.bx.heap.cast_to_f64(turn_v, ip) as f32 };
    // Rigid material params (harmless defaults on every other body kind).
    let f32_or = |vm: &mut ScriptVm, key: LiveId, default: f32| -> f32 {
        let v = opts_value(vm, opts, key);
        if v.is_nil() {
            default
        } else {
            let ip = vm.bx.threads.cur_ref().trap.ip;
            vm.bx.heap.cast_to_f64(v, ip) as f32
        }
    };
    let density = f32_or(vm, id!(density), 1.0).max(0.01);
    let friction = f32_or(vm, id!(friction), 0.6).clamp(0.0, 4.0);
    let restitution = f32_or(vm, id!(restitution), 0.0).clamp(0.0, 1.0);

    let shape_v = opts_value(vm, opts, id!(shape));
    let shape = if shape_v.is_nil() {
        Shape::Box
    } else {
        let name = vm.bx.heap.temp_string_with(|heap, out| {
            heap.cast_to_string(shape_v, out);
            out.to_string()
        });
        Shape::parse(&name)
    };

    let mut world = world.borrow_mut();
    world.mark_render_dirty();
    world.next_id += 1;
    let id = world.next_id;
    world.push_entity(Entity {
        id,
        kind,
        shape,
        pos,
        vel,
        half: vec3f(
            (size.x * 0.5).max(0.01),
            (size.y * 0.5).max(0.01),
            (size.z * 0.5).max(0.01),
        ),
        color,
        tag,
        sensor,
        collide,
        gravity_scale,
        on_floor: false,
        floor_id: 0,
        attached_to: 0,
        attach_offset: vec3f(0.0, 0.0, 0.0),
        attach_ride: false,
        attach_spin: 0.0,
        speed_mult: 1.0,
        life,
        hits,
        hit_wall: 0,
        yaw,
        // Movers face their walk direction like every Godot actor; boxes and
        // platforms hold whatever `face:` gave them.
        auto_face: kind == BodyKind::Mover,
        turn_rate,
        scale: vec3f(1.0, 1.0, 1.0),
        scale_target: vec3f(1.0, 1.0, 1.0),
        glow,
        orient: Quat::default(),
        density,
        friction,
        restitution,
    });
    ScriptValue::from_f64(id as f64)
}

/// One call builds a whole heightfield of column boxes — the corpus built
/// ~960 of these by hand in script. Heights come from a flat row-major
/// script array (index z * cells + x, world-y column tops) or, absent that,
/// from built-in terraced value noise seeded by `seed`. Colors: parallel
/// `colors` array, or `color` auto-shaded darker (low) to lighter (high).
pub(crate) fn spawn_terrain(
    vm: &mut ScriptVm,
    world: &Rc<RefCell<GameWorld>>,
    opts: ScriptObject,
) -> ScriptValue {
    let f32_opt = |vm: &mut ScriptVm, opts: ScriptObject, key: LiveId, default: f32| -> f32 {
        let v = opts_value(vm, opts, key);
        if v.is_nil() {
            default
        } else {
            let ip = vm.bx.threads.cur_ref().trap.ip;
            vm.bx.heap.cast_to_f64(v, ip) as f32
        }
    };
    let span = f32_opt(vm, opts, id!(size), 120.0).max(2.0);
    // 384^2 vertices ≈ 147k — engine-generated, so no script instruction cost;
    // the Godot corpus runs 257x257.
    let cells = f32_opt(vm, opts, id!(cells), 24.0).clamp(2.0, 384.0) as usize;
    let base = f32_opt(vm, opts, id!(base), -12.0);
    let amp = f32_opt(vm, opts, id!(amp), 6.0);
    let seed = f32_opt(vm, opts, id!(seed), 1.0) as u64;
    // Noise shaping (engine-side so big worlds don't burn the eval budget):
    // `freq` lattice frequency per cell, `offset` raises the whole field,
    // `step` terrace size (0 = smooth), `min`/`max` clamp, and `plaza`
    // flattens a disc at the origin with a blend ramp — the corpus layout.
    let noise_freq = f32_opt(vm, opts, id!(freq), 0.18).clamp(0.005, 2.0);
    let noise_offset = f32_opt(vm, opts, id!(offset), 0.0);
    let terrace = f32_opt(vm, opts, id!(step), 1.0).max(0.0);
    let clamp_min = f32_opt(vm, opts, id!(min), f32::MIN);
    let clamp_max = f32_opt(vm, opts, id!(max), f32::MAX);
    let plaza_v = opts_value(vm, opts, id!(plaza));
    let plaza = plaza_v.as_object().map(|p| {
        (
            f32_opt(vm, p, id!(r), 20.0),
            f32_opt(vm, p, id!(ramp), 12.0).max(0.01),
            f32_opt(vm, p, id!(h), 0.0),
        )
    });
    let color_v = opts_value(vm, opts, id!(color));
    let base_color = if color_v.is_nil() {
        vec4(0.36, 0.62, 0.32, 1.0)
    } else {
        value_color(vm, color_v)
    };
    let tag_v = opts_value(vm, opts, id!(tag));
    let tag = if tag_v.is_nil() {
        "terrain".to_string()
    } else {
        vm.bx.heap.temp_string_with(|heap, out| {
            heap.cast_to_string(tag_v, out);
            out.to_string()
        })
    };

    let heights_v = opts_value(vm, opts, id!(heights));
    let colors_v = opts_value(vm, opts, id!(colors));
    let count = cells * cells;

    // Column tops: script array, or terraced value noise.
    let mut tops = Vec::with_capacity(count);
    let heights_len = list_len(vm, heights_v);
    if heights_len > 0 {
        let len = heights_len.min(count);
        let ip = vm.bx.threads.cur_ref().trap.ip;
        for index in 0..count {
            let top = if index < len {
                let v = list_value(vm, heights_v, index);
                if v.is_err() || v.is_nil() { base } else { vm.bx.heap.cast_to_f64(v, ip) as f32 }
            } else {
                base
            };
            tops.push(top);
        }
    } else {
        // Deterministic terraced value noise (xorshift lattice + bilinear).
        let lattice = |x: i64, z: i64| -> f32 {
            let mut h = seed
                .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                .wrapping_add((x as u64).wrapping_mul(0x2545_F491_4F6C_DD1D))
                .wrapping_add((z as u64).wrapping_mul(0x27D4_EB2F_1656_67C5));
            h ^= h >> 33;
            h = h.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
            h ^= h >> 33;
            (h >> 11) as f32 / (1u64 << 53) as f32
        };
        let cell_size = span / (cells - 1).max(1) as f32;
        let origin = -span * 0.5;
        for iz in 0..cells {
            for ix in 0..cells {
                let fx = ix as f32 * noise_freq;
                let fz = iz as f32 * noise_freq;
                let (x0, z0) = (fx.floor() as i64, fz.floor() as i64);
                let (tx, tz) = (fx.fract(), fz.fract());
                let smooth = |t: f32| t * t * (3.0 - 2.0 * t);
                let (sx, sz) = (smooth(tx), smooth(tz));
                let h00 = lattice(x0, z0);
                let h10 = lattice(x0 + 1, z0);
                let h01 = lattice(x0, z0 + 1);
                let h11 = lattice(x0 + 1, z0 + 1);
                let h = h00 + (h10 - h00) * sx + (h01 - h00) * sz
                    + (h00 - h10 - h01 + h11) * sx * sz;
                let mut top = noise_offset + h * amp;
                if let Some((r, ramp, flat_h)) = plaza {
                    let wx = origin + ix as f32 * cell_size;
                    let wz = origin + iz as f32 * cell_size;
                    let d = (wx * wx + wz * wz).sqrt();
                    if d < r {
                        top = flat_h;
                    } else if d < r + ramp {
                        top = flat_h + (top - flat_h) * ((d - r) / ramp);
                    }
                }
                // Terraces: steps a mover can jump up, like the corpus.
                if terrace > 0.0 {
                    top = (top / terrace).floor() * terrace;
                }
                tops.push(top.clamp(clamp_min, clamp_max));
            }
        }
    }

    let (min_top, max_top) = tops.iter().fold((f32::MAX, f32::MIN), |(lo, hi), t| {
        (lo.min(*t), hi.max(*t))
    });
    let colors_len = list_len(vm, colors_v);

    // Height bands: `bands: [{h: 3.6, color: SAND}, ..., {h: 999, color: SNOW}]`
    // — a handful of thresholds instead of a 257x257 colors array. This is how
    // the corpus paints sand/grass/dirt/stone/snowy-mountain terrain.
    let bands_v = opts_value(vm, opts, id!(bands));
    let bands_len = list_len(vm, bands_v);
    let mut bands: Vec<(f32, Vec4f)> = Vec::with_capacity(bands_len);
    for index in 0..bands_len {
        let entry = list_value(vm, bands_v, index);
        if let Some(entry) = entry.as_object() {
            let h_v = opts_value(vm, entry, id!(h));
            let c_v = opts_value(vm, entry, id!(color));
            let ip = vm.bx.threads.cur_ref().trap.ip;
            let h = if h_v.is_nil() { f32::MAX } else { vm.bx.heap.cast_to_f64(h_v, ip) as f32 };
            let c = if c_v.is_nil() { base_color } else { value_color(vm, c_v) };
            bands.push((h, c));
        }
    }
    bands.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    // Per-index colors (bands, script array, or auto-shade valleys darker).
    let mut vertex_colors = Vec::with_capacity(count);
    for index in 0..count {
        let color = if !bands.is_empty() {
            bands
                .iter()
                .find(|(h, _)| tops[index] <= *h)
                .map(|(_, c)| *c)
                .unwrap_or_else(|| bands.last().map(|(_, c)| *c).unwrap_or(base_color))
        } else if colors_len > 0 {
            if index < colors_len {
                let v = list_value(vm, colors_v, index);
                if v.is_err() || v.is_nil() { base_color } else { value_color(vm, v) }
            } else {
                base_color
            }
        } else {
            let t = if max_top > min_top { (tops[index] - min_top) / (max_top - min_top) } else { 0.5 };
            let shade = 0.75 + t * 0.4;
            vec4(
                (base_color.x * shade).min(1.0),
                (base_color.y * shade).min(1.0),
                (base_color.z * shade).min(1.0),
                base_color.w,
            )
        };
        vertex_colors.push(color);
    }

    let smooth = opts_value(vm, opts, id!(smooth)).as_bool().unwrap_or(false);
    let water_v = opts_value(vm, opts, id!(water));

    if smooth {
        // Smooth mode: the same heights array becomes VERTEX heights of one
        // triangulated ground mesh (cells = vertices per side) with height
        // lookups for collision — no column entities at all.
        let cell_size = span / (cells - 1).max(1) as f32;
        let mut world = world.borrow_mut();
        let revision = world.terrain.as_ref().map_or(1, |t| t.revision + 1);
        world.terrain = Some(Terrain {
            cells,
            cell_size,
            origin: -span * 0.5,
            heights: tops,
            colors: vertex_colors,
            revision,
        });
        if !water_v.is_nil() {
            let ip = vm.bx.threads.cur_ref().trap.ip;
            let level = vm.bx.heap.cast_to_f64(water_v, ip) as f32;
            world.next_id += 1;
            let id = world.next_id;
            // One translucent sensor slab: gameplay touch + the water look.
            world.push_entity(Entity {
                id,
                kind: BodyKind::Static,
                pos: vec3f(0.0, level - 0.05, 0.0),
                vel: vec3f(0.0, 0.0, 0.0),
                half: vec3f(span * 0.5, 0.05, span * 0.5),
                color: vec4(0.25, 0.55, 0.85, 0.6),
                tag: "water".to_string(),
                sensor: true,
                collide: true,
                gravity_scale: 1.0,
                on_floor: false,
                floor_id: 0,
                attached_to: 0,
                attach_offset: vec3f(0.0, 0.0, 0.0),
                attach_ride: false,
                attach_spin: 0.0,
                speed_mult: 1.0,
                life: 0.0,
                hits: false,
                hit_wall: 0,
                yaw: 0.0,
                auto_face: false,
                turn_rate: 7.0,
                scale: vec3f(1.0, 1.0, 1.0),
                scale_target: vec3f(1.0, 1.0, 1.0),
                glow: 0.0,
                shape: Shape::Box,
                            orient: Quat::default(),
                density: 1.0,
                friction: 0.6,
                restitution: 0.0,
            });
        }
        return ScriptValue::from_f64(count as f64);
    }

    let cell_size = span / cells as f32;
    let mut spawned = 0usize;
    for iz in 0..cells {
        for ix in 0..cells {
            let index = iz * cells + ix;
            let top = tops[index].max(base + 0.05);
            let color = vertex_colors[index];
            let x = (ix as f32 + 0.5) * cell_size - span * 0.5;
            let z = (iz as f32 + 0.5) * cell_size - span * 0.5;
            let mut world = world.borrow_mut();
            world.next_id += 1;
            let id = world.next_id;
            world.push_entity(Entity {
                id,
                kind: BodyKind::Static,
                pos: vec3f(x, (base + top) * 0.5, z),
                vel: vec3f(0.0, 0.0, 0.0),
                half: vec3f(cell_size * 0.5, ((top - base) * 0.5).max(0.05), cell_size * 0.5),
                color,
                tag: tag.clone(),
                sensor: false,
                collide: true,
                gravity_scale: 1.0,
                on_floor: false,
                floor_id: 0,
                attached_to: 0,
                attach_offset: vec3f(0.0, 0.0, 0.0),
                attach_ride: false,
                attach_spin: 0.0,
                speed_mult: 1.0,
                life: 0.0,
                hits: false,
                hit_wall: 0,
                yaw: 0.0,
                auto_face: false,
                turn_rate: 7.0,
                scale: vec3f(1.0, 1.0, 1.0),
                scale_target: vec3f(1.0, 1.0, 1.0),
                glow: 0.0,
                shape: Shape::Box,
                            orient: Quat::default(),
                density: 1.0,
                friction: 0.6,
                restitution: 0.0,
            });
            spawned += 1;
        }
    }
    ScriptValue::from_f64(spawned as f64)
}

