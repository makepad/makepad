//! FLUX.1 LoRA adapters: key mapping + merge-at-load into the resident
//! transformer weights.
//!
//! # Why merge, not a runtime adapter
//!
//! The FLUX.1 transformer has THREE execution engines behind one weight
//! arena — the compiled ggml graph (`build_flux_transformer_graph`), the
//! host "lazy" rows path (`LazyFluxTransformer::execute_internal`) and the
//! device path (`execute_device_core`, plus its captured CUDA step graph) —
//! and the device path silently FALLS BACK to the host path on any error.
//! A runtime `y += scale * B(A x)` adapter would have to be implemented in
//! all three (and inside the captured graph) or a fallback would quietly
//! render the pristine model. All three read their weight bytes out of the
//! same `Context` tensors, so patching those bytes once at load time reaches
//! every engine, needs no new kernels, and costs zero per-step time.
//!
//! The price is that a LoRA change is a re-load, not a hot swap. That is
//! paid for by making the LoRA set part of the pipeline identity
//! ([`FluxLoraStack::fingerprint`] keys `flux_cache_namespace` and
//! `FluxPipeline::serves_plan`), so a job with different LoRAs or strengths
//! rebuilds instead of reusing patched weights, and the device weight cache
//! can never alias pristine and patched bytes under one key.
//!
//! # Supported key styles
//!
//! 1. diffusers — `transformer.transformer_blocks.0.attn.to_q.lora_A.weight`
//!    / `.lora_B.weight` (also `single_transformer_blocks`, `norm1.linear`,
//!    `ff.net.0.proj`, `ff.net.2`, `attn.to_out.0`, `attn.add_*_proj`,
//!    `context_embedder`, `x_embedder`, `proj_out`, `norm_out.linear`,
//!    `time_text_embed.*`).
//! 2. ComfyUI/kohya (sd-scripts) — `lora_unet_double_blocks_0_img_attn_qkv
//!    .lora_down.weight` / `.lora_up.weight` / `.alpha`, BFL module naming
//!    with `.` flattened to `_`.
//! 3. the dotted BFL variants with a `diffusion_model.` / `model.
//!    diffusion_model.` / `transformer.` prefix.
//!
//! All three funnel through [`canonicalize_flux_diffusion_tensor_name`],
//! which is already the loader's single source of truth for FLUX tensor
//! naming — including the diffusers→BFL fused-qkv mapping, where `to_q` /
//! `to_k` / `to_v` land on `img_attn.qkv.weight`, `…weight.1`, `…weight.2`.
//! See [`resolve_target`] for the rank/row bookkeeping that turns those
//! piece indices into a row offset inside a fused `qkv` / `linear1` matrix.

use crate::flux::{canonical_name_recognized, canonicalize_flux_diffusion_tensor_name};
use crate::{emit_progress, DiffusionError, ProgressHook, Result};
use makepad_ai_common::{
    bf16_to_f32, f16_to_f32, f32_to_f16_rn, f8_e4m3_to_f32, Context, TensorId, TensorType,
};
use makepad_ai_loader::{MlxDType, MlxSafetensorsHeader};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// One requested adapter: the operator-visible name, the resolved file and
/// the user strength multiplier.
#[derive(Clone, Debug, PartialEq)]
pub struct FluxLoraRef {
    pub name: String,
    pub path: PathBuf,
    pub strength: f32,
}

/// The ordered set of adapters a job asked for. Empty = pristine model.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FluxLoraStack {
    refs: Vec<FluxLoraRef>,
}

impl FluxLoraStack {
    pub fn new(refs: Vec<FluxLoraRef>) -> Self {
        Self { refs }
    }

    pub fn is_empty(&self) -> bool {
        self.effective().next().is_none()
    }

    pub fn refs(&self) -> &[FluxLoraRef] {
        &self.refs
    }

    /// Adapters that actually change weights: strength 0 (and non-finite
    /// strengths) are dropped, so `strength: 0` is byte-for-byte the
    /// pristine model and shares its cache namespace.
    fn effective(&self) -> impl Iterator<Item = &FluxLoraRef> {
        self.refs
            .iter()
            .filter(|entry| entry.strength.is_finite() && entry.strength != 0.0)
    }

    /// Stable identity of the applied adaptation. `""` for "no LoRAs" so
    /// pristine namespaces are unchanged from before this feature existed.
    /// Sorted: merging is a sum, so request order does not change the
    /// result and must not fork the cache.
    pub fn fingerprint(&self) -> String {
        let mut parts = self
            .effective()
            .map(|entry| format!("{}@{}", entry.name, format_strength(entry.strength)))
            .collect::<Vec<_>>();
        if parts.is_empty() {
            return String::new();
        }
        parts.sort();
        parts.join("+")
    }
}

/// Short, exact, locale-free strength rendering for the fingerprint.
fn format_strength(strength: f32) -> String {
    let text = format!("{:.4}", strength);
    let trimmed = text.trim_end_matches('0').trim_end_matches('.');
    if trimmed.is_empty() || trimmed == "-" {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}

/// How many unrecognized keys a merge report names before it stops (the
/// message is a diagnosis aid, not a full listing).
const SKIPPED_KEYS_REPORTED: usize = 8;

/// What a merge actually did — surfaced in logs so an operator can tell a
/// working adapter from a silently-unmatched one.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FluxLoraReport {
    /// Distinct model tensors whose bytes were rewritten.
    pub patched_tensors: usize,
    /// LoRA modules (A/B pairs) merged in.
    pub merged_modules: usize,
    /// Keys the mapper did not recognize (text-encoder LoRAs, DoRA scales,
    /// bias diffs, …), capped for the message.
    pub skipped_keys: Vec<String>,
}

// ---------------------------------------------------------------------------
// Key mapping (pure)
// ---------------------------------------------------------------------------

/// Which half of a LoRA pair a key carries.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoraSide {
    /// `lora_A` / `lora_down`: `[rank, in_features]`.
    Down,
    /// `lora_B` / `lora_up`: `[out_features, rank]`.
    Up,
    /// `alpha`: scalar, `scale = alpha / rank * strength`.
    Alpha,
}

/// A LoRA key resolved onto the loader's canonical FLUX naming.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoraKey {
    /// Canonical weight tensor name, e.g. `double_blocks.3.img_attn.qkv.weight`.
    pub tensor: String,
    /// Sub-block index inside a fused matrix: 0 = the whole tensor (or `q`),
    /// 1 = `k`, 2 = `v`, 3 = the `mlp` third of `single_blocks.N.linear1`.
    pub piece: u32,
    pub side: LoraSide,
}

/// Splits a raw LoRA tensor key into (module path, side). Returns `None` for
/// keys that are not a LoRA weight pair (metadata, DoRA scales, bias diffs).
fn split_side(key: &str) -> Option<(&str, LoraSide)> {
    for (suffix, side) in [
        (".lora_A.weight", LoraSide::Down),
        (".lora_down.weight", LoraSide::Down),
        (".lora_B.weight", LoraSide::Up),
        (".lora_up.weight", LoraSide::Up),
        (".alpha", LoraSide::Alpha),
    ] {
        if let Some(base) = key.strip_suffix(suffix) {
            return Some((base, side));
        }
    }
    None
}

/// The BFL module vocabulary in kohya's flattened (`.` → `_`) spelling.
/// Longest-match first so `img_mlp_0` cannot be shadowed by a prefix.
const KOHYA_BLOCK_MODULES: &[(&str, &str)] = &[
    ("img_attn_proj", "img_attn.proj"),
    ("img_attn_qkv", "img_attn.qkv"),
    ("img_mlp_0", "img_mlp.0"),
    ("img_mlp_2", "img_mlp.2"),
    ("img_mod_lin", "img_mod.lin"),
    ("txt_attn_proj", "txt_attn.proj"),
    ("txt_attn_qkv", "txt_attn.qkv"),
    ("txt_mlp_0", "txt_mlp.0"),
    ("txt_mlp_2", "txt_mlp.2"),
    ("txt_mod_lin", "txt_mod.lin"),
    ("linear1", "linear1"),
    ("linear2", "linear2"),
    ("modulation_lin", "modulation.lin"),
];

const KOHYA_TOP_MODULES: &[(&str, &str)] = &[
    ("final_layer_adaLN_modulation_1", "final_layer.adaLN_modulation.1"),
    ("final_layer_linear", "final_layer.linear"),
    ("guidance_in_in_layer", "guidance_in.in_layer"),
    ("guidance_in_out_layer", "guidance_in.out_layer"),
    ("img_in", "img_in"),
    ("time_in_in_layer", "time_in.in_layer"),
    ("time_in_out_layer", "time_in.out_layer"),
    ("txt_in", "txt_in"),
    ("vector_in_in_layer", "vector_in.in_layer"),
    ("vector_in_out_layer", "vector_in.out_layer"),
];

/// kohya/sd-scripts flattened module path → dotted BFL module path.
/// `double_blocks_7_img_attn_qkv` → `double_blocks.7.img_attn.qkv`.
fn kohya_module_to_dotted(rest: &str) -> Option<String> {
    for (prefix, dotted) in [("double_blocks_", "double_blocks"), ("single_blocks_", "single_blocks")] {
        if let Some(tail) = rest.strip_prefix(prefix) {
            let (index, module) = tail.split_once('_')?;
            if index.is_empty() || !index.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let module = KOHYA_BLOCK_MODULES
                .iter()
                .find(|(flat, _)| *flat == module)
                .map(|(_, dotted)| *dotted)?;
            return Some(format!("{dotted}.{index}.{module}"));
        }
    }
    KOHYA_TOP_MODULES
        .iter()
        .find(|(flat, _)| *flat == rest)
        .map(|(_, dotted)| dotted.to_string())
}

/// Maps ANY supported LoRA key style onto the loader's canonical tensor
/// naming. Pure — the whole format surface is testable without a checkpoint.
pub fn map_lora_key(key: &str) -> Option<LoraKey> {
    let (module, side) = split_side(key)?;
    // kohya/ComfyUI flattened names first: they carry no dots at all, so the
    // dotted canonicalizer cannot see through them.
    let dotted = if let Some(rest) = module
        .strip_prefix("lora_unet_")
        .or_else(|| module.strip_prefix("lora_transformer_"))
        .or_else(|| module.strip_prefix("lora_flux_"))
    {
        kohya_module_to_dotted(rest)?
    } else if module.starts_with("lora_te")
        || module.starts_with("lora_te1_")
        || module.starts_with("lora_te2_")
    {
        // Text-encoder LoRA: the CLIP/T5 stacks are untouched by this merge.
        return None;
    } else {
        module.to_string()
    };

    let canonical = canonicalize_flux_diffusion_tensor_name(&format!("{dotted}.weight"));
    let (tensor, piece) = split_piece(&canonical)?;
    if !canonical_name_recognized(&tensor) {
        return None;
    }
    Some(LoraKey {
        tensor,
        piece,
        side,
    })
}

/// True for a LoRA key written in the diffusers spelling of the FINAL
/// layer's adaLN linear (`norm_out.linear`).
///
/// This is the one module where the two conventions disagree on more than a
/// name: diffusers' `AdaLayerNormContinuous` chunks its output as
/// `(scale, shift)` while BFL's `LastLayer` chunks `(shift, scale)`. The
/// official diffusers conversion swaps the two halves along the out-dim, so
/// a diffusers-authored LoRA must have its `B` rows swapped the same way
/// before it is merged into a BFL-named checkpoint. Every other adaLN
/// module (`norm1.linear`, `norm1_context.linear`, single `norm.linear`)
/// agrees on `(shift, scale, gate)` and needs no swap.
pub fn diffusers_final_layer_key(raw_key: &str) -> bool {
    let Some((module, _)) = split_side(raw_key) else {
        return false;
    };
    module == "norm_out.linear"
        || module.ends_with(".norm_out.linear")
}

/// Swaps the two halves of a `[out_features, rank]` B matrix along the
/// out-dim — the LoRA form of diffusers' `swap_scale_shift`.
fn swap_row_halves(up: &mut [f32], out_features: usize, rank: usize) -> Result<()> {
    if out_features % 2 != 0 {
        return Err(DiffusionError::model(format!(
            "flux lora scale/shift swap needs an even out-dim, got {out_features}"
        )));
    }
    let half = out_features / 2 * rank;
    up.rotate_left(half);
    Ok(())
}

/// `img_attn.qkv.weight.2` → (`…qkv.weight`, 2); `…qkv.weight` → (…, 0).
fn split_piece(canonical: &str) -> Option<(String, u32)> {
    if canonical.ends_with(".weight") {
        return Some((canonical.to_string(), 0));
    }
    let (head, tail) = canonical.rsplit_once('.')?;
    if !head.ends_with(".weight") {
        return None;
    }
    let piece: u32 = tail.parse().ok()?;
    Some((head.to_string(), piece))
}

// ---------------------------------------------------------------------------
// Target resolution (rank/row bookkeeping for fused matrices)
// ---------------------------------------------------------------------------

/// Where a mapped LoRA module writes inside the resident weights.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoraTarget {
    /// The resident tensor to patch.
    pub tensor: String,
    /// First output row of this module's delta inside that tensor.
    pub row_offset: usize,
}

/// Resolves a mapped key against the model's actual tensor set.
///
/// The checkpoint decides how `q`/`k`/`v` are stored. A diffusers-named
/// checkpoint keeps them as separate parts (`…qkv.weight`, `…weight.1`,
/// `…weight.2` — the loader's own multi-part convention), so a piece maps to
/// its own tensor at row 0. The canonical combined-FP8 checkpoints store one
/// FUSED matrix, so the same piece maps to a row range inside it:
/// `img_attn.qkv` = `[3*hidden, hidden]` with q/k/v at rows 0/H/2H, and
/// `single_blocks.N.linear1` = `[3*hidden + 4*hidden, hidden]` with the
/// `proj_mlp` third (piece 3) starting at row 3H. Every piece before the mlp
/// is exactly `hidden` rows tall, so the offset is `piece * hidden`.
///
/// This is the fused-qkv "rank bookkeeping" in its merge form: each piece
/// keeps its OWN rank `r` and contributes `scale * B_piece @ A_piece` to its
/// own row band. (The equivalent runtime-adapter form — stacking
/// `A = [A_q; A_k; A_v]` to rank `3r` against a block-diagonal `B` — computes
/// the same delta; merging never needs to materialize the zero blocks.)
pub fn resolve_target(
    key: &LoraKey,
    hidden_size: usize,
    has_tensor: &dyn Fn(&str) -> bool,
) -> Result<LoraTarget> {
    if key.piece == 0 {
        if !has_tensor(&key.tensor) {
            return Err(DiffusionError::model(format!(
                "flux lora targets '{}', which this checkpoint does not have",
                key.tensor
            )));
        }
        return Ok(LoraTarget {
            tensor: key.tensor.clone(),
            row_offset: 0,
        });
    }
    let part = format!("{}.{}", key.tensor, key.piece);
    if has_tensor(&part) {
        return Ok(LoraTarget {
            tensor: part,
            row_offset: 0,
        });
    }
    if !has_tensor(&key.tensor) {
        return Err(DiffusionError::model(format!(
            "flux lora targets '{}' (piece {}), which this checkpoint has neither fused nor split",
            key.tensor, key.piece
        )));
    }
    Ok(LoraTarget {
        tensor: key.tensor.clone(),
        row_offset: key.piece as usize * hidden_size,
    })
}

// ---------------------------------------------------------------------------
// LoRA file reading
// ---------------------------------------------------------------------------

/// A rank-decomposed module read out of one adapter file.
struct LoraModule {
    /// `[rank, in_features]`, row-major.
    down: Vec<f32>,
    /// `[out_features, rank]`, row-major.
    up: Vec<f32>,
    rank: usize,
    in_features: usize,
    out_features: usize,
    /// `alpha / rank * strength` — the full multiplier on `B @ A`.
    scale: f32,
}

/// Everything one adapter file contributes, grouped by target tensor.
struct LoraFileModules {
    /// target tensor -> (row offset, module)
    by_target: BTreeMap<String, Vec<(usize, LoraModule)>>,
    skipped: Vec<String>,
    module_count: usize,
}

fn read_matrix(header: &MlxSafetensorsHeader, name: &str) -> Result<(Vec<f32>, usize, usize)> {
    let entry = header
        .tensor(name)
        .ok_or_else(|| DiffusionError::model(format!("flux lora: missing tensor '{name}'")))?;
    let (rows, cols) = match entry.shape.as_slice() {
        [rows, cols] => (*rows as usize, *cols as usize),
        other => {
            return Err(DiffusionError::model(format!(
                "flux lora tensor '{name}' must be rank 2, got {other:?}"
            )))
        }
    };
    let bytes = header.read_tensor_bytes(name)?;
    let values = decode_dtype(entry.dtype, &bytes, name)?;
    if values.len() != rows * cols {
        return Err(DiffusionError::model(format!(
            "flux lora tensor '{name}': {} values for {rows}x{cols}",
            values.len()
        )));
    }
    Ok((values, rows, cols))
}

fn read_scalar(header: &MlxSafetensorsHeader, name: &str) -> Result<f32> {
    let entry = header
        .tensor(name)
        .ok_or_else(|| DiffusionError::model(format!("flux lora: missing tensor '{name}'")))?;
    let bytes = header.read_tensor_bytes(name)?;
    let values = decode_dtype(entry.dtype, &bytes, name)?;
    values.first().copied().ok_or_else(|| {
        DiffusionError::model(format!("flux lora alpha '{name}' is empty"))
    })
}

fn decode_dtype(dtype: MlxDType, bytes: &[u8], name: &str) -> Result<Vec<f32>> {
    match dtype {
        MlxDType::F32 => {
            if bytes.len() % 4 != 0 {
                return Err(DiffusionError::model(format!(
                    "flux lora '{name}': f32 payload is not a multiple of 4 bytes"
                )));
            }
            Ok(bytes
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect())
        }
        MlxDType::F16 => {
            if bytes.len() % 2 != 0 {
                return Err(DiffusionError::model(format!(
                    "flux lora '{name}': f16 payload is not a multiple of 2 bytes"
                )));
            }
            Ok(bytes
                .chunks_exact(2)
                .map(|c| f16_to_f32(u16::from_le_bytes([c[0], c[1]])))
                .collect())
        }
        MlxDType::BF16 => {
            if bytes.len() % 2 != 0 {
                return Err(DiffusionError::model(format!(
                    "flux lora '{name}': bf16 payload is not a multiple of 2 bytes"
                )));
            }
            Ok(bytes
                .chunks_exact(2)
                .map(|c| bf16_to_f32(u16::from_le_bytes([c[0], c[1]])))
                .collect())
        }
        MlxDType::F8E4M3 => Ok(bytes.iter().map(|&b| f8_e4m3_to_f32(b)).collect()),
        other => Err(DiffusionError::model(format!(
            "flux lora '{name}': unsupported dtype {other:?}"
        ))),
    }
}

/// Reads one adapter file and resolves every module it carries against the
/// model's tensor set. Fails loudly when NOTHING matched (a wrong-family
/// adapter — an SDXL or FLUX.2 LoRA pointed at FLUX.1).
fn load_lora_file(
    entry: &FluxLoraRef,
    hidden_size: usize,
    bfl_final_layer: bool,
    has_tensor: &dyn Fn(&str) -> bool,
) -> Result<LoraFileModules> {
    let header = MlxSafetensorsHeader::load(&entry.path)?;
    // Group raw keys by (module path) so A/B/alpha meet.
    let mut pairs: BTreeMap<String, (Option<String>, Option<String>, Option<String>)> =
        BTreeMap::new();
    let mut mapped: BTreeMap<String, LoraKey> = BTreeMap::new();
    let mut skipped = Vec::new();
    let mut names = header.tensors.keys().cloned().collect::<Vec<_>>();
    names.sort();
    for name in names {
        let Some(key) = map_lora_key(&name) else {
            skipped.push(name);
            continue;
        };
        let module = format!("{}#{}", key.tensor, key.piece);
        let slot = pairs.entry(module.clone()).or_default();
        match key.side {
            LoraSide::Down => slot.0 = Some(name),
            LoraSide::Up => slot.1 = Some(name),
            LoraSide::Alpha => slot.2 = Some(name),
        }
        mapped.insert(module, key);
    }
    if pairs.is_empty() {
        return Err(DiffusionError::model(format!(
            "lora '{}' ({}) has no FLUX.1 transformer keys — {} keys, first: {:?}",
            entry.name,
            entry.path.display(),
            skipped.len(),
            skipped.first()
        )));
    }

    let mut by_target: BTreeMap<String, Vec<(usize, LoraModule)>> = BTreeMap::new();
    let mut module_count = 0usize;
    for (module, (down_name, up_name, alpha_name)) in pairs {
        let key = &mapped[&module];
        let (Some(down_name), Some(up_name)) = (down_name, up_name) else {
            return Err(DiffusionError::model(format!(
                "lora '{}': module '{}' has only one half of its A/B pair",
                entry.name, module
            )));
        };
        let (down, rank, in_features) = read_matrix(&header, &down_name)?;
        let (mut up, out_features, up_rank) = read_matrix(&header, &up_name)?;
        if bfl_final_layer && diffusers_final_layer_key(&up_name) {
            swap_row_halves(&mut up, out_features, up_rank)?;
        }
        if rank != up_rank {
            return Err(DiffusionError::model(format!(
                "lora '{}': module '{}' rank mismatch, A is {rank} and B is {up_rank}",
                entry.name, module
            )));
        }
        if rank == 0 {
            return Err(DiffusionError::model(format!(
                "lora '{}': module '{}' has rank 0",
                entry.name, module
            )));
        }
        // diffusers files carry no alpha: alpha = rank, i.e. scale 1.
        let alpha = match alpha_name {
            Some(name) => read_scalar(&header, &name)?,
            None => rank as f32,
        };
        if !alpha.is_finite() {
            return Err(DiffusionError::model(format!(
                "lora '{}': module '{}' has a non-finite alpha",
                entry.name, module
            )));
        }
        let target = resolve_target(key, hidden_size, has_tensor)?;
        module_count += 1;
        by_target.entry(target.tensor).or_default().push((
            target.row_offset,
            LoraModule {
                down,
                up,
                rank,
                in_features,
                out_features,
                scale: alpha / rank as f32 * entry.strength,
            },
        ));
    }
    Ok(LoraFileModules {
        by_target,
        skipped,
        module_count,
    })
}

// ---------------------------------------------------------------------------
// Merge
// ---------------------------------------------------------------------------

/// Merges `stack` into the resident transformer tensors in `ctx`, in place.
///
/// Rows no adapter touches are never rewritten — an empty stack (or all
/// strengths 0) leaves the arena byte-for-byte pristine.
///
/// `bfl_final_layer` says the checkpoint's own tensor names are BFL
/// (`final_layer.adaLN_modulation.1`), which is what the canonical
/// combined-FP8 tier ships — see [`diffusers_final_layer_key`].
pub(crate) fn apply_lora_stack(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    hidden_size: usize,
    bfl_final_layer: bool,
    stack: &FluxLoraStack,
    progress: &mut Option<ProgressHook>,
) -> Result<FluxLoraReport> {
    let mut report = FluxLoraReport::default();
    if stack.is_empty() {
        return Ok(report);
    }
    let has_tensor = |name: &str| tensor_ids.contains_key(name);

    // Group EVERY adapter's contribution by target tensor first: overlapping
    // deltas from several LoRAs are then summed in f32 and the target is
    // re-encoded exactly once, so stacking cannot compound rounding.
    let mut by_target: BTreeMap<String, Vec<(usize, LoraModule)>> = BTreeMap::new();
    for entry in stack.effective() {
        let file = load_lora_file(entry, hidden_size, bfl_final_layer, &has_tensor)?;
        report.merged_modules += file.module_count;
        let room = SKIPPED_KEYS_REPORTED.saturating_sub(report.skipped_keys.len());
        report.skipped_keys.extend(file.skipped.into_iter().take(room));
        for (target, modules) in file.by_target {
            by_target.entry(target).or_default().extend(modules);
        }
    }

    let total = by_target.len().max(1);
    for (index, (target, modules)) in by_target.iter().enumerate() {
        emit_progress(
            progress,
            "apply lora",
            index as f64 / total as f64,
        )?;
        merge_target(ctx, tensor_ids, target, modules)?;
        report.patched_tensors += 1;
    }
    emit_progress(progress, "apply lora", 1.0)?;
    Ok(report)
}

fn merge_target(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    target: &str,
    modules: &[(usize, LoraModule)],
) -> Result<()> {
    let tensor_id = *tensor_ids
        .get(target)
        .ok_or_else(|| DiffusionError::model(format!("flux lora: no tensor '{target}'")))?;
    let tensor = ctx
        .tensor(tensor_id)
        .ok_or_else(|| DiffusionError::model(format!("flux lora: bad tensor id for '{target}'")))?;
    let ty = tensor.desc.ty;
    let cols = usize::try_from(tensor.ne[0])
        .map_err(|_| DiffusionError::model(format!("flux lora: '{target}' cols exceed usize")))?;
    let rows = usize::try_from(tensor.ne[1])
        .map_err(|_| DiffusionError::model(format!("flux lora: '{target}' rows exceed usize")))?;
    let elem_bytes = match ty {
        TensorType::F32 => 4usize,
        TensorType::F16 | TensorType::BF16 => 2,
        TensorType::F8E4M3 => 1,
        other => {
            return Err(DiffusionError::model(format!(
                "flux lora cannot patch '{target}': weight type {other:?} is not dense \
                 (block-quantized GGUF tiers must be merged offline)"
            )))
        }
    };

    for (row_offset, module) in modules {
        if module.in_features != cols {
            return Err(DiffusionError::model(format!(
                "flux lora '{target}': A has {} input features, weight has {cols}",
                module.in_features
            )));
        }
        let end = row_offset
            .checked_add(module.out_features)
            .ok_or_else(|| DiffusionError::model("flux lora row range overflow"))?;
        if end > rows {
            return Err(DiffusionError::model(format!(
                "flux lora '{target}': rows {}..{} exceed the weight's {rows} rows",
                row_offset, end
            )));
        }
    }

    let bytes = ctx
        .tensor_data_mut(tensor_id)
        .map_err(DiffusionError::model)?;
    let row_bytes = cols * elem_bytes;
    if bytes.len() != rows * row_bytes {
        return Err(DiffusionError::model(format!(
            "flux lora '{target}': {} bytes for {rows}x{cols} of {elem_bytes}B",
            bytes.len()
        )));
    }

    // Only rows some module actually covers are rewritten.
    let first = modules.iter().map(|(offset, _)| *offset).min().unwrap_or(0);
    let last = modules
        .iter()
        .map(|(offset, module)| offset + module.out_features)
        .max()
        .unwrap_or(0);
    if last <= first {
        return Ok(());
    }
    let span = &mut bytes[first * row_bytes..last * row_bytes];

    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, 32);
    let rows_in_span = last - first;
    let chunk_rows = rows_in_span.div_ceil(threads).max(1);
    let results: Vec<Result<()>> = std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for (chunk_index, chunk) in span.chunks_mut(chunk_rows * row_bytes).enumerate() {
            let base_row = first + chunk_index * chunk_rows;
            handles.push(scope.spawn(move || {
                merge_rows(chunk, base_row, cols, elem_bytes, ty, modules)
            }));
        }
        handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|_| Err(DiffusionError::model("flux lora merge thread panicked")))
            })
            .collect()
    });
    for result in results {
        result?;
    }
    Ok(())
}

/// Patches `chunk` (rows `base_row..`) with every module covering each row.
fn merge_rows(
    chunk: &mut [u8],
    base_row: usize,
    cols: usize,
    elem_bytes: usize,
    ty: TensorType,
    modules: &[(usize, LoraModule)],
) -> Result<()> {
    let row_bytes = cols * elem_bytes;
    let mut delta = vec![0f32; cols];
    for (row_index, row) in chunk.chunks_mut(row_bytes).enumerate() {
        let global_row = base_row + row_index;
        delta.iter_mut().for_each(|value| *value = 0.0);
        let mut touched = false;
        for (row_offset, module) in modules {
            if global_row < *row_offset || global_row >= row_offset + module.out_features {
                continue;
            }
            touched = true;
            let local = global_row - row_offset;
            let up_row = &module.up[local * module.rank..(local + 1) * module.rank];
            for (rank_index, &b) in up_row.iter().enumerate() {
                let coefficient = b * module.scale;
                if coefficient == 0.0 {
                    continue;
                }
                let down_row = &module.down[rank_index * cols..(rank_index + 1) * cols];
                for (out, &a) in delta.iter_mut().zip(down_row) {
                    *out += coefficient * a;
                }
            }
        }
        if !touched {
            continue;
        }
        patch_row(row, &delta, ty)?;
    }
    Ok(())
}

fn patch_row(row: &mut [u8], delta: &[f32], ty: TensorType) -> Result<()> {
    match ty {
        TensorType::F32 => {
            for (chunk, &d) in row.chunks_exact_mut(4).zip(delta) {
                let value = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]) + d;
                chunk.copy_from_slice(&value.to_le_bytes());
            }
        }
        TensorType::F16 => {
            for (chunk, &d) in row.chunks_exact_mut(2).zip(delta) {
                let value = f16_to_f32(u16::from_le_bytes([chunk[0], chunk[1]])) + d;
                chunk.copy_from_slice(&f32_to_f16_rn(value).to_le_bytes());
            }
        }
        TensorType::BF16 => {
            for (chunk, &d) in row.chunks_exact_mut(2).zip(delta) {
                let value = bf16_to_f32(u16::from_le_bytes([chunk[0], chunk[1]])) + d;
                chunk.copy_from_slice(&f32_to_bf16_rn(value).to_le_bytes());
            }
        }
        TensorType::F8E4M3 => {
            for (byte, &d) in row.iter_mut().zip(delta) {
                *byte = f32_to_f8_e4m3(f8_e4m3_to_f32(*byte) + d);
            }
        }
        other => {
            return Err(DiffusionError::model(format!(
                "flux lora cannot patch weight type {other:?}"
            )))
        }
    }
    Ok(())
}

/// f32 → bf16, round-to-nearest-even (torch `.bfloat16()` parity).
pub fn f32_to_bf16_rn(value: f32) -> u16 {
    let bits = value.to_bits();
    if value.is_nan() {
        // Quiet NaN, sign preserved.
        return ((bits >> 16) as u16) | 0x0040;
    }
    let rounded = bits + 0x7fff + ((bits >> 16) & 1);
    (rounded >> 16) as u16
}

/// f32 → signed FP8 E4M3FN (torch `float8_e4m3fn`), round-to-nearest-even,
/// saturating at ±448. Never emits the two NaN encodings (0x7f / 0xff): the
/// loader's fail-closed NaN screen rejects those bytes, and a merged weight
/// must stay loadable.
pub fn f32_to_f8_e4m3(value: f32) -> u8 {
    if !value.is_finite() {
        // Saturate rather than fabricate a NaN byte: an infinite/NaN delta is
        // caught upstream, and here the arena must stay decodable.
        return if value.is_sign_negative() { 0xfe } else { 0x7e };
    }
    let sign: u8 = if value.is_sign_negative() { 0x80 } else { 0x00 };
    let magnitude = value.abs();
    // Largest finite E4M3FN magnitude: 1.75 * 2^8.
    const MAX: f32 = 448.0;
    // Smallest positive subnormal: 2^-9.
    const SUBNORMAL_STEP: f32 = 1.0 / 512.0;
    if magnitude >= MAX {
        // Ties-to-even at the top rounds 464 (=448+16, the midpoint to the
        // would-be 480) down to 448; anything above saturates there too.
        return sign | 0x7e;
    }
    if magnitude < SUBNORMAL_STEP * 0.5 {
        return sign;
    }
    // Subnormal band: |x| < 2^-6, quantized in steps of 2^-9.
    if magnitude < 0.015_625 {
        let steps = round_half_even(magnitude / SUBNORMAL_STEP);
        let steps = steps.clamp(0, 7) as u8;
        return sign | steps;
    }
    // 2^-6 <= magnitude < 448, so the exponent is in [-6, 8].
    let mut exponent = magnitude.log2().floor() as i32;
    debug_assert!((-6..=8).contains(&exponent), "e4m3 exponent {exponent} out of range");
    let mut mantissa = round_half_even(magnitude / exp2f(exponent) * 8.0 - 8.0);
    if mantissa > 7 {
        // Rounding carried into the next binade.
        mantissa = 0;
        exponent += 1;
        if exponent > 8 {
            return sign | 0x7e;
        }
    }
    let biased = (exponent + 7) as u8;
    let byte = sign | (biased << 3) | mantissa as u8;
    // 0x7f/0xff are NaN in E4M3FN — the encoder must never produce them.
    debug_assert!(byte & 0x7f != 0x7f, "e4m3 encode produced a NaN byte");
    byte
}

fn exp2f(exponent: i32) -> f32 {
    2f32.powi(exponent)
}

fn round_half_even(value: f32) -> i32 {
    let floor = value.floor();
    let fract = value - floor;
    let floor = floor as i32;
    match fract.partial_cmp(&0.5) {
        Some(std::cmp::Ordering::Less) => floor,
        Some(std::cmp::Ordering::Greater) => floor + 1,
        _ => {
            if floor % 2 == 0 {
                floor
            } else {
                floor + 1
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(tensor: &str, piece: u32, side: LoraSide) -> LoraKey {
        LoraKey {
            tensor: tensor.to_string(),
            piece,
            side,
        }
    }

    /// Style 1: diffusers. Every pattern here is a real key from
    /// `alvdansen/frosting_lane_flux` / `ByteDance/Hyper-SD`
    /// (`Hyper-FLUX.1-dev-8steps-lora`), read off their safetensors headers.
    #[test]
    fn maps_diffusers_keys() {
        let cases: &[(&str, LoraKey)] = &[
            (
                "transformer.transformer_blocks.0.attn.to_q.lora_A.weight",
                key("double_blocks.0.img_attn.qkv.weight", 0, LoraSide::Down),
            ),
            (
                "transformer.transformer_blocks.7.attn.to_k.lora_B.weight",
                key("double_blocks.7.img_attn.qkv.weight", 1, LoraSide::Up),
            ),
            (
                "transformer.transformer_blocks.7.attn.to_v.lora_B.weight",
                key("double_blocks.7.img_attn.qkv.weight", 2, LoraSide::Up),
            ),
            (
                "transformer.transformer_blocks.3.attn.add_q_proj.lora_A.weight",
                key("double_blocks.3.txt_attn.qkv.weight", 0, LoraSide::Down),
            ),
            (
                "transformer.transformer_blocks.3.attn.add_v_proj.lora_A.weight",
                key("double_blocks.3.txt_attn.qkv.weight", 2, LoraSide::Down),
            ),
            (
                "transformer.transformer_blocks.2.attn.to_out.0.lora_B.weight",
                key("double_blocks.2.img_attn.proj.weight", 0, LoraSide::Up),
            ),
            (
                "transformer.transformer_blocks.2.attn.to_add_out.lora_B.weight",
                key("double_blocks.2.txt_attn.proj.weight", 0, LoraSide::Up),
            ),
            (
                "transformer.transformer_blocks.5.norm1.linear.lora_A.weight",
                key("double_blocks.5.img_mod.lin.weight", 0, LoraSide::Down),
            ),
            (
                "transformer.transformer_blocks.5.norm1_context.linear.lora_A.weight",
                key("double_blocks.5.txt_mod.lin.weight", 0, LoraSide::Down),
            ),
            (
                "transformer.transformer_blocks.9.ff.net.0.proj.lora_A.weight",
                key("double_blocks.9.img_mlp.0.weight", 0, LoraSide::Down),
            ),
            (
                "transformer.transformer_blocks.9.ff.net.2.lora_B.weight",
                key("double_blocks.9.img_mlp.2.weight", 0, LoraSide::Up),
            ),
            (
                "transformer.transformer_blocks.9.ff_context.net.0.proj.lora_A.weight",
                key("double_blocks.9.txt_mlp.0.weight", 0, LoraSide::Down),
            ),
            (
                "transformer.transformer_blocks.9.ff_context.net.2.lora_B.weight",
                key("double_blocks.9.txt_mlp.2.weight", 0, LoraSide::Up),
            ),
            (
                "transformer.single_transformer_blocks.11.attn.to_q.lora_A.weight",
                key("single_blocks.11.linear1.weight", 0, LoraSide::Down),
            ),
            (
                "transformer.single_transformer_blocks.11.attn.to_k.lora_B.weight",
                key("single_blocks.11.linear1.weight", 1, LoraSide::Up),
            ),
            (
                "transformer.single_transformer_blocks.11.attn.to_v.lora_B.weight",
                key("single_blocks.11.linear1.weight", 2, LoraSide::Up),
            ),
            (
                "transformer.single_transformer_blocks.11.proj_mlp.lora_B.weight",
                key("single_blocks.11.linear1.weight", 3, LoraSide::Up),
            ),
            (
                "transformer.single_transformer_blocks.11.proj_out.lora_B.weight",
                key("single_blocks.11.linear2.weight", 0, LoraSide::Up),
            ),
            (
                "transformer.single_transformer_blocks.11.norm.linear.lora_A.weight",
                key("single_blocks.11.modulation.lin.weight", 0, LoraSide::Down),
            ),
            (
                "transformer.context_embedder.lora_A.weight",
                key("txt_in.weight", 0, LoraSide::Down),
            ),
            (
                "transformer.x_embedder.lora_B.weight",
                key("img_in.weight", 0, LoraSide::Up),
            ),
            (
                "transformer.proj_out.lora_B.weight",
                key("final_layer.linear.weight", 0, LoraSide::Up),
            ),
            (
                "transformer.norm_out.linear.lora_B.weight",
                key("final_layer.adaLN_modulation.1.weight", 0, LoraSide::Up),
            ),
            (
                "transformer.time_text_embed.timestep_embedder.linear_1.lora_A.weight",
                key("time_in.in_layer.weight", 0, LoraSide::Down),
            ),
            (
                "transformer.time_text_embed.timestep_embedder.linear_2.lora_B.weight",
                key("time_in.out_layer.weight", 0, LoraSide::Up),
            ),
            (
                "transformer.time_text_embed.text_embedder.linear_1.lora_A.weight",
                key("vector_in.in_layer.weight", 0, LoraSide::Down),
            ),
            (
                "transformer.time_text_embed.guidance_embedder.linear_2.lora_B.weight",
                key("guidance_in.out_layer.weight", 0, LoraSide::Up),
            ),
        ];
        for (raw, expected) in cases {
            assert_eq!(map_lora_key(raw).as_ref(), Some(expected), "key {raw}");
        }
    }

    /// Style 2: ComfyUI / kohya (sd-scripts) flattened BFL names.
    #[test]
    fn maps_kohya_keys() {
        let cases: &[(&str, LoraKey)] = &[
            (
                "lora_unet_double_blocks_0_img_attn_qkv.lora_down.weight",
                key("double_blocks.0.img_attn.qkv.weight", 0, LoraSide::Down),
            ),
            (
                "lora_unet_double_blocks_0_img_attn_qkv.lora_up.weight",
                key("double_blocks.0.img_attn.qkv.weight", 0, LoraSide::Up),
            ),
            (
                "lora_unet_double_blocks_0_img_attn_qkv.alpha",
                key("double_blocks.0.img_attn.qkv.weight", 0, LoraSide::Alpha),
            ),
            (
                "lora_unet_double_blocks_12_img_attn_proj.lora_down.weight",
                key("double_blocks.12.img_attn.proj.weight", 0, LoraSide::Down),
            ),
            (
                "lora_unet_double_blocks_12_img_mlp_0.lora_down.weight",
                key("double_blocks.12.img_mlp.0.weight", 0, LoraSide::Down),
            ),
            (
                "lora_unet_double_blocks_12_img_mlp_2.lora_up.weight",
                key("double_blocks.12.img_mlp.2.weight", 0, LoraSide::Up),
            ),
            (
                "lora_unet_double_blocks_12_img_mod_lin.lora_up.weight",
                key("double_blocks.12.img_mod.lin.weight", 0, LoraSide::Up),
            ),
            (
                "lora_unet_double_blocks_5_txt_attn_qkv.lora_down.weight",
                key("double_blocks.5.txt_attn.qkv.weight", 0, LoraSide::Down),
            ),
            (
                "lora_unet_double_blocks_5_txt_mod_lin.lora_down.weight",
                key("double_blocks.5.txt_mod.lin.weight", 0, LoraSide::Down),
            ),
            (
                "lora_unet_single_blocks_37_linear1.lora_down.weight",
                key("single_blocks.37.linear1.weight", 0, LoraSide::Down),
            ),
            (
                "lora_unet_single_blocks_37_linear2.lora_up.weight",
                key("single_blocks.37.linear2.weight", 0, LoraSide::Up),
            ),
            (
                "lora_unet_single_blocks_37_modulation_lin.alpha",
                key("single_blocks.37.modulation.lin.weight", 0, LoraSide::Alpha),
            ),
            (
                "lora_unet_img_in.lora_down.weight",
                key("img_in.weight", 0, LoraSide::Down),
            ),
            (
                "lora_unet_txt_in.lora_up.weight",
                key("txt_in.weight", 0, LoraSide::Up),
            ),
            (
                "lora_unet_time_in_in_layer.lora_down.weight",
                key("time_in.in_layer.weight", 0, LoraSide::Down),
            ),
            (
                "lora_unet_vector_in_out_layer.lora_up.weight",
                key("vector_in.out_layer.weight", 0, LoraSide::Up),
            ),
            (
                "lora_unet_guidance_in_in_layer.lora_down.weight",
                key("guidance_in.in_layer.weight", 0, LoraSide::Down),
            ),
            (
                "lora_unet_final_layer_linear.lora_up.weight",
                key("final_layer.linear.weight", 0, LoraSide::Up),
            ),
            (
                "lora_unet_final_layer_adaLN_modulation_1.lora_up.weight",
                key("final_layer.adaLN_modulation.1.weight", 0, LoraSide::Up),
            ),
        ];
        for (raw, expected) in cases {
            assert_eq!(map_lora_key(raw).as_ref(), Some(expected), "key {raw}");
        }
    }

    /// Style 3: dotted BFL names behind a checkpoint prefix.
    #[test]
    fn maps_prefixed_bfl_keys() {
        let cases: &[(&str, LoraKey)] = &[
            (
                "diffusion_model.double_blocks.0.img_attn.qkv.lora_down.weight",
                key("double_blocks.0.img_attn.qkv.weight", 0, LoraSide::Down),
            ),
            (
                "model.diffusion_model.single_blocks.3.linear1.lora_up.weight",
                key("single_blocks.3.linear1.weight", 0, LoraSide::Up),
            ),
            (
                "double_blocks.9.txt_mlp.2.lora_A.weight",
                key("double_blocks.9.txt_mlp.2.weight", 0, LoraSide::Down),
            ),
            (
                "diffusion_model.final_layer.adaLN_modulation.1.alpha",
                key("final_layer.adaLN_modulation.1.weight", 0, LoraSide::Alpha),
            ),
        ];
        for (raw, expected) in cases {
            assert_eq!(map_lora_key(raw).as_ref(), Some(expected), "key {raw}");
        }
    }

    #[test]
    fn rejects_non_flux1_keys() {
        for raw in [
            "__metadata__",
            "lora_te1_text_model_encoder_layers_0_mlp_fc1.lora_down.weight",
            "lora_te_text_model_encoder_layers_0_mlp_fc1.alpha",
            "lora_unet_down_blocks_0_attentions_0_proj_in.lora_down.weight",
            "transformer.transformer_blocks.0.attn.to_q.dora_scale",
            "transformer.transformer_blocks.0.attn.to_q.lora_B.bias",
            "lora_unet_double_blocks_x_img_attn_qkv.lora_down.weight",
        ] {
            assert_eq!(map_lora_key(raw), None, "key {raw} must not map");
        }
    }

    /// The fused-qkv bookkeeping: separate q/k/v pieces land on their own
    /// tensors when the checkpoint stores parts, and on `piece * hidden` row
    /// bands when it stores one fused matrix.
    #[test]
    fn resolves_fused_and_split_qkv_targets() {
        let hidden = 3072usize;
        let fused: &dyn Fn(&str) -> bool = &|name: &str| {
            matches!(
                name,
                "double_blocks.0.img_attn.qkv.weight" | "single_blocks.0.linear1.weight"
            )
        };
        let cases = [
            ("double_blocks.0.img_attn.qkv.weight", 0u32, 0usize),
            ("double_blocks.0.img_attn.qkv.weight", 1, 3072),
            ("double_blocks.0.img_attn.qkv.weight", 2, 6144),
            ("single_blocks.0.linear1.weight", 0, 0),
            ("single_blocks.0.linear1.weight", 3, 9216),
        ];
        for (tensor, piece, offset) in cases {
            let resolved =
                resolve_target(&key(tensor, piece, LoraSide::Up), hidden, fused).unwrap();
            assert_eq!(
                resolved,
                LoraTarget {
                    tensor: tensor.to_string(),
                    row_offset: offset,
                },
                "fused {tensor} piece {piece}"
            );
        }

        // Split checkpoint: each piece is its own tensor at row 0.
        let split: &dyn Fn(&str) -> bool = &|name: &str| {
            name.starts_with("double_blocks.0.img_attn.qkv.weight")
        };
        for piece in 0..3u32 {
            let resolved = resolve_target(
                &key("double_blocks.0.img_attn.qkv.weight", piece, LoraSide::Up),
                hidden,
                split,
            )
            .unwrap();
            let expected = if piece == 0 {
                "double_blocks.0.img_attn.qkv.weight".to_string()
            } else {
                format!("double_blocks.0.img_attn.qkv.weight.{piece}")
            };
            assert_eq!(resolved.tensor, expected);
            assert_eq!(resolved.row_offset, 0);
        }

        // A target the checkpoint does not have fails loudly.
        let none: &dyn Fn(&str) -> bool = &|_| false;
        assert!(resolve_target(
            &key("double_blocks.0.img_attn.qkv.weight", 1, LoraSide::Up),
            hidden,
            none
        )
        .is_err());
    }

    /// Only the diffusers spelling of the FINAL layer's adaLN linear needs
    /// its `B` halves swapped; the per-block modulation linears agree on
    /// (shift, scale, gate) in both conventions and must NOT be touched.
    #[test]
    fn only_the_diffusers_final_layer_swaps_scale_and_shift() {
        for raw in [
            "transformer.norm_out.linear.lora_B.weight",
            "norm_out.linear.lora_up.weight",
            "transformer.norm_out.linear.alpha",
        ] {
            assert!(diffusers_final_layer_key(raw), "{raw}");
        }
        for raw in [
            "diffusion_model.final_layer.adaLN_modulation.1.lora_up.weight",
            "lora_unet_final_layer_adaLN_modulation_1.lora_up.weight",
            "transformer.transformer_blocks.0.norm1.linear.lora_B.weight",
            "transformer.transformer_blocks.0.norm1_context.linear.lora_B.weight",
            "transformer.single_transformer_blocks.0.norm.linear.lora_B.weight",
            "transformer.proj_out.lora_B.weight",
        ] {
            assert!(!diffusers_final_layer_key(raw), "{raw}");
        }

        // [4 out, 2 rank] -> the two halves trade places, rows intact.
        let mut up = vec![
            0.0, 1.0, // out 0 (scale half)
            2.0, 3.0, // out 1
            4.0, 5.0, // out 2 (shift half)
            6.0, 7.0, // out 3
        ];
        swap_row_halves(&mut up, 4, 2).unwrap();
        assert_eq!(up, vec![4.0, 5.0, 6.0, 7.0, 0.0, 1.0, 2.0, 3.0]);
        // Swapping twice is the identity.
        swap_row_halves(&mut up, 4, 2).unwrap();
        assert_eq!(up, vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]);
        assert!(swap_row_halves(&mut vec![0.0; 3], 3, 1).is_err());
    }

    #[test]
    fn fingerprint_is_order_insensitive_and_drops_zero_strength() {
        let make = |entries: &[(&str, f32)]| {
            FluxLoraStack::new(
                entries
                    .iter()
                    .map(|(name, strength)| FluxLoraRef {
                        name: name.to_string(),
                        path: PathBuf::from(format!("/loras/{name}.safetensors")),
                        strength: *strength,
                    })
                    .collect(),
            )
        };
        let a = make(&[("style", 0.8), ("detail", 1.0)]);
        let b = make(&[("detail", 1.0), ("style", 0.8)]);
        assert_eq!(a.fingerprint(), b.fingerprint());
        assert_eq!(a.fingerprint(), "detail@1+style@0.8");

        // Strength 0 is exactly the pristine model: same (empty) identity.
        assert_eq!(make(&[("style", 0.0)]).fingerprint(), "");
        assert!(make(&[("style", 0.0)]).is_empty());
        assert_eq!(FluxLoraStack::default().fingerprint(), "");
        assert!(FluxLoraStack::default().is_empty());
        assert_ne!(make(&[("style", 0.8)]).fingerprint(), make(&[("style", 0.9)]).fingerprint());
    }

    /// scale = alpha / rank * strength; diffusers files (no alpha) get
    /// alpha = rank, i.e. plain `strength`.
    #[test]
    fn merges_rows_with_alpha_over_rank_scaling() {
        // 4x3 weight, rank 2 module covering rows 1..3.
        let cols = 3usize;
        let rows = 4usize;
        let rank = 2usize;
        let down = vec![
            1.0, 0.0, 0.0, // r0
            0.0, 2.0, 0.0, // r1
        ];
        let up = vec![
            1.0, 0.0, // out row 0 -> picks r0
            0.0, 1.0, // out row 1 -> picks r1
        ];
        // alpha 8 over rank 2 = 4, times strength 0.5 -> 2.
        let module = LoraModule {
            down,
            up,
            rank,
            in_features: cols,
            out_features: 2,
            scale: 8.0 / rank as f32 * 0.5,
        };
        let mut bytes = vec![0u8; rows * cols * 4];
        merge_rows(&mut bytes, 0, cols, 4, TensorType::F32, &[(1, module)]).unwrap();
        let values: Vec<f32> = bytes
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        // Row 0 and row 3 are outside the module: untouched.
        assert_eq!(&values[0..3], &[0.0, 0.0, 0.0]);
        assert_eq!(&values[9..12], &[0.0, 0.0, 0.0]);
        // Row 1 = 2 * (B[0] @ A) = 2 * [1,0,0]; row 2 = 2 * [0,2,0].
        assert_eq!(&values[3..6], &[2.0, 0.0, 0.0]);
        assert_eq!(&values[6..9], &[0.0, 4.0, 0.0]);
    }

    /// Two adapters over one tensor sum in f32 before a single re-encode.
    #[test]
    fn stacked_modules_sum_before_encoding() {
        let cols = 2usize;
        let make = |value: f32| LoraModule {
            down: vec![1.0, 1.0],
            up: vec![value],
            rank: 1,
            in_features: cols,
            out_features: 1,
            scale: 1.0,
        };
        let mut bytes = vec![0u8; cols * 4];
        merge_rows(
            &mut bytes,
            0,
            cols,
            4,
            TensorType::F32,
            &[(0, make(0.25)), (0, make(0.5))],
        )
        .unwrap();
        let values: Vec<f32> = bytes
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        assert_eq!(values, vec![0.75, 0.75]);
    }

    /// The unpatched path must be bit-identical: rows outside every module's
    /// band are never re-encoded, so an FP8 weight cannot drift.
    #[test]
    fn untouched_f8_rows_are_bit_identical() {
        let cols = 4usize;
        let rows = 3usize;
        let original: Vec<u8> = (0..(rows * cols) as u8).map(|b| b.wrapping_add(0x30)).collect();
        let mut bytes = original.clone();
        let module = LoraModule {
            down: vec![0.0; cols],
            up: vec![0.0],
            rank: 1,
            in_features: cols,
            out_features: 1,
            scale: 1.0,
        };
        merge_rows(&mut bytes, 0, cols, 1, TensorType::F8E4M3, &[(1, module)]).unwrap();
        assert_eq!(&bytes[0..cols], &original[0..cols], "row 0 untouched");
        assert_eq!(&bytes[2 * cols..], &original[2 * cols..], "row 2 untouched");
        // The covered row round-trips through decode/encode with a zero
        // delta, which must be the identity on every finite E4M3 byte.
        assert_eq!(&bytes[cols..2 * cols], &original[cols..2 * cols]);
    }

    #[test]
    fn f8_e4m3_encode_round_trips_every_finite_byte() {
        for byte in 0u8..=255 {
            if byte & 0x7f == 0x7f {
                continue; // NaN encodings
            }
            let value = f8_e4m3_to_f32(byte);
            let encoded = f32_to_f8_e4m3(value);
            // Exact identity, signed zero included: a zero LoRA delta must
            // not perturb a single byte of the resident FP8 arena.
            assert_eq!(
                encoded, byte,
                "byte {byte:#04x} decoded to {value} re-encoded to {encoded:#04x}"
            );
        }
    }

    #[test]
    fn f8_e4m3_encode_rounds_and_saturates_without_nan() {
        // Midway between 1.0 (0x38) and 1.125 (0x39): ties to even -> 1.0.
        assert_eq!(f32_to_f8_e4m3(1.0625), 0x38);
        // Midway between 1.125 and 1.25: ties to even -> 1.25 (0x3a).
        assert_eq!(f32_to_f8_e4m3(1.1875), 0x3a);
        assert_eq!(f32_to_f8_e4m3(448.0), 0x7e);
        assert_eq!(f32_to_f8_e4m3(1.0e9), 0x7e);
        assert_eq!(f32_to_f8_e4m3(-1.0e9), 0xfe);
        assert_eq!(f32_to_f8_e4m3(f32::NAN), 0x7e);
        assert_eq!(f32_to_f8_e4m3(0.0), 0x00);
        // Subnormal band: 2^-9 steps below 2^-6.
        assert_eq!(f8_e4m3_to_f32(f32_to_f8_e4m3(3.0 / 512.0)), 3.0 / 512.0);
        for bits in 0u32..(1 << 16) {
            let value = f32::from_bits(bits << 8);
            if !value.is_finite() {
                continue;
            }
            let byte = f32_to_f8_e4m3(value);
            assert!(byte & 0x7f != 0x7f, "value {value} encoded to a NaN byte");
        }
    }

    /// The merge's numeric heart: re-encoding a patched weight must land on
    /// the NEAREST representable E4M3 value, never a systematically biased
    /// one — a whole-checkpoint bias is what would show up as a washed-out
    /// render rather than a visible failure.
    #[test]
    fn f8_e4m3_encode_picks_the_nearest_representable_value() {
        let representable: Vec<f32> = (0u8..=255)
            .filter(|byte| byte & 0x7f != 0x7f)
            .map(f8_e4m3_to_f32)
            .collect();
        let mut state = 0x1234_5678_9abc_def0u64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            // Spread over the weight/delta range that actually occurs.
            let unit = ((state >> 40) as u32) as f32 / (1u32 << 24) as f32;
            (unit - 0.5) * 900.0
        };
        for _ in 0..50_000 {
            let value = next();
            let encoded = f8_e4m3_to_f32(f32_to_f8_e4m3(value));
            let best = representable
                .iter()
                .copied()
                .fold(f32::INFINITY, |best, candidate| {
                    if (candidate - value).abs() < (best - value).abs() {
                        candidate
                    } else {
                        best
                    }
                });
            assert!(
                (encoded - value).abs() <= (best - value).abs(),
                "{value} encoded to {encoded}, nearest is {best}"
            );
        }
    }

    #[test]
    fn bf16_round_to_nearest_even() {
        assert_eq!(f32_to_bf16_rn(1.0), 0x3f80);
        assert_eq!(bf16_to_f32(f32_to_bf16_rn(-2.5)), -2.5);
        // 1 + 2^-9 sits exactly between two bf16 steps; ties to even -> 1.0.
        assert_eq!(f32_to_bf16_rn(1.0 + 2f32.powi(-9)), 0x3f80);
        // 1 + 3*2^-9 rounds up to 1 + 2^-7.
        assert_eq!(bf16_to_f32(f32_to_bf16_rn(1.0 + 3.0 * 2f32.powi(-9))), 1.0 + 2f32.powi(-7));
    }

}
