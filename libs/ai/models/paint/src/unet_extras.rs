//! Basic2p5DTransformerBlock extras: MDA `_mr`, reference attention,
//! multiview + PoseRoPE, and DINO cross-attention.

use crate::unet_attn::{
    as_f32, attn_cross, attn_cross_batched, attn_packed, attn_packed_batched, match_half,
    paint_scale_like, ref_attn_wide_like,
};
use crate::unet_first::{paint_fast, BatchAct, UnetFirst, ATTN_GN_EPS, GN_GROUPS, NS};
use makepad_ai_common::backend::cuda::{
    gpu_add, gpu_concat_cols, gpu_concat_rows_many, gpu_download,
    gpu_group_norm_nchw, gpu_group_norm_planar, gpu_nchw_to_tokens, gpu_paint_group_norm_batched,
    gpu_paint_pose_rope, gpu_paint_pose_rope_dev,
    gpu_planar_tokens_transpose, gpu_slice_cols, gpu_slice_rows, gpu_tokens_to_nchw, gpu_upload,
    GpuTensor,
};

/// Pre-uploaded Dual / text / DINO / voxel tensors for one extras-on layer.
pub struct ExtraResident<'a> {
    pub enc0: &'a GpuTensor,
    pub enc_alb: &'a GpuTensor,
    pub enc_mr: &'a GpuTensor,
    pub dino0: &'a GpuTensor,
    pub dino: &'a GpuTensor,
    pub ref_tokens: &'a GpuTensor,
    pub voxel: &'a GpuTensor,
    pub voxel_res: usize,
}

#[derive(Clone, Copy, Default)]
pub struct ExtraFlags {
    pub mda: bool,
    pub dino: bool,
    pub ra: bool,
    pub ma: bool,
}

pub struct ExtraInputs<'a> {
    pub samples: &'a [&'a [f32]],
    /// If set, planar `[C, HW]` activations already on device (skips host upload).
    pub samples_gpu: Option<&'a [GpuTensor]>,
    pub encoders: &'a [&'a [f32]],
    pub width: usize,
    pub height: usize,
    pub channels: usize,
    pub heads: usize,
    pub prefix: &'a str,
    pub flags: ExtraFlags,
    pub dino: Option<&'a [f32]>,
    pub ref_samples: Option<&'a [&'a [f32]]>,
    /// Precomputed write-cache tokens `[n_ref * hw, channels]` (official `condition_embed_dict`).
    pub ref_tokens: Option<&'a [f32]>,
    pub ref_scale: f32,
    pub n_views: usize,
    pub voxel_xyz: Option<&'a [[u32; 3]]>,
    pub voxel_res: usize,
    pub mva_scale: f32,
    /// Per-CFG DINO packs (`n_cfg` entries). Broadcasts [`Self::dino`] when None.
    pub dinos: Option<&'a [&'a [f32]]>,
    /// Per-CFG reference scales. Broadcasts [`Self::ref_scale`] when None.
    pub ref_scales: Option<&'a [f32]>,
}

impl UnetFirst {
    /// Transformer2D + 2.5D extras. `samples` is CFG-major then
    /// `(n_pbr, n_views)` in `(pbr major, view minor)` order:
    /// `[alb0, alb1, ..., mr0, mr1, ...]` per CFG branch.
    pub fn transformer2d_extras_gpu(&self, inp: ExtraInputs<'_>) -> Result<Vec<GpuTensor>, String> {
        let ExtraInputs {
            samples,
            samples_gpu,
            encoders,
            width,
            height,
            channels,
            heads,
            prefix,
            flags,
            dino,
            ref_samples,
            ref_tokens,
            ref_scale,
            n_views,
            voxel_xyz,
            voxel_res,
            mva_scale,
            dinos,
            ref_scales,
        } = inp;
        let n = samples_gpu.map(|g| g.len()).unwrap_or(samples.len());
        if n == 0 || n != encoders.len() || n_views == 0 || n % n_views != 0 {
            return Err(format!(
                "{prefix} extras batch {} views {n_views} enc {}",
                n,
                encoders.len()
            ));
        }
        const N_PBR: usize = 2;
        let (n_cfg, n_pbr) = if n % (N_PBR * n_views) == 0 {
            (n / (N_PBR * n_views), N_PBR)
        } else {
            (1, n / n_views)
        };
        let group = n_pbr * n_views;
        let pbr_of = |i: usize| (i / n_views) % n_pbr;
        let cfg_of = |i: usize| i / group;
        let idx_of = |cfg: usize, pbr: usize, view: usize| cfg * group + pbr * n_views + view;
        let dino_of = |cfg: usize| -> Result<&[f32], String> {
            if let Some(ds) = dinos {
                ds.get(cfg)
                    .copied()
                    .ok_or_else(|| format!("{prefix} dinos len {} vs n_cfg {n_cfg}", ds.len()))
            } else {
                dino.ok_or_else(|| format!("{prefix} dino requested without tokens"))
            }
        };
        let ref_scale_of = |cfg: usize| -> Result<f32, String> {
            if let Some(rs) = ref_scales {
                rs.get(cfg)
                    .copied()
                    .ok_or_else(|| format!("{prefix} ref_scales len {} vs n_cfg {n_cfg}", rs.len()))
            } else {
                Ok(ref_scale)
            }
        };
        let plane = width * height;
        let hidden = channels;
        if hidden % heads != 0 {
            return Err(format!("{prefix} hidden {hidden} vs heads {heads}"));
        }
        let head_dim = hidden / heads;
        let scale = 1.0 / (head_dim as f32).sqrt();
        let inner = format!("{prefix}.transformer_blocks.0.transformer");
        let tb = format!("{prefix}.transformer_blocks.0");

        let nw = self.get(&format!("{prefix}.norm.weight"))?;
        let nb = self.get(&format!("{prefix}.norm.bias"))?;
        let residuals: Vec<GpuTensor> = if let Some(gs) = samples_gpu {
            gs.iter()
                .map(|g| gpu_slice_rows(g, 0, g.rows()))
                .collect::<Result<_, _>>()?
        } else {
            samples
                .iter()
                .map(|s| gpu_upload(s, channels, plane))
                .collect::<Result<_, _>>()?
        };
        let (n1s, mut tokens) = if paint_fast() && n > 1 {
            let refs: Vec<&GpuTensor> = residuals.iter().collect();
            let stacked = gpu_concat_cols(&refs)?;
            let gn = gpu_paint_group_norm_batched(
                &stacked,
                width,
                height,
                n,
                GN_GROUPS,
                NS,
                &format!("{prefix}.norm"),
                &nw.data,
                &nb.data,
                ATTN_GN_EPS,
            )?;
            let packed = gpu_planar_tokens_transpose(&gn)?;
            let packed = self.linear_tokens(&packed, &format!("{prefix}.proj_in"), true)?;
            let n1_all = self.layer_norm(&packed, &format!("{inner}.norm1"))?;
            (split_gpu(&n1_all, n, plane)?, split_gpu(&packed, n, plane)?)
        } else {
            let mut toks = Vec::with_capacity(n);
            for residual in &residuals {
                let gn = gpu_group_norm_planar(
                    residual,
                    width,
                    height,
                    GN_GROUPS,
                    NS,
                    &format!("{prefix}.norm"),
                    &nw.data,
                    &nb.data,
                    ATTN_GN_EPS,
                )?;
                toks.push(gpu_planar_tokens_transpose(&gn)?);
            }
            let packed = concat_gpu(&toks)?;
            let packed = self.linear_tokens(&packed, &format!("{prefix}.proj_in"), true)?;
            let n1_all = self.layer_norm(&packed, &format!("{inner}.norm1"))?;
            (split_gpu(&n1_all, n, plane)?, split_gpu(&packed, n, plane)?)
        };

        if flags.mda && n_pbr == 2 {
            let mut alb_n1 = Vec::with_capacity(n_cfg * n_views);
            let mut mr_n1 = Vec::with_capacity(n_cfg * n_views);
            for cfg in 0..n_cfg {
                for v in 0..n_views {
                    alb_n1.push(gpu_slice_rows(
                        &n1s[idx_of(cfg, 0, v)],
                        0,
                        n1s[idx_of(cfg, 0, v)].rows(),
                    )?);
                    mr_n1.push(gpu_slice_rows(
                        &n1s[idx_of(cfg, 1, v)],
                        0,
                        n1s[idx_of(cfg, 1, v)].rows(),
                    )?);
                }
            }
            let alb_attn = self.packed_self_batched(
                &alb_n1,
                &format!("{inner}.attn1.to_q"),
                &format!("{inner}.attn1.to_k"),
                &format!("{inner}.attn1.to_v"),
                &format!("{inner}.attn1.to_out.0"),
                heads,
                scale,
            )?;
            let mr_attn = self.packed_self_batched(
                &mr_n1,
                &format!("{inner}.attn1.processor.to_q_mr"),
                &format!("{inner}.attn1.processor.to_k_mr"),
                &format!("{inner}.attn1.processor.to_v_mr"),
                &format!("{inner}.attn1.processor.to_out_mr.0"),
                heads,
                scale,
            )?;
            let alb_split = split_gpu(&alb_attn, n_cfg * n_views, plane)?;
            let mr_split = split_gpu(&mr_attn, n_cfg * n_views, plane)?;
            for cfg in 0..n_cfg {
                for v in 0..n_views {
                    let ai = cfg * n_views + v;
                    let ia = idx_of(cfg, 0, v);
                    let im = idx_of(cfg, 1, v);
                    tokens[ia] = gpu_add(&tokens[ia], &alb_split[ai])?;
                    tokens[im] = gpu_add(&tokens[im], &mr_split[ai])?;
                }
            }
        } else {
            for i in 0..n {
                let pbr = pbr_of(i);
                let (q_pre, k_pre, v_pre, o_pre) = if flags.mda && pbr == 1 {
                    (
                        format!("{inner}.attn1.processor.to_q_mr"),
                        format!("{inner}.attn1.processor.to_k_mr"),
                        format!("{inner}.attn1.processor.to_v_mr"),
                        format!("{inner}.attn1.processor.to_out_mr.0"),
                    )
                } else {
                    (
                        format!("{inner}.attn1.to_q"),
                        format!("{inner}.attn1.to_k"),
                        format!("{inner}.attn1.to_v"),
                        format!("{inner}.attn1.to_out.0"),
                    )
                };
                let attn = self.packed_self(&n1s[i], &q_pre, &k_pre, &v_pre, &o_pre, heads, scale)?;
                tokens[i] = gpu_add(&tokens[i], &attn)?;
            }
        }

        if flags.ra {
            let cond = if let Some(tok) = ref_tokens {
                if hidden == 0 || tok.len() % hidden != 0 {
                    return Err(format!("{prefix} ra cache len {} vs hidden {hidden}", tok.len()));
                }
                gpu_upload(tok, tok.len() / hidden, hidden)?
            } else {
                let refs = ref_samples.ok_or("ra requested without ref_samples or ref_tokens")?;
                let mut cond_tok = Vec::new();
                for sample in refs {
                    let x = gpu_upload(sample, channels, plane)?;
                    let nw = self.get(&format!("{prefix}.norm.weight"))?;
                    let nb = self.get(&format!("{prefix}.norm.bias"))?;
                    let gn = gpu_group_norm_planar(
                        &x,
                        width,
                        height,
                        GN_GROUPS,
                        NS,
                        &format!("{prefix}.norm.ref"),
                        &nw.data,
                        &nb.data,
                        ATTN_GN_EPS,
                    )?;
                    let tok = gpu_planar_tokens_transpose(&gn)?;
                    let tok = self.linear_tokens(&tok, &format!("{prefix}.proj_in"), true)?;
                    cond_tok.push(self.layer_norm(&tok, &format!("{inner}.norm1"))?);
                }
                concat_gpu(&cond_tok)?
            };
            let mut alb_n1 = Vec::with_capacity(n_cfg * n_views);
            for cfg in 0..n_cfg {
                for v in 0..n_views {
                    alb_n1.push(gpu_slice_rows(
                        &n1s[idx_of(cfg, 0, v)],
                        0,
                        n1s[idx_of(cfg, 0, v)].rows(),
                    )?);
                }
            }
            let alb_q = concat_gpu(&alb_n1)?;
            let q = self.linear_tokens(&alb_q, &format!("{tb}.attn_refview.to_q"), false)?;
            let k = self.linear_tokens(&cond, &format!("{tb}.attn_refview.to_k"), false)?;
            let v_alb = self.linear_tokens(&cond, &format!("{tb}.attn_refview.to_v"), false)?;
            let v_mr = self.linear_tokens(&cond, &format!("{tb}.attn_refview.processor.to_v_mr"), false)?;
            let (o_alb, o_mr) = ref_attn_wide_like(&q, &k, &v_alb, &v_mr, heads, scale)?;
            let o_alb = self.linear_tokens(&o_alb, &format!("{tb}.attn_refview.to_out.0"), true)?;
            let o_mr = self.linear_tokens(
                &o_mr,
                &format!("{tb}.attn_refview.processor.to_out_mr.0"),
                true,
            )?;
            let cfg_rows = n_views * plane;
            for cfg in 0..n_cfg {
                let rs = ref_scale_of(cfg)?;
                if rs == 0.0 {
                    continue;
                }
                let alb = gpu_slice_rows(&o_alb, cfg * cfg_rows, cfg_rows)?;
                let mr = gpu_slice_rows(&o_mr, cfg * cfg_rows, cfg_rows)?;
                for pbr in 0..n_pbr {
                    let src = if pbr == 0 { &alb } else { &mr };
                    for v in 0..n_views {
                        let idx = idx_of(cfg, pbr, v);
                        let add = gpu_slice_rows(src, v * plane, plane)?;
                        let add = if (rs - 1.0).abs() < 1e-6 {
                            add
                        } else {
                            paint_scale_like(&add, rs)?
                        };
                        tokens[idx] = gpu_add(&tokens[idx], &add)?;
                    }
                }
            }
        }

        if flags.ma && n_views > 1 {
            let xyz = voxel_xyz.ok_or("ma requested without voxel_xyz")?;
            if xyz.len() != n_views * plane {
                return Err(format!(
                    "voxel_xyz {} vs views {n_views} plane {plane}",
                    xyz.len()
                ));
            }
            for cfg in 0..n_cfg {
                for pbr in 0..n_pbr {
                    let mut parts = Vec::with_capacity(n_views);
                    for v in 0..n_views {
                        parts.push(gpu_slice_rows(
                            &n1s[idx_of(cfg, pbr, v)],
                            0,
                            n1s[idx_of(cfg, pbr, v)].rows(),
                        )?);
                    }
                    let packed = concat_gpu(&parts)?;
                    let q = self.linear_tokens(&packed, &format!("{tb}.attn_multiview.to_q"), false)?;
                    let k = self.linear_tokens(&packed, &format!("{tb}.attn_multiview.to_k"), false)?;
                    let v = self.linear_tokens(&packed, &format!("{tb}.attn_multiview.to_v"), false)?;
                    let xyz_u32: Vec<u32> = xyz.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
                    let q = match_half(
                        gpu_paint_pose_rope(&as_f32(&q)?, &xyz_u32, heads, voxel_res)?,
                        &n1s[idx_of(cfg, pbr, 0)],
                    )?;
                    let k = match_half(
                        gpu_paint_pose_rope(&as_f32(&k)?, &xyz_u32, heads, voxel_res)?,
                        &n1s[idx_of(cfg, pbr, 0)],
                    )?;
                    let attn = attn_packed(&q, &k, &v, heads, scale)?;
                    let attn = self.linear_tokens(&attn, &format!("{tb}.attn_multiview.to_out.0"), true)?;
                    let attn = if (mva_scale - 1.0).abs() < 1e-6 {
                        attn
                    } else {
                        paint_scale_like(&attn, mva_scale)?
                    };
                    for view in 0..n_views {
                        let add = gpu_slice_rows(&attn, view * plane, plane)?;
                        let idx = idx_of(cfg, pbr, view);
                        tokens[idx] = gpu_add(&tokens[idx], &add)?;
                    }
                }
            }
        }

        let packed = concat_gpu(&tokens)?;
        let n2 = self.layer_norm(&packed, &format!("{inner}.norm2"))?;
        let q_all = self.linear_tokens(&n2, &format!("{inner}.attn2.to_q"), false)?;
        let mut groups: Vec<(usize, Vec<usize>)> = Vec::new();
        for i in 0..n {
            let key = encoders[i].as_ptr() as usize;
            match groups.iter_mut().find(|(k, _)| *k == key) {
                Some((_, idx)) => idx.push(i),
                None => groups.push((key, vec![i])),
            }
        }
        let mut crosses: Vec<Option<GpuTensor>> = (0..n).map(|_| None).collect();
        for (_, idx) in &groups {
            let enc = gpu_upload(encoders[idx[0]], encoders[idx[0]].len() / 1024, 1024)?;
            let k = self.linear_tokens(&enc, &format!("{inner}.attn2.to_k"), false)?;
            let v = self.linear_tokens(&enc, &format!("{inner}.attn2.to_v"), false)?;
            let qs: Vec<GpuTensor> = idx
                .iter()
                .map(|&i| gpu_slice_rows(&q_all, i * plane, plane))
                .collect::<Result<_, _>>()?;
            let q = concat_gpu(&qs)?;
            let cross = attn_cross(&q, &k, &v, heads, scale)?;
            let cross = self.linear_tokens(&cross, &format!("{inner}.attn2.to_out.0"), true)?;
            let split = split_gpu(&cross, idx.len(), plane)?;
            for (i, t) in idx.iter().copied().zip(split.into_iter()) {
                crosses[i] = Some(t);
            }
        }
        for i in 0..n {
            let add = crosses[i].take().ok_or("missing grouped cross")?;
            tokens[i] = gpu_add(&tokens[i], &add)?;
        }
        if flags.dino {
            let q_dino_all = self.linear_tokens(&n2, &format!("{tb}.attn_dino.to_q"), false)?;
            let mut dgroups: Vec<(usize, Vec<usize>)> = Vec::new();
            let mut dino_refs: Vec<&[f32]> = Vec::with_capacity(n);
            for i in 0..n {
                let d = dino_of(cfg_of(i))?;
                dino_refs.push(d);
                let key = d.as_ptr() as usize;
                match dgroups.iter_mut().find(|(k, _)| *k == key) {
                    Some((_, idx)) => idx.push(i),
                    None => dgroups.push((key, vec![i])),
                }
            }
            for (_, idx) in &dgroups {
                let dino = dino_refs[idx[0]];
                let dino_g = gpu_upload(dino, dino.len() / 1024, 1024)?;
                let k = self.linear_tokens(&dino_g, &format!("{tb}.attn_dino.to_k"), false)?;
                let v = self.linear_tokens(&dino_g, &format!("{tb}.attn_dino.to_v"), false)?;
                let qs: Vec<GpuTensor> = idx
                    .iter()
                    .map(|&i| gpu_slice_rows(&q_dino_all, i * plane, plane))
                    .collect::<Result<_, _>>()?;
                let q = concat_gpu(&qs)?;
                let d = attn_cross(&q, &k, &v, heads, scale)?;
                let d = self.linear_tokens(&d, &format!("{tb}.attn_dino.to_out.0"), true)?;
                let split = split_gpu(&d, idx.len(), plane)?;
                for (j, &i) in idx.iter().enumerate() {
                    tokens[i] = gpu_add(&tokens[i], &split[j])?;
                }
            }
        }
        let packed = concat_gpu(&tokens)?;
        let n3 = self.layer_norm(&packed, &format!("{inner}.norm3"))?;
        let ff = self.geglu_ff(&n3, &inner)?;
        let packed = gpu_add(&packed, &ff)?;
        let packed = self.linear_tokens(&packed, &format!("{prefix}.proj_out"), true)?;
        tokens = split_gpu(&packed, n, plane)?;

        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let planar = gpu_planar_tokens_transpose(&tokens[i])?;
            out.push(gpu_add(&planar, &residuals[i])?);
        }
        Ok(out)
    }

    /// Transformer2D + 2.5D extras on one stacked `[C, n*H*W]` / `[n*HW, C]`
    /// tensor. MDA/RA gather alb vs mr as `n_cfg` blocks of `n_views*HW`;
    /// MA is one batched attn over `(n_cfg*n_pbr, n_views*HW)`.
    pub(crate) fn transformer2d_extras_batch(
        &self,
        x: &BatchAct,
        prefix: &str,
        channels: usize,
        heads: usize,
        n_views: usize,
        res: &ExtraResident<'_>,
        ref_scales: &[f32],
        mva_scale: f32,
    ) -> Result<BatchAct, String> {
        let n = x.n;
        const N_PBR: usize = 2;
        if n == 0 || n_views == 0 || n % (N_PBR * n_views) != 0 {
            return Err(format!("{prefix} extras batch n={} views {n_views}", n));
        }
        let n_cfg = n / (N_PBR * n_views);
        if ref_scales.len() != n_cfg {
            return Err(format!(
                "{prefix} ref_scales {} vs n_cfg {n_cfg}",
                ref_scales.len()
            ));
        }
        x.expect(channels, x.w, x.h, prefix)?;
        let hidden = channels;
        if hidden % heads != 0 {
            return Err(format!("{prefix} hidden {hidden} vs heads {heads}"));
        }
        let head_dim = hidden / heads;
        let scale = 1.0 / (head_dim as f32).sqrt();
        let plane = x.plane();
        let view_blk = n_views * plane;
        let inner = format!("{prefix}.transformer_blocks.0.transformer");
        let tb = format!("{prefix}.transformer_blocks.0");
        let nw = self.get(&format!("{prefix}.norm.weight"))?;
        let nb = self.get(&format!("{prefix}.norm.bias"))?;
        let residual = x.clone_act()?;
        let gn = gpu_group_norm_nchw(
            &residual.t,
            channels,
            x.w,
            x.h,
            GN_GROUPS,
            NS,
            &format!("{prefix}.norm"),
            &nw.data,
            &nb.data,
            ATTN_GN_EPS,
        )?;
        let packed = gpu_nchw_to_tokens(&gn, channels, x.w, x.h)?;
        let mut tokens = self.linear_tokens(&packed, &format!("{prefix}.proj_in"), true)?;
        let n1 = self.layer_norm(&tokens, &format!("{inner}.norm1"))?;

        let alb_n1 = gather_pbr(&n1, n_cfg, n_views, 0, plane)?;
        let mr_n1 = gather_pbr(&n1, n_cfg, n_views, 1, plane)?;
        let alb_attn = self.packed_self_tensor(
            &alb_n1,
            n_cfg * n_views,
            &format!("{inner}.attn1.to_q"),
            &format!("{inner}.attn1.to_k"),
            &format!("{inner}.attn1.to_v"),
            &format!("{inner}.attn1.to_out.0"),
            heads,
            scale,
        )?;
        let mr_attn = self.packed_self_tensor(
            &mr_n1,
            n_cfg * n_views,
            &format!("{inner}.attn1.processor.to_q_mr"),
            &format!("{inner}.attn1.processor.to_k_mr"),
            &format!("{inner}.attn1.processor.to_v_mr"),
            &format!("{inner}.attn1.processor.to_out_mr.0"),
            heads,
            scale,
        )?;
        tokens = scatter_add_pbr(&tokens, &alb_attn, &mr_attn, n_cfg, n_views, plane)?;

        let alb_q = self.linear_tokens(&alb_n1, &format!("{tb}.attn_refview.to_q"), false)?;
        let (k, v_alb) = self.encoder_kv(res.ref_tokens, &format!("{tb}.attn_refview"), "ref")?;
        let v_mr = self.encoder_lin(
            res.ref_tokens,
            &format!("{tb}.attn_refview.processor.to_v_mr"),
            &format!("{tb}.attn_refview|ref_mr"),
        )?;
        let (o_alb, o_mr) = ref_attn_wide_like(&alb_q, &k, &v_alb, &v_mr, heads, scale)?;
        let mut o_alb = self.linear_tokens(&o_alb, &format!("{tb}.attn_refview.to_out.0"), true)?;
        let mut o_mr = self.linear_tokens(
            &o_mr,
            &format!("{tb}.attn_refview.processor.to_out_mr.0"),
            true,
        )?;
        for cfg in 0..n_cfg {
            let rs = ref_scales[cfg];
            if (rs - 1.0).abs() < 1e-6 {
                continue;
            }
            let alb = gpu_slice_rows(&o_alb, cfg * view_blk, view_blk)?;
            let mr = gpu_slice_rows(&o_mr, cfg * view_blk, view_blk)?;
            let alb = paint_scale_like(&alb, rs)?;
            let mr = paint_scale_like(&mr, rs)?;
            o_alb = replace_block(&o_alb, cfg * view_blk, &alb)?;
            o_mr = replace_block(&o_mr, cfg * view_blk, &mr)?;
        }
        tokens = scatter_add_pbr(&tokens, &o_alb, &o_mr, n_cfg, n_views, plane)?;

        if n_views > 1 {
            let (q, k, v) = self.linear_qkv(
                &n1,
                &format!("{tb}.attn_multiview.to_q"),
                &format!("{tb}.attn_multiview.to_k"),
                &format!("{tb}.attn_multiview.to_v"),
            )?;
            let q = match_half(
                gpu_paint_pose_rope_dev(&as_f32(&q)?, res.voxel, heads, res.voxel_res)?,
                &n1,
            )?;
            let k = match_half(
                gpu_paint_pose_rope_dev(&as_f32(&k)?, res.voxel, heads, res.voxel_res)?,
                &n1,
            )?;
            let ma_batch = n_cfg * N_PBR;
            // Official: one F.sdpa on [B,H,S,64] after rearrange (b n_pbr)(n l)c.
            let attn = attn_packed_batched(&q, &k, &v, ma_batch, heads, scale)?;
            let attn = self.linear_tokens(&attn, &format!("{tb}.attn_multiview.to_out.0"), true)?;
            let attn = if (mva_scale - 1.0).abs() < 1e-6 {
                attn
            } else {
                paint_scale_like(&attn, mva_scale)?
            };
            tokens = gpu_add(&tokens, &attn)?;
        }

        let packed = tokens;
        let n2 = self.layer_norm(&packed, &format!("{inner}.norm2"))?;
        let q_all = self.linear_tokens(&n2, &format!("{inner}.attn2.to_q"), false)?;
        let cross = self.cross_cfg_groups(
            &q_all,
            n_cfg,
            n_views,
            plane,
            heads,
            scale,
            &format!("{inner}.attn2"),
            res.enc0,
            res.enc_alb,
            res.enc_mr,
        )?;
        let tokens = gpu_add(&packed, &cross)?;
        let q_dino = self.linear_tokens(&n2, &format!("{tb}.attn_dino.to_q"), false)?;
        let dino = self.dino_cfg_groups(
            &q_dino,
            n_cfg,
            n_views,
            plane,
            heads,
            scale,
            &format!("{tb}.attn_dino"),
            res.dino0,
            res.dino,
        )?;
        let tokens = gpu_add(&tokens, &dino)?;
        let n3 = self.layer_norm(&tokens, &format!("{inner}.norm3"))?;
        let ff = self.geglu_ff(&n3, &inner)?;
        let tokens = gpu_add(&tokens, &ff)?;
        let tokens = self.linear_tokens(&tokens, &format!("{prefix}.proj_out"), true)?;
        let nchw = gpu_tokens_to_nchw(&tokens, n, x.w, x.h)?;
        Ok(BatchAct {
            t: gpu_add(&nchw, &residual.t)?,
            n,
            c: channels,
            w: x.w,
            h: x.h,
        })
    }

    pub fn transformer2d_extras(&self, inp: ExtraInputs<'_>) -> Result<Vec<Vec<f32>>, String> {
        self.transformer2d_extras_gpu(inp)?
            .iter()
            .map(gpu_download)
            .collect()
    }

    fn packed_self(
        &self,
        x: &GpuTensor,
        q: &str,
        k: &str,
        v: &str,
        o: &str,
        heads: usize,
        scale: f32,
    ) -> Result<GpuTensor, String> {
        let (q, k, v) = self.linear_qkv(x, q, k, v)?;
        let attn = attn_packed(&q, &k, &v, heads, scale)?;
        self.linear_tokens(&attn, o, true)
    }

    fn packed_self_batched(
        &self,
        xs: &[GpuTensor],
        q: &str,
        k: &str,
        v: &str,
        o: &str,
        heads: usize,
        scale: f32,
    ) -> Result<GpuTensor, String> {
        if xs.is_empty() {
            return Err("packed_self_batched empty".into());
        }
        let packed = concat_gpu(xs)?;
        let q = self.linear_tokens(&packed, q, false)?;
        let k = self.linear_tokens(&packed, k, false)?;
        let v = self.linear_tokens(&packed, v, false)?;
        let attn = attn_packed_batched(&q, &k, &v, xs.len(), heads, scale)?;
        self.linear_tokens(&attn, o, true)
    }

    fn packed_self_tensor(
        &self,
        packed: &GpuTensor,
        batch: usize,
        q: &str,
        k: &str,
        v: &str,
        o: &str,
        heads: usize,
        scale: f32,
    ) -> Result<GpuTensor, String> {
        let (q, k, v) = self.linear_qkv(packed, q, k, v)?;
        let attn = attn_packed_batched(&q, &k, &v, batch, heads, scale)?;
        self.linear_tokens(&attn, o, true)
    }

    fn cross_cfg_groups(
        &self,
        q_all: &GpuTensor,
        n_cfg: usize,
        n_views: usize,
        plane: usize,
        heads: usize,
        scale: f32,
        prefix: &str,
        enc0: &GpuTensor,
        enc_alb: &GpuTensor,
        enc_mr: &GpuTensor,
    ) -> Result<GpuTensor, String> {
        let view_blk = n_views * plane;
        let group = 2 * view_blk;
        let batch = n_cfg * 2 * n_views;
        if n_cfg == 3 && paint_fast() && q_all.rows() == batch * plane {
            // Official: encoder.unsqueeze.repeat(N_gen) then
            // rearrange (b n_pbr n) — zeros cfg0 both materials, learned alb,
            // learned mr. One F.sdpa [36,H,L,64]×[36,H,77,64]. Q is already
            // in that row order; packed K/V are cached across the 15 steps.
            let (k, v) = self.cache_kv(&format!("{prefix}|text36"), || {
                let (k0, v0) = self.encoder_kv(enc0, prefix, "enc0")?;
                let (k_alb, v_alb) = self.encoder_kv(enc_alb, prefix, "enc_alb")?;
                let (k_mr, v_mr) = self.encoder_kv(enc_mr, prefix, "enc_mr")?;
                Ok((
                    pack_text_kv_official(&k0, &k_alb, &k_mr, n_views)?,
                    pack_text_kv_official(&v0, &v_alb, &v_mr, n_views)?,
                ))
            })?;
            let attn = attn_cross_batched(q_all, &k, &v, batch, heads, scale)?;
            return self.linear_tokens(&attn, &format!("{prefix}.to_out.0"), true);
        }
        if n_cfg == 3 {
            let (k0, v0) = self.encoder_kv(enc0, prefix, "enc0")?;
            let q0 = gpu_slice_rows(q_all, 0, group)?;
            let c0 = attn_cross(&q0, &k0, &v0, heads, scale)?;

            let q_alb = concat_rows_n(&[
                gpu_slice_rows(q_all, group, view_blk)?,
                gpu_slice_rows(q_all, 2 * group, view_blk)?,
            ])?;
            let (k_alb, v_alb) = self.encoder_kv(enc_alb, prefix, "enc_alb")?;
            let c_alb = attn_cross(&q_alb, &k_alb, &v_alb, heads, scale)?;

            let q_mr = concat_rows_n(&[
                gpu_slice_rows(q_all, group + view_blk, view_blk)?,
                gpu_slice_rows(q_all, 2 * group + view_blk, view_blk)?,
            ])?;
            let (k_mr, v_mr) = self.encoder_kv(enc_mr, prefix, "enc_mr")?;
            let c_mr = attn_cross(&q_mr, &k_mr, &v_mr, heads, scale)?;

            let packed = concat_rows_n(&[c0, c_alb, c_mr])?;
            let packed = self.linear_tokens(&packed, &format!("{prefix}.to_out.0"), true)?;
            let c0 = gpu_slice_rows(&packed, 0, group)?;
            let c1_alb = gpu_slice_rows(&packed, group, view_blk)?;
            let c2_alb = gpu_slice_rows(&packed, group + view_blk, view_blk)?;
            let c1_mr = gpu_slice_rows(&packed, group + 2 * view_blk, view_blk)?;
            let c2_mr = gpu_slice_rows(&packed, group + 3 * view_blk, view_blk)?;
            return concat_rows_n(&[c0, c1_alb, c1_mr, c2_alb, c2_mr]);
        }
        let mut parts = Vec::with_capacity(n_cfg * 2);
        for cfg in 0..n_cfg {
            let q_alb = gpu_slice_rows(q_all, cfg * group, view_blk)?;
            let q_mr = gpu_slice_rows(q_all, cfg * group + view_blk, view_blk)?;
            let k_alb = self.linear_tokens(enc_alb, &format!("{prefix}.to_k"), false)?;
            let v_alb = self.linear_tokens(enc_alb, &format!("{prefix}.to_v"), false)?;
            let k_mr = self.linear_tokens(enc_mr, &format!("{prefix}.to_k"), false)?;
            let v_mr = self.linear_tokens(enc_mr, &format!("{prefix}.to_v"), false)?;
            let c_alb = attn_cross(&q_alb, &k_alb, &v_alb, heads, scale)?;
            let c_mr = attn_cross(&q_mr, &k_mr, &v_mr, heads, scale)?;
            parts.push(self.linear_tokens(&c_alb, &format!("{prefix}.to_out.0"), true)?);
            parts.push(self.linear_tokens(&c_mr, &format!("{prefix}.to_out.0"), true)?);
        }
        concat_rows_n(&parts)
    }

    fn dino_cfg_groups(
        &self,
        q_all: &GpuTensor,
        n_cfg: usize,
        n_views: usize,
        plane: usize,
        heads: usize,
        scale: f32,
        prefix: &str,
        dino0: &GpuTensor,
        dino: &GpuTensor,
    ) -> Result<GpuTensor, String> {
        let group = 2 * n_views * plane;
        let batch = n_cfg * 2 * n_views;
        if n_cfg == 3 && paint_fast() && q_all.rows() == batch * plane {
            // Official: dino.unsqueeze(1).repeat(1, N_pbr*N_gen) then
            // rearrange (b n) — zeros on cfg0+cfg1, real DINO on cfg2.
            // One F.sdpa [36,H,L,64]×[36,H,1028,64].
            let (k, v) = self.cache_kv(&format!("{prefix}|dino36"), || {
                let (k0, v0) = self.encoder_kv(dino0, prefix, "dino0")?;
                let (k1, v1) = self.encoder_kv(dino, prefix, "dino")?;
                Ok((
                    pack_dino_kv_official(&k0, &k1, n_views)?,
                    pack_dino_kv_official(&v0, &v1, n_views)?,
                ))
            })?;
            let attn = attn_cross_batched(q_all, &k, &v, batch, heads, scale)?;
            return self.linear_tokens(&attn, &format!("{prefix}.to_out.0"), true);
        }
        if n_cfg == 3 {
            let (k0, v0) = self.encoder_kv(dino0, prefix, "dino0")?;
            let q0 = gpu_slice_rows(q_all, 0, 2 * group)?;
            let d0 = attn_cross(&q0, &k0, &v0, heads, scale)?;
            let (k1, v1) = self.encoder_kv(dino, prefix, "dino")?;
            let q1 = gpu_slice_rows(q_all, 2 * group, group)?;
            let d1 = attn_cross(&q1, &k1, &v1, heads, scale)?;
            let packed = concat_rows_n(&[d0, d1])?;
            return self.linear_tokens(&packed, &format!("{prefix}.to_out.0"), true);
        }
        let k = self.linear_tokens(dino, &format!("{prefix}.to_k"), false)?;
        let v = self.linear_tokens(dino, &format!("{prefix}.to_v"), false)?;
        let d = attn_cross(q_all, &k, &v, heads, scale)?;
        self.linear_tokens(&d, &format!("{prefix}.to_out.0"), true)
    }
}

fn concat_gpu(xs: &[GpuTensor]) -> Result<GpuTensor, String> {
    let refs: Vec<&GpuTensor> = xs.iter().collect();
    gpu_concat_rows_many(&refs)
}

fn split_gpu(x: &GpuTensor, n: usize, rows: usize) -> Result<Vec<GpuTensor>, String> {
    if x.rows() != n * rows {
        return Err(format!("split_gpu rows {} vs {n}*{rows}", x.rows()));
    }
    (0..n)
        .map(|i| gpu_slice_rows(x, i * rows, rows))
        .collect()
}

fn concat_rows_n(xs: &[GpuTensor]) -> Result<GpuTensor, String> {
    let refs: Vec<&GpuTensor> = xs.iter().collect();
    gpu_concat_rows_many(&refs)
}

/// Official `encoder.unsqueeze(-3).repeat(1,1,N_gen)` then
/// `rearrange("b n_pbr n l c -> (b n_pbr n) l c")` for 3-branch CFG:
/// zeros on cfg0 both materials, learned alb, learned mr.
fn pack_text_kv_official(
    k0: &GpuTensor,
    k_alb: &GpuTensor,
    k_mr: &GpuTensor,
    n_views: usize,
) -> Result<GpuTensor, String> {
    let mut parts = Vec::with_capacity(3 * 2 * n_views);
    for _ in 0..n_views {
        parts.push(k0);
    }
    for _ in 0..n_views {
        parts.push(k0);
    }
    for _ in 0..n_views {
        parts.push(k_alb);
    }
    for _ in 0..n_views {
        parts.push(k_mr);
    }
    for _ in 0..n_views {
        parts.push(k_alb);
    }
    for _ in 0..n_views {
        parts.push(k_mr);
    }
    gpu_concat_rows_many(&parts)
}

/// Official DINO: each CFG token pack is repeated `N_pbr * N_gen` times.
/// 3-branch table is `[zeros, zeros, dino]`.
fn pack_dino_kv_official(
    k0: &GpuTensor,
    k1: &GpuTensor,
    n_views: usize,
) -> Result<GpuTensor, String> {
    let reps = 2 * n_views;
    let mut parts = Vec::with_capacity(3 * reps);
    for _ in 0..reps {
        parts.push(k0);
    }
    for _ in 0..reps {
        parts.push(k0);
    }
    for _ in 0..reps {
        parts.push(k1);
    }
    gpu_concat_rows_many(&parts)
}

fn gather_pbr(
    tokens: &GpuTensor,
    n_cfg: usize,
    n_views: usize,
    pbr: usize,
    plane: usize,
) -> Result<GpuTensor, String> {
    let view_blk = n_views * plane;
    let group = 2 * view_blk;
    let mut parts = Vec::with_capacity(n_cfg);
    for cfg in 0..n_cfg {
        parts.push(gpu_slice_rows(tokens, cfg * group + pbr * view_blk, view_blk)?);
    }
    concat_rows_n(&parts)
}

fn scatter_add_pbr(
    tokens: &GpuTensor,
    add_alb: &GpuTensor,
    add_mr: &GpuTensor,
    n_cfg: usize,
    n_views: usize,
    plane: usize,
) -> Result<GpuTensor, String> {
    let view_blk = n_views * plane;
    let mut parts = Vec::with_capacity(n_cfg * 2);
    for cfg in 0..n_cfg {
        parts.push(gpu_slice_rows(add_alb, cfg * view_blk, view_blk)?);
        parts.push(gpu_slice_rows(add_mr, cfg * view_blk, view_blk)?);
    }
    let add = concat_rows_n(&parts)?;
    gpu_add(tokens, &add)
}

fn replace_block(src: &GpuTensor, start: usize, block: &GpuTensor) -> Result<GpuTensor, String> {
    let end = start + block.rows();
    if end > src.rows() {
        return Err(format!(
            "replace_block {start}+{} vs {}",
            block.rows(),
            src.rows()
        ));
    }
    let mut parts = Vec::new();
    if start > 0 {
        parts.push(gpu_slice_rows(src, 0, start)?);
    }
    parts.push(gpu_slice_rows(block, 0, block.rows())?);
    if end < src.rows() {
        parts.push(gpu_slice_rows(src, end, src.rows() - end)?);
    }
    concat_rows_n(&parts)
}

/// Official `reshape_qkv` + one `F.sdpa` over batch `B`.
/// Packed tokens are `[B*S, H*D]`; concat-cols makes `[S, (B*H)*D]`, which is
/// the same independent-head math as `[B,H,S,D]`.
fn attn_sdpa_groups(
    q: &GpuTensor,
    k: &GpuTensor,
    v: &GpuTensor,
    batch: usize,
    seq: usize,
    heads: usize,
    scale: f32,
) -> Result<GpuTensor, String> {
    if batch == 0 || q.rows() != batch * seq {
        return Err(format!("attn_sdpa_groups rows {} vs {batch}*{seq}", q.rows()));
    }
    if k.rows() % batch != 0 || v.rows() != k.rows() {
        return Err(format!(
            "attn_sdpa_groups kv {} {} batch {batch}",
            k.rows(),
            v.rows()
        ));
    }
    if batch == 1 {
        return attn_packed(q, k, v, heads, scale);
    }
    let hidden = q.cols();
    let k_seq = k.rows() / batch;
    let mut qs = Vec::with_capacity(batch);
    let mut ks = Vec::with_capacity(batch);
    let mut vs = Vec::with_capacity(batch);
    for i in 0..batch {
        qs.push(gpu_slice_rows(q, i * seq, seq)?);
        ks.push(gpu_slice_rows(k, i * k_seq, k_seq)?);
        vs.push(gpu_slice_rows(v, i * k_seq, k_seq)?);
    }
    let q_refs: Vec<&GpuTensor> = qs.iter().collect();
    let k_refs: Vec<&GpuTensor> = ks.iter().collect();
    let v_refs: Vec<&GpuTensor> = vs.iter().collect();
    let q_b = gpu_concat_cols(&q_refs)?;
    let k_b = gpu_concat_cols(&k_refs)?;
    let v_b = gpu_concat_cols(&v_refs)?;
    let out = attn_packed(&q_b, &k_b, &v_b, batch * heads, scale)?;
    let mut parts = Vec::with_capacity(batch);
    for i in 0..batch {
        parts.push(gpu_slice_cols(&out, i * hidden, hidden)?);
    }
    concat_rows_n(&parts)
}

#[allow(dead_code)]
fn ref_attn_wide_v(
    q: &[f32],
    k: &[f32],
    v_alb: &[f32],
    v_mr: &[f32],
    q_len: usize,
    kv_len: usize,
    hidden: usize,
    heads: usize,
    scale: f32,
) -> Result<(Vec<f32>, Vec<f32>), String> {
    if heads == 0 || hidden % heads != 0 {
        return Err("ref attn head mismatch".into());
    }
    let head_dim = hidden / heads;
    let v_wide = hidden * 2;
    let v_head = v_wide / heads;
    if v_head != head_dim * 2 {
        return Err(format!("ref attn v_head {v_head} vs {}", head_dim * 2));
    }
    let mut v_pack = vec![0.0f32; kv_len * v_wide];
    for ki in 0..kv_len {
        let dst = &mut v_pack[ki * v_wide..(ki + 1) * v_wide];
        dst[..hidden].copy_from_slice(&v_alb[ki * hidden..(ki + 1) * hidden]);
        dst[hidden..].copy_from_slice(&v_mr[ki * hidden..(ki + 1) * hidden]);
    }
    let mut out_wide = vec![0.0f32; q_len * v_wide];
    for h in 0..heads {
        for qi in 0..q_len {
            let qh = &q[qi * hidden + h * head_dim..qi * hidden + (h + 1) * head_dim];
            let mut scores = vec![0.0f32; kv_len];
            let mut max = f32::NEG_INFINITY;
            for ki in 0..kv_len {
                let kh = &k[ki * hidden + h * head_dim..ki * hidden + (h + 1) * head_dim];
                let mut dot = 0.0f32;
                for d in 0..head_dim {
                    dot += qh[d] * kh[d];
                }
                scores[ki] = dot * scale;
                max = max.max(scores[ki]);
            }
            let mut sum = 0.0f32;
            for s in &mut scores {
                *s = (*s - max).exp();
                sum += *s;
            }
            let inv = 1.0 / sum.max(1e-12);
            let dst = &mut out_wide[qi * v_wide + h * v_head..qi * v_wide + (h + 1) * v_head];
            for ki in 0..kv_len {
                let p = scores[ki] * inv;
                let src = &v_pack[ki * v_wide + h * v_head..ki * v_wide + (h + 1) * v_head];
                for d in 0..v_head {
                    dst[d] += p * src[d];
                }
            }
        }
    }
    let mut o0 = vec![0.0f32; q_len * hidden];
    let mut o1 = vec![0.0f32; q_len * hidden];
    for qi in 0..q_len {
        for h in 0..heads {
            let src = &out_wide[qi * v_wide + h * v_head..qi * v_wide + (h + 1) * v_head];
            o0[qi * hidden + h * head_dim..qi * hidden + (h + 1) * head_dim]
                .copy_from_slice(&src[..head_dim]);
            o1[qi * hidden + h * head_dim..qi * hidden + (h + 1) * head_dim]
                .copy_from_slice(&src[head_dim..]);
        }
    }
    Ok((o0, o1))
}

#[allow(dead_code)]
fn scale_vec(x: &[f32], s: f32) -> Vec<f32> {
    x.iter().map(|v| v * s).collect()
}

fn rotary_pair_table(dim: usize, pos: usize, theta: f32) -> (Vec<f32>, Vec<f32>) {
    let half = dim / 2;
    let mut cos = vec![0.0f32; dim];
    let mut sin = vec![0.0f32; dim];
    for i in 0..half {
        let freq = 1.0 / theta.powf((2 * i) as f32 / dim as f32);
        let a = pos as f32 * freq;
        let c = a.cos();
        let s = a.sin();
        cos[i * 2] = c;
        cos[i * 2 + 1] = c;
        sin[i * 2] = s;
        sin[i * 2 + 1] = s;
    }
    (cos, sin)
}

fn rope_3d(xyz: [u32; 3], embed_dim: usize, voxel_res: usize) -> (Vec<f32>, Vec<f32>) {
    let dim_xy = embed_dim / 8 * 3;
    let dim_z = embed_dim / 8 * 2;
    let clamp = |v: u32| v.min(voxel_res.saturating_sub(1) as u32) as usize;
    let (xc, xs) = rotary_pair_table(dim_xy, clamp(xyz[0]), 10000.0);
    let (yc, ys) = rotary_pair_table(dim_xy, clamp(xyz[1]), 10000.0);
    let (zc, zs) = rotary_pair_table(dim_z, clamp(xyz[2]), 10000.0);
    let mut cos = Vec::with_capacity(embed_dim);
    let mut sin = Vec::with_capacity(embed_dim);
    cos.extend_from_slice(&xc);
    cos.extend_from_slice(&yc);
    cos.extend_from_slice(&zc);
    sin.extend_from_slice(&xs);
    sin.extend_from_slice(&ys);
    sin.extend_from_slice(&zs);
    (cos, sin)
}

fn apply_pair_rope(x: &mut [f32], cos: &[f32], sin: &[f32]) {
    for i in 0..x.len() / 2 {
        let re = x[i * 2];
        let im = x[i * 2 + 1];
        x[i * 2] = re * cos[i * 2] + (-im) * sin[i * 2];
        x[i * 2 + 1] = im * cos[i * 2 + 1] + re * sin[i * 2 + 1];
    }
}

#[allow(dead_code)]
fn apply_pose_rope(
    x: &GpuTensor,
    xyz: &[[u32; 3]],
    voxel_res: usize,
    heads: usize,
) -> Result<GpuTensor, String> {
    let hidden = x.cols();
    let seq = x.rows();
    if seq != xyz.len() {
        return Err(format!("rope seq {seq} vs xyz {}", xyz.len()));
    }
    if hidden % heads != 0 {
        return Err("rope head mismatch".into());
    }
    let head_dim = hidden / heads;
    let mut host = gpu_download(x)?;
    for s in 0..seq {
        let (cos, sin) = rope_3d(xyz[s], head_dim, voxel_res);
        for h in 0..heads {
            let start = s * hidden + h * head_dim;
            apply_pair_rope(&mut host[start..start + head_dim], &cos, &sin);
        }
    }
    gpu_upload(&host, seq, hidden)
}
