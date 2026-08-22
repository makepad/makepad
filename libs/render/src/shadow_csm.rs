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

/// Cascade count. Three covers a village-scale view with a sensible
/// resolution ladder; the fitting math is count-agnostic.
pub const CSM_CASCADES: usize = 3;

/// Split ends as fractions of the shadow range: cascade i covers
/// [SPLITS[i-1], SPLITS[i]] * range. Hand-picked practical ladder (a pure
/// log split wastes cascade 0 on the first metre).
const SPLITS: [f32; CSM_CASCADES] = [0.10, 0.30, 1.0];

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
    /// Depth bias in z01 units: texel footprint x 1.5 + 5mm, the bake
    /// depth passes' tolerance (raster error only — anything fatter reads
    /// kit detail sheets as lit).
    pub bias01: f32,
}

/// The frame's fitted cascade set.
#[derive(Clone, Copy, Debug, Default)]
pub struct CsmFrame {
    pub cascades: [CsmCascade; CSM_CASCADES],
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
    let bias_world = texel * 1.5 + 0.005;
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
    }
}

/// Fit the frame's cascades. `range` caps how far shadows reach (the last
/// split's end); `res` is one cascade's edge in texels.
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
    let mut frame = CsmFrame::default();
    match view {
        Some(view) if view.focus_distance > 0.0 => {
            // Pixel-stable orbit: cascade 0 is a tight slab around the
            // look-at so its texel footprint tracks the screen as the
            // camera zooms. Cascade 1 is a 2× nest; cascade 2 keeps the
            // configured reach for ground that is still in frame.
            let far = view_far(view);
            let (lo, hi) = focus_window(view.focus_distance, far);
            let mid = (lo + hi) * 0.5;
            let half = (hi - lo) * 0.5;
            let far_d = {
                let mut m = 0.0f32;
                for fc in &view.far_corners {
                    m = m.max((*fc - view.cam).length());
                }
                m.max(1.0)
            };
            let scale = (range / far_d).min(1.0);
            let windows = [
                (lo, hi),
                ((mid - 2.0 * half).max(0.0), (mid + 2.0 * half).min(1.0)),
                (0.0, scale),
            ];
            for (i, (f0, f1)) in windows.iter().enumerate() {
                let (c, r) = slice_sphere(view, *f0, *f1);
                frame.cascades[i] = fit_cascade(c, r, sun_dir, scene_min, scene_max, res);
            }
        }
        Some(view) => {
            // Rescale the far corners so the ladder covers `range` rather
            // than the projection's own far plane.
            let far_d = {
                let mut m = 0.0f32;
                for fc in &view.far_corners {
                    m = m.max((*fc - view.cam).length());
                }
                m.max(1.0)
            };
            let scale = (range / far_d).min(1.0);
            let mut f0 = 0.0;
            for (i, f1) in SPLITS.iter().enumerate() {
                let (c, r) = slice_sphere(view, f0 * scale, f1 * scale);
                frame.cascades[i] = fit_cascade(c, r, sun_dir, scene_min, scene_max, res);
                f0 = *f1;
            }
        }
        None => {
            for (i, f1) in SPLITS.iter().enumerate() {
                let r = range * f1 * 0.5;
                frame.cascades[i] = fit_cascade(eye, r, sun_dir, scene_min, scene_max, res);
            }
        }
    }
    frame
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

    /// Every material-family copy of csm_vis must fall through when a
    /// point is outside a cascade's DEPTH interval, not merely its xy
    /// square. Sampling a cleared tile with ref01 > 1 paints the uncovered
    /// footprint solid black—the 90-degree-orbit cascade hole.
    #[test]
    fn shader_selection_requires_full_xyz_coverage() {
        let src = include_str!("shaders.rs");
        assert_eq!(src.matches("csm_vis: fn").count(), 4);
        assert_eq!(
            src.matches("|| nz < 0.0 || nz > 1.0").count(),
            12,
            "all three fall-through checks in all four csm_vis copies must reject uncovered z"
        );
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
        let mut f0 = 0.0;
        for (i, f1) in SPLITS.iter().enumerate() {
            let scale = (80.0f32
                / view
                    .far_corners
                    .iter()
                    .map(|fc| (*fc - view.cam).length())
                    .fold(1.0f32, f32::max))
            .min(1.0);
            for fc in &view.far_corners {
                let d = *fc - view.cam;
                for f in [f0 * scale, f1 * scale] {
                    let p = view.cam + d * f;
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
}
