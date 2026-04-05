use makepad_ggml::backend::metal::{
    BufferStorageMode, MetalBufferBindingRef, MetalPipelineDescriptor, MetalRuntime, MetalSize,
};
use makepad_mlx_rt_core::{
    fnv1a64_u32_words, gemma4_qproj_case_input_bf16_words, MlxSafetensorsHeader,
};
use std::env;
use std::error::Error;
use std::io::{self, Write};
use std::mem::size_of;
use std::path::PathBuf;
use std::slice;
use std::time::Instant;

const INPUT_NORM_WEIGHT_NAME: &str = "language_model.model.layers.0.input_layernorm.weight";
const Q_NORM_WEIGHT_NAME: &str = "language_model.model.layers.0.self_attn.q_norm.weight";
const K_NORM_WEIGHT_NAME: &str = "language_model.model.layers.0.self_attn.k_norm.weight";
const NORM_LEN: usize = 2_816;
const EPS: f32 = 1e-6;
const ROPE_BASE: f32 = 10_000.0;
const ROPE_SCALE: f32 = 1.0;
const ROPE_OFFSET: i32 = 17;

#[derive(Clone, Copy)]
struct ProjectionPathOracle {
    weight_name: &'static str,
    scales_name: &'static str,
    biases_name: &'static str,
    norm_weight_name: Option<&'static str>,
    expected_norm_hash: u64,
    expected_norm_first16_bits: [u32; 16],
    expected_rope_hash: Option<u64>,
    expected_rope_first16_bits: Option<[u32; 16]>,
}

const Q_PATH_ORACLE: ProjectionPathOracle = ProjectionPathOracle {
    weight_name: "language_model.model.layers.0.self_attn.q_proj.weight",
    scales_name: "language_model.model.layers.0.self_attn.q_proj.scales",
    biases_name: "language_model.model.layers.0.self_attn.q_proj.biases",
    norm_weight_name: Some(Q_NORM_WEIGHT_NAME),
    expected_norm_hash: 0xFDFC_27D5_C13C_170B,
    expected_norm_first16_bits: [
        0x3DA1_0000,
        0x40DC_0000,
        0x3F63_0000,
        0xBE80_0000,
        0x3F08_0000,
        0xBEFB_0000,
        0xBF65_0000,
        0x4000_0000,
        0x3F0C_0000,
        0x3C62_0000,
        0x3F25_0000,
        0x3F3E_0000,
        0xBE40_0000,
        0x3EDF_0000,
        0xBCD3_0000,
        0xC047_0000,
    ],
    expected_rope_hash: Some(0xCE41_D175_51C1_C0FA),
    expected_rope_first16_bits: Some([
        0xC027_0000,
        0xC0DE_0000,
        0xBF6C_0000,
        0xBEF8_0000,
        0x3EF2_0000,
        0xBFC8_0000,
        0xBF7C_0000,
        0x3E4E_0000,
        0xBF1E_0000,
        0x3E1D_0000,
        0xBF73_0000,
        0xBFE0_0000,
        0xBFA2_0000,
        0x3EAD_0000,
        0xBD06_0000,
        0xC047_0000,
    ]),
};

const K_PATH_ORACLE: ProjectionPathOracle = ProjectionPathOracle {
    weight_name: "language_model.model.layers.0.self_attn.k_proj.weight",
    scales_name: "language_model.model.layers.0.self_attn.k_proj.scales",
    biases_name: "language_model.model.layers.0.self_attn.k_proj.biases",
    norm_weight_name: Some(K_NORM_WEIGHT_NAME),
    expected_norm_hash: 0x77A8_4686_A3CE_50CB,
    expected_norm_first16_bits: [
        0x3EC9_0000,
        0xBDAC_0000,
        0xBC29_0000,
        0xBCAC_0000,
        0xBDB1_0000,
        0x3E36_0000,
        0xBDE4_0000,
        0xBE3B_0000,
        0x3D6A_0000,
        0x3E0B_0000,
        0x3DAF_0000,
        0xBDAA_0000,
        0xBDA3_0000,
        0x3D8B_0000,
        0x3CC7_0000,
        0x3DE5_0000,
    ],
    expected_rope_hash: Some(0x9731_B5D8_139C_BB3D),
    expected_rope_first16_bits: Some([
        0xBE4E_0000,
        0x3DD3_0000,
        0xBD8F_0000,
        0xBBC6_0000,
        0xBDA6_0000,
        0x3E0F_0000,
        0x3DCB_0000,
        0x3E35_0000,
        0xBD66_0000,
        0xBE66_0000,
        0xBD12_0000,
        0x3C18_0000,
        0xBDEC_0000,
        0x3D99_0000,
        0x3CAC_0000,
        0x3DD2_0000,
    ]),
};

const V_PATH_ORACLE: ProjectionPathOracle = ProjectionPathOracle {
    weight_name: "language_model.model.layers.0.self_attn.v_proj.weight",
    scales_name: "language_model.model.layers.0.self_attn.v_proj.scales",
    biases_name: "language_model.model.layers.0.self_attn.v_proj.biases",
    norm_weight_name: None,
    expected_norm_hash: 0xDAC6_F97C_1CD9_387D,
    expected_norm_first16_bits: [
        0xBE60_0000,
        0xBEC6_0000,
        0xBF89_0000,
        0xBF59_0000,
        0x3C84_0000,
        0xBF22_0000,
        0x3EAD_0000,
        0x3FA4_0000,
        0x3EA1_0000,
        0x3E11_0000,
        0x3E04_0000,
        0x3E2E_0000,
        0xBF02_0000,
        0x3F55_0000,
        0x3F78_0000,
        0x3E64_0000,
    ],
    expected_rope_hash: None,
    expected_rope_first16_bits: None,
};

const O_PROJ_WEIGHT_NAME: &str = "language_model.model.layers.0.self_attn.o_proj.weight";
const O_PROJ_SCALES_NAME: &str = "language_model.model.layers.0.self_attn.o_proj.scales";
const O_PROJ_BIASES_NAME: &str = "language_model.model.layers.0.self_attn.o_proj.biases";
const POST_ATTENTION_NORM_WEIGHT_NAME: &str =
    "language_model.model.layers.0.post_attention_layernorm.weight";
const PRE_FEEDFORWARD_NORM_WEIGHT_NAME: &str =
    "language_model.model.layers.0.pre_feedforward_layernorm.weight";
const MLP_GATE_WEIGHT_NAME: &str = "language_model.model.layers.0.mlp.gate_proj.weight";
const MLP_GATE_SCALES_NAME: &str = "language_model.model.layers.0.mlp.gate_proj.scales";
const MLP_GATE_BIASES_NAME: &str = "language_model.model.layers.0.mlp.gate_proj.biases";
const MLP_UP_WEIGHT_NAME: &str = "language_model.model.layers.0.mlp.up_proj.weight";
const MLP_UP_SCALES_NAME: &str = "language_model.model.layers.0.mlp.up_proj.scales";
const MLP_UP_BIASES_NAME: &str = "language_model.model.layers.0.mlp.up_proj.biases";

const EXPECTED_LOGITS_HASH: u64 = 0x2BF6_1C33_DE46_DDBE;
const EXPECTED_LOGITS_BITS: [u32; 16] = [
    0x4032_0000,
    0xC107_0000,
    0xC079_0000,
    0x3F94_0000,
    0x412B_0000,
    0x4146_0000,
    0x3EAA_0000,
    0xBFE9_0000,
    0xC078_0000,
    0xBFE9_0000,
    0xC0F40000,
    0x41960000,
    0x411F0000,
    0xC0890000,
    0x411A0000,
    0xBFA40000,
];
const EXPECTED_ATTN_OUT_HASH: u64 = 0xA13F_A961_56EA_7045;
const EXPECTED_ATTN_OUT_FIRST16_BITS: [u32; 16] = [
    0xBE60_0000,
    0xBEC6_0000,
    0xBF890000,
    0xBF590000,
    0x3C840000,
    0xBF220000,
    0x3EAD0000,
    0x3FA40000,
    0x3EA10000,
    0x3E110000,
    0x3E040000,
    0x3E2E0000,
    0xBF020000,
    0x3F550000,
    0x3F780000,
    0x3E640000,
];
const EXPECTED_O_PROJ_HASH: u64 = 0xE718_D50A_FB8F_60ED;
const EXPECTED_O_PROJ_ROW_1836_BITS: u32 = 0xC08C_0000;
const EXPECTED_O_PROJ_FIRST16_BITS: [u32; 16] = [
    0x3F1E_0000,
    0xC03E_0000,
    0xC036_0000,
    0xBE1A_0000,
    0xBFE6_0000,
    0x3D81_0000,
    0x3EC7_0000,
    0x3F85_0000,
    0xC0A8_0000,
    0x3FC9_0000,
    0x3FE4_0000,
    0xC02E_0000,
    0x4012_0000,
    0xC010_0000,
    0x4052_0000,
    0x408A_0000,
];
const EXPECTED_POST_ATTENTION_NORM_HASH: u64 = 0x8DA0_389E_F76D_4387;
const EXPECTED_POST_ATTENTION_NORM_FIRST16_BITS: [u32; 16] = [
    0x3E26_0000,
    0xBF1C_0000,
    0xBE1F_0000,
    0xBD5D_0000,
    0xBE62_0000,
    0x3C63_0000,
    0x3D1F_0000,
    0x3DD5_0000,
    0xC0B1_0000,
    0x3E18_0000,
    0x3E35_0000,
    0xC020_0000,
    0x3E0A_0000,
    0xBFFB_0000,
    0x3ECB_0000,
    0x3F05_0000,
];
const EXPECTED_POST_ATTENTION_RESIDUAL_HASH: u64 = 0xC598_EAB5_ED6E_F234;
const EXPECTED_POST_ATTENTION_RESIDUAL_FIRST16_BITS: [u32; 16] = [
    0xBF56_0000,
    0xBFAE_0000,
    0xBF28_0000,
    0xBE9C_0000,
    0xBE62_0000,
    0x3E87_0000,
    0x3F0A_0000,
    0x3F5B_0000,
    0xC091_0000,
    0x3F26_0000,
    0x3E35_0000,
    0xC040_0000,
    0xBF5E_0000,
    0xBFEB_0000,
    0x3F46_0000,
    0x3F92_0000,
];
const EXPECTED_PRE_FEEDFORWARD_NORM_HASH: u64 = 0x66B4_9D12_816F_6D20;
const EXPECTED_PRE_FEEDFORWARD_NORM_FIRST16_BITS: [u32; 16] = [
    0xBE83_0000,
    0xBED7_0000,
    0xBE40_0000,
    0xBDD0_0000,
    0xBD88_0000,
    0x3DAE_0000,
    0x3E18_0000,
    0x3E9F_0000,
    0xC003_0000,
    0x3E4A_0000,
    0x3D96_0000,
    0xBF5A_0000,
    0xBE9F_0000,
    0xBF15_0000,
    0x3EA6_0000,
    0x3ECF_0000,
];
const EXPECTED_PRE_FEEDFORWARD_GATE_HASH: u64 = 0x2093_7429_0E12_7C9C;
const EXPECTED_PRE_FEEDFORWARD_GATE_FIRST16_BITS: [u32; 16] = [
    0xBFEE_0000,
    0xBE19_0000,
    0xC012_0000,
    0x3F78_0000,
    0x3F66_0000,
    0x4004_0000,
    0x3F04_0000,
    0x3FB0_0000,
    0x3E07_0000,
    0x3FC3_0000,
    0xBFE8_0000,
    0xC035_0000,
    0x3D84_0000,
    0x4095_0000,
    0xBFC3_0000,
    0xBEE6_0000,
];
const EXPECTED_PRE_FEEDFORWARD_UP_HASH: u64 = 0x6890_EC02_1671_D897;
const EXPECTED_PRE_FEEDFORWARD_UP_FIRST16_BITS: [u32; 16] = [
    0xBEED_0000,
    0xC020_0000,
    0x3FAD_0000,
    0xBFAD_0000,
    0xBFCA_0000,
    0xC059_0000,
    0x402D_0000,
    0x3FA5_0000,
    0x3DE5_0000,
    0x3FB2_0000,
    0x3DC2_0000,
    0x3F82_0000,
    0x4019_0000,
    0xBFC0_0000,
    0x401A_0000,
    0x3F41_0000,
];
const EXPECTED_PRE_FEEDFORWARD_GEGLU_HASH: u64 = 0xF3F1_469F_EDE3_FB40;
const EXPECTED_PRE_FEEDFORWARD_GEGLU_FIRST16_BITS: [u32; 16] = [
    0x3CDC_0000,
    0x3E29_0000,
    0xBD14_0000,
    0xBF8C_0000,
    0xBF94_0000,
    0xC0DB_0000,
    0x3F79_0000,
    0x3FD0_0000,
    0x3C05_0000,
    0x3FFE_0000,
    0xBBC5_0000,
    0xBBB8_0000,
    0x3DA6_0000,
    0xC0E0_0000,
    0xBE72_0000,
    0xBDE4_0000,
];

#[repr(C)]
struct MlxRmsNormRowArgs {
    n: u32,
    eps: f32,
}

#[repr(C)]
struct MlxRmsNormRowsArgs {
    n: u32,
    row_stride: u32,
    row_count: u32,
    eps: f32,
}

#[repr(C)]
struct MlxAffineQprojRowArgs {
    n_in: u32,
    weight_words_per_row: u32,
    qparams_per_row: u32,
    out_rows: u32,
}

#[repr(C)]
struct MlxAffineQmvSingleRowArgs {
    n_in: u32,
    weight_words_per_row: u32,
    qparams_per_row: u32,
    row_index: u32,
}

#[repr(C)]
struct MlxReduceRowsF32Args {
    n: u32,
    row_stride: u32,
    row_count: u32,
}

#[repr(C)]
struct MlxRopeSingleArgs {
    half_dims: u32,
    row_stride: u32,
    row_count: u32,
    offset: i32,
    scale: f32,
    base_log2: f32,
}

#[repr(C)]
struct MlxGqaAttentionLogitsArgs {
    head_dim: u32,
    q_head_stride: u32,
    k_head_stride: u32,
    q_head_count: u32,
    q_heads_per_kv: u32,
}

#[repr(C)]
struct MlxGqaAttentionOutputSingleArgs {
    head_dim: u32,
    v_head_stride: u32,
    out_head_stride: u32,
    q_head_count: u32,
    q_heads_per_kv: u32,
}

#[repr(C)]
struct MlxAddRowArgs {
    n: u32,
}

#[repr(C)]
struct MlxGegluRowArgs {
    n: u32,
}

fn default_model_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../local/models/gemma-4-26b-mlx/model-00001-of-00003.safetensors")
}

fn bytes_of<T>(value: &T) -> &[u8] {
    unsafe { slice::from_raw_parts((value as *const T).cast::<u8>(), size_of::<T>()) }
}

fn bytes_from_bf16_words(words: &[u16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(words.len() * 2);
    for word in words {
        out.extend_from_slice(&word.to_le_bytes());
    }
    out
}

fn bf16_word_to_f32(word: u16) -> f32 {
    f32::from_bits((word as u32) << 16)
}

fn bf16_round_to_f32(value: f32) -> f32 {
    let bits = value.to_bits();
    let lsb = (bits >> 16) & 1;
    let rounded = bits.wrapping_add(0x7FFF + lsb) & 0xFFFF_0000;
    f32::from_bits(rounded)
}

fn read_u16_le(bytes: &[u8], index: usize) -> Result<u16, Box<dyn Error>> {
    let start = index.checked_mul(2).ok_or("u16 index overflow")?;
    let end = start.checked_add(2).ok_or("u16 slice overflow")?;
    let chunk = bytes.get(start..end).ok_or("u16 slice out of bounds")?;
    Ok(u16::from_le_bytes([chunk[0], chunk[1]]))
}

fn read_u16x4_le(bytes: &[u8], byte_offset: usize) -> Result<[u16; 4], Box<dyn Error>> {
    let chunk = bytes
        .get(byte_offset..byte_offset + 8)
        .ok_or("u16x4 slice out of bounds")?;
    Ok([
        u16::from_le_bytes([chunk[0], chunk[1]]),
        u16::from_le_bytes([chunk[2], chunk[3]]),
        u16::from_le_bytes([chunk[4], chunk[5]]),
        u16::from_le_bytes([chunk[6], chunk[7]]),
    ])
}

fn analyze_o_proj_row(
    row: usize,
    attn_out_bits: &[u32],
    o_proj_bits: &[u32],
    o_weight_bytes: &[u8],
    o_scales_bytes: &[u8],
    o_biases_bytes: &[u8],
    weight_words_per_row: usize,
    qparams_per_row: usize,
    gpu_group_term_bits: &[u32],
) -> Result<(), Box<dyn Error>> {
    const SIMD_SIZE: usize = 32;
    const VALUES_PER_THREAD: usize = 16;
    const BLOCK_SIZE: usize = SIMD_SIZE * VALUES_PER_THREAD;
    const GROUP_SIZE: usize = 64;
    const GROUPS_PER_BLOCK: usize = BLOCK_SIZE / GROUP_SIZE;
    const LANE_GROUP: usize = GROUP_SIZE / VALUES_PER_THREAD;

    let x = attn_out_bits
        .iter()
        .map(|&bits| f32::from_bits(bits))
        .collect::<Vec<_>>();
    if x.len() % BLOCK_SIZE != 0 {
        return Err(format!("unexpected attn_out length {}", x.len()).into());
    }

    let num_blocks = x.len() / BLOCK_SIZE;
    let weight_row_stride = weight_words_per_row
        .checked_mul(4)
        .ok_or("weight row stride overflow")?;
    let qparam_row_stride = qparams_per_row
        .checked_mul(2)
        .ok_or("qparam row stride overflow")?;
    let row_weight_base = row
        .checked_mul(weight_row_stride)
        .ok_or("row weight base overflow")?;
    let row_scales_base = row
        .checked_mul(qparam_row_stride)
        .ok_or("row scales base overflow")?;
    let row_biases_base = row
        .checked_mul(qparam_row_stride)
        .ok_or("row biases base overflow")?;

    let mut lane_sums = vec![vec![0.0f32; SIMD_SIZE]; num_blocks];
    let mut lane_accums = vec![vec![0.0f32; SIMD_SIZE]; num_blocks];
    let mut lane_terms = vec![vec![0.0f32; SIMD_SIZE]; num_blocks];

    for block in 0..num_blocks {
        let block_weight_base = row_weight_base + block * (BLOCK_SIZE / 2);
        for lane in 0..SIMD_SIZE {
            let x_base = block * BLOCK_SIZE + lane * VALUES_PER_THREAD;
            let mut x_thread = [0.0f32; VALUES_PER_THREAD];
            let mut sum = 0.0f32;
            for i in (0..VALUES_PER_THREAD).step_by(4) {
                let x0 = x[x_base + i + 0];
                let x1 = x[x_base + i + 1];
                let x2 = x[x_base + i + 2];
                let x3 = x[x_base + i + 3];
                sum += x0 + x1 + x2 + x3;
                x_thread[i + 0] = x0;
                x_thread[i + 1] = x1 / 16.0;
                x_thread[i + 2] = x2 / 256.0;
                x_thread[i + 3] = x3 / 4096.0;
            }

            let packed = read_u16x4_le(o_weight_bytes, block_weight_base + lane * 8)?;
            let mut accum = 0.0f32;
            for (i, word) in packed.into_iter().enumerate() {
                let base = i * 4;
                accum += x_thread[base + 0] * f32::from(word & 0x000F);
                accum += x_thread[base + 1] * f32::from(word & 0x00F0);
                accum += x_thread[base + 2] * f32::from(word & 0x0F00);
                accum += x_thread[base + 3] * f32::from(word & 0xF000);
            }

            let group = block * GROUPS_PER_BLOCK + lane / LANE_GROUP;
            let scale = bf16_word_to_f32(read_u16_le(o_scales_bytes, row_scales_base / 2 + group)?);
            let bias = bf16_word_to_f32(read_u16_le(o_biases_bytes, row_biases_base / 2 + group)?);
            lane_sums[block][lane] = sum;
            lane_accums[block][lane] = accum;
            lane_terms[block][lane] = scale * accum + bias * sum;
        }
    }

    let mut lane_totals = [0.0f32; SIMD_SIZE];
    let mut lane_totals_block_bf16 = [0.0f32; SIMD_SIZE];
    for lane in 0..SIMD_SIZE {
        let mut running = 0.0f32;
        for block in 0..num_blocks {
            lane_totals[lane] += lane_terms[block][lane];
            running = bf16_round_to_f32(running + lane_terms[block][lane]);
        }
        lane_totals_block_bf16[lane] = running;
    }

    let sequential = bf16_round_to_f32(lane_totals.iter().copied().sum::<f32>());

    let mut tree = lane_totals;
    for offset in [16usize, 8, 4, 2, 1] {
        for lane in 0..(SIMD_SIZE - offset) {
            tree[lane] += tree[lane + offset];
        }
    }
    let tree_sum = bf16_round_to_f32(tree[0]);

    let mut tree_block_bf16 = lane_totals_block_bf16;
    for offset in [16usize, 8, 4, 2, 1] {
        for lane in 0..(SIMD_SIZE - offset) {
            tree_block_bf16[lane] += tree_block_bf16[lane + offset];
        }
    }
    let tree_block_bf16_sum = bf16_round_to_f32(tree_block_bf16[0]);

    let mut quad_group_bf16 = 0.0f32;
    let mut host_group_term_bits = Vec::with_capacity(num_blocks * GROUPS_PER_BLOCK);
    for block in 0..num_blocks {
        for group in 0..GROUPS_PER_BLOCK {
            let lane0 = group * LANE_GROUP;
            let lane1 = lane0 + 1;
            let lane2 = lane0 + 2;
            let lane3 = lane0 + 3;
            let group_accum = lane_accums[block][lane0]
                + lane_accums[block][lane1]
                + lane_accums[block][lane2]
                + lane_accums[block][lane3];
            let group_sum = lane_sums[block][lane0]
                + lane_sums[block][lane1]
                + lane_sums[block][lane2]
                + lane_sums[block][lane3];
            let qparam = block * GROUPS_PER_BLOCK + group;
            let scale = bf16_word_to_f32(read_u16_le(o_scales_bytes, row_scales_base / 2 + qparam)?);
            let bias = bf16_word_to_f32(read_u16_le(o_biases_bytes, row_biases_base / 2 + qparam)?);
            let group_term =
                bf16_round_to_f32(scale * group_accum) + bf16_round_to_f32(bias * group_sum);
            host_group_term_bits.push(group_term.to_bits());
            quad_group_bf16 += group_term;
        }
    }
    quad_group_bf16 = bf16_round_to_f32(quad_group_bf16);

    let mut lane_total_bits = [0u32; SIMD_SIZE];
    for lane in 0..SIMD_SIZE {
        lane_total_bits[lane] = lane_totals[lane].to_bits();
    }

    println!("analysis_row={row}");
    println!("analysis_rust_output_bits=0x{:08X}", o_proj_bits[row]);
    println!(
        "analysis_current_seq_bits=0x{:08X}",
        sequential.to_bits()
    );
    println!(
        "analysis_current_tree_bits=0x{:08X}",
        tree_sum.to_bits()
    );
    println!(
        "analysis_block_bf16_tree_bits=0x{:08X}",
        tree_block_bf16_sum.to_bits()
    );
    println!(
        "analysis_quad_group_bf16_bits=0x{:08X}",
        quad_group_bf16.to_bits()
    );
    println!(
        "analysis_host_group_term_fnv1a64=0x{:016X}",
        fnv1a64_u32_words(&host_group_term_bits)
    );
    println!(
        "analysis_gpu_group_term_fnv1a64=0x{:016X}",
        fnv1a64_u32_words(gpu_group_term_bits)
    );
    let gpu_group_terms = gpu_group_term_bits
        .iter()
        .map(|&bits| f32::from_bits(bits))
        .collect::<Vec<_>>();
    let gpu_group_seq = bf16_round_to_f32(gpu_group_terms.iter().copied().sum::<f32>());
    let mut gpu_group_running_bf16 = 0.0f32;
    for term in &gpu_group_terms {
        gpu_group_running_bf16 = bf16_round_to_f32(gpu_group_running_bf16 + *term);
    }
    let mut gpu_group_tree = gpu_group_terms.clone();
    let mut width = gpu_group_tree.len();
    while width > 1 {
        let half = width / 2;
        for index in 0..half {
            gpu_group_tree[index] += gpu_group_tree[index + half];
        }
        width = half;
    }
    let gpu_group_tree_sum = bf16_round_to_f32(gpu_group_tree[0]);
    println!("analysis_gpu_group_seq_bits=0x{:08X}", gpu_group_seq.to_bits());
    println!(
        "analysis_gpu_group_running_bf16_bits=0x{:08X}",
        gpu_group_running_bf16.to_bits()
    );
    println!(
        "analysis_gpu_group_tree_bits=0x{:08X}",
        gpu_group_tree_sum.to_bits()
    );
    if host_group_term_bits != gpu_group_term_bits {
        if let Some((index, (&host_bits, &gpu_bits))) = host_group_term_bits
            .iter()
            .zip(gpu_group_term_bits.iter())
            .enumerate()
            .find(|(_, (host_bits, gpu_bits))| host_bits != gpu_bits)
        {
            println!("analysis_group_term_first_mismatch_index={index}");
            println!("analysis_group_term_host_bits=0x{host_bits:08X}");
            println!("analysis_group_term_gpu_bits=0x{gpu_bits:08X}");
        }
    }
    println!(
        "analysis_lane_total_fnv1a64=0x{:016X}",
        fnv1a64_u32_words(&lane_total_bits)
    );
    print!("analysis_lane_total_first8_bits=");
    for (index, bits) in lane_total_bits.iter().take(8).enumerate() {
        if index != 0 {
            print!(",");
        }
        print!("0x{bits:08X}");
    }
    println!();

    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut model_path = default_model_path();
    let mut warmup_iters = 0usize;
    let mut bench_iters = 1usize;
    let mut dump_all_f32_bits = false;
    let mut analyze_row: Option<usize> = None;

    let mut args_iter = env::args().skip(1);
    while let Some(arg) = args_iter.next() {
        match arg.as_str() {
            "--warmup" => {
                let value = args_iter.next().ok_or("--warmup expects a value")?;
                warmup_iters = value.parse::<usize>()?;
            }
            "--iters" => {
                let value = args_iter.next().ok_or("--iters expects a value")?;
                bench_iters = value.parse::<usize>()?;
            }
            "--dump-all-f32-bits" => {
                dump_all_f32_bits = true;
            }
            "--analyze-row-1836" => {
                analyze_row = Some(1_836);
            }
            "--analyze-row" => {
                let value = args_iter.next().ok_or("--analyze-row expects a value")?;
                analyze_row = Some(value.parse::<usize>()?);
            }
            "-h" | "--help" => {
                eprintln!(
                    "Usage: metal_attention_pre_feedforward_geglu_row [model.safetensors] [--warmup N] [--iters N] [--dump-all-f32-bits] [--analyze-row-1836] [--analyze-row N]"
                );
                return Ok(());
            }
            _ if arg.starts_with("--") => {
                return Err(format!("unknown option {arg}").into());
            }
            _ => {
                model_path = PathBuf::from(arg);
            }
        }
    }
    if bench_iters == 0 {
        return Err("--iters must be greater than zero".into());
    }
    let debug_row_index = analyze_row.unwrap_or(1_836);

    let header = MlxSafetensorsHeader::load(&model_path)?;
    let runtime = MetalRuntime::new().map_err(|err| format!("MetalRuntime::new failed: {err}"))?;
    if !runtime.features().has_bfloat {
        return Err("Metal device does not report BF16 support".into());
    }

    let x_words = gemma4_qproj_case_input_bf16_words(NORM_LEN);
    let x_bytes = bytes_from_bf16_words(&x_words);
    let input_norm_weight_bytes = header.read_tensor_bytes(INPUT_NORM_WEIGHT_NAME)?;
    let q_weight_bytes = header.read_tensor_bytes(Q_PATH_ORACLE.weight_name)?;
    let q_scales_bytes = header.read_tensor_bytes(Q_PATH_ORACLE.scales_name)?;
    let q_biases_bytes = header.read_tensor_bytes(Q_PATH_ORACLE.biases_name)?;
    let q_norm_weight_bytes = header.read_tensor_bytes(Q_PATH_ORACLE.norm_weight_name.unwrap())?;
    let k_weight_bytes = header.read_tensor_bytes(K_PATH_ORACLE.weight_name)?;
    let k_scales_bytes = header.read_tensor_bytes(K_PATH_ORACLE.scales_name)?;
    let k_biases_bytes = header.read_tensor_bytes(K_PATH_ORACLE.biases_name)?;
    let k_norm_weight_bytes = header.read_tensor_bytes(K_PATH_ORACLE.norm_weight_name.unwrap())?;
    let v_weight_bytes = header.read_tensor_bytes(V_PATH_ORACLE.weight_name)?;
    let v_scales_bytes = header.read_tensor_bytes(V_PATH_ORACLE.scales_name)?;
    let v_biases_bytes = header.read_tensor_bytes(V_PATH_ORACLE.biases_name)?;
    let o_weight_bytes = header.read_tensor_bytes(O_PROJ_WEIGHT_NAME)?;
    let o_scales_bytes = header.read_tensor_bytes(O_PROJ_SCALES_NAME)?;
    let o_biases_bytes = header.read_tensor_bytes(O_PROJ_BIASES_NAME)?;
    let post_attention_norm_weight_bytes =
        header.read_tensor_bytes(POST_ATTENTION_NORM_WEIGHT_NAME)?;
    let pre_feedforward_norm_weight_bytes =
        header.read_tensor_bytes(PRE_FEEDFORWARD_NORM_WEIGHT_NAME)?;
    let mlp_gate_weight_bytes = header.read_tensor_bytes(MLP_GATE_WEIGHT_NAME)?;
    let mlp_gate_scales_bytes = header.read_tensor_bytes(MLP_GATE_SCALES_NAME)?;
    let mlp_gate_biases_bytes = header.read_tensor_bytes(MLP_GATE_BIASES_NAME)?;
    let mlp_up_weight_bytes = header.read_tensor_bytes(MLP_UP_WEIGHT_NAME)?;
    let mlp_up_scales_bytes = header.read_tensor_bytes(MLP_UP_SCALES_NAME)?;
    let mlp_up_biases_bytes = header.read_tensor_bytes(MLP_UP_BIASES_NAME)?;

    let q_weight_entry = header.tensor(Q_PATH_ORACLE.weight_name).ok_or("missing q projection weight entry")?;
    let q_scales_entry = header.tensor(Q_PATH_ORACLE.scales_name).ok_or("missing q projection scales entry")?;
    let q_norm_weight_entry = header.tensor(Q_PATH_ORACLE.norm_weight_name.unwrap()).ok_or("missing q norm weight entry")?;
    let k_weight_entry = header.tensor(K_PATH_ORACLE.weight_name).ok_or("missing k projection weight entry")?;
    let k_scales_entry = header.tensor(K_PATH_ORACLE.scales_name).ok_or("missing k projection scales entry")?;
    let k_norm_weight_entry = header.tensor(K_PATH_ORACLE.norm_weight_name.unwrap()).ok_or("missing k norm weight entry")?;
    let v_weight_entry = header.tensor(V_PATH_ORACLE.weight_name).ok_or("missing v projection weight entry")?;
    let v_scales_entry = header.tensor(V_PATH_ORACLE.scales_name).ok_or("missing v projection scales entry")?;
    let o_weight_entry = header.tensor(O_PROJ_WEIGHT_NAME).ok_or("missing o_proj weight entry")?;
    let o_scales_entry = header.tensor(O_PROJ_SCALES_NAME).ok_or("missing o_proj scales entry")?;
    let post_attention_norm_weight_entry = header
        .tensor(POST_ATTENTION_NORM_WEIGHT_NAME)
        .ok_or("missing post-attention layernorm weight entry")?;
    let pre_feedforward_norm_weight_entry = header
        .tensor(PRE_FEEDFORWARD_NORM_WEIGHT_NAME)
        .ok_or("missing pre-feedforward layernorm weight entry")?;
    let mlp_gate_weight_entry = header
        .tensor(MLP_GATE_WEIGHT_NAME)
        .ok_or("missing mlp gate_proj weight entry")?;
    let mlp_gate_scales_entry = header
        .tensor(MLP_GATE_SCALES_NAME)
        .ok_or("missing mlp gate_proj scales entry")?;
    let mlp_up_weight_entry = header
        .tensor(MLP_UP_WEIGHT_NAME)
        .ok_or("missing mlp up_proj weight entry")?;
    let mlp_up_scales_entry = header
        .tensor(MLP_UP_SCALES_NAME)
        .ok_or("missing mlp up_proj scales entry")?;

    let q_out_len = usize::try_from(q_weight_entry.shape[0])?;
    let k_out_len = usize::try_from(k_weight_entry.shape[0])?;
    let v_out_len = usize::try_from(v_weight_entry.shape[0])?;
    let o_out_len = usize::try_from(o_weight_entry.shape[0])?;
    let post_attention_norm_len = usize::try_from(post_attention_norm_weight_entry.shape[0])?;
    let pre_feedforward_norm_len = usize::try_from(pre_feedforward_norm_weight_entry.shape[0])?;
    let mlp_gate_out_len = usize::try_from(mlp_gate_weight_entry.shape[0])?;
    let mlp_up_out_len = usize::try_from(mlp_up_weight_entry.shape[0])?;
    let head_dim = usize::try_from(q_norm_weight_entry.shape[0])?;
    let k_head_dim = usize::try_from(k_norm_weight_entry.shape[0])?;
    if head_dim == 0 || head_dim != k_head_dim {
        return Err(format!("invalid q/k head_dim: q={head_dim} k={k_head_dim}").into());
    }
    if q_out_len % head_dim != 0 || k_out_len % head_dim != 0 || v_out_len % head_dim != 0 {
        return Err(format!(
            "invalid q/k/v head layout: q_out_len={} k_out_len={} v_out_len={} head_dim={}",
            q_out_len, k_out_len, v_out_len, head_dim
        )
        .into());
    }
    let q_head_count = q_out_len / head_dim;
    let k_head_count = k_out_len / head_dim;
    let v_head_count = v_out_len / head_dim;
    if k_head_count == 0 || v_head_count != k_head_count || q_head_count % k_head_count != 0 {
        return Err(format!(
            "invalid grouped-query head layout: q_head_count={} k_head_count={} v_head_count={}",
            q_head_count, k_head_count, v_head_count
        )
        .into());
    }
    let q_heads_per_kv = q_head_count / k_head_count;
    if post_attention_norm_len != o_out_len {
        return Err(format!(
            "invalid post-attention layernorm length: got {} expected {}",
            post_attention_norm_len, o_out_len
        )
        .into());
    }
    if pre_feedforward_norm_len != post_attention_norm_len {
        return Err(format!(
            "invalid pre-feedforward layernorm length: got {} expected {}",
            pre_feedforward_norm_len, post_attention_norm_len
        )
        .into());
    }
    let mlp_gate_n_in = usize::try_from(mlp_gate_weight_entry.shape[1] * 8)?;
    if mlp_gate_n_in != pre_feedforward_norm_len {
        return Err(format!(
            "invalid mlp gate_proj input size: got {} expected {}",
            mlp_gate_n_in, pre_feedforward_norm_len
        )
        .into());
    }
    let mlp_up_n_in = usize::try_from(mlp_up_weight_entry.shape[1] * 8)?;
    if mlp_up_n_in != pre_feedforward_norm_len {
        return Err(format!(
            "invalid mlp up_proj input size: got {} expected {}",
            mlp_up_n_in, pre_feedforward_norm_len
        )
        .into());
    }
    if mlp_up_out_len != mlp_gate_out_len {
        return Err(format!(
            "invalid shared MLP projection mismatch: gate {} up {}",
            mlp_gate_out_len, mlp_up_out_len
        )
        .into());
    }

    let x_buf = runtime.create_buffer_with_bytes(&x_bytes, BufferStorageMode::Private)?;
    let input_norm_weight_buf =
        runtime.create_buffer_with_bytes(&input_norm_weight_bytes, BufferStorageMode::Private)?;
    let h_buf = runtime.create_buffer(NORM_LEN * 2, BufferStorageMode::Private)?;

    let q_weight_buf = runtime.create_buffer_with_bytes(&q_weight_bytes, BufferStorageMode::Private)?;
    let q_scales_buf = runtime.create_buffer_with_bytes(&q_scales_bytes, BufferStorageMode::Private)?;
    let q_biases_buf = runtime.create_buffer_with_bytes(&q_biases_bytes, BufferStorageMode::Private)?;
    let q_norm_weight_buf = runtime.create_buffer_with_bytes(&q_norm_weight_bytes, BufferStorageMode::Private)?;
    let q_proj_buf = runtime.create_buffer(q_out_len * 2, BufferStorageMode::Private)?;
    let q_norm_buf = runtime.create_buffer(q_out_len * 2, BufferStorageMode::Private)?;
    let q_rope_buf = runtime.create_buffer(q_out_len * 2, BufferStorageMode::Private)?;

    let k_weight_buf = runtime.create_buffer_with_bytes(&k_weight_bytes, BufferStorageMode::Private)?;
    let k_scales_buf = runtime.create_buffer_with_bytes(&k_scales_bytes, BufferStorageMode::Private)?;
    let k_biases_buf = runtime.create_buffer_with_bytes(&k_biases_bytes, BufferStorageMode::Private)?;
    let k_norm_weight_buf = runtime.create_buffer_with_bytes(&k_norm_weight_bytes, BufferStorageMode::Private)?;
    let k_proj_buf = runtime.create_buffer(k_out_len * 2, BufferStorageMode::Private)?;
    let k_norm_buf = runtime.create_buffer(k_out_len * 2, BufferStorageMode::Private)?;
    let k_rope_buf = runtime.create_buffer(k_out_len * 2, BufferStorageMode::Private)?;

    let v_weight_buf = runtime.create_buffer_with_bytes(&v_weight_bytes, BufferStorageMode::Private)?;
    let v_scales_buf = runtime.create_buffer_with_bytes(&v_scales_bytes, BufferStorageMode::Private)?;
    let v_biases_buf = runtime.create_buffer_with_bytes(&v_biases_bytes, BufferStorageMode::Private)?;
    let v_proj_buf = runtime.create_buffer(v_out_len * 2, BufferStorageMode::Private)?;
    let v_norm_buf = runtime.create_buffer(v_out_len * 2, BufferStorageMode::Private)?;

    let o_weight_buf = runtime.create_buffer_with_bytes(&o_weight_bytes, BufferStorageMode::Private)?;
    let o_scales_buf = runtime.create_buffer_with_bytes(&o_scales_bytes, BufferStorageMode::Private)?;
    let o_biases_buf = runtime.create_buffer_with_bytes(&o_biases_bytes, BufferStorageMode::Private)?;
    let post_attention_norm_weight_buf =
        runtime.create_buffer_with_bytes(&post_attention_norm_weight_bytes, BufferStorageMode::Private)?;
    let pre_feedforward_norm_weight_buf =
        runtime.create_buffer_with_bytes(&pre_feedforward_norm_weight_bytes, BufferStorageMode::Private)?;
    let mlp_gate_weight_buf =
        runtime.create_buffer_with_bytes(&mlp_gate_weight_bytes, BufferStorageMode::Private)?;
    let mlp_gate_scales_buf =
        runtime.create_buffer_with_bytes(&mlp_gate_scales_bytes, BufferStorageMode::Private)?;
    let mlp_gate_biases_buf =
        runtime.create_buffer_with_bytes(&mlp_gate_biases_bytes, BufferStorageMode::Private)?;
    let mlp_up_weight_buf =
        runtime.create_buffer_with_bytes(&mlp_up_weight_bytes, BufferStorageMode::Private)?;
    let mlp_up_scales_buf =
        runtime.create_buffer_with_bytes(&mlp_up_scales_bytes, BufferStorageMode::Private)?;
    let mlp_up_biases_buf =
        runtime.create_buffer_with_bytes(&mlp_up_biases_bytes, BufferStorageMode::Private)?;

    let logits_buf = runtime.create_buffer(q_head_count * 2, BufferStorageMode::Private)?;
    let attn_out_buf = runtime.create_buffer(q_out_len * 2, BufferStorageMode::Private)?;
    let o_proj_group_terms_buf = runtime.create_buffer(
        o_out_len * o_scales_entry.shape[1] as usize * 4,
        BufferStorageMode::Private,
    )?;
    let o_proj_out_buf = runtime.create_buffer(o_out_len * 2, BufferStorageMode::Private)?;
    let o_proj_row1836_out_buf = runtime.create_buffer(2, BufferStorageMode::Private)?;
    let o_proj_row1836_seq_out_buf = runtime.create_buffer(2, BufferStorageMode::Private)?;
    let o_proj_row1836_group_terms_buf =
        runtime.create_buffer(o_scales_entry.shape[1] as usize * 4, BufferStorageMode::Private)?;
    let o_proj_fast_out_buf = runtime.create_buffer(o_out_len * 2, BufferStorageMode::Private)?;
    let post_attention_norm_out_buf =
        runtime.create_buffer(post_attention_norm_len * 2, BufferStorageMode::Private)?;
    let residual_out_buf = runtime.create_buffer(post_attention_norm_len * 2, BufferStorageMode::Private)?;
    let pre_feedforward_norm_out_buf =
        runtime.create_buffer(pre_feedforward_norm_len * 2, BufferStorageMode::Private)?;
    let mlp_gate_out_buf = runtime.create_buffer(mlp_gate_out_len * 2, BufferStorageMode::Private)?;
    let mlp_up_out_buf = runtime.create_buffer(mlp_up_out_len * 2, BufferStorageMode::Private)?;
    let geglu_out_buf = runtime.create_buffer(mlp_gate_out_len * 2, BufferStorageMode::Private)?;
    let o_proj_generic_out_buf = runtime.create_buffer(o_out_len * 2, BufferStorageMode::Private)?;
    let o_proj_serial_out_buf = runtime.create_buffer(o_out_len * 2, BufferStorageMode::Private)?;

    let rms_pipeline = runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
        cache_name: "kernel_mlx_rms_norm_row_bf16".to_string(),
        base_name: "kernel_mlx_rms_norm_row_bf16".to_string(),
        constants: Vec::new(),
        smem_bytes: 0,
        nr0: 0,
        nr1: 0,
        nsg: 0,
    })?;
    let proj_pipeline = runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
        cache_name: "kernel_mlx_affine_qmv_row_bf16".to_string(),
        base_name: "kernel_mlx_affine_qmv_row_bf16".to_string(),
        constants: Vec::new(),
        smem_bytes: 0,
        nr0: 0,
        nr1: 0,
        nsg: 0,
    })?;
    let o_proj_serial_pipeline = runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
        cache_name: "kernel_mlx_affine_qproj_row_bf16".to_string(),
        base_name: "kernel_mlx_affine_qproj_row_bf16".to_string(),
        constants: Vec::new(),
        smem_bytes: 0,
        nr0: 0,
        nr1: 0,
        nsg: 0,
    })?;
    let o_proj_fast_pipeline = runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
        cache_name: "kernel_mlx_affine_qmv_fast_row_bf16".to_string(),
        base_name: "kernel_mlx_affine_qmv_fast_row_bf16".to_string(),
        constants: Vec::new(),
        smem_bytes: 0,
        nr0: 0,
        nr1: 0,
        nsg: 0,
    })?;
    let o_proj_group_terms_pipeline = runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
        cache_name: "kernel_mlx_affine_qmv_group_terms_f32".to_string(),
        base_name: "kernel_mlx_affine_qmv_group_terms_f32".to_string(),
        constants: Vec::new(),
        smem_bytes: 0,
        nr0: 0,
        nr1: 0,
        nsg: 0,
    })?;
    let o_proj_reduce_pipeline = runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
        cache_name: "kernel_mlx_reduce_rows_f32_to_bf16".to_string(),
        base_name: "kernel_mlx_reduce_rows_f32_to_bf16".to_string(),
        constants: Vec::new(),
        smem_bytes: 0,
        nr0: 0,
        nr1: 0,
        nsg: 0,
    })?;
    let head_norm_pipeline = runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
        cache_name: "kernel_mlx_rms_norm_rows_bf16".to_string(),
        base_name: "kernel_mlx_rms_norm_rows_bf16".to_string(),
        constants: Vec::new(),
        smem_bytes: 0,
        nr0: 0,
        nr1: 0,
        nsg: 0,
    })?;
    let rope_pipeline = runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
        cache_name: "kernel_mlx_rope_single_bf16".to_string(),
        base_name: "kernel_mlx_rope_single_bf16".to_string(),
        constants: Vec::new(),
        smem_bytes: 0,
        nr0: 0,
        nr1: 0,
        nsg: 0,
    })?;
    let logits_pipeline = runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
        cache_name: "kernel_mlx_gqa_attention_logits_bf16".to_string(),
        base_name: "kernel_mlx_gqa_attention_logits_bf16".to_string(),
        constants: Vec::new(),
        smem_bytes: 0,
        nr0: 0,
        nr1: 0,
        nsg: 0,
    })?;
    let attn_out_pipeline = runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
        cache_name: "kernel_mlx_gqa_attention_output_single_bf16".to_string(),
        base_name: "kernel_mlx_gqa_attention_output_single_bf16".to_string(),
        constants: Vec::new(),
        smem_bytes: 0,
        nr0: 0,
        nr1: 0,
        nsg: 0,
    })?;
    let residual_pipeline = runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
        cache_name: "kernel_mlx_add_row_bf16".to_string(),
        base_name: "kernel_mlx_add_row_bf16".to_string(),
        constants: Vec::new(),
        smem_bytes: 0,
        nr0: 0,
        nr1: 0,
        nsg: 0,
    })?;
    let geglu_pipeline = runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
        cache_name: "kernel_mlx_geglu_row_bf16".to_string(),
        base_name: "kernel_mlx_geglu_row_bf16".to_string(),
        constants: Vec::new(),
        smem_bytes: 0,
        nr0: 0,
        nr1: 0,
        nsg: 0,
    })?;
    let o_proj_row1836_pipeline = runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
        cache_name: "kernel_mlx_affine_qmv_single_row_bf16_groupbf16".to_string(),
        base_name: "kernel_mlx_affine_qmv_single_row_bf16_groupbf16".to_string(),
        constants: Vec::new(),
        smem_bytes: 0,
        nr0: 0,
        nr1: 0,
        nsg: 0,
    })?;
    let o_proj_row1836_seq_pipeline = runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
        cache_name: "kernel_mlx_affine_qmv_single_row_seq_bf16_groupbf16".to_string(),
        base_name: "kernel_mlx_affine_qmv_single_row_seq_bf16_groupbf16".to_string(),
        constants: Vec::new(),
        smem_bytes: 0,
        nr0: 0,
        nr1: 0,
        nsg: 0,
    })?;
    let o_proj_row1836_group_terms_pipeline = runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
        cache_name: "kernel_mlx_affine_qmv_single_row_seq_group_terms_bf16".to_string(),
        base_name: "kernel_mlx_affine_qmv_single_row_seq_group_terms_bf16".to_string(),
        constants: Vec::new(),
        smem_bytes: 0,
        nr0: 0,
        nr1: 0,
        nsg: 0,
    })?;

    let n_reads = 4usize;
    let simd_size = 32usize;
    let rms_threadgroup_needed = NORM_LEN.div_ceil(n_reads);
    let rms_simds_needed = rms_threadgroup_needed.div_ceil(simd_size);
    let rms_threadgroup_size = simd_size * rms_simds_needed;
    let head_norm_threadgroup_needed = head_dim.div_ceil(n_reads);
    let head_norm_simds_needed = head_norm_threadgroup_needed.div_ceil(simd_size);
    let head_norm_threadgroup_size = simd_size * head_norm_simds_needed;
    if rms_threadgroup_size as u64 > rms_pipeline.max_threads_per_threadgroup {
        return Err("rms threadgroup exceeds pipeline max".into());
    }
    if head_norm_threadgroup_size as u64 > head_norm_pipeline.max_threads_per_threadgroup {
        return Err("head_norm threadgroup exceeds pipeline max".into());
    }

    let rms_args = MlxRmsNormRowArgs { n: NORM_LEN as u32, eps: EPS };
    let q_proj_args = MlxAffineQprojRowArgs {
        n_in: NORM_LEN as u32,
        weight_words_per_row: q_weight_entry.shape[1] as u32,
        qparams_per_row: q_scales_entry.shape[1] as u32,
        out_rows: q_out_len as u32,
    };
    let k_proj_args = MlxAffineQprojRowArgs {
        n_in: NORM_LEN as u32,
        weight_words_per_row: k_weight_entry.shape[1] as u32,
        qparams_per_row: k_scales_entry.shape[1] as u32,
        out_rows: k_out_len as u32,
    };
    let v_proj_args = MlxAffineQprojRowArgs {
        n_in: NORM_LEN as u32,
        weight_words_per_row: v_weight_entry.shape[1] as u32,
        qparams_per_row: v_scales_entry.shape[1] as u32,
        out_rows: v_out_len as u32,
    };
    let o_proj_args = MlxAffineQprojRowArgs {
        n_in: q_out_len as u32,
        weight_words_per_row: o_weight_entry.shape[1] as u32,
        qparams_per_row: o_scales_entry.shape[1] as u32,
        out_rows: o_out_len as u32,
    };
    let o_proj_row1836_args = MlxAffineQmvSingleRowArgs {
        n_in: q_out_len as u32,
        weight_words_per_row: o_weight_entry.shape[1] as u32,
        qparams_per_row: o_scales_entry.shape[1] as u32,
        row_index: debug_row_index as u32,
    };
    let o_proj_reduce_args = MlxReduceRowsF32Args {
        n: o_scales_entry.shape[1] as u32,
        row_stride: o_scales_entry.shape[1] as u32,
        row_count: o_out_len as u32,
    };
    let q_head_norm_args = MlxRmsNormRowsArgs {
        n: head_dim as u32,
        row_stride: head_dim as u32,
        row_count: q_head_count as u32,
        eps: EPS,
    };
    let k_head_norm_args = MlxRmsNormRowsArgs {
        n: head_dim as u32,
        row_stride: head_dim as u32,
        row_count: k_head_count as u32,
        eps: EPS,
    };
    let v_head_norm_args = MlxRmsNormRowsArgs {
        n: head_dim as u32,
        row_stride: head_dim as u32,
        row_count: v_head_count as u32,
        eps: EPS,
    };
    let q_rope_args = MlxRopeSingleArgs {
        half_dims: (head_dim / 2) as u32,
        row_stride: head_dim as u32,
        row_count: q_head_count as u32,
        offset: ROPE_OFFSET,
        scale: ROPE_SCALE,
        base_log2: ROPE_BASE.log2(),
    };
    let k_rope_args = MlxRopeSingleArgs {
        half_dims: (head_dim / 2) as u32,
        row_stride: head_dim as u32,
        row_count: k_head_count as u32,
        offset: ROPE_OFFSET,
        scale: ROPE_SCALE,
        base_log2: ROPE_BASE.log2(),
    };
    let logits_args = MlxGqaAttentionLogitsArgs {
        head_dim: head_dim as u32,
        q_head_stride: head_dim as u32,
        k_head_stride: head_dim as u32,
        q_head_count: q_head_count as u32,
        q_heads_per_kv: q_heads_per_kv as u32,
    };
    let attn_out_args = MlxGqaAttentionOutputSingleArgs {
        head_dim: head_dim as u32,
        v_head_stride: head_dim as u32,
        out_head_stride: head_dim as u32,
        q_head_count: q_head_count as u32,
        q_heads_per_kv: q_heads_per_kv as u32,
    };
    let residual_args = MlxAddRowArgs {
        n: post_attention_norm_len as u32,
    };
    let mlp_gate_args = MlxAffineQprojRowArgs {
        n_in: pre_feedforward_norm_len as u32,
        weight_words_per_row: mlp_gate_weight_entry.shape[1] as u32,
        qparams_per_row: mlp_gate_scales_entry.shape[1] as u32,
        out_rows: mlp_gate_out_len as u32,
    };
    let mlp_up_args = MlxAffineQprojRowArgs {
        n_in: pre_feedforward_norm_len as u32,
        weight_words_per_row: mlp_up_weight_entry.shape[1] as u32,
        qparams_per_row: mlp_up_scales_entry.shape[1] as u32,
        out_rows: mlp_up_out_len as u32,
    };
    let geglu_args = MlxGegluRowArgs {
        n: mlp_gate_out_len as u32,
    };

    let rms_bindings = [
        MetalBufferBindingRef { index: 1, buffer: &x_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 2, buffer: &input_norm_weight_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 3, buffer: &h_buf, offset_bytes: 0 },
    ];
    let q_proj_bindings = [
        MetalBufferBindingRef { index: 1, buffer: &h_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 2, buffer: &q_weight_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 3, buffer: &q_scales_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 4, buffer: &q_biases_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 5, buffer: &q_proj_buf, offset_bytes: 0 },
    ];
    let q_head_norm_bindings = [
        MetalBufferBindingRef { index: 1, buffer: &q_proj_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 2, buffer: &q_norm_weight_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 3, buffer: &q_norm_buf, offset_bytes: 0 },
    ];
    let q_rope_bindings = [
        MetalBufferBindingRef { index: 1, buffer: &q_norm_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 2, buffer: &q_rope_buf, offset_bytes: 0 },
    ];
    let k_proj_bindings = [
        MetalBufferBindingRef { index: 1, buffer: &h_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 2, buffer: &k_weight_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 3, buffer: &k_scales_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 4, buffer: &k_biases_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 5, buffer: &k_proj_buf, offset_bytes: 0 },
    ];
    let k_head_norm_bindings = [
        MetalBufferBindingRef { index: 1, buffer: &k_proj_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 2, buffer: &k_norm_weight_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 3, buffer: &k_norm_buf, offset_bytes: 0 },
    ];
    let k_rope_bindings = [
        MetalBufferBindingRef { index: 1, buffer: &k_norm_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 2, buffer: &k_rope_buf, offset_bytes: 0 },
    ];
    let v_proj_bindings = [
        MetalBufferBindingRef { index: 1, buffer: &h_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 2, buffer: &v_weight_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 3, buffer: &v_scales_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 4, buffer: &v_biases_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 5, buffer: &v_proj_buf, offset_bytes: 0 },
    ];
    let ones_bytes = bytes_from_bf16_words(&vec![0x3F80u16; head_dim]);
    let v_norm_weight_buf = runtime.create_buffer_with_bytes(&ones_bytes, BufferStorageMode::Private)?;
    let v_head_norm_bindings = [
        MetalBufferBindingRef { index: 1, buffer: &v_proj_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 2, buffer: &v_norm_weight_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 3, buffer: &v_norm_buf, offset_bytes: 0 },
    ];
    let logits_bindings = [
        MetalBufferBindingRef { index: 1, buffer: &q_rope_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 2, buffer: &k_rope_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 3, buffer: &logits_buf, offset_bytes: 0 },
    ];
    let attn_out_bindings = [
        MetalBufferBindingRef { index: 1, buffer: &v_norm_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 2, buffer: &attn_out_buf, offset_bytes: 0 },
    ];
    let o_proj_bindings = [
        MetalBufferBindingRef { index: 1, buffer: &attn_out_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 2, buffer: &o_weight_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 3, buffer: &o_scales_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 4, buffer: &o_biases_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 5, buffer: &o_proj_group_terms_buf, offset_bytes: 0 },
    ];
    let o_proj_reduce_bindings = [
        MetalBufferBindingRef { index: 1, buffer: &o_proj_group_terms_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 2, buffer: &o_proj_out_buf, offset_bytes: 0 },
    ];
    let o_proj_row1836_bindings = [
        MetalBufferBindingRef { index: 1, buffer: &attn_out_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 2, buffer: &o_weight_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 3, buffer: &o_scales_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 4, buffer: &o_biases_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 5, buffer: &o_proj_row1836_out_buf, offset_bytes: 0 },
    ];
    let o_proj_row1836_seq_bindings = [
        MetalBufferBindingRef { index: 1, buffer: &attn_out_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 2, buffer: &o_weight_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 3, buffer: &o_scales_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 4, buffer: &o_biases_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 5, buffer: &o_proj_row1836_seq_out_buf, offset_bytes: 0 },
    ];
    let o_proj_row1836_group_terms_bindings = [
        MetalBufferBindingRef { index: 1, buffer: &attn_out_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 2, buffer: &o_weight_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 3, buffer: &o_scales_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 4, buffer: &o_biases_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 5, buffer: &o_proj_row1836_group_terms_buf, offset_bytes: 0 },
    ];
    let o_proj_generic_bindings = [
        MetalBufferBindingRef { index: 1, buffer: &attn_out_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 2, buffer: &o_weight_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 3, buffer: &o_scales_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 4, buffer: &o_biases_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 5, buffer: &o_proj_generic_out_buf, offset_bytes: 0 },
    ];
    let o_proj_fast_bindings = [
        MetalBufferBindingRef { index: 1, buffer: &attn_out_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 2, buffer: &o_weight_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 3, buffer: &o_scales_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 4, buffer: &o_biases_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 5, buffer: &o_proj_fast_out_buf, offset_bytes: 0 },
    ];
    let post_attention_norm_bindings = [
        MetalBufferBindingRef { index: 1, buffer: &o_proj_fast_out_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 2, buffer: &post_attention_norm_weight_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 3, buffer: &post_attention_norm_out_buf, offset_bytes: 0 },
    ];
    let residual_bindings = [
        MetalBufferBindingRef { index: 1, buffer: &x_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 2, buffer: &post_attention_norm_out_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 3, buffer: &residual_out_buf, offset_bytes: 0 },
    ];
    let pre_feedforward_norm_bindings = [
        MetalBufferBindingRef { index: 1, buffer: &residual_out_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 2, buffer: &pre_feedforward_norm_weight_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 3, buffer: &pre_feedforward_norm_out_buf, offset_bytes: 0 },
    ];
    let mlp_gate_bindings = [
        MetalBufferBindingRef { index: 1, buffer: &pre_feedforward_norm_out_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 2, buffer: &mlp_gate_weight_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 3, buffer: &mlp_gate_scales_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 4, buffer: &mlp_gate_biases_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 5, buffer: &mlp_gate_out_buf, offset_bytes: 0 },
    ];
    let mlp_up_bindings = [
        MetalBufferBindingRef { index: 1, buffer: &pre_feedforward_norm_out_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 2, buffer: &mlp_up_weight_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 3, buffer: &mlp_up_scales_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 4, buffer: &mlp_up_biases_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 5, buffer: &mlp_up_out_buf, offset_bytes: 0 },
    ];
    let geglu_bindings = [
        MetalBufferBindingRef { index: 1, buffer: &mlp_gate_out_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 2, buffer: &mlp_up_out_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 3, buffer: &geglu_out_buf, offset_bytes: 0 },
    ];
    let o_proj_serial_bindings = [
        MetalBufferBindingRef { index: 1, buffer: &attn_out_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 2, buffer: &o_weight_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 3, buffer: &o_scales_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 4, buffer: &o_biases_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 5, buffer: &o_proj_serial_out_buf, offset_bytes: 0 },
    ];

    let rms_threadgroups = MetalSize { width: 1, height: 1, depth: 1 };
    let rms_threads_per_threadgroup = MetalSize { width: rms_threadgroup_size as u64, height: 1, depth: 1 };
    let q_proj_threadgroups = MetalSize { width: 1, height: (q_out_len as u64).div_ceil(8), depth: 1 };
    let q_proj_threads_per_threadgroup = MetalSize { width: 32, height: 2, depth: 1 };
    let q_head_norm_threadgroups = MetalSize { width: q_head_count as u64, height: 1, depth: 1 };
    let q_head_norm_threads_per_threadgroup = MetalSize { width: head_norm_threadgroup_size as u64, height: 1, depth: 1 };
    let q_rope_threadgroups = MetalSize { width: ((head_dim / 2) as u64).div_ceil(32), height: q_head_count as u64, depth: 1 };
    let q_rope_threads_per_threadgroup = MetalSize { width: 32, height: 1, depth: 1 };
    let k_proj_threadgroups = MetalSize { width: 1, height: (k_out_len as u64).div_ceil(8), depth: 1 };
    let k_proj_threads_per_threadgroup = MetalSize { width: 32, height: 2, depth: 1 };
    let k_head_norm_threadgroups = MetalSize { width: k_head_count as u64, height: 1, depth: 1 };
    let k_head_norm_threads_per_threadgroup = MetalSize { width: head_norm_threadgroup_size as u64, height: 1, depth: 1 };
    let k_rope_threadgroups = MetalSize { width: ((head_dim / 2) as u64).div_ceil(32), height: k_head_count as u64, depth: 1 };
    let k_rope_threads_per_threadgroup = MetalSize { width: 32, height: 1, depth: 1 };
    let v_proj_threadgroups = MetalSize { width: 1, height: (v_out_len as u64).div_ceil(8), depth: 1 };
    let v_proj_threads_per_threadgroup = MetalSize { width: 32, height: 2, depth: 1 };
    let v_head_norm_threadgroups = MetalSize { width: v_head_count as u64, height: 1, depth: 1 };
    let v_head_norm_threads_per_threadgroup = MetalSize { width: head_norm_threadgroup_size as u64, height: 1, depth: 1 };
    let logits_threadgroups = MetalSize { width: q_head_count as u64, height: 1, depth: 1 };
    let logits_threads_per_threadgroup = MetalSize { width: 32, height: 1, depth: 1 };
    let attn_out_threadgroups = MetalSize { width: head_dim as u64, height: q_head_count as u64, depth: 1 };
    let attn_out_threads_per_threadgroup = MetalSize { width: 16, height: 16, depth: 1 };
    let o_proj_threadgroups = MetalSize { width: 1, height: (o_out_len as u64).div_ceil(8), depth: 1 };
    let o_proj_threads_per_threadgroup = MetalSize { width: 32, height: 2, depth: 1 };
    let o_proj_group_terms_threadgroups = MetalSize {
        width: o_scales_entry.shape[1] as u64,
        height: o_out_len as u64,
        depth: 1,
    };
    let o_proj_group_terms_threads_per_threadgroup = MetalSize { width: 1, height: 1, depth: 1 };
    let o_proj_reduce_threadgroups = MetalSize { width: o_out_len as u64, height: 1, depth: 1 };
    let o_proj_reduce_threads_per_threadgroup = MetalSize { width: 1, height: 1, depth: 1 };
    let o_proj_row1836_threadgroups = MetalSize { width: 1, height: 1, depth: 1 };
    let o_proj_row1836_threads_per_threadgroup = MetalSize { width: 32, height: 1, depth: 1 };
    let o_proj_row1836_seq_threadgroups = MetalSize { width: 1, height: 1, depth: 1 };
    let o_proj_row1836_seq_threads_per_threadgroup = MetalSize { width: 1, height: 1, depth: 1 };
    let o_proj_row1836_group_terms_threadgroups = MetalSize { width: 1, height: 1, depth: 1 };
    let o_proj_row1836_group_terms_threads_per_threadgroup = MetalSize { width: 1, height: 1, depth: 1 };
    let o_proj_serial_threadgroups = MetalSize { width: o_out_len as u64, height: 1, depth: 1 };
    let o_proj_serial_threads_per_threadgroup = MetalSize { width: 1, height: 1, depth: 1 };
    let residual_threads_per_threadgroup = MetalSize { width: 256, height: 1, depth: 1 };
    let residual_threadgroups = MetalSize {
        width: (post_attention_norm_len as u64).div_ceil(residual_threads_per_threadgroup.width),
        height: 1,
        depth: 1,
    };
    let mlp_gate_threadgroups = MetalSize {
        width: 1,
        height: (mlp_gate_out_len as u64).div_ceil(8),
        depth: 1,
    };
    let mlp_gate_threads_per_threadgroup = MetalSize { width: 32, height: 2, depth: 1 };
    let mlp_up_threadgroups = MetalSize {
        width: 1,
        height: (mlp_up_out_len as u64).div_ceil(8),
        depth: 1,
    };
    let mlp_up_threads_per_threadgroup = MetalSize { width: 32, height: 2, depth: 1 };
    let geglu_threads_per_threadgroup = MetalSize { width: 256, height: 1, depth: 1 };
    let geglu_threadgroups = MetalSize {
        width: (mlp_gate_out_len as u64).div_ceil(geglu_threads_per_threadgroup.width),
        height: 1,
        depth: 1,
    };

    let rms_args_bytes = bytes_of(&rms_args);
    let q_proj_args_bytes = bytes_of(&q_proj_args);
    let k_proj_args_bytes = bytes_of(&k_proj_args);
    let v_proj_args_bytes = bytes_of(&v_proj_args);
    let o_proj_args_bytes = bytes_of(&o_proj_args);
    let q_head_norm_args_bytes = bytes_of(&q_head_norm_args);
    let k_head_norm_args_bytes = bytes_of(&k_head_norm_args);
    let v_head_norm_args_bytes = bytes_of(&v_head_norm_args);
    let q_rope_args_bytes = bytes_of(&q_rope_args);
    let k_rope_args_bytes = bytes_of(&k_rope_args);
    let logits_args_bytes = bytes_of(&logits_args);
    let attn_out_args_bytes = bytes_of(&attn_out_args);
    let residual_args_bytes = bytes_of(&residual_args);
    let mlp_gate_args_bytes = bytes_of(&mlp_gate_args);
    let mlp_up_args_bytes = bytes_of(&mlp_up_args);
    let geglu_args_bytes = bytes_of(&geglu_args);
    let o_proj_row1836_args_bytes = bytes_of(&o_proj_row1836_args);
    let o_proj_reduce_args_bytes = bytes_of(&o_proj_reduce_args);
    let diagnostics_enabled = analyze_row.is_some();

    let run_once = || -> Result<(), Box<dyn Error>> {
        runtime.begin_command_batch()?;
        runtime.dispatch_compute(&rms_pipeline, rms_args_bytes, &rms_bindings, &[], rms_threadgroups, rms_threads_per_threadgroup)?;
        runtime.memory_barrier_buffers()?;

        runtime.dispatch_compute(&proj_pipeline, q_proj_args_bytes, &q_proj_bindings, &[], q_proj_threadgroups, q_proj_threads_per_threadgroup)?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(&head_norm_pipeline, q_head_norm_args_bytes, &q_head_norm_bindings, &[], q_head_norm_threadgroups, q_head_norm_threads_per_threadgroup)?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(&rope_pipeline, q_rope_args_bytes, &q_rope_bindings, &[], q_rope_threadgroups, q_rope_threads_per_threadgroup)?;
        runtime.memory_barrier_buffers()?;

        runtime.dispatch_compute(&proj_pipeline, k_proj_args_bytes, &k_proj_bindings, &[], k_proj_threadgroups, k_proj_threads_per_threadgroup)?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(&head_norm_pipeline, k_head_norm_args_bytes, &k_head_norm_bindings, &[], k_head_norm_threadgroups, k_head_norm_threads_per_threadgroup)?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(&rope_pipeline, k_rope_args_bytes, &k_rope_bindings, &[], k_rope_threadgroups, k_rope_threads_per_threadgroup)?;
        runtime.memory_barrier_buffers()?;

        runtime.dispatch_compute(&proj_pipeline, v_proj_args_bytes, &v_proj_bindings, &[], v_proj_threadgroups, v_proj_threads_per_threadgroup)?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(&head_norm_pipeline, v_head_norm_args_bytes, &v_head_norm_bindings, &[], v_head_norm_threadgroups, v_head_norm_threads_per_threadgroup)?;
        runtime.memory_barrier_buffers()?;

        runtime.dispatch_compute(&logits_pipeline, logits_args_bytes, &logits_bindings, &[], logits_threadgroups, logits_threads_per_threadgroup)?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(&attn_out_pipeline, attn_out_args_bytes, &attn_out_bindings, &[], attn_out_threadgroups, attn_out_threads_per_threadgroup)?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(&o_proj_fast_pipeline, o_proj_args_bytes, &o_proj_fast_bindings, &[], o_proj_threadgroups, o_proj_threads_per_threadgroup)?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(&rms_pipeline, rms_args_bytes, &post_attention_norm_bindings, &[], rms_threadgroups, rms_threads_per_threadgroup)?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(&residual_pipeline, residual_args_bytes, &residual_bindings, &[], residual_threadgroups, residual_threads_per_threadgroup)?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(&rms_pipeline, rms_args_bytes, &pre_feedforward_norm_bindings, &[], rms_threadgroups, rms_threads_per_threadgroup)?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(&proj_pipeline, mlp_gate_args_bytes, &mlp_gate_bindings, &[], mlp_gate_threadgroups, mlp_gate_threads_per_threadgroup)?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(&proj_pipeline, mlp_up_args_bytes, &mlp_up_bindings, &[], mlp_up_threadgroups, mlp_up_threads_per_threadgroup)?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(&geglu_pipeline, geglu_args_bytes, &geglu_bindings, &[], geglu_threadgroups, geglu_threads_per_threadgroup)?;
        if diagnostics_enabled {
            runtime.memory_barrier_buffers()?;
            runtime.dispatch_compute(&o_proj_row1836_pipeline, o_proj_row1836_args_bytes, &o_proj_row1836_bindings, &[], o_proj_row1836_threadgroups, o_proj_row1836_threads_per_threadgroup)?;
            runtime.memory_barrier_buffers()?;
            runtime.dispatch_compute(&o_proj_row1836_seq_pipeline, o_proj_row1836_args_bytes, &o_proj_row1836_seq_bindings, &[], o_proj_row1836_seq_threadgroups, o_proj_row1836_seq_threads_per_threadgroup)?;
            runtime.memory_barrier_buffers()?;
            runtime.dispatch_compute(&o_proj_row1836_group_terms_pipeline, o_proj_row1836_args_bytes, &o_proj_row1836_group_terms_bindings, &[], o_proj_row1836_group_terms_threadgroups, o_proj_row1836_group_terms_threads_per_threadgroup)?;
            runtime.memory_barrier_buffers()?;
            runtime.dispatch_compute(&o_proj_group_terms_pipeline, o_proj_args_bytes, &o_proj_bindings, &[], o_proj_group_terms_threadgroups, o_proj_group_terms_threads_per_threadgroup)?;
            runtime.memory_barrier_buffers()?;
            runtime.dispatch_compute(&o_proj_reduce_pipeline, o_proj_reduce_args_bytes, &o_proj_reduce_bindings, &[], o_proj_reduce_threadgroups, o_proj_reduce_threads_per_threadgroup)?;
            runtime.memory_barrier_buffers()?;
            runtime.dispatch_compute(&proj_pipeline, o_proj_args_bytes, &o_proj_generic_bindings, &[], o_proj_threadgroups, o_proj_threads_per_threadgroup)?;
            runtime.memory_barrier_buffers()?;
            runtime.dispatch_compute(&o_proj_serial_pipeline, o_proj_args_bytes, &o_proj_serial_bindings, &[], o_proj_serial_threadgroups, o_proj_serial_threads_per_threadgroup)?;
        }
        runtime.end_command_batch()?;
        runtime.wait_idle()?;
        Ok(())
    };

    for _ in 0..warmup_iters {
        run_once()?;
    }
    let start = Instant::now();
    for _ in 0..bench_iters {
        run_once()?;
    }
    let elapsed = start.elapsed();
    run_once()?;

    let decode_bits = |bytes: Vec<u8>| {
        bytes.chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .map(bf16_word_to_f32)
            .map(f32::to_bits)
            .collect::<Vec<_>>()
    };

    let q_norm_bits = decode_bits(runtime.read_buffer(&q_norm_buf, q_out_len * 2)?);
    let q_rope_bits = decode_bits(runtime.read_buffer(&q_rope_buf, q_out_len * 2)?);
    let k_norm_bits = decode_bits(runtime.read_buffer(&k_norm_buf, k_out_len * 2)?);
    let k_rope_bits = decode_bits(runtime.read_buffer(&k_rope_buf, k_out_len * 2)?);
    let v_norm_bits = decode_bits(runtime.read_buffer(&v_norm_buf, v_out_len * 2)?);
    let logits_bits = decode_bits(runtime.read_buffer(&logits_buf, q_head_count * 2)?);
    let attn_out_bits = decode_bits(runtime.read_buffer(&attn_out_buf, q_out_len * 2)?);
    let o_proj_fast_bits = decode_bits(runtime.read_buffer(&o_proj_fast_out_buf, o_out_len * 2)?);
    let post_attention_norm_bits =
        decode_bits(runtime.read_buffer(&post_attention_norm_out_buf, post_attention_norm_len * 2)?);
    let residual_bits = decode_bits(runtime.read_buffer(&residual_out_buf, post_attention_norm_len * 2)?);
    let pre_feedforward_norm_bits =
        decode_bits(runtime.read_buffer(&pre_feedforward_norm_out_buf, pre_feedforward_norm_len * 2)?);
    let mlp_gate_bits =
        decode_bits(runtime.read_buffer(&mlp_gate_out_buf, mlp_gate_out_len * 2)?);
    let mlp_up_bits =
        decode_bits(runtime.read_buffer(&mlp_up_out_buf, mlp_up_out_len * 2)?);
    let geglu_bits =
        decode_bits(runtime.read_buffer(&geglu_out_buf, mlp_gate_out_len * 2)?);
    let (o_proj_row1836_bits, o_proj_row1836_seq_bits, o_proj_row1836_group_term_bits, o_proj_proof_bits, o_proj_generic_bits, o_proj_serial_bits) =
        if diagnostics_enabled {
            (
                decode_bits(runtime.read_buffer(&o_proj_row1836_out_buf, 2)?),
                decode_bits(runtime.read_buffer(&o_proj_row1836_seq_out_buf, 2)?),
                runtime
                    .read_buffer(&o_proj_row1836_group_terms_buf, o_scales_entry.shape[1] as usize * 4)?
                    .chunks_exact(4)
                    .map(|chunk| u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                    .collect::<Vec<_>>(),
                decode_bits(runtime.read_buffer(&o_proj_out_buf, o_out_len * 2)?),
                decode_bits(runtime.read_buffer(&o_proj_generic_out_buf, o_out_len * 2)?),
                decode_bits(runtime.read_buffer(&o_proj_serial_out_buf, o_out_len * 2)?),
            )
        } else {
            (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new())
        };
    let o_proj_bits = &o_proj_fast_bits;

    let q_norm_hash = fnv1a64_u32_words(&q_norm_bits);
    let q_rope_hash = fnv1a64_u32_words(&q_rope_bits);
    let k_norm_hash = fnv1a64_u32_words(&k_norm_bits);
    let k_rope_hash = fnv1a64_u32_words(&k_rope_bits);
    let v_norm_hash = fnv1a64_u32_words(&v_norm_bits);
    let logits_hash = fnv1a64_u32_words(&logits_bits);
    let attn_out_hash = fnv1a64_u32_words(&attn_out_bits);
    let o_proj_row1836_bit = o_proj_row1836_bits.first().copied().unwrap_or(0);
    let o_proj_row1836_seq_bit = o_proj_row1836_seq_bits.first().copied().unwrap_or(0);
    let o_proj_proof_hash = if o_proj_proof_bits.is_empty() {
        0
    } else {
        fnv1a64_u32_words(&o_proj_proof_bits)
    };
    let o_proj_fast_hash = fnv1a64_u32_words(&o_proj_fast_bits);
    let o_proj_generic_hash = if o_proj_generic_bits.is_empty() {
        0
    } else {
        fnv1a64_u32_words(&o_proj_generic_bits)
    };
    let o_proj_serial_hash = if o_proj_serial_bits.is_empty() {
        0
    } else {
        fnv1a64_u32_words(&o_proj_serial_bits)
    };
    let o_proj_hash = fnv1a64_u32_words(o_proj_bits);
    let post_attention_norm_hash = fnv1a64_u32_words(&post_attention_norm_bits);
    let residual_hash = fnv1a64_u32_words(&residual_bits);
    let pre_feedforward_norm_hash = fnv1a64_u32_words(&pre_feedforward_norm_bits);
    let mlp_gate_hash = fnv1a64_u32_words(&mlp_gate_bits);
    let mlp_up_hash = fnv1a64_u32_words(&mlp_up_bits);
    let geglu_hash = fnv1a64_u32_words(&geglu_bits);
    let o_proj_main_first16 = &o_proj_bits[..o_proj_bits.len().min(16)];
    let post_attention_norm_first16 =
        &post_attention_norm_bits[..post_attention_norm_bits.len().min(16)];
    let residual_first16 = &residual_bits[..residual_bits.len().min(16)];
    let pre_feedforward_norm_first16 =
        &pre_feedforward_norm_bits[..pre_feedforward_norm_bits.len().min(16)];
    let mlp_gate_first16 = &mlp_gate_bits[..mlp_gate_bits.len().min(16)];
    let mlp_up_first16 = &mlp_up_bits[..mlp_up_bits.len().min(16)];
    let geglu_first16 = &geglu_bits[..geglu_bits.len().min(16)];
    let o_proj_proof_first16 = &o_proj_proof_bits[..o_proj_proof_bits.len().min(16)];
    let o_proj_fast_first16 = &o_proj_fast_bits[..o_proj_fast_bits.len().min(16)];
    let o_proj_generic_first16 = &o_proj_generic_bits[..o_proj_generic_bits.len().min(16)];
    let o_proj_serial_first16 = &o_proj_serial_bits[..o_proj_serial_bits.len().min(16)];

    if let Some(row) = analyze_row {
        println!("analysis_debug_row={row}");
        println!("analysis_debug_kernel_bits=0x{o_proj_row1836_bit:08X}");
        println!("analysis_debug_seq_kernel_bits=0x{o_proj_row1836_seq_bit:08X}");
        analyze_o_proj_row(
            row,
            &attn_out_bits,
            o_proj_bits,
            &o_weight_bytes,
            &o_scales_bytes,
            &o_biases_bytes,
            o_weight_entry.shape[1] as usize,
            o_scales_entry.shape[1] as usize,
            &o_proj_row1836_group_term_bits,
        )?;
    }

    if dump_all_f32_bits {
        print!("attn_out_all_f32_bits=");
        for (index, bits) in attn_out_bits.iter().enumerate() {
            if index != 0 {
                print!(",");
            }
            print!("0x{bits:08X}");
        }
        println!();
        print!("all_f32_bits=");
        for (index, bits) in geglu_bits.iter().enumerate() {
            if index != 0 {
                print!(",");
            }
            print!("0x{bits:08X}");
        }
        println!();
        io::stdout().flush()?;
    }

    if q_norm_bits[..16] != Q_PATH_ORACLE.expected_norm_first16_bits || q_norm_hash != Q_PATH_ORACLE.expected_norm_hash {
        return Err(format!(
            "q_norm mismatch: got hash 0x{q_norm_hash:016X} first16 {:08X?}",
            &q_norm_bits[..16]
        ).into());
    }
    if q_rope_bits[..16] != Q_PATH_ORACLE.expected_rope_first16_bits.unwrap() || q_rope_hash != Q_PATH_ORACLE.expected_rope_hash.unwrap() {
        return Err(format!(
            "q_rope mismatch: got hash 0x{q_rope_hash:016X} first16 {:08X?}",
            &q_rope_bits[..16]
        ).into());
    }
    if k_norm_bits[..16] != K_PATH_ORACLE.expected_norm_first16_bits || k_norm_hash != K_PATH_ORACLE.expected_norm_hash {
        return Err(format!(
            "k_norm mismatch: got hash 0x{k_norm_hash:016X} first16 {:08X?}",
            &k_norm_bits[..16]
        ).into());
    }
    if k_rope_bits[..16] != K_PATH_ORACLE.expected_rope_first16_bits.unwrap() || k_rope_hash != K_PATH_ORACLE.expected_rope_hash.unwrap() {
        return Err(format!(
            "k_rope mismatch: got hash 0x{k_rope_hash:016X} first16 {:08X?}",
            &k_rope_bits[..16]
        ).into());
    }
    if v_norm_bits[..16] != V_PATH_ORACLE.expected_norm_first16_bits || v_norm_hash != V_PATH_ORACLE.expected_norm_hash {
        return Err(format!(
            "v_norm mismatch: got hash 0x{v_norm_hash:016X} first16 {:08X?}",
            &v_norm_bits[..16]
        ).into());
    }
    if logits_bits != EXPECTED_LOGITS_BITS || logits_hash != EXPECTED_LOGITS_HASH {
        return Err(format!(
            "attention logits mismatch: got hash 0x{logits_hash:016X} bits {:08X?}",
            &logits_bits
        ).into());
    }
    if attn_out_bits[..16] != EXPECTED_ATTN_OUT_FIRST16_BITS || attn_out_hash != EXPECTED_ATTN_OUT_HASH {
        return Err(format!(
            "attention output mismatch: got hash 0x{attn_out_hash:016X} first16 {:08X?}",
            &attn_out_bits[..16]
        ).into());
    }
    if o_proj_bits[..16] != EXPECTED_O_PROJ_FIRST16_BITS || o_proj_hash != EXPECTED_O_PROJ_HASH {
        return Err(format!(
            "o_proj branch output mismatch: main hash 0x{o_proj_hash:016X} main first16 {:08X?} proof hash 0x{o_proj_proof_hash:016X} proof first16 {:08X?} debug_row {debug_row_index} kernel 0x{o_proj_row1836_bit:08X} seq 0x{o_proj_row1836_seq_bit:08X} row1836_expected 0x{EXPECTED_O_PROJ_ROW_1836_BITS:08X} fast hash 0x{o_proj_fast_hash:016X} fast first16 {:08X?} generic hash 0x{o_proj_generic_hash:016X} generic first16 {:08X?} serial hash 0x{o_proj_serial_hash:016X} serial first16 {:08X?} expected hash 0x{:016X} first16 {:08X?}",
            o_proj_main_first16,
            o_proj_proof_first16,
            o_proj_fast_first16,
            o_proj_generic_first16,
            o_proj_serial_first16,
            EXPECTED_O_PROJ_HASH,
            EXPECTED_O_PROJ_FIRST16_BITS
        ).into());
    }
    if post_attention_norm_bits[..16] != EXPECTED_POST_ATTENTION_NORM_FIRST16_BITS
        || post_attention_norm_hash != EXPECTED_POST_ATTENTION_NORM_HASH
    {
        return Err(format!(
            "post_attention_layernorm mismatch: got hash 0x{post_attention_norm_hash:016X} first16 {:08X?} expected hash 0x{EXPECTED_POST_ATTENTION_NORM_HASH:016X} expected first16 {:08X?}",
            post_attention_norm_first16,
            EXPECTED_POST_ATTENTION_NORM_FIRST16_BITS
        )
        .into());
    }
    if residual_bits[..16] != EXPECTED_POST_ATTENTION_RESIDUAL_FIRST16_BITS
        || residual_hash != EXPECTED_POST_ATTENTION_RESIDUAL_HASH
    {
        return Err(format!(
            "post_attention residual mismatch: got hash 0x{residual_hash:016X} first16 {:08X?} expected hash 0x{EXPECTED_POST_ATTENTION_RESIDUAL_HASH:016X} expected first16 {:08X?}",
            residual_first16,
            EXPECTED_POST_ATTENTION_RESIDUAL_FIRST16_BITS
        )
        .into());
    }
    if pre_feedforward_norm_bits[..16] != EXPECTED_PRE_FEEDFORWARD_NORM_FIRST16_BITS
        || pre_feedforward_norm_hash != EXPECTED_PRE_FEEDFORWARD_NORM_HASH
    {
        return Err(format!(
            "pre_feedforward_layernorm mismatch: got hash 0x{pre_feedforward_norm_hash:016X} first16 {:08X?} expected hash 0x{EXPECTED_PRE_FEEDFORWARD_NORM_HASH:016X} expected first16 {:08X?}",
            pre_feedforward_norm_first16,
            EXPECTED_PRE_FEEDFORWARD_NORM_FIRST16_BITS
        )
        .into());
    }
    if mlp_gate_bits[..16] != EXPECTED_PRE_FEEDFORWARD_GATE_FIRST16_BITS
        || mlp_gate_hash != EXPECTED_PRE_FEEDFORWARD_GATE_HASH
    {
        return Err(format!(
            "pre_feedforward gate_proj mismatch: got hash 0x{mlp_gate_hash:016X} first16 {:08X?} expected hash 0x{EXPECTED_PRE_FEEDFORWARD_GATE_HASH:016X} expected first16 {:08X?}",
            mlp_gate_first16,
            EXPECTED_PRE_FEEDFORWARD_GATE_FIRST16_BITS
        )
        .into());
    }
    if mlp_up_bits[..16] != EXPECTED_PRE_FEEDFORWARD_UP_FIRST16_BITS
        || mlp_up_hash != EXPECTED_PRE_FEEDFORWARD_UP_HASH
    {
        return Err(format!(
            "pre_feedforward up_proj mismatch: got hash 0x{mlp_up_hash:016X} first16 {:08X?} expected hash 0x{EXPECTED_PRE_FEEDFORWARD_UP_HASH:016X} expected first16 {:08X?}",
            mlp_up_first16,
            EXPECTED_PRE_FEEDFORWARD_UP_FIRST16_BITS
        )
        .into());
    }
    if geglu_bits[..16] != EXPECTED_PRE_FEEDFORWARD_GEGLU_FIRST16_BITS
        || geglu_hash != EXPECTED_PRE_FEEDFORWARD_GEGLU_HASH
    {
        return Err(format!(
            "pre_feedforward geglu mismatch: got hash 0x{geglu_hash:016X} first16 {:08X?} expected hash 0x{EXPECTED_PRE_FEEDFORWARD_GEGLU_HASH:016X} expected first16 {:08X?}",
            geglu_first16,
            EXPECTED_PRE_FEEDFORWARD_GEGLU_FIRST16_BITS
        )
        .into());
    }

    println!("backend={}", runtime.backend_info().name);
    println!("model_path={}", model_path.display());
    println!("input_norm_weight_name={INPUT_NORM_WEIGHT_NAME}");
    println!("o_proj_weight_name={O_PROJ_WEIGHT_NAME}");
    println!("post_attention_norm_weight_name={POST_ATTENTION_NORM_WEIGHT_NAME}");
    println!("pre_feedforward_norm_weight_name={PRE_FEEDFORWARD_NORM_WEIGHT_NAME}");
    println!("mlp_gate_weight_name={MLP_GATE_WEIGHT_NAME}");
    println!("mlp_up_weight_name={MLP_UP_WEIGHT_NAME}");
    println!("eps={EPS}");
    println!("rope_base={ROPE_BASE}");
    println!("rope_scale={ROPE_SCALE}");
    println!("rope_offset={ROPE_OFFSET}");
    println!("warmup_iters={warmup_iters}");
    println!("bench_iters={bench_iters}");
    println!("rms_threads_per_threadgroup={rms_threadgroup_size}");
    println!("head_norm_threads_per_threadgroup={head_norm_threadgroup_size}");
    println!("q_head_count={q_head_count}");
    println!("k_head_count={k_head_count}");
    println!("v_head_count={v_head_count}");
    println!("q_heads_per_kv={q_heads_per_kv}");
    println!("head_dim={head_dim}");
    println!("total_ns={}", elapsed.as_nanos());
    println!("avg_ns={:.0}", elapsed.as_secs_f64() * 1e9 / bench_iters as f64);
    println!("avg_us={:.3}", elapsed.as_secs_f64() * 1e6 / bench_iters as f64);
    println!("attention_output_fnv1a64=0x{attn_out_hash:016X}");
    println!("attention_oproj_fnv1a64=0x{o_proj_hash:016X}");
    println!("attention_post_attn_norm_fnv1a64=0x{post_attention_norm_hash:016X}");
    println!("attention_post_attn_residual_fnv1a64=0x{residual_hash:016X}");
    println!("attention_pre_ffn_norm_fnv1a64=0x{pre_feedforward_norm_hash:016X}");
    println!("attention_pre_ffn_gate_fnv1a64=0x{mlp_gate_hash:016X}");
    println!("attention_pre_ffn_up_fnv1a64=0x{mlp_up_hash:016X}");
    println!("attention_pre_ffn_geglu_fnv1a64=0x{geglu_hash:016X}");
    print!("attention_pre_ffn_gate_first16_f32_bits=");
    for (index, bits) in mlp_gate_first16.iter().enumerate() {
        if index != 0 {
            print!(",");
        }
        print!("0x{bits:08X}");
    }
    println!();
    print!("attention_pre_ffn_up_first16_f32_bits=");
    for (index, bits) in mlp_up_first16.iter().enumerate() {
        if index != 0 {
            print!(",");
        }
        print!("0x{bits:08X}");
    }
    println!();
    print!("attention_pre_ffn_geglu_first16_f32_bits=");
    for (index, bits) in geglu_first16.iter().enumerate() {
        if index != 0 {
            print!(",");
        }
        print!("0x{bits:08X}");
    }
    println!();
    println!("status=ok");

    Ok(())
}
