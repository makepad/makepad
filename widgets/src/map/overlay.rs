//! Map overlay layer: route polyline (casing + fill, traveled portion
//! dimmed), drop markers and the current-position puck. Everything here is
//! immediate-mode `DrawVector` geometry rebuilt per frame in screen space —
//! route scale is a few hundred visible points, well within budget.

use crate::makepad_draw::vector::{LineCap, LineJoin};
use crate::makepad_draw::*;
use crate::DrawVector;

/// The "space warp" mode (close-3D): a unified fold + perspective camera.
///
/// The base renderer is ORTHOGRAPHIC (screen y = rel_y*cos(tilt) −
/// lift_px*sin(tilt): an axonometric camera pitched 90°−tilt below the
/// horizon, at infinite distance). This struct is that same camera pulled in
/// to a finite dolly distance D = 1/kappa (scale 1 at the pivot, ortho as
/// kappa→0), looking at a ground surface that FOLDS: beyond `start_px` the
/// ground curls up along a circle of radius `radius_px` until its tangent is
/// PERPENDICULAR to the view axis (cap angle = tilt), then continues straight.
/// Past the cap, z along the view axis is constant, so the risen far field
/// renders as an undistorted, uniform-scale, face-on flat map — near field
/// stays true perspective street view; the fold is the hinge between them.
///
/// One math, three implementations — this struct is the CPU twin of the
/// `space_warp`/`space_warp2` uniform branch in DrawMapVector's vertex fn
/// AND of the `warp_ground` fn in DrawRotatedText (labels are emitted
/// unwarped and fold per frame on the GPU); keep all three in LOCKSTEP or
/// CPU-projected overlays / GPU-folded labels detach from the warped tiles.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct SpaceWarp {
    /// Tween 0..1 (eased); 0 compiles to the exact identity path.
    pub amount: f64,
    /// Fold start r0: pre-tilt ground px up-screen from the pivot.
    pub start_px: f64,
    /// Curl radius R in pre-tilt ground px.
    pub radius_px: f64,
    /// cos(tilt) — doubles as cos of the bend cap.
    pub cos_t: f64,
    /// sin(tilt) — doubles as sin of the bend cap.
    pub sin_t: f64,
    /// Bend cap angle = tilt in radians: the wall stops curling exactly
    /// face-on to the camera (view axis pitch below horizon is 90°−tilt).
    pub cap: f64,
    /// Perspective strength 1/D in px⁻¹, ALREADY amount-scaled; 0 = ortho.
    pub kappa: f64,
}

/// The perspective divide's floor: `w = 1/max(1+κ·z, PERSP_FLOOR)`. Inside
/// the clamp the forward map is CONSTANT in z (geometry behind the eye stops
/// moving instead of blowing up / flipping), so it is not invertible there —
/// the inverse reports the far clamp rather than a bogus root. Unreachable
/// for on-screen points: at κ = 1/H the clamp starts ~0.9·H behind the eye.
const PERSP_FLOOR: f64 = 0.12;
/// Ground-distance clamp the inverse returns when a screen point sits at or
/// past a horizon (only possible at PARTIAL amount, where the wall still
/// converges; at amount 1 the wall is face-on and has no horizon at all).
const FAR_GROUND_PX: f64 = 1.0e7;

impl SpaceWarp {
    pub fn is_on(&self) -> bool {
        self.amount > 1e-4
    }

    /// Fold the ground surface: ground distance g (pre-tilt px ahead of the
    /// pivot) with height lift_px above it → (forward, up, applied via the
    /// LOCAL surface normal so wall buildings point out of the wall), already
    /// blended toward flat by `amount`.
    fn fold(&self, g: f64, lift_px: f64) -> (f64, f64) {
        let a = g - self.start_px;
        let (f, u, nx, ny) = if a > 0.0 {
            let r = self.radius_px.max(1.0);
            let th = (a / r).min(self.cap);
            let (sth, cth) = (th.sin(), th.cos());
            let mut f = self.start_px + r * sth;
            let mut u = r * (1.0 - cth);
            let e = a - r * self.cap;
            if e > 0.0 {
                // straight, face-on continuation: 1 ground px = 1 wall px
                f += e * self.cos_t;
                u += e * self.sin_t;
            }
            (f, u, -sth, cth)
        } else {
            (g, 0.0, 0.0, 1.0)
        };
        let (pf, pu) = (f + lift_px * nx, u + lift_px * ny);
        (
            g + (pf - g) * self.amount,
            lift_px + (pu - lift_px) * self.amount,
        )
    }

    /// The ON-path camera: pre-tilt rotated rel offset from the pivot
    /// (x lateral, y: up-screen = negative = forward) plus vertical lift in
    /// ground px → screen offset from the pivot. Callers keep the legacy
    /// ortho expression when `!is_on()` (byte-identical flat mode).
    pub fn project(&self, rel_x: f64, rel_y: f64, lift_px: f64) -> Vec2d {
        let (bf, bu) = self.fold(-rel_y, lift_px);
        // z along the view axis rel to the pivot plane; w = D/z, scale 1 at
        // the pivot. Floor keeps geometry behind the eye finite (off-screen
        // anyway — must not blow up or flip).
        let zrel = bf * self.sin_t - bu * self.cos_t;
        let w = 1.0 / self.persp_denom(zrel);
        dvec2(rel_x * w, -(bf * self.cos_t + bu * self.sin_t) * w)
    }

    /// `1/w`: the perspective divide's denominator at view-axis depth `z`.
    #[inline]
    fn persp_denom(&self, z: f64) -> f64 {
        (1.0 + self.kappa * z).max(PERSP_FLOOR)
    }

    /// The GROUND surface (lift 0) at ground distance `g`, in camera terms:
    /// `y` = up-screen distance before the perspective divide, `z` = depth
    /// along the view axis. Everything the inverse needs, read straight out
    /// of the forward `fold`, so the two can never drift apart.
    fn surface_yz(&self, g: f64) -> (f64, f64) {
        let (bf, bu) = self.fold(g, 0.0);
        (
            bf * self.cos_t + bu * self.sin_t,
            bf * self.sin_t - bu * self.cos_t,
        )
    }

    /// Up-screen offset (= −screen.y, the forward map's forward component)
    /// of the ground point at distance `g`. Monotone increasing in `g` for
    /// the shipped geometry law — that is what makes the piece selection
    /// below a simple compare against the two breakpoint values.
    fn surface_up_screen(&self, g: f64) -> f64 {
        let (y, z) = self.surface_yz(g);
        y / self.persp_denom(z)
    }

    /// EXACT inverse of `project` on the ground surface (lift 0): a screen
    /// offset from the pivot back to the pre-tilt rotated rel it came from
    /// (x lateral, y: up-screen = negative = forward). This is the one
    /// function every warp-aware pointer op goes through — tap, long press,
    /// grab-pan and zoom-about-cursor.
    ///
    /// `project` splits cleanly: the lateral term is `rel_x · w` and the
    /// forward term depends ONLY on the ground distance `g = −rel_y`. So the
    /// inverse is a 1-D solve for `g` from `q = −screen.y`, then one multiply
    /// to recover `rel_x`. Each forward piece is monotone and inverts in
    /// closed form; the perspective coupling `w = 1/(1+κz)` never needs
    /// iteration on its own because it enters LINEARLY once cross-multiplied:
    ///
    /// * **flat plane** (`g ≤ start_px`): `q·(1 + κ·g·sin_t) = g·cos_t` →
    ///   `g = q / (cos_t − q·κ·sin_t)`. Exact, no iteration.
    /// * **wall** (`g ≥ start_px + R·cap`): past the cap both fold outputs
    ///   are AFFINE in the wall distance `e`, so `y` and `z` are affine and
    ///   the same cross-multiplied linear solve applies. Exact. (At amount 1
    ///   the wall is face-on: `z` is constant, `w` is constant, and the wall
    ///   maps 1 ground px → `w` screen px — no horizon, so every screen row
    ///   has a finite ground answer.)
    /// * **arc** (`0 < θ < cap`): at amount 1 the fold is a circle, so with
    ///   `ψ = θ − tilt`, `y = P + R·sin ψ` and `z = Q + R·cos ψ`; the
    ///   equation becomes `sin ψ − qκ·cos ψ = C/R`, solved by harmonic
    ///   addition (one `asin`). BOTH asin branches are evaluated and the
    ///   better residual wins: at steep tilt `ψ+δ` runs just past −π/2 near
    ///   the fold start, where the principal branch is the wrong root.
    ///
    /// The ONLY iterated case is a PARTIAL amount on the arc (live during
    /// the 600 ms tween): the blend puts `(1−amount)·θ` next to `sin θ`,
    /// which is Kepler-shaped and has no closed form. There the two
    /// closed-form roots (flat and amount-1) seed a safeguarded Newton on
    /// `[0, cap]` — quadratic, 2-3 steps to 1e-13, bisection as the guard.
    pub fn unproject(&self, screen_rel: Vec2d) -> Vec2d {
        if !self.is_on() {
            // Legacy flat inverse (callers normally keep their own off-path
            // so amount 0 stays byte-identical to a never-warped session).
            return dvec2(screen_rel.x, screen_rel.y / self.cos_t.max(1e-3));
        }
        let g = self.ground_at_up_screen(-screen_rel.y);
        let (_, z) = self.surface_yz(g);
        dvec2(screen_rel.x * self.persp_denom(z), -g)
    }

    /// Ground distance whose surface point lands at up-screen offset `q`:
    /// pick the piece by the two breakpoint screen offsets, then solve it.
    fn ground_at_up_screen(&self, q: f64) -> f64 {
        let r = self.radius_px.max(1.0);
        let cap = self.cap.max(0.0);
        let g_arc = self.start_px; // flat → arc
        let g_wall = g_arc + r * cap; // arc → wall
        if q <= self.surface_up_screen(g_arc) {
            self.solve_flat(q)
        } else if cap <= 1e-9 || q >= self.surface_up_screen(g_wall) {
            g_wall + self.solve_wall(q, g_wall)
        } else {
            g_arc + r * self.solve_arc(q, r, cap)
        }
    }

    /// Flat piece, exact: `g = q / (cos_t − q·κ·sin_t)`.
    fn solve_flat(&self, q: f64) -> f64 {
        let den = self.cos_t - q * self.kappa * self.sin_t;
        if den <= 1e-9 {
            // At/past the un-folded horizon — off-screen by construction
            // (the fold starts well before it).
            return FAR_GROUND_PX;
        }
        q / den
    }

    /// Wall piece, exact: `y` and `z` are affine in the wall distance `e`,
    /// so cross-multiplying the perspective divide gives a linear equation.
    /// Returns `e ≥ 0` (ground px past the cap).
    fn solve_wall(&self, q: f64, g_wall: f64) -> f64 {
        let (y0, z0) = self.surface_yz(g_wall);
        let a = self.amount;
        // d/de of the blended fold outputs: bf' = (1−a) + a·cos_t,
        // bu' = a·sin_t → y' = (1−a)·cos_t + a, z' = (1−a)·sin_t.
        let y_e = (1.0 - a) * self.cos_t + a;
        let z_e = (1.0 - a) * self.sin_t;
        let den = q * self.kappa * z_e - y_e;
        if den >= -1e-12 {
            // Only reachable at partial amount, where the wall still
            // converges to a horizon; at amount 1 z_e = 0 and den = −1.
            return FAR_GROUND_PX;
        }
        ((y0 - q * (1.0 + self.kappa * z0)) / den).max(0.0)
    }

    /// Arc piece → bend angle θ ∈ [0, cap]. Closed form at amount 1;
    /// safeguarded Newton (seeded from the closed forms) while the tween is
    /// mid-flight, where the blend is Kepler-shaped.
    fn solve_arc(&self, q: f64, r: f64, cap: f64) -> f64 {
        let closed = self.arc_theta_closed(q, r, cap);
        if (self.amount - 1.0).abs() <= 1e-9 {
            return closed;
        }
        // Seed: blend the amount-0 root (pure flat: g = q/cos_t) with the
        // amount-1 root, exactly how the surface itself is blended.
        let flat = ((q / self.cos_t.max(1e-6)) - self.start_px) / r;
        let a = self.amount;
        let mut lo = 0.0f64;
        let mut hi = cap;
        let mut th = ((1.0 - a) * flat + a * closed).clamp(lo, hi);
        for _ in 0..40 {
            let (f, df) = self.arc_residual(q, r, th);
            // F has the sign of (screen(θ) − q) — the divide's denominator
            // is positive outside the clamp — so this keeps a true bracket.
            if f > 0.0 {
                hi = th;
            } else {
                lo = th;
            }
            let next = if df.abs() > 1e-12 {
                let n = th - f / df;
                if n > lo && n < hi {
                    n
                } else {
                    0.5 * (lo + hi)
                }
            } else {
                0.5 * (lo + hi)
            };
            let done = (next - th).abs() < 1e-13;
            th = next;
            if done {
                break;
            }
        }
        th
    }

    /// Amount-1 arc in closed form. `ψ = θ − tilt` turns the circle into
    /// `y = P + R·sin ψ`, `z = Q + R·cos ψ`; cross-multiplying the divide
    /// gives `sin ψ − qκ·cos ψ = C/R = M·sin(ψ+δ)`. Both asin branches are
    /// scored because ψ+δ leaves (−π/2, π/2) at steep tilt.
    fn arc_theta_closed(&self, q: f64, r: f64, cap: f64) -> f64 {
        let tilt = self.sin_t.atan2(self.cos_t);
        let p = self.start_px * self.cos_t + r * self.sin_t;
        let qq = self.start_px * self.sin_t - r * self.cos_t;
        let c = q * (1.0 + self.kappa * qq) - p;
        let qk = q * self.kappa;
        let m = (1.0 + qk * qk).sqrt();
        let delta = (-qk).atan2(1.0);
        let asin = (c / (r * m)).clamp(-1.0, 1.0).asin();
        let mut best = 0.0;
        let mut best_err = f64::INFINITY;
        for phi in [asin, -std::f64::consts::PI - asin] {
            let th = (phi - delta + tilt).clamp(0.0, cap);
            let err = (self.surface_up_screen(self.start_px + r * th) - q).abs();
            if err < best_err {
                best_err = err;
                best = th;
            }
        }
        best
    }

    /// Arc residual `F(θ) = y − q·(1 + κ·z)` and its derivative, on the
    /// BLENDED surface — the exact same expression `fold` builds, written
    /// in θ so Newton has an analytic slope.
    fn arc_residual(&self, q: f64, r: f64, th: f64) -> (f64, f64) {
        let a = self.amount;
        let (s, c) = (th.sin(), th.cos());
        let bf = self.start_px + r * ((1.0 - a) * th + a * s);
        let bu = a * r * (1.0 - c);
        let y = bf * self.cos_t + bu * self.sin_t;
        let z = bf * self.sin_t - bu * self.cos_t;
        let bf_d = r * ((1.0 - a) + a * c);
        let bu_d = a * r * s;
        let y_d = bf_d * self.cos_t + bu_d * self.sin_t;
        let z_d = bf_d * self.sin_t - bu_d * self.cos_t;
        (
            y - q * (1.0 + self.kappa * z),
            y_d - q * self.kappa * z_d,
        )
    }

    /// Re-project a point the label funnel already carried to ortho GROUND
    /// screen space (rot + tilt applied, lift NOT yet applied): recover the
    /// pre-tilt rel, then run the full camera with the point's lift.
    pub fn warp_screen_point(&self, p: Vec2d, pivot: Vec2d, lift_px: f64) -> Vec2d {
        if !self.is_on() {
            return dvec2(p.x, p.y - lift_px * self.sin_t);
        }
        let rel_x = p.x - pivot.x;
        let rel_y = (p.y - pivot.y) / self.cos_t.max(1e-6);
        pivot + self.project(rel_x, rel_y, lift_px)
    }

    /// Perspective factor w (screen scale) at an ortho GROUND screen point;
    /// 1 when off. Label lifts/badges scale by this so far-wall pins don't
    /// tower over their perspective-shrunken buildings.
    pub fn screen_w(&self, p: Vec2d, pivot: Vec2d) -> f64 {
        if !self.is_on() {
            return 1.0;
        }
        let rel_y = (p.y - pivot.y) / self.cos_t.max(1e-6);
        let (bf, bu) = self.fold(-rel_y, 0.0);
        let zrel = bf * self.sin_t - bu * self.cos_t;
        1.0 / self.persp_denom(zrel)
    }

    /// Tile-culling extents under the warp. `half_h_flat` is the flat-mode
    /// pre-tilt ground reach (screen_half/tilt_cos); returns (ground reach,
    /// lateral widen factor ≥1). The wall advances up-screen slower than the
    /// flat ortho compression at low tilt, so the fold can SEE FURTHER than
    /// the flat frustum — cull honestly or the wall runs out of city
    /// (perf-never-breaks-the-picture).
    pub fn cull_extents(&self, screen_half: f64, half_h_flat: f64) -> (f64, f64) {
        if !self.is_on() {
            return (half_h_flat, 1.0);
        }
        // End of the bend (lift 0, amount folded in):
        let r = self.radius_px.max(1.0);
        let g_cap = self.start_px + r * self.cap;
        let (f_cap, u_cap) = self.fold(g_cap, 0.0);
        let z_cap = f_cap * self.sin_t - u_cap * self.cos_t;
        let w_wall = 1.0 / self.persp_denom(z_cap);
        let y_cap = f_cap * self.cos_t + u_cap * self.sin_t;
        // On the wall screen-y advances 1:1·amount·w_wall per ground px
        // (blended toward the flat cos_t rate when amount < 1).
        let rate = (self.amount + (1.0 - self.amount) * self.cos_t) * w_wall;
        let need = screen_half - y_cap * w_wall;
        let reach = g_cap + (need / rate.max(1e-3)).max(0.0);
        (reach.max(half_h_flat), (1.0 / w_wall).max(1.0))
    }
}

/// Screen-space camera for one overlay frame; built by `MapView::draw_walk`
/// from the same numbers the tile pass uses.
pub struct OverlayCamera {
    /// Pixels per normalized-mercator unit at the current view zoom.
    pub world_size: f64,
    /// Screen offset: `screen = norm * world_size + offset` (before rotation).
    pub offset: Vec2d,
    pub rect: Rect,
    /// Ground meters per screen pixel at the view center latitude.
    pub meters_per_px: f64,
    /// (cos, sin) of the heading-up screen rotation; identity = north-up.
    pub rot: (f64, f64),
    pub rot_pivot: Vec2d,
    /// Map bearing pointing up, degrees (for billboard heading math).
    pub rotation_deg: f64,
    /// cos(tilt) of the 2.5D camera; 1.0 = top-down.
    pub tilt_cos: f64,
    /// The Inception fold, identity when off — every CPU ground projection
    /// funnels through it so overlays/markers/terrain track the GPU tiles.
    pub warp: SpaceWarp,
}

impl OverlayCamera {
    pub fn norm_to_screen(&self, p: Vec2d) -> Vec2d {
        self.norm_to_screen_with_rel(p).0
    }

    /// Screen position AND the pre-tilt, UN-warped ground rel-y — depth
    /// must stay a function of the original ground plane (the tile shader
    /// computes depth from unwarped `ground_rel_y`), so callers that build
    /// depth ladders take the second value instead of un-compressing the
    /// warped screen y.
    pub fn norm_to_screen_with_rel(&self, p: Vec2d) -> (Vec2d, f64) {
        let s = p * self.world_size + self.offset;
        if self.rot == (1.0, 0.0) && self.tilt_cos == 1.0 && !self.warp.is_on() {
            return (s, s.y - self.rot_pivot.y);
        }
        let rel = s - self.rot_pivot;
        let rotated = dvec2(
            rel.x * self.rot.0 - rel.y * self.rot.1,
            rel.x * self.rot.1 + rel.y * self.rot.0,
        );
        let screen = if self.warp.is_on() {
            self.rot_pivot + self.warp.project(rotated.x, rotated.y, 0.0)
        } else {
            self.rot_pivot + dvec2(rotated.x, rotated.y * self.tilt_cos)
        };
        (screen, rotated.y)
    }

    /// Ground point with a vertical lift (in GROUND px) through the warp
    /// camera — terrain/overlay callers use this when the warp is on so the
    /// lift rides the fold normal and the perspective divide; when it is
    /// off they keep their legacy straight-up `lift_m * ppm * sin(tilt)`
    /// screen offset (byte-identical flat path).
    /// INVERSE of `norm_to_screen` on the ground surface: an absolute screen
    /// point → the world-aligned (un-rotated) px offset of the ground point
    /// under it, relative to the view pivot. Every pointer op that has to
    /// turn a screen position back into a map position starts here; with the
    /// fold on it runs `SpaceWarp::unproject`, off it is the legacy
    /// `y / tilt_cos` divide, byte-for-byte.
    pub fn screen_to_world_rel(&self, abs: Vec2d) -> Vec2d {
        let s = abs - self.rot_pivot;
        let rotated = if self.warp.is_on() {
            self.warp.unproject(s)
        } else {
            dvec2(s.x, s.y / self.tilt_cos.max(1e-3))
        };
        // Un-rotate the heading-up screen rotation (transpose of the
        // forward rotation in norm_to_screen_with_rel).
        dvec2(
            rotated.x * self.rot.0 + rotated.y * self.rot.1,
            -rotated.x * self.rot.1 + rotated.y * self.rot.0,
        )
    }

    /// Absolute screen point → normalized mercator, the exact inverse of
    /// `norm_to_screen` (fold included).
    pub fn screen_to_norm(&self, abs: Vec2d) -> Vec2d {
        (self.rot_pivot + self.screen_to_world_rel(abs) - self.offset) / self.world_size
    }

    pub fn norm_to_screen_lifted(&self, p: Vec2d, lift_px: f64) -> (Vec2d, f64) {
        let s = p * self.world_size + self.offset;
        let rel = s - self.rot_pivot;
        let rotated = dvec2(
            rel.x * self.rot.0 - rel.y * self.rot.1,
            rel.x * self.rot.1 + rel.y * self.rot.0,
        );
        (
            self.rot_pivot + self.warp.project(rotated.x, rotated.y, lift_px),
            rotated.y,
        )
    }
}

/// Re-center for a zoom-about-cursor step: `anchor_rel` is the world-aligned
/// px offset of the ground point under the cursor
/// (`OverlayCamera::screen_to_world_rel`, evaluated at the OLD zoom) and the
/// result is the new center that keeps that point under the cursor.
///
/// This survives the fold unchanged: the fold's geometry keys off the view
/// RECT and the tilt only (r0, R and κ are all functions of the view height),
/// never off the zoom — so screen px ↔ ground px is a fixed map and one
/// anchor offset is valid at both world sizes. All the fold changes is WHICH
/// ground offset a cursor position means, which is exactly what
/// `screen_to_world_rel` now answers correctly.
pub fn zoom_anchor_center_norm(
    center_norm: Vec2d,
    anchor_rel: Vec2d,
    old_world_size: f64,
    new_world_size: f64,
) -> Vec2d {
    let anchor_world = center_norm * old_world_size + anchor_rel;
    let anchor_norm = anchor_world / old_world_size;
    (anchor_norm * new_world_size - anchor_rel) / new_world_size
}

#[derive(Clone, Debug)]
pub struct MapMarker {
    pub id: u64,
    pub lon: f64,
    pub lat: f64,
    /// Normalized mercator, cached at set time.
    pub pos_norm: Vec2d,
    pub color: Vec4f,
}

impl MapMarker {
    pub fn new(id: u64, lon: f64, lat: f64, color: Vec4f) -> Self {
        let pos_norm = super::geometry::lon_lat_to_normalized(lon, lat);
        Self {
            id,
            lon,
            lat,
            pos_norm,
            color,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct MapRouteOverlay {
    /// Normalized mercator polyline.
    pub points_norm: Vec<Vec2d>,
    /// Points before this index are drawn dimmed (already traveled).
    pub traveled_index: usize,
}

#[derive(Clone, Debug)]
pub struct MapPuck {
    pub lon: f64,
    pub lat: f64,
    pub pos_norm: Vec2d,
    /// Compass heading in degrees (0 = north, clockwise); draws the wedge.
    pub heading_deg: Option<f64>,
    pub accuracy_m: f64,
}

impl MapPuck {
    pub fn new(lon: f64, lat: f64, heading_deg: Option<f64>, accuracy_m: f64) -> Self {
        let pos_norm = super::geometry::lon_lat_to_normalized(lon, lat);
        Self {
            lon,
            lat,
            pos_norm,
            heading_deg,
            accuracy_m,
        }
    }
}

#[derive(Default)]
pub struct MapOverlayState {
    pub markers: Vec<MapMarker>,
    pub route: Option<MapRouteOverlay>,
    pub puck: Option<MapPuck>,
    /// shiny.md T5b: additive halo passes under the route stroke (no HDR,
    /// no bloom — premultiplied rgb with alpha 0 is pure additive over
    /// whatever is underneath). Stamped from the theme per frame.
    pub route_glow: bool,
    /// Scratch screen-space buffer reused across frames.
    scratch_screen: Vec<Vec2d>,
}

impl MapOverlayState {
    pub fn is_empty(&self) -> bool {
        self.markers.is_empty() && self.route.is_none() && self.puck.is_none()
    }

    /// Topmost marker whose pin head is within tap distance of `abs`.
    ///
    /// Warp-correct by construction, no inverse needed: each marker is
    /// FORWARD-projected through the same camera that drew it (fold and
    /// perspective divide included, `draw_marker` uses this very call), and
    /// the pin art is fixed screen px — so a fixed screen-px tap radius is
    /// the right test, on the wall as much as on the near ground.
    pub fn marker_at(&self, camera: &OverlayCamera, abs: Vec2d) -> Option<u64> {
        for marker in self.markers.iter().rev() {
            let p = camera.norm_to_screen(marker.pos_norm);
            let head = dvec2(p.x, p.y - PIN_HEAD_LIFT);
            if (abs - head).length() < 16.0 {
                return Some(marker.id);
            }
        }
        None
    }
}

const ROUTE_CASING: Vec4f = Vec4f {
    x: 0.06,
    y: 0.27,
    z: 0.55,
    w: 1.0,
};
const ROUTE_FILL: Vec4f = Vec4f {
    x: 0.20,
    y: 0.51,
    z: 0.95,
    w: 1.0,
};
/// Traveled portion: same hue, mostly transparent.
const ROUTE_DIM_ALPHA: f32 = 0.30;
const PIN_HEAD_LIFT: f64 = 15.0;

/// Draw the whole overlay. Order: route under markers under puck.
pub fn draw_map_overlay(
    cx: &mut Cx2d,
    dv: &mut DrawVector,
    camera: &OverlayCamera,
    state: &mut MapOverlayState,
) {
    if state.is_empty() {
        return;
    }
    let rect = camera.rect;
    // DrawVector geometry maps through the current turtle; pin one to our
    // rect (same pattern as PerfGraph) so paths land where we compute them.
    cx.begin_turtle(
        Walk {
            abs_pos: Some(rect.pos),
            width: Size::Fixed(rect.size.x),
            height: Size::Fixed(rect.size.y),
            margin: Inset::default(),
            metrics: Metrics::default(),
        },
        Layout {
            clip_x: true,
            clip_y: true,
            ..Layout::default()
        },
    );
    dv.begin();

    let route = state.route.take();
    if let Some(route) = &route {
        draw_route(dv, camera, route, state.route_glow, &mut state.scratch_screen);
    }
    state.route = route;

    for marker in &state.markers {
        draw_marker(dv, camera, marker);
    }
    if let Some(puck) = &state.puck {
        draw_puck(dv, camera, puck);
    }

    dv.end(cx);
    cx.end_turtle();
}

fn draw_route(
    dv: &mut DrawVector,
    camera: &OverlayCamera,
    route: &MapRouteOverlay,
    glow: bool,
    screen: &mut Vec<Vec2d>,
) {
    if route.points_norm.len() < 2 {
        return;
    }
    screen.clear();
    for p in &route.points_norm {
        screen.push(camera.norm_to_screen(*p));
    }
    let margin = 24.0;
    let min_x = camera.rect.pos.x - margin;
    let min_y = camera.rect.pos.y - margin;
    let max_x = camera.rect.pos.x + camera.rect.size.x + margin;
    let max_y = camera.rect.pos.y + camera.rect.size.y + margin;
    let seg_visible = |a: Vec2d, b: Vec2d| -> bool {
        !(a.x < min_x && b.x < min_x
            || a.x > max_x && b.x > max_x
            || a.y < min_y && b.y < min_y
            || a.y > max_y && b.y > max_y)
    };

    // Halo passes (widest first) then casing then fill, each split at the
    // traveled boundary so the behind-us part fades out. Halo colors are
    // premultiplied-additive (rgb energy, alpha 0): roads underneath
    // BRIGHTEN instead of being covered — the no-HDR glow trick.
    let mut passes: Vec<(f32, Vec4f)> = Vec::with_capacity(4);
    if glow {
        passes.push((
            26.0,
            Vec4f { x: ROUTE_FILL.x * 0.07, y: ROUTE_FILL.y * 0.07, z: ROUTE_FILL.z * 0.07, w: 0.0 },
        ));
        passes.push((
            14.0,
            Vec4f { x: ROUTE_FILL.x * 0.16, y: ROUTE_FILL.y * 0.16, z: ROUTE_FILL.z * 0.16, w: 0.0 },
        ));
    }
    passes.push((9.0, ROUTE_CASING));
    passes.push((5.5, ROUTE_FILL));
    let split = route.traveled_index.min(screen.len());
    for (width, color) in passes {
        for (range, alpha) in [
            (0..split.saturating_add(1).min(screen.len()), ROUTE_DIM_ALPHA),
            (split..screen.len(), 1.0),
        ] {
            if range.len() < 2 {
                continue;
            }
            // Additive halos (w = 0) carry their energy in rgb, so the
            // traveled-portion dim must scale rgb, not the no-op alpha.
            if color.w == 0.0 {
                dv.set_color(color.x * alpha, color.y * alpha, color.z * alpha, 0.0);
            } else {
                dv.set_color(color.x, color.y, color.z, color.w * alpha);
            }
            let mut pen_down = false;
            let mut last_drawn = dvec2(0.0, 0.0);
            let start = range.start;
            let end = range.end;
            for i in start..end - 1 {
                let a = screen[i];
                let b = screen[i + 1];
                // Decimate against the last DRAWN point so error stays
                // bounded (~1.5px) — neighbor-pairwise skipping compounded
                // and visibly reshaped the route when zoomed out.
                if pen_down && (b - last_drawn).length() < 1.5 && i + 2 < end {
                    continue;
                }
                if !seg_visible(last_drawn, b) && !seg_visible(a, b) {
                    if pen_down {
                        dv.stroke_opts(width, LineCap::Round, LineJoin::Round, 4.0, 1.0);
                        dv.clear();
                        pen_down = false;
                    }
                    continue;
                }
                if !pen_down {
                    dv.move_to(a.x as f32, a.y as f32);
                    pen_down = true;
                }
                dv.line_to(b.x as f32, b.y as f32);
                last_drawn = b;
            }
            if pen_down {
                dv.stroke_opts(width, LineCap::Round, LineJoin::Round, 4.0, 1.0);
                dv.clear();
            }
        }
    }

    // Destination dot at the very end of the line.
    if let Some(&end) = screen.last() {
        if end.x > min_x && end.x < max_x && end.y > min_y && end.y < max_y {
            dv.set_color(ROUTE_CASING.x, ROUTE_CASING.y, ROUTE_CASING.z, 1.0);
            dv.circle(end.x as f32, end.y as f32, 6.0);
            dv.fill();
            dv.set_color(1.0, 1.0, 1.0, 1.0);
            dv.circle(end.x as f32, end.y as f32, 2.6);
            dv.fill();
        }
    }
}

fn draw_marker(dv: &mut DrawVector, camera: &OverlayCamera, marker: &MapMarker) {
    let p = camera.norm_to_screen(marker.pos_norm);
    let margin = 30.0;
    if p.x < camera.rect.pos.x - margin
        || p.y < camera.rect.pos.y - margin
        || p.x > camera.rect.pos.x + camera.rect.size.x + margin
        || p.y > camera.rect.pos.y + camera.rect.size.y + margin
    {
        return;
    }
    let (x, y) = (p.x as f32, p.y as f32);
    let head_y = y - PIN_HEAD_LIFT as f32;
    let c = marker.color;
    // Soft ground shadow.
    dv.set_color(0.0, 0.0, 0.0, 0.18);
    dv.ellipse(x, y + 1.5, 5.0, 2.2);
    dv.fill();
    // Tail triangle + head disc read as one pin shape.
    dv.set_color(c.x, c.y, c.z, c.w);
    dv.move_to(x, y);
    dv.line_to(x - 7.2, head_y + 3.0);
    dv.line_to(x + 7.2, head_y + 3.0);
    dv.close();
    dv.fill();
    dv.circle(x, head_y, 8.6);
    dv.fill();
    // White pip.
    dv.set_color(1.0, 1.0, 1.0, 0.95);
    dv.circle(x, head_y, 3.4);
    dv.fill();
}

fn draw_puck(dv: &mut DrawVector, camera: &OverlayCamera, puck: &MapPuck) {
    let p = camera.norm_to_screen(puck.pos_norm);
    let margin = 60.0;
    if p.x < camera.rect.pos.x - margin
        || p.y < camera.rect.pos.y - margin
        || p.x > camera.rect.pos.x + camera.rect.size.x + margin
        || p.y > camera.rect.pos.y + camera.rect.size.y + margin
    {
        return;
    }
    let (x, y) = (p.x as f32, p.y as f32);

    // Accuracy circle in map space (scales with zoom).
    if puck.accuracy_m > 0.0 && camera.meters_per_px > 0.0 {
        let r = (puck.accuracy_m / camera.meters_per_px) as f32;
        let max_r = (camera.rect.size.x + camera.rect.size.y) as f32;
        if r > 12.0 && r < max_r {
            dv.set_color(0.20, 0.51, 0.95, 0.10);
            dv.circle(x, y, r);
            dv.fill();
            dv.set_color(0.20, 0.51, 0.95, 0.28);
            dv.circle(x, y, r);
            dv.stroke(1.0);
        }
    }

    // Heading wedge behind the dot; under a heading-up camera the wedge
    // shows the heading relative to the rotated map.
    if let Some(heading) = puck.heading_deg {
        let rad = (heading - camera.rotation_deg).to_radians();
        let (dir_x, dir_y) = (rad.sin() as f32, -(rad.cos()) as f32);
        let (side_x, side_y) = (-dir_y, dir_x);
        let tip = 20.0f32;
        let half = 8.5f32;
        dv.set_color(0.20, 0.51, 0.95, 0.55);
        dv.move_to(x + dir_x * tip, y + dir_y * tip);
        dv.line_to(x + side_x * half, y + side_y * half);
        dv.line_to(x - side_x * half, y - side_y * half);
        dv.close();
        dv.fill();
    }

    // White ring + blue dot.
    dv.set_color(1.0, 1.0, 1.0, 1.0);
    dv.circle(x, y, 9.0);
    dv.fill();
    dv.set_color(0.13, 0.45, 0.92, 1.0);
    dv.circle(x, y, 6.2);
    dv.fill();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped geometry law from `MapView::draw_walk`, so the tests
    /// exercise the warp the renderer actually builds.
    fn warp(h: f64, tilt_deg: f64, amount: f64) -> SpaceWarp {
        let tilt = tilt_deg.to_radians();
        SpaceWarp {
            amount,
            start_px: 0.18 * h / tilt.cos().max(0.087),
            radius_px: (0.30 * h).max(1.0),
            cos_t: tilt.cos(),
            sin_t: tilt.sin(),
            cap: tilt,
            kappa: amount / h,
        }
    }

    /// Ground distances at the two fold breakpoints: (fold start, cap).
    fn breakpoints(w: &SpaceWarp) -> (f64, f64) {
        let r = w.radius_px.max(1.0);
        (w.start_px, w.start_px + r * w.cap)
    }

    /// Round-trip a ground point: forward-project it, invert the screen
    /// point, and report (lateral error px, ground-distance error px).
    fn round_trip(w: &SpaceWarp, rel_x: f64, g: f64) -> (f64, f64) {
        let s = w.project(rel_x, -g, 0.0);
        let inv = w.unproject(s);
        ((inv.x - rel_x).abs(), (-inv.y - g).abs())
    }

    /// Screen-space round-trip: invert a screen offset, project it back.
    fn screen_round_trip(w: &SpaceWarp, s: Vec2d) -> f64 {
        let inv = w.unproject(s);
        let back = w.project(inv.x, inv.y, 0.0);
        (back - s).length()
    }

    struct Lcg(u64);
    impl Lcg {
        fn unit(&mut self) -> f64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((self.0 >> 11) as f64) / ((1u64 << 53) as f64)
        }
        fn range(&mut self, lo: f64, hi: f64) -> f64 {
            lo + (hi - lo) * self.unit()
        }
    }

    #[test]
    fn warp_inverse_round_trips_all_three_pieces() {
        let w = warp(800.0, 78.0, 1.0);
        let (g0, g1) = breakpoints(&w);
        let mut cases: Vec<(&str, f64)> = vec![
            ("flat behind pivot", -300.0),
            ("flat at pivot", 0.0),
            ("flat mid", g0 * 0.5),
            ("flat just inside", g0 - 1.0),
        ];
        for i in 1..8 {
            cases.push(("arc", g0 + (g1 - g0) * (i as f64) / 8.0));
        }
        for e in [1.0, 40.0, 400.0, 4000.0] {
            cases.push(("wall", g1 + e));
        }
        for (piece, g) in cases {
            for rel_x in [-500.0, 0.0, 137.0] {
                let (ex, eg) = round_trip(&w, rel_x, g);
                assert!(
                    ex < 1e-6 && eg < 1e-6,
                    "{piece} g={g} x={rel_x}: lateral {ex} ground {eg}"
                );
            }
        }
    }

    #[test]
    fn warp_inverse_round_trips_the_transition_bands() {
        for amount in [0.35, 0.7, 1.0] {
            let w = warp(800.0, 78.0, amount);
            let (g0, g1) = breakpoints(&w);
            for anchor in [g0, g1] {
                for d in [-1.0, -1e-3, -1e-9, 0.0, 1e-9, 1e-3, 1.0] {
                    let (ex, eg) = round_trip(&w, 90.0, anchor + d);
                    assert!(
                        ex < 1e-6 && eg < 1e-6,
                        "amount {amount} band {anchor}{d:+}: lateral {ex} ground {eg}"
                    );
                }
            }
        }
    }

    #[test]
    fn warp_inverse_is_identity_over_random_screen_points() {
        // Property sweep: every amount the tween passes through, every tilt
        // the close-3D regime allows, random points over a 1200x800 view.
        let mut rng = Lcg(0x5EED_1234_ABCD_0001);
        let mut worst = 0.0f64;
        let mut n = 0;
        for tilt in [35.0, 55.0, 70.0, 78.0, 85.0] {
            for step in 1..=20 {
                let amount = step as f64 / 20.0;
                let w = warp(800.0, tilt, amount);
                for _ in 0..200 {
                    let s = dvec2(rng.range(-600.0, 600.0), rng.range(-400.0, 400.0));
                    let err = screen_round_trip(&w, s);
                    worst = worst.max(err);
                    n += 1;
                    assert!(
                        err < 1e-3,
                        "tilt {tilt} amount {amount} at {s:?}: {err} px"
                    );
                }
            }
        }
        assert_eq!(n, 20_000);
        assert!(worst < 1e-3, "worst round-trip {worst} px");
    }

    #[test]
    fn warp_inverse_at_amount_zero_is_the_legacy_flat_divide() {
        // amount 0 is not "close to" the old camera, it IS it.
        let off = SpaceWarp {
            amount: 0.0,
            ..warp(800.0, 78.0, 1.0)
        };
        assert!(!off.is_on());
        for y in [-400.0, -37.0, 0.0, 91.0, 400.0] {
            let inv = off.unproject(dvec2(211.0, y));
            assert_eq!(inv.x.to_bits(), 211.0f64.to_bits());
            assert_eq!(inv.y.to_bits(), (y / off.cos_t).to_bits());
        }
    }

    fn test_camera(center_norm: Vec2d, world_size: f64, warp: SpaceWarp) -> OverlayCamera {
        let rect = Rect {
            pos: dvec2(10.0, 20.0),
            size: dvec2(1200.0, 800.0),
        };
        let rot = (0.9455185755993168, -0.3255681544571567); // -19 degrees
        OverlayCamera {
            world_size,
            offset: dvec2(
                rect.pos.x + rect.size.x * 0.5 - center_norm.x * world_size,
                rect.pos.y + rect.size.y * 0.5 - center_norm.y * world_size,
            ),
            rect,
            meters_per_px: 40_075_016.686 * 0.7 / world_size,
            rot,
            rot_pivot: rect.pos + rect.size * 0.5,
            rotation_deg: 19.0,
            tilt_cos: warp.cos_t,
            warp,
        }
    }

    #[test]
    fn camera_inverse_with_warp_off_matches_the_legacy_expression_bit_for_bit() {
        let flat = SpaceWarp {
            amount: 0.0,
            ..warp(800.0, 78.0, 1.0)
        };
        let world_size = 256.0 * 2f64.powi(17);
        let cam = test_camera(dvec2(0.5213, 0.3357), world_size, flat);
        for abs in [
            dvec2(11.0, 21.0),
            dvec2(610.0, 420.0),
            dvec2(1120.0, 133.0),
            dvec2(300.0, 790.0),
        ] {
            // Verbatim the pre-fix MapView::screen_to_lon_lat body.
            let pivot = cam.rot_pivot;
            let v = abs - pivot;
            let untilted = dvec2(v.x, v.y / cam.tilt_cos.max(1e-3));
            let legacy_rel = dvec2(
                untilted.x * cam.rot.0 + untilted.y * cam.rot.1,
                -untilted.x * cam.rot.1 + untilted.y * cam.rot.0,
            );
            let legacy = (legacy_rel + pivot - cam.offset) / cam.world_size;
            let got = cam.screen_to_norm(abs);
            assert_eq!(got.x.to_bits(), legacy.x.to_bits(), "x at {abs:?}");
            assert_eq!(got.y.to_bits(), legacy.y.to_bits(), "y at {abs:?}");
        }
    }

    #[test]
    fn camera_inverse_round_trips_through_rotation_and_the_fold() {
        let w = warp(800.0, 78.0, 1.0);
        let world_size = 256.0 * 2f64.powi(17);
        let cam = test_camera(dvec2(0.5213, 0.3357), world_size, w);
        let mut rng = Lcg(0xC0FFEE_1111);
        for _ in 0..500 {
            let abs = dvec2(rng.range(10.0, 1210.0), rng.range(20.0, 820.0));
            let norm = cam.screen_to_norm(abs);
            let back = cam.norm_to_screen(norm);
            assert!((back - abs).length() < 1e-3, "{abs:?} -> {back:?}");
        }
    }

    #[test]
    fn zoom_about_cursor_holds_a_wall_point_fixed() {
        let w = warp(800.0, 78.0, 1.0);
        let (_, g_cap) = breakpoints(&w);
        // Sanity: the probe really is up on the WALL, not the near ground.
        let probe_up = 300.0;
        assert!(w.surface_up_screen(g_cap) < probe_up);

        let mut center = dvec2(0.5213, 0.3357);
        let mut world_size = 256.0 * 2f64.powi(17);
        let cam0 = test_camera(center, world_size, w);
        let cursor = cam0.rot_pivot + dvec2(120.0, -probe_up);
        let anchored_norm = cam0.screen_to_norm(cursor);

        for _ in 0..3 {
            let cam = test_camera(center, world_size, w);
            let anchor_rel = cam.screen_to_world_rel(cursor);
            let new_world_size = world_size * 2f64.powf(0.5);
            center = zoom_anchor_center_norm(center, anchor_rel, world_size, new_world_size);
            world_size = new_world_size;
            let after = test_camera(center, world_size, w);
            let landed = after.norm_to_screen(anchored_norm);
            assert!(
                (landed - cursor).length() < 1e-3,
                "wall anchor slid to {landed:?} from {cursor:?}"
            );
        }
    }

    #[test]
    fn the_flat_inverse_is_the_bug_the_warp_inverse_closes() {
        // Same screen point, both inverses: on the wall the legacy divide is
        // off by hundreds of ground px (that is the reported drift), while
        // the warp inverse is exact.
        let w = warp(800.0, 78.0, 1.0);
        let (_, g_cap) = breakpoints(&w);
        let g_true = g_cap + 600.0;
        let s = w.project(0.0, -g_true, 0.0);
        let warped = -w.unproject(s).y;
        let legacy = -(s.y / w.cos_t);
        assert!((warped - g_true).abs() < 1e-6, "warp inverse {warped}");
        assert!(
            (legacy - g_true).abs() > 300.0,
            "legacy inverse should drift badly on the wall, got {legacy} vs {g_true}"
        );
    }

    #[test]
    fn marker_hit_testing_follows_the_fold_onto_the_wall() {
        let w = warp(800.0, 78.0, 1.0);
        let world_size = 256.0 * 2f64.powi(17);
        let cam = test_camera(dvec2(0.5213, 0.3357), world_size, w);
        // Place a marker at the map point under a wall-region screen point.
        let cursor = cam.rot_pivot + dvec2(-80.0, -300.0);
        let norm = cam.screen_to_norm(cursor);
        let mut state = MapOverlayState::default();
        state.markers.push(MapMarker {
            id: 77,
            lon: 0.0,
            lat: 0.0,
            pos_norm: norm,
            color: Vec4f { x: 1.0, y: 0.0, z: 0.0, w: 1.0 },
        });
        let head = cursor - dvec2(0.0, PIN_HEAD_LIFT);
        assert_eq!(state.marker_at(&cam, head), Some(77));
        assert_eq!(state.marker_at(&cam, head + dvec2(60.0, 0.0)), None);
    }
}
