//! ACE-Step 1.5 condition encoder + XL-turbo DiT.
//!
//! Condition encoder: Linear text projector (1024→2048) + 8-layer lyric
//! encoder + 4-layer timbre encoder, packed lyrics+timbre+text for DiT
//! cross-attention. DiT: Conv1d patchify (k=2,s=2), 32 AdaLN-Zero blocks
//! with alternating sliding-128 / full GQA self-attn, SwiGLU MLP, dual
//! timestep embedding, ConvTranspose1d depatchify to 64-ch velocity.

use crate::ace::{
    ace_adaln, ace_add_inplace, ace_apply_rope_half, ace_gqa_attention, ace_gated_residual,
    ace_open_shards, ace_pack_sequences, ace_rms_head, ace_rope_tables, ace_rope_tables_bf16,
    ace_swiglu, ace_tensor_any, ace_tensor_shaped, ace_time_sinusoid, ACE_COND_DIM, ACE_COND_FFN,
    ACE_COND_HEADS, ACE_COND_KV_HEADS, ACE_CONTEXT_DIM, ACE_DIT_DIM, ACE_DIT_ENCODER_DIM,
    ACE_DIT_FFN, ACE_DIT_HEADS, ACE_DIT_HEAD_DIM, ACE_DIT_KV_HEADS, ACE_DIT_LAYERS,
    ACE_IN_CHANNELS, ACE_LATENT_DIM, ACE_LYRIC_LAYERS, ACE_PATCH_SIZE, ACE_RMS_EPS,
    ACE_SLIDING_WINDOW, ACE_TE_HIDDEN, ACE_TEXT_HIDDEN, ACE_TIME_FREQ_DIM, ACE_TIMBRE_LAYERS,
};
use crate::error::{DiffusionError, Result};
use crate::h3::H3ShardedWeights;
use crate::sa3::{linear, par_rows, rms_norm_rows, silu};
use crate::{emit_byte_progress, emit_progress, ProgressHook};
use std::cell::RefCell;
use std::io::Write;
use std::path::Path;
use std::rc::Rc;

/// Step-constant device tensors reused across the 100 DiT forwards of one
/// generate. Everything cached here is bit-identical to what the forward
/// used to rebuild per call (zero/one fills, rope tables, weight-derived
/// modulation rows), so hits cannot change results — only launch/PCIe cost.
#[derive(Default)]
struct AceFwdConsts {
    device_id: u64,
    rows: usize,
    zeros: Option<Rc<makepad_ggml::backend::cuda::GpuTensor>>,
    ones_tok: Option<Rc<makepad_ggml::backend::cuda::GpuTensor>>,
    ones_row: Option<Rc<makepad_ggml::backend::cuda::GpuTensor>>,
    idx: Option<Rc<makepad_ggml::backend::cuda::GpuTensor>>,
    rope_key: (usize, usize, bool),
    rope: Option<(
        Rc<makepad_ggml::backend::cuda::GpuTensor>,
        Rc<makepad_ggml::backend::cuda::GpuTensor>,
    )>,
    sst: Vec<Option<Rc<makepad_ggml::backend::cuda::GpuTensor>>>,
    head_sst: Option<Rc<makepad_ggml::backend::cuda::GpuTensor>>,
    w_out: Option<Rc<makepad_ggml::backend::cuda::GpuTensor>>,
    t_bits: (u32, u32),
    mods: Vec<Option<Rc<makepad_ggml::backend::cuda::GpuTensor>>>,
    temb: Option<(
        Rc<makepad_ggml::backend::cuda::GpuTensor>,
        Rc<makepad_ggml::backend::cuda::GpuTensor>,
    )>,
    head_g: Option<Rc<makepad_ggml::backend::cuda::GpuTensor>>,
}

thread_local! {
    static ACE_FWD_CONSTS: RefCell<AceFwdConsts> = RefCell::new(AceFwdConsts::default());
}

static ACE_DEVICE_IDS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Validate the cache against this forward's device/shape/timestep and drop
/// whatever no longer applies (everything on device or row-count change, the
/// timestep-derived entries on t change).
fn ace_fwd_prepare(device_id: u64, rows: usize, layers: usize, t_key: (u32, u32)) {
    ACE_FWD_CONSTS.with(|cell| {
        let mut c = cell.borrow_mut();
        if c.device_id != device_id || c.rows != rows {
            *c = AceFwdConsts::default();
            c.device_id = device_id;
            c.rows = rows;
        }
        if c.sst.len() != layers {
            c.sst = vec![None; layers];
        }
        if c.t_bits != t_key || c.mods.len() != layers {
            c.t_bits = t_key;
            c.mods = vec![None; layers];
            c.temb = None;
            c.head_g = None;
        }
    });
}

fn ace_fwd_slot<F, M>(pick: F, make: M) -> Result<Rc<makepad_ggml::backend::cuda::GpuTensor>>
where
    F: for<'a> FnOnce(
        &'a mut AceFwdConsts,
    ) -> &'a mut Option<Rc<makepad_ggml::backend::cuda::GpuTensor>>,
    M: FnOnce() -> Result<makepad_ggml::backend::cuda::GpuTensor>,
{
    ACE_FWD_CONSTS.with(|cell| {
        let mut c = cell.borrow_mut();
        let slot = pick(&mut c);
        if let Some(tensor) = slot.as_ref() {
            return Ok(tensor.clone());
        }
        let made = Rc::new(make()?);
        *slot = Some(made.clone());
        Ok(made)
    })
}

fn ace_fwd_temb<M>(
    make: M,
) -> Result<(
    Rc<makepad_ggml::backend::cuda::GpuTensor>,
    Rc<makepad_ggml::backend::cuda::GpuTensor>,
)>
where
    M: FnOnce() -> Result<(
        makepad_ggml::backend::cuda::GpuTensor,
        makepad_ggml::backend::cuda::GpuTensor,
    )>,
{
    ACE_FWD_CONSTS.with(|cell| {
        let mut c = cell.borrow_mut();
        if let Some((temb, tproj)) = c.temb.as_ref() {
            return Ok((temb.clone(), tproj.clone()));
        }
        let (temb, tproj) = make()?;
        let (temb, tproj) = (Rc::new(temb), Rc::new(tproj));
        c.temb = Some((temb.clone(), tproj.clone()));
        Ok((temb, tproj))
    })
}

fn ace_fwd_rope(
    tokens: usize,
    batch: usize,
    rope_f32: bool,
    rows: usize,
    half: usize,
) -> Result<(
    Rc<makepad_ggml::backend::cuda::GpuTensor>,
    Rc<makepad_ggml::backend::cuda::GpuTensor>,
)> {
    use crate::sa3::dev_err;
    use makepad_ggml::backend::cuda::gpu_upload;
    ACE_FWD_CONSTS.with(|cell| {
        let mut c = cell.borrow_mut();
        let key = (tokens, batch, rope_f32);
        if c.rope_key == key {
            if let Some((cos, sin)) = c.rope.as_ref() {
                return Ok((cos.clone(), sin.clone()));
            }
        }
        let (mut cos_h, mut sin_h) = if rope_f32 {
            ace_rope_tables(tokens)
        } else {
            ace_rope_tables_bf16(tokens)
        };
        if batch == 2 {
            let cos0 = cos_h.clone();
            cos_h.extend_from_slice(&cos0);
            let sin0 = sin_h.clone();
            sin_h.extend_from_slice(&sin0);
        }
        let cos = Rc::new(
            gpu_upload(&cos_h, rows, half).map_err(|e| dev_err("ace dit rope cos", e))?,
        );
        let sin = Rc::new(
            gpu_upload(&sin_h, rows, half).map_err(|e| dev_err("ace dit rope sin", e))?,
        );
        c.rope_key = key;
        c.rope = Some((cos.clone(), sin.clone()));
        Ok((cos, sin))
    })
}

fn layer_is_sliding(index: usize) -> bool {
    // Official default: sliding if (i+1) % 2 != 0 → odd 1-based → even 0-based.
    (index + 1) % 2 == 1
}

fn ace_dbg_cmp(tag: &str, gpu: &[f32], cpu: &[f32]) {
    let n = gpu.len().min(cpu.len());
    if n == 0 {
        eprintln!("cmp {tag} empty gpu={} cpu={}", gpu.len(), cpu.len());
        return;
    }
    let mut max_abs = 0f32;
    let mut max_i = 0usize;
    let mut dot = 0f64;
    let mut na = 0f64;
    let mut nb = 0f64;
    for i in 0..n {
        let d = (gpu[i] - cpu[i]).abs();
        if d > max_abs {
            max_abs = d;
            max_i = i;
        }
        dot += gpu[i] as f64 * cpu[i] as f64;
        na += (gpu[i] as f64) * (gpu[i] as f64);
        nb += (cpu[i] as f64) * (cpu[i] as f64);
    }
    let cos = dot / (na.sqrt() * nb.sqrt() + 1e-30);
    eprintln!(
        "cmp {tag} n={n} cos={cos:.6} max_abs={max_abs:.4e} @{max_i} gpu[:4]={:?} cpu[:4]={:?}",
        &gpu[..4.min(n)],
        &cpu[..4.min(n)]
    );
}

struct EncLayer {
    input_ln: Vec<f32>,
    q_proj: Vec<f32>,
    k_proj: Vec<f32>,
    v_proj: Vec<f32>,
    o_proj: Vec<f32>,
    q_norm: Vec<f32>,
    k_norm: Vec<f32>,
    post_ln: Vec<f32>,
    gate: Vec<f32>,
    up: Vec<f32>,
    down: Vec<f32>,
    sliding: bool,
}

struct TimeEmbed {
    linear1_w: Vec<f32>,
    linear1_b: Vec<f32>,
    linear2_w: Vec<f32>,
    linear2_b: Vec<f32>,
    time_proj_w: Vec<f32>,
    time_proj_b: Vec<f32>,
}

struct DitBlock {
    self_norm: Vec<f32>,
    self_q: Vec<f32>,
    self_k: Vec<f32>,
    self_v: Vec<f32>,
    self_o: Vec<f32>,
    self_qn: Vec<f32>,
    self_kn: Vec<f32>,
    cross_norm: Vec<f32>,
    cross_q: Vec<f32>,
    cross_k: Vec<f32>,
    cross_v: Vec<f32>,
    cross_o: Vec<f32>,
    cross_qn: Vec<f32>,
    cross_kn: Vec<f32>,
    mlp_norm: Vec<f32>,
    gate: Vec<f32>,
    up: Vec<f32>,
    down: Vec<f32>,
    scale_shift: Vec<f32>, // (6, dim)
    sliding: bool,
}

pub struct AceConditionEncoder {
    text_proj: Vec<f32>,
    lyric_embed: Vec<f32>,
    lyric_embed_b: Option<Vec<f32>>,
    lyric_norm: Vec<f32>,
    lyric_layers: Vec<EncLayer>,
    timbre_embed: Vec<f32>,
    timbre_embed_b: Option<Vec<f32>>,
    timbre_norm: Vec<f32>,
    timbre_layers: Vec<EncLayer>,
    pub silence_latent: Vec<f32>,
    pub silence_frames: usize,
    pub null_condition_emb: Option<Vec<f32>>,
    rope_cos: Vec<f32>,
    rope_sin: Vec<f32>,
}

pub struct AceDit {
    proj_in_w: Vec<f32>, // (hidden, in_ch, k)
    proj_in_b: Vec<f32>,
    time: TimeEmbed,
    time_r: TimeEmbed,
    cond_embed_w: Vec<f32>,
    cond_embed_b: Vec<f32>,
    blocks: Vec<DitBlock>,
    norm_out: Vec<f32>,
    proj_out_w: Vec<f32>, // ConvTranspose (hidden, latent, k)
    proj_out_b: Vec<f32>,
    scale_shift: Vec<f32>, // (2, dim)
    rope_cos: Vec<f32>,
    rope_sin: Vec<f32>,
}

#[derive(Clone)]
pub struct AceConditioning {
    pub encoder_hidden: Vec<f32>,
    pub encoder_mask: Vec<bool>,
    pub tokens: usize,
}

fn load_enc_layer(weights: &H3ShardedWeights, prefix: &str, sliding: bool) -> Result<EncLayer> {
    let h = ACE_COND_DIM;
    let q = ACE_COND_HEADS * ACE_DIT_HEAD_DIM;
    let kv = ACE_COND_KV_HEADS * ACE_DIT_HEAD_DIM;
    let attn = format!("{prefix}.self_attn");
    Ok(EncLayer {
        input_ln: ace_tensor_shaped(weights, &[&format!("{prefix}.input_layernorm.weight")], h)?,
        q_proj: ace_tensor_shaped(
            weights,
            &[
                &format!("{attn}.to_q.weight"),
                &format!("{attn}.q_proj.weight"),
            ],
            q * h,
        )?,
        k_proj: ace_tensor_shaped(
            weights,
            &[
                &format!("{attn}.to_k.weight"),
                &format!("{attn}.k_proj.weight"),
            ],
            kv * h,
        )?,
        v_proj: ace_tensor_shaped(
            weights,
            &[
                &format!("{attn}.to_v.weight"),
                &format!("{attn}.v_proj.weight"),
            ],
            kv * h,
        )?,
        o_proj: ace_tensor_shaped(
            weights,
            &[
                &format!("{attn}.to_out.0.weight"),
                &format!("{attn}.o_proj.weight"),
            ],
            h * q,
        )?,
        q_norm: ace_tensor_shaped(
            weights,
            &[
                &format!("{attn}.norm_q.weight"),
                &format!("{attn}.q_norm.weight"),
            ],
            ACE_DIT_HEAD_DIM,
        )?,
        k_norm: ace_tensor_shaped(
            weights,
            &[
                &format!("{attn}.norm_k.weight"),
                &format!("{attn}.k_norm.weight"),
            ],
            ACE_DIT_HEAD_DIM,
        )?,
        post_ln: ace_tensor_shaped(
            weights,
            &[&format!("{prefix}.post_attention_layernorm.weight")],
            h,
        )?,
        gate: ace_tensor_shaped(weights, &[&format!("{prefix}.mlp.gate_proj.weight")], ACE_COND_FFN * h)?,
        up: ace_tensor_shaped(weights, &[&format!("{prefix}.mlp.up_proj.weight")], ACE_COND_FFN * h)?,
        down: ace_tensor_shaped(weights, &[&format!("{prefix}.mlp.down_proj.weight")], h * ACE_COND_FFN)?,
        sliding,
    })
}

fn cond_gqa_attn(
    q: &[f32],
    k: &[f32],
    v: &[f32],
    q_heads: usize,
    kv_heads: usize,
    hd: usize,
    seq: usize,
    window: Option<usize>,
    mask: Option<&[bool]>,
) -> Vec<f32> {
    let use_gpu = mask.is_none()
        && seq >= 64
        && crate::ace::ace_device_enabled()
        && std::env::var("MAKEPAD_DIFFUSION_CPU").ok().as_deref() != Some("1");
    if use_gpu {
        if let Ok(attn) = cond_gqa_attn_dev(q, k, v, q_heads, kv_heads, hd, seq, window) {
            return attn;
        }
    }
    ace_gqa_attention(
        q, k, v, q_heads, kv_heads, hd, seq, seq, window, false, mask,
    )
}

fn cond_gqa_attn_dev(
    q: &[f32],
    k: &[f32],
    v: &[f32],
    q_heads: usize,
    kv_heads: usize,
    hd: usize,
    seq: usize,
    window: Option<usize>,
) -> Result<Vec<f32>> {
    use crate::sa3::dev_err;
    use makepad_ggml::backend::cuda::{
        gpu_attention_packed, gpu_download, gpu_gather_cols, gpu_rms_norm_mul, gpu_rope_half,
        gpu_upload,
    };
    let qdim = q_heads * hd;
    let kvdim = kv_heads * hd;
    let group = q_heads / kv_heads.max(1);
    let scale = 1.0 / (hd as f32).sqrt();
    let qg = gpu_upload(q, seq, qdim).map_err(|e| dev_err("ace cond q", e))?;
    let kg = gpu_upload(k, seq, kvdim).map_err(|e| dev_err("ace cond k", e))?;
    let vg = gpu_upload(v, seq, kvdim).map_err(|e| dev_err("ace cond v", e))?;
    let (kg, vg) = expand_gqa_dev(kg, vg, q_heads, kv_heads, hd, group)?;
    let _ = (window, gpu_rms_norm_mul, gpu_rope_half);
    let attn = gpu_attention_packed(&qg, &kg, &vg, q_heads, scale)
        .map_err(|e| dev_err("ace cond attn", e))?;
    gpu_download(&attn).map_err(|e| dev_err("ace cond down", e))
}

fn run_enc_layer(
    layer: &EncLayer,
    x: &mut [f32],
    seq: usize,
    dim: usize,
    rope_cos: &[f32],
    rope_sin: &[f32],
    mask: Option<&[bool]>,
) {
    let q_heads = ACE_COND_HEADS;
    let kv_heads = ACE_COND_KV_HEADS;
    let hd = ACE_DIT_HEAD_DIM;
    let mut normed = x.to_vec();
    rms_norm_rows(&mut normed, &layer.input_ln, dim, ACE_RMS_EPS);
    let mut q = linear(&normed, &layer.q_proj, None, seq, dim, q_heads * hd);
    let mut k = linear(&normed, &layer.k_proj, None, seq, dim, kv_heads * hd);
    let v = linear(&normed, &layer.v_proj, None, seq, dim, kv_heads * hd);
    for row in 0..seq {
        let qrow = &mut q[row * q_heads * hd..(row + 1) * q_heads * hd];
        for head in 0..q_heads {
            ace_rms_head(&mut qrow[head * hd..(head + 1) * hd], &layer.q_norm);
        }
        ace_apply_rope_half(qrow, row, q_heads, rope_cos, rope_sin);
        let krow = &mut k[row * kv_heads * hd..(row + 1) * kv_heads * hd];
        for head in 0..kv_heads {
            ace_rms_head(&mut krow[head * hd..(head + 1) * hd], &layer.k_norm);
        }
        ace_apply_rope_half(krow, row, kv_heads, rope_cos, rope_sin);
    }
    let window = if layer.sliding {
        Some(ACE_SLIDING_WINDOW)
    } else {
        None
    };
    let attn = cond_gqa_attn(&q, &k, &v, q_heads, kv_heads, hd, seq, window, mask);
    let proj = linear(&attn, &layer.o_proj, None, seq, q_heads * hd, dim);
    ace_add_inplace(x, &proj);

    let mut normed = x.to_vec();
    rms_norm_rows(&mut normed, &layer.post_ln, dim, ACE_RMS_EPS);
    let gate = linear(&normed, &layer.gate, None, seq, dim, ACE_COND_FFN);
    let up = linear(&normed, &layer.up, None, seq, dim, ACE_COND_FFN);
    let act = ace_swiglu(&gate, &up);
    let down = linear(&act, &layer.down, None, seq, ACE_COND_FFN, dim);
    ace_add_inplace(x, &down);
}

impl AceConditionEncoder {
    pub fn load(dir: impl AsRef<Path>) -> Result<Self> {
        Self::load_with_progress(dir, None)
    }

    pub fn load_with_progress(
        dir: impl AsRef<Path>,
        mut progress: Option<ProgressHook>,
    ) -> Result<Self> {
        let weights = ace_open_shards(dir)?;
        emit_progress(&mut progress, "load ace cond", 0.0)?;
        let h = ACE_COND_DIM;
        let text_proj = ace_tensor_shaped(
            &weights,
            &["text_projector.weight"],
            h * ACE_TEXT_HIDDEN,
        )?;
        let lyric_embed = ace_tensor_shaped(
            &weights,
            &["lyric_encoder.embed_tokens.weight"],
            h * ACE_TEXT_HIDDEN,
        )?;
        let lyric_embed_b = if weights.has_tensor("lyric_encoder.embed_tokens.bias") {
            Some(ace_tensor_shaped(
                &weights,
                &["lyric_encoder.embed_tokens.bias"],
                h,
            )?)
        } else {
            None
        };
        let lyric_norm = ace_tensor_shaped(&weights, &["lyric_encoder.norm.weight"], h)?;
        let mut lyric_layers = Vec::with_capacity(ACE_LYRIC_LAYERS);
        for i in 0..ACE_LYRIC_LAYERS {
            lyric_layers.push(load_enc_layer(
                &weights,
                &format!("lyric_encoder.layers.{i}"),
                layer_is_sliding(i),
            )?);
        }
        emit_progress(&mut progress, "load ace cond lyric", 0.4)?;
        let timbre_embed = ace_tensor_any(
            &weights,
            &[
                "timbre_encoder.embed_tokens.weight",
                "timbre_encoder.embed_tokens.weight",
            ],
        )?;
        if timbre_embed.len() != h * ACE_LATENT_DIM {
            return Err(DiffusionError::model(format!(
                "ace timbre embed {} values, expected {}",
                timbre_embed.len(),
                h * ACE_LATENT_DIM
            )));
        }
        let timbre_embed_b = if weights.has_tensor("timbre_encoder.embed_tokens.bias") {
            Some(ace_tensor_shaped(
                &weights,
                &["timbre_encoder.embed_tokens.bias"],
                h,
            )?)
        } else {
            None
        };
        let timbre_norm = ace_tensor_shaped(&weights, &["timbre_encoder.norm.weight"], h)?;
        let mut timbre_layers = Vec::with_capacity(ACE_TIMBRE_LAYERS);
        for i in 0..ACE_TIMBRE_LAYERS {
            timbre_layers.push(load_enc_layer(
                &weights,
                &format!("timbre_encoder.layers.{i}"),
                layer_is_sliding(i),
            )?);
        }
        let silence_latent = ace_tensor_any(&weights, &["silence_latent"])?;
        if silence_latent.len() % ACE_LATENT_DIM != 0 {
            return Err(DiffusionError::model(format!(
                "ace silence_latent {} not divisible by {}",
                silence_latent.len(),
                ACE_LATENT_DIM
            )));
        }
        let silence_frames = silence_latent.len() / ACE_LATENT_DIM;
        let null_condition_emb = if weights.has_tensor("null_condition_emb") {
            Some(ace_tensor_any(&weights, &["null_condition_emb"])?)
        } else {
            None
        };
        let (rope_cos, rope_sin) = ace_rope_tables(16_384);
        emit_progress(&mut progress, "load ace cond", 1.0)?;
        Ok(Self {
            text_proj,
            lyric_embed,
            lyric_embed_b,
            lyric_norm,
            lyric_layers,
            timbre_embed,
            timbre_embed_b,
            timbre_norm,
            timbre_layers,
            silence_latent,
            silence_frames,
            null_condition_emb,
            rope_cos,
            rope_sin,
        })
    }

    pub fn silence_slice(&self, frames: usize) -> Vec<f32> {
        let take = frames.min(self.silence_frames);
        let mut out = vec![0f32; frames * ACE_LATENT_DIM];
        if take > 0 {
            out[..take * ACE_LATENT_DIM]
                .copy_from_slice(&self.silence_latent[..take * ACE_LATENT_DIM]);
        }
        if frames > take && take > 0 {
            for i in take..frames {
                let src = i % take;
                out[i * ACE_LATENT_DIM..(i + 1) * ACE_LATENT_DIM].copy_from_slice(
                    &self.silence_latent[src * ACE_LATENT_DIM..(src + 1) * ACE_LATENT_DIM],
                );
            }
        }
        out
    }

    pub fn encode(
        &self,
        text_hidden: &[f32],
        text_len: usize,
        lyric_embed: &[f32],
        lyric_len: usize,
        refer_audio: &[f32],
        refer_frames: usize,
    ) -> Result<AceConditioning> {
        let h = ACE_COND_DIM;
        let text = linear(
            text_hidden,
            &self.text_proj,
            None,
            text_len,
            ACE_TE_HIDDEN,
            h,
        );
        let text_mask = vec![true; text_len];

        let mut lyrics = linear(
            lyric_embed,
            &self.lyric_embed,
            self.lyric_embed_b.as_deref(),
            lyric_len,
            ACE_TEXT_HIDDEN,
            h,
        );
        let lyric_mask = vec![true; lyric_len];
        for layer in &self.lyric_layers {
            run_enc_layer(
                layer,
                &mut lyrics,
                lyric_len,
                h,
                &self.rope_cos,
                &self.rope_sin,
                Some(&lyric_mask),
            );
        }
        rms_norm_rows(&mut lyrics, &self.lyric_norm, h, ACE_RMS_EPS);

        let mut timbre = linear(
            refer_audio,
            &self.timbre_embed,
            self.timbre_embed_b.as_deref(),
            refer_frames,
            ACE_LATENT_DIM,
            h,
        );
        for layer in &self.timbre_layers {
            run_enc_layer(
                layer,
                &mut timbre,
                refer_frames,
                h,
                &self.rope_cos,
                &self.rope_sin,
                None,
            );
        }
        rms_norm_rows(&mut timbre, &self.timbre_norm, h, ACE_RMS_EPS);
        // Official: first token after the encoder (special_token is unused).
        let timbre_tok = timbre[..h].to_vec();
        let timbre_mask = vec![true];

        let (packed, mask) =
            ace_pack_sequences(&lyrics, &lyric_mask, &timbre_tok, &timbre_mask, h)?;
        let (packed, mask) = ace_pack_sequences(&packed, &mask, &text, &text_mask, h)?;
        let tokens = packed.len() / h;
        Ok(AceConditioning {
            encoder_hidden: packed,
            encoder_mask: mask,
            tokens,
        })
    }

    pub fn prepare_device(&self) -> AceCondDevice {
        let h = ACE_COND_DIM;
        let q = ACE_COND_HEADS * ACE_DIT_HEAD_DIM;
        let kv = ACE_COND_KV_HEADS * ACE_DIT_HEAD_DIM;
        let pack = |prefix: &str, layers: &[EncLayer]| {
            layers
                .iter()
                .enumerate()
                .map(|(i, layer)| EncDevLayer {
                    q: Bf16Weight::new(format!("{prefix}.{i}.q"), &layer.q_proj, q, h),
                    k: Bf16Weight::new(format!("{prefix}.{i}.k"), &layer.k_proj, kv, h),
                    v: Bf16Weight::new(format!("{prefix}.{i}.v"), &layer.v_proj, kv, h),
                    o: Bf16Weight::new(format!("{prefix}.{i}.o"), &layer.o_proj, h, q),
                    gate: Bf16Weight::new(format!("{prefix}.{i}.gate"), &layer.gate, ACE_COND_FFN, h),
                    up: Bf16Weight::new(format!("{prefix}.{i}.up"), &layer.up, ACE_COND_FFN, h),
                    down: Bf16Weight::new(format!("{prefix}.{i}.dn"), &layer.down, h, ACE_COND_FFN),
                    sliding: layer.sliding,
                })
                .collect()
        };
        AceCondDevice {
            text_proj: Bf16Weight::new("acecond.tp", &self.text_proj, h, ACE_TE_HIDDEN),
            lyric_embed: Bf16Weight::new("acecond.ly", &self.lyric_embed, h, ACE_TEXT_HIDDEN),
            lyric_embed_b: self.lyric_embed_b.clone().unwrap_or_else(|| vec![0f32; h]),
            timbre_embed: Bf16Weight::new("acecond.tb", &self.timbre_embed, h, ACE_LATENT_DIM),
            timbre_embed_b: self.timbre_embed_b.clone().unwrap_or_else(|| vec![0f32; h]),
            lyric_layers: pack("acecond.lyr", &self.lyric_layers),
            timbre_layers: pack("acecond.tbr", &self.timbre_layers),
        }
    }

    pub fn encode_device(
        &self,
        device: &AceCondDevice,
        text_hidden: &[f32],
        text_len: usize,
        lyric_embed: &[f32],
        lyric_len: usize,
        refer_audio: &[f32],
        refer_frames: usize,
    ) -> Result<AceConditioning> {
        use crate::sa3::dev_err;
        use makepad_ggml::backend::cuda::{
            gpu_add, gpu_attention_packed_composite_bf16, gpu_attention_packed_sliding_bf16,
            gpu_bf16_round, gpu_download, gpu_linear_nt_cached_bf16_bias_epilogue,
            gpu_linear_nt_cached_bf16_mm, gpu_mul, gpu_rope_half, gpu_silu, gpu_upload,
        };
        let h = ACE_COND_DIM;
        let q_heads = ACE_COND_HEADS;
        let kv_heads = ACE_COND_KV_HEADS;
        let hd = ACE_DIT_HEAD_DIM;
        let qdim = q_heads * hd;
        let group = q_heads / kv_heads.max(1);
        let scale = 1.0 / (hd as f32).sqrt();
        let half = hd / 2;
        let lin = |x: &makepad_ggml::backend::cuda::GpuTensor, w: &Bf16Weight| {
            gpu_linear_nt_cached_bf16_mm(x, "acecond", &[w.part()])
        };
        let lin_b = |x: &makepad_ggml::backend::cuda::GpuTensor, w: &Bf16Weight, bias: &[f32]| {
            gpu_linear_nt_cached_bf16_bias_epilogue(x, "acecond", &[w.part()], bias)
        };

        let text_up = gpu_upload(text_hidden, text_len, ACE_TE_HIDDEN)
            .map_err(|e| dev_err("ace cond text up", e))?;
        let text = lin(&text_up, &device.text_proj).map_err(|e| dev_err("ace cond text proj", e))?;

        let ly_up = gpu_upload(lyric_embed, lyric_len, ACE_TEXT_HIDDEN)
            .map_err(|e| dev_err("ace cond ly up", e))?;
        let mut lyrics = lin_b(&ly_up, &device.lyric_embed, &device.lyric_embed_b)
            .map_err(|e| dev_err("ace cond ly emb", e))?;
        let (ly_cos_h, ly_sin_h) = ace_rope_tables_bf16(lyric_len);
        let ly_cos = gpu_upload(&ly_cos_h, lyric_len, half).map_err(|e| dev_err("ace cond ly cos", e))?;
        let ly_sin = gpu_upload(&ly_sin_h, lyric_len, half).map_err(|e| dev_err("ace cond ly sin", e))?;
        for (i, (layer, devb)) in self.lyric_layers.iter().zip(&device.lyric_layers).enumerate() {
            lyrics = cond_enc_layer_dev(
                &lyrics, layer, devb, lyric_len, h, q_heads, kv_heads, hd, qdim, group, scale, half,
                &ly_cos, &ly_sin, i, lin,
            )?;
        }
        lyrics = ace_rms_two_store(&lyrics, &self.lyric_norm, h, "acecond.ly.norm", ACE_RMS_EPS)?;

        let tb_up = gpu_upload(refer_audio, refer_frames, ACE_LATENT_DIM)
            .map_err(|e| dev_err("ace cond tb up", e))?;
        let mut timbre = lin_b(&tb_up, &device.timbre_embed, &device.timbre_embed_b)
            .map_err(|e| dev_err("ace cond tb emb", e))?;
        let (tb_cos_h, tb_sin_h) = ace_rope_tables_bf16(refer_frames);
        let tb_cos = gpu_upload(&tb_cos_h, refer_frames, half).map_err(|e| dev_err("ace cond tb cos", e))?;
        let tb_sin = gpu_upload(&tb_sin_h, refer_frames, half).map_err(|e| dev_err("ace cond tb sin", e))?;
        for (i, (layer, devb)) in self.timbre_layers.iter().zip(&device.timbre_layers).enumerate() {
            timbre = cond_enc_layer_dev(
                &timbre, layer, devb, refer_frames, h, q_heads, kv_heads, hd, qdim, group, scale,
                half, &tb_cos, &tb_sin, i + 100, lin,
            )?;
        }
        timbre = ace_rms_two_store(&timbre, &self.timbre_norm, h, "acecond.tb.norm", ACE_RMS_EPS)?;

        let text_h = gpu_download(&text).map_err(|e| dev_err("ace cond text dl", e))?;
        let lyrics_h = gpu_download(&lyrics).map_err(|e| dev_err("ace cond ly dl", e))?;
        let timbre_h = gpu_download(&timbre).map_err(|e| dev_err("ace cond tb dl", e))?;
        let timbre_tok = timbre_h[..h].to_vec();
        let text_mask = vec![true; text_len];
        let lyric_mask = vec![true; lyric_len];
        let timbre_mask = vec![true];
        let (packed, mask) =
            ace_pack_sequences(&lyrics_h, &lyric_mask, &timbre_tok, &timbre_mask, h)?;
        let (packed, mask) = ace_pack_sequences(&packed, &mask, &text_h, &text_mask, h)?;
        let tokens = packed.len() / h;
        Ok(AceConditioning {
            encoder_hidden: packed,
            encoder_mask: mask,
            tokens,
        })
    }
}

fn cond_enc_layer_dev(
    x: &makepad_ggml::backend::cuda::GpuTensor,
    layer: &EncLayer,
    devb: &EncDevLayer,
    seq: usize,
    dim: usize,
    q_heads: usize,
    kv_heads: usize,
    hd: usize,
    qdim: usize,
    group: usize,
    scale: f32,
    half: usize,
    rope_cos: &makepad_ggml::backend::cuda::GpuTensor,
    rope_sin: &makepad_ggml::backend::cuda::GpuTensor,
    idx: usize,
    lin: impl Fn(
        &makepad_ggml::backend::cuda::GpuTensor,
        &Bf16Weight,
    ) -> std::result::Result<makepad_ggml::backend::cuda::GpuTensor, String>,
) -> Result<makepad_ggml::backend::cuda::GpuTensor> {
    use crate::sa3::dev_err;
    use makepad_ggml::backend::cuda::{
        gpu_add, gpu_attention_packed_composite_bf16, gpu_attention_packed_sliding_bf16,
        gpu_bf16_round, gpu_mul, gpu_rope_half, gpu_silu,
    };
    let _ = (seq, qdim);
    let normed = ace_rms_two_store(x, &layer.input_ln, dim, &format!("acecond.l{idx}.in"), ACE_RMS_EPS)?;
    let q = lin(&normed, &devb.q).map_err(|e| dev_err("ace cond q", e))?;
    let k = lin(&normed, &devb.k).map_err(|e| dev_err("ace cond k", e))?;
    let v = lin(&normed, &devb.v).map_err(|e| dev_err("ace cond v", e))?;
    let q = ace_rms_two_store(&q, &layer.q_norm, hd, &format!("acecond.l{idx}.qn"), ACE_RMS_EPS)?;
    let k = ace_rms_two_store(&k, &layer.k_norm, hd, &format!("acecond.l{idx}.kn"), ACE_RMS_EPS)?;
    let q = gpu_rope_half(&q, q_heads, half, rope_cos, rope_sin).map_err(|e| dev_err("ace cond rq", e))?;
    let k = gpu_rope_half(&k, kv_heads, half, rope_cos, rope_sin).map_err(|e| dev_err("ace cond rk", e))?;
    let q = gpu_bf16_round(&q).map_err(|e| dev_err("ace cond rq rnd", e))?;
    let k = gpu_bf16_round(&k).map_err(|e| dev_err("ace cond rk rnd", e))?;
    let (k, v) = expand_gqa_dev(k, v, q_heads, kv_heads, hd, group)?;
    let attn = if devb.sliding {
        gpu_attention_packed_sliding_bf16(&q, &k, &v, q_heads, scale, ACE_SLIDING_WINDOW)
            .map_err(|e| dev_err("ace cond slide", e))?
    } else {
        gpu_attention_packed_composite_bf16(&q, &k, &v, q_heads, scale)
            .map_err(|e| dev_err("ace cond full", e))?
    };
    let attn = gpu_bf16_round(&attn).map_err(|e| dev_err("ace cond attn rnd", e))?;
    let proj = lin(&attn, &devb.o).map_err(|e| dev_err("ace cond o", e))?;
    let mut h = gpu_add(x, &proj).map_err(|e| dev_err("ace cond attn add", e))?;
    h = gpu_bf16_round(&h).map_err(|e| dev_err("ace cond attn add rnd", e))?;

    let fnorm = ace_rms_two_store(&h, &layer.post_ln, dim, &format!("acecond.l{idx}.pn"), ACE_RMS_EPS)?;
    let gate = lin(&fnorm, &devb.gate).map_err(|e| dev_err("ace cond gate", e))?;
    let up = lin(&fnorm, &devb.up).map_err(|e| dev_err("ace cond up", e))?;
    let act = gpu_silu(&gate).map_err(|e| dev_err("ace cond silu", e))?;
    let act = gpu_bf16_round(&act).map_err(|e| dev_err("ace cond silu rnd", e))?;
    let act = gpu_mul(&act, &up).map_err(|e| dev_err("ace cond swiglu", e))?;
    let act = gpu_bf16_round(&act).map_err(|e| dev_err("ace cond swiglu rnd", e))?;
    let down = lin(&act, &devb.down).map_err(|e| dev_err("ace cond down", e))?;
    h = gpu_add(&h, &down).map_err(|e| dev_err("ace cond mlp add", e))?;
    gpu_bf16_round(&h).map_err(|e| dev_err("ace cond mlp add rnd", e))
}

fn load_time_embed(weights: &H3ShardedWeights, prefix: &str, dim: usize) -> Result<TimeEmbed> {
    Ok(TimeEmbed {
        linear1_w: ace_tensor_shaped(
            weights,
            &[&format!("{prefix}.linear_1.weight")],
            dim * ACE_TIME_FREQ_DIM,
        )?,
        linear1_b: ace_tensor_shaped(weights, &[&format!("{prefix}.linear_1.bias")], dim)?,
        linear2_w: ace_tensor_shaped(weights, &[&format!("{prefix}.linear_2.weight")], dim * dim)?,
        linear2_b: ace_tensor_shaped(weights, &[&format!("{prefix}.linear_2.bias")], dim)?,
        time_proj_w: ace_tensor_shaped(
            weights,
            &[&format!("{prefix}.time_proj.weight")],
            6 * dim * dim,
        )?,
        time_proj_b: ace_tensor_shaped(weights, &[&format!("{prefix}.time_proj.bias")], 6 * dim)?,
    })
}

fn apply_time_embed(embed: &TimeEmbed, t: f32, dim: usize) -> (Vec<f32>, Vec<f32>) {
    let freq = ace_time_sinusoid(t);
    let mut temb = linear(
        &freq,
        &embed.linear1_w,
        Some(&embed.linear1_b),
        1,
        ACE_TIME_FREQ_DIM,
        dim,
    );
    for v in temb.iter_mut() {
        *v = silu(*v);
    }
    let temb = linear(&temb, &embed.linear2_w, Some(&embed.linear2_b), 1, dim, dim);
    let mut act = temb.clone();
    for v in act.iter_mut() {
        *v = silu(*v);
    }
    let proj = linear(
        &act,
        &embed.time_proj_w,
        Some(&embed.time_proj_b),
        1,
        dim,
        6 * dim,
    );
    (temb, proj)
}

fn apply_time_embed_dev(
    embed: &TimeEmbed,
    dev: &TimeEmbedDev,
    t: f32,
) -> Result<(
    makepad_ggml::backend::cuda::GpuTensor,
    makepad_ggml::backend::cuda::GpuTensor,
)> {
    use crate::sa3::dev_err;
    use makepad_ggml::backend::cuda::{
        gpu_bf16_round, gpu_linear_nt_cached_bf16_bias_epilogue, gpu_silu, gpu_upload,
    };
    let freq = ace_time_sinusoid(t);
    let freq = gpu_upload(&freq, 1, ACE_TIME_FREQ_DIM).map_err(|e| dev_err("ace time freq", e))?;
    let freq = gpu_bf16_round(&freq).map_err(|e| dev_err("ace time freq rnd", e))?;
    let temb = gpu_linear_nt_cached_bf16_bias_epilogue(
        &freq,
        "acedit",
        &[dev.linear1.part()],
        &embed.linear1_b,
    )
    .map_err(|e| dev_err("ace time l1", e))?;
    let act1 = gpu_silu(&temb).map_err(|e| dev_err("ace time silu1", e))?;
    let act1 = gpu_bf16_round(&act1).map_err(|e| dev_err("ace time silu1 rnd", e))?;
    let temb = gpu_linear_nt_cached_bf16_bias_epilogue(
        &act1,
        "acedit",
        &[dev.linear2.part()],
        &embed.linear2_b,
    )
    .map_err(|e| dev_err("ace time l2", e))?;
    let act2 = gpu_silu(&temb).map_err(|e| dev_err("ace time silu2", e))?;
    let act2 = gpu_bf16_round(&act2).map_err(|e| dev_err("ace time silu2 rnd", e))?;
    let proj = gpu_linear_nt_cached_bf16_bias_epilogue(
        &act2,
        "acedit",
        &[dev.time_proj.part()],
        &embed.time_proj_b,
    )
    .map_err(|e| dev_err("ace time proj", e))?;
    Ok((temb, proj))
}

fn hook_dump_dir() -> Option<std::path::PathBuf> {
    std::env::var("ACE_HOOK_DUMP").ok().filter(|s| !s.is_empty()).map(std::path::PathBuf::from)
}

fn hook_dump_take() -> Option<std::path::PathBuf> {
    use std::sync::atomic::{AtomicBool, Ordering};
    static DUMPED: AtomicBool = AtomicBool::new(false);
    let dir = hook_dump_dir()?;
    if DUMPED.swap(true, Ordering::SeqCst) {
        None
    } else {
        Some(dir)
    }
}

fn write_npy_f32(path: &str, data: &[f32], shape: &[usize]) -> std::io::Result<()> {
    let shape_txt = if shape.len() == 1 {
        format!("({},)", shape[0])
    } else {
        format!(
            "({})",
            shape
                .iter()
                .map(|d| d.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let mut dict = format!("{{'descr': '<f4', 'fortran_order': False, 'shape': {shape_txt}, }}");
    let prefix = 10usize;
    let mut header_len = dict.len() + 1;
    let pad = (16 - ((prefix + header_len) % 16)) % 16;
    header_len += pad;
    dict.push_str(&" ".repeat(pad));
    dict.push('\n');
    let mut f = std::fs::File::create(path)?;
    f.write_all(b"\x93NUMPY\x01\x00")?;
    f.write_all(&(header_len as u16).to_le_bytes())?;
    f.write_all(dict.as_bytes())?;
    for v in data {
        f.write_all(&v.to_le_bytes())?;
    }
    Ok(())
}

fn dump_hook_npy(dir: &std::path::Path, name: &str, data: &[f32], shape: &[usize]) {
    let path = dir.join(name);
    match path.to_str() {
        Some(p) => {
            if let Err(err) = write_npy_f32(p, data, shape) {
                eprintln!("ACE_HOOK_DUMP {name}: {err}");
            }
        }
        None => eprintln!("ACE_HOOK_DUMP {name}: non-utf8 path"),
    }
}

fn pin_proj_in_path() -> Option<std::path::PathBuf> {
    std::env::var("ACE_PIN_PROJ_IN")
        .ok()
        .filter(|s| !s.is_empty())
        .map(std::path::PathBuf::from)
}

fn pin_q_rms_path() -> Option<std::path::PathBuf> {
    std::env::var("ACE_PIN_Q_RMS")
        .ok()
        .filter(|s| !s.is_empty())
        .map(std::path::PathBuf::from)
}

fn pin_self_attn_path() -> Option<std::path::PathBuf> {
    std::env::var("ACE_PIN_SELF_ATTN")
        .ok()
        .filter(|s| !s.is_empty())
        .map(std::path::PathBuf::from)
}

fn pin_cross_attn_path() -> Option<std::path::PathBuf> {
    std::env::var("ACE_PIN_CROSS_ATTN")
        .ok()
        .filter(|s| !s.is_empty())
        .map(std::path::PathBuf::from)
}

/// Fused elementwise chains (bit-identical single kernels) are the default;
/// `ACE_FUSE_OFF=1` restores every original multi-kernel chain, and the
/// per-site `which` flag (e.g. `ACE_FUSE_NO_RMS=1`) restores just one for
/// bisection.
fn ace_fuse_on(which: &str) -> bool {
    !ace_env_flag("ACE_FUSE_OFF") && !ace_env_flag(which)
}

fn ace_env_flag(name: &str) -> bool {
    std::env::var(name).ok().as_deref() == Some("1")
}

fn load_npy_f32(path: &Path) -> Result<Vec<f32>> {
    let bytes = std::fs::read(path).map_err(|err| {
        DiffusionError::model(format!("npy {}: {err}", path.display()))
    })?;
    if bytes.len() < 10 || &bytes[..6] != b"\x93NUMPY" {
        return Err(DiffusionError::model(format!("{}: not npy", path.display())));
    }
    let major = bytes[6];
    let (header_len, header_start) = if major == 1 {
        (u16::from_le_bytes([bytes[8], bytes[9]]) as usize, 10usize)
    } else {
        (
            u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize,
            12usize,
        )
    };
    let header = String::from_utf8_lossy(&bytes[header_start..header_start + header_len]).to_string();
    let descr = header
        .split("'descr':")
        .nth(1)
        .and_then(|rest| rest.split('\'').nth(1))
        .unwrap_or("");
    let payload = &bytes[header_start + header_len..];
    let raw = match descr {
        "<f4" => payload
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect::<Vec<_>>(),
        "<f8" => payload
            .chunks_exact(8)
            .map(|c| f64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]) as f32)
            .collect::<Vec<_>>(),
        other => {
            return Err(DiffusionError::model(format!(
                "{}: descr {other} not f32",
                path.display()
            )))
        }
    };
    Ok(raw)
}

fn ace_rms_two_store(
    x: &makepad_ggml::backend::cuda::GpuTensor,
    weight: &[f32],
    group_cols: usize,
    cache_key: &str,
    eps: f32,
) -> Result<makepad_ggml::backend::cuda::GpuTensor> {
    use crate::sa3::dev_err;
    use makepad_ggml::backend::cuda::{
        gpu_bf16_round, gpu_gated_residual, gpu_rms_norm_mul, gpu_rms_norm_mul_round_bf16,
        gpu_upload,
    };
    if group_cols == 0 || x.cols() % group_cols != 0 || weight.len() != group_cols {
        return Err(DiffusionError::model("ace rms weight broadcast mismatch"));
    }
    if !ace_env_flag("ACE_RMS_TWO") {
        if ace_fuse_on("ACE_FUSE_NO_RMS") {
            return gpu_rms_norm_mul_round_bf16(x, group_cols, "acedit", cache_key, weight, eps)
                .map_err(|e| dev_err("ace rms1 fused", e));
        }
        let n = gpu_rms_norm_mul(x, group_cols, "acedit", cache_key, weight, eps)
            .map_err(|e| dev_err("ace rms1", e))?;
        return gpu_bf16_round(&n).map_err(|e| dev_err("ace rms1 rnd", e));
    }
    let ones = vec![1.0f32; group_cols];
    let n = gpu_rms_norm_mul(x, group_cols, "acedit", &format!("{cache_key}.rms1"), &ones, eps)
        .map_err(|e| dev_err("ace rms1", e))?;
    let n = gpu_bf16_round(&n).map_err(|e| dev_err("ace rms1 rnd", e))?;
    let zeros = gpu_upload(&vec![0f32; x.rows() * x.cols()], x.rows(), x.cols())
        .map_err(|e| dev_err("ace rms z", e))?;
    let mut w_row = vec![0f32; x.cols()];
    for (i, slot) in w_row.iter_mut().enumerate() {
        *slot = weight[i % group_cols];
    }
    let s = gpu_gated_residual(&zeros, &n, &w_row).map_err(|e| dev_err("ace rms w", e))?;
    gpu_bf16_round(&s).map_err(|e| dev_err("ace rms w rnd", e))
}

/// `normed * (1 + mods[scale_off..]) + mods[shift_off..]`, all on device.
///
/// Bitwise-equal to the old host-gate two-kernel recipe: the scale multiply
/// runs as `fmaf(scale1, normed, 0.0)` against an exact-zero residual (one
/// rounding of the product either way, and `-0 + +0 = +0` matches the old
/// `round(product) + 0.0`), the shift add as `fmaf(shift, 1.0, scaled)`
/// (`shift * 1.0` is exact, single rounding of the sum either way).
fn ace_adaln_stores(
    normed: &makepad_ggml::backend::cuda::GpuTensor,
    mods: &makepad_ggml::backend::cuda::GpuTensor,
    scale_off: usize,
    shift_off: usize,
    dim: usize,
    zeros: &makepad_ggml::backend::cuda::GpuTensor,
    ones_tok: &makepad_ggml::backend::cuda::GpuTensor,
    ones_row: &makepad_ggml::backend::cuda::GpuTensor,
) -> Result<makepad_ggml::backend::cuda::GpuTensor> {
    use crate::sa3::dev_err;
    use makepad_ggml::backend::cuda::{
        gpu_adaln_mod, gpu_add, gpu_bf16_round, gpu_gated_residual_mod, gpu_slice_cols,
    };
    if !ace_env_flag("ACE_ADALN_TWO") && ace_fuse_on("ACE_FUSE_NO_ADALN") {
        return gpu_adaln_mod(normed, mods, scale_off, shift_off)
            .map_err(|e| dev_err("ace adaln fused", e));
    }
    let scale = gpu_slice_cols(mods, scale_off, dim).map_err(|e| dev_err("ace adaln scale", e))?;
    let scale1 = gpu_add(ones_row, &scale).map_err(|e| dev_err("ace adaln 1+s", e))?;
    let single = !ace_env_flag("ACE_ADALN_TWO");
    let scale1 = if single {
        scale1
    } else {
        gpu_bf16_round(&scale1).map_err(|e| dev_err("ace adaln 1+s rnd", e))?
    };
    let scaled = gpu_gated_residual_mod(zeros, normed, &scale1, 0)
        .map_err(|e| dev_err("ace adaln mul", e))?;
    let scaled = if single {
        scaled
    } else {
        gpu_bf16_round(&scaled).map_err(|e| dev_err("ace adaln mul rnd", e))?
    };
    let out = gpu_gated_residual_mod(&scaled, ones_tok, mods, shift_off)
        .map_err(|e| dev_err("ace adaln add", e))?;
    if single {
        Ok(out)
    } else {
        gpu_bf16_round(&out).map_err(|e| dev_err("ace adaln add rnd", e))
    }
}

fn load_dit_block(weights: &H3ShardedWeights, index: usize) -> Result<DitBlock> {
    let d = ACE_DIT_DIM;
    let q = ACE_DIT_HEADS * ACE_DIT_HEAD_DIM;
    let kv = ACE_DIT_KV_HEADS * ACE_DIT_HEAD_DIM;
    let p = format!("layers.{index}");
    let attn = |kind: &str| format!("{p}.{kind}");
    let load_attn = |kind: &str| -> Result<(Vec<f32>, Vec<f32>, Vec<f32>, Vec<f32>, Vec<f32>, Vec<f32>)> {
        let a = attn(kind);
        Ok((
            ace_tensor_shaped(
                weights,
                &[&format!("{a}.to_q.weight"), &format!("{a}.q_proj.weight")],
                q * d,
            )?,
            ace_tensor_shaped(
                weights,
                &[&format!("{a}.to_k.weight"), &format!("{a}.k_proj.weight")],
                kv * d,
            )?,
            ace_tensor_shaped(
                weights,
                &[&format!("{a}.to_v.weight"), &format!("{a}.v_proj.weight")],
                kv * d,
            )?,
            ace_tensor_shaped(
                weights,
                &[
                    &format!("{a}.to_out.0.weight"),
                    &format!("{a}.o_proj.weight"),
                ],
                d * q,
            )?,
            ace_tensor_shaped(
                weights,
                &[&format!("{a}.norm_q.weight"), &format!("{a}.q_norm.weight")],
                ACE_DIT_HEAD_DIM,
            )?,
            ace_tensor_shaped(
                weights,
                &[&format!("{a}.norm_k.weight"), &format!("{a}.k_norm.weight")],
                ACE_DIT_HEAD_DIM,
            )?,
        ))
    };
    let (self_q, self_k, self_v, self_o, self_qn, self_kn) = load_attn("self_attn")?;
    let (cross_q, cross_k, cross_v, cross_o, cross_qn, cross_kn) = load_attn("cross_attn")?;
    Ok(DitBlock {
        self_norm: ace_tensor_shaped(weights, &[&format!("{p}.self_attn_norm.weight")], d)?,
        self_q,
        self_k,
        self_v,
        self_o,
        self_qn,
        self_kn,
        cross_norm: ace_tensor_shaped(weights, &[&format!("{p}.cross_attn_norm.weight")], d)?,
        cross_q,
        cross_k,
        cross_v,
        cross_o,
        cross_qn,
        cross_kn,
        mlp_norm: ace_tensor_shaped(weights, &[&format!("{p}.mlp_norm.weight")], d)?,
        gate: ace_tensor_shaped(weights, &[&format!("{p}.mlp.gate_proj.weight")], ACE_DIT_FFN * d)?,
        up: ace_tensor_shaped(weights, &[&format!("{p}.mlp.up_proj.weight")], ACE_DIT_FFN * d)?,
        down: ace_tensor_shaped(weights, &[&format!("{p}.mlp.down_proj.weight")], d * ACE_DIT_FFN)?,
        scale_shift: ace_tensor_shaped(weights, &[&format!("{p}.scale_shift_table")], 6 * d)?,
        sliding: layer_is_sliding(index),
    })
}

fn conv1d_stride2(x: &[f32], w: &[f32], b: &[f32], seq: usize, in_ch: usize, out_ch: usize) -> Vec<f32> {
    // x is [seq, in_ch] token-major. Conv1d k=2 s=2 p=0.
    let out_seq = seq / ACE_PATCH_SIZE;
    let k = ACE_PATCH_SIZE;
    let mut out = vec![0f32; out_seq * out_ch];
    par_rows(&mut out, out_ch, &|t, row| {
        row.copy_from_slice(b);
        let t0 = t * k;
        for o in 0..out_ch {
            let mut acc = row[o];
            let w_row = &w[o * in_ch * k..(o + 1) * in_ch * k];
            for c in 0..in_ch {
                for tap in 0..k {
                    acc += w_row[c * k + tap] * x[(t0 + tap) * in_ch + c];
                }
            }
            row[o] = acc;
        }
    });
    out
}

fn conv_transpose1d_stride2(
    x: &[f32],
    w: &[f32],
    b: &[f32],
    seq: usize,
    in_ch: usize,
    out_ch: usize,
) -> Vec<f32> {
    // ConvTranspose1d k=2 s=2 p=0. Weight layout (in, out, k).
    let out_seq = seq * ACE_PATCH_SIZE;
    let k = ACE_PATCH_SIZE;
    let mut out = vec![0f32; out_seq * out_ch];
    for o in 0..out_ch {
        for t in 0..out_seq {
            out[t * out_ch + o] = b[o];
        }
    }
    for t in 0..seq {
        for c in 0..in_ch {
            let xv = x[t * in_ch + c];
            let w_row = &w[c * out_ch * k..(c + 1) * out_ch * k];
            for tap in 0..k {
                let ot = t * k + tap;
                for o in 0..out_ch {
                    out[ot * out_ch + o] += w_row[o * k + tap] * xv;
                }
            }
        }
    }
    out
}

impl AceDit {
    pub fn load(dir: impl AsRef<Path>) -> Result<Self> {
        Self::load_with_progress(dir, None)
    }

    pub fn load_with_progress(
        dir: impl AsRef<Path>,
        mut progress: Option<ProgressHook>,
    ) -> Result<Self> {
        let weights = ace_open_shards(dir)?;
        let d = ACE_DIT_DIM;
        emit_byte_progress(&mut progress, "load ace dit", 0, weights.total_disk_bytes() as usize)?;
        let proj_in_w = ace_tensor_shaped(
            &weights,
            &["proj_in_conv.weight", "proj_in.1.weight"],
            d * ACE_IN_CHANNELS * ACE_PATCH_SIZE,
        )?;
        let proj_in_b = ace_tensor_shaped(
            &weights,
            &["proj_in_conv.bias", "proj_in.1.bias"],
            d,
        )?;
        let time = load_time_embed(&weights, "time_embed", d)?;
        let time_r = load_time_embed(&weights, "time_embed_r", d)?;
        let cond_embed_w = ace_tensor_shaped(
            &weights,
            &["condition_embedder.weight"],
            d * ACE_DIT_ENCODER_DIM,
        )?;
        let cond_embed_b = ace_tensor_shaped(&weights, &["condition_embedder.bias"], d)?;
        let mut blocks = Vec::with_capacity(ACE_DIT_LAYERS);
        for i in 0..ACE_DIT_LAYERS {
            if i % 4 == 0 {
                emit_progress(
                    &mut progress,
                    &format!("load ace dit {i}/{ACE_DIT_LAYERS}"),
                    i as f64 / ACE_DIT_LAYERS as f64,
                )?;
            }
            blocks.push(load_dit_block(&weights, i)?);
        }
        let norm_out = ace_tensor_shaped(&weights, &["norm_out.weight"], d)?;
        let proj_out_w = ace_tensor_shaped(
            &weights,
            &["proj_out_conv.weight", "proj_out.1.weight"],
            d * ACE_LATENT_DIM * ACE_PATCH_SIZE,
        )?;
        let proj_out_b = ace_tensor_shaped(
            &weights,
            &["proj_out_conv.bias", "proj_out.1.bias"],
            ACE_LATENT_DIM,
        )?;
        let scale_shift = ace_tensor_shaped(&weights, &["scale_shift_table"], 2 * d)?;
        let (rope_cos, rope_sin) = ace_rope_tables(16_384);
        emit_progress(&mut progress, "load ace dit", 1.0)?;
        Ok(Self {
            proj_in_w,
            proj_in_b,
            time,
            time_r,
            cond_embed_w,
            cond_embed_b,
            blocks,
            norm_out,
            proj_out_w,
            proj_out_b,
            scale_shift,
            rope_cos,
            rope_sin,
        })
    }

    /// One velocity prediction. `x` and `context` are token-major `[T, 64]`
    /// and `[T, 128]`. `encoder` is `[S, 2048]`.
    pub fn forward(
        &self,
        x: &[f32],
        context: &[f32],
        frames: usize,
        encoder: &AceConditioning,
        t: f32,
        t_r: f32,
        mut progress: Option<ProgressHook>,
    ) -> Result<Vec<f32>> {
        let d = ACE_DIT_DIM;
        if x.len() != frames * ACE_LATENT_DIM {
            return Err(DiffusionError::model(format!(
                "ace dit x {} vs {}",
                x.len(),
                frames * ACE_LATENT_DIM
            )));
        }
        if context.len() != frames * ACE_CONTEXT_DIM {
            return Err(DiffusionError::model(format!(
                "ace dit context {} vs {}",
                context.len(),
                frames * ACE_CONTEXT_DIM
            )));
        }
        let (temb_t, proj_t) = apply_time_embed(&self.time, t, d);
        let (temb_r, proj_r) = apply_time_embed(&self.time_r, t - t_r, d);
        let mut temb = temb_t;
        ace_add_inplace(&mut temb, &temb_r);
        let mut tproj = proj_t;
        ace_add_inplace(&mut tproj, &proj_r);

        let mut cat = vec![0f32; frames * ACE_IN_CHANNELS];
        for i in 0..frames {
            cat[i * ACE_IN_CHANNELS..i * ACE_IN_CHANNELS + ACE_CONTEXT_DIM]
                .copy_from_slice(&context[i * ACE_CONTEXT_DIM..(i + 1) * ACE_CONTEXT_DIM]);
            cat[i * ACE_IN_CHANNELS + ACE_CONTEXT_DIM..(i + 1) * ACE_IN_CHANNELS]
                .copy_from_slice(&x[i * ACE_LATENT_DIM..(i + 1) * ACE_LATENT_DIM]);
        }
        let original = frames;
        let mut padded = frames;
        if padded % ACE_PATCH_SIZE != 0 {
            padded += ACE_PATCH_SIZE - (padded % ACE_PATCH_SIZE);
            cat.resize(padded * ACE_IN_CHANNELS, 0.0);
        }
        let mut h = conv1d_stride2(
            &cat,
            &self.proj_in_w,
            &self.proj_in_b,
            padded,
            ACE_IN_CHANNELS,
            d,
        );
        let tokens = padded / ACE_PATCH_SIZE;
        let debug_l0 = std::env::var("ACE_DEBUG_L0").ok().as_deref() == Some("1");
        if std::env::var("ACE_DEBUG_STEM").ok().as_deref() == Some("1") || debug_l0 {
            let freq = ace_time_sinusoid(t);
            eprintln!(
                "stem t={t} tr={} freq[:4]={:?} temb[:4]={:?} tproj[:4]={:?} tproj_abs={} h0[:4]={:?} h_absmax={}",
                t - t_r,
                &freq[..4],
                &temb[..4],
                &tproj[..4],
                tproj.iter().fold(0.0f32, |a, v| a.max(v.abs())),
                &h[..4],
                h.iter().fold(0.0f32, |a, v| a.max(v.abs()))
            );
        }
        let enc = linear(
            &encoder.encoder_hidden,
            &self.cond_embed_w,
            Some(&self.cond_embed_b),
            encoder.tokens,
            ACE_DIT_ENCODER_DIM,
            d,
        );
        let enc_tokens = encoder.tokens;
        let q_heads = ACE_DIT_HEADS;
        let kv_heads = ACE_DIT_KV_HEADS;
        let hd = ACE_DIT_HEAD_DIM;

        for (bi, block) in self.blocks.iter().enumerate() {
            if progress.is_some() && bi % 4 == 0 {
                emit_progress(
                    &mut progress,
                    &format!("dit-block {}/{ACE_DIT_LAYERS}", bi + 1),
                    bi as f64 / ACE_DIT_LAYERS as f64,
                )?;
            }
            let mut mods = block.scale_shift.clone();
            ace_add_inplace(&mut mods, &tproj);
            let shift_msa = &mods[0..d];
            let scale_msa = &mods[d..2 * d];
            let gate_msa = &mods[2 * d..3 * d];
            let c_shift = &mods[3 * d..4 * d];
            let c_scale = &mods[4 * d..5 * d];
            let c_gate = &mods[5 * d..6 * d];

            let mut self_rms = h.clone();
            rms_norm_rows(&mut self_rms, &block.self_norm, d, ACE_RMS_EPS);
            let normed = ace_adaln(&h, &block.self_norm, shift_msa, scale_msa, d);
            if debug_l0 && bi <= 1 {
                eprintln!(
                    "l0 sst[:4]={:?} shift[:4]={:?} scale[:4]={:?} gate[:4]={:?} self_rms[:4]={:?} self_rms_abs={} adaln[:4]={:?}",
                    &block.scale_shift[..4],
                    &shift_msa[..4],
                    &scale_msa[..4],
                    &gate_msa[..4],
                    &self_rms[..4],
                    self_rms.iter().fold(0.0f32, |a, v| a.max(v.abs())),
                    &normed[..4]
                );
            }
            let mut q = linear(&normed, &block.self_q, None, tokens, d, q_heads * hd);
            let mut k = linear(&normed, &block.self_k, None, tokens, d, kv_heads * hd);
            let v = linear(&normed, &block.self_v, None, tokens, d, kv_heads * hd);
            for row in 0..tokens {
                let qrow = &mut q[row * q_heads * hd..(row + 1) * q_heads * hd];
                for head in 0..q_heads {
                    ace_rms_head(&mut qrow[head * hd..(head + 1) * hd], &block.self_qn);
                }
                ace_apply_rope_half(qrow, row, q_heads, &self.rope_cos, &self.rope_sin);
                let krow = &mut k[row * kv_heads * hd..(row + 1) * kv_heads * hd];
                for head in 0..kv_heads {
                    ace_rms_head(&mut krow[head * hd..(head + 1) * hd], &block.self_kn);
                }
                ace_apply_rope_half(krow, row, kv_heads, &self.rope_cos, &self.rope_sin);
            }
            if debug_l0 && bi == 0 {
                eprintln!(
                    "cpu l0 q_rope[:4]={:?} abs={} adaln[:4]={:?}",
                    &q[..4],
                    q.iter().fold(0.0f32, |a, v| a.max(v.abs())),
                    &normed[..4]
                );
            }
            let window = if block.sliding {
                Some(ACE_SLIDING_WINDOW)
            } else {
                None
            };
            let attn = ace_gqa_attention(
                &q, &k, &v, q_heads, kv_heads, hd, tokens, tokens, window, false, None,
            );
            let proj = linear(&attn, &block.self_o, None, tokens, q_heads * hd, d);
            if debug_l0 && bi <= 1 {
                eprintln!(
                    "l0 self_attn[:4]={:?} abs={}",
                    &proj[..4],
                    proj.iter().fold(0.0f32, |a, v| a.max(v.abs()))
                );
            }
            ace_gated_residual(&mut h, &proj, gate_msa, d);

            let mut cn = h.clone();
            rms_norm_rows(&mut cn, &block.cross_norm, d, ACE_RMS_EPS);
            if debug_l0 && bi <= 1 {
                eprintln!(
                    "l0 cross_norm[:4]={:?} abs={}",
                    &cn[..4],
                    cn.iter().fold(0.0f32, |a, v| a.max(v.abs()))
                );
            }
            let mut cq = linear(&cn, &block.cross_q, None, tokens, d, q_heads * hd);
            let mut ck = linear(&enc, &block.cross_k, None, enc_tokens, d, kv_heads * hd);
            let cv = linear(&enc, &block.cross_v, None, enc_tokens, d, kv_heads * hd);
            for row in 0..tokens {
                let qrow = &mut cq[row * q_heads * hd..(row + 1) * q_heads * hd];
                for head in 0..q_heads {
                    ace_rms_head(&mut qrow[head * hd..(head + 1) * hd], &block.cross_qn);
                }
            }
            for row in 0..enc_tokens {
                let krow = &mut ck[row * kv_heads * hd..(row + 1) * kv_heads * hd];
                for head in 0..kv_heads {
                    ace_rms_head(&mut krow[head * hd..(head + 1) * hd], &block.cross_kn);
                }
            }
            let cattn = ace_gqa_attention(
                &cq, &ck, &cv, q_heads, kv_heads, hd, tokens, enc_tokens, None, false, None,
            );
            let cproj = linear(&cattn, &block.cross_o, None, tokens, q_heads * hd, d);
            if debug_l0 && bi <= 1 {
                eprintln!(
                    "l0 cross_attn[:4]={:?} abs={}",
                    &cproj[..4],
                    cproj.iter().fold(0.0f32, |a, v| a.max(v.abs()))
                );
            }
            ace_add_inplace(&mut h, &cproj);

            let fnorm = ace_adaln(&h, &block.mlp_norm, c_shift, c_scale, d);
            let gate = linear(&fnorm, &block.gate, None, tokens, d, ACE_DIT_FFN);
            let up = linear(&fnorm, &block.up, None, tokens, d, ACE_DIT_FFN);
            let act = ace_swiglu(&gate, &up);
            let down = linear(&act, &block.down, None, tokens, ACE_DIT_FFN, d);
            if debug_l0 && bi <= 1 {
                eprintln!(
                    "l0 mlp[:4]={:?} abs={} before_mlp_h[:4]={:?}",
                    &down[..4],
                    down.iter().fold(0.0f32, |a, v| a.max(v.abs())),
                    &h[..4]
                );
            }
            ace_gated_residual(&mut h, &down, c_gate, d);
            if debug_l0 && (bi < 2 || bi % 8 == 7 || bi + 1 == ACE_DIT_LAYERS) {
                eprintln!(
                    "l{bi} out[:4]={:?} abs={} sliding={}",
                    &h[..4],
                    h.iter().fold(0.0f32, |a, v| a.max(v.abs())),
                    block.sliding
                );
            }
        }

        let mut shift = self.scale_shift[..d].to_vec();
        let mut scale = self.scale_shift[d..].to_vec();
        ace_add_inplace(&mut shift, &temb);
        ace_add_inplace(&mut scale, &temb);
        let hout = ace_adaln(&h, &self.norm_out, &shift, &scale, d);
        let mut y = conv_transpose1d_stride2(
            &hout,
            &self.proj_out_w,
            &self.proj_out_b,
            tokens,
            d,
            ACE_LATENT_DIM,
        );
        y.truncate(original * ACE_LATENT_DIM);
        if debug_l0 {
            eprintln!(
                "pred[:8]={:?} abs={} tokens={} frames={}",
                &y[..8.min(y.len())],
                y.iter().fold(0.0f32, |a, v| a.max(v.abs())),
                tokens,
                original
            );
        }
        Ok(y)
    }

    pub fn prepare_device(&self) -> AceDitDevice {
        let d = ACE_DIT_DIM;
        let q = ACE_DIT_HEADS * ACE_DIT_HEAD_DIM;
        let kv = ACE_DIT_KV_HEADS * ACE_DIT_HEAD_DIM;
        let in_ch = ACE_IN_CHANNELS;
        let k = ACE_PATCH_SIZE;
        let in_k = in_ch * k;
        let mut pin = vec![0f32; d * in_k];
        for o in 0..d {
            for c in 0..in_ch {
                for tap in 0..k {
                    pin[o * in_k + tap * in_ch + c] = self.proj_in_w[(o * in_ch + c) * k + tap];
                }
            }
        }
        let out_ch = ACE_LATENT_DIM;
        let mut pout0 = vec![0f32; out_ch * d];
        let mut pout1 = vec![0f32; out_ch * d];
        for c in 0..d {
            for o in 0..out_ch {
                pout0[o * d + c] = self.proj_out_w[(c * out_ch + o) * k];
                pout1[o * d + c] = self.proj_out_w[(c * out_ch + o) * k + 1];
            }
        }
        let blocks = self
            .blocks
            .iter()
            .enumerate()
            .map(|(i, block)| {
                let mut gu = Vec::with_capacity(2 * ACE_DIT_FFN * d);
                gu.extend_from_slice(&block.up);
                gu.extend_from_slice(&block.gate);
                let mut qkv = Vec::with_capacity((q + kv + kv) * d);
                qkv.extend_from_slice(&block.self_q);
                qkv.extend_from_slice(&block.self_k);
                qkv.extend_from_slice(&block.self_v);
                DitDevBlock {
                    q: Bf16Weight::new(format!("acedit.b{i}.q"), &block.self_q, q, d),
                    k: Bf16Weight::new(format!("acedit.b{i}.k"), &block.self_k, kv, d),
                    v: Bf16Weight::new(format!("acedit.b{i}.v"), &block.self_v, kv, d),
                    qkv: Bf16Weight::new(format!("acedit.b{i}.qkv"), &qkv, q + kv + kv, d),
                    o: Bf16Weight::new(format!("acedit.b{i}.o"), &block.self_o, d, q),
                    cross_q: Bf16Weight::new(format!("acedit.b{i}.cq"), &block.cross_q, q, d),
                    cross_k: Bf16Weight::new(format!("acedit.b{i}.ck"), &block.cross_k, kv, d),
                    cross_v: Bf16Weight::new(format!("acedit.b{i}.cv"), &block.cross_v, kv, d),
                    cross_o: Bf16Weight::new(format!("acedit.b{i}.co"), &block.cross_o, d, q),
                    gate: Bf16Weight::new(format!("acedit.b{i}.gate"), &block.gate, ACE_DIT_FFN, d),
                    up: Bf16Weight::new(format!("acedit.b{i}.up"), &block.up, ACE_DIT_FFN, d),
                    gate_up: Bf16Weight::new(format!("acedit.b{i}.gu"), &gu, 2 * ACE_DIT_FFN, d),
                    down: Bf16Weight::new(format!("acedit.b{i}.dn"), &block.down, d, ACE_DIT_FFN),
                    sliding: block.sliding,
                }
            })
            .collect();
        AceDitDevice {
            proj_in: Bf16Weight::new("acedit.pin", &pin, d, in_k),
            cond_embed: Bf16Weight::new("acedit.cond", &self.cond_embed_w, d, ACE_DIT_ENCODER_DIM),
            proj_out0: Bf16Weight::new("acedit.pout0", &pout0, out_ch, d),
            proj_out1: Bf16Weight::new("acedit.pout1", &pout1, out_ch, d),
            time: time_embed_dev("acedit.time", &self.time, d),
            time_r: time_embed_dev("acedit.time_r", &self.time_r, d),
            blocks,
            cache_id: ACE_DEVICE_IDS.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        }
    }

    /// Encoder-only cross-attn K/V (Linear + k-norm + GQA expand). Step-constant.
    pub fn prepare_cross_kv(
        &self,
        device: &AceDitDevice,
        encoder: &AceConditioning,
    ) -> Result<AceCrossKvCache> {
        use crate::sa3::dev_err;
        use makepad_ggml::backend::cuda::{
            gpu_bf16_round, gpu_linear_nt_cached_bf16_bias_epilogue, gpu_upload,
        };
        let q_heads = ACE_DIT_HEADS;
        let kv_heads = ACE_DIT_KV_HEADS;
        let hd = ACE_DIT_HEAD_DIM;
        let group = q_heads / kv_heads;
        let enc_tokens = encoder.tokens;
        let enc_host = gpu_upload(&encoder.encoder_hidden, enc_tokens, ACE_DIT_ENCODER_DIM)
            .map_err(|e| dev_err("ace kv enc up", e))?;
        let enc = gpu_linear_nt_cached_bf16_bias_epilogue(
            &enc_host,
            "acedit",
            &[device.cond_embed.part()],
            &self.cond_embed_b,
        )
        .map_err(|e| dev_err("ace kv cond", e))?;
        let enc = gpu_bf16_round(&enc).map_err(|e| dev_err("ace kv cond rnd", e))?;
        let mut layers = Vec::with_capacity(self.blocks.len());
        for (bi, (block, devb)) in self.blocks.iter().zip(&device.blocks).enumerate() {
            layers.push(ace_layer_cross_kv(
                &enc, block, devb, bi, q_heads, kv_heads, hd, group,
            )?);
        }
        eprintln!(
            "ace cross-kv cache layers={} enc_tokens={}",
            layers.len(),
            enc_tokens
        );
        Ok(AceCrossKvCache { layers, enc_tokens })
    }

    pub fn forward_device(
        &self,
        device: &AceDitDevice,
        x: &[f32],
        context: &[f32],
        frames: usize,
        encoder: &AceConditioning,
        t: f32,
        t_r: f32,
    ) -> Result<Vec<f32>> {
        self.forward_device_enc(device, x, context, frames, encoder, None, t, t_r, None)
    }

    pub fn forward_device_kv(
        &self,
        device: &AceDitDevice,
        x: &[f32],
        context: &[f32],
        frames: usize,
        encoder: &AceConditioning,
        t: f32,
        t_r: f32,
        cache: &AceCrossKvCache,
    ) -> Result<Vec<f32>> {
        self.forward_device_enc(
            device,
            x,
            context,
            frames,
            encoder,
            None,
            t,
            t_r,
            Some(cache),
        )
    }

    pub fn forward_device_cfg(
        &self,
        device: &AceDitDevice,
        x: &[f32],
        context: &[f32],
        frames: usize,
        encoder: &AceConditioning,
        encoder2: &AceConditioning,
        t: f32,
        t_r: f32,
    ) -> Result<(Vec<f32>, Vec<f32>)> {
        let y = self.forward_device_enc(
            device, x, context, frames, encoder, Some(encoder2), t, t_r, None,
        )?;
        let n = frames * ACE_LATENT_DIM;
        if y.len() != 2 * n {
            return Err(DiffusionError::model(format!(
                "ace dit cfg out {} want {}",
                y.len(),
                2 * n
            )));
        }
        Ok((y[..n].to_vec(), y[n..].to_vec()))
    }

    fn forward_device_enc(
        &self,
        device: &AceDitDevice,
        x: &[f32],
        context: &[f32],
        frames: usize,
        encoder: &AceConditioning,
        encoder2: Option<&AceConditioning>,
        t: f32,
        t_r: f32,
        cross_kv: Option<&AceCrossKvCache>,
    ) -> Result<Vec<f32>> {
        use crate::sa3::dev_err;
        use makepad_ggml::backend::cuda::{
            gpu_add, gpu_add_bf16, gpu_attention_packed_composite_bf16,
            gpu_attention_packed_cross_composite_bf16, gpu_attention_packed_f32,
            gpu_attention_packed_fa2_bf16, gpu_attention_packed_sliding,
            gpu_attention_packed_sliding_bf16, gpu_bf16_round,
            gpu_concat_cols, gpu_concat_rows, gpu_download, gpu_gated_residual,
            gpu_gated_residual_mod, gpu_gated_residual_round_add_bf16, gpu_slice_rows,
            gpu_linear_nt_cached_bf16_bias_epilogue, gpu_linear_nt_cached_bf16_mm, gpu_mul,
            gpu_rms_norm_mod_indexed, gpu_rms_norm_mul_bf16, gpu_rope_half,
            gpu_rope_half_round_bf16, gpu_silu, gpu_silu_round_mul_round_bf16,
            gpu_slice_cols, gpu_swiglu_value_gate, gpu_upload, gpu_upload_u32,
        };
        let lin = |x: &makepad_ggml::backend::cuda::GpuTensor, w: &Bf16Weight| {
            gpu_linear_nt_cached_bf16_mm(x, "acedit", &[w.part()])
        };
        let lin_b = |x: &makepad_ggml::backend::cuda::GpuTensor, w: &Bf16Weight, bias: &[f32]| {
            gpu_linear_nt_cached_bf16_bias_epilogue(x, "acedit", &[w.part()], bias)
        };
        let dump_dir = hook_dump_take();

        let d = ACE_DIT_DIM;
        let q_heads = ACE_DIT_HEADS;
        let kv_heads = ACE_DIT_KV_HEADS;
        let hd = ACE_DIT_HEAD_DIM;
        let qdim = q_heads * hd;
        let kvdim = kv_heads * hd;
        let group = q_heads / kv_heads;
        let scale = 1.0 / (hd as f32).sqrt();
        let half = hd / 2;

        if x.len() != frames * ACE_LATENT_DIM || context.len() != frames * ACE_CONTEXT_DIM {
            return Err(DiffusionError::model("ace dit device: shape mismatch"));
        }
        let original = frames;
        let mut padded = frames;
        if padded % ACE_PATCH_SIZE != 0 {
            padded += ACE_PATCH_SIZE - (padded % ACE_PATCH_SIZE);
        }
        let tokens = padded / ACE_PATCH_SIZE;
        let batch = if encoder2.is_some() { 2usize } else { 1usize };
        let rows = tokens * batch;
        ace_fwd_prepare(
            device.cache_id,
            rows,
            self.blocks.len(),
            (t.to_bits(), (t - t_r).to_bits()),
        );
        let zeros_tok = ace_fwd_slot(
            |c| &mut c.zeros,
            || {
                makepad_ggml::backend::cuda::gpu_upload(&vec![0f32; rows * d], rows, d)
                    .map_err(|e| dev_err("ace dit zeros", e))
            },
        )?;
        let ones_tok = ace_fwd_slot(
            |c| &mut c.ones_tok,
            || {
                makepad_ggml::backend::cuda::gpu_upload(&vec![1.0f32; rows * d], rows, d)
                    .map_err(|e| dev_err("ace dit ones", e))
            },
        )?;
        let ones_row = ace_fwd_slot(
            |c| &mut c.ones_row,
            || {
                makepad_ggml::backend::cuda::gpu_upload(&vec![1.0f32; d], 1, d)
                    .map_err(|e| dev_err("ace dit ones row", e))
            },
        )?;
        let (temb, tproj) = if dump_dir.is_some() {
            let (temb_t, proj_t) = apply_time_embed_dev(&self.time, &device.time, t)?;
            let (temb_r, proj_r) = apply_time_embed_dev(&self.time_r, &device.time_r, t - t_r)?;
            let temb = gpu_add(&temb_t, &temb_r).map_err(|e| dev_err("ace time temb add", e))?;
            let temb = gpu_bf16_round(&temb).map_err(|e| dev_err("ace time temb rnd", e))?;
            let tproj = gpu_add(&proj_t, &proj_r).map_err(|e| dev_err("ace time tproj add", e))?;
            let tproj = gpu_bf16_round(&tproj).map_err(|e| dev_err("ace time tproj rnd", e))?;
            if let Some(dir) = dump_dir.as_ref() {
                let _ = std::fs::create_dir_all(&dir);
                let temb_t_h = gpu_download(&temb_t).map_err(|e| dev_err("ace hook temb_t", e))?;
                let temb_r_h = gpu_download(&temb_r).map_err(|e| dev_err("ace hook temb_r", e))?;
                let proj_t_h = gpu_download(&proj_t).map_err(|e| dev_err("ace hook proj_t", e))?;
                let proj_r_h = gpu_download(&proj_r).map_err(|e| dev_err("ace hook proj_r", e))?;
                let temb_h = gpu_download(&temb).map_err(|e| dev_err("ace hook temb", e))?;
                let tproj_h = gpu_download(&tproj).map_err(|e| dev_err("ace hook tproj", e))?;
                dump_hook_npy(&dir, "native_step0_time_embed_temb.npy", &temb_t_h, &[1, d]);
                dump_hook_npy(&dir, "native_step0_time_embed_r_temb.npy", &temb_r_h, &[1, d]);
                dump_hook_npy(&dir, "native_step0_time_embed_tproj.npy", &proj_t_h, &[1, 6, d]);
                dump_hook_npy(&dir, "native_step0_time_embed_r_tproj.npy", &proj_r_h, &[1, 6, d]);
                dump_hook_npy(&dir, "native_step0_temb.npy", &temb_h, &[1, d]);
                dump_hook_npy(&dir, "native_step0_tproj.npy", &tproj_h, &[1, 6, d]);
            }
            (Rc::new(temb), Rc::new(tproj))
        } else {
            ace_fwd_temb(|| {
                let (temb_t, proj_t) = apply_time_embed_dev(&self.time, &device.time, t)?;
                let (temb_r, proj_r) =
                    apply_time_embed_dev(&self.time_r, &device.time_r, t - t_r)?;
                let temb = gpu_add(&temb_t, &temb_r).map_err(|e| dev_err("ace time temb add", e))?;
                let temb = gpu_bf16_round(&temb).map_err(|e| dev_err("ace time temb rnd", e))?;
                let tproj = gpu_add(&proj_t, &proj_r).map_err(|e| dev_err("ace time tproj add", e))?;
                let tproj = gpu_bf16_round(&tproj).map_err(|e| dev_err("ace time tproj rnd", e))?;
                Ok((temb, tproj))
            })?
        };

        let mut cat = vec![0f32; frames * ACE_IN_CHANNELS];
        for i in 0..frames {
            cat[i * ACE_IN_CHANNELS..i * ACE_IN_CHANNELS + ACE_CONTEXT_DIM]
                .copy_from_slice(&context[i * ACE_CONTEXT_DIM..(i + 1) * ACE_CONTEXT_DIM]);
            cat[i * ACE_IN_CHANNELS + ACE_CONTEXT_DIM..(i + 1) * ACE_IN_CHANNELS]
                .copy_from_slice(&x[i * ACE_LATENT_DIM..(i + 1) * ACE_LATENT_DIM]);
        }
        if padded != frames {
            cat.resize(padded * ACE_IN_CHANNELS, 0.0);
        }
        let in_k = ACE_IN_CHANNELS * ACE_PATCH_SIZE;
        let mut z = vec![0f32; tokens * in_k];
        for i in 0..tokens {
            z[i * in_k..i * in_k + ACE_IN_CHANNELS].copy_from_slice(
                &cat[(2 * i) * ACE_IN_CHANNELS..(2 * i + 1) * ACE_IN_CHANNELS],
            );
            z[i * in_k + ACE_IN_CHANNELS..(i + 1) * in_k].copy_from_slice(
                &cat[(2 * i + 1) * ACE_IN_CHANNELS..(2 * i + 2) * ACE_IN_CHANNELS],
            );
        }
        let z_g = gpu_upload(&z, tokens, in_k).map_err(|e| dev_err("ace dit pin z", e))?;
        let mut h = lin_b(&z_g, &device.proj_in, &self.proj_in_b)
            .map_err(|e| dev_err("ace dit pin", e))?;
        if batch == 2 {
            h = gpu_concat_rows(&h, &h).map_err(|e| dev_err("ace dit batch h", e))?;
        }
        if let Some(path) = pin_proj_in_path() {
            let data = load_npy_f32(&path)?;
            if data.len() != tokens * d {
                return Err(DiffusionError::model(format!(
                    "ACE_PIN_PROJ_IN {} len {} want {}",
                    path.display(),
                    data.len(),
                    tokens * d
                )));
            }
            h = gpu_upload(&data, tokens, d).map_err(|e| dev_err("ace pin proj_in up", e))?;
            h = gpu_bf16_round(&h).map_err(|e| dev_err("ace pin proj_in rnd", e))?;
        }
        if let Some(dir) = dump_dir.as_ref() {
            let host = gpu_download(&h).map_err(|e| dev_err("ace hook proj_in", e))?;
            dump_hook_npy(&dir, "native_step0_proj_in.npy", &host, &[1, tokens, d]);
        }
        let debug_cmp = std::env::var("ACE_DEBUG_CMP").ok().as_deref() == Some("1");
        // The o/cross_o GEMM staging already rounds its f32 input RN-even, so
        // the explicit post-attention bf16 round is redundant for the model
        // path (round is idempotent). Keep it when dump/debug taps read the
        // rounded tensor.
        let skip_pregemm_round =
            dump_dir.is_none() && !debug_cmp && ace_fuse_on("ACE_FUSE_NO_PREGEMM");
        if debug_cmp {
            let host = gpu_download(&h).map_err(|e| dev_err("ace dit dbg h0", e))?;
            eprintln!(
                "cuda h0[:4]={:?} abs={}",
                &host[..4.min(host.len())],
                host.iter().fold(0.0f32, |a, v| a.max(v.abs()))
            );
        }

        let enc_tokens = encoder.tokens;
        if let Some(cache) = cross_kv {
            if cache.enc_tokens != enc_tokens || cache.layers.len() != self.blocks.len() {
                return Err(DiffusionError::model("ace dit cross-kv cache mismatch"));
            }
        }
        let enc = if cross_kv.is_some() && encoder2.is_none() {
            None
        } else if let Some(enc2) = encoder2 {
            if enc2.tokens != enc_tokens {
                return Err(DiffusionError::model("ace dit cfg encoder token mismatch"));
            }
            let mut both = encoder.encoder_hidden.clone();
            both.extend_from_slice(&enc2.encoder_hidden);
            let enc_host = gpu_upload(&both, enc_tokens * 2, ACE_DIT_ENCODER_DIM)
                .map_err(|e| dev_err("ace dit enc2 up", e))?;
            let enc = lin_b(&enc_host, &device.cond_embed, &self.cond_embed_b)
                .map_err(|e| dev_err("ace dit cond2", e))?;
            Some(gpu_bf16_round(&enc).map_err(|e| dev_err("ace dit cond2 rnd", e))?)
        } else {
            let enc_host = gpu_upload(&encoder.encoder_hidden, enc_tokens, ACE_DIT_ENCODER_DIM)
                .map_err(|e| dev_err("ace dit enc up", e))?;
            let enc = lin_b(&enc_host, &device.cond_embed, &self.cond_embed_b)
                .map_err(|e| dev_err("ace dit cond", e))?;
            Some(gpu_bf16_round(&enc).map_err(|e| dev_err("ace dit cond rnd", e))?)
        };

        let rope_f32 = ace_env_flag("ACE_ROPE_F32");
        let (rope_cos, rope_sin) = ace_fwd_rope(tokens, batch, rope_f32, rows, half)?;
        let idx = ace_fwd_slot(
            |c| &mut c.idx,
            || gpu_upload_u32(&vec![0u32; rows]).map_err(|e| dev_err("ace dit idx", e)),
        )?;

        for (bi, (block, devb)) in self.blocks.iter().zip(&device.blocks).enumerate() {
            let sst = ace_fwd_slot(
                |c| &mut c.sst[bi],
                || {
                    let sst = gpu_upload(&block.scale_shift, 1, 6 * d)
                        .map_err(|e| dev_err("ace dit sst", e))?;
                    gpu_bf16_round(&sst).map_err(|e| dev_err("ace dit sst rnd", e))
                },
            )?;
            let mods_g = ace_fwd_slot(
                |c| &mut c.mods[bi],
                || {
                    let mods_g = gpu_add(&sst, &tproj).map_err(|e| dev_err("ace dit mods", e))?;
                    gpu_bf16_round(&mods_g).map_err(|e| dev_err("ace dit mods rnd", e))
                },
            )?;
            let rms_self = ace_rms_two_store(&h, &block.self_norm, d, &format!("b{bi}.sn"), ACE_RMS_EPS)?;
            let normed =
                ace_adaln_stores(&rms_self, &mods_g, d, 0, d, &zeros_tok, &ones_tok, &ones_row)?;
            if debug_cmp && bi == 0 {
                let host = gpu_download(&normed).map_err(|e| dev_err("ace dit dbg adaln", e))?;
                eprintln!(
                    "cuda l0 adaln[:4]={:?} abs={}",
                    &host[..4.min(host.len())],
                    host.iter().fold(0.0f32, |a, v| a.max(v.abs()))
                );
            }
            let (q, k, v) = if ace_env_flag("ACE_QKV_FUSED") {
                let qkv = lin(&normed, &devb.qkv).map_err(|e| dev_err("ace dit qkv", e))?;
                let q = gpu_slice_cols(&qkv, 0, qdim).map_err(|e| dev_err("ace dit q sl", e))?;
                let k = gpu_slice_cols(&qkv, qdim, kvdim).map_err(|e| dev_err("ace dit k sl", e))?;
                let v = gpu_slice_cols(&qkv, qdim + kvdim, kvdim)
                    .map_err(|e| dev_err("ace dit v sl", e))?;
                (q, k, v)
            } else {
                (
                    lin(&normed, &devb.q).map_err(|e| dev_err("ace dit q", e))?,
                    lin(&normed, &devb.k).map_err(|e| dev_err("ace dit k", e))?,
                    lin(&normed, &devb.v).map_err(|e| dev_err("ace dit v", e))?,
                )
            };
            if bi == 0 {
                if let Some(dir) = dump_dir.as_ref() {
                    let qh = gpu_download(&q).map_err(|e| dev_err("ace hook qlin", e))?;
                    dump_hook_npy(&dir, "native_step0_l0_q_lin.npy", &qh, &[1, tokens, q_heads, hd]);
                }
            }
            let mut q = ace_rms_two_store(&q, &block.self_qn, hd, &format!("b{bi}.qn"), ACE_RMS_EPS)?;
            let k = ace_rms_two_store(&k, &block.self_kn, hd, &format!("b{bi}.kn"), ACE_RMS_EPS)?;
            if bi == 0 {
                if let Some(path) = pin_q_rms_path() {
                    let data = load_npy_f32(&path)?;
                    if data.len() != tokens * qdim {
                        return Err(DiffusionError::model(format!(
                            "ACE_PIN_Q_RMS {} len {} want {}",
                            path.display(),
                            data.len(),
                            tokens * qdim
                        )));
                    }
                    q = gpu_upload(&data, tokens, qdim).map_err(|e| dev_err("ace pin q_rms up", e))?;
                    q = gpu_bf16_round(&q).map_err(|e| dev_err("ace pin q_rms rnd", e))?;
                }
                if let Some(dir) = dump_dir.as_ref() {
                    let qh = gpu_download(&q).map_err(|e| dev_err("ace hook qrms", e))?;
                    dump_hook_npy(&dir, "native_step0_l0_q_rms.npy", &qh, &[1, tokens, q_heads, hd]);
                }
            }
            let (q, k) = if !rope_f32 && ace_fuse_on("ACE_FUSE_NO_ROPE") {
                (
                    gpu_rope_half_round_bf16(&q, q_heads, half, &rope_cos, &rope_sin)
                        .map_err(|e| dev_err("ace dit rq", e))?,
                    gpu_rope_half_round_bf16(&k, kv_heads, half, &rope_cos, &rope_sin)
                        .map_err(|e| dev_err("ace dit rk", e))?,
                )
            } else {
                let q = gpu_rope_half(&q, q_heads, half, &rope_cos, &rope_sin)
                    .map_err(|e| dev_err("ace dit rq", e))?;
                let k = gpu_rope_half(&k, kv_heads, half, &rope_cos, &rope_sin)
                    .map_err(|e| dev_err("ace dit rk", e))?;
                if rope_f32 {
                    (q, k)
                } else {
                    (
                        gpu_bf16_round(&q).map_err(|e| dev_err("ace dit rq rnd", e))?,
                        gpu_bf16_round(&k).map_err(|e| dev_err("ace dit rk rnd", e))?,
                    )
                }
            };
            if bi == 0 {
                if let Some(dir) = dump_dir.as_ref() {
                    let mods_h = gpu_download(&mods_g).map_err(|e| dev_err("ace hook mods", e))?;
                    let norm_h = gpu_download(&normed).map_err(|e| dev_err("ace hook normed", e))?;
                    let qh = gpu_download(&q).map_err(|e| dev_err("ace hook q", e))?;
                    let kh = gpu_download(&k).map_err(|e| dev_err("ace hook k", e))?;
                    let vh = gpu_download(&v).map_err(|e| dev_err("ace hook v", e))?;
                    dump_hook_npy(&dir, "native_step0_l0_mods.npy", &mods_h, &[1, 6, d]);
                    dump_hook_npy(&dir, "native_step0_l0_normed.npy", &norm_h, &[1, tokens, d]);
                    dump_hook_npy(&dir, "native_step0_l0_q_rope.npy", &qh, &[1, tokens, q_heads, hd]);
                    dump_hook_npy(&dir, "native_step0_l0_k_rope.npy", &kh, &[1, tokens, kv_heads, hd]);
                    dump_hook_npy(&dir, "native_step0_l0_v.npy", &vh, &[1, tokens, kvdim]);
                }
            }
            if debug_cmp && bi == 0 {
                let host = gpu_download(&q).map_err(|e| dev_err("ace dit dbg q", e))?;
                eprintln!(
                    "cuda l0 q_rope[:4]={:?} abs={}",
                    &host[..4.min(host.len())],
                    host.iter().fold(0.0f32, |a, v| a.max(v.abs()))
                );
            }
            let attn_cpu = std::env::var("ACE_ATTN_CPU").ok().as_deref() == Some("1");
            let window = if devb.sliding {
                Some(ACE_SLIDING_WINDOW)
            } else {
                None
            };
            let cpu_attn_ref = if debug_cmp && bi == 0 && !attn_cpu {
                let qh = gpu_download(&q).map_err(|e| dev_err("ace cmp q", e))?;
                let kh = gpu_download(&k).map_err(|e| dev_err("ace cmp k", e))?;
                let vh = gpu_download(&v).map_err(|e| dev_err("ace cmp v", e))?;
                Some(ace_gqa_attention(
                    &qh, &kh, &vh, q_heads, kv_heads, hd, tokens, tokens, window, false, None,
                ))
            } else {
                None
            };
            let attn = if attn_cpu {
                let qh = gpu_download(&q).map_err(|e| dev_err("ace attn q", e))?;
                let kh = gpu_download(&k).map_err(|e| dev_err("ace attn k", e))?;
                let vh = gpu_download(&v).map_err(|e| dev_err("ace attn v", e))?;
                let host = ace_gqa_attention(
                    &qh, &kh, &vh, q_heads, kv_heads, hd, tokens, tokens, window, false, None,
                );
                let up = gpu_upload(&host, tokens, qdim).map_err(|e| dev_err("ace attn up", e))?;
                gpu_bf16_round(&up).map_err(|e| dev_err("ace attn rnd", e))?
            } else {
                let (k, v) = expand_gqa_dev(k, v, q_heads, kv_heads, hd, group)?;
                let attn = ace_self_attn_dev(&q, &k, &v, q_heads, scale, tokens, devb.sliding)?;
                if skip_pregemm_round {
                    attn
                } else {
                    gpu_bf16_round(&attn).map_err(|e| dev_err("ace dit attn rnd", e))?
                }
            };
            if let Some(cpu) = cpu_attn_ref.as_ref() {
                let gpu_h = gpu_download(&attn).map_err(|e| dev_err("ace cmp attn", e))?;
                ace_dbg_cmp("l0 self_attn vs cpu_gqa", &gpu_h, cpu);
            }
            let mut proj = lin(&attn, &devb.o).map_err(|e| dev_err("ace dit o", e))?;
            if bi == 0 {
                if let Some(path) = pin_self_attn_path() {
                    let data = load_npy_f32(&path)?;
                    if data.len() != tokens * d {
                        return Err(DiffusionError::model(format!(
                            "ACE_PIN_SELF_ATTN {} len {} want {}",
                            path.display(),
                            data.len(),
                            tokens * d
                        )));
                    }
                    proj = gpu_upload(&data, tokens, d).map_err(|e| dev_err("ace pin sa up", e))?;
                    proj = gpu_bf16_round(&proj).map_err(|e| dev_err("ace pin sa rnd", e))?;
                }
                if let Some(dir) = dump_dir.as_ref() {
                    let ah = gpu_download(&attn).map_err(|e| dev_err("ace hook attn", e))?;
                    let ph = gpu_download(&proj).map_err(|e| dev_err("ace hook o", e))?;
                    dump_hook_npy(&dir, "native_step0_l0_attn.npy", &ah, &[1, tokens, qdim]);
                    dump_hook_npy(&dir, "native_step0_l0_self_attn.npy", &ph, &[1, tokens, d]);
                }
            }
            if debug_cmp && bi == 0 {
                let host = gpu_download(&proj).map_err(|e| dev_err("ace dit dbg sa", e))?;
                eprintln!(
                    "cuda l0 self_attn_o[:4]={:?} abs={} sliding={}",
                    &host[..4.min(host.len())],
                    host.iter().fold(0.0f32, |a, v| a.max(v.abs())),
                    devb.sliding
                );
            }
            h = if !ace_env_flag("ACE_RES_SINGLE") && ace_fuse_on("ACE_FUSE_NO_RES") {
                gpu_gated_residual_round_add_bf16(&h, &proj, &mods_g, 2 * d)
                    .map_err(|e| dev_err("ace dit sgate fused", e))?
            } else {
                let gated = gpu_gated_residual_mod(&zeros_tok, &proj, &mods_g, 2 * d)
                    .map_err(|e| dev_err("ace dit sgate mul", e))?;
                let gated = if ace_env_flag("ACE_RES_SINGLE") {
                    gated
                } else {
                    gpu_bf16_round(&gated).map_err(|e| dev_err("ace dit sgate mul rnd", e))?
                };
                // add_bf16 = round_bf16(f32 add): same one-rounding as add + round.
                gpu_add_bf16(&h, &gated).map_err(|e| dev_err("ace dit sgate add", e))?
            };
            if bi == 0 {
                if let Some(dir) = dump_dir.as_ref() {
                    let host = gpu_download(&h).map_err(|e| dev_err("ace hook l0 self", e))?;
                    dump_hook_npy(
                        &dir,
                        "native_step0_l0_hidden_after_selfattn.npy",
                        &host,
                        &[1, tokens, d],
                    );
                }
            }

            let cn = ace_rms_two_store(&h, &block.cross_norm, d, &format!("b{bi}.cn"), ACE_RMS_EPS)?;
            let cq = lin(&cn, &devb.cross_q).map_err(|e| dev_err("ace dit cq", e))?;
            let cq = ace_rms_two_store(&cq, &block.cross_qn, hd, &format!("b{bi}.cqn"), ACE_RMS_EPS)?;
            let computed_kv;
            let (ck, cv) = if let Some(cache) = cross_kv {
                (&cache.layers[bi].0, &cache.layers[bi].1)
            } else {
                let enc = enc.as_ref().ok_or_else(|| {
                    DiffusionError::model("ace dit missing encoder hidden")
                })?;
                let ck = lin(enc, &devb.cross_k).map_err(|e| dev_err("ace dit ck", e))?;
                let cv = lin(enc, &devb.cross_v).map_err(|e| dev_err("ace dit cv", e))?;
                let ck = ace_rms_two_store(&ck, &block.cross_kn, hd, &format!("b{bi}.ckn"), ACE_RMS_EPS)?;
                if bi == 0 {
                    if let Some(dir) = dump_dir.as_ref() {
                        let qh = gpu_download(&cq).map_err(|e| dev_err("ace hook cq", e))?;
                        let kh = gpu_download(&ck).map_err(|e| dev_err("ace hook ck", e))?;
                        let vh = gpu_download(&cv).map_err(|e| dev_err("ace hook cv", e))?;
                        dump_hook_npy(&dir, "native_step0_l0_cross_q.npy", &qh, &[1, tokens, q_heads, hd]);
                        dump_hook_npy(&dir, "native_step0_l0_cross_k.npy", &kh, &[1, encoder.tokens, kv_heads, hd]);
                        dump_hook_npy(&dir, "native_step0_l0_cross_v.npy", &vh, &[1, encoder.tokens, kvdim]);
                    }
                }
                computed_kv = expand_gqa_dev(ck, cv, q_heads, kv_heads, hd, group)?;
                (&computed_kv.0, &computed_kv.1)
            };
            if bi == 0 && cross_kv.is_some() {
                if let Some(dir) = dump_dir.as_ref() {
                    let qh = gpu_download(&cq).map_err(|e| dev_err("ace hook cq", e))?;
                    dump_hook_npy(&dir, "native_step0_l0_cross_q.npy", &qh, &[1, tokens, q_heads, hd]);
                }
            }
            let cpu_cross_ref = if debug_cmp && bi == 0 && !attn_cpu {
                let qh = gpu_download(&cq).map_err(|e| dev_err("ace ccmp q", e))?;
                let kh = gpu_download(ck).map_err(|e| dev_err("ace ccmp k", e))?;
                let vh = gpu_download(cv).map_err(|e| dev_err("ace ccmp v", e))?;
                Some(ace_gqa_attention(
                    &qh, &kh, &vh, q_heads, kv_heads, hd, tokens, encoder.tokens, None, false, None,
                ))
            } else {
                None
            };
            let cattn = if attn_cpu {
                let qh = gpu_download(&cq).map_err(|e| dev_err("ace cattn q", e))?;
                let kh = gpu_download(ck).map_err(|e| dev_err("ace cattn k", e))?;
                let vh = gpu_download(cv).map_err(|e| dev_err("ace cattn v", e))?;
                let host = ace_gqa_attention(
                    &qh, &kh, &vh, q_heads, kv_heads, hd, tokens, encoder.tokens, None, false, None,
                );
                let up = gpu_upload(&host, tokens, qdim).map_err(|e| dev_err("ace cattn up", e))?;
                gpu_bf16_round(&up).map_err(|e| dev_err("ace cattn rnd", e))?
            } else {
                let cattn = ace_cross_attn_dev(&cq, ck, cv, q_heads, scale, tokens, enc_tokens)?;
                if skip_pregemm_round {
                    cattn
                } else {
                    gpu_bf16_round(&cattn).map_err(|e| dev_err("ace dit cross rnd", e))?
                }
            };
            if let Some(cpu) = cpu_cross_ref.as_ref() {
                let gpu_h = gpu_download(&cattn).map_err(|e| dev_err("ace ccmp attn", e))?;
                ace_dbg_cmp("l0 cross_attn vs cpu_gqa", &gpu_h, cpu);
            }
            let mut cproj = lin(&cattn, &devb.cross_o).map_err(|e| dev_err("ace dit co", e))?;
            if bi == 0 {
                if let Some(path) = pin_cross_attn_path() {
                    let data = load_npy_f32(&path)?;
                    if data.len() != tokens * d {
                        return Err(DiffusionError::model(format!(
                            "ACE_PIN_CROSS_ATTN {} len {} want {}",
                            path.display(),
                            data.len(),
                            tokens * d
                        )));
                    }
                    cproj = gpu_upload(&data, tokens, d).map_err(|e| dev_err("ace pin ca up", e))?;
                    cproj = gpu_bf16_round(&cproj).map_err(|e| dev_err("ace pin ca rnd", e))?;
                }
                if let Some(dir) = dump_dir.as_ref() {
                    let ph = gpu_download(&cproj).map_err(|e| dev_err("ace hook co", e))?;
                    dump_hook_npy(&dir, "native_step0_l0_cross_attn.npy", &ph, &[1, tokens, d]);
                }
            }
            h = gpu_add_bf16(&h, &cproj).map_err(|e| dev_err("ace dit cadd", e))?;
            if bi == 0 {
                if let Some(dir) = dump_dir.as_ref() {
                    let host = gpu_download(&h).map_err(|e| dev_err("ace hook l0 xadd", e))?;
                    dump_hook_npy(
                        &dir,
                        "native_step0_l0_hidden_after_crossattn.npy",
                        &host,
                        &[1, tokens, d],
                    );
                }
            }

            let rms_mlp = ace_rms_two_store(&h, &block.mlp_norm, d, &format!("b{bi}.mn"), ACE_RMS_EPS)?;
            let fnorm = ace_adaln_stores(
                &rms_mlp, &mods_g, 4 * d, 3 * d, d, &zeros_tok, &ones_tok, &ones_row,
            )?;
            if bi == 0 {
                if let Some(dir) = dump_dir.as_ref() {
                    let host = gpu_download(&fnorm).map_err(|e| dev_err("ace hook mlp n", e))?;
                    dump_hook_npy(&dir, "native_step0_l0_mlp_normed.npy", &host, &[1, tokens, d]);
                }
            }
            let fused_ffn = std::env::var("ACE_FUSED_FFN").ok().as_deref() == Some("1");
            let down = if fused_ffn {
                let gu = lin(&fnorm, &devb.gate_up).map_err(|e| dev_err("ace dit gu", e))?;
                let act = gpu_swiglu_value_gate(&gu).map_err(|e| dev_err("ace dit swiglu", e))?;
                let act = gpu_bf16_round(&act).map_err(|e| dev_err("ace dit swiglu rnd", e))?;
                lin(&act, &devb.down).map_err(|e| dev_err("ace dit down", e))?
            } else {
                let gate = lin(&fnorm, &devb.gate).map_err(|e| dev_err("ace dit gate", e))?;
                let up = lin(&fnorm, &devb.up).map_err(|e| dev_err("ace dit up", e))?;
                if bi == 0 {
                    if let Some(dir) = dump_dir.as_ref() {
                        let gh = gpu_download(&gate).map_err(|e| dev_err("ace hook mlp gate", e))?;
                        let uh = gpu_download(&up).map_err(|e| dev_err("ace hook mlp up", e))?;
                        dump_hook_npy(&dir, "native_step0_l0_mlp_gate.npy", &gh, &[1, tokens, ACE_DIT_FFN]);
                        dump_hook_npy(&dir, "native_step0_l0_mlp_up.npy", &uh, &[1, tokens, ACE_DIT_FFN]);
                    }
                }
                let act = if ace_fuse_on("ACE_FUSE_NO_SWIGLU") {
                    gpu_silu_round_mul_round_bf16(&gate, &up)
                        .map_err(|e| dev_err("ace dit swiglu fused", e))?
                } else {
                    let act = gpu_silu(&gate).map_err(|e| dev_err("ace dit silu", e))?;
                    let act = gpu_bf16_round(&act).map_err(|e| dev_err("ace dit silu rnd", e))?;
                    let act = gpu_mul(&act, &up).map_err(|e| dev_err("ace dit swiglu mul", e))?;
                    gpu_bf16_round(&act).map_err(|e| dev_err("ace dit swiglu mul rnd", e))?
                };
                if bi == 0 {
                    if let Some(dir) = dump_dir.as_ref() {
                        let ah = gpu_download(&act).map_err(|e| dev_err("ace hook mlp act", e))?;
                        dump_hook_npy(&dir, "native_step0_l0_mlp_act.npy", &ah, &[1, tokens, ACE_DIT_FFN]);
                    }
                }
                lin(&act, &devb.down).map_err(|e| dev_err("ace dit down", e))?
            };
            if bi == 0 {
                if let Some(dir) = dump_dir.as_ref() {
                    let host = gpu_download(&down).map_err(|e| dev_err("ace hook mlp", e))?;
                    dump_hook_npy(&dir, "native_step0_l0_mlp.npy", &host, &[1, tokens, d]);
                }
            }
            h = if !ace_env_flag("ACE_RES_SINGLE") && ace_fuse_on("ACE_FUSE_NO_RES") {
                gpu_gated_residual_round_add_bf16(&h, &down, &mods_g, 5 * d)
                    .map_err(|e| dev_err("ace dit mgate fused", e))?
            } else {
                let gated_m = gpu_gated_residual_mod(&zeros_tok, &down, &mods_g, 5 * d)
                    .map_err(|e| dev_err("ace dit mgate mul", e))?;
                let gated_m = if ace_env_flag("ACE_RES_SINGLE") {
                    gated_m
                } else {
                    gpu_bf16_round(&gated_m).map_err(|e| dev_err("ace dit mgate mul rnd", e))?
                };
                gpu_add_bf16(&h, &gated_m).map_err(|e| dev_err("ace dit mgate add", e))?
            };
            if bi == 0 {
                if let Some(dir) = dump_dir.as_ref() {
                    let host = gpu_download(&h).map_err(|e| dev_err("ace hook l0 out", e))?;
                    dump_hook_npy(&dir, "native_step0_l0_out.npy", &host, &[1, tokens, d]);
                }
            }
            if debug_cmp && bi < 2 {
                let host = gpu_download(&h).map_err(|e| dev_err("ace dit dbg l", e))?;
                eprintln!(
                    "cuda l{bi}[:4]={:?} abs={} sliding={}",
                    &host[..4.min(host.len())],
                    host.iter().fold(0.0f32, |a, v| a.max(v.abs())),
                    devb.sliding
                );
            }
        }

        let head_sst = ace_fwd_slot(
            |c| &mut c.head_sst,
            || {
                let head_sst = gpu_upload(&self.scale_shift, 1, 2 * d)
                    .map_err(|e| dev_err("ace dit hout sst", e))?;
                gpu_bf16_round(&head_sst).map_err(|e| dev_err("ace dit hout sst rnd", e))
            },
        )?;
        let head_g = ace_fwd_slot(
            |c| &mut c.head_g,
            || {
                let temb2 =
                    gpu_concat_cols(&[&temb, &temb]).map_err(|e| dev_err("ace dit temb2", e))?;
                let head_g = gpu_add(&head_sst, &temb2).map_err(|e| dev_err("ace dit hout m", e))?;
                gpu_bf16_round(&head_g).map_err(|e| dev_err("ace dit hout m rnd", e))
            },
        )?;
        let w_out = ace_fwd_slot(
            |c| &mut c.w_out,
            || gpu_upload(&self.norm_out, 1, d).map_err(|e| dev_err("ace dit nout", e)),
        )?;
        let hout = gpu_rms_norm_mod_indexed(
            &h, &w_out, &head_g, &idx, 2 * d, d, 0, ACE_RMS_EPS, false,
        )
        .map_err(|e| dev_err("ace dit hout", e))?;
        let e = lin_b(&hout, &device.proj_out0, &self.proj_out_b)
            .map_err(|e| dev_err("ace dit pout0", e))?;
        let o = lin(&hout, &device.proj_out1).map_err(|e| dev_err("ace dit pout1", e))?;
        // Interleave even/odd frames: one [rows, 128] download, stitched on host.
        // (Column-concat then a single D2H is the same bytes as the old two
        // downloads, with one stream sync instead of two.)
        let eo = gpu_concat_cols(&[&e, &o]).map_err(|e| dev_err("ace dit eo cat", e))?;
        let eo_y = gpu_download(&eo).map_err(|e| dev_err("ace dit eo", e))?;
        let mut y = vec![0f32; batch * padded * ACE_LATENT_DIM];
        for b in 0..batch {
            let dst = b * padded * ACE_LATENT_DIM;
            for i in 0..tokens {
                let src = (b * tokens + i) * 2 * ACE_LATENT_DIM;
                y[dst + (2 * i) * ACE_LATENT_DIM..dst + (2 * i + 1) * ACE_LATENT_DIM]
                    .copy_from_slice(&eo_y[src..src + ACE_LATENT_DIM]);
                y[dst + (2 * i + 1) * ACE_LATENT_DIM..dst + (2 * i + 2) * ACE_LATENT_DIM]
                    .copy_from_slice(&eo_y[src + ACE_LATENT_DIM..src + 2 * ACE_LATENT_DIM]);
            }
        }
        if batch == 1 {
            y.truncate(original * ACE_LATENT_DIM);
        } else {
            let mut packed = vec![0f32; 2 * original * ACE_LATENT_DIM];
            packed[..original * ACE_LATENT_DIM]
                .copy_from_slice(&y[..original * ACE_LATENT_DIM]);
            packed[original * ACE_LATENT_DIM..]
                .copy_from_slice(&y[padded * ACE_LATENT_DIM..padded * ACE_LATENT_DIM + original * ACE_LATENT_DIM]);
            y = packed;
        }
        if let Some(dir) = dump_dir.as_ref() {
            dump_hook_npy(
                &dir,
                "native_step0_vt_cond.npy",
                &y,
                &[1, original, ACE_LATENT_DIM],
            );
        }
        Ok(y)
    }
}

/// BF16 weight bytes for `gpu_linear_nt_cached_bf16_f32acc` — official ACE
/// dumps were produced in bf16, and f16 GEMM was distorting the APG residual.
struct Bf16Weight {
    n: usize,
    key: String,
    bytes: Vec<u8>,
}

fn f32_to_bf16_word(v: f32) -> u16 {
    let bits = v.to_bits();
    let rounding_bias = 0x7FFFu32 + ((bits >> 16) & 1);
    ((bits + rounding_bias) >> 16) as u16
}

impl Bf16Weight {
    fn new(key: impl Into<String>, w: &[f32], n: usize, k: usize) -> Self {
        debug_assert_eq!(w.len(), n * k);
        let mut bytes = Vec::with_capacity(w.len() * 2);
        for &v in w {
            bytes.extend_from_slice(&f32_to_bf16_word(v).to_le_bytes());
        }
        Self {
            n,
            key: key.into(),
            bytes,
        }
    }

    fn part(&self) -> makepad_ggml::backend::cuda::GpuLinearPart<'_> {
        makepad_ggml::backend::cuda::GpuLinearPart {
            bt_ggml_type: makepad_ggml::quant::GGML_TYPE_BF16,
            n: self.n,
            cache_key: &self.key,
            bytes: &self.bytes,
        }
    }
}

struct EncDevLayer {
    q: Bf16Weight,
    k: Bf16Weight,
    v: Bf16Weight,
    o: Bf16Weight,
    gate: Bf16Weight,
    up: Bf16Weight,
    down: Bf16Weight,
    sliding: bool,
}

pub struct AceCondDevice {
    text_proj: Bf16Weight,
    lyric_embed: Bf16Weight,
    lyric_embed_b: Vec<f32>,
    timbre_embed: Bf16Weight,
    timbre_embed_b: Vec<f32>,
    lyric_layers: Vec<EncDevLayer>,
    timbre_layers: Vec<EncDevLayer>,
}

struct DitDevBlock {
    q: Bf16Weight,
    k: Bf16Weight,
    v: Bf16Weight,
    qkv: Bf16Weight,
    o: Bf16Weight,
    cross_q: Bf16Weight,
    cross_k: Bf16Weight,
    cross_v: Bf16Weight,
    cross_o: Bf16Weight,
    gate: Bf16Weight,
    up: Bf16Weight,
    gate_up: Bf16Weight,
    down: Bf16Weight,
    sliding: bool,
}

struct TimeEmbedDev {
    linear1: Bf16Weight,
    linear2: Bf16Weight,
    time_proj: Bf16Weight,
}

fn time_embed_dev(prefix: &str, embed: &TimeEmbed, dim: usize) -> TimeEmbedDev {
    TimeEmbedDev {
        linear1: Bf16Weight::new(
            format!("{prefix}.l1"),
            &embed.linear1_w,
            dim,
            ACE_TIME_FREQ_DIM,
        ),
        linear2: Bf16Weight::new(format!("{prefix}.l2"), &embed.linear2_w, dim, dim),
        time_proj: Bf16Weight::new(format!("{prefix}.tp"), &embed.time_proj_w, 6 * dim, dim),
    }
}

pub struct AceDitDevice {
    proj_in: Bf16Weight,
    cond_embed: Bf16Weight,
    proj_out0: Bf16Weight,
    proj_out1: Bf16Weight,
    time: TimeEmbedDev,
    time_r: TimeEmbedDev,
    blocks: Vec<DitDevBlock>,
    /// Distinguishes device instances in the thread-local forward cache.
    cache_id: u64,
}

/// Per-layer expanded cross-attn K/V. Encoder-only; reuse across denoise steps.
pub struct AceCrossKvCache {
    layers: Vec<(
        makepad_ggml::backend::cuda::GpuTensor,
        makepad_ggml::backend::cuda::GpuTensor,
    )>,
    enc_tokens: usize,
}

fn ace_layer_cross_kv(
    enc: &makepad_ggml::backend::cuda::GpuTensor,
    block: &DitBlock,
    devb: &DitDevBlock,
    bi: usize,
    q_heads: usize,
    kv_heads: usize,
    hd: usize,
    group: usize,
) -> Result<(
    makepad_ggml::backend::cuda::GpuTensor,
    makepad_ggml::backend::cuda::GpuTensor,
)> {
    use crate::sa3::dev_err;
    use makepad_ggml::backend::cuda::gpu_linear_nt_cached_bf16_mm;
    let ck = gpu_linear_nt_cached_bf16_mm(enc, "acedit", &[devb.cross_k.part()])
        .map_err(|e| dev_err("ace kv ck", e))?;
    let cv = gpu_linear_nt_cached_bf16_mm(enc, "acedit", &[devb.cross_v.part()])
        .map_err(|e| dev_err("ace kv cv", e))?;
    let ck = ace_rms_two_store(&ck, &block.cross_kn, hd, &format!("b{bi}.ckn"), ACE_RMS_EPS)?;
    expand_gqa_dev(ck, cv, q_heads, kv_heads, hd, group)
}

fn expand_gqa_dev(
    k: makepad_ggml::backend::cuda::GpuTensor,
    v: makepad_ggml::backend::cuda::GpuTensor,
    q_heads: usize,
    kv_heads: usize,
    hd: usize,
    group: usize,
) -> Result<(makepad_ggml::backend::cuda::GpuTensor, makepad_ggml::backend::cuda::GpuTensor)> {
    use crate::sa3::dev_err;
    use makepad_ggml::backend::cuda::gpu_gather_cols;
    if group <= 1 {
        return Ok((k, v));
    }
    let mut idx = Vec::with_capacity(q_heads * hd);
    for head in 0..q_heads {
        let kv_head = head / group;
        let base = (kv_head * hd) as u32;
        for i in 0..hd {
            idx.push(base + i as u32);
        }
    }
    let _ = kv_heads;
    let k_full = gpu_gather_cols(&k, &idx).map_err(|e| dev_err("ace gqa k", e))?;
    let v_full = gpu_gather_cols(&v, &idx).map_err(|e| dev_err("ace gqa v", e))?;
    Ok((k_full, v_full))
}

fn ace_self_attn_dev(
    q: &makepad_ggml::backend::cuda::GpuTensor,
    k: &makepad_ggml::backend::cuda::GpuTensor,
    v: &makepad_ggml::backend::cuda::GpuTensor,
    heads: usize,
    scale: f32,
    tokens: usize,
    sliding: bool,
) -> Result<makepad_ggml::backend::cuda::GpuTensor> {
    use crate::sa3::dev_err;
    use makepad_ggml::backend::cuda::{
        gpu_attention_packed_composite_bf16, gpu_attention_packed_f32,
        gpu_attention_packed_sliding, gpu_attention_packed_sliding_bf16, gpu_concat_rows,
        gpu_slice_rows,
    };
    let attn_bf16 = !ace_env_flag("ACE_ATTN_F32");
    let one = |q: &_, k: &_, v: &_| -> Result<_> {
        let attn = if sliding {
            if attn_bf16 {
                gpu_attention_packed_sliding_bf16(q, k, v, heads, scale, ACE_SLIDING_WINDOW)
                    .map_err(|e| dev_err("ace self slide bf16", e))?
            } else {
                gpu_attention_packed_sliding(q, k, v, heads, scale, ACE_SLIDING_WINDOW)
                    .map_err(|e| dev_err("ace self slide", e))?
            }
        } else if attn_bf16 {
            gpu_attention_packed_composite_bf16(q, k, v, heads, scale)
                .map_err(|e| dev_err("ace self full bf16", e))?
        } else {
            gpu_attention_packed_f32(q, k, v, heads, scale).map_err(|e| dev_err("ace self full", e))?
        };
        Ok(attn)
    };
    if q.rows() == tokens {
        return one(q, k, v);
    }
    if q.rows() != 2 * tokens || k.rows() != 2 * tokens || v.rows() != 2 * tokens {
        return Err(DiffusionError::model("ace self attn batch rows"));
    }
    let q0 = gpu_slice_rows(q, 0, tokens).map_err(|e| dev_err("ace self q0", e))?;
    let q1 = gpu_slice_rows(q, tokens, tokens).map_err(|e| dev_err("ace self q1", e))?;
    let k0 = gpu_slice_rows(k, 0, tokens).map_err(|e| dev_err("ace self k0", e))?;
    let k1 = gpu_slice_rows(k, tokens, tokens).map_err(|e| dev_err("ace self k1", e))?;
    let v0 = gpu_slice_rows(v, 0, tokens).map_err(|e| dev_err("ace self v0", e))?;
    let v1 = gpu_slice_rows(v, tokens, tokens).map_err(|e| dev_err("ace self v1", e))?;
    let a0 = one(&q0, &k0, &v0)?;
    let a1 = one(&q1, &k1, &v1)?;
    gpu_concat_rows(&a0, &a1).map_err(|e| dev_err("ace self cat", e))
}

fn ace_cross_attn_dev(
    q: &makepad_ggml::backend::cuda::GpuTensor,
    k: &makepad_ggml::backend::cuda::GpuTensor,
    v: &makepad_ggml::backend::cuda::GpuTensor,
    heads: usize,
    scale: f32,
    q_tokens: usize,
    kv_tokens: usize,
) -> Result<makepad_ggml::backend::cuda::GpuTensor> {
    use crate::sa3::dev_err;
    use makepad_ggml::backend::cuda::{
        gpu_attention_packed_cross_composite_bf16, gpu_attention_packed_f32, gpu_concat_rows,
        gpu_slice_rows,
    };
    let attn_bf16 = !ace_env_flag("ACE_ATTN_F32");
    let one = |q: &_, k: &_, v: &_| -> Result<_> {
        let attn = if attn_bf16 {
            gpu_attention_packed_cross_composite_bf16(q, k, v, heads, scale)
                .map_err(|e| dev_err("ace cross bf16", e))?
        } else {
            gpu_attention_packed_f32(q, k, v, heads, scale).map_err(|e| dev_err("ace cross f32", e))?
        };
        Ok(attn)
    };
    if q.rows() == q_tokens {
        return one(q, k, v);
    }
    if q.rows() != 2 * q_tokens || k.rows() != 2 * kv_tokens || v.rows() != 2 * kv_tokens {
        return Err(DiffusionError::model("ace cross attn batch rows"));
    }
    let q0 = gpu_slice_rows(q, 0, q_tokens).map_err(|e| dev_err("ace cross q0", e))?;
    let q1 = gpu_slice_rows(q, q_tokens, q_tokens).map_err(|e| dev_err("ace cross q1", e))?;
    let k0 = gpu_slice_rows(k, 0, kv_tokens).map_err(|e| dev_err("ace cross k0", e))?;
    let k1 = gpu_slice_rows(k, kv_tokens, kv_tokens).map_err(|e| dev_err("ace cross k1", e))?;
    let v0 = gpu_slice_rows(v, 0, kv_tokens).map_err(|e| dev_err("ace cross v0", e))?;
    let v1 = gpu_slice_rows(v, kv_tokens, kv_tokens).map_err(|e| dev_err("ace cross v1", e))?;
    let a0 = one(&q0, &k0, &v0)?;
    let a1 = one(&q1, &k1, &v1)?;
    gpu_concat_rows(&a0, &a1).map_err(|e| dev_err("ace cross cat", e))
}

fn sliding_attn_dev(
    q: &makepad_ggml::backend::cuda::GpuTensor,
    k: &makepad_ggml::backend::cuda::GpuTensor,
    v: &makepad_ggml::backend::cuda::GpuTensor,
    heads: usize,
    scale: f32,
    window: usize,
) -> Result<makepad_ggml::backend::cuda::GpuTensor> {
    use crate::sa3::dev_err;
    use makepad_ggml::backend::cuda::{
        gpu_attention_packed, gpu_attention_packed_cross, gpu_concat_rows, gpu_slice_rows,
        GpuTensor,
    };
    let seq = q.rows();
    if seq == 0 {
        return Err(DiffusionError::model("ace sliding: empty"));
    }
    // Queries that see every key: i-W<=0 and i+W>=seq-1 → i ∈ [seq-1-W, W].
    let first_full = seq.saturating_sub(1).saturating_sub(window);
    let last_full = window.min(seq.saturating_sub(1));
    if first_full > last_full || window + 1 >= seq {
        return gpu_attention_packed(q, k, v, heads, scale).map_err(|e| dev_err("ace slide full", e));
    }

    let mut parts: Vec<GpuTensor> = Vec::new();
    for i in 0..first_full {
        let k1 = (i + window + 1).min(seq);
        let qi = gpu_slice_rows(q, i, 1).map_err(|e| dev_err("ace sl q", e))?;
        let ki = gpu_slice_rows(k, 0, k1).map_err(|e| dev_err("ace sl k", e))?;
        let vi = gpu_slice_rows(v, 0, k1).map_err(|e| dev_err("ace sl v", e))?;
        parts.push(
            gpu_attention_packed_cross(&qi, &ki, &vi, heads, scale)
                .map_err(|e| dev_err("ace sl edge", e))?,
        );
    }
    let mid_n = last_full + 1 - first_full;
    let qmid = gpu_slice_rows(q, first_full, mid_n).map_err(|e| dev_err("ace sl qm", e))?;
    parts.push(
        gpu_attention_packed_cross(&qmid, k, v, heads, scale).map_err(|e| dev_err("ace sl mid", e))?,
    );
    for i in (last_full + 1)..seq {
        let k0 = i.saturating_sub(window);
        let qi = gpu_slice_rows(q, i, 1).map_err(|e| dev_err("ace sl q2", e))?;
        let ki = gpu_slice_rows(k, k0, seq - k0).map_err(|e| dev_err("ace sl k2", e))?;
        let vi = gpu_slice_rows(v, k0, seq - k0).map_err(|e| dev_err("ace sl v2", e))?;
        parts.push(
            gpu_attention_packed_cross(&qi, &ki, &vi, heads, scale)
                .map_err(|e| dev_err("ace sl edge2", e))?,
        );
    }
    let mut acc = parts.remove(0);
    for p in parts {
        acc = gpu_concat_rows(&acc, &p).map_err(|e| dev_err("ace sl cat", e))?;
    }
    Ok(acc)
}
