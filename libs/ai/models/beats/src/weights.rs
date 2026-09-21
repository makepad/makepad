//! Strict Beat This! Lightning-checkpoint loader.
//!
//! The upstream `.ckpt` is read directly. A complete name/shape census runs
//! before allocation; inference never accepts a partial or architecture-mixed
//! state dict. Torch linear dimensions are re-declared in ggml `[in,out]`
//! order, BatchNorm is folded into affine scale/bias, and convolution kernels
//! are reordered once for the graph's compact manual-im2col layout.

use crate::config::*;
use makepad_ai_common::quant::f32_to_f16_rn;
use makepad_ai_common::{
    ggml_pad, BufferUsage, Context, DiffusionError, InitParams, Result, Tensor, TensorDesc,
    TensorId, TensorLayout, TensorType, GGML_MEM_ALIGN,
};
use makepad_ai_loader::formats::torch_pth::PthStateDict;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub const DEFAULT_GRAPH_EXTRA_BYTES: usize = 16 << 20;

pub fn f16_weights_enabled() -> bool {
    !matches!(
        std::env::var("MAKEPAD_BEATS_F16").as_deref(),
        Ok("0") | Ok("false")
    )
}

pub fn f16_weights_requested() -> bool {
    matches!(
        std::env::var("MAKEPAD_BEATS_F16").as_deref(),
        Ok("1") | Ok("true")
    )
}

#[derive(Clone, Debug)]
pub struct CheckpointCensus {
    pub config: BeatsConfig,
    pub tensors: BTreeMap<String, Vec<usize>>,
}

pub fn checkpoint_census(path: impl AsRef<Path>) -> Result<CheckpointCensus> {
    let path = path.as_ref();
    let state = PthStateDict::load(path).map_err(|error| {
        DiffusionError::model(format!("beats checkpoint {}: {error}", path.display()))
    })?;
    validate_census(&state)
}

pub struct BeatsWeights {
    pub ctx: Context,
    pub ids: BTreeMap<String, TensorId>,
    pub path: PathBuf,
    pub config: BeatsConfig,
}

impl BeatsWeights {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        Self::load_with_options(path, DEFAULT_GRAPH_EXTRA_BYTES, f16_weights_enabled())
    }

    pub fn load_with_options(
        path: impl AsRef<Path>,
        extra_bytes: usize,
        f16: bool,
    ) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mut state = PthStateDict::load(&path).map_err(|error| {
            DiffusionError::model(format!("beats checkpoint {}: {error}", path.display()))
        })?;
        let census = validate_census(&state)?;
        let plan = weight_plan(census.config);
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
            if values.len() != item.elements() {
                return Err(DiffusionError::model(format!(
                    "beats weight '{}' expected {} floats, checkpoint produced {}",
                    item.name,
                    item.elements(),
                    values.len()
                )));
            }
            if ty == TensorType::F16 {
                let half: Vec<u16> = values.into_iter().map(f32_to_f16_rn).collect();
                ctx.write_tensor_data(id, bytes_u16(&half))
                    .map_err(DiffusionError::model)?;
            } else {
                ctx.write_tensor_data(id, bytes_f32(&values))
                    .map_err(DiffusionError::model)?;
            }
            ids.insert(item.name.clone(), id);
        }
        Ok(Self {
            ctx,
            ids,
            path,
            config: census.config,
        })
    }
}

fn validate_census(state: &PthStateDict) -> Result<CheckpointCensus> {
    let linear_shape = state
        .shape("model.frontend.linear.weight")
        .map_err(|error| DiffusionError::model(format!("beats checkpoint: {error}")))?;
    let config = match linear_shape.as_slice() {
        [512, FRONTEND_FEATURES] => BeatsConfig::FINAL,
        [128, FRONTEND_FEATURES] => BeatsConfig::SMALL,
        other => {
            return Err(DiffusionError::model(format!(
                "beats checkpoint has unsupported frontend.linear.weight shape {other:?}"
            )))
        }
    };
    let expected = expected_census(config);
    let actual_names: BTreeSet<String> = state.names().cloned().collect();
    let expected_names: BTreeSet<String> = expected.keys().cloned().collect();
    let missing: Vec<_> = expected_names.difference(&actual_names).cloned().collect();
    let unexpected: Vec<_> = actual_names.difference(&expected_names).cloned().collect();
    if !missing.is_empty() || !unexpected.is_empty() {
        return Err(DiffusionError::model(format!(
            "beats checkpoint tensor census failed: missing={missing:?}, unexpected={unexpected:?}"
        )));
    }
    for (name, wanted) in &expected {
        let got = state
            .shape(name)
            .map_err(|error| DiffusionError::model(format!("beats checkpoint: {error}")))?;
        if &got != wanted {
            return Err(DiffusionError::model(format!(
                "beats checkpoint tensor '{name}' has shape {got:?}, expected {wanted:?}"
            )));
        }
    }
    Ok(CheckpointCensus {
        config,
        tensors: expected,
    })
}

fn expected_census(config: BeatsConfig) -> BTreeMap<String, Vec<usize>> {
    fn add(out: &mut BTreeMap<String, Vec<usize>>, name: String, shape: &[usize]) {
        out.insert(name, shape.to_vec());
    }
    fn batch_norm(
        out: &mut BTreeMap<String, Vec<usize>>,
        prefix: &str,
        channels: usize,
    ) {
        for part in ["weight", "bias", "running_mean", "running_var"] {
            add(out, format!("{prefix}.{part}"), &[channels]);
        }
        add(out, format!("{prefix}.num_batches_tracked"), &[]);
    }
    let mut out = BTreeMap::new();
    batch_norm(&mut out, "model.frontend.stem.bn1d", MEL_BINS);
    add(
        &mut out,
        "model.frontend.stem.conv2d.weight".into(),
        &[STEM_DIM, 1, 4, 3],
    );
    batch_norm(&mut out, "model.frontend.stem.bn2d", STEM_DIM);

    for block in 0..STEM_BLOCKS {
        let dim = STEM_CHANNELS[block];
        let next = STEM_CHANNELS[block + 1];
        let root = format!("model.frontend.blocks.{block}");
        for (attn_tag, ff_tag) in [("attnF", "ffF"), ("attnT", "ffT")] {
            let attn = format!("{root}.partial.{attn_tag}");
            let heads = dim / HEAD_DIM;
            add(&mut out, format!("{attn}.norm.gamma"), &[dim]);
            add(&mut out, format!("{attn}.to_qkv.weight"), &[dim * 3, dim]);
            add(&mut out, format!("{attn}.to_gates.weight"), &[heads, dim]);
            add(&mut out, format!("{attn}.to_gates.bias"), &[heads]);
            add(&mut out, format!("{attn}.to_out.0.weight"), &[dim, dim]);
            add(&mut out, format!("{attn}.rotary_embed.freqs"), &[HEAD_DIM / 2]);
            let ff = format!("{root}.partial.{ff_tag}.net");
            add(&mut out, format!("{ff}.0.gamma"), &[dim]);
            add(&mut out, format!("{ff}.1.weight"), &[dim * FF_MULT, dim]);
            add(&mut out, format!("{ff}.1.bias"), &[dim * FF_MULT]);
            add(&mut out, format!("{ff}.4.weight"), &[dim, dim * FF_MULT]);
            add(&mut out, format!("{ff}.4.bias"), &[dim]);
        }
        add(&mut out, format!("{root}.conv2d.weight"), &[next, dim, 2, 3]);
        batch_norm(&mut out, &format!("{root}.norm"), next);
    }

    add(
        &mut out,
        "model.frontend.linear.weight".into(),
        &[config.transformer_dim, FRONTEND_FEATURES],
    );
    add(
        &mut out,
        "model.frontend.linear.bias".into(),
        &[config.transformer_dim],
    );
    for layer in 0..MAIN_LAYERS {
        let dim = config.transformer_dim;
        let heads = config.heads();
        let root = format!("model.transformer_blocks.layers.{layer}");
        add(&mut out, format!("{root}.0.norm.gamma"), &[dim]);
        add(&mut out, format!("{root}.0.to_qkv.weight"), &[dim * 3, dim]);
        add(&mut out, format!("{root}.0.to_gates.weight"), &[heads, dim]);
        add(&mut out, format!("{root}.0.to_gates.bias"), &[heads]);
        add(&mut out, format!("{root}.0.to_out.0.weight"), &[dim, dim]);
        add(&mut out, format!("{root}.0.rotary_embed.freqs"), &[HEAD_DIM / 2]);
        add(&mut out, format!("{root}.1.net.0.gamma"), &[dim]);
        add(&mut out, format!("{root}.1.net.1.weight"), &[dim * FF_MULT, dim]);
        add(&mut out, format!("{root}.1.net.1.bias"), &[dim * FF_MULT]);
        add(&mut out, format!("{root}.1.net.4.weight"), &[dim, dim * FF_MULT]);
        add(&mut out, format!("{root}.1.net.4.bias"), &[dim]);
    }
    add(
        &mut out,
        "model.transformer_blocks.norm.gamma".into(),
        &[config.transformer_dim],
    );
    add(
        &mut out,
        "model.task_heads.beat_downbeat_lin.weight".into(),
        &[2, config.transformer_dim],
    );
    add(
        &mut out,
        "model.task_heads.beat_downbeat_lin.bias".into(),
        &[2],
    );
    out
}

#[derive(Clone, Debug)]
enum Source {
    Whole(String),
    BatchNorm { prefix: String, channels: usize, bias: bool },
    Conv {
        name: String,
        out_channels: usize,
        in_channels: usize,
        kernel_h: usize,
        kernel_w: usize,
    },
}

impl Source {
    fn gather(&self, state: &mut PthStateDict) -> Result<Vec<f32>> {
        match self {
            Source::Whole(name) => read(state, name),
            Source::BatchNorm {
                prefix,
                channels,
                bias,
            } => {
                let gamma = read(state, &format!("{prefix}.weight"))?;
                let beta = read(state, &format!("{prefix}.bias"))?;
                let mean = read(state, &format!("{prefix}.running_mean"))?;
                let variance = read(state, &format!("{prefix}.running_var"))?;
                let mut out = Vec::with_capacity(*channels);
                for channel in 0..*channels {
                    let scale = gamma[channel] / (variance[channel] + BATCH_NORM_EPS).sqrt();
                    out.push(if *bias {
                        beta[channel] - mean[channel] * scale
                    } else {
                        scale
                    });
                }
                Ok(out)
            }
            Source::Conv {
                name,
                out_channels,
                in_channels,
                kernel_h,
                kernel_w,
            } => {
                let source = read(state, name)?;
                let kernel = in_channels * kernel_h * kernel_w;
                let mut out = Vec::with_capacity(source.len());
                // Torch is [out,in,ky,kx]. The graph forms patches in
                // [ky,kx,in] order (in fastest), cutting patch nodes from
                // in*kh*kw to kh*kw. Reorder once at load.
                for oc in 0..*out_channels {
                    for ky in 0..*kernel_h {
                        for kx in 0..*kernel_w {
                            for ic in 0..*in_channels {
                                let at = oc * kernel
                                    + ic * kernel_h * kernel_w
                                    + ky * kernel_w
                                    + kx;
                                out.push(source[at]);
                            }
                        }
                    }
                }
                Ok(out)
            }
        }
    }
}

fn read(state: &mut PthStateDict, name: &str) -> Result<Vec<f32>> {
    state.f32(name).map_err(|error| {
        DiffusionError::model(format!("beats checkpoint tensor '{name}': {error}"))
    })
}

#[derive(Clone, Debug)]
struct PlanItem {
    name: String,
    extents: Vec<i64>,
    source: Source,
    matmul: bool,
}

impl PlanItem {
    fn elements(&self) -> usize {
        self.extents.iter().product::<i64>() as usize
    }

    fn dtype(&self, f16: bool) -> TensorType {
        if f16 && self.matmul {
            TensorType::F16
        } else {
            TensorType::F32
        }
    }
}

fn item(name: impl Into<String>, extents: &[usize], source: Source) -> PlanItem {
    PlanItem {
        name: name.into(),
        extents: extents.iter().map(|&value| value as i64).collect(),
        source,
        matmul: false,
    }
}

fn mat(name: impl Into<String>, extents: &[usize], source: Source) -> PlanItem {
    let mut item = item(name, extents, source);
    item.matmul = true;
    item
}

pub(crate) const INPUT_BN_SCALE: &str = "stem.input_bn.scale";
pub(crate) const INPUT_BN_BIAS: &str = "stem.input_bn.bias";
pub(crate) const STEM_CONV: &str = "stem.conv";
pub(crate) const STEM_BN_SCALE: &str = "stem.bn.scale";
pub(crate) const STEM_BN_BIAS: &str = "stem.bn.bias";
pub(crate) const FRONT_LINEAR_W: &str = "frontend.linear.weight";
pub(crate) const FRONT_LINEAR_B: &str = "frontend.linear.bias";
pub(crate) const FINAL_NORM: &str = "main.final_norm.gamma";
pub(crate) const HEAD_W: &str = "head.weight";
pub(crate) const HEAD_B: &str = "head.bias";

pub(crate) fn block_conv(block: usize) -> String {
    format!("front{block}.conv")
}
pub(crate) fn block_bn(block: usize, part: &str) -> String {
    format!("front{block}.bn.{part}")
}
pub(crate) fn transformer_name(prefix: &str, part: &str) -> String {
    format!("{prefix}.{part}")
}

fn add_bn(
    plan: &mut Vec<PlanItem>,
    graph_prefix: &str,
    checkpoint_prefix: &str,
    channels: usize,
    rank3: bool,
) {
    let shape = if rank3 {
        vec![channels, 1, 1]
    } else {
        vec![channels]
    };
    plan.push(item(
        format!("{graph_prefix}.scale"),
        &shape,
        Source::BatchNorm {
            prefix: checkpoint_prefix.into(),
            channels,
            bias: false,
        },
    ));
    plan.push(item(
        format!("{graph_prefix}.bias"),
        &shape,
        Source::BatchNorm {
            prefix: checkpoint_prefix.into(),
            channels,
            bias: true,
        },
    ));
}

fn add_transformer(
    plan: &mut Vec<PlanItem>,
    graph_prefix: &str,
    attn_prefix: &str,
    ff_prefix: &str,
    dim: usize,
) {
    let heads = dim / HEAD_DIM;
    plan.push(item(
        transformer_name(graph_prefix, "attn.gamma"),
        &[dim],
        Source::Whole(format!("{attn_prefix}.norm.gamma")),
    ));
    plan.push(mat(
        transformer_name(graph_prefix, "attn.qkv"),
        &[dim, dim * 3],
        Source::Whole(format!("{attn_prefix}.to_qkv.weight")),
    ));
    plan.push(mat(
        transformer_name(graph_prefix, "attn.gates_w"),
        &[dim, heads],
        Source::Whole(format!("{attn_prefix}.to_gates.weight")),
    ));
    plan.push(item(
        transformer_name(graph_prefix, "attn.gates_b"),
        &[heads],
        Source::Whole(format!("{attn_prefix}.to_gates.bias")),
    ));
    plan.push(mat(
        transformer_name(graph_prefix, "attn.out"),
        &[dim, dim],
        Source::Whole(format!("{attn_prefix}.to_out.0.weight")),
    ));
    plan.push(item(
        transformer_name(graph_prefix, "ff.gamma"),
        &[dim],
        Source::Whole(format!("{ff_prefix}.net.0.gamma")),
    ));
    plan.push(mat(
        transformer_name(graph_prefix, "ff.w1"),
        &[dim, dim * FF_MULT],
        Source::Whole(format!("{ff_prefix}.net.1.weight")),
    ));
    plan.push(item(
        transformer_name(graph_prefix, "ff.b1"),
        &[dim * FF_MULT],
        Source::Whole(format!("{ff_prefix}.net.1.bias")),
    ));
    plan.push(mat(
        transformer_name(graph_prefix, "ff.w2"),
        &[dim * FF_MULT, dim],
        Source::Whole(format!("{ff_prefix}.net.4.weight")),
    ));
    plan.push(item(
        transformer_name(graph_prefix, "ff.b2"),
        &[dim],
        Source::Whole(format!("{ff_prefix}.net.4.bias")),
    ));
}

fn weight_plan(config: BeatsConfig) -> Vec<PlanItem> {
    let mut plan = Vec::new();
    add_bn(
        &mut plan,
        "stem.input_bn",
        "model.frontend.stem.bn1d",
        MEL_BINS,
        false,
    );
    plan.push(mat(
        STEM_CONV,
        &[1 * 4 * 3, STEM_DIM],
        Source::Conv {
            name: "model.frontend.stem.conv2d.weight".into(),
            out_channels: STEM_DIM,
            in_channels: 1,
            kernel_h: 4,
            kernel_w: 3,
        },
    ));
    add_bn(
        &mut plan,
        "stem.bn",
        "model.frontend.stem.bn2d",
        STEM_DIM,
        true,
    );

    for block in 0..STEM_BLOCKS {
        let dim = STEM_CHANNELS[block];
        let next = STEM_CHANNELS[block + 1];
        let checkpoint = format!("model.frontend.blocks.{block}");
        add_transformer(
            &mut plan,
            &format!("front{block}.freq"),
            &format!("{checkpoint}.partial.attnF"),
            &format!("{checkpoint}.partial.ffF"),
            dim,
        );
        add_transformer(
            &mut plan,
            &format!("front{block}.time"),
            &format!("{checkpoint}.partial.attnT"),
            &format!("{checkpoint}.partial.ffT"),
            dim,
        );
        plan.push(mat(
            block_conv(block),
            &[dim * 2 * 3, next],
            Source::Conv {
                name: format!("{checkpoint}.conv2d.weight"),
                out_channels: next,
                in_channels: dim,
                kernel_h: 2,
                kernel_w: 3,
            },
        ));
        add_bn(
            &mut plan,
            &format!("front{block}.bn"),
            &format!("{checkpoint}.norm"),
            next,
            true,
        );
    }

    plan.push(mat(
        FRONT_LINEAR_W,
        &[FRONTEND_FEATURES, config.transformer_dim],
        Source::Whole("model.frontend.linear.weight".into()),
    ));
    plan.push(item(
        FRONT_LINEAR_B,
        &[config.transformer_dim],
        Source::Whole("model.frontend.linear.bias".into()),
    ));
    for layer in 0..MAIN_LAYERS {
        let checkpoint = format!("model.transformer_blocks.layers.{layer}");
        add_transformer(
            &mut plan,
            &format!("main{layer}"),
            &format!("{checkpoint}.0"),
            &format!("{checkpoint}.1"),
            config.transformer_dim,
        );
    }
    plan.push(item(
        FINAL_NORM,
        &[config.transformer_dim],
        Source::Whole("model.transformer_blocks.norm.gamma".into()),
    ));
    plan.push(mat(
        HEAD_W,
        &[config.transformer_dim, 2],
        Source::Whole("model.task_heads.beat_downbeat_lin.weight".into()),
    ));
    plan.push(item(
        HEAD_B,
        &[2],
        Source::Whole("model.task_heads.beat_downbeat_lin.bias".into()),
    ));
    plan
}

fn plan_total_bytes(plan: &[PlanItem], f16: bool, extra: usize) -> Result<usize> {
    let mut total = 0usize;
    for item in plan {
        let ty = item.dtype(f16);
        let layout = TensorLayout::for_ggml(ty, &item.extents).map_err(DiffusionError::model)?;
        let bytes = Tensor::from_desc(0, TensorDesc::new(ty, layout, BufferUsage::Weights)).nbytes();
        total = ggml_pad(total, GGML_MEM_ALIGN)
            .checked_add(bytes)
            .ok_or_else(|| DiffusionError::model("beats weight arena overflow"))?;
    }
    ggml_pad(total, GGML_MEM_ALIGN)
        .checked_add(extra)
        .ok_or_else(|| DiffusionError::model("beats context arena overflow"))
}

fn bytes_f32(values: &[f32]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(values.as_ptr().cast(), values.len() * 4) }
}

fn bytes_u16(values: &[u16]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(values.as_ptr().cast(), values.len() * 2) }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WEIGHTS: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../../local/models/weights/beat_this"
    );

    fn census_if_present(name: &str, want: BeatsConfig) {
        let path = Path::new(WEIGHTS).join(name);
        if !path.is_file() {
            eprintln!("beats census: SKIP, {} is not seeded", path.display());
            return;
        }
        let census = checkpoint_census(&path).unwrap();
        assert_eq!(census.config, want);
        assert_eq!(census.tensors.len(), 166);
        assert_eq!(
            census.tensors["model.frontend.stem.conv2d.weight"],
            vec![32, 1, 4, 3]
        );
        assert_eq!(
            census.tensors["model.frontend.blocks.2.conv2d.weight"],
            vec![256, 128, 2, 3]
        );
        assert_eq!(
            census.tensors["model.frontend.linear.weight"],
            vec![want.transformer_dim, 1024]
        );
        assert_eq!(
            census.tensors["model.transformer_blocks.layers.5.0.to_qkv.weight"],
            vec![want.transformer_dim * 3, want.transformer_dim]
        );
        assert_eq!(
            census.tensors["model.task_heads.beat_downbeat_lin.weight"],
            vec![2, want.transformer_dim]
        );
    }

    #[test]
    fn final0_tensor_census_is_exact() {
        census_if_present("final0.ckpt", BeatsConfig::FINAL);
    }

    #[test]
    fn small0_tensor_census_is_exact() {
        census_if_present("small0.ckpt", BeatsConfig::SMALL);
    }

    #[test]
    fn plans_cover_both_model_widths() {
        for config in [BeatsConfig::FINAL, BeatsConfig::SMALL] {
            let plan = weight_plan(config);
            let names: BTreeSet<_> = plan.iter().map(|item| &item.name).collect();
            assert_eq!(names.len(), plan.len());
            let head = plan.iter().find(|item| item.name == HEAD_W).unwrap();
            assert_eq!(head.extents, vec![config.transformer_dim as i64, 2]);
        }
    }
}
