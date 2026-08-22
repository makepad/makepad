//! CUDA path for the IndexTTS-2.5 BigVGAN vocoder — convolutions as
//! im2col GEMMs and anti-aliased SnakeBeta activations as one fused device
//! kernel; a synthesis has no mid-network host round trips.
//!
//! Layout: activations live as `[t, ch]` time-major rows (GEMM operand
//! orientation), the transpose of the CPU `Plane` (`[ch][t]`).
//!
//! - Conv1d (stride 1, zero pad, dilation d): per tap `kk` a
//!   `gpu_gather_rows_colblock` over `x` extended with one sentinel zero row
//!   (out-of-range taps land there), the taps column-concatenated into
//!   `[t, k*ch]` and hit with one cached GEMM whose weight rows pack the
//!   taps kk-major (`w2[o][kk*ch+i] = w[o][i][kk]`) — the s2mel WaveNet
//!   "tap5" trick generalized.
//! - ConvTranspose1d (k = 2*stride, padding = stride/2, the BigVGAN
//!   configs): polyphase split. Output row `n` belongs to phase
//!   `r = (n+p) mod s` and only reads input rows `q-1, q, q+1` (`q = n/s`),
//!   so ONE shared 3-offset gather `[t, 3*ch_in]` feeds `s` per-phase GEMMs
//!   (unused taps zero in the packed weight), whose outputs are stacked and
//!   re-interleaved with a final row gather.
//! - The mean over the 3 AMP blocks (`x = xs/num_kernels`) is folded into
//!   the consuming ConvTranspose weights (stages 1..) or applied during the
//!   host transpose feeding `activation_post` (last stage) — no device
//!   scale op needed.
//! - AliasFreeSnake (up2x -> snakebeta -> down2x) is one fused device kernel
//!   (`gpu_alias_snake_updown2x`) over the time-major rows. Each of the 109
//!   activations owns a resident combined parameter buffer
//!   `[alpha | inv_beta | up_filter | down_filter]` (values exactly as the
//!   CPU preprocesses them: alpha/inv_beta preexponentiated, ratio gain
//!   folded into the up taps), uploaded once at session build under a unique
//!   per-activation key. The `input_scale` argument carries the last stage's
//!   mean-of-blocks fold into `activation_post`.
//!
//! Parity is gated by `indextts_cuda_validate --stage solve`: CUDA-vs-CPU
//! wav cosine plus wav-level spectrogram cosine against the official oracle
//! wav (the same phase-robust gate the CPU path passes).

use super::*;
use crate::backend::{
    gpu_add, gpu_concat_cols, gpu_concat_rows, gpu_device_available, gpu_download,
    gpu_gather_rows_colblock, gpu_gemm_f16acc_enabled, gpu_linear_nt_cached, gpu_upload,
    gpu_upload_u32, gpu_weight_cache_ensure, GpuLinearPart, GpuTensor,
};
use makepad_ai_common::quant::{f32_to_f16, GGML_TYPE_F16};
use std::collections::HashMap;
use std::sync::Mutex;

const NS: &str = "indextts_bigvgan";

/// Error context for the conv-GEMM plumbing shared with the codec CUDA path
/// (`indextts_codec_cuda.rs` imports the `pub(crate)` helpers below).
pub(crate) fn gm(error: String) -> DiffusionError {
    DiffusionError::model(format!("indextts cuda: {error}"))
}

/// True when the CUDA device path is available in this build.
pub fn bigvgan_cuda_available() -> bool {
    gpu_device_available()
}

pub(crate) fn f32_to_f16_bytes(values: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(values.len() * 2);
    for &value in values {
        out.extend_from_slice(&f32_to_f16(value).to_le_bytes());
    }
    out
}

pub(crate) fn ensure_weight(
    ns: &str,
    key: &str,
    w: &[f32],
    out_dim: usize,
    in_dim: usize,
) -> Result<()> {
    debug_assert_eq!(w.len(), out_dim * in_dim);
    // Same ::a16 cache-key rule as the s2mel path: rows > 1 GEMMs look up
    // with the f16acc suffix exactly when f16acc gemms are enabled.
    let want_a16 = gpu_gemm_f16acc_enabled();
    gpu_weight_cache_ensure(ns, key, GGML_TYPE_F16, out_dim, in_dim, want_a16, || {
        Ok(f32_to_f16_bytes(w))
    })
    .map_err(gm)
}

pub(crate) fn lin(
    x: &GpuTensor,
    ns: &str,
    key: &str,
    n: usize,
    bias: &[f32],
) -> Result<GpuTensor> {
    let parts = [GpuLinearPart {
        bt_ggml_type: GGML_TYPE_F16,
        n,
        cache_key: key,
        bytes: &[],
    }];
    gpu_linear_nt_cached(x, ns, &parts, bias).map_err(gm)
}

/// Conv1d weight `(out, in, k)` packed to GEMM rows `[out, k*in]` kk-major,
/// matching the tap-major column concat of the gathered operand.
fn pack_conv1d(conv: &Conv1d) -> Vec<f32> {
    let (out_ch, in_ch, k) = (conv.out_ch, conv.in_ch, conv.k);
    let mut wk = vec![0f32; out_ch * k * in_ch];
    for o in 0..out_ch {
        for i in 0..in_ch {
            for kk in 0..k {
                wk[o * k * in_ch + kk * in_ch + i] = conv.weight[(o * in_ch + i) * k + kk];
            }
        }
    }
    wk
}

/// One polyphase GEMM weight `[out, 3*in]` for output phase `r` of a
/// ConvTranspose1d, offsets -1/0/+1 blocks in order; taps the phase does not
/// use stay zero. `scale` folds the upstream mean-of-blocks (bias unscaled).
fn pack_convt_phase(up: &ConvTranspose1d, r: usize, scale: f32) -> Vec<f32> {
    let (in_ch, out_ch, k, s, p) = (up.in_ch, up.out_ch, up.k, up.stride, up.padding);
    let mut wk = vec![0f32; out_ch * 3 * in_ch];
    for kk in 0..k {
        if kk % s != r % s {
            continue; // tap belongs to another phase
        }
        // Output rows n = q*s + ((r-p) mod s) read x[(n+p-kk)/s] = x[q+off].
        let base = ((r + s) - (p % s)) % s; // (r-p) mod s
        let off = (base as isize + p as isize - kk as isize) / s as isize;
        assert!(
            (-1..=1).contains(&off) && (base as isize + p as isize - kk as isize) % s as isize == 0,
            "bigvgan convt tap off {off} out of polyphase range"
        );
        let block = (off + 1) as usize;
        for i in 0..in_ch {
            for o in 0..out_ch {
                wk[o * 3 * in_ch + block * in_ch + i] =
                    up.weight[(i * out_ch + o) * k + kk] * scale;
            }
        }
    }
    wk
}

/// Row-gather indices for one conv tap: `idx[n] = n + kk*d - p`, out-of-range
/// positions pointing at the sentinel zero row `t`.
pub(crate) fn tap_indices(t: usize, k: usize, dilation: usize, padding: usize) -> Vec<Vec<u32>> {
    (0..k)
        .map(|kk| {
            let off = (kk * dilation) as isize - padding as isize;
            (0..t as isize)
                .map(|n| {
                    let src = n + off;
                    if src < 0 || src >= t as isize {
                        t as u32
                    } else {
                        src as u32
                    }
                })
                .collect()
        })
        .collect()
}

/// Per-length device index tensors (frames are stable across warm runs, so
/// these cache; a new utterance length uploads ~25 MB of u32 maps once).
struct RunIdx {
    /// Per-conv tap gathers: conv_pre, then per stage [convt 3-offset], per
    /// block per dilation [conv1 taps], [conv2 taps].
    conv_pre: Vec<GpuTensor>,
    /// Per stage: 3-offset gather (offsets -1,0,+1) at the input length.
    convt: Vec<Vec<GpuTensor>>,
    /// Per stage: interleave gather `[s*t]` over the stacked phase outputs.
    convt_interleave: Vec<GpuTensor>,
    /// Per stage, per (kernel, dilation, padding) signature at the stage's
    /// output length — shared by every conv with the same signature.
    taps: HashMap<(usize, usize, usize, usize), Vec<GpuTensor>>,
    /// Sentinel zero rows per operand width.
    zero: HashMap<usize, GpuTensor>,
    conv_post: Vec<GpuTensor>,
}

impl RunIdx {
    fn build(model: &IndexTtsBigVgan, frames: usize) -> Result<Self> {
        let mut zero = HashMap::new();
        let ensure_zero = |zero: &mut HashMap<usize, GpuTensor>, w: usize| -> Result<()> {
            if !zero.contains_key(&w) {
                zero.insert(w, gpu_upload(&vec![0f32; w], 1, w).map_err(gm)?);
            }
            Ok(())
        };
        let upload_taps = |maps: Vec<Vec<u32>>| -> Result<Vec<GpuTensor>> {
            maps.into_iter()
                .map(|m| gpu_upload_u32(&m).map_err(gm))
                .collect()
        };

        ensure_zero(&mut zero, model.conv_pre.in_ch)?;
        let cp = &model.conv_pre;
        let conv_pre = upload_taps(tap_indices(frames, cp.k, cp.dilation, cp.padding))?;

        let mut convt = Vec::new();
        let mut convt_interleave = Vec::new();
        let mut taps: HashMap<(usize, usize, usize, usize), Vec<GpuTensor>> = HashMap::new();
        let mut t = frames;
        for stage in &model.stages {
            let s = stage.up.stride;
            ensure_zero(&mut zero, stage.up.in_ch)?;
            // Shared 3-offset gather at the input length.
            convt.push(upload_taps(tap_indices(t, 3, 1, 1))?);
            let t_out = t * s;
            let p = stage.up.padding;
            let interleave: Vec<u32> = (0..t_out)
                .map(|n| {
                    let r = (n + p) % s;
                    let q = n / s;
                    (r * t + q) as u32
                })
                .collect();
            convt_interleave.push(gpu_upload_u32(&interleave).map_err(gm)?);
            for block in &stage.blocks {
                for conv in block.convs1.iter().chain(block.convs2.iter()) {
                    ensure_zero(&mut zero, conv.in_ch)?;
                    let sig = (t_out, conv.k, conv.dilation, conv.padding);
                    if !taps.contains_key(&sig) {
                        taps.insert(
                            sig,
                            upload_taps(tap_indices(t_out, conv.k, conv.dilation, conv.padding))?,
                        );
                    }
                }
            }
            t = t_out;
        }
        ensure_zero(&mut zero, model.conv_post.in_ch)?;
        let cp = &model.conv_post;
        let conv_post = upload_taps(tap_indices(t, cp.k, cp.dilation, cp.padding))?;
        Ok(Self {
            conv_pre,
            convt,
            convt_interleave,
            taps,
            zero,
            conv_post,
        })
    }
}

/// Weight-cache handle + per-length index cache. Weights are ensured once at
/// construction; the session holds no tensor data of its own.
pub struct BigVganCudaSession {
    idx: Mutex<HashMap<usize, RunIdx>>,
}

impl BigVganCudaSession {
    pub fn new(model: &IndexTtsBigVgan) -> Result<Self> {
        ensure_weight(
            NS,
            "conv_pre",
            &pack_conv1d(&model.conv_pre),
            model.conv_pre.out_ch,
            model.conv_pre.k * model.conv_pre.in_ch,
        )?;
        let inv_mean = 1.0 / model.num_kernels as f32;
        for (i, stage) in model.stages.iter().enumerate() {
            // Stage 0 consumes conv_pre directly; later stages consume the
            // previous stage's block sum, so the 1/num_kernels mean folds
            // into their transpose-conv weights.
            let scale = if i == 0 { 1.0 } else { inv_mean };
            for r in 0..stage.up.stride {
                ensure_weight(
                    NS,
                    &format!("ups{i}.ph{r}"),
                    &pack_convt_phase(&stage.up, r, scale),
                    stage.up.out_ch,
                    3 * stage.up.in_ch,
                )?;
            }
            for (b, block) in stage.blocks.iter().enumerate() {
                for (j, conv) in block.convs1.iter().enumerate() {
                    ensure_weight(
                        NS,
                        &format!("rb{i}.{b}.c1.{j}"),
                        &pack_conv1d(conv),
                        conv.out_ch,
                        conv.k * conv.in_ch,
                    )?;
                }
                for (j, conv) in block.convs2.iter().enumerate() {
                    ensure_weight(
                        NS,
                        &format!("rb{i}.{b}.c2.{j}"),
                        &pack_conv1d(conv),
                        conv.out_ch,
                        conv.k * conv.in_ch,
                    )?;
                }
            }
        }
        ensure_weight(
            NS,
            "conv_post",
            &pack_conv1d(&model.conv_post),
            model.conv_post.out_ch,
            model.conv_post.k * model.conv_post.in_ch,
        )?;
        Ok(Self {
            idx: Mutex::new(HashMap::new()),
        })
    }

    /// Device counterpart of [`IndexTtsBigVgan::synthesize_cpu`]; same input
    /// contract (`(80, frames)` mel plane `[mel][t]`), same clamp finish.
    pub fn synthesize(
        &self,
        model: &IndexTtsBigVgan,
        mel: &[f32],
        frames: usize,
    ) -> Result<Vec<f32>> {
        if frames == 0 || mel.len() != model.conv_pre.in_ch * frames {
            return Err(DiffusionError::model(format!(
                "bigvgan cuda synthesize: expected {}*{frames} values, got {}",
                model.conv_pre.in_ch,
                mel.len()
            )));
        }
        let mut idx_cache = self.idx.lock().map_err(|_| gm("index cache poisoned".into()))?;
        if !idx_cache.contains_key(&frames) {
            idx_cache.insert(frames, RunIdx::build(model, frames)?);
        }
        let idx = idx_cache.get(&frames).expect("just inserted");
        let threads = model.threads;

        // Mel plane (80, frames) -> [t, 80] rows.
        let mel_rows = {
            let mut rows = vec![0f32; frames * model.conv_pre.in_ch];
            let ch = model.conv_pre.in_ch;
            par_rows(threads, &mut rows, ch, &|n, row| {
                for (c, slot) in row.iter_mut().enumerate() {
                    *slot = mel[c * frames + n];
                }
            });
            gpu_upload(&rows, frames, ch).map_err(gm)?
        };

        let mut hidden = conv_gemm(
            &mel_rows,
            &idx.conv_pre,
            &idx.zero[&model.conv_pre.in_ch],
            NS,
            "conv_pre",
            model.conv_pre.out_ch,
            &model.conv_pre.bias,
        )?;

        let inv_mean = 1.0 / model.num_kernels as f32;
        for (i, stage) in model.stages.iter().enumerate() {
            let up = convt_gemm(&hidden, stage, i, idx)?;
            let mut sum: Option<GpuTensor> = None;
            for (b, block) in stage.blocks.iter().enumerate() {
                let out = amp_block(&up, block, i, b, idx, threads)?;
                sum = Some(match sum {
                    None => out,
                    Some(acc) => gpu_add(&acc, &out).map_err(gm)?,
                });
            }
            hidden = sum.expect("bigvgan stage has resblocks");
        }
        // Last stage's mean lands here (host transpose scale); earlier means
        // were folded into the next stage's transpose-conv weights.
        let post = act_host(&model.activation_post, &hidden, threads, inv_mean)?;
        let out = conv_gemm(
            &post,
            &idx.conv_post,
            &idx.zero[&model.conv_post.in_ch],
            NS,
            "conv_post",
            model.conv_post.out_ch,
            &model.conv_post.bias,
        )?;

        let mut wav = gpu_download(&out).map_err(gm)?;
        debug_assert_eq!(wav.len(), frames * BIGVGAN_HOP);
        for v in wav.iter_mut() {
            *v = v.clamp(-1.0, 1.0);
        }
        Ok(wav)
    }
}

/// Zero-pad conv as tap gathers + concat + one cached GEMM.
pub(crate) fn conv_gemm(
    x: &GpuTensor,
    taps: &[GpuTensor],
    zero: &GpuTensor,
    ns: &str,
    key: &str,
    out_ch: usize,
    bias: &[f32],
) -> Result<GpuTensor> {
    let x_ext = gpu_concat_rows(x, zero).map_err(gm)?;
    let mut gathered = Vec::with_capacity(taps.len());
    for tap in taps {
        gathered.push(gpu_gather_rows_colblock(&x_ext, tap, None, x.cols()).map_err(gm)?);
    }
    let refs: Vec<&GpuTensor> = gathered.iter().collect();
    let cols = gpu_concat_cols(&refs).map_err(gm)?;
    lin(&cols, ns, key, out_ch, bias)
}

/// Polyphase transposed conv: shared 3-offset gather, per-phase GEMMs,
/// stack + interleave back to `[s*t, out_ch]`.
fn convt_gemm(
    x: &GpuTensor,
    stage: &UpStage,
    stage_index: usize,
    idx: &RunIdx,
) -> Result<GpuTensor> {
    let up = &stage.up;
    let x_ext = gpu_concat_rows(x, &idx.zero[&up.in_ch]).map_err(gm)?;
    let mut gathered = Vec::with_capacity(3);
    for tap in &idx.convt[stage_index] {
        gathered.push(gpu_gather_rows_colblock(&x_ext, tap, None, x.cols()).map_err(gm)?);
    }
    let refs: Vec<&GpuTensor> = gathered.iter().collect();
    let cols = gpu_concat_cols(&refs).map_err(gm)?;
    let mut stacked: Option<GpuTensor> = None;
    for r in 0..up.stride {
        let phase = lin(&cols, NS, &format!("ups{stage_index}.ph{r}"), up.out_ch, &up.bias)?;
        stacked = Some(match stacked {
            None => phase,
            Some(acc) => gpu_concat_rows(&acc, &phase).map_err(gm)?,
        });
    }
    let stacked = stacked.expect("stride >= 1");
    gpu_gather_rows_colblock(
        &stacked,
        &idx.convt_interleave[stage_index],
        None,
        up.out_ch,
    )
    .map_err(gm)
}

/// AMPBlock1 on device: per dilation `x += conv2(act2(conv1(act1(x))))`,
/// activations through the host.
fn amp_block(
    x: &GpuTensor,
    block: &AmpBlock,
    stage_index: usize,
    block_index: usize,
    idx: &RunIdx,
    threads: usize,
) -> Result<GpuTensor> {
    let t = x.rows();
    // The running residual: `x` itself until the first dilation's add
    // produces a fresh tensor (gpu_add allocates, so no device copy needed).
    let mut hidden: Option<GpuTensor> = None;
    for j in 0..block.convs1.len() {
        let a1 = act_host(&block.acts[2 * j], hidden.as_ref().unwrap_or(x), threads, 1.0)?;
        let c1 = &block.convs1[j];
        let s1 = conv_gemm(
            &a1,
            &idx.taps[&(t, c1.k, c1.dilation, c1.padding)],
            &idx.zero[&c1.in_ch],
            NS,
            &format!("rb{stage_index}.{block_index}.c1.{j}"),
            c1.out_ch,
            &c1.bias,
        )?;
        let a2 = act_host(&block.acts[2 * j + 1], &s1, threads, 1.0)?;
        let c2 = &block.convs2[j];
        let s2 = conv_gemm(
            &a2,
            &idx.taps[&(t, c2.k, c2.dilation, c2.padding)],
            &idx.zero[&c2.in_ch],
            NS,
            &format!("rb{stage_index}.{block_index}.c2.{j}"),
            c2.out_ch,
            &c2.bias,
        )?;
        hidden = Some(gpu_add(hidden.as_ref().unwrap_or(x), &s2).map_err(gm)?);
    }
    Ok(hidden.expect("bigvgan amp block has dilations"))
}

/// Host round-trip for one anti-aliased SnakeBeta: download `[t, ch]`,
/// transpose (applying `scale`), run the validated CPU activation, transpose
/// back, upload. Swaps for the fused device kernel when approved.
fn act_host(
    act: &AliasFreeSnake,
    x: &GpuTensor,
    threads: usize,
    scale: f32,
) -> Result<GpuTensor> {
    let (t, ch) = (x.rows(), x.cols());
    let host = gpu_download(x).map_err(gm)?;
    let mut plane = Plane {
        ch,
        len: t,
        data: vec![0f32; ch * t],
    };
    par_rows(threads, &mut plane.data, t, &|c, row| {
        for (n, slot) in row.iter_mut().enumerate() {
            *slot = host[n * ch + c] * scale;
        }
    });
    let out = act.forward(&plane, threads);
    let mut rows = vec![0f32; t * ch];
    par_rows(threads, &mut rows, ch, &|n, row| {
        for (c, slot) in row.iter_mut().enumerate() {
            *slot = out.data[c * t + n];
        }
    });
    gpu_upload(&rows, t, ch).map_err(gm)
}
