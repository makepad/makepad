//! Checkpoint -> ggml `Context`.
//!
//! Reads the official `model_bs_roformer_ep_17_sdr_9.6568.ckpt` (a torch
//! zip+pickle state dict) with `makepad-ai-loader` and lays every tensor out
//! in the exact shape the forward graph consumes. Two rewrites happen here,
//! once, at load time — never per chunk:
//!
//! 1. **Dim reversal.** torch `Linear.weight` is `(out, in)` row-major; the
//!    same bytes are a ggml `[in, out]` tensor, which is what `mul_mat` wants.
//!    So the reversal is free: only the declared extents change.
//! 2. **Band grouping.** The 62 per-band parameter sets of the band split and
//!    of each mask estimator are concatenated into one 3-D tensor per
//!    *band-width group* (`config::band_groups`, 7 of them), so the graph runs
//!    7 batched `mul_mat`s where the reference runs 62 small ones. Bands in a
//!    group share a width, so this needs no zero padding and wastes no FLOPs.
//! 3. **GLU pre-split.** The mask estimator's second `Linear` emits `2*w`
//!    values that `nn.GLU` splits into `value * sigmoid(gate)`. The two halves
//!    are stored as separate weights so the graph never slices a matmul
//!    output.

use crate::config::*;
use makepad_ai_common::quant::f32_to_f16_rn;
use makepad_ai_common::{
    ggml_pad, BufferUsage, Context, DiffusionError, InitParams, Result, Tensor, TensorDesc,
    TensorId, TensorLayout, TensorType, GGML_MEM_ALIGN,
};
use makepad_ai_loader::formats::torch_pth::PthStateDict;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Arena headroom on top of the weights. Only the graph's real leaves live
/// here — the chunk spectrum the caller writes (4100 x 1101 f32 = 18 MB) and
/// the two position vectors. Intermediates are planned into the device buffer
/// by the graph planner (see `graph.rs`), so this stays small.
pub const DEFAULT_GRAPH_EXTRA_BYTES: usize = 64 << 20;

/// Store the matmul weights as F16 (norm scales and biases stay F32).
///
/// This halves the 527 MB weight stream and lets Metal pick the half-precision
/// simdgroup matmul; the model was trained under mixed precision, so f16
/// weights are in-distribution rather than a reinterpretation. Set
/// `MAKEPAD_STEMS_F16=0` to keep every weight F32 — that is the mode the
/// oracle-parity gate measures the arithmetic in.
pub fn f16_weights_enabled() -> bool {
    !matches!(
        std::env::var("MAKEPAD_STEMS_F16").as_deref(),
        Ok("0") | Ok("false")
    )
}

/// Did the operator ASK for f16, as opposed to just not saying anything?
///
/// The default above is a Metal-shaped default. A store where f16 weights are
/// a pessimisation (CUDA — no f16-weight x f32-activation GEMM in cuBLAS)
/// wants "off unless explicitly requested" rather than "on unless explicitly
/// refused", without losing the operator's ability to force the comparison.
pub fn f16_weights_requested() -> bool {
    matches!(
        std::env::var("MAKEPAD_STEMS_F16").as_deref(),
        Ok("1") | Ok("true")
    )
}

/// The loaded model: one ggml arena holding every weight, plus the name index.
pub struct StemsWeights {
    pub ctx: Context,
    pub ids: BTreeMap<String, TensorId>,
    pub path: PathBuf,
    pub graph_extra_bytes: usize,
}

impl StemsWeights {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        Self::load_with_extra(path, DEFAULT_GRAPH_EXTRA_BYTES)
    }

    pub fn load_with_extra(path: impl AsRef<Path>, extra_bytes: usize) -> Result<Self> {
        Self::load_with_options(path, extra_bytes, f16_weights_enabled())
    }

    /// As [`Self::load_with_extra`], with the matmul-weight precision decided
    /// by the caller (which knows which device store it is loading for).
    pub fn load_with_options(
        path: impl AsRef<Path>,
        extra_bytes: usize,
        f16: bool,
    ) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mut state = PthStateDict::load(&path)
            .map_err(|e| DiffusionError::model(format!("stems checkpoint {}: {e}", path.display())))?;
        let plan = weight_plan();
        let total = plan_total_bytes(&plan, f16, extra_bytes)?;
        let mut ctx = Context::new(InitParams {
            mem_size: total,
            mem_buffer: None,
            no_alloc: false,
        });
        let mut ids = BTreeMap::new();
        for item in &plan {
            let ty = item.dtype(f16);
            let id = ctx
                .new_named_tensor(
                    item.name.clone(),
                    ty,
                    item.extents.len(),
                    &item.extents,
                    BufferUsage::Weights,
                )
                .map_err(DiffusionError::model)?;
            let values = item.source.gather(&mut state)?;
            let want = item.elements();
            if values.len() != want {
                return Err(DiffusionError::model(format!(
                    "stems weight '{}' expected {want} floats, checkpoint gave {}",
                    item.name,
                    values.len()
                )));
            }
            match ty {
                TensorType::F16 => {
                    let half: Vec<u16> = values.iter().copied().map(f32_to_f16_rn).collect();
                    ctx.write_tensor_data(id, bytes_of_u16(&half))
                        .map_err(DiffusionError::model)?;
                }
                _ => ctx
                    .write_tensor_data(id, bytes_of_f32(&values))
                    .map_err(DiffusionError::model)?,
            }
            ids.insert(item.name.clone(), id);
        }
        Ok(Self {
            ctx,
            ids,
            path,
            graph_extra_bytes: extra_bytes,
        })
    }

    pub fn id(&self, name: &str) -> Result<TensorId> {
        self.ids
            .get(name)
            .copied()
            .ok_or_else(|| DiffusionError::model(format!("missing stems tensor '{name}'")))
    }
}

fn bytes_of_f32(values: &[f32]) -> &[u8] {
    // f32 has no padding and no invalid bit patterns; the slice is 4-aligned.
    unsafe { std::slice::from_raw_parts(values.as_ptr() as *const u8, values.len() * 4) }
}

fn bytes_of_u16(values: &[u16]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(values.as_ptr() as *const u8, values.len() * 2) }
}

// ---------------------------------------------------------------------------
// The plan: every graph tensor, its ggml extents, and how to build it from
// checkpoint entries. Declaring it as data keeps allocation, sizing and
// filling in exact lockstep (a mismatch is impossible by construction) and
// makes the layout testable without a checkpoint on disk.
// ---------------------------------------------------------------------------

/// How one graph tensor is assembled out of checkpoint entries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// One checkpoint tensor, verbatim.
    Whole(String),
    /// Concatenate one checkpoint tensor per band in a group, verbatim.
    Bands(Vec<String>),
    /// Concatenate a contiguous ROW range of one checkpoint tensor per band —
    /// how the GLU halves are peeled off the `(2w, 768)` second Linear.
    /// `rows` is `(start_row, row_count)`; `row_len` is the row width.
    BandRows {
        names: Vec<String>,
        rows: (usize, usize),
        row_len: usize,
    },
}

impl Source {
    fn gather(&self, state: &mut PthStateDict) -> Result<Vec<f32>> {
        match self {
            Source::Whole(name) => read(state, name),
            Source::Bands(names) => {
                let mut out = Vec::new();
                for name in names {
                    out.extend_from_slice(&read(state, name)?);
                }
                Ok(out)
            }
            Source::BandRows {
                names,
                rows: (start, count),
                row_len,
            } => {
                let mut out = Vec::new();
                for name in names {
                    let values = read(state, name)?;
                    let from = start * row_len;
                    let to = from + count * row_len;
                    if to > values.len() {
                        return Err(DiffusionError::model(format!(
                            "stems weight '{name}': rows {start}..{} x {row_len} exceed {} floats",
                            start + count,
                            values.len()
                        )));
                    }
                    out.extend_from_slice(&values[from..to]);
                }
                Ok(out)
            }
        }
    }
}

fn read(state: &mut PthStateDict, name: &str) -> Result<Vec<f32>> {
    state
        .f32(name)
        .map_err(|e| DiffusionError::model(format!("stems checkpoint tensor '{name}': {e}")))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanItem {
    pub name: String,
    /// ggml extents, ne[0] fastest.
    pub extents: Vec<i64>,
    pub source: Source,
    /// True for the `a` operand of a `mul_mat` — the only tensors big enough
    /// for half precision to matter, and the only ones where it is safe.
    pub is_matmul_weight: bool,
}

impl PlanItem {
    pub fn elements(&self) -> usize {
        self.extents.iter().product::<i64>() as usize
    }

    pub fn dtype(&self, f16: bool) -> TensorType {
        if f16 && self.is_matmul_weight {
            TensorType::F16
        } else {
            TensorType::F32
        }
    }
}

/// A norm scale or bias: always F32 (they are rank-1 or per-band vectors,
/// negligible in size, and are consumed by elementwise ops that would have to
/// convert anyway).
fn item(name: String, extents: Vec<i64>, source: Source) -> PlanItem {
    PlanItem {
        name,
        extents,
        source,
        is_matmul_weight: false,
    }
}

/// The `a` operand of a `mul_mat`.
fn mat(name: String, extents: Vec<i64>, source: Source) -> PlanItem {
    PlanItem {
        name,
        extents,
        source,
        is_matmul_weight: true,
    }
}

// -- graph tensor names (also the ggml tensor names, so a Metal op log is
//    readable) --

pub fn bs_gamma(group: usize) -> String {
    format!("bandsplit.gamma.g{group}")
}
pub fn bs_weight(group: usize) -> String {
    format!("bandsplit.weight.g{group}")
}
pub fn bs_bias(group: usize) -> String {
    format!("bandsplit.bias.g{group}")
}
/// `axis` is 0 for the time transformer, 1 for the frequency transformer.
pub fn attn_name(block: usize, axis: usize, part: &str) -> String {
    format!("block{block}.{}.attn.{part}", axis_tag(axis))
}
pub fn ff_name(block: usize, axis: usize, part: &str) -> String {
    format!("block{block}.{}.ff.{part}", axis_tag(axis))
}
fn axis_tag(axis: usize) -> &'static str {
    if axis == 0 {
        "time"
    } else {
        "freq"
    }
}
pub const FINAL_NORM: &str = "final_norm.gamma";
pub fn mask_name(stem: usize, group: usize, part: &str) -> String {
    format!("mask{stem}.g{group}.{part}")
}

fn ckpt_transformer(block: usize, axis: usize) -> String {
    format!("layers.{block}.{axis}.layers.0")
}

/// Every tensor the forward graph reads, in allocation order.
pub fn weight_plan() -> Vec<PlanItem> {
    let groups = band_groups();
    let mut plan = Vec::new();

    // -- band split, one batched set per band-width group --
    for (g, group) in groups.iter().enumerate() {
        let bands: Vec<usize> = (group.first_band..group.first_band + group.count).collect();
        let w = group.width as i64;
        let n = group.count as i64;
        plan.push(item(
            bs_gamma(g),
            vec![w, 1, n],
            Source::Bands(
                bands
                    .iter()
                    .map(|b| format!("band_split.to_features.{b}.0.gamma"))
                    .collect(),
            ),
        ));
        plan.push(mat(
            bs_weight(g),
            vec![w, DIM as i64, n],
            Source::Bands(
                bands
                    .iter()
                    .map(|b| format!("band_split.to_features.{b}.1.weight"))
                    .collect(),
            ),
        ));
        plan.push(item(
            bs_bias(g),
            vec![DIM as i64, 1, n],
            Source::Bands(
                bands
                    .iter()
                    .map(|b| format!("band_split.to_features.{b}.1.bias"))
                    .collect(),
            ),
        ));
    }

    // -- 8 blocks x {time, freq} transformers, one layer each --
    for block in 0..DEPTH {
        for axis in 0..2 {
            let src = ckpt_transformer(block, axis);
            plan.push(item(
                attn_name(block, axis, "gamma"),
                vec![DIM as i64],
                Source::Whole(format!("{src}.0.norm.gamma")),
            ));
            plan.push(mat(
                attn_name(block, axis, "qkv"),
                vec![DIM as i64, (DIM_INNER * 3) as i64],
                Source::Whole(format!("{src}.0.to_qkv.weight")),
            ));
            plan.push(mat(
                attn_name(block, axis, "gates_w"),
                vec![DIM as i64, HEADS as i64],
                Source::Whole(format!("{src}.0.to_gates.weight")),
            ));
            plan.push(item(
                attn_name(block, axis, "gates_b"),
                vec![HEADS as i64],
                Source::Whole(format!("{src}.0.to_gates.bias")),
            ));
            plan.push(mat(
                attn_name(block, axis, "out"),
                vec![DIM_INNER as i64, DIM as i64],
                Source::Whole(format!("{src}.0.to_out.0.weight")),
            ));

            plan.push(item(
                ff_name(block, axis, "gamma"),
                vec![DIM as i64],
                Source::Whole(format!("{src}.1.net.0.gamma")),
            ));
            plan.push(mat(
                ff_name(block, axis, "w1"),
                vec![DIM as i64, FF_INNER as i64],
                Source::Whole(format!("{src}.1.net.1.weight")),
            ));
            plan.push(item(
                ff_name(block, axis, "b1"),
                vec![FF_INNER as i64],
                Source::Whole(format!("{src}.1.net.1.bias")),
            ));
            plan.push(mat(
                ff_name(block, axis, "w2"),
                vec![FF_INNER as i64, DIM as i64],
                Source::Whole(format!("{src}.1.net.4.weight")),
            ));
            plan.push(item(
                ff_name(block, axis, "b2"),
                vec![DIM as i64],
                Source::Whole(format!("{src}.1.net.4.bias")),
            ));
        }
    }

    plan.push(item(
        FINAL_NORM.to_string(),
        vec![DIM as i64],
        Source::Whole("final_norm.gamma".to_string()),
    ));

    // -- 4 mask estimators x 7 band groups --
    for stem in 0..NUM_STEMS {
        for (g, group) in groups.iter().enumerate() {
            let bands: Vec<usize> = (group.first_band..group.first_band + group.count).collect();
            let w = group.width as i64;
            let n = group.count as i64;
            let l0: Vec<String> = bands
                .iter()
                .map(|b| format!("mask_estimators.{stem}.to_freqs.{b}.0.0.weight"))
                .collect();
            let l0b: Vec<String> = bands
                .iter()
                .map(|b| format!("mask_estimators.{stem}.to_freqs.{b}.0.0.bias"))
                .collect();
            let l2: Vec<String> = bands
                .iter()
                .map(|b| format!("mask_estimators.{stem}.to_freqs.{b}.0.2.weight"))
                .collect();
            let l2b: Vec<String> = bands
                .iter()
                .map(|b| format!("mask_estimators.{stem}.to_freqs.{b}.0.2.bias"))
                .collect();
            plan.push(mat(
                mask_name(stem, g, "w1"),
                vec![DIM as i64, MASK_HIDDEN as i64, n],
                Source::Bands(l0),
            ));
            plan.push(item(
                mask_name(stem, g, "b1"),
                vec![MASK_HIDDEN as i64, 1, n],
                Source::Bands(l0b),
            ));
            // nn.GLU(dim=-1): first half is the value, second half the gate.
            plan.push(mat(
                mask_name(stem, g, "wv"),
                vec![MASK_HIDDEN as i64, w, n],
                Source::BandRows {
                    names: l2.clone(),
                    rows: (0, group.width),
                    row_len: MASK_HIDDEN,
                },
            ));
            plan.push(mat(
                mask_name(stem, g, "wg"),
                vec![MASK_HIDDEN as i64, w, n],
                Source::BandRows {
                    names: l2,
                    rows: (group.width, group.width),
                    row_len: MASK_HIDDEN,
                },
            ));
            plan.push(item(
                mask_name(stem, g, "bv"),
                vec![w, 1, n],
                Source::BandRows {
                    names: l2b.clone(),
                    rows: (0, group.width),
                    row_len: 1,
                },
            ));
            plan.push(item(
                mask_name(stem, g, "bg"),
                vec![w, 1, n],
                Source::BandRows {
                    names: l2b,
                    rows: (group.width, group.width),
                    row_len: 1,
                },
            ));
        }
    }

    plan
}

fn plan_total_bytes(plan: &[PlanItem], f16: bool, extra_bytes: usize) -> Result<usize> {
    let mut total = 0usize;
    for item in plan {
        let ty = item.dtype(f16);
        let layout =
            TensorLayout::for_ggml(ty, &item.extents).map_err(DiffusionError::model)?;
        let nbytes =
            Tensor::from_desc(0, TensorDesc::new(ty, layout, BufferUsage::Weights)).nbytes();
        total = ggml_pad(total, GGML_MEM_ALIGN)
            .checked_add(nbytes)
            .ok_or_else(|| DiffusionError::model("stems weight size overflow"))?;
    }
    ggml_pad(total, GGML_MEM_ALIGN)
        .checked_add(extra_bytes)
        .ok_or_else(|| DiffusionError::model("stems context size overflow"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn plan_reads_every_checkpoint_tensor_exactly_once() {
        // The published checkpoint has 1355 tensors. 16 of them are the frozen
        // `rotary_embed.freqs` buffers (one per transformer: 8 time + 8 freq),
        // which ggml's `rope` regenerates from ROPE_THETA instead of reading —
        // verified numerically identical to the stored buffer. The plan must
        // therefore consume the other 1339, and no tensor twice.
        let mut seen: Vec<String> = Vec::new();
        for item in weight_plan() {
            match item.source {
                Source::Whole(name) => seen.push(name),
                Source::Bands(names) => seen.extend(names),
                Source::BandRows { names, .. } => seen.extend(names),
            }
        }
        // BandRows appears twice per checkpoint tensor (value half + gate
        // half), which is legitimate; count distinct names instead.
        let distinct: BTreeSet<String> = seen.iter().cloned().collect();
        assert_eq!(
            distinct.len(),
            1355 - 16,
            "plan should cover every non-rope checkpoint tensor"
        );
        assert!(distinct.iter().all(|n| !n.contains("rotary_embed")));
    }

    #[test]
    fn plan_element_counts_match_the_parameter_count() {
        // 131,704,612 total params minus the 16 x 32 rope buffers; the GLU
        // pre-split copies no data twice (each half is read once).
        let total: usize = weight_plan().iter().map(|i| i.elements()).sum();
        assert_eq!(total, 131_704_612 - 16 * 32);
    }

    #[test]
    fn plan_names_are_unique() {
        let plan = weight_plan();
        let names: BTreeSet<&str> = plan.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names.len(), plan.len());
    }

    #[test]
    fn band_split_and_mask_shapes_are_grouped() {
        let plan = weight_plan();
        let find = |name: &str| plan.iter().find(|i| i.name == name).unwrap().clone();
        // Group 0: 24 bands of width 8.
        assert_eq!(find(&bs_gamma(0)).extents, vec![8, 1, 24]);
        assert_eq!(find(&bs_weight(0)).extents, vec![8, 384, 24]);
        assert_eq!(find(&bs_bias(0)).extents, vec![384, 1, 24]);
        // Group 6: the single 516-wide band.
        assert_eq!(find(&bs_weight(6)).extents, vec![516, 384, 1]);
        assert_eq!(find(&mask_name(3, 6, "w1")).extents, vec![384, 768, 1]);
        assert_eq!(find(&mask_name(3, 6, "wv")).extents, vec![768, 516, 1]);
        assert_eq!(find(&mask_name(3, 6, "bg")).extents, vec![516, 1, 1]);
    }

    #[test]
    fn transformer_shapes_are_dim_reversed() {
        let plan = weight_plan();
        let find = |name: String| plan.iter().find(|i| i.name == name).unwrap().clone();
        assert_eq!(find(attn_name(0, 0, "qkv")).extents, vec![384, 1536]);
        assert_eq!(find(attn_name(7, 1, "out")).extents, vec![512, 384]);
        assert_eq!(find(ff_name(3, 1, "w1")).extents, vec![384, 1536]);
        assert_eq!(find(ff_name(3, 1, "w2")).extents, vec![1536, 384]);
        assert_eq!(find(attn_name(0, 0, "gates_w")).extents, vec![384, 8]);
    }

    #[test]
    fn arena_size_covers_the_weights_plus_headroom() {
        let plan = weight_plan();
        let total = plan_total_bytes(&plan, false, 0).unwrap();
        let params: usize = plan.iter().map(|i| i.elements()).sum();
        assert!(total >= params * 4);
        // Alignment padding must stay small relative to the payload.
        assert!(total < params * 4 + plan.len() * GGML_MEM_ALIGN + 4096);
        // Half precision halves everything except the norm scales and biases.
        let half = plan_total_bytes(&plan, true, 0).unwrap();
        assert!(half < total * 55 / 100, "f16 plan is {half} vs f32 {total}");
    }
}
