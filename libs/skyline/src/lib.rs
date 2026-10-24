//! The night skyline: a 360-degree strip of land, mountains, city, trees
//! and props, rendered on the GPU (see `render::Skyline`). Used by the
//! skyline example and as the HDRI generator's skyline layer.
pub use makepad_widgets;
use makepad_widgets::*;

pub mod params;
pub mod rng;
pub mod zones;
pub mod lakes;
pub mod terrain;
pub mod city;
pub mod props;
pub mod shaders;
pub mod post;
pub mod render;
pub mod export;
pub mod light;
pub mod fit;

pub use export::{Band, BandDone, ExportKind};
pub use fit::{band_fit, fit_band, BandFit};
pub use light::{add_light, add_scaled, coast_ring, example_haze_share, srgb_decode, srgb_encode, SkyLight, StripPlan, BACKDROP_HORIZON, HOST_LIGHT_NIGHT_SHARE, POINT_GAIN, POINT_SIGMA_DEG, RING, SHORE_DEG};
pub use params::{ParamDef, Params, COUNT, P, PARAMS, SEED_MAX};
pub use render::{SceneTarget, Skyline, ViewSpec};

/// Registers the skyline's shaders and its `mod.draw.Skyline`, in the
/// order they depend on each other. Call after makepad_draw's own.
pub fn script_mod(vm: &mut ScriptVm) {
    shaders::script_mod(vm);
    terrain::script_mod(vm);
    city::script_mod(vm);
    props::kit::script_mod(vm);
    props::script_mod(vm);
    post::script_mod(vm);
    render::script_mod(vm);
}

/// The strip's elevation range in degrees, (top, bottom): rows are linear
/// in elevation, the horizon sits `horizon` of the way down.
pub fn band_range_deg(p: &Params) -> (f64, f64) { let g = band_geom(p); (g.top, g.bottom) }

/// The elevation range in degrees, (top, bottom), of a strip of `p` rendered `w` x `h` (`ViewSpec::strip`): its top
/// row's top edge and its bottom row's bottom edge. The rows are square texels of 360 / w degrees about the horizon line
/// (`ViewSpec::row_of`), and h is rounded, so they span h x 360 / w degrees, a little more or less than the band's
/// own range (`band_range_deg`): this is the range to label a rendered strip with.
pub fn strip_range_deg(p: &Params, w: usize, h: usize) -> (f64, f64) {
    let deg = h as f64 * 360.0 / w.max(1) as f64;
    let top = band_geom(p).horizon * deg;
    (top, top - deg)
}

/// The strip's rows: its elevation range (degrees), its height in degrees and the horizon line (a fraction of the height
/// from the top).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BandGeom { pub top: f64, pub bottom: f64, pub deg: f64, pub horizon: f64 }

/// The strip's rows at `p`: the field of view times Size, the horizon `horizon` of the way down, so the tall things a
/// larger Size makes keep inside it. An end that Size pushes past 85 degrees stops there (a base range already past it
/// stays as it is), and the horizon line then moves so the rows stay linear in elevation.
pub fn band_geom(p: &Params) -> BandGeom {
    let (vfov, horizon, k) = (p[P::Vfov], p[P::Horizon], p[P::Size]);
    let (top, bottom) = (horizon * vfov * k, -(1.0 - horizon) * vfov * k);
    let (tc, bc) = (top.min(85f64.max(horizon * vfov)), bottom.max((-85f64).min(-(1.0 - horizon) * vfov)));
    if tc == top && bc == bottom { return BandGeom { top, bottom, deg: vfov * k, horizon }; }
    BandGeom { top: tc, bottom: bc, deg: tc - bc, horizon: tc / (tc - bc) }
}

/// The sea distance's ends at Size 1 (m): the slider's first step is about SEA_NEAR_M, its end SEA_FAR_M (the owner's R2:
/// 2/3 of plan 6's 50 m and 5 km, so the sea is 1.5x as deep below the horizon at every slider value).
pub const SEA_NEAR_M: f64 = 100.0 / 3.0;
pub const SEA_FAR_M: f64 = 10000.0 / 3.0;
/// `sea_edge_rad` when there is no shore (the sea at the viewer's feet): past every view's depth.
pub const NO_SHORE: f64 = 10.0;

/// Where the sea starts (m from the eye): SEA_NEAR_M x (SEA_FAR_M / SEA_NEAR_M)^sea_d, times Size; None at sea_d 0.
pub fn sea_distance_m(p: &Params) -> Option<f64> {
    let x = p[P::SeaD];
    (x > 0.0).then(|| SEA_NEAR_M * (SEA_FAR_M / SEA_NEAR_M).powf(x) * p[P::Size])
}

/// The sea's near edge, rad below the horizon: atan(EYE x Size / sea_distance_m) (the same at every Size); NO_SHORE without a shore.
pub fn sea_edge_rad(p: &Params) -> f64 {
    sea_distance_m(p).map_or(NO_SHORE, |d| (crate::props::kit::EYE * p[P::Size] / d).atan())
}

/// Whether a thing d m out stands on the open sea (beyond the sea distance; everywhere without a shore).
pub fn at_sea(p: &Params, d: f64) -> bool { sea_distance_m(p).map_or(true, |s| d >= s) }

/// The distance (m) the strip hands for its near ground rise and its front treeline (`Band::dist`; the shaders'
/// `near_rise`). They stand where they stood at any Size, as a prop does, in front of what stands on the land behind them (the
/// farms from 900 m, the power lines the treeline hides from 1 km); the treeline's tallest trees (Height 0.035 rad at the
/// defaults) are then about 18 m tall. Over the land the ground below the horizon reads no farther than the rise, which
/// hides what lies beyond it: the rows down to its foot, atan(EYE x Size / NEAR_RISE_M) down, read the rise.
pub const NEAR_RISE_M: f64 = 500.0;
/// The farthest distance (m) the strip hands for its ground, at the horizon: the city march's own far (`DrawSkyCity`'s
/// 6e4; the shaders' literal 60000), inside the half float's range of the distance target.
pub const FAR_M: f64 = 6e4;

/// The strip's eye over its own ground (m) as a host bounds its ground's distance by (the engine's
/// `SkylineBand::eye_m`): EYE x Size, over the nearest the off-centre camera scales a column's ground by (`terrain::warp`'s
/// t, 1 - Off at the least), so at a depression al no texel of the strip's ground, or of what stands on it, reads
/// farther than this / tan(al) (`DrawSkyGround`'s `ground_d`), and one that reads well past it shows what the water
/// mirrors (`Band::dist`: the water hands the distance of what it mirrors).
pub fn ground_eye_m(p: &Params) -> f64 {
    crate::props::kit::EYE * p[P::Size] / (1.0 - p[P::Off]).max(0.1)
}
/// The engine's MIRROR_FROM and MIRROR_TO (makepad_render's `hdri::skyline`, the app holds them equal): a host takes a
/// texel below the horizon for what the water mirrors from MIRROR_FROM times `ground_eye_m` / tan(al), wholly from
/// MIRROR_TO times it.
pub const MIRROR_FROM: f64 = 1.5;
pub const MIRROR_TO: f64 = 2.5;

#[cfg(test)]
mod tests {
    use super::*;

    /// The final review: a strip `w` wide renders `h` rows, h rounded from w x the band's degrees / 360, so its rows
    /// span h x 360 / w degrees, not the band's own (band_range_deg). `strip_range_deg` is the range of the rows as
    /// rendered: the top row's top edge and the bottom row's bottom edge fall on the view's rows 0 and h
    /// (`ViewSpec::row_of`), at 512 (46 rows for 45.5: the drift is a third of a degree) and at 1024, at Size 1 and 2.
    #[test]
    fn the_strips_range_is_its_rendered_rows() {
        let mut p = Params::default();
        for k in [1.0, 2.0] {
            p.set(P::Size as usize, k);
            for w in [512usize, 1024] {
                let v = ViewSpec::strip(&p, w);
                let (top, bottom) = strip_range_deg(&p, v.w, v.h);
                assert!(v.row_of(top.to_radians()).abs() < 1e-6, "Size {k}, {w}: the top {top} falls on row {}", v.row_of(top.to_radians()));
                assert!((v.row_of(bottom.to_radians()) - v.h as f64).abs() < 1e-6, "Size {k}, {w}: the bottom {bottom} falls on row {}", v.row_of(bottom.to_radians()));
                println!("Size {k}, {w} x {}: rows {top:.4} .. {bottom:.4}, the band {:?}", v.h, band_range_deg(&p));
            }
        }
        // the premise: at 512 the band's own range is not the rows'
        let p = Params::default();
        let v = ViewSpec::strip(&p, 512);
        assert!((strip_range_deg(&p, v.w, v.h).0 - band_range_deg(&p).0).abs() > 0.1);
    }

    /// The band grows with Size (both ends, the horizon line where it was) and the strip's rows with it; a top or a bottom
    /// that Size pushes past 85 degrees stops there, the horizon line moving so the rows stay linear in elevation. At
    /// Size 1 nothing changes, a base range already past 85 degrees included.
    #[test]
    fn the_band_grows_with_size_and_stops_at_85_degrees() {
        let mut p = Params::default();
        let (t1, b1) = band_range_deg(&p);
        assert_eq!((t1, b1), (0.8 * 32.0, -(1.0 - 0.8) * 32.0), "Size 1: as before");
        assert_eq!((ViewSpec::strip(&p, 1024).h, ViewSpec::strip(&p, 1024).horizon), ((1024.0 * 32.0 / 360.0f64).round() as usize, 0.8));
        for k in [0.5, 2.0] {
            p.set(P::Size as usize, k);
            let (t, b) = band_range_deg(&p);
            assert!((t - k * t1).abs() < 1e-12 && (b - k * b1).abs() < 1e-12, "Size {k}: {t} {b}");
            let v = ViewSpec::strip(&p, 1024);
            assert_eq!((v.h, v.horizon), ((1024.0 * 32.0 * k / 360.0).round() as usize, 0.8), "Size {k}");
        }
        // the top stops at 85 degrees: 0.8 x 90 x 2 = 144
        let set = |p: &mut Params, vfov: f64, horizon: f64, k: f64| { p.set(P::Vfov as usize, vfov); p.set(P::Horizon as usize, horizon); p.set(P::Size as usize, k); };
        set(&mut p, 90.0, 0.8, 2.0);
        let (t, b) = band_range_deg(&p);
        assert!((t - 85.0).abs() < 1e-12 && (b + 0.2 * 90.0 * 2.0).abs() < 1e-9, "{t} {b}");
        let v = ViewSpec::strip(&p, 1024);
        assert!((v.horizon - 85.0 / (85.0 - b)).abs() < 1e-12 && v.h == (1024.0 * (85.0 - b) / 360.0).round() as usize, "{v:?}");
        // and the bottom at -85: 0.7 x 90 x 2 = 126 below
        set(&mut p, 90.0, 0.3, 2.0);
        let (t, b) = band_range_deg(&p);
        assert!((t - 54.0).abs() < 1e-9 && (b + 85.0).abs() < 1e-12, "{t} {b}");
        // Size 1 keeps the base's own 85.5 degrees (0.95 x 90)
        set(&mut p, 90.0, 0.95, 1.0);
        assert_eq!(band_range_deg(&p).0, 0.95 * 90.0);
        assert_eq!(ViewSpec::strip(&p, 1024).horizon, 0.95);
    }

    /// Plan hdri-6 §3 and the owner's R2 (the water 1.5x as deep: 2/3 of the distance): the sea distance runs from about
    /// 33 m (the slider's first step) to 3.3 km on a log curve, times Size; 0 is no shore (the example's sea at the
    /// viewer's feet). Its edge's depression is the same at every Size (the eye is Size times as high too), and at Sea
    /// distance 0.6 it is about 0.68 degrees (0.46 at plan 6's 790 m).
    #[test]
    fn the_sea_distance_runs_from_33_m_to_3_3_km_with_size() {
        let mut p = Params::default();
        assert_eq!((sea_distance_m(&p), sea_edge_rad(&p)), (None, NO_SHORE), "the table's default: no shore");
        assert!(at_sea(&p, 1.0), "without a shore everything on the water is at sea");
        p.set(P::SeaD as usize, 0.01);
        assert!((sea_distance_m(&p).unwrap() - 34.904).abs() < 0.001, "{:?}", sea_distance_m(&p));
        p.set(P::SeaD as usize, 1.0);
        assert!((sea_distance_m(&p).unwrap() - 10000.0 / 3.0).abs() < 1e-9);
        p.set(P::SeaD as usize, 0.6);
        assert!((sea_edge_rad(&p).to_degrees() - 0.6832).abs() < 1e-4, "{}", sea_edge_rad(&p).to_degrees());
        p.set(P::SeaD as usize, 0.3);
        let (d1, e1) = (sea_distance_m(&p).unwrap(), sea_edge_rad(&p));
        assert!((d1 - 132.702).abs() < 0.001 && (e1 - (6.3f64 / d1).atan()).abs() < 1e-15, "{d1} {e1}");
        assert!(!at_sea(&p, 120.0) && at_sea(&p, 145.0));
        p.set(P::Size as usize, 2.0);
        assert!((sea_distance_m(&p).unwrap() - 2.0 * d1).abs() < 1e-9 && (sea_edge_rad(&p) - e1).abs() < 1e-12, "Size scales the distance, not the angle");
    }
}
