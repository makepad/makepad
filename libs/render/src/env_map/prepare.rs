//! What the renderer prepares from an environment map, off the UI thread
//! (renderer/ibl.rs submits it as a Background job): the IBL lane texture
//! (render-material's prefiltered atlas + SH9 meta row), a full-resolution
//! dome for the background (at most 2048 wide, RGBA f32), and the numbers
//! the sun rig, the exposure meter and the fog take from the map.
//!
//! The lighting products are built from a copy at most
//! [`LIGHTING_MAX_WIDTH`] wide without the environment's key: the renderer's
//! directional light carries that energy, so the SH and the atlas must not.
//! How the key leaves is [`KeyRemoval`]: a generated map is lit from the
//! same map baked without exactly its key's light (N3), so the lights beside
//! a wide studio key stay; a sun found in a file has its covering cone
//! filled with the sky around it ([`remove_sun`]). The dome keeps
//! the key, because that is what the eye should see.
//!
//! [`prepare_ibl_until`] asks a `stop` callback between its stages, so a
//! background job whose environment was replaced gives up early.

use makepad_render_material::ibl::{self, EnvMap};
use super::*;
use std::borrow::Cow;
use std::sync::{atomic::{AtomicBool, Ordering}, Mutex};

/// The dome texture is never wider than this: a 2048 x 1024 RGBA f32 dome
/// is 32 MB of texels, allocated at exactly that size (the HAL pads no
/// texture; the old D3D11 backend allocated three times the height, about
/// 100 MB). The atan2 seam at that size is invisible at 4K.
pub const DOME_MAX_WIDTH: usize = 2048;
/// The horizon band the fog colour averages: +- this elevation.
pub const HORIZON_BAND_DEG: f32 = 3.0;
/// The IBL lane texture's atlas width (`ibl::ibl_texture` = prefilter 256, 6).
pub const ATLAS_WIDTH: usize = 256;
const ATLAS_LEVELS: usize = 6;

/// One environment, prepared for the renderer.
#[derive(Clone, Debug, PartialEq)]
pub struct PreparedIbl {
    /// The lane texture (`TextureFormat::VecRGBAf32`, width x height).
    pub texture: ibl::IblTexture,
    /// The dome, `dome_width` x `dome_height` RGBA f32 (alpha 1), the
    /// source resized to at most the cap and never upsampled.
    pub dome_width: usize,
    pub dome_height: usize,
    pub dome: Vec<f32>,
    /// The SH9 irradiance coefficients of the lighting copy (`ibl::sh9`).
    pub sh: [[f32; 3]; 9],
    /// Solid-angle mean luminance of the lighting copy: the exposure meter.
    pub mean_luminance: f32,
    /// Linear mean colour of the rows within +- HORIZON_BAND_DEG of the
    /// horizon (lighting copy): the fog colour.
    pub horizon_rgb: Vec3f,
}

/// How the lighting copy loses the key the renderer's directional light
/// carries, so no part of the key is lit twice.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum KeyRemoval<'a> {
    /// No key: the copy keeps all of the map's light.
    Nothing,
    /// A sun found in a file (`detect_sun`), or any key the map came without
    /// a keyless copy for: its covering cone is filled with the ring of sky
    /// around it (`remove_sun`), which takes whatever else lies in the cone.
    Fill(&'a EnvSun),
    /// A generated map: the copy is made from this map, the same map without
    /// exactly its key's light (`GeneratedEnvMap::keyless`), so the lights
    /// beside the key stay. Any width: it is box-filtered to the copy's.
    Exact(&'a EnvMap),
}

/// The skeleton's entry point: no declared sun, the engine's atlas width.
pub fn prepare_ibl(env: &EnvMap, intensity: f32, rotation_deg: f32, max_dome_width: usize) -> PreparedIbl {
    prepare_ibl_sized(env, None, intensity, rotation_deg, max_dome_width, ATLAS_WIDTH)
}

/// [`prepare_ibl`] with the environment's declared sun: its covering cone is
/// filled in the copy the atlas, SH, meter and band are taken from; the dome
/// keeps it.
pub fn prepare_ibl_lit(env: &EnvMap, sun: Option<&EnvSun>, intensity: f32, rotation_deg: f32, max_dome_width: usize) -> PreparedIbl {
    prepare_ibl_sized(env, sun, intensity, rotation_deg, max_dome_width, ATLAS_WIDTH)
}

/// [`prepare_ibl_lit`] at a chosen atlas width. 256 is the engine's
/// (`ibl::ibl_texture`); tests and the synchronous small-resolution
/// fallback (a closed task pool) pass less. The shader reads the level
/// height from the meta row, so any width draws correctly.
pub fn prepare_ibl_sized(
    env: &EnvMap,
    sun: Option<&EnvSun>,
    intensity: f32,
    rotation_deg: f32,
    max_dome_width: usize,
    atlas_width: usize,
) -> PreparedIbl {
    let sizes = PrepareSizes { dome: max_dome_width, atlas: atlas_width, lighting: LIGHTING_MAX_WIDTH };
    let removal = sun.map_or(KeyRemoval::Nothing, KeyRemoval::Fill);
    prepare_ibl_until(env, removal, intensity, rotation_deg, sizes, &|| false, &|n, f| (0..n).for_each(f)).expect("a preparation nobody stops runs to its end")
}

/// The lighting copy is never wider than this. The prefilter reads its
/// source at most 512 wide (its pyramid) and the SH at 256, so 1024 loses
/// nothing they see, while a 4K source is no longer cloned whole for every
/// job (134 MB at 4K, 536 MB at 8K). `remove_sun` widens its cone by one
/// texel of the copy, which takes the disc the resize smeared with it.
pub const LIGHTING_MAX_WIDTH: usize = 1024;

/// The widths a preparation works at: the dome's cap, the atlas, and the
/// lighting copy's cap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrepareSizes {
    pub dome: usize,
    pub atlas: usize,
    pub lighting: usize,
}

/// [`prepare_ibl_sized`] at chosen sizes, with the key leaving the lighting
/// copy as `removal` says, that asks `stop` between its stages (after the
/// dome, after the lighting copy and its key removal, before every prefilter
/// level, before the SH) and gives up (`None`) once it says so: a background
/// job whose environment was replaced stops within a stage instead of
/// finishing a preparation nobody will use.
pub fn prepare_ibl_until(
    env: &EnvMap,
    removal: KeyRemoval,
    intensity: f32,
    rotation_deg: f32,
    sizes: PrepareSizes,
    stop: &(dyn Fn() -> bool + Sync),
    run: &dyn Fn(usize, &(dyn Fn(usize) + Sync)),
) -> Option<PreparedIbl> {
    let env = sane_spread(env, run);
    // The dome first, from the source with its sun: resized only when the
    // cap bites (resized clones at the same size, and a 4K map is 128 MB).
    let dome_width = dome_width(env.width, sizes.dome);
    let dome_height = dome_width / 2;
    // Resizing owns the RGBA array already; flatten it without a second
    // full-size allocation and copy.
    let dome = env.resized_spread(dome_width, run).data.into_flattened();
    if stop() {
        return None;
    }
    let lit = lighting_copy_spread(&env, removal, sizes.lighting, run);
    if stop() {
        return None;
    }
    let atlas = ibl::prefilter_spread(&lit, sizes.atlas, ATLAS_LEVELS, stop, run)?;
    if stop() {
        return None;
    }
    let sh = ibl::sh9(&lit);
    let texture = ibl::pack_ibl(&atlas, &sh, intensity, rotation_deg);
    Some(PreparedIbl {
        texture,
        dome_width,
        dome_height,
        dome,
        sh,
        mean_luminance: mean_luminance(&lit),
        horizon_rgb: horizon_band(&lit),
    })
}

/// The copy the atlas, the SH, the meter and the band are taken from, at
/// most `max_width` wide (box-filtered down): the source with the key's
/// covering cone filled there (`Fill`), the source as it is (`Nothing`), or
/// the keyless map (`Exact`, made sane first like the source). A map that is
/// narrow enough and has nothing to fill is not copied at all. `env` is
/// already `sane` (`resized` indexes every texel its size names).
pub fn lighting_copy<'a>(env: &'a EnvMap, removal: KeyRemoval<'a>, max_width: usize) -> Cow<'a, EnvMap> {
    lighting_copy_spread(env, removal, max_width, &|n, f| (0..n).for_each(f))
}

fn lighting_copy_spread<'a>(env: &'a EnvMap, removal: KeyRemoval<'a>, max_width: usize, run: &dyn Fn(usize, &(dyn Fn(usize) + Sync))) -> Cow<'a, EnvMap> {
    let narrow = |map: Cow<'a, EnvMap>| if map.width > max_width { Cow::Owned(map.resized_spread(max_width, run)) } else { map };
    match removal {
        KeyRemoval::Nothing => narrow(Cow::Borrowed(env)),
        KeyRemoval::Fill(sun) => {
            let mut lit = narrow(Cow::Borrowed(env));
            remove_sun(lit.to_mut(), sun);
            lit
        }
        KeyRemoval::Exact(keyless) => narrow(sane_spread(keyless, run)),
    }
}

/// The dome's width for a source of `src_width`: the source's, capped by
/// the caller and by [`DOME_MAX_WIDTH`], even (the dome is width x
/// width / 2) and at least 2.
pub fn dome_width(src_width: usize, max_dome_width: usize) -> usize {
    let w = src_width.min(max_dome_width).min(DOME_MAX_WIDTH);
    (w - w % 2).max(2)
}

/// Solid-angle weighted mean of the rows whose centres lie within
/// [`HORIZON_BAND_DEG`] of the horizon. A map too small to have such a row
/// takes the two rows either side of the horizon. Bad texels count as
/// black.
pub fn horizon_band(env: &EnvMap) -> Vec3f {
    let env = sane(env);
    let (w, h) = (env.width, env.height);
    let band = HORIZON_BAND_DEG / 180.0;
    let mut rows: Vec<usize> = (0..h).filter(|&y| ((y as f32 + 0.5) / h as f32 - 0.5).abs() <= band).collect();
    if rows.is_empty() {
        rows = if h >= 2 { vec![h / 2 - 1, h / 2] } else { vec![0] };
    }
    let pi = std::f32::consts::PI;
    let mut sum = Vec3f::default();
    let mut area = 0.0f32;
    for y in rows {
        // The band of cos(polar angle) this row covers (its longitude slice
        // is the same for every column, so it cancels).
        let sa = (pi * y as f32 / h as f32).cos() - (pi * (y + 1) as f32 / h as f32).cos();
        for x in 0..w {
            let c = env.data[y * w + x];
            sum += vec3f(c[0], c[1], c[2]) * sa;
            area += sa;
        }
    }
    if area > 0.0 { sum * (1.0 / area) } else { Vec3f::default() }
}

/// A map the rest of this file can index: a malformed one (fewer texels
/// than its size claims, or too small) becomes a 2 x 1 black map, and
/// NaN, infinite or negative channels become 0. Borrowed when nothing is
/// wrong, so the common case copies nothing.
fn sane(env: &EnvMap) -> Cow<'_, EnvMap> {
    sane_spread(env, &|n, f| (0..n).for_each(f))
}

fn sane_spread<'a>(env: &'a EnvMap, run: &dyn Fn(usize, &(dyn Fn(usize) + Sync))) -> Cow<'a, EnvMap> {
    if env.width < 2 || env.height < 1 || env.data.len() < env.width * env.height {
        return Cow::Owned(EnvMap::constant(2, [0.0, 0.0, 0.0]));
    }
    let bad = |v: f32| !(v.is_finite() && v >= 0.0);
    let found_bad = AtomicBool::new(false);
    run(env.data.len().div_ceil(env.width), &|y| {
        let end = ((y + 1) * env.width).min(env.data.len());
        if env.data[y * env.width..end].iter().any(|c| bad(c[0]) || bad(c[1]) || bad(c[2])) {
            found_bad.store(true, Ordering::Relaxed);
        }
    });
    if !found_bad.load(Ordering::Relaxed) {
        return Cow::Borrowed(env);
    }
    let mut data = vec![[0.0; 4]; env.data.len()];
    {
        let rows: Vec<_> = data.chunks_mut(env.width).map(|row| Mutex::new(Some(row))).collect();
        run(rows.len(), &|y| {
            let row = rows[y].lock().unwrap().take();
            if let Some(row) = row {
                for (out, c) in row.iter_mut().zip(&env.data[y * env.width..]) {
                    *out = [if bad(c[0]) { 0.0 } else { c[0] }, if bad(c[1]) { 0.0 } else { c[1] }, if bad(c[2]) { 0.0 } else { c[2] }, 1.0];
                }
            }
        });
    }
    Cow::Owned(EnvMap { width: env.width, height: env.height, data })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sky::luminance;

    /// Job-sized stage timings; generation and warm-up are not measured.
    #[test]
    #[ignore = "a measurement"]
    fn measure_job_ibl_stages() {
        use makepad_draw::makepad_platform::{Cx, thread::{checkpoint, Class}};
        use std::{cell::Cell, time::Instant};
        let paint = |d: [f32; 3], key: bool| {
            let sky = 0.2 + 0.8 * d[1].max(0.0);
            let panel = if d[0] > 0.85 { 3.0 } else { 0.0 };
            let key = if key && d[1] > 0.98 { 40.0 } else { 0.0 };
            [sky + panel + key, sky + panel * 0.8 + key, sky + panel * 0.6 + key]
        };
        let source = EnvMap::from_fn(4096, |d| paint(d, true));
        let keyless = EnvMap::from_fn(4096, |d| paint(d, false));
        let measure = |mode: &str, source: &EnvMap, keyless: &EnvMap, run: &dyn Fn(usize, &(dyn Fn(usize) + Sync)), stop: &(dyn Fn() -> bool + Sync)| {
            let sizes = PrepareSizes { dome: DOME_MAX_WIDTH, atlas: ATLAS_WIDTH, lighting: LIGHTING_MAX_WIDTH };
            std::hint::black_box(prepare_ibl_until(source, KeyRemoval::Exact(keyless), 1.0, 0.0, sizes, stop, run).unwrap());
            let total = Instant::now();
            let stage = Cell::new(total);
            let report = |name: &str| {
                let now = Instant::now();
                eprintln!("ibl_stage {mode} {name}: {:.3} ms", now.duration_since(stage.replace(now)).as_secs_f64() * 1000.0);
            };
            let env = sane_spread(source, run);
            report("sane scan");
            let dome = env.resized_spread(dome_width(env.width, sizes.dome), run).data.into_flattened();
            report("dome");
            assert!(!stop());
            let lit = lighting_copy_spread(&env, KeyRemoval::Exact(keyless), sizes.lighting, run);
            report("lighting copy");
            assert!(!stop());
            let level = Cell::new(0usize);
            let timed_run = |n, f: &(dyn Fn(usize) + Sync)| {
                let k = level.get();
                if k == 0 { report("pyramid + frames"); }
                run(n, f);
                report(&format!("prefilter level {k}"));
                level.set(k + 1);
            };
            let atlas = ibl::prefilter_spread(&lit, sizes.atlas, ATLAS_LEVELS, stop, &timed_run).unwrap();
            assert!(!stop());
            let sh = ibl::sh9(&lit);
            report("sh9");
            let meter = mean_luminance(&lit);
            let horizon = horizon_band(&lit);
            report("mean luminance + horizon band");
            let texture = ibl::pack_ibl(&atlas, &sh, 1.0, 0.0);
            report("pack");
            eprintln!("ibl_stage {mode} total: {:.3} ms", total.elapsed().as_secs_f64() * 1000.0);
            std::hint::black_box((dome, texture, meter, horizon));
        };
        measure("serial", &source, &keyless, &|n, f| (0..n).for_each(f), &|| false);
        let cx = Cx::new(Box::new(|_, _| {}));
        let pool = cx.task_pool();
        let work_pool = pool.clone();
        // Join off the pool's UI owner, as required by TaskHandle::join.
        std::thread::scope(|scope| scope.spawn(move || {
            pool.submit(Class::Background, move || {
                measure("pool", &source, &keyless, &|n, f| work_pool.fan_out(n, f), &|| checkpoint().is_err());
            }).unwrap().join().unwrap();
        }).join().unwrap());
    }

    /// A w x w/2 map painted by direction at texel centres.
    fn map(w: usize, paint: impl Fn(Vec3f) -> Vec3f) -> EnvMap {
        EnvMap::from_fn(w, |d| arr(paint(vec(d))))
    }
    fn grey(v: f32) -> Vec3f {
        vec3f(v, v, v)
    }
    fn elevation_deg(d: Vec3f) -> f32 {
        d.y.clamp(-1.0, 1.0).asin().to_degrees()
    }

    /// prepare_ibl is ibl_texture plus the dome, SH, meter and band: at the
    /// same atlas width the lane texture is bit-identical to render-material's
    /// own packing (prefilter is deterministic). 16 wide keeps the test fast;
    /// prepare_ibl itself passes 256, ibl_texture's width, by construction.
    #[test]
    fn the_lane_texture_is_the_engine_packing() {
        let env = map(64, |d| grey(0.2 + 0.8 * d.y.max(0.0)));
        let p = prepare_ibl_sized(&env, None, 1.5, 30.0, 2048, 16);
        let want = ibl::pack_ibl(&ibl::prefilter(&env, 16, 6), &ibl::sh9(&env), 1.5, 30.0);
        assert_eq!(p.texture, want);
        assert_eq!(p.sh, ibl::sh9(&env));
        // The meta row is row 0 (plan 1a task A7): texel 0 the SH's DC term,
        // texel 9 what was asked (levels, level height, intensity, rotation
        // in radians); the atlas starts at row 1.
        assert_eq!(&p.texture.data[0..3], &p.sh[0]);
        let meta = 9 * 4;
        assert_eq!(&p.texture.data[meta..meta + 4], &[6.0, 8.0, 1.5, 30.0f32.to_radians()]);
        let first_atlas_texel = ibl::prefilter(&env, 16, 6).data[0];
        assert_eq!(&p.texture.data[p.texture.width * 4..p.texture.width * 4 + 3], &first_atlas_texel[..3]);
    }

    /// K1: a preparation asks `stop` between its stages (after the dome,
    /// after the lighting copy, before every prefilter level, before the
    /// SH) and gives up once it says so; never told to stop it is exactly
    /// `prepare_ibl_sized`'s.
    #[test]
    fn a_preparation_asks_to_stop_between_its_stages() {
        let env = map(64, |d| grey(0.2 + 0.8 * d.y.max(0.0)));
        let sizes = PrepareSizes { dome: 2048, atlas: 16, lighting: LIGHTING_MAX_WIDTH };
        let asked = std::sync::atomic::AtomicUsize::new(0);
        let never = || {
            asked.fetch_add(1, Ordering::Relaxed);
            false
        };
        assert_eq!(prepare_ibl_until(&env, KeyRemoval::Nothing, 1.0, 0.0, sizes, &never, &|n, f| (0..n).for_each(f)), Some(prepare_ibl_sized(&env, None, 1.0, 0.0, 2048, 16)));
        let stages = asked.load(Ordering::Relaxed);
        assert_eq!(stages, 2 + ATLAS_LEVELS * (1 + sizes.atlas / 2) + 1, "after the dome, lighting copy, before every level and row, before the SH");
        for k in 0..stages {
            asked.store(0, Ordering::Relaxed);
            let at_k = || {
                asked.fetch_add(1, Ordering::Relaxed);
                asked.load(Ordering::Relaxed) > k
            };
            assert_eq!(prepare_ibl_until(&env, KeyRemoval::Nothing, 1.0, 0.0, sizes, &at_k, &|n, f| (0..n).for_each(f)), None, "stopped at question {k}");
            assert_eq!(asked.load(Ordering::Relaxed), k + 1, "nothing asked after the stop");
        }
    }

    #[test]
    fn the_dome_is_capped_and_never_upsampled() {
        let env = map(512, |_| grey(0.25));
        let p = prepare_ibl_sized(&env, None, 1.0, 0.0, 128, 16);
        assert_eq!((p.dome_width, p.dome_height, p.dome.len()), (128, 64, 128 * 64 * 4));
        for texel in p.dome.chunks(4) {
            assert!((texel[0] - 0.25).abs() < 1.0e-4 && (texel[2] - 0.25).abs() < 1.0e-4 && texel[3] == 1.0, "{texel:?}");
        }
        assert_eq!(prepare_ibl_sized(&env, None, 1.0, 0.0, 4096, 16).dome_width, 512, "never upsampled");
        assert_eq!(dome_width(8192, 4096), DOME_MAX_WIDTH, "the 2048 cap holds whatever is asked");
        assert_eq!(dome_width(1000, 2048), 1000);
        assert_eq!(dome_width(1000, 3), 2, "an odd cap rounds down to an even width");
        assert_eq!(dome_width(1, 2048), 2);
    }

    #[test]
    fn the_meter_is_the_mean_luminance_with_the_sun_filled() {
        let flat = prepare_ibl_sized(&map(64, |_| grey(1.0)), None, 1.0, 0.0, 2048, 16);
        assert!((flat.mean_luminance - 1.0).abs() < 1.0e-3, "{}", flat.mean_luminance);
        let black = prepare_ibl_sized(&map(64, |_| grey(0.0)), None, 1.0, 0.0, 2048, 16);
        assert_eq!(black.mean_luminance, 0.0);
        // A sun the environment declares belongs to the directional light:
        // it is not metered, not in the SH, but still in the dome.
        let sun_dir = dir_from_az_el(90.0, 45.0);
        let cos_radius = 3.0f32.to_radians().cos();
        let sun = EnvSun { dir: sun_dir, radiance: grey(1.0e4), cos_radius, facing: 1.0, cos_cover: cos_radius };
        let env = map(256, |d| grey(if d.dot(sun_dir) >= sun.cos_radius { 1.0e4 } else { 0.5 }));
        let lit = prepare_ibl_sized(&env, Some(&sun), 1.0, 0.0, 2048, 16);
        let raw = prepare_ibl_sized(&env, None, 1.0, 0.0, 2048, 16);
        assert!((lit.mean_luminance - 0.5).abs() < 0.01, "{}", lit.mean_luminance);
        assert!(raw.mean_luminance > 2.0, "an undeclared sun floods the meter: {}", raw.mean_luminance);
        let e_lit = ibl::sh9_irradiance(&lit.sh, arr(sun_dir))[1];
        let e_raw = ibl::sh9_irradiance(&raw.sh, arr(sun_dir))[1];
        assert!((e_lit - std::f32::consts::PI * 0.5).abs() < 0.15, "E(sun) with the cone filled: {e_lit}");
        assert!(e_raw > 3.0 * e_lit, "an undeclared sun floods the SH: {e_raw}");
        // The dome keeps the disc (same size as the source: no resample).
        assert_eq!((lit.dome_width, lit.dome_height), (256, 128));
        let uv = ibl::dir_to_equirect_uv(arr(sun_dir));
        let i = ((uv[1] * 128.0) as usize * 256 + (uv[0] * 256.0) as usize) * 4;
        assert!(lit.dome[i] > 1000.0, "the dome shows the sun: {}", lit.dome[i]);
        assert_eq!(prepare_ibl(&map(16, |_| grey(0.3)), 1.0, 0.0, 2048).texture, ibl::ibl_texture(&map(16, |_| grey(0.3)), 1.0, 0.0));
    }

    /// The amendment: the lighting copy is filled at `cos_cover`, the cone
    /// that holds the key's whole reach, not only at the `cos_radius` cone
    /// its radiance is averaged over. A wide key (a studio softbox) has part
    /// of its emission in a halo outside the radiance cone; the directional
    /// light carries that part too, so the copy must not.
    #[test]
    fn the_lighting_copy_is_filled_at_the_covering_cone() {
        let sun_dir = dir_from_az_el(200.0, 50.0);
        let (cos_radius, cos_cover) = (3.0f32.to_radians().cos(), 6.0f32.to_radians().cos());
        // A hot core inside the radiance cone, a dim halo out to the cover.
        let paint = |d: Vec3f| {
            let c = d.dot(sun_dir);
            grey(if c >= cos_radius { 1.0e4 } else if c >= cos_cover { 50.0 } else { 0.5 })
        };
        let env = map(256, paint);
        let key = EnvSun { dir: sun_dir, radiance: grey(1.0e4), cos_radius, facing: 0.9, cos_cover };
        let radius_only = EnvSun { cos_cover: cos_radius, ..key };
        let covered = prepare_ibl_sized(&env, Some(&key), 1.0, 0.0, 2048, 16);
        let narrow = prepare_ibl_sized(&env, Some(&radius_only), 1.0, 0.0, 2048, 16);
        assert!((covered.mean_luminance - 0.5).abs() < 0.01, "the halo is filled too: {}", covered.mean_luminance);
        assert!(narrow.mean_luminance > covered.mean_luminance + 0.05, "a cone filled only at cos_radius keeps the halo: {} vs {}", narrow.mean_luminance, covered.mean_luminance);
        let e_covered = ibl::sh9_irradiance(&covered.sh, arr(sun_dir))[1];
        assert!((e_covered - std::f32::consts::PI * 0.5).abs() < 0.15, "E toward the key with the cover filled: {e_covered}");
    }

    #[test]
    fn the_horizon_band_is_the_rows_within_three_degrees() {
        // At 64 x 32, rows 15 and 16 have their centres 2.8 deg above and
        // below the horizon; rows 14 and 17 sit at 8.4 deg.
        let horizon = vec3f(1.0, 0.4, 0.1);
        let env = map(64, |d| if elevation_deg(d).abs() < 3.0 { horizon } else { vec3f(0.1, 0.2, 1.0) });
        let p = prepare_ibl_sized(&env, None, 1.0, 0.0, 2048, 16);
        assert!((p.horizon_rgb - horizon).length() < 1.0e-4, "{:?}", p.horizon_rgb);
        assert!((horizon_band(&env) - horizon).length() < 1.0e-4);
        // A map too small to have a row inside the band takes the two rows
        // either side of the horizon.
        let tiny = map(8, |d| if d.y >= 0.0 { vec3f(1.0, 0.0, 0.0) } else { vec3f(0.0, 0.0, 1.0) });
        let band = horizon_band(&tiny);
        assert!((band.x - 0.5).abs() < 1.0e-4 && (band.z - 0.5).abs() < 1.0e-4, "{band:?}");
        // The whole-sphere meter and the band are both plain luminance.
        assert!((luminance(p.horizon_rgb) - luminance(horizon)).abs() < 1.0e-4);
    }

    #[test]
    fn a_broken_map_prepares_to_black_instead_of_panicking() {
        let broken = EnvMap { width: 5, height: 5, data: vec![[1.0; 4]; 3] };
        let p = prepare_ibl_sized(&broken, None, 1.0, 0.0, 2048, 16);
        assert_eq!((p.dome_width, p.dome_height, p.dome.len()), (2, 1, 8));
        assert_eq!(p.mean_luminance, 0.0);
        assert!(p.horizon_rgb.is_finite() && p.sh.iter().flatten().all(|v| v.is_finite()));
        // NaN, negative and infinite texels (bad files) become black before
        // the dome, the atlas, the SH and the meter see them.
        let mut bad = map(64, |_| grey(1.0));
        bad.data[0] = [f32::NAN, 1.0, 1.0, 1.0];
        bad.data[1] = [1.0, -3.0, 1.0, 1.0];
        bad.data[2] = [1.0, 1.0, f32::INFINITY, 1.0];
        let p = prepare_ibl_sized(&bad, None, 1.0, 0.0, 2048, 16);
        assert!(p.mean_luminance.is_finite() && p.mean_luminance < 1.0);
        assert!(p.texture.data.iter().all(|v| v.is_finite()) && p.dome.iter().all(|v| v.is_finite()));
        assert_eq!((p.dome[0], p.dome[5], p.dome[10]), (0.0, 0.0, 0.0));
    }
}
