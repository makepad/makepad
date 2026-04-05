use makepad_ggml::backend::metal::{
    BufferStorageMode, MetalBufferBindingRef, MetalPipelineDescriptor, MetalRuntime, MetalSize,
};
use makepad_mlx_rt_core::{
    fnv1a64_u32_words, gemma4_qproj_case_input_bf16_words_with_phase, MlxSafetensorsHeader,
};
use crate::{GemmaAttentionKind, GemmaKvCache, GemmaKvCacheSpec, KvTensor, KvTensorShape};
use std::env;
use std::error::Error;
use std::mem::size_of;
use std::path::PathBuf;
use std::slice;

const INPUT_NORM_WEIGHT_NAME: &str = "language_model.model.layers.0.input_layernorm.weight";
const Q_NORM_WEIGHT_NAME: &str = "language_model.model.layers.0.self_attn.q_norm.weight";
const K_NORM_WEIGHT_NAME: &str = "language_model.model.layers.0.self_attn.k_norm.weight";
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
const MLP_DOWN_WEIGHT_NAME: &str = "language_model.model.layers.0.mlp.down_proj.weight";
const MLP_DOWN_SCALES_NAME: &str = "language_model.model.layers.0.mlp.down_proj.scales";
const MLP_DOWN_BIASES_NAME: &str = "language_model.model.layers.0.mlp.down_proj.biases";
const ROUTER_SCALE_NAME: &str = "language_model.model.layers.0.router.scale";
const ROUTER_PER_EXPERT_SCALE_NAME: &str =
    "language_model.model.layers.0.router.per_expert_scale";
const ROUTER_PROJ_WEIGHT_NAME: &str = "language_model.model.layers.0.router.proj.weight";
const ROUTER_PROJ_SCALES_NAME: &str = "language_model.model.layers.0.router.proj.scales";
const ROUTER_PROJ_BIASES_NAME: &str = "language_model.model.layers.0.router.proj.biases";
const NORM_LEN: usize = 2_816;
const EPS: f32 = 1e-6;
const ROPE_BASE: f32 = 10_000.0;
const ROPE_SCALE: f32 = 1.0;
const PREFILL_ROPE_OFFSET: i32 = 17;
const DECODE_ROPE_OFFSET: i32 = 18;
const PREFILL_ACTIVATION_PHASE: usize = 0;
const DECODE_ACTIVATION_PHASE: usize = 5;
const ROUTER_TOP_K: usize = 8;

const EXPECTED_PREFILL_K_CACHE_HASH: u64 = 0x9731_B5D8_139C_BB3D;
const EXPECTED_PREFILL_V_CACHE_HASH: u64 = 0xDAC6_F97C_1CD9_387D;
const EXPECTED_DECODE_Q_ROPE_HASH: u64 = 0x3F6F_A467_761B_31DF;
const EXPECTED_DECODE_K_ROPE_HASH: u64 = 0xB42F_28FB_C28A_D462;
const EXPECTED_DECODE_V_NORM_HASH: u64 = 0x7B8D_EC22_8439_1DCD;
const EXPECTED_FULL_K_CACHE_HASH: u64 = 0xFE62_63E0_0D86_D3E6;
const EXPECTED_FULL_V_CACHE_HASH: u64 = 0xA2E2_2974_EAFA_CE55;
const EXPECTED_ATTENTION_SCORES_HASH: u64 = 0x70A1_56FC_A6D2_7DF6;
const EXPECTED_ATTENTION_PROBS_HASH: u64 = 0x3925_2430_4F5E_3D93;
const EXPECTED_ATTENTION_OUTPUT_HASH: u64 = 0xF7DB_8A67_DB76_F54B;
const EXPECTED_ATTENTION_OPROJ_HASH: u64 = 0xDAEC_5FD1_7FB3_D98B;
const EXPECTED_POST_ATTENTION_NORM_HASH: u64 = 0x3455_1F4E_45EB_BC8A;
const EXPECTED_POST_ATTENTION_RESIDUAL_HASH: u64 = 0x815C_0DD4_F5C9_C359;
const EXPECTED_PRE_FEEDFORWARD_NORM_HASH: u64 = 0x80C8_AFCC_A68D_8C32;
const EXPECTED_PRE_FEEDFORWARD_GATE_HASH: u64 = 0x0121_389B_E5C9_3499;
const EXPECTED_PRE_FEEDFORWARD_UP_HASH: u64 = 0x5051_42C8_06C5_DD15;
const EXPECTED_PRE_FEEDFORWARD_GEGLU_HASH: u64 = 0x1A8D_3407_909E_7CDD;
const EXPECTED_PRE_FEEDFORWARD_DOWN_HASH: u64 = 0x8254_77E5_568B_BA78;
const EXPECTED_ROUTER_SCALED_HASH: u64 = 0x00A0_0F1A_6C5F_56B3;
const EXPECTED_ROUTER_EXPERT_SCORES_HASH: u64 = 0x112B_3439_F573_0364;
const EXPECTED_ROUTER_PROBS_HASH: u64 = 0x2BB7_8933_AA3F_E931;
const EXPECTED_ROUTER_TOPK_INDICES_HASH: u64 = 0xD911_5D45_6505_F5AF;
const EXPECTED_ROUTER_TOPK_WEIGHTS_HASH: u64 = 0xF6C4_9320_7320_26E6;

const EXPECTED_PREFILL_K_CACHE_FIRST16_BITS: [u32; 16] = [
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
];
const EXPECTED_PREFILL_V_CACHE_FIRST16_BITS: [u32; 16] = [
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
];
const EXPECTED_DECODE_Q_ROPE_FIRST16_BITS: [u32; 16] = [
    0xC001_0000,
    0xBFA6_0000,
    0x3F91_0000,
    0x3FA8_0000,
    0xC007_0000,
    0x3DD2_0000,
    0x3ED9_0000,
    0x3F13_0000,
    0x3E93_0000,
    0xBD9C_0000,
    0xBC81_0000,
    0xBF20_0000,
    0x3EEC_0000,
    0xBF23_0000,
    0x3F7D_0000,
    0x3EB6_0000,
];
const EXPECTED_DECODE_K_ROPE_FIRST16_BITS: [u32; 16] = [
    0x3DC5_0000,
    0xBE5E_0000,
    0x3E22_0000,
    0x3E88_0000,
    0xBD96_0000,
    0x3C11_0000,
    0xBD16_0000,
    0x3DA5_0000,
    0x3D39_0000,
    0xBD34_0000,
    0x3BBE_0000,
    0xBE7A_0000,
    0x3CFE_0000,
    0x3D04_0000,
    0x3E32_0000,
    0xBE39_0000,
];
const EXPECTED_DECODE_V_NORM_FIRST16_BITS: [u32; 16] = [
    0xBFA1_0000,
    0x3F6D_0000,
    0x3E95_0000,
    0x3F85_0000,
    0xBF5A_0000,
    0xBFCC_0000,
    0xBC2E_0000,
    0xBFC3_0000,
    0xBEC4_0000,
    0x3EE2_0000,
    0x3BE2_0000,
    0xBF19_0000,
    0x3F58_0000,
    0x3E89_0000,
    0xBE96_0000,
    0x3ECF_0000,
];
const EXPECTED_FULL_K_CACHE_FIRST16_BITS: [u32; 16] = [
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
];
const EXPECTED_FULL_V_CACHE_FIRST16_BITS: [u32; 16] = [
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
];
const EXPECTED_ATTENTION_SCORES_FIRST16_BITS: [u32; 16] = [
    0xBFF3_0000,
    0x3F3E_0000,
    0x3E97_0000,
    0xC0EF_0000,
    0x40A7_0000,
    0xC08D_0000,
    0xC014_0000,
    0x4072_0000,
    0xC06A_0000,
    0xBFB9_0000,
    0xC088_0000,
    0x40B0_0000,
    0x401A_0000,
    0xC0C5_0000,
    0x3FA1_0000,
    0x3E8B_0000,
];
const EXPECTED_ATTENTION_PROBS_FIRST16_BITS: [u32; 16] = [
    0x3D88_0000,
    0x3F6F_0000,
    0x3F80_0000,
    0x39DF_0000,
    0x3F80_0000,
    0x388B_0000,
    0x3B14_0000,
    0x3F7F_0000,
    0x3DCA_0000,
    0x3F67_0000,
    0x3874_0000,
    0x3F80_0000,
    0x3F80_0000,
    0x3948_0000,
    0x3F3A_0000,
    0x3E8B_0000,
];
const EXPECTED_ATTENTION_OUTPUT_FIRST16_BITS: [u32; 16] = [
    0xBF98_0000,
    0x3F57_0000,
    0x3E4D_0000,
    0x3F6A_0000,
    0xBF4B_0000,
    0xBFC4_0000,
    0x3C4D_0000,
    0xBFAB_0000,
    0xBEAC_0000,
    0x3ED8_0000,
    0x3C76_0000,
    0xBF0C_0000,
    0x3F41_0000,
    0x3E9C_0000,
    0xBE56_0000,
    0x3EC9_0000,
];
const EXPECTED_ATTENTION_OPROJ_FIRST16_BITS: [u32; 16] = [
    0xBF9C_0000,
    0x3E86_0000,
    0xBEFE_0000,
    0xC01F_0000,
    0x3EA0_0000,
    0x3FB1_0000,
    0xBF48_0000,
    0x3FA0_0000,
    0xC0BC_0000,
    0xBC4C_0000,
    0xBF9E_0000,
    0x3E66_0000,
    0xBE4B_0000,
    0xBF7D_0000,
    0xBF19_0000,
    0xBF1F_0000,
];
const EXPECTED_POST_ATTENTION_NORM_FIRST16_BITS: [u32; 16] = [
    0xBE97_0000,
    0x3D4A_0000,
    0xBCCC_0000,
    0xBF51_0000,
    0x3D10_0000,
    0x3E8F_0000,
    0xBD93_0000,
    0x3DEB_0000,
    0xC0B6_0000,
    0xBA8D_0000,
    0xBDE7_0000,
    0x3E43_0000,
    0xBC31_0000,
    0xBF4A_0000,
    0xBD88_0000,
    0xBD8C_0000,
];
const EXPECTED_POST_ATTENTION_RESIDUAL_FIRST16_BITS: [u32; 16] = [
    0xBD38_0000,
    0x3F0D_0000,
    0x3F3A_0000,
    0x3E3C_0000,
    0x3F09_0000,
    0x3E8F_0000,
    0xBF12_0000,
    0xBF63_0000,
    0xC0B2_0000,
    0x3EBF_0000,
    0x3F03_0000,
    0xBF4F_0000,
    0xBF43_0000,
    0xBFA5_0000,
    0xBEA2_0000,
    0xBD8C_0000,
];
const EXPECTED_PRE_FEEDFORWARD_NORM_FIRST16_BITS: [u32; 16] = [
    0xBC59_0000,
    0x3E29_0000,
    0x3E4C_0000,
    0x3D71_0000,
    0x3E20_0000,
    0x3DB2_0000,
    0xBE1B_0000,
    0xBE9F_0000,
    0xC01C_0000,
    0x3DE0_0000,
    0x3E53_0000,
    0xBE64_0000,
    0xBE87_0000,
    0xBECB_0000,
    0xBE04_0000,
    0xBCC0_0000,
];
const EXPECTED_PRE_FEEDFORWARD_GATE_FIRST16_BITS: [u32; 16] = [
    0x404A_0000,
    0xBF73_0000,
    0x3D9D_0000,
    0x3F6F_0000,
    0xBF19_0000,
    0x3F21_0000,
    0xBFB6_0000,
    0x4004_0000,
    0x3F82_0000,
    0x400B_0000,
    0x4000_0000,
    0xC082_0000,
    0x3F7A_0000,
    0x3F81_0000,
    0xBE8A_0000,
    0x3EDD_0000,
];
const EXPECTED_PRE_FEEDFORWARD_UP_FIRST16_BITS: [u32; 16] = [
    0xBF5D_0000,
    0xC0A6_0000,
    0x3E91_0000,
    0xBFC7_0000,
    0xBF0F_0000,
    0xBFEC_0000,
    0xBFAD_0000,
    0x4024_0000,
    0x3F1E_0000,
    0xBEBB_0000,
    0xBE09_0000,
    0x3F0B_0000,
    0xBE8D_0000,
    0x3EC3_0000,
    0xBEC2_0000,
    0x3EC3_0000,
];
const EXPECTED_PRE_FEEDFORWARD_GEGLU_FIRST16_BITS: [u32; 16] = [
    0xC02E_0000,
    0x3F59_0000,
    0x3C3D_0000,
    0xBF99_0000,
    0x3DBE_0000,
    0xBF5A_0000,
    0x3E1A_0000,
    0x40A5_0000,
    0x3F07_0000,
    0xBF48_0000,
    0xBE86_0000,
    0x8000_0000,
    0xBE66_0000,
    0x3EA6_0000,
    0x3D25_0000,
    0x3DE1_0000,
];
const EXPECTED_PRE_FEEDFORWARD_DOWN_FIRST16_BITS: [u32; 16] = [
    0xBEAB_0000,
    0x4040_0000,
    0xBF9F_0000,
    0xC04D_0000,
    0xBF04_0000,
    0x400E_0000,
    0x3F0E_0000,
    0xBE3E_0000,
    0x402B_0000,
    0xC0BB_0000,
    0xC05B_0000,
    0x4069_0000,
    0x4098_0000,
    0x40C6_0000,
    0x40B6_0000,
    0x3D33_0000,
];
const EXPECTED_ROUTER_SCALED_FIRST16_BITS: [u32; 16] = [
    0xBB5E_0000,
    0x3D32_0000,
    0x3D6A_0000,
    0x3C5D_0000,
    0x3D27_0000,
    0x3CB9_0000,
    0xBD3E_0000,
    0xBD93_0000,
    0xBEE5_0000,
    0x3CF7_0000,
    0x3D1B_0000,
    0xBD76_0000,
    0xBD66_0000,
    0xBDD2_0000,
    0xBCCC_0000,
    0xBBAD_0000,
];
const EXPECTED_ROUTER_EXPERT_SCORES_FIRST16_BITS: [u32; 16] = [
    0x3CBB_0000,
    0x3FA7_0000,
    0x3E5E_0000,
    0x3FD2_0000,
    0x4006_0000,
    0x3EDF_0000,
    0x3FF2_0000,
    0x3F4B_0000,
    0x3F98_0000,
    0x3F1B_0000,
    0x3F3A_0000,
    0x3F96_0000,
    0x3E39_0000,
    0x3FCC_0000,
    0x401A_0000,
    0x3F48_0000,
];
const EXPECTED_ROUTER_PROBS_FIRST16_BITS: [u32; 16] = [
    0x3B1D_0000,
    0x3C0D_0000,
    0x3B3E_0000,
    0x3C46_0000,
    0x3C9B_0000,
    0x3B6D_0000,
    0x3C7E_0000,
    0x3BA9_0000,
    0x3BFB_0000,
    0x3B8C_0000,
    0x3B9E_0000,
    0x3BF7_0000,
    0x3B38_0000,
    0x3C3D_0000,
    0x3CD4_0000,
    0x3BA7_0000,
];
const EXPECTED_ROUTER_TOPK_INDICES: [u32; ROUTER_TOP_K] = [51, 55, 41, 114, 14, 100, 88, 103];
const EXPECTED_ROUTER_TOPK_WEIGHTS_FIRST8_BITS: [u32; ROUTER_TOP_K] = [
    0x3E66_0000,
    0x3E31_0000,
    0x3E07_0000,
    0x3DE2_0000,
    0x3DD6_0000,
    0x3DAF_0000,
    0x3DA8_0000,
    0x3DA5_0000,
];

#[derive(Clone, Copy)]
struct ProjectionPathOracle {
    weight_name: &'static str,
    scales_name: &'static str,
    biases_name: &'static str,
    norm_weight_name: Option<&'static str>,
}

const Q_PATH_ORACLE: ProjectionPathOracle = ProjectionPathOracle {
    weight_name: "language_model.model.layers.0.self_attn.q_proj.weight",
    scales_name: "language_model.model.layers.0.self_attn.q_proj.scales",
    biases_name: "language_model.model.layers.0.self_attn.q_proj.biases",
    norm_weight_name: Some(Q_NORM_WEIGHT_NAME),
};

const K_PATH_ORACLE: ProjectionPathOracle = ProjectionPathOracle {
    weight_name: "language_model.model.layers.0.self_attn.k_proj.weight",
    scales_name: "language_model.model.layers.0.self_attn.k_proj.scales",
    biases_name: "language_model.model.layers.0.self_attn.k_proj.biases",
    norm_weight_name: Some(K_NORM_WEIGHT_NAME),
};

const V_PATH_ORACLE: ProjectionPathOracle = ProjectionPathOracle {
    weight_name: "language_model.model.layers.0.self_attn.v_proj.weight",
    scales_name: "language_model.model.layers.0.self_attn.v_proj.scales",
    biases_name: "language_model.model.layers.0.self_attn.v_proj.biases",
    norm_weight_name: None,
};

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
struct MlxAddRowArgs {
    n: u32,
}

#[repr(C)]
struct MlxGegluRowArgs {
    n: u32,
}

#[repr(C)]
struct MlxRouterScaleArgs {
    n: u32,
    eps: f32,
    root_size: f32,
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

#[derive(Clone, Debug)]
struct CachedRouterOutput {
    router_scaled_bits: Vec<u32>,
    expert_scores_bits: Vec<u32>,
    router_probs_bits: Vec<u32>,
    top_k_indices: Vec<u32>,
    top_k_weights_bits: Vec<u32>,
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

fn bf16_words_from_f32_bits(bits: &[u32]) -> Vec<u16> {
    bits.iter().copied().map(|bits| (bits >> 16) as u16).collect()
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

fn decode_bf16_buffer_bits(bytes: &[u8]) -> Vec<u32> {
    bytes.chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .map(bf16_word_to_f32)
        .map(f32::to_bits)
        .collect()
}

fn bits_to_f32(bits: &[u32]) -> Vec<f32> {
    bits.iter().copied().map(f32::from_bits).collect()
}

fn flatten_heads_to_tensor(bits: &[u32], head_count: usize, head_dim: usize) -> Result<KvTensor<f32>, Box<dyn Error>> {
    let shape = KvTensorShape {
        batch_size: 1,
        kv_head_count: head_count,
        seq_len: 1,
        head_dim,
    };
    KvTensor::from_vec(shape, bits_to_f32(bits)).map_err(|err| err.into())
}

fn compute_cached_attention(
    q_bits: &[u32],
    cache: &GemmaKvCache<f32>,
    q_head_count: usize,
    q_heads_per_kv: usize,
    head_dim: usize,
) -> Result<(Vec<u32>, Vec<u32>, Vec<u32>), Box<dyn Error>> {
    let q_values = bits_to_f32(q_bits);
    let cache_state = cache.fetch()?;
    let seq_len = cache_state.stored_tokens();
    let mut score_bits = Vec::with_capacity(q_head_count * seq_len);
    let mut prob_bits = Vec::with_capacity(q_head_count * seq_len);
    let mut out_bits = Vec::with_capacity(q_head_count * head_dim);

    for q_head in 0..q_head_count {
        let q_base = q_head * head_dim;
        let q_row = &q_values[q_base..q_base + head_dim];
        let kv_head = q_head / q_heads_per_kv;

        let mut scores = Vec::with_capacity(seq_len);
        for token in 0..seq_len {
            let k_row = cache_state.keys.row(0, kv_head, token)?;
            let mut sum = 0.0f32;
            for dim in 0..head_dim {
                sum += q_row[dim] * k_row[dim];
            }
            scores.push(bf16_round_to_f32(sum));
        }

        let max_score = scores
            .iter()
            .copied()
            .fold(f32::NEG_INFINITY, f32::max);
        let exp_scores = scores
            .iter()
            .map(|score| (score - max_score).exp())
            .collect::<Vec<_>>();
        let exp_sum = exp_scores.iter().copied().sum::<f32>();
        let probs = exp_scores
            .iter()
            .map(|score| bf16_round_to_f32(score / exp_sum))
            .collect::<Vec<_>>();

        for score in &scores {
            score_bits.push(score.to_bits());
        }
        for prob in &probs {
            prob_bits.push(prob.to_bits());
        }

        for dim in 0..head_dim {
            let mut acc = 0.0f32;
            for (token, prob) in probs.iter().enumerate() {
                let v_row = cache_state.values.row(0, kv_head, token)?;
                acc += *prob * v_row[dim];
            }
            out_bits.push(bf16_round_to_f32(acc).to_bits());
        }
    }

    Ok((score_bits, prob_bits, out_bits))
}

fn compute_router_output_from_expert_scores(
    router_scaled_bits: Vec<u32>,
    expert_scores_bits: Vec<u32>,
    per_expert_scale_words: &[u16],
    top_k: usize,
) -> Result<CachedRouterOutput, Box<dyn Error>> {
    let expert_scores = expert_scores_bits
        .iter()
        .copied()
        .map(f32::from_bits)
        .collect::<Vec<_>>();
    if top_k == 0 || top_k > expert_scores.len() || top_k > per_expert_scale_words.len() {
        return Err(format!(
            "invalid router top_k {} for expert_scores={} per_expert_scales={}",
            top_k,
            expert_scores.len(),
            per_expert_scale_words.len()
        )
        .into());
    }

    let max_score = expert_scores
        .iter()
        .copied()
        .fold(f32::NEG_INFINITY, f32::max);
    let exp_scores = expert_scores
        .iter()
        .copied()
        .map(|value| (value - max_score).exp())
        .collect::<Vec<_>>();
    let exp_sum = exp_scores.iter().copied().sum::<f32>();
    let router_probs = exp_scores
        .iter()
        .copied()
        .map(|value| bf16_round_to_f32(value / exp_sum))
        .collect::<Vec<_>>();
    let router_probs_bits = router_probs.iter().copied().map(f32::to_bits).collect::<Vec<_>>();

    let mut top_k_indices = (0..expert_scores.len()).collect::<Vec<_>>();
    top_k_indices.sort_by(|&lhs, &rhs| {
        expert_scores[rhs]
            .total_cmp(&expert_scores[lhs])
            .then_with(|| lhs.cmp(&rhs))
    });
    let top_k_indices = top_k_indices
        .into_iter()
        .take(top_k)
        .map(|index| index as u32)
        .collect::<Vec<_>>();

    let mut top_k_weights = top_k_indices
        .iter()
        .copied()
        .map(|index| router_probs[index as usize])
        .collect::<Vec<_>>();
    let mut top_k_sum = 0.0f32;
    for weight in &top_k_weights {
        top_k_sum = bf16_round_to_f32(top_k_sum + *weight);
    }
    for (slot, weight) in top_k_weights.iter_mut().enumerate() {
        let normalized = bf16_round_to_f32(*weight / top_k_sum);
        let expert_scale = bf16_word_to_f32(per_expert_scale_words[top_k_indices[slot] as usize]);
        *weight = bf16_round_to_f32(normalized * expert_scale);
    }

    Ok(CachedRouterOutput {
        router_scaled_bits,
        expert_scores_bits,
        router_probs_bits,
        top_k_indices,
        top_k_weights_bits: top_k_weights.iter().copied().map(f32::to_bits).collect(),
    })
}

fn validate_hash_and_prefix<const N: usize>(
    label: &str,
    bits: &[u32],
    expected_hash: u64,
    expected_prefix: &[u32; N],
) -> Result<(), Box<dyn Error>> {
    let hash = fnv1a64_u32_words(bits);
    let prefix = &bits[..bits.len().min(N)];
    if expected_hash != 0 && hash != expected_hash {
        return Err(format!(
            "{label} hash mismatch: got 0x{hash:016X} expected 0x{expected_hash:016X}"
        )
        .into());
    }
    if expected_prefix.iter().any(|word| *word != 0) && prefix != expected_prefix {
        return Err(format!("{label} prefix mismatch").into());
    }
    Ok(())
}

fn print_prefix(label: &str, bits: &[u32], count: usize) {
    print!("{label}=");
    for (index, bit) in bits.iter().take(count).enumerate() {
        if index != 0 {
            print!(",");
        }
        print!("0x{bit:08X}");
    }
    println!();
}

fn print_first16(label: &str, bits: &[u32]) {
    print_prefix(label, bits, 16);
}

pub fn run_cli() -> Result<(), Box<dyn Error>> {
    let mut model_path = default_model_path();
    let mut validate_oproj = false;
    let mut validate_residual = false;
    let mut validate_pre_ffn_norm = false;
    let mut validate_dense_gate = false;
    let mut validate_dense_up = false;
    let mut validate_dense_geglu = false;
    let mut validate_dense_down = false;
    let mut validate_router = false;
    let mut args_iter = env::args().skip(1);
    while let Some(arg) = args_iter.next() {
        match arg.as_str() {
            "--oproj" => {
                validate_oproj = true;
            }
            "--residual" => {
                validate_residual = true;
                validate_oproj = true;
            }
            "--pre-ffn-norm" => {
                validate_pre_ffn_norm = true;
                validate_residual = true;
                validate_oproj = true;
            }
            "--dense-gate" => {
                validate_dense_gate = true;
                validate_pre_ffn_norm = true;
                validate_residual = true;
                validate_oproj = true;
            }
            "--dense-up" => {
                validate_dense_up = true;
                validate_pre_ffn_norm = true;
                validate_residual = true;
                validate_oproj = true;
            }
            "--dense-geglu" => {
                validate_dense_geglu = true;
                validate_dense_gate = true;
                validate_dense_up = true;
                validate_pre_ffn_norm = true;
                validate_residual = true;
                validate_oproj = true;
            }
            "--dense-down" => {
                validate_dense_down = true;
                validate_dense_geglu = true;
                validate_dense_gate = true;
                validate_dense_up = true;
                validate_pre_ffn_norm = true;
                validate_residual = true;
                validate_oproj = true;
            }
            "--router" => {
                validate_router = true;
                validate_residual = true;
                validate_oproj = true;
            }
            "-h" | "--help" => {
                eprintln!(
                    "Usage: metal_qkv_attention_output_cached_row [model.safetensors] [--oproj] [--residual] [--pre-ffn-norm] [--dense-gate] [--dense-up] [--dense-geglu] [--dense-down] [--router]"
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

    let header = MlxSafetensorsHeader::load(&model_path)?;
    let runtime = MetalRuntime::new().map_err(|err| format!("MetalRuntime::new failed: {err}"))?;
    if !runtime.features().has_bfloat {
        return Err("Metal device does not report BF16 support".into());
    }

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
    let o_weight_bytes = if validate_oproj {
        Some(header.read_tensor_bytes(O_PROJ_WEIGHT_NAME)?)
    } else {
        None
    };
    let o_scales_bytes = if validate_oproj {
        Some(header.read_tensor_bytes(O_PROJ_SCALES_NAME)?)
    } else {
        None
    };
    let o_biases_bytes = if validate_oproj {
        Some(header.read_tensor_bytes(O_PROJ_BIASES_NAME)?)
    } else {
        None
    };
    let post_attention_norm_weight_bytes = if validate_residual {
        Some(header.read_tensor_bytes(POST_ATTENTION_NORM_WEIGHT_NAME)?)
    } else {
        None
    };
    let pre_feedforward_norm_weight_bytes = if validate_pre_ffn_norm {
        Some(header.read_tensor_bytes(PRE_FEEDFORWARD_NORM_WEIGHT_NAME)?)
    } else {
        None
    };
    let mlp_gate_weight_bytes = if validate_dense_gate {
        Some(header.read_tensor_bytes(MLP_GATE_WEIGHT_NAME)?)
    } else {
        None
    };
    let mlp_gate_scales_bytes = if validate_dense_gate {
        Some(header.read_tensor_bytes(MLP_GATE_SCALES_NAME)?)
    } else {
        None
    };
    let mlp_gate_biases_bytes = if validate_dense_gate {
        Some(header.read_tensor_bytes(MLP_GATE_BIASES_NAME)?)
    } else {
        None
    };
    let mlp_up_weight_bytes = if validate_dense_up {
        Some(header.read_tensor_bytes(MLP_UP_WEIGHT_NAME)?)
    } else {
        None
    };
    let mlp_up_scales_bytes = if validate_dense_up {
        Some(header.read_tensor_bytes(MLP_UP_SCALES_NAME)?)
    } else {
        None
    };
    let mlp_up_biases_bytes = if validate_dense_up {
        Some(header.read_tensor_bytes(MLP_UP_BIASES_NAME)?)
    } else {
        None
    };
    let mlp_down_weight_bytes = if validate_dense_down {
        Some(header.read_tensor_bytes(MLP_DOWN_WEIGHT_NAME)?)
    } else {
        None
    };
    let mlp_down_scales_bytes = if validate_dense_down {
        Some(header.read_tensor_bytes(MLP_DOWN_SCALES_NAME)?)
    } else {
        None
    };
    let mlp_down_biases_bytes = if validate_dense_down {
        Some(header.read_tensor_bytes(MLP_DOWN_BIASES_NAME)?)
    } else {
        None
    };
    let router_scale_bytes = if validate_router {
        Some(header.read_tensor_bytes(ROUTER_SCALE_NAME)?)
    } else {
        None
    };
    let router_proj_weight_bytes = if validate_router {
        Some(header.read_tensor_bytes(ROUTER_PROJ_WEIGHT_NAME)?)
    } else {
        None
    };
    let router_proj_scales_bytes = if validate_router {
        Some(header.read_tensor_bytes(ROUTER_PROJ_SCALES_NAME)?)
    } else {
        None
    };
    let router_proj_biases_bytes = if validate_router {
        Some(header.read_tensor_bytes(ROUTER_PROJ_BIASES_NAME)?)
    } else {
        None
    };
    let router_per_expert_scale_words = if validate_router {
        Some(header.read_bf16_tensor_words(ROUTER_PER_EXPERT_SCALE_NAME)?)
    } else {
        None
    };

    let q_weight_entry = header.tensor(Q_PATH_ORACLE.weight_name).ok_or("missing q projection weight entry")?;
    let q_scales_entry = header.tensor(Q_PATH_ORACLE.scales_name).ok_or("missing q projection scales entry")?;
    let q_norm_weight_entry = header.tensor(Q_PATH_ORACLE.norm_weight_name.unwrap()).ok_or("missing q norm weight entry")?;
    let k_weight_entry = header.tensor(K_PATH_ORACLE.weight_name).ok_or("missing k projection weight entry")?;
    let k_scales_entry = header.tensor(K_PATH_ORACLE.scales_name).ok_or("missing k projection scales entry")?;
    let k_norm_weight_entry = header.tensor(K_PATH_ORACLE.norm_weight_name.unwrap()).ok_or("missing k norm weight entry")?;
    let v_weight_entry = header.tensor(V_PATH_ORACLE.weight_name).ok_or("missing v projection weight entry")?;
    let v_scales_entry = header.tensor(V_PATH_ORACLE.scales_name).ok_or("missing v projection scales entry")?;
    let o_weight_entry = if validate_oproj {
        Some(header.tensor(O_PROJ_WEIGHT_NAME).ok_or("missing o_proj weight entry")?)
    } else {
        None
    };
    let o_scales_entry = if validate_oproj {
        Some(header.tensor(O_PROJ_SCALES_NAME).ok_or("missing o_proj scales entry")?)
    } else {
        None
    };
    let post_attention_norm_weight_entry = if validate_residual {
        Some(
            header
                .tensor(POST_ATTENTION_NORM_WEIGHT_NAME)
                .ok_or("missing post-attention norm weight entry")?,
        )
    } else {
        None
    };
    let pre_feedforward_norm_weight_entry = if validate_pre_ffn_norm {
        Some(
            header
                .tensor(PRE_FEEDFORWARD_NORM_WEIGHT_NAME)
                .ok_or("missing pre-feedforward norm weight entry")?,
        )
    } else {
        None
    };
    let mlp_gate_weight_entry = if validate_dense_gate {
        Some(
            header
                .tensor(MLP_GATE_WEIGHT_NAME)
                .ok_or("missing mlp gate_proj weight entry")?,
        )
    } else {
        None
    };
    let mlp_gate_scales_entry = if validate_dense_gate {
        Some(
            header
                .tensor(MLP_GATE_SCALES_NAME)
                .ok_or("missing mlp gate_proj scales entry")?,
        )
    } else {
        None
    };
    let mlp_up_weight_entry = if validate_dense_up {
        Some(
            header
                .tensor(MLP_UP_WEIGHT_NAME)
                .ok_or("missing mlp up_proj weight entry")?,
        )
    } else {
        None
    };
    let mlp_up_scales_entry = if validate_dense_up {
        Some(
            header
                .tensor(MLP_UP_SCALES_NAME)
                .ok_or("missing mlp up_proj scales entry")?,
        )
    } else {
        None
    };
    let mlp_down_weight_entry = if validate_dense_down {
        Some(
            header
                .tensor(MLP_DOWN_WEIGHT_NAME)
                .ok_or("missing mlp down_proj weight entry")?,
        )
    } else {
        None
    };
    let mlp_down_scales_entry = if validate_dense_down {
        Some(
            header
                .tensor(MLP_DOWN_SCALES_NAME)
                .ok_or("missing mlp down_proj scales entry")?,
        )
    } else {
        None
    };
    let router_scale_entry = if validate_router {
        Some(
            header
                .tensor(ROUTER_SCALE_NAME)
                .ok_or("missing router scale entry")?,
        )
    } else {
        None
    };
    let router_proj_weight_entry = if validate_router {
        Some(
            header
                .tensor(ROUTER_PROJ_WEIGHT_NAME)
                .ok_or("missing router proj weight entry")?,
        )
    } else {
        None
    };
    let router_proj_scales_entry = if validate_router {
        Some(
            header
                .tensor(ROUTER_PROJ_SCALES_NAME)
                .ok_or("missing router proj scales entry")?,
        )
    } else {
        None
    };

    let q_out_len = usize::try_from(q_weight_entry.shape[0])?;
    let k_out_len = usize::try_from(k_weight_entry.shape[0])?;
    let v_out_len = usize::try_from(v_weight_entry.shape[0])?;
    let o_out_len = if let Some(entry) = o_weight_entry {
        usize::try_from(entry.shape[0])?
    } else {
        0
    };
    let post_attention_norm_len = if let Some(entry) = post_attention_norm_weight_entry {
        usize::try_from(entry.shape[0])?
    } else {
        0
    };
    let pre_feedforward_norm_len = if let Some(entry) = pre_feedforward_norm_weight_entry {
        usize::try_from(entry.shape[0])?
    } else {
        0
    };
    let mlp_gate_out_len = if let Some(entry) = mlp_gate_weight_entry {
        usize::try_from(entry.shape[0])?
    } else {
        0
    };
    let mlp_up_out_len = if let Some(entry) = mlp_up_weight_entry {
        usize::try_from(entry.shape[0])?
    } else {
        0
    };
    let mlp_down_out_len = if let Some(entry) = mlp_down_weight_entry {
        usize::try_from(entry.shape[0])?
    } else {
        0
    };
    let router_scale_len = if let Some(entry) = router_scale_entry {
        usize::try_from(entry.shape[0])?
    } else {
        0
    };
    let router_out_len = if let Some(entry) = router_proj_weight_entry {
        usize::try_from(entry.shape[0])?
    } else {
        0
    };
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
    if validate_residual && post_attention_norm_len != o_out_len {
        return Err(format!(
            "invalid post-attention norm length: got {} expected {}",
            post_attention_norm_len, o_out_len
        )
        .into());
    }
    if validate_pre_ffn_norm && pre_feedforward_norm_len != post_attention_norm_len {
        return Err(format!(
            "invalid pre-feedforward norm length: got {} expected {}",
            pre_feedforward_norm_len, post_attention_norm_len
        )
        .into());
    }
    if let Some(weight_entry) = mlp_gate_weight_entry {
        let mlp_gate_n_in = usize::try_from(weight_entry.shape[1] * 8)?;
        if mlp_gate_n_in != pre_feedforward_norm_len {
            return Err(format!(
                "invalid mlp gate_proj input size: got {} expected {}",
                mlp_gate_n_in, pre_feedforward_norm_len
            )
            .into());
        }
    }
    if let Some(weight_entry) = mlp_up_weight_entry {
        let mlp_up_n_in = usize::try_from(weight_entry.shape[1] * 8)?;
        if mlp_up_n_in != pre_feedforward_norm_len {
            return Err(format!(
                "invalid mlp up_proj input size: got {} expected {}",
                mlp_up_n_in, pre_feedforward_norm_len
            )
            .into());
        }
    }
    if let Some(weight_entry) = mlp_down_weight_entry {
        let mlp_down_n_in = usize::try_from(weight_entry.shape[1] * 8)?;
        if mlp_down_n_in != mlp_gate_out_len {
            return Err(format!(
                "invalid mlp down_proj input size: got {} expected {}",
                mlp_down_n_in, mlp_gate_out_len
            )
            .into());
        }
        if mlp_down_out_len != pre_feedforward_norm_len {
            return Err(format!(
                "invalid mlp down_proj output size: got {} expected {}",
                mlp_down_out_len, pre_feedforward_norm_len
            )
            .into());
        }
    }
    if validate_router && router_scale_len != post_attention_norm_len {
        return Err(format!(
            "invalid router scale length: got {} expected {}",
            router_scale_len, post_attention_norm_len
        )
        .into());
    }
    if validate_router && router_out_len < ROUTER_TOP_K {
        return Err(format!(
            "invalid router output length: got {} expected at least {}",
            router_out_len, ROUTER_TOP_K
        )
        .into());
    }

    let prefill_x_words = gemma4_qproj_case_input_bf16_words_with_phase(NORM_LEN, PREFILL_ACTIVATION_PHASE);
    let decode_x_words = gemma4_qproj_case_input_bf16_words_with_phase(NORM_LEN, DECODE_ACTIVATION_PHASE);
    let x_buf = runtime.create_buffer(NORM_LEN * 2, BufferStorageMode::Shared)?;
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
    let ones_bytes = bytes_from_bf16_words(&vec![0x3F80u16; head_dim]);
    let v_norm_weight_buf = runtime.create_buffer_with_bytes(&ones_bytes, BufferStorageMode::Private)?;
    let o_weight_buf = if let Some(bytes) = &o_weight_bytes {
        Some(runtime.create_buffer_with_bytes(bytes, BufferStorageMode::Private)?)
    } else {
        None
    };
    let o_scales_buf = if let Some(bytes) = &o_scales_bytes {
        Some(runtime.create_buffer_with_bytes(bytes, BufferStorageMode::Private)?)
    } else {
        None
    };
    let o_biases_buf = if let Some(bytes) = &o_biases_bytes {
        Some(runtime.create_buffer_with_bytes(bytes, BufferStorageMode::Private)?)
    } else {
        None
    };
    let post_attention_norm_weight_buf = if let Some(bytes) = &post_attention_norm_weight_bytes {
        Some(runtime.create_buffer_with_bytes(bytes, BufferStorageMode::Private)?)
    } else {
        None
    };
    let pre_feedforward_norm_weight_buf = if let Some(bytes) = &pre_feedforward_norm_weight_bytes {
        Some(runtime.create_buffer_with_bytes(bytes, BufferStorageMode::Private)?)
    } else {
        None
    };
    let mlp_gate_weight_buf = if let Some(bytes) = &mlp_gate_weight_bytes {
        Some(runtime.create_buffer_with_bytes(bytes, BufferStorageMode::Private)?)
    } else {
        None
    };
    let mlp_gate_scales_buf = if let Some(bytes) = &mlp_gate_scales_bytes {
        Some(runtime.create_buffer_with_bytes(bytes, BufferStorageMode::Private)?)
    } else {
        None
    };
    let mlp_gate_biases_buf = if let Some(bytes) = &mlp_gate_biases_bytes {
        Some(runtime.create_buffer_with_bytes(bytes, BufferStorageMode::Private)?)
    } else {
        None
    };
    let mlp_up_weight_buf = if let Some(bytes) = &mlp_up_weight_bytes {
        Some(runtime.create_buffer_with_bytes(bytes, BufferStorageMode::Private)?)
    } else {
        None
    };
    let mlp_up_scales_buf = if let Some(bytes) = &mlp_up_scales_bytes {
        Some(runtime.create_buffer_with_bytes(bytes, BufferStorageMode::Private)?)
    } else {
        None
    };
    let mlp_up_biases_buf = if let Some(bytes) = &mlp_up_biases_bytes {
        Some(runtime.create_buffer_with_bytes(bytes, BufferStorageMode::Private)?)
    } else {
        None
    };
    let mlp_down_weight_buf = if let Some(bytes) = &mlp_down_weight_bytes {
        Some(runtime.create_buffer_with_bytes(bytes, BufferStorageMode::Private)?)
    } else {
        None
    };
    let mlp_down_scales_buf = if let Some(bytes) = &mlp_down_scales_bytes {
        Some(runtime.create_buffer_with_bytes(bytes, BufferStorageMode::Private)?)
    } else {
        None
    };
    let mlp_down_biases_buf = if let Some(bytes) = &mlp_down_biases_bytes {
        Some(runtime.create_buffer_with_bytes(bytes, BufferStorageMode::Private)?)
    } else {
        None
    };
    let router_scale_weight_buf = if let Some(bytes) = &router_scale_bytes {
        Some(runtime.create_buffer_with_bytes(bytes, BufferStorageMode::Private)?)
    } else {
        None
    };
    let router_proj_weight_buf = if let Some(bytes) = &router_proj_weight_bytes {
        Some(runtime.create_buffer_with_bytes(bytes, BufferStorageMode::Private)?)
    } else {
        None
    };
    let router_proj_scales_buf = if let Some(bytes) = &router_proj_scales_bytes {
        Some(runtime.create_buffer_with_bytes(bytes, BufferStorageMode::Private)?)
    } else {
        None
    };
    let router_proj_biases_buf = if let Some(bytes) = &router_proj_biases_bytes {
        Some(runtime.create_buffer_with_bytes(bytes, BufferStorageMode::Private)?)
    } else {
        None
    };
    let attn_out_buf = if validate_oproj {
        Some(runtime.create_buffer(q_out_len * 2, BufferStorageMode::Shared)?)
    } else {
        None
    };
    let o_proj_out_buf = if validate_oproj {
        Some(runtime.create_buffer(o_out_len * 2, BufferStorageMode::Private)?)
    } else {
        None
    };
    let post_attention_norm_out_buf = if validate_residual {
        Some(runtime.create_buffer(post_attention_norm_len * 2, BufferStorageMode::Private)?)
    } else {
        None
    };
    let residual_out_buf = if validate_residual {
        Some(runtime.create_buffer(post_attention_norm_len * 2, BufferStorageMode::Private)?)
    } else {
        None
    };
    let pre_feedforward_norm_out_buf = if validate_pre_ffn_norm {
        Some(runtime.create_buffer(pre_feedforward_norm_len * 2, BufferStorageMode::Private)?)
    } else {
        None
    };
    let mlp_gate_out_buf = if validate_dense_gate {
        Some(runtime.create_buffer(mlp_gate_out_len * 2, BufferStorageMode::Private)?)
    } else {
        None
    };
    let mlp_up_out_buf = if validate_dense_up {
        Some(runtime.create_buffer(mlp_up_out_len * 2, BufferStorageMode::Private)?)
    } else {
        None
    };
    let geglu_out_buf = if validate_dense_geglu {
        Some(runtime.create_buffer(mlp_gate_out_len * 2, BufferStorageMode::Private)?)
    } else {
        None
    };
    let mlp_down_out_buf = if validate_dense_down {
        Some(runtime.create_buffer(mlp_down_out_len * 2, BufferStorageMode::Private)?)
    } else {
        None
    };
    let router_scaled_out_buf = if validate_router {
        Some(runtime.create_buffer(post_attention_norm_len * 2, BufferStorageMode::Private)?)
    } else {
        None
    };
    let router_proj_out_buf = if validate_router {
        Some(runtime.create_buffer(router_out_len * 2, BufferStorageMode::Shared)?)
    } else {
        None
    };

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
    let o_proj_fast_pipeline = if validate_oproj {
        Some(runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
            cache_name: "kernel_mlx_affine_qmv_fast_row_bf16".to_string(),
            base_name: "kernel_mlx_affine_qmv_fast_row_bf16".to_string(),
            constants: Vec::new(),
            smem_bytes: 0,
            nr0: 0,
            nr1: 0,
            nsg: 0,
        })?)
    } else {
        None
    };
    let residual_pipeline = if validate_residual {
        Some(runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
            cache_name: "kernel_mlx_add_row_bf16".to_string(),
            base_name: "kernel_mlx_add_row_bf16".to_string(),
            constants: Vec::new(),
            smem_bytes: 0,
            nr0: 0,
            nr1: 0,
            nsg: 0,
        })?)
    } else {
        None
    };
    let geglu_pipeline = if validate_dense_geglu {
        Some(runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
            cache_name: "kernel_mlx_geglu_row_bf16".to_string(),
            base_name: "kernel_mlx_geglu_row_bf16".to_string(),
            constants: Vec::new(),
            smem_bytes: 0,
            nr0: 0,
            nr1: 0,
            nsg: 0,
        })?)
    } else {
        None
    };
    let router_scale_pipeline = if validate_router {
        Some(runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
            cache_name: "kernel_mlx_router_scale_bf16".to_string(),
            base_name: "kernel_mlx_router_scale_bf16".to_string(),
            constants: Vec::new(),
            smem_bytes: 0,
            nr0: 0,
            nr1: 0,
            nsg: 0,
        })?)
    } else {
        None
    };

    let n_reads = 4usize;
    let simd_size = 32usize;
    let rms_threadgroup_needed = NORM_LEN.div_ceil(n_reads);
    let rms_simds_needed = rms_threadgroup_needed.div_ceil(simd_size);
    let rms_threadgroup_size = simd_size * rms_simds_needed;
    let head_norm_threadgroup_needed = head_dim.div_ceil(n_reads);
    let head_norm_simds_needed = head_norm_threadgroup_needed.div_ceil(simd_size);
    let head_norm_threadgroup_size = simd_size * head_norm_simds_needed;

    let rms_args = MlxRmsNormRowArgs {
        n: NORM_LEN as u32,
        eps: EPS,
    };
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
    let o_proj_args = if let (Some(weight_entry), Some(scales_entry)) = (o_weight_entry, o_scales_entry) {
        Some(MlxAffineQprojRowArgs {
            n_in: q_out_len as u32,
            weight_words_per_row: weight_entry.shape[1] as u32,
            qparams_per_row: scales_entry.shape[1] as u32,
            out_rows: o_out_len as u32,
        })
    } else {
        None
    };
    let post_attention_norm_args = if validate_residual {
        Some(MlxRmsNormRowArgs {
            n: post_attention_norm_len as u32,
            eps: EPS,
        })
    } else {
        None
    };
    let residual_args = if validate_residual {
        Some(MlxAddRowArgs {
            n: post_attention_norm_len as u32,
        })
    } else {
        None
    };
    let pre_feedforward_norm_args = if validate_pre_ffn_norm {
        Some(MlxRmsNormRowArgs {
            n: pre_feedforward_norm_len as u32,
            eps: EPS,
        })
    } else {
        None
    };
    let router_scale_args = if validate_router {
        Some(MlxRouterScaleArgs {
            n: post_attention_norm_len as u32,
            eps: EPS,
            root_size: bf16_round_to_f32((post_attention_norm_len as f32).powf(-0.5)),
        })
    } else {
        None
    };
    let mlp_gate_args = if let (Some(weight_entry), Some(scales_entry)) =
        (mlp_gate_weight_entry, mlp_gate_scales_entry)
    {
        Some(MlxAffineQprojRowArgs {
            n_in: pre_feedforward_norm_len as u32,
            weight_words_per_row: weight_entry.shape[1] as u32,
            qparams_per_row: scales_entry.shape[1] as u32,
            out_rows: mlp_gate_out_len as u32,
        })
    } else {
        None
    };
    let mlp_up_args = if let (Some(weight_entry), Some(scales_entry)) =
        (mlp_up_weight_entry, mlp_up_scales_entry)
    {
        Some(MlxAffineQprojRowArgs {
            n_in: pre_feedforward_norm_len as u32,
            weight_words_per_row: weight_entry.shape[1] as u32,
            qparams_per_row: scales_entry.shape[1] as u32,
            out_rows: mlp_up_out_len as u32,
        })
    } else {
        None
    };
    let geglu_args = if validate_dense_geglu {
        Some(MlxGegluRowArgs {
            n: mlp_gate_out_len as u32,
        })
    } else {
        None
    };
    let mlp_down_args = if let (Some(weight_entry), Some(scales_entry)) =
        (mlp_down_weight_entry, mlp_down_scales_entry)
    {
        Some(MlxAffineQprojRowArgs {
            n_in: mlp_gate_out_len as u32,
            weight_words_per_row: weight_entry.shape[1] as u32,
            qparams_per_row: scales_entry.shape[1] as u32,
            out_rows: mlp_down_out_len as u32,
        })
    } else {
        None
    };
    let router_proj_args = if let (Some(weight_entry), Some(scales_entry)) =
        (router_proj_weight_entry, router_proj_scales_entry)
    {
        Some(MlxAffineQprojRowArgs {
            n_in: post_attention_norm_len as u32,
            weight_words_per_row: weight_entry.shape[1] as u32,
            qparams_per_row: scales_entry.shape[1] as u32,
            out_rows: router_out_len as u32,
        })
    } else {
        None
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
    let v_proj_bindings = [
        MetalBufferBindingRef { index: 1, buffer: &h_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 2, buffer: &v_weight_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 3, buffer: &v_scales_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 4, buffer: &v_biases_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 5, buffer: &v_proj_buf, offset_bytes: 0 },
    ];
    let v_head_norm_bindings = [
        MetalBufferBindingRef { index: 1, buffer: &v_proj_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 2, buffer: &v_norm_weight_buf, offset_bytes: 0 },
        MetalBufferBindingRef { index: 3, buffer: &v_norm_buf, offset_bytes: 0 },
    ];
    let router_scale_bindings = if let (Some(scale_buf), Some(out_buf)) =
        (router_scale_weight_buf.as_ref(), router_scaled_out_buf.as_ref())
    {
        Some([
            MetalBufferBindingRef {
                index: 1,
                buffer: residual_out_buf
                    .as_ref()
                    .ok_or("missing residual output buffer for router")?,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 2,
                buffer: scale_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 3,
                buffer: out_buf,
                offset_bytes: 0,
            },
        ])
    } else {
        None
    };
    let mlp_gate_bindings = if let (Some(weight_buf), Some(scales_buf), Some(biases_buf), Some(out_buf)) = (
        mlp_gate_weight_buf.as_ref(),
        mlp_gate_scales_buf.as_ref(),
        mlp_gate_biases_buf.as_ref(),
        mlp_gate_out_buf.as_ref(),
    ) {
        Some([
            MetalBufferBindingRef {
                index: 1,
                buffer: pre_feedforward_norm_out_buf
                    .as_ref()
                    .ok_or("missing pre-feedforward norm output buffer")?,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 2,
                buffer: weight_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 3,
                buffer: scales_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 4,
                buffer: biases_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 5,
                buffer: out_buf,
                offset_bytes: 0,
            },
        ])
    } else {
        None
    };
    let mlp_up_bindings = if let (Some(weight_buf), Some(scales_buf), Some(biases_buf), Some(out_buf)) = (
        mlp_up_weight_buf.as_ref(),
        mlp_up_scales_buf.as_ref(),
        mlp_up_biases_buf.as_ref(),
        mlp_up_out_buf.as_ref(),
    ) {
        Some([
            MetalBufferBindingRef {
                index: 1,
                buffer: pre_feedforward_norm_out_buf
                    .as_ref()
                    .ok_or("missing pre-feedforward norm output buffer")?,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 2,
                buffer: weight_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 3,
                buffer: scales_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 4,
                buffer: biases_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 5,
                buffer: out_buf,
                offset_bytes: 0,
            },
        ])
    } else {
        None
    };
    let geglu_bindings = if let (Some(out_buf), Some(gate_buf), Some(up_buf)) = (
        geglu_out_buf.as_ref(),
        mlp_gate_out_buf.as_ref(),
        mlp_up_out_buf.as_ref(),
    ) {
        Some([
            MetalBufferBindingRef {
                index: 1,
                buffer: gate_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 2,
                buffer: up_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 3,
                buffer: out_buf,
                offset_bytes: 0,
            },
        ])
    } else {
        None
    };
    let mlp_down_bindings = if let (Some(weight_buf), Some(scales_buf), Some(biases_buf), Some(out_buf)) = (
        mlp_down_weight_buf.as_ref(),
        mlp_down_scales_buf.as_ref(),
        mlp_down_biases_buf.as_ref(),
        mlp_down_out_buf.as_ref(),
    ) {
        Some([
            MetalBufferBindingRef {
                index: 1,
                buffer: geglu_out_buf
                    .as_ref()
                    .ok_or("missing geglu output buffer")?,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 2,
                buffer: weight_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 3,
                buffer: scales_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 4,
                buffer: biases_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 5,
                buffer: out_buf,
                offset_bytes: 0,
            },
        ])
    } else {
        None
    };
    let router_proj_bindings = if let (Some(weight_buf), Some(scales_buf), Some(biases_buf), Some(out_buf)) = (
        router_proj_weight_buf.as_ref(),
        router_proj_scales_buf.as_ref(),
        router_proj_biases_buf.as_ref(),
        router_proj_out_buf.as_ref(),
    ) {
        Some([
            MetalBufferBindingRef {
                index: 1,
                buffer: router_scaled_out_buf
                    .as_ref()
                    .ok_or("missing router scaled output buffer")?,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 2,
                buffer: weight_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 3,
                buffer: scales_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 4,
                buffer: biases_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 5,
                buffer: out_buf,
                offset_bytes: 0,
            },
        ])
    } else {
        None
    };

    let rms_threadgroups = MetalSize { width: 1, height: 1, depth: 1 };
    let rms_threads_per_threadgroup = MetalSize { width: rms_threadgroup_size as u64, height: 1, depth: 1 };
    let q_proj_threadgroups = MetalSize { width: 1, height: (q_out_len as u64).div_ceil(8), depth: 1 };
    let q_proj_threads_per_threadgroup = MetalSize { width: 32, height: 2, depth: 1 };
    let q_head_norm_threadgroups = MetalSize { width: q_head_count as u64, height: 1, depth: 1 };
    let q_head_norm_threads_per_threadgroup = MetalSize { width: head_norm_threadgroup_size as u64, height: 1, depth: 1 };
    let k_proj_threadgroups = MetalSize { width: 1, height: (k_out_len as u64).div_ceil(8), depth: 1 };
    let k_proj_threads_per_threadgroup = MetalSize { width: 32, height: 2, depth: 1 };
    let k_head_norm_threadgroups = MetalSize { width: k_head_count as u64, height: 1, depth: 1 };
    let k_head_norm_threads_per_threadgroup = MetalSize { width: head_norm_threadgroup_size as u64, height: 1, depth: 1 };
    let v_proj_threadgroups = MetalSize { width: 1, height: (v_out_len as u64).div_ceil(8), depth: 1 };
    let v_proj_threads_per_threadgroup = MetalSize { width: 32, height: 2, depth: 1 };
    let v_head_norm_threadgroups = MetalSize { width: v_head_count as u64, height: 1, depth: 1 };
    let v_head_norm_threads_per_threadgroup = MetalSize { width: head_norm_threadgroup_size as u64, height: 1, depth: 1 };
    let o_proj_threadgroups = MetalSize { width: 1, height: (o_out_len as u64).div_ceil(8), depth: 1 };
    let o_proj_threads_per_threadgroup = MetalSize { width: 32, height: 2, depth: 1 };
    let residual_threads_per_threadgroup = MetalSize { width: 256, height: 1, depth: 1 };
    let residual_threadgroups = MetalSize {
        width: (post_attention_norm_len as u64).div_ceil(residual_threads_per_threadgroup.width),
        height: 1,
        depth: 1,
    };
    let pre_feedforward_norm_threadgroups = MetalSize { width: 1, height: 1, depth: 1 };
    let mlp_gate_threadgroups =
        MetalSize { width: 1, height: (mlp_gate_out_len as u64).div_ceil(8), depth: 1 };
    let mlp_gate_threads_per_threadgroup = MetalSize { width: 32, height: 2, depth: 1 };
    let mlp_up_threadgroups =
        MetalSize { width: 1, height: (mlp_up_out_len as u64).div_ceil(8), depth: 1 };
    let mlp_up_threads_per_threadgroup = MetalSize { width: 32, height: 2, depth: 1 };
    let geglu_threads_per_threadgroup = MetalSize { width: 256, height: 1, depth: 1 };
    let geglu_threadgroups = MetalSize {
        width: (mlp_gate_out_len as u64).div_ceil(geglu_threads_per_threadgroup.width),
        height: 1,
        depth: 1,
    };
    let mlp_down_threadgroups =
        MetalSize { width: 1, height: (mlp_down_out_len as u64).div_ceil(8), depth: 1 };
    let mlp_down_threads_per_threadgroup = MetalSize { width: 32, height: 2, depth: 1 };
    let router_scale_threads_per_threadgroup = MetalSize { width: 256, height: 1, depth: 1 };
    let router_scale_threadgroups = MetalSize { width: 1, height: 1, depth: 1 };
    let router_proj_threadgroups =
        MetalSize { width: 1, height: (router_out_len as u64).div_ceil(8), depth: 1 };
    let router_proj_threads_per_threadgroup = MetalSize { width: 32, height: 2, depth: 1 };

    let run_projection = |input_words: &[u16], rope_offset: i32| -> Result<(Vec<u32>, Vec<u32>, Vec<u32>), Box<dyn Error>> {
        runtime.write_buffer(&x_buf, 0, &bytes_from_bf16_words(input_words))?;

        let q_rope_args = MlxRopeSingleArgs {
            half_dims: (head_dim / 2) as u32,
            row_stride: head_dim as u32,
            row_count: q_head_count as u32,
            offset: rope_offset,
            scale: ROPE_SCALE,
            base_log2: ROPE_BASE.log2(),
        };
        let k_rope_args = MlxRopeSingleArgs {
            half_dims: (head_dim / 2) as u32,
            row_stride: head_dim as u32,
            row_count: k_head_count as u32,
            offset: rope_offset,
            scale: ROPE_SCALE,
            base_log2: ROPE_BASE.log2(),
        };
        let q_rope_bindings = [
            MetalBufferBindingRef { index: 1, buffer: &q_norm_buf, offset_bytes: 0 },
            MetalBufferBindingRef { index: 2, buffer: &q_rope_buf, offset_bytes: 0 },
        ];
        let k_rope_bindings = [
            MetalBufferBindingRef { index: 1, buffer: &k_norm_buf, offset_bytes: 0 },
            MetalBufferBindingRef { index: 2, buffer: &k_rope_buf, offset_bytes: 0 },
        ];
        let q_rope_threadgroups = MetalSize { width: ((head_dim / 2) as u64).div_ceil(32), height: q_head_count as u64, depth: 1 };
        let q_rope_threads_per_threadgroup = MetalSize { width: 32, height: 1, depth: 1 };
        let k_rope_threadgroups = MetalSize { width: ((head_dim / 2) as u64).div_ceil(32), height: k_head_count as u64, depth: 1 };
        let k_rope_threads_per_threadgroup = MetalSize { width: 32, height: 1, depth: 1 };

        runtime.begin_command_batch()?;
        runtime.dispatch_compute(&rms_pipeline, bytes_of(&rms_args), &rms_bindings, &[], rms_threadgroups, rms_threads_per_threadgroup)?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(&proj_pipeline, bytes_of(&q_proj_args), &q_proj_bindings, &[], q_proj_threadgroups, q_proj_threads_per_threadgroup)?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(&head_norm_pipeline, bytes_of(&q_head_norm_args), &q_head_norm_bindings, &[], q_head_norm_threadgroups, q_head_norm_threads_per_threadgroup)?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(&rope_pipeline, bytes_of(&q_rope_args), &q_rope_bindings, &[], q_rope_threadgroups, q_rope_threads_per_threadgroup)?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(&proj_pipeline, bytes_of(&k_proj_args), &k_proj_bindings, &[], k_proj_threadgroups, k_proj_threads_per_threadgroup)?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(&head_norm_pipeline, bytes_of(&k_head_norm_args), &k_head_norm_bindings, &[], k_head_norm_threadgroups, k_head_norm_threads_per_threadgroup)?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(&rope_pipeline, bytes_of(&k_rope_args), &k_rope_bindings, &[], k_rope_threadgroups, k_rope_threads_per_threadgroup)?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(&proj_pipeline, bytes_of(&v_proj_args), &v_proj_bindings, &[], v_proj_threadgroups, v_proj_threads_per_threadgroup)?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(&head_norm_pipeline, bytes_of(&v_head_norm_args), &v_head_norm_bindings, &[], v_head_norm_threadgroups, v_head_norm_threads_per_threadgroup)?;
        runtime.end_command_batch()?;
        runtime.wait_idle()?;

        let q_bits = decode_bf16_buffer_bits(&runtime.read_buffer(&q_rope_buf, q_out_len * 2)?);
        let k_bits = decode_bf16_buffer_bits(&runtime.read_buffer(&k_rope_buf, k_out_len * 2)?);
        let v_bits = decode_bf16_buffer_bits(&runtime.read_buffer(&v_norm_buf, v_out_len * 2)?);
        Ok((q_bits, k_bits, v_bits))
    };

    let (_, prefill_k_bits, prefill_v_bits) = run_projection(&prefill_x_words, PREFILL_ROPE_OFFSET)?;
    let (decode_q_bits, decode_k_bits, decode_v_bits) =
        run_projection(&decode_x_words, DECODE_ROPE_OFFSET)?;

    let mut kv_cache = GemmaKvCache::<f32>::new(GemmaKvCacheSpec::new(
        GemmaAttentionKind::Full,
        1,
        k_head_count,
        head_dim,
        2,
    )?)?;
    let prefill_k_tensor = flatten_heads_to_tensor(&prefill_k_bits, k_head_count, head_dim)?;
    let prefill_v_tensor = flatten_heads_to_tensor(&prefill_v_bits, v_head_count, head_dim)?;
    let decode_k_tensor = flatten_heads_to_tensor(&decode_k_bits, k_head_count, head_dim)?;
    let decode_v_tensor = flatten_heads_to_tensor(&decode_v_bits, v_head_count, head_dim)?;
    kv_cache.update_and_fetch(prefill_k_tensor.view(), prefill_v_tensor.view())?;
    kv_cache.update_and_fetch(decode_k_tensor.view(), decode_v_tensor.view())?;

    let full_state = kv_cache.fetch()?;
    let full_k_bits = full_state
        .keys
        .to_tensor()?
        .data()
        .iter()
        .copied()
        .map(f32::to_bits)
        .collect::<Vec<_>>();
    let full_v_bits = full_state
        .values
        .to_tensor()?
        .data()
        .iter()
        .copied()
        .map(f32::to_bits)
        .collect::<Vec<_>>();
    let (attention_score_bits, attention_prob_bits, attention_out_bits) =
        compute_cached_attention(&decode_q_bits, &kv_cache, q_head_count, q_heads_per_kv, head_dim)?;
    let attention_oproj_bits = if validate_oproj {
        let attn_out_words = bf16_words_from_f32_bits(&attention_out_bits);
        let attn_out_buf = attn_out_buf.as_ref().ok_or("missing attention output buffer")?;
        let o_proj_out_buf = o_proj_out_buf
            .as_ref()
            .ok_or("missing attention o_proj output buffer")?;
        let o_weight_buf = o_weight_buf.as_ref().ok_or("missing o_proj weight buffer")?;
        let o_scales_buf = o_scales_buf.as_ref().ok_or("missing o_proj scales buffer")?;
        let o_biases_buf = o_biases_buf.as_ref().ok_or("missing o_proj biases buffer")?;
        let o_proj_fast_pipeline = o_proj_fast_pipeline
            .as_ref()
            .ok_or("missing o_proj fast pipeline")?;
        let o_proj_args = o_proj_args.as_ref().ok_or("missing o_proj args")?;
        runtime.write_buffer(attn_out_buf, 0, &bytes_from_bf16_words(&attn_out_words))?;
        let o_proj_bindings = [
            MetalBufferBindingRef {
                index: 1,
                buffer: attn_out_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 2,
                buffer: o_weight_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 3,
                buffer: o_scales_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 4,
                buffer: o_biases_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 5,
                buffer: o_proj_out_buf,
                offset_bytes: 0,
            },
        ];
        runtime.begin_command_batch()?;
        runtime.dispatch_compute(
            o_proj_fast_pipeline,
            bytes_of(o_proj_args),
            &o_proj_bindings,
            &[],
            o_proj_threadgroups,
            o_proj_threads_per_threadgroup,
        )?;
        runtime.end_command_batch()?;
        runtime.wait_idle()?;
        Some(decode_bf16_buffer_bits(
            &runtime.read_buffer(o_proj_out_buf, o_out_len * 2)?,
        ))
    } else {
        None
    };
    let post_attention_stage_bits = if validate_residual {
        let decode_x_bytes = bytes_from_bf16_words(&decode_x_words);
        let x_buf = &x_buf;
        let o_proj_out_buf = o_proj_out_buf
            .as_ref()
            .ok_or("missing attention o_proj output buffer")?;
        let post_attention_norm_weight_buf = post_attention_norm_weight_buf
            .as_ref()
            .ok_or("missing post-attention norm weight buffer")?;
        let post_attention_norm_out_buf = post_attention_norm_out_buf
            .as_ref()
            .ok_or("missing post-attention norm output buffer")?;
        let residual_out_buf = residual_out_buf
            .as_ref()
            .ok_or("missing residual output buffer")?;
        let residual_pipeline = residual_pipeline
            .as_ref()
            .ok_or("missing residual pipeline")?;
        let post_attention_norm_args = post_attention_norm_args
            .as_ref()
            .ok_or("missing post-attention norm args")?;
        let residual_args = residual_args.as_ref().ok_or("missing residual args")?;
        let pre_feedforward_norm_weight_buf = pre_feedforward_norm_weight_buf.as_ref();
        let pre_feedforward_norm_out_buf = pre_feedforward_norm_out_buf.as_ref();
        let pre_feedforward_norm_args = pre_feedforward_norm_args.as_ref();
        let post_attention_norm_bindings = [
            MetalBufferBindingRef {
                index: 1,
                buffer: o_proj_out_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 2,
                buffer: post_attention_norm_weight_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 3,
                buffer: post_attention_norm_out_buf,
                offset_bytes: 0,
            },
        ];
        let residual_bindings = [
            MetalBufferBindingRef {
                index: 1,
                buffer: x_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 2,
                buffer: post_attention_norm_out_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 3,
                buffer: residual_out_buf,
                offset_bytes: 0,
            },
        ];
        let pre_feedforward_norm_bindings = if let (Some(weight_buf), Some(out_buf)) = (
            pre_feedforward_norm_weight_buf,
            pre_feedforward_norm_out_buf,
        ) {
            Some([
                MetalBufferBindingRef {
                    index: 1,
                    buffer: residual_out_buf,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: weight_buf,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: out_buf,
                    offset_bytes: 0,
                },
            ])
        } else {
            None
        };
        runtime.write_buffer(x_buf, 0, &decode_x_bytes)?;
        runtime.begin_command_batch()?;
        runtime.dispatch_compute(
            &rms_pipeline,
            bytes_of(post_attention_norm_args),
            &post_attention_norm_bindings,
            &[],
            rms_threadgroups,
            rms_threads_per_threadgroup,
        )?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            residual_pipeline,
            bytes_of(residual_args),
            &residual_bindings,
            &[],
            residual_threadgroups,
            residual_threads_per_threadgroup,
        )?;
        if let (Some(args), Some(bindings)) = (pre_feedforward_norm_args, &pre_feedforward_norm_bindings) {
            runtime.memory_barrier_buffers()?;
            runtime.dispatch_compute(
                &rms_pipeline,
                bytes_of(args),
                bindings,
                &[],
                pre_feedforward_norm_threadgroups,
                rms_threads_per_threadgroup,
            )?;
        }
        runtime.end_command_batch()?;
        runtime.wait_idle()?;
        let post_attention_norm_bits = decode_bf16_buffer_bits(
            &runtime.read_buffer(post_attention_norm_out_buf, post_attention_norm_len * 2)?,
        );
        let residual_bits = decode_bf16_buffer_bits(
            &runtime.read_buffer(residual_out_buf, post_attention_norm_len * 2)?,
        );
        let pre_feedforward_norm_bits = if let Some(out_buf) = pre_feedforward_norm_out_buf {
            Some(decode_bf16_buffer_bits(
                &runtime.read_buffer(out_buf, pre_feedforward_norm_len * 2)?,
            ))
        } else {
            None
        };
        Some((post_attention_norm_bits, residual_bits, pre_feedforward_norm_bits))
    } else {
        None
    };
    let dense_gate_bits = if validate_dense_gate {
        let mlp_gate_args = mlp_gate_args
            .as_ref()
            .ok_or("missing mlp gate args")?;
        let mlp_gate_bindings = mlp_gate_bindings
            .as_ref()
            .ok_or("missing mlp gate bindings")?;
        let mlp_gate_out_buf = mlp_gate_out_buf
            .as_ref()
            .ok_or("missing mlp gate output buffer")?;
        runtime.begin_command_batch()?;
        runtime.dispatch_compute(
            &proj_pipeline,
            bytes_of(mlp_gate_args),
            mlp_gate_bindings,
            &[],
            mlp_gate_threadgroups,
            mlp_gate_threads_per_threadgroup,
        )?;
        runtime.end_command_batch()?;
        runtime.wait_idle()?;
        Some(decode_bf16_buffer_bits(
            &runtime.read_buffer(mlp_gate_out_buf, mlp_gate_out_len * 2)?,
        ))
    } else {
        None
    };
    let dense_up_bits = if validate_dense_up {
        let mlp_up_args = mlp_up_args.as_ref().ok_or("missing mlp up args")?;
        let mlp_up_bindings = mlp_up_bindings
            .as_ref()
            .ok_or("missing mlp up bindings")?;
        let mlp_up_out_buf = mlp_up_out_buf
            .as_ref()
            .ok_or("missing mlp up output buffer")?;
        runtime.begin_command_batch()?;
        runtime.dispatch_compute(
            &proj_pipeline,
            bytes_of(mlp_up_args),
            mlp_up_bindings,
            &[],
            mlp_up_threadgroups,
            mlp_up_threads_per_threadgroup,
        )?;
        runtime.end_command_batch()?;
        runtime.wait_idle()?;
        Some(decode_bf16_buffer_bits(
            &runtime.read_buffer(mlp_up_out_buf, mlp_up_out_len * 2)?,
        ))
    } else {
        None
    };
    let dense_geglu_bits = if validate_dense_geglu {
        let geglu_pipeline = geglu_pipeline
            .as_ref()
            .ok_or("missing geglu pipeline")?;
        let geglu_args = geglu_args.as_ref().ok_or("missing geglu args")?;
        let geglu_bindings = geglu_bindings
            .as_ref()
            .ok_or("missing geglu bindings")?;
        let geglu_out_buf = geglu_out_buf
            .as_ref()
            .ok_or("missing geglu output buffer")?;
        runtime.begin_command_batch()?;
        runtime.dispatch_compute(
            geglu_pipeline,
            bytes_of(geglu_args),
            geglu_bindings,
            &[],
            geglu_threadgroups,
            geglu_threads_per_threadgroup,
        )?;
        runtime.end_command_batch()?;
        runtime.wait_idle()?;
        Some(decode_bf16_buffer_bits(
            &runtime.read_buffer(geglu_out_buf, mlp_gate_out_len * 2)?,
        ))
    } else {
        None
    };
    let dense_down_bits = if validate_dense_down {
        let mlp_down_args = mlp_down_args.as_ref().ok_or("missing mlp down args")?;
        let mlp_down_bindings = mlp_down_bindings
            .as_ref()
            .ok_or("missing mlp down bindings")?;
        let mlp_down_out_buf = mlp_down_out_buf
            .as_ref()
            .ok_or("missing mlp down output buffer")?;
        runtime.begin_command_batch()?;
        runtime.dispatch_compute(
            &proj_pipeline,
            bytes_of(mlp_down_args),
            mlp_down_bindings,
            &[],
            mlp_down_threadgroups,
            mlp_down_threads_per_threadgroup,
        )?;
        runtime.end_command_batch()?;
        runtime.wait_idle()?;
        Some(decode_bf16_buffer_bits(
            &runtime.read_buffer(mlp_down_out_buf, mlp_down_out_len * 2)?,
        ))
    } else {
        None
    };
    let router_output = if validate_router {
        let router_scale_pipeline = router_scale_pipeline
            .as_ref()
            .ok_or("missing router scale pipeline")?;
        let router_scale_args = router_scale_args
            .as_ref()
            .ok_or("missing router scale args")?;
        let router_scale_bindings = router_scale_bindings
            .as_ref()
            .ok_or("missing router scale bindings")?;
        let router_proj_args = router_proj_args
            .as_ref()
            .ok_or("missing router proj args")?;
        let router_proj_bindings = router_proj_bindings
            .as_ref()
            .ok_or("missing router proj bindings")?;
        let router_scaled_out_buf = router_scaled_out_buf
            .as_ref()
            .ok_or("missing router scaled output buffer")?;
        let router_proj_out_buf = router_proj_out_buf
            .as_ref()
            .ok_or("missing router proj output buffer")?;
        let per_expert_scale_words = router_per_expert_scale_words
            .as_ref()
            .ok_or("missing router per-expert scales")?;
        runtime.begin_command_batch()?;
        runtime.dispatch_compute(
            router_scale_pipeline,
            bytes_of(router_scale_args),
            router_scale_bindings,
            &[],
            router_scale_threadgroups,
            router_scale_threads_per_threadgroup,
        )?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            &proj_pipeline,
            bytes_of(router_proj_args),
            router_proj_bindings,
            &[],
            router_proj_threadgroups,
            router_proj_threads_per_threadgroup,
        )?;
        runtime.end_command_batch()?;
        runtime.wait_idle()?;
        let router_scaled_bits = decode_bf16_buffer_bits(
            &runtime.read_buffer(router_scaled_out_buf, post_attention_norm_len * 2)?,
        );
        let expert_scores_bits = decode_bf16_buffer_bits(
            &runtime.read_buffer(router_proj_out_buf, router_out_len * 2)?,
        );
        Some(compute_router_output_from_expert_scores(
            router_scaled_bits,
            expert_scores_bits,
            per_expert_scale_words,
            ROUTER_TOP_K,
        )?)
    } else {
        None
    };

    validate_hash_and_prefix(
        "prefill_k_cache",
        &prefill_k_bits,
        EXPECTED_PREFILL_K_CACHE_HASH,
        &EXPECTED_PREFILL_K_CACHE_FIRST16_BITS,
    )?;
    validate_hash_and_prefix(
        "prefill_v_cache",
        &prefill_v_bits,
        EXPECTED_PREFILL_V_CACHE_HASH,
        &EXPECTED_PREFILL_V_CACHE_FIRST16_BITS,
    )?;
    validate_hash_and_prefix(
        "decode_q_rope",
        &decode_q_bits,
        EXPECTED_DECODE_Q_ROPE_HASH,
        &EXPECTED_DECODE_Q_ROPE_FIRST16_BITS,
    )?;
    validate_hash_and_prefix(
        "decode_k_rope",
        &decode_k_bits,
        EXPECTED_DECODE_K_ROPE_HASH,
        &EXPECTED_DECODE_K_ROPE_FIRST16_BITS,
    )?;
    validate_hash_and_prefix(
        "decode_v_norm",
        &decode_v_bits,
        EXPECTED_DECODE_V_NORM_HASH,
        &EXPECTED_DECODE_V_NORM_FIRST16_BITS,
    )?;
    validate_hash_and_prefix(
        "full_k_cache",
        &full_k_bits,
        EXPECTED_FULL_K_CACHE_HASH,
        &EXPECTED_FULL_K_CACHE_FIRST16_BITS,
    )?;
    validate_hash_and_prefix(
        "full_v_cache",
        &full_v_bits,
        EXPECTED_FULL_V_CACHE_HASH,
        &EXPECTED_FULL_V_CACHE_FIRST16_BITS,
    )?;
    validate_hash_and_prefix(
        "attention_scores",
        &attention_score_bits,
        EXPECTED_ATTENTION_SCORES_HASH,
        &EXPECTED_ATTENTION_SCORES_FIRST16_BITS,
    )?;
    validate_hash_and_prefix(
        "attention_probs",
        &attention_prob_bits,
        EXPECTED_ATTENTION_PROBS_HASH,
        &EXPECTED_ATTENTION_PROBS_FIRST16_BITS,
    )?;
    validate_hash_and_prefix(
        "attention_output",
        &attention_out_bits,
        EXPECTED_ATTENTION_OUTPUT_HASH,
        &EXPECTED_ATTENTION_OUTPUT_FIRST16_BITS,
    )?;
    if let Some(attention_oproj_bits) = &attention_oproj_bits {
        validate_hash_and_prefix(
            "attention_oproj",
            attention_oproj_bits,
            EXPECTED_ATTENTION_OPROJ_HASH,
            &EXPECTED_ATTENTION_OPROJ_FIRST16_BITS,
        )?;
    }
    if let Some((post_attention_norm_bits, residual_bits, pre_feedforward_norm_bits)) =
        &post_attention_stage_bits
    {
        validate_hash_and_prefix(
            "attention_post_attn_norm",
            post_attention_norm_bits,
            EXPECTED_POST_ATTENTION_NORM_HASH,
            &EXPECTED_POST_ATTENTION_NORM_FIRST16_BITS,
        )?;
        validate_hash_and_prefix(
            "attention_post_attn_residual",
            residual_bits,
            EXPECTED_POST_ATTENTION_RESIDUAL_HASH,
            &EXPECTED_POST_ATTENTION_RESIDUAL_FIRST16_BITS,
        )?;
        if let Some(pre_feedforward_norm_bits) = pre_feedforward_norm_bits {
            validate_hash_and_prefix(
                "attention_pre_ffn_norm",
                pre_feedforward_norm_bits,
                EXPECTED_PRE_FEEDFORWARD_NORM_HASH,
                &EXPECTED_PRE_FEEDFORWARD_NORM_FIRST16_BITS,
            )?;
        }
    }
    if let Some(dense_gate_bits) = &dense_gate_bits {
        validate_hash_and_prefix(
            "attention_pre_ffn_gate",
            dense_gate_bits,
            EXPECTED_PRE_FEEDFORWARD_GATE_HASH,
            &EXPECTED_PRE_FEEDFORWARD_GATE_FIRST16_BITS,
        )?;
    }
    if let Some(dense_up_bits) = &dense_up_bits {
        validate_hash_and_prefix(
            "attention_pre_ffn_up",
            dense_up_bits,
            EXPECTED_PRE_FEEDFORWARD_UP_HASH,
            &EXPECTED_PRE_FEEDFORWARD_UP_FIRST16_BITS,
        )?;
    }
    if let Some(dense_geglu_bits) = &dense_geglu_bits {
        validate_hash_and_prefix(
            "attention_pre_ffn_geglu",
            dense_geglu_bits,
            EXPECTED_PRE_FEEDFORWARD_GEGLU_HASH,
            &EXPECTED_PRE_FEEDFORWARD_GEGLU_FIRST16_BITS,
        )?;
    }
    if let Some(dense_down_bits) = &dense_down_bits {
        validate_hash_and_prefix(
            "attention_pre_ffn_down",
            dense_down_bits,
            EXPECTED_PRE_FEEDFORWARD_DOWN_HASH,
            &EXPECTED_PRE_FEEDFORWARD_DOWN_FIRST16_BITS,
        )?;
    }
    if let Some(router_output) = &router_output {
        validate_hash_and_prefix(
            "router_scaled",
            &router_output.router_scaled_bits,
            EXPECTED_ROUTER_SCALED_HASH,
            &EXPECTED_ROUTER_SCALED_FIRST16_BITS,
        )?;
        validate_hash_and_prefix(
            "router_expert_scores",
            &router_output.expert_scores_bits,
            EXPECTED_ROUTER_EXPERT_SCORES_HASH,
            &EXPECTED_ROUTER_EXPERT_SCORES_FIRST16_BITS,
        )?;
        validate_hash_and_prefix(
            "router_probs",
            &router_output.router_probs_bits,
            EXPECTED_ROUTER_PROBS_HASH,
            &EXPECTED_ROUTER_PROBS_FIRST16_BITS,
        )?;
        validate_hash_and_prefix(
            "router_topk_indices",
            &router_output.top_k_indices,
            EXPECTED_ROUTER_TOPK_INDICES_HASH,
            &EXPECTED_ROUTER_TOPK_INDICES,
        )?;
        validate_hash_and_prefix(
            "router_topk_weights",
            &router_output.top_k_weights_bits,
            EXPECTED_ROUTER_TOPK_WEIGHTS_HASH,
            &EXPECTED_ROUTER_TOPK_WEIGHTS_FIRST8_BITS,
        )?;
    }

    println!("backend={}", runtime.backend_info().name);
    println!("model_path={}", model_path.display());
    println!("prefill_rope_offset={PREFILL_ROPE_OFFSET}");
    println!("decode_rope_offset={DECODE_ROPE_OFFSET}");
    println!("prefill_activation_phase={PREFILL_ACTIVATION_PHASE}");
    println!("decode_activation_phase={DECODE_ACTIVATION_PHASE}");
    println!("q_head_count={q_head_count}");
    println!("k_head_count={k_head_count}");
    println!("v_head_count={v_head_count}");
    println!("q_heads_per_kv={q_heads_per_kv}");
    println!("head_dim={head_dim}");
    if validate_dense_down {
        println!("stage=attention_pre_ffn_down_cached");
    } else if validate_dense_geglu {
        println!("stage=attention_pre_ffn_geglu_cached");
    } else if validate_dense_up {
        println!("stage=attention_pre_ffn_up_cached");
    } else if validate_dense_gate {
        println!("stage=attention_pre_ffn_gate_cached");
    } else if validate_router {
        println!("stage=attention_router_cached");
    } else if validate_pre_ffn_norm {
        println!("stage=attention_pre_ffn_norm_cached");
    } else if validate_residual {
        println!("stage=attention_post_attn_residual_cached");
    } else if validate_oproj {
        println!("stage=attention_oproj_cached");
    } else {
        println!("stage=qkv_attention_output_cached");
    }
    println!("prefill_k_cache_fnv1a64=0x{:016X}", fnv1a64_u32_words(&prefill_k_bits));
    println!("prefill_v_cache_fnv1a64=0x{:016X}", fnv1a64_u32_words(&prefill_v_bits));
    println!("decode_q_rope_fnv1a64=0x{:016X}", fnv1a64_u32_words(&decode_q_bits));
    println!("decode_k_rope_fnv1a64=0x{:016X}", fnv1a64_u32_words(&decode_k_bits));
    println!("decode_v_norm_fnv1a64=0x{:016X}", fnv1a64_u32_words(&decode_v_bits));
    println!("full_k_cache_fnv1a64=0x{:016X}", fnv1a64_u32_words(&full_k_bits));
    println!("full_v_cache_fnv1a64=0x{:016X}", fnv1a64_u32_words(&full_v_bits));
    println!("attention_scores_fnv1a64=0x{:016X}", fnv1a64_u32_words(&attention_score_bits));
    println!("attention_probs_fnv1a64=0x{:016X}", fnv1a64_u32_words(&attention_prob_bits));
    println!("attention_output_fnv1a64=0x{:016X}", fnv1a64_u32_words(&attention_out_bits));
    if let Some(attention_oproj_bits) = &attention_oproj_bits {
        println!(
            "attention_oproj_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(attention_oproj_bits)
        );
    }
    if let Some((post_attention_norm_bits, residual_bits, pre_feedforward_norm_bits)) =
        &post_attention_stage_bits
    {
        println!(
            "attention_post_attn_norm_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(post_attention_norm_bits)
        );
        println!(
            "attention_post_attn_residual_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(residual_bits)
        );
        if let Some(pre_feedforward_norm_bits) = pre_feedforward_norm_bits {
            println!(
                "attention_pre_ffn_norm_fnv1a64=0x{:016X}",
                fnv1a64_u32_words(pre_feedforward_norm_bits)
            );
        }
    }
    if let Some(dense_gate_bits) = &dense_gate_bits {
        println!(
            "attention_pre_ffn_gate_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(dense_gate_bits)
        );
    }
    if let Some(dense_up_bits) = &dense_up_bits {
        println!(
            "attention_pre_ffn_up_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(dense_up_bits)
        );
    }
    if let Some(dense_geglu_bits) = &dense_geglu_bits {
        println!(
            "attention_pre_ffn_geglu_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(dense_geglu_bits)
        );
    }
    if let Some(dense_down_bits) = &dense_down_bits {
        println!(
            "attention_pre_ffn_down_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(dense_down_bits)
        );
    }
    if let Some(router_output) = &router_output {
        println!(
            "router_scaled_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(&router_output.router_scaled_bits)
        );
        println!(
            "expert_scores_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(&router_output.expert_scores_bits)
        );
        println!(
            "router_probs_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(&router_output.router_probs_bits)
        );
        println!(
            "router_topk_indices_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(&router_output.top_k_indices)
        );
        println!(
            "router_topk_weights_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(&router_output.top_k_weights_bits)
        );
    }
    print_first16("prefill_k_cache_first16_f32_bits", &prefill_k_bits);
    print_first16("prefill_v_cache_first16_f32_bits", &prefill_v_bits);
    print_first16("decode_q_rope_first16_f32_bits", &decode_q_bits);
    print_first16("decode_k_rope_first16_f32_bits", &decode_k_bits);
    print_first16("decode_v_norm_first16_f32_bits", &decode_v_bits);
    print_first16("full_k_cache_first16_f32_bits", &full_k_bits);
    print_first16("full_v_cache_first16_f32_bits", &full_v_bits);
    print_first16("attention_scores_first16_f32_bits", &attention_score_bits);
    print_first16("attention_probs_first16_f32_bits", &attention_prob_bits);
    print_first16("attention_output_first16_f32_bits", &attention_out_bits);
    if let Some(attention_oproj_bits) = &attention_oproj_bits {
        print_first16("attention_oproj_first16_f32_bits", attention_oproj_bits);
    }
    if let Some((post_attention_norm_bits, residual_bits, pre_feedforward_norm_bits)) =
        &post_attention_stage_bits
    {
        print_first16(
            "attention_post_attn_norm_first16_f32_bits",
            post_attention_norm_bits,
        );
        print_first16(
            "attention_post_attn_residual_first16_f32_bits",
            residual_bits,
        );
        if let Some(pre_feedforward_norm_bits) = pre_feedforward_norm_bits {
            print_first16(
                "attention_pre_ffn_norm_first16_f32_bits",
                pre_feedforward_norm_bits,
            );
        }
    }
    if let Some(dense_gate_bits) = &dense_gate_bits {
        print_first16("attention_pre_ffn_gate_first16_f32_bits", dense_gate_bits);
    }
    if let Some(dense_up_bits) = &dense_up_bits {
        print_first16("attention_pre_ffn_up_first16_f32_bits", dense_up_bits);
    }
    if let Some(dense_geglu_bits) = &dense_geglu_bits {
        print_first16("attention_pre_ffn_geglu_first16_f32_bits", dense_geglu_bits);
    }
    if let Some(dense_down_bits) = &dense_down_bits {
        print_first16("attention_pre_ffn_down_first16_f32_bits", dense_down_bits);
    }
    if let Some(router_output) = &router_output {
        print_first16("router_scaled_first16_f32_bits", &router_output.router_scaled_bits);
        print_first16("expert_scores_first16_f32_bits", &router_output.expert_scores_bits);
        print_first16("router_probs_first16_f32_bits", &router_output.router_probs_bits);
        print!("top_k_indices=");
        for (index, value) in router_output.top_k_indices.iter().enumerate() {
            if index != 0 {
                print!(",");
            }
            print!("{value}");
        }
        println!();
        print_prefix(
            "top_k_weights_first8_f32_bits",
            &router_output.top_k_weights_bits,
            ROUTER_TOP_K,
        );
    }
    println!("status=ok");
    Ok(())
}
