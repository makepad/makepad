//! What the renderer prepares from an environment map, off the UI thread
//! (renderer/ibl.rs submits it as a Background job): the IBL lane texture
//! (render-material's prefiltered atlas + SH9 meta row), a full-resolution
//! dome for the background (at most 2048 wide, RGBA f32), and the numbers
//! the sun rig, the exposure meter and the fog take from the map.
//!
//! The lighting products are built from a copy at most
//! [`LIGHTING_MAX_WIDTH`] wide without the environment's key: the renderer's
//! directional light carries that energy, so the SH must not. By default
//! the atlas also loses the key; `keep_key` retains it below a roughness
//! cut while the renderer gives each directional lobe the complementary gain.
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
pub const ATLAS_LEVELS: usize = 6;

/// First atlas level without the key. GGX's half-width at half maximum in
/// light space is `2 * atan(0.644 * alpha)`, with `alpha = rough^2`: the
/// directional lobe may stand in for the key once it is at least as wide.
/// For `theta = acos(sun.cos_radius)`, `rough_cut = sqrt(tan(theta / 2) / 0.644)`
/// and `k = clamp(ceil(rough_cut * (levels - 1)), 1, levels - 1)`.
/// Fewer than two levels cannot hold both sources and return zero.
pub fn key_cut_level(sun: &EnvSun, levels: usize) -> usize {
    if levels < 2 {
        return 0;
    }
    // f64 keeps tan(theta / 2) on the positive side at the 180-degree limit.
    let theta = (sun.cos_radius as f64).clamp(-1.0, 1.0).acos();
    let rough_cut = ((theta * 0.5).tan() / 0.644).sqrt();
    ((rough_cut * (levels - 1) as f64).ceil() as usize).clamp(1, levels - 1)
}

/// One environment, prepared for the renderer.
#[derive(Clone, Debug, PartialEq)]
pub struct PreparedIbl {
    /// The lane texture (`TextureFormat::VecRGBAf32`, width x height).
    pub texture: ibl::IblTexture,
    /// Levels `0..key_levels` retain the declared key; zero for Light/no key.
    pub key_levels: u32,
    /// The key's colour at unit luminance and the measured fit (`ibl::KeyTint::uniform`); zero without keyed levels.
    pub key_tint: [f32; 4],
    /// The atlas level count, shared with the directional lobe's gain.
    pub levels: u32,
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
    prepare_ibl_until(env, removal, None, intensity, rotation_deg, sizes, &|| false, &|n, f| (0..n).for_each(f)).expect("a preparation nobody stops runs to its end")
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
/// dome, after each lighting copy and its key removal, before every prefilter
/// level, before the SH) and gives up (`None`) once it says so: a background
/// job whose environment was replaced stops within a stage instead of
/// finishing a preparation nobody will use.
/// With `keep_key` and a key removal, atlas levels below [`key_cut_level`]
/// read the full source narrowed to `sizes.lighting`; the remaining levels,
/// SH, meter and band read the keyless copy. The dome is unchanged.
/// Each level is prefiltered once. Cost: one extra lighting copy (about
/// 8 MB at 1024 x 512), one pass over it writing keyless luminance in alpha,
/// two passes measuring the key's colour and fit,
/// one extra 512-wide pyramid and one more accumulator per keyed prefilter
/// sample; no second prefilter. Without a key removal, both modes share the
/// same copy and do no extra work.
pub fn prepare_ibl_until(
    env: &EnvMap,
    removal: KeyRemoval,
    keep_key: Option<&EnvSun>,
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
    let key_levels = keep_key.filter(|_| !matches!(removal, KeyRemoval::Nothing)).map_or(0, |sun| key_cut_level(sun, ATLAS_LEVELS));
    let (full, key_tint) = if key_levels > 0 {
        let full = lighting_copy_spread(&env, KeyRemoval::Nothing, sizes.lighting, run);
        if stop() {
            return None;
        }
        let mut full = full.into_owned();
        full.set_alpha_luminance_of_spread(&lit, run);
        let tint = full.key_tint_spread(&lit, run);
        log!("ibl: key tint k=({:.4}, {:.4}, {:.4}) eps={:.3e} fit={:.3}", tint.k[0], tint.k[1], tint.k[2], tint.eps, tint.fit);
        if stop() {
            return None;
        }
        (Some(full), tint.uniform())
    } else {
        (None, [0.0; 4])
    };
    let atlas = if let Some(full) = full {
        ibl::prefilter_split_spread(&full, &lit, key_levels, sizes.atlas, ATLAS_LEVELS, stop, run)?
    } else {
        ibl::prefilter_spread(&lit, sizes.atlas, ATLAS_LEVELS, stop, run)?
    };
    if stop() {
        return None;
    }
    let sh = ibl::sh9(&lit);
    let texture = ibl::pack_ibl(&atlas, &sh, intensity, rotation_deg);
    Some(PreparedIbl {
        texture,
        key_levels: key_levels as u32,
        key_tint,
        levels: atlas.levels as u32,
        dome_width,
        dome_height,
        dome,
        sh,
        mean_luminance: mean_luminance(&lit),
        horizon_rgb: horizon_band(&lit),
    })
}

/// The copy the SH, the meter, the band and (by default) the atlas read, at
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

    #[test]
    fn key_cut_follows_the_declared_angular_radius_and_level_count() {
        for (degrees, expected) in [(22.0f32, 3), (40.0, 4), (0.27, 1), (89.0, ATLAS_LEVELS - 1), (180.0, ATLAS_LEVELS - 1)] {
            let sun = EnvSun { dir: vec3f(0.0, 1.0, 0.0), radiance: grey(1.0), cos_radius: degrees.to_radians().cos(), facing: 1.0, cos_cover: degrees.to_radians().cos() };
            assert_eq!(key_cut_level(&sun, ATLAS_LEVELS), expected, "{degrees} degrees: the GGX width selects the cut");
            assert_eq!(key_cut_level(&sun, 2), 1, "{degrees} degrees: two levels keep one keyed level");
            assert_eq!(key_cut_level(&sun, 1), 0, "{degrees} degrees: one level cannot split the sources");
        }
    }

    #[test]
    fn key_reflection_changes_only_the_atlas_and_light_keeps_its_construction() {
        let sun = EnvSun { dir: dir_from_az_el(90.0, 30.0), radiance: grey(20.0), cos_radius: 20.0f32.to_radians().cos(), facing: 1.0, cos_cover: 20.0f32.to_radians().cos() };
        let fill = |d: Vec3f| grey(0.2 + 0.3 * d.y.max(0.0));
        let keyless = map(64, fill);
        let env = map(64, |d| fill(d) + grey(if d.dot(sun.dir) >= sun.cos_radius { 20.0 } else { 0.0 }));
        let sizes = PrepareSizes { dome: 64, atlas: 16, lighting: 32 };
        let prepare = |removal, keep| prepare_ibl_until(&env, removal, keep, 1.0, 0.0, sizes, &|| false, &|n, f| (0..n).for_each(f)).expect("the small preparation completes");
        let bits = |data: &[f32]| data.iter().map(|v| v.to_bits()).collect::<Vec<_>>();
        let full = lighting_copy(&env, KeyRemoval::Nothing, sizes.lighting);
        let full_atlas = ibl::prefilter(&full, sizes.atlas, ATLAS_LEVELS);
        for (name, removal) in [("Exact", KeyRemoval::Exact(&keyless)), ("Fill", KeyRemoval::Fill(&sun))] {
            let light = prepare(removal, None);
            let kept = prepare(removal, Some(&sun));
            let lit = lighting_copy(&env, removal, sizes.lighting);
            let old = ibl::pack_ibl(&ibl::prefilter(&lit, sizes.atlas, ATLAS_LEVELS), &ibl::sh9(&lit), 1.0, 0.0);
            assert_eq!(bits(&light.texture.data), bits(&old.data), "{name}: Light is bit-identical to prefiltering the keyless lighting copy");
            let want = ibl::pack_ibl(&full_atlas, &light.sh, 1.0, 0.0);
            let k = key_cut_level(&sun, ATLAS_LEVELS);
            assert_eq!((light.key_levels, light.levels), (0, ATLAS_LEVELS as u32), "{name}: Light reports no keyed levels");
            assert_eq!((kept.key_levels, kept.levels), (k as u32, ATLAS_LEVELS as u32), "{name}: Map reports its derived cut and actual level count");
            let row = kept.texture.width * 4;
            assert_eq!(bits(&kept.texture.data[..row]), bits(&light.texture.data[..row]), "{name}: the meta row stays keyless");
            let level_size = row * full_atlas.level_height;
            for level in 0..ATLAS_LEVELS {
                let start = row + level * level_size;
                let end = start + level_size;
                let source = if level < k { &want } else { &light.texture };
                for texel in (start..end).step_by(4) {
                    let c = &kept.texture.data[texel..texel + 4];
                    assert_eq!(bits(&c[..3]), bits(&source.data[texel..texel + 3]), "{name}: level {level}, texel {texel}: RGB is bit-identical to its selected source (cut {k})");
                    let lum = |c: &[f32]| ibl::LUMINANCE[0] * c[0] + ibl::LUMINANCE[1] * c[1] + ibl::LUMINANCE[2] * c[2];
                    if level < k {
                        let a = c[3];
                        let b = lum(&light.texture.data[texel..texel + 3]);
                        assert!((a - b).abs() <= 1e-5 * a.abs().max(b.abs()).max(1e-6), "{name}: keyed level {level}, texel {texel}: alpha is Light's luminance (got {a}, expected {b})");
                    } else {
                        assert_eq!(c[3].to_bits(), lum(c).to_bits(), "{name}: keyless level {level}, texel {texel}: alpha is exactly its own RGB luminance");
                    }
                }
            }
            assert_ne!(bits(&kept.texture.data), bits(&light.texture.data), "{name}: the broad key contributes to Map reflections");
            assert_eq!(kept.sh.map(|c| c.map(f32::to_bits)), light.sh.map(|c| c.map(f32::to_bits)), "{name}: SH is bit-identical across reflection modes");
            assert_eq!(kept.mean_luminance.to_bits(), light.mean_luminance.to_bits(), "{name}: the exposure meter is bit-identical across reflection modes");
            assert_eq!(arr(kept.horizon_rgb).map(f32::to_bits), arr(light.horizon_rgb).map(f32::to_bits), "{name}: the horizon band is bit-identical across reflection modes");
            assert_eq!(bits(&kept.dome), bits(&light.dome), "{name}: the dome is bit-identical across reflection modes");
        }
        let light = prepare(KeyRemoval::Nothing, None);
        let kept = prepare(KeyRemoval::Nothing, Some(&sun));
        assert_eq!(bits(&kept.texture.data), bits(&light.texture.data), "without key removal the texture is bit-identical across reflection modes");
        assert_eq!(kept, light, "without key removal every preparation product is unchanged");
    }

    #[test]
    fn key_cut_weights_partition_and_keyed_minus_keyless_lookup_identity() {
        let cos_radius = 22.0f32.to_radians().cos();
        let sun = EnvSun { dir: dir_from_az_el(90.0, 30.0), radiance: grey(20.0), cos_radius, facing: 1.0, cos_cover: cos_radius };
        let fill = |d: Vec3f| grey(0.2 + 0.3 * d.y.max(0.0));
        let keyless = map(64, fill);
        let env = map(64, |d| fill(d) + grey(if d.dot(sun.dir) >= cos_radius { 20.0 } else { 0.0 }));
        let sizes = PrepareSizes { dome: 64, atlas: 16, lighting: 32 };
        let prepare = |keep| prepare_ibl_until(&env, KeyRemoval::Exact(&keyless), keep, 1.0, 0.0, sizes, &|| false, &|n, f| (0..n).for_each(f)).expect("the small key-cut preparation completes");
        let light = prepare(None);
        let kept = prepare(Some(&sun));
        let full = lighting_copy(&env, KeyRemoval::Nothing, sizes.lighting);
        let full_atlas = ibl::prefilter(&full, sizes.atlas, ATLAS_LEVELS);
        let levels = kept.levels as usize;
        let k = kept.key_levels as usize;
        assert_eq!((k, levels), (3, 6), "the 22-degree key cuts the six-level atlas at level three");
        for step in 0..=10 {
            let rough = step as f32 / 10.0;
            // mat_ibl_sky_env in render_material/src/builtin.rs (also used
            // by SdfEnv.env_reflect): the next level has zero weight at an
            // integer lf, so this is the floor/ceil blend there too.
            let lf = rough.clamp(0.0, 1.0) * (levels - 1) as f32;
            let k0 = lf.floor() as usize;
            let k1 = (k0 + 1).min(levels - 1);
            let t = lf - k0 as f32;
            let weights = [(k0, 1.0 - t), (k1, t)];
            let keyed_weight: f32 = weights.iter().filter(|(i, _)| *i < k).map(|(_, w)| *w).sum();
            let lobe_gain = 1.0 - (k as f32 - lf).clamp(0.0, 1.0);
            assert!((keyed_weight + lobe_gain - 1.0).abs() <= 1e-6, "roughness {rough}: keyed level weights plus the lobe gain partition unity (atlas {keyed_weight}, lobe {lobe_gain})");
            for (direction, dir) in [("key", sun.dir), ("90 degrees from key", vec3f(-sun.dir.y, sun.dir.x, 0.0))] {
                // Read the nearest texel centre of each level, rather than
                // bilinear sampling; only the level blend follows the shader.
                let uv = ibl::dir_to_equirect_uv(arr(dir));
                let x = (uv[0] * full_atlas.width as f32).floor() as usize % full_atlas.width;
                let y = ((uv[1] * full_atlas.level_height as f32).floor() as usize).min(full_atlas.level_height - 1);
                for channel in 0..3 {
                    let read = |texture: &ibl::IblTexture, i: usize| {
                        let row = 1 + i * full_atlas.level_height + y;
                        texture.data[(row * texture.width + x) * 4 + channel] as f64
                    };
                    // Accumulate the f32 texel values in f64 so cancellation
                    // does not mask the relative error of a small difference.
                    let blend = |texture: &ibl::IblTexture| read(texture, k0) * (1.0 - t) as f64 + read(texture, k1) * t as f64;
                    let map_lookup = blend(&kept.texture);
                    let light_lookup = blend(&light.texture);
                    let want: f64 = weights.iter().filter(|(i, _)| *i < k).map(|&(i, w)| {
                        let full_i = full_atlas.data[(i * full_atlas.level_height + y) * full_atlas.width + x][channel] as f64;
                        w as f64 * (full_i - read(&light.texture, i))
                    }).sum();
                    let got = map_lookup - light_lookup;
                    let scale = got.abs().max(want.abs());
                    assert!((got - want).abs() <= 1e-5 * scale, "roughness {rough}, {direction}, channel {channel}: Map minus Light equals the weighted full-minus-Light sum below the cut (got {got}, expected {want})");
                }
            }
        }
    }

    #[test]
    fn keyless_share_of_a_keyed_lookup_is_the_keyless_lookup() {
        let half_w = 22.5f32.to_radians().tan();
        let half_h = 17.5f32.to_radians().tan();
        let cos_radius = 1.0 / (1.0 + half_w * half_w + half_h * half_h).sqrt();
        let sun = EnvSun { dir: dir_from_az_el(250.0, 52.0), radiance: vec3f(24.0, 18.0, 12.0), cos_radius, facing: 1.0, cos_cover: cos_radius };
        let right = dir_from_az_el(340.0, 0.0);
        let up = Vec3f::cross(right, sun.dir);
        let fill = |d: Vec3f| vec3f(0.08 + 0.25 * d.y.max(0.0), 0.16 + 0.1 * (d.x + 1.0), 0.28 + 0.15 * (d.z + 1.0));
        let keyless = map(64, fill);
        // A hard-edged gnomonic rectangle, offset from the source texel grid.
        let env = map(64, |d| {
            let c = d.dot(sun.dir);
            let inside = c > 0.0 && (d.dot(right) / c).abs() <= half_w && (d.dot(up) / c).abs() <= half_h;
            fill(d) + if inside { sun.radiance } else { grey(0.0) }
        });
        let sizes = PrepareSizes { dome: 64, atlas: 16, lighting: 32 };
        let prepare = |keep| prepare_ibl_until(&env, KeyRemoval::Exact(&keyless), keep, 1.0, 0.0, sizes, &|| false, &|n, f| (0..n).for_each(f)).expect("the small softbox preparation completes");
        let kept = prepare(Some(&sun));
        let light = prepare(None);
        assert_eq!(kept.key_tint[3], 1.0, "a one-colour key fits");
        assert_eq!(light.key_tint, [0.0; 4], "Light has no key tint");

        // builtin.rs mat_ibl_level and mat_ibl_sky_env, rotation 0, in f64.
        fn lookup(texture: &ibl::IblTexture, dir: Vec3f, rough: f64) -> [f64; 4] {
            let d = arr(dir).map(f64::from);
            let u = 0.5 + d[0].atan2(-d[2]) / 6.2831853;
            let uv = [u - u.floor(), d[1].clamp(-1.0, 1.0).acos() / 3.14159265];
            let levels = texture.data[36] as usize;
            let h = texture.data[37] as usize;
            let w = texture.width;
            let mix = |a: [f64; 4], b: [f64; 4], t: f64| -> [f64; 4] { std::array::from_fn(|i| a[i] * (1.0 - t) + b[i] * t) };
            let level = |k: usize| {
                let px = uv[0] * w as f64 - 0.5;
                let py = (uv[1] * h as f64 - 0.5).clamp(0.0, (h - 1) as f64);
                let x0 = px.floor() as i64;
                let y0 = py.floor() as usize;
                let fx = px - x0 as f64;
                let fy = py - y0 as f64;
                let texel = |x: i64, y: usize| -> [f64; 4] {
                    let x = x.rem_euclid(w as i64) as usize;
                    let offset = ((1 + k * h + y) * w + x) * 4;
                    std::array::from_fn(|i| texture.data[offset + i] as f64)
                };
                let y1 = (y0 + 1).min(h - 1);
                let a = mix(texel(x0, y0), texel(x0 + 1, y0), fx);
                let b = mix(texel(x0, y1), texel(x0 + 1, y1), fx);
                mix(a, b, fy)
            };
            let lf = rough.clamp(0.0, 1.0) * (levels - 1) as f64;
            let k0 = lf.floor() as usize;
            let k1 = (k0 + 1).min(levels - 1);
            mix(level(k0), level(k1), lf - k0 as f64)
        }

        let mut directions = Vec::with_capacity(64);
        let projected = |x: f32, y: f32| (sun.dir + right * x + up * y).normalize();
        for y in [-0.6, -0.2, 0.2, 0.6] {
            for x in [-0.6, -0.2, 0.2, 0.6] {
                directions.push(("interior", projected(x * half_w, y * half_h)));
            }
        }
        // Both sides of all four edges, less than one source texel away.
        for t in [-0.75, -0.25, 0.25, 0.75] {
            for (class, offset) in [("edge inside", -1.0f32.to_radians()), ("edge outside", 1.0f32.to_radians())] {
                directions.push((class, projected(-half_w - offset, t * half_h)));
                directions.push((class, projected(half_w + offset, t * half_h)));
                directions.push((class, projected(t * half_w, -half_h - offset)));
                directions.push((class, projected(t * half_w, half_h + offset)));
            }
        }
        for (az, el) in [(-180.0, -89.0), (180.0, -89.0), (-179.9, 0.0), (179.9, 0.0), (0.0, 89.0), (0.0, -89.0), (0.0, -60.0), (90.0, -60.0), (180.0, -60.0), (270.0, -60.0), (0.0, 0.0), (90.0, 0.0), (180.0, 0.0), (270.0, 0.0), (45.0, 60.0), (135.0, 60.0)] {
            directions.push(("away", dir_from_az_el(az, el)));
        }
        let lum = |c: [f64; 4]| ibl::LUMINANCE[0] as f64 * c[0] + ibl::LUMINANCE[1] as f64 * c[1] + ibl::LUMINANCE[2] as f64 * c[2];
        let k = kept.key_tint.map(f64::from);
        for (index, (class, dir)) in directions.into_iter().enumerate() {
            for step in 0..=20 {
                let rough = step as f64 / 20.0;
                let e = lookup(&kept.texture, dir, rough);
                let e_light = lookup(&light.texture, dir, rough);
                let q = (e[3] / lum(e).max(1e-8)).clamp(0.0, 1.0);
                let a = lum(e) * q;
                let b = lum(e_light);
                assert!((a - b).abs() <= 1e-5 * a.abs().max(b.abs()).max(1e-6), "{class} direction {index}, roughness {rough}: the keyed lookup's keyless share equals Light's lookup luminance (got {a}, expected {b})");
                for c in 0..3 {
                    let got = e[c] - (lum(e) - e[3]) * k[c];
                    let want = e_light[c];
                    assert!((got - want).abs() <= 1e-3 * lum(e_light).max(1e-6), "{class} direction {index}, roughness {rough}, channel {c}: the keyed lookup minus the key in its colour is Light's lookup (got {got}, want {want})");
                }
            }
        }
        let env = map(64, |d| {
            let c = d.dot(sun.dir);
            let inside = c > 0.0 && (d.dot(right) / c).abs() <= half_w && (d.dot(up) / c).abs() <= half_h;
            fill(d) + if inside { if d.dot(right) / c > 0.0 { vec3f(12.0, 18.0, 24.0) } else { sun.radiance } } else { grey(0.0) }
        });
        let two_colours = prepare_ibl_until(&env, KeyRemoval::Exact(&keyless), Some(&sun), 1.0, 0.0, sizes, &|| false, &|n, f| (0..n).for_each(f)).expect("the small two-colour preparation completes");
        assert_eq!(two_colours.key_tint[3], 0.0, "a key of two colours falls back to the luminance share");
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
        assert_eq!(prepare_ibl_until(&env, KeyRemoval::Nothing, None, 1.0, 0.0, sizes, &never, &|n, f| (0..n).for_each(f)), Some(prepare_ibl_sized(&env, None, 1.0, 0.0, 2048, 16)));
        let stages = asked.load(Ordering::Relaxed);
        assert_eq!(stages, 2 + ATLAS_LEVELS * (1 + sizes.atlas / 2) + 1, "after the dome, lighting copy, before every level and row, before the SH");
        for k in 0..stages {
            asked.store(0, Ordering::Relaxed);
            let at_k = || {
                asked.fetch_add(1, Ordering::Relaxed);
                asked.load(Ordering::Relaxed) > k
            };
            assert_eq!(prepare_ibl_until(&env, KeyRemoval::Nothing, None, 1.0, 0.0, sizes, &at_k, &|n, f| (0..n).for_each(f)), None, "stopped at question {k}");
            assert_eq!(asked.load(Ordering::Relaxed), k + 1, "nothing asked after the stop");
        }
        asked.store(0, Ordering::Relaxed);
        let sun = EnvSun { dir: vec3f(0.0, 1.0, 0.0), radiance: grey(1.0), cos_radius: 22.0f32.to_radians().cos(), facing: 1.0, cos_cover: 22.0f32.to_radians().cos() };
        let keep = prepare_ibl_until(&env, KeyRemoval::Nothing, Some(&sun), 1.0, 0.0, sizes, &never, &|n, f| (0..n).for_each(f));
        assert!(keep.is_some(), "keeping an undeclared key completes the preparation");
        assert_eq!(asked.load(Ordering::Relaxed), stages, "without removal Map adds no preparation stage");
        asked.store(0, Ordering::Relaxed);
        let after_full_copy = || asked.fetch_add(1, Ordering::Relaxed) >= 2;
        assert_eq!(prepare_ibl_until(&env, KeyRemoval::Exact(&env), Some(&sun), 1.0, 0.0, sizes, &after_full_copy, &|n, f| (0..n).for_each(f)), None, "Map can stop after the extra lighting copy before prefiltering");
        assert_eq!(asked.load(Ordering::Relaxed), 3, "the extra copy's stop prevents every prefilter stage");
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
