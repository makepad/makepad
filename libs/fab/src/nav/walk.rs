//! Lane C. First-person navigation: **fly** (free 6-DOF) and **walk**
//! (eye height locked to the floor under you, gliding over slabs and stairs).
//!
//! Capture follows `apps/sandbox/src/sandbox_view.rs` — click to take the
//! pointer (cursor hidden, OS pointer locked/warped), `lock_delta` is the
//! real motion while it is ours, and **Escape / right-click always give it
//! back**. Window blur (and the viewport's `HoverOut`) also release the
//! pointer so a hidden grab cannot outlive the focused window.
//! `FAB_NO_MOUSE_LOCK=1` keeps look-by-absolute-motion working for scripted
//! sessions without ever taking the OS pointer.
//!
//! The floor and the walls are the scene's own BVH: one ray down per frame
//! for the floor, six rays (knee + chest × centre ± 0.25 m) along each move
//! for a 0.3 m capsule. No second copy of the geometry. Walk applies gravity
//! when nothing is below; fly ignores floors.

use crate::api::*;
use crate::nav::orbit::WORLD_UP;
use makepad_widgets::*;
use std::sync::atomic::{AtomicU64, Ordering};

pub const K_FWD: u16 = 1 << 0;
pub const K_BACK: u16 = 1 << 1;
pub const K_LEFT: u16 = 1 << 2;
pub const K_RIGHT: u16 = 1 << 3;
pub const K_UP: u16 = 1 << 4;
pub const K_DOWN: u16 = 1 << 5;
pub const K_RUN: u16 = 1 << 6;
pub const K_SLOW: u16 = 1 << 7;
pub const K_JUMP: u16 = 1 << 8;

/// Standing eye height, metres. A person's (not the architectural storey).
pub const EYE_HEIGHT: f32 = 1.62;
/// Physical walk speed, metres per second. Independent of model size.
pub const WALK_SPEED: f32 = 4.2;
/// Shift-run speed in walk, metres per second. Fly run stays [`RUN_MULTIPLIER`].
pub const RUN_SPEED: f32 = 9.0;
/// Walk wheel clamps the physical speed to this range, metres per second.
pub const WALK_SPEED_MIN: f32 = 2.0;
pub const WALK_SPEED_MAX: f32 = 8.0;
/// Tallest step we glide up; anything higher is a wall. A real stair riser
/// is ≈ 0.18 m; 0.25 sits between that and a 0.4 m plinth.
pub const STEP_UP: f32 = 0.25;
/// Falling longer than this with no floor in reach puts the walker back on
/// the last floor it stood on (about 20 m of free fall): stepping off the
/// edge of the site must never be an endless fall.
pub const FALL_RESPAWN_S: f32 = 2.0;
/// How far below the probe a floor still counts while falling.
pub const FALL_MAX: f32 = 60.0;
/// Radians of look per layout point at default sensitivity.
/// A 100-point flick is 30° of yaw (`100.0 * LOOK_SENS`).
pub const LOOK_SENS: f32 = (30.0f32).to_radians() / 100.0;
/// Pitch clamp: ±89°, no roll (world up is re-asserted every frame).
pub const PITCH_LIMIT: f32 = 1.5533;
/// Mouse-look exponential smoothing, seconds.
pub const LOOK_SMOOTH_TAU: f32 = 0.008;
/// Horizontal accel / decel time constants, seconds.
pub const ACCEL_TAU: f32 = 0.10;
pub const DECEL_TAU: f32 = 0.15;
/// Eye-height spring onto the floor.
pub const HEIGHT_TAU: f32 = 0.09;
/// Fly Shift-run multiplier. Walk run is [`RUN_SPEED`], not this.
pub const RUN_MULTIPLIER: f32 = 3.0;
pub const SLOW_MULTIPLIER: f32 = 0.3;
/// Walk vertical field of view (degrees). Phone-like 60–75°; 70° is the
/// middle. Previous fov is restored on the way out.
pub const WALK_FOV_DEG: f32 = 70.0;
/// Capsule radius against walls, metres. Diameter 0.6 m, so ≥ 0.7 m doors pass.
pub const COLLISION_RADIUS: f32 = 0.3;
/// Side-ray offset from the capsule centre, meters.
pub const LANE_OFFSET: f32 = 0.25;
/// Knee / chest probe heights above the feet. The walkable riser is
/// [`STEP_UP`]; these rays catch walls, not stairs.
pub const KNEE_HEIGHT: f32 = 0.45;
pub const CHEST_HEIGHT: f32 = 1.2;
/// Fly speed = model AABB diagonal / this many seconds (then ×2 in `adopt`).
/// Walk does **not** use this — walk is physical metres per second.
pub const SPEED_DIAGONAL_SECS: f32 = 60.0;
pub const GRAVITY: f32 = 9.81;
pub const TERMINAL_FALL: f32 = 8.0;
pub const JUMP_SPEED: f32 = 3.8;
/// Head-bob amplitude (meters) when enabled. Off by default.
pub const BOB_AMP: f32 = 0.025;
pub const BOB_HZ_AT_SPEED: f32 = 1.8;

/// True when the process was told never to take the OS pointer.
pub fn no_mouse_lock() -> bool {
    std::env::var_os("FAB_NO_MOUSE_LOCK").is_some()
}

/// Mailbox from the gizmo (which sees `WindowLostFocus`) to every navigator
/// (each viewport owns its own capture flag). A generation counter so both
/// locked viewports see the same blur, unlike a one-shot bool.
static RELEASE_GEN: AtomicU64 = AtomicU64::new(0);

/// Ask every navigator to drop pointer capture on the next input/frame.
pub fn request_capture_release() {
    RELEASE_GEN.fetch_add(1, Ordering::SeqCst);
}

/// `true` when a blur/Esc mailbox has been posted since `seen` was last
/// updated. Advances `seen` so each navigator reacts once per request.
pub fn pending_capture_release(seen: &mut u64) -> bool {
    let g = RELEASE_GEN.load(Ordering::SeqCst);
    if g != *seen {
        *seen = g;
        true
    } else {
        false
    }
}

/// Exponential approach that behaves the same at any frame rate.
fn approach(dt: f32, tau: f32) -> f32 {
    1.0 - (-dt / tau.max(1e-4)).exp()
}

fn wrap_pi(a: f32) -> f32 {
    let tau = std::f32::consts::TAU;
    let mut a = a;
    if a > std::f32::consts::PI {
        a -= tau;
    } else if a < -std::f32::consts::PI {
        a += tau;
    }
    a
}

#[derive(Clone, Copy, Debug)]
pub struct WalkState {
    /// The pointer is ours: mouse motion looks around.
    pub captured: bool,
    pub keys: u16,
    pub yaw: f32,
    pub pitch: f32,
    /// Unsmoothed look target; `yaw`/`pitch` chase these with `LOOK_SMOOTH_TAU`.
    pub yaw_tgt: f32,
    pub pitch_tgt: f32,
    /// Radians per point of pointer travel. The "sensitivity setting".
    pub look_sens: f32,
    pub vel: Vec3f,
    pub eye_height: f32,
    /// Meters per second before the run/slow multiplier and the user's scale.
    pub base_speed: f32,
    /// Wheel-adjustable multiplier, Fab's walk-speed wheel.
    pub speed_scale: f32,
    /// Where the pivot sits ahead of the eye, so the handoff back to orbit
    /// and the pan/zoom rates stay sane.
    pub pivot_dist: f32,
    /// Set on the frame after entering walk, to drop the eye onto the floor.
    pub settling: bool,
    /// FOV to restore when leaving walk. `None` when we did not change it.
    pub saved_fov: Option<f32>,
    /// Smooth head bob. Off by default.
    pub head_bob: bool,
    pub bob_phase: f32,
    /// Last observed `RELEASE_GEN` (window-blur mailbox).
    pub release_gen: u64,
    /// Seconds spent falling with no floor in reach; see `FALL_RESPAWN_S`.
    pub fall_time: f32,
    /// Eye position the last time the walker stood on a floor.
    pub last_grounded: Option<Vec3f>,
}

impl Default for WalkState {
    fn default() -> Self {
        WalkState {
            captured: false,
            keys: 0,
            yaw: 0.0,
            pitch: 0.0,
            yaw_tgt: 0.0,
            pitch_tgt: 0.0,
            look_sens: LOOK_SENS,
            fall_time: 0.0,
            last_grounded: None,
            vel: vec3(0.0, 0.0, 0.0),
            eye_height: EYE_HEIGHT,
            base_speed: WALK_SPEED,
            speed_scale: 1.0,
            pivot_dist: 6.0,
            settling: false,
            saved_fov: None,
            head_bob: false,
            bob_phase: 0.0,
            release_gen: RELEASE_GEN.load(Ordering::Relaxed),
        }
    }
}

impl WalkState {
    /// Something still has to happen next frame.
    pub fn active(&self) -> bool {
        self.captured
            || self.keys != 0
            || self.vel.length() > 0.01
            || self.settling
            || (self.yaw - self.yaw_tgt).abs() > 1e-4
            || (self.pitch - self.pitch_tgt).abs() > 1e-4
    }

    pub fn clear_keys(&mut self) {
        self.keys = 0;
        self.vel = vec3(0.0, 0.0, 0.0);
    }

    /// Give the pointer back. Escape, window blur, and hover-out all call
    /// this; it is the one path that must never fail to unlock.
    pub fn release_capture(&mut self) {
        self.captured = false;
        self.clear_keys();
    }

    /// Take the camera's current aim as the look angles. Called on every
    /// orbit → fly/walk handoff so the view does not jump one pixel.
    pub fn adopt(&mut self, cam: &Camera, mode: NavMode, bounds_diagonal: f32) {
        let f = cam.forward();
        if f.is_finite() {
            let horiz = (f.x * f.x + f.y * f.y).sqrt();
            self.yaw = f.y.atan2(f.x);
            self.pitch = f.z.atan2(horiz.max(1e-6)).clamp(-PITCH_LIMIT, PITCH_LIMIT);
        }
        self.yaw_tgt = self.yaw;
        self.pitch_tgt = self.pitch;
        let diag = bounds_diagonal.max(0.5);
        if mode == NavMode::Walk {
            // Metres per second, regardless of whether the villa is 20 m or
            // a city block is 400 m. Scaling walk to the AABB made a house
            // crawl and a campus sprint. Reset the shared wheel scale so a
            // sped-up Fly cannot leak into Walk.
            self.base_speed = WALK_SPEED;
            self.speed_scale = 1.0;
        } else {
            // Fly still covers a model in about SPEED_DIAGONAL_SECS / 2.
            self.base_speed = (diag / SPEED_DIAGONAL_SECS).max(0.05) * 2.0;
        }
        self.pivot_dist = (diag * 0.125).clamp(1.0, 25.0);
        self.vel = vec3(0.0, 0.0, 0.0);
        self.keys = 0;
        self.settling = mode == NavMode::Walk;
        self.bob_phase = 0.0;
        self.eye_height = EYE_HEIGHT;
    }

    /// Chase the look target by `dt`. Exponential, frame-rate independent.
    pub fn smooth_look(&mut self, dt: f32) {
        let k = approach(dt.max(0.0), LOOK_SMOOTH_TAU);
        let dyaw = wrap_pi(self.yaw_tgt - self.yaw);
        self.yaw = wrap_pi(self.yaw + dyaw * k);
        self.pitch = (self.pitch + (self.pitch_tgt - self.pitch) * k).clamp(-PITCH_LIMIT, PITCH_LIMIT);
    }

    /// Add physical pointer travel to the unsmoothed aim target. The
    /// navigator calls this once per frame with all events accumulated since
    /// the previous frame, so event and redraw cadence cannot amplify look.
    pub fn add_look_delta(&mut self, dx: f32, dy: f32) {
        let s = self.look_sens.max(1e-6);
        self.yaw_tgt = wrap_pi(self.yaw_tgt - dx * s);
        self.pitch_tgt = (self.pitch_tgt - dy * s).clamp(-PITCH_LIMIT, PITCH_LIMIT);
    }

    pub fn look(&mut self, dx: f32, dy: f32) {
        self.add_look_delta(dx, dy);
        // One time-constant of smoothing so a look event is not a frame late,
        // without snapping the full delta in direct/non-navigator callers.
        self.smooth_look(LOOK_SMOOTH_TAU);
    }

    pub fn forward(&self) -> Vec3f {
        let cp = self.pitch.cos();
        vec3(cp * self.yaw.cos(), cp * self.yaw.sin(), self.pitch.sin())
    }

    /// The eye's ground-plane basis (walk moves along the floor, not the aim).
    fn ground_basis(&self) -> (Vec3f, Vec3f) {
        let fwd = vec3(self.yaw.cos(), self.yaw.sin(), 0.0);
        // Right is fwd × up in the z-up, right-handed world: facing +x,
        // right is −y. (The mirrored form put D on the left.)
        let right = vec3(self.yaw.sin(), -self.yaw.cos(), 0.0);
        (fwd, right)
    }

    pub fn set_key(&mut self, key: KeyCode, down: bool) -> bool {
        let bit = match key {
            KeyCode::KeyW | KeyCode::ArrowUp => K_FWD,
            KeyCode::KeyS | KeyCode::ArrowDown => K_BACK,
            KeyCode::KeyA | KeyCode::ArrowLeft => K_LEFT,
            KeyCode::KeyD | KeyCode::ArrowRight => K_RIGHT,
            KeyCode::KeyE => K_UP,
            KeyCode::KeyQ => K_DOWN,
            KeyCode::Shift => K_RUN,
            KeyCode::Control => K_SLOW,
            KeyCode::Space => K_JUMP,
            _ => return false,
        };
        if down {
            self.keys |= bit;
        } else {
            self.keys &= !bit;
        }
        true
    }

    pub fn set_run(&mut self, run: bool) {
        if run {
            self.keys |= K_RUN;
        } else {
            self.keys &= !K_RUN;
        }
    }

    pub fn set_slow(&mut self, slow: bool) {
        if slow {
            self.keys |= K_SLOW;
        } else {
            self.keys &= !K_SLOW;
        }
    }

    /// Wheel input changes the flying or walking speed.
    /// Walk stays inside [`WALK_SPEED_MIN`]..=[`WALK_SPEED_MAX`]; Fly keeps
    /// the wide 0.1–12× multiplier on its bounds-scaled base.
    pub fn nudge_speed(&mut self, steps: f32, mode: NavMode) {
        let next = self.speed_scale * 1.2f32.powf(steps);
        self.speed_scale = if mode == NavMode::Walk {
            next.clamp(WALK_SPEED_MIN / WALK_SPEED, WALK_SPEED_MAX / WALK_SPEED)
        } else {
            next.clamp(0.1, 12.0)
        };
    }

    pub fn speed(&self, mode: NavMode) -> f32 {
        if mode == NavMode::Walk {
            let walk = (WALK_SPEED * self.speed_scale).clamp(WALK_SPEED_MIN, WALK_SPEED_MAX);
            if self.keys & K_SLOW != 0 {
                walk * SLOW_MULTIPLIER
            } else if self.keys & K_RUN != 0 {
                RUN_SPEED
            } else {
                walk
            }
        } else {
            let mul = if self.keys & K_SLOW != 0 {
                SLOW_MULTIPLIER
            } else if self.keys & K_RUN != 0 {
                RUN_MULTIPLIER
            } else {
                1.0
            };
            self.base_speed * self.speed_scale * mul
        }
    }
}

/// Status-bar HUD: `"Walk · 1.6 m · 1.4 m/s · Esc to release"`.
pub fn hud_line(walk: &WalkState, mode: NavMode) -> String {
    let name = match mode {
        NavMode::Walk => "Walk",
        NavMode::Fly => "Fly",
        NavMode::Orbit => "Orbit",
    };
    format!(
        "{name} · {:.1} m · {:.1} m/s · Esc to release",
        walk.eye_height,
        walk.speed(mode)
    )
}

/// Scene raycast with the same visibility filter picking uses.
pub fn scene_raycast(state: &AppState, ray: &Ray) -> Option<RayHit> {
    if state.scene.is_empty() {
        return None;
    }
    state
        .scene
        .bvh
        .raycast(&state.scene.batches, ray, &|id| state.is_visible(id))
}

#[derive(Clone, Copy)]
enum NavRayKind {
    Move,
    Floor,
}

/// One-pass navigation query. Door/opening/zone elements are rejected by the
/// BVH's element predicate before triangle intersection. The hit predicate
/// then keeps only collision walls or upward-facing floors above the cached
/// terrain-skirt cutoff; it never rescans scene bounds and never restarts a
/// ray after a rejected surface.
fn scene_nav_raycast(state: &AppState, ray: &Ray, kind: NavRayKind) -> Option<RayHit> {
    if state.scene.is_empty() || !ray.dir.is_finite() {
        return None;
    }
    let analysis = state.current_walk_analysis();
    let floor_min_z = analysis.map_or(f32::NEG_INFINITY, |cache| cache.floor_min_z);
    let visible = |id: ElementId| {
        state.is_visible(id) && !analysis.is_some_and(|cache| cache.element_is_passable(id))
    };
    let accept = |hit: &RayHit| match kind {
        // Horizontal floors/ceilings belong to the floor probe, not the
        // capsule. Rejecting both faces also prevents collision from below.
        NavRayKind::Move => hit.point.z >= floor_min_z && hit.normal.z.abs() < 0.7,
        // A downward ray returns the nearest accepted floor, even with a
        // mirrored terrain skirt farther below it.
        NavRayKind::Floor => hit.point.z >= floor_min_z && hit.normal.z >= 0.35,
    };
    state
        .scene
        .bvh
        .raycast_filtered(&state.scene.batches, ray, &visible, &accept)
}

/// Cached front-door pose from the tour site's exterior/interior portal
/// analysis. The fallback is used only when no analysed entrance exists.
pub fn walk_start_pose(state: &AppState) -> makepad_fab_tour::WalkEntryPose {
    if let Some(cache) = state.current_walk_analysis() {
        return cache.entry;
    }
    let bounds = framing_bounds(&state.scene);
    if aabb_is_empty(&bounds) {
        return makepad_fab_tour::WalkEntryPose {
            eye: vec3(0.0, 0.0, EYE_HEIGHT),
            forward: vec3(0.0, 1.0, 0.0),
        };
    }
    makepad_fab_tour::WalkEntryPose {
        eye: vec3(
            (bounds.min.x + bounds.max.x) * 0.5,
            (bounds.min.y + bounds.max.y) * 0.5,
            bounds.min.z + EYE_HEIGHT,
        ),
        forward: vec3(0.0, 1.0, 0.0),
    }
}

/// Height of the first surface below `(x, y, from_z)`, within `max_drop`.
pub fn ground_below(
    state: &AppState,
    x: f32,
    y: f32,
    from_z: f32,
    max_drop: f32,
) -> Option<f32> {
    ground_z(
        &|ray| scene_nav_raycast(state, ray, NavRayKind::Floor),
        vec3(x, y, from_z + 1e-4),
        from_z,
        max_drop,
    )
}

fn ground_z(
    raycast: &dyn Fn(&Ray) -> Option<RayHit>,
    at: Vec3f,
    from_z: f32,
    max_drop: f32,
) -> Option<f32> {
    let origin = vec3(at.x, at.y, from_z);
    let ray = Ray::new(origin, vec3(0.0, 0.0, -1.0));
    if !ray.dir.is_finite() {
        return None;
    }
    let hit = raycast(&ray)?;
    if !hit.t.is_finite() || hit.t > max_drop || hit.t < 0.0 {
        return None;
    }
    // A wall seen by a downward ray is not a floor.
    if hit.normal.z < 0.35 {
        return None;
    }
    Some(hit.point.z)
}

fn horiz_perp(dir: Vec3f) -> Vec3f {
    let h = vec3(dir.x, dir.y, 0.0);
    let len = h.length();
    if len < 1e-4 {
        vec3(1.0, 0.0, 0.0)
    } else {
        vec3(-h.y / len, h.x / len, 0.0)
    }
}

/// Slide `from` by `delta`, stopping `radius` short of any hit and sliding
/// the remainder along the wall plane. Six probes: knee + chest, each with
/// a centre ray and ±`LANE_OFFSET` lanes. Returns `(new_pos, wall_normal)`.
pub fn collide_move(
    from: Vec3f,
    delta: Vec3f,
    radius: f32,
    eye_height: f32,
    raycast: &dyn Fn(&Ray) -> Option<RayHit>,
) -> (Vec3f, Option<Vec3f>) {
    let mut pos = from;
    let mut remain = delta;
    let mut wall: Option<Vec3f> = None;
    let feet_z = from.z - eye_height;
    let heights = [KNEE_HEIGHT, CHEST_HEIGHT];
    let lanes = [-LANE_OFFSET, 0.0, LANE_OFFSET];
    for _ in 0..2 {
        let dist = remain.length();
        if dist < 1e-6 || !remain.is_finite() {
            break;
        }
        let dir = remain * (1.0 / dist);
        if !dir.is_finite() {
            break;
        }
        let side = horiz_perp(dir);
        let mut best: Option<RayHit> = None;
        for h in heights {
            for lane in lanes {
                let origin = vec3(pos.x, pos.y, feet_z + h) + side * lane;
                if !origin.is_finite() {
                    continue;
                }
                let ray = Ray::new(origin, dir);
                let Some(hit) = raycast(&ray) else {
                    continue;
                };
                if !hit.t.is_finite() || hit.t <= 1e-4 || hit.t >= dist + radius {
                    continue;
                }
                // Floors are the downward probe's job, not a wall.
                if hit.normal.z > 0.35 {
                    continue;
                }
                if best.map(|b| hit.t < b.t).unwrap_or(true) {
                    best = Some(hit);
                }
            }
        }
        match best {
            Some(hit) => {
                let allowed = (hit.t - radius).max(0.0);
                pos += dir * allowed;
                let leftover = remain - dir * allowed;
                let mut n = hit.normal;
                if n.dot(dir) > 0.0 {
                    n = n * -1.0;
                }
                let n = n.normalize();
                if !n.is_finite() {
                    break;
                }
                wall = Some(n);
                // Project leftover onto the wall plane. A positive dot with
                // the original dir is the slide (walking 45° into a wall),
                // not a reason to stop.
                remain = leftover - n * leftover.dot(n);
            }
            None => {
                pos += remain;
                remain = vec3(0.0, 0.0, 0.0);
            }
        }
    }
    if pos.is_finite() {
        (pos, wall)
    } else {
        (from, wall)
    }
}

/// Pure walk/fly integrator. `raycast` is the scene (or a synthetic world in
/// tests). Returns the new eye (without head-bob).
pub fn integrate(
    walk: &mut WalkState,
    mode: NavMode,
    eye: Vec3f,
    dt: f32,
    raycast: &dyn Fn(&Ray) -> Option<RayHit>,
) -> Vec3f {
    let dt = dt.clamp(0.0, 0.1);
    if dt <= 0.0 {
        return eye;
    }
    let start = eye;
    walk.smooth_look(dt);
    let (gf, gr) = walk.ground_basis();
    let aim = walk.forward();
    let walking = mode == NavMode::Walk;

    let feet_z = eye.z - walk.eye_height;
    // Start a hair above the step-up so a floor exactly `STEP_UP` up is not
    // missed with t = 0.
    let probe_from = if walk.settling {
        eye.z + 0.1
    } else {
        feet_z + STEP_UP + 0.05
    };
    let max_drop = if walk.settling {
        FALL_MAX * 4.0
    } else {
        FALL_MAX + STEP_UP
    };
    let floor = if walking {
        ground_z(raycast, eye, probe_from, max_drop)
    } else {
        None
    };

    let mut on_ground = false;
    if walking {
        if let Some(g) = floor {
            let dz = g - feet_z;
            // Positive vertical velocity means we are still on the jump
            // ascent — do not snap back onto the floor we just left.
            if walk.vel.z <= 0.0 && dz <= STEP_UP + 1e-3 && dz >= -0.08 {
                on_ground = true;
            }
        }
        if walk.settling && walk.vel.z <= 0.0 {
            on_ground = floor.is_some();
        }
    }

    // Jump is cheap: one impulse off the floor, then gravity does the rest.
    // Ground snapping stays suppressed until vel.z ≤ 0 (apex / descent).
    if walking && on_ground && walk.keys & K_JUMP != 0 {
        walk.vel.z = JUMP_SPEED;
        on_ground = false;
        walk.settling = false;
        walk.keys &= !K_JUMP;
    }

    let fwd = if walking { gf } else { aim };
    let mut want = vec3(0.0, 0.0, 0.0);
    if walk.keys & K_FWD != 0 {
        want += fwd;
    }
    if walk.keys & K_BACK != 0 {
        want -= fwd;
    }
    if walk.keys & K_RIGHT != 0 {
        want += gr;
    }
    if walk.keys & K_LEFT != 0 {
        want -= gr;
    }
    // Q/E is fly-only (up/down in world Z). Walk stays on the floor.
    if !walking {
        if walk.keys & K_UP != 0 {
            want += WORLD_UP;
        }
        if walk.keys & K_DOWN != 0 {
            want -= WORLD_UP;
        }
    }
    let len = want.length();
    let mut target_vel = if len > 1e-4 {
        want * (walk.speed(mode) / len)
    } else {
        vec3(0.0, 0.0, 0.0)
    };
    if walking {
        target_vel.z = 0.0;
    }

    let horiz = vec3(walk.vel.x, walk.vel.y, if walking { 0.0 } else { walk.vel.z });
    let tau = if target_vel.length() + 1e-4 >= horiz.length() && len > 1e-4 {
        ACCEL_TAU
    } else {
        DECEL_TAU
    };
    let k = approach(dt, tau);
    if walking {
        walk.vel.x += (target_vel.x - walk.vel.x) * k;
        walk.vel.y += (target_vel.y - walk.vel.y) * k;
    } else {
        walk.vel += (target_vel - walk.vel) * k;
    }
    if !walk.vel.is_finite() {
        walk.vel = vec3(0.0, 0.0, 0.0);
    }
    let vmax = walk.speed(mode);
    if walking {
        let hlen = vec3(walk.vel.x, walk.vel.y, 0.0).length();
        if vmax > 0.0 && hlen > vmax {
            let s = vmax / hlen;
            walk.vel.x *= s;
            walk.vel.y *= s;
        }
        if hlen < 1e-4 && len < 1e-4 {
            walk.vel.x = 0.0;
            walk.vel.y = 0.0;
        }
    } else {
        let vlen = walk.vel.length();
        if vmax > 0.0 && vlen > vmax {
            walk.vel = walk.vel * (vmax / vlen);
        }
        if vlen < 1e-3 && len < 1e-4 {
            walk.vel = vec3(0.0, 0.0, 0.0);
        }
    }

    let delta = if walking {
        vec3(walk.vel.x, walk.vel.y, 0.0) * dt
    } else {
        walk.vel * dt
    };
    let (mut eye, wall) = collide_move(eye, delta, COLLISION_RADIUS, walk.eye_height, raycast);
    if let Some(n) = wall {
        let n = if walking {
            let n = vec3(n.x, n.y, 0.0);
            let l = n.length();
            if l > 1e-4 {
                n * (1.0 / l)
            } else {
                n
            }
        } else {
            n
        };
        if n.is_finite() && n.length() > 1e-4 {
            let into = walk.vel.dot(n);
            if into < 0.0 {
                walk.vel = walk.vel - n * into;
            }
        }
    }

    if walking {
        // Knee rays miss a riser under KNEE_HEIGHT. Query the floor at the
        // candidate XY and refuse a step taller than STEP_UP.
        let cand_floor = ground_z(raycast, eye, probe_from, max_drop);
        let floor = match cand_floor {
            Some(g) if g - feet_z > STEP_UP => {
                eye.x = start.x;
                eye.y = start.y;
                walk.vel.x = 0.0;
                walk.vel.y = 0.0;
                floor
            }
            other => other,
        };

        if on_ground && walk.vel.z <= 0.0 {
            if let Some(g) = floor {
                if g - feet_z <= STEP_UP + 1e-3 {
                    let want_z = g + walk.eye_height;
                    let kz = approach(dt, HEIGHT_TAU);
                    eye.z += (want_z - eye.z) * kz;
                    if (want_z - eye.z).abs() < 0.005 {
                        eye.z = want_z;
                        walk.settling = false;
                    }
                    walk.vel.z = 0.0;
                }
            }
        } else {
            walk.settling = false;
            walk.vel.z = (walk.vel.z - GRAVITY * dt).max(-TERMINAL_FALL);
            let mut new_z = eye.z + walk.vel.z * dt;
            if let Some(g) = floor {
                // Do not snap up onto a ledge the feet have not reached.
                if g <= feet_z + 1e-3 {
                    let land = g + walk.eye_height;
                    if new_z <= land {
                        new_z = land;
                        walk.vel.z = 0.0;
                    }
                }
            }
            eye.z = new_z;
        }
    }

    let hspeed = vec3(walk.vel.x, walk.vel.y, 0.0).length();
    if walk.head_bob && walking && on_ground && hspeed > 0.05 {
        walk.bob_phase += dt
            * BOB_HZ_AT_SPEED
            * (hspeed / vmax.max(1e-3)).clamp(0.4, 1.6)
            * std::f32::consts::TAU;
    } else {
        walk.bob_phase *= (1.0 - approach(dt, 0.12)).max(0.0);
    }

    if !eye.is_finite() {
        walk.vel = vec3(0.0, 0.0, 0.0);
        return start;
    }
    if walking {
        if on_ground {
            walk.fall_time = 0.0;
            walk.last_grounded = Some(eye);
        } else {
            walk.fall_time += dt;
            if walk.fall_time > FALL_RESPAWN_S {
                if let Some(back) = walk.last_grounded {
                    walk.fall_time = 0.0;
                    walk.vel = vec3(0.0, 0.0, 0.0);
                    walk.settling = true;
                    return back;
                }
            }
        }
    }
    eye
}

fn bob_offset(walk: &WalkState) -> f32 {
    if !walk.head_bob {
        return 0.0;
    }
    walk.bob_phase.sin() * BOB_AMP
}

/// Advance fly/walk by `dt` and write the camera. Returns true when anything
/// moved (the caller marks the view and keeps the frames coming).
pub fn step(walk: &mut WalkState, state: &mut AppState, view: usize, dt: f32) -> bool {
    let dt = dt.clamp(0.0, 0.1);
    if dt <= 0.0 {
        return false;
    }
    let mode = state.view_at(view).nav_mode;
    let rendered = state.view_at(view).camera.eye;
    let start = vec3(rendered.x, rendered.y, rendered.z - bob_offset(walk));
    let raycast = |ray: &Ray| {
        if mode == NavMode::Walk && ray.dir.z < -0.999 {
            scene_nav_raycast(state, ray, NavRayKind::Floor)
        } else {
            scene_nav_raycast(state, ray, NavRayKind::Move)
        }
    };
    let eye = integrate(walk, mode, start, dt, &raycast);
    write_camera(walk, state, view, eye);
    let end = state.view_at(view).camera.eye;
    (end - start).length() > 1e-6
        || (walk.yaw - walk.yaw_tgt).abs() > 1e-5
        || (walk.pitch - walk.pitch_tgt).abs() > 1e-5
}

fn write_camera(walk: &WalkState, state: &mut AppState, view: usize, eye: Vec3f) {
    let aim = walk.forward();
    let bob = bob_offset(walk);
    let vs = state.view_at_mut(view);
    vs.camera.eye = vec3(eye.x, eye.y, eye.z + bob);
    vs.camera.target = vs.camera.eye + aim * walk.pivot_dist.max(0.05);
    vs.camera.up = WORLD_UP;
    vs.camera.ortho = false;
    if vs.nav_mode == NavMode::Walk {
        vs.camera.fov_y_deg = WALK_FOV_DEG;
    }
}

/// Aim the camera from the look angles without moving the eye — used the
/// instant the pointer turns, so looking is never one frame late.
pub fn apply_look(walk: &WalkState, state: &mut AppState, view: usize) {
    let aim = walk.forward();
    let vs = state.view_at_mut(view);
    vs.camera.target = vs.camera.eye + aim * walk.pivot_dist.max(0.05);
    vs.camera.up = WORLD_UP;
    vs.camera.ortho = false;
    if vs.nav_mode == NavMode::Walk {
        vs.camera.fov_y_deg = WALK_FOV_DEG;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(t: f32, point: Vec3f, normal: Vec3f) -> RayHit {
        RayHit {
            element: ElementId(0),
            batch: 0,
            triangle: 0,
            t,
            point,
            normal,
            bary: [0.0, 0.0],
        }
    }

    /// Infinite floor at `z = floor` and an optional infinite wall at `x = wall_x`.
    fn world(floor: Option<f32>, wall_x: Option<f32>) -> impl Fn(&Ray) -> Option<RayHit> {
        move |ray: &Ray| {
            let mut best: Option<RayHit> = None;
            if let Some(z) = floor {
                if ray.dir.z.abs() > 1e-8 {
                    let t = (z - ray.origin.z) / ray.dir.z;
                    if t > 1e-4 {
                        let mut n = vec3(0.0, 0.0, 1.0);
                        if n.dot(ray.dir) > 0.0 {
                            n = n * -1.0;
                        }
                        best = Some(hit(t, ray.at(t), n));
                    }
                }
            }
            if let Some(x) = wall_x {
                if ray.dir.x.abs() > 1e-8 {
                    let t = (x - ray.origin.x) / ray.dir.x;
                    if t > 1e-4 {
                        let mut n = vec3(-1.0, 0.0, 0.0);
                        if ray.origin.x > x {
                            n = vec3(1.0, 0.0, 0.0);
                        }
                        if n.dot(ray.dir) > 0.0 {
                            n = n * -1.0;
                        }
                        let cand = hit(t, ray.at(t), n);
                        if best.map(|h| t < h.t).unwrap_or(true) {
                            best = Some(cand);
                        }
                    }
                }
            }
            best
        }
    }

    /// Floor at z=0 for x < step_x, z=step_h for x >= step_x, plus a riser.
    fn stepped(step_x: f32, step_h: f32) -> impl Fn(&Ray) -> Option<RayHit> {
        move |ray: &Ray| {
            let mut best: Option<RayHit> = None;
            let consider = |best: &mut Option<RayHit>, t: f32, n: Vec3f| {
                if t > 1e-4 && t.is_finite() {
                    let p = ray.at(t);
                    let mut n = n;
                    if n.dot(ray.dir) > 0.0 {
                        n = n * -1.0;
                    }
                    if best.map(|h| t < h.t).unwrap_or(true) {
                        *best = Some(hit(t, p, n));
                    }
                }
            };
            if ray.dir.z.abs() > 1e-8 {
                let t = (0.0 - ray.origin.z) / ray.dir.z;
                let p = ray.at(t);
                if p.x < step_x {
                    consider(&mut best, t, vec3(0.0, 0.0, 1.0));
                }
            }
            if ray.dir.z.abs() > 1e-8 {
                let t = (step_h - ray.origin.z) / ray.dir.z;
                let p = ray.at(t);
                if p.x >= step_x {
                    consider(&mut best, t, vec3(0.0, 0.0, 1.0));
                }
            }
            if ray.dir.x.abs() > 1e-8 {
                let t = (step_x - ray.origin.x) / ray.dir.x;
                let p = ray.at(t);
                if p.z >= -0.01 && p.z <= step_h + 0.01 {
                    consider(&mut best, t, vec3(-1.0, 0.0, 0.0));
                }
            }
            best
        }
    }

    /// Two infinite walls at y = ±half, a door passage of width `2*half`.
    fn corridor(half: f32) -> impl Fn(&Ray) -> Option<RayHit> {
        move |ray: &Ray| {
            let mut best: Option<RayHit> = None;
            if ray.dir.y.abs() > 1e-8 {
                for (y, n) in [(half, vec3(0.0, -1.0, 0.0)), (-half, vec3(0.0, 1.0, 0.0))] {
                    let t = (y - ray.origin.y) / ray.dir.y;
                    if t > 1e-4 {
                        let mut n = n;
                        if n.dot(ray.dir) > 0.0 {
                            n = n * -1.0;
                        }
                        let cand = hit(t, ray.at(t), n);
                        if best.map(|h| t < h.t).unwrap_or(true) {
                            best = Some(cand);
                        }
                    }
                }
            }
            if ray.dir.z.abs() > 1e-8 {
                let t = (0.0 - ray.origin.z) / ray.dir.z;
                if t > 1e-4 {
                    let cand = hit(t, ray.at(t), vec3(0.0, 0.0, 1.0));
                    if best.map(|h| t < h.t).unwrap_or(true) {
                        best = Some(cand);
                    }
                }
            }
            best
        }
    }

    fn cam_looking_x() -> Camera {
        let mut c = Camera::default();
        c.eye = vec3(0.0, 0.0, EYE_HEIGHT);
        c.target = vec3(4.0, 0.0, EYE_HEIGHT);
        c.up = WORLD_UP;
        c.fov_y_deg = WALK_FOV_DEG;
        c
    }

    #[test]
    fn adopt_preserves_the_aim() {
        let mut cam = Camera::default();
        cam.eye = vec3(10.0, -14.0, 6.0);
        cam.target = vec3(2.0, 1.0, 2.0);
        let before = cam.forward();
        let mut w = WalkState::default();
        w.adopt(&cam, NavMode::Fly, 24.0);
        let after = w.forward();
        assert!(
            after.dot(before) > 0.9999,
            "handoff turned the camera: {before:?} -> {after:?}"
        );
    }

    #[test]
    fn look_is_not_inverted_and_clamps() {
        let mut w = WalkState::default();
        w.yaw = 0.0;
        w.pitch = 0.0;
        w.yaw_tgt = 0.0;
        w.pitch_tgt = 0.0;
        // Pointer right: the aim swings to the right of +X, i.e. toward -Y.
        w.look(100.0, 0.0);
        assert!(w.forward().y < 0.0, "{:?}", w.forward());
        // Pointer down: the aim goes down.
        w.look(-100.0, 100.0);
        assert!(w.forward().z < 0.0, "{:?}", w.forward());
        for _ in 0..200 {
            w.look(0.0, 1000.0);
        }
        assert!(w.pitch >= -PITCH_LIMIT - 1e-6);
        assert!(w.pitch_tgt >= -PITCH_LIMIT - 1e-6);
        assert!(w.forward().is_finite());
        assert!(w.forward().z.abs() < 1e-3 || w.pitch < 0.0 || w.pitch > 0.0);
        // No roll: forward has no dependency on a roll angle; up is world.
        let f = w.forward();
        let r = Vec3f::cross(f, WORLD_UP).normalize();
        assert!(r.z.abs() < 0.15, "look introduced roll: right.z={}", r.z);
    }

    #[test]
    fn walk_speed_is_physical_fly_scales_with_the_model() {
        let cam = Camera::default();
        let mut small = WalkState::default();
        small.adopt(&cam, NavMode::Walk, 12.0);
        let mut big = WalkState::default();
        big.adopt(&cam, NavMode::Walk, 180.0);
        assert!(
            (small.speed(NavMode::Walk) - WALK_SPEED).abs() < 1e-5,
            "walk must stay 1.4 m/s on a small model, got {}",
            small.speed(NavMode::Walk)
        );
        assert!(
            (big.speed(NavMode::Walk) - WALK_SPEED).abs() < 1e-5,
            "walk must stay 1.4 m/s on a huge model, got {}",
            big.speed(NavMode::Walk)
        );
        let mut fly_small = WalkState::default();
        fly_small.adopt(&cam, NavMode::Fly, 12.0);
        let mut fly_big = WalkState::default();
        fly_big.adopt(&cam, NavMode::Fly, 180.0);
        assert!((fly_small.base_speed - 12.0 / SPEED_DIAGONAL_SECS * 2.0).abs() < 1e-5);
        assert!((fly_big.base_speed - 180.0 / SPEED_DIAGONAL_SECS * 2.0).abs() < 1e-5);
        assert!(
            fly_big.base_speed > fly_small.base_speed,
            "fly still scales to the model"
        );
        fly_big.keys = K_RUN;
        assert!(
            (fly_big.speed(NavMode::Fly) / fly_big.base_speed - RUN_MULTIPLIER).abs() < 1e-4,
            "fly run must stay 3×, got {}",
            fly_big.speed(NavMode::Fly)
        );
        big.keys = K_RUN;
        assert!(
            (big.speed(NavMode::Walk) - RUN_SPEED).abs() < 1e-4,
            "walk run must stay 4 m/s, got {}",
            big.speed(NavMode::Walk)
        );
    }

    #[test]
    fn walk_does_not_inherit_fly_speed_scale() {
        let cam = Camera::default();
        let mut w = WalkState::default();
        w.adopt(&cam, NavMode::Fly, 180.0);
        w.nudge_speed(1.0, NavMode::Fly);
        assert!(
            (w.speed_scale - 1.2).abs() < 1e-5,
            "fly wheel 1 step is 1.2×, got {}",
            w.speed_scale
        );
        w.adopt(&cam, NavMode::Walk, 180.0);
        assert!(
            (w.speed_scale - 1.0).abs() < 1e-5,
            "entering Walk must reset speed_scale, got {}",
            w.speed_scale
        );
        assert!(
            (w.speed(NavMode::Walk) - WALK_SPEED).abs() < 1e-4,
            "entering Walk after a wheel-scaled Fly must walk at 1.4 m/s, got {}",
            w.speed(NavMode::Walk)
        );
    }

    #[test]
    fn integrator_never_nans() {
        let mut w = WalkState::default();
        w.adopt(&cam_looking_x(), NavMode::Walk, 60.0);
        w.keys = K_FWD | K_LEFT | K_UP | K_RUN;
        let raycast = world(Some(0.0), Some(8.0));
        let mut eye = vec3(1.0, 1.0, EYE_HEIGHT);
        for i in 0..240 {
            w.look((i % 17) as f32 * 3.0 - 20.0, (i % 11) as f32 * 5.0 - 15.0);
            let dt = if i % 9 == 0 { 0.1 } else { 1.0 / 60.0 };
            eye = integrate(&mut w, NavMode::Walk, eye, dt, &raycast);
            assert!(eye.is_finite(), "eye NaN at frame {i}: {eye:?}");
            assert!(w.vel.is_finite(), "vel NaN at frame {i}: {:?}", w.vel);
            assert!(w.forward().is_finite());
        }
        w.pitch = PITCH_LIMIT;
        w.pitch_tgt = PITCH_LIMIT;
        eye = integrate(&mut w, NavMode::Walk, eye, 0.0, &raycast);
        assert!(eye.is_finite());
        w.keys = 0;
        eye = integrate(&mut w, NavMode::Fly, eye, 0.05, &world(None, None));
        assert!(eye.is_finite());
    }

    #[test]
    fn speed_limits() {
        let mut w = WalkState::default();
        w.adopt(&cam_looking_x(), NavMode::Walk, 60.0);
        w.speed_scale = 1.0;
        w.keys = K_FWD;
        let raycast = world(Some(0.0), None);
        let mut eye = vec3(0.0, 0.0, EYE_HEIGHT);
        for _ in 0..180 {
            eye = integrate(&mut w, NavMode::Walk, eye, 1.0 / 60.0, &raycast);
        }
        let v = vec3(w.vel.x, w.vel.y, 0.0).length();
        assert!(
            v <= w.speed(NavMode::Walk) * 1.01 + 1e-4,
            "walk speed {v} exceeded cap {}",
            w.speed(NavMode::Walk)
        );
        assert!(
            v > w.speed(NavMode::Walk) * 0.85,
            "never reached walk speed: {v}"
        );
        assert!(
            (w.speed(NavMode::Walk) - WALK_SPEED).abs() < 1e-4,
            "walk cap {} want {}",
            w.speed(NavMode::Walk),
            WALK_SPEED
        );

        w.keys = K_FWD | K_RUN;
        for _ in 0..180 {
            eye = integrate(&mut w, NavMode::Walk, eye, 1.0 / 60.0, &raycast);
        }
        let v = vec3(w.vel.x, w.vel.y, 0.0).length();
        assert!(
            v <= w.speed(NavMode::Walk) * 1.01 + 1e-4,
            "run speed {v} exceeded cap {}",
            w.speed(NavMode::Walk)
        );
        assert!(
            (w.speed(NavMode::Walk) - RUN_SPEED).abs() < 1e-4,
            "run cap {} want {}",
            w.speed(NavMode::Walk),
            RUN_SPEED
        );
        assert!(v > RUN_SPEED * 0.85, "never reached run speed: {v}");

        w.keys = K_FWD | K_SLOW;
        for _ in 0..180 {
            eye = integrate(&mut w, NavMode::Walk, eye, 1.0 / 60.0, &raycast);
        }
        let v = vec3(w.vel.x, w.vel.y, 0.0).length();
        assert!(
            v <= w.speed(NavMode::Walk) * 1.01 + 1e-4,
            "slow speed {v} exceeded cap {}",
            w.speed(NavMode::Walk)
        );
        assert!(
            (w.speed(NavMode::Walk) / (WALK_SPEED * w.speed_scale) - SLOW_MULTIPLIER).abs() < 1e-5
        );
        let _ = eye;
    }

    #[test]
    fn escape_releases_capture() {
        let mut w = WalkState::default();
        w.captured = true;
        w.keys = K_FWD | K_RUN;
        w.vel = vec3(2.0, 0.0, 0.0);
        w.release_capture();
        assert!(!w.captured, "Escape must drop pointer capture");
        assert_eq!(w.keys, 0);
        assert_eq!(w.vel, vec3(0.0, 0.0, 0.0));

        let mut seen = w.release_gen;
        request_capture_release();
        assert!(pending_capture_release(&mut seen));
        assert!(!pending_capture_release(&mut seen));
    }

    #[test]
    fn eye_height_tracks_a_synthetic_floor() {
        let mut w = WalkState::default();
        w.adopt(&cam_looking_x(), NavMode::Walk, 60.0);
        w.keys = 0;
        let floor_z = 2.4;
        let raycast = world(Some(floor_z), None);
        let mut eye = vec3(0.0, 0.0, 20.0);
        for _ in 0..180 {
            eye = integrate(&mut w, NavMode::Walk, eye, 1.0 / 60.0, &raycast);
        }
        assert!(
            (eye.z - (floor_z + EYE_HEIGHT)).abs() < 0.02,
            "eye {} expected {}",
            eye.z,
            floor_z + EYE_HEIGHT
        );
        // Raise the floor by a real stair riser (≈ 0.18, under STEP_UP 0.25).
        let riser = 0.20;
        let raycast = world(Some(floor_z + riser), None);
        for _ in 0..180 {
            eye = integrate(&mut w, NavMode::Walk, eye, 1.0 / 60.0, &raycast);
        }
        assert!((eye.z - (floor_z + riser + EYE_HEIGHT)).abs() < 0.02);
    }

    #[test]
    fn wall_stop_distance() {
        let mut w = WalkState::default();
        w.adopt(&cam_looking_x(), NavMode::Walk, 60.0);
        w.speed_scale = 4.0;
        w.keys = K_FWD;
        let wall_x = 5.0;
        let raycast = world(Some(0.0), Some(wall_x));
        let mut eye = vec3(0.0, 0.0, EYE_HEIGHT);
        for _ in 0..300 {
            eye = integrate(&mut w, NavMode::Walk, eye, 1.0 / 60.0, &raycast);
            assert!(
                eye.x <= wall_x - COLLISION_RADIUS + 0.02,
                "penetrated wall: x={} wall={} radius={}",
                eye.x,
                wall_x,
                COLLISION_RADIUS
            );
        }
        assert!(
            eye.x > wall_x - COLLISION_RADIUS - 0.15,
            "stopped too far from the wall: x={}",
            eye.x
        );
        assert!((eye.x - (wall_x - COLLISION_RADIUS)).abs() < 0.08);
        assert!(eye.is_finite());
    }

    #[test]
    fn slide_preserves_tangential_velocity() {
        let wall_x = 2.0;
        let raycast = world(None, Some(wall_x));
        let from = vec3(0.0, 0.0, EYE_HEIGHT);
        // Hit the wall going +X+Y: leftover should become pure +Y.
        let delta = vec3(3.0, 3.0, 0.0);
        let (pos, n) = collide_move(from, delta, COLLISION_RADIUS, EYE_HEIGHT, &raycast);
        assert!(pos.is_finite());
        assert!(
            pos.x <= wall_x - COLLISION_RADIUS + 0.02,
            "x={} wall-r={}",
            pos.x,
            wall_x - COLLISION_RADIUS
        );
        assert!(
            pos.y > 2.0,
            "slide should keep the tangential travel, y={}",
            pos.y
        );
        let n = n.expect("wall normal");
        let vel = vec3(4.0, 4.0, 0.0);
        let slid = vel - n * vel.dot(n);
        assert!(slid.x.abs() < 0.05, "normal component must die: {slid:?}");
        assert!(
            (slid.y - vel.y).abs() < 0.05,
            "tangential velocity must survive: {slid:?} vs {vel:?}"
        );

        let mut w = WalkState::default();
        w.adopt(&cam_looking_x(), NavMode::Walk, 60.0);
        w.yaw = std::f32::consts::FRAC_PI_4;
        w.yaw_tgt = w.yaw;
        w.pitch = 0.0;
        w.pitch_tgt = 0.0;
        w.keys = K_FWD;
        w.speed_scale = 4.0;
        let mut eye = vec3(0.0, 0.0, EYE_HEIGHT);
        let raycast = world(Some(0.0), Some(wall_x));
        for _ in 0..240 {
            eye = integrate(&mut w, NavMode::Walk, eye, 1.0 / 60.0, &raycast);
        }
        assert!(eye.x <= wall_x - COLLISION_RADIUS + 0.05);
        assert!(
            eye.y > 1.0,
            "walking 45° into a wall must slide along it, y={}",
            eye.y
        );
        assert!(
            w.vel.x.abs() < w.speed(NavMode::Walk) * 0.25,
            "normal vel should be killed: {:?}",
            w.vel
        );
        assert!(
            w.vel.y > w.speed(NavMode::Walk) * 0.4,
            "tangential vel should remain: {:?}",
            w.vel
        );
    }

    #[test]
    fn step_up_point_two_four_accepted_point_two_six_rejected() {
        assert!((STEP_UP - 0.25).abs() < 1e-6);
        let mut w = WalkState::default();
        w.adopt(&cam_looking_x(), NavMode::Walk, 60.0);
        w.keys = K_FWD;
        w.speed_scale = 2.0;
        let mut eye = vec3(0.0, 0.0, EYE_HEIGHT);
        let ray = stepped(4.0, 0.24);
        for _ in 0..360 {
            eye = integrate(&mut w, NavMode::Walk, eye, 1.0 / 60.0, &ray);
        }
        assert!(
            eye.x > 4.5,
            "0.24 m step must be walkable, x={}",
            eye.x
        );
        assert!(
            (eye.z - (0.24 + EYE_HEIGHT)).abs() < 0.05,
            "eye should sit {EYE_HEIGHT} m over the 0.24 m step, z={}",
            eye.z
        );

        let mut w = WalkState::default();
        w.adopt(&cam_looking_x(), NavMode::Walk, 60.0);
        w.keys = K_FWD;
        w.speed_scale = 2.0;
        let mut eye = vec3(0.0, 0.0, EYE_HEIGHT);
        let ray = stepped(4.0, 0.26);
        for _ in 0..360 {
            eye = integrate(&mut w, NavMode::Walk, eye, 1.0 / 60.0, &ray);
            assert!(
                eye.x < 4.0 + 0.05,
                "0.26 m riser must not be crossed, x={}",
                eye.x
            );
        }
        assert!(
            eye.x > 3.0,
            "should walk up to the 0.26 m step, x={}",
            eye.x
        );
        assert!(
            (eye.z - EYE_HEIGHT).abs() < 0.08,
            "must stay on the low floor, z={}",
            eye.z
        );
    }

    #[test]
    fn door_passage_zero_point_seven() {
        let mut w = WalkState::default();
        w.adopt(&cam_looking_x(), NavMode::Walk, 60.0);
        w.keys = K_FWD;
        w.speed_scale = 2.0;
        let mut eye = vec3(0.0, 0.0, EYE_HEIGHT);
        // 0.7 m opening: walls at y = ±0.35.
        let ray = corridor(0.35);
        for _ in 0..240 {
            eye = integrate(&mut w, NavMode::Walk, eye, 1.0 / 60.0, &ray);
        }
        assert!(
            eye.x > 3.0,
            "a 0.7 m door must let a 0.3 m capsule through, x={}",
            eye.x
        );
        assert!(eye.y.abs() < 0.2, "should stay centred, y={}", eye.y);
    }

    #[test]
    fn d_strafes_to_the_right_of_the_facing() {
        // Facing +x with z up, the right-hand side is −y.
        let mut w = WalkState::default();
        w.adopt(&cam_looking_x(), NavMode::Walk, 60.0);
        w.keys = K_RIGHT;
        let mut eye = vec3(0.0, 0.0, 1.7);
        for _ in 0..30 {
            eye = integrate(&mut w, NavMode::Walk, eye, 1.0 / 60.0, &world(Some(0.0), None));
        }
        assert!(eye.y < -0.2, "D must move to the right (−y when facing +x), eye={:?}", eye);
        assert!(eye.x.abs() < 0.05);
    }

    #[test]
    fn no_floor_falls_walk_and_fly_ignores_floors() {
        let mut w = WalkState::default();
        w.adopt(&cam_looking_x(), NavMode::Walk, 60.0);
        w.keys = K_FWD;
        let mut eye = vec3(0.0, 0.0, 8.0);
        let start_z = eye.z;
        for _ in 0..90 {
            eye = integrate(&mut w, NavMode::Walk, eye, 1.0 / 60.0, &world(None, None));
        }
        assert!(eye.z < start_z - 1.0, "walk with no floor must fall, z={}", eye.z);
        assert!(
            (w.vel.z + TERMINAL_FALL).abs() < 0.2 || w.vel.z < -6.0,
            "should approach terminal 8 m/s, vz={}",
            w.vel.z
        );
        assert!(w.vel.z >= -TERMINAL_FALL - 1e-3);
        // Never an endless fall: once a floor was stood on, a long fall
        // returns there.
        let mut w = WalkState::default();
        w.adopt(&cam_looking_x(), NavMode::Walk, 60.0);
        let mut eye = vec3(0.0, 0.0, 1.7);
        for _ in 0..30 {
            eye = integrate(&mut w, NavMode::Walk, eye, 1.0 / 60.0, &world(Some(0.0), None));
        }
        let stood = eye;
        assert!(w.last_grounded.is_some(), "standing on a floor must be remembered");
        for _ in 0..(FALL_RESPAWN_S * 60.0) as usize + 5 {
            eye = integrate(&mut w, NavMode::Walk, eye, 1.0 / 60.0, &world(None, None));
        }
        assert!((eye - stood).length() < 0.5, "a long fall must respawn on the last floor, eye={:?}", eye);
        // …and again: with no floor anywhere the walker keeps coming back.
        for _ in 0..(FALL_RESPAWN_S * 60.0) as usize + 5 {
            eye = integrate(&mut w, NavMode::Walk, eye, 1.0 / 60.0, &world(None, None));
        }
        assert!((eye - stood).length() < 0.5, "the respawn must repeat, eye={:?}", eye);

        let mut f = WalkState::default();
        f.adopt(&cam_looking_x(), NavMode::Fly, 60.0);
        f.pitch = 0.0;
        f.pitch_tgt = 0.0;
        f.keys = K_FWD | K_UP;
        let mut eye = vec3(0.0, 0.0, 8.0);
        let start_z = eye.z;
        for _ in 0..60 {
            eye = integrate(&mut f, NavMode::Fly, eye, 1.0 / 60.0, &world(Some(0.0), None));
        }
        assert!(eye.z > start_z + 0.2, "fly Q/E must lift even over a floor");
        assert!(eye.x > 0.2, "fly WASD still moves");
        assert!(eye.is_finite());
    }

    #[test]
    fn analysed_one_door_house_starts_one_point_five_metres_outside() {
        let scene = makepad_fab_tour::synthetic::building(
            &makepad_fab_tour::synthetic::Plan {
                storeys: 1,
                // One unsplit room: the only Door is the exterior entrance.
                min_room_area: 10_000.0,
                ..Default::default()
            },
        );
        assert_eq!(
            scene
                .elements
                .iter()
                .filter(|element| element.class == makepad_fab_tour::TourClass::Door)
                .count(),
            1
        );
        let site = makepad_fab_tour::SiteAnalysis::analyse(
            &scene,
            &makepad_fab_tour::AnalysisConfig::default(),
        );
        let entrance = site.entrance.as_ref().expect("front door analysis");
        let pose = site.walk_entry_pose(EYE_HEIGHT, 1.5);
        let horizontal = vec3(
            pose.eye.x - entrance.center.x,
            pose.eye.y - entrance.center.y,
            0.0,
        );
        assert!(
            (horizontal.length() - 1.5).abs() < 0.02,
            "standoff={} pose={pose:?} entrance={entrance:?}",
            horizontal.length()
        );
        assert!(
            horizontal.normalize().dot(entrance.outward) > 0.999,
            "eye is not outside the analysed entrance"
        );
        assert!(
            pose.forward.dot(-entrance.outward) > 0.999,
            "walk camera does not face into the house"
        );
        assert!((pose.eye.z - EYE_HEIGHT).abs() < 0.02, "eye={:?}", pose.eye);
    }

    #[test]
    fn thousand_walk_frames_on_100k_triangles_stay_below_300_ms() {
        let scene = std::sync::Arc::new(Scene::from_model(
            crate::model::demo::synthetic_model(100_000, 1_000),
            &mut |_| {},
        ));
        assert!(
            scene.stats.triangles >= 90_000,
            "fixture has only {} triangles",
            scene.stats.triangles
        );
        let building = scene.bounds;
        let entry = makepad_fab_tour::WalkEntryPose {
            eye: vec3(0.0, 0.0, EYE_HEIGHT + 0.8),
            forward: vec3(1.0, 0.0, 0.0),
        };
        let cache = std::sync::Arc::new(WalkSceneAnalysis::for_nav_test(
            &scene, building, entry,
        ));
        let mut state = AppState::default();
        state.set_scene(scene);
        state.walk_analysis = Some(cache);
        state.walk_analysis_revision = state.scene_revision;
        state.view_mut().nav_mode = NavMode::Walk;
        state.view_mut().camera.eye = entry.eye;
        state.view_mut().camera.target = entry.eye + entry.forward * 4.0;
        let reset_camera = state.view().camera;

        let mut walk = WalkState::default();
        walk.adopt(&reset_camera, NavMode::Walk, 100.0);
        walk.keys = K_FWD;
        walk.settling = false;
        let started = std::time::Instant::now();
        for _ in 0..1_000 {
            std::hint::black_box(step(&mut walk, &mut state, 0, 1.0 / 120.0));
            // Keep all probes in the populated part of the BVH rather than
            // measuring fast misses after the camera walks out of the model.
            state.view_mut().camera = reset_camera;
        }
        let elapsed = started.elapsed();
        let per_frame_ms = elapsed.as_secs_f64() * 1_000.0 / 1_000.0;
        eprintln!("walk BVH: {per_frame_ms:.4} ms/frame ({elapsed:?} / 1000)");
        assert!(
            elapsed < std::time::Duration::from_millis(300),
            "walk navigation took {per_frame_ms:.3} ms/frame"
        );
    }

    #[test]
    fn hud_line_format() {
        let mut w = WalkState::default();
        w.adopt(&cam_looking_x(), NavMode::Walk, 84.0);
        let s = hud_line(&w, NavMode::Walk);
        assert!(s.starts_with("Walk · "), "{s}");
        assert!(s.contains("1.6 m"), "{s}"); // {:.1} of 1.62 m
        assert!(s.contains("m/s"), "{s}");
        assert!(s.contains("Esc to release"), "{s}");
    }

    #[test]
    fn head_bob_defaults_off() {
        assert!(!WalkState::default().head_bob);
        let mut w = WalkState::default();
        w.adopt(&cam_looking_x(), NavMode::Walk, 60.0);
        w.keys = K_FWD;
        w.head_bob = false;
        let mut eye = vec3(0.0, 0.0, EYE_HEIGHT);
        let raycast = world(Some(0.0), None);
        for _ in 0..30 {
            eye = integrate(&mut w, NavMode::Walk, eye, 1.0 / 60.0, &raycast);
        }
        assert!(bob_offset(&w).abs() < 1e-8);
        let _ = eye;
    }

    #[test]
    fn hundred_points_is_thirty_degrees() {
        let mut w = WalkState::default();
        w.yaw = 0.0;
        w.pitch = 0.0;
        w.yaw_tgt = 0.0;
        w.pitch_tgt = 0.0;
        w.look_sens = LOOK_SENS;
        w.look(100.0, 0.0);
        let deg = w.yaw_tgt.abs().to_degrees();
        assert!(
            (deg - 30.0).abs() < 0.05,
            "100 layout-point flick yaw {deg}° want 30° (LOOK_SENS={LOOK_SENS})"
        );
    }

    /// 10 m / 1.4 m/s = 7.142… s. Reach walk speed first so the number is
    /// the physical time, not the velocity-ramp constant.
    #[test]
    fn ten_metres_at_walk_speed_takes_7_1_seconds() {
        let mut w = WalkState::default();
        w.adopt(&cam_looking_x(), NavMode::Walk, 400.0);
        assert!(
            (w.speed(NavMode::Walk) - WALK_SPEED).abs() < 1e-5,
            "walk scaled to the 400 m bounds: {}",
            w.speed(NavMode::Walk)
        );
        w.keys = K_FWD;
        w.speed_scale = 1.0;
        w.head_bob = false;
        let raycast = world(Some(0.0), None);
        let mut eye = vec3(0.0, 0.0, EYE_HEIGHT);
        for _ in 0..120 {
            eye = integrate(&mut w, NavMode::Walk, eye, 1.0 / 60.0, &raycast);
        }
        let v = vec3(w.vel.x, w.vel.y, 0.0).length();
        assert!(
            (v - WALK_SPEED).abs() < 0.02,
            "not at walk speed before the 10 m: {v}"
        );
        let start = eye;
        let dt = 1.0 / 60.0;
        let mut t = 0.0;
        let mut horiz = 0.0;
        while horiz < 10.0 {
            eye = integrate(&mut w, NavMode::Walk, eye, dt, &raycast);
            t += dt;
            horiz = vec3(eye.x - start.x, eye.y - start.y, 0.0).length();
            assert!(t < 12.0, "never covered 10 m (t={t}, horiz={horiz})");
        }
        let want = 10.0 / WALK_SPEED;
        assert!(
            (t - want).abs() < 0.12,
            "10 m at {WALK_SPEED} m/s took {t:.3}s, want {want:.3}s (~7.1)"
        );
    }

    /// Eye 1.5 m in front of a 2.1 m opening. The lintel sits just above
    /// eye level on screen: above the horizon, not at the top of the frame.
    #[test]
    fn door_lintel_projects_just_above_eye_level() {
        const DOOR_H: f32 = 2.1;
        const DIST: f32 = 1.5;
        let mut cam = Camera::default();
        cam.eye = vec3(-DIST, 0.0, EYE_HEIGHT);
        cam.target = vec3(0.0, 0.0, EYE_HEIGHT);
        cam.up = WORLD_UP;
        cam.fov_y_deg = WALK_FOV_DEG;
        cam.ortho = false;
        let aspect = 16.0 / 9.0;

        let eye_level = cam.project(vec3(0.0, 0.0, EYE_HEIGHT), aspect).unwrap();
        assert!(
            eye_level.y.abs() < 1e-3,
            "eye-level point must sit on the screen centre, ndc.y={}",
            eye_level.y
        );

        let lintel = cam.project(vec3(0.0, 0.0, DOOR_H), aspect).unwrap();
        let above = DOOR_H - EYE_HEIGHT;
        let half_fov = (WALK_FOV_DEG.to_radians() * 0.5).max(1e-4);
        let expected = (above / DIST) / half_fov.tan();
        assert!(
            lintel.y > 0.0,
            "lintel must sit above screen centre, ndc={lintel:?}"
        );
        assert!(
            (lintel.y - expected).abs() < 0.02,
            "lintel ndc.y={} expected {expected} (above={above}m, fov={WALK_FOV_DEG}°)",
            lintel.y
        );
        assert!(
            lintel.y > 0.25 && lintel.y < 0.75,
            "lintel fraction of half-screen {} is not 'just above eye'",
            lintel.y
        );
    }

    #[test]
    fn jump_apex_matches_v_squared_over_two_g() {
        let mut w = WalkState::default();
        w.adopt(&cam_looking_x(), NavMode::Walk, 60.0);
        w.head_bob = false;
        w.keys = 0;
        let raycast = world(Some(0.0), None);
        let mut eye = vec3(0.0, 0.0, EYE_HEIGHT);
        for _ in 0..60 {
            eye = integrate(&mut w, NavMode::Walk, eye, 1.0 / 60.0, &raycast);
        }
        let floor_eye = eye.z;
        assert!(
            (floor_eye - EYE_HEIGHT).abs() < 0.02,
            "must stand on the floor before jumping, z={}",
            floor_eye
        );
        w.keys = K_JUMP;
        let mut apex = eye.z;
        let mut left_floor = false;
        for _ in 0..180 {
            eye = integrate(&mut w, NavMode::Walk, eye, 1.0 / 60.0, &raycast);
            if eye.z > apex {
                apex = eye.z;
            }
            if eye.z > floor_eye + 0.2 {
                left_floor = true;
            }
        }
        assert!(left_floor, "jump must leave the floor, apex={}", apex);
        let height = apex - floor_eye;
        let want = JUMP_SPEED * JUMP_SPEED / (2.0 * GRAVITY);
        assert!(
            (height - want).abs() < 0.12,
            "apex height {height:.3} m, want v²/2g={want:.3} m"
        );
    }
}
