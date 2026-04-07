use crate::context::Context;
use crate::core::{
    ggml_pad, PoolOp, ScaleMode, SortOrder, GGML_ROPE_TYPE_IMROPE, GGML_ROPE_TYPE_MROPE,
    GGML_ROPE_TYPE_NEOX, GGML_ROPE_TYPE_VISION, GGML_SCALE_FLAG_ANTIALIAS,
};
use crate::graph::{Graph, NodeId};
use crate::op::{GluOp, Op, UnaryOp};
use crate::tensor::{ggml_type_size_for_type, Tensor, TensorType};

use super::{
    FunctionConstant, FunctionConstantValue, MetalDeviceFeatures, MetalPipelineDescriptor,
};

const FC_FLASH_ATTN_EXT_PAD: i32 = 100;
const FC_FLASH_ATTN_EXT_BLK: i32 = 200;
const FC_FLASH_ATTN_EXT: i32 = 300;
const FC_FLASH_ATTN_EXT_VEC: i32 = 400;
const FC_FLASH_ATTN_EXT_VEC_REDUCE: i32 = 500;
const FC_MUL_MV: i32 = 600;
const FC_MUL_MM: i32 = 700;
const FC_ROPE: i32 = 800;
pub(super) const FC_SSM_CONV: i32 = 900;
const FC_SOLVE_TRI: i32 = 1000;
const FC_COUNT_EQUAL: i32 = 1100;
const FC_UNARY: i32 = 1200;
const FC_BIN: i32 = 1300;
const FC_SUM_ROWS: i32 = 1400;
const FC_UPSCALE: i32 = 1500;
pub(super) const FC_GATED_DELTA_NET: i32 = 1600;

const OP_FLASH_ATTN_EXT_NQPSG: i32 = 8;
const OP_FLASH_ATTN_EXT_NCPSG: i32 = 64;
const OP_FLASH_ATTN_EXT_VEC_NQPSG: i32 = 1;
const OP_FLASH_ATTN_EXT_VEC_NCPSG: i32 = 32;

const OP_UNARY_NUM_SCALE: i16 = 10;
const OP_UNARY_NUM_FILL: i16 = 11;
const OP_UNARY_NUM_CLAMP: i16 = 12;
const OP_UNARY_NUM_SQR: i16 = 13;
const OP_UNARY_NUM_SQRT: i16 = 14;
const OP_UNARY_NUM_SIN: i16 = 15;
const OP_UNARY_NUM_COS: i16 = 16;
const OP_UNARY_NUM_LOG: i16 = 17;
const OP_UNARY_NUM_LEAKY_RELU: i16 = 18;
const OP_UNARY_NUM_TANH: i16 = 100;
const OP_UNARY_NUM_RELU: i16 = 101;
const OP_UNARY_NUM_SIGMOID: i16 = 102;
const OP_UNARY_NUM_GELU: i16 = 103;
const OP_UNARY_NUM_GELU_ERF: i16 = 104;
const OP_UNARY_NUM_GELU_QUICK: i16 = 105;
const OP_UNARY_NUM_SILU: i16 = 106;
const OP_UNARY_NUM_ELU: i16 = 107;
const OP_UNARY_NUM_NEG: i16 = 108;
const OP_UNARY_NUM_ABS: i16 = 109;
const OP_UNARY_NUM_SGN: i16 = 110;
const OP_UNARY_NUM_STEP: i16 = 111;
const OP_UNARY_NUM_HARDSWISH: i16 = 112;
const OP_UNARY_NUM_HARDSIGMOID: i16 = 113;
const OP_UNARY_NUM_EXP: i16 = 114;
const OP_UNARY_NUM_SOFTPLUS: i16 = 115;
const OP_UNARY_NUM_EXPM1: i16 = 116;
const OP_UNARY_NUM_FLOOR: i16 = 117;
const OP_UNARY_NUM_CEIL: i16 = 118;
const OP_UNARY_NUM_ROUND: i16 = 119;
const OP_UNARY_NUM_TRUNC: i16 = 120;

const OP_SUM_ROWS_NUM_SUM_ROWS: i16 = 10;
const OP_SUM_ROWS_NUM_MEAN: i16 = 11;

const N_R0_Q4_0: i32 = 4;
const N_SG_Q4_0: i32 = 2;
const N_R0_Q4_1: i32 = 4;
const N_SG_Q4_1: i32 = 2;
const N_R0_Q5_0: i32 = 4;
const N_SG_Q5_0: i32 = 2;
const N_R0_Q5_1: i32 = 4;
const N_SG_Q5_1: i32 = 2;
const N_R0_Q8_0: i32 = 2;
const N_SG_Q8_0: i32 = 4;
const N_R0_MXFP4: i32 = 2;
const N_SG_MXFP4: i32 = 2;
const N_R0_Q2_K: i32 = 4;
const N_SG_Q2_K: i32 = 2;
const N_R0_Q3_K: i32 = 2;
const N_SG_Q3_K: i32 = 2;
const N_R0_Q4_K: i32 = 2;
const N_SG_Q4_K: i32 = 2;
const N_R0_Q5_K: i32 = 1;
const N_SG_Q5_K: i32 = 2;
const N_R0_Q6_K: i32 = 2;
const N_SG_Q6_K: i32 = 2;
const N_R0_IQ1_S: i32 = 4;
const N_SG_IQ1_S: i32 = 2;
const N_R0_IQ1_M: i32 = 4;
const N_SG_IQ1_M: i32 = 2;
const N_R0_IQ2_XXS: i32 = 4;
const N_SG_IQ2_XXS: i32 = 2;
const N_R0_IQ2_XS: i32 = 4;
const N_SG_IQ2_XS: i32 = 2;
const N_R0_IQ2_S: i32 = 4;
const N_SG_IQ2_S: i32 = 2;
const N_R0_IQ3_XXS: i32 = 4;
const N_SG_IQ3_XXS: i32 = 2;
const N_R0_IQ3_S: i32 = 4;
const N_SG_IQ3_S: i32 = 2;
const N_R0_IQ4_NL: i32 = 2;
const N_SG_IQ4_NL: i32 = 2;
const N_R0_IQ4_XS: i32 = 2;
const N_SG_IQ4_XS: i32 = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MetalStageKind {
    Main,
    Copy,
    Aux,
    Merge,
    Reduce,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MetalPipelineSpec {
    pub kind: MetalStageKind,
    pub descriptor: MetalPipelineDescriptor,
    pub c4: bool,
    pub cnt: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MetalOpResources {
    pub output_tail_bytes: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MetalOpProgram {
    pub stages: Vec<MetalPipelineSpec>,
    pub resources: MetalOpResources,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MetalGraphNodePlan {
    pub node_id: NodeId,
    pub program: MetalOpProgram,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MetalGraphPlan {
    pub nodes: Vec<MetalGraphNodePlan>,
    pub total_output_tail_bytes: usize,
}

pub fn supports_program(tensors: &[Tensor], op: &Tensor, features: MetalDeviceFeatures) -> bool {
    build_program(tensors, op, features).is_ok()
}

pub fn build_program(
    tensors: &[Tensor],
    op: &Tensor,
    features: MetalDeviceFeatures,
) -> Result<MetalOpProgram, String> {
    match op.op {
        Op::Concat => program_concat(),
        Op::Add | Op::Sub | Op::Mul | Op::Div => program_bin(tensors, op, 1),
        Op::AddId => program_add_id(),
        Op::Repeat => program_repeat(tensors, op),
        Op::Acc => program_acc(tensors, op),
        Op::Scale
        | Op::Fill
        | Op::Clamp
        | Op::LeakyRelu
        | Op::Sqr
        | Op::Sqrt
        | Op::Sin
        | Op::Cos
        | Op::Log
        | Op::Unary => program_unary(tensors, op),
        Op::Glu => program_glu(tensors, op),
        Op::Sum => program_sum(tensors, op),
        Op::SumRows | Op::Mean => program_sum_rows(tensors, op),
        Op::CumSum => program_cumsum(tensors, op),
        Op::SoftMax => program_soft_max(tensors, op),
        Op::SsmConv => program_ssm_conv(tensors, op),
        Op::SsmScan => program_ssm_scan(tensors, op),
        Op::RwkvWkv6 | Op::RwkvWkv7 => program_rwkv(tensors, op),
        Op::GatedDeltaNet => program_gated_delta_net(tensors, op),
        Op::SolveTri => program_solve_tri(tensors, op),
        Op::MulMat => program_mul_mat(tensors, op, features),
        Op::MulMatId => program_mul_mat_id(tensors, op, features),
        Op::GetRows => program_get_rows(tensors, op),
        Op::SetRows => program_set_rows(tensors, op),
        Op::Diag => program_diag(tensors, op),
        Op::L2Norm => program_l2_norm(tensors, op),
        Op::GroupNorm => program_group_norm(),
        Op::Norm | Op::RmsNorm => program_norm(tensors, op, 1),
        Op::Rope => program_rope(tensors, op),
        Op::Im2col => program_im2col(op),
        Op::Conv2d => program_conv2d(tensors, op),
        Op::ConvTranspose1d => program_conv_transpose_1d(tensors, op),
        Op::ConvTranspose2d => program_conv_transpose_2d(tensors, op),
        Op::Conv3d => program_conv3d(tensors, op),
        Op::Upscale => program_upscale(tensors, op),
        Op::Pad => program_pad(tensors, op),
        Op::PadReflect1d => program_pad_reflect_1d(tensors, op),
        Op::Arange => program_arange(op),
        Op::TimestepEmbedding => program_timestep_embedding(tensors, op),
        Op::Argsort => program_argsort(tensors, op),
        Op::TopK => program_top_k(tensors, op),
        Op::Tri => program_tri(tensors, op),
        Op::FlashAttnExt => program_flash_attn_ext(tensors, op),
        Op::Set => program_set(tensors, op),
        Op::Dup | Op::Cpy | Op::Cont => program_cpy(tensors, op),
        Op::Pool1d => program_pool_1d(tensors, op),
        Op::Pool2d => program_pool_2d(tensors, op),
        Op::ArgMax => program_argmax(tensors, op),
        Op::OptStepAdamw => program_opt_step_adamw(tensors, op),
        Op::OptStepSgd => program_opt_step_sgd(tensors, op),
        Op::CountEqual => program_count_equal(tensors, op),
        _ => Err(format!(
            "Metal selector does not support ggml op {} yet",
            op.op.name()
        )),
    }
}

pub fn build_graph_plan(
    ctx: &Context,
    graph: &Graph,
    features: MetalDeviceFeatures,
) -> Result<MetalGraphPlan, String> {
    let tensors = ctx.tensors();
    let mut nodes = Vec::with_capacity(graph.nodes.len());
    let mut total_output_tail_bytes = 0usize;

    for &node_id in &graph.nodes {
        let op = tensors
            .get(node_id)
            .ok_or_else(|| format!("graph references invalid tensor {}", node_id))?;
        if is_metadata_only_op(op.op) {
            continue;
        }
        let program = build_program(tensors, op, features)?;
        total_output_tail_bytes = total_output_tail_bytes
            .checked_add(program.resources.output_tail_bytes)
            .ok_or_else(|| "Metal graph tail-byte accounting overflow".to_string())?;
        nodes.push(MetalGraphNodePlan { node_id, program });
    }

    Ok(MetalGraphPlan {
        nodes,
        total_output_tail_bytes,
    })
}

fn is_metadata_only_op(op: Op) -> bool {
    matches!(op, Op::View | Op::Reshape | Op::Permute | Op::Transpose)
}

fn program_concat() -> Result<MetalOpProgram, String> {
    Ok(program_with_stage(stage_simple(
        MetalStageKind::Main,
        "kernel_concat",
        "kernel_concat",
    )))
}

fn program_add_id() -> Result<MetalOpProgram, String> {
    Ok(program_with_stage(stage_simple(
        MetalStageKind::Main,
        "kernel_add_id",
        "kernel_add_id",
    )))
}

fn program_repeat(tensors: &[Tensor], op: &Tensor) -> Result<MetalOpProgram, String> {
    let src0 = src(tensors, op, 0)?;
    let base = format!("kernel_repeat_{}", src0.desc.ty.name());
    Ok(program_with_stage(stage_simple(
        MetalStageKind::Main,
        &base,
        &base,
    )))
}

fn program_pool_1d(tensors: &[Tensor], op: &Tensor) -> Result<MetalOpProgram, String> {
    let src0 = src(tensors, op, 0)?;
    let pool = pool_name(op.op_param_i32(0))?;
    let base = format!("kernel_pool_1d_{}_{}", pool, src0.desc.ty.name());
    Ok(program_with_stage(stage_simple(
        MetalStageKind::Main,
        &base,
        &base,
    )))
}

fn program_pool_2d(tensors: &[Tensor], op: &Tensor) -> Result<MetalOpProgram, String> {
    let src0 = src(tensors, op, 0)?;
    let pool = pool_name(op.op_param_i32(0))?;
    let base = format!("kernel_pool_2d_{}_{}", pool, src0.desc.ty.name());
    Ok(program_with_stage(stage_simple(
        MetalStageKind::Main,
        &base,
        &base,
    )))
}

fn program_get_rows(tensors: &[Tensor], op: &Tensor) -> Result<MetalOpProgram, String> {
    let src0 = src(tensors, op, 0)?;
    let base = format!("kernel_get_rows_{}", src0.desc.ty.name());
    Ok(program_with_stage(stage_simple(
        MetalStageKind::Main,
        &base,
        &base,
    )))
}

fn program_set_rows(tensors: &[Tensor], op: &Tensor) -> Result<MetalOpProgram, String> {
    let idx = src(tensors, op, 1)?;
    let base = format!(
        "kernel_set_rows_{}_{}",
        op.desc.ty.name(),
        idx.desc.ty.name()
    );
    Ok(program_with_stage(stage_simple(
        MetalStageKind::Main,
        &base,
        &base,
    )))
}

fn program_diag(tensors: &[Tensor], op: &Tensor) -> Result<MetalOpProgram, String> {
    let src0 = src(tensors, op, 0)?;
    let base = format!("kernel_diag_{}", src0.desc.ty.name());
    let cache = format!("{}_n={}", base, src0.ne[0]);
    Ok(program_with_stage(stage(
        MetalStageKind::Main,
        &base,
        &cache,
        Vec::new(),
        0,
        0,
        0,
        1,
        false,
        false,
    )))
}

fn program_unary(tensors: &[Tensor], op: &Tensor) -> Result<MetalOpProgram, String> {
    let src0 = src(tensors, op, 0)?;
    let op_num = unary_num(op)?;
    let is_c4 = src0.ne[0] % 4 == 0;
    let is_cnt = src0.is_contiguous() && op.nelements() < 32_768;
    let suffix = if is_c4 { "_4" } else { "" };
    let base = format!(
        "kernel_unary_{}_{}{}",
        src0.desc.ty.name(),
        op.desc.ty.name(),
        suffix
    );
    let cache = format!("{}_op={}_cnt={}", base, op_num, bool_num(is_cnt));
    Ok(program_with_stage(stage(
        MetalStageKind::Main,
        &base,
        &cache,
        vec![
            i16_const(FC_UNARY + 0, op_num),
            bool_const(FC_UNARY + 1, is_cnt),
        ],
        0,
        0,
        0,
        0,
        is_c4,
        is_cnt,
    )))
}

fn program_glu(tensors: &[Tensor], op: &Tensor) -> Result<MetalOpProgram, String> {
    let src0 = src(tensors, op, 0)?;
    let op_name = glu_kernel_name(op)?;
    let base = format!("kernel_{}_{}", op_name, src0.desc.ty.name());
    Ok(program_with_stage(stage_simple(
        MetalStageKind::Main,
        &base,
        &base,
    )))
}

fn program_sum(tensors: &[Tensor], op: &Tensor) -> Result<MetalOpProgram, String> {
    let src0 = src(tensors, op, 0)?;
    let base = format!("kernel_op_sum_{}", src0.desc.ty.name());
    Ok(program_with_stage(stage_simple(
        MetalStageKind::Main,
        &base,
        &base,
    )))
}

fn program_sum_rows(tensors: &[Tensor], op: &Tensor) -> Result<MetalOpProgram, String> {
    let src0 = src(tensors, op, 0)?;
    let op_num = match op.op {
        Op::SumRows => OP_SUM_ROWS_NUM_SUM_ROWS,
        Op::Mean => OP_SUM_ROWS_NUM_MEAN,
        _ => return Err(format!("expected sum_rows/mean, got {}", op.op.name())),
    };
    let is_c4 = src0.ne[0] % 4 == 0;
    let suffix = if is_c4 { "_4" } else { "" };
    let base = format!(
        "kernel_sum_rows_{}_{}{}",
        src0.desc.ty.name(),
        op.desc.ty.name(),
        suffix
    );
    let cache = format!("{}_op={}", base, op_num);
    let mut smem = 32 * std::mem::size_of::<f32>();
    if is_c4 {
        smem *= 4;
    }
    Ok(program_with_stage(stage(
        MetalStageKind::Main,
        &base,
        &cache,
        vec![i16_const(FC_SUM_ROWS + 0, op_num)],
        smem,
        0,
        0,
        0,
        is_c4,
        false,
    )))
}

fn program_cumsum(tensors: &[Tensor], op: &Tensor) -> Result<MetalOpProgram, String> {
    let src0 = src(tensors, op, 0)?;
    let ty = src0.desc.ty.name();
    Ok(MetalOpProgram {
        stages: vec![
            stage_simple(
                MetalStageKind::Aux,
                &format!("kernel_cumsum_blk_{ty}"),
                &format!("kernel_cumsum_blk_{ty}"),
            ),
            stage_simple(
                MetalStageKind::Main,
                &format!("kernel_cumsum_add_{ty}"),
                &format!("kernel_cumsum_add_{ty}"),
            ),
        ],
        resources: MetalOpResources {
            output_tail_bytes: op.nbytes(),
        },
    })
}

fn program_soft_max(tensors: &[Tensor], op: &Tensor) -> Result<MetalOpProgram, String> {
    let src0 = src(tensors, op, 0)?;
    let src1_ty = src_opt(tensors, op, 1)
        .map(|tensor| tensor.desc.ty)
        .unwrap_or(TensorType::F32);
    let suffix = if src0.ne[0] % 4 == 0 { "_4" } else { "" };
    let base = format!("kernel_soft_max_{}{}", src1_ty.name(), suffix);
    Ok(program_with_stage(stage(
        MetalStageKind::Main,
        &base,
        &base,
        Vec::new(),
        32 * std::mem::size_of::<f32>(),
        0,
        0,
        0,
        false,
        false,
    )))
}

fn program_ssm_conv(tensors: &[Tensor], op: &Tensor) -> Result<MetalOpProgram, String> {
    let src0 = src(tensors, op, 0)?;
    let src1 = src(tensors, op, 1)?;
    let use_batched = op.ne[1] > 1;
    let suffix = if src1.ne[0] % 4 == 0 { "_4" } else { "" };
    if use_batched {
        let batch_size = match op.ne[1] {
            x if x > 128 => 256,
            x if x > 64 => 128,
            x if x > 32 => 64,
            x if x > 16 => 32,
            x if x > 8 => 16,
            x if x > 4 => 8,
            _ => 2,
        };
        let base = format!(
            "kernel_ssm_conv_{}_{}_batched{}",
            src0.desc.ty.name(),
            src1.desc.ty.name(),
            suffix
        );
        let cache = format!("{}_ssm_conv_bs={}", base, batch_size);
        return Ok(program_with_stage(stage(
            MetalStageKind::Main,
            &base,
            &cache,
            vec![i16_const(FC_SSM_CONV + 0, batch_size as i16)],
            0,
            0,
            0,
            0,
            false,
            false,
        )));
    }
    let base = format!(
        "kernel_ssm_conv_{}_{}{}",
        src0.desc.ty.name(),
        src1.desc.ty.name(),
        suffix
    );
    Ok(program_with_stage(stage_simple(
        MetalStageKind::Main,
        &base,
        &base,
    )))
}

fn program_ssm_scan(tensors: &[Tensor], op: &Tensor) -> Result<MetalOpProgram, String> {
    let src0 = src(tensors, op, 0)?;
    let nsg = i32::try_from((src0.ne[0] + 31) / 32)
        .map_err(|_| "ssm_scan nsg exceeds i32".to_string())?;
    let base = format!("kernel_ssm_scan_{}", src0.desc.ty.name());
    let cache = format!("{}_nsg={}", base, nsg);
    let smem = usize::try_from(nsg).map_err(|_| "ssm_scan nsg exceeds usize".to_string())?
        * (32 + 2)
        * std::mem::size_of::<f32>();
    Ok(program_with_stage(stage(
        MetalStageKind::Main,
        &base,
        &cache,
        Vec::new(),
        smem,
        0,
        0,
        0,
        false,
        false,
    )))
}

fn program_rwkv(tensors: &[Tensor], op: &Tensor) -> Result<MetalOpProgram, String> {
    let src0 = src(tensors, op, 0)?;
    let prefix = match op.op {
        Op::RwkvWkv6 => "kernel_rwkv_wkv6",
        Op::RwkvWkv7 => "kernel_rwkv_wkv7",
        _ => return Err(format!("expected rwkv op, got {}", op.op.name())),
    };
    let base = format!("{prefix}_{}", src0.desc.ty.name());
    Ok(program_with_stage(stage_simple(
        MetalStageKind::Main,
        &base,
        &base,
    )))
}

fn program_gated_delta_net(tensors: &[Tensor], op: &Tensor) -> Result<MetalOpProgram, String> {
    let src0 = src(tensors, op, 0)?;
    let src2 = src(tensors, op, 2)?;
    let src3 = src(tensors, op, 3)?;
    let ne20 =
        i16::try_from(src2.ne[0]).map_err(|_| "gated_delta_net ne20 exceeds i16".to_string())?;
    let ne30 =
        i16::try_from(src3.ne[0]).map_err(|_| "gated_delta_net ne30 exceeds i16".to_string())?;
    let nsg = i32::try_from(src2.ne[0] / 32)
        .map_err(|_| "gated_delta_net nsg exceeds i32".to_string())?;
    let base = format!("kernel_gated_delta_net_{}_{}", src0.desc.ty.name(), nsg);
    let cache = format!("{}_ne20={}_ne30={}", base, ne20, ne30);
    Ok(program_with_stage(stage(
        MetalStageKind::Main,
        &base,
        &cache,
        vec![
            i16_const(FC_GATED_DELTA_NET + 0, ne20),
            i16_const(FC_GATED_DELTA_NET + 1, ne30),
        ],
        0,
        0,
        0,
        nsg,
        false,
        false,
    )))
}

fn program_solve_tri(tensors: &[Tensor], op: &Tensor) -> Result<MetalOpProgram, String> {
    let src0 = src(tensors, op, 0)?;
    let src1 = src(tensors, op, 1)?;
    let nsg = 8_i16;
    let n = i16::try_from(src1.ne[1]).map_err(|_| "solve_tri n exceeds i16".to_string())?;
    let k = i16::try_from(src1.ne[0]).map_err(|_| "solve_tri k exceeds i16".to_string())?;
    let base = format!("kernel_solve_tri_{}", src0.desc.ty.name());
    let cache = format!("{}_nsg={}_n={}_k={}", base, nsg, n, k);
    let smem = ggml_pad(
        ggml_pad(n as usize, 32) * nsg as usize * std::mem::size_of::<f32>(),
        16,
    );
    Ok(program_with_stage(stage(
        MetalStageKind::Main,
        &base,
        &cache,
        vec![
            i16_const(FC_SOLVE_TRI + 0, nsg),
            i16_const(FC_SOLVE_TRI + 1, n),
            i16_const(FC_SOLVE_TRI + 2, k),
        ],
        smem,
        0,
        0,
        nsg as i32,
        false,
        false,
    )))
}

fn program_mul_mat(
    tensors: &[Tensor],
    op: &Tensor,
    features: MetalDeviceFeatures,
) -> Result<MetalOpProgram, String> {
    let src0 = src(tensors, op, 0)?;
    let src1 = src(tensors, op, 1)?;
    let ne00 = src0.ne[0];
    let ne11 = src1.ne[1];

    if use_mul_mv_ext(src0, src1) {
        let nsg = 2_i16;
        let nxpsg = if ne00 % 256 == 0 && ne11 < 3 {
            16_i16
        } else if ne00 % 128 == 0 {
            8_i16
        } else {
            4_i16
        };
        let r1ptg = match ne11 {
            2 => 2_i16,
            3 | 6 => 3_i16,
            4 | 7 | 8 => 4_i16,
            5 => 5_i16,
            _ => return Err(format!("unsupported mul_mv_ext batch size {}", ne11)),
        };
        let base = format!(
            "kernel_mul_mv_ext_{}_{}_r1_{}",
            src0.desc.ty.name(),
            src1.desc.ty.name(),
            r1ptg
        );
        let cache = format!("{}_nsg={}_nxpsg={}", base, nsg, nxpsg);
        return Ok(program_with_stage(stage(
            MetalStageKind::Main,
            &base,
            &cache,
            vec![
                i16_const(FC_MUL_MV + 0, nsg),
                i16_const(FC_MUL_MV + 1, nxpsg),
            ],
            0,
            0,
            0,
            nsg as i32,
            false,
            false,
        )));
    }

    if features.has_simdgroup_mm
        && ne00 >= 64
        && ne11 > 8
        && !src0.is_transposed()
        && !src1.is_transposed()
    {
        let bc_inp = src0.ne[0] % 32 != 0;
        let bc_out = op.ne[0] % 64 != 0 || op.ne[1] % 32 != 0;
        let base = format!(
            "kernel_mul_mm_{}_{}",
            src0.desc.ty.name(),
            src1.desc.ty.name()
        );
        let cache = format!("{}_bci={}_bco={}", base, bool_num(bc_inp), bool_num(bc_out));
        return Ok(program_with_stage(stage(
            MetalStageKind::Main,
            &base,
            &cache,
            vec![
                bool_const(FC_MUL_MM + 0, bc_inp),
                bool_const(FC_MUL_MM + 1, bc_out),
            ],
            if bc_out { 8192 } else { 4096 + 2048 },
            0,
            0,
            0,
            false,
            false,
        )));
    }

    let params = mul_mv_params(src0.desc.ty, src0.ne[0], true)?;
    let suffix = params.suffix;
    let base = format!(
        "kernel_mul_mv_{}_{}{}",
        src0.desc.ty.name(),
        src1.desc.ty.name(),
        suffix
    );
    let cache = format!("{}_nsg={}", base, params.nsg);
    Ok(program_with_stage(stage(
        MetalStageKind::Main,
        &base,
        &cache,
        vec![i16_const(FC_MUL_MV + 0, params.nsg as i16)],
        params.smem,
        params.nr0,
        params.nr1,
        params.nsg,
        false,
        false,
    )))
}

fn program_mul_mat_id(
    tensors: &[Tensor],
    op: &Tensor,
    features: MetalDeviceFeatures,
) -> Result<MetalOpProgram, String> {
    let src0 = src(tensors, op, 0)?;
    let src1 = src(tensors, op, 1)?;
    let src2 = src(tensors, op, 2)?;
    let ne00 = src0.ne[0];
    let ne02 = src0.ne[2];
    let ne20 = src2.ne[0];
    let ne21 = src2.ne[1];

    if features.has_simdgroup_mm && ne00 >= 64 && ne21 >= 32 {
        let map_base = format!("kernel_mul_mm_id_map0_ne20_{}", ne20);
        let map_cache = format!("{}_ne02={}", map_base, ne02);
        let bc_inp = src0.ne[0] % 32 != 0;
        let mm_base = format!(
            "kernel_mul_mm_id_{}_{}",
            src0.desc.ty.name(),
            src1.desc.ty.name()
        );
        let mm_cache = format!("{}_bci={}", mm_base, bool_num(bc_inp));
        return Ok(MetalOpProgram {
            stages: vec![
                stage(
                    MetalStageKind::Aux,
                    &map_base,
                    &map_cache,
                    Vec::new(),
                    usize::try_from(ne02 * ne20)
                        .map_err(|_| "mul_mm_id_map0 smem overflow".to_string())?
                        * std::mem::size_of::<u16>(),
                    0,
                    0,
                    0,
                    false,
                    false,
                ),
                stage(
                    MetalStageKind::Main,
                    &mm_base,
                    &mm_cache,
                    vec![bool_const(FC_MUL_MM + 0, bc_inp)],
                    8192,
                    0,
                    0,
                    0,
                    false,
                    false,
                ),
            ],
            resources: MetalOpResources {
                output_tail_bytes: mul_mat_id_extra_tpe(src0)? + mul_mat_id_extra_ids(src0, src2)?,
            },
        });
    }

    let params = mul_mv_params(src0.desc.ty, src0.ne[0], false)?;
    let base = format!(
        "kernel_mul_mv_id_{}_{}{}",
        src0.desc.ty.name(),
        src1.desc.ty.name(),
        params.suffix
    );
    let cache = format!("{}_nsg={}", base, params.nsg);
    Ok(program_with_stage(stage(
        MetalStageKind::Main,
        &base,
        &cache,
        vec![i16_const(FC_MUL_MV + 0, params.nsg as i16)],
        params.smem,
        params.nr0,
        params.nr1,
        params.nsg,
        false,
        false,
    )))
}

fn program_argmax(tensors: &[Tensor], op: &Tensor) -> Result<MetalOpProgram, String> {
    let src0 = src(tensors, op, 0)?;
    let base = format!("kernel_argmax_{}", src0.desc.ty.name());
    Ok(program_with_stage(stage(
        MetalStageKind::Main,
        &base,
        &base,
        Vec::new(),
        32 * (std::mem::size_of::<f32>() + std::mem::size_of::<i32>()),
        0,
        0,
        0,
        false,
        false,
    )))
}

fn program_argsort(tensors: &[Tensor], op: &Tensor) -> Result<MetalOpProgram, String> {
    let src0 = src(tensors, op, 0)?;
    let order = sort_name(op.op_param_i32(0))?;
    let base = format!(
        "kernel_argsort_{}_{}_{}",
        src0.desc.ty.name(),
        op.desc.ty.name(),
        order
    );
    let merge_base = format!(
        "kernel_argsort_merge_{}_{}_{}",
        src0.desc.ty.name(),
        op.desc.ty.name(),
        order
    );
    Ok(MetalOpProgram {
        stages: vec![
            stage_simple(MetalStageKind::Main, &base, &base),
            stage_simple(MetalStageKind::Merge, &merge_base, &merge_base),
        ],
        resources: MetalOpResources {
            output_tail_bytes: op.nbytes(),
        },
    })
}

fn program_top_k(tensors: &[Tensor], op: &Tensor) -> Result<MetalOpProgram, String> {
    let src0 = src(tensors, op, 0)?;
    let order = SortOrder::Desc.name();
    let base = format!(
        "kernel_argsort_{}_{}_{}",
        src0.desc.ty.name(),
        op.desc.ty.name(),
        order
    );
    let merge_base = format!(
        "kernel_argsort_merge_{}_{}_{}",
        src0.desc.ty.name(),
        op.desc.ty.name(),
        order
    );
    Ok(MetalOpProgram {
        stages: vec![
            stage_simple(MetalStageKind::Main, &base, &base),
            stage_simple(MetalStageKind::Merge, &merge_base, &merge_base),
        ],
        resources: MetalOpResources {
            output_tail_bytes: usize::try_from(src0.nelements())
                .map_err(|_| "top_k temp size overflow".to_string())?
                * std::mem::size_of::<i32>(),
        },
    })
}

fn program_bin(tensors: &[Tensor], op: &Tensor, n_fuse: i16) -> Result<MetalOpProgram, String> {
    let src0 = src(tensors, op, 0)?;
    let src1 = src(tensors, op, 1)?;
    let op_num = match op.op {
        Op::Add => 0_i16,
        Op::Sub => 1_i16,
        Op::Mul => 2_i16,
        Op::Div => 3_i16,
        _ => return Err(format!("expected binary op, got {}", op.op.name())),
    };
    let is_c4 = (src0.ne[0] % 4 == 0) && (src1.ne[0] % 4 == 0);
    let is_cb = src0.ne[0] != src1.ne[0];
    let is_rb = src0.is_contiguous()
        && src1.is_contiguous()
        && src1.nrows() == 1
        && op.nelements() < 65_536;
    let suffix = if is_c4 { "_4" } else { "" };
    let base = format!(
        "kernel_bin_fuse_{}_{}_{}{}",
        src0.desc.ty.name(),
        src1.desc.ty.name(),
        op.desc.ty.name(),
        suffix
    );
    let cache = format!(
        "{}_op={}_nf={}_rb={}_cb={}",
        base,
        op_num,
        n_fuse,
        bool_num(is_rb),
        bool_num(is_cb)
    );
    Ok(program_with_stage(stage(
        MetalStageKind::Main,
        &base,
        &cache,
        vec![
            i16_const(FC_BIN + 0, op_num),
            i16_const(FC_BIN + 1, n_fuse),
            bool_const(FC_BIN + 2, is_rb),
            bool_const(FC_BIN + 3, is_cb),
        ],
        0,
        0,
        0,
        0,
        is_c4,
        is_rb,
    )))
}

fn program_bin_one_add() -> MetalPipelineSpec {
    stage(
        MetalStageKind::Main,
        "kernel_bin_fuse_f32_f32_f32",
        "kernel_bin_fuse_f32_f32_f32_op=0_nf=1",
        vec![
            i16_const(FC_BIN + 0, 0),
            i16_const(FC_BIN + 1, 1),
            bool_const(FC_BIN + 2, false),
        ],
        0,
        0,
        0,
        0,
        false,
        false,
    )
}

fn program_l2_norm(tensors: &[Tensor], op: &Tensor) -> Result<MetalOpProgram, String> {
    let src0 = src(tensors, op, 0)?;
    let is_c4 = src0.ne[0] % 4 == 0;
    let suffix = if is_c4 { "_4" } else { "" };
    let base = format!(
        "kernel_l2_norm_{}_{}{}",
        src0.desc.ty.name(),
        op.desc.ty.name(),
        suffix
    );
    Ok(program_with_stage(stage(
        MetalStageKind::Main,
        &base,
        &base,
        Vec::new(),
        32 * std::mem::size_of::<f32>(),
        0,
        0,
        0,
        is_c4,
        false,
    )))
}

fn program_group_norm() -> Result<MetalOpProgram, String> {
    Ok(program_with_stage(stage(
        MetalStageKind::Main,
        "kernel_group_norm_f32",
        "kernel_group_norm_f32",
        Vec::new(),
        32 * std::mem::size_of::<f32>(),
        0,
        0,
        0,
        false,
        false,
    )))
}

fn program_norm(tensors: &[Tensor], op: &Tensor, n_fuse: i32) -> Result<MetalOpProgram, String> {
    let suffix = if op.ne[0] % 4 == 0 { "_4" } else { "" };
    let base = match op.op {
        Op::Norm => match n_fuse {
            1 => format!("kernel_norm_f32{suffix}"),
            2 => format!("kernel_norm_mul_f32{suffix}"),
            3 => format!("kernel_norm_mul_add_f32{suffix}"),
            _ => return Err(format!("unsupported norm fusion count {}", n_fuse)),
        },
        Op::RmsNorm => match n_fuse {
            1 => format!("kernel_rms_norm_f32{suffix}"),
            2 => format!("kernel_rms_norm_mul_f32{suffix}"),
            3 => format!("kernel_rms_norm_mul_add_f32{suffix}"),
            _ => return Err(format!("unsupported rms_norm fusion count {}", n_fuse)),
        },
        _ => return Err(format!("expected norm op, got {}", op.op.name())),
    };
    let _ = src(tensors, op, 0)?;
    Ok(program_with_stage(stage(
        MetalStageKind::Main,
        &base,
        &base,
        Vec::new(),
        32 * std::mem::size_of::<f32>(),
        0,
        0,
        0,
        false,
        false,
    )))
}

fn program_rope(tensors: &[Tensor], op: &Tensor) -> Result<MetalOpProgram, String> {
    let src0 = src(tensors, op, 0)?;
    let mode = op.op_param_i32(2);
    let is_neox = mode & GGML_ROPE_TYPE_NEOX != 0;
    let is_mrope = mode & GGML_ROPE_TYPE_MROPE != 0;
    let is_imrope = mode == GGML_ROPE_TYPE_IMROPE;
    let is_vision = mode == GGML_ROPE_TYPE_VISION;
    let base = if is_neox {
        format!("kernel_rope_neox_{}", src0.desc.ty.name())
    } else if (is_mrope || is_imrope) && !is_vision {
        format!("kernel_rope_multi_{}", src0.desc.ty.name())
    } else if is_vision {
        format!("kernel_rope_vision_{}", src0.desc.ty.name())
    } else {
        format!("kernel_rope_norm_{}", src0.desc.ty.name())
    };
    let cache = format!("{}_imrope={}", base, bool_num(is_imrope));
    Ok(program_with_stage(stage(
        MetalStageKind::Main,
        &base,
        &cache,
        vec![bool_const(FC_ROPE + 0, is_imrope)],
        0,
        0,
        0,
        0,
        false,
        false,
    )))
}

fn program_im2col(op: &Tensor) -> Result<MetalOpProgram, String> {
    let base = format!("kernel_im2col_{}", op.desc.ty.name());
    Ok(program_with_stage(stage_simple(
        MetalStageKind::Main,
        &base,
        &base,
    )))
}

fn program_conv_transpose_1d(tensors: &[Tensor], op: &Tensor) -> Result<MetalOpProgram, String> {
    let src0 = src(tensors, op, 0)?;
    let src1 = src(tensors, op, 1)?;
    let base = format!(
        "kernel_conv_transpose_1d_{}_{}",
        src0.desc.ty.name(),
        src1.desc.ty.name()
    );
    Ok(program_with_stage(stage_simple(
        MetalStageKind::Main,
        &base,
        &base,
    )))
}

fn program_conv_transpose_2d(tensors: &[Tensor], op: &Tensor) -> Result<MetalOpProgram, String> {
    let src0 = src(tensors, op, 0)?;
    let src1 = src(tensors, op, 1)?;
    let base = format!(
        "kernel_conv_transpose_2d_{}_{}",
        src0.desc.ty.name(),
        src1.desc.ty.name()
    );
    Ok(program_with_stage(stage_simple(
        MetalStageKind::Main,
        &base,
        &base,
    )))
}

fn program_conv2d(tensors: &[Tensor], op: &Tensor) -> Result<MetalOpProgram, String> {
    let src0 = src(tensors, op, 0)?;
    let src1 = src(tensors, op, 1)?;
    let base = format!(
        "kernel_conv_2d_{}_{}",
        src0.desc.ty.name(),
        src1.desc.ty.name()
    );
    Ok(program_with_stage(stage_simple(
        MetalStageKind::Main,
        &base,
        &base,
    )))
}

fn program_conv3d(tensors: &[Tensor], op: &Tensor) -> Result<MetalOpProgram, String> {
    let src0 = src(tensors, op, 0)?;
    let src1 = src(tensors, op, 1)?;
    let base = format!(
        "kernel_conv_3d_{}_{}",
        src0.desc.ty.name(),
        src1.desc.ty.name()
    );
    Ok(program_with_stage(stage_simple(
        MetalStageKind::Main,
        &base,
        &base,
    )))
}

fn program_upscale(tensors: &[Tensor], op: &Tensor) -> Result<MetalOpProgram, String> {
    let src0 = src(tensors, op, 0)?;
    let mode_flags = op.op_param_i32(0);
    let mode = ScaleMode::from_i32(mode_flags & 0xFF)
        .ok_or_else(|| format!("unsupported upscale mode {}", mode_flags & 0xFF))?;
    let antialias = (mode_flags & GGML_SCALE_FLAG_ANTIALIAS) != 0;
    let base = match mode {
        ScaleMode::Nearest => format!("kernel_upscale_nearest_{}", src0.desc.ty.name()),
        ScaleMode::Bilinear => format!("kernel_upscale_bilinear_{}", src0.desc.ty.name()),
        ScaleMode::Bicubic => format!("kernel_upscale_bicubic_{}", src0.desc.ty.name()),
    };
    let cache = format!("{}_aa={}", base, bool_num(antialias));
    Ok(program_with_stage(stage(
        MetalStageKind::Main,
        &base,
        &cache,
        vec![bool_const(FC_UPSCALE + 0, antialias)],
        0,
        0,
        0,
        0,
        false,
        false,
    )))
}

fn program_pad(tensors: &[Tensor], op: &Tensor) -> Result<MetalOpProgram, String> {
    let src0 = src(tensors, op, 0)?;
    let base = format!("kernel_pad_{}", src0.desc.ty.name());
    Ok(program_with_stage(stage_simple(
        MetalStageKind::Main,
        &base,
        &base,
    )))
}

fn program_pad_reflect_1d(tensors: &[Tensor], op: &Tensor) -> Result<MetalOpProgram, String> {
    let src0 = src(tensors, op, 0)?;
    let base = format!("kernel_pad_reflect_1d_{}", src0.desc.ty.name());
    Ok(program_with_stage(stage_simple(
        MetalStageKind::Main,
        &base,
        &base,
    )))
}

fn program_arange(op: &Tensor) -> Result<MetalOpProgram, String> {
    let base = format!("kernel_arange_{}", op.desc.ty.name());
    Ok(program_with_stage(stage_simple(
        MetalStageKind::Main,
        &base,
        &base,
    )))
}

fn program_timestep_embedding(tensors: &[Tensor], op: &Tensor) -> Result<MetalOpProgram, String> {
    let src0 = src(tensors, op, 0)?;
    let base = format!("kernel_timestep_embedding_{}", src0.desc.ty.name());
    Ok(program_with_stage(stage_simple(
        MetalStageKind::Main,
        &base,
        &base,
    )))
}

fn program_tri(tensors: &[Tensor], op: &Tensor) -> Result<MetalOpProgram, String> {
    let src0 = src(tensors, op, 0)?;
    let ttype = op.op_param_i32(0);
    let base = format!("kernel_tri_{}_{}", src0.desc.ty.name(), ttype);
    Ok(program_with_stage(stage_simple(
        MetalStageKind::Main,
        &base,
        &base,
    )))
}

fn program_opt_step_adamw(tensors: &[Tensor], op: &Tensor) -> Result<MetalOpProgram, String> {
    let src0 = src(tensors, op, 0)?;
    let base = format!("kernel_opt_step_adamw_{}", src0.desc.ty.name());
    Ok(program_with_stage(stage_simple(
        MetalStageKind::Main,
        &base,
        &base,
    )))
}

fn program_opt_step_sgd(tensors: &[Tensor], op: &Tensor) -> Result<MetalOpProgram, String> {
    let src0 = src(tensors, op, 0)?;
    let base = format!("kernel_opt_step_sgd_{}", src0.desc.ty.name());
    Ok(program_with_stage(stage_simple(
        MetalStageKind::Main,
        &base,
        &base,
    )))
}

fn program_count_equal(tensors: &[Tensor], op: &Tensor) -> Result<MetalOpProgram, String> {
    let src0 = src(tensors, op, 0)?;
    let mut nsg = 1_i16;
    while 32 * i64::from(nsg) < src0.ne[0] && nsg < 32 {
        nsg *= 2;
    }
    let base = format!("kernel_count_equal_{}", src0.desc.ty.name());
    let cache = format!("{}_nsg={}", base, nsg);
    Ok(program_with_stage(stage(
        MetalStageKind::Main,
        &base,
        &cache,
        vec![i16_const(FC_COUNT_EQUAL + 0, nsg)],
        0,
        0,
        0,
        nsg as i32,
        false,
        false,
    )))
}

fn program_cpy(tensors: &[Tensor], op: &Tensor) -> Result<MetalOpProgram, String> {
    let src0 = src(tensors, op, 0)?;
    Ok(program_with_stage(cpy_stage(
        MetalStageKind::Main,
        src0.desc.ty,
        op.desc.ty,
    )))
}

fn program_set(tensors: &[Tensor], op: &Tensor) -> Result<MetalOpProgram, String> {
    let src0 = src(tensors, op, 0)?;
    let src1 = src(tensors, op, 1)?;
    let inplace = op.op_param_i32(4) != 0;
    let mut stages = Vec::new();
    if !inplace {
        stages.push(cpy_stage(MetalStageKind::Copy, src0.desc.ty, op.desc.ty));
    }
    stages.push(cpy_stage(MetalStageKind::Main, src1.desc.ty, op.desc.ty));
    Ok(MetalOpProgram {
        stages,
        resources: MetalOpResources::default(),
    })
}

fn program_acc(tensors: &[Tensor], op: &Tensor) -> Result<MetalOpProgram, String> {
    let src0 = src(tensors, op, 0)?;
    let inplace = op.op_param_i32(4) != 0;
    let mut stages = Vec::new();
    if !inplace {
        stages.push(cpy_stage(MetalStageKind::Copy, src0.desc.ty, op.desc.ty));
    }
    stages.push(program_bin_one_add());
    Ok(MetalOpProgram {
        stages,
        resources: MetalOpResources::default(),
    })
}

fn program_flash_attn_ext(tensors: &[Tensor], op: &Tensor) -> Result<MetalOpProgram, String> {
    let q = src(tensors, op, 0)?;
    let k = src(tensors, op, 1)?;
    let v = src(tensors, op, 2)?;
    let mask = src_opt(tensors, op, 3);

    let has_mask = mask.is_some();
    let has_sinks = src_opt(tensors, op, 4).is_some();
    let max_bias = op.op_param_f32(1);
    let logit_softcap = op.op_param_f32(2);
    let has_bias = max_bias != 0.0;
    let has_scap = logit_softcap != 0.0;
    let use_vec = flash_attn_ext_use_vec(q);

    let extra_pad = flash_attn_ext_extra_pad(q, k, v, mask)?;
    let extra_blk = flash_attn_ext_extra_blk(q, mask)?;
    let extra_tmp = flash_attn_ext_extra_tmp(q, v)?;

    let mut stages = Vec::new();

    if use_vec {
        let ncpsg = OP_FLASH_ATTN_EXT_VEC_NCPSG;
        let has_kvpad = k.ne[1] % i64::from(ncpsg) != 0;
        if has_kvpad {
            let pad_base = "kernel_flash_attn_ext_pad";
            let pad_cache = format!("{}_mask={}_ncpsg={}", pad_base, bool_num(has_mask), ncpsg);
            stages.push(stage(
                MetalStageKind::Aux,
                pad_base,
                &pad_cache,
                vec![
                    bool_const(FC_FLASH_ATTN_EXT_PAD + 0, has_mask),
                    i32_const(FC_FLASH_ATTN_EXT_PAD + 25, ncpsg),
                ],
                0,
                0,
                0,
                0,
                false,
                false,
            ));
        }

        let dk = k.ne[0];
        let dv = v.ne[0];
        let ns10 = k.nb[1] / k.nb[0];
        let ns20 = v.nb[1] / v.nb[0];
        let mut nsg = 1_i32;
        let nwg = 32_i32;
        while 2 * i64::from(nwg) * i64::from(nsg) * i64::from(ncpsg) < k.ne[1] && nsg < 4 {
            nsg *= 2;
        }
        let vec_base = format!(
            "kernel_flash_attn_ext_vec_{}_dk{}_dv{}",
            k.desc.ty.name(),
            dk,
            dv
        );
        let vec_cache = format!(
            "{}_mask={}_sink={}_bias={}_scap={}_kvpad={}_ns10={}_ns20={}_nsg={}_nwg={}",
            vec_base,
            bool_num(has_mask),
            bool_num(has_sinks),
            bool_num(has_bias),
            bool_num(has_scap),
            bool_num(has_kvpad),
            ns10,
            ns20,
            nsg,
            nwg
        );
        stages.push(stage(
            MetalStageKind::Main,
            &vec_base,
            &vec_cache,
            vec![
                bool_const(FC_FLASH_ATTN_EXT_VEC + 0, has_mask),
                bool_const(FC_FLASH_ATTN_EXT_VEC + 1, has_sinks),
                bool_const(FC_FLASH_ATTN_EXT_VEC + 2, has_bias),
                bool_const(FC_FLASH_ATTN_EXT_VEC + 3, has_scap),
                bool_const(FC_FLASH_ATTN_EXT_VEC + 4, has_kvpad),
                i32_const(
                    FC_FLASH_ATTN_EXT_VEC + 20,
                    i32::try_from(ns10).map_err(|_| "flash_attn ns10 overflow".to_string())?,
                ),
                i32_const(
                    FC_FLASH_ATTN_EXT_VEC + 21,
                    i32::try_from(ns20).map_err(|_| "flash_attn ns20 overflow".to_string())?,
                ),
                i32_const(FC_FLASH_ATTN_EXT_VEC + 22, nsg),
                i32_const(FC_FLASH_ATTN_EXT_VEC + 23, nwg),
            ],
            flash_attn_vec_smem_bytes(
                usize::try_from(dk).map_err(|_| "flash_attn dk overflow".to_string())?,
                usize::try_from(dv).map_err(|_| "flash_attn dv overflow".to_string())?,
                nsg,
            ),
            0,
            0,
            nsg,
            false,
            false,
        ));

        let reduce_base = "kernel_flash_attn_ext_vec_reduce".to_string();
        let reduce_cache = format!("{}_dv={}_nwg={}", reduce_base, dv, nwg);
        stages.push(stage(
            MetalStageKind::Reduce,
            &reduce_base,
            &reduce_cache,
            vec![
                i32_const(
                    FC_FLASH_ATTN_EXT_VEC_REDUCE + 0,
                    i32::try_from(dv).map_err(|_| "flash_attn dv overflow".to_string())?,
                ),
                i32_const(FC_FLASH_ATTN_EXT_VEC_REDUCE + 1, nwg),
            ],
            0,
            0,
            0,
            0,
            false,
            false,
        ));
    } else {
        let nqptg = OP_FLASH_ATTN_EXT_NQPSG;
        let ncpsg = OP_FLASH_ATTN_EXT_NCPSG;
        let has_kvpad = k.ne[1] % i64::from(ncpsg) != 0;
        if has_kvpad {
            let pad_base = "kernel_flash_attn_ext_pad";
            let pad_cache = format!("{}_mask={}_ncpsg={}", pad_base, bool_num(has_mask), ncpsg);
            stages.push(stage(
                MetalStageKind::Aux,
                pad_base,
                &pad_cache,
                vec![
                    bool_const(FC_FLASH_ATTN_EXT_PAD + 0, has_mask),
                    i32_const(FC_FLASH_ATTN_EXT_PAD + 25, ncpsg),
                ],
                0,
                0,
                0,
                0,
                false,
                false,
            ));
        }
        if has_mask {
            let blk_base = "kernel_flash_attn_ext_blk";
            let blk_cache = format!("{}_nqptg={}_ncpsg={}", blk_base, nqptg, ncpsg);
            stages.push(stage(
                MetalStageKind::Aux,
                blk_base,
                &blk_cache,
                vec![
                    i32_const(FC_FLASH_ATTN_EXT_BLK + 24, nqptg),
                    i32_const(FC_FLASH_ATTN_EXT_BLK + 25, ncpsg),
                ],
                0,
                0,
                0,
                0,
                false,
                false,
            ));
        }

        let dk = k.ne[0];
        let dv = v.ne[0];
        let ns10 = k.nb[1] / k.nb[0];
        let ns20 = v.nb[1] / v.nb[0];
        let bc_mask = has_mask && mask.unwrap().ne[1] % 8 != 0;
        let nsg = if q.ne[0] >= 512 { 8_i32 } else { 4_i32 };
        let main_base = format!(
            "kernel_flash_attn_ext_{}_dk{}_dv{}",
            k.desc.ty.name(),
            dk,
            dv
        );
        let main_cache = format!(
            "{}_mask={}_sinks={}_bias={}_scap={}_kvpad={}_bcm={}_ns10={}_ns20={}_nsg={}",
            main_base,
            bool_num(has_mask),
            bool_num(has_sinks),
            bool_num(has_bias),
            bool_num(has_scap),
            bool_num(has_kvpad),
            bool_num(bc_mask),
            ns10,
            ns20,
            nsg
        );
        stages.push(stage(
            MetalStageKind::Main,
            &main_base,
            &main_cache,
            vec![
                bool_const(FC_FLASH_ATTN_EXT + 0, has_mask),
                bool_const(FC_FLASH_ATTN_EXT + 1, has_sinks),
                bool_const(FC_FLASH_ATTN_EXT + 2, has_bias),
                bool_const(FC_FLASH_ATTN_EXT + 3, has_scap),
                bool_const(FC_FLASH_ATTN_EXT + 4, has_kvpad),
                bool_const(FC_FLASH_ATTN_EXT + 10, bc_mask),
                i32_const(
                    FC_FLASH_ATTN_EXT + 20,
                    i32::try_from(ns10).map_err(|_| "flash_attn ns10 overflow".to_string())?,
                ),
                i32_const(
                    FC_FLASH_ATTN_EXT + 21,
                    i32::try_from(ns20).map_err(|_| "flash_attn ns20 overflow".to_string())?,
                ),
                i32_const(FC_FLASH_ATTN_EXT + 22, nsg),
            ],
            flash_attn_smem_bytes(
                usize::try_from(dk).map_err(|_| "flash_attn dk overflow".to_string())?,
                usize::try_from(dv).map_err(|_| "flash_attn dv overflow".to_string())?,
                nsg,
            ),
            0,
            0,
            nsg,
            false,
            false,
        ));
    }

    Ok(MetalOpProgram {
        stages,
        resources: MetalOpResources {
            output_tail_bytes: extra_pad + extra_blk + extra_tmp,
        },
    })
}

fn src<'a>(tensors: &'a [Tensor], op: &Tensor, index: usize) -> Result<&'a Tensor, String> {
    let id = op
        .src
        .get(index)
        .and_then(|src| *src)
        .ok_or_else(|| format!("op {} missing src{}", op.op.name(), index))?;
    tensors
        .get(id)
        .ok_or_else(|| format!("op {} references invalid tensor {}", op.op.name(), id))
}

fn src_opt<'a>(tensors: &'a [Tensor], op: &Tensor, index: usize) -> Option<&'a Tensor> {
    let id = op.src.get(index).and_then(|src| *src)?;
    tensors.get(id)
}

fn program_with_stage(stage: MetalPipelineSpec) -> MetalOpProgram {
    MetalOpProgram {
        stages: vec![stage],
        resources: MetalOpResources::default(),
    }
}

fn stage_simple(kind: MetalStageKind, base: &str, cache: &str) -> MetalPipelineSpec {
    stage(kind, base, cache, Vec::new(), 0, 0, 0, 0, false, false)
}

fn cpy_stage(kind: MetalStageKind, src: TensorType, dst: TensorType) -> MetalPipelineSpec {
    let base = format!("kernel_cpy_{}_{}", src.name(), dst.name());
    stage(kind, &base, &base, Vec::new(), 0, 0, 0, 0, false, false)
}

fn stage(
    kind: MetalStageKind,
    base_name: &str,
    cache_name: &str,
    constants: Vec<FunctionConstant>,
    smem_bytes: usize,
    nr0: i32,
    nr1: i32,
    nsg: i32,
    c4: bool,
    cnt: bool,
) -> MetalPipelineSpec {
    MetalPipelineSpec {
        kind,
        descriptor: MetalPipelineDescriptor {
            cache_name: cache_name.to_string(),
            base_name: base_name.to_string(),
            constants,
            smem_bytes,
            nr0,
            nr1,
            nsg,
        },
        c4,
        cnt,
    }
}

fn i16_const(idx: i32, value: i16) -> FunctionConstant {
    FunctionConstant {
        idx,
        value: FunctionConstantValue::Int16(value),
    }
}

fn i32_const(idx: i32, value: i32) -> FunctionConstant {
    FunctionConstant {
        idx,
        value: FunctionConstantValue::Int32(value),
    }
}

fn bool_const(idx: i32, value: bool) -> FunctionConstant {
    FunctionConstant {
        idx,
        value: FunctionConstantValue::Bool(value),
    }
}

fn bool_num(value: bool) -> i32 {
    if value {
        1
    } else {
        0
    }
}

fn pool_name(value: i32) -> Result<&'static str, String> {
    PoolOp::from_i32(value)
        .map(PoolOp::name)
        .ok_or_else(|| format!("unsupported pool op {}", value))
}

fn sort_name(value: i32) -> Result<&'static str, String> {
    SortOrder::from_i32(value)
        .map(SortOrder::name)
        .ok_or_else(|| format!("unsupported sort order {}", value))
}

fn glu_kernel_name(op: &Tensor) -> Result<&'static str, String> {
    let glu = op
        .glu_op()
        .ok_or_else(|| format!("GLU op missing subtype for {}", op.op.name()))?;
    Ok(match glu {
        GluOp::Reglu => "reglu",
        GluOp::Geglu => "geglu",
        GluOp::Swiglu => "swiglu",
        GluOp::SwigluOai => "swiglu_oai",
        GluOp::GegluErf => "geglu_erf",
        GluOp::GegluQuick => "geglu_quick",
    })
}

fn unary_num(op: &Tensor) -> Result<i16, String> {
    Ok(match op.op {
        Op::Scale => OP_UNARY_NUM_SCALE,
        Op::Fill => OP_UNARY_NUM_FILL,
        Op::Clamp => OP_UNARY_NUM_CLAMP,
        Op::Sqr => OP_UNARY_NUM_SQR,
        Op::Sqrt => OP_UNARY_NUM_SQRT,
        Op::Sin => OP_UNARY_NUM_SIN,
        Op::Cos => OP_UNARY_NUM_COS,
        Op::Log => OP_UNARY_NUM_LOG,
        Op::LeakyRelu => OP_UNARY_NUM_LEAKY_RELU,
        Op::Unary => match op
            .unary_op()
            .ok_or_else(|| "missing unary subtype".to_string())?
        {
            UnaryOp::Tanh => OP_UNARY_NUM_TANH,
            UnaryOp::Relu => OP_UNARY_NUM_RELU,
            UnaryOp::Sigmoid => OP_UNARY_NUM_SIGMOID,
            UnaryOp::Gelu => OP_UNARY_NUM_GELU,
            UnaryOp::GeluErf => OP_UNARY_NUM_GELU_ERF,
            UnaryOp::GeluQuick => OP_UNARY_NUM_GELU_QUICK,
            UnaryOp::Silu => OP_UNARY_NUM_SILU,
            UnaryOp::Elu => OP_UNARY_NUM_ELU,
            UnaryOp::Neg => OP_UNARY_NUM_NEG,
            UnaryOp::Abs => OP_UNARY_NUM_ABS,
            UnaryOp::Sgn => OP_UNARY_NUM_SGN,
            UnaryOp::Step => OP_UNARY_NUM_STEP,
            UnaryOp::Hardswish => OP_UNARY_NUM_HARDSWISH,
            UnaryOp::Hardsigmoid => OP_UNARY_NUM_HARDSIGMOID,
            UnaryOp::Exp => OP_UNARY_NUM_EXP,
            UnaryOp::SoftPlus => OP_UNARY_NUM_SOFTPLUS,
            UnaryOp::Expm1 => OP_UNARY_NUM_EXPM1,
            UnaryOp::Floor => OP_UNARY_NUM_FLOOR,
            UnaryOp::Ceil => OP_UNARY_NUM_CEIL,
            UnaryOp::Round => OP_UNARY_NUM_ROUND,
            UnaryOp::Trunc => OP_UNARY_NUM_TRUNC,
            other => {
                return Err(format!(
                    "Metal selector does not support unary op {} yet",
                    other.name()
                ))
            }
        },
        _ => return Err(format!("expected unary op, got {}", op.op.name())),
    })
}

fn use_mul_mv_ext(src0: &Tensor, src1: &Tensor) -> bool {
    src1.desc.ty == TensorType::F32
        && src0.ne[0] % 128 == 0
        && ((matches!(
            src0.desc.ty,
            TensorType::F32
                | TensorType::F16
                | TensorType::BF16
                | TensorType::Q4_0
                | TensorType::Q4_1
                | TensorType::Q5_0
                | TensorType::Q5_1
                | TensorType::Q8_0
                | TensorType::MXFP4
                | TensorType::IQ4Nl
        ) && (2..=8).contains(&src1.ne[1]))
            || (matches!(
                src0.desc.ty,
                TensorType::Q4K
                    | TensorType::Q5K
                    | TensorType::Q6K
                    | TensorType::Q2K
                    | TensorType::Q3K
            ) && (4..=8).contains(&src1.ne[1])))
}

struct MulMvParams {
    nsg: i32,
    nr0: i32,
    nr1: i32,
    smem: usize,
    suffix: &'static str,
}

fn mul_mv_params(ty: TensorType, ne00: i64, allow_short: bool) -> Result<MulMvParams, String> {
    let (nsg, nr0, nr1, smem, suffix) = match ty {
        TensorType::F32 | TensorType::F16 | TensorType::BF16 => {
            if allow_short && ne00 < 32 {
                (1, 32, 1, 0, "_short")
            } else {
                let nsg = i32::min(
                    4,
                    i32::try_from((ne00 + 127) / 128)
                        .map_err(|_| "mul_mv nsg overflow".to_string())?,
                );
                let suffix = if ne00 % 4 == 0 { "_4" } else { "" };
                (nsg, 2, 1, 32 * std::mem::size_of::<f32>() * 2, suffix)
            }
        }
        TensorType::Q4_0 => (N_SG_Q4_0, N_R0_Q4_0, 1, 0, ""),
        TensorType::Q4_1 => (N_SG_Q4_1, N_R0_Q4_1, 1, 0, ""),
        TensorType::Q5_0 => (N_SG_Q5_0, N_R0_Q5_0, 1, 0, ""),
        TensorType::Q5_1 => (N_SG_Q5_1, N_R0_Q5_1, 1, 0, ""),
        TensorType::Q8_0 => (
            N_SG_Q8_0,
            N_R0_Q8_0,
            1,
            32 * std::mem::size_of::<f32>() * usize::try_from(N_R0_Q8_0).unwrap_or(0),
            "",
        ),
        TensorType::MXFP4 => (
            N_SG_MXFP4,
            N_R0_MXFP4,
            1,
            32 * std::mem::size_of::<f32>(),
            "",
        ),
        TensorType::Q2K => (N_SG_Q2_K, N_R0_Q2_K, 1, 0, ""),
        TensorType::Q3K => (N_SG_Q3_K, N_R0_Q3_K, 1, 0, ""),
        TensorType::Q4K => (N_SG_Q4_K, N_R0_Q4_K, 1, 0, ""),
        TensorType::Q5K => (N_SG_Q5_K, N_R0_Q5_K, 1, 0, ""),
        TensorType::Q6K => (N_SG_Q6_K, N_R0_Q6_K, 1, 0, ""),
        TensorType::IQ2Xxs => (N_SG_IQ2_XXS, N_R0_IQ2_XXS, 1, 256 * 8 + 128, ""),
        TensorType::IQ2Xs => (N_SG_IQ2_XS, N_R0_IQ2_XS, 1, 512 * 8 + 128, ""),
        TensorType::IQ3Xxs => (N_SG_IQ3_XXS, N_R0_IQ3_XXS, 1, 256 * 4 + 128, ""),
        TensorType::IQ3S => (N_SG_IQ3_S, N_R0_IQ3_S, 1, 512 * 4, ""),
        TensorType::IQ2S => (N_SG_IQ2_S, N_R0_IQ2_S, 1, 0, ""),
        TensorType::IQ1S => (N_SG_IQ1_S, N_R0_IQ1_S, 1, 0, ""),
        TensorType::IQ1M => (N_SG_IQ1_M, N_R0_IQ1_M, 1, 0, ""),
        TensorType::IQ4Nl => (
            N_SG_IQ4_NL,
            N_R0_IQ4_NL,
            1,
            32 * std::mem::size_of::<f32>(),
            "",
        ),
        TensorType::IQ4Xs => (
            N_SG_IQ4_XS,
            N_R0_IQ4_XS,
            1,
            32 * std::mem::size_of::<f32>(),
            "",
        ),
        other => {
            return Err(format!(
                "unsupported Metal mul_mv source type {}",
                other.name()
            ))
        }
    };

    Ok(MulMvParams {
        nsg,
        nr0,
        nr1,
        smem,
        suffix,
    })
}

fn flash_attn_ext_use_vec(q: &Tensor) -> bool {
    q.ne[1] < 20 && q.ne[0] % 32 == 0
}

fn pad_to(v: usize, align: usize) -> usize {
    if align == 0 {
        return v;
    }
    let rem = v % align;
    if rem == 0 {
        v
    } else {
        v + (align - rem)
    }
}

fn flash_attn_smem_bytes(dk: usize, dv: usize, _nsg: i32) -> usize {
    let nqptg = OP_FLASH_ATTN_EXT_NQPSG as usize;
    let ncpsg = OP_FLASH_ATTN_EXT_NCPSG as usize;
    let words = nqptg.saturating_mul(dk + 2 * pad_to(dv, 64) + 2 * (2 * ncpsg));
    pad_to(words.saturating_mul(std::mem::size_of::<f32>() / 2), 16)
}

fn flash_attn_vec_smem_bytes(dk: usize, dv: usize, nsg: i32) -> usize {
    let ncpsg = OP_FLASH_ATTN_EXT_VEC_NCPSG as usize;
    let words =
        (pad_to(dk, 128) + 4 * ncpsg + 2 * pad_to(dv, 128)).saturating_mul(nsg.max(1) as usize);
    pad_to(words.saturating_mul(std::mem::size_of::<f32>() / 2), 16)
}

fn flash_attn_ext_extra_pad(
    q: &Tensor,
    k: &Tensor,
    v: &Tensor,
    mask: Option<&Tensor>,
) -> Result<usize, String> {
    let has_mask = mask.is_some();
    let has_kvpad = true;
    if !has_kvpad {
        return Ok(0);
    }
    let mask_bytes = if has_mask {
        ggml_type_size_for_type(TensorType::F16)
            * usize::try_from(mask.unwrap().ne[1] * mask.unwrap().ne[2] * mask.unwrap().ne[3])
                .map_err(|_| "flash_attn pad mask bytes overflow".to_string())?
    } else {
        0
    };
    let ncpsg = if flash_attn_ext_use_vec(q) {
        OP_FLASH_ATTN_EXT_VEC_NCPSG
    } else {
        OP_FLASH_ATTN_EXT_NCPSG
    };
    Ok(
        usize::try_from(ncpsg).map_err(|_| "flash_attn ncpsg overflow".to_string())?
            * (k.nb[1]
                * usize::try_from(k.ne[2] * k.ne[3])
                    .map_err(|_| "flash_attn k dims overflow".to_string())?
                + v.nb[1]
                    * usize::try_from(v.ne[2] * v.ne[3])
                        .map_err(|_| "flash_attn v dims overflow".to_string())?
                + mask_bytes),
    )
}

fn flash_attn_ext_extra_blk(q: &Tensor, mask: Option<&Tensor>) -> Result<usize, String> {
    let Some(mask) = mask else {
        return Ok(0);
    };
    let is_vec = flash_attn_ext_use_vec(q);
    let nqptg = if is_vec {
        OP_FLASH_ATTN_EXT_VEC_NQPSG
    } else {
        OP_FLASH_ATTN_EXT_NQPSG
    };
    let ncpsg = if is_vec {
        OP_FLASH_ATTN_EXT_VEC_NCPSG
    } else {
        OP_FLASH_ATTN_EXT_NCPSG
    };
    let ne1 = (q.ne[1] + i64::from(nqptg) - 1) / i64::from(nqptg);
    let ne0 = (mask.ne[0] + i64::from(ncpsg) - 1) / i64::from(ncpsg);
    let bytes = usize::try_from(ne0 * ne1 * mask.ne[2] * mask.ne[3])
        .map_err(|_| "flash_attn blk bytes overflow".to_string())?;
    Ok(ggml_pad(bytes * std::mem::size_of::<i8>(), 32))
}

fn flash_attn_ext_extra_tmp(q: &Tensor, v: &Tensor) -> Result<usize, String> {
    let nwg = 32_i64;
    let ne01_max = q.ne[1].min(32);
    let items = ne01_max * q.ne[2] * q.ne[3] * nwg * (v.ne[0] + 2);
    Ok(ggml_type_size_for_type(TensorType::F32)
        * usize::try_from(items).map_err(|_| "flash_attn tmp bytes overflow".to_string())?)
}

fn mul_mat_id_extra_tpe(src0: &Tensor) -> Result<usize, String> {
    Ok(ggml_type_size_for_type(TensorType::I32)
        * usize::try_from(src0.ne[2]).map_err(|_| "mul_mat_id tpe bytes overflow".to_string())?)
}

fn mul_mat_id_extra_ids(src0: &Tensor, src2: &Tensor) -> Result<usize, String> {
    Ok(ggml_type_size_for_type(TensorType::I32)
        * usize::try_from(src0.ne[2] * src2.ne[1])
            .map_err(|_| "mul_mat_id ids bytes overflow".to_string())?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::Context;
    use crate::core::{InitParams, GGML_ROPE_TYPE_NEOX, GGML_ROPE_TYPE_NORMAL};
    use crate::graph::Graph;
    use crate::tensor::{BufferUsage, TensorDesc, TensorLayout};

    fn ctx() -> Context {
        Context::new(InitParams {
            mem_size: 0,
            mem_buffer: None,
            no_alloc: true,
        })
    }

    fn tensor(ctx: &mut Context, ty: TensorType, ne: &[i64]) -> usize {
        let layout = TensorLayout::for_ggml(ty, ne).unwrap();
        let desc = TensorDesc::new(ty, layout, BufferUsage::Activations);
        ctx.new_op_tensor(desc, Op::None, &[]).unwrap()
    }

    #[test]
    fn unary_selector_matches_kernel_name() {
        let mut ctx = ctx();
        let src = tensor(&mut ctx, TensorType::F32, &[128, 1, 1, 1]);
        let dst = tensor(&mut ctx, TensorType::F32, &[128, 1, 1, 1]);
        let tensors = ctx.tensors().to_vec();
        let mut op = tensors[dst].clone();
        op.op = Op::Unary;
        op.src[0] = Some(src);
        op.set_unary_op(UnaryOp::Gelu);

        let program = build_program(&tensors, &op, MetalDeviceFeatures::default()).unwrap();
        assert_eq!(
            program.stages[0].descriptor.base_name,
            "kernel_unary_f32_f32_4"
        );
        assert_eq!(
            program.stages[0].descriptor.cache_name,
            "kernel_unary_f32_f32_4_op=103_cnt=1"
        );
    }

    #[test]
    fn rope_selector_picks_neox_kernel() {
        let mut ctx = ctx();
        let src = tensor(&mut ctx, TensorType::F32, &[128, 8, 1, 1]);
        let pos = tensor(&mut ctx, TensorType::I32, &[32, 1, 1, 1]);
        let dst = tensor(&mut ctx, TensorType::F32, &[128, 8, 1, 1]);
        let tensors = ctx.tensors().to_vec();
        let mut op = tensors[dst].clone();
        op.op = Op::Rope;
        op.src[0] = Some(src);
        op.src[1] = Some(pos);
        op.set_op_param_i32(2, GGML_ROPE_TYPE_NEOX);

        let program = build_program(&tensors, &op, MetalDeviceFeatures::default()).unwrap();
        assert_eq!(
            program.stages[0].descriptor.base_name,
            "kernel_rope_neox_f32"
        );
    }

    #[test]
    fn rope_selector_picks_normal_kernel() {
        let mut ctx = ctx();
        let src = tensor(&mut ctx, TensorType::F32, &[128, 8, 1, 1]);
        let pos = tensor(&mut ctx, TensorType::I32, &[32, 1, 1, 1]);
        let dst = tensor(&mut ctx, TensorType::F32, &[128, 8, 1, 1]);
        let tensors = ctx.tensors().to_vec();
        let mut op = tensors[dst].clone();
        op.op = Op::Rope;
        op.src[0] = Some(src);
        op.src[1] = Some(pos);
        op.set_op_param_i32(2, GGML_ROPE_TYPE_NORMAL);

        let program = build_program(&tensors, &op, MetalDeviceFeatures::default()).unwrap();
        assert_eq!(
            program.stages[0].descriptor.base_name,
            "kernel_rope_norm_f32"
        );
    }

    #[test]
    fn argsort_selector_adds_merge_and_temp() {
        let mut ctx = ctx();
        let src = tensor(&mut ctx, TensorType::F32, &[257, 4, 1, 1]);
        let dst = tensor(&mut ctx, TensorType::I32, &[257, 4, 1, 1]);
        let tensors = ctx.tensors().to_vec();
        let mut op = tensors[dst].clone();
        op.op = Op::Argsort;
        op.src[0] = Some(src);
        op.set_op_param_i32(0, SortOrder::Desc as i32);

        let program = build_program(&tensors, &op, MetalDeviceFeatures::default()).unwrap();
        assert_eq!(program.stages.len(), 2);
        assert_eq!(program.resources.output_tail_bytes, op.nbytes());
    }

    #[test]
    fn cumsum_selector_adds_temp_tail() {
        let mut ctx = ctx();
        let src = tensor(&mut ctx, TensorType::F32, &[257, 4, 1, 1]);
        let dst = tensor(&mut ctx, TensorType::F32, &[257, 4, 1, 1]);
        let tensors = ctx.tensors().to_vec();
        let mut op = tensors[dst].clone();
        op.op = Op::CumSum;
        op.src[0] = Some(src);

        let program = build_program(&tensors, &op, MetalDeviceFeatures::default()).unwrap();
        assert_eq!(program.stages.len(), 2);
        assert_eq!(program.resources.output_tail_bytes, op.nbytes());
    }

    #[test]
    fn mul_mat_selector_uses_mul_mm_when_feature_allows() {
        let mut ctx = ctx();
        let a = tensor(&mut ctx, TensorType::F16, &[128, 64, 1, 1]);
        let b = tensor(&mut ctx, TensorType::F32, &[128, 16, 1, 1]);
        let dst = tensor(&mut ctx, TensorType::F32, &[64, 16, 1, 1]);
        let tensors = ctx.tensors().to_vec();
        let mut op = tensors[dst].clone();
        op.op = Op::MulMat;
        op.src[0] = Some(a);
        op.src[1] = Some(b);

        let program = build_program(
            &tensors,
            &op,
            MetalDeviceFeatures {
                has_simdgroup_mm: true,
                ..MetalDeviceFeatures::default()
            },
        )
        .unwrap();
        assert_eq!(
            program.stages[0].descriptor.base_name,
            "kernel_mul_mm_f16_f32"
        );
    }

    #[test]
    fn flash_attn_selector_builds_vec_pipeline_and_reduce() {
        let mut ctx = ctx();
        let q = tensor(&mut ctx, TensorType::F32, &[128, 8, 2, 1]);
        let k = tensor(&mut ctx, TensorType::F16, &[128, 64, 2, 1]);
        let v = tensor(&mut ctx, TensorType::F16, &[128, 64, 2, 1]);
        let dst = tensor(&mut ctx, TensorType::F32, &[128, 8, 2, 1]);
        let tensors = ctx.tensors().to_vec();
        let mut op = tensors[dst].clone();
        op.op = Op::FlashAttnExt;
        op.src[0] = Some(q);
        op.src[1] = Some(k);
        op.src[2] = Some(v);

        let program = build_program(&tensors, &op, MetalDeviceFeatures::default()).unwrap();
        assert_eq!(program.stages.len(), 2);
        assert!(program.stages[0]
            .descriptor
            .base_name
            .starts_with("kernel_flash_attn_ext_vec_f16_dk128_dv128"));
        assert_eq!(
            program.stages[1].descriptor.base_name,
            "kernel_flash_attn_ext_vec_reduce"
        );
        assert!(program.resources.output_tail_bytes > 0);
    }

    #[test]
    fn graph_plan_accumulates_tail_bytes() {
        let mut ctx = ctx();
        let src = tensor(&mut ctx, TensorType::F32, &[257, 4, 1, 1]);
        let dst = tensor(&mut ctx, TensorType::I32, &[257, 4, 1, 1]);

        {
            let op = ctx.tensor_mut(dst).unwrap();
            op.op = Op::Argsort;
            op.src[0] = Some(src);
            op.set_op_param_i32(0, SortOrder::Asc as i32);
        }

        let mut graph = Graph::new();
        graph.add_node(dst);

        let plan = build_graph_plan(&ctx, &graph, MetalDeviceFeatures::default()).unwrap();
        assert_eq!(plan.nodes.len(), 1);
        assert_eq!(
            plan.total_output_tail_bytes,
            ctx.tensor(dst).unwrap().nbytes()
        );
    }

    #[test]
    fn graph_plan_skips_metadata_only_nodes() {
        let mut ctx = ctx();
        let src = tensor(&mut ctx, TensorType::F32, &[16, 8, 1, 1]);
        let view = ctx
            .view(src, TensorType::F32, &[8, 8], &[4, 64], 32)
            .unwrap();
        let cont = ctx.cont(view).unwrap();

        let mut graph = Graph::new();
        graph.build_forward_expand(&ctx, cont).unwrap();

        let plan = build_graph_plan(&ctx, &graph, MetalDeviceFeatures::default()).unwrap();
        assert_eq!(plan.nodes.len(), 1);
        assert_eq!(plan.nodes[0].node_id, cont);
    }
}
