//! What the renderer prepares from an environment map, off the UI thread
//! (renderer/ibl.rs submits it as a Heavy job): the IBL lane texture
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
//! filled with the sky around it (envmap.rs `remove_sun`). The dome keeps
//! the key, because that is what the eye should see.
//!
//! [`prepare_ibl_until`] asks a `stop` callback between its stages, so a
//! background job whose environment was replaced gives up early.

use makepad_render_material::ibl::{self, EnvMap};
use super::*;
use super::envmap::remove_sun;
use super::image::mean_luminance;
use std::borrow::Cow;

/// The dome texture is never wider than this: a 2048 x 1024 RGBA f32 dome
/// is 32 MB of texels; D3D11 allocates RGBA32F textures at three times the
/// height (about 100 MB there). The atan2 seam at that size is invisible
/// at 4K.
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
    /// exactly its key's light (`Env::bake_keyless_par`), so the lights
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
    prepare_ibl_until(env, removal, intensity, rotation_deg, sizes, &|| false).expect("a preparation nobody stops runs to its end")
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
    stop: &dyn Fn() -> bool,
) -> Option<PreparedIbl> {
    let env = sane(env);
    // The dome first, from the source with its sun: resized only when the
    // cap bites (resized clones at the same size, and a 4K map is 128 MB).
    let dome_width = dome_width(env.width, sizes.dome);
    let dome_height = dome_width / 2;
    let dome = if dome_width == env.width && dome_height == env.height {
        env.to_rgba_f32()
    } else {
        env.resized(dome_width).to_rgba_f32()
    };
    if stop() {
        return None;
    }
    let lit = lighting_copy(&env, removal, sizes.lighting);
    if stop() {
        return None;
    }
    let atlas = ibl::prefilter_until(&lit, sizes.atlas, ATLAS_LEVELS, stop)?;
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
fn lighting_copy<'a>(env: &'a EnvMap, removal: KeyRemoval<'a>, max_width: usize) -> Cow<'a, EnvMap> {
    let narrow = |map: Cow<'a, EnvMap>| if map.width > max_width { Cow::Owned(map.resized(max_width)) } else { map };
    match removal {
        KeyRemoval::Nothing => narrow(Cow::Borrowed(env)),
        KeyRemoval::Fill(sun) => {
            let mut lit = narrow(Cow::Borrowed(env));
            remove_sun(lit.to_mut(), sun);
            lit
        }
        KeyRemoval::Exact(keyless) => narrow(sane(keyless)),
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
    if env.width < 2 || env.height < 1 || env.data.len() < env.width * env.height {
        return Cow::Owned(EnvMap::constant(2, [0.0, 0.0, 0.0]));
    }
    let bad = |v: f32| !(v.is_finite() && v >= 0.0);
    if env.data.iter().any(|c| bad(c[0]) || bad(c[1]) || bad(c[2])) {
        let mut copy = env.clone();
        for c in &mut copy.data {
            for v in c.iter_mut().take(3) {
                if bad(*v) {
                    *v = 0.0;
                }
            }
            c[3] = 1.0;
        }
        return Cow::Owned(copy);
    }
    Cow::Borrowed(env)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sky::luminance;

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
        let asked = std::cell::Cell::new(0usize);
        let never = || {
            asked.set(asked.get() + 1);
            false
        };
        assert_eq!(prepare_ibl_until(&env, KeyRemoval::Nothing, 1.0, 0.0, sizes, &never), Some(prepare_ibl_sized(&env, None, 1.0, 0.0, 2048, 16)));
        let stages = asked.get();
        assert_eq!(stages, 2 + ATLAS_LEVELS + 1, "after the dome, after the lighting copy, before every level, before the SH");
        for k in 0..stages {
            asked.set(0);
            let at_k = || {
                asked.set(asked.get() + 1);
                asked.get() > k
            };
            assert_eq!(prepare_ibl_until(&env, KeyRemoval::Nothing, 1.0, 0.0, sizes, &at_k), None, "stopped at question {k}");
            assert_eq!(asked.get(), k + 1, "nothing asked after the stop");
        }
    }

    /// M5: the lighting copy is at most LIGHTING_MAX_WIDTH wide. A wider
    /// source's atlas, SH, meter and band are those of its 1024 wide resize
    /// with the sun filled there (the dome keeps up to 2048), and the
    /// resize does not leave the disc behind: `remove_sun` widens its cone
    /// by one texel of the copy, which holds what the resize smeared. A
    /// narrower source is not copied at all. A keyless copy (N3) is sized
    /// the same way.
    #[test]
    fn the_lighting_copy_is_at_most_1024_wide() {
        let sun_dir = dir_from_az_el(120.0, 35.0);
        let cos_radius = 0.265f32.to_radians().cos();
        let sun = EnvSun { dir: sun_dir, radiance: grey(1.0e5), cos_radius, facing: 1.0, cos_cover: cos_radius };
        let wide = map(2048, |d| if d.dot(sun_dir) >= cos_radius { grey(1.0e5) } else { vec3f(0.5, 0.5 + 0.1 * d.x, 0.5 - 0.1 * d.y) });
        assert!(mean_luminance(&wide) > 0.6, "premise: the disc moves the meter: {}", mean_luminance(&wide));
        assert_eq!(lighting_copy(&wide, KeyRemoval::Nothing, LIGHTING_MAX_WIDTH).width, LIGHTING_MAX_WIDTH);
        let lit = lighting_copy(&wide, KeyRemoval::Fill(&sun), LIGHTING_MAX_WIDTH);
        let mut want = wide.resized(LIGHTING_MAX_WIDTH);
        remove_sun(&mut want, &sun);
        assert!(*lit == want, "the 1024 wide resize with the sun filled there");
        assert!((mean_luminance(&lit) - 0.5).abs() < 0.005, "no disc left in the copy: {}", mean_luminance(&lit));
        let p = prepare_ibl_sized(&wide, Some(&sun), 1.0, 0.0, 2048, 16);
        assert_eq!(p.mean_luminance, mean_luminance(&want));
        assert_eq!(p.horizon_rgb, horizon_band(&want));
        assert_eq!(p.sh, ibl::sh9(&want));
        assert_eq!(p.dome_width, 2048, "the dome is not the lighting copy");
        let narrow = map(512, |_| grey(0.5));
        assert!(matches!(lighting_copy(&narrow, KeyRemoval::Nothing, LIGHTING_MAX_WIDTH), Cow::Borrowed(_)), "a narrower source is not copied");
        // A generated map's keyless copy is box-filtered the same way, and a
        // narrow one is the lighting copy itself.
        assert!(*lighting_copy(&narrow, KeyRemoval::Exact(&wide), LIGHTING_MAX_WIDTH) == wide.resized(LIGHTING_MAX_WIDTH));
        assert!(matches!(lighting_copy(&wide, KeyRemoval::Exact(&narrow), LIGHTING_MAX_WIDTH), Cow::Borrowed(m) if std::ptr::eq(m, &narrow)));
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

    fn serial(n: usize, f: &(dyn Fn(usize) + Sync)) {
        for i in 0..n {
            f(i)
        }
    }

    /// f(0..n) on every core, for the finer bakes.
    fn all_cores(n: usize, f: &(dyn Fn(usize) + Sync)) {
        let next = std::sync::atomic::AtomicUsize::new(0);
        let cores = std::thread::available_parallelism().map_or(4, |c| c.get()).min(16);
        std::thread::scope(|scope| {
            for _ in 0..cores {
                scope.spawn(|| loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if i >= n {
                        break;
                    }
                    f(i);
                });
            }
        });
    }

    /// A map's whole emission, ∫ L dΩ per channel, with exact texel solid angles.
    fn emission(env: &EnvMap) -> [f64; 3] {
        let (w, h) = (env.width, env.height);
        let pi = std::f64::consts::PI;
        let mut sum = [0.0f64; 3];
        for y in 0..h {
            let cell = std::f64::consts::TAU / w as f64 * ((pi * y as f64 / h as f64).cos() - (pi * (y + 1) as f64 / h as f64).cos());
            for t in &env.data[y * w..(y + 1) * w] {
                for k in 0..3 {
                    sum[k] += t[k] as f64 * cell;
                }
            }
        }
        sum
    }

    /// A preset with its key switched off where the key has a switch of its
    /// own: the key light disabled (a studio's, or a light over a sky), the
    /// sun's disc hidden. The moon's disc has none: `moon: false` takes the
    /// moonlit air and the moonlit ground with it.
    fn switched_off(params: &HdriParams, term: KeyTerm) -> Option<HdriParams> {
        let mut p = params.clone();
        match term {
            KeyTerm::Light => {
                let key = p.lights.iter().position(|l| l.enabled && l.key && l.blend() == Blend::Add)?;
                p.lights[key].enabled = false;
            }
            KeyTerm::Sun => p.sky.sun_disc.visible = false,
            KeyTerm::Moon(_) => return None,
            KeyTerm::Image => return None,
        }
        Some(p)
    }

    /// N3: a generated map's lighting copy is the map without exactly its
    /// key, so every preset's other lights survive in it: the Overcast dome's
    /// side panels under its 76 degree cover, a three-point's fill and rim,
    /// the sky and the clouds around the sun, the glow around the moon. The
    /// copy is compared with a bake of the preset with its key switched off
    /// (the key light disabled, the sun's disc hidden), its texels on and
    /// around the discs area-averaged as the map's are; the moon's disc has
    /// no switch of its own, so for the moonlit night it is a whole bake
    /// through the lookup without the disc. Each preset as it is and turned
    /// by 37 degrees (the key's reach is found in world space). Tolerance:
    /// none, texel for texel, because the copy IS that bake (the same
    /// function at the same texel centres, refined the same way; outside the
    /// key's reach, the map).
    #[test]
    fn every_presets_non_key_lights_survive_in_the_lighting_copy() {
        use crate::hdri::presets::{preset, PRESET_NAMES};
        let mut off = Vec::new();
        let mut keys = 0;
        for (name, rotation_deg) in PRESET_NAMES.iter().flat_map(|n| [(*n, 0.0f32), (*n, 37.0)]) {
            let params = HdriParams { rotation_deg, ..preset(name).unwrap() };
            let baked = super::super::envmap::bake_generated_env_map(&params, 64, serial);
            let env = Env::new(&params);
            let removal = baked.keyless.as_ref().map_or(KeyRemoval::Nothing, KeyRemoval::Exact);
            let lit = lighting_copy(&baked.map, removal, LIGHTING_MAX_WIDTH);
            let want = match env.key() {
                None => {
                    assert!(baked.keyless.is_none(), "{name}: a map without a key has no keyless copy");
                    baked.map.clone()
                }
                Some((term, _)) => {
                    keys += 1;
                    match switched_off(&params, term) {
                        Some(p) => {
                            let without = Env::new(&p);
                            let mut want = without.bake(64);
                            refine_hot_spots(&mut want, &|d| without.radiance(d), &env.hot_spots());
                            want
                        }
                        None => env.bake_without_key(64).unwrap(),
                    }
                }
            };
            assert_eq!((lit.width, lit.height), (want.width, want.height), "{name}");
            if lit.data != want.data {
                let (got, all) = (emission(&lit), emission(&want));
                let changed = lit.data.iter().zip(&want.data).filter(|(a, b)| a != b).count();
                off.push(format!(
                    "{name} turned {rotation_deg}: {changed} texels differ; the copy holds {:.4} {:.4} {:.4} of the light the map has without its key",
                    got[0] / all[0], got[1] / all[1], got[2] / all[2]
                ));
            }
        }
        assert_eq!(keys, 2 * (PRESET_NAMES.len() - 2), "every preset but blue hour and the starry night has a key");
        assert!(off.is_empty(), "the lighting copy vs the map without its key:\n{}", off.join("\n"));
    }

    /// N3: a generated key's light reaches the engine once. What the lighting
    /// copy lacks against the map, emission(map) - emission(copy), is the
    /// light the directional light carries, `EnvSun::irradiance`: nothing of
    /// the key stays in the copy (no double count) and nothing else leaves it
    /// (no loss); a key the clouds hide entirely (the Overcast preset's)
    /// takes nothing. Measured on 1024 wide maps, the engine's bake width.
    /// The tolerances are the bake's own sampling of the key, which N3 does
    /// not change: a studio key's texels hold its integral to 0.17 % at 1024
    /// (the thin ring; the broad keys to 0.003 %), a disc a fraction of a
    /// degree wide is area-averaged to a few percent (the moon's 0.52 degree
    /// disc, the worst, to 2.2 %). The cone fill fails it: the Overcast
    /// dome's cover held 13-16 % of the light beside the key.
    #[test]
    fn a_generated_keys_light_reaches_the_engine_once() {
        use crate::hdri::presets::{preset, PRESET_NAMES};
        let mut off = Vec::new();
        for name in PRESET_NAMES {
            let params = preset(name).unwrap();
            let baked = super::super::envmap::bake_generated_env_map(&params, 1024, all_cores);
            let Some(key) = baked.sun else { continue };
            let removal = baked.keyless.as_ref().map_or(KeyRemoval::Nothing, KeyRemoval::Exact);
            let (map, lit) = (emission(&baked.map), emission(&lighting_copy(&baked.map, removal, LIGHTING_MAX_WIDTH)));
            let removed = [map[0] - lit[0], map[1] - lit[1], map[2] - lit[2]];
            let light = key.irradiance();
            let light = [light.x as f64, light.y as f64, light.z as f64];
            if light == [0.0; 3] {
                if removed != [0.0; 3] {
                    off.push(format!("{name}: a key with no light took {removed:?}"));
                }
                continue;
            }
            let disc = matches!(Env::new(&params).key(), Some((KeyTerm::Sun | KeyTerm::Moon(_), _)));
            let tolerance = if disc { 0.03 } else { 0.002 };
            let ratio = [removed[0] / light[0], removed[1] / light[1], removed[2] / light[2]];
            if !ratio.iter().all(|r| (r - 1.0).abs() < tolerance) {
                off.push(format!("{name}: the copy lacks {:.4} {:.4} {:.4} of the key's light (within {tolerance})", ratio[0], ratio[1], ratio[2]));
            }
        }
        assert!(off.is_empty(), "the light the copy lacks vs the light the key carries:\n{}", off.join("\n"));
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
