//! Cascaded shadow-map fitting for the Realtime shadow tier.
//!
//! Realtime sun shadows are a classic CSM: per frame, before the main pass,
//! the sun's depth renders into `CSM_CASCADES` ortho cascades fitted to
//! slices of the VIEW frustum, and every material shader takes its sun
//! visibility from a PCF depth compare against them. One receive path for
//! everything — statics, dynamics and characters sample the same maps — so
//! no class of receiver can inherit another class's shadow bookkeeping
//! (the failure mode the ground-projected atlas path had for movers).
//!
//! This module is the pure half: given the camera slice and the sun, fit
//! the cascade matrices. The pass encoding lives in gpu_lightmap.rs (it
//! owns the pre-main-pass chain machinery), the sampling in shaders.rs.
//!
//! # Fitting rules (each earned by a classic CSM artifact)
//!
//! * Cascades fit the BOUNDING SPHERE of their frustum slice, not its
//!   tight OBB: a sphere's radius is invariant under camera rotation, so
//!   the shadow texel footprint never changes size as the camera pans —
//!   size-breathing edges shimmer.
//! * The cascade origin snaps to whole shadow texels in the sun's frame:
//!   sub-texel camera translation otherwise re-rasterizes every edge each
//!   frame — the crawling-edge artifact.
//! * The z range extends from the slice back to the SCENE bound toward the
//!   sun, so a tower far outside the slice still casts into it.

use makepad_draw::*;

/// Cascade count: three detail cascades over the configured range plus a
/// far cascade that reaches [`FAR_REACH`] times further, so a town's distant
/// houses still cast (the old three-cascade set ended in a hard cut at the
/// range). The fitting math is count-agnostic.
pub const CSM_CASCADES: usize = 4;

/// The cascade tiles sit in a 2x2 grid in one texture (tile i at column
/// i % 2, row i / 2, row 0 on top): a square target keeps the texture edge
/// at 2x the tile edge on every backend instead of 4x in a strip.
pub const CSM_GRID: usize = 2;

/// How far the last cascade reaches, as a multiple of the configured
/// `far_range`: 80 m of detail range reaches 320 m.
pub const FAR_REACH: f32 = 4.0;

/// Practical split scheme (Zhang et al.): split end i is
/// `LAMBDA * log_i + (1 - LAMBDA) * uniform_i` over `[SPLIT_NEAR, reach]`.
/// With four cascades over 0.5..320 m this puts the ends near 10 / 27 / 81 /
/// 320 m — the detail ladder the three-cascade set had, plus the far tile.
/// A lower lambda (0.75) would stretch cascade 0 to 22 m and blur the
/// contact shadows the player looks at most.
const SPLIT_LAMBDA: f32 = 0.9;
const SPLIT_NEAR: f32 = 0.5;

/// Cascade radii are rounded UP to this many steps per octave, so a slow
/// FOV / aspect / zoom change re-rasterizes the maps once per ~4% of size
/// instead of every frame (size-breathing texels shimmer).
const RADIUS_STEPS_PER_OCTAVE: f32 = 16.0;

/// The light basis follows the sun in 0.1-degree steps: a day cycle that
/// rotated the grid every frame would re-rasterize every edge every frame.
/// One step moves a 10 m caster's shadow by under 2 cm.
const SUN_STEP_RAD: f32 = 0.001_745;

/// Split ends as fractions of `reach` (the last is 1).
pub fn split_fractions(reach: f32) -> [f32; CSM_CASCADES] {
    let reach = reach.max(SPLIT_NEAR * 2.0);
    let mut out = [1.0; CSM_CASCADES];
    for (i, f) in out.iter_mut().enumerate().take(CSM_CASCADES - 1) {
        let t = (i + 1) as f32 / CSM_CASCADES as f32;
        let log = SPLIT_NEAR * (reach / SPLIT_NEAR).powf(t);
        let uni = SPLIT_NEAR + (reach - SPLIT_NEAR) * t;
        *f = (SPLIT_LAMBDA * log + (1.0 - SPLIT_LAMBDA) * uni) / reach;
    }
    out
}

/// Tile i's top-left corner in the texture's uv space (tiles are 0.5 wide).
pub fn csm_tile_origin(i: usize) -> (f32, f32) {
    let g = CSM_GRID as f32;
    ((i % CSM_GRID) as f32 / g, (i / CSM_GRID) as f32 / g)
}

/// Tile i as the depth shaders' clip-space mapping (sx, sy, ox, oy):
/// ndc (x, y) of the cascade lands at (x*sx + ox, y*sy + oy) of the target.
pub fn csm_tile_clip(i: usize) -> Vec4f {
    let g = CSM_GRID as f32;
    let (u, v) = csm_tile_origin(i);
    Vec4f {
        x: 1.0 / g,
        y: 1.0 / g,
        z: (2.0 * u + 1.0 / g) - 1.0,
        w: 1.0 - (2.0 * v + 1.0 / g),
    }
}

/// Round a radius up to the quantized ladder.
fn quantize_radius(r: f32) -> f32 {
    let steps = (r.max(0.5).log2() * RADIUS_STEPS_PER_OCTAVE).ceil();
    (steps / RADIUS_STEPS_PER_OCTAVE).exp2()
}

/// Snap the sun direction to the angular grid the cascades use.
fn quantize_sun(d: Vec3f) -> Vec3f {
    let d = d.normalize();
    let el = d.y.clamp(-1.0, 1.0).asin();
    let az = d.z.atan2(d.x);
    let q = |a: f32| (a / SUN_STEP_RAD).round() * SUN_STEP_RAD;
    let (el, az) = (q(el), q(az));
    v3(el.cos() * az.cos(), el.sin(), el.cos() * az.sin())
}

/// One fitted cascade: world -> shadow-map rows, in exactly the sun-depth
/// shader's convention — `dot(row.xyz, world) + row.w` gives ndc x / ndc y
/// / z01.
#[derive(Clone, Copy, Debug, Default)]
pub struct CsmCascade {
    pub rx: Vec4f,
    pub ry: Vec4f,
    pub rz: Vec4f,
    /// World units one shadow texel covers (receiver-side bias input).
    pub texel_world: f32,
    /// Depth bias in z01 units: three quarters of a texel plus 2 mm. The
    /// receiver shader adds a normal/slope offset at grazing angles; keeping
    /// this base small preserves crisp roof/wall contact without acne.
    pub bias01: f32,
    /// z01 units per world unit along the sun (1 / the z window's depth):
    /// the receiver converts its world-space slope bias with this.
    pub z_per_world: f32,
}

/// The frame's fitted cascade set.
#[derive(Clone, Copy, Debug, Default)]
pub struct CsmFrame {
    pub cascades: [CsmCascade; CSM_CASCADES],
    /// Each tile's depth generation (see [`CSM_DEPTH_GENS`]).
    pub generation: [u32; CSM_CASCADES],
    /// False for the off-tier binding (fallback texture, `csm_vis` = 1).
    pub on: bool,
}

/// The camera slice the cascades cover: eye position plus the four far-plane
/// corners (unprojected clip corners at z = far, which both depth
/// conventions put at +w). `None` at the call site (XR — the runtime owns
/// the eye matrices) falls back to view-independent rings around the eye.
///
/// `focus_distance` is the look-at depth along the view axis (orbit zoom,
/// metres). When it is > 0, cascade 0 is fitted to a tight slab around that
/// plane so one shadow texel stays about one screen pixel as the camera
/// zooms. 0 keeps the village-scale `[0, range]` ladder (first-person walk,
/// XR rings).
#[derive(Clone, Copy, Debug)]
pub struct CsmView {
    pub cam: Vec3f,
    pub far_corners: [Vec3f; 4],
    pub focus_distance: f32,
}

fn v3(x: f32, y: f32, z: f32) -> Vec3f {
    Vec3f { x, y, z }
}

/// Sphere center + radius for one frustum slice `[f0, f1]` (fractions of
/// the far corners): centroid of the 8 slice corners, radius to the
/// farthest. Rotation-invariant by construction (the slice is rigid), which
/// is the property the texel-snap needs.
fn slice_sphere(view: &CsmView, f0: f32, f1: f32) -> (Vec3f, f32) {
    let mut c = v3(0.0, 0.0, 0.0);
    let mut corners = [v3(0.0, 0.0, 0.0); 8];
    for (i, fc) in view.far_corners.iter().enumerate() {
        let d = *fc - view.cam;
        corners[i] = view.cam + d * f0;
        corners[i + 4] = view.cam + d * f1;
    }
    for p in &corners {
        c = c + *p;
    }
    c = c * (1.0 / 8.0);
    let mut r: f32 = 0.0;
    for p in &corners {
        r = r.max((*p - c).length());
    }
    (c, r.max(0.5))
}

/// View-axis far (camera → centre of the far plane). Focus depths are
/// measured along this axis (orbit `look.distance`), not along a corner ray.
fn view_far(view: &CsmView) -> f32 {
    let mut mid = v3(0.0, 0.0, 0.0);
    for fc in &view.far_corners {
        mid = mid + *fc;
    }
    mid = mid * 0.25;
    (mid - view.cam).length().max(1.0)
}

/// Cascade-0 depth window around `focus`, as fractions of the far plane.
///
/// Slack is half the focus so a subject that fills the view stays inside
/// the tight cascade, with a 0.9 m floor so a close zoom still covers a
/// fitted Kenney prop (~1.75 units).
fn focus_window(focus: f32, far: f32) -> (f32, f32) {
    let slack = (focus * 0.5).max(0.9);
    let z0 = (focus - slack).max(0.05);
    let z1 = (focus + slack).min(far);
    ((z0 / far).clamp(0.0, 1.0), (z1 / far).clamp(0.0, 1.0))
}

/// Fit one cascade around a sphere: ortho extents = radius, origin snapped
/// to the texel grid in the sun's frame, z reaching back to the scene
/// bounds toward the sun.
fn fit_cascade(
    center: Vec3f,
    radius: f32,
    sun_dir: Vec3f,
    scene_min: Vec3f,
    scene_max: Vec3f,
    res: f32,
) -> CsmCascade {
    // fwd = the direction sunlight TRAVELS (away from the sun) — the same
    // frame fit_sun_cam builds for the bake.
    let fwd = (sun_dir * -1.0).normalize();
    let up_hint = if fwd.y.abs() > 0.99 {
        v3(1.0, 0.0, 0.0)
    } else {
        v3(0.0, 1.0, 0.0)
    };
    let right = Vec3f::cross(up_hint, fwd).normalize();
    let up = Vec3f::cross(fwd, right);
    // About one texel of pad so the PCF kernel never reads past the fitted
    // edge. The snap quantum must be the FINAL map texel (2*ex/res, pad
    // included) — snapping by any other stride leaves sub-texel phase
    // drift, exactly the shimmer the snap exists to kill.
    let radius = quantize_radius(radius);
    let ex = radius * (1.0 + 2.0 / res);
    let texel = 2.0 * ex / res;
    // Texel snap: quantize the center's right/up coordinates.
    let snap = |v: f32| (v / texel).floor() * texel;
    let cr = snap(right.dot(center));
    let cu = snap(up.dot(center));
    let cf = fwd.dot(center);
    let c = right * cr + up * cu + fwd * cf;
    // z window: from the whole scene toward the sun (a caster far outside
    // the slice still shadows into it) to the slice's far side.
    let mut z_min = cf - radius;
    for i in 0..8 {
        let p = v3(
            if i & 1 == 0 { scene_min.x } else { scene_max.x },
            if i & 2 == 0 { scene_min.y } else { scene_max.y },
            if i & 4 == 0 { scene_min.z } else { scene_max.z },
        );
        z_min = z_min.min(fwd.dot(p));
    }
    let z0 = z_min - 0.5;
    let z1 = cf + radius + 0.5;
    let zr = (z1 - z0).max(0.01);
    let bias_world = texel * 0.75 + 0.002;
    CsmCascade {
        rx: Vec4f {
            x: right.x / ex,
            y: right.y / ex,
            z: right.z / ex,
            w: -right.dot(c) / ex,
        },
        ry: Vec4f {
            x: up.x / ex,
            y: up.y / ex,
            z: up.z / ex,
            w: -up.dot(c) / ex,
        },
        rz: Vec4f {
            x: fwd.x / zr,
            y: fwd.y / zr,
            z: fwd.z / zr,
            // `z0` and `z1` already are absolute light-space world
            // coordinates. Subtracting the snapped cascade centre here as
            // well shifts the depth interval whenever the camera orbits;
            // casters then clip while receivers still address the tile.
            w: -z0 / zr,
        },
        texel_world: texel,
        bias01: bias_world / zr,
        z_per_world: 1.0 / zr,
    }
}

/// Fit the frame's cascades. `range` is the detail range (the third
/// cascade ends near it); the far cascade reaches `range * FAR_REACH`.
/// `res` is one cascade's edge in texels.
///
/// With a view: slices of the camera frustum. Without one (XR — the eye
/// matrices belong to the runtime): view-independent rings around `eye`,
/// radii proportional to the same split ladder, so a headset still gets
/// nested cascades that follow the player.
pub fn fit_cascades(
    view: Option<&CsmView>,
    eye: Vec3f,
    sun_dir: Vec3f,
    scene_min: Vec3f,
    scene_max: Vec3f,
    range: f32,
    res: f32,
) -> CsmFrame {
    fit_cascades_reach(view, eye, sun_dir, scene_min, scene_max, range, 0.0, res)
}

/// [`fit_cascades`] with the FAR cascade stretched to `far_reach` metres
/// (0 or anything shorter than `range * FAR_REACH` = the default). The three
/// detail cascades keep their ladder; only the last tile reaches further —
/// what a city skyline needs, where towers kilometres away should still
/// stand in their own shadows.
#[allow(clippy::too_many_arguments)]
pub fn fit_cascades_reach(
    view: Option<&CsmView>,
    eye: Vec3f,
    sun_dir: Vec3f,
    scene_min: Vec3f,
    scene_max: Vec3f,
    range: f32,
    far_reach: f32,
    res: f32,
) -> CsmFrame {
    let mut frame = CsmFrame::default();
    let sun_dir = quantize_sun(sun_dir);
    let base = range * FAR_REACH;
    let reach = if far_reach.is_finite() { far_reach.max(base) } else { base };
    let mut splits = split_fractions(base);
    if reach > base {
        for f in splits.iter_mut().take(CSM_CASCADES - 1) { *f *= base / reach; }
        splits[CSM_CASCADES - 1] = 1.0;
    }
    // The slice's corner rays are scaled so fractions measure `reach`
    // rather than the projection's own far plane.
    let corner_scale = |view: &CsmView, d: f32| {
        let mut m = 0.0f32;
        for fc in &view.far_corners {
            m = m.max((*fc - view.cam).length());
        }
        (d / m.max(1.0)).min(1.0)
    };
    match view {
        Some(view) if view.focus_distance > 0.0 => {
            // Pixel-stable orbit: cascade 0 is a tight slab around the
            // look-at so its texel footprint tracks the screen as the
            // camera zooms. Cascade 1 is a 2x nest; cascade 2 keeps the
            // configured reach for ground that is still in frame and the
            // far cascade the extended one.
            let far = view_far(view);
            let (lo, hi) = focus_window(view.focus_distance, far);
            let mid = (lo + hi) * 0.5;
            let half = (hi - lo) * 0.5;
            let windows = [
                (lo, hi),
                ((mid - 2.0 * half).max(0.0), (mid + 2.0 * half).min(1.0)),
                (0.0, corner_scale(view, range)),
                (0.0, corner_scale(view, reach)),
            ];
            for (i, (f0, f1)) in windows.iter().enumerate() {
                let (c, r) = slice_sphere(view, *f0, *f1);
                frame.cascades[i] = fit_cascade(c, r, sun_dir, scene_min, scene_max, res);
            }
        }
        Some(view) => {
            // Split distances are anchored to `reach`, then clamped to the
            // view's own far plane (a short far plane collapses the outer
            // cascades rather than compressing the near ladder).
            let mut f0 = 0.0;
            for (i, f1) in splits.iter().enumerate() {
                let (a, b) = (corner_scale(view, f0 * reach), corner_scale(view, f1 * reach));
                let (c, r) = slice_sphere(view, a, b);
                frame.cascades[i] = fit_cascade(c, r, sun_dir, scene_min, scene_max, res);
                f0 = *f1;
            }
        }
        None => {
            for (i, f1) in splits.iter().enumerate() {
                let r = reach * f1 * 0.5;
                frame.cascades[i] = fit_cascade(eye, r, sun_dir, scene_min, scene_max, res);
            }
        }
    }
    frame
}

/// Depth "generations" per tile. The platform clears a pass's depth only
/// as a whole and always tests LessEqual, so a tile that is re-rendered
/// while its neighbours are kept (staggered cascades) cannot be cleared on
/// its own. Instead each re-render of a tile writes into the next LOWER
/// slice of the depth range: generation g maps z01 to
/// `[(G-1-g)/G, (G-g)/G]`, and a tile-sized quad at the slice's far end
/// (which every older generation's depth is >= to) clears it. After G
/// renders of any tile the whole target is cleared and all tiles restart at
/// generation 0. 32-bit float depth keeps ~20 bits per slice.
pub const CSM_DEPTH_GENS: u32 = 16;

/// Re-render period in frames per cascade: the two near cascades every
/// frame, cascade 2 every 2nd, the far cascade every 4th (on a phase that
/// avoids cascade 2's frames). A kept tile keeps the matrices it was
/// rendered with, so a receiver always compares against a consistent map;
/// only movers' shadows in the far tiles lag by up to 3 frames.
const CSM_PERIOD: [u64; CSM_CASCADES] = [1, 1, 2, 4];
const CSM_PHASE: [u64; CSM_CASCADES] = [0, 0, 0, 1];

/// Clip-space z window of a depth generation: `z_clip = z01 * scale + offset`.
pub fn depth_generation_window(generation: u32) -> (f32, f32) {
    let g = CSM_DEPTH_GENS as f32;
    (1.0 / g, (g - 1.0 - generation.min(CSM_DEPTH_GENS - 1) as f32) / g)
}

/// Which tiles a frame re-renders and in which depth generation.
#[derive(Clone, Copy, Debug, Default)]
pub struct CsmSchedule {
    pub generation: [u32; CSM_CASCADES],
    valid: [bool; CSM_CASCADES],
}

impl CsmSchedule {
    /// Forget every tile: the next frame clears the whole target.
    pub fn invalidate(&mut self) {
        self.valid = [false; CSM_CASCADES];
    }

    /// Plan frame `frame_no`: returns (clear the whole target, tiles to
    /// re-render). `stagger` false re-renders every tile every frame.
    pub fn plan(&mut self, frame_no: u64, stagger: bool) -> (bool, [bool; CSM_CASCADES]) {
        let mut due = [false; CSM_CASCADES];
        for i in 0..CSM_CASCADES {
            due[i] = !stagger || !self.valid[i] || frame_no % CSM_PERIOD[i] == CSM_PHASE[i];
        }
        let full = self.valid.iter().any(|v| !v)
            || (0..CSM_CASCADES).any(|i| due[i] && self.generation[i] + 1 >= CSM_DEPTH_GENS);
        if full {
            self.generation = [0; CSM_CASCADES];
            self.valid = [true; CSM_CASCADES];
            return (true, [true; CSM_CASCADES]);
        }
        for i in 0..CSM_CASCADES {
            if due[i] {
                self.generation[i] += 1;
            }
        }
        (false, due)
    }
}

/// Can a caster with this world AABB put depth into cascade `c`'s tile?
/// Its light-space box must overlap the tile's square and reach the z
/// window: a caster farther from the sun than the slice cannot shadow it,
/// and one outside the square projects outside the tile (ortho along z).
pub fn cascade_overlaps(c: &CsmCascade, min: Vec3f, max: Vec3f) -> bool {
    let center = (min + max) * 0.5;
    let half = (max - min) * 0.5;
    let span = |r: Vec4f| {
        let at = r.x * center.x + r.y * center.y + r.z * center.z + r.w;
        let ext = r.x.abs() * half.x + r.y.abs() * half.y + r.z.abs() * half.z;
        (at - ext, at + ext)
    };
    let (x0, x1) = span(c.rx);
    let (y0, y1) = span(c.ry);
    let (z0, z1) = span(c.rz);
    x0 <= 1.0 && x1 >= -1.0 && y0 <= 1.0 && y1 >= -1.0 && z0 <= 1.0 && z1 >= 0.0
}

/// Project a world point through a cascade: (ndc_x, ndc_y, z01).
pub fn cascade_project(c: &CsmCascade, p: Vec3f) -> Vec3f {
    v3(
        c.rx.x * p.x + c.rx.y * p.y + c.rx.z * p.z + c.rx.w,
        c.ry.x * p.x + c.ry.y * p.y + c.ry.z * p.z + c.ry.w,
        c.rz.x * p.x + c.rz.y * p.y + c.rz.z * p.z + c.rz.w,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_view() -> CsmView {
        // A camera at y=1.7 looking down +z with a ~60 degree fov, far 100.
        let cam = v3(3.0, 1.7, -5.0);
        CsmView {
            cam,
            far_corners: [
                cam + v3(-58.0, -40.0, 100.0),
                cam + v3(58.0, -40.0, 100.0),
                cam + v3(-58.0, 40.0, 100.0),
                cam + v3(58.0, 40.0, 100.0),
            ],
            focus_distance: 0.0,
        }
    }

    fn sun() -> Vec3f {
        v3(0.55, 0.62, 0.56).normalize()
    }

    /// The fitted z interval is expressed in absolute light-space world
    /// coordinates. Projecting it must therefore subtract `z0` exactly
    /// once; coupling the offset to the cascade centre makes the interval
    /// slide as an orbit camera rotates and eventually clips whole casters.
    #[test]
    fn z_row_maps_the_absolute_fitted_interval() {
        let center = v3(37.0, 4.0, -23.0);
        let radius = 7.0;
        let scene_min = v3(-60.0, -2.0, -60.0);
        let scene_max = v3(60.0, 35.0, 60.0);
        let sun_dir = sun();
        let c = fit_cascade(
            center,
            radius,
            sun_dir,
            scene_min,
            scene_max,
            2048.0,
        );
        let fwd = (sun_dir * -1.0).normalize();
        let cf = fwd.dot(center);
        let radius = quantize_radius(radius);
        let mut z_min = cf - radius;
        for i in 0..8 {
            let p = v3(
                if i & 1 == 0 { scene_min.x } else { scene_max.x },
                if i & 2 == 0 { scene_min.y } else { scene_max.y },
                if i & 4 == 0 { scene_min.z } else { scene_max.z },
            );
            z_min = z_min.min(fwd.dot(p));
        }
        let z0 = z_min - 0.5;
        let z1 = cf + radius + 0.5;
        let expected = (cf - z0) / (z1 - z0);
        let actual = cascade_project(&c, center).z;
        assert!(
            (actual - expected).abs() < 1.0e-6,
            "cascade z offset moved with its centre: actual {actual}, expected {expected}"
        );
    }

    /// The one shared csm_vis (shaders/csm.rs, spread by every lit family)
    /// must fall through when a point is outside a cascade's DEPTH interval,
    /// not merely its xy square. Sampling a cleared tile with ref01 > 1
    /// paints the uncovered footprint solid black—the 90-degree-orbit
    /// cascade hole.
    #[test]
    fn shader_selection_requires_full_xyz_coverage() {
        let src = crate::shaders::SHADER_SOURCE;
        assert_eq!(src.matches("csm_vis: fn").count(), 1, "one shared receive path");
        assert_eq!(src.matches("..mod.draw.SunCascades").count(), 4, "every lit family spreads it");
        assert_eq!(
            src.matches("q.z < 0.0 || q.z > 1.0").count(),
            1,
            "the one coverage test must reject uncovered z"
        );
        assert!(src.contains("self.csm_inside(q, 0.99) < 0.5"), "selection walks the cascades by full XYZ coverage");
    }

    /// Every slice corner must land inside its cascade's ndc square and z
    /// window — a slice the cascade does not cover shows as a hole in the
    /// shadows at that distance.
    #[test]
    fn each_cascade_contains_its_frustum_slice() {
        let view = test_view();
        let frame = fit_cascades(
            Some(&view),
            view.cam,
            sun(),
            v3(-60.0, 0.0, -60.0),
            v3(60.0, 12.0, 60.0),
            80.0,
            2048.0,
        );
        let reach = 80.0 * FAR_REACH;
        let far = view
            .far_corners
            .iter()
            .map(|fc| (*fc - view.cam).length())
            .fold(1.0f32, f32::max);
        let mut f0 = 0.0;
        for (i, f1) in split_fractions(reach).iter().enumerate() {
            for fc in &view.far_corners {
                let d = *fc - view.cam;
                for f in [f0, *f1] {
                    let p = view.cam + d * (f * reach / far).min(1.0);
                    let n = cascade_project(&frame.cascades[i], p);
                    assert!(
                        n.x.abs() <= 1.0 && n.y.abs() <= 1.0,
                        "cascade {i} misses slice corner: {n:?}"
                    );
                    assert!(
                        (-0.01..=1.01).contains(&n.z),
                        "cascade {i} z window misses slice corner: {n:?}"
                    );
                }
            }
            f0 = *f1;
        }
    }

    /// Texel snapping's real guarantee: as the camera translates, a FIXED
    /// world point's position on the shadow map moves by WHOLE texels only
    /// (the map's world-space grid phase never drifts) — sub-texel phase
    /// drift is what re-rasterizes every edge each frame and makes them
    /// crawl. The origin itself may legitimately step, so comparing rows
    /// directly over-asserts.
    #[test]
    fn texel_snap_keeps_the_grid_phase_through_camera_motion() {
        let view = test_view();
        let fit = |cam_off: Vec3f| {
            let moved = CsmView {
                cam: view.cam + cam_off,
                far_corners: [
                    view.far_corners[0] + cam_off,
                    view.far_corners[1] + cam_off,
                    view.far_corners[2] + cam_off,
                    view.far_corners[3] + cam_off,
                ],
                focus_distance: 0.0,
            };
            fit_cascades(
                Some(&moved),
                moved.cam,
                sun(),
                v3(-60.0, 0.0, -60.0),
                v3(60.0, 12.0, 60.0),
                80.0,
                2048.0,
            )
        };
        let res = 2048.0;
        let probe = v3(1.2, 0.3, 4.5);
        let a = fit(v3(0.0, 0.0, 0.0));
        for off in [
            v3(0.001, 0.0, 0.001),
            v3(0.4, 0.0, -0.7),
            v3(-1.3, 0.2, 2.9),
        ] {
            let b = fit(off);
            for i in 0..CSM_CASCADES {
                let (ca, cb) = (&a.cascades[i], &b.cascades[i]);
                // Same extents (the slice is rigid under translation)...
                assert!(
                    (ca.texel_world - cb.texel_world).abs() < 1.0e-4,
                    "cascade {i} breathed under camera translation"
                );
                // ...and integer-texel phase for a fixed world point.
                let pa = cascade_project(ca, probe);
                let pb = cascade_project(cb, probe);
                for (u, v) in [(pa.x, pb.x), (pa.y, pb.y)] {
                    let du = (u - v) * 0.5 * res; // ndc -> texels
                    assert!(
                        (du - du.round()).abs() < 0.05,
                        "cascade {i} drifted {du} texels (fraction {})",
                        du - du.round()
                    );
                }
            }
        }
    }

    /// A caster at the scene bound toward the sun must land inside the z
    /// window (z01 >= 0), or tall off-slice geometry loses its shadow.
    #[test]
    fn z_window_reaches_the_scene_bound_toward_the_sun() {
        let view = test_view();
        let s_min = v3(-60.0, 0.0, -60.0);
        let s_max = v3(60.0, 40.0, 60.0);
        let frame = fit_cascades(Some(&view), view.cam, sun(), s_min, s_max, 80.0, 2048.0);
        // The scene corner nearest the sun.
        let sun_most = v3(s_max.x, s_max.y, s_max.z);
        for (i, c) in frame.cascades.iter().enumerate() {
            let n = cascade_project(c, sun_most);
            assert!(n.z >= 0.0, "cascade {i} clips the sun-most scene corner");
        }
    }

    /// A focused orbit camera must put the look-at plane inside cascade 0
    /// and shrink that cascade's texel when the camera zooms in, so shadow
    /// resolution stays roughly constant in pixel space instead of blowing
    /// up into unfiltered blocks.
    #[test]
    fn focus_tightens_cascade0_and_tracks_zoom() {
        let base = test_view();
        let scene_min = v3(-60.0, 0.0, -60.0);
        let scene_max = v3(60.0, 12.0, 60.0);
        let fit = |focus: f32| {
            let view = CsmView {
                focus_distance: focus,
                ..base
            };
            fit_cascades(
                Some(&view),
                view.cam,
                sun(),
                scene_min,
                scene_max,
                80.0,
                2048.0,
            )
        };
        // Mid-plane of the default test view is ~100 units out; pick a
        // preview-scale focus well inside that.
        let wide = fit(4.2);
        let tight = fit(1.0);
        let axis = {
            let mut mid = v3(0.0, 0.0, 0.0);
            for fc in &base.far_corners {
                mid = mid + *fc;
            }
            (mid * 0.25 - base.cam).normalize()
        };
        let probe = |focus: f32| base.cam + axis * focus;
        for focus in [4.2, 1.0] {
            let frame = fit(focus);
            let n = cascade_project(&frame.cascades[0], probe(focus));
            assert!(
                n.x.abs() <= 1.0 && n.y.abs() <= 1.0 && (0.0..=1.0).contains(&n.z),
                "cascade 0 must cover the look-at at focus {focus}: {n:?}"
            );
        }
        assert!(
            tight.cascades[0].texel_world < wide.cascades[0].texel_world * 0.55,
            "zooming in 4.2 → 1.0 must shrink cascade 0 texels (wide {}, tight {})",
            wide.cascades[0].texel_world,
            tight.cascades[0].texel_world
        );
        // Screen-space stability: texel / focus is the pixel footprint of
        // one shadow texel at the look-at. It must stay in the same band
        // across a 4× zoom (not grow like the unfocused 80 m ladder).
        let wide_px = wide.cascades[0].texel_world / 4.2;
        let tight_px = tight.cascades[0].texel_world / 1.0;
        let ratio = tight_px / wide_px;
        assert!(
            (0.4..=2.5).contains(&ratio),
            "pixel-space texel drifted under zoom: wide {wide_px}, tight {tight_px}, ratio {ratio}"
        );
        // Unfocused fitting must NOT shrink with a focus we didn't set —
        // the village ladder is independent of look-at depth.
        let unfocused = fit_cascades(
            Some(&base),
            base.cam,
            sun(),
            scene_min,
            scene_max,
            80.0,
            2048.0,
        );
        assert!(
            wide.cascades[0].texel_world < unfocused.cascades[0].texel_world,
            "focused cascade 0 ({}) must be finer than village cascade 0 ({})",
            wide.cascades[0].texel_world,
            unfocused.cascades[0].texel_world
        );
    }

    /// The ring fallback (no view) still produces nested cascades centered
    /// on the eye.
    #[test]
    fn ring_fallback_nests_around_the_eye() {
        let eye = v3(5.0, 1.6, 7.0);
        let frame = fit_cascades(
            None,
            eye,
            sun(),
            v3(-60.0, 0.0, -60.0),
            v3(60.0, 12.0, 60.0),
            80.0,
            2048.0,
        );
        let mut last = 0.0;
        for c in &frame.cascades {
            let n = cascade_project(&c.clone(), eye);
            assert!(n.x.abs() <= 1.0 && n.y.abs() <= 1.0);
            assert!(c.texel_world > last, "cascades must coarsen outward");
            last = c.texel_world;
        }
    }

    /// The practical split keeps the near detail ladder (cascade 0 about
    /// 10 m, cascade 2 about the configured 80 m) and adds the far tile.
    #[test]
    fn practical_splits_keep_the_detail_ladder_and_reach_far() {
        let reach = 80.0 * FAR_REACH;
        let f = split_fractions(reach);
        let ends: Vec<f32> = f.iter().map(|f| f * reach).collect();
        assert!(ends.windows(2).all(|w| w[0] < w[1]), "{ends:?}");
        assert!((8.0..=13.0).contains(&ends[0]), "cascade 0 end {}", ends[0]);
        assert!((70.0..=95.0).contains(&ends[2]), "cascade 2 end {}", ends[2]);
        assert_eq!(ends[3], reach);
    }

    /// A small FOV/zoom drift must not change the fitted texel size: the
    /// radius sits on a quantized ladder, so the maps re-rasterize once per
    /// step instead of every frame.
    #[test]
    fn radius_quantization_holds_texels_through_small_fov_drift() {
        let base = test_view();
        let fit = |k: f32| {
            let mut v = base;
            for fc in &mut v.far_corners {
                let d = *fc - v.cam;
                *fc = v.cam + v3(d.x * k, d.y * k, d.z);
            }
            fit_cascades(Some(&v), v.cam, sun(), v3(-60.0, 0.0, -60.0), v3(60.0, 12.0, 60.0), 80.0, 2048.0)
        };
        let a = fit(1.0);
        let mut same = 0;
        for k in [1.0005, 1.001, 1.002] {
            let b = fit(k);
            same += (0..CSM_CASCADES)
                .filter(|i| a.cascades[*i].texel_world == b.cascades[*i].texel_world)
                .count();
        }
        assert!(same >= 3 * CSM_CASCADES - 2, "radius ladder re-quantized too often ({same})");
    }

    /// The sun basis moves in fixed angular steps: a sub-step drift fits the
    /// identical cascade rows.
    #[test]
    fn sun_quantization_holds_the_grid_through_a_slow_day_cycle() {
        let view = test_view();
        let fit = |d: Vec3f| {
            fit_cascades(Some(&view), view.cam, d, v3(-60.0, 0.0, -60.0), v3(60.0, 12.0, 60.0), 80.0, 2048.0)
        };
        let s0 = quantize_sun(sun());
        let s1 = (s0 + v3(0.00005, 0.0, -0.00005)).normalize();
        let (a, b) = (fit(s0), fit(s1));
        for i in 0..CSM_CASCADES {
            assert_eq!(a.cascades[i].rx.x, b.cascades[i].rx.x);
            assert_eq!(a.cascades[i].ry.w, b.cascades[i].ry.w);
        }
        let err = (quantize_sun(sun()) - sun().normalize()).length();
        assert!(err < SUN_STEP_RAD, "quantized sun strays {err}");
    }

    /// The clip-space tile mapping and the uv origin describe the same
    /// quadrant (ndc +y is the top row, uv v grows downward).
    #[test]
    fn tile_clip_and_uv_origin_agree() {
        for i in 0..CSM_CASCADES {
            let t = csm_tile_clip(i);
            let (u0, v0) = csm_tile_origin(i);
            // ndc (-1, +1) of the cascade = its tile's top-left corner.
            let x = -t.x + t.z;
            let y = t.y + t.w;
            assert!(((x * 0.5 + 0.5) - u0).abs() < 1e-6);
            assert!(((0.5 - y * 0.5) - v0).abs() < 1e-6);
        }
    }

    /// Per-cascade culling keeps a caster inside a cascade's box and drops
    /// one outside its square or beyond its slice from the sun.
    #[test]
    fn cascade_culling_keeps_inside_and_drops_outside() {
        let view = test_view();
        let frame = fit_cascades(Some(&view), view.cam, sun(), v3(-60.0, 0.0, -60.0), v3(60.0, 12.0, 60.0), 80.0, 2048.0);
        let c = &frame.cascades[0];
        let at = view.cam + v3(0.0, -1.7, 4.0);
        let h = v3(0.5, 0.5, 0.5);
        assert!(cascade_overlaps(c, at - h, at + h));
        let far = view.cam + v3(0.0, -1.7, 90.0);
        assert!(!cascade_overlaps(c, far - h, far + h), "a caster 90 m out is not in cascade 0");
        assert!(cascade_overlaps(&frame.cascades[3], far - h, far + h));
    }

    /// The generation windows nest strictly downward, so a newer tile
    /// clear passes LessEqual against any older content, and the schedule
    /// renders each cascade at its period and clears in full before a tile
    /// runs out of generations.
    #[test]
    fn depth_generations_descend_and_the_schedule_staggers() {
        for g in 1..CSM_DEPTH_GENS {
            let (s0, o0) = depth_generation_window(g - 1);
            let (s1, o1) = depth_generation_window(g);
            assert!(o1 + s1 <= o0 + 1e-6, "generation {g} must sit below {}", g - 1);
            assert!(o1 >= 0.0 && s0 == s1);
        }
        let mut s = CsmSchedule::default();
        let (full, due) = s.plan(0, true);
        assert!(full && due.iter().all(|d| *d), "first frame clears and renders all");
        let mut renders = [0u32; CSM_CASCADES];
        let mut fulls = 0;
        for f in 1..=64u64 {
            let (full, due) = s.plan(f, true);
            fulls += full as u32;
            for i in 0..CSM_CASCADES {
                renders[i] += due[i] as u32;
                assert!(s.generation[i] < CSM_DEPTH_GENS);
            }
        }
        assert_eq!(renders[0], 64);
        assert_eq!(renders[1], 64);
        assert!(renders[2] >= 32 && renders[2] <= 36, "{renders:?}");
        assert!(renders[3] >= 16 && renders[3] <= 20, "{renders:?}");
        assert!(fulls >= 3 && fulls <= 5, "one full clear per {CSM_DEPTH_GENS} frames: {fulls}");
        let mut s = CsmSchedule::default();
        s.plan(0, false);
        let (_, due) = s.plan(1, false);
        assert!(due.iter().all(|d| *d), "stagger off renders every tile");
    }
}
