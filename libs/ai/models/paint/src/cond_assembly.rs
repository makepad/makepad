//! Exact conditioning / batch-assembly math for the UNet2p5D executor,
//! pinned against `hunyuanpaintpbr/pipeline.py` + `unet/modules.py`
//! @ 82920d643c0dc2f7bfd7255f45f62d386edfe60c. Everything here is pure,
//! deterministic, and executor-agnostic: the CUDA graph consumes these
//! layouts; tests lock them long before a GPU is involved.
//!
//! Pinned facts:
//! * latent input packing is `[noise 4ch | normal-latent 4ch | position-latent
//!   4ch]` = the UNet's 12 conv_in channels;
//! * batch flattening is `(b, n_pbr, n_view) -> (b*n_pbr + p)*n_views + v`
//!   (einops `"b n_pbr n c h w -> (b n_pbr n) c h w"`), materials = [albedo, mr];
//! * CFG runs a 3-branch batch `[negative, ref-only, ref+dino]`:
//!   prompt embeds `[zeros, learned, learned]` (77x1024 learned tokens per
//!   material), `ref_scale = [0, 1, 1]`, DINO states `[zeros, zeros, dino]`;
//! * guidance combine is `uncond + g*vs*(ref - uncond) + g*vs*(full - ref)`
//!   with the per-view azimuth scale `vs` (1 at the front view, 2 on
//!   sides/back) — kept in this exact two-term order for fp parity;
//! * multires voxel-RoPE indices quantize the per-view position maps at
//!   grids `[64,32,16,8]` with voxel resolutions `[512,256,128,64]`; background
//!   is white (`all channels == 1`), cell mean over valid pixels (computed in
//!   f16 like upstream), cells under 1/16 coverage zeroed, `round(p*(res-1))`.

pub const PBR_MATERIALS: usize = 2;
pub const LATENT_CHANNELS: usize = 4;
pub const PACKED_INPUT_CHANNELS: usize = 12;
pub const TEXT_TOKENS: usize = 77;
pub const TEXT_DIM: usize = 1024;
pub const CFG_BRANCHES: usize = 3;

/// Multires voxel-RoPE levels as (latent grid, voxel resolution).
pub const ROPE_LEVELS: [(usize, usize); 4] = [(64, 512), (32, 256), (16, 128), (8, 64)];

/// `[noise | normal | position]` channel-planar packing for one view's
/// latent input. All three inputs are `LATENT_CHANNELS * hw` planar f32.
pub fn pack_view_latent(noise: &[f32], normal_lat: &[f32], position_lat: &[f32], hw: usize) -> Vec<f32> {
    assert_eq!(noise.len(), LATENT_CHANNELS * hw);
    assert_eq!(normal_lat.len(), LATENT_CHANNELS * hw);
    assert_eq!(position_lat.len(), LATENT_CHANNELS * hw);
    let mut out = Vec::with_capacity(PACKED_INPUT_CHANNELS * hw);
    out.extend_from_slice(noise);
    out.extend_from_slice(normal_lat);
    out.extend_from_slice(position_lat);
    out
}

/// einops `(b n_pbr n)` flattening.
pub fn flat_batch_index(b: usize, material: usize, view: usize, n_views: usize) -> usize {
    (b * PBR_MATERIALS + material) * n_views + view
}

/// The 3-branch CFG conditioning table (batch is branch-major: the whole
/// `(b n_pbr n)` block repeats once per branch).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CfgBranchTable {
    /// Reference-attention scale per branch.
    pub ref_scale: [f32; CFG_BRANCHES],
    /// Whether the DINO hidden states are zeroed per branch.
    pub dino_zeroed: [bool; CFG_BRANCHES],
    /// Whether the learned prompt embeddings are zeroed per branch.
    pub prompt_zeroed: [bool; CFG_BRANCHES],
}

pub fn cfg_branch_table() -> CfgBranchTable {
    CfgBranchTable {
        ref_scale: [0.0, 1.0, 1.0],
        dino_zeroed: [true, true, false],
        // Official pipeline.py: `negative_prompt_embeds = stack(all_shading_tokens)`
        // — the uncond branch keeps the learned per-material shading tokens
        // (the `zeros_like` variant is commented out upstream). Only DINO
        // and the reference scale differ across branches.
        prompt_zeroed: [false, false, false],
    }
}

/// Upstream `cam_mapping`: per-view guidance weight from the view azimuth.
pub fn view_guidance_scale(azim: f32) -> f32 {
    if (0.0..90.0).contains(&azim) {
        azim / 90.0 + 1.0
    } else if (90.0..330.0).contains(&azim) {
        2.0
    } else {
        -azim / 90.0 + 5.0
    }
}

/// The exact two-term guidance combine (kept unsimplified for fp parity):
/// `uncond + g*vs*(ref - uncond) + g*vs*(full - ref)`, applied elementwise
/// with `vs` constant per flattened batch row.
pub fn guidance_combine(
    uncond: &[f32],
    ref_only: &[f32],
    full: &[f32],
    guidance_scale: f32,
    view_scales: &[f32],
    row_len: usize,
) -> Vec<f32> {
    assert_eq!(uncond.len(), ref_only.len());
    assert_eq!(uncond.len(), full.len());
    assert_eq!(uncond.len(), view_scales.len() * row_len);
    let mut out = Vec::with_capacity(uncond.len());
    for (row, vs) in view_scales.iter().enumerate() {
        let a = guidance_scale * *vs;
        let base = row * row_len;
        for k in 0..row_len {
            let u = uncond[base + k];
            let r = ref_only[base + k];
            let f = full[base + k];
            out.push(u + a * (r - u) + a * (f - r));
        }
    }
    out
}

/// Per-flattened-row view scales for a `(b=1, n_pbr, n_views)` batch:
/// repeated per material (upstream `.repeat(n_pbr, 1).view(-1)`).
pub fn view_scales_for_batch(azims: &[f32]) -> Vec<f32> {
    let mut out = Vec::with_capacity(PBR_MATERIALS * azims.len());
    for _ in 0..PBR_MATERIALS {
        for azim in azims {
            out.push(view_guidance_scale(*azim));
        }
    }
    out
}

/// Round-trip f32 through IEEE binary16 (round-to-nearest-even) — upstream
/// quantizes position maps to half before averaging; parity needs the same.
pub fn f16_round(v: f32) -> f32 {
    f16_to_f32(f32_to_f16_bits(v))
}

/// IEEE binary16 bits of an f32 (round-to-nearest-even) — public so weight
/// packing for the f16 linear paths shares one conversion with the parity
/// helpers.
pub fn f32_to_f16_bits(v: f32) -> u16 {
    let bits = v.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xff) as i32;
    let mant = bits & 0x007f_ffff;
    if exp == 255 {
        // Inf/NaN
        return sign | 0x7c00 | if mant != 0 { 0x200 } else { 0 };
    }
    let unbiased = exp - 127;
    if unbiased > 15 {
        return sign | 0x7c00; // overflow -> inf
    }
    if unbiased >= -14 {
        // Normal half: 10-bit mantissa with round-to-nearest-even on 13 bits.
        let mut half_exp = (unbiased + 15) as u32;
        let mut half_mant = mant >> 13;
        let rest = mant & 0x1fff;
        if rest > 0x1000 || (rest == 0x1000 && (half_mant & 1) == 1) {
            half_mant += 1;
            if half_mant == 0x400 {
                half_mant = 0;
                half_exp += 1;
                if half_exp >= 31 {
                    return sign | 0x7c00;
                }
            }
        }
        sign | ((half_exp as u16) << 10) | half_mant as u16
    } else if unbiased >= -24 {
        // Subnormal half: m_half = full_mant24 >> (-1 - unbiased), RNE.
        // A rounding carry into 0x400 lands on the smallest normal exactly.
        let full = mant | 0x0080_0000;
        let drop = (-1 - unbiased) as u32;
        let kept = full >> drop;
        let rest = full & ((1u32 << drop) - 1);
        let half_bit = 1u32 << (drop - 1);
        let mut half_mant = kept;
        if rest > half_bit || (rest == half_bit && (half_mant & 1) == 1) {
            half_mant += 1;
        }
        sign | half_mant as u16
    } else {
        sign // underflow to zero
    }
}

fn f16_to_f32(bits: u16) -> f32 {
    let sign = ((bits & 0x8000) as u32) << 16;
    let exp = ((bits >> 10) & 0x1f) as u32;
    let mant = (bits & 0x3ff) as u32;
    let out = if exp == 0 {
        if mant == 0 {
            sign
        } else {
            // Subnormal: normalize.
            let mut exp = 127 - 15 + 1;
            let mut mant = mant;
            while mant & 0x400 == 0 {
                mant <<= 1;
                exp -= 1;
            }
            sign | ((exp as u32) << 23) | ((mant & 0x3ff) << 13)
        }
    } else if exp == 31 {
        sign | 0x7f80_0000 | (mant << 13)
    } else {
        sign | ((exp + 127 - 15) << 23) | (mant << 13)
    };
    f32::from_bits(out)
}

/// Exact port of `compute_discrete_voxel_indice` for one view's position map
/// (RGB8, `size`x`size`, white background): returns `grid*grid` voxel index
/// triples. `grid` must divide `size`.
pub fn discrete_voxel_indices(
    position_rgb: &[u8],
    size: usize,
    grid: usize,
    voxel_resolution: usize,
) -> Vec<[u32; 3]> {
    assert_eq!(position_rgb.len(), size * size * 3);
    assert!(size % grid == 0, "grid {grid} must divide size {size}");
    let cell = size / grid;
    let thres = (cell * cell) / 16;
    let mut out = Vec::with_capacity(grid * grid);
    for gy in 0..grid {
        for gx in 0..grid {
            let mut sum = [0f32; 3];
            let mut count = 0usize;
            for py in gy * cell..(gy + 1) * cell {
                for px in gx * cell..(gx + 1) * cell {
                    let at = (py * size + px) * 3;
                    let r = position_rgb[at];
                    let g = position_rgb[at + 1];
                    let b = position_rgb[at + 2];
                    // Background is exactly white (all channels == 1.0).
                    if r == 255 && g == 255 && b == 255 {
                        continue;
                    }
                    sum[0] += f16_round(r as f32 / 255.0);
                    sum[1] += f16_round(g as f32 / 255.0);
                    sum[2] += f16_round(b as f32 / 255.0);
                    count += 1;
                }
            }
            let mut index = [0u32; 3];
            if count >= thres.max(1) {
                for c in 0..3 {
                    let mean = (sum[c] / count as f32).clamp(0.0, 1.0);
                    index[c] = (mean * (voxel_resolution - 1) as f32).round() as u32;
                }
            }
            out.push(index);
        }
    }
    out
}

/// All four RoPE levels for one view, keyed by token count (`grid*grid`).
pub fn multires_voxel_indices(position_rgb: &[u8], size: usize) -> Vec<(usize, usize, Vec<[u32; 3]>)> {
    ROPE_LEVELS
        .iter()
        .map(|(grid, voxel)| {
            (
                grid * grid,
                *voxel,
                discrete_voxel_indices(position_rgb, size, *grid, *voxel),
            )
        })
        .collect()
}

/// Official `calc_multires_voxel_idxs` grids for latent spatial size `lat`
/// (view resolution / 8): `(grid, voxel_resolution)`.
pub fn rope_levels_for_latent(lat: usize) -> [(usize, usize); 4] {
    [
        (lat, lat * 8),
        (lat / 2, lat * 4),
        (lat / 4, lat * 2),
        (lat / 8, lat),
    ]
}

/// Concatenate per-view voxel indices at one RoPE level (view-major).
pub fn voxel_xyz_for_views(
    position_maps: &[&[u8]],
    size: usize,
    grid: usize,
    voxel: usize,
) -> Result<Vec<[u32; 3]>, crate::test_backend::PbrError> {
    if position_maps.is_empty() {
        return Err(crate::test_backend::PbrError::InvalidParams(
            "voxel_xyz_for_views needs at least one view".into(),
        ));
    }
    let mut out = Vec::with_capacity(position_maps.len() * grid * grid);
    for rgb in position_maps {
        out.extend(discrete_voxel_indices(rgb, size, grid, voxel));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packing_order_is_noise_normal_position() {
        let hw = 2;
        let noise = vec![1.0f32; 8];
        let normal = vec![2.0f32; 8];
        let position = vec![3.0f32; 8];
        let packed = pack_view_latent(&noise, &normal, &position, hw);
        assert_eq!(packed.len(), PACKED_INPUT_CHANNELS * hw);
        assert_eq!(packed[0], 1.0);
        assert_eq!(packed[4 * hw], 2.0);
        assert_eq!(packed[8 * hw], 3.0);
    }

    #[test]
    fn flat_index_matches_einops_flatten() {
        let n_views = 6;
        let mut expect = 0;
        for b in 0..1 {
            for material in 0..PBR_MATERIALS {
                for view in 0..n_views {
                    assert_eq!(flat_batch_index(b, material, view, n_views), expect);
                    expect += 1;
                }
            }
        }
    }

    #[test]
    fn cfg_table_is_pinned() {
        let table = cfg_branch_table();
        assert_eq!(table.ref_scale, [0.0, 1.0, 1.0]);
        assert_eq!(table.dino_zeroed, [true, true, false]);
        assert_eq!(table.prompt_zeroed, [false, false, false]);
        assert_eq!(TEXT_TOKENS, 77);
        assert_eq!(TEXT_DIM, 1024);
    }

    #[test]
    fn view_scale_matches_cam_mapping() {
        assert_eq!(view_guidance_scale(0.0), 1.0);
        assert!((view_guidance_scale(45.0) - 1.5).abs() < 1e-6);
        assert_eq!(view_guidance_scale(90.0), 2.0);
        assert_eq!(view_guidance_scale(180.0), 2.0);
        assert_eq!(view_guidance_scale(270.0), 2.0);
        assert!((view_guidance_scale(330.0) - (5.0 - 330.0 / 90.0)).abs() < 1e-6);
        // Repeated per material, view-major.
        let scales = view_scales_for_batch(&[0.0, 90.0]);
        assert_eq!(scales.len(), 4);
        assert_eq!(scales[0], 1.0);
        assert_eq!(scales[1], 2.0);
        assert_eq!(scales[2], 1.0);
    }

    #[test]
    fn guidance_combine_telescopes_but_keeps_term_order() {
        let uncond = vec![0.0f32, 0.0];
        let ref_only = vec![10.0f32, -10.0];
        let full = vec![1.0f32, 1.0];
        let out = guidance_combine(&uncond, &ref_only, &full, 3.0, &[1.0], 2);
        // Algebraically uncond + g*vs*(full - uncond) = 3.0 * full.
        assert!((out[0] - 3.0).abs() < 1e-5);
        assert!((out[1] - 3.0).abs() < 1e-5);
    }

    #[test]
    fn f16_round_matches_known_values() {
        assert_eq!(f16_round(1.0), 1.0);
        assert_eq!(f16_round(0.5), 0.5);
        // 0.1 in binary16 is 0.0999755859375.
        assert!((f16_round(0.1) - 0.099_975_586).abs() < 1e-9);
        assert_eq!(f16_round(0.0), 0.0);
    }

    #[test]
    fn rope_levels_64_match_pin() {
        assert_eq!(rope_levels_for_latent(64), ROPE_LEVELS);
    }

    #[test]
    fn voxel_xyz_concats_views() {
        let size = 64;
        let a = vec![128u8; size * size * 3];
        let b = vec![200u8; size * size * 3];
        let xyz = voxel_xyz_for_views(&[&a, &b], size, 8, 64).unwrap();
        assert_eq!(xyz.len(), 2 * 8 * 8);
        let one = discrete_voxel_indices(&a, size, 8, 64);
        assert_eq!(&xyz[..64], one.as_slice());
    }

    #[test]
    fn voxel_indices_constant_map() {
        // Constant mid-gray position map: every cell mean = f16(128/255).
        // Size 64 is the smallest all four RoPE grids divide.
        let size = 64;
        let rgb = vec![128u8; size * size * 3];
        for (tokens, voxel, indices) in multires_voxel_indices(&rgb, size) {
            assert_eq!(indices.len(), tokens);
            let expect = (f16_round(128.0 / 255.0) * (voxel - 1) as f32).round() as u32;
            assert!(indices.iter().all(|v| *v == [expect; 3]), "level {voxel}");
        }
    }

    #[test]
    fn voxel_background_and_low_coverage_zeroed() {
        let size = 32;
        let grid = 8; // 4x4 cells; threshold = 16/16 = 1 valid pixel
        let mut rgb = vec![255u8; size * size * 3]; // all background
        // One valid pixel in cell (0,0).
        rgb[0] = 10;
        rgb[1] = 20;
        rgb[2] = 30;
        let indices = discrete_voxel_indices(&rgb, size, grid, 128);
        let expect = [
            (f16_round(10.0 / 255.0) * 127.0).round() as u32,
            (f16_round(20.0 / 255.0) * 127.0).round() as u32,
            (f16_round(30.0 / 255.0) * 127.0).round() as u32,
        ];
        assert_eq!(indices[0], expect);
        // Every other cell is pure background -> zero index.
        assert!(indices[1..].iter().all(|v| *v == [0, 0, 0]));
    }

    #[test]
    fn voxel_gradient_is_monotonic() {
        let size = 64;
        let mut rgb = vec![0u8; size * size * 3];
        for y in 0..size {
            for x in 0..size {
                let at = (y * size + x) * 3;
                rgb[at] = ((x * 254) / (size - 1)) as u8; // never 255 -> all valid
                rgb[at + 1] = 0;
                rgb[at + 2] = 0;
            }
        }
        let indices = discrete_voxel_indices(&rgb, size, 8, 512);
        for row in 0..8 {
            for col in 1..8 {
                assert!(indices[row * 8 + col][0] > indices[row * 8 + col - 1][0]);
            }
        }
    }
}
