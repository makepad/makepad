//! The night skyline: a 360-degree strip of land, mountains, city, trees
//! and props, rendered on the GPU (see `render::Skyline`). Used by the
//! skyline example and as the HDRI generator's skyline layer.
pub use makepad_widgets;
use makepad_widgets::*;

pub mod params;
pub mod rng;
pub mod zones;
pub mod terrain;
pub mod city;
pub mod props;
pub mod shaders;
pub mod post;
pub mod render;
pub mod export;
pub mod light;

pub use export::{Band, BandDone, ExportKind};
pub use light::{add_light, add_scaled, coast_ring, example_haze_share, srgb_decode, srgb_encode, SkyLight, StripPlan, BACKDROP_HORIZON, HOST_LIGHT_NIGHT_SHARE, RING, SHORE_DEG};
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
