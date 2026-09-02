//! Water volumes with an analytic Gerstner-sum surface + hull-probe buoyancy
//! (mix.md D7 / §5.7, tasks W1-W2).
//!
//! The wave sum is a PURE function of `(x, z, tick)` evaluated through
//! [`makepad_game_math`] sin/cos — bit-deterministic on every machine, zero
//! simulation cost. This CPU-side sum is the ONLY physics truth: the render
//! shader displaces the visible sheet with the SAME sum (same coefficients,
//! same expression — see `game_render::shaders`' water shader and its pin
//! test), but the shader is visual-only and physics never reads the GPU.
//!
//! Scope wall (D7): no grid or particle fluid sim, ever. A volume is a box
//! of still water plus a list of analytic wave components; everything else
//! (buoyancy, drag, orbital push, swim, surf) derives from the sum.
//!
//! ## The canonical wave expression
//!
//! For each wave `w` at world position `(x, z)` and tick `t = tick·TICK_DT`:
//!
//! ```text
//! phase = w.k·(w.dir_x·x + w.dir_z·z) − w.omega·t + w.phase
//! env   = 1                          when w.group <= 0   (steady wave)
//!       = e²,  e = 0.5 + 0.5·cos(phase / w.group)        (set-wave train)
//! height  += w.amp · env · sin(phase)
//! ∂h/∂x   += w.amp · env · cos(phase) · w.k · w.dir_x
//! ∂h/∂z   += w.amp · env · cos(phase) · w.k · w.dir_z
//! orbital_xz += dir · w.amp · env · w.omega · sin(phase)
//! orbital_y  += −w.amp · env · w.omega · cos(phase)
//! ```
//!
//! The envelope's own derivative is deliberately omitted from the slope and
//! orbital terms — it varies `group`× slower than the carrier — and the SAME
//! omission is made in the shader, so CPU and GPU agree by construction.
//! `orbital_y` is exactly `∂h/∂t` (the linearized kinematic surface
//! condition); `orbital_xz` is the deep-water Airy surface velocity, in phase
//! with the elevation, which is what makes a crest push things forward.
//!
//! The set-wave envelope rides the carrier's own phase, so a "set" travels
//! shoreward at the wave's phase speed and a big crest arrives every
//! `group · (wave period)` seconds — `game.surf_spot`'s repeating train.
//!
//! ## Units (D1-clean)
//!
//! Buoyant force = displaced volume × `world.gravity` × water density —
//! derived from the LIVE gravity every tick, never a constant, so the 9.81
//! flip changes the float height math not at all (density ratios decide) and
//! the wave DEFAULTS (deep-water dispersion c = √(g·λ/2π), used by the verbs
//! when no speed is given) pick up the new gravity at declaration time.
//! Water density lives in the same unit as entity `density` (the box3d shape
//! density), so float-or-sink is literally `entity.density < volume.density`.

use makepad_math::*;

use crate::entity::{BodyKind, Entity};
use crate::TICK_DT;

use makepad_box3d::body::{
    body_get_mass, body_get_world_point_velocity, body_apply_force,
};
use makepad_box3d::math_functions::{pos as b3pos, vec3 as b3vec3};

/// One analytic wave component. All fields are the RAW coefficients the
/// canonical expression consumes — the render path uploads them unmodified
/// (no unit conversion that could drift between CPU and GPU).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaterWave {
    /// Unit travel direction on the ground plane.
    pub dir_x: f32,
    pub dir_z: f32,
    /// Amplitude, world units (crest = level + amp for a steady wave).
    pub amp: f32,
    /// Spatial frequency k = τ / wavelength.
    pub k: f32,
    /// Temporal frequency ω = k · phase-speed.
    pub omega: f32,
    /// Phase offset, radians.
    pub phase: f32,
    /// 0 = steady. ≥ 1 = set-wave train: the envelope `(0.5+0.5·cos(phase/
    /// group))²` makes every `group`-th crest the big one (surf sets).
    pub group: f32,
}

impl WaterWave {
    /// Build from the authorable quantities: direction (normalized here),
    /// amplitude, wavelength and phase speed.
    pub fn new(dir_x: f32, dir_z: f32, amp: f32, len: f32, speed: f32) -> Self {
        let l = crate::math::hypot(dir_x, dir_z);
        let (dx, dz) = if l > 1.0e-6 {
            (dir_x / l, dir_z / l)
        } else {
            (1.0, 0.0)
        };
        let k = std::f32::consts::TAU / len.max(0.05);
        Self {
            dir_x: dx,
            dir_z: dz,
            amp: amp.max(0.0),
            k,
            omega: k * speed.max(0.0),
            phase: 0.0,
            group: 0.0,
        }
    }
}

/// Deep-water dispersion phase speed c = √(g·λ/2π) — the default the verbs
/// use when no explicit speed is given. Derives from LIVE gravity (D1):
/// flipping the world to 9.81 retunes default wave speeds at declaration.
pub fn dispersion_speed(gravity: f32, wavelength: f32) -> f32 {
    crate::math::sqrt((gravity.max(0.0) * wavelength.max(0.05)) / std::f32::consts::TAU)
}

/// Wave slots the render sheet mirrors per volume. The sim accepts no more
/// either (the verbs warn and drop extras), so physics and visuals can never
/// disagree about which waves exist.
pub const MAX_WAVES: usize = 8;

/// One axis-aligned water region. The still-water surface sits at `max.y`;
/// waves displace around it.
#[derive(Clone, Debug)]
pub struct WaterVolume {
    pub min: Vec3f,
    pub max: Vec3f,
    /// Water density, in the entity-`density` unit. 1.0 = the default that
    /// makes a default crate neutrally buoyant.
    pub density: f32,
    /// Constant advection velocity (rivers, rip currents). y is ignored.
    pub current: Vec3f,
    pub waves: Vec<WaterWave>,
    /// Render color of the sheet (alpha = translucency).
    pub color: Vec4f,
    /// The hidden sensor slab spawned alongside (touch reports keep the old
    /// `on_touch` water contract). 0 = none.
    pub entity: u64,
    /// Does the renderer draw this volume's sheet? False makes the volume
    /// PHYSICS ONLY — buoyancy, swimming and the touch sensor stay, and
    /// something else draws the surface (a river's channel-following ribbon).
    ///
    /// A "fully transparent" sheet is NOT a substitute: the scene blends
    /// premultiplied (source factor ONE, destination ONE_MINUS_SRC_ALPHA), so
    /// an alpha-0 sheet does not vanish — it ADDS its colour to whatever is
    /// behind it. That is exactly how a river's axis-aligned physics boxes
    /// came out as bright stepped rectangles over the meadow.
    pub draw_sheet: bool,
}

impl WaterVolume {
    /// Still-water surface height.
    pub fn level(&self) -> f32 {
        self.max.y
    }

    /// Sum of amplitudes — the headroom used for "is this point plausibly
    /// under the surface" checks before paying for the exact sum.
    pub fn amp_sum(&self) -> f32 {
        self.waves.iter().map(|w| w.amp).sum()
    }

    /// Does the volume own this ground-plane position?
    pub fn contains_xz(&self, x: f32, z: f32) -> bool {
        x >= self.min.x && x <= self.max.x && z >= self.min.z && z <= self.max.z
    }

    /// The canonical wave sum: surface height at (x, z, tick). Level plus
    /// every component's displacement. Pure function of its arguments through
    /// game_math — THE determinism-critical path.
    pub fn surface_height(&self, x: f32, z: f32, tick: u64) -> f32 {
        let t = tick_time(tick);
        let mut h = self.level();
        for w in &self.waves {
            let (env, s, _c) = wave_terms(w, x, z, t);
            h += w.amp * env * s;
        }
        h
    }

    /// Analytic surface normal at (x, z, tick), unit length, y-up.
    pub fn surface_normal(&self, x: f32, z: f32, tick: u64) -> Vec3f {
        let t = tick_time(tick);
        let mut dhx = 0.0f32;
        let mut dhz = 0.0f32;
        for w in &self.waves {
            let (env, _s, c) = wave_terms(w, x, z, t);
            let slope = w.amp * env * c * w.k;
            dhx += slope * w.dir_x;
            dhz += slope * w.dir_z;
        }
        let n = vec3f(-dhx, 1.0, -dhz);
        let len = crate::math::sqrt(n.x * n.x + n.y * n.y + n.z * n.z);
        n * (1.0 / len)
    }

    /// Water particle velocity at the surface (orbital motion + current):
    /// horizontal in phase with the elevation (a crest pushes forward),
    /// vertical = ∂h/∂t.
    pub fn orbital_velocity(&self, x: f32, z: f32, tick: u64) -> Vec3f {
        let t = tick_time(tick);
        let mut v = vec3f(self.current.x, 0.0, self.current.z);
        for w in &self.waves {
            let (env, s, c) = wave_terms(w, x, z, t);
            let u = w.amp * env * w.omega;
            v.x += u * s * w.dir_x;
            v.z += u * s * w.dir_z;
            v.y -= u * c;
        }
        v
    }
}

/// Wave-sum time for a tick. f32 on purpose: the shader receives the same
/// f32, so both sides reduce identically. Accuracy (not determinism) of the
/// sum degrades as ω·t grows — days of continuous runtime; documented.
#[inline]
pub fn tick_time(tick: u64) -> f32 {
    tick as f32 * TICK_DT
}

/// The shared per-wave kernel: envelope, sin(phase), cos(phase).
#[inline]
fn wave_terms(w: &WaterWave, x: f32, z: f32, t: f32) -> (f32, f32, f32) {
    let phase = w.k * (w.dir_x * x + w.dir_z * z) - w.omega * t + w.phase;
    let (s, c) = crate::math::sincos(phase);
    let env = if w.group > 0.0 {
        let e = 0.5 + 0.5 * crate::math::cos(phase / w.group);
        e * e
    } else {
        1.0
    };
    (env, s, c)
}

/// Buoyancy + drag applied to one entity this tick — the boat's telemetry
/// feed (R7 pattern: model forces, not finite differences).
#[derive(Clone, Copy, Debug, Default)]
pub struct BuoyancyApplied {
    pub entity: u64,
    /// Sum of every water force applied this tick (buoyancy + drag), world
    /// frame, force units.
    pub force: Vec3f,
    /// Per-probe upward buoyant load (the boat's "wheel loads").
    pub probe_load: [f32; 4],
    /// Mean submersion across probes, 0..1.
    pub submerged: f32,
}

/// All water in one world. Lives on [`crate::world::GameWorld`] as
/// `Option<Box<WaterState>>` — a world without the verb carries a null
/// pointer and every pre-water code path stays byte-identical.
#[derive(Clone, Debug, Default)]
pub struct WaterState {
    /// Declaration order — the FIXED iteration order for every query and the
    /// per-tick force pass (determinism rule).
    pub volumes: Vec<WaterVolume>,
    /// Bumped on any volume/wave change; the renderer rebuilds sheets on it.
    pub rev: u64,
    /// Rebuilt by [`apply_buoyancy`] every tick, entity-id order. Blocks read
    /// it in `post_step` for telemetry. Cleared when nothing is in water.
    pub applied: Vec<BuoyancyApplied>,
}

impl WaterState {
    /// First volume (declaration order) owning this position: inside the xz
    /// bounds, above the floor of the box, below surface + wave headroom.
    pub fn volume_at(&self, pos: Vec3f) -> Option<usize> {
        self.volumes.iter().position(|v| {
            v.contains_xz(pos.x, pos.z)
                && pos.y >= v.min.y
                && pos.y <= v.level() + v.amp_sum()
        })
    }

    /// Depth of `pos` below the wave surface of the volume that owns it
    /// (positive = submerged). None when no volume owns the position.
    pub fn depth_at(&self, pos: Vec3f, tick: u64) -> Option<(usize, f32)> {
        let at = self.volume_at(pos)?;
        let h = self.volumes[at].surface_height(pos.x, pos.z, tick);
        Some((at, h - pos.y))
    }

    /// What buoyancy applied to `entity` this tick, if it touched water.
    pub fn applied_to(&self, entity: u64) -> Option<&BuoyancyApplied> {
        self.applied.iter().find(|a| a.entity == entity)
    }
}

// ---------------------------------------------------------------- buoyancy

/// Hull probes: bottom corners of the entity's local box, spread inside the
/// footprint. Four probes — the truck-suspension shape — give heave, pitch
/// AND roll response from one mechanism.
pub const PROBE_SPREAD: f32 = 0.7;
/// Linear drag rate against the displaced volume (viscous settle term).
pub const DRAG_LINEAR: f32 = 0.8;
/// Quadratic drag coefficient, applied per BODY AXIS against the projected
/// area normal to that axis — which is what makes drag anisotropic with no
/// tuning knob: a long hull slips along its length (small bow area) and
/// resists sideways/vertical motion (big flank/bottom areas), and a cube is
/// simply isotropic. This is the whole difference between a boat that
/// planes and one that pushes a barn door.
pub const DRAG_QUAD: f32 = 0.25;

/// The dynamics pre-step force pass (W2): every RIGID entity with a probe in
/// water gets buoyancy + drag + wave orbital push, applied straight into its
/// box3d body — the same "forces in, poses out" contract the car suspension
/// uses. Runs inside `step_world` after the mirror reconcile (so bodies exist
/// even on their spawn tick) and before the solver step, so the forces land
/// in this tick's integration.
///
/// Movers are DELIBERATELY excluded — they get swim handling in the
/// character block (W4), never probe forces. A world with no submerged rigid
/// performs zero box3d calls here, which is what keeps every no-water golden
/// bit-identical.
pub fn apply_buoyancy(
    dynamics: &mut crate::dynamics::RigidDynamics,
    entities: &[Entity],
    water: &mut WaterState,
    gravity: f32,
    tick: u64,
) {
    water.applied.clear();
    if water.volumes.is_empty() {
        return;
    }
    for e in entities {
        if e.kind != BodyKind::Rigid || e.sensor || !e.collide {
            continue;
        }
        // Cheap reject: entity box vs any volume (xz + y headroom).
        let near = water.volumes.iter().any(|v| {
            e.pos.x + e.half.x >= v.min.x
                && e.pos.x - e.half.x <= v.max.x
                && e.pos.z + e.half.z >= v.min.z
                && e.pos.z - e.half.z <= v.max.z
                && e.pos.y - e.half.y <= v.level() + v.amp_sum()
                && e.pos.y + e.half.y >= v.min.y
        });
        if !near {
            continue;
        }
        let Some(body) = dynamics.rigid_body_of(e.id) else {
            continue;
        };
        let mass = body_get_mass(&dynamics.world, body).max(1.0e-3);
        let volume = 8.0 * e.half.x * e.half.y * e.half.z;
        let draft = (2.0 * e.half.y).max(0.01);
        // Body axes for the anisotropic drag, and the projected area normal
        // to each (per-probe quarter shares).
        let fwd = rotate_q(e.orient, vec3f(0.0, 0.0, -1.0));
        let right = rotate_q(e.orient, vec3f(1.0, 0.0, 0.0));
        let up_axis = rotate_q(e.orient, vec3f(0.0, 1.0, 0.0));
        let area_fwd = 4.0 * e.half.x * e.half.y * 0.25;
        let area_right = 4.0 * e.half.z * e.half.y * 0.25;
        let area_up = 4.0 * e.half.x * e.half.z * 0.25;

        // Bottom-corner probes in the body frame, rotated by the LIVE
        // orientation (read back last tick) — a heeling hull lifts its
        // windward probes out of the water, which is the righting moment.
        let corners = [
            (-PROBE_SPREAD, -PROBE_SPREAD),
            (PROBE_SPREAD, -PROBE_SPREAD),
            (-PROBE_SPREAD, PROBE_SPREAD),
            (PROBE_SPREAD, PROBE_SPREAD),
        ];
        let mut rec = BuoyancyApplied {
            entity: e.id,
            ..Default::default()
        };
        let mut touched = false;
        for (i, (sx, sz)) in corners.iter().enumerate() {
            let local = vec3f(sx * e.half.x, -e.half.y, sz * e.half.z);
            let p = e.pos + rotate_q(e.orient, local);
            let Some(at) = water.volume_at(p) else {
                continue;
            };
            let vol = &water.volumes[at];
            let surface = vol.surface_height(p.x, p.z, tick);
            let depth = surface - p.y;
            if depth <= 0.0 {
                continue;
            }
            let sub = (depth / draft).min(1.0);
            // Archimedes, per probe: ρ_w · g · (V/4) · submerged share.
            // Fully submerged (all four at 1) sums to ρ_w·g·V exactly, so
            // float-or-sink is the density ratio and nothing else.
            let lift = vol.density * gravity * (volume * 0.25) * sub;
            let mut force = vec3f(0.0, lift, 0.0);

            // Drag toward the water's own motion — the orbital term IS the
            // wave push (a crest drags the hull shoreward through this).
            // Quadratic term per body axis against that axis' projected
            // area: anisotropic streamlining for free (see DRAG_QUAD).
            let pv = body_get_world_point_velocity(&dynamics.world, body, b3pos(p.x, p.y, p.z));
            let water_v = vol.orbital_velocity(p.x, p.z, tick);
            let rel = vec3f(pv.x - water_v.x, pv.y - water_v.y, pv.z - water_v.z);
            let speed = crate::vec3_len(rel);
            let lin = DRAG_LINEAR * vol.density * (volume * 0.25);
            let qs = DRAG_QUAD * vol.density * speed;
            let quad_v = fwd * (rel.dot(fwd) * qs * area_fwd)
                + right * (rel.dot(right) * qs * area_right)
                + up_axis * (rel.dot(up_axis) * qs * area_up);
            let mut drag = (rel * lin + quad_v) * (-sub);
            // Never let one tick's drag reverse the relative velocity: cap
            // the impulse at half of what would null it (per-probe mass
            // share). Keeps thin fast boards stable at 60 Hz.
            let cap = 0.5 * speed * (mass * 0.25) / TICK_DT;
            let dl = crate::vec3_len(drag);
            if dl > cap && dl > 1.0e-9 {
                drag = drag * (cap / dl);
            }
            force = force + drag;
            if force.x.is_finite() && force.y.is_finite() && force.z.is_finite() {
                body_apply_force(
                    &mut dynamics.world,
                    body,
                    b3vec3(force.x, force.y, force.z),
                    b3pos(p.x, p.y, p.z),
                    true,
                );
                rec.force = rec.force + force;
                rec.probe_load[i] = lift;
                rec.submerged += sub * 0.25;
                touched = true;
            }
        }
        if touched {
            water.applied.push(rec);
        }
    }
}

/// Quaternion rotate without transcendentals (shared shape with the blocks'
/// `rotate`, local so sim has no blocks dependency).
#[inline]
fn rotate_q(q: Quat, v: Vec3f) -> Vec3f {
    let u = vec3f(q.x, q.y, q.z);
    let s = q.w;
    u * (2.0 * u.dot(v)) + v * (s * s - u.dot(u)) + Vec3f::cross(u, v) * (2.0 * s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::GameWorld;

    fn test_volume() -> WaterVolume {
        let mut a = WaterWave::new(1.0, 0.25, 0.6, 14.0, 3.5);
        a.phase = 0.4;
        let mut b = WaterWave::new(-0.3, 1.0, 0.25, 6.0, 2.0);
        b.phase = 1.7;
        b.group = 5.0;
        WaterVolume {
            min: vec3f(-40.0, -6.0, -40.0),
            max: vec3f(40.0, 2.0, 40.0),
            density: 1.0,
            current: vec3f(0.4, 0.0, 0.0),
            waves: vec![a, b],
            color: vec4(0.25, 0.55, 0.85, 0.6),
            entity: 0,
            draw_sheet: true,
        }
    }

    /// THE determinism gate (W1): the wave sum is a pure function of
    /// (x, z, tick). Two evaluations agree bit-exactly, and a pinned golden
    /// (recorded on aarch64) must reproduce on every machine — that equality
    /// IS the cross-arch claim, exactly like game_math's parity hashes.
    #[test]
    fn wave_sum_is_deterministic_and_matches_golden() {
        let v = test_volume();
        let (x, z, tick) = (3.25f32, -7.5f32, 12345u64);
        let a = v.surface_height(x, z, tick);
        let b = v.surface_height(x, z, tick);
        assert_eq!(a.to_bits(), b.to_bits(), "same inputs, different bits");
        // Recorded from this exact volume/coefficients; a kernel or
        // expression-order change moves it and must be deliberate.
        assert_eq!(
            a.to_bits(),
            0x3fcdb348,
            "pinned wave-sum golden moved: {a} ({:#010x})",
            a.to_bits()
        );
        // The normal and orbital derive from the same kernel: pin their
        // determinism too (bit-equality across evaluations).
        let n1 = v.surface_normal(x, z, tick);
        let n2 = v.surface_normal(x, z, tick);
        assert_eq!(n1.x.to_bits(), n2.x.to_bits());
        assert_eq!(n1.y.to_bits(), n2.y.to_bits());
        let o1 = v.orbital_velocity(x, z, tick);
        let o2 = v.orbital_velocity(x, z, tick);
        assert_eq!(o1.x.to_bits(), o2.x.to_bits());
        assert_eq!(o1.y.to_bits(), o2.y.to_bits());
    }

    #[test]
    fn still_water_is_flat_level_with_up_normal_and_current_only() {
        let mut v = test_volume();
        v.waves.clear();
        assert_eq!(v.surface_height(5.0, 5.0, 999), v.level());
        let n = v.surface_normal(1.0, 2.0, 3);
        assert!((n.y - 1.0).abs() < 1.0e-6);
        let o = v.orbital_velocity(0.0, 0.0, 77);
        assert_eq!(o.x, 0.4);
        assert_eq!(o.y, 0.0);
    }

    #[test]
    fn set_wave_group_pulses_the_amplitude() {
        // A grouped wave's crest heights vary across a set period; a steady
        // wave's do not. Sample crest-to-crest peaks over one group.
        let mut v = test_volume();
        v.waves.truncate(1);
        v.waves[0].group = 4.0;
        let mut min_peak = f32::MAX;
        let mut max_peak = f32::MIN;
        // Track the running max over each carrier period at a fixed point.
        let period_ticks = (std::f32::consts::TAU / v.waves[0].omega / TICK_DT) as u64;
        for g in 0..4u64 {
            let mut peak = f32::MIN;
            for t in 0..period_ticks {
                let h = v.surface_height(0.0, 0.0, g * period_ticks + t) - v.level();
                peak = peak.max(h);
            }
            min_peak = min_peak.min(peak);
            max_peak = max_peak.max(peak);
        }
        assert!(
            max_peak > min_peak * 2.0 + 0.05,
            "set envelope did not pulse: peaks {min_peak}..{max_peak}"
        );
    }

    #[test]
    fn dispersion_speed_derives_from_live_gravity() {
        let arcade = dispersion_speed(30.0, 20.0);
        let real = dispersion_speed(9.81, 20.0);
        assert!(arcade > real, "higher g = faster deep-water waves");
        assert!((arcade - crate::math::sqrt(30.0 * 20.0 / std::f32::consts::TAU)).abs() < 1.0e-6);
    }

    // ---- buoyancy (W2) ---------------------------------------------------

    fn world_with_pool() -> GameWorld {
        let mut w = GameWorld::new();
        // Seabed, so sinkers have somewhere to rest.
        w.next_id += 1;
        let id = w.next_id;
        w.push_entity(Entity {
            id,
            kind: BodyKind::Static,
            pos: vec3f(0.0, -6.5, 0.0),
            half: vec3f(50.0, 0.5, 50.0),
            collide: true,
            push_mass: 1.0,
            speed_mult: 1.0,
            scale: vec3f(1.0, 1.0, 1.0),
            scale_target: vec3f(1.0, 1.0, 1.0),
            density: 1.0,
            friction: 0.6,
            tag: "bed".into(),
            ..Default::default()
        });
        w.water = Some(Box::new(WaterState {
            volumes: vec![WaterVolume {
                min: vec3f(-50.0, -6.0, -50.0),
                max: vec3f(50.0, 0.0, 50.0),
                density: 1.0,
                current: vec3f(0.0, 0.0, 0.0),
                waves: Vec::new(),
                color: vec4(0.25, 0.55, 0.85, 0.6),
                entity: 0,
                draw_sheet: true,
            }],
            rev: 1,
            applied: Vec::new(),
        }));
        w
    }

    fn drop_crate(w: &mut GameWorld, x: f32, density: f32) -> u64 {
        w.next_id += 1;
        let id = w.next_id;
        w.push_entity(Entity {
            id,
            kind: BodyKind::Rigid,
            pos: vec3f(x, 1.5, 0.0),
            half: vec3f(0.5, 0.5, 0.5),
            collide: true,
            gravity_scale: 1.0,
            push_mass: 1.0,
            speed_mult: 1.0,
            scale: vec3f(1.0, 1.0, 1.0),
            scale_target: vec3f(1.0, 1.0, 1.0),
            density,
            friction: 0.5,
            tag: "crate".into(),
            ..Default::default()
        });
        id
    }

    /// W2's gate: float-or-sink emerges from density alone.
    #[test]
    fn float_or_sink_is_decided_by_density() {
        let mut w = world_with_pool();
        let cork = drop_crate(&mut w, -5.0, 0.3);
        let rock = drop_crate(&mut w, 5.0, 2.5);
        for _ in 0..600 {
            crate::step::step_world(&mut w);
            // The HOST owns the tick counter; mirror its contract or the
            // wave field freezes at t = 0.
            w.tick += 1;
        }
        let cork_y = w.entity(cork).unwrap().pos.y;
        let rock_y = w.entity(rock).unwrap().pos.y;
        // The cork floats near the surface: equilibrium draft = density
        // ratio × height → centre at level − h·(ratio − 0.5) = +0.2.
        assert!(
            (cork_y - 0.2).abs() < 0.25,
            "cork settled at {cork_y}, expected ≈ 0.2 (30% draft)"
        );
        // The rock is on the seabed (bed top −6, half 0.5 → centre −5.5).
        assert!(
            rock_y < -5.0,
            "rock at {rock_y} did not sink to the seabed"
        );
        // Neutral-ish check: buoyancy recorded forces for both while wet.
        assert!(w.water.as_ref().unwrap().applied_to(cork).is_some());
    }

    /// The orbital coupling is real and bounded: waves visibly SURGE a
    /// floater back and forth (nonzero horizontal motion where still water
    /// gives none) without flinging it anywhere. The NET drift of a free
    /// floater is second-order small and its sign genuinely flips with the
    /// heave transfer function (a stiff floater overshoots the surface and
    /// picks up a slight upwave push — textbook, not a bug), so no test
    /// pins it; the gameplay-strength push is the surf block's slope force,
    /// gated in blocks/tests/water.rs (`a_surfboard_rides_a_set_wave_
    /// shoreward`). An early version of THIS test asserted a downwave drift
    /// and only ever measured the tick counter not advancing — see the
    /// host-tick note in float_or_sink.
    #[test]
    fn waves_surge_a_floater_without_flinging_it() {
        let mut w = world_with_pool();
        {
            let water = w.water.as_mut().unwrap();
            water.volumes[0].waves = vec![WaterWave::new(1.0, 0.0, 0.5, 12.0, 4.0)];
            water.rev += 1;
        }
        let cork = drop_crate(&mut w, 0.0, 0.3);
        let mut max_surge = 0.0f32;
        for _ in 0..900 {
            crate::step::step_world(&mut w);
            w.tick += 1;
            max_surge = max_surge.max(w.entity(cork).unwrap().vel.x.abs());
        }
        assert!(
            max_surge > 0.3,
            "waves never surged the floater (peak |vx| = {max_surge})"
        );
        let x = w.entity(cork).unwrap().pos.x.abs();
        assert!(x < 5.0, "free floater was flung to |x|={x} by drag rectification");
    }

    /// The no-water golden gate: a distant volume must not move one bit of
    /// an untouched simulation. The pass performs zero box3d calls for dry
    /// bodies, so the solver's call sequence is byte-identical.
    #[test]
    fn a_dry_world_is_bit_identical_with_and_without_water() {
        let build = |with_water: bool| -> GameWorld {
            let mut w = GameWorld::new();
            w.next_id += 1;
            let ground = w.next_id;
            w.push_entity(Entity {
                id: ground,
                kind: BodyKind::Static,
                pos: vec3f(0.0, -0.5, 0.0),
                half: vec3f(30.0, 0.5, 30.0),
                collide: true,
                push_mass: 1.0,
                speed_mult: 1.0,
                scale: vec3f(1.0, 1.0, 1.0),
                scale_target: vec3f(1.0, 1.0, 1.0),
                density: 1.0,
                friction: 0.6,
                ..Default::default()
            });
            let _ = drop_crate(&mut w, 0.0, 1.0);
            if with_water {
                w.water = Some(Box::new(WaterState {
                    volumes: vec![WaterVolume {
                        min: vec3f(200.0, -5.0, 200.0),
                        max: vec3f(260.0, 210.0, 260.0),
                        density: 1.0,
                        current: vec3f(0.0, 0.0, 0.0),
                        waves: vec![WaterWave::new(1.0, 0.0, 0.5, 10.0, 3.0)],
                        color: vec4(0.25, 0.55, 0.85, 0.6),
                        entity: 0,
                        draw_sheet: true,
                    }],
                    rev: 1,
                    applied: Vec::new(),
                }));
            }
            w
        };
        let mut dry = build(false);
        let mut wet = build(true);
        for _ in 0..240 {
            crate::step::step_world(&mut dry);
            dry.tick += 1;
            crate::step::step_world(&mut wet);
            wet.tick += 1;
        }
        for (a, b) in dry.entities.iter().zip(wet.entities.iter()) {
            assert_eq!(a.pos.x.to_bits(), b.pos.x.to_bits(), "entity {}", a.id);
            assert_eq!(a.pos.y.to_bits(), b.pos.y.to_bits(), "entity {}", a.id);
            assert_eq!(a.pos.z.to_bits(), b.pos.z.to_bits(), "entity {}", a.id);
            assert_eq!(a.vel.x.to_bits(), b.vel.x.to_bits(), "entity {}", a.id);
            assert_eq!(a.vel.y.to_bits(), b.vel.y.to_bits(), "entity {}", a.id);
            assert_eq!(a.vel.z.to_bits(), b.vel.z.to_bits(), "entity {}", a.id);
        }
    }

    /// Clone carries the full water state (worlds snapshot by value).
    #[test]
    fn water_state_survives_a_world_clone() {
        let mut w = world_with_pool();
        let cork = drop_crate(&mut w, 0.0, 0.3);
        for _ in 0..120 {
            crate::step::step_world(&mut w);
            w.tick += 1;
        }
        let snap = w.clone();
        let a = snap.water.as_ref().unwrap();
        let b = w.water.as_ref().unwrap();
        assert_eq!(a.volumes.len(), b.volumes.len());
        assert_eq!(a.rev, b.rev);
        assert_eq!(
            a.applied_to(cork).map(|r| r.force.y.to_bits()),
            b.applied_to(cork).map(|r| r.force.y.to_bits())
        );
    }
}
