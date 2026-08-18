//! Public Hunyuan3D-Paint-2.1 UNet2p5D graph (write-cache + extras-on).
//!
//! Lifted from the 15-step DDIM canary so [`crate::native_exec::NativeHunyuanExec`]
//! can call one copy of the walk. Cache keys are the **official 16 write-mode
//! names** from [`crate::dual_stream::write_layer_names`]:
//!
//! `down_0_0_0`, `down_0_1_0`, `down_1_0_0`, `down_1_1_0`, `down_2_0_0`,
//! `down_2_1_0`, `mid_0_0`, `up_1_0_0`, `up_1_1_0`, `up_1_2_0`, `up_2_0_0`,
//! `up_2_1_0`, `up_2_2_0`, `up_3_0_0`, `up_3_1_0`, `up_3_2_0`.
//!
//! Official dump files prefix those as `dual_{name}`. Remap before passing a
//! dump cache into [`forward_extras_on`]. Write and extras-on share the same
//! keys.

use crate::dual_stream::REF_TIMESTEP;
use crate::unet_extras::{ExtraFlags, ExtraInputs, ExtraResident};
use crate::unet_first::{paint_fast, unpack_planar_host, BatchAct, UnetFirst};
use makepad_ggml::backend::cuda::{
    gpu_add, gpu_concat_cols, gpu_concat_rows, gpu_download, gpu_mul, gpu_paint_scale,
    gpu_slice_cols, gpu_slice_rows, gpu_upload, gpu_upload_u32, GpuTensor,
};
use std::collections::HashMap;

/// PoseRoPE voxel bins at one UNet spatial scale: `n_views * w * h` xyz.
pub struct VoxelLevel<'a> {
    pub xyz: &'a [[u32; 3]],
    pub res: usize,
}

/// Four-level voxel pyramid matching UNet spatial scales
/// `lat`, `lat/2`, `lat/4`, `lat/8`.
pub struct VoxelPyramid<'a> {
    pub full: VoxelLevel<'a>,
    pub half: VoxelLevel<'a>,
    pub quarter: VoxelLevel<'a>,
    pub eighth: VoxelLevel<'a>,
}

/// Three CFG branches: uncond (zeros, `ref_scale=0`), ref-only (learned,
/// DINO zeros, `ref_scale=1`), full (learned + DINO, `ref_scale=1`).
pub struct CfgThreeBranch {
    pub uncond: Vec<Vec<f32>>,
    pub ref_only: Vec<Vec<f32>>,
    pub full: Vec<Vec<f32>>,
}

impl CfgThreeBranch {
    /// Flatten to `[uncond | ref_only | full]` planar 4-ch packs.
    /// Matches [`crate::denoise::DenoiseBatch::apply_cfg_step`].
    pub fn stacked(&self) -> Vec<f32> {
        let mut out = flatten_vpreds(&self.uncond);
        out.extend(flatten_vpreds(&self.ref_only));
        out.extend(flatten_vpreds(&self.full));
        out
    }
}

/// Official t=0 `mode=w` extras-off `unet_dual` walk.
///
/// `ref_latents_4ch` is planar 4-ch VAE latents (one per reference view).
/// Returns inner `norm1` tokens keyed by the official 16 names above.
pub fn write_dual_cache(
    unet: &UnetFirst,
    ref_latents_4ch: &[&[f32]],
    lat_w: usize,
    lat_h: usize,
) -> Result<HashMap<String, Vec<f32>>, String> {
    if ref_latents_4ch.is_empty() {
        return Err("write_dual_cache expects at least one 4-ch ref latent".into());
    }
    if lat_w == 0 || lat_h == 0 || lat_w % 8 != 0 || lat_h % 8 != 0 {
        return Err(format!("write_dual_cache latent {lat_w}x{lat_h} must be a positive multiple of 8"));
    }
    let s8w = lat_w;
    let s8h = lat_h;
    let s4w = s8w / 2;
    let s4h = s8h / 2;
    let s2w = s4w / 2;
    let s2h = s4h / 2;
    let s1w = s2w / 2;
    let s1h = s2h / 2;
    let temb = unet.timestep_embedding_named(REF_TIMESTEP, "unet_dual.time_embedding")?;
    let enc = unet.learned_text_clip_ref()?;
    let p = "unet_dual";
    let mut xs: Vec<Vec<f32>> = ref_latents_4ch
        .iter()
        .map(|x| unet.conv_in_dual(x, s8w, s8h))
        .collect::<Result<_, _>>()?;
    let mut skips: Vec<Vec<Vec<f32>>> = vec![xs.clone()];
    let mut cache = HashMap::new();

    xs = map_n_resnet(unet, &xs, s8w, s8h, &temb, &format!("{p}.down_blocks.0.resnets.0"), 320)?;
    xs = stash_attn(unet, &xs, s8w, s8h, &enc, &format!("{p}.down_blocks.0.attentions.0"), 320, 5, &mut cache, "down_0_0_0")?;
    skips.push(xs.clone());
    xs = map_n_resnet(unet, &xs, s8w, s8h, &temb, &format!("{p}.down_blocks.0.resnets.1"), 320)?;
    xs = stash_attn(unet, &xs, s8w, s8h, &enc, &format!("{p}.down_blocks.0.attentions.1"), 320, 5, &mut cache, "down_0_1_0")?;
    skips.push(xs.clone());
    xs = map_n_down(unet, &xs, s8w, s8h, &format!("{p}.down_blocks.0.downsamplers.0.conv"), 320)?;
    skips.push(xs.clone());

    xs = map_n_resnet(unet, &xs, s4w, s4h, &temb, &format!("{p}.down_blocks.1.resnets.0"), 320)?;
    xs = stash_attn(unet, &xs, s4w, s4h, &enc, &format!("{p}.down_blocks.1.attentions.0"), 640, 10, &mut cache, "down_1_0_0")?;
    skips.push(xs.clone());
    xs = map_n_resnet(unet, &xs, s4w, s4h, &temb, &format!("{p}.down_blocks.1.resnets.1"), 640)?;
    xs = stash_attn(unet, &xs, s4w, s4h, &enc, &format!("{p}.down_blocks.1.attentions.1"), 640, 10, &mut cache, "down_1_1_0")?;
    skips.push(xs.clone());
    xs = map_n_down(unet, &xs, s4w, s4h, &format!("{p}.down_blocks.1.downsamplers.0.conv"), 640)?;
    skips.push(xs.clone());

    xs = map_n_resnet(unet, &xs, s2w, s2h, &temb, &format!("{p}.down_blocks.2.resnets.0"), 640)?;
    xs = stash_attn(unet, &xs, s2w, s2h, &enc, &format!("{p}.down_blocks.2.attentions.0"), 1280, 20, &mut cache, "down_2_0_0")?;
    skips.push(xs.clone());
    xs = map_n_resnet(unet, &xs, s2w, s2h, &temb, &format!("{p}.down_blocks.2.resnets.1"), 1280)?;
    xs = stash_attn(unet, &xs, s2w, s2h, &enc, &format!("{p}.down_blocks.2.attentions.1"), 1280, 20, &mut cache, "down_2_1_0")?;
    skips.push(xs.clone());
    xs = map_n_down(unet, &xs, s2w, s2h, &format!("{p}.down_blocks.2.downsamplers.0.conv"), 1280)?;
    skips.push(xs.clone());

    xs = map_n_resnet(unet, &xs, s1w, s1h, &temb, &format!("{p}.down_blocks.3.resnets.0"), 1280)?;
    skips.push(xs.clone());
    xs = map_n_resnet(unet, &xs, s1w, s1h, &temb, &format!("{p}.down_blocks.3.resnets.1"), 1280)?;
    skips.push(xs.clone());

    xs = map_n_resnet(unet, &xs, s1w, s1h, &temb, &format!("{p}.mid_block.resnets.0"), 1280)?;
    xs = stash_attn(unet, &xs, s1w, s1h, &enc, &format!("{p}.mid_block.attentions.0"), 1280, 20, &mut cache, "mid_0_0")?;
    xs = map_n_resnet(unet, &xs, s1w, s1h, &temb, &format!("{p}.mid_block.resnets.1"), 1280)?;

    let pop = |skips: &mut Vec<Vec<Vec<f32>>>| skips.pop().unwrap();
    xs = map_n_resnet(unet, &cat_skip(&xs, &pop(&mut skips)), s1w, s1h, &temb, &format!("{p}.up_blocks.0.resnets.0"), 2560)?;
    xs = map_n_resnet(unet, &cat_skip(&xs, &pop(&mut skips)), s1w, s1h, &temb, &format!("{p}.up_blocks.0.resnets.1"), 2560)?;
    xs = map_n_resnet(unet, &cat_skip(&xs, &pop(&mut skips)), s1w, s1h, &temb, &format!("{p}.up_blocks.0.resnets.2"), 2560)?;
    xs = map_n_up(unet, &xs, s1w, s1h, &format!("{p}.up_blocks.0.upsamplers.0.conv"), 1280)?;

    xs = map_n_resnet(unet, &cat_skip(&xs, &pop(&mut skips)), s2w, s2h, &temb, &format!("{p}.up_blocks.1.resnets.0"), 2560)?;
    xs = stash_attn(unet, &xs, s2w, s2h, &enc, &format!("{p}.up_blocks.1.attentions.0"), 1280, 20, &mut cache, "up_1_0_0")?;
    xs = map_n_resnet(unet, &cat_skip(&xs, &pop(&mut skips)), s2w, s2h, &temb, &format!("{p}.up_blocks.1.resnets.1"), 2560)?;
    xs = stash_attn(unet, &xs, s2w, s2h, &enc, &format!("{p}.up_blocks.1.attentions.1"), 1280, 20, &mut cache, "up_1_1_0")?;
    xs = map_n_resnet(unet, &cat_skip(&xs, &pop(&mut skips)), s2w, s2h, &temb, &format!("{p}.up_blocks.1.resnets.2"), 1920)?;
    xs = stash_attn(unet, &xs, s2w, s2h, &enc, &format!("{p}.up_blocks.1.attentions.2"), 1280, 20, &mut cache, "up_1_2_0")?;
    xs = map_n_up(unet, &xs, s2w, s2h, &format!("{p}.up_blocks.1.upsamplers.0.conv"), 1280)?;

    xs = map_n_resnet(unet, &cat_skip(&xs, &pop(&mut skips)), s4w, s4h, &temb, &format!("{p}.up_blocks.2.resnets.0"), 1920)?;
    xs = stash_attn(unet, &xs, s4w, s4h, &enc, &format!("{p}.up_blocks.2.attentions.0"), 640, 10, &mut cache, "up_2_0_0")?;
    xs = map_n_resnet(unet, &cat_skip(&xs, &pop(&mut skips)), s4w, s4h, &temb, &format!("{p}.up_blocks.2.resnets.1"), 1280)?;
    xs = stash_attn(unet, &xs, s4w, s4h, &enc, &format!("{p}.up_blocks.2.attentions.1"), 640, 10, &mut cache, "up_2_1_0")?;
    xs = map_n_resnet(unet, &cat_skip(&xs, &pop(&mut skips)), s4w, s4h, &temb, &format!("{p}.up_blocks.2.resnets.2"), 960)?;
    xs = stash_attn(unet, &xs, s4w, s4h, &enc, &format!("{p}.up_blocks.2.attentions.2"), 640, 10, &mut cache, "up_2_2_0")?;
    xs = map_n_up(unet, &xs, s4w, s4h, &format!("{p}.up_blocks.2.upsamplers.0.conv"), 640)?;

    xs = map_n_resnet(unet, &cat_skip(&xs, &pop(&mut skips)), s8w, s8h, &temb, &format!("{p}.up_blocks.3.resnets.0"), 960)?;
    xs = stash_attn(unet, &xs, s8w, s8h, &enc, &format!("{p}.up_blocks.3.attentions.0"), 320, 5, &mut cache, "up_3_0_0")?;
    xs = map_n_resnet(unet, &cat_skip(&xs, &pop(&mut skips)), s8w, s8h, &temb, &format!("{p}.up_blocks.3.resnets.1"), 640)?;
    xs = stash_attn(unet, &xs, s8w, s8h, &enc, &format!("{p}.up_blocks.3.attentions.1"), 320, 5, &mut cache, "up_3_1_0")?;
    xs = map_n_resnet(unet, &cat_skip(&xs, &pop(&mut skips)), s8w, s8h, &temb, &format!("{p}.up_blocks.3.resnets.2"), 640)?;
    let _ = stash_attn(unet, &xs, s8w, s8h, &enc, &format!("{p}.up_blocks.3.attentions.2"), 320, 5, &mut cache, "up_3_2_0")?;
    Ok(cache)
}

/// Extras-on 12-ch UNet (MDA + RA + MA + DINO) using a dual-stream write cache.
///
/// `x12s` is planar 12-ch packs in `(pbr major, view minor)` order:
/// `[alb0, alb1, …, mr0, mr1, …]` (`n_pbr=2`, `n_views = x12s.len() / 2`).
/// `cache` must use the official 16 names from [`write_dual_cache`].
/// Returns 4-ch planar v-preds, one per pack (same order).
pub fn forward_extras_on(
    unet: &UnetFirst,
    x12s: &[&[f32]],
    temb: &[f32],
    enc_alb: &[f32],
    enc_mr: &[f32],
    dino: &[f32],
    cache: &HashMap<String, Vec<f32>>,
    voxels: &VoxelPyramid<'_>,
    lat_w: usize,
    lat_h: usize,
    ref_scale: f32,
) -> Result<Vec<Vec<f32>>, String> {
    forward_extras_on_cfg(
        unet,
        x12s,
        temb,
        &[enc_alb],
        &[enc_mr],
        &[dino],
        &[ref_scale],
        cache,
        voxels,
        lat_w,
        lat_h,
    )
}

fn forward_extras_on_cfg(
    unet: &UnetFirst,
    x12s: &[&[f32]],
    temb: &[f32],
    enc_albs: &[&[f32]],
    enc_mrs: &[&[f32]],
    dinos: &[&[f32]],
    ref_scales: &[f32],
    cache: &HashMap<String, Vec<f32>>,
    voxels: &VoxelPyramid<'_>,
    lat_w: usize,
    lat_h: usize,
) -> Result<Vec<Vec<f32>>, String> {
    let n_cfg = enc_albs.len();
    if n_cfg == 0
        || enc_mrs.len() != n_cfg
        || dinos.len() != n_cfg
        || ref_scales.len() != n_cfg
    {
        return Err(format!(
            "forward_extras_on_cfg branch lens alb={} mr={} dino={} ref={}",
            enc_albs.len(),
            enc_mrs.len(),
            dinos.len(),
            ref_scales.len()
        ));
    }
    if x12s.len() < 2 * n_cfg || x12s.len() % (2 * n_cfg) != 0 {
        return Err(format!(
            "forward_extras_on_cfg expects {}*2*n_views packs, got {}",
            n_cfg,
            x12s.len()
        ));
    }
    if lat_w == 0 || lat_h == 0 || lat_w % 8 != 0 || lat_h % 8 != 0 {
        return Err(format!("forward_extras_on latent {lat_w}x{lat_h} must be a positive multiple of 8"));
    }
    if paint_fast() {
        return forward_extras_on_cfg_batch(
            unet, x12s, temb, enc_albs, enc_mrs, dinos, ref_scales, cache, voxels, lat_w, lat_h,
        );
    }
    let s8w = lat_w;
    let s8h = lat_h;
    let s4w = s8w / 2;
    let s4h = s8h / 2;
    let s2w = s4w / 2;
    let s2h = s4h / 2;
    let s1w = s2w / 2;
    let s1h = s2h / 2;
    let n_views = x12s.len() / (2 * n_cfg);
    let plane8 = s8w * s8h;
    let conv_in = x12s
        .iter()
        .map(|x| gpu_upload(x, 12, plane8))
        .collect::<Result<Vec<_>, _>>()?;
    let mut xs = unet.conv_n(&conv_in, s8w, s8h, "unet.conv_in", 3, 1)?;
    let mut skips: Vec<Vec<GpuTensor>> = vec![clone_n(&xs)?];

    xs = map_n_resnet_gpu(unet, &xs, s8w, s8h, temb, "unet.down_blocks.0.resnets.0", 320)?;
    xs = extras_on_attn_cache(
        unet, &xs, cache_attn(cache, "down_0_0_0")?, s8w, s8h, 320, 5,
        "unet.down_blocks.0.attentions.0", enc_albs, enc_mrs, dinos,
        voxels.full.xyz, voxels.full.res, ref_scales, n_views,
    )?;
    skips.push(clone_n(&xs)?);
    xs = map_n_resnet_gpu(unet, &xs, s8w, s8h, temb, "unet.down_blocks.0.resnets.1", 320)?;
    xs = extras_on_attn_cache(
        unet, &xs, cache_attn(cache, "down_0_1_0")?, s8w, s8h, 320, 5,
        "unet.down_blocks.0.attentions.1", enc_albs, enc_mrs, dinos,
        voxels.full.xyz, voxels.full.res, ref_scales, n_views,
    )?;
    skips.push(clone_n(&xs)?);
    xs = map_n_down_gpu(unet, &xs, s8w, s8h, "unet.down_blocks.0.downsamplers.0.conv", 320)?;
    skips.push(clone_n(&xs)?);

    xs = map_n_resnet_gpu(unet, &xs, s4w, s4h, temb, "unet.down_blocks.1.resnets.0", 320)?;
    xs = extras_on_attn_cache(
        unet, &xs, cache_attn(cache, "down_1_0_0")?, s4w, s4h, 640, 10,
        "unet.down_blocks.1.attentions.0", enc_albs, enc_mrs, dinos,
        voxels.half.xyz, voxels.half.res, ref_scales, n_views,
    )?;
    skips.push(clone_n(&xs)?);
    xs = map_n_resnet_gpu(unet, &xs, s4w, s4h, temb, "unet.down_blocks.1.resnets.1", 640)?;
    xs = extras_on_attn_cache(
        unet, &xs, cache_attn(cache, "down_1_1_0")?, s4w, s4h, 640, 10,
        "unet.down_blocks.1.attentions.1", enc_albs, enc_mrs, dinos,
        voxels.half.xyz, voxels.half.res, ref_scales, n_views,
    )?;
    skips.push(clone_n(&xs)?);
    xs = map_n_down_gpu(unet, &xs, s4w, s4h, "unet.down_blocks.1.downsamplers.0.conv", 640)?;
    skips.push(clone_n(&xs)?);

    xs = map_n_resnet_gpu(unet, &xs, s2w, s2h, temb, "unet.down_blocks.2.resnets.0", 640)?;
    xs = extras_on_attn_cache(
        unet, &xs, cache_attn(cache, "down_2_0_0")?, s2w, s2h, 1280, 20,
        "unet.down_blocks.2.attentions.0", enc_albs, enc_mrs, dinos,
        voxels.quarter.xyz, voxels.quarter.res, ref_scales, n_views,
    )?;
    skips.push(clone_n(&xs)?);
    xs = map_n_resnet_gpu(unet, &xs, s2w, s2h, temb, "unet.down_blocks.2.resnets.1", 1280)?;
    xs = extras_on_attn_cache(
        unet, &xs, cache_attn(cache, "down_2_1_0")?, s2w, s2h, 1280, 20,
        "unet.down_blocks.2.attentions.1", enc_albs, enc_mrs, dinos,
        voxels.quarter.xyz, voxels.quarter.res, ref_scales, n_views,
    )?;
    skips.push(clone_n(&xs)?);
    xs = map_n_down_gpu(unet, &xs, s2w, s2h, "unet.down_blocks.2.downsamplers.0.conv", 1280)?;
    skips.push(clone_n(&xs)?);

    xs = map_n_resnet_gpu(unet, &xs, s1w, s1h, temb, "unet.down_blocks.3.resnets.0", 1280)?;
    skips.push(clone_n(&xs)?);
    xs = map_n_resnet_gpu(unet, &xs, s1w, s1h, temb, "unet.down_blocks.3.resnets.1", 1280)?;
    skips.push(clone_n(&xs)?);

    xs = map_n_resnet_gpu(unet, &xs, s1w, s1h, temb, "unet.mid_block.resnets.0", 1280)?;
    xs = extras_on_attn_cache(
        unet, &xs, cache_attn(cache, "mid_0_0")?, s1w, s1h, 1280, 20,
        "unet.mid_block.attentions.0", enc_albs, enc_mrs, dinos,
        voxels.eighth.xyz, voxels.eighth.res, ref_scales, n_views,
    )?;
    xs = map_n_resnet_gpu(unet, &xs, s1w, s1h, temb, "unet.mid_block.resnets.1", 1280)?;

    let pop = |skips: &mut Vec<Vec<GpuTensor>>| skips.pop().unwrap();
    xs = map_n_resnet_gpu(unet, &cat_skip_gpu(&xs, &pop(&mut skips))?, s1w, s1h, temb, "unet.up_blocks.0.resnets.0", 2560)?;
    xs = map_n_resnet_gpu(unet, &cat_skip_gpu(&xs, &pop(&mut skips))?, s1w, s1h, temb, "unet.up_blocks.0.resnets.1", 2560)?;
    xs = map_n_resnet_gpu(unet, &cat_skip_gpu(&xs, &pop(&mut skips))?, s1w, s1h, temb, "unet.up_blocks.0.resnets.2", 2560)?;
    xs = map_n_up_gpu(unet, &xs, s1w, s1h, "unet.up_blocks.0.upsamplers.0.conv", 1280)?;

    xs = map_n_resnet_gpu(unet, &cat_skip_gpu(&xs, &pop(&mut skips))?, s2w, s2h, temb, "unet.up_blocks.1.resnets.0", 2560)?;
    xs = extras_on_attn_cache(
        unet, &xs, cache_attn(cache, "up_1_0_0")?, s2w, s2h, 1280, 20,
        "unet.up_blocks.1.attentions.0", enc_albs, enc_mrs, dinos,
        voxels.quarter.xyz, voxels.quarter.res, ref_scales, n_views,
    )?;
    xs = map_n_resnet_gpu(unet, &cat_skip_gpu(&xs, &pop(&mut skips))?, s2w, s2h, temb, "unet.up_blocks.1.resnets.1", 2560)?;
    xs = extras_on_attn_cache(
        unet, &xs, cache_attn(cache, "up_1_1_0")?, s2w, s2h, 1280, 20,
        "unet.up_blocks.1.attentions.1", enc_albs, enc_mrs, dinos,
        voxels.quarter.xyz, voxels.quarter.res, ref_scales, n_views,
    )?;
    xs = map_n_resnet_gpu(unet, &cat_skip_gpu(&xs, &pop(&mut skips))?, s2w, s2h, temb, "unet.up_blocks.1.resnets.2", 1920)?;
    xs = extras_on_attn_cache(
        unet, &xs, cache_attn(cache, "up_1_2_0")?, s2w, s2h, 1280, 20,
        "unet.up_blocks.1.attentions.2", enc_albs, enc_mrs, dinos,
        voxels.quarter.xyz, voxels.quarter.res, ref_scales, n_views,
    )?;
    xs = map_n_up_gpu(unet, &xs, s2w, s2h, "unet.up_blocks.1.upsamplers.0.conv", 1280)?;

    xs = map_n_resnet_gpu(unet, &cat_skip_gpu(&xs, &pop(&mut skips))?, s4w, s4h, temb, "unet.up_blocks.2.resnets.0", 1920)?;
    xs = extras_on_attn_cache(
        unet, &xs, cache_attn(cache, "up_2_0_0")?, s4w, s4h, 640, 10,
        "unet.up_blocks.2.attentions.0", enc_albs, enc_mrs, dinos,
        voxels.half.xyz, voxels.half.res, ref_scales, n_views,
    )?;
    xs = map_n_resnet_gpu(unet, &cat_skip_gpu(&xs, &pop(&mut skips))?, s4w, s4h, temb, "unet.up_blocks.2.resnets.1", 1280)?;
    xs = extras_on_attn_cache(
        unet, &xs, cache_attn(cache, "up_2_1_0")?, s4w, s4h, 640, 10,
        "unet.up_blocks.2.attentions.1", enc_albs, enc_mrs, dinos,
        voxels.half.xyz, voxels.half.res, ref_scales, n_views,
    )?;
    xs = map_n_resnet_gpu(unet, &cat_skip_gpu(&xs, &pop(&mut skips))?, s4w, s4h, temb, "unet.up_blocks.2.resnets.2", 960)?;
    xs = extras_on_attn_cache(
        unet, &xs, cache_attn(cache, "up_2_2_0")?, s4w, s4h, 640, 10,
        "unet.up_blocks.2.attentions.2", enc_albs, enc_mrs, dinos,
        voxels.half.xyz, voxels.half.res, ref_scales, n_views,
    )?;
    xs = map_n_up_gpu(unet, &xs, s4w, s4h, "unet.up_blocks.2.upsamplers.0.conv", 640)?;

    xs = map_n_resnet_gpu(unet, &cat_skip_gpu(&xs, &pop(&mut skips))?, s8w, s8h, temb, "unet.up_blocks.3.resnets.0", 960)?;
    xs = extras_on_attn_cache(
        unet, &xs, cache_attn(cache, "up_3_0_0")?, s8w, s8h, 320, 5,
        "unet.up_blocks.3.attentions.0", enc_albs, enc_mrs, dinos,
        voxels.full.xyz, voxels.full.res, ref_scales, n_views,
    )?;
    xs = map_n_resnet_gpu(unet, &cat_skip_gpu(&xs, &pop(&mut skips))?, s8w, s8h, temb, "unet.up_blocks.3.resnets.1", 640)?;
    xs = extras_on_attn_cache(
        unet, &xs, cache_attn(cache, "up_3_1_0")?, s8w, s8h, 320, 5,
        "unet.up_blocks.3.attentions.1", enc_albs, enc_mrs, dinos,
        voxels.full.xyz, voxels.full.res, ref_scales, n_views,
    )?;
    xs = map_n_resnet_gpu(unet, &cat_skip_gpu(&xs, &pop(&mut skips))?, s8w, s8h, temb, "unet.up_blocks.3.resnets.2", 640)?;
    xs = extras_on_attn_cache(
        unet, &xs, cache_attn(cache, "up_3_2_0")?, s8w, s8h, 320, 5,
        "unet.up_blocks.3.attentions.2", enc_albs, enc_mrs, dinos,
        voxels.full.xyz, voxels.full.res, ref_scales, n_views,
    )?;
    let mut n1 = Vec::with_capacity(xs.len());
    for h in &xs {
        n1.push(unet.conv_head_gpu(h, s8w, s8h)?);
    }
    n1.iter().map(gpu_download).collect()
}

/// Dual / text / DINO / voxels uploaded once per job.
pub struct ExtrasJobCtx {
    enc0: GpuTensor,
    enc_alb: GpuTensor,
    enc_mr: GpuTensor,
    dino0: GpuTensor,
    dino: GpuTensor,
    c_down00: GpuTensor,
    c_down01: GpuTensor,
    c_down10: GpuTensor,
    c_down11: GpuTensor,
    c_down20: GpuTensor,
    c_down21: GpuTensor,
    c_mid: GpuTensor,
    c_up10: GpuTensor,
    c_up11: GpuTensor,
    c_up12: GpuTensor,
    c_up20: GpuTensor,
    c_up21: GpuTensor,
    c_up22: GpuTensor,
    c_up30: GpuTensor,
    c_up31: GpuTensor,
    c_up32: GpuTensor,
    vox_full: GpuTensor,
    vox_half: GpuTensor,
    vox_quarter: GpuTensor,
    vox_eighth: GpuTensor,
    vox_full_res: usize,
    vox_half_res: usize,
    vox_quarter_res: usize,
    vox_eighth_res: usize,
}

impl ExtrasJobCtx {
    pub fn upload(
        enc_alb: &[f32],
        enc_mr: &[f32],
        dino: &[f32],
        cache: &HashMap<String, Vec<f32>>,
        voxels: &VoxelPyramid<'_>,
        n_cfg: usize,
    ) -> Result<Self, String> {
        let enc0 = vec![0.0f32; enc_alb.len()];
        let dino0 = vec![0.0f32; dino.len()];
        let vox_rep = n_cfg * 2;
        Ok(Self {
            enc0: upload_tok(&enc0, 1024)?,
            enc_alb: upload_tok(enc_alb, 1024)?,
            enc_mr: upload_tok(enc_mr, 1024)?,
            dino0: upload_tok(&dino0, 1024)?,
            dino: upload_tok(dino, 1024)?,
            c_down00: upload_ref(cache_attn(cache, "down_0_0_0")?, 320)?,
            c_down01: upload_ref(cache_attn(cache, "down_0_1_0")?, 320)?,
            c_down10: upload_ref(cache_attn(cache, "down_1_0_0")?, 640)?,
            c_down11: upload_ref(cache_attn(cache, "down_1_1_0")?, 640)?,
            c_down20: upload_ref(cache_attn(cache, "down_2_0_0")?, 1280)?,
            c_down21: upload_ref(cache_attn(cache, "down_2_1_0")?, 1280)?,
            c_mid: upload_ref(cache_attn(cache, "mid_0_0")?, 1280)?,
            c_up10: upload_ref(cache_attn(cache, "up_1_0_0")?, 1280)?,
            c_up11: upload_ref(cache_attn(cache, "up_1_1_0")?, 1280)?,
            c_up12: upload_ref(cache_attn(cache, "up_1_2_0")?, 1280)?,
            c_up20: upload_ref(cache_attn(cache, "up_2_0_0")?, 640)?,
            c_up21: upload_ref(cache_attn(cache, "up_2_1_0")?, 640)?,
            c_up22: upload_ref(cache_attn(cache, "up_2_2_0")?, 640)?,
            c_up30: upload_ref(cache_attn(cache, "up_3_0_0")?, 320)?,
            c_up31: upload_ref(cache_attn(cache, "up_3_1_0")?, 320)?,
            c_up32: upload_ref(cache_attn(cache, "up_3_2_0")?, 320)?,
            vox_full: upload_voxel_rep(voxels.full.xyz, vox_rep)?,
            vox_half: upload_voxel_rep(voxels.half.xyz, vox_rep)?,
            vox_quarter: upload_voxel_rep(voxels.quarter.xyz, vox_rep)?,
            vox_eighth: upload_voxel_rep(voxels.eighth.xyz, vox_rep)?,
            vox_full_res: voxels.full.res,
            vox_half_res: voxels.half.res,
            vox_quarter_res: voxels.quarter.res,
            vox_eighth_res: voxels.eighth.res,
        })
    }
}

/// One extras-on walk over the official 3-branch CFG batch
/// (`3 * n_pbr * n_views` packs) instead of three separate walks.
pub fn predict_v_cfg_three_branch(
    unet: &UnetFirst,
    x12s: &[&[f32]],
    temb: &[f32],
    enc_alb: &[f32],
    enc_mr: &[f32],
    dino: &[f32],
    cache: &HashMap<String, Vec<f32>>,
    voxels: &VoxelPyramid<'_>,
    lat_w: usize,
    lat_h: usize,
) -> Result<CfgThreeBranch, String> {
    let n = x12s.len();
    if n < 2 || n % 2 != 0 {
        return Err(format!(
            "predict_v_cfg_three_branch expects even packs, got {n}"
        ));
    }
    let enc0 = vec![0.0f32; enc_alb.len()];
    let dino0 = vec![0.0f32; dino.len()];
    let mut packed = Vec::with_capacity(3 * n);
    for _ in 0..3 {
        packed.extend_from_slice(x12s);
    }
    let enc_albs = [enc0.as_slice(), enc_alb, enc_alb];
    let enc_mrs = [enc0.as_slice(), enc_mr, enc_mr];
    let dinos = [dino0.as_slice(), dino0.as_slice(), dino];
    let ref_scales = [0.0f32, 1.0, 1.0];
    let out = forward_extras_on_cfg(
        unet,
        &packed,
        temb,
        &enc_albs,
        &enc_mrs,
        &dinos,
        &ref_scales,
        cache,
        voxels,
        lat_w,
        lat_h,
    )?;
    if out.len() != 3 * n {
        return Err(format!("cfg walk returned {} packs, want {}", out.len(), 3 * n));
    }
    Ok(CfgThreeBranch {
        uncond: out[..n].to_vec(),
        ref_only: out[n..2 * n].to_vec(),
        full: out[2 * n..].to_vec(),
    })
}

fn forward_extras_on_cfg_batch(
    unet: &UnetFirst,
    x12s: &[&[f32]],
    temb: &[f32],
    enc_albs: &[&[f32]],
    enc_mrs: &[&[f32]],
    dinos: &[&[f32]],
    ref_scales: &[f32],
    cache: &HashMap<String, Vec<f32>>,
    voxels: &VoxelPyramid<'_>,
    lat_w: usize,
    lat_h: usize,
) -> Result<Vec<Vec<f32>>, String> {
    let n_cfg = enc_albs.len();
    let n_views = x12s.len() / (2 * n_cfg);
    let ctx = ExtrasJobCtx::upload(
        enc_albs[if n_cfg >= 2 { 1 } else { 0 }],
        enc_mrs[if n_cfg >= 2 { 1 } else { 0 }],
        dinos[n_cfg - 1],
        cache,
        voxels,
        n_cfg,
    )?;
    let xs = unet.conv_in_batch(x12s, lat_w, lat_h)?;
    let head = walk_extras_on_resident(unet, xs, &unet.silu_temb(temb)?, &ctx, n_views, ref_scales)?;
    unpack_planar_host(&gpu_download(&head.t)?, 4, head.n, head.plane())
}

pub(crate) fn walk_extras_on_resident(
    unet: &UnetFirst,
    mut xs: BatchAct,
    temb_act: &GpuTensor,
    ctx: &ExtrasJobCtx,
    n_views: usize,
    ref_scales: &[f32],
) -> Result<BatchAct, String> {
    let enc0 = &ctx.enc0;
    let enc_alb = &ctx.enc_alb;
    let enc_mr = &ctx.enc_mr;
    let dino0 = &ctx.dino0;
    let dino = &ctx.dino;
    let c_down00 = &ctx.c_down00;
    let c_down01 = &ctx.c_down01;
    let c_down10 = &ctx.c_down10;
    let c_down11 = &ctx.c_down11;
    let c_down20 = &ctx.c_down20;
    let c_down21 = &ctx.c_down21;
    let c_mid = &ctx.c_mid;
    let c_up10 = &ctx.c_up10;
    let c_up11 = &ctx.c_up11;
    let c_up12 = &ctx.c_up12;
    let c_up20 = &ctx.c_up20;
    let c_up21 = &ctx.c_up21;
    let c_up22 = &ctx.c_up22;
    let c_up30 = &ctx.c_up30;
    let c_up31 = &ctx.c_up31;
    let c_up32 = &ctx.c_up32;
    let vox_full = &ctx.vox_full;
    let vox_half = &ctx.vox_half;
    let vox_quarter = &ctx.vox_quarter;
    let vox_eighth = &ctx.vox_eighth;

    let mut skips: Vec<BatchAct> = vec![xs.clone_act()?];

    xs = unet.resnet_batch_act(&xs, &temb_act, "unet.down_blocks.0.resnets.0", 320)?;
    xs = unet.transformer2d_extras_batch(
        &xs,
        "unet.down_blocks.0.attentions.0",
        320,
        5,
        n_views,
        &extras_res(&enc0, &enc_alb, &enc_mr, &dino0, &dino, &c_down00, &vox_full, ctx.vox_full_res),
        ref_scales,
        1.0,
    )?;
    skips.push(xs.clone_act()?);
    xs = unet.resnet_batch_act(&xs, &temb_act, "unet.down_blocks.0.resnets.1", 320)?;
    xs = unet.transformer2d_extras_batch(
        &xs,
        "unet.down_blocks.0.attentions.1",
        320,
        5,
        n_views,
        &extras_res(&enc0, &enc_alb, &enc_mr, &dino0, &dino, &c_down01, &vox_full, ctx.vox_full_res),
        ref_scales,
        1.0,
    )?;
    skips.push(xs.clone_act()?);
    xs = unet.downsample_batch(&xs, "unet.down_blocks.0.downsamplers.0.conv", 320)?;
    skips.push(xs.clone_act()?);

    xs = unet.resnet_batch_act(&xs, &temb_act, "unet.down_blocks.1.resnets.0", 320)?;
    xs = unet.transformer2d_extras_batch(
        &xs,
        "unet.down_blocks.1.attentions.0",
        640,
        10,
        n_views,
        &extras_res(&enc0, &enc_alb, &enc_mr, &dino0, &dino, &c_down10, &vox_half, ctx.vox_half_res),
        ref_scales,
        1.0,
    )?;
    skips.push(xs.clone_act()?);
    xs = unet.resnet_batch_act(&xs, &temb_act, "unet.down_blocks.1.resnets.1", 640)?;
    xs = unet.transformer2d_extras_batch(
        &xs,
        "unet.down_blocks.1.attentions.1",
        640,
        10,
        n_views,
        &extras_res(&enc0, &enc_alb, &enc_mr, &dino0, &dino, &c_down11, &vox_half, ctx.vox_half_res),
        ref_scales,
        1.0,
    )?;
    skips.push(xs.clone_act()?);
    xs = unet.downsample_batch(&xs, "unet.down_blocks.1.downsamplers.0.conv", 640)?;
    skips.push(xs.clone_act()?);

    xs = unet.resnet_batch_act(&xs, &temb_act, "unet.down_blocks.2.resnets.0", 640)?;
    xs = unet.transformer2d_extras_batch(
        &xs,
        "unet.down_blocks.2.attentions.0",
        1280,
        20,
        n_views,
        &extras_res(&enc0, &enc_alb, &enc_mr, &dino0, &dino, &c_down20, &vox_quarter, ctx.vox_quarter_res),
        ref_scales,
        1.0,
    )?;
    skips.push(xs.clone_act()?);
    xs = unet.resnet_batch_act(&xs, &temb_act, "unet.down_blocks.2.resnets.1", 1280)?;
    xs = unet.transformer2d_extras_batch(
        &xs,
        "unet.down_blocks.2.attentions.1",
        1280,
        20,
        n_views,
        &extras_res(&enc0, &enc_alb, &enc_mr, &dino0, &dino, &c_down21, &vox_quarter, ctx.vox_quarter_res),
        ref_scales,
        1.0,
    )?;
    skips.push(xs.clone_act()?);
    xs = unet.downsample_batch(&xs, "unet.down_blocks.2.downsamplers.0.conv", 1280)?;
    skips.push(xs.clone_act()?);

    xs = unet.resnet_batch_act(&xs, &temb_act, "unet.down_blocks.3.resnets.0", 1280)?;
    skips.push(xs.clone_act()?);
    xs = unet.resnet_batch_act(&xs, &temb_act, "unet.down_blocks.3.resnets.1", 1280)?;
    skips.push(xs.clone_act()?);

    xs = unet.resnet_batch_act(&xs, &temb_act, "unet.mid_block.resnets.0", 1280)?;
    xs = unet.transformer2d_extras_batch(
        &xs,
        "unet.mid_block.attentions.0",
        1280,
        20,
        n_views,
        &extras_res(&enc0, &enc_alb, &enc_mr, &dino0, &dino, &c_mid, &vox_eighth, ctx.vox_eighth_res),
        ref_scales,
        1.0,
    )?;
    xs = unet.resnet_batch_act(&xs, &temb_act, "unet.mid_block.resnets.1", 1280)?;

    let pop = |skips: &mut Vec<BatchAct>| skips.pop().unwrap();
    xs = unet.resnet_batch_act(
        &UnetFirst::cat_skip_batch(&xs, &pop(&mut skips))?,
        &temb_act,
        "unet.up_blocks.0.resnets.0",
        2560,
    )?;
    xs = unet.resnet_batch_act(
        &UnetFirst::cat_skip_batch(&xs, &pop(&mut skips))?,
        &temb_act,
        "unet.up_blocks.0.resnets.1",
        2560,
    )?;
    xs = unet.resnet_batch_act(
        &UnetFirst::cat_skip_batch(&xs, &pop(&mut skips))?,
        &temb_act,
        "unet.up_blocks.0.resnets.2",
        2560,
    )?;
    xs = unet.upsample_batch(&xs, "unet.up_blocks.0.upsamplers.0.conv", 1280)?;

    xs = unet.resnet_batch_act(
        &UnetFirst::cat_skip_batch(&xs, &pop(&mut skips))?,
        &temb_act,
        "unet.up_blocks.1.resnets.0",
        2560,
    )?;
    xs = unet.transformer2d_extras_batch(
        &xs,
        "unet.up_blocks.1.attentions.0",
        1280,
        20,
        n_views,
        &extras_res(&enc0, &enc_alb, &enc_mr, &dino0, &dino, &c_up10, &vox_quarter, ctx.vox_quarter_res),
        ref_scales,
        1.0,
    )?;
    xs = unet.resnet_batch_act(
        &UnetFirst::cat_skip_batch(&xs, &pop(&mut skips))?,
        &temb_act,
        "unet.up_blocks.1.resnets.1",
        2560,
    )?;
    xs = unet.transformer2d_extras_batch(
        &xs,
        "unet.up_blocks.1.attentions.1",
        1280,
        20,
        n_views,
        &extras_res(&enc0, &enc_alb, &enc_mr, &dino0, &dino, &c_up11, &vox_quarter, ctx.vox_quarter_res),
        ref_scales,
        1.0,
    )?;
    xs = unet.resnet_batch_act(
        &UnetFirst::cat_skip_batch(&xs, &pop(&mut skips))?,
        &temb_act,
        "unet.up_blocks.1.resnets.2",
        1920,
    )?;
    xs = unet.transformer2d_extras_batch(
        &xs,
        "unet.up_blocks.1.attentions.2",
        1280,
        20,
        n_views,
        &extras_res(&enc0, &enc_alb, &enc_mr, &dino0, &dino, &c_up12, &vox_quarter, ctx.vox_quarter_res),
        ref_scales,
        1.0,
    )?;
    xs = unet.upsample_batch(&xs, "unet.up_blocks.1.upsamplers.0.conv", 1280)?;

    xs = unet.resnet_batch_act(
        &UnetFirst::cat_skip_batch(&xs, &pop(&mut skips))?,
        &temb_act,
        "unet.up_blocks.2.resnets.0",
        1920,
    )?;
    xs = unet.transformer2d_extras_batch(
        &xs,
        "unet.up_blocks.2.attentions.0",
        640,
        10,
        n_views,
        &extras_res(&enc0, &enc_alb, &enc_mr, &dino0, &dino, &c_up20, &vox_half, ctx.vox_half_res),
        ref_scales,
        1.0,
    )?;
    xs = unet.resnet_batch_act(
        &UnetFirst::cat_skip_batch(&xs, &pop(&mut skips))?,
        &temb_act,
        "unet.up_blocks.2.resnets.1",
        1280,
    )?;
    xs = unet.transformer2d_extras_batch(
        &xs,
        "unet.up_blocks.2.attentions.1",
        640,
        10,
        n_views,
        &extras_res(&enc0, &enc_alb, &enc_mr, &dino0, &dino, &c_up21, &vox_half, ctx.vox_half_res),
        ref_scales,
        1.0,
    )?;
    xs = unet.resnet_batch_act(
        &UnetFirst::cat_skip_batch(&xs, &pop(&mut skips))?,
        &temb_act,
        "unet.up_blocks.2.resnets.2",
        960,
    )?;
    xs = unet.transformer2d_extras_batch(
        &xs,
        "unet.up_blocks.2.attentions.2",
        640,
        10,
        n_views,
        &extras_res(&enc0, &enc_alb, &enc_mr, &dino0, &dino, &c_up22, &vox_half, ctx.vox_half_res),
        ref_scales,
        1.0,
    )?;
    xs = unet.upsample_batch(&xs, "unet.up_blocks.2.upsamplers.0.conv", 640)?;

    xs = unet.resnet_batch_act(
        &UnetFirst::cat_skip_batch(&xs, &pop(&mut skips))?,
        &temb_act,
        "unet.up_blocks.3.resnets.0",
        960,
    )?;
    xs = unet.transformer2d_extras_batch(
        &xs,
        "unet.up_blocks.3.attentions.0",
        320,
        5,
        n_views,
        &extras_res(&enc0, &enc_alb, &enc_mr, &dino0, &dino, &c_up30, &vox_full, ctx.vox_full_res),
        ref_scales,
        1.0,
    )?;
    xs = unet.resnet_batch_act(
        &UnetFirst::cat_skip_batch(&xs, &pop(&mut skips))?,
        &temb_act,
        "unet.up_blocks.3.resnets.1",
        640,
    )?;
    xs = unet.transformer2d_extras_batch(
        &xs,
        "unet.up_blocks.3.attentions.1",
        320,
        5,
        n_views,
        &extras_res(&enc0, &enc_alb, &enc_mr, &dino0, &dino, &c_up31, &vox_full, ctx.vox_full_res),
        ref_scales,
        1.0,
    )?;
    xs = unet.resnet_batch_act(
        &UnetFirst::cat_skip_batch(&xs, &pop(&mut skips))?,
        &temb_act,
        "unet.up_blocks.3.resnets.2",
        640,
    )?;
    xs = unet.transformer2d_extras_batch(
        &xs,
        "unet.up_blocks.3.attentions.2",
        320,
        5,
        n_views,
        &extras_res(&enc0, &enc_alb, &enc_mr, &dino0, &dino, &c_up32, &vox_full, ctx.vox_full_res),
        ref_scales,
        1.0,
    )?;
    unet.conv_head_batch(&xs)
}

/// `[noise | normal | position]` then 3-way CFG cat on device.
pub fn pack_cfg_x12_gpu(
    sample: &GpuTensor,
    normals_pbr: &GpuTensor,
    positions_pbr: &GpuTensor,
) -> Result<GpuTensor, String> {
    let one = gpu_concat_rows(&gpu_concat_rows(sample, normals_pbr)?, positions_pbr)?;
    gpu_concat_cols(&[&one, &one, &one])
}

/// `uncond + scale*(ref-uncond) + scale*(full-ref)` then `c1*x + c2*v`.
pub fn cfg_ddim_gpu(
    sample: &GpuTensor,
    head: &GpuTensor,
    scale_t: &GpuTensor,
    c1: f32,
    c2: f32,
) -> Result<GpuTensor, String> {
    let branch = sample.cols();
    if head.rows() != sample.rows() || head.cols() != 3 * branch || scale_t.cols() != branch {
        return Err(format!(
            "cfg_ddim shape sample {}x{} head {}x{} scale {}x{}",
            sample.rows(),
            sample.cols(),
            head.rows(),
            head.cols(),
            scale_t.rows(),
            scale_t.cols()
        ));
    }
    let uncond = gpu_slice_cols(head, 0, branch)?;
    let ref_only = gpu_slice_cols(head, branch, branch)?;
    let full = gpu_slice_cols(head, 2 * branch, branch)?;
    let ru = gpu_add(&ref_only, &gpu_paint_scale(&uncond, -1.0)?)?;
    let fr = gpu_add(&full, &gpu_paint_scale(&ref_only, -1.0)?)?;
    let guided = gpu_add(
        &uncond,
        &gpu_add(&gpu_mul(scale_t, &ru)?, &gpu_mul(scale_t, &fr)?)?,
    )?;
    gpu_add(
        &gpu_paint_scale(sample, c1)?,
        &gpu_paint_scale(&guided, c2)?,
    )
}

pub fn cfg_ddim_gpu_t(
    sample: &GpuTensor,
    head: &GpuTensor,
    scale_t: &GpuTensor,
    c1_t: &GpuTensor,
    c2_t: &GpuTensor,
) -> Result<GpuTensor, String> {
    let branch = sample.cols();
    if head.cols() != 3 * branch || scale_t.cols() != branch || c1_t.cols() != branch {
        return Err("cfg_ddim_gpu_t shape".into());
    }
    let uncond = gpu_slice_cols(head, 0, branch)?;
    let ref_only = gpu_slice_cols(head, branch, branch)?;
    let full = gpu_slice_cols(head, 2 * branch, branch)?;
    let ru = gpu_add(&ref_only, &gpu_paint_scale(&uncond, -1.0)?)?;
    let fr = gpu_add(&full, &gpu_paint_scale(&ref_only, -1.0)?)?;
    let guided = gpu_add(
        &uncond,
        &gpu_add(&gpu_mul(scale_t, &ru)?, &gpu_mul(scale_t, &fr)?)?,
    )?;
    gpu_add(&gpu_mul(sample, c1_t)?, &gpu_mul(&guided, c2_t)?)
}

fn extras_res<'a>(
    enc0: &'a GpuTensor,
    enc_alb: &'a GpuTensor,
    enc_mr: &'a GpuTensor,
    dino0: &'a GpuTensor,
    dino: &'a GpuTensor,
    ref_tokens: &'a GpuTensor,
    voxel: &'a GpuTensor,
    voxel_res: usize,
) -> ExtraResident<'a> {
    ExtraResident {
        enc0,
        enc_alb,
        enc_mr,
        dino0,
        dino,
        ref_tokens,
        voxel,
        voxel_res,
    }
}

fn upload_tok(x: &[f32], dim: usize) -> Result<GpuTensor, String> {
    if dim == 0 || x.len() % dim != 0 {
        return Err(format!("upload_tok len {} vs dim {dim}", x.len()));
    }
    gpu_upload(x, x.len() / dim, dim)
}

fn upload_ref(cache: &[f32], hidden: usize) -> Result<GpuTensor, String> {
    if hidden == 0 || cache.len() % hidden != 0 {
        return Err(format!("upload_ref len {} vs hidden {hidden}", cache.len()));
    }
    gpu_upload(cache, cache.len() / hidden, hidden)
}

fn upload_voxel_rep(xyz: &[[u32; 3]], times: usize) -> Result<GpuTensor, String> {
    if times == 0 {
        return Err("upload_voxel_rep times=0".into());
    }
    let mut host = Vec::with_capacity(xyz.len() * 3 * times);
    for _ in 0..times {
        for p in xyz {
            host.extend_from_slice(&[p[0], p[1], p[2]]);
        }
    }
    gpu_upload_u32(&host)
}

fn flatten_vpreds(xs: &[Vec<f32>]) -> Vec<f32> {
    let mut out = Vec::new();
    for x in xs {
        out.extend_from_slice(x);
    }
    out
}

fn cache_attn<'a>(cache: &'a HashMap<String, Vec<f32>>, name: &str) -> Result<&'a [f32], String> {
    cache
        .get(name)
        .map(Vec::as_slice)
        .ok_or_else(|| format!("missing dual cache {name}"))
}

fn extras_on_attn_cache(
    unet: &UnetFirst,
    xs: &[GpuTensor],
    cache: &[f32],
    width: usize,
    height: usize,
    channels: usize,
    heads: usize,
    prefix: &str,
    enc_albs: &[&[f32]],
    enc_mrs: &[&[f32]],
    dinos: &[&[f32]],
    voxels: &[[u32; 3]],
    voxel_res: usize,
    ref_scales: &[f32],
    n_views: usize,
) -> Result<Vec<GpuTensor>, String> {
    let n_cfg = enc_albs.len();
    let want = n_cfg * 2 * n_views;
    if xs.len() != want {
        return Err(format!(
            "{prefix} extras-on expects {want} samples ({n_cfg} cfg × 2 pbr × {n_views} views), got {}",
            xs.len()
        ));
    }
    let empty: [&[f32]; 0] = [];
    let mut encoders = Vec::with_capacity(xs.len());
    for cfg in 0..n_cfg {
        for _ in 0..n_views {
            encoders.push(enc_albs[cfg]);
        }
        for _ in 0..n_views {
            encoders.push(enc_mrs[cfg]);
        }
    }
    unet.transformer2d_extras_gpu(ExtraInputs {
        samples: &empty,
        samples_gpu: Some(xs),
        encoders: &encoders,
        width,
        height,
        channels,
        heads,
        prefix,
        flags: ExtraFlags {
            mda: true,
            dino: true,
            ra: true,
            ma: true,
        },
        dino: dinos.first().copied(),
        ref_samples: None,
        ref_tokens: Some(cache),
        ref_scale: ref_scales.first().copied().unwrap_or(1.0),
        n_views,
        voxel_xyz: Some(voxels),
        voxel_res,
        mva_scale: 1.0,
        dinos: Some(dinos),
        ref_scales: Some(ref_scales),
    })
}

fn stash_attn(
    unet: &UnetFirst,
    xs: &[Vec<f32>],
    w: usize,
    h: usize,
    enc: &[f32],
    prefix: &str,
    ch: usize,
    heads: usize,
    cache: &mut HashMap<String, Vec<f32>>,
    layer: &str,
) -> Result<Vec<Vec<f32>>, String> {
    let mut outs = Vec::with_capacity(xs.len());
    let mut norms = Vec::new();
    for x in xs {
        let (out, n1) = unet.transformer2d_norm1(x, w, h, enc, 77, prefix, ch, heads)?;
        outs.push(out);
        norms.extend_from_slice(&n1);
    }
    cache.insert(layer.to_string(), norms);
    Ok(outs)
}

fn map_n_resnet(
    unet: &UnetFirst,
    xs: &[Vec<f32>],
    w: usize,
    h: usize,
    temb: &[f32],
    prefix: &str,
    cin: usize,
) -> Result<Vec<Vec<f32>>, String> {
    xs.iter()
        .map(|x| unet.resnet(x, w, h, temb, prefix, cin))
        .collect()
}

fn map_n_resnet_gpu(
    unet: &UnetFirst,
    xs: &[GpuTensor],
    w: usize,
    h: usize,
    temb: &[f32],
    prefix: &str,
    cin: usize,
) -> Result<Vec<GpuTensor>, String> {
    unet.resnet_n(xs, w, h, temb, prefix, cin)
}

fn map_n_down(
    unet: &UnetFirst,
    xs: &[Vec<f32>],
    w: usize,
    h: usize,
    prefix: &str,
    ch: usize,
) -> Result<Vec<Vec<f32>>, String> {
    let mut out = Vec::with_capacity(xs.len());
    for x in xs {
        out.push(unet.downsample(x, w, h, prefix, ch)?.0);
    }
    Ok(out)
}

fn map_n_down_gpu(
    unet: &UnetFirst,
    xs: &[GpuTensor],
    w: usize,
    h: usize,
    prefix: &str,
    ch: usize,
) -> Result<Vec<GpuTensor>, String> {
    let mut out = Vec::with_capacity(xs.len());
    for x in xs {
        out.push(unet.downsample_gpu(x, w, h, prefix, ch)?.0);
    }
    Ok(out)
}

fn map_n_up(
    unet: &UnetFirst,
    xs: &[Vec<f32>],
    w: usize,
    h: usize,
    prefix: &str,
    ch: usize,
) -> Result<Vec<Vec<f32>>, String> {
    let mut out = Vec::with_capacity(xs.len());
    for x in xs {
        out.push(unet.upsample(x, w, h, prefix, ch)?.0);
    }
    Ok(out)
}

fn map_n_up_gpu(
    unet: &UnetFirst,
    xs: &[GpuTensor],
    w: usize,
    h: usize,
    prefix: &str,
    ch: usize,
) -> Result<Vec<GpuTensor>, String> {
    let mut out = Vec::with_capacity(xs.len());
    for x in xs {
        out.push(unet.upsample_gpu(x, w, h, prefix, ch)?.0);
    }
    Ok(out)
}

fn cat_skip(hidden: &[Vec<f32>], skip: &[Vec<f32>]) -> Vec<Vec<f32>> {
    hidden
        .iter()
        .zip(skip)
        .map(|(h, s)| UnetFirst::concat_planar(h, s))
        .collect()
}

fn cat_skip_gpu(hidden: &[GpuTensor], skip: &[GpuTensor]) -> Result<Vec<GpuTensor>, String> {
    hidden
        .iter()
        .zip(skip)
        .map(|(h, s)| gpu_concat_rows(h, s))
        .collect()
}

fn clone_n(xs: &[GpuTensor]) -> Result<Vec<GpuTensor>, String> {
    xs.iter().map(|x| gpu_slice_rows(x, 0, x.rows())).collect()
}
