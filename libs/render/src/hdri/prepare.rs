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
    use crate::hdri::caster::fixtures::caster_at;
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
    ///
    /// Every preset, and every kind of key plan 1b brings ([`keys_1b`], the
    /// plan owner's D1: the key is measured from the map's own drawing), so
    /// the next feature cannot break it silently. Each case names its
    /// tolerance, the sampling of the key by the bake and by the key's own
    /// integral, which N3 does not change: a studio key's texels hold its
    /// integral to 0.17 % at 1024 (the thin ring; the broad keys to 0.003 %),
    /// a disc a fraction of a degree wide is area-averaged to a few percent
    /// (the moon's 0.52 degree disc, the worst, to 2.2 %), and a key whose
    /// integral is jittered (a stroke's, a window's, what hangs in front of a
    /// key) holds it to its strata. The cone fill fails it: the Overcast
    /// dome's cover held 13-16 % of the light beside the key.
    #[test]
    fn a_generated_keys_light_reaches_the_engine_once() {
        use crate::hdri::presets::{preset, PRESET_NAMES};
        let mut cases: Vec<KeyCase> = PRESET_NAMES
            .iter()
            .map(|name| {
                let params = preset(name).unwrap();
                let disc = matches!(Env::new(&params).key(), Some((KeyTerm::Sun | KeyTerm::Moon(_), _)));
                KeyCase { name: name.to_string(), params, images: ImageSet::default(), tolerance: if disc { 0.03 } else { 0.002 } }
            })
            .collect();
        cases.extend(keys_1b());
        let mut off = Vec::new();
        for case in &cases {
            let name = &case.name;
            let baked = super::super::envmap::bake_generated_env_map_with(&case.params, &case.images, 1024, all_cores);
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
            let tolerance = case.tolerance;
            let ratio = [removed[0] / light[0], removed[1] / light[1], removed[2] / light[2]];
            println!("{name}: {:.4} {:.4} {:.4}", ratio[0], ratio[1], ratio[2]);
            if !ratio.iter().all(|r| (r - 1.0).abs() < tolerance) {
                off.push(format!("{name}: the copy lacks {:.4} {:.4} {:.4} of the key's light (within {tolerance})", ratio[0], ratio[1], ratio[2]));
            }
        }
        assert!(off.is_empty(), "the light the copy lacks vs the light the key carries:\n{}", off.join("\n"));
    }

    /// D1: a key the map does not show carries no light, so it casts no
    /// shadows from a light nobody sees: a stroke under a black flag, a
    /// softbox behind a window filled with sky (its glass shows the sky in
    /// the key's place), a picture that is all transparent, a sun behind
    /// opaque cloth or under the base image's horizon line, the moon behind a
    /// flag, a photographed sun behind a flag (a whole-base image's key).
    /// Each against the same set-up with nothing hiding the key, whose key
    /// has light.
    #[test]
    fn a_key_the_map_hides_carries_no_light() {
        let flag = |azimuth_deg: f32, elevation_deg: f32, size_deg: f32| LightParams {
            name: "Flag".to_string(),
            azimuth_deg,
            elevation_deg,
            width_deg: size_deg,
            height_deg: size_deg,
            corner: 0.0,
            softness: 0.0,
            blend: "multiply".to_string(),
            rgb: Some([0.0; 3]),
            ..LightParams::default()
        };
        let cloth = |azimuth_deg: f32, elevation_deg: f32| LightParams {
            name: "Scrim".to_string(),
            shape: "scrim".to_string(),
            azimuth_deg,
            elevation_deg,
            width_deg: 16.0,
            height_deg: 16.0,
            softness: 0.0,
            ..LightParams::default()
        };
        let photo = images_1b();
        let mut cases: Vec<(&str, HdriParams, HdriParams, &ImageSet)> = Vec::new();
        let open = studio_1b(vec![arch_key()]);
        cases.push(("a stroke under a black flag", studio_1b(vec![arch_key(), flag(90.0, 10.0, 90.0)]), open, &photo));
        // A window's glass shows the sky in place of what is behind it: one
        // pane, no frame, so all of it is glass where the softbox hangs.
        let softbox = LightParams { key: true, ..LightParams::default() };
        let pane = LightParams {
            name: "Window".to_string(),
            width_deg: 60.0,
            height_deg: 50.0,
            softness: 0.0,
            gobo: Some(GoboParams { panes_x: 1, panes_y: 1, frame: 0.0, fill_sky: true, ..GoboParams::default() }),
            ..LightParams::default()
        };
        cases.push(("a softbox behind a window filled with sky", studio_1b(vec![softbox.clone(), pane]), studio_1b(vec![softbox]), &photo));
        let clear = LightParams { image: Some(LightImageParams { key: "clear".to_string(), fit: "stretch".to_string(), alpha_as_mask: true, ..LightImageParams::default() }), ..picture_key() };
        cases.push(("an all-transparent picture", studio_1b(vec![clear]), studio_1b(vec![picture_key()]), &photo));
        let sun = manual_sun(150.0, 35.0);
        cases.push(("the sun behind opaque cloth", HdriParams { lights: vec![cloth(150.0, 35.0)], ..sun.clone() }, sun.clone(), &photo));
        cases.push(("the sun under the image's horizon line", HdriParams { base_image: Some(replace_sky(45.0, 0.0)), ..sun.clone() }, sun.clone(), &photo));
        let night = crate::hdri::presets::preset("Moonlit night").unwrap();
        let (moon_az, moon_el) = (night.sky.night.moon_azimuth_deg, night.sky.night.moon_elevation_deg);
        cases.push(("the moon behind a black flag", HdriParams { lights: vec![flag(moon_az, moon_el, 10.0)], ..night.clone() }, night, &photo));
        // A whole-base image brings its own sun: a flag over it in the map hides it from the key too.
        let photo_sun = HdriParams { base_image: Some(BaseImageParams { key: "photo_sun".to_string(), ..BaseImageParams::default() }), ..HdriParams::default() };
        cases.push(("the photographed sun behind a black flag", HdriParams { lights: vec![flag(120.0, 30.0, 10.0)], ..photo_sun.clone() }, photo_sun, &photo));
        // Redesign §5: a solid caster at strength 1 and a black tint hides
        // all of what is behind it, the sun's disc and a softbox key alike.
        let solid = |azimuth_deg: f32, elevation_deg: f32, width_deg: f32, height_deg: f32| caster_at(azimuth_deg, elevation_deg, width_deg, height_deg, None, CasterParams::default());
        cases.push(("4: sun behind a solid caster", HdriParams { lights: vec![solid(150.0, 35.0, 10.0, 10.0)], ..sun.clone() }, sun.clone(), &photo));
        let boxed = LightParams { key: true, ..LightParams::default() };
        cases.push(("4: softbox behind a solid caster", studio_1b(vec![boxed.clone(), solid(0.0, 30.0, 60.0, 50.0)]), studio_1b(vec![boxed]), &photo));
        let mut off = Vec::new();
        for (name, hidden, open, images) in cases {
            let seen = Env::new_with(&open, images).sun().map_or(0.0, |k| crate::sky::luminance(k.irradiance()));
            assert!(seen > 0.0, "{name}: the open set-up has a lit key");
            let hidden = Env::new_with(&hidden, images).sun().map_or(0.0, |k| crate::sky::luminance(k.irradiance()));
            if !(hidden <= 1.0e-6 * seen) {
                off.push(format!("{name}: the key carries {:.3e} of what it carries in the open", hidden / seen));
            }
        }
        assert!(off.is_empty(), "keys the map hides:\n{}", off.join("\n"));
    }

    /// One map of the end-to-end key test: its params, the pictures it
    /// names, and the tolerance its key is held to.
    struct KeyCase {
        name: String,
        params: HdriParams,
        images: ImageSet,
        tolerance: f64,
    }

    /// A studio of the given lights over the default grey backdrop, which a
    /// window or a scrim key hides where it hangs.
    fn studio_1b(lights: Vec<LightParams>) -> HdriParams {
        HdriParams { mode: "studio".to_string(), lights, ..HdriParams::default() }
    }

    /// The default sky with a manual sun.
    fn manual_sun(azimuth_deg: f32, elevation_deg: f32) -> HdriParams {
        let mut p = HdriParams::default();
        p.sky.sun.mode = "manual".to_string();
        p.sky.sun.azimuth_deg = azimuth_deg;
        p.sky.sun.elevation_deg = elevation_deg;
        p
    }

    /// A soft 4 degree arch as the key, with a hotspot.
    fn arch_key() -> LightParams {
        LightParams {
            key: true,
            shape: "stroke".to_string(),
            softness: 0.6,
            hotspot: 0.3,
            stroke: Some(StrokeParams { points: vec![[60.0, 5.0], [90.0, 15.0], [120.0, 5.0]], width_deg: 4.0, ..StrokeParams::default() }),
            ..LightParams::default()
        }
    }

    /// A 60 x 40 degree soft rect key showing the picture "frame".
    fn picture_key() -> LightParams {
        LightParams {
            key: true,
            azimuth_deg: 200.0,
            elevation_deg: 20.0,
            width_deg: 60.0,
            height_deg: 40.0,
            softness: 0.1,
            image: Some(LightImageParams { key: "frame".to_string(), fit: "stretch".to_string(), alpha_as_mask: true, ..LightImageParams::default() }),
            ..LightParams::default()
        }
    }

    /// P1: a 60 x 40 degree soft rect key showing the picture "logo" in fit
    /// (letterboxed above and below), its alpha cutting it out, with a
    /// penumbra, and its cut-out edges softened by `mask_softness` (P2).
    fn logo_key(penumbra: f32, mask_softness: f32) -> LightParams {
        LightParams {
            key: true,
            azimuth_deg: 150.0,
            elevation_deg: 25.0,
            width_deg: 60.0,
            height_deg: 40.0,
            softness: 0.1,
            image: Some(LightImageParams {
                key: "logo".to_string(),
                fit: "fit".to_string(),
                alpha_as_mask: true,
                penumbra: Some(penumbra),
                mask_softness: Some(mask_softness),
            }),
            ..LightParams::default()
        }
    }

    /// Sky replacement over the photograph "photo": its line at `horizon_deg`
    /// with a soft hand-over `feather_deg` wide.
    fn replace_sky(horizon_deg: f32, feather_deg: f32) -> BaseImageParams {
        BaseImageParams { key: "photo".to_string(), replace_sky: true, horizon_deg, feather_deg, ..BaseImageParams::default() }
    }

    /// The pictures the 1b cases name: "frame", a window frame whose frame
    /// and cross bars are transparent and whose glass runs warm to cool;
    /// "stripes", vertical opaque stripes on a transparent ground (a cut-out
    /// flag); "clear", all transparent; "logo" (P1), a 5:2 badge, an
    /// ellipse and four bars running warm to cool on a transparent black
    /// ground, 200 x 80 so Mask softness 1 (P2) blurs it over 4 texels; "photo", a 256 x 128 equirect of a blue sky over brown
    /// ground; and "photo_sun", the same with a 3 degree sun at azimuth 120,
    /// 30 degrees up.
    fn images_1b() -> ImageSet {
        let picture = |w: usize, h: usize, texel: &dyn Fn(usize, usize) -> [f32; 4]| EnvMap {
            width: w,
            height: h,
            data: (0..w * h).map(|i| texel(i % w, i / w)).collect(),
        };
        let mut images = ImageSet::default();
        images.insert(
            "frame",
            picture(48, 32, &|x, y| {
                let bar = x < 4 || x >= 44 || y < 4 || y >= 28 || (22..26).contains(&x) || (14..18).contains(&y);
                let t = x as f32 / 47.0;
                [1.5 - t, 1.0, 0.5 + t, if bar { 0.0 } else { 1.0 }]
            }),
        );
        images.insert("stripes", picture(32, 32, &|x, _| [0.0, 0.0, 0.0, if (x / 4) % 2 == 0 { 1.0 } else { 0.0 }]));
        images.insert("clear", picture(8, 8, &|_, _| [1.0, 1.0, 1.0, 0.0]));
        images.insert(
            "logo",
            picture(200, 80, &|x, y| {
                let (dx, dy) = ((x as f32 - 99.5) / 90.0, (y as f32 - 39.5) / 30.0);
                let bar = x % 50 < 15 && (20..60).contains(&y);
                let t = x as f32 / 199.0;
                if dx * dx + dy * dy < 1.0 || bar { [1.4 - t, 1.0, 0.4 + t, 1.0] } else { [0.0; 4] }
            }),
        );
        images.insert("photo", EnvMap::from_fn(256, |d| if d[1] > 0.0 { [0.6, 0.7, 0.9] } else { [0.12, 0.1, 0.08] }));
        let sun = dir_from_az_el(120.0, 30.0);
        images.insert("photo_sun", EnvMap::from_fn(256, |d| if vec(d).dot(sun) > 1.5f32.to_radians().cos() { [2.0e4; 3] } else if d[1] > 0.0 { [0.6, 0.7, 0.9] } else { [0.12, 0.1, 0.08] }));
        images
    }

    /// Every kind of key plan 1b brings, each where the map does something
    /// to it that the key must follow (D1): keys behind a window filled with
    /// sky, strokes under flags (a polygon flag's hotspot too), gels and
    /// cloth, a wide stroke, pictures, what hangs in front of a key, a scrim
    /// as the key (cut by its gobo, worn, under a flag, against the sky), a
    /// window filled with sky as the key (a storm's bolt in it too, in both
    /// modes), and the sun and the moon behind lights, under the base image's
    /// line, behind a rain shaft, in fog and under a cloud volume; and (R4)
    /// shadow casters in front of a softbox, a stroke, the sun and the moon,
    /// one with an image mask, and one listed before the key.
    fn keys_1b() -> Vec<KeyCase> {
        let images = images_1b();
        let soft_flag = |azimuth_deg: f32, elevation_deg: f32, size_deg: f32, rgb: [f32; 3]| LightParams {
            name: "Flag".to_string(),
            azimuth_deg,
            elevation_deg,
            width_deg: size_deg,
            height_deg: size_deg,
            softness: 0.6,
            blend: "multiply".to_string(),
            rgb: Some(rgb),
            ..LightParams::default()
        };
        let scrim = |azimuth_deg: f32, elevation_deg: f32, size_deg: f32, softness: f32| LightParams {
            name: "Scrim".to_string(),
            shape: "scrim".to_string(),
            azimuth_deg,
            elevation_deg,
            width_deg: size_deg,
            height_deg: size_deg,
            softness,
            scrim: Some(ScrimParams { source_size: 0.5, frame: 0.0 }),
            ..LightParams::default()
        };
        let with_stroke = |edit: &dyn Fn(&mut StrokeParams)| {
            let mut light = arch_key();
            edit(light.stroke.as_mut().unwrap());
            light
        };
        let sun = manual_sun(150.0, 35.0);
        let night = crate::hdri::presets::preset("Moonlit night").unwrap();
        let (moon_az, moon_el) = (night.sky.night.moon_azimuth_deg, night.sky.night.moon_elevation_deg);
        let weather = |edit: &dyn Fn(&mut WeatherParams)| {
            let mut w = WeatherParams::default();
            edit(&mut w);
            w
        };
        // A low sun behind the densest rain shaft along its circle.
        let virga_sun = {
            let mut p = manual_sun(0.0, 8.0);
            p.sky.weather = Some(weather(&|w| w.rain = 0.8));
            let rain = crate::hdri::weather::Weather::new(&p.sky, dir_from_az_el(0.0, 8.0)).expect("rain is weather");
            let through = |az: f32| rain.veil(dir_from_az_el(az, 8.0), vec3f(1.0, 1.0, 1.0), Vec3f::default(), Vec3f::default()).y;
            let az = (0..360).map(|a| a as f32).min_by(|a, b| through(*a).total_cmp(&through(*b))).unwrap();
            assert!(through(az) < 0.95 * (0..360).map(|a| through(a as f32)).fold(0.0, f32::max), "a shaft darkens the sun's circle");
            p.sky.sun.azimuth_deg = az;
            p
        };
        let window = LightParams {
            key: true,
            azimuth_deg: 0.0,
            elevation_deg: 20.0,
            width_deg: 40.0,
            height_deg: 30.0,
            softness: 0.05,
            intensity_ev: 1.0,
            gobo: Some(GoboParams { fill_sky: true, ..GoboParams::default() }),
            ..LightParams::default()
        };
        let sky_window = LightParams {
            key: false,
            azimuth_deg: 150.0,
            elevation_deg: 35.0,
            width_deg: 20.0,
            height_deg: 20.0,
            softness: 0.0,
            intensity_ev: 1.0,
            gobo: Some(GoboParams { panes_x: 1, panes_y: 1, frame: 0.0, fill_sky: true, ..GoboParams::default() }),
            ..LightParams::default()
        };
        // A window filled with sky listed after a key: its glass replaces
        // what is behind it, the key included, with the sky (its frame and
        // bars leave the key as it was).
        let window_over = |azimuth_deg: f32, elevation_deg: f32, width_deg: f32, height_deg: f32| LightParams {
            name: "Window".to_string(),
            azimuth_deg,
            elevation_deg,
            width_deg,
            height_deg,
            softness: 0.05,
            intensity_ev: 0.0,
            gobo: Some(GoboParams { fill_sky: true, ..GoboParams::default() }),
            ..LightParams::default()
        };
        let softbox = LightParams { key: true, azimuth_deg: 0.0, elevation_deg: 25.0, width_deg: 40.0, height_deg: 30.0, softness: 0.3, ..LightParams::default() };
        // A window filled with sky as the key, 3 degrees beside a storm's
        // bolt (rain with the Lightning tick), in Sky or Studio mode: the
        // channel, 0.12 degrees across, is far thinner than the key's samples
        // and the bake's texels alike (the review's item 4: these two held
        // 1.056 and 0.972 of the key; ten seeds now hold it within 0.6 %).
        let storm_window = |mode: &str, seed: u32, gobo: GoboParams| {
            let mut p = HdriParams { mode: mode.to_string(), ..HdriParams::default() };
            p.sky.weather = Some(weather(&|w| {
                w.kind = "rain".to_string();
                w.lightning = true;
                w.seed = seed;
            }));
            // Where the bolt stands: the brightest azimuth 10 degrees up.
            let storm = crate::hdri::weather::Weather::new(&p.sky, dir_from_az_el(180.0, 40.0)).expect("rain is weather");
            let channel = |az: f32| luminance(storm.lightning(dir_from_az_el(az, 10.0), 0.0));
            let bolt_az = (0..3600).map(|a| a as f32 * 0.1).max_by(|a, b| channel(*a).total_cmp(&channel(*b))).unwrap();
            p.lights = vec![LightParams {
                key: true,
                azimuth_deg: bolt_az + 3.0,
                elevation_deg: 14.0,
                width_deg: 30.0,
                height_deg: 30.0,
                softness: 0.05,
                intensity_ev: 0.0,
                gobo: Some(GoboParams { fill_sky: true, ..gobo }),
                ..LightParams::default()
            }];
            p
        };
        let one_pane = GoboParams { panes_x: 1, panes_y: 1, frame: 0.0, ..GoboParams::default() };
        // Shadow casters (redesign §5, `caster_at`): pattern layers that add
        // no light and darken what is listed before them.
        let canopy = GoboParams { kind: "leaves".to_string(), density: 0.5, seed: 4, ..GoboParams::default() };
        let stripes = GoboParams { kind: "image".to_string(), image: "stripes".to_string(), ..GoboParams::default() };
        let case = |name: &str, params: HdriParams, tolerance: f64| KeyCase { name: name.to_string(), params, images: images.clone(), tolerance };
        vec![
            case("1b: a softbox key half behind a window filled with sky", studio_1b(vec![softbox.clone(), window_over(18.0, 25.0, 30.0, 40.0)]), 0.01),
            case("1b: a stroke key half behind a window filled with sky", studio_1b(vec![arch_key(), window_over(110.0, 10.0, 30.0, 30.0)]), 0.01),
            case(
                "1b: a window filled with sky as the key, half behind another",
                studio_1b(vec![window.clone(), window_over(15.0, 25.0, 30.0, 30.0)]),
                0.02,
            ),
            case("1b: a stroke key half under a soft black flag", studio_1b(vec![arch_key(), soft_flag(112.0, 8.0, 24.0, [0.0; 3])]), 0.01),
            case("1b: a stroke key under a blue gel", studio_1b(vec![arch_key(), soft_flag(80.0, 12.0, 30.0, [0.3, 0.5, 1.0])]), 0.01),
            case(
                "1b: a ramped stroke key half behind a scrim",
                studio_1b(vec![with_stroke(&|s| s.end_rgb = Some([0.2, 0.4, 1.0])), scrim(72.0, 8.0, 30.0, 0.5)]),
                0.01,
            ),
            case(
                "1b: a wide closed stroke key (45 degrees)",
                studio_1b(vec![with_stroke(&|s| {
                    s.points = vec![[80.0, 0.0], [100.0, 0.0], [90.0, 15.0]];
                    s.width_deg = 45.0;
                    s.closed = true;
                })]),
                0.01,
            ),
            case(
                "1b: a stroke key behind leaves, worn",
                studio_1b(vec![LightParams {
                    gobo: Some(GoboParams { kind: "leaves".to_string(), density: 0.4, distance: 0.3, seed: 2, ..GoboParams::default() }),
                    imperfections: Some(ImperfectionParams { wrinkle: 0.8, dust: 0.4, weave: 0.5, seed: 1 }),
                    ..arch_key()
                }]),
                0.02,
            ),
            case("1b: a picture key with a masked window frame", studio_1b(vec![picture_key()]), 0.005),
            // P1: a picture's penumbra, and its softened cut-out edges, are in
            // the map's drawing, so the key carries them too.
            case("1b: a logo key in fit mode with a penumbra", studio_1b(vec![logo_key(0.5, 0.0)]), 0.005),
            case("1b: a logo key in fit mode, its cut-out edges softened", studio_1b(vec![logo_key(0.3, 1.0)]), 0.005),
            // P2: Mask softness is the picture's own, Penumbra or none.
            case("1b: a logo key, its cut-out edges softened without a penumbra", studio_1b(vec![logo_key(0.0, 0.6)]), 0.005),
            case(
                "1b: a picture key in stretch, its penumbra widening its soft edge",
                studio_1b(vec![{
                    let mut light = picture_key();
                    light.image.as_mut().unwrap().penumbra = Some(0.6);
                    light
                }]),
                0.005,
            ),
            case(
                "1b: a picture key behind a cut-out flag",
                studio_1b(vec![
                    picture_key(),
                    LightParams {
                        image: Some(LightImageParams { key: "stripes".to_string(), fit: "stretch".to_string(), alpha_as_mask: true, ..LightImageParams::default() }),
                        softness: 0.0,
                        ..soft_flag(210.0, 25.0, 30.0, [1.0; 3])
                    },
                ]),
                0.01,
            ),
            case(
                "1b: a rect key behind a window gobo, with barn doors, a grid and wear",
                studio_1b(vec![LightParams {
                    key: true,
                    azimuth_deg: 30.0,
                    elevation_deg: 25.0,
                    width_deg: 50.0,
                    height_deg: 40.0,
                    gobo: Some(GoboParams::default()),
                    aim: Some(AimParams { yaw_deg: 8.0, grid: 0.3, barn_doors: [30.0, 0.0, 45.0, 0.0], ..AimParams::default() }),
                    imperfections: Some(ImperfectionParams { wrinkle: 0.6, dust: 0.3, weave: 0.3, seed: 3 }),
                    ..LightParams::default()
                }]),
                0.02,
            ),
            case(
                "1b: a hexagon key and a scrim's cloth over it",
                studio_1b(vec![
                    LightParams { key: true, shape: "polygon".to_string(), azimuth_deg: 300.0, elevation_deg: 15.0, width_deg: 40.0, height_deg: 30.0, softness: 0.3, ..LightParams::default() },
                    scrim(310.0, 20.0, 24.0, 0.4),
                ]),
                0.002,
            ),
            // A scrim as the key (B5): the copy keeps its cloth and loses its
            // glow, so what the cloth hides stays hidden and the key is the
            // glow alone. Its window gobo cuts the cloth out where it blocks:
            // there the softbox behind shows in the map and the copy alike.
            case(
                "1b: a scrim key over a softbox, cut by a window gobo",
                studio_1b(vec![
                    LightParams { azimuth_deg: 40.0, elevation_deg: 20.0, width_deg: 20.0, height_deg: 16.0, softness: 0.3, ..LightParams::default() },
                    LightParams { key: true, gobo: Some(GoboParams::default()), scrim: Some(ScrimParams { source_size: 0.5, frame: 0.05 }), ..scrim(40.0, 20.0, 36.0, 0.3) },
                ]),
                0.02,
            ),
            case(
                "1b: a worn scrim key half under a soft black flag",
                studio_1b(vec![
                    LightParams { key: true, imperfections: Some(ImperfectionParams { wrinkle: 0.7, dust: 0.4, weave: 0.4, seed: 5 }), ..scrim(250.0, 15.0, 30.0, 0.5) },
                    soft_flag(262.0, 15.0, 20.0, [0.0; 3]),
                ]),
                0.02,
            ),
            case(
                // The cloth hides part of the sun, in the map and the copy alike;
                // the key is the cloth's glow, not the sun.
                "1b: a scrim key against the sky, the sun in its soft edge (Sky mode)",
                {
                    let light = LightParams { key: true, ..scrim(162.0, 35.0, 24.0, 1.0) };
                    let p = HdriParams { lights: vec![light.clone()], ..sun.clone() };
                    let env = Env::new(&p);
                    assert!(matches!(env.key(), Some((KeyTerm::Light, _))), "the scrim is the key");
                    let sun_dir = env.sun_dir().expect("a manual sun");
                    let cloth = crate::hdri::shaped::ShapedLight::new(&light).expect("a scrim").sample(sun_dir).mask;
                    assert!(cloth > 0.05 && cloth < 0.95, "the sun is in the cloth's soft edge: {cloth}");
                    p
                },
                0.02,
            ),
            // A polygon flag honours its hotspot (D10, L:26): it dims the key
            // most at its middle, in the map and in the key alike.
            case(
                "1b: a stroke key half under a polygon flag with a hotspot",
                studio_1b(vec![
                    arch_key(),
                    LightParams { shape: "polygon".to_string(), hotspot: 0.5, softness: 0.3, ..soft_flag(108.0, 10.0, 26.0, [0.0; 3]) },
                ]),
                0.01,
            ),
            case("1b: a window filled with sky as the key", studio_1b(vec![window.clone()]), 0.02),
            case(
                // The sun's disc is a fraction of a degree: the window's integral
                // must find it as the bake's refinement does.
                "1b: a window filled with sky as the key, the sun in it",
                {
                    let sun_dir = Env::new(&HdriParams::default()).sun_dir().expect("the default sky has a sun");
                    let (azimuth_deg, elevation_deg) = az_el_from_dir(sun_dir);
                    studio_1b(vec![LightParams { azimuth_deg, elevation_deg: elevation_deg - 4.0, intensity_ev: 0.0, ..window.clone() }])
                },
                0.03,
            ),
            case("1b: the sun in a soft scrim's edge", HdriParams { lights: vec![scrim(160.0, 35.0, 20.0, 1.0)], ..sun.clone() }, 0.03),
            case("1b: the sun behind a soft coloured gel", HdriParams { lights: vec![soft_flag(150.0, 36.0, 16.0, [0.6, 0.4, 0.2])], ..sun.clone() }, 0.03),
            case("1b: the sun in a window filled with sky at +1 EV", HdriParams { lights: vec![sky_window], ..sun.clone() }, 0.03),
            // D12 (M8): against the sky a window's frame and bars hide the sky
            // behind them; its key is still exactly the sky its glass lets in.
            case("1b: a framed window filled with sky as the key, against the sky (Sky mode)", HdriParams { lights: vec![window.clone()], ..HdriParams::default() }, 0.02),
            case("1b: a window filled with sky as the key beside a storm's bolt (Sky mode)", storm_window("sky", 2, one_pane.clone()), 0.02),
            case("1b: a window filled with sky as the key beside a storm's bolt (Studio mode)", storm_window("studio", 1, GoboParams::default()), 0.02),
            case("1b: the moon in a soft scrim's edge", HdriParams { lights: vec![scrim(moon_az, moon_el + 9.0, 16.0, 1.0)], ..night.clone() }, 0.03),
            case(
                "1b: the sun half under the image's horizon line",
                HdriParams { base_image: Some(replace_sky(6.0, 8.0)), ..manual_sun(120.0, 6.0) },
                0.03,
            ),
            case("1b: the sun behind a rain shaft", virga_sun, 0.03),
            case(
                "1b: the sun in fog",
                HdriParams { sky: SkyParams { weather: Some(weather(&|w| w.fog = 0.4)), ..sun.sky.clone() }, ..sun.clone() },
                0.03,
            ),
            case(
                // At the edge of a cloud: about half of the sun gets through.
                "1b: the sun at the edge of a cloud volume",
                {
                    let mut p = manual_sun(220.0, 35.0);
                    p.sky.clouds.coverage = 0.4;
                    p.sky.volumetric = Some(VolumetricParams::default());
                    p
                },
                0.03,
            ),
            case(
                "4: softbox key behind a leaves caster",
                studio_1b(vec![softbox.clone(), caster_at(10.0, 28.0, 50.0, 40.0, Some(canopy.clone()), CasterParams { strength: 0.9, ..CasterParams::default() })]),
                0.02,
            ),
            // Listed before the key, the caster darkens the backdrop under it and leaves the key as it is.
            case("4: caster listed before the key", studio_1b(vec![caster_at(10.0, 28.0, 50.0, 40.0, Some(canopy.clone()), CasterParams::default()), softbox.clone()]), 0.002),
            case(
                "4: sun behind a tinted caster",
                HdriParams {
                    lights: vec![caster_at(150.0, 35.0, 20.0, 20.0, Some(canopy.clone()), CasterParams { strength: 0.8, tint: [0.6, 0.4, 0.2], softness: 0.3, ..CasterParams::default() })],
                    ..sun.clone()
                },
                0.03,
            ),
            case(
                "4: moon behind a caster",
                HdriParams { lights: vec![caster_at(moon_az, moon_el, 16.0, 16.0, Some(canopy.clone()), CasterParams { softness: 0.4, ..CasterParams::default() })], ..night.clone() },
                0.03,
            ),
            case(
                "4: stroke key behind a soft caster",
                studio_1b(vec![arch_key(), caster_at(95.0, 10.0, 40.0, 30.0, Some(canopy.clone()), CasterParams { softness: 0.6, ..CasterParams::default() })]),
                0.02,
            ),
            case("4: key behind a caster with an image mask", studio_1b(vec![softbox.clone(), caster_at(5.0, 25.0, 50.0, 40.0, Some(stripes), CasterParams::default())]), 0.01),
            // R5: the sun half behind a solid caster's soft lower edge (the caster's
            // centre 10 degrees above the sun, 20 degrees tall). Its bloom thins the
            // caster round the disc, in the map and in the key alike: one raster,
            // read by the bake, the keyless copy and disc_share. Emission test only:
            // with the key disabled its bloom goes too, so "keyless == key
            // disabled" does not hold here.
            case(
                "5: sun behind a caster with bloom",
                HdriParams {
                    lights: vec![LightParams {
                        azimuth_deg: 150.0,
                        elevation_deg: 45.0,
                        width_deg: 30.0,
                        height_deg: 20.0,
                        gobo: None,
                        caster: Some(CasterParams { softness: 0.3, bloom_strength: 1.0, bloom_length_deg: 4.0, ..CasterParams::default() }),
                        ..LightParams::new_caster()
                    }],
                    ..sun.clone()
                },
                0.03,
            ),
        ]
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
