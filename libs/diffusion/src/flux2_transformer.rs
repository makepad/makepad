//! FLUX.2 transformer: klein-4B (guidance_embed = false) and dev-32B
//! (guidance_embed = true, `guidance_in` MLPEmbedder summed into the time
//! vector exactly like the reference `vec += guidance_in(timestep_embedding
//! (guidance, 256))`).
//!
//! Global modulation (three shared SiLU+Linear modules computed once per
//! step), bias-free linears, SwiGLU (gate-first), 4-axis interleaved RoPE
//! at theta 2000, QK RMSNorm. Reuses flux1/h3 device kernels.
//!
//! Dev fp8mixed weights (Comfy `flux2_dev_fp8mixed.safetensors`): the double
//! mlps and single linear1/linear2 are F8_E4M3 with a scalar f32
//! `.weight_scale` sibling; they stay 1-byte resident in the device cache
//! (the dense backend dequantizes into transient bf16 scratch per call) so
//! the 32B DiT fits a 32GB card.

use crate::backend::{
    gpu_add_bf16, gpu_attention_packed_composite_f32, gpu_attention_packed_flash_cross,
    gpu_attention_packed_flash_cross_bf16_rn, gpu_attention_packed_flash_cross_bf16pre_f16,
    gpu_bf16_round, gpu_bf16buf_slab_to_f32, gpu_concat_f32rn_bf16buf, gpu_concat_rows,
    gpu_device_available, gpu_download, gpu_gated_residual_mod_round_bf16, gpu_layer_norm_mod,
    gpu_layer_norm_mod_to_bf16buf, gpu_linear_nt_cached_bf16_mm,
    gpu_linear_nt_cached_bf16_mm_from_buf, gpu_linear_nt_cached_bf16_mm_from_buf_to_buf,
    gpu_linear_nt_cached_f8_mm, gpu_linear_nt_cached_f8_mm_from_buf,
    gpu_linear_nt_cached_f8_mm_from_buf_to_buf,
    gpu_pool_clear, gpu_rms_norm_mul_from_bf16_slab, gpu_rope_interleaved, gpu_silu,
    gpu_slice_rows, gpu_swiglu_gate_first_from_bf16, gpu_upload, gpu_upload_into,
    gpu_weight_cache_ensure,
    gpu_weight_cache_evict_prefix, GpuBf16Buf, GpuLinearPart, GpuTensor,
};
use crate::flux2::{Flux2PosId, Flux2TransformerConfig, Flux2WeightFile};
use crate::{DiffusionError, Result};
use makepad_ggml::quant::{GGML_TYPE_BF16, GGML_TYPE_F8_E4M3};
use std::path::{Path, PathBuf};

pub const FLUX2_DIT_NAMESPACE: &str = "flux2-dit-bf16";
const LAYER_NORM_EPS: f32 = 1.0e-6;
const TIME_EMBED_DIM: usize = 256;

pub struct Flux2TransformerWeights {
    pub path: PathBuf,
    pub config: Flux2TransformerConfig,
    pub file: Flux2WeightFile,
    /// Lazily-read `<name>_scale` values of the fp8mixed checkpoint, cached
    /// so the per-step ensure path never re-opens the weight file for a
    /// 4-byte scalar.
    f8_scales: std::cell::RefCell<std::collections::HashMap<String, f32>>,
}

impl Flux2TransformerWeights {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file = Flux2WeightFile::load(&path)?;
        let config = file.detect_transformer_config()?;
        if config.guidance_embed
            && !file.has_tensor("guidance_in.in_layer.weight")
            && !file.has_tensor("time_guidance_embed.guidance_embedder.linear_1.weight")
        {
            return Err(DiffusionError::model(
                "flux2 dit config wants guidance_in but the file has none",
            ));
        }
        Ok(Self {
            path,
            config,
            file,
            f8_scales: std::cell::RefCell::new(std::collections::HashMap::new()),
        })
    }

    fn f8_scale(&self, resolved: &str) -> Result<f32> {
        if let Some(scale) = self.f8_scales.borrow().get(resolved) {
            return Ok(*scale);
        }
        let scale_name = format!("{resolved}_scale");
        let scale = if self.file.has_tensor(&scale_name) {
            self.file.read_f32(&scale_name)?.first().copied().unwrap_or(1.0)
        } else {
            1.0
        };
        self.f8_scales
            .borrow_mut()
            .insert(resolved.to_owned(), scale);
        Ok(scale)
    }
}

pub fn flux2_transformer_release() -> Result<usize> {
    gpu_weight_cache_evict_prefix(&format!("{FLUX2_DIT_NAMESPACE}::")).map_err(DiffusionError::model)
}

fn ns(weights: &Flux2TransformerWeights) -> String {
    format!("{FLUX2_DIT_NAMESPACE}:{}", weights.path.display())
}

fn alias_one(name: &str) -> Option<String> {
    match name {
        "img_in.weight" => Some("x_embedder.weight".into()),
        "txt_in.weight" => Some("context_embedder.weight".into()),
        "time_in.in_layer.weight" => {
            Some("time_guidance_embed.timestep_embedder.linear_1.weight".into())
        }
        "time_in.out_layer.weight" => {
            Some("time_guidance_embed.timestep_embedder.linear_2.weight".into())
        }
        "guidance_in.in_layer.weight" => {
            Some("time_guidance_embed.guidance_embedder.linear_1.weight".into())
        }
        "guidance_in.out_layer.weight" => {
            Some("time_guidance_embed.guidance_embedder.linear_2.weight".into())
        }
        "double_stream_modulation_img.lin.weight" => {
            Some("double_stream_modulation_img.linear.weight".into())
        }
        "double_stream_modulation_txt.lin.weight" => {
            Some("double_stream_modulation_txt.linear.weight".into())
        }
        "single_stream_modulation.lin.weight" => {
            Some("single_stream_modulation.linear.weight".into())
        }
        "final_layer.adaLN_modulation.1.weight" => Some("norm_out.linear.weight".into()),
        "final_layer.linear.weight" => Some("proj_out.weight".into()),
        other => alias_block(other),
    }
}

fn alias_block(name: &str) -> Option<String> {
    if let Some(rest) = name.strip_prefix("double_blocks.") {
        let (idx, rest) = rest.split_once('.')?;
        let mapped = match rest {
            "img_attn.proj.weight" => format!("transformer_blocks.{idx}.attn.to_out.0.weight"),
            "txt_attn.proj.weight" => format!("transformer_blocks.{idx}.attn.to_add_out.weight"),
            "img_attn.norm.query_norm.scale" => {
                format!("transformer_blocks.{idx}.attn.norm_q.weight")
            }
            "img_attn.norm.key_norm.scale" => {
                format!("transformer_blocks.{idx}.attn.norm_k.weight")
            }
            "txt_attn.norm.query_norm.scale" => {
                format!("transformer_blocks.{idx}.attn.norm_added_q.weight")
            }
            "txt_attn.norm.key_norm.scale" => {
                format!("transformer_blocks.{idx}.attn.norm_added_k.weight")
            }
            "img_mlp.0.weight" => format!("transformer_blocks.{idx}.ff.linear_in.weight"),
            "img_mlp.2.weight" => format!("transformer_blocks.{idx}.ff.linear_out.weight"),
            "txt_mlp.0.weight" => format!("transformer_blocks.{idx}.ff_context.linear_in.weight"),
            "txt_mlp.2.weight" => format!("transformer_blocks.{idx}.ff_context.linear_out.weight"),
            _ => return None,
        };
        return Some(mapped);
    }
    if let Some(rest) = name.strip_prefix("single_blocks.") {
        let (idx, rest) = rest.split_once('.')?;
        let mapped = match rest {
            "linear1.weight" => {
                format!("single_transformer_blocks.{idx}.attn.to_qkv_mlp_proj.weight")
            }
            "linear2.weight" => format!("single_transformer_blocks.{idx}.attn.to_out.weight"),
            "norm.query_norm.scale" => {
                format!("single_transformer_blocks.{idx}.attn.norm_q.weight")
            }
            "norm.key_norm.scale" => format!("single_transformer_blocks.{idx}.attn.norm_k.weight"),
            _ => return None,
        };
        return Some(mapped);
    }
    None
}

fn alias_fused(name: &str) -> Option<[String; 3]> {
    let rest = name.strip_prefix("double_blocks.")?;
    let (idx, rest) = rest.split_once('.')?;
    match rest {
        "img_attn.qkv.weight" => Some([
            format!("transformer_blocks.{idx}.attn.to_q.weight"),
            format!("transformer_blocks.{idx}.attn.to_k.weight"),
            format!("transformer_blocks.{idx}.attn.to_v.weight"),
        ]),
        "txt_attn.qkv.weight" => Some([
            format!("transformer_blocks.{idx}.attn.add_q_proj.weight"),
            format!("transformer_blocks.{idx}.attn.add_k_proj.weight"),
            format!("transformer_blocks.{idx}.attn.add_v_proj.weight"),
        ]),
        _ => None,
    }
}

fn load_linear_bytes(weights: &Flux2TransformerWeights, name: &str) -> Result<Vec<u8>> {
    if weights.file.has_tensor(name) {
        return weights.file.read_bf16_bytes(name);
    }
    let prefixed = format!("transformer.{name}");
    if weights.file.has_tensor(&prefixed) {
        return weights.file.read_bf16_bytes(&prefixed);
    }
    if let Some(alias) = alias_one(name) {
        if weights.file.has_tensor(&alias) {
            return weights.file.read_bf16_bytes(&alias);
        }
    }
    if let Some(parts) = alias_fused(name) {
        let mut out = Vec::new();
        for part in &parts {
            if !weights.file.has_tensor(part) {
                return Err(DiffusionError::model(format!(
                    "flux2 dit missing fused part {part} for {name}"
                )));
            }
            out.extend(weights.file.read_bf16_bytes(part)?);
        }
        return Ok(out);
    }
    Err(DiffusionError::model(format!("flux2 dit missing {name}")))
}

fn final_adaln_scale_first(weights: &Flux2TransformerWeights) -> bool {
    weights.file.has_tensor("norm_out.linear.weight")
        && !weights.file.has_tensor("final_layer.adaLN_modulation.1.weight")
}

/// Resolve the on-file tensor name for a canonical name (direct, then
/// `transformer.` prefixed, then the diffusers alias).
fn resolve_name(weights: &Flux2TransformerWeights, name: &str) -> Option<String> {
    if weights.file.has_tensor(name) {
        return Some(name.to_owned());
    }
    let prefixed = format!("transformer.{name}");
    if weights.file.has_tensor(&prefixed) {
        return Some(prefixed);
    }
    alias_one(name).filter(|alias| weights.file.has_tensor(alias))
}

/// The fp8mixed checkpoint's per-tensor dequant scale (`<name>_scale`
/// sibling); `None` when the tensor is stored bf16 (or has no scale, which
/// the raw e4m3 mapping treats as 1.0 — pinned: the Comfy dev file always
/// carries scales on its fp8 tensors).
struct EnsuredLinear<'a> {
    part: GpuLinearPart<'a>,
    f8_scale: Option<f32>,
}

fn ensure_linear<'a>(
    weights: &'a Flux2TransformerWeights,
    name: &'a str,
    output_cols: usize,
    input_cols: usize,
) -> Result<EnsuredLinear<'a>> {
    let f8_scale = match resolve_name(weights, name) {
        Some(resolved) if weights.file.tensor(&resolved)?.dtype == "F8_E4M3" => {
            let scale = weights.f8_scale(&resolved)?;
            gpu_weight_cache_ensure(
                &ns(weights),
                name,
                GGML_TYPE_F8_E4M3,
                output_cols,
                input_cols,
                false,
                || {
                    weights
                        .file
                        .read_bytes(&resolved)
                        .map_err(|err| err.to_string())
                },
            )
            .map_err(DiffusionError::model)?;
            Some(scale)
        }
        _ => {
            gpu_weight_cache_ensure(
                &ns(weights),
                name,
                GGML_TYPE_BF16,
                output_cols,
                input_cols,
                false,
                || load_linear_bytes(weights, name).map_err(|err| err.to_string()),
            )
            .map_err(DiffusionError::model)?;
            None
        }
    };
    Ok(EnsuredLinear {
        part: GpuLinearPart {
            bt_ggml_type: if f8_scale.is_some() {
                GGML_TYPE_F8_E4M3
            } else {
                GGML_TYPE_BF16
            },
            n: output_cols,
            cache_key: name,
            bytes: &[],
        },
        f8_scale,
    })
}

fn linear(
    weights: &Flux2TransformerWeights,
    input: &GpuTensor,
    name: &str,
    output_cols: usize,
) -> Result<GpuTensor> {
    let ensured = ensure_linear(weights, name, output_cols, input.cols())?;
    match ensured.f8_scale {
        Some(scale) => gpu_linear_nt_cached_f8_mm(input, &ns(weights), &[ensured.part], scale),
        None => gpu_linear_nt_cached_bf16_mm(input, &ns(weights), &[ensured.part]),
    }
    .map_err(DiffusionError::model)
}

fn linear_from_buf(
    weights: &Flux2TransformerWeights,
    input: &GpuBf16Buf,
    name: &str,
    output_cols: usize,
) -> Result<GpuTensor> {
    let ensured = ensure_linear(weights, name, output_cols, input.cols())?;
    match ensured.f8_scale {
        Some(scale) => {
            gpu_linear_nt_cached_f8_mm_from_buf(input, &ns(weights), &[ensured.part], scale)
        }
        None => gpu_linear_nt_cached_bf16_mm_from_buf(input, &ns(weights), &[ensured.part]),
    }
    .map_err(DiffusionError::model)
}

fn linear_from_buf_to_buf(
    weights: &Flux2TransformerWeights,
    input: &GpuBf16Buf,
    name: &str,
    output_cols: usize,
) -> Result<GpuBf16Buf> {
    let ensured = ensure_linear(weights, name, output_cols, input.cols())?;
    match ensured.f8_scale {
        Some(scale) => {
            gpu_linear_nt_cached_f8_mm_from_buf_to_buf(input, &ns(weights), &[ensured.part], scale)
        }
        None => {
            gpu_linear_nt_cached_bf16_mm_from_buf_to_buf(input, &ns(weights), &[ensured.part])
        }
    }
    .map_err(DiffusionError::model)
}

fn round_bf16(tensor: GpuTensor) -> Result<GpuTensor> {
    gpu_bf16_round(&tensor).map_err(DiffusionError::model)
}

fn vec_param(weights: &Flux2TransformerWeights, name: &str, expected: usize) -> Result<Vec<f32>> {
    let values = if weights.file.has_tensor(name) {
        weights.file.read_f32(name)?
    } else if let Some(alias) = alias_one(name) {
        weights.file.read_f32(&alias)?
    } else {
        return Err(DiffusionError::model(format!("flux2 dit missing {name}")));
    };
    if values.len() != expected {
        return Err(DiffusionError::model(format!(
            "flux2 dit {name} has {} values, expected {expected}",
            values.len()
        )));
    }
    Ok(values)
}

/// Sinusoidal timestep embedding (BFL `timestep_embedding`, time_factor 1000).
pub fn flux2_timestep_embedding(t: f32, dim: usize) -> Vec<f32> {
    let t = t * 1000.0;
    let half = dim / 2;
    let mut out = vec![0.0f32; dim];
    for i in 0..half {
        let freq = (-(10000.0f32.ln()) * i as f32 / half as f32).exp();
        let arg = t * freq;
        out[i] = arg.cos();
        out[half + i] = arg.sin();
    }
    out
}

pub fn flux2_rope_tables(ids: &[Flux2PosId], config: &Flux2TransformerConfig) -> (Vec<f32>, Vec<f32>) {
    let half_dim = (config.head_dim / 2) as usize;
    let token_count = ids.len();
    let mut cos = vec![1.0f32; token_count * half_dim];
    let mut sin = vec![0.0f32; token_count * half_dim];
    let theta = config.theta as f32;
    for (token, id) in ids.iter().enumerate() {
        let positions = id.as_array();
        let mut pair_offset = 0usize;
        for (axis, axis_dim) in config.axes_dim.iter().enumerate() {
            let section_dim = *axis_dim as usize;
            let section_pairs = section_dim / 2;
            for pair in 0..section_pairs {
                let exponent = (2.0 * pair as f32) / section_dim as f32;
                let angle = positions[axis] / theta.powf(exponent);
                let index = token * half_dim + pair_offset + pair;
                cos[index] = angle.cos();
                sin[index] = angle.sin();
            }
            pair_offset += section_pairs;
        }
    }
    (cos, sin)
}

pub struct Flux2TransformerRun {
    pub prediction: Vec<f32>,
    pub image_token_count: usize,
}

/// One DiT forward. `img_tokens` is token-major `[N, 128]` including any
/// reference tokens concatenated after the generated tokens. The returned
/// prediction is trimmed to `gen_tokens` (the first generated-image tokens).
/// `guidance` feeds the dev `guidance_in` MLPEmbedder (required when
/// `config.guidance_embed`, rejected otherwise).
pub fn flux2_transformer_forward(
    weights: &Flux2TransformerWeights,
    img_tokens: &[f32],
    img_ids: &[Flux2PosId],
    txt_tokens: &[f32],
    txt_ids: &[Flux2PosId],
    sigma: f32,
    guidance: Option<f32>,
    gen_tokens: usize,
) -> Result<Flux2TransformerRun> {
    if !gpu_device_available() {
        return Err(DiffusionError::workflow(
            "flux2 dit requires CUDA (MAKEPAD_GGML_REQUIRE_CUDA=1)",
        ));
    }
    let config = &weights.config;
    let img_count = img_ids.len();
    let txt_count = txt_ids.len();
    if img_tokens.len() != img_count * config.in_channels as usize {
        return Err(DiffusionError::workflow(format!(
            "flux2 dit img tokens {} != {}x{}",
            img_tokens.len(),
            img_count,
            config.in_channels
        )));
    }
    if txt_tokens.len() != txt_count * config.context_in_dim as usize {
        return Err(DiffusionError::workflow(format!(
            "flux2 dit txt tokens {} != {}x{}",
            txt_tokens.len(),
            txt_count,
            config.context_in_dim
        )));
    }
    if gen_tokens == 0 || gen_tokens > img_count {
        return Err(DiffusionError::workflow(format!(
            "flux2 dit gen_tokens {gen_tokens} outside 1..={img_count}"
        )));
    }

    if config.guidance_embed && guidance.is_none() {
        return Err(DiffusionError::workflow(
            "flux2-dev transformer requires a guidance value",
        ));
    }
    if !config.guidance_embed && guidance.is_some() {
        return Err(DiffusionError::workflow(
            "flux2-klein transformer takes no guidance value",
        ));
    }
    let temb_in = flux2_timestep_embedding(sigma, TIME_EMBED_DIM);
    let gemb_in = guidance.map(|g| flux2_timestep_embedding(g, TIME_EMBED_DIM));
    if flux2_state_enabled() {
        return flux2_forward_state(
            weights, img_tokens, img_ids, txt_tokens, txt_ids, &temb_in,
            gemb_in.as_deref(), gen_tokens,
        );
    }

    let mut all_ids = txt_ids.to_vec();
    all_ids.extend_from_slice(img_ids);
    let (cos, sin) = flux2_rope_tables(&all_ids, config);
    let half = (config.head_dim / 2) as usize;
    let rope_cos = gpu_upload(&cos, all_ids.len(), half).map_err(DiffusionError::model)?;
    let rope_sin = gpu_upload(&sin, all_ids.len(), half).map_err(DiffusionError::model)?;
    let img = gpu_upload(img_tokens, img_count, config.in_channels as usize)
        .map_err(DiffusionError::model)?;
    let txt = gpu_upload(txt_tokens, txt_count, config.context_in_dim as usize)
        .map_err(DiffusionError::model)?;
    let temb = gpu_upload(&temb_in, 1, TIME_EMBED_DIM).map_err(DiffusionError::model)?;
    let gemb = match &gemb_in {
        Some(values) => {
            Some(gpu_upload(values, 1, TIME_EMBED_DIM).map_err(DiffusionError::model)?)
        }
        None => None,
    };
    let pred = flux2_transformer_core(
        weights, &img, &txt, &temb, gemb.as_ref(), &rope_cos, &rope_sin, txt_count, img_count,
        gen_tokens,
    )?;
    let prediction = gpu_download(&pred).map_err(DiffusionError::model)?;
    return Ok(Flux2TransformerRun {
        prediction,
        image_token_count: gen_tokens,
    });
}

/// The device-resident step from persistent inputs to the still-on-device
/// prediction (trimmed to `gen_tokens` rows): pure async stream work,
/// capturable as a CUDA graph.
#[allow(clippy::too_many_arguments)]
fn flux2_transformer_core(
    weights: &Flux2TransformerWeights,
    img_input: &GpuTensor,
    txt_input: &GpuTensor,
    temb_input: &GpuTensor,
    gemb_input: Option<&GpuTensor>,
    rope_cos: &GpuTensor,
    rope_sin: &GpuTensor,
    txt_count: usize,
    img_count: usize,
    gen_tokens: usize,
) -> Result<GpuTensor> {
    let config = &weights.config;
    let hidden = config.hidden_size as usize;
    let heads = config.num_heads as usize;
    let head_dim = config.head_dim as usize;
    let mlp = config.mlp_hidden_size as usize;
    let temb = linear(weights, temb_input, "time_in.in_layer.weight", hidden)?;
    let temb = round_bf16(gpu_silu(&temb).map_err(DiffusionError::model)?)?;
    let temb = linear(weights, &temb, "time_in.out_layer.weight", hidden)?;
    dump_named("temb", &temb);

    // Dev: vec = time_in(...) + guidance_in(timestep_embedding(guidance)) —
    // the same MLPEmbedder shape, summed on the bf16 grid like the bf16
    // reference's `vec + self.guidance_in(...)`.
    let temb = match gemb_input {
        Some(gemb_input) => {
            if !config.guidance_embed {
                return Err(DiffusionError::workflow(
                    "guidance embedding fed to a guidance_embed=false transformer",
                ));
            }
            let gemb = linear(weights, gemb_input, "guidance_in.in_layer.weight", hidden)?;
            let gemb = round_bf16(gpu_silu(&gemb).map_err(DiffusionError::model)?)?;
            let gemb = linear(weights, &gemb, "guidance_in.out_layer.weight", hidden)?;
            dump_named("gemb", &gemb);
            gpu_add_bf16(&temb, &gemb).map_err(DiffusionError::model)?
        }
        None => {
            if config.guidance_embed {
                return Err(DiffusionError::workflow(
                    "flux2-dev transformer core needs the guidance embedding",
                ));
            }
            temb
        }
    };
    dump_named("vec", &temb);

    let mut img = linear(weights, img_input, "img_in.weight", hidden)?;
    let mut txt = linear(weights, txt_input, "txt_in.weight", hidden)?;
    dump_named("img_in", &img);
    dump_named("txt_in", &txt);

    let silu_temb = round_bf16(gpu_silu(&temb).map_err(DiffusionError::model)?)?;
    dump_named("silu_temb", &silu_temb);
    let img_mod = linear(
        weights,
        &silu_temb,
        "double_stream_modulation_img.lin.weight",
        6 * hidden,
    )?;
    let txt_mod = linear(
        weights,
        &silu_temb,
        "double_stream_modulation_txt.lin.weight",
        6 * hidden,
    )?;
    let single_mod = linear(
        weights,
        &silu_temb,
        "single_stream_modulation.lin.weight",
        3 * hidden,
    )?;
    dump_named("img_mod", &img_mod);
    dump_named("txt_mod", &txt_mod);
    dump_named("single_mod", &single_mod);

    let attn_scale = 1.0 / (head_dim as f32).sqrt();
    let depth = config.depth as usize;
    let depth_single = config.depth_single_blocks as usize;

    for layer in 0..depth {
        let prefix = format!("double_blocks.{layer}");
        let (img_next, txt_next) = double_block(
            weights,
            &img,
            &txt,
            &img_mod,
            &txt_mod,
            rope_cos,
            rope_sin,
            &prefix,
            hidden,
            mlp,
            heads,
            head_dim,
            attn_scale,
            txt_count,
        )?;
        img = img_next;
        txt = txt_next;
        dump_named(&format!("double{layer}_img"), &img);
        dump_named(&format!("double{layer}_txt"), &txt);
    }

    let mut joint = gpu_concat_rows(&txt, &img).map_err(DiffusionError::model)?;
    dump_named("joint_in", &joint);
    for layer in 0..depth_single {
        let prefix = format!("single_blocks.{layer}");
        joint = single_block(
            weights,
            &joint,
            &single_mod,
            rope_cos,
            rope_sin,
            &prefix,
            hidden,
            mlp,
            heads,
            head_dim,
            attn_scale,
        )?;
        if layer == 0 || layer + 1 == depth_single {
            dump_named(&format!("single{layer}"), &joint);
        }
    }

    // Official: AdaLN + proj_out over ALL image tokens (gen+refs), then the
    // pipeline trims to gen. LN is per-token so this is not just cosmetics —
    // keep the same tensor the official module sees.
    let img = gpu_slice_rows(&joint, txt_count, img_count).map_err(DiffusionError::model)?;
    dump_named("after_singles_img", &img);
    let ada = linear(
        weights,
        &silu_temb,
        "final_layer.adaLN_modulation.1.weight",
        2 * hidden,
    )?;
    // BFL LastLayer is [shift, scale]; AdaLayerNormContinuous is [scale, shift].
    let (scale_off, shift_off) = if final_adaln_scale_first(weights) {
        (0, hidden)
    } else {
        (hidden, 0)
    };
    let img = gpu_layer_norm_mod(&img, &ada, scale_off, shift_off, LAYER_NORM_EPS)
        .map_err(DiffusionError::model)?;
    dump_named("after_adaln", &img);
    let img = round_bf16(img)?;
    let pred = linear(weights, &img, "final_layer.linear.weight", config.in_channels as usize)?;
    dump_named("pred_full", &pred);
    let pred = gpu_slice_rows(&pred, 0, gen_tokens).map_err(DiffusionError::model)?;
    dump_named("pred", &pred);
    Ok(pred)
}

/// The persistent-state warm path is the default: step inputs live in
/// device tensors reused across steps and edits, and the rope tables (host
/// sincos over 2560 tokens x 64 pairs — milliseconds per step) are computed
/// once per geometry. CUDA-graph capture of the whole step was tried and is
/// impossible here: capture pins every pool buffer it touches (no reuse
/// within one capture), and one Klein step's transients — 25 attention
/// score matrices at 630MB alone — sum to tens of GB unpooled.
/// MAKEPAD_FLUX2_STATE=0 forces the stateless path.
fn flux2_state_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        !matches!(std::env::var("MAKEPAD_FLUX2_STATE").as_deref(), Ok("0"))
    })
}

struct Flux2DeviceState {
    namespace: String,
    img_count: usize,
    txt_count: usize,
    gen_tokens: usize,
    ids_fp: u64,
    img: GpuTensor,
    txt: GpuTensor,
    temb_in: GpuTensor,
    /// Present iff the transformer is guidance-embedded (dev).
    gemb_in: Option<GpuTensor>,
    rope_cos: GpuTensor,
    rope_sin: GpuTensor,
}

thread_local! {
    static FLUX2_DEVICE_STATE: std::cell::RefCell<Option<Flux2DeviceState>> =
        const { std::cell::RefCell::new(None) };
}

/// Position ids feed the rope tables baked into the graph state; a cheap
/// bit-fingerprint detects when they change (different ref layout) so stale
/// tables can never be replayed against new geometry.
fn flux2_ids_fingerprint(txt_ids: &[Flux2PosId], img_ids: &[Flux2PosId]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    let mut mix = |v: f32| {
        hash ^= v.to_bits() as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    };
    for id in txt_ids.iter().chain(img_ids.iter()) {
        mix(id.t);
        mix(id.h);
        mix(id.w);
        mix(id.l);
    }
    hash
}

/// The persistent-state path: inputs are re-uploaded into pinned device
/// tensors every call (teacher runs swap embeds/latents at identical shapes
/// — shape-matching alone must never serve stale tensors), rope tables are
/// rebuilt only when the position-id fingerprint changes.
fn flux2_forward_state(
    weights: &Flux2TransformerWeights,
    img_tokens: &[f32],
    img_ids: &[Flux2PosId],
    txt_tokens: &[f32],
    txt_ids: &[Flux2PosId],
    temb_in: &[f32],
    gemb_in: Option<&[f32]>,
    gen_tokens: usize,
) -> Result<Flux2TransformerRun> {
    let config = &weights.config;
    let img_count = img_ids.len();
    let txt_count = txt_ids.len();
    FLUX2_DEVICE_STATE.with(|cell| {
        let mut slot = cell.borrow_mut();
        let namespace = ns(weights);
        let ids_fp = flux2_ids_fingerprint(txt_ids, img_ids);
        let matches = slot.as_ref().is_some_and(|state| {
            state.namespace == namespace
                && state.img_count == img_count
                && state.txt_count == txt_count
                && state.gen_tokens == gen_tokens
                && state.ids_fp == ids_fp
                && state.gemb_in.is_some() == gemb_in.is_some()
        });
        if !matches {
            *slot = None;
            let mut all_ids = txt_ids.to_vec();
            all_ids.extend_from_slice(img_ids);
            let (cos, sin) = flux2_rope_tables(&all_ids, config);
            let half = (config.head_dim / 2) as usize;
            *slot = Some(Flux2DeviceState {
                namespace,
                img_count,
                txt_count,
                gen_tokens,
                ids_fp,
                img: gpu_upload(img_tokens, img_count, config.in_channels as usize)
                    .map_err(DiffusionError::model)?,
                txt: gpu_upload(txt_tokens, txt_count, config.context_in_dim as usize)
                    .map_err(DiffusionError::model)?,
                temb_in: gpu_upload(temb_in, 1, TIME_EMBED_DIM).map_err(DiffusionError::model)?,
                gemb_in: match gemb_in {
                    Some(values) => Some(
                        gpu_upload(values, 1, TIME_EMBED_DIM).map_err(DiffusionError::model)?,
                    ),
                    None => None,
                },
                rope_cos: gpu_upload(&cos, all_ids.len(), half).map_err(DiffusionError::model)?,
                rope_sin: gpu_upload(&sin, all_ids.len(), half).map_err(DiffusionError::model)?,
            });
        } else {
            let state = slot.as_ref().expect("matching flux2 device state");
            gpu_upload_into(&state.img, img_tokens).map_err(DiffusionError::model)?;
            gpu_upload_into(&state.txt, txt_tokens).map_err(DiffusionError::model)?;
            gpu_upload_into(&state.temb_in, temb_in).map_err(DiffusionError::model)?;
            if let (Some(target), Some(values)) = (&state.gemb_in, gemb_in) {
                gpu_upload_into(target, values).map_err(DiffusionError::model)?;
            }
        }
        let state = slot.as_ref().expect("flux2 device state");
        let pred = flux2_transformer_core(
            weights,
            &state.img,
            &state.txt,
            &state.temb_in,
            state.gemb_in.as_ref(),
            &state.rope_cos,
            &state.rope_sin,
            txt_count,
            img_count,
            gen_tokens,
        )?;
        let prediction = gpu_download(&pred).map_err(DiffusionError::model)?;
        Ok(Flux2TransformerRun {
            prediction,
            image_token_count: gen_tokens,
        })
    })
}

#[derive(Clone, Copy, PartialEq)]
enum Flux2Attn {
    /// Composite f32 cublas QK/softmax/PV — the port's original reference
    /// path. Memory-bound: materializes heads*seq*seq f32 scores.
    CompositeF32,
    /// FA2 bf16 tensor cores with RN-even input staging (see
    /// `gpu_attention_packed_flash_cross_bf16_rn`): the closest match to the
    /// oracle's bf16 SDPA arithmetic (bf16 QK/PV operands, f32 softmax/acc).
    Fa2Bf16Rn,
    /// FA2 f16 tensor cores (RN staging via __float2half).
    Fa2F16,
    /// FA2 f16 mma over q/k/v pre-rounded to the oracle's bf16 value grid
    /// (exact in f16): oracle QK operands, finer P.
    Fa2Bf16PreF16,
}

fn flux2_attn_mode() -> Flux2Attn {
    static MODE: std::sync::OnceLock<Flux2Attn> = std::sync::OnceLock::new();
    *MODE.get_or_init(|| {
        // Default fa2bf16pre — the only arithmetic class that beats the
        // 671ms oracle gate (594.8ms measured), with the best decoded-PNG
        // agreement of any variant (156/0.999588 vs f32's 190/0.999556)
        // and visual parity on the fixture. MAKEPAD_FLUX2_ATTN=f32 selects
        // the composite reference path (885ms) that reproduces the landed
        // baseline bit-for-bit — use it for residual-metric work: the
        // teacher single-step max_abs reads 0.203 there vs 0.414 here,
        // a bf16-ulp chaos lottery, not an edit-quality signal (see
        // flux2cuda.md).
        match std::env::var("MAKEPAD_FLUX2_ATTN").as_deref() {
            Ok("f32") => Flux2Attn::CompositeF32,
            Ok("fa2f16") => Flux2Attn::Fa2F16,
            Ok("fa2bf16") => Flux2Attn::Fa2Bf16Rn,
            _ => Flux2Attn::Fa2Bf16PreF16,
        }
    })
}

fn attn_joint(
    q: &GpuTensor,
    k: &GpuTensor,
    v: &GpuTensor,
    heads: usize,
    scale: f32,
) -> Result<GpuTensor> {
    // Official Klein is bf16 SDPA (torch EFFICIENT_bf16: bf16 QK/PV operands,
    // f32 softmax and accumulate). Self-attn is q_len == kv_len on the FA2
    // cross body.
    match flux2_attn_mode() {
        Flux2Attn::CompositeF32 => {
            gpu_attention_packed_composite_f32(q, k, v, heads, scale)
        }
        Flux2Attn::Fa2Bf16Rn => {
            gpu_attention_packed_flash_cross_bf16_rn(q, k, v, heads, scale)
        }
        Flux2Attn::Fa2F16 => gpu_attention_packed_flash_cross(q, k, v, heads, scale, false),
        Flux2Attn::Fa2Bf16PreF16 => {
            gpu_attention_packed_flash_cross_bf16pre_f16(q, k, v, heads, scale)
        }
    }
    .map_err(DiffusionError::model)
}

/// Per-head QK RMSNorm reading its `cols`-wide slab straight out of the
/// bf16 qkv buffer (bit-identical to slice+expand+rms).
fn head_rms_slab(
    weights: &Flux2TransformerWeights,
    x: &GpuBf16Buf,
    col_off: usize,
    cols: usize,
    name: &str,
    head_dim: usize,
) -> Result<GpuTensor> {
    let scale = vec_param(weights, name, head_dim)?;
    gpu_rms_norm_mul_from_bf16_slab(
        x,
        col_off,
        cols,
        head_dim,
        &ns(weights),
        name,
        &scale,
        LAYER_NORM_EPS,
    )
    .map_err(DiffusionError::model)
}

fn double_block(
    weights: &Flux2TransformerWeights,
    img: &GpuTensor,
    txt: &GpuTensor,
    img_mod: &GpuTensor,
    txt_mod: &GpuTensor,
    rope_cos: &GpuTensor,
    rope_sin: &GpuTensor,
    prefix: &str,
    hidden: usize,
    mlp: usize,
    heads: usize,
    head_dim: usize,
    attn_scale: f32,
    txt_count: usize,
) -> Result<(GpuTensor, GpuTensor)> {
    // LN-mod stores the bf16-RN bits the qkv linear's staging would have
    // produced; the linear chain then stays in bf16 storage (bit-identical
    // values, no standalone conversion passes). The bf16-buffer
    // intermediates are no longer dump-visible; the f32 tensors from RMS
    // onward still are.
    let img_norm = gpu_layer_norm_mod_to_bf16buf(img, img_mod, hidden, 0, LAYER_NORM_EPS)
        .map_err(DiffusionError::model)?;
    let txt_norm = gpu_layer_norm_mod_to_bf16buf(txt, txt_mod, hidden, 0, LAYER_NORM_EPS)
        .map_err(DiffusionError::model)?;
    let tag = match prefix {
        "double_blocks.0" => Some("d0"),
        "double_blocks.4" => Some("d4"),
        _ => None,
    };
    let img_qkv = linear_from_buf_to_buf(
        weights,
        &img_norm,
        &format!("{prefix}.img_attn.qkv.weight"),
        3 * hidden,
    )?;
    let txt_qkv = linear_from_buf_to_buf(
        weights,
        &txt_norm,
        &format!("{prefix}.txt_attn.qkv.weight"),
        3 * hidden,
    )?;
    let img_v = gpu_bf16buf_slab_to_f32(&img_qkv, 2 * hidden, hidden)
        .map_err(DiffusionError::model)?;
    let txt_v = gpu_bf16buf_slab_to_f32(&txt_qkv, 2 * hidden, hidden)
        .map_err(DiffusionError::model)?;

    let img_q = head_rms_slab(
        weights,
        &img_qkv,
        0,
        hidden,
        &format!("{prefix}.img_attn.norm.query_norm.scale"),
        head_dim,
    )?;
    let img_k = head_rms_slab(
        weights,
        &img_qkv,
        hidden,
        hidden,
        &format!("{prefix}.img_attn.norm.key_norm.scale"),
        head_dim,
    )?;
    let txt_q = head_rms_slab(
        weights,
        &txt_qkv,
        0,
        hidden,
        &format!("{prefix}.txt_attn.norm.query_norm.scale"),
        head_dim,
    )?;
    let txt_k = head_rms_slab(
        weights,
        &txt_qkv,
        hidden,
        hidden,
        &format!("{prefix}.txt_attn.norm.key_norm.scale"),
        head_dim,
    )?;
    if let Some(tag) = tag {
        dump_named(&format!("{tag}_img_q_rms"), &img_q);
        dump_named(&format!("{tag}_txt_q_rms"), &txt_q);
        dump_named(&format!("{tag}_img_k_rms"), &img_k);
        dump_named(&format!("{tag}_txt_k_rms"), &txt_k);
    }

    let q = gpu_concat_rows(&txt_q, &img_q).map_err(DiffusionError::model)?;
    let k = gpu_concat_rows(&txt_k, &img_k).map_err(DiffusionError::model)?;
    let v = gpu_concat_rows(&txt_v, &img_v).map_err(DiffusionError::model)?;
    let q = gpu_rope_interleaved(&q, heads, rope_cos, rope_sin).map_err(DiffusionError::model)?;
    let k = gpu_rope_interleaved(&k, heads, rope_cos, rope_sin).map_err(DiffusionError::model)?;
    if let Some(tag) = tag {
        dump_named(&format!("{tag}_q_rope"), &q);
        dump_named(&format!("{tag}_k_rope"), &k);
        dump_named(&format!("{tag}_v"), &v);
    }
    let attn = attn_joint(&q, &k, &v, heads, attn_scale)?;
    let txt_attn = gpu_slice_rows(&attn, 0, txt_count).map_err(DiffusionError::model)?;
    let img_attn = gpu_slice_rows(&attn, txt_count, img.rows()).map_err(DiffusionError::model)?;
    if let Some(tag) = tag {
        dump_named(&format!("{tag}_txt_attn"), &txt_attn);
        dump_named(&format!("{tag}_img_attn"), &img_attn);
    }
    let img_attn = linear(weights, &img_attn, &format!("{prefix}.img_attn.proj.weight"), hidden)?;
    let txt_attn = linear(weights, &txt_attn, &format!("{prefix}.txt_attn.proj.weight"), hidden)?;
    if let Some(tag) = tag {
        dump_named(&format!("{tag}_img_attn_proj"), &img_attn);
        dump_named(&format!("{tag}_txt_attn_proj"), &txt_attn);
    }
    let img = gpu_gated_residual_mod_round_bf16(img, &img_attn, img_mod, 2 * hidden)
        .map_err(DiffusionError::model)?;
    let txt = gpu_gated_residual_mod_round_bf16(txt, &txt_attn, txt_mod, 2 * hidden)
        .map_err(DiffusionError::model)?;
    if let Some(tag) = tag {
        dump_named(&format!("{tag}_img_res1"), &img);
        dump_named(&format!("{tag}_txt_res1"), &txt);
    }

    let img_ff = gpu_layer_norm_mod_to_bf16buf(&img, img_mod, 4 * hidden, 3 * hidden, LAYER_NORM_EPS)
        .map_err(DiffusionError::model)?;
    let txt_ff = gpu_layer_norm_mod_to_bf16buf(&txt, txt_mod, 4 * hidden, 3 * hidden, LAYER_NORM_EPS)
        .map_err(DiffusionError::model)?;
    let img_ff = swiglu_mlp(weights, &img_ff, &format!("{prefix}.img_mlp"), hidden, mlp)?;
    let txt_ff = swiglu_mlp(weights, &txt_ff, &format!("{prefix}.txt_mlp"), hidden, mlp)?;
    if let Some(tag) = tag {
        dump_named(&format!("{tag}_img_ff"), &img_ff);
        dump_named(&format!("{tag}_txt_ff"), &txt_ff);
    }
    let img = gpu_gated_residual_mod_round_bf16(&img, &img_ff, img_mod, 5 * hidden)
        .map_err(DiffusionError::model)?;
    let txt = gpu_gated_residual_mod_round_bf16(&txt, &txt_ff, txt_mod, 5 * hidden)
        .map_err(DiffusionError::model)?;
    Ok((img, txt))
}

fn swiglu_mlp(
    weights: &Flux2TransformerWeights,
    input: &GpuBf16Buf,
    prefix: &str,
    hidden: usize,
    mlp: usize,
) -> Result<GpuTensor> {
    let fused = linear_from_buf_to_buf(weights, input, &format!("{prefix}.0.weight"), 2 * mlp)?;
    // DiT SwiGLU is gate-first: silu(left) * right, read straight off the
    // fused bf16 buffer (same silu/mul arithmetic as the old slice+swap
    // path, stored bf16-RN like the down-projection's staging).
    let act = gpu_swiglu_gate_first_from_bf16(&fused, 0, mlp).map_err(DiffusionError::model)?;
    linear_from_buf(weights, &act, &format!("{prefix}.2.weight"), hidden)
}

fn single_block(
    weights: &Flux2TransformerWeights,
    x: &GpuTensor,
    single_mod: &GpuTensor,
    rope_cos: &GpuTensor,
    rope_sin: &GpuTensor,
    prefix: &str,
    hidden: usize,
    mlp: usize,
    heads: usize,
    head_dim: usize,
    attn_scale: f32,
) -> Result<GpuTensor> {
    let normed = gpu_layer_norm_mod_to_bf16buf(x, single_mod, hidden, 0, LAYER_NORM_EPS)
        .map_err(DiffusionError::model)?;
    let linear1_out = 3 * hidden + 2 * mlp;
    let fused =
        linear_from_buf_to_buf(weights, &normed, &format!("{prefix}.linear1.weight"), linear1_out)?;
    let v = gpu_bf16buf_slab_to_f32(&fused, 2 * hidden, hidden).map_err(DiffusionError::model)?;
    let q = head_rms_slab(
        weights,
        &fused,
        0,
        hidden,
        &format!("{prefix}.norm.query_norm.scale"),
        head_dim,
    )?;
    let k = head_rms_slab(
        weights,
        &fused,
        hidden,
        hidden,
        &format!("{prefix}.norm.key_norm.scale"),
        head_dim,
    )?;
    let q = gpu_rope_interleaved(&q, heads, rope_cos, rope_sin).map_err(DiffusionError::model)?;
    let k = gpu_rope_interleaved(&k, heads, rope_cos, rope_sin).map_err(DiffusionError::model)?;
    let attn = attn_joint(&q, &k, &v, heads, attn_scale)?;
    let mlp_act =
        gpu_swiglu_gate_first_from_bf16(&fused, 3 * hidden, mlp).map_err(DiffusionError::model)?;
    let cat = gpu_concat_f32rn_bf16buf(&attn, &mlp_act).map_err(DiffusionError::model)?;
    let proj = linear_from_buf(weights, &cat, &format!("{prefix}.linear2.weight"), hidden)?;
    gpu_gated_residual_mod_round_bf16(x, &proj, single_mod, 2 * hidden).map_err(DiffusionError::model)
}

/// Host-side Euler update: `x += (sigma_next - sigma) * pred`.
pub fn flux2_euler_step(sample: &mut [f32], pred: &[f32], sigma: f32, sigma_next: f32) -> Result<()> {
    if sample.len() != pred.len() {
        return Err(DiffusionError::workflow(format!(
            "flux2 euler length mismatch {} vs {}",
            sample.len(),
            pred.len()
        )));
    }
    let dt = sigma_next - sigma;
    for (x, p) in sample.iter_mut().zip(pred.iter()) {
        *x += dt * *p;
    }
    Ok(())
}

pub fn flux2_dit_clear_pool() {
    gpu_pool_clear();
}

fn dump_named(name: &str, tensor: &GpuTensor) {
    let Ok(dir) = std::env::var("FLUX2_DIT_DUMP") else {
        return;
    };
    if dir.is_empty() {
        return;
    };
    let Ok(values) = gpu_download(tensor) else {
        return;
    };
    let path = std::path::Path::new(&dir).join(format!("{name}.f32"));
    let mut bytes = Vec::with_capacity(8 + values.len() * 4);
    bytes.extend_from_slice(&(tensor.rows() as u32).to_le_bytes());
    bytes.extend_from_slice(&(tensor.cols() as u32).to_le_bytes());
    for v in values {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(path, bytes);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flux2::Flux2TransformerConfig;

    #[test]
    fn timestep_embed_dim() {
        let e = flux2_timestep_embedding(1.0, 256);
        assert_eq!(e.len(), 256);
        assert_eq!(e[0], 1000.0f32.cos());
        assert_eq!(e[128], 1000.0f32.sin());
    }

    #[test]
    fn rope_table_axes() {
        let config = Flux2TransformerConfig::flux2_klein_4b();
        let ids = [Flux2PosId {
            t: 0.0,
            h: 1.0,
            w: 2.0,
            l: 3.0,
        }];
        let (cos, sin) = flux2_rope_tables(&ids, &config);
        assert_eq!(cos.len(), 64);
        assert_eq!(sin.len(), 64);
        // T-axis pairs (first 16) are identity at t=0.
        for i in 0..16 {
            assert!((cos[i] - 1.0).abs() < 1e-5);
            assert!(sin[i].abs() < 1e-5);
        }
        // L-axis last 16 pairs are nonzero at l=3.
        assert!(sin[48..].iter().any(|v| v.abs() > 1e-6));
    }

    #[test]
    fn diffusers_name_aliases() {
        assert_eq!(
            alias_one("img_in.weight").as_deref(),
            Some("x_embedder.weight")
        );
        assert_eq!(
            alias_one("final_layer.adaLN_modulation.1.weight").as_deref(),
            Some("norm_out.linear.weight")
        );
        assert_eq!(
            alias_one("double_blocks.4.img_mlp.2.weight").as_deref(),
            Some("transformer_blocks.4.ff.linear_out.weight")
        );
        assert_eq!(
            alias_fused("double_blocks.2.img_attn.qkv.weight").unwrap()[1],
            "transformer_blocks.2.attn.to_k.weight"
        );
        assert_eq!(
            alias_one("single_blocks.19.linear1.weight").as_deref(),
            Some("single_transformer_blocks.19.attn.to_qkv_mlp_proj.weight")
        );
    }
}
