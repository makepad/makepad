//! The band's rows a host fits to the scene (the HDRI app, plan hdri-6 §1). The top sits just over the tallest thing the
//! scene holds (the ranges and their trees, the city, every prop, the city's glow dome). The bottom sits a little below
//! where the shore meets the water (or the nearest lake). So nothing is cut off at any Size, and the band holds no empty
//! sky. The rows stay linear in elevation (`band_geom`): `fit_band` writes the field of view and the horizon line that
//! give them.
use crate::params::{Params, P};
use crate::zones::Zones;
use crate::{city, props, render, terrain};

/// The columns round the turn the ranges are measured over.
pub const FIT_W: usize = 1024;
/// The margin over the content: TOP_MARGIN_DEG plus TOP_MARGIN_SHARE of it.
pub const TOP_MARGIN_DEG: f64 = 1.0;
pub const TOP_MARGIN_SHARE: f64 = 0.04;
/// The top's ends (degrees).
pub const MIN_TOP_DEG: f64 = 3.0;
pub const MAX_TOP_DEG: f64 = 85.0;
/// How far the band reaches under the ground's depth (degrees), and the least depth (no shore, no lake).
pub const SHORE_BELOW_DEG: f64 = 1.5;
pub const MIN_GROUND_DEG: f64 = 1.5;
/// The glow dome's reach in its e-folds: where it has fallen to 1 % of its light at the horizon.
pub const GLOW_FOLDS: f64 = 4.6;

/// The parameters the fit depends on (with the seed): the scene's geometry, not its light or its view.
pub const FIT_KEYS: &[P] = &[
    P::Land, P::LandC, P::Size, P::SeaD, P::MtH, P::MtW, P::MtC, P::MtRug, P::CtH, P::CtD, P::CtW, P::CtC, P::CtR, P::TrD, P::TrH,
    P::TrCon, P::TrW, P::TrC, P::Treeline, P::PrPyl, P::PrTurb, P::PrCrane, P::PrInd, P::PrChurch, P::PrTown, P::PrLamp, P::PrBridge,
    P::PrFair, P::PrMast, P::PrFar, P::PrFarm, P::PrCoast, P::PrLake, P::Glow, P::Off, P::OffDir,
];

/// The band's rows (degrees): its top and bottom, and the content's top (the tallest thing, before the margin).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BandFit { pub top_deg: f64, pub bottom_deg: f64, pub content_deg: f64 }

/// The highest raw elevation (rad) the scene of `p` reaches: its ranges (FIT_W columns), the city, the props, the glow.
pub fn content_top_rad(p: &Params, z: &Zones) -> f64 {
    let m = terrain::margin(p, 1.0, FIT_W);
    let cols = terrain::columns(z, p, 0.0, 1.0, FIT_W, m);
    let col = |x: usize, r: usize| cols[(x + m) * terrain::ROWS + r];
    let ranges = (0..6).map(|k| render::range_top(p, FIT_W, &col, k)).fold(0.0, f64::max);
    let city = city::reach(p).map_or(0.0, |(top, _)| top);
    let props = props::reach(&props::build(z, p), p);
    let glow = if p[P::Glow] > 0.0 && p[P::CtW] >= 0.001 { GLOW_FOLDS / render::dome_k(p) } else { 0.0 };
    ranges.max(city).max(props).max(glow)
}

/// How far down (rad) the band's ground must reach: the sea's near edge, the nearest lake's shore, at least MIN_GROUND_DEG.
pub fn ground_depth_rad(p: &Params, z: &Zones) -> f64 {
    let sea = if p[P::Land] < 0.999 && crate::sea_distance_m(p).is_some() { crate::sea_edge_rad(p) } else { 0.0 };
    let eye = crate::props::kit::EYE * p[P::Size];
    let lake = z.lakes.iter().map(|l| (eye / (l.d - l.half_d)).atan()).fold(0.0, f64::max);
    sea.max(lake).max(MIN_GROUND_DEG.to_radians())
}

/// The fitted rows of `p` (cached by FIT_KEYS: the last eight).
pub fn band_fit(p: &Params) -> BandFit {
    static LAST: std::sync::Mutex<Vec<(u64, BandFit)>> = std::sync::Mutex::new(Vec::new());
    let key = p.hash(FIT_KEYS, &[]);
    if let Some(f) = LAST.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(k, _)| *k == key).map(|(_, f)| *f) { return f; }
    let z = Zones::new(p);
    let content = content_top_rad(p, &z).to_degrees();
    let top = (content * (1.0 + TOP_MARGIN_SHARE) + TOP_MARGIN_DEG).clamp(MIN_TOP_DEG, MAX_TOP_DEG);
    let bottom = -(ground_depth_rad(p, &z).to_degrees() + SHORE_BELOW_DEG);
    let f = BandFit { top_deg: top, bottom_deg: bottom, content_deg: content };
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    last.retain(|(k, _)| *k != key);
    last.push((key, f));
    if last.len() > 8 { last.remove(0); }
    f
}

/// Writes `p`'s field of view and horizon line (unsnapped) so `band_geom(p)` is the fit; returns it.
pub fn fit_band(p: &mut Params) -> BandFit {
    let f = band_fit(p);
    let deg = f.top_deg - f.bottom_deg;
    // written as they are, not snapped (`Params::set`): the rows must be the fit's, and no slider shows them
    p.v[P::Vfov as usize] = deg / p[P::Size];
    p.v[P::Horizon as usize] = f.top_deg / deg;
    f
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{band_range_deg, render::ViewSpec, PARAMS};
    /// The tallest towers, the highest mountains, every prop, the brightest glow.
    fn tall() -> Params {
        let mut p = Params::default();
        for (i, d) in PARAMS.iter().enumerate() { if d.group == "Props" { p.set(i, 1.0); } }
        p.set(P::CtH as usize, 2.4); p.set(P::MtH as usize, 0.5); p.set(P::Glow as usize, 2.0); p.set(P::CtD as usize, 0.9);
        p
    }
    /// Plan hdri-6 §1: the fitted top holds the city's reach, every prop's and the ranges' (the content), plus its margin,
    /// at any Size up to 2, and stays at or below 85 degrees; the bottom reaches past the ground's depth.
    #[test]
    fn the_fit_holds_everything_the_scene_holds() {
        for (name, base) in [("defaults", Params::default()), ("tall", tall())] {
            for k in [0.5, 1.0, 2.0] {
                let mut p = base.clone();
                p.set(P::Size as usize, k);
                let z = Zones::new(&p);
                let t = std::time::Instant::now();
                let f = band_fit(&p);
                let ms = t.elapsed().as_secs_f64() * 1000.0;
                let city = crate::city::reach(&p).map_or(0.0, |r| r.0).to_degrees();
                let props = crate::props::reach(&crate::props::build(&z, &p), &p).to_degrees();
                println!("{name}, Size {k}: {f:?} (city {city:.2}, props {props:.2}), {ms:.1} ms cold");
                assert!(f.content_deg >= city && f.content_deg >= props, "{name}, Size {k}");
                assert!(f.top_deg >= (f.content_deg + TOP_MARGIN_DEG).min(MAX_TOP_DEG) && f.top_deg <= MAX_TOP_DEG, "{name}, Size {k}: {f:?}");
                assert!(f.bottom_deg <= -(ground_depth_rad(&p, &z).to_degrees() + SHORE_BELOW_DEG) + 1e-9, "{name}, Size {k}: {f:?}");
                assert_eq!(band_fit(&p), f, "cached");
            }
        }
    }
    /// The fitted rows are the fit: `band_geom` after `fit_band` gives its top and bottom, and the strip's rows span them.
    #[test]
    fn the_fitted_rows_are_the_fit() {
        let mut p = tall();
        p.set(P::Size as usize, 2.0);
        let f = fit_band(&mut p);
        let (top, bottom) = band_range_deg(&p);
        assert!((top - f.top_deg).abs() < 1e-9 && (bottom - f.bottom_deg).abs() < 1e-9, "{top} {bottom} vs {f:?}");
        let v = ViewSpec::strip(&p, 1024);
        assert_eq!(v.h, (1024.0 * (f.top_deg - f.bottom_deg) / 360.0).round() as usize);
    }
    /// The band ends SHORE_BELOW_DEG under where the shore meets the sea (or the nearest lake's shore), at least
    /// MIN_GROUND_DEG + SHORE_BELOW_DEG down.
    #[test]
    fn the_band_ends_below_the_shore() {
        let mut p = Params::default();
        assert!((band_fit(&p).bottom_deg + MIN_GROUND_DEG + SHORE_BELOW_DEG).abs() < 1e-9, "no shore, no lakes");
        p.set(P::SeaD as usize, 0.01);
        let edge = crate::sea_edge_rad(&p).to_degrees();
        assert!(edge > 10.0 && (band_fit(&p).bottom_deg + edge + SHORE_BELOW_DEG).abs() < 1e-9, "the sea 35 m out: {edge}");
        let mut q = Params::default();
        q.set(P::Land as usize, 1.0);
        q.set(P::PrLake as usize, 1.0);
        let z = Zones::new(&q);
        let eye = crate::props::kit::EYE * q[P::Size];
        let lake = z.lakes.iter().map(|l| (eye / (l.d - l.half_d)).atan().to_degrees()).fold(MIN_GROUND_DEG, f64::max);
        assert!((band_fit(&q).bottom_deg + lake + SHORE_BELOW_DEG).abs() < 1e-9, "the nearest lake: {lake}");
    }
    /// The fit's two measures of `p`, uncached: the content's top and the ground's depth (rad).
    fn uncached(p: &Params) -> (f64, f64) {
        let z = Zones::new(p);
        (content_top_rad(p, &z), ground_depth_rad(p, &z))
    }
    /// Bare land: low ranges without trees on them, no city, no props.
    fn bare() -> Params {
        let mut p = Params::default();
        for (i, d) in PARAMS.iter().enumerate() { if d.group == "Props" { p.set(i, 0.0); } }
        for (key, v) in [(P::MtH, 0.02), (P::TrD, 0.0), (P::TrW, 0.0), (P::CtW, 0.0)] { p.set(key as usize, v); }
        p
    }
    /// The bases the fit is measured over, so that every row of FIT_KEYS moves it over one of them (the premise of
    /// `every_row_outside_fit_keys_leaves_the_fit_alone`). The table's defaults, the app's (the sea about 133 m out, a
    /// few lakes), all land with every lake and `tall`; and a base for each term of `content_top_rad` that binds nowhere
    /// else: bare land seen off-centre (any prop stands over it, the props' reach growing as the camera nears them), tall
    /// trees on low ranges (the trees' tops, some capped by the treeline, over the ridges), a flat forest (no mountains,
    /// dense trees everywhere: the canopy is the top), the glow off over tall towers far out (the dome would stand over
    /// them), and cranes seen from an off-centre camera beside them (over the low city).
    fn bases() -> Vec<(&'static str, Params)> {
        let with = |mut p: Params, rows: &[(P, f64)]| { for &(key, v) in rows { p.set(key as usize, v); } p };
        vec![
            ("the defaults", Params::default()),
            ("the app's", with(Params::default(), &[(P::SeaD, 0.3), (P::PrLake, 0.6)])),
            ("all lakes", with(Params::default(), &[(P::Land, 1.0), (P::PrLake, 1.0)])),
            ("tall", tall()),
            ("bare land, off-centre", with(bare(), &[(P::Off, 0.5)])),
            ("tall trees on low ranges", with(bare(), &[(P::MtH, 0.1), (P::TrH, 0.08), (P::TrW, 0.55)])),
            ("a flat forest", with(bare(), &[(P::MtW, 0.0), (P::TrD, 1.0), (P::TrCon, 0.5), (P::TrW, 1.0)])),
            ("the glow off", with(bare(), &[(P::CtW, 0.3), (P::CtH, 2.4), (P::CtR, 2.0), (P::Glow, 0.0)])),
            ("cranes, off-centre", with(bare(), &[(P::CtW, 0.3), (P::CtH, 0.2), (P::CtR, 0.5), (P::Off, 0.6), (P::PrCrane, 1.0)])),
        ]
    }
    /// The fit ignores the view (the field of view, the horizon, the detail view) and the light: the content's top and the
    /// ground's depth, measured afresh (`band_fit` hands back the cached entry of FIT_KEYS, which these rows are not in).
    #[test]
    fn the_fit_ignores_the_view_and_the_light() {
        for (name, p) in bases() {
            let mut q = p.clone();
            for (key, v) in [(P::Vfov, 80.0), (P::Horizon, 0.4), (P::Zoom, 0.5), (P::Haze, 1.0), (P::Lights, 9.0), (P::Temp, 6000.0)] { q.set(key as usize, v); }
            assert_eq!(uncached(&p), uncached(&q), "{name}");
        }
    }
    /// `base` with row `i` at a share `t` of its range.
    fn moved(base: &Params, i: usize, t: f64) -> Params {
        let mut q = base.clone();
        q.set(i, PARAMS[i].min + t * (PARAMS[i].max - PARAMS[i].min));
        q
    }
    /// Every row outside FIT_KEYS but the field of view and the horizon (which `fit_band` writes) leaves the fit alone: set
    /// to two other values (a tenth and nine tenths of its range) over each base, the content's top and the ground's
    /// depth, measured afresh, do not move. A row of the scene's geometry left out of FIT_KEYS, whose change the cache
    /// would not see, fails here: the premise is that every row in FIT_KEYS, set the same way, moves the fit over one of
    /// the bases.
    #[test]
    fn every_row_outside_fit_keys_leaves_the_fit_alone() {
        let bases: Vec<_> = bases().into_iter().map(|(name, p)| { let want = uncached(&p); (name, p, want) }).collect();
        let moves = |i: usize| bases.iter().any(|(_, base, want)| [0.1, 0.9].iter().any(|&t| uncached(&moved(base, i, t)) != *want));
        let still: Vec<&str> = FIT_KEYS.iter().filter(|k| !moves(**k as usize)).map(|k| PARAMS[*k as usize].key).collect();
        assert!(still.is_empty(), "the premise: these rows of FIT_KEYS move the fit over none of the bases: {still:?}");
        let mut rows = 0;
        for (name, base, want) in &bases {
            for (i, d) in PARAMS.iter().enumerate() {
                if FIT_KEYS.iter().any(|k| *k as usize == i) || i == P::Vfov as usize || i == P::Horizon as usize { continue; }
                for t in [0.1, 0.9] {
                    let q = moved(base, i, t);
                    assert_ne!(q.v[i], base.v[i], "{name}: {} is not perturbed", d.key);
                    assert_eq!(uncached(&q), *want, "{name}: {} at {} moves the fit", d.key, q.v[i]);
                }
                rows += 1;
            }
        }
        assert!(rows >= bases.len() * 9, "the premise: the light's and the view's rows are perturbed ({rows})");
    }
}
