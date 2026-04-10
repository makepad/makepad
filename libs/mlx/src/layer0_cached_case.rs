#![cfg_attr(not(test), allow(dead_code))]

use crate::{GemmaAttentionKind, GemmaKvCacheLayout, GemmaKvCacheSpec, KvTensor, KvTensorShape};
use makepad_ggml::backend::metal::{
    BufferStorageMode, MetalBuffer, MetalBufferBindingRef, MetalPipeline, MetalPipelineDescriptor,
    MetalRuntime, MetalRuntimeCounters, MetalSize,
};
use crate::{
    fnv1a64_u32_words, gemma4_qproj_case_input_bf16_words_with_phase, MlxDType,
    MlxGreedyToken, MlxIndexedSafetensors,
};
use crate::text_runtime::{
    sample_token_from_softcapped_bf16_bytes, GemmaTextSamplingOptions, MlxTextSamplingRng,
};
use std::cell::{RefCell, RefMut};
use std::collections::{BTreeSet, HashMap};
use std::env;
use std::error::Error;
use std::fs;
use std::mem::size_of;
use std::path::{Path, PathBuf};
use std::slice;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const NORM_LEN: usize = 2_816;
const EPS: f32 = 1e-6;
const ROPE_SCALE: f32 = 1.0;
const PREFILL_ROPE_OFFSET: i32 = 17;
const DECODE_ROPE_OFFSET: i32 = 18;
const PREFILL_ACTIVATION_PHASE: usize = 0;
const DECODE_ACTIVATION_PHASE: usize = 5;
const ROUTER_TOP_K: usize = 8;
const DEVICE_GREEDY_DECODE_CHUNK_TOKENS: usize = 8;
const EMBED_TOKENS_WEIGHT_NAME: &str = "language_model.model.embed_tokens.weight";
const EMBED_TOKENS_SCALES_NAME: &str = "language_model.model.embed_tokens.scales";
const EMBED_TOKENS_BIASES_NAME: &str = "language_model.model.embed_tokens.biases";
const FINAL_TEXT_NORM_WEIGHT_NAME: &str = "language_model.model.norm.weight";

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
const EXPECTED_MOE_EXPERT_GATE_HASH: u64 = 0x5890_3AC5_7D0A_353B;
const EXPECTED_MOE_EXPERT_UP_HASH: u64 = 0x0834_48ED_211B_5962;
const EXPECTED_MOE_EXPERT_GEGLU_HASH: u64 = 0x0EDF_808A_0376_BC68;
const EXPECTED_MOE_EXPERT_DOWN_HASH: u64 = 0xCF0A_F77B_4E0A_D20A;
const EXPECTED_POST_FFN_NORM1_HASH: u64 = 0x45A3_0135_90F6_A92B;
const EXPECTED_MOE_EXPERT_OUT_HASH: u64 = 0xB2AE_987F_8D52_83B7;
const EXPECTED_MOE_POST_FFN_NORM2_HASH: u64 = 0x0F8D_5C98_366F_571F;
const EXPECTED_MOE_MERGE_HASH: u64 = 0xFB0B_FD1E_3F3F_BACE;
const EXPECTED_POST_FFN_RESIDUAL_HASH: u64 = 0xC176_97C1_02C9_B81D;

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
const EXPECTED_MOE_EXPERT_GATE_FIRST16_BITS: [u32; 16] = [
    0x3F85_0000,
    0xBF82_0000,
    0xBEE1_0000,
    0x3CE5_0000,
    0xBDA9_0000,
    0x3F07_0000,
    0x3E5F_0000,
    0xBE8F_0000,
    0xBF15_0000,
    0xBE9F_0000,
    0xBE97_0000,
    0xBD49_0000,
    0x3DC0_0000,
    0xBF3F_0000,
    0x3F08_0000,
    0x3F0E_0000,
];
const EXPECTED_MOE_EXPERT_UP_FIRST16_BITS: [u32; 16] = [
    0x3DD7_0000,
    0xBED7_0000,
    0xBF58_0000,
    0xBF05_0000,
    0xBF1A_0000,
    0xBF12_0000,
    0xBDD0_0000,
    0x3EB2_0000,
    0x3D58_0000,
    0x3E9A_0000,
    0x3EE3_0000,
    0x3E3B_0000,
    0xBE71_0000,
    0x3F25_0000,
    0xBEBD_0000,
    0xBEB7_0000,
];
const EXPECTED_MOE_EXPERT_GEGLU_FIRST16_BITS: [u32; 16] = [
    0x3DBF_0000,
    0x3D86_0000,
    0x3DFB_0000,
    0xBBF3_0000,
    0x3CBE_0000,
    0xBE59_0000,
    0xBC55_0000,
    0xBD1B_0000,
    0xBC0E_0000,
    0xBD11_0000,
    0xBD4D_0000,
    0xBB8D_0000,
    0xBC43_0000,
    0xBDE2_0000,
    0xBE0D_0000,
    0xBE10_0000,
];
const EXPECTED_MOE_EXPERT_DOWN_FIRST16_BITS: [u32; 16] = [
    0x3DCA_0000,
    0xBD0D_0000,
    0x3DE7_0000,
    0x3E2A_0000,
    0xBE22_0000,
    0xBD09_0000,
    0x3DB3_0000,
    0xBD65_0000,
    0x3BD4_0000,
    0x3CA4_0000,
    0xBECD_0000,
    0x3ED3_0000,
    0xBD91_0000,
    0x3E42_0000,
    0x3D05_0000,
    0x3CC2_0000,
];
const EXPECTED_POST_FFN_NORM1_FIRST16_BITS: [u32; 16] = [
    0xBA92_0000,
    0x3DA7_0000,
    0xBE2E_0000,
    0xBF98_0000,
    0xBDE4_0000,
    0x3E83_0000,
    0x3DC8_0000,
    0xBD96_0000,
    0x40B3_0000,
    0xBFC4_0000,
    0xC00C_0000,
    0x4031_0000,
    0x4029_0000,
    0x40CE_0000,
    0x4065_0000,
    0x3C8A_0000,
];
const EXPECTED_MOE_EXPERT_OUT_FIRST16_BITS: [u32; 16] = [
    0x3DA4_0000,
    0x3DC0_0000,
    0xBD38_0000,
    0x3D04_0000,
    0xBDBA_0000,
    0xBD4B_0000,
    0xBC2F_0000,
    0x3D34_0000,
    0x3DB4_0000,
    0xBB8E_0000,
    0xBE39_0000,
    0x3D97_0000,
    0xBDA0_0000,
    0x3D85_0000,
    0x3D02_0000,
    0x3C01_0000,
];
const EXPECTED_MOE_POST_FFN_NORM2_FIRST16_BITS: [u32; 16] = [
    0x4042_0000,
    0x408B_0000,
    0xBFA7_0000,
    0x3FFC_0000,
    0xC06F_0000,
    0xBF99_0000,
    0xBEC8_0000,
    0x400B_0000,
    0x40F3_0000,
    0xBE42_0000,
    0xC133_0000,
    0x40E3_0000,
    0xC08F_0000,
    0x40F2_0000,
    0x3FD4_0000,
    0x3EC5_0000,
];
const EXPECTED_MOE_MERGE_FIRST16_BITS: [u32; 16] = [
    0x4042_0000,
    0x408E_0000,
    0xBFBD_0000,
    0x3F48_0000,
    0xC076_0000,
    0xBF70_0000,
    0xBE96_0000,
    0x4006_0000,
    0x4153_0000,
    0xBFDC_0000,
    0xC156_0000,
    0x411E_0000,
    0xBFEA_0000,
    0x4160_0000,
    0x40A8_0000,
    0x3ECE_0000,
];
const EXPECTED_POST_FFN_RESIDUAL_FIRST16_BITS: [u32; 16] = [
    0x403F_0000,
    0x40A0_0000,
    0xBF40_0000,
    0x3F77_0000,
    0xC054_0000,
    0xBF28_0000,
    0xBF5D_0000,
    0x3F9A_0000,
    0x40F4_0000,
    0xBFAC_0000,
    0xC14E_0000,
    0x4111_0000,
    0xC026_0000,
    0x414B_0000,
    0x409E_0000,
    0x3EAB_0000,
];

#[derive(Clone, Copy)]
#[repr(C)]
struct MlxAffineDequantRowArgs {
    n: u32,
    embed_scale: f32,
}

#[derive(Clone, Copy)]
#[repr(C)]
struct MlxAffineDequantTokenRowArgs {
    n: u32,
    embed_scale: f32,
    weight_words_per_row: u32,
    qparams_per_row: u32,
    vocab_size: u32,
    history_slot: u32,
}

#[derive(Clone, Copy)]
#[repr(C)]
struct MlxRmsNormRowArgs {
    n: u32,
    eps: f32,
}

#[derive(Clone, Copy)]
#[repr(C)]
struct MlxRmsNormRowsArgs {
    n: u32,
    row_stride: u32,
    row_count: u32,
    eps: f32,
}

#[derive(Clone, Copy)]
#[repr(C)]
struct MlxAffineQprojRowArgs {
    n_in: u32,
    weight_words_per_row: u32,
    qparams_per_row: u32,
    out_rows: u32,
}

#[derive(Clone, Copy)]
#[repr(C)]
struct MlxAffineSelectedExpertsQprojRowArgs {
    n_in: u32,
    weight_words_per_row: u32,
    qparams_per_row: u32,
    out_rows: u32,
    input_row_stride: u32,
}

#[derive(Clone, Copy)]
#[repr(C)]
struct MlxAddRowArgs {
    n: u32,
}

#[derive(Clone, Copy)]
#[repr(C)]
struct MlxScaleRowArgs {
    n: u32,
    scale: f32,
}

#[derive(Clone, Copy)]
#[repr(C)]
struct MlxWeightedRowsArgs {
    n: u32,
    row_stride: u32,
    row_count: u32,
}

#[derive(Clone, Copy)]
#[repr(C)]
struct MlxGegluRowArgs {
    n: u32,
}

#[derive(Clone, Copy)]
#[repr(C)]
struct MlxGegluStridedRowsArgs {
    n: u32,
    row_width: u32,
    input_row_stride: u32,
    input_split_offset: u32,
}

#[derive(Clone, Copy)]
#[repr(C)]
struct MlxRouterScaleArgs {
    n: u32,
    eps: f32,
    root_size: f32,
}

#[derive(Clone, Copy)]
#[repr(C)]
struct MlxGqaAttentionLogitsSeqArgs {
    head_dim: u32,
    q_head_stride: u32,
    kv_row_stride: u32,
    q_head_count: u32,
    q_heads_per_kv: u32,
    seq_len: u32,
    start_slot: u32,
    capacity: u32,
}

#[derive(Clone, Copy)]
#[repr(C)]
struct MlxGqaAttentionOutputArgs {
    logits_row_stride: u32,
    head_dim: u32,
    kv_row_stride: u32,
    out_head_stride: u32,
    q_head_count: u32,
    q_heads_per_kv: u32,
    seq_len: u32,
    start_slot: u32,
    capacity: u32,
}

#[derive(Clone, Copy)]
#[repr(C)]
struct MlxSoftmaxRowsArgs {
    row_stride: u32,
    row_count: u32,
    seq_len: u32,
}

#[derive(Clone, Copy)]
#[repr(C)]
struct MlxGqaAttentionWeightedSumArgs {
    probs_row_stride: u32,
    head_dim: u32,
    kv_row_stride: u32,
    out_head_stride: u32,
    q_head_count: u32,
    q_heads_per_kv: u32,
    seq_len: u32,
    start_slot: u32,
    capacity: u32,
}

#[derive(Clone, Copy)]
#[repr(C)]
struct MlxRouterTopKArgs {
    expert_count: u32,
    top_k: u32,
}

struct MlxRopeSingleArgs {
    half_dims: u32,
    row_stride: u32,
    row_count: u32,
    offset: i32,
    scale: f32,
    base_log2: f32,
}

#[derive(Clone, Copy)]
#[repr(C)]
struct MlxArgmaxSoftcappedBf16Args {
    n: u32,
    softcap: f32,
    has_softcap: u32,
}

#[derive(Clone, Copy)]
#[repr(C)]
struct MlxKvAppendBf16Args {
    head_dim: u32,
    src_row_stride: u32,
    dst_row_stride: u32,
    head_count: u32,
    slot: u32,
}

#[derive(Clone, Debug)]
struct ProjectionTensorNames {
    weight_name: String,
    scales_name: String,
    biases_name: String,
    norm_weight_name: Option<String>,
}

impl ProjectionTensorNames {
    fn new(base: &str, prefix: &str, norm_weight_name: Option<String>) -> Self {
        Self {
            weight_name: format!("{base}.self_attn.{prefix}.weight"),
            scales_name: format!("{base}.self_attn.{prefix}.scales"),
            biases_name: format!("{base}.self_attn.{prefix}.biases"),
            norm_weight_name,
        }
    }
}

#[derive(Clone, Debug)]
struct LayerTensorNames {
    input_norm_weight_name: String,
    q: ProjectionTensorNames,
    k: ProjectionTensorNames,
    v: ProjectionTensorNames,
    o: ProjectionTensorNames,
    post_attention_norm_weight_name: String,
    pre_feedforward_norm_weight_name: String,
    pre_feedforward_norm2_weight_name: String,
    post_feedforward_norm_weight_name: String,
    post_feedforward_norm1_weight_name: String,
    post_feedforward_norm2_weight_name: String,
    mlp_gate_weight_name: String,
    mlp_gate_scales_name: String,
    mlp_gate_biases_name: String,
    mlp_up_weight_name: String,
    mlp_up_scales_name: String,
    mlp_up_biases_name: String,
    mlp_down_weight_name: String,
    mlp_down_scales_name: String,
    mlp_down_biases_name: String,
    router_scale_name: String,
    router_per_expert_scale_name: String,
    router_proj_weight_name: String,
    router_proj_scales_name: String,
    router_proj_biases_name: String,
    expert_gate_weight_name: String,
    expert_gate_scales_name: String,
    expert_gate_biases_name: String,
    expert_up_weight_name: String,
    expert_up_scales_name: String,
    expert_up_biases_name: String,
    expert_down_weight_name: String,
    expert_down_scales_name: String,
    expert_down_biases_name: String,
    layer_scalar_name: String,
}

impl LayerTensorNames {
    fn for_layer(layer_idx: usize, attention_k_eq_v: bool) -> Self {
        let base = format!("language_model.model.layers.{layer_idx}");
        let q = ProjectionTensorNames::new(
            &base,
            "q_proj",
            Some(format!("{base}.self_attn.q_norm.weight")),
        );
        let k = ProjectionTensorNames::new(
            &base,
            "k_proj",
            Some(format!("{base}.self_attn.k_norm.weight")),
        );
        let v = if attention_k_eq_v {
            ProjectionTensorNames {
                weight_name: k.weight_name.clone(),
                scales_name: k.scales_name.clone(),
                biases_name: k.biases_name.clone(),
                norm_weight_name: None,
            }
        } else {
            ProjectionTensorNames::new(&base, "v_proj", None)
        };
        let o = ProjectionTensorNames::new(&base, "o_proj", None);
        Self {
            input_norm_weight_name: format!("{base}.input_layernorm.weight"),
            q,
            k,
            v,
            o,
            post_attention_norm_weight_name: format!("{base}.post_attention_layernorm.weight"),
            pre_feedforward_norm_weight_name: format!("{base}.pre_feedforward_layernorm.weight"),
            pre_feedforward_norm2_weight_name: format!("{base}.pre_feedforward_layernorm_2.weight"),
            post_feedforward_norm_weight_name: format!("{base}.post_feedforward_layernorm.weight"),
            post_feedforward_norm1_weight_name: format!(
                "{base}.post_feedforward_layernorm_1.weight"
            ),
            post_feedforward_norm2_weight_name: format!(
                "{base}.post_feedforward_layernorm_2.weight"
            ),
            mlp_gate_weight_name: format!("{base}.mlp.gate_proj.weight"),
            mlp_gate_scales_name: format!("{base}.mlp.gate_proj.scales"),
            mlp_gate_biases_name: format!("{base}.mlp.gate_proj.biases"),
            mlp_up_weight_name: format!("{base}.mlp.up_proj.weight"),
            mlp_up_scales_name: format!("{base}.mlp.up_proj.scales"),
            mlp_up_biases_name: format!("{base}.mlp.up_proj.biases"),
            mlp_down_weight_name: format!("{base}.mlp.down_proj.weight"),
            mlp_down_scales_name: format!("{base}.mlp.down_proj.scales"),
            mlp_down_biases_name: format!("{base}.mlp.down_proj.biases"),
            router_scale_name: format!("{base}.router.scale"),
            router_per_expert_scale_name: format!("{base}.router.per_expert_scale"),
            router_proj_weight_name: format!("{base}.router.proj.weight"),
            router_proj_scales_name: format!("{base}.router.proj.scales"),
            router_proj_biases_name: format!("{base}.router.proj.biases"),
            expert_gate_weight_name: format!("{base}.experts.switch_glu.gate_proj.weight"),
            expert_gate_scales_name: format!("{base}.experts.switch_glu.gate_proj.scales"),
            expert_gate_biases_name: format!("{base}.experts.switch_glu.gate_proj.biases"),
            expert_up_weight_name: format!("{base}.experts.switch_glu.up_proj.weight"),
            expert_up_scales_name: format!("{base}.experts.switch_glu.up_proj.scales"),
            expert_up_biases_name: format!("{base}.experts.switch_glu.up_proj.biases"),
            expert_down_weight_name: format!("{base}.experts.switch_glu.down_proj.weight"),
            expert_down_scales_name: format!("{base}.experts.switch_glu.down_proj.scales"),
            expert_down_biases_name: format!("{base}.experts.switch_glu.down_proj.biases"),
            layer_scalar_name: format!("{base}.layer_scalar"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct CachedLayerInputs {
    pub prefill_input_words: Vec<u16>,
    pub decode_input_words: Vec<u16>,
    pub prefill_rope_offset: i32,
    pub decode_rope_offset: i32,
    pub validate_against_oracle: bool,
}

impl CachedLayerInputs {
    pub fn synthetic_case() -> Self {
        Self {
            prefill_input_words: gemma4_qproj_case_input_bf16_words_with_phase(
                NORM_LEN,
                PREFILL_ACTIVATION_PHASE,
            ),
            decode_input_words: gemma4_qproj_case_input_bf16_words_with_phase(
                NORM_LEN,
                DECODE_ACTIVATION_PHASE,
            ),
            prefill_rope_offset: PREFILL_ROPE_OFFSET,
            decode_rope_offset: DECODE_ROPE_OFFSET,
            validate_against_oracle: true,
        }
    }
}

#[derive(Clone, Debug)]
pub struct CachedLayerSequenceInputs {
    pub prefill_input_words_list: Vec<Vec<u16>>,
    pub decode_input_words: Vec<u16>,
    pub prefill_rope_offset: i32,
    pub decode_rope_offset: i32,
    pub validate_against_oracle: bool,
}

impl CachedLayerSequenceInputs {
    pub fn from_single(inputs: CachedLayerInputs) -> Self {
        Self {
            prefill_input_words_list: vec![inputs.prefill_input_words],
            decode_input_words: inputs.decode_input_words,
            prefill_rope_offset: inputs.prefill_rope_offset,
            decode_rope_offset: inputs.decode_rope_offset,
            validate_against_oracle: inputs.validate_against_oracle,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Layer0CachedRouterOutput {
    pub router_scaled_bits: Vec<u32>,
    pub expert_scores_bits: Vec<u32>,
    pub router_probs_bits: Vec<u32>,
    pub top_k_indices: Vec<u32>,
    pub top_k_weights_bits: Vec<u32>,
}

#[derive(Clone, Debug)]
pub struct Layer0CachedArtifacts {
    pub backend_name: String,
    pub model_path: PathBuf,
    pub layer_idx: usize,
    pub selected_stage: Option<Layer0CachedStage>,
    pub prefill_rope_offset: i32,
    pub decode_rope_offset: i32,
    pub q_head_count: usize,
    pub k_head_count: usize,
    pub v_head_count: usize,
    pub q_heads_per_kv: usize,
    pub head_dim: usize,
    pub prefill_input_norm_bits: Vec<u32>,
    pub prefill_v_proj_bits: Vec<u32>,
    pub prefill_q_bits: Vec<u32>,
    pub prefill_k_bits: Vec<u32>,
    pub prefill_v_bits: Vec<u32>,
    pub decode_input_norm_bits: Vec<u32>,
    pub decode_v_proj_bits: Vec<u32>,
    pub decode_q_bits: Vec<u32>,
    pub decode_k_bits: Vec<u32>,
    pub decode_v_bits: Vec<u32>,
    pub full_k_bits: Vec<u32>,
    pub full_v_bits: Vec<u32>,
    pub attention_score_bits: Vec<u32>,
    pub attention_prob_bits: Vec<u32>,
    pub attention_out_bits: Vec<u32>,
    pub attention_oproj_bits: Option<Vec<u32>>,
    pub post_attention_norm_bits: Option<Vec<u32>>,
    pub post_attention_residual_bits: Option<Vec<u32>>,
    pub pre_feedforward_norm_bits: Option<Vec<u32>>,
    pub dense_gate_bits: Option<Vec<u32>>,
    pub dense_up_bits: Option<Vec<u32>>,
    pub dense_geglu_bits: Option<Vec<u32>>,
    pub dense_down_bits: Option<Vec<u32>>,
    pub router_output: Option<Layer0CachedRouterOutput>,
    pub moe_expert_gate_bits: Option<Vec<u32>>,
    pub moe_expert_up_bits: Option<Vec<u32>>,
    pub moe_expert_geglu_bits: Option<Vec<u32>>,
    pub moe_expert_down_bits: Option<Vec<u32>>,
    pub post_ffn_norm1_bits: Option<Vec<u32>>,
    pub moe_expert_out_bits: Option<Vec<u32>>,
    pub moe_post_ffn_norm2_bits: Option<Vec<u32>>,
    pub moe_merge_bits: Option<Vec<u32>>,
    pub prefill_post_ffn_residual_bits: Option<Vec<u32>>,
    pub post_ffn_residual_bits: Option<Vec<u32>>,
}

impl Layer0CachedArtifacts {
    pub fn stage_name(&self) -> &'static str {
        self.selected_stage
            .map(Layer0CachedStage::stage_name)
            .unwrap_or("qkv_attention_output_cached")
    }

    pub fn tensor_bits_for_stage(&self, stage: Layer0CachedStage) -> Option<&[u32]> {
        match stage {
            Layer0CachedStage::AttentionOproj => self.attention_oproj_bits.as_deref(),
            Layer0CachedStage::PostAttentionResidual => {
                self.post_attention_residual_bits.as_deref()
            }
            Layer0CachedStage::PreFeedforwardNorm => self.pre_feedforward_norm_bits.as_deref(),
            Layer0CachedStage::DenseGate => self.dense_gate_bits.as_deref(),
            Layer0CachedStage::DenseUp => self.dense_up_bits.as_deref(),
            Layer0CachedStage::DenseGeGlu => self.dense_geglu_bits.as_deref(),
            Layer0CachedStage::DenseDown => self.dense_down_bits.as_deref(),
            Layer0CachedStage::PostFfnNorm1 => self.post_ffn_norm1_bits.as_deref(),
            Layer0CachedStage::Router => None,
            Layer0CachedStage::MoeExpertGate => self.moe_expert_gate_bits.as_deref(),
            Layer0CachedStage::MoeExpertUp => self.moe_expert_up_bits.as_deref(),
            Layer0CachedStage::MoeExpertGeGlu => self.moe_expert_geglu_bits.as_deref(),
            Layer0CachedStage::MoeExpertDown => self.moe_expert_down_bits.as_deref(),
            Layer0CachedStage::MoeExpertOut => self.moe_expert_out_bits.as_deref(),
            Layer0CachedStage::MoePostFfnNorm2 => self.moe_post_ffn_norm2_bits.as_deref(),
            Layer0CachedStage::MoeMerge => self.moe_merge_bits.as_deref(),
            Layer0CachedStage::PostFfnResidual => self.post_ffn_residual_bits.as_deref(),
        }
    }

    pub fn bf16_words_for_stage(&self, stage: Layer0CachedStage) -> Option<Vec<u16>> {
        self.tensor_bits_for_stage(stage)
            .map(bf16_words_from_f32_bits)
    }

    pub fn layer_output_bits(&self) -> Option<&[u32]> {
        self.tensor_bits_for_stage(Layer0CachedStage::PostFfnResidual)
    }

    pub fn prefill_layer_output_bits(&self) -> Option<&[u32]> {
        self.prefill_post_ffn_residual_bits.as_deref()
    }

    pub fn prefill_layer_output_bf16_words(&self) -> Option<Vec<u16>> {
        self.prefill_layer_output_bits()
            .map(bf16_words_from_f32_bits)
    }
}

fn default_model_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../local/models/gemma-4-26b-mlx/model-00001-of-00003.safetensors")
}

fn model_root_dir(model_path: &Path) -> Result<PathBuf, Box<dyn Error>> {
    if model_path.is_dir() {
        return Ok(model_path.to_path_buf());
    }
    model_path.parent().map(Path::to_path_buf).ok_or_else(|| {
        format!(
            "model path {} has no parent directory",
            model_path.display()
        )
        .into()
    })
}

fn create_private_buffer_with_concatenated_tensors(
    runtime: &MetalRuntime,
    weights: &MlxIndexedSafetensors,
    tensor_names: &[&str],
) -> Result<MetalBuffer, Box<dyn Error>> {
    let mut bytes = Vec::new();
    for tensor_name in tensor_names {
        bytes.extend(weights.read_tensor_bytes(tensor_name)?);
    }
    Ok(runtime.create_buffer_with_bytes(&bytes, BufferStorageMode::Private)?)
}

fn create_private_buffer_with_concatenated_expert_tensors(
    runtime: &MetalRuntime,
    weights: &MlxIndexedSafetensors,
    tensor_names: &[&str],
) -> Result<MetalBuffer, Box<dyn Error>> {
    struct ExpertTensorBytes {
        bytes: Vec<u8>,
        expert_chunk_bytes: usize,
    }

    let first_name = *tensor_names
        .first()
        .ok_or("expected at least one expert tensor to concatenate")?;
    let first_entry = weights.tensor(first_name)?;
    if first_entry.shape.len() != 3 {
        return Err(format!(
            "expected rank-3 expert tensor for {}, got {:?}",
            first_name, first_entry.shape
        )
        .into());
    }
    let expert_count = usize::try_from(first_entry.shape[0])?;
    let row_width = first_entry.shape[2];
    let dtype = first_entry.dtype;
    let element_bytes = usize::try_from(dtype.byte_width())?;

    let mut total_bytes = 0usize;
    let mut tensors = Vec::with_capacity(tensor_names.len());
    for tensor_name in tensor_names {
        let entry = weights.tensor(tensor_name)?;
        if entry.shape.len() != 3 {
            return Err(format!(
                "expected rank-3 expert tensor for {}, got {:?}",
                tensor_name, entry.shape
            )
            .into());
        }
        if entry.dtype != dtype
            || entry.shape[0] != first_entry.shape[0]
            || entry.shape[2] != row_width
        {
            return Err(format!(
                "expert tensor layout mismatch for {}: dtype={:?} shape={:?}, expected dtype={:?} expert_count={} row_width={}",
                tensor_name,
                entry.dtype,
                entry.shape,
                dtype,
                first_entry.shape[0],
                row_width,
            )
            .into());
        }
        let rows = usize::try_from(entry.shape[1])?;
        let expert_chunk_bytes = rows
            .checked_mul(usize::try_from(row_width)?)
            .and_then(|value| value.checked_mul(element_bytes))
            .ok_or("expert tensor chunk size overflow")?;
        let bytes = weights.read_tensor_bytes(tensor_name)?;
        if bytes.len()
            != expert_count
                .checked_mul(expert_chunk_bytes)
                .ok_or("expert tensor size overflow")?
        {
            return Err(format!(
                "expert tensor byte size mismatch for {}: got {} expected {}",
                tensor_name,
                bytes.len(),
                expert_count * expert_chunk_bytes
            )
            .into());
        }
        total_bytes = total_bytes
            .checked_add(bytes.len())
            .ok_or("combined expert tensor size overflow")?;
        tensors.push(ExpertTensorBytes {
            bytes,
            expert_chunk_bytes,
        });
    }

    let mut bytes = Vec::with_capacity(total_bytes);
    for expert_idx in 0..expert_count {
        for tensor in &tensors {
            let start = expert_idx
                .checked_mul(tensor.expert_chunk_bytes)
                .ok_or("expert tensor slice start overflow")?;
            let end = start
                .checked_add(tensor.expert_chunk_bytes)
                .ok_or("expert tensor slice end overflow")?;
            bytes.extend_from_slice(&tensor.bytes[start..end]);
        }
    }
    Ok(runtime.create_buffer_with_bytes(&bytes, BufferStorageMode::Private)?)
}

fn load_optional_scalar_f32(
    weights: &MlxIndexedSafetensors,
    tensor_name: &str,
) -> Result<Option<f32>, Box<dyn Error>> {
    let entry = match weights.tensor(tensor_name) {
        Ok(entry) => entry,
        Err(_) => return Ok(None),
    };
    if entry.element_count() != 1 {
        return Err(format!("layer scalar tensor {} is not scalar", tensor_name).into());
    }
    let bytes = weights.read_tensor_bytes(tensor_name)?;
    let value = match entry.dtype {
        MlxDType::BF16 => {
            if bytes.len() != size_of::<u16>() {
                return Err(format!(
                    "layer scalar tensor {} bf16 byte length mismatch: {}",
                    tensor_name,
                    bytes.len()
                )
                .into());
            }
            bf16_word_to_f32(u16::from_le_bytes([bytes[0], bytes[1]]))
        }
        MlxDType::F32 => {
            if bytes.len() != size_of::<f32>() {
                return Err(format!(
                    "layer scalar tensor {} f32 byte length mismatch: {}",
                    tensor_name,
                    bytes.len()
                )
                .into());
            }
            f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
        }
        other => {
            return Err(format!(
                "layer scalar tensor {} has unsupported dtype {:?}",
                tensor_name, other
            )
            .into())
        }
    };
    Ok(Some(value))
}

struct LayerExecutionSession {
    model_path: PathBuf,
    weights: Arc<MlxIndexedSafetensors>,
    runtime: MetalRuntime,
    private_weight_buffers: HashMap<String, MetalBuffer>,
}

impl LayerExecutionSession {
    fn load(model_path: PathBuf) -> Result<Self, Box<dyn Error>> {
        let model_root = model_root_dir(&model_path)?;
        let weights = Arc::new(MlxIndexedSafetensors::load(&model_root)?);
        Self::load_with_weights(model_path, weights)
    }

    fn load_with_weights(
        model_path: PathBuf,
        weights: Arc<MlxIndexedSafetensors>,
    ) -> Result<Self, Box<dyn Error>> {
        let runtime =
            MetalRuntime::new().map_err(|err| format!("MetalRuntime::new failed: {err}"))?;
        if !runtime.features().has_bfloat {
            return Err("Metal device does not report BF16 support".into());
        }
        Ok(Self {
            model_path,
            weights,
            runtime,
            private_weight_buffers: HashMap::new(),
        })
    }

    fn private_weight_buffer(&mut self, name: &str) -> Result<MetalBuffer, Box<dyn Error>> {
        if let Some(buffer) = self.private_weight_buffers.get(name) {
            return Ok(buffer.clone());
        }
        let bytes = self.weights.read_tensor_bytes(name)?;
        let buffer = self
            .runtime
            .create_buffer_with_bytes(&bytes, BufferStorageMode::Private)?;
        self.private_weight_buffers
            .insert(name.to_string(), buffer.clone());
        Ok(buffer)
    }
}

struct ExactMetalKvCache {
    spec: GemmaKvCacheSpec,
    key_buffer: MetalBuffer,
    value_buffer: MetalBuffer,
    stored_tokens: usize,
    next_position: usize,
}

impl ExactMetalKvCache {
    fn load(runtime: &MetalRuntime, spec: GemmaKvCacheSpec) -> Result<Self, Box<dyn Error>> {
        let storage_words = spec
            .batch_size
            .checked_mul(spec.kv_head_count)
            .and_then(|value| value.checked_mul(spec.max_tokens))
            .and_then(|value| value.checked_mul(spec.head_dim))
            .ok_or("exact metal KV cache storage overflow")?;
        Ok(Self {
            key_buffer: create_bf16_buffer(runtime, storage_words, BufferStorageMode::Private)?,
            value_buffer: create_bf16_buffer(runtime, storage_words, BufferStorageMode::Private)?,
            spec,
            stored_tokens: 0,
            next_position: 0,
        })
    }

    fn reset(&mut self) {
        self.stored_tokens = 0;
        self.next_position = 0;
    }

    fn capacity_tokens(&self) -> usize {
        self.spec.max_tokens
    }

    fn row_stride_words(&self) -> Result<usize, Box<dyn Error>> {
        self.spec
            .max_tokens
            .checked_mul(self.spec.head_dim)
            .ok_or_else(|| "exact metal KV row stride overflow".into())
    }

    fn start_slot(&self) -> usize {
        match self.spec.attention {
            GemmaAttentionKind::Full => 0,
            GemmaAttentionKind::Sliding if self.stored_tokens < self.spec.max_tokens => 0,
            GemmaAttentionKind::Sliding => self.next_position % self.spec.max_tokens,
        }
    }

    fn seq_len(&self) -> usize {
        self.stored_tokens
    }

    fn append_token_from_buffers(
        &mut self,
        runtime: &MetalRuntime,
        src_k: &MetalBuffer,
        src_v: &MetalBuffer,
    ) -> Result<(), Box<dyn Error>> {
        if self.spec.attention == GemmaAttentionKind::Full
            && self.stored_tokens >= self.spec.max_tokens
        {
            return Err(format!(
                "exact metal full KV cache overflow: attempted token {} with capacity {}",
                self.next_position + 1,
                self.spec.max_tokens
            )
            .into());
        }

        let slot = self.next_position % self.spec.max_tokens;
        let head_dim_words = self.spec.head_dim;
        let row_stride_words = self.row_stride_words()?;
        let bytes_per_head = head_dim_words * size_of::<u16>();

        for head in 0..self.spec.kv_head_count {
            let src_offset = head
                .checked_mul(bytes_per_head)
                .ok_or("exact metal KV src offset overflow")?;
            let dst_word_offset = head
                .checked_mul(row_stride_words)
                .and_then(|value| value.checked_add(slot * head_dim_words))
                .ok_or("exact metal KV dst offset overflow")?;
            let dst_offset = dst_word_offset
                .checked_mul(size_of::<u16>())
                .ok_or("exact metal KV dst byte offset overflow")?;
            runtime.copy_buffer_range(
                src_k,
                src_offset,
                &self.key_buffer,
                dst_offset,
                bytes_per_head,
            )?;
            runtime.copy_buffer_range(
                src_v,
                src_offset,
                &self.value_buffer,
                dst_offset,
                bytes_per_head,
            )?;
        }

        self.next_position = self
            .next_position
            .checked_add(1)
            .ok_or("exact metal KV next_position overflow")?;
        self.stored_tokens = self
            .stored_tokens
            .saturating_add(1)
            .min(self.spec.max_tokens);
        Ok(())
    }

    fn append_token_from_buffers_compute(
        &mut self,
        runtime: &MetalRuntime,
        append_pipeline: &MetalPipeline,
        src_k: &MetalBuffer,
        src_v: &MetalBuffer,
    ) -> Result<(), Box<dyn Error>> {
        if self.spec.attention == GemmaAttentionKind::Full
            && self.stored_tokens >= self.spec.max_tokens
        {
            return Err(format!(
                "exact metal full KV cache overflow: attempted token {} with capacity {}",
                self.next_position + 1,
                self.spec.max_tokens
            )
            .into());
        }

        let slot = self.next_position % self.spec.max_tokens;
        let row_stride_words = self.row_stride_words()?;
        let args = MlxKvAppendBf16Args {
            head_dim: self.spec.head_dim as u32,
            src_row_stride: self.spec.head_dim as u32,
            dst_row_stride: row_stride_words as u32,
            head_count: self.spec.kv_head_count as u32,
            slot: slot as u32,
        };
        let threadgroups = MetalSize {
            width: (self.spec.head_dim as u64).div_ceil(64),
            height: self.spec.kv_head_count as u64,
            depth: 1,
        };
        let threads_per_threadgroup = MetalSize {
            width: 64,
            height: 1,
            depth: 1,
        };

        dispatch_compute_tracked_split(
            runtime,
            append_pipeline,
            bytes_of(&args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: src_k,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: src_v,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &self.key_buffer,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 4,
                    buffer: &self.value_buffer,
                    offset_bytes: 0,
                },
            ],
            2,
            &[],
            threadgroups,
            threads_per_threadgroup,
        )?;
        self.next_position = self
            .next_position
            .checked_add(1)
            .ok_or("exact metal KV next_position overflow")?;
        self.stored_tokens = self
            .stored_tokens
            .saturating_add(1)
            .min(self.spec.max_tokens);
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
struct ExactMetalQprojLayout {
    weight_words_per_row: u32,
    qparams_per_row: u32,
    out_rows: u32,
}

impl ExactMetalQprojLayout {
    fn out_len(self) -> usize {
        self.out_rows as usize
    }

    fn uses_fast_qmv(self, n_in: u32) -> bool {
        self.out_rows % 8 == 0 && n_in % 512 == 0
    }

    fn row_args(self, n_in: u32) -> MlxAffineQprojRowArgs {
        MlxAffineQprojRowArgs {
            n_in,
            weight_words_per_row: self.weight_words_per_row,
            qparams_per_row: self.qparams_per_row,
            out_rows: self.out_rows,
        }
    }

    fn selected_experts_args(
        self,
        n_in: u32,
        input_row_stride: u32,
    ) -> MlxAffineSelectedExpertsQprojRowArgs {
        MlxAffineSelectedExpertsQprojRowArgs {
            n_in,
            weight_words_per_row: self.weight_words_per_row,
            qparams_per_row: self.qparams_per_row,
            out_rows: self.out_rows,
            input_row_stride,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct ExactMetalRopeLayout {
    half_dims: u32,
    row_stride: u32,
    row_count: u32,
    base_log2: f32,
}

impl ExactMetalRopeLayout {
    fn args(self, position: usize) -> Result<MlxRopeSingleArgs, Box<dyn Error>> {
        Ok(MlxRopeSingleArgs {
            half_dims: self.half_dims,
            row_stride: self.row_stride,
            row_count: self.row_count,
            offset: i32::try_from(position)?,
            scale: ROPE_SCALE,
            base_log2: self.base_log2,
        })
    }
}

#[derive(Clone)]
struct ExactMetalLayerBuffers {
    x: MetalBuffer,
    h: MetalBuffer,
    qkv_proj_out: MetalBuffer,
    q_norm: MetalBuffer,
    q_rope: MetalBuffer,
    k_norm: MetalBuffer,
    k_rope: MetalBuffer,
    v_norm: MetalBuffer,
    attention_logits: MetalBuffer,
    attention_probs: MetalBuffer,
    attn_out: MetalBuffer,
    o_proj_out: MetalBuffer,
    post_attention_norm_out: MetalBuffer,
    residual_out: MetalBuffer,
    pre_feedforward_norm_out: MetalBuffer,
    mlp_gate_up_out: MetalBuffer,
    geglu_out: MetalBuffer,
    mlp_down_out: MetalBuffer,
    router_scaled_out: MetalBuffer,
    router_proj_out: MetalBuffer,
    router_probs_out: MetalBuffer,
    pre_feedforward_norm2_out: MetalBuffer,
    moe_top_k_indices: MetalBuffer,
    moe_top_k_weights: MetalBuffer,
    expert_gate_up_out: MetalBuffer,
    expert_geglu_out: MetalBuffer,
    expert_down_out: MetalBuffer,
    post_feedforward_norm_out: MetalBuffer,
    post_feedforward_norm1_out: MetalBuffer,
    moe_weighted_out: MetalBuffer,
    moe_post_ffn_norm2_out: MetalBuffer,
    moe_merge_out: MetalBuffer,
    post_ffn_residual_out: MetalBuffer,
}

#[derive(Clone)]
struct ExactMetalLayerWeights {
    input_norm_weight: MetalBuffer,
    qkv_proj_weight: MetalBuffer,
    qkv_proj_scales: MetalBuffer,
    qkv_proj_biases: MetalBuffer,
    q_norm_weight: MetalBuffer,
    k_norm_weight: MetalBuffer,
    v_norm_weight: MetalBuffer,
    o_weight: MetalBuffer,
    o_scales: MetalBuffer,
    o_biases: MetalBuffer,
    post_attention_norm_weight: MetalBuffer,
    pre_feedforward_norm_weight: MetalBuffer,
    pre_feedforward_norm2_weight: MetalBuffer,
    mlp_gate_up_weight: MetalBuffer,
    mlp_gate_up_scales: MetalBuffer,
    mlp_gate_up_biases: MetalBuffer,
    mlp_down_weight: MetalBuffer,
    mlp_down_scales: MetalBuffer,
    mlp_down_biases: MetalBuffer,
    router_scale_weight: MetalBuffer,
    router_proj_weight: MetalBuffer,
    router_proj_scales: MetalBuffer,
    router_proj_biases: MetalBuffer,
    router_per_expert_scale: MetalBuffer,
    expert_gate_up_weight: MetalBuffer,
    expert_gate_up_scales: MetalBuffer,
    expert_gate_up_biases: MetalBuffer,
    expert_down_weight: MetalBuffer,
    expert_down_scales: MetalBuffer,
    expert_down_biases: MetalBuffer,
    post_feedforward_norm_weight: MetalBuffer,
    post_feedforward_norm1_weight: MetalBuffer,
    post_feedforward_norm2_weight: MetalBuffer,
}

#[derive(Clone)]
struct ExactMetalLayerPipelines {
    rms: MetalPipeline,
    proj: MetalPipeline,
    proj_fast: MetalPipeline,
    head_norm: MetalPipeline,
    rope: MetalPipeline,
    attention_logits_seq: MetalPipeline,
    attention_softmax_rows: MetalPipeline,
    attention_weighted_sum: MetalPipeline,
    o_proj_fast: MetalPipeline,
    residual: MetalPipeline,
    weighted_sum_rows: MetalPipeline,
    geglu: MetalPipeline,
    geglu_strided: MetalPipeline,
    router_scale_pair: MetalPipeline,
    router_topk: MetalPipeline,
    selected_expert_proj: MetalPipeline,
    scale_row: MetalPipeline,
}

#[derive(Clone)]
struct ExactMetalLayerWorkspace {
    qkv_proj: ExactMetalQprojLayout,
    q_proj: ExactMetalQprojLayout,
    k_proj: ExactMetalQprojLayout,
    o_proj: ExactMetalQprojLayout,
    mlp_gate_up: ExactMetalQprojLayout,
    mlp_gate: ExactMetalQprojLayout,
    mlp_down: ExactMetalQprojLayout,
    router_proj: ExactMetalQprojLayout,
    expert_gate_up: ExactMetalQprojLayout,
    expert_gate: ExactMetalQprojLayout,
    expert_down: ExactMetalQprojLayout,
    post_attention_norm_len: usize,
    pre_feedforward_norm_len: usize,
    pre_feedforward_norm2_len: usize,
    post_feedforward_norm_len: usize,
    post_feedforward_norm1_len: usize,
    post_feedforward_norm2_len: usize,
    q_head_count: usize,
    k_head_count: usize,
    v_head_count: usize,
    q_heads_per_kv: usize,
    head_dim: usize,
    kv_cache_capacity_tokens: usize,
    eps: f32,
    layer_scalar: Option<f32>,
    q_rope: ExactMetalRopeLayout,
    k_rope: ExactMetalRopeLayout,
    buffers: ExactMetalLayerBuffers,
    weights: ExactMetalLayerWeights,
    pipelines: ExactMetalLayerPipelines,
}

#[derive(Clone)]
struct ExactMetalTextIoBuffers {
    standalone_hidden: MetalBuffer,
    hidden_scratch: MetalBuffer,
    final_norm_out: MetalBuffer,
    logits_out: MetalBuffer,
    argmax_index_out: MetalBuffer,
    generated_token_chunk_out: MetalBuffer,
}

#[derive(Clone)]
struct ExactMetalTextIoWeights {
    embed_weight: MetalBuffer,
    embed_scales: MetalBuffer,
    embed_biases: MetalBuffer,
    final_norm_weight: MetalBuffer,
}

#[derive(Clone)]
struct ExactMetalTextIoPipelines {
    dequant_row: MetalPipeline,
    dequant_row_from_token_buffer: MetalPipeline,
    rms: MetalPipeline,
    logits_proj: MetalPipeline,
    argmax_softcapped_bf16: MetalPipeline,
}

#[derive(Clone)]
struct ExactMetalTextIoWorkspace {
    embed_weight_row_bytes: usize,
    embed_qparams_row_bytes: usize,
    logits_qproj: ExactMetalQprojLayout,
    vocab_size: usize,
    eps: f32,
    softcap: Option<f32>,
    buffers: ExactMetalTextIoBuffers,
    weights: ExactMetalTextIoWeights,
    pipelines: ExactMetalTextIoPipelines,
}

fn dispatch_exact_mlx_qmv_row(
    runtime: &MetalRuntime,
    generic_pipeline: &MetalPipeline,
    fast_pipeline: &MetalPipeline,
    layout: ExactMetalQprojLayout,
    args: &MlxAffineQprojRowArgs,
    bindings: &[MetalBufferBindingRef<'_>],
    threadgroups: MetalSize,
    threads_per_threadgroup: MetalSize,
) -> Result<(), Box<dyn Error>> {
    let pipeline = if layout.uses_fast_qmv(args.n_in) {
        fast_pipeline
    } else {
        generic_pipeline
    };
    runtime.dispatch_compute_tracked(
        pipeline,
        bytes_of(args),
        &bindings[..bindings.len() - 1],
        &bindings[bindings.len() - 1..],
        &[],
        threadgroups,
        threads_per_threadgroup,
    )?;
    Ok(())
}

fn dispatch_compute_tracked_split<const N: usize>(
    runtime: &MetalRuntime,
    pipeline: &MetalPipeline,
    args_bytes: &[u8],
    bindings: [MetalBufferBindingRef<'_>; N],
    output_start: usize,
    threadgroup_memory_lengths: &[(u64, usize)],
    threadgroups: MetalSize,
    threads_per_threadgroup: MetalSize,
) -> Result<(), Box<dyn Error>> {
    runtime.dispatch_compute_tracked(
        pipeline,
        args_bytes,
        &bindings[..output_start],
        &bindings[output_start..],
        threadgroup_memory_lengths,
        threadgroups,
        threads_per_threadgroup,
    )?;
    Ok(())
}

impl ExactMetalLayerWorkspace {
    fn load(session: &mut LayerExecutionSession, layer_idx: usize) -> Result<Self, Box<dyn Error>> {
        let indexed = session.weights.clone();
        let runtime = session.runtime.clone();
        let text_config = &indexed.snapshot.config.text_config;
        let kv_layout = GemmaKvCacheLayout::from_text_config(text_config, 1)?;
        let cache_spec = kv_layout.cache_spec_for_layer(layer_idx)?.clone();
        if !text_config.enable_moe_block {
            return Err("exact metal text runtime currently expects Gemma MoE layers".into());
        }
        if text_config.top_k_experts as usize != ROUTER_TOP_K {
            return Err(format!(
                "exact metal text runtime expects top_k_experts={}, got {}",
                ROUTER_TOP_K, text_config.top_k_experts
            )
            .into());
        }

        let layer_type = text_config
            .layer_types
            .get(layer_idx)
            .ok_or_else(|| format!("missing text layer type for layer {layer_idx}"))?;
        let attention_k_eq_v = text_config.attention_k_eq_v && layer_type == "full_attention";
        let layer_names = LayerTensorNames::for_layer(layer_idx, attention_k_eq_v);
        let q_norm_weight_name = layer_names
            .q
            .norm_weight_name
            .as_deref()
            .ok_or("missing q norm weight name")?;
        let k_norm_weight_name = layer_names
            .k
            .norm_weight_name
            .as_deref()
            .ok_or("missing k norm weight name")?;

        let q_weight_entry = indexed.tensor(&layer_names.q.weight_name)?;
        let q_scales_entry = indexed.tensor(&layer_names.q.scales_name)?;
        let q_norm_weight_entry = indexed.tensor(q_norm_weight_name)?;
        let k_weight_entry = indexed.tensor(&layer_names.k.weight_name)?;
        let k_scales_entry = indexed.tensor(&layer_names.k.scales_name)?;
        let k_norm_weight_entry = indexed.tensor(k_norm_weight_name)?;
        let v_weight_entry = indexed.tensor(&layer_names.v.weight_name)?;
        let v_scales_entry = indexed.tensor(&layer_names.v.scales_name)?;
        let o_weight_entry = indexed.tensor(&layer_names.o.weight_name)?;
        let o_scales_entry = indexed.tensor(&layer_names.o.scales_name)?;
        let mlp_gate_weight_entry = indexed.tensor(&layer_names.mlp_gate_weight_name)?;
        let mlp_gate_scales_entry = indexed.tensor(&layer_names.mlp_gate_scales_name)?;
        let mlp_up_weight_entry = indexed.tensor(&layer_names.mlp_up_weight_name)?;
        let mlp_up_scales_entry = indexed.tensor(&layer_names.mlp_up_scales_name)?;
        let mlp_down_weight_entry = indexed.tensor(&layer_names.mlp_down_weight_name)?;
        let mlp_down_scales_entry = indexed.tensor(&layer_names.mlp_down_scales_name)?;
        let router_proj_weight_entry = indexed.tensor(&layer_names.router_proj_weight_name)?;
        let router_proj_scales_entry = indexed.tensor(&layer_names.router_proj_scales_name)?;
        let expert_gate_weight_entry = indexed.tensor(&layer_names.expert_gate_weight_name)?;
        let expert_gate_scales_entry = indexed.tensor(&layer_names.expert_gate_scales_name)?;
        let expert_up_weight_entry = indexed.tensor(&layer_names.expert_up_weight_name)?;
        let expert_up_scales_entry = indexed.tensor(&layer_names.expert_up_scales_name)?;
        let expert_down_weight_entry = indexed.tensor(&layer_names.expert_down_weight_name)?;
        let expert_down_scales_entry = indexed.tensor(&layer_names.expert_down_scales_name)?;
        let layer_scalar = load_optional_scalar_f32(&indexed, &layer_names.layer_scalar_name)?;

        let q_proj = ExactMetalQprojLayout {
            weight_words_per_row: q_weight_entry.shape[1] as u32,
            qparams_per_row: q_scales_entry.shape[1] as u32,
            out_rows: u32::try_from(q_weight_entry.shape[0])?,
        };
        let k_proj = ExactMetalQprojLayout {
            weight_words_per_row: k_weight_entry.shape[1] as u32,
            qparams_per_row: k_scales_entry.shape[1] as u32,
            out_rows: u32::try_from(k_weight_entry.shape[0])?,
        };
        let v_proj = ExactMetalQprojLayout {
            weight_words_per_row: v_weight_entry.shape[1] as u32,
            qparams_per_row: v_scales_entry.shape[1] as u32,
            out_rows: u32::try_from(v_weight_entry.shape[0])?,
        };
        if q_proj.weight_words_per_row != k_proj.weight_words_per_row
            || q_proj.weight_words_per_row != v_proj.weight_words_per_row
            || q_proj.qparams_per_row != k_proj.qparams_per_row
            || q_proj.qparams_per_row != v_proj.qparams_per_row
        {
            return Err(format!(
                "q/k/v projection layout mismatch in layer {layer_idx}: q=({}, {}) k=({}, {}) v=({}, {})",
                q_proj.weight_words_per_row,
                q_proj.qparams_per_row,
                k_proj.weight_words_per_row,
                k_proj.qparams_per_row,
                v_proj.weight_words_per_row,
                v_proj.qparams_per_row,
            )
            .into());
        }
        let qkv_proj = ExactMetalQprojLayout {
            weight_words_per_row: q_proj.weight_words_per_row,
            qparams_per_row: q_proj.qparams_per_row,
            out_rows: q_proj
                .out_rows
                .checked_add(k_proj.out_rows)
                .and_then(|value| value.checked_add(v_proj.out_rows))
                .ok_or("qkv combined out_rows overflow")?,
        };
        let o_proj = ExactMetalQprojLayout {
            weight_words_per_row: o_weight_entry.shape[1] as u32,
            qparams_per_row: o_scales_entry.shape[1] as u32,
            out_rows: u32::try_from(o_weight_entry.shape[0])?,
        };
        let mlp_gate = ExactMetalQprojLayout {
            weight_words_per_row: mlp_gate_weight_entry.shape[1] as u32,
            qparams_per_row: mlp_gate_scales_entry.shape[1] as u32,
            out_rows: u32::try_from(mlp_gate_weight_entry.shape[0])?,
        };
        let mlp_up = ExactMetalQprojLayout {
            weight_words_per_row: mlp_up_weight_entry.shape[1] as u32,
            qparams_per_row: mlp_up_scales_entry.shape[1] as u32,
            out_rows: u32::try_from(mlp_up_weight_entry.shape[0])?,
        };
        if mlp_gate.weight_words_per_row != mlp_up.weight_words_per_row
            || mlp_gate.qparams_per_row != mlp_up.qparams_per_row
        {
            return Err(format!(
                "dense MLP gate/up layout mismatch in layer {layer_idx}: gate=({}, {}) up=({}, {})",
                mlp_gate.weight_words_per_row,
                mlp_gate.qparams_per_row,
                mlp_up.weight_words_per_row,
                mlp_up.qparams_per_row,
            )
            .into());
        }
        let mlp_gate_up = ExactMetalQprojLayout {
            weight_words_per_row: mlp_gate.weight_words_per_row,
            qparams_per_row: mlp_gate.qparams_per_row,
            out_rows: mlp_gate
                .out_rows
                .checked_add(mlp_up.out_rows)
                .ok_or("mlp gate/up combined out_rows overflow")?,
        };
        let mlp_down = ExactMetalQprojLayout {
            weight_words_per_row: mlp_down_weight_entry.shape[1] as u32,
            qparams_per_row: mlp_down_scales_entry.shape[1] as u32,
            out_rows: u32::try_from(mlp_down_weight_entry.shape[0])?,
        };
        let router_proj = ExactMetalQprojLayout {
            weight_words_per_row: router_proj_weight_entry.shape[1] as u32,
            qparams_per_row: router_proj_scales_entry.shape[1] as u32,
            out_rows: u32::try_from(router_proj_weight_entry.shape[0])?,
        };
        let expert_gate = ExactMetalQprojLayout {
            weight_words_per_row: expert_gate_weight_entry.shape[2] as u32,
            qparams_per_row: expert_gate_scales_entry.shape[2] as u32,
            out_rows: u32::try_from(expert_gate_weight_entry.shape[1])?,
        };
        let expert_up = ExactMetalQprojLayout {
            weight_words_per_row: expert_up_weight_entry.shape[2] as u32,
            qparams_per_row: expert_up_scales_entry.shape[2] as u32,
            out_rows: u32::try_from(expert_up_weight_entry.shape[1])?,
        };
        if expert_gate.weight_words_per_row != expert_up.weight_words_per_row
            || expert_gate.qparams_per_row != expert_up.qparams_per_row
            || expert_gate.out_rows != expert_up.out_rows
        {
            return Err(format!(
                "expert gate/up layout mismatch in layer {layer_idx}: gate=({}, {}, {}) up=({}, {}, {})",
                expert_gate.weight_words_per_row,
                expert_gate.qparams_per_row,
                expert_gate.out_rows,
                expert_up.weight_words_per_row,
                expert_up.qparams_per_row,
                expert_up.out_rows,
            )
            .into());
        }
        let expert_gate_up = ExactMetalQprojLayout {
            weight_words_per_row: expert_gate.weight_words_per_row,
            qparams_per_row: expert_gate.qparams_per_row,
            out_rows: expert_gate
                .out_rows
                .checked_add(expert_up.out_rows)
                .ok_or("expert gate/up combined out_rows overflow")?,
        };
        let expert_down = ExactMetalQprojLayout {
            weight_words_per_row: expert_down_weight_entry.shape[2] as u32,
            qparams_per_row: expert_down_scales_entry.shape[2] as u32,
            out_rows: u32::try_from(expert_down_weight_entry.shape[1])?,
        };

        let post_attention_norm_len = usize::try_from(
            indexed
                .tensor(&layer_names.post_attention_norm_weight_name)?
                .shape[0],
        )?;
        let pre_feedforward_norm_len = usize::try_from(
            indexed
                .tensor(&layer_names.pre_feedforward_norm_weight_name)?
                .shape[0],
        )?;
        let pre_feedforward_norm2_len = usize::try_from(
            indexed
                .tensor(&layer_names.pre_feedforward_norm2_weight_name)?
                .shape[0],
        )?;
        let post_feedforward_norm_len = usize::try_from(
            indexed
                .tensor(&layer_names.post_feedforward_norm_weight_name)?
                .shape[0],
        )?;
        let post_feedforward_norm1_len = usize::try_from(
            indexed
                .tensor(&layer_names.post_feedforward_norm1_weight_name)?
                .shape[0],
        )?;
        let post_feedforward_norm2_len = usize::try_from(
            indexed
                .tensor(&layer_names.post_feedforward_norm2_weight_name)?
                .shape[0],
        )?;
        if post_attention_norm_len != NORM_LEN
            || pre_feedforward_norm_len != NORM_LEN
            || pre_feedforward_norm2_len != NORM_LEN
            || post_feedforward_norm_len != NORM_LEN
            || post_feedforward_norm1_len != NORM_LEN
            || post_feedforward_norm2_len != NORM_LEN
            || o_proj.out_len() != NORM_LEN
            || mlp_down.out_len() != NORM_LEN
            || expert_down.out_len() != NORM_LEN
        {
            return Err(format!(
                "exact metal text runtime expects hidden-size-preserving layer {layer_idx}"
            )
            .into());
        }

        let head_dim = usize::try_from(q_norm_weight_entry.shape[0])?;
        let k_head_dim = usize::try_from(k_norm_weight_entry.shape[0])?;
        if head_dim == 0 || head_dim != k_head_dim {
            return Err(format!("invalid q/k head_dim: q={head_dim} k={k_head_dim}").into());
        }
        if q_proj.out_len() % head_dim != 0
            || k_proj.out_len() % head_dim != 0
            || v_proj.out_len() % head_dim != 0
        {
            return Err(format!(
                "invalid q/k/v head layout: q_out_len={} k_out_len={} v_out_len={} head_dim={}",
                q_proj.out_len(),
                k_proj.out_len(),
                v_proj.out_len(),
                head_dim
            )
            .into());
        }
        let q_head_count = q_proj.out_len() / head_dim;
        let k_head_count = k_proj.out_len() / head_dim;
        let v_head_count = v_proj.out_len() / head_dim;
        if k_head_count == 0 || v_head_count != k_head_count || q_head_count % k_head_count != 0 {
            return Err(format!(
                "invalid grouped-query head layout: q_head_count={} k_head_count={} v_head_count={}",
                q_head_count, k_head_count, v_head_count
            )
            .into());
        }
        let q_heads_per_kv = q_head_count / k_head_count;
        let rope_params = if layer_type == "full_attention" {
            &text_config.rope_parameters.full_attention
        } else {
            &text_config.rope_parameters.sliding_attention
        };
        let rope_rotary_dim = if let Some(partial_factor) = rope_params.partial_rotary_factor {
            let rotary_dim = (head_dim as f32 * partial_factor).round() as usize;
            if rotary_dim == 0 || rotary_dim > head_dim || rotary_dim % 2 != 0 {
                return Err(format!(
                    "invalid rope rotary dim {} for layer {} head_dim {} factor {}",
                    rotary_dim, layer_idx, head_dim, partial_factor
                )
                .into());
            }
            rotary_dim
        } else {
            head_dim
        };
        let rope_half_dims = rope_rotary_dim / 2;
        let rope_base_log2 = (rope_params.rope_theta as f32).log2();

        let buffers = ExactMetalLayerBuffers {
            x: create_bf16_buffer(&runtime, NORM_LEN, BufferStorageMode::Shared)?,
            h: create_bf16_buffer(&runtime, NORM_LEN, BufferStorageMode::Private)?,
            qkv_proj_out: create_bf16_buffer(
                &runtime,
                qkv_proj.out_len(),
                BufferStorageMode::Private,
            )?,
            q_norm: create_bf16_buffer(&runtime, q_proj.out_len(), BufferStorageMode::Private)?,
            q_rope: create_bf16_buffer(&runtime, q_proj.out_len(), BufferStorageMode::Private)?,
            k_norm: create_bf16_buffer(&runtime, k_proj.out_len(), BufferStorageMode::Private)?,
            k_rope: create_bf16_buffer(&runtime, k_proj.out_len(), BufferStorageMode::Private)?,
            v_norm: create_bf16_buffer(&runtime, v_proj.out_len(), BufferStorageMode::Private)?,
            attention_logits: create_bf16_buffer(
                &runtime,
                q_head_count * cache_spec.max_tokens,
                BufferStorageMode::Private,
            )?,
            attention_probs: create_bf16_buffer(
                &runtime,
                q_head_count * cache_spec.max_tokens,
                BufferStorageMode::Private,
            )?,
            attn_out: create_bf16_buffer(&runtime, q_proj.out_len(), BufferStorageMode::Private)?,
            o_proj_out: create_bf16_buffer(&runtime, o_proj.out_len(), BufferStorageMode::Private)?,
            post_attention_norm_out: create_bf16_buffer(
                &runtime,
                post_attention_norm_len,
                BufferStorageMode::Private,
            )?,
            residual_out: create_bf16_buffer(
                &runtime,
                post_attention_norm_len,
                BufferStorageMode::Private,
            )?,
            pre_feedforward_norm_out: create_bf16_buffer(
                &runtime,
                pre_feedforward_norm_len,
                BufferStorageMode::Private,
            )?,
            mlp_gate_up_out: create_bf16_buffer(
                &runtime,
                mlp_gate_up.out_len(),
                BufferStorageMode::Private,
            )?,
            geglu_out: create_bf16_buffer(
                &runtime,
                mlp_gate.out_len(),
                BufferStorageMode::Private,
            )?,
            mlp_down_out: create_bf16_buffer(
                &runtime,
                mlp_down.out_len(),
                BufferStorageMode::Private,
            )?,
            router_scaled_out: create_bf16_buffer(
                &runtime,
                post_attention_norm_len,
                BufferStorageMode::Private,
            )?,
            router_proj_out: create_bf16_buffer(
                &runtime,
                router_proj.out_len(),
                BufferStorageMode::Private,
            )?,
            router_probs_out: create_bf16_buffer(
                &runtime,
                router_proj.out_len(),
                BufferStorageMode::Private,
            )?,
            pre_feedforward_norm2_out: create_bf16_buffer(
                &runtime,
                pre_feedforward_norm2_len,
                BufferStorageMode::Private,
            )?,
            moe_top_k_indices: runtime
                .create_buffer(ROUTER_TOP_K * size_of::<u32>(), BufferStorageMode::Private)?,
            moe_top_k_weights: create_bf16_buffer(
                &runtime,
                ROUTER_TOP_K,
                BufferStorageMode::Private,
            )?,
            expert_gate_up_out: create_bf16_buffer(
                &runtime,
                ROUTER_TOP_K * expert_gate_up.out_len(),
                BufferStorageMode::Private,
            )?,
            expert_geglu_out: create_bf16_buffer(
                &runtime,
                ROUTER_TOP_K * expert_gate.out_len(),
                BufferStorageMode::Private,
            )?,
            expert_down_out: create_bf16_buffer(
                &runtime,
                ROUTER_TOP_K * expert_down.out_len(),
                BufferStorageMode::Private,
            )?,
            post_feedforward_norm_out: create_bf16_buffer(
                &runtime,
                post_feedforward_norm_len,
                BufferStorageMode::Private,
            )?,
            post_feedforward_norm1_out: create_bf16_buffer(
                &runtime,
                post_feedforward_norm1_len,
                BufferStorageMode::Private,
            )?,
            moe_weighted_out: create_bf16_buffer(
                &runtime,
                post_feedforward_norm2_len,
                BufferStorageMode::Private,
            )?,
            moe_post_ffn_norm2_out: create_bf16_buffer(
                &runtime,
                post_feedforward_norm2_len,
                BufferStorageMode::Private,
            )?,
            moe_merge_out: create_bf16_buffer(
                &runtime,
                post_feedforward_norm1_len,
                BufferStorageMode::Private,
            )?,
            post_ffn_residual_out: create_bf16_buffer(
                &runtime,
                post_feedforward_norm1_len,
                BufferStorageMode::Shared,
            )?,
        };

        let v_norm_weight = runtime.create_buffer_with_bytes(
            &bytes_from_bf16_words(&vec![0x3F80u16; head_dim]),
            BufferStorageMode::Private,
        )?;
        let weights = ExactMetalLayerWeights {
            input_norm_weight: session
                .private_weight_buffer(&layer_names.input_norm_weight_name)?,
            qkv_proj_weight: create_private_buffer_with_concatenated_tensors(
                &runtime,
                &indexed,
                &[
                    &layer_names.q.weight_name,
                    &layer_names.k.weight_name,
                    &layer_names.v.weight_name,
                ],
            )?,
            qkv_proj_scales: create_private_buffer_with_concatenated_tensors(
                &runtime,
                &indexed,
                &[
                    &layer_names.q.scales_name,
                    &layer_names.k.scales_name,
                    &layer_names.v.scales_name,
                ],
            )?,
            qkv_proj_biases: create_private_buffer_with_concatenated_tensors(
                &runtime,
                &indexed,
                &[
                    &layer_names.q.biases_name,
                    &layer_names.k.biases_name,
                    &layer_names.v.biases_name,
                ],
            )?,
            q_norm_weight: session.private_weight_buffer(q_norm_weight_name)?,
            k_norm_weight: session.private_weight_buffer(k_norm_weight_name)?,
            v_norm_weight,
            o_weight: session.private_weight_buffer(&layer_names.o.weight_name)?,
            o_scales: session.private_weight_buffer(&layer_names.o.scales_name)?,
            o_biases: session.private_weight_buffer(&layer_names.o.biases_name)?,
            post_attention_norm_weight: session
                .private_weight_buffer(&layer_names.post_attention_norm_weight_name)?,
            pre_feedforward_norm_weight: session
                .private_weight_buffer(&layer_names.pre_feedforward_norm_weight_name)?,
            pre_feedforward_norm2_weight: session
                .private_weight_buffer(&layer_names.pre_feedforward_norm2_weight_name)?,
            mlp_gate_up_weight: create_private_buffer_with_concatenated_tensors(
                &runtime,
                &indexed,
                &[
                    &layer_names.mlp_gate_weight_name,
                    &layer_names.mlp_up_weight_name,
                ],
            )?,
            mlp_gate_up_scales: create_private_buffer_with_concatenated_tensors(
                &runtime,
                &indexed,
                &[
                    &layer_names.mlp_gate_scales_name,
                    &layer_names.mlp_up_scales_name,
                ],
            )?,
            mlp_gate_up_biases: create_private_buffer_with_concatenated_tensors(
                &runtime,
                &indexed,
                &[
                    &layer_names.mlp_gate_biases_name,
                    &layer_names.mlp_up_biases_name,
                ],
            )?,
            mlp_down_weight: session.private_weight_buffer(&layer_names.mlp_down_weight_name)?,
            mlp_down_scales: session.private_weight_buffer(&layer_names.mlp_down_scales_name)?,
            mlp_down_biases: session.private_weight_buffer(&layer_names.mlp_down_biases_name)?,
            router_scale_weight: session.private_weight_buffer(&layer_names.router_scale_name)?,
            router_proj_weight: session
                .private_weight_buffer(&layer_names.router_proj_weight_name)?,
            router_proj_scales: session
                .private_weight_buffer(&layer_names.router_proj_scales_name)?,
            router_proj_biases: session
                .private_weight_buffer(&layer_names.router_proj_biases_name)?,
            router_per_expert_scale: session
                .private_weight_buffer(&layer_names.router_per_expert_scale_name)?,
            expert_gate_up_weight: create_private_buffer_with_concatenated_expert_tensors(
                &runtime,
                &indexed,
                &[
                    &layer_names.expert_gate_weight_name,
                    &layer_names.expert_up_weight_name,
                ],
            )?,
            expert_gate_up_scales: create_private_buffer_with_concatenated_expert_tensors(
                &runtime,
                &indexed,
                &[
                    &layer_names.expert_gate_scales_name,
                    &layer_names.expert_up_scales_name,
                ],
            )?,
            expert_gate_up_biases: create_private_buffer_with_concatenated_expert_tensors(
                &runtime,
                &indexed,
                &[
                    &layer_names.expert_gate_biases_name,
                    &layer_names.expert_up_biases_name,
                ],
            )?,
            expert_down_weight: session
                .private_weight_buffer(&layer_names.expert_down_weight_name)?,
            expert_down_scales: session
                .private_weight_buffer(&layer_names.expert_down_scales_name)?,
            expert_down_biases: session
                .private_weight_buffer(&layer_names.expert_down_biases_name)?,
            post_feedforward_norm_weight: session
                .private_weight_buffer(&layer_names.post_feedforward_norm_weight_name)?,
            post_feedforward_norm1_weight: session
                .private_weight_buffer(&layer_names.post_feedforward_norm1_weight_name)?,
            post_feedforward_norm2_weight: session
                .private_weight_buffer(&layer_names.post_feedforward_norm2_weight_name)?,
        };

        let pipelines = ExactMetalLayerPipelines {
            rms: compile_default_pipeline(&runtime, "kernel_mlx_rms_norm_row_bf16")?,
            proj: compile_default_pipeline(&runtime, "kernel_mlx_affine_qmv_row_bf16")?,
            proj_fast: compile_default_pipeline(&runtime, "kernel_mlx_affine_qmv_fast_row_bf16")?,
            head_norm: compile_default_pipeline(&runtime, "kernel_mlx_rms_norm_rows_bf16")?,
            rope: compile_default_pipeline(&runtime, "kernel_mlx_rope_single_bf16")?,
            attention_logits_seq: compile_default_pipeline(
                &runtime,
                "kernel_mlx_gqa_attention_logits_seq_bf16",
            )?,
            attention_softmax_rows: compile_default_pipeline(
                &runtime,
                "kernel_mlx_softmax_rows_bf16",
            )?,
            attention_weighted_sum: compile_default_pipeline(
                &runtime,
                "kernel_mlx_gqa_attention_weighted_sum_bf16",
            )?,
            o_proj_fast: compile_default_pipeline(&runtime, "kernel_mlx_affine_qmv_fast_row_bf16")?,
            residual: compile_default_pipeline(&runtime, "kernel_mlx_add_row_bf16")?,
            weighted_sum_rows: compile_default_pipeline(
                &runtime,
                "kernel_mlx_weighted_sum_rows_bf16",
            )?,
            geglu: compile_default_pipeline(&runtime, "kernel_mlx_geglu_row_bf16")?,
            geglu_strided: compile_default_pipeline(
                &runtime,
                "kernel_mlx_geglu_strided_rows_bf16",
            )?,
            router_scale_pair: compile_default_pipeline(
                &runtime,
                "kernel_mlx_router_scale_pair_bf16",
            )?,
            router_topk: compile_default_pipeline(&runtime, "kernel_mlx_router_topk_bf16")?,
            selected_expert_proj: compile_default_pipeline(
                &runtime,
                "kernel_mlx_affine_qmv_selected_experts_row_bf16",
            )?,
            scale_row: compile_default_pipeline(&runtime, "kernel_mlx_scale_row_bf16")?,
        };

        Ok(Self {
            qkv_proj,
            q_proj,
            k_proj,
            o_proj,
            mlp_gate_up,
            mlp_gate,
            mlp_down,
            router_proj,
            expert_gate_up,
            expert_gate,
            expert_down,
            post_attention_norm_len,
            pre_feedforward_norm_len,
            pre_feedforward_norm2_len,
            post_feedforward_norm_len,
            post_feedforward_norm1_len,
            post_feedforward_norm2_len,
            q_head_count,
            k_head_count,
            v_head_count,
            q_heads_per_kv,
            head_dim,
            kv_cache_capacity_tokens: cache_spec.max_tokens,
            eps: text_config.rms_norm_eps,
            layer_scalar,
            q_rope: ExactMetalRopeLayout {
                half_dims: rope_half_dims as u32,
                row_stride: head_dim as u32,
                row_count: q_head_count as u32,
                base_log2: rope_base_log2,
            },
            k_rope: ExactMetalRopeLayout {
                half_dims: rope_half_dims as u32,
                row_stride: head_dim as u32,
                row_count: k_head_count as u32,
                base_log2: rope_base_log2,
            },
            buffers,
            weights,
            pipelines,
        })
    }
}

impl ExactMetalTextIoWorkspace {
    fn load(session: &mut LayerExecutionSession) -> Result<Self, Box<dyn Error>> {
        let indexed = session.weights.clone();
        let runtime = session.runtime.clone();
        let embed_weight_entry = indexed.tensor(EMBED_TOKENS_WEIGHT_NAME)?;
        let embed_scales_entry = indexed.tensor(EMBED_TOKENS_SCALES_NAME)?;
        let embed_biases_entry = indexed.tensor(EMBED_TOKENS_BIASES_NAME)?;
        let final_norm_entry = indexed.tensor(FINAL_TEXT_NORM_WEIGHT_NAME)?;
        if embed_weight_entry.shape.len() != 2
            || embed_scales_entry.shape.len() != 2
            || embed_biases_entry.shape.len() != 2
        {
            return Err("exact text IO expects rank-2 embed tensors".into());
        }
        if final_norm_entry.shape.len() != 1 {
            return Err("exact text IO expects rank-1 final norm weight".into());
        }
        let hidden_size = usize::try_from(final_norm_entry.shape[0])?;
        if hidden_size != NORM_LEN {
            return Err(format!(
                "exact text IO hidden size mismatch: got {} expected {}",
                hidden_size, NORM_LEN
            )
            .into());
        }
        if embed_weight_entry.shape[0] != embed_scales_entry.shape[0]
            || embed_weight_entry.shape[0] != embed_biases_entry.shape[0]
            || embed_scales_entry.shape != embed_biases_entry.shape
        {
            return Err("exact text IO embed tensor shape mismatch".into());
        }
        let logits_qproj = ExactMetalQprojLayout {
            weight_words_per_row: u32::try_from(embed_weight_entry.shape[1])?,
            qparams_per_row: u32::try_from(embed_scales_entry.shape[1])?,
            out_rows: u32::try_from(embed_weight_entry.shape[0])?,
        };
        let embed_weight_row_bytes = usize::try_from(
            embed_weight_entry.shape[1]
                .checked_mul(embed_weight_entry.dtype.byte_width())
                .ok_or("exact text IO embed weight row stride overflow")?,
        )?;
        let embed_qparams_row_bytes = usize::try_from(
            embed_scales_entry.shape[1]
                .checked_mul(embed_scales_entry.dtype.byte_width())
                .ok_or("exact text IO embed qparams row stride overflow")?,
        )?;
        let vocab_size = usize::try_from(embed_weight_entry.shape[0])?;

        Ok(Self {
            embed_weight_row_bytes,
            embed_qparams_row_bytes,
            logits_qproj,
            vocab_size,
            eps: indexed.snapshot.config.text_config.rms_norm_eps,
            softcap: Some(indexed.snapshot.config.text_config.final_logit_softcapping)
                .filter(|softcap| *softcap > 0.0),
            buffers: ExactMetalTextIoBuffers {
                standalone_hidden: create_bf16_buffer(
                    &runtime,
                    NORM_LEN,
                    BufferStorageMode::Private,
                )?,
                hidden_scratch: create_bf16_buffer(&runtime, NORM_LEN, BufferStorageMode::Private)?,
                final_norm_out: create_bf16_buffer(&runtime, NORM_LEN, BufferStorageMode::Private)?,
                logits_out: create_bf16_buffer(&runtime, vocab_size, BufferStorageMode::Shared)?,
                argmax_index_out: runtime
                    .create_buffer(size_of::<u32>(), BufferStorageMode::Shared)?,
                generated_token_chunk_out: runtime.create_buffer(
                    DEVICE_GREEDY_DECODE_CHUNK_TOKENS * size_of::<u32>(),
                    BufferStorageMode::Shared,
                )?,
            },
            weights: ExactMetalTextIoWeights {
                embed_weight: session.private_weight_buffer(EMBED_TOKENS_WEIGHT_NAME)?,
                embed_scales: session.private_weight_buffer(EMBED_TOKENS_SCALES_NAME)?,
                embed_biases: session.private_weight_buffer(EMBED_TOKENS_BIASES_NAME)?,
                final_norm_weight: session.private_weight_buffer(FINAL_TEXT_NORM_WEIGHT_NAME)?,
            },
            pipelines: ExactMetalTextIoPipelines {
                dequant_row: compile_default_pipeline(
                    &runtime,
                    "kernel_mlx_affine_dequant_row_bf16",
                )?,
                dequant_row_from_token_buffer: compile_default_pipeline(
                    &runtime,
                    "kernel_mlx_affine_dequant_row_from_token_buffer_bf16",
                )?,
                rms: compile_default_pipeline(&runtime, "kernel_mlx_rms_norm_row_bf16")?,
                logits_proj: compile_default_pipeline(&runtime, "kernel_mlx_affine_qmv_row_bf16")?,
                argmax_softcapped_bf16: compile_default_pipeline(
                    &runtime,
                    "kernel_mlx_argmax_softcapped_bf16_single",
                )?,
            },
        })
    }
}

pub(crate) struct ExactMetalTextRuntimeSession {
    session: LayerExecutionSession,
    kv_layout: GemmaKvCacheLayout,
    kv_caches: Vec<RefCell<ExactMetalKvCache>>,
    kv_append_pipeline: MetalPipeline,
    text_io: ExactMetalTextIoWorkspace,
    layer_workspaces: HashMap<usize, ExactMetalLayerWorkspace>,
}

#[derive(Clone, Debug)]
pub struct ExactMetalStageProfile {
    pub stage_name: &'static str,
    pub elapsed: Duration,
    pub counters: MetalRuntimeCounters,
}

#[derive(Clone, Debug)]
pub struct ExactMetalLayerProfile {
    pub layer_idx: usize,
    pub attention: GemmaAttentionKind,
    pub elapsed: Duration,
    pub counters: MetalRuntimeCounters,
}

#[derive(Clone, Debug)]
pub struct ExactMetalDecodeStepProfile {
    pub prompt_token_count: usize,
    pub first_generated_token_id: u32,
    pub profiled_token_id: u32,
    pub profiled_position: usize,
    pub embed: ExactMetalStageProfile,
    pub layers: Vec<ExactMetalLayerProfile>,
    pub head: ExactMetalStageProfile,
    pub head_stages: Vec<ExactMetalStageProfile>,
    pub predicted_token_id: u32,
}

fn sum_metal_runtime_counters(stages: &[ExactMetalStageProfile]) -> MetalRuntimeCounters {
    let mut out = MetalRuntimeCounters::default();
    for stage in stages {
        out.command_batches_begun = out
            .command_batches_begun
            .saturating_add(stage.counters.command_batches_begun);
        out.command_batches_committed = out
            .command_batches_committed
            .saturating_add(stage.counters.command_batches_committed);
        out.command_buffer_commits = out
            .command_buffer_commits
            .saturating_add(stage.counters.command_buffer_commits);
        out.compute_encoder_starts = out
            .compute_encoder_starts
            .saturating_add(stage.counters.compute_encoder_starts);
        out.compute_encoder_ends = out
            .compute_encoder_ends
            .saturating_add(stage.counters.compute_encoder_ends);
        out.compute_dispatches = out
            .compute_dispatches
            .saturating_add(stage.counters.compute_dispatches);
        out.buffer_barriers = out
            .buffer_barriers
            .saturating_add(stage.counters.buffer_barriers);
        out.blit_copy_calls = out
            .blit_copy_calls
            .saturating_add(stage.counters.blit_copy_calls);
        out.fence_waits = out.fence_waits.saturating_add(stage.counters.fence_waits);
        out.fence_updates = out
            .fence_updates
            .saturating_add(stage.counters.fence_updates);
        out.wait_idle_calls = out
            .wait_idle_calls
            .saturating_add(stage.counters.wait_idle_calls);
        out.completion_wait_calls = out
            .completion_wait_calls
            .saturating_add(stage.counters.completion_wait_calls);
        out.readback_calls = out
            .readback_calls
            .saturating_add(stage.counters.readback_calls);
        out.gpu_elapsed_ns = out
            .gpu_elapsed_ns
            .saturating_add(stage.counters.gpu_elapsed_ns);
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExactMetalGenerationStopReason {
    MaxNewTokens,
    EosToken(u32),
}

pub(crate) struct ExactMetalGenerationCursor {
    backend: Arc<Mutex<ExactMetalTextRuntimeSession>>,
    prompt_token_ids: Arc<[u32]>,
    stop_tokens: BTreeSet<u32>,
    max_new_tokens: Option<usize>,
    processed_prompt_tokens: usize,
    position: usize,
    pending_next: Option<u32>,
    generated_token_ids: Vec<u32>,
    stop_reason: Option<ExactMetalGenerationStopReason>,
}

#[derive(Clone)]
pub(crate) struct ExactMetalGenerationSnapshot {
    pub(crate) generated_token_ids: Arc<[u32]>,
    pub(crate) stop_reason: Option<ExactMetalGenerationStopReason>,
    #[cfg(test)]
    pub(crate) processed_prompt_tokens: usize,
    #[cfg(test)]
    pub(crate) position: usize,
    #[cfg(test)]
    pub(crate) has_pending_next: bool,
}

pub(crate) struct ExactMetalPromptPrefillNode {
    cursor: Arc<Mutex<ExactMetalGenerationCursor>>,
    value: OnceLock<Result<Arc<ExactMetalGenerationSnapshot>, String>>,
}

enum ExactMetalGenerationDependency {
    PromptPrefill(Arc<ExactMetalPromptPrefillNode>),
    Previous(Arc<ExactMetalGenerationStepNode>),
}

pub(crate) struct ExactMetalGenerationStepNode {
    cursor: Arc<Mutex<ExactMetalGenerationCursor>>,
    target_count: usize,
    dependency: ExactMetalGenerationDependency,
    value: OnceLock<Result<Arc<ExactMetalGenerationSnapshot>, String>>,
}

pub(crate) struct ExactMetalGenerationGraph {
    cursor: Arc<Mutex<ExactMetalGenerationCursor>>,
    prompt_prefill: Arc<ExactMetalPromptPrefillNode>,
    step_nodes: Mutex<Vec<Arc<ExactMetalGenerationStepNode>>>,
    final_snapshot: OnceLock<Result<Arc<ExactMetalGenerationSnapshot>, String>>,
    max_new_tokens: Option<usize>,
}

impl ExactMetalTextRuntimeSession {
    pub(crate) fn load(model_path: PathBuf) -> Result<Self, Box<dyn Error>> {
        let model_root = model_root_dir(&model_path)?;
        let weights = Arc::new(MlxIndexedSafetensors::load(&model_root)?);
        Self::load_with_weights(model_path, weights)
    }

    pub(crate) fn load_with_weights(
        model_path: PathBuf,
        weights: Arc<MlxIndexedSafetensors>,
    ) -> Result<Self, Box<dyn Error>> {
        let mut session = LayerExecutionSession::load_with_weights(model_path, weights)?;
        let text_io = ExactMetalTextIoWorkspace::load(&mut session)?;
        let kv_append_pipeline =
            compile_default_pipeline(&session.runtime, "kernel_mlx_kv_append_pair_bf16")?;
        let kv_layout =
            GemmaKvCacheLayout::from_text_config(&session.weights.snapshot.config.text_config, 1)?;
        let mut kv_caches = Vec::with_capacity(kv_layout.cache_specs.len());
        for spec in &kv_layout.cache_specs {
            kv_caches.push(RefCell::new(ExactMetalKvCache::load(
                &session.runtime,
                spec.clone(),
            )?));
        }
        Ok(Self {
            session,
            kv_layout,
            kv_caches,
            kv_append_pipeline,
            text_io,
            layer_workspaces: HashMap::new(),
        })
    }

    pub(crate) fn reset_kv_caches(&mut self) {
        for cache in &self.kv_caches {
            cache.borrow_mut().reset();
        }
    }

    pub(crate) fn reset_runtime_counters(&self) {
        self.session.runtime.reset_counters();
    }

    pub(crate) fn runtime_counters(&self) -> MetalRuntimeCounters {
        self.session.runtime.counters()
    }

    fn profile_runtime_stage<F>(
        &mut self,
        stage_name: &'static str,
        f: F,
    ) -> Result<ExactMetalStageProfile, Box<dyn Error>>
    where
        F: FnOnce(&mut Self) -> Result<(), Box<dyn Error>>,
    {
        let runtime = self.session.runtime.clone();
        runtime.reset_counters();
        let started = Instant::now();
        f(self)?;
        runtime.wait_idle()?;
        Ok(ExactMetalStageProfile {
            stage_name,
            elapsed: started.elapsed(),
            counters: runtime.counters(),
        })
    }

    fn profile_decode_step_from_token_id(
        &mut self,
        token_id: u32,
        position: usize,
        prompt_token_count: usize,
        first_generated_token_id: u32,
    ) -> Result<ExactMetalDecodeStepProfile, Box<dyn Error>> {
        let layer_count = self
            .session
            .weights
            .snapshot
            .config
            .text_config
            .num_hidden_layers as usize;
        let input_buffer = self.token_input_buffer()?;
        let embed = self.profile_runtime_stage("embed", |this| {
            this.dequantize_token_embedding_into_buffer(token_id, &input_buffer)
        })?;

        let hidden_a = self.text_io.buffers.standalone_hidden.clone();
        let hidden_b = self.text_io.buffers.hidden_scratch.clone();
        let mut layers = Vec::with_capacity(layer_count);
        for layer_idx in 0..layer_count {
            let attention = self.kv_layout.cache_specs[layer_idx].attention;
            let (input_hidden_buffer, output_hidden_buffer) = if layer_idx % 2 == 0 {
                (&hidden_a, &hidden_b)
            } else {
                (&hidden_b, &hidden_a)
            };
            let runtime = self.session.runtime.clone();
            runtime.reset_counters();
            let started = Instant::now();
            runtime.begin_command_batch()?;
            let layer_result = self.eval_layer_hidden_state_core(
                layer_idx,
                None,
                Some(input_hidden_buffer),
                Some(output_hidden_buffer),
                position,
                false,
            );
            if let Err(err) = layer_result {
                let _ = runtime.discard_command_batch();
                return Err(err);
            }
            runtime.end_command_batch()?;
            runtime.wait_idle()?;
            layers.push(ExactMetalLayerProfile {
                layer_idx,
                attention,
                elapsed: started.elapsed(),
                counters: runtime.counters(),
            });
        }

        let final_hidden = self.final_hidden_buffer()?;
        let head_stages = vec![
            self.profile_runtime_stage("head.final_norm", |this| {
                this.dispatch_final_text_norm_on_hidden_buffer(&final_hidden)
            })?,
            self.profile_runtime_stage("head.logits_qmv", |this| {
                this.dispatch_logits_projection_from_final_norm()
            })?,
            self.profile_runtime_stage("head.argmax_softcap", |this| {
                this.dispatch_argmax_from_logits()
            })?,
        ];
        let head_elapsed = head_stages
            .iter()
            .fold(Duration::ZERO, |sum, stage| sum + stage.elapsed);
        let head = ExactMetalStageProfile {
            stage_name: "head",
            elapsed: head_elapsed,
            counters: sum_metal_runtime_counters(&head_stages),
        };
        let predicted_token_id = self.read_device_argmax_token_id()?;
        Ok(ExactMetalDecodeStepProfile {
            prompt_token_count,
            first_generated_token_id,
            profiled_token_id: token_id,
            profiled_position: position,
            embed,
            layers,
            head,
            head_stages,
            predicted_token_id,
        })
    }

    fn profile_decode_layers_after_prompt(
        &mut self,
        prompt_token_ids: &[u32],
    ) -> Result<ExactMetalDecodeStepProfile, Box<dyn Error>> {
        let first_generated_token_id =
            self.prefill_prompt_greedy_token_id_from_token_ids(prompt_token_ids, 0)?;
        self.profile_decode_step_from_token_id(
            first_generated_token_id,
            prompt_token_ids.len(),
            prompt_token_ids.len(),
            first_generated_token_id,
        )
    }

    pub(crate) fn generation_cursor(
        backend: Arc<Mutex<Self>>,
        prompt_token_ids: Arc<[u32]>,
        stop_tokens: BTreeSet<u32>,
        max_new_tokens: Option<usize>,
    ) -> Result<ExactMetalGenerationCursor, Box<dyn Error>> {
        if prompt_token_ids.is_empty() {
            return Err("generation requires at least one prompt token".into());
        }
        Ok(ExactMetalGenerationCursor {
            backend,
            prompt_token_ids,
            stop_tokens,
            max_new_tokens,
            processed_prompt_tokens: 0,
            position: 0,
            pending_next: None,
            generated_token_ids: Vec::with_capacity(
                max_new_tokens.unwrap_or(DEVICE_GREEDY_DECODE_CHUNK_TOKENS),
            ),
            stop_reason: None,
        })
    }

    pub(crate) fn generation_graph(
        backend: Arc<Mutex<Self>>,
        prompt_token_ids: Arc<[u32]>,
        stop_tokens: BTreeSet<u32>,
        max_new_tokens: Option<usize>,
    ) -> Result<ExactMetalGenerationGraph, Box<dyn Error>> {
        ExactMetalGenerationGraph::new(Self::generation_cursor(
            backend,
            prompt_token_ids,
            stop_tokens,
            max_new_tokens,
        )?)
    }

    fn kv_cache_for_layer(
        &self,
        layer_idx: usize,
    ) -> Result<RefMut<'_, ExactMetalKvCache>, Box<dyn Error>> {
        let cache_idx = self.kv_layout.cache_idx_for_layer(layer_idx)?;
        self.kv_caches
            .get(cache_idx)
            .ok_or_else(|| format!("missing exact metal KV cache {cache_idx}").into())
            .map(|cache| cache.borrow_mut())
    }

    fn layer_workspace(
        &mut self,
        layer_idx: usize,
    ) -> Result<ExactMetalLayerWorkspace, Box<dyn Error>> {
        if !self.layer_workspaces.contains_key(&layer_idx) {
            let workspace = ExactMetalLayerWorkspace::load(&mut self.session, layer_idx)?;
            self.layer_workspaces.insert(layer_idx, workspace);
        }
        self.layer_workspaces
            .get(&layer_idx)
            .cloned()
            .ok_or_else(|| format!("missing exact metal workspace for layer {layer_idx}").into())
    }

    fn token_input_buffer(&mut self) -> Result<MetalBuffer, Box<dyn Error>> {
        Ok(self.text_io.buffers.standalone_hidden.clone())
    }

    fn final_hidden_buffer(&mut self) -> Result<MetalBuffer, Box<dyn Error>> {
        let layer_count = self
            .session
            .weights
            .snapshot
            .config
            .text_config
            .num_hidden_layers as usize;
        if layer_count == 0 {
            return Ok(self.text_io.buffers.standalone_hidden.clone());
        }
        if layer_count % 2 == 0 {
            Ok(self.text_io.buffers.standalone_hidden.clone())
        } else {
            Ok(self.text_io.buffers.hidden_scratch.clone())
        }
    }

    fn dequantize_token_embedding_into_buffer(
        &mut self,
        token_id: u32,
        dst: &MetalBuffer,
    ) -> Result<(), Box<dyn Error>> {
        let token_idx = usize::try_from(token_id)?;
        if token_idx >= self.text_io.vocab_size {
            return Err(format!(
                "token id {} exceeds exact text IO vocabulary {}",
                token_id, self.text_io.vocab_size
            )
            .into());
        }
        let weight_offset = token_idx
            .checked_mul(self.text_io.embed_weight_row_bytes)
            .ok_or("exact text IO embed weight offset overflow")?;
        let qparams_offset = token_idx
            .checked_mul(self.text_io.embed_qparams_row_bytes)
            .ok_or("exact text IO embed qparams offset overflow")?;
        let runtime = self.session.runtime.clone();
        let owns_command_batch = !runtime.command_batch_is_active();
        let args = MlxAffineDequantRowArgs {
            n: NORM_LEN as u32,
            embed_scale: bf16_round_to_f32(
                ((self.text_io.embed_weight_row_bytes / size_of::<u32>()) as f32).sqrt(),
            ),
        };
        if owns_command_batch {
            runtime.begin_command_batch()?;
        }
        dispatch_compute_tracked_split(
            &runtime,
            &self.text_io.pipelines.dequant_row,
            bytes_of(&args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &self.text_io.weights.embed_weight,
                    offset_bytes: weight_offset,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &self.text_io.weights.embed_scales,
                    offset_bytes: qparams_offset,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &self.text_io.weights.embed_biases,
                    offset_bytes: qparams_offset,
                },
                MetalBufferBindingRef {
                    index: 4,
                    buffer: dst,
                    offset_bytes: 0,
                },
            ],
            3,
            &[],
            MetalSize {
                width: (NORM_LEN as u64).div_ceil(64),
                height: 1,
                depth: 1,
            },
            MetalSize {
                width: 64,
                height: 1,
                depth: 1,
            },
        )?;
        if owns_command_batch {
            runtime.end_command_batch()?;
        }
        Ok(())
    }

    fn dequantize_next_token_embedding_from_device_buffer(
        &mut self,
        dst: &MetalBuffer,
        history_slot: usize,
    ) -> Result<(), Box<dyn Error>> {
        if history_slot >= DEVICE_GREEDY_DECODE_CHUNK_TOKENS {
            return Err(format!(
                "device greedy decode chunk slot {} exceeds capacity {}",
                history_slot, DEVICE_GREEDY_DECODE_CHUNK_TOKENS
            )
            .into());
        }
        let runtime = self.session.runtime.clone();
        let owns_command_batch = !runtime.command_batch_is_active();
        let args = MlxAffineDequantTokenRowArgs {
            n: NORM_LEN as u32,
            embed_scale: bf16_round_to_f32(
                ((self.text_io.embed_weight_row_bytes / size_of::<u32>()) as f32).sqrt(),
            ),
            weight_words_per_row: self.text_io.logits_qproj.weight_words_per_row,
            qparams_per_row: self.text_io.logits_qproj.qparams_per_row,
            vocab_size: self.text_io.vocab_size as u32,
            history_slot: history_slot as u32,
        };
        if owns_command_batch {
            runtime.begin_command_batch()?;
        }
        let dispatch_result = dispatch_compute_tracked_split(
            &runtime,
            &self.text_io.pipelines.dequant_row_from_token_buffer,
            bytes_of(&args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &self.text_io.weights.embed_weight,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &self.text_io.weights.embed_scales,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &self.text_io.weights.embed_biases,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 4,
                    buffer: &self.text_io.buffers.argmax_index_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 5,
                    buffer: dst,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 6,
                    buffer: &self.text_io.buffers.generated_token_chunk_out,
                    offset_bytes: 0,
                },
            ],
            4,
            &[],
            MetalSize {
                width: (NORM_LEN as u64).div_ceil(64),
                height: 1,
                depth: 1,
            },
            MetalSize {
                width: 64,
                height: 1,
                depth: 1,
            },
        );
        if let Err(err) = dispatch_result {
            if owns_command_batch {
                let _ = runtime.discard_command_batch();
            }
            return Err(err);
        }
        if owns_command_batch {
            runtime.end_command_batch()?;
        }
        Ok(())
    }

    fn read_generated_token_chunk(&self, token_count: usize) -> Result<Vec<u32>, Box<dyn Error>> {
        if token_count > DEVICE_GREEDY_DECODE_CHUNK_TOKENS {
            return Err(format!(
                "requested generated token chunk {} exceeds capacity {}",
                token_count, DEVICE_GREEDY_DECODE_CHUNK_TOKENS
            )
            .into());
        }
        let runtime = self.session.runtime.clone();
        runtime
            .with_readable_buffer_range(
                &self.text_io.buffers.generated_token_chunk_out,
                0,
                token_count * size_of::<u32>(),
                |bytes| {
                    let mut token_ids = Vec::with_capacity(token_count);
                    for chunk in bytes.chunks_exact(size_of::<u32>()) {
                        let token_id = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                        let token_idx = usize::try_from(token_id).map_err(|err| {
                            format!("generated token id conversion failed: {err}")
                        })?;
                        if token_idx >= self.text_io.vocab_size {
                            return Err(format!(
                                "exact text IO generated token {} exceeds vocab {}",
                                token_id, self.text_io.vocab_size
                            ));
                        }
                        token_ids.push(token_id);
                    }
                    Ok(token_ids)
                },
            )
            .map_err(|err| err.into())
    }

    fn dispatch_greedy_head_on_hidden_buffer(
        &mut self,
        hidden_buffer: &MetalBuffer,
    ) -> Result<(), Box<dyn Error>> {
        let runtime = self.session.runtime.clone();
        let owns_command_batch = !runtime.command_batch_is_active();
        if owns_command_batch {
            runtime.begin_command_batch()?;
        }
        self.dispatch_final_text_norm_on_hidden_buffer(hidden_buffer)?;
        self.dispatch_logits_projection_from_final_norm()?;
        self.dispatch_argmax_from_logits()?;
        if owns_command_batch {
            runtime.end_command_batch()?;
        }
        Ok(())
    }

    fn dispatch_final_text_norm_on_hidden_buffer(
        &mut self,
        hidden_buffer: &MetalBuffer,
    ) -> Result<(), Box<dyn Error>> {
        let runtime = self.session.runtime.clone();
        let owns_command_batch = !runtime.command_batch_is_active();
        let n_reads = 4usize;
        let simd_size = 32usize;
        let rms_threadgroup_size = simd_size * NORM_LEN.div_ceil(n_reads).div_ceil(simd_size);
        let rms_args = MlxRmsNormRowArgs {
            n: NORM_LEN as u32,
            eps: self.text_io.eps,
        };
        if owns_command_batch {
            runtime.begin_command_batch()?;
        }
        dispatch_compute_tracked_split(
            &runtime,
            &self.text_io.pipelines.rms,
            bytes_of(&rms_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: hidden_buffer,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &self.text_io.weights.final_norm_weight,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &self.text_io.buffers.final_norm_out,
                    offset_bytes: 0,
                },
            ],
            2,
            &[],
            MetalSize {
                width: 1,
                height: 1,
                depth: 1,
            },
            MetalSize {
                width: rms_threadgroup_size as u64,
                height: 1,
                depth: 1,
            },
        )?;
        if owns_command_batch {
            runtime.end_command_batch()?;
        }
        Ok(())
    }

    fn dispatch_logits_projection_from_final_norm(&mut self) -> Result<(), Box<dyn Error>> {
        let runtime = self.session.runtime.clone();
        let owns_command_batch = !runtime.command_batch_is_active();
        let logits_args = self.text_io.logits_qproj.row_args(NORM_LEN as u32);
        if owns_command_batch {
            runtime.begin_command_batch()?;
        }
        dispatch_compute_tracked_split(
            &runtime,
            &self.text_io.pipelines.logits_proj,
            bytes_of(&logits_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &self.text_io.buffers.final_norm_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &self.text_io.weights.embed_weight,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &self.text_io.weights.embed_scales,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 4,
                    buffer: &self.text_io.weights.embed_biases,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 5,
                    buffer: &self.text_io.buffers.logits_out,
                    offset_bytes: 0,
                },
            ],
            4,
            &[],
            MetalSize {
                width: 1,
                height: (self.text_io.vocab_size as u64).div_ceil(8),
                depth: 1,
            },
            MetalSize {
                width: 32,
                height: 2,
                depth: 1,
            },
        )?;
        if owns_command_batch {
            runtime.end_command_batch()?;
        }
        Ok(())
    }

    fn dispatch_argmax_from_logits(&mut self) -> Result<(), Box<dyn Error>> {
        let runtime = self.session.runtime.clone();
        let owns_command_batch = !runtime.command_batch_is_active();
        let argmax_args = MlxArgmaxSoftcappedBf16Args {
            n: self.text_io.vocab_size as u32,
            softcap: self.text_io.softcap.unwrap_or(0.0),
            has_softcap: u32::from(self.text_io.softcap.is_some()),
        };
        if owns_command_batch {
            runtime.begin_command_batch()?;
        }
        dispatch_compute_tracked_split(
            &runtime,
            &self.text_io.pipelines.argmax_softcapped_bf16,
            bytes_of(&argmax_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &self.text_io.buffers.logits_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &self.text_io.buffers.argmax_index_out,
                    offset_bytes: 0,
                },
            ],
            1,
            &[],
            MetalSize {
                width: 1,
                height: 1,
                depth: 1,
            },
            MetalSize {
                width: 1,
                height: 1,
                depth: 1,
            },
        )?;
        if owns_command_batch {
            runtime.end_command_batch()?;
        }
        Ok(())
    }

    fn sampled_token_from_logits(
        &mut self,
        disallowed_token_ids: &[u32],
        sampling_options: &GemmaTextSamplingOptions,
        rng: &mut MlxTextSamplingRng,
    ) -> Result<MlxGreedyToken, Box<dyn Error>> {
        self.read_shared_logits_sampled_token(disallowed_token_ids, sampling_options, rng)
    }

    fn read_device_argmax_token_id(&self) -> Result<u32, Box<dyn Error>> {
        let runtime = self.session.runtime.clone();
        let token_id = runtime.with_readable_buffer(
            &self.text_io.buffers.argmax_index_out,
            size_of::<u32>(),
            |bytes| {
                if bytes.len() != size_of::<u32>() {
                    return Err(format!(
                        "exact text IO argmax byte length mismatch: {}",
                        bytes.len()
                    ));
                }
                Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
            },
        )?;
        let token_idx = usize::try_from(token_id)?;
        if token_idx >= self.text_io.vocab_size {
            return Err(format!(
                "exact text IO argmax token {} exceeds vocab {}",
                token_id, self.text_io.vocab_size
            )
            .into());
        }
        Ok(token_id)
    }

    fn read_device_greedy_token(&self) -> Result<MlxGreedyToken, Box<dyn Error>> {
        let runtime = self.session.runtime.clone();
        let token_id = self.read_device_argmax_token_id()?;
        let token_idx = usize::try_from(token_id)?;
        let raw_logit = runtime.with_readable_buffer_range(
            &self.text_io.buffers.logits_out,
            token_idx * size_of::<u16>(),
            size_of::<u16>(),
            |bytes| {
                if bytes.len() != size_of::<u16>() {
                    return Err(format!(
                        "exact text IO bf16 logit byte length mismatch: {}",
                        bytes.len()
                    ));
                }
                Ok(bf16_word_to_f32(u16::from_le_bytes([bytes[0], bytes[1]])))
            },
        )?;
        let logit = if let Some(softcap) = self.text_io.softcap {
            bf16_round_to_f32((raw_logit / softcap).tanh() * softcap)
        } else {
            raw_logit
        };
        Ok(MlxGreedyToken { token_id, logit })
    }

    fn read_hidden_words_from_buffer(
        &self,
        hidden_buffer: &MetalBuffer,
    ) -> Result<Vec<u16>, Box<dyn Error>> {
        Ok(bf16_words_from_f32_bits(&read_bf16_buffer_bits(
            &self.session.runtime,
            hidden_buffer,
            NORM_LEN,
        )?))
    }

    fn read_shared_logits_greedy_token(&self) -> Result<MlxGreedyToken, Box<dyn Error>> {
        let runtime = self.session.runtime.clone();
        Ok(runtime.with_readable_buffer(
            &self.text_io.buffers.logits_out,
            self.text_io.vocab_size * size_of::<u16>(),
            |bytes| {
                if bytes.len() != self.text_io.vocab_size * size_of::<u16>() {
                    return Err(format!(
                        "exact text IO logits byte length mismatch: {}",
                        bytes.len()
                    ));
                }
                let mut best_token_id = 0u32;
                let mut best_logit = f32::NEG_INFINITY;
                for (token_idx, word_bytes) in bytes.chunks_exact(size_of::<u16>()).enumerate() {
                    let raw_logit =
                        bf16_word_to_f32(u16::from_le_bytes([word_bytes[0], word_bytes[1]]));
                    let logit = if let Some(softcap) = self.text_io.softcap {
                        bf16_round_to_f32((raw_logit / softcap).tanh() * softcap)
                    } else {
                        raw_logit
                    };
                    let token_id = token_idx as u32;
                    if logit > best_logit || (logit == best_logit && token_id < best_token_id) {
                        best_logit = logit;
                        best_token_id = token_id;
                    }
                }
                Ok(MlxGreedyToken {
                    token_id: best_token_id,
                    logit: best_logit,
                })
            },
        )?)
    }

    fn read_shared_logits_softcapped(&self) -> Result<Vec<f32>, Box<dyn Error>> {
        let runtime = self.session.runtime.clone();
        Ok(runtime.with_readable_buffer(
            &self.text_io.buffers.logits_out,
            self.text_io.vocab_size * size_of::<u16>(),
            |bytes| {
                if bytes.len() != self.text_io.vocab_size * size_of::<u16>() {
                    return Err(format!(
                        "exact text IO logits byte length mismatch: {}",
                        bytes.len()
                    ));
                }
                let mut logits = Vec::with_capacity(self.text_io.vocab_size);
                for word_bytes in bytes.chunks_exact(size_of::<u16>()) {
                    let raw_logit =
                        bf16_word_to_f32(u16::from_le_bytes([word_bytes[0], word_bytes[1]]));
                    let logit = if let Some(softcap) = self.text_io.softcap {
                        bf16_round_to_f32((raw_logit / softcap).tanh() * softcap)
                    } else {
                        raw_logit
                    };
                    logits.push(logit);
                }
                Ok(logits)
            },
        )?)
    }

    fn read_shared_logits_sampled_token(
        &self,
        disallowed_token_ids: &[u32],
        sampling_options: &GemmaTextSamplingOptions,
        rng: &mut MlxTextSamplingRng,
    ) -> Result<MlxGreedyToken, Box<dyn Error>> {
        let runtime = self.session.runtime.clone();
        Ok(runtime.with_readable_buffer(
            &self.text_io.buffers.logits_out,
            self.text_io.vocab_size * size_of::<u16>(),
            |bytes| {
                sample_token_from_softcapped_bf16_bytes(
                    bytes,
                    self.text_io.softcap,
                    disallowed_token_ids,
                    sampling_options,
                    rng,
                )
            },
        )?)
    }

    fn greedy_token_from_hidden_buffer(
        &mut self,
        hidden_buffer: &MetalBuffer,
    ) -> Result<MlxGreedyToken, Box<dyn Error>> {
        self.dispatch_final_text_norm_on_hidden_buffer(hidden_buffer)?;
        self.dispatch_logits_projection_from_final_norm()?;
        self.read_shared_logits_greedy_token()
    }

    fn eval_layer_hidden_state_core(
        &mut self,
        layer_idx: usize,
        input_words: Option<&[u16]>,
        input_hidden_buffer: Option<&MetalBuffer>,
        output_hidden_buffer: Option<&MetalBuffer>,
        position: usize,
        read_output: bool,
    ) -> Result<Option<Vec<u16>>, Box<dyn Error>> {
        if let Some(input_words) = input_words {
            if input_words.len() != NORM_LEN {
                return Err(format!(
                    "exact metal layer input length mismatch: got {} expected {}",
                    input_words.len(),
                    NORM_LEN
                )
                .into());
            }
        }

        let runtime = self.session.runtime.clone();
        let workspace = self.layer_workspace(layer_idx)?;
        let input_hidden_buffer = input_hidden_buffer.unwrap_or(&workspace.buffers.x);
        let output_hidden_buffer =
            output_hidden_buffer.unwrap_or(&workspace.buffers.post_ffn_residual_out);
        let owns_command_batch = !runtime.command_batch_is_active();
        if read_output && !owns_command_batch {
            return Err(
                "cannot read exact layer output while Metal command batch is active".into(),
            );
        }

        let n_reads = 4usize;
        let simd_size = 32usize;
        let rms_threadgroup_size = simd_size * NORM_LEN.div_ceil(n_reads).div_ceil(simd_size);
        let head_norm_threadgroup_size =
            simd_size * workspace.head_dim.div_ceil(n_reads).div_ceil(simd_size);
        let rms_threadgroups = MetalSize {
            width: 1,
            height: 1,
            depth: 1,
        };
        let rms_threads_per_threadgroup = MetalSize {
            width: rms_threadgroup_size as u64,
            height: 1,
            depth: 1,
        };
        let proj_threads_per_threadgroup = MetalSize {
            width: 32,
            height: 2,
            depth: 1,
        };
        let qkv_proj_threadgroups = MetalSize {
            width: 1,
            height: (workspace.qkv_proj.out_len() as u64).div_ceil(8),
            depth: 1,
        };
        let o_proj_threadgroups = MetalSize {
            width: 1,
            height: (workspace.o_proj.out_len() as u64).div_ceil(8),
            depth: 1,
        };
        let q_head_norm_threadgroups = MetalSize {
            width: workspace.q_head_count as u64,
            height: 1,
            depth: 1,
        };
        let k_head_norm_threadgroups = MetalSize {
            width: workspace.k_head_count as u64,
            height: 1,
            depth: 1,
        };
        let v_head_norm_threadgroups = MetalSize {
            width: workspace.v_head_count as u64,
            height: 1,
            depth: 1,
        };
        let head_norm_threads_per_threadgroup = MetalSize {
            width: head_norm_threadgroup_size as u64,
            height: 1,
            depth: 1,
        };
        let q_rope_threadgroups = MetalSize {
            width: (workspace.head_dim as u64).div_ceil(32),
            height: workspace.q_head_count as u64,
            depth: 1,
        };
        let k_rope_threadgroups = MetalSize {
            width: (workspace.head_dim as u64).div_ceil(32),
            height: workspace.k_head_count as u64,
            depth: 1,
        };
        let rope_threads_per_threadgroup = MetalSize {
            width: 32,
            height: 1,
            depth: 1,
        };
        let attention_output_threadgroups = MetalSize {
            width: (workspace.head_dim as u64).div_ceil(64),
            height: 1,
            depth: workspace.q_head_count as u64,
        };
        let attention_logits_threads_per_threadgroup = MetalSize {
            width: 32,
            height: 1,
            depth: 1,
        };
        let attention_output_threads_per_threadgroup = MetalSize {
            width: 32,
            height: 4,
            depth: 1,
        };
        let residual_threads_per_threadgroup = MetalSize {
            width: 256,
            height: 1,
            depth: 1,
        };
        let residual_threadgroups = MetalSize {
            width: (workspace.post_attention_norm_len as u64)
                .div_ceil(residual_threads_per_threadgroup.width),
            height: 1,
            depth: 1,
        };
        let mlp_gate_up_threadgroups = MetalSize {
            width: 1,
            height: (workspace.mlp_gate_up.out_len() as u64).div_ceil(8),
            depth: 1,
        };
        let mlp_down_threadgroups = MetalSize {
            width: 1,
            height: (workspace.mlp_down.out_len() as u64).div_ceil(8),
            depth: 1,
        };
        let geglu_threads_per_threadgroup = MetalSize {
            width: 256,
            height: 1,
            depth: 1,
        };
        let geglu_threadgroups = MetalSize {
            width: (workspace.mlp_gate.out_len() as u64)
                .div_ceil(geglu_threads_per_threadgroup.width),
            height: 1,
            depth: 1,
        };
        let router_scale_threads_per_threadgroup = MetalSize {
            width: rms_threadgroup_size as u64,
            height: 1,
            depth: 1,
        };
        let router_scale_threadgroups = MetalSize {
            width: 1,
            height: 1,
            depth: 1,
        };
        let router_proj_threadgroups = MetalSize {
            width: 1,
            height: (workspace.router_proj.out_len() as u64).div_ceil(8),
            depth: 1,
        };
        let router_softmax_threadgroups = MetalSize {
            width: 1,
            height: 1,
            depth: 1,
        };
        let router_topk_threadgroups = MetalSize {
            width: 1,
            height: 1,
            depth: 1,
        };
        let router_topk_threads_per_threadgroup = MetalSize {
            width: 1,
            height: 1,
            depth: 1,
        };
        let selected_expert_threadgroups = MetalSize {
            width: ROUTER_TOP_K as u64,
            height: (workspace.expert_gate_up.out_len() as u64).div_ceil(8),
            depth: 1,
        };
        let selected_expert_down_threadgroups = MetalSize {
            width: ROUTER_TOP_K as u64,
            height: (workspace.expert_down.out_len() as u64).div_ceil(8),
            depth: 1,
        };
        let selected_expert_threads_per_threadgroup = MetalSize {
            width: 32,
            height: 2,
            depth: 1,
        };
        let expert_geglu_threadgroups = MetalSize {
            width: ((ROUTER_TOP_K * workspace.expert_gate.out_len()) as u64)
                .div_ceil(geglu_threads_per_threadgroup.width),
            height: 1,
            depth: 1,
        };

        let rms_args = MlxRmsNormRowArgs {
            n: NORM_LEN as u32,
            eps: workspace.eps,
        };
        let qkv_proj_args = workspace.qkv_proj.row_args(NORM_LEN as u32);
        let o_proj_args = workspace.o_proj.row_args(workspace.q_proj.out_rows);
        let q_head_norm_args = MlxRmsNormRowsArgs {
            n: workspace.head_dim as u32,
            row_stride: workspace.head_dim as u32,
            row_count: workspace.q_head_count as u32,
            eps: workspace.eps,
        };
        let k_head_norm_args = MlxRmsNormRowsArgs {
            n: workspace.head_dim as u32,
            row_stride: workspace.head_dim as u32,
            row_count: workspace.k_head_count as u32,
            eps: workspace.eps,
        };
        let v_head_norm_args = MlxRmsNormRowsArgs {
            n: workspace.head_dim as u32,
            row_stride: workspace.head_dim as u32,
            row_count: workspace.v_head_count as u32,
            eps: workspace.eps,
        };
        let q_rope_args = workspace.q_rope.args(position)?;
        let k_rope_args = workspace.k_rope.args(position)?;
        let residual_args = MlxAddRowArgs {
            n: workspace.post_attention_norm_len as u32,
        };
        let scale_args = workspace.layer_scalar.map(|scale| MlxScaleRowArgs {
            n: workspace.post_feedforward_norm1_len as u32,
            scale,
        });
        let q_proj_offset_bytes = 0usize;
        let k_proj_offset_bytes = workspace
            .q_proj
            .out_len()
            .checked_mul(size_of::<u16>())
            .ok_or("q projection offset overflow")?;
        let v_proj_offset_bytes = workspace
            .q_proj
            .out_len()
            .checked_add(workspace.k_proj.out_len())
            .and_then(|value| value.checked_mul(size_of::<u16>()))
            .ok_or("v projection offset overflow")?;
        let post_attention_norm_args = MlxRmsNormRowArgs {
            n: workspace.post_attention_norm_len as u32,
            eps: workspace.eps,
        };
        let pre_ffn_norm_args = MlxRmsNormRowArgs {
            n: workspace.pre_feedforward_norm_len as u32,
            eps: workspace.eps,
        };
        let mlp_gate_up_args = workspace
            .mlp_gate_up
            .row_args(workspace.pre_feedforward_norm_len as u32);
        let geglu_args = MlxGegluRowArgs {
            n: workspace.mlp_gate.out_rows,
        };
        let mlp_down_args = workspace.mlp_down.row_args(workspace.mlp_gate.out_rows);
        let mlp_gate_up_split_offset_bytes = workspace
            .mlp_gate
            .out_len()
            .checked_mul(size_of::<u16>())
            .ok_or("mlp gate/up split offset overflow")?;
        let router_scale_args = MlxRouterScaleArgs {
            n: workspace.post_attention_norm_len as u32,
            eps: workspace.eps,
            root_size: bf16_round_to_f32((workspace.post_attention_norm_len as f32).powf(-0.5)),
        };
        let router_proj_args = workspace
            .router_proj
            .row_args(workspace.post_attention_norm_len as u32);
        let router_softmax_args = MlxSoftmaxRowsArgs {
            row_stride: workspace.router_proj.out_rows,
            row_count: 1,
            seq_len: workspace.router_proj.out_rows,
        };
        let router_topk_args = MlxRouterTopKArgs {
            expert_count: workspace.router_proj.out_rows,
            top_k: ROUTER_TOP_K as u32,
        };
        let expert_gate_args = workspace
            .expert_gate_up
            .selected_experts_args(workspace.pre_feedforward_norm2_len as u32, 0);
        let expert_geglu_args = MlxGegluStridedRowsArgs {
            n: (ROUTER_TOP_K * workspace.expert_gate.out_len()) as u32,
            row_width: workspace.expert_gate.out_rows,
            input_row_stride: workspace.expert_gate_up.out_rows,
            input_split_offset: workspace.expert_gate.out_rows,
        };
        let expert_down_args = workspace.expert_down.selected_experts_args(
            workspace.expert_gate.out_rows,
            workspace.expert_gate.out_rows,
        );
        let moe_weighted_args = MlxWeightedRowsArgs {
            n: workspace.expert_down.out_rows,
            row_stride: workspace.expert_down.out_rows,
            row_count: ROUTER_TOP_K as u32,
        };
        let post_ffn_norm1_args = MlxRmsNormRowArgs {
            n: workspace.post_feedforward_norm1_len as u32,
            eps: workspace.eps,
        };
        let post_ffn_norm_args = MlxRmsNormRowArgs {
            n: workspace.post_feedforward_norm_len as u32,
            eps: workspace.eps,
        };
        let post_ffn_norm2_args = MlxRmsNormRowArgs {
            n: workspace.post_feedforward_norm2_len as u32,
            eps: workspace.eps,
        };

        if let Some(input_words) = input_words {
            runtime.write_buffer(input_hidden_buffer, 0, &bytes_from_bf16_words(input_words))?;
        }
        if owns_command_batch {
            runtime.begin_command_batch()?;
        }
        dispatch_compute_tracked_split(
            &runtime,
            &workspace.pipelines.rms,
            bytes_of(&rms_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: input_hidden_buffer,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.weights.input_norm_weight,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &workspace.buffers.h,
                    offset_bytes: 0,
                },
            ],
            2,
            &[],
            rms_threadgroups,
            rms_threads_per_threadgroup,
        )?;
        dispatch_exact_mlx_qmv_row(
            &runtime,
            &workspace.pipelines.proj,
            &workspace.pipelines.proj_fast,
            workspace.qkv_proj,
            &qkv_proj_args,
            &[
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.h,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.weights.qkv_proj_weight,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &workspace.weights.qkv_proj_scales,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 4,
                    buffer: &workspace.weights.qkv_proj_biases,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 5,
                    buffer: &workspace.buffers.qkv_proj_out,
                    offset_bytes: 0,
                },
            ],
            qkv_proj_threadgroups,
            proj_threads_per_threadgroup,
        )?;
        dispatch_compute_tracked_split(
            &runtime,
            &workspace.pipelines.head_norm,
            bytes_of(&q_head_norm_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.qkv_proj_out,
                    offset_bytes: q_proj_offset_bytes,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.weights.q_norm_weight,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &workspace.buffers.q_norm,
                    offset_bytes: 0,
                },
            ],
            2,
            &[],
            q_head_norm_threadgroups,
            head_norm_threads_per_threadgroup,
        )?;
        dispatch_compute_tracked_split(
            &runtime,
            &workspace.pipelines.head_norm,
            bytes_of(&k_head_norm_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.qkv_proj_out,
                    offset_bytes: k_proj_offset_bytes,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.weights.k_norm_weight,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &workspace.buffers.k_norm,
                    offset_bytes: 0,
                },
            ],
            2,
            &[],
            k_head_norm_threadgroups,
            head_norm_threads_per_threadgroup,
        )?;
        dispatch_compute_tracked_split(
            &runtime,
            &workspace.pipelines.head_norm,
            bytes_of(&v_head_norm_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.qkv_proj_out,
                    offset_bytes: v_proj_offset_bytes,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.weights.v_norm_weight,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &workspace.buffers.v_norm,
                    offset_bytes: 0,
                },
            ],
            2,
            &[],
            v_head_norm_threadgroups,
            head_norm_threads_per_threadgroup,
        )?;
        dispatch_compute_tracked_split(
            &runtime,
            &workspace.pipelines.rope,
            bytes_of(&q_rope_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.q_norm,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.buffers.q_rope,
                    offset_bytes: 0,
                },
            ],
            1,
            &[],
            q_rope_threadgroups,
            rope_threads_per_threadgroup,
        )?;
        dispatch_compute_tracked_split(
            &runtime,
            &workspace.pipelines.rope,
            bytes_of(&k_rope_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.k_norm,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.buffers.k_rope,
                    offset_bytes: 0,
                },
            ],
            1,
            &[],
            k_rope_threadgroups,
            rope_threads_per_threadgroup,
        )?;

        let (
            attention_seq_len,
            attention_start_slot,
            attention_kv_row_stride,
            attention_key_buffer,
            attention_value_buffer,
        ) = {
            let mut layer_cache = self.kv_cache_for_layer(layer_idx)?;
            layer_cache.append_token_from_buffers_compute(
                &runtime,
                &self.kv_append_pipeline,
                &workspace.buffers.k_rope,
                &workspace.buffers.v_norm,
            )?;
            (
                layer_cache.seq_len(),
                layer_cache.start_slot(),
                layer_cache.row_stride_words()?,
                layer_cache.key_buffer.clone(),
                layer_cache.value_buffer.clone(),
            )
        };
        let attention_logits_args = MlxGqaAttentionLogitsSeqArgs {
            head_dim: workspace.head_dim as u32,
            q_head_stride: workspace.head_dim as u32,
            kv_row_stride: attention_kv_row_stride as u32,
            q_head_count: workspace.q_head_count as u32,
            q_heads_per_kv: workspace.q_heads_per_kv as u32,
            seq_len: attention_seq_len as u32,
            start_slot: attention_start_slot as u32,
            capacity: workspace.kv_cache_capacity_tokens as u32,
        };
        let attention_softmax_args = MlxSoftmaxRowsArgs {
            row_stride: workspace.kv_cache_capacity_tokens as u32,
            row_count: workspace.q_head_count as u32,
            seq_len: attention_seq_len as u32,
        };
        let attention_logits_threadgroups = MetalSize {
            width: attention_seq_len as u64,
            height: workspace.q_head_count as u64,
            depth: 1,
        };
        let attention_weighted_sum_args = MlxGqaAttentionWeightedSumArgs {
            probs_row_stride: workspace.kv_cache_capacity_tokens as u32,
            head_dim: workspace.head_dim as u32,
            kv_row_stride: attention_kv_row_stride as u32,
            out_head_stride: workspace.head_dim as u32,
            q_head_count: workspace.q_head_count as u32,
            q_heads_per_kv: workspace.q_heads_per_kv as u32,
            seq_len: attention_seq_len as u32,
            start_slot: attention_start_slot as u32,
            capacity: workspace.kv_cache_capacity_tokens as u32,
        };

        dispatch_compute_tracked_split(
            &runtime,
            &workspace.pipelines.attention_logits_seq,
            bytes_of(&attention_logits_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.q_rope,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &attention_key_buffer,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &workspace.buffers.attention_logits,
                    offset_bytes: 0,
                },
            ],
            2,
            &[],
            attention_logits_threadgroups,
            attention_logits_threads_per_threadgroup,
        )?;
        dispatch_compute_tracked_split(
            &runtime,
            &workspace.pipelines.attention_softmax_rows,
            bytes_of(&attention_softmax_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.attention_logits,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.buffers.attention_probs,
                    offset_bytes: 0,
                },
            ],
            1,
            &[],
            MetalSize {
                width: workspace.q_head_count as u64,
                height: 1,
                depth: 1,
            },
            mlx_softmax_threads_per_threadgroup(
                attention_seq_len,
                workspace
                    .pipelines
                    .attention_softmax_rows
                    .max_threads_per_threadgroup,
            )?,
        )?;
        dispatch_compute_tracked_split(
            &runtime,
            &workspace.pipelines.attention_weighted_sum,
            bytes_of(&attention_weighted_sum_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.attention_probs,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &attention_value_buffer,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &workspace.buffers.attn_out,
                    offset_bytes: 0,
                },
            ],
            2,
            &[],
            attention_output_threadgroups,
            attention_output_threads_per_threadgroup,
        )?;
        dispatch_exact_mlx_qmv_row(
            &runtime,
            &workspace.pipelines.proj,
            &workspace.pipelines.proj_fast,
            workspace.o_proj,
            &o_proj_args,
            &[
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.attn_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.weights.o_weight,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &workspace.weights.o_scales,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 4,
                    buffer: &workspace.weights.o_biases,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 5,
                    buffer: &workspace.buffers.o_proj_out,
                    offset_bytes: 0,
                },
            ],
            o_proj_threadgroups,
            proj_threads_per_threadgroup,
        )?;
        dispatch_compute_tracked_split(
            &runtime,
            &workspace.pipelines.rms,
            bytes_of(&post_attention_norm_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.o_proj_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.weights.post_attention_norm_weight,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &workspace.buffers.post_attention_norm_out,
                    offset_bytes: 0,
                },
            ],
            2,
            &[],
            rms_threadgroups,
            rms_threads_per_threadgroup,
        )?;
        dispatch_compute_tracked_split(
            &runtime,
            &workspace.pipelines.residual,
            bytes_of(&residual_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: input_hidden_buffer,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.buffers.post_attention_norm_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &workspace.buffers.residual_out,
                    offset_bytes: 0,
                },
            ],
            2,
            &[],
            residual_threadgroups,
            residual_threads_per_threadgroup,
        )?;
        dispatch_compute_tracked_split(
            &runtime,
            &workspace.pipelines.rms,
            bytes_of(&pre_ffn_norm_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.residual_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.weights.pre_feedforward_norm_weight,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &workspace.buffers.pre_feedforward_norm_out,
                    offset_bytes: 0,
                },
            ],
            2,
            &[],
            rms_threadgroups,
            rms_threads_per_threadgroup,
        )?;
        dispatch_exact_mlx_qmv_row(
            &runtime,
            &workspace.pipelines.proj,
            &workspace.pipelines.proj_fast,
            workspace.mlp_gate_up,
            &mlp_gate_up_args,
            &[
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.pre_feedforward_norm_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.weights.mlp_gate_up_weight,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &workspace.weights.mlp_gate_up_scales,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 4,
                    buffer: &workspace.weights.mlp_gate_up_biases,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 5,
                    buffer: &workspace.buffers.mlp_gate_up_out,
                    offset_bytes: 0,
                },
            ],
            mlp_gate_up_threadgroups,
            proj_threads_per_threadgroup,
        )?;
        dispatch_compute_tracked_split(
            &runtime,
            &workspace.pipelines.geglu,
            bytes_of(&geglu_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.mlp_gate_up_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.buffers.mlp_gate_up_out,
                    offset_bytes: mlp_gate_up_split_offset_bytes,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &workspace.buffers.geglu_out,
                    offset_bytes: 0,
                },
            ],
            2,
            &[],
            geglu_threadgroups,
            geglu_threads_per_threadgroup,
        )?;
        dispatch_exact_mlx_qmv_row(
            &runtime,
            &workspace.pipelines.proj,
            &workspace.pipelines.proj_fast,
            workspace.mlp_down,
            &mlp_down_args,
            &[
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.geglu_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.weights.mlp_down_weight,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &workspace.weights.mlp_down_scales,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 4,
                    buffer: &workspace.weights.mlp_down_biases,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 5,
                    buffer: &workspace.buffers.mlp_down_out,
                    offset_bytes: 0,
                },
            ],
            mlp_down_threadgroups,
            proj_threads_per_threadgroup,
        )?;
        dispatch_compute_tracked_split(
            &runtime,
            &workspace.pipelines.rms,
            bytes_of(&post_ffn_norm1_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.mlp_down_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.weights.post_feedforward_norm1_weight,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &workspace.buffers.post_feedforward_norm1_out,
                    offset_bytes: 0,
                },
            ],
            2,
            &[],
            rms_threadgroups,
            rms_threads_per_threadgroup,
        )?;
        dispatch_compute_tracked_split(
            &runtime,
            &workspace.pipelines.router_scale_pair,
            bytes_of(&router_scale_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.residual_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.weights.router_scale_weight,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &workspace.weights.pre_feedforward_norm2_weight,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 4,
                    buffer: &workspace.buffers.router_scaled_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 5,
                    buffer: &workspace.buffers.pre_feedforward_norm2_out,
                    offset_bytes: 0,
                },
            ],
            3,
            &[],
            router_scale_threadgroups,
            router_scale_threads_per_threadgroup,
        )?;
        dispatch_exact_mlx_qmv_row(
            &runtime,
            &workspace.pipelines.proj,
            &workspace.pipelines.proj_fast,
            workspace.router_proj,
            &router_proj_args,
            &[
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.router_scaled_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.weights.router_proj_weight,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &workspace.weights.router_proj_scales,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 4,
                    buffer: &workspace.weights.router_proj_biases,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 5,
                    buffer: &workspace.buffers.router_proj_out,
                    offset_bytes: 0,
                },
            ],
            router_proj_threadgroups,
            proj_threads_per_threadgroup,
        )?;
        dispatch_compute_tracked_split(
            &runtime,
            &workspace.pipelines.attention_softmax_rows,
            bytes_of(&router_softmax_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.router_proj_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.buffers.router_probs_out,
                    offset_bytes: 0,
                },
            ],
            1,
            &[],
            router_softmax_threadgroups,
            mlx_softmax_threads_per_threadgroup(
                workspace.router_proj.out_rows as usize,
                workspace
                    .pipelines
                    .attention_softmax_rows
                    .max_threads_per_threadgroup,
            )?,
        )?;
        dispatch_compute_tracked_split(
            &runtime,
            &workspace.pipelines.router_topk,
            bytes_of(&router_topk_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.router_proj_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.buffers.router_probs_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &workspace.weights.router_per_expert_scale,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 4,
                    buffer: &workspace.buffers.moe_top_k_indices,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 5,
                    buffer: &workspace.buffers.moe_top_k_weights,
                    offset_bytes: 0,
                },
            ],
            3,
            &[],
            router_topk_threadgroups,
            router_topk_threads_per_threadgroup,
        )?;
        dispatch_compute_tracked_split(
            &runtime,
            &workspace.pipelines.selected_expert_proj,
            bytes_of(&expert_gate_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.pre_feedforward_norm2_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.buffers.moe_top_k_indices,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &workspace.weights.expert_gate_up_weight,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 4,
                    buffer: &workspace.weights.expert_gate_up_scales,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 5,
                    buffer: &workspace.weights.expert_gate_up_biases,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 6,
                    buffer: &workspace.buffers.expert_gate_up_out,
                    offset_bytes: 0,
                },
            ],
            5,
            &[],
            selected_expert_threadgroups,
            selected_expert_threads_per_threadgroup,
        )?;
        dispatch_compute_tracked_split(
            &runtime,
            &workspace.pipelines.geglu_strided,
            bytes_of(&expert_geglu_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.expert_gate_up_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.buffers.expert_geglu_out,
                    offset_bytes: 0,
                },
            ],
            1,
            &[],
            expert_geglu_threadgroups,
            geglu_threads_per_threadgroup,
        )?;
        dispatch_compute_tracked_split(
            &runtime,
            &workspace.pipelines.selected_expert_proj,
            bytes_of(&expert_down_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.expert_geglu_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.buffers.moe_top_k_indices,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &workspace.weights.expert_down_weight,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 4,
                    buffer: &workspace.weights.expert_down_scales,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 5,
                    buffer: &workspace.weights.expert_down_biases,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 6,
                    buffer: &workspace.buffers.expert_down_out,
                    offset_bytes: 0,
                },
            ],
            5,
            &[],
            selected_expert_down_threadgroups,
            selected_expert_threads_per_threadgroup,
        )?;
        dispatch_compute_tracked_split(
            &runtime,
            &workspace.pipelines.weighted_sum_rows,
            bytes_of(&moe_weighted_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.expert_down_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.buffers.moe_top_k_weights,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &workspace.buffers.moe_weighted_out,
                    offset_bytes: 0,
                },
            ],
            2,
            &[],
            MetalSize {
                width: workspace.expert_down.out_len() as u64,
                height: 1,
                depth: 1,
            },
            MetalSize {
                width: 1,
                height: 1,
                depth: 1,
            },
        )?;
        dispatch_compute_tracked_split(
            &runtime,
            &workspace.pipelines.rms,
            bytes_of(&post_ffn_norm2_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.moe_weighted_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.weights.post_feedforward_norm2_weight,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &workspace.buffers.moe_post_ffn_norm2_out,
                    offset_bytes: 0,
                },
            ],
            2,
            &[],
            rms_threadgroups,
            rms_threads_per_threadgroup,
        )?;
        dispatch_compute_tracked_split(
            &runtime,
            &workspace.pipelines.residual,
            bytes_of(&residual_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.post_feedforward_norm1_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.buffers.moe_post_ffn_norm2_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &workspace.buffers.moe_merge_out,
                    offset_bytes: 0,
                },
            ],
            2,
            &[],
            residual_threadgroups,
            residual_threads_per_threadgroup,
        )?;
        dispatch_compute_tracked_split(
            &runtime,
            &workspace.pipelines.rms,
            bytes_of(&post_ffn_norm_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.moe_merge_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.weights.post_feedforward_norm_weight,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &workspace.buffers.post_feedforward_norm_out,
                    offset_bytes: 0,
                },
            ],
            2,
            &[],
            rms_threadgroups,
            rms_threads_per_threadgroup,
        )?;
        dispatch_compute_tracked_split(
            &runtime,
            &workspace.pipelines.residual,
            bytes_of(&residual_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.residual_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.buffers.post_feedforward_norm_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: output_hidden_buffer,
                    offset_bytes: 0,
                },
            ],
            2,
            &[],
            residual_threadgroups,
            residual_threads_per_threadgroup,
        )?;
        if let Some(scale_args) = scale_args.as_ref() {
            dispatch_compute_tracked_split(
                &runtime,
                &workspace.pipelines.scale_row,
                bytes_of(scale_args),
                [
                    MetalBufferBindingRef {
                        index: 1,
                        buffer: output_hidden_buffer,
                        offset_bytes: 0,
                    },
                    MetalBufferBindingRef {
                        index: 2,
                        buffer: output_hidden_buffer,
                        offset_bytes: 0,
                    },
                ],
                1,
                &[],
                residual_threadgroups,
                residual_threads_per_threadgroup,
            )?;
        }
        if owns_command_batch {
            runtime.end_command_batch()?;
        }

        if read_output {
            Ok(Some(bf16_words_from_f32_bits(&read_bf16_buffer_bits(
                &runtime,
                output_hidden_buffer,
                workspace.post_feedforward_norm1_len,
            )?)))
        } else {
            Ok(None)
        }
    }

    pub(crate) fn eval_layer_hidden_state(
        &mut self,
        layer_idx: usize,
        input_words: &[u16],
        position: usize,
    ) -> Result<Vec<u16>, Box<dyn Error>> {
        self.eval_layer_hidden_state_core(layer_idx, Some(input_words), None, None, position, true)?
            .ok_or_else(|| "exact metal layer eval did not return output".into())
    }

    fn eval_token_hidden_state_core(
        &mut self,
        input_words: Option<&[u16]>,
        position: usize,
        read_output: bool,
    ) -> Result<Option<Vec<u16>>, Box<dyn Error>> {
        if let Some(input_words) = input_words {
            if input_words.len() != NORM_LEN {
                return Err(format!(
                    "exact metal token input length mismatch: got {} expected {}",
                    input_words.len(),
                    NORM_LEN
                )
                .into());
            }
        }

        let layer_count = self
            .session
            .weights
            .snapshot
            .config
            .text_config
            .num_hidden_layers as usize;
        if layer_count == 0 {
            if !read_output {
                return Ok(None);
            }
            if let Some(input_words) = input_words {
                return Ok(Some(input_words.to_vec()));
            }
            return Ok(Some(bf16_words_from_f32_bits(&read_bf16_buffer_bits(
                &self.session.runtime,
                &self.text_io.buffers.standalone_hidden,
                NORM_LEN,
            )?)));
        }

        let hidden_a = self.text_io.buffers.standalone_hidden.clone();
        let hidden_b = self.text_io.buffers.hidden_scratch.clone();
        for layer_idx in 0..layer_count {
            let (input_buffer, output_buffer) = if layer_idx % 2 == 0 {
                (&hidden_a, &hidden_b)
            } else {
                (&hidden_b, &hidden_a)
            };
            let maybe_words = self.eval_layer_hidden_state_core(
                layer_idx,
                if layer_idx == 0 { input_words } else { None },
                Some(input_buffer),
                Some(output_buffer),
                position,
                read_output && layer_idx + 1 == layer_count,
            )?;
            if let Some(words) = maybe_words {
                return Ok(Some(words));
            }
        }

        if read_output {
            Err("exact metal token eval completed without a final layer output".into())
        } else {
            Ok(None)
        }
    }

    pub(crate) fn eval_token_hidden_state(
        &mut self,
        input_words: &[u16],
        position: usize,
    ) -> Result<Vec<u16>, Box<dyn Error>> {
        let runtime = self.session.runtime.clone();
        runtime.begin_command_batch()?;
        let batch_result = self.eval_token_hidden_state_core(Some(input_words), position, false);
        if let Err(err) = batch_result {
            let _ = runtime.discard_command_batch();
            return Err(err);
        }
        runtime.end_command_batch()?;
        let hidden_buffer = self.final_hidden_buffer()?;
        self.read_hidden_words_from_buffer(&hidden_buffer)
    }

    pub(crate) fn eval_token_hidden_state_from_token_id(
        &mut self,
        token_id: u32,
        position: usize,
    ) -> Result<Vec<u16>, Box<dyn Error>> {
        let input_buffer = self.token_input_buffer()?;
        let runtime = self.session.runtime.clone();
        runtime.begin_command_batch()?;
        let batch_result = (|| -> Result<(), Box<dyn Error>> {
            self.dequantize_token_embedding_into_buffer(token_id, &input_buffer)?;
            self.eval_token_hidden_state_core(None, position, false)?;
            Ok(())
        })();
        if let Err(err) = batch_result {
            let _ = runtime.discard_command_batch();
            return Err(err);
        }
        runtime.end_command_batch()?;
        let hidden_buffer = self.final_hidden_buffer()?;
        self.read_hidden_words_from_buffer(&hidden_buffer)
    }

    pub(crate) fn eval_token_greedy_token_id_from_token_id(
        &mut self,
        token_id: u32,
        position: usize,
    ) -> Result<u32, Box<dyn Error>> {
        let input_buffer = self.token_input_buffer()?;
        let runtime = self.session.runtime.clone();
        runtime.begin_command_batch()?;
        let batch_result = (|| -> Result<(), Box<dyn Error>> {
            self.dequantize_token_embedding_into_buffer(token_id, &input_buffer)?;
            self.eval_token_hidden_state_core(None, position, false)?;
            let hidden_buffer = self.final_hidden_buffer()?;
            self.dispatch_final_text_norm_on_hidden_buffer(&hidden_buffer)?;
            self.dispatch_logits_projection_from_final_norm()?;
            Ok(())
        })();
        if let Err(err) = batch_result {
            let _ = runtime.discard_command_batch();
            return Err(err);
        }
        runtime.end_command_batch()?;
        Ok(self.read_shared_logits_greedy_token()?.token_id)
    }

    pub(crate) fn eval_token_logits_from_token_id(
        &mut self,
        token_id: u32,
        position: usize,
    ) -> Result<Vec<f32>, Box<dyn Error>> {
        let input_buffer = self.token_input_buffer()?;
        let runtime = self.session.runtime.clone();
        runtime.begin_command_batch()?;
        let batch_result = (|| -> Result<(), Box<dyn Error>> {
            self.dequantize_token_embedding_into_buffer(token_id, &input_buffer)?;
            self.eval_token_hidden_state_core(None, position, false)?;
            let hidden_buffer = self.final_hidden_buffer()?;
            self.dispatch_final_text_norm_on_hidden_buffer(&hidden_buffer)?;
            self.dispatch_logits_projection_from_final_norm()?;
            Ok(())
        })();
        if let Err(err) = batch_result {
            let _ = runtime.discard_command_batch();
            return Err(err);
        }
        runtime.end_command_batch()?;
        self.read_shared_logits_softcapped()
    }

    pub(crate) fn eval_token_sampled_from_token_id(
        &mut self,
        token_id: u32,
        position: usize,
        disallowed_token_ids: &[u32],
        sampling_options: &GemmaTextSamplingOptions,
        rng: &mut MlxTextSamplingRng,
    ) -> Result<MlxGreedyToken, Box<dyn Error>> {
        let input_buffer = self.token_input_buffer()?;
        let runtime = self.session.runtime.clone();
        runtime.begin_command_batch()?;
        let batch_result = (|| -> Result<(), Box<dyn Error>> {
            self.dequantize_token_embedding_into_buffer(token_id, &input_buffer)?;
            self.eval_token_hidden_state_core(None, position, false)?;
            let hidden_buffer = self.final_hidden_buffer()?;
            self.dispatch_final_text_norm_on_hidden_buffer(&hidden_buffer)?;
            self.dispatch_logits_projection_from_final_norm()?;
            Ok(())
        })();
        if let Err(err) = batch_result {
            let _ = runtime.discard_command_batch();
            return Err(err);
        }
        runtime.end_command_batch()?;
        self.sampled_token_from_logits(disallowed_token_ids, sampling_options, rng)
    }

    pub(crate) fn eval_token_greedy_from_token_id(
        &mut self,
        token_id: u32,
        position: usize,
    ) -> Result<MlxGreedyToken, Box<dyn Error>> {
        let input_buffer = self.token_input_buffer()?;
        let runtime = self.session.runtime.clone();
        runtime.begin_command_batch()?;
        let batch_result = (|| -> Result<(), Box<dyn Error>> {
            self.dequantize_token_embedding_into_buffer(token_id, &input_buffer)?;
            self.eval_token_hidden_state_core(None, position, false)?;
            let hidden_buffer = self.final_hidden_buffer()?;
            self.dispatch_final_text_norm_on_hidden_buffer(&hidden_buffer)?;
            self.dispatch_logits_projection_from_final_norm()?;
            Ok(())
        })();
        if let Err(err) = batch_result {
            let _ = runtime.discard_command_batch();
            return Err(err);
        }
        runtime.end_command_batch()?;
        self.read_shared_logits_greedy_token()
    }

    pub(crate) fn eval_token_greedy_token_chunk_from_token_id(
        &mut self,
        token_id: u32,
        position: usize,
        token_count: usize,
    ) -> Result<Vec<u32>, Box<dyn Error>> {
        if token_count == 0 {
            return Ok(Vec::new());
        }
        if token_count > DEVICE_GREEDY_DECODE_CHUNK_TOKENS {
            return Err(format!(
                "device greedy decode chunk {} exceeds capacity {}",
                token_count, DEVICE_GREEDY_DECODE_CHUNK_TOKENS
            )
            .into());
        }
        if position == 0 {
            self.reset_kv_caches();
        }
        let input_buffer = self.token_input_buffer()?;
        let runtime = self.session.runtime.clone();
        runtime.begin_command_batch()?;
        let batch_result = (|| -> Result<(), Box<dyn Error>> {
            self.dequantize_token_embedding_into_buffer(token_id, &input_buffer)?;
            for step_idx in 0..token_count {
                self.eval_token_hidden_state_core(None, position + step_idx, false)?;
                let hidden_buffer = self.final_hidden_buffer()?;
                self.dispatch_greedy_head_on_hidden_buffer(&hidden_buffer)?;
                self.dequantize_next_token_embedding_from_device_buffer(&input_buffer, step_idx)?;
                if step_idx + 1 < token_count {
                    runtime.seal_command_batch_encoder()?;
                }
            }
            Ok(())
        })();
        if let Err(err) = batch_result {
            let _ = runtime.discard_command_batch();
            return Err(err);
        }
        runtime.end_command_batch()?;
        self.read_generated_token_chunk(token_count)
    }

    pub(crate) fn prefill_prompt_greedy_token_id_from_token_ids(
        &mut self,
        prompt_token_ids: &[u32],
        start_position: usize,
    ) -> Result<u32, Box<dyn Error>> {
        if prompt_token_ids.is_empty() {
            return Err("prompt prefill requires at least one token".into());
        }
        if start_position == 0 {
            self.reset_kv_caches();
        }
        let input_buffer = self.token_input_buffer()?;
        let runtime = self.session.runtime.clone();
        runtime.begin_command_batch()?;
        let batch_result = (|| -> Result<(), Box<dyn Error>> {
            for (offset, token_id) in prompt_token_ids.iter().copied().enumerate() {
                let position = start_position + offset;
                self.dequantize_token_embedding_into_buffer(token_id, &input_buffer)?;
                self.eval_token_hidden_state_core(None, position, false)?;
            }
            let hidden_buffer = self.final_hidden_buffer()?;
            self.dispatch_final_text_norm_on_hidden_buffer(&hidden_buffer)?;
            self.dispatch_logits_projection_from_final_norm()?;
            Ok(())
        })();
        if let Err(err) = batch_result {
            let _ = runtime.discard_command_batch();
            return Err(err);
        }
        runtime.end_command_batch()?;
        Ok(self.read_shared_logits_greedy_token()?.token_id)
    }

    pub(crate) fn prefill_prompt_logits_from_token_ids(
        &mut self,
        prompt_token_ids: &[u32],
        start_position: usize,
    ) -> Result<Vec<f32>, Box<dyn Error>> {
        if prompt_token_ids.is_empty() {
            return Err("prompt prefill requires at least one token".into());
        }
        if start_position == 0 {
            self.reset_kv_caches();
        }
        let input_buffer = self.token_input_buffer()?;
        let runtime = self.session.runtime.clone();
        runtime.begin_command_batch()?;
        let batch_result = (|| -> Result<(), Box<dyn Error>> {
            for (offset, token_id) in prompt_token_ids.iter().copied().enumerate() {
                let position = start_position + offset;
                self.dequantize_token_embedding_into_buffer(token_id, &input_buffer)?;
                self.eval_token_hidden_state_core(None, position, false)?;
            }
            let hidden_buffer = self.final_hidden_buffer()?;
            self.dispatch_final_text_norm_on_hidden_buffer(&hidden_buffer)?;
            self.dispatch_logits_projection_from_final_norm()?;
            Ok(())
        })();
        if let Err(err) = batch_result {
            let _ = runtime.discard_command_batch();
            return Err(err);
        }
        runtime.end_command_batch()?;
        self.read_shared_logits_softcapped()
    }

    pub(crate) fn prefill_prompt_sampled_from_token_ids(
        &mut self,
        prompt_token_ids: &[u32],
        start_position: usize,
        disallowed_token_ids: &[u32],
        sampling_options: &GemmaTextSamplingOptions,
        rng: &mut MlxTextSamplingRng,
    ) -> Result<MlxGreedyToken, Box<dyn Error>> {
        if prompt_token_ids.is_empty() {
            return Err("prompt prefill requires at least one token".into());
        }
        if start_position == 0 {
            self.reset_kv_caches();
        }
        let input_buffer = self.token_input_buffer()?;
        let runtime = self.session.runtime.clone();
        runtime.begin_command_batch()?;
        let batch_result = (|| -> Result<(), Box<dyn Error>> {
            for (offset, token_id) in prompt_token_ids.iter().copied().enumerate() {
                let position = start_position + offset;
                self.dequantize_token_embedding_into_buffer(token_id, &input_buffer)?;
                self.eval_token_hidden_state_core(None, position, false)?;
            }
            let hidden_buffer = self.final_hidden_buffer()?;
            self.dispatch_final_text_norm_on_hidden_buffer(&hidden_buffer)?;
            self.dispatch_logits_projection_from_final_norm()?;
            Ok(())
        })();
        if let Err(err) = batch_result {
            let _ = runtime.discard_command_batch();
            return Err(err);
        }
        runtime.end_command_batch()?;
        self.sampled_token_from_logits(disallowed_token_ids, sampling_options, rng)
    }

    pub(crate) fn prefill_prompt_hidden_words_from_token_ids(
        &mut self,
        prompt_token_ids: &[u32],
        start_position: usize,
    ) -> Result<Vec<u16>, Box<dyn Error>> {
        if prompt_token_ids.is_empty() {
            return Err("prompt prefill requires at least one token".into());
        }
        if start_position == 0 {
            self.reset_kv_caches();
        }
        let input_buffer = self.token_input_buffer()?;
        let runtime = self.session.runtime.clone();
        runtime.begin_command_batch()?;
        let batch_result = (|| -> Result<(), Box<dyn Error>> {
            for (offset, token_id) in prompt_token_ids.iter().copied().enumerate() {
                let position = start_position + offset;
                self.dequantize_token_embedding_into_buffer(token_id, &input_buffer)?;
                self.eval_token_hidden_state_core(None, position, false)?;
            }
            Ok(())
        })();
        if let Err(err) = batch_result {
            let _ = runtime.discard_command_batch();
            return Err(err);
        }
        runtime.end_command_batch()?;
        let hidden_buffer = self.final_hidden_buffer()?;
        self.read_hidden_words_from_buffer(&hidden_buffer)
    }

    pub(crate) fn prefill_prompt_greedy_from_token_ids(
        &mut self,
        prompt_token_ids: &[u32],
        start_position: usize,
    ) -> Result<MlxGreedyToken, Box<dyn Error>> {
        if prompt_token_ids.is_empty() {
            return Err("prompt prefill requires at least one token".into());
        }
        if start_position == 0 {
            self.reset_kv_caches();
        }
        let input_buffer = self.token_input_buffer()?;
        let runtime = self.session.runtime.clone();
        runtime.begin_command_batch()?;
        let batch_result = (|| -> Result<(), Box<dyn Error>> {
            for (offset, token_id) in prompt_token_ids.iter().copied().enumerate() {
                let position = start_position + offset;
                self.dequantize_token_embedding_into_buffer(token_id, &input_buffer)?;
                self.eval_token_hidden_state_core(None, position, false)?;
            }
            let hidden_buffer = self.final_hidden_buffer()?;
            self.dispatch_final_text_norm_on_hidden_buffer(&hidden_buffer)?;
            self.dispatch_logits_projection_from_final_norm()?;
            Ok(())
        })();
        if let Err(err) = batch_result {
            let _ = runtime.discard_command_batch();
            return Err(err);
        }
        runtime.end_command_batch()?;
        self.read_shared_logits_greedy_token()
    }

    pub(crate) fn greedy_token_from_hidden_words(
        &mut self,
        hidden_words: &[u16],
    ) -> Result<MlxGreedyToken, Box<dyn Error>> {
        if hidden_words.len() != NORM_LEN {
            return Err(format!(
                "exact metal hidden-word length mismatch: got {} expected {}",
                hidden_words.len(),
                NORM_LEN
            )
            .into());
        }
        self.session.runtime.write_buffer(
            &self.text_io.buffers.standalone_hidden,
            0,
            &bytes_from_bf16_words(hidden_words),
        )?;
        let hidden_buffer = self.text_io.buffers.standalone_hidden.clone();
        self.greedy_token_from_hidden_buffer(&hidden_buffer)
    }

    #[cfg(test)]
    pub(crate) fn compare_greedy_token_paths_from_hidden_words(
        &mut self,
        hidden_words: &[u16],
    ) -> Result<(MlxGreedyToken, MlxGreedyToken), Box<dyn Error>> {
        if hidden_words.len() != NORM_LEN {
            return Err(format!(
                "exact metal hidden-word length mismatch: got {} expected {}",
                hidden_words.len(),
                NORM_LEN
            )
            .into());
        }
        self.session.runtime.write_buffer(
            &self.text_io.buffers.standalone_hidden,
            0,
            &bytes_from_bf16_words(hidden_words),
        )?;
        let hidden_buffer = self.text_io.buffers.standalone_hidden.clone();
        self.dispatch_greedy_head_on_hidden_buffer(&hidden_buffer)?;
        let device = self.read_device_greedy_token()?;
        let shared = self.read_shared_logits_greedy_token()?;
        Ok((device, shared))
    }
}

pub fn profile_decode_layers_after_prompt_token_ids(
    model_path: PathBuf,
    prompt_token_ids: &[u32],
) -> Result<ExactMetalDecodeStepProfile, Box<dyn Error>> {
    let mut backend = ExactMetalTextRuntimeSession::load(model_path)?;
    backend.profile_decode_layers_after_prompt(prompt_token_ids)
}

impl ExactMetalGenerationCursor {
    fn target_count(&self, requested_count: usize) -> usize {
        self.max_new_tokens
            .map_or(requested_count, |limit| requested_count.min(limit))
    }

    fn remaining_generation_limit(&self) -> usize {
        self.max_new_tokens
            .map_or(usize::MAX, |limit| limit.saturating_sub(self.generated_token_ids.len()))
    }

    fn reached_generation_limit(&self) -> bool {
        self.max_new_tokens
            .is_some_and(|limit| self.generated_token_ids.len() >= limit)
    }

    fn eval_token_next_with_backend(
        backend: &mut ExactMetalTextRuntimeSession,
        token_id: u32,
        position: usize,
    ) -> Result<u32, Box<dyn Error>> {
        if position == 0 {
            backend.reset_kv_caches();
        }
        backend.eval_token_greedy_token_id_from_token_id(token_id, position)
    }

    fn eval_token_chunk_with_backend(
        backend: &mut ExactMetalTextRuntimeSession,
        token_id: u32,
        position: usize,
        token_count: usize,
    ) -> Result<Vec<u32>, Box<dyn Error>> {
        if token_count == 0 {
            return Ok(Vec::new());
        }
        if position == 0 {
            backend.reset_kv_caches();
        }
        let mut current_token = token_id;
        let mut generated = Vec::with_capacity(token_count);
        for step_idx in 0..token_count {
            let next_token = backend
                .eval_token_greedy_token_id_from_token_id(current_token, position + step_idx)?;
            generated.push(next_token);
            current_token = next_token;
        }
        Ok(generated)
    }

    fn ensure_prompt_prefilled_locked(
        &mut self,
        backend: &mut ExactMetalTextRuntimeSession,
    ) -> Result<(), Box<dyn Error>> {
        if self.processed_prompt_tokens >= self.prompt_token_ids.len() {
            return Ok(());
        }
        let remaining_prompt_tokens = &self.prompt_token_ids[self.processed_prompt_tokens..];
        self.pending_next = Some(backend.prefill_prompt_greedy_token_id_from_token_ids(
            remaining_prompt_tokens,
            self.position,
        )?);
        self.processed_prompt_tokens += remaining_prompt_tokens.len();
        self.position += remaining_prompt_tokens.len();
        Ok(())
    }

    pub(crate) fn ensure_prompt_prefilled(&mut self) -> Result<(), Box<dyn Error>> {
        let backend_handle = Arc::clone(&self.backend);
        let mut backend = backend_handle
            .lock()
            .map_err(|_| "exact backend mutex poisoned".to_string())?;
        self.ensure_prompt_prefilled_locked(&mut backend)
    }

    fn snapshot(&self) -> ExactMetalGenerationSnapshot {
        ExactMetalGenerationSnapshot {
            generated_token_ids: Arc::<[u32]>::from(self.generated_token_ids.clone()),
            stop_reason: self.stop_reason,
            #[cfg(test)]
            processed_prompt_tokens: self.processed_prompt_tokens,
            #[cfg(test)]
            position: self.position,
            #[cfg(test)]
            has_pending_next: self.pending_next.is_some(),
        }
    }

    pub(crate) fn ensure_generated(
        &mut self,
        requested_count: usize,
    ) -> Result<(), Box<dyn Error>> {
        let target = self.target_count(requested_count);
        let backend_handle = Arc::clone(&self.backend);
        let mut backend = backend_handle
            .lock()
            .map_err(|_| "exact backend mutex poisoned".to_string())?;
        while self.generated_token_ids.len() < target {
            if self.stop_reason.is_some() {
                break;
            }
            if self.pending_next.is_none() {
                if self.processed_prompt_tokens < self.prompt_token_ids.len() {
                    self.ensure_prompt_prefilled_locked(&mut backend)?;
                } else if let Some(&last_generated) = self.generated_token_ids.last() {
                    let input_position = self
                        .position
                        .checked_sub(1)
                        .ok_or("generation cursor position underflow")?;
                    let remaining_target = target.saturating_sub(self.generated_token_ids.len());
                    let remaining_max = self.remaining_generation_limit();
                    let chunk_len = remaining_target
                        .min(remaining_max)
                        .min(DEVICE_GREEDY_DECODE_CHUNK_TOKENS);
                    if chunk_len > 1 {
                        let chunk_tokens = Self::eval_token_chunk_with_backend(
                            &mut backend,
                            last_generated,
                            input_position,
                            chunk_len,
                        )?;
                        for token_id in chunk_tokens {
                            if self.stop_tokens.contains(&token_id) {
                                self.stop_reason =
                                    Some(ExactMetalGenerationStopReason::EosToken(token_id));
                                break;
                            }
                            self.generated_token_ids.push(token_id);
                            self.position += 1;
                            if self.reached_generation_limit() {
                                self.stop_reason =
                                    Some(ExactMetalGenerationStopReason::MaxNewTokens);
                                break;
                            }
                            if self.generated_token_ids.len() >= target {
                                break;
                            }
                        }
                        continue;
                    }
                    self.pending_next = Some(Self::eval_token_next_with_backend(
                        &mut backend,
                        last_generated,
                        input_position,
                    )?);
                }
            }
            let next_token = self
                .pending_next
                .take()
                .ok_or_else(|| "generation cursor missing pending next token".to_string())?;
            if self.stop_tokens.contains(&next_token) {
                self.stop_reason = Some(ExactMetalGenerationStopReason::EosToken(next_token));
                break;
            }
            self.generated_token_ids.push(next_token);
            self.position += 1;
            if self.reached_generation_limit() {
                self.stop_reason = Some(ExactMetalGenerationStopReason::MaxNewTokens);
                break;
            }
            if self.generated_token_ids.len() >= target {
                break;
            }
            self.pending_next = Some(Self::eval_token_next_with_backend(
                &mut backend,
                next_token,
                self.position - 1,
            )?);
        }
        if self.reached_generation_limit() && self.stop_reason.is_none() {
            self.stop_reason = Some(ExactMetalGenerationStopReason::MaxNewTokens);
            self.pending_next = None;
        }
        Ok(())
    }

    pub(crate) fn ensure_finished(&mut self) -> Result<(), Box<dyn Error>> {
        if let Some(limit) = self.max_new_tokens {
            self.ensure_generated(limit)
        } else {
            while self.stop_reason.is_none() {
                let next_target = self
                    .generated_token_ids
                    .len()
                    .saturating_add(DEVICE_GREEDY_DECODE_CHUNK_TOKENS);
                self.ensure_generated(next_target)?;
            }
            Ok(())
        }
    }

    #[cfg(test)]
    pub(crate) fn generated_token_ids(&self) -> &[u32] {
        &self.generated_token_ids
    }

    #[cfg(test)]
    pub(crate) fn processed_prompt_tokens(&self) -> usize {
        self.processed_prompt_tokens
    }

    #[cfg(test)]
    pub(crate) fn position(&self) -> usize {
        self.position
    }

    #[cfg(test)]
    pub(crate) fn has_pending_next(&self) -> bool {
        self.pending_next.is_some()
    }
}

impl ExactMetalPromptPrefillNode {
    fn new(cursor: Arc<Mutex<ExactMetalGenerationCursor>>) -> Self {
        Self {
            cursor,
            value: OnceLock::new(),
        }
    }

    fn eval(&self) -> Result<Arc<ExactMetalGenerationSnapshot>, String> {
        self.value
            .get_or_init(|| {
                let mut cursor = self
                    .cursor
                    .lock()
                    .map_err(|_| "generation cursor mutex poisoned".to_string())?;
                cursor
                    .ensure_prompt_prefilled()
                    .map_err(|err| err.to_string())?;
                Ok(Arc::new(cursor.snapshot()))
            })
            .clone()
    }
}

impl ExactMetalGenerationStepNode {
    fn new(
        cursor: Arc<Mutex<ExactMetalGenerationCursor>>,
        target_count: usize,
        dependency: ExactMetalGenerationDependency,
    ) -> Self {
        Self {
            cursor,
            target_count,
            dependency,
            value: OnceLock::new(),
        }
    }

    fn eval(&self) -> Result<Arc<ExactMetalGenerationSnapshot>, String> {
        self.value
            .get_or_init(|| {
                match &self.dependency {
                    ExactMetalGenerationDependency::PromptPrefill(node) => {
                        node.eval()?;
                    }
                    ExactMetalGenerationDependency::Previous(node) => {
                        node.eval()?;
                    }
                }
                let mut cursor = self
                    .cursor
                    .lock()
                    .map_err(|_| "generation cursor mutex poisoned".to_string())?;
                cursor
                    .ensure_generated(self.target_count)
                    .map_err(|err| err.to_string())?;
                Ok(Arc::new(cursor.snapshot()))
            })
            .clone()
    }
}

impl ExactMetalGenerationGraph {
    fn new(cursor: ExactMetalGenerationCursor) -> Result<Self, Box<dyn Error>> {
        let max_new_tokens = cursor.max_new_tokens;
        let cursor = Arc::new(Mutex::new(cursor));
        Ok(Self {
            prompt_prefill: Arc::new(ExactMetalPromptPrefillNode::new(Arc::clone(&cursor))),
            cursor,
            step_nodes: Mutex::new(Vec::with_capacity(
                max_new_tokens.unwrap_or(DEVICE_GREEDY_DECODE_CHUNK_TOKENS),
            )),
            final_snapshot: OnceLock::new(),
            max_new_tokens,
        })
    }

    fn step_node(
        &self,
        requested_count: usize,
    ) -> Result<Arc<ExactMetalGenerationStepNode>, String> {
        let target = self
            .max_new_tokens
            .map_or(requested_count, |limit| requested_count.min(limit));
        if target == 0 {
            return Err("generation step nodes start at token count 1".to_string());
        }
        let mut nodes = self
            .step_nodes
            .lock()
            .map_err(|_| "generation step-node mutex poisoned".to_string())?;
        while nodes.len() < target {
            let next_count = nodes.len() + 1;
            let dependency = if let Some(prev) = nodes.last() {
                ExactMetalGenerationDependency::Previous(Arc::clone(prev))
            } else {
                ExactMetalGenerationDependency::PromptPrefill(Arc::clone(&self.prompt_prefill))
            };
            nodes.push(Arc::new(ExactMetalGenerationStepNode::new(
                Arc::clone(&self.cursor),
                next_count,
                dependency,
            )));
        }
        nodes
            .get(target - 1)
            .cloned()
            .ok_or_else(|| format!("missing generation step node {target}"))
    }

    pub(crate) fn generated_token_ids_up_to(
        &self,
        requested_count: usize,
    ) -> Result<Arc<[u32]>, String> {
        let target = self
            .max_new_tokens
            .map_or(requested_count, |limit| requested_count.min(limit));
        if target == 0 {
            return Ok(Arc::<[u32]>::from(Vec::<u32>::new()));
        }
        Ok(self.step_node(target)?.eval()?.generated_token_ids.clone())
    }

    pub(crate) fn finish_snapshot(&self) -> Result<Arc<ExactMetalGenerationSnapshot>, String> {
        self.final_snapshot
            .get_or_init(|| {
                let mut cursor = self
                    .cursor
                    .lock()
                    .map_err(|_| "generation cursor mutex poisoned".to_string())?;
                cursor.ensure_finished().map_err(|err| err.to_string())?;
                Ok(Arc::new(cursor.snapshot()))
            })
            .clone()
    }
}

fn optional_private_weight_buffer(
    session: &mut LayerExecutionSession,
    enabled: bool,
    name: &str,
) -> Result<Option<MetalBuffer>, Box<dyn Error>> {
    if enabled {
        Ok(Some(session.private_weight_buffer(name)?))
    } else {
        Ok(None)
    }
}

fn create_bf16_buffer(
    runtime: &MetalRuntime,
    len_words: usize,
    storage: BufferStorageMode,
) -> Result<MetalBuffer, Box<dyn Error>> {
    Ok(runtime.create_buffer(len_words * size_of::<u16>(), storage)?)
}

fn compile_default_pipeline(
    runtime: &MetalRuntime,
    name: &str,
) -> Result<MetalPipeline, Box<dyn Error>> {
    compile_pipeline(runtime, name, 0)
}

fn compile_pipeline(
    runtime: &MetalRuntime,
    name: &str,
    smem_bytes: usize,
) -> Result<MetalPipeline, Box<dyn Error>> {
    Ok(runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
        cache_name: name.to_string(),
        base_name: name.to_string(),
        constants: Vec::new(),
        smem_bytes,
        nr0: 0,
        nr1: 0,
        nsg: 0,
    })?)
}

fn read_bf16_buffer_bits(
    runtime: &MetalRuntime,
    buffer: &MetalBuffer,
    len_words: usize,
) -> Result<Vec<u32>, Box<dyn Error>> {
    Ok(
        runtime.with_readable_buffer(buffer, len_words * size_of::<u16>(), |bytes| {
            Ok(decode_bf16_buffer_bits(bytes))
        })?,
    )
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

fn read_f32_file_as_bf16_words(path: &Path) -> Result<Vec<u16>, Box<dyn Error>> {
    let bytes = fs::read(path)?;
    if bytes.len() % size_of::<f32>() != 0 {
        return Err(format!(
            "f32 input file {} length {} is not a multiple of {}",
            path.display(),
            bytes.len(),
            size_of::<f32>()
        )
        .into());
    }
    let mut words = Vec::with_capacity(bytes.len() / size_of::<f32>());
    for chunk in bytes.chunks_exact(size_of::<f32>()) {
        let value = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        words.push((bf16_round_to_f32(value).to_bits() >> 16) as u16);
    }
    Ok(words)
}

fn write_bf16_words_as_f32_file(path: &Path, words: &[u16]) -> Result<(), Box<dyn Error>> {
    let mut bytes = Vec::with_capacity(words.len() * size_of::<f32>());
    for &word in words {
        bytes.extend_from_slice(&bf16_word_to_f32(word).to_le_bytes());
    }
    fs::write(path, bytes)?;
    Ok(())
}

fn bf16_words_from_f32_bits(bits: &[u32]) -> Vec<u16> {
    bits.iter()
        .copied()
        .map(|bits| (bits >> 16) as u16)
        .collect()
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
    bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .map(bf16_word_to_f32)
        .map(f32::to_bits)
        .collect()
}

fn decode_u32_buffer_words(bytes: &[u8]) -> Vec<u32> {
    bytes
        .chunks_exact(size_of::<u32>())
        .map(|chunk| u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

fn read_router_output_from_device(
    runtime: &MetalRuntime,
    router_scaled_out: &MetalBuffer,
    router_proj_out: &MetalBuffer,
    router_probs_out: &MetalBuffer,
    moe_top_k_indices: &MetalBuffer,
    moe_top_k_weights: &MetalBuffer,
    router_scaled_len: usize,
    expert_count: usize,
    top_k: usize,
) -> Result<Layer0CachedRouterOutput, Box<dyn Error>> {
    Ok(Layer0CachedRouterOutput {
        router_scaled_bits: decode_bf16_buffer_bits(
            &runtime.read_buffer(router_scaled_out, router_scaled_len * size_of::<u16>())?,
        ),
        expert_scores_bits: decode_bf16_buffer_bits(
            &runtime.read_buffer(router_proj_out, expert_count * size_of::<u16>())?,
        ),
        router_probs_bits: decode_bf16_buffer_bits(
            &runtime.read_buffer(router_probs_out, expert_count * size_of::<u16>())?,
        ),
        top_k_indices: decode_u32_buffer_words(
            &runtime.read_buffer(moe_top_k_indices, top_k * size_of::<u32>())?,
        ),
        top_k_weights_bits: decode_bf16_buffer_bits(
            &runtime.read_buffer(moe_top_k_weights, top_k * size_of::<u16>())?,
        ),
    })
}

fn bits_to_f32(bits: &[u32]) -> Vec<f32> {
    bits.iter().copied().map(f32::from_bits).collect()
}

fn flatten_heads_to_tensor(
    bits: &[u32],
    head_count: usize,
    head_dim: usize,
) -> Result<KvTensor<f32>, Box<dyn Error>> {
    let shape = KvTensorShape {
        batch_size: 1,
        kv_head_count: head_count,
        seq_len: 1,
        head_dim,
    };
    KvTensor::from_vec(shape, bits_to_f32(bits)).map_err(|err| err.into())
}

fn attention_prob_bits_from_logits(
    logits_bits: &[u32],
    q_head_count: usize,
    seq_len: usize,
) -> Vec<u32> {
    let mut prob_bits = Vec::with_capacity(q_head_count * seq_len);
    for q_head in 0..q_head_count {
        let row_start = q_head * seq_len;
        let row = &logits_bits[row_start..row_start + seq_len];
        let max_score = row
            .iter()
            .copied()
            .map(f32::from_bits)
            .fold(f32::NEG_INFINITY, f32::max);
        let exp_scores = row
            .iter()
            .copied()
            .map(f32::from_bits)
            .map(|score| (score - max_score).exp())
            .collect::<Vec<_>>();
        let exp_sum = exp_scores.iter().copied().sum::<f32>();
        for value in exp_scores {
            prob_bits.push(bf16_round_to_f32(value / exp_sum).to_bits());
        }
    }
    prob_bits
}

fn read_exact_kv_cache_tensor_bits(
    runtime: &MetalRuntime,
    cache: &ExactMetalKvCache,
    buffer: &MetalBuffer,
) -> Result<Vec<u32>, Box<dyn Error>> {
    let row_stride_words = cache.row_stride_words()?;
    let storage_words = cache
        .spec
        .batch_size
        .checked_mul(cache.spec.kv_head_count)
        .and_then(|value| value.checked_mul(row_stride_words))
        .ok_or("exact metal KV cache readback overflow")?;
    let storage_bits = read_bf16_buffer_bits(runtime, buffer, storage_words)?;
    let mut out = Vec::with_capacity(
        cache
            .spec
            .batch_size
            .checked_mul(cache.spec.kv_head_count)
            .and_then(|value| value.checked_mul(cache.stored_tokens))
            .and_then(|value| value.checked_mul(cache.spec.head_dim))
            .ok_or("exact metal KV tensor compact size overflow")?,
    );
    let start_slot = cache.start_slot();
    for batch in 0..cache.spec.batch_size {
        for head in 0..cache.spec.kv_head_count {
            let row_base = (batch * cache.spec.kv_head_count + head)
                .checked_mul(row_stride_words)
                .ok_or("exact metal KV compact row base overflow")?;
            for token in 0..cache.stored_tokens {
                let slot = (start_slot + token) % cache.spec.max_tokens;
                let token_base = row_base
                    .checked_add(slot * cache.spec.head_dim)
                    .ok_or("exact metal KV compact token base overflow")?;
                out.extend_from_slice(&storage_bits[token_base..token_base + cache.spec.head_dim]);
            }
        }
    }
    Ok(out)
}

fn read_attention_logits_bits(
    runtime: &MetalRuntime,
    buffer: &MetalBuffer,
    q_head_count: usize,
    seq_len: usize,
    row_stride_words: usize,
) -> Result<Vec<u32>, Box<dyn Error>> {
    let storage_words = q_head_count
        .checked_mul(row_stride_words)
        .ok_or("attention logits storage overflow")?;
    let storage_bits = read_bf16_buffer_bits(runtime, buffer, storage_words)?;
    let mut out = Vec::with_capacity(
        q_head_count
            .checked_mul(seq_len)
            .ok_or("attention logits compact size overflow")?,
    );
    for q_head in 0..q_head_count {
        let row_base = q_head
            .checked_mul(row_stride_words)
            .ok_or("attention logits row base overflow")?;
        out.extend_from_slice(&storage_bits[row_base..row_base + seq_len]);
    }
    Ok(out)
}

fn mlx_softmax_threads_per_threadgroup(
    seq_len: usize,
    max_threads_per_threadgroup: u64,
) -> Result<MetalSize, Box<dyn Error>> {
    const MLX_SOFTMAX_N_READS: usize = 4;
    const MLX_SIMD_WIDTH: usize = 32;

    let threadgroup_needed = seq_len.max(1).div_ceil(MLX_SOFTMAX_N_READS);
    let simds_needed = threadgroup_needed.div_ceil(MLX_SIMD_WIDTH).max(1);
    let threadgroup_width = u64::try_from(
        MLX_SIMD_WIDTH
            .checked_mul(simds_needed)
            .ok_or("softmax threadgroup size overflow")?,
    )?;
    if threadgroup_width > max_threads_per_threadgroup {
        return Err(format!(
            "softmax threadgroup width {} exceeds pipeline max {} for seq_len {}",
            threadgroup_width, max_threads_per_threadgroup, seq_len
        )
        .into());
    }
    Ok(MetalSize {
        width: threadgroup_width,
        height: 1,
        depth: 1,
    })
}

fn compute_cached_attention_metal(
    runtime: &MetalRuntime,
    logits_pipeline: &MetalPipeline,
    softmax_pipeline: &MetalPipeline,
    weighted_sum_pipeline: &MetalPipeline,
    q_buffer: &MetalBuffer,
    cache: &ExactMetalKvCache,
    q_head_count: usize,
    q_heads_per_kv: usize,
    head_dim: usize,
    logits_buffer: &MetalBuffer,
    probs_buffer: &MetalBuffer,
    out_buffer: &MetalBuffer,
) -> Result<(Vec<u32>, Vec<u32>, Vec<u32>), Box<dyn Error>> {
    let seq_len = cache.seq_len();
    let kv_row_stride = cache.row_stride_words()?;
    let logits_row_stride = cache.spec.max_tokens;
    let logits_args = MlxGqaAttentionLogitsSeqArgs {
        head_dim: head_dim as u32,
        q_head_stride: head_dim as u32,
        kv_row_stride: kv_row_stride as u32,
        q_head_count: q_head_count as u32,
        q_heads_per_kv: q_heads_per_kv as u32,
        seq_len: seq_len as u32,
        start_slot: cache.start_slot() as u32,
        capacity: cache.spec.max_tokens as u32,
    };
    let softmax_args = MlxSoftmaxRowsArgs {
        row_stride: logits_row_stride as u32,
        row_count: q_head_count as u32,
        seq_len: seq_len as u32,
    };
    let weighted_sum_args = MlxGqaAttentionWeightedSumArgs {
        probs_row_stride: logits_row_stride as u32,
        head_dim: head_dim as u32,
        kv_row_stride: kv_row_stride as u32,
        out_head_stride: head_dim as u32,
        q_head_count: q_head_count as u32,
        q_heads_per_kv: q_heads_per_kv as u32,
        seq_len: seq_len as u32,
        start_slot: cache.start_slot() as u32,
        capacity: cache.spec.max_tokens as u32,
    };
    let threadgroups_logits = MetalSize {
        width: seq_len as u64,
        height: q_head_count as u64,
        depth: 1,
    };
    let softmax_threads_per_threadgroup =
        mlx_softmax_threads_per_threadgroup(seq_len, softmax_pipeline.max_threads_per_threadgroup)?;
    let threadgroups_output = MetalSize {
        width: (head_dim as u64).div_ceil(64),
        height: 1,
        depth: q_head_count as u64,
    };
    let logits_threads_per_threadgroup = MetalSize {
        width: 32,
        height: 1,
        depth: 1,
    };
    let output_threads_per_threadgroup = MetalSize {
        width: 32,
        height: 4,
        depth: 1,
    };

    runtime.begin_command_batch()?;
    runtime.dispatch_compute(
        logits_pipeline,
        bytes_of(&logits_args),
        &[
            MetalBufferBindingRef {
                index: 1,
                buffer: q_buffer,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 2,
                buffer: &cache.key_buffer,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 3,
                buffer: logits_buffer,
                offset_bytes: 0,
            },
        ],
        &[],
        threadgroups_logits,
        logits_threads_per_threadgroup,
    )?;
    runtime.memory_barrier_buffers()?;
    runtime.dispatch_compute(
        softmax_pipeline,
        bytes_of(&softmax_args),
        &[
            MetalBufferBindingRef {
                index: 1,
                buffer: logits_buffer,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 2,
                buffer: probs_buffer,
                offset_bytes: 0,
            },
        ],
        &[],
        MetalSize {
            width: q_head_count as u64,
            height: 1,
            depth: 1,
        },
        softmax_threads_per_threadgroup,
    )?;
    runtime.memory_barrier_buffers()?;
    runtime.dispatch_compute(
        weighted_sum_pipeline,
        bytes_of(&weighted_sum_args),
        &[
            MetalBufferBindingRef {
                index: 1,
                buffer: probs_buffer,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 2,
                buffer: &cache.value_buffer,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 3,
                buffer: out_buffer,
                offset_bytes: 0,
            },
        ],
        &[],
        threadgroups_output,
        output_threads_per_threadgroup,
    )?;
    runtime.end_command_batch()?;
    runtime.wait_idle()?;

    let logits_bits = read_attention_logits_bits(
        runtime,
        logits_buffer,
        q_head_count,
        seq_len,
        logits_row_stride,
    )?;
    let prob_bits = read_attention_logits_bits(
        runtime,
        probs_buffer,
        q_head_count,
        seq_len,
        logits_row_stride,
    )?;
    let out_bits = read_bf16_buffer_bits(runtime, out_buffer, q_head_count * head_dim)?;
    Ok((logits_bits, prob_bits, out_bits))
}

fn moe_weighted_expert_out_bits(
    down_bits: &[u32],
    top_k_weights_bits: &[u32],
    hidden: usize,
) -> Result<Vec<u32>, Box<dyn Error>> {
    if top_k_weights_bits.len() != ROUTER_TOP_K {
        return Err(format!(
            "expected {} routed expert weights, got {}",
            ROUTER_TOP_K,
            top_k_weights_bits.len()
        )
        .into());
    }
    let expected_len = ROUTER_TOP_K
        .checked_mul(hidden)
        .ok_or("expert_out size overflow")?;
    if down_bits.len() != expected_len {
        return Err(format!(
            "expert_down length mismatch: got {} expected {}",
            down_bits.len(),
            expected_len
        )
        .into());
    }

    let mut out = Vec::with_capacity(hidden);
    for hidden_index in 0..hidden {
        let mut acc = 0.0f32;
        for (expert_slot, &weight_bits) in top_k_weights_bits.iter().enumerate() {
            let down = f32::from_bits(down_bits[expert_slot * hidden + hidden_index]);
            let weight = f32::from_bits(weight_bits);
            let weighted = bf16_round_to_f32(down * weight);
            acc = bf16_round_to_f32(acc + weighted);
        }
        out.push(acc.to_bits());
    }
    Ok(out)
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

fn print_cached_artifacts(artifacts: &Layer0CachedArtifacts) {
    println!("backend={}", artifacts.backend_name);
    println!("model_path={}", artifacts.model_path.display());
    println!("prefill_rope_offset={}", artifacts.prefill_rope_offset);
    println!("decode_rope_offset={}", artifacts.decode_rope_offset);
    println!("prefill_activation_phase={PREFILL_ACTIVATION_PHASE}");
    println!("decode_activation_phase={DECODE_ACTIVATION_PHASE}");
    println!("q_head_count={}", artifacts.q_head_count);
    println!("k_head_count={}", artifacts.k_head_count);
    println!("v_head_count={}", artifacts.v_head_count);
    println!("q_heads_per_kv={}", artifacts.q_heads_per_kv);
    println!("head_dim={}", artifacts.head_dim);
    println!("stage={}", artifacts.stage_name());
    println!(
        "prefill_k_cache_fnv1a64=0x{:016X}",
        fnv1a64_u32_words(&artifacts.prefill_k_bits)
    );
    println!(
        "prefill_v_proj_fnv1a64=0x{:016X}",
        fnv1a64_u32_words(&artifacts.prefill_v_proj_bits)
    );
    println!(
        "prefill_v_cache_fnv1a64=0x{:016X}",
        fnv1a64_u32_words(&artifacts.prefill_v_bits)
    );
    println!(
        "decode_q_rope_fnv1a64=0x{:016X}",
        fnv1a64_u32_words(&artifacts.decode_q_bits)
    );
    println!(
        "decode_k_rope_fnv1a64=0x{:016X}",
        fnv1a64_u32_words(&artifacts.decode_k_bits)
    );
    println!(
        "decode_v_proj_fnv1a64=0x{:016X}",
        fnv1a64_u32_words(&artifacts.decode_v_proj_bits)
    );
    println!(
        "decode_v_norm_fnv1a64=0x{:016X}",
        fnv1a64_u32_words(&artifacts.decode_v_bits)
    );
    println!(
        "full_k_cache_fnv1a64=0x{:016X}",
        fnv1a64_u32_words(&artifacts.full_k_bits)
    );
    println!(
        "full_v_cache_fnv1a64=0x{:016X}",
        fnv1a64_u32_words(&artifacts.full_v_bits)
    );
    println!(
        "attention_scores_fnv1a64=0x{:016X}",
        fnv1a64_u32_words(&artifacts.attention_score_bits)
    );
    println!(
        "attention_probs_fnv1a64=0x{:016X}",
        fnv1a64_u32_words(&artifacts.attention_prob_bits)
    );
    println!(
        "attention_output_fnv1a64=0x{:016X}",
        fnv1a64_u32_words(&artifacts.attention_out_bits)
    );
    if let Some(bits) = &artifacts.attention_oproj_bits {
        println!("attention_oproj_fnv1a64=0x{:016X}", fnv1a64_u32_words(bits));
    }
    if let Some(bits) = &artifacts.post_attention_norm_bits {
        println!(
            "attention_post_attn_norm_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(bits)
        );
    }
    if let Some(bits) = &artifacts.post_attention_residual_bits {
        println!(
            "attention_post_attn_residual_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(bits)
        );
    }
    if let Some(bits) = &artifacts.pre_feedforward_norm_bits {
        println!(
            "attention_pre_ffn_norm_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(bits)
        );
    }
    if let Some(bits) = &artifacts.dense_gate_bits {
        println!(
            "attention_pre_ffn_gate_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(bits)
        );
    }
    if let Some(bits) = &artifacts.dense_up_bits {
        println!(
            "attention_pre_ffn_up_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(bits)
        );
    }
    if let Some(bits) = &artifacts.dense_geglu_bits {
        println!(
            "attention_pre_ffn_geglu_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(bits)
        );
    }
    if let Some(bits) = &artifacts.dense_down_bits {
        println!(
            "attention_pre_ffn_down_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(bits)
        );
    }
    if let Some(router_output) = &artifacts.router_output {
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
    if let Some(bits) = &artifacts.moe_expert_gate_bits {
        println!(
            "attention_moe_expert_gate_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(bits)
        );
    }
    if let Some(bits) = &artifacts.moe_expert_up_bits {
        println!(
            "attention_moe_expert_up_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(bits)
        );
    }
    if let Some(bits) = &artifacts.moe_expert_geglu_bits {
        println!(
            "attention_moe_expert_geglu_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(bits)
        );
    }
    if let Some(bits) = &artifacts.moe_expert_down_bits {
        println!(
            "attention_moe_expert_down_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(bits)
        );
    }
    if let Some(bits) = &artifacts.post_ffn_norm1_bits {
        println!(
            "attention_post_ffn_norm1_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(bits)
        );
    }
    if let Some(bits) = &artifacts.moe_expert_out_bits {
        println!(
            "attention_moe_expert_out_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(bits)
        );
    }
    if let Some(bits) = &artifacts.moe_post_ffn_norm2_bits {
        println!(
            "attention_moe_post_ffn_norm2_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(bits)
        );
    }
    if let Some(bits) = &artifacts.moe_merge_bits {
        println!(
            "attention_moe_merge_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(bits)
        );
    }
    if let Some(bits) = &artifacts.post_ffn_residual_bits {
        println!(
            "attention_post_ffn_residual_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(bits)
        );
    }
    print_first16(
        "prefill_k_cache_first16_f32_bits",
        &artifacts.prefill_k_bits,
    );
    print_first16(
        "prefill_v_proj_first16_f32_bits",
        &artifacts.prefill_v_proj_bits,
    );
    print_first16(
        "prefill_v_cache_first16_f32_bits",
        &artifacts.prefill_v_bits,
    );
    print_first16("decode_q_rope_first16_f32_bits", &artifacts.decode_q_bits);
    print_first16("decode_k_rope_first16_f32_bits", &artifacts.decode_k_bits);
    print_first16(
        "decode_v_proj_first16_f32_bits",
        &artifacts.decode_v_proj_bits,
    );
    print_first16("decode_v_norm_first16_f32_bits", &artifacts.decode_v_bits);
    print_first16("full_k_cache_first16_f32_bits", &artifacts.full_k_bits);
    print_first16("full_v_cache_first16_f32_bits", &artifacts.full_v_bits);
    print_first16(
        "attention_scores_first16_f32_bits",
        &artifacts.attention_score_bits,
    );
    print_first16(
        "attention_probs_first16_f32_bits",
        &artifacts.attention_prob_bits,
    );
    print_first16(
        "attention_output_first16_f32_bits",
        &artifacts.attention_out_bits,
    );
    if let Some(bits) = &artifacts.attention_oproj_bits {
        print_first16("attention_oproj_first16_f32_bits", bits);
    }
    if let Some(bits) = &artifacts.post_attention_norm_bits {
        print_first16("attention_post_attn_norm_first16_f32_bits", bits);
    }
    if let Some(bits) = &artifacts.post_attention_residual_bits {
        print_first16("attention_post_attn_residual_first16_f32_bits", bits);
    }
    if let Some(bits) = &artifacts.pre_feedforward_norm_bits {
        print_first16("attention_pre_ffn_norm_first16_f32_bits", bits);
    }
    if let Some(bits) = &artifacts.dense_gate_bits {
        print_first16("attention_pre_ffn_gate_first16_f32_bits", bits);
    }
    if let Some(bits) = &artifacts.dense_up_bits {
        print_first16("attention_pre_ffn_up_first16_f32_bits", bits);
    }
    if let Some(bits) = &artifacts.dense_geglu_bits {
        print_first16("attention_pre_ffn_geglu_first16_f32_bits", bits);
    }
    if let Some(bits) = &artifacts.dense_down_bits {
        print_first16("attention_pre_ffn_down_first16_f32_bits", bits);
    }
    if let Some(router_output) = &artifacts.router_output {
        print_first16(
            "router_scaled_first16_f32_bits",
            &router_output.router_scaled_bits,
        );
        print_first16(
            "expert_scores_first16_f32_bits",
            &router_output.expert_scores_bits,
        );
        print_first16(
            "router_probs_first16_f32_bits",
            &router_output.router_probs_bits,
        );
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
    if let Some(bits) = &artifacts.moe_expert_gate_bits {
        print_first16("attention_moe_expert_gate_first16_f32_bits", bits);
    }
    if let Some(bits) = &artifacts.moe_expert_up_bits {
        print_first16("attention_moe_expert_up_first16_f32_bits", bits);
    }
    if let Some(bits) = &artifacts.moe_expert_geglu_bits {
        print_first16("attention_moe_expert_geglu_first16_f32_bits", bits);
    }
    if let Some(bits) = &artifacts.moe_expert_down_bits {
        print_first16("attention_moe_expert_down_first16_f32_bits", bits);
    }
    if let Some(bits) = &artifacts.post_ffn_norm1_bits {
        print_first16("attention_post_ffn_norm1_first16_f32_bits", bits);
    }
    if let Some(bits) = &artifacts.moe_expert_out_bits {
        print_first16("attention_moe_expert_out_first16_f32_bits", bits);
    }
    if let Some(bits) = &artifacts.moe_post_ffn_norm2_bits {
        print_first16("attention_moe_post_ffn_norm2_first16_f32_bits", bits);
    }
    if let Some(bits) = &artifacts.moe_merge_bits {
        print_first16("attention_moe_merge_first16_f32_bits", bits);
    }
    if let Some(bits) = &artifacts.post_ffn_residual_bits {
        print_first16("attention_post_ffn_residual_first16_f32_bits", bits);
    }
    println!("status=ok");
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Layer0CachedStage {
    AttentionOproj = 0,
    PostAttentionResidual = 1,
    PreFeedforwardNorm = 2,
    DenseGate = 3,
    DenseUp = 4,
    DenseGeGlu = 5,
    DenseDown = 6,
    PostFfnNorm1 = 7,
    Router = 8,
    MoeExpertGate = 9,
    MoeExpertUp = 10,
    MoeExpertGeGlu = 11,
    MoeExpertDown = 12,
    MoeExpertOut = 13,
    MoePostFfnNorm2 = 14,
    MoeMerge = 15,
    PostFfnResidual = 16,
}

impl Layer0CachedStage {
    const fn bit(self) -> u32 {
        1u32 << (self as u8)
    }

    pub const fn cli_flag(self) -> &'static str {
        match self {
            Self::AttentionOproj => "--oproj",
            Self::PostAttentionResidual => "--residual",
            Self::PreFeedforwardNorm => "--pre-ffn-norm",
            Self::DenseGate => "--dense-gate",
            Self::DenseUp => "--dense-up",
            Self::DenseGeGlu => "--dense-geglu",
            Self::DenseDown => "--dense-down",
            Self::PostFfnNorm1 => "--post-ffn-norm1",
            Self::Router => "--router",
            Self::MoeExpertGate => "--moe-expert-gate",
            Self::MoeExpertUp => "--moe-expert-up",
            Self::MoeExpertGeGlu => "--moe-expert-geglu",
            Self::MoeExpertDown => "--moe-expert-down",
            Self::MoeExpertOut => "--moe-expert-out",
            Self::MoePostFfnNorm2 => "--moe-post-ffn-norm2",
            Self::MoeMerge => "--moe-merge",
            Self::PostFfnResidual => "--post-ffn-residual",
        }
    }

    pub const fn stage_name(self) -> &'static str {
        match self {
            Self::AttentionOproj => "attention_oproj_cached",
            Self::PostAttentionResidual => "attention_post_attn_residual_cached",
            Self::PreFeedforwardNorm => "attention_pre_ffn_norm_cached",
            Self::DenseGate => "attention_pre_ffn_gate_cached",
            Self::DenseUp => "attention_pre_ffn_up_cached",
            Self::DenseGeGlu => "attention_pre_ffn_geglu_cached",
            Self::DenseDown => "attention_pre_ffn_down_cached",
            Self::PostFfnNorm1 => "attention_post_ffn_norm1_cached",
            Self::Router => "attention_router_cached",
            Self::MoeExpertGate => "attention_moe_expert_gate_cached",
            Self::MoeExpertUp => "attention_moe_expert_up_cached",
            Self::MoeExpertGeGlu => "attention_moe_expert_geglu_cached",
            Self::MoeExpertDown => "attention_moe_expert_down_cached",
            Self::MoeExpertOut => "attention_moe_expert_out_cached",
            Self::MoePostFfnNorm2 => "attention_moe_post_ffn_norm2_cached",
            Self::MoeMerge => "attention_moe_merge_cached",
            Self::PostFfnResidual => "attention_post_ffn_residual_cached",
        }
    }

    pub fn from_cli_flag(flag: &str) -> Option<Self> {
        match flag {
            "--oproj" => Some(Self::AttentionOproj),
            "--residual" => Some(Self::PostAttentionResidual),
            "--pre-ffn-norm" => Some(Self::PreFeedforwardNorm),
            "--dense-gate" => Some(Self::DenseGate),
            "--dense-up" => Some(Self::DenseUp),
            "--dense-geglu" => Some(Self::DenseGeGlu),
            "--dense-down" => Some(Self::DenseDown),
            "--post-ffn-norm1" => Some(Self::PostFfnNorm1),
            "--router" => Some(Self::Router),
            "--moe-expert-gate" => Some(Self::MoeExpertGate),
            "--moe-expert-up" => Some(Self::MoeExpertUp),
            "--moe-expert-geglu" => Some(Self::MoeExpertGeGlu),
            "--moe-expert-down" => Some(Self::MoeExpertDown),
            "--moe-expert-out" => Some(Self::MoeExpertOut),
            "--moe-post-ffn-norm2" => Some(Self::MoePostFfnNorm2),
            "--moe-merge" => Some(Self::MoeMerge),
            "--post-ffn-residual" | "--layer-output" => Some(Self::PostFfnResidual),
            _ => None,
        }
    }
}

const LAYER0_CACHED_EVALUATION_ORDER: [Layer0CachedStage; 17] = [
    Layer0CachedStage::AttentionOproj,
    Layer0CachedStage::PostAttentionResidual,
    Layer0CachedStage::PreFeedforwardNorm,
    Layer0CachedStage::DenseGate,
    Layer0CachedStage::DenseUp,
    Layer0CachedStage::DenseGeGlu,
    Layer0CachedStage::DenseDown,
    Layer0CachedStage::PostFfnNorm1,
    Layer0CachedStage::Router,
    Layer0CachedStage::MoeExpertGate,
    Layer0CachedStage::MoeExpertUp,
    Layer0CachedStage::MoeExpertGeGlu,
    Layer0CachedStage::MoeExpertDown,
    Layer0CachedStage::MoeExpertOut,
    Layer0CachedStage::MoePostFfnNorm2,
    Layer0CachedStage::MoeMerge,
    Layer0CachedStage::PostFfnResidual,
];

const LAYER0_CACHED_DISPLAY_ORDER: [Layer0CachedStage; 17] = [
    Layer0CachedStage::PostFfnResidual,
    Layer0CachedStage::MoeMerge,
    Layer0CachedStage::MoePostFfnNorm2,
    Layer0CachedStage::MoeExpertOut,
    Layer0CachedStage::PostFfnNorm1,
    Layer0CachedStage::DenseDown,
    Layer0CachedStage::MoeExpertDown,
    Layer0CachedStage::MoeExpertGeGlu,
    Layer0CachedStage::MoeExpertUp,
    Layer0CachedStage::MoeExpertGate,
    Layer0CachedStage::DenseGeGlu,
    Layer0CachedStage::DenseUp,
    Layer0CachedStage::DenseGate,
    Layer0CachedStage::Router,
    Layer0CachedStage::PreFeedforwardNorm,
    Layer0CachedStage::PostAttentionResidual,
    Layer0CachedStage::AttentionOproj,
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Layer0CachedPlan {
    mask: u32,
}

impl Layer0CachedPlan {
    pub const fn new() -> Self {
        Self { mask: 0 }
    }

    pub const fn is_empty(self) -> bool {
        self.mask == 0
    }

    pub const fn requires(self, stage: Layer0CachedStage) -> bool {
        (self.mask & stage.bit()) != 0
    }

    pub fn require_stage(&mut self, stage: Layer0CachedStage) {
        match stage {
            Layer0CachedStage::AttentionOproj => {}
            Layer0CachedStage::PostAttentionResidual => {
                self.require_stage(Layer0CachedStage::AttentionOproj);
            }
            Layer0CachedStage::PreFeedforwardNorm => {
                self.require_stage(Layer0CachedStage::PostAttentionResidual);
            }
            Layer0CachedStage::DenseGate | Layer0CachedStage::DenseUp => {
                self.require_stage(Layer0CachedStage::PreFeedforwardNorm);
            }
            Layer0CachedStage::DenseGeGlu => {
                self.require_stage(Layer0CachedStage::DenseGate);
                self.require_stage(Layer0CachedStage::DenseUp);
            }
            Layer0CachedStage::DenseDown => {
                self.require_stage(Layer0CachedStage::DenseGeGlu);
            }
            Layer0CachedStage::PostFfnNorm1 => {
                self.require_stage(Layer0CachedStage::DenseDown);
            }
            Layer0CachedStage::Router => {
                self.require_stage(Layer0CachedStage::PostAttentionResidual);
            }
            Layer0CachedStage::MoeExpertGate => {
                self.require_stage(Layer0CachedStage::Router);
            }
            Layer0CachedStage::MoeExpertUp => {
                self.require_stage(Layer0CachedStage::MoeExpertGate);
            }
            Layer0CachedStage::MoeExpertGeGlu => {
                self.require_stage(Layer0CachedStage::MoeExpertUp);
            }
            Layer0CachedStage::MoeExpertDown => {
                self.require_stage(Layer0CachedStage::MoeExpertGeGlu);
            }
            Layer0CachedStage::MoeExpertOut => {
                self.require_stage(Layer0CachedStage::MoeExpertDown);
            }
            Layer0CachedStage::MoePostFfnNorm2 => {
                self.require_stage(Layer0CachedStage::MoeExpertOut);
            }
            Layer0CachedStage::MoeMerge => {
                self.require_stage(Layer0CachedStage::PostFfnNorm1);
                self.require_stage(Layer0CachedStage::MoePostFfnNorm2);
            }
            Layer0CachedStage::PostFfnResidual => {
                self.require_stage(Layer0CachedStage::MoeMerge);
            }
        }
        self.mask |= stage.bit();
    }

    pub fn evaluation_order(self) -> Vec<Layer0CachedStage> {
        LAYER0_CACHED_EVALUATION_ORDER
            .iter()
            .copied()
            .filter(|stage| self.requires(*stage))
            .collect()
    }

    pub fn display_stage(self) -> Option<Layer0CachedStage> {
        LAYER0_CACHED_DISPLAY_ORDER
            .iter()
            .copied()
            .find(|stage| self.requires(*stage))
    }
}

pub fn run_cli() -> Result<(), Box<dyn Error>> {
    let mut model_path = default_model_path();
    let mut layer_idx = 0usize;
    let mut plan = Layer0CachedPlan::new();
    let mut prefill_token_ids = Vec::<u32>::new();
    let mut decode_token_id = None;
    let mut prefill_input_f32_files = Vec::<PathBuf>::new();
    let mut decode_input_f32_file = None;
    let mut write_prefill_layer_output_f32_file = None;
    let mut write_layer_output_f32_file = None;
    let mut prefill_rope_offset = PREFILL_ROPE_OFFSET;
    let mut decode_rope_offset = DECODE_ROPE_OFFSET;
    let mut args_iter = env::args().skip(1);
    while let Some(arg) = args_iter.next() {
        if let Some(stage) = Layer0CachedStage::from_cli_flag(arg.as_str()) {
            plan.require_stage(stage);
            continue;
        }
        match arg.as_str() {
            "-h" | "--help" => {
                eprintln!(
                    "Usage: metal_qkv_attention_output_cached_row [model.safetensors] [--layer N] [--prefill-token ID] [--prefill-tokens ID,ID,...] [--decode-token ID] [--prefill-input-f32-file PATH] [--prefill-input-f32-files PATH,PATH,...] [--decode-input-f32-file PATH] [--write-prefill-layer-output-f32-file PATH] [--write-layer-output-f32-file PATH] [--prefill-position N] [--decode-position N] [--oproj] [--residual] [--pre-ffn-norm] [--dense-gate] [--dense-up] [--dense-geglu] [--dense-down] [--post-ffn-norm1] [--router] [--moe-expert-gate] [--moe-expert-up] [--moe-expert-geglu] [--moe-expert-down] [--moe-expert-out] [--moe-post-ffn-norm2] [--moe-merge] [--post-ffn-residual|--layer-output]"
                );
                return Ok(());
            }
            "--layer" => {
                let value = args_iter.next().ok_or("--layer expects a value")?;
                layer_idx = value.parse()?;
            }
            "--prefill-token" => {
                let value = args_iter.next().ok_or("--prefill-token expects a value")?;
                prefill_token_ids.push(value.parse()?);
            }
            "--prefill-tokens" => {
                let value = args_iter.next().ok_or("--prefill-tokens expects a value")?;
                for token in value.split(',').filter(|token| !token.is_empty()) {
                    prefill_token_ids.push(token.parse()?);
                }
            }
            "--decode-token" => {
                let value = args_iter.next().ok_or("--decode-token expects a value")?;
                decode_token_id = Some(value.parse()?);
            }
            "--prefill-input-f32-file" => {
                let value = args_iter
                    .next()
                    .ok_or("--prefill-input-f32-file expects a value")?;
                prefill_input_f32_files.push(PathBuf::from(value));
            }
            "--prefill-input-f32-files" => {
                let value = args_iter
                    .next()
                    .ok_or("--prefill-input-f32-files expects a value")?;
                for path in value.split(',').filter(|path| !path.is_empty()) {
                    prefill_input_f32_files.push(PathBuf::from(path));
                }
            }
            "--decode-input-f32-file" => {
                let value = args_iter
                    .next()
                    .ok_or("--decode-input-f32-file expects a value")?;
                decode_input_f32_file = Some(PathBuf::from(value));
            }
            "--write-prefill-layer-output-f32-file" => {
                let value = args_iter
                    .next()
                    .ok_or("--write-prefill-layer-output-f32-file expects a value")?;
                write_prefill_layer_output_f32_file = Some(PathBuf::from(value));
            }
            "--write-layer-output-f32-file" => {
                let value = args_iter
                    .next()
                    .ok_or("--write-layer-output-f32-file expects a value")?;
                write_layer_output_f32_file = Some(PathBuf::from(value));
            }
            "--prefill-position" => {
                let value = args_iter
                    .next()
                    .ok_or("--prefill-position expects a value")?;
                prefill_rope_offset = value.parse()?;
            }
            "--decode-position" => {
                let value = args_iter
                    .next()
                    .ok_or("--decode-position expects a value")?;
                decode_rope_offset = value.parse()?;
            }
            _ if arg.starts_with("--") => {
                return Err(format!("unknown option {arg}").into());
            }
            _ => {
                model_path = PathBuf::from(arg);
            }
        }
    }

    let using_file_inputs = !prefill_input_f32_files.is_empty() || decode_input_f32_file.is_some();
    let using_token_inputs = !prefill_token_ids.is_empty() || decode_token_id.is_some();
    if using_file_inputs && using_token_inputs {
        return Err("choose either token-id inputs or f32 hidden-state inputs, not both".into());
    }

    let artifacts = if !using_file_inputs && !using_token_inputs {
        run_layer_plan(
            model_path,
            layer_idx,
            CachedLayerInputs::synthetic_case(),
            plan,
        )?
    } else if using_file_inputs {
        let decode_input_f32_file = decode_input_f32_file
            .ok_or("--decode-input-f32-file must be provided with f32 hidden-state inputs")?;
        if prefill_input_f32_files.is_empty() {
            return Err("at least one --prefill-input-f32-file is required".into());
        }
        let prefill_input_words_list = prefill_input_f32_files
            .iter()
            .map(|path| read_f32_file_as_bf16_words(path))
            .collect::<Result<Vec<_>, _>>()?;
        let decode_input_words = read_f32_file_as_bf16_words(&decode_input_f32_file)?;
        run_layer_plan_from_sequence(
            model_path,
            layer_idx,
            CachedLayerSequenceInputs {
                prefill_input_words_list,
                decode_input_words,
                prefill_rope_offset,
                decode_rope_offset,
                validate_against_oracle: false,
            },
            plan,
        )?
    } else {
        let decode_token_id = decode_token_id.ok_or(
            "--decode-token must be provided when using --prefill-token or --prefill-tokens",
        )?;
        let mut session = LayerExecutionSession::load(model_path.clone())?;
        let prefill_input_words_list = prefill_token_ids
            .iter()
            .copied()
            .map(|token_id| session.weights.embed_token_bf16_words(token_id))
            .collect::<Result<Vec<_>, _>>()?;
        if prefill_input_words_list.is_empty() {
            return Err("at least one prefill token is required".into());
        }
        let decode_input_words = session.weights.embed_token_bf16_words(decode_token_id)?;
        run_layer_plan_with_session_from_sequence(
            &mut session,
            layer_idx,
            CachedLayerSequenceInputs {
                prefill_input_words_list,
                decode_input_words,
                prefill_rope_offset,
                decode_rope_offset,
                validate_against_oracle: false,
            },
            plan,
        )?
    };
    print_cached_artifacts(&artifacts);
    if let Some(path) = write_prefill_layer_output_f32_file {
        let prefill_words = artifacts.prefill_layer_output_bf16_words().ok_or(
            "--write-prefill-layer-output-f32-file requires prefill post-ffn residual output",
        )?;
        write_bf16_words_as_f32_file(&path, &prefill_words)?;
        println!("prefill_layer_output_f32_file={}", path.display());
    }
    if let Some(path) = write_layer_output_f32_file {
        let decode_words = artifacts
            .bf16_words_for_stage(Layer0CachedStage::PostFfnResidual)
            .ok_or("--write-layer-output-f32-file requires post-ffn residual output")?;
        write_bf16_words_as_f32_file(&path, &decode_words)?;
        println!("layer_output_f32_file={}", path.display());
    }
    Ok(())
}

pub fn run_plan(
    model_path: PathBuf,
    plan: Layer0CachedPlan,
) -> Result<Layer0CachedArtifacts, Box<dyn Error>> {
    run_layer_plan(model_path, 0, CachedLayerInputs::synthetic_case(), plan)
}

pub fn run_layer_plan(
    model_path: PathBuf,
    layer_idx: usize,
    inputs: CachedLayerInputs,
    plan: Layer0CachedPlan,
) -> Result<Layer0CachedArtifacts, Box<dyn Error>> {
    let mut session = LayerExecutionSession::load(model_path)?;
    run_layer_plan_with_session(&mut session, layer_idx, inputs, plan)
}

pub fn run_layer_plan_from_sequence(
    model_path: PathBuf,
    layer_idx: usize,
    inputs: CachedLayerSequenceInputs,
    plan: Layer0CachedPlan,
) -> Result<Layer0CachedArtifacts, Box<dyn Error>> {
    let mut session = LayerExecutionSession::load(model_path)?;
    run_layer_plan_with_session_from_sequence(&mut session, layer_idx, inputs, plan)
}

fn run_layer_plan_with_session(
    session: &mut LayerExecutionSession,
    layer_idx: usize,
    inputs: CachedLayerInputs,
    plan: Layer0CachedPlan,
) -> Result<Layer0CachedArtifacts, Box<dyn Error>> {
    run_layer_plan_with_session_from_sequence(
        session,
        layer_idx,
        CachedLayerSequenceInputs::from_single(inputs),
        plan,
    )
}

fn run_layer_plan_with_session_from_sequence(
    session: &mut LayerExecutionSession,
    layer_idx: usize,
    inputs: CachedLayerSequenceInputs,
    plan: Layer0CachedPlan,
) -> Result<Layer0CachedArtifacts, Box<dyn Error>> {
    let validate_oproj = plan.requires(Layer0CachedStage::AttentionOproj);
    let validate_residual = plan.requires(Layer0CachedStage::PostAttentionResidual);
    let validate_pre_ffn_norm = plan.requires(Layer0CachedStage::PreFeedforwardNorm);
    let validate_dense_gate = plan.requires(Layer0CachedStage::DenseGate);
    let validate_dense_up = plan.requires(Layer0CachedStage::DenseUp);
    let validate_dense_geglu = plan.requires(Layer0CachedStage::DenseGeGlu);
    let validate_dense_down = plan.requires(Layer0CachedStage::DenseDown);
    let validate_post_ffn_norm1 = plan.requires(Layer0CachedStage::PostFfnNorm1);
    let validate_router = plan.requires(Layer0CachedStage::Router);
    let validate_moe_expert_gate = plan.requires(Layer0CachedStage::MoeExpertGate);
    let validate_moe_expert_up = plan.requires(Layer0CachedStage::MoeExpertUp);
    let validate_moe_expert_geglu = plan.requires(Layer0CachedStage::MoeExpertGeGlu);
    let validate_moe_expert_down = plan.requires(Layer0CachedStage::MoeExpertDown);
    let validate_moe_expert_out = plan.requires(Layer0CachedStage::MoeExpertOut);
    let validate_moe_post_ffn_norm2 = plan.requires(Layer0CachedStage::MoePostFfnNorm2);
    let validate_moe_merge = plan.requires(Layer0CachedStage::MoeMerge);
    let validate_post_ffn_residual = plan.requires(Layer0CachedStage::PostFfnResidual);

    let model_path = session.model_path.clone();
    let weights = session.weights.clone();
    let runtime = session.runtime.clone();
    let layer_type = weights
        .snapshot
        .config
        .text_config
        .layer_types
        .get(layer_idx)
        .ok_or_else(|| format!("missing text layer type for layer {layer_idx}"))?;
    let attention_k_eq_v =
        weights.snapshot.config.text_config.attention_k_eq_v && layer_type == "full_attention";
    let layer_names = LayerTensorNames::for_layer(layer_idx, attention_k_eq_v);

    let q_weight_entry = weights
        .tensor(&layer_names.q.weight_name)
        .map_err(|_| "missing q projection weight entry")?;
    let q_scales_entry = weights
        .tensor(&layer_names.q.scales_name)
        .map_err(|_| "missing q projection scales entry")?;
    let q_norm_weight_entry = weights
        .tensor(
            layer_names
                .q
                .norm_weight_name
                .as_deref()
                .ok_or("missing q norm weight name")?,
        )
        .map_err(|_| "missing q norm weight entry")?;
    let k_weight_entry = weights
        .tensor(&layer_names.k.weight_name)
        .map_err(|_| "missing k projection weight entry")?;
    let k_scales_entry = weights
        .tensor(&layer_names.k.scales_name)
        .map_err(|_| "missing k projection scales entry")?;
    let k_norm_weight_entry = weights
        .tensor(
            layer_names
                .k
                .norm_weight_name
                .as_deref()
                .ok_or("missing k norm weight name")?,
        )
        .map_err(|_| "missing k norm weight entry")?;
    let v_weight_entry = weights
        .tensor(&layer_names.v.weight_name)
        .map_err(|_| "missing v projection weight entry")?;
    let v_scales_entry = weights
        .tensor(&layer_names.v.scales_name)
        .map_err(|_| "missing v projection scales entry")?;
    let o_weight_entry = if validate_oproj {
        Some(
            weights
                .tensor(&layer_names.o.weight_name)
                .map_err(|_| "missing o_proj weight entry")?,
        )
    } else {
        None
    };
    let o_scales_entry = if validate_oproj {
        Some(
            weights
                .tensor(&layer_names.o.scales_name)
                .map_err(|_| "missing o_proj scales entry")?,
        )
    } else {
        None
    };
    let post_attention_norm_weight_entry = if validate_residual {
        Some(
            weights
                .tensor(&layer_names.post_attention_norm_weight_name)
                .map_err(|_| "missing post-attention norm weight entry")?,
        )
    } else {
        None
    };
    let pre_feedforward_norm_weight_entry = if validate_pre_ffn_norm {
        Some(
            weights
                .tensor(&layer_names.pre_feedforward_norm_weight_name)
                .map_err(|_| "missing pre-feedforward norm weight entry")?,
        )
    } else {
        None
    };
    let pre_feedforward_norm2_weight_entry = if validate_moe_expert_gate {
        Some(
            weights
                .tensor(&layer_names.pre_feedforward_norm2_weight_name)
                .map_err(|_| "missing pre-feedforward norm2 weight entry")?,
        )
    } else {
        None
    };
    let post_feedforward_norm1_weight_entry = if validate_post_ffn_norm1 {
        Some(
            weights
                .tensor(&layer_names.post_feedforward_norm1_weight_name)
                .map_err(|_| "missing post-feedforward norm1 weight entry")?,
        )
    } else {
        None
    };
    let post_feedforward_norm2_weight_entry = if validate_moe_post_ffn_norm2 {
        Some(
            weights
                .tensor(&layer_names.post_feedforward_norm2_weight_name)
                .map_err(|_| "missing post-feedforward norm2 weight entry")?,
        )
    } else {
        None
    };
    let mlp_gate_weight_entry = if validate_dense_gate {
        Some(
            weights
                .tensor(&layer_names.mlp_gate_weight_name)
                .map_err(|_| "missing mlp gate_proj weight entry")?,
        )
    } else {
        None
    };
    let mlp_gate_scales_entry = if validate_dense_gate {
        Some(
            weights
                .tensor(&layer_names.mlp_gate_scales_name)
                .map_err(|_| "missing mlp gate_proj scales entry")?,
        )
    } else {
        None
    };
    let mlp_up_weight_entry = if validate_dense_up {
        Some(
            weights
                .tensor(&layer_names.mlp_up_weight_name)
                .map_err(|_| "missing mlp up_proj weight entry")?,
        )
    } else {
        None
    };
    let mlp_up_scales_entry = if validate_dense_up {
        Some(
            weights
                .tensor(&layer_names.mlp_up_scales_name)
                .map_err(|_| "missing mlp up_proj scales entry")?,
        )
    } else {
        None
    };
    let mlp_down_weight_entry = if validate_dense_down {
        Some(
            weights
                .tensor(&layer_names.mlp_down_weight_name)
                .map_err(|_| "missing mlp down_proj weight entry")?,
        )
    } else {
        None
    };
    let mlp_down_scales_entry = if validate_dense_down {
        Some(
            weights
                .tensor(&layer_names.mlp_down_scales_name)
                .map_err(|_| "missing mlp down_proj scales entry")?,
        )
    } else {
        None
    };
    let router_scale_entry = if validate_router {
        Some(
            weights
                .tensor(&layer_names.router_scale_name)
                .map_err(|_| "missing router scale entry")?,
        )
    } else {
        None
    };
    let router_proj_weight_entry = if validate_router {
        Some(
            weights
                .tensor(&layer_names.router_proj_weight_name)
                .map_err(|_| "missing router proj weight entry")?,
        )
    } else {
        None
    };
    let router_proj_scales_entry = if validate_router {
        Some(
            weights
                .tensor(&layer_names.router_proj_scales_name)
                .map_err(|_| "missing router proj scales entry")?,
        )
    } else {
        None
    };
    let expert_gate_weight_entry = if validate_moe_expert_gate {
        Some(
            weights
                .tensor(&layer_names.expert_gate_weight_name)
                .map_err(|_| "missing expert gate weight entry")?,
        )
    } else {
        None
    };
    let expert_gate_scales_entry = if validate_moe_expert_gate {
        Some(
            weights
                .tensor(&layer_names.expert_gate_scales_name)
                .map_err(|_| "missing expert gate scales entry")?,
        )
    } else {
        None
    };
    let expert_up_weight_entry = if validate_moe_expert_up {
        Some(
            weights
                .tensor(&layer_names.expert_up_weight_name)
                .map_err(|_| "missing expert up weight entry")?,
        )
    } else {
        None
    };
    let expert_up_scales_entry = if validate_moe_expert_up {
        Some(
            weights
                .tensor(&layer_names.expert_up_scales_name)
                .map_err(|_| "missing expert up scales entry")?,
        )
    } else {
        None
    };
    let expert_down_weight_entry = if validate_moe_expert_down {
        Some(
            weights
                .tensor(&layer_names.expert_down_weight_name)
                .map_err(|_| "missing expert down weight entry")?,
        )
    } else {
        None
    };
    let expert_down_scales_entry = if validate_moe_expert_down {
        Some(
            weights
                .tensor(&layer_names.expert_down_scales_name)
                .map_err(|_| "missing expert down scales entry")?,
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
    let pre_feedforward_norm2_len = if let Some(entry) = pre_feedforward_norm2_weight_entry {
        usize::try_from(entry.shape[0])?
    } else {
        0
    };
    let post_feedforward_norm1_len = if let Some(entry) = post_feedforward_norm1_weight_entry {
        usize::try_from(entry.shape[0])?
    } else {
        0
    };
    let post_feedforward_norm2_len = if let Some(entry) = post_feedforward_norm2_weight_entry {
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
    let expert_gate_out_len = if let Some(entry) = expert_gate_weight_entry {
        usize::try_from(entry.shape[1])?
    } else {
        0
    };
    let expert_up_out_len = if let Some(entry) = expert_up_weight_entry {
        usize::try_from(entry.shape[1])?
    } else {
        0
    };
    let expert_down_out_len = if let Some(entry) = expert_down_weight_entry {
        usize::try_from(entry.shape[1])?
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
    let rope_params = if layer_type == "full_attention" {
        &weights
            .snapshot
            .config
            .text_config
            .rope_parameters
            .full_attention
    } else {
        &weights
            .snapshot
            .config
            .text_config
            .rope_parameters
            .sliding_attention
    };
    let rope_rotary_dim = if let Some(partial_factor) = rope_params.partial_rotary_factor {
        let rotary_dim = (head_dim as f32 * partial_factor).round() as usize;
        if rotary_dim == 0 || rotary_dim > head_dim || rotary_dim % 2 != 0 {
            return Err(format!(
                "invalid rope rotary dim {} for layer {} head_dim {} factor {}",
                rotary_dim, layer_idx, head_dim, partial_factor
            )
            .into());
        }
        rotary_dim
    } else {
        head_dim
    };
    let rope_half_dims = rope_rotary_dim / 2;
    let rope_base = rope_params.rope_theta as f32;
    let layer_attention_kind = if layer_type == "full_attention" {
        GemmaAttentionKind::Full
    } else {
        GemmaAttentionKind::Sliding
    };
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
    if validate_post_ffn_norm1 && post_feedforward_norm1_len != mlp_down_out_len {
        return Err(format!(
            "invalid post-feedforward norm1 length: got {} expected {}",
            post_feedforward_norm1_len, mlp_down_out_len
        )
        .into());
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
    if validate_moe_expert_gate && pre_feedforward_norm2_len != post_attention_norm_len {
        return Err(format!(
            "invalid pre-feedforward norm2 length: got {} expected {}",
            pre_feedforward_norm2_len, post_attention_norm_len
        )
        .into());
    }
    if let Some(weight_entry) = expert_gate_weight_entry {
        let expert_gate_n_in = usize::try_from(weight_entry.shape[2] * 8)?;
        if expert_gate_n_in != pre_feedforward_norm2_len {
            return Err(format!(
                "invalid expert gate input size: got {} expected {}",
                expert_gate_n_in, pre_feedforward_norm2_len
            )
            .into());
        }
    }
    if let Some(weight_entry) = expert_up_weight_entry {
        let expert_up_n_in = usize::try_from(weight_entry.shape[2] * 8)?;
        if expert_up_n_in != pre_feedforward_norm2_len {
            return Err(format!(
                "invalid expert up input size: got {} expected {}",
                expert_up_n_in, pre_feedforward_norm2_len
            )
            .into());
        }
    }
    if let Some(weight_entry) = expert_down_weight_entry {
        let expert_down_n_in = usize::try_from(weight_entry.shape[2] * 8)?;
        if expert_down_n_in != expert_gate_out_len {
            return Err(format!(
                "invalid expert down input size: got {} expected {}",
                expert_down_n_in, expert_gate_out_len
            )
            .into());
        }
        if expert_down_out_len != pre_feedforward_norm2_len {
            return Err(format!(
                "invalid expert down output size: got {} expected {}",
                expert_down_out_len, pre_feedforward_norm2_len
            )
            .into());
        }
    }
    if validate_moe_post_ffn_norm2 && post_feedforward_norm2_len != expert_down_out_len {
        return Err(format!(
            "invalid post-feedforward norm2 length: got {} expected {}",
            post_feedforward_norm2_len, expert_down_out_len
        )
        .into());
    }

    let CachedLayerSequenceInputs {
        prefill_input_words_list,
        decode_input_words: decode_x_words,
        prefill_rope_offset,
        decode_rope_offset,
        validate_against_oracle,
    } = inputs;
    if prefill_input_words_list.is_empty() {
        return Err("cached layer sequence requires at least one prefill input".into());
    }
    for (prefill_index, prefill_x_words) in prefill_input_words_list.iter().enumerate() {
        if prefill_x_words.len() != NORM_LEN {
            return Err(format!(
                "prefill input length mismatch at index {}: got {} expected {}",
                prefill_index,
                prefill_x_words.len(),
                NORM_LEN
            )
            .into());
        }
    }
    if decode_x_words.len() != NORM_LEN {
        return Err(format!(
            "decode input length mismatch: got {} expected {}",
            decode_x_words.len(),
            NORM_LEN
        )
        .into());
    }
    let kv_capacity = prefill_input_words_list.len() + 1;
    let x_buf = runtime.create_buffer(NORM_LEN * 2, BufferStorageMode::Shared)?;
    let input_norm_weight_buf =
        session.private_weight_buffer(&layer_names.input_norm_weight_name)?;
    let h_buf = runtime.create_buffer(NORM_LEN * 2, BufferStorageMode::Private)?;

    let q_weight_buf = session.private_weight_buffer(&layer_names.q.weight_name)?;
    let q_scales_buf = session.private_weight_buffer(&layer_names.q.scales_name)?;
    let q_biases_buf = session.private_weight_buffer(&layer_names.q.biases_name)?;
    let q_norm_weight_buf = session.private_weight_buffer(
        layer_names
            .q
            .norm_weight_name
            .as_deref()
            .ok_or("missing q norm weight name")?,
    )?;
    let q_proj_buf = runtime.create_buffer(q_out_len * 2, BufferStorageMode::Private)?;
    let q_norm_buf = runtime.create_buffer(q_out_len * 2, BufferStorageMode::Private)?;
    let q_rope_buf = runtime.create_buffer(q_out_len * 2, BufferStorageMode::Private)?;

    let k_weight_buf = session.private_weight_buffer(&layer_names.k.weight_name)?;
    let k_scales_buf = session.private_weight_buffer(&layer_names.k.scales_name)?;
    let k_biases_buf = session.private_weight_buffer(&layer_names.k.biases_name)?;
    let k_norm_weight_buf = session.private_weight_buffer(
        layer_names
            .k
            .norm_weight_name
            .as_deref()
            .ok_or("missing k norm weight name")?,
    )?;
    let k_proj_buf = runtime.create_buffer(k_out_len * 2, BufferStorageMode::Private)?;
    let k_norm_buf = runtime.create_buffer(k_out_len * 2, BufferStorageMode::Private)?;
    let k_rope_buf = runtime.create_buffer(k_out_len * 2, BufferStorageMode::Private)?;

    let v_weight_buf = session.private_weight_buffer(&layer_names.v.weight_name)?;
    let v_scales_buf = session.private_weight_buffer(&layer_names.v.scales_name)?;
    let v_biases_buf = session.private_weight_buffer(&layer_names.v.biases_name)?;
    let v_proj_buf = runtime.create_buffer(v_out_len * 2, BufferStorageMode::Private)?;
    let v_norm_buf = runtime.create_buffer(v_out_len * 2, BufferStorageMode::Private)?;
    let ones_bytes = bytes_from_bf16_words(&vec![0x3F80u16; head_dim]);
    let v_norm_weight_buf =
        runtime.create_buffer_with_bytes(&ones_bytes, BufferStorageMode::Private)?;
    let attention_logits_buf =
        runtime.create_buffer(q_head_count * kv_capacity * 2, BufferStorageMode::Private)?;
    let o_weight_buf =
        optional_private_weight_buffer(session, validate_oproj, &layer_names.o.weight_name)?;
    let o_scales_buf =
        optional_private_weight_buffer(session, validate_oproj, &layer_names.o.scales_name)?;
    let o_biases_buf =
        optional_private_weight_buffer(session, validate_oproj, &layer_names.o.biases_name)?;
    let post_attention_norm_weight_buf = optional_private_weight_buffer(
        session,
        validate_residual,
        &layer_names.post_attention_norm_weight_name,
    )?;
    let pre_feedforward_norm_weight_buf = optional_private_weight_buffer(
        session,
        validate_pre_ffn_norm,
        &layer_names.pre_feedforward_norm_weight_name,
    )?;
    let pre_feedforward_norm2_weight_buf = optional_private_weight_buffer(
        session,
        validate_moe_expert_gate,
        &layer_names.pre_feedforward_norm2_weight_name,
    )?;
    let post_feedforward_norm1_weight_buf = optional_private_weight_buffer(
        session,
        validate_post_ffn_norm1,
        &layer_names.post_feedforward_norm1_weight_name,
    )?;
    let post_feedforward_norm2_weight_buf = optional_private_weight_buffer(
        session,
        validate_moe_post_ffn_norm2,
        &layer_names.post_feedforward_norm2_weight_name,
    )?;
    let mlp_gate_weight_buf = optional_private_weight_buffer(
        session,
        validate_dense_gate,
        &layer_names.mlp_gate_weight_name,
    )?;
    let mlp_gate_scales_buf = optional_private_weight_buffer(
        session,
        validate_dense_gate,
        &layer_names.mlp_gate_scales_name,
    )?;
    let mlp_gate_biases_buf = optional_private_weight_buffer(
        session,
        validate_dense_gate,
        &layer_names.mlp_gate_biases_name,
    )?;
    let mlp_up_weight_buf = optional_private_weight_buffer(
        session,
        validate_dense_up,
        &layer_names.mlp_up_weight_name,
    )?;
    let mlp_up_scales_buf = optional_private_weight_buffer(
        session,
        validate_dense_up,
        &layer_names.mlp_up_scales_name,
    )?;
    let mlp_up_biases_buf = optional_private_weight_buffer(
        session,
        validate_dense_up,
        &layer_names.mlp_up_biases_name,
    )?;
    let mlp_down_weight_buf = optional_private_weight_buffer(
        session,
        validate_dense_down,
        &layer_names.mlp_down_weight_name,
    )?;
    let mlp_down_scales_buf = optional_private_weight_buffer(
        session,
        validate_dense_down,
        &layer_names.mlp_down_scales_name,
    )?;
    let mlp_down_biases_buf = optional_private_weight_buffer(
        session,
        validate_dense_down,
        &layer_names.mlp_down_biases_name,
    )?;
    let router_scale_weight_buf =
        optional_private_weight_buffer(session, validate_router, &layer_names.router_scale_name)?;
    let router_proj_weight_buf = optional_private_weight_buffer(
        session,
        validate_router,
        &layer_names.router_proj_weight_name,
    )?;
    let router_proj_scales_buf = optional_private_weight_buffer(
        session,
        validate_router,
        &layer_names.router_proj_scales_name,
    )?;
    let router_proj_biases_buf = optional_private_weight_buffer(
        session,
        validate_router,
        &layer_names.router_proj_biases_name,
    )?;
    let router_per_expert_scale_buf = optional_private_weight_buffer(
        session,
        validate_router,
        &layer_names.router_per_expert_scale_name,
    )?;
    let expert_gate_weight_buf = optional_private_weight_buffer(
        session,
        validate_moe_expert_gate,
        &layer_names.expert_gate_weight_name,
    )?;
    let expert_gate_scales_buf = optional_private_weight_buffer(
        session,
        validate_moe_expert_gate,
        &layer_names.expert_gate_scales_name,
    )?;
    let expert_gate_biases_buf = optional_private_weight_buffer(
        session,
        validate_moe_expert_gate,
        &layer_names.expert_gate_biases_name,
    )?;
    let expert_up_weight_buf = optional_private_weight_buffer(
        session,
        validate_moe_expert_up,
        &layer_names.expert_up_weight_name,
    )?;
    let expert_up_scales_buf = optional_private_weight_buffer(
        session,
        validate_moe_expert_up,
        &layer_names.expert_up_scales_name,
    )?;
    let expert_up_biases_buf = optional_private_weight_buffer(
        session,
        validate_moe_expert_up,
        &layer_names.expert_up_biases_name,
    )?;
    let expert_down_weight_buf = optional_private_weight_buffer(
        session,
        validate_moe_expert_down,
        &layer_names.expert_down_weight_name,
    )?;
    let expert_down_scales_buf = optional_private_weight_buffer(
        session,
        validate_moe_expert_down,
        &layer_names.expert_down_scales_name,
    )?;
    let expert_down_biases_buf = optional_private_weight_buffer(
        session,
        validate_moe_expert_down,
        &layer_names.expert_down_biases_name,
    )?;
    let attention_probs_buf =
        Some(runtime.create_buffer(q_head_count * kv_capacity * 2, BufferStorageMode::Private)?);
    let attn_out_buf = Some(runtime.create_buffer(q_out_len * 2, BufferStorageMode::Private)?);
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
    let router_probs_out_buf = if validate_router {
        Some(runtime.create_buffer(router_out_len * 2, BufferStorageMode::Shared)?)
    } else {
        None
    };
    let pre_feedforward_norm2_out_buf = if validate_moe_expert_gate {
        Some(runtime.create_buffer(pre_feedforward_norm2_len * 2, BufferStorageMode::Private)?)
    } else {
        None
    };
    let moe_top_k_indices_buf = if validate_router {
        Some(runtime.create_buffer(ROUTER_TOP_K * size_of::<u32>(), BufferStorageMode::Shared)?)
    } else {
        None
    };
    let moe_top_k_weights_buf = if validate_router {
        Some(runtime.create_buffer(ROUTER_TOP_K * size_of::<u16>(), BufferStorageMode::Shared)?)
    } else {
        None
    };
    let expert_gate_out_buf = if validate_moe_expert_gate {
        Some(runtime.create_buffer(
            ROUTER_TOP_K * expert_gate_out_len * 2,
            BufferStorageMode::Private,
        )?)
    } else {
        None
    };
    let expert_up_out_buf = if validate_moe_expert_up {
        Some(runtime.create_buffer(
            ROUTER_TOP_K * expert_up_out_len * 2,
            BufferStorageMode::Private,
        )?)
    } else {
        None
    };
    let expert_geglu_out_buf = if validate_moe_expert_geglu {
        Some(runtime.create_buffer(
            ROUTER_TOP_K * expert_gate_out_len * 2,
            BufferStorageMode::Private,
        )?)
    } else {
        None
    };
    let expert_down_out_buf = if validate_moe_expert_down {
        Some(runtime.create_buffer(
            ROUTER_TOP_K * expert_down_out_len * 2,
            BufferStorageMode::Private,
        )?)
    } else {
        None
    };
    let post_feedforward_norm1_out_buf = if validate_post_ffn_norm1 {
        Some(runtime.create_buffer(post_feedforward_norm1_len * 2, BufferStorageMode::Private)?)
    } else {
        None
    };
    let moe_weighted_out_buf = if validate_moe_post_ffn_norm2 {
        Some(runtime.create_buffer(post_feedforward_norm2_len * 2, BufferStorageMode::Shared)?)
    } else {
        None
    };
    let moe_post_ffn_norm2_out_buf = if validate_moe_post_ffn_norm2 {
        Some(runtime.create_buffer(post_feedforward_norm2_len * 2, BufferStorageMode::Private)?)
    } else {
        None
    };
    let moe_merge_out_buf = if validate_moe_merge {
        Some(runtime.create_buffer(post_feedforward_norm1_len * 2, BufferStorageMode::Private)?)
    } else {
        None
    };
    let post_ffn_residual_out_buf = if validate_post_ffn_residual {
        Some(runtime.create_buffer(post_feedforward_norm1_len * 2, BufferStorageMode::Private)?)
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
    let attention_logits_seq_pipeline =
        runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
            cache_name: "kernel_mlx_gqa_attention_logits_seq_bf16".to_string(),
            base_name: "kernel_mlx_gqa_attention_logits_seq_bf16".to_string(),
            constants: Vec::new(),
            smem_bytes: 0,
            nr0: 0,
            nr1: 0,
            nsg: 0,
        })?;
    let attention_softmax_pipeline = runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
        cache_name: "kernel_mlx_softmax_rows_bf16".to_string(),
        base_name: "kernel_mlx_softmax_rows_bf16".to_string(),
        constants: Vec::new(),
        smem_bytes: 0,
        nr0: 0,
        nr1: 0,
        nsg: 0,
    })?;
    let attention_weighted_sum_pipeline =
        runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
            cache_name: "kernel_mlx_gqa_attention_weighted_sum_bf16".to_string(),
            base_name: "kernel_mlx_gqa_attention_weighted_sum_bf16".to_string(),
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
    let geglu_pipeline = if validate_dense_geglu || validate_moe_expert_geglu {
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
    let router_topk_pipeline = if validate_router {
        Some(runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
            cache_name: "kernel_mlx_router_topk_bf16".to_string(),
            base_name: "kernel_mlx_router_topk_bf16".to_string(),
            constants: Vec::new(),
            smem_bytes: 0,
            nr0: 0,
            nr1: 0,
            nsg: 0,
        })?)
    } else {
        None
    };
    let selected_expert_proj_pipeline = if validate_moe_expert_gate {
        Some(runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
            cache_name: "kernel_mlx_affine_qmv_selected_experts_row_bf16".to_string(),
            base_name: "kernel_mlx_affine_qmv_selected_experts_row_bf16".to_string(),
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
    let o_proj_layout =
        if let (Some(weight_entry), Some(scales_entry)) = (o_weight_entry, o_scales_entry) {
            Some(ExactMetalQprojLayout {
                weight_words_per_row: weight_entry.shape[1] as u32,
                qparams_per_row: scales_entry.shape[1] as u32,
                out_rows: o_out_len as u32,
            })
        } else {
            None
        };
    let o_proj_args = o_proj_layout.map(|layout| layout.row_args(q_out_len as u32));
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
    let router_softmax_args = if validate_router {
        Some(MlxSoftmaxRowsArgs {
            row_stride: router_out_len as u32,
            row_count: 1,
            seq_len: router_out_len as u32,
        })
    } else {
        None
    };
    let router_topk_args = if validate_router {
        Some(MlxRouterTopKArgs {
            expert_count: router_out_len as u32,
            top_k: ROUTER_TOP_K as u32,
        })
    } else {
        None
    };
    let pre_feedforward_norm2_args = if validate_moe_expert_gate {
        Some(MlxRmsNormRowArgs {
            n: pre_feedforward_norm2_len as u32,
            eps: EPS,
        })
    } else {
        None
    };
    let expert_gate_selected_args = if let (Some(weight_entry), Some(scales_entry)) =
        (expert_gate_weight_entry, expert_gate_scales_entry)
    {
        Some(MlxAffineSelectedExpertsQprojRowArgs {
            n_in: pre_feedforward_norm2_len as u32,
            weight_words_per_row: weight_entry.shape[2] as u32,
            qparams_per_row: scales_entry.shape[2] as u32,
            out_rows: expert_gate_out_len as u32,
            input_row_stride: 0,
        })
    } else {
        None
    };
    let expert_up_selected_args = if let (Some(weight_entry), Some(scales_entry)) =
        (expert_up_weight_entry, expert_up_scales_entry)
    {
        Some(MlxAffineSelectedExpertsQprojRowArgs {
            n_in: pre_feedforward_norm2_len as u32,
            weight_words_per_row: weight_entry.shape[2] as u32,
            qparams_per_row: scales_entry.shape[2] as u32,
            out_rows: expert_up_out_len as u32,
            input_row_stride: 0,
        })
    } else {
        None
    };
    let moe_expert_geglu_args = if validate_moe_expert_geglu {
        Some(MlxGegluRowArgs {
            n: (ROUTER_TOP_K * expert_gate_out_len) as u32,
        })
    } else {
        None
    };
    let expert_down_selected_args = if let (Some(weight_entry), Some(scales_entry)) =
        (expert_down_weight_entry, expert_down_scales_entry)
    {
        Some(MlxAffineSelectedExpertsQprojRowArgs {
            n_in: expert_gate_out_len as u32,
            weight_words_per_row: weight_entry.shape[2] as u32,
            qparams_per_row: scales_entry.shape[2] as u32,
            out_rows: expert_down_out_len as u32,
            input_row_stride: expert_gate_out_len as u32,
        })
    } else {
        None
    };
    let post_ffn_norm1_args = if validate_post_ffn_norm1 {
        Some(MlxRmsNormRowArgs {
            n: post_feedforward_norm1_len as u32,
            eps: EPS,
        })
    } else {
        None
    };
    let moe_post_ffn_norm2_args = if validate_moe_post_ffn_norm2 {
        Some(MlxRmsNormRowArgs {
            n: post_feedforward_norm2_len as u32,
            eps: EPS,
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
        MetalBufferBindingRef {
            index: 1,
            buffer: &x_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 2,
            buffer: &input_norm_weight_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 3,
            buffer: &h_buf,
            offset_bytes: 0,
        },
    ];
    let q_proj_bindings = [
        MetalBufferBindingRef {
            index: 1,
            buffer: &h_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 2,
            buffer: &q_weight_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 3,
            buffer: &q_scales_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 4,
            buffer: &q_biases_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 5,
            buffer: &q_proj_buf,
            offset_bytes: 0,
        },
    ];
    let q_head_norm_bindings = [
        MetalBufferBindingRef {
            index: 1,
            buffer: &q_proj_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 2,
            buffer: &q_norm_weight_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 3,
            buffer: &q_norm_buf,
            offset_bytes: 0,
        },
    ];
    let k_proj_bindings = [
        MetalBufferBindingRef {
            index: 1,
            buffer: &h_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 2,
            buffer: &k_weight_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 3,
            buffer: &k_scales_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 4,
            buffer: &k_biases_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 5,
            buffer: &k_proj_buf,
            offset_bytes: 0,
        },
    ];
    let k_head_norm_bindings = [
        MetalBufferBindingRef {
            index: 1,
            buffer: &k_proj_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 2,
            buffer: &k_norm_weight_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 3,
            buffer: &k_norm_buf,
            offset_bytes: 0,
        },
    ];
    let v_proj_bindings = [
        MetalBufferBindingRef {
            index: 1,
            buffer: &h_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 2,
            buffer: &v_weight_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 3,
            buffer: &v_scales_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 4,
            buffer: &v_biases_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 5,
            buffer: &v_proj_buf,
            offset_bytes: 0,
        },
    ];
    let v_head_norm_bindings = [
        MetalBufferBindingRef {
            index: 1,
            buffer: &v_proj_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 2,
            buffer: &v_norm_weight_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 3,
            buffer: &v_norm_buf,
            offset_bytes: 0,
        },
    ];
    let router_scale_bindings = if let (Some(scale_buf), Some(out_buf)) = (
        router_scale_weight_buf.as_ref(),
        router_scaled_out_buf.as_ref(),
    ) {
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
    let mlp_gate_bindings =
        if let (Some(weight_buf), Some(scales_buf), Some(biases_buf), Some(out_buf)) = (
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
    let mlp_up_bindings =
        if let (Some(weight_buf), Some(scales_buf), Some(biases_buf), Some(out_buf)) = (
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
    let mlp_down_bindings =
        if let (Some(weight_buf), Some(scales_buf), Some(biases_buf), Some(out_buf)) = (
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
    let router_proj_bindings =
        if let (Some(weight_buf), Some(scales_buf), Some(biases_buf), Some(out_buf)) = (
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
    let pre_feedforward_norm2_bindings = if let (Some(weight_buf), Some(out_buf)) = (
        pre_feedforward_norm2_weight_buf.as_ref(),
        pre_feedforward_norm2_out_buf.as_ref(),
    ) {
        Some([
            MetalBufferBindingRef {
                index: 1,
                buffer: residual_out_buf
                    .as_ref()
                    .ok_or("missing residual output buffer for pre-feedforward norm2")?,
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
    let expert_gate_selected_bindings = if let (
        Some(indices_buf),
        Some(weight_buf),
        Some(scales_buf),
        Some(biases_buf),
        Some(out_buf),
    ) = (
        moe_top_k_indices_buf.as_ref(),
        expert_gate_weight_buf.as_ref(),
        expert_gate_scales_buf.as_ref(),
        expert_gate_biases_buf.as_ref(),
        expert_gate_out_buf.as_ref(),
    ) {
        Some([
            MetalBufferBindingRef {
                index: 1,
                buffer: pre_feedforward_norm2_out_buf
                    .as_ref()
                    .ok_or("missing pre-feedforward norm2 output buffer")?,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 2,
                buffer: indices_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 3,
                buffer: weight_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 4,
                buffer: scales_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 5,
                buffer: biases_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 6,
                buffer: out_buf,
                offset_bytes: 0,
            },
        ])
    } else {
        None
    };
    let expert_up_selected_bindings = if let (
        Some(indices_buf),
        Some(weight_buf),
        Some(scales_buf),
        Some(biases_buf),
        Some(out_buf),
    ) = (
        moe_top_k_indices_buf.as_ref(),
        expert_up_weight_buf.as_ref(),
        expert_up_scales_buf.as_ref(),
        expert_up_biases_buf.as_ref(),
        expert_up_out_buf.as_ref(),
    ) {
        Some([
            MetalBufferBindingRef {
                index: 1,
                buffer: pre_feedforward_norm2_out_buf
                    .as_ref()
                    .ok_or("missing pre-feedforward norm2 output buffer")?,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 2,
                buffer: indices_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 3,
                buffer: weight_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 4,
                buffer: scales_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 5,
                buffer: biases_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 6,
                buffer: out_buf,
                offset_bytes: 0,
            },
        ])
    } else {
        None
    };
    let moe_expert_geglu_bindings = if let (Some(out_buf), Some(gate_buf), Some(up_buf)) = (
        expert_geglu_out_buf.as_ref(),
        expert_gate_out_buf.as_ref(),
        expert_up_out_buf.as_ref(),
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
    let expert_down_selected_bindings = if let (
        Some(indices_buf),
        Some(weight_buf),
        Some(scales_buf),
        Some(biases_buf),
        Some(out_buf),
    ) = (
        moe_top_k_indices_buf.as_ref(),
        expert_down_weight_buf.as_ref(),
        expert_down_scales_buf.as_ref(),
        expert_down_biases_buf.as_ref(),
        expert_down_out_buf.as_ref(),
    ) {
        Some([
            MetalBufferBindingRef {
                index: 1,
                buffer: expert_geglu_out_buf
                    .as_ref()
                    .ok_or("missing expert geglu output buffer")?,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 2,
                buffer: indices_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 3,
                buffer: weight_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 4,
                buffer: scales_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 5,
                buffer: biases_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 6,
                buffer: out_buf,
                offset_bytes: 0,
            },
        ])
    } else {
        None
    };
    let post_ffn_norm1_bindings = if let (Some(weight_buf), Some(out_buf)) = (
        post_feedforward_norm1_weight_buf.as_ref(),
        post_feedforward_norm1_out_buf.as_ref(),
    ) {
        Some([
            MetalBufferBindingRef {
                index: 1,
                buffer: mlp_down_out_buf
                    .as_ref()
                    .ok_or("missing dense down output buffer for post-ffn norm1")?,
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
    let moe_post_ffn_norm2_bindings = if let (Some(weight_buf), Some(out_buf), Some(weighted_buf)) = (
        post_feedforward_norm2_weight_buf.as_ref(),
        moe_post_ffn_norm2_out_buf.as_ref(),
        moe_weighted_out_buf.as_ref(),
    ) {
        Some([
            MetalBufferBindingRef {
                index: 1,
                buffer: weighted_buf,
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
    let moe_merge_bindings = if let (Some(dense_buf), Some(moe_buf), Some(out_buf)) = (
        post_feedforward_norm1_out_buf.as_ref(),
        moe_post_ffn_norm2_out_buf.as_ref(),
        moe_merge_out_buf.as_ref(),
    ) {
        Some([
            MetalBufferBindingRef {
                index: 1,
                buffer: dense_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 2,
                buffer: moe_buf,
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
    let post_ffn_residual_bindings = if let (Some(base_buf), Some(merge_buf), Some(out_buf)) = (
        residual_out_buf.as_ref(),
        moe_merge_out_buf.as_ref(),
        post_ffn_residual_out_buf.as_ref(),
    ) {
        Some([
            MetalBufferBindingRef {
                index: 1,
                buffer: base_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 2,
                buffer: merge_buf,
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

    let rms_threadgroups = MetalSize {
        width: 1,
        height: 1,
        depth: 1,
    };
    let rms_threads_per_threadgroup = MetalSize {
        width: rms_threadgroup_size as u64,
        height: 1,
        depth: 1,
    };
    let q_proj_threadgroups = MetalSize {
        width: 1,
        height: (q_out_len as u64).div_ceil(8),
        depth: 1,
    };
    let q_proj_threads_per_threadgroup = MetalSize {
        width: 32,
        height: 2,
        depth: 1,
    };
    let q_head_norm_threadgroups = MetalSize {
        width: q_head_count as u64,
        height: 1,
        depth: 1,
    };
    let q_head_norm_threads_per_threadgroup = MetalSize {
        width: head_norm_threadgroup_size as u64,
        height: 1,
        depth: 1,
    };
    let k_proj_threadgroups = MetalSize {
        width: 1,
        height: (k_out_len as u64).div_ceil(8),
        depth: 1,
    };
    let k_proj_threads_per_threadgroup = MetalSize {
        width: 32,
        height: 2,
        depth: 1,
    };
    let k_head_norm_threadgroups = MetalSize {
        width: k_head_count as u64,
        height: 1,
        depth: 1,
    };
    let k_head_norm_threads_per_threadgroup = MetalSize {
        width: head_norm_threadgroup_size as u64,
        height: 1,
        depth: 1,
    };
    let v_proj_threadgroups = MetalSize {
        width: 1,
        height: (v_out_len as u64).div_ceil(8),
        depth: 1,
    };
    let v_proj_threads_per_threadgroup = MetalSize {
        width: 32,
        height: 2,
        depth: 1,
    };
    let v_head_norm_threadgroups = MetalSize {
        width: v_head_count as u64,
        height: 1,
        depth: 1,
    };
    let v_head_norm_threads_per_threadgroup = MetalSize {
        width: head_norm_threadgroup_size as u64,
        height: 1,
        depth: 1,
    };
    let o_proj_threadgroups = MetalSize {
        width: 1,
        height: (o_out_len as u64).div_ceil(8),
        depth: 1,
    };
    let o_proj_threads_per_threadgroup = MetalSize {
        width: 32,
        height: 2,
        depth: 1,
    };
    let residual_threads_per_threadgroup = MetalSize {
        width: 256,
        height: 1,
        depth: 1,
    };
    let residual_threadgroups = MetalSize {
        width: (post_attention_norm_len as u64).div_ceil(residual_threads_per_threadgroup.width),
        height: 1,
        depth: 1,
    };
    let pre_feedforward_norm_threadgroups = MetalSize {
        width: 1,
        height: 1,
        depth: 1,
    };
    let mlp_gate_threadgroups = MetalSize {
        width: 1,
        height: (mlp_gate_out_len as u64).div_ceil(8),
        depth: 1,
    };
    let mlp_gate_threads_per_threadgroup = MetalSize {
        width: 32,
        height: 2,
        depth: 1,
    };
    let mlp_up_threadgroups = MetalSize {
        width: 1,
        height: (mlp_up_out_len as u64).div_ceil(8),
        depth: 1,
    };
    let mlp_up_threads_per_threadgroup = MetalSize {
        width: 32,
        height: 2,
        depth: 1,
    };
    let geglu_threads_per_threadgroup = MetalSize {
        width: 256,
        height: 1,
        depth: 1,
    };
    let geglu_threadgroups = MetalSize {
        width: (mlp_gate_out_len as u64).div_ceil(geglu_threads_per_threadgroup.width),
        height: 1,
        depth: 1,
    };
    let mlp_down_threadgroups = MetalSize {
        width: 1,
        height: (mlp_down_out_len as u64).div_ceil(8),
        depth: 1,
    };
    let mlp_down_threads_per_threadgroup = MetalSize {
        width: 32,
        height: 2,
        depth: 1,
    };
    let router_scale_threads_per_threadgroup = MetalSize {
        width: rms_threadgroup_size as u64,
        height: 1,
        depth: 1,
    };
    let router_scale_threadgroups = MetalSize {
        width: 1,
        height: 1,
        depth: 1,
    };
    let router_proj_threadgroups = MetalSize {
        width: 1,
        height: (router_out_len as u64).div_ceil(8),
        depth: 1,
    };
    let router_proj_threads_per_threadgroup = MetalSize {
        width: 32,
        height: 2,
        depth: 1,
    };
    let router_softmax_threadgroups = MetalSize {
        width: 1,
        height: 1,
        depth: 1,
    };
    let router_topk_threadgroups = MetalSize {
        width: 1,
        height: 1,
        depth: 1,
    };
    let router_topk_threads_per_threadgroup = MetalSize {
        width: 1,
        height: 1,
        depth: 1,
    };
    let expert_gate_selected_threadgroups = MetalSize {
        width: ROUTER_TOP_K as u64,
        height: (expert_gate_out_len as u64).div_ceil(8),
        depth: 1,
    };
    let expert_gate_threads_per_threadgroup = MetalSize {
        width: 32,
        height: 2,
        depth: 1,
    };
    let expert_up_selected_threadgroups = MetalSize {
        width: ROUTER_TOP_K as u64,
        height: (expert_up_out_len as u64).div_ceil(8),
        depth: 1,
    };
    let expert_up_threads_per_threadgroup = MetalSize {
        width: 32,
        height: 2,
        depth: 1,
    };
    let moe_expert_geglu_threadgroups = MetalSize {
        width: ((ROUTER_TOP_K * expert_gate_out_len) as u64)
            .div_ceil(geglu_threads_per_threadgroup.width),
        height: 1,
        depth: 1,
    };
    let expert_down_selected_threadgroups = MetalSize {
        width: ROUTER_TOP_K as u64,
        height: (expert_down_out_len as u64).div_ceil(8),
        depth: 1,
    };
    let expert_down_threads_per_threadgroup = MetalSize {
        width: 32,
        height: 2,
        depth: 1,
    };

    let run_projection =
        |input_words: &[u16],
         rope_offset: i32|
         -> Result<(Vec<u32>, Vec<u32>, Vec<u32>, Vec<u32>, Vec<u32>), Box<dyn Error>> {
            runtime.write_buffer(&x_buf, 0, &bytes_from_bf16_words(input_words))?;

            let q_rope_args = MlxRopeSingleArgs {
                half_dims: rope_half_dims as u32,
                row_stride: head_dim as u32,
                row_count: q_head_count as u32,
                offset: rope_offset,
                scale: ROPE_SCALE,
                base_log2: rope_base.log2(),
            };
            let k_rope_args = MlxRopeSingleArgs {
                half_dims: rope_half_dims as u32,
                row_stride: head_dim as u32,
                row_count: k_head_count as u32,
                offset: rope_offset,
                scale: ROPE_SCALE,
                base_log2: rope_base.log2(),
            };
            let q_rope_bindings = [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &q_norm_buf,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &q_rope_buf,
                    offset_bytes: 0,
                },
            ];
            let k_rope_bindings = [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &k_norm_buf,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &k_rope_buf,
                    offset_bytes: 0,
                },
            ];
            let q_rope_threadgroups = MetalSize {
                width: (rope_half_dims as u64).div_ceil(32),
                height: q_head_count as u64,
                depth: 1,
            };
            let q_rope_threads_per_threadgroup = MetalSize {
                width: 32,
                height: 1,
                depth: 1,
            };
            let k_rope_threadgroups = MetalSize {
                width: (rope_half_dims as u64).div_ceil(32),
                height: k_head_count as u64,
                depth: 1,
            };
            let k_rope_threads_per_threadgroup = MetalSize {
                width: 32,
                height: 1,
                depth: 1,
            };

            runtime.begin_command_batch()?;
            runtime.dispatch_compute(
                &rms_pipeline,
                bytes_of(&rms_args),
                &rms_bindings,
                &[],
                rms_threadgroups,
                rms_threads_per_threadgroup,
            )?;
            runtime.memory_barrier_buffers()?;
            runtime.dispatch_compute(
                &proj_pipeline,
                bytes_of(&q_proj_args),
                &q_proj_bindings,
                &[],
                q_proj_threadgroups,
                q_proj_threads_per_threadgroup,
            )?;
            runtime.memory_barrier_buffers()?;
            runtime.dispatch_compute(
                &head_norm_pipeline,
                bytes_of(&q_head_norm_args),
                &q_head_norm_bindings,
                &[],
                q_head_norm_threadgroups,
                q_head_norm_threads_per_threadgroup,
            )?;
            runtime.memory_barrier_buffers()?;
            if rope_half_dims * 2 < head_dim {
                runtime.copy_buffer_range(
                    &q_norm_buf,
                    0,
                    &q_rope_buf,
                    0,
                    q_out_len * size_of::<u16>(),
                )?;
            }
            runtime.dispatch_compute(
                &rope_pipeline,
                bytes_of(&q_rope_args),
                &q_rope_bindings,
                &[],
                q_rope_threadgroups,
                q_rope_threads_per_threadgroup,
            )?;
            runtime.memory_barrier_buffers()?;
            runtime.dispatch_compute(
                &proj_pipeline,
                bytes_of(&k_proj_args),
                &k_proj_bindings,
                &[],
                k_proj_threadgroups,
                k_proj_threads_per_threadgroup,
            )?;
            runtime.memory_barrier_buffers()?;
            runtime.dispatch_compute(
                &head_norm_pipeline,
                bytes_of(&k_head_norm_args),
                &k_head_norm_bindings,
                &[],
                k_head_norm_threadgroups,
                k_head_norm_threads_per_threadgroup,
            )?;
            runtime.memory_barrier_buffers()?;
            if rope_half_dims * 2 < head_dim {
                runtime.copy_buffer_range(
                    &k_norm_buf,
                    0,
                    &k_rope_buf,
                    0,
                    k_out_len * size_of::<u16>(),
                )?;
            }
            runtime.dispatch_compute(
                &rope_pipeline,
                bytes_of(&k_rope_args),
                &k_rope_bindings,
                &[],
                k_rope_threadgroups,
                k_rope_threads_per_threadgroup,
            )?;
            runtime.memory_barrier_buffers()?;
            runtime.dispatch_compute(
                &proj_pipeline,
                bytes_of(&v_proj_args),
                &v_proj_bindings,
                &[],
                v_proj_threadgroups,
                v_proj_threads_per_threadgroup,
            )?;
            runtime.memory_barrier_buffers()?;
            runtime.dispatch_compute(
                &head_norm_pipeline,
                bytes_of(&v_head_norm_args),
                &v_head_norm_bindings,
                &[],
                v_head_norm_threadgroups,
                v_head_norm_threads_per_threadgroup,
            )?;
            runtime.end_command_batch()?;
            runtime.wait_idle()?;

            let input_norm_bits =
                decode_bf16_buffer_bits(&runtime.read_buffer(&h_buf, NORM_LEN * 2)?);
            let v_proj_bits =
                decode_bf16_buffer_bits(&runtime.read_buffer(&v_proj_buf, v_out_len * 2)?);
            let q_bits = decode_bf16_buffer_bits(&runtime.read_buffer(&q_rope_buf, q_out_len * 2)?);
            let k_bits = decode_bf16_buffer_bits(&runtime.read_buffer(&k_rope_buf, k_out_len * 2)?);
            let v_bits = decode_bf16_buffer_bits(&runtime.read_buffer(&v_norm_buf, v_out_len * 2)?);
            Ok((input_norm_bits, v_proj_bits, q_bits, k_bits, v_bits))
        };

    let mut kv_cache = ExactMetalKvCache::load(
        &runtime,
        GemmaKvCacheSpec::new(layer_attention_kind, 1, k_head_count, head_dim, kv_capacity)?,
    )?;
    let mut prefill_attention_cache = if validate_post_ffn_residual {
        Some(ExactMetalKvCache::load(
            &runtime,
            GemmaKvCacheSpec::new(layer_attention_kind, 1, k_head_count, head_dim, kv_capacity)?,
        )?)
    } else {
        None
    };
    let mut prefill_input_norm_bits = Vec::new();
    let mut prefill_v_proj_bits = Vec::new();
    let mut prefill_q_bits = Vec::new();
    let mut prefill_k_bits = Vec::new();
    let mut prefill_v_bits = Vec::new();
    let mut prefill_x_words = Vec::new();
    let mut prefill_attention_out_bits = None;
    for (prefill_index, input_words) in prefill_input_words_list.iter().enumerate() {
        let rope_offset = prefill_rope_offset + prefill_index as i32;
        let (
            current_input_norm_bits,
            current_v_proj_bits,
            current_q_bits,
            current_k_bits,
            current_v_bits,
        ) = run_projection(input_words, rope_offset)?;
        kv_cache.append_token_from_buffers(&runtime, &k_rope_buf, &v_norm_buf)?;
        if let Some(cache) = prefill_attention_cache.as_mut() {
            cache.append_token_from_buffers(&runtime, &k_rope_buf, &v_norm_buf)?;
            if prefill_index + 1 == prefill_input_words_list.len() {
                prefill_attention_out_bits = Some(
                    compute_cached_attention_metal(
                        &runtime,
                        &attention_logits_seq_pipeline,
                        &attention_softmax_pipeline,
                        &attention_weighted_sum_pipeline,
                        &q_rope_buf,
                        cache,
                        q_head_count,
                        q_heads_per_kv,
                        head_dim,
                        &attention_logits_buf,
                        attention_probs_buf
                            .as_ref()
                            .ok_or("missing attention probs buffer for prefill attention")?,
                        attn_out_buf
                            .as_ref()
                            .ok_or("missing attention output buffer for prefill attention")?,
                    )?
                    .2,
                );
            }
        }
        if prefill_index + 1 == prefill_input_words_list.len() {
            prefill_input_norm_bits = current_input_norm_bits;
            prefill_v_proj_bits = current_v_proj_bits;
            prefill_q_bits = current_q_bits;
            prefill_k_bits = current_k_bits;
            prefill_v_bits = current_v_bits;
            prefill_x_words = input_words.clone();
        }
    }
    let (decode_input_norm_bits, decode_v_proj_bits, decode_q_bits, decode_k_bits, decode_v_bits) =
        run_projection(&decode_x_words, decode_rope_offset)?;

    kv_cache.append_token_from_buffers(&runtime, &k_rope_buf, &v_norm_buf)?;

    let full_k_bits = read_exact_kv_cache_tensor_bits(&runtime, &kv_cache, &kv_cache.key_buffer)?;
    let full_v_bits = read_exact_kv_cache_tensor_bits(&runtime, &kv_cache, &kv_cache.value_buffer)?;
    let (attention_score_bits, attention_prob_bits, attention_out_bits) =
        compute_cached_attention_metal(
            &runtime,
            &attention_logits_seq_pipeline,
            &attention_softmax_pipeline,
            &attention_weighted_sum_pipeline,
            &q_rope_buf,
            &kv_cache,
            q_head_count,
            q_heads_per_kv,
            head_dim,
            &attention_logits_buf,
            attention_probs_buf
                .as_ref()
                .ok_or("missing attention probs buffer for decode attention")?,
            attn_out_buf
                .as_ref()
                .ok_or("missing attention output buffer for decode attention")?,
        )?;
    let attention_oproj_bits = if validate_oproj {
        let attn_out_buf = attn_out_buf
            .as_ref()
            .ok_or("missing attention output buffer")?;
        let o_proj_out_buf = o_proj_out_buf
            .as_ref()
            .ok_or("missing attention o_proj output buffer")?;
        let o_weight_buf = o_weight_buf
            .as_ref()
            .ok_or("missing o_proj weight buffer")?;
        let o_scales_buf = o_scales_buf
            .as_ref()
            .ok_or("missing o_proj scales buffer")?;
        let o_biases_buf = o_biases_buf
            .as_ref()
            .ok_or("missing o_proj biases buffer")?;
        let o_proj_fast_pipeline = o_proj_fast_pipeline
            .as_ref()
            .ok_or("missing o_proj fast pipeline")?;
        let o_proj_layout = o_proj_layout.as_ref().ok_or("missing o_proj layout")?;
        let o_proj_args = o_proj_args.as_ref().ok_or("missing o_proj args")?;
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
        dispatch_exact_mlx_qmv_row(
            &runtime,
            &proj_pipeline,
            o_proj_fast_pipeline,
            *o_proj_layout,
            o_proj_args,
            &o_proj_bindings,
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
        if let (Some(args), Some(bindings)) =
            (pre_feedforward_norm_args, &pre_feedforward_norm_bindings)
        {
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
        Some((
            post_attention_norm_bits,
            residual_bits,
            pre_feedforward_norm_bits,
        ))
    } else {
        None
    };
    let dense_gate_bits = if validate_dense_gate {
        let mlp_gate_args = mlp_gate_args.as_ref().ok_or("missing mlp gate args")?;
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
        let mlp_up_bindings = mlp_up_bindings.as_ref().ok_or("missing mlp up bindings")?;
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
        let geglu_pipeline = geglu_pipeline.as_ref().ok_or("missing geglu pipeline")?;
        let geglu_args = geglu_args.as_ref().ok_or("missing geglu args")?;
        let geglu_bindings = geglu_bindings.as_ref().ok_or("missing geglu bindings")?;
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
        let router_softmax_args = router_softmax_args
            .as_ref()
            .ok_or("missing router softmax args")?;
        let router_topk_args = router_topk_args
            .as_ref()
            .ok_or("missing router top-k args")?;
        let router_proj_bindings = router_proj_bindings
            .as_ref()
            .ok_or("missing router proj bindings")?;
        let router_topk_pipeline = router_topk_pipeline
            .as_ref()
            .ok_or("missing router top-k pipeline")?;
        let router_scaled_out_buf = router_scaled_out_buf
            .as_ref()
            .ok_or("missing router scaled output buffer")?;
        let router_proj_out_buf = router_proj_out_buf
            .as_ref()
            .ok_or("missing router proj output buffer")?;
        let router_probs_out_buf = router_probs_out_buf
            .as_ref()
            .ok_or("missing router probs output buffer")?;
        let router_per_expert_scale_buf = router_per_expert_scale_buf
            .as_ref()
            .ok_or("missing router per-expert scale buffer")?;
        let moe_top_k_indices_buf = moe_top_k_indices_buf
            .as_ref()
            .ok_or("missing moe top-k indices buffer")?;
        let moe_top_k_weights_buf = moe_top_k_weights_buf
            .as_ref()
            .ok_or("missing moe top-k weights buffer")?;
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
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            &attention_softmax_pipeline,
            bytes_of(router_softmax_args),
            &[
                MetalBufferBindingRef {
                    index: 1,
                    buffer: router_proj_out_buf,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: router_probs_out_buf,
                    offset_bytes: 0,
                },
            ],
            &[],
            router_softmax_threadgroups,
            mlx_softmax_threads_per_threadgroup(
                router_out_len,
                attention_softmax_pipeline.max_threads_per_threadgroup,
            )?,
        )?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            router_topk_pipeline,
            bytes_of(router_topk_args),
            &[
                MetalBufferBindingRef {
                    index: 1,
                    buffer: router_proj_out_buf,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: router_probs_out_buf,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: router_per_expert_scale_buf,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 4,
                    buffer: moe_top_k_indices_buf,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 5,
                    buffer: moe_top_k_weights_buf,
                    offset_bytes: 0,
                },
            ],
            &[],
            router_topk_threadgroups,
            router_topk_threads_per_threadgroup,
        )?;
        runtime.end_command_batch()?;
        runtime.wait_idle()?;
        Some(read_router_output_from_device(
            &runtime,
            router_scaled_out_buf,
            router_proj_out_buf,
            router_probs_out_buf,
            moe_top_k_indices_buf,
            moe_top_k_weights_buf,
            post_attention_norm_len,
            router_out_len,
            ROUTER_TOP_K,
        )?)
    } else {
        None
    };
    let (moe_expert_gate_bits, moe_expert_up_bits, moe_expert_geglu_bits, moe_expert_down_bits) =
        if validate_moe_expert_gate {
            let router_output = router_output
                .as_ref()
                .ok_or("missing router output for moe expert gate")?;
            let pre_feedforward_norm2_args = pre_feedforward_norm2_args
                .as_ref()
                .ok_or("missing pre-feedforward norm2 args")?;
            let pre_feedforward_norm2_bindings = pre_feedforward_norm2_bindings
                .as_ref()
                .ok_or("missing pre-feedforward norm2 bindings")?;
            let selected_expert_proj_pipeline = selected_expert_proj_pipeline
                .as_ref()
                .ok_or("missing selected expert projection pipeline")?;
            let expert_gate_selected_args = expert_gate_selected_args
                .as_ref()
                .ok_or("missing expert gate selected args")?;
            let expert_gate_selected_bindings = expert_gate_selected_bindings
                .as_ref()
                .ok_or("missing expert gate selected bindings")?;
            let moe_top_k_indices_buf = moe_top_k_indices_buf
                .as_ref()
                .ok_or("missing moe top-k indices buffer")?;
            let expert_gate_out_buf = expert_gate_out_buf
                .as_ref()
                .ok_or("missing expert gate output buffer")?;
            let expert_up_selected_args = expert_up_selected_args.as_ref();
            let expert_up_selected_bindings = expert_up_selected_bindings.as_ref();
            let expert_up_out_buf = expert_up_out_buf.as_ref();
            let moe_expert_geglu_args = moe_expert_geglu_args.as_ref();
            let moe_expert_geglu_bindings = moe_expert_geglu_bindings.as_ref();
            let expert_geglu_out_buf = expert_geglu_out_buf.as_ref();
            let expert_down_selected_args = expert_down_selected_args.as_ref();
            let expert_down_selected_bindings = expert_down_selected_bindings.as_ref();
            let expert_down_out_buf = expert_down_out_buf.as_ref();
            let geglu_pipeline = geglu_pipeline.as_ref();
            let mut top_k_index_bytes = Vec::with_capacity(ROUTER_TOP_K * size_of::<u32>());
            for &index in &router_output.top_k_indices {
                top_k_index_bytes.extend_from_slice(&index.to_le_bytes());
            }
            runtime.write_buffer(moe_top_k_indices_buf, 0, &top_k_index_bytes)?;
            runtime.begin_command_batch()?;
            runtime.dispatch_compute(
                &rms_pipeline,
                bytes_of(pre_feedforward_norm2_args),
                pre_feedforward_norm2_bindings,
                &[],
                rms_threadgroups,
                rms_threads_per_threadgroup,
            )?;
            runtime.memory_barrier_buffers()?;
            runtime.dispatch_compute(
                selected_expert_proj_pipeline,
                bytes_of(expert_gate_selected_args),
                expert_gate_selected_bindings,
                &[],
                expert_gate_selected_threadgroups,
                expert_gate_threads_per_threadgroup,
            )?;
            if let (Some(args), Some(bindings)) =
                (expert_up_selected_args, expert_up_selected_bindings)
            {
                runtime.memory_barrier_buffers()?;
                runtime.dispatch_compute(
                    selected_expert_proj_pipeline,
                    bytes_of(args),
                    bindings,
                    &[],
                    expert_up_selected_threadgroups,
                    expert_up_threads_per_threadgroup,
                )?;
            }
            if let (Some(pipeline), Some(args), Some(bindings)) = (
                geglu_pipeline,
                moe_expert_geglu_args,
                moe_expert_geglu_bindings,
            ) {
                runtime.memory_barrier_buffers()?;
                runtime.dispatch_compute(
                    pipeline,
                    bytes_of(args),
                    bindings,
                    &[],
                    moe_expert_geglu_threadgroups,
                    geglu_threads_per_threadgroup,
                )?;
            }
            if let (Some(args), Some(bindings)) =
                (expert_down_selected_args, expert_down_selected_bindings)
            {
                runtime.memory_barrier_buffers()?;
                runtime.dispatch_compute(
                    selected_expert_proj_pipeline,
                    bytes_of(args),
                    bindings,
                    &[],
                    expert_down_selected_threadgroups,
                    expert_down_threads_per_threadgroup,
                )?;
            }
            runtime.end_command_batch()?;
            runtime.wait_idle()?;
            let gate_bits = Some(decode_bf16_buffer_bits(
                &runtime
                    .read_buffer(expert_gate_out_buf, ROUTER_TOP_K * expert_gate_out_len * 2)?,
            ));
            let up_bits = if let Some(out_buf) = expert_up_out_buf {
                Some(decode_bf16_buffer_bits(&runtime.read_buffer(
                    out_buf,
                    ROUTER_TOP_K * expert_up_out_len * 2,
                )?))
            } else {
                None
            };
            let geglu_bits = if let Some(out_buf) = expert_geglu_out_buf {
                Some(decode_bf16_buffer_bits(&runtime.read_buffer(
                    out_buf,
                    ROUTER_TOP_K * expert_gate_out_len * 2,
                )?))
            } else {
                None
            };
            let down_bits = if let Some(out_buf) = expert_down_out_buf {
                Some(decode_bf16_buffer_bits(&runtime.read_buffer(
                    out_buf,
                    ROUTER_TOP_K * expert_down_out_len * 2,
                )?))
            } else {
                None
            };
            (gate_bits, up_bits, geglu_bits, down_bits)
        } else {
            (None, None, None, None)
        };
    let (
        post_ffn_norm1_bits,
        moe_expert_out_bits,
        moe_post_ffn_norm2_bits,
        moe_merge_bits,
        post_ffn_residual_bits,
    ) = if validate_post_ffn_norm1
        || validate_moe_expert_out
        || validate_moe_post_ffn_norm2
        || validate_moe_merge
        || validate_post_ffn_residual
    {
        let weighted_bits = if validate_moe_expert_out
            || validate_moe_post_ffn_norm2
            || validate_moe_merge
            || validate_post_ffn_residual
        {
            Some(moe_weighted_expert_out_bits(
                moe_expert_down_bits
                    .as_ref()
                    .ok_or("missing moe expert down output for weighted expert reduction")?,
                &router_output
                    .as_ref()
                    .ok_or("missing router output for weighted expert reduction")?
                    .top_k_weights_bits,
                expert_down_out_len,
            )?)
        } else {
            None
        };

        let post_ffn_norm1_args = post_ffn_norm1_args.as_ref();
        let post_ffn_norm1_bindings = post_ffn_norm1_bindings.as_ref();
        let moe_post_ffn_norm2_args = moe_post_ffn_norm2_args.as_ref();
        let moe_post_ffn_norm2_bindings = moe_post_ffn_norm2_bindings.as_ref();
        let moe_merge_bindings = moe_merge_bindings.as_ref();
        let post_ffn_residual_bindings = post_ffn_residual_bindings.as_ref();

        if let (Some(bits), Some(weighted_buf)) = (&weighted_bits, moe_weighted_out_buf.as_ref()) {
            let weighted_words = bf16_words_from_f32_bits(bits);
            let weighted_bytes = bytes_from_bf16_words(&weighted_words);
            runtime.write_buffer(weighted_buf, 0, &weighted_bytes)?;
        }

        runtime.begin_command_batch()?;
        if let (Some(args), Some(bindings)) = (post_ffn_norm1_args, post_ffn_norm1_bindings) {
            runtime.dispatch_compute(
                &rms_pipeline,
                bytes_of(args),
                bindings,
                &[],
                rms_threadgroups,
                rms_threads_per_threadgroup,
            )?;
        }
        if let (Some(args), Some(bindings)) = (moe_post_ffn_norm2_args, moe_post_ffn_norm2_bindings)
        {
            runtime.memory_barrier_buffers()?;
            runtime.dispatch_compute(
                &rms_pipeline,
                bytes_of(args),
                bindings,
                &[],
                rms_threadgroups,
                rms_threads_per_threadgroup,
            )?;
        }
        if let Some(bindings) = moe_merge_bindings {
            runtime.memory_barrier_buffers()?;
            runtime.dispatch_compute(
                residual_pipeline
                    .as_ref()
                    .ok_or("missing add pipeline for moe merge")?,
                bytes_of(
                    residual_args
                        .as_ref()
                        .ok_or("missing add args for moe merge")?,
                ),
                bindings,
                &[],
                residual_threadgroups,
                residual_threads_per_threadgroup,
            )?;
        }
        if let Some(bindings) = post_ffn_residual_bindings {
            runtime.memory_barrier_buffers()?;
            runtime.dispatch_compute(
                residual_pipeline
                    .as_ref()
                    .ok_or("missing add pipeline for post-ffn residual")?,
                bytes_of(
                    residual_args
                        .as_ref()
                        .ok_or("missing add args for post-ffn residual")?,
                ),
                bindings,
                &[],
                residual_threadgroups,
                residual_threads_per_threadgroup,
            )?;
        }
        runtime.end_command_batch()?;
        runtime.wait_idle()?;

        let post_ffn_norm1_bits = if let Some(out_buf) = post_feedforward_norm1_out_buf.as_ref() {
            Some(decode_bf16_buffer_bits(
                &runtime.read_buffer(out_buf, post_feedforward_norm1_len * 2)?,
            ))
        } else {
            None
        };
        let moe_post_ffn_norm2_bits = if let Some(out_buf) = moe_post_ffn_norm2_out_buf.as_ref() {
            Some(decode_bf16_buffer_bits(
                &runtime.read_buffer(out_buf, post_feedforward_norm2_len * 2)?,
            ))
        } else {
            None
        };
        let moe_merge_bits = if let Some(out_buf) = moe_merge_out_buf.as_ref() {
            Some(decode_bf16_buffer_bits(
                &runtime.read_buffer(out_buf, post_feedforward_norm1_len * 2)?,
            ))
        } else {
            None
        };
        let post_ffn_residual_bits = if let Some(out_buf) = post_ffn_residual_out_buf.as_ref() {
            Some(decode_bf16_buffer_bits(
                &runtime.read_buffer(out_buf, post_feedforward_norm1_len * 2)?,
            ))
        } else {
            None
        };
        (
            post_ffn_norm1_bits,
            weighted_bits,
            moe_post_ffn_norm2_bits,
            moe_merge_bits,
            post_ffn_residual_bits,
        )
    } else {
        (None, None, None, None, None)
    };
    let prefill_post_ffn_residual_bits = if let Some(prefill_attention_out_bits) =
        &prefill_attention_out_bits
    {
        let attn_out_buf = attn_out_buf
            .as_ref()
            .ok_or("missing attention output buffer for prefill tail")?;
        let o_proj_out_buf = o_proj_out_buf
            .as_ref()
            .ok_or("missing attention o_proj output buffer for prefill tail")?;
        let o_weight_buf = o_weight_buf
            .as_ref()
            .ok_or("missing o_proj weight buffer for prefill tail")?;
        let o_scales_buf = o_scales_buf
            .as_ref()
            .ok_or("missing o_proj scales buffer for prefill tail")?;
        let o_biases_buf = o_biases_buf
            .as_ref()
            .ok_or("missing o_proj biases buffer for prefill tail")?;
        let o_proj_fast_pipeline = o_proj_fast_pipeline
            .as_ref()
            .ok_or("missing o_proj fast pipeline for prefill tail")?;
        let o_proj_layout = o_proj_layout
            .as_ref()
            .ok_or("missing o_proj layout for prefill tail")?;
        let o_proj_args = o_proj_args
            .as_ref()
            .ok_or("missing o_proj args for prefill tail")?;
        let post_attention_norm_weight_buf = post_attention_norm_weight_buf
            .as_ref()
            .ok_or("missing post-attention norm weight buffer for prefill tail")?;
        let post_attention_norm_out_buf = post_attention_norm_out_buf
            .as_ref()
            .ok_or("missing post-attention norm output buffer for prefill tail")?;
        let residual_out_buf = residual_out_buf
            .as_ref()
            .ok_or("missing residual output buffer for prefill tail")?;
        let residual_pipeline = residual_pipeline
            .as_ref()
            .ok_or("missing add pipeline for prefill tail")?;
        let post_attention_norm_args = post_attention_norm_args
            .as_ref()
            .ok_or("missing post-attention norm args for prefill tail")?;
        let residual_args = residual_args
            .as_ref()
            .ok_or("missing residual args for prefill tail")?;
        let pre_feedforward_norm_weight_buf = pre_feedforward_norm_weight_buf
            .as_ref()
            .ok_or("missing pre-feedforward norm weight buffer for prefill tail")?;
        let pre_feedforward_norm_out_buf = pre_feedforward_norm_out_buf
            .as_ref()
            .ok_or("missing pre-feedforward norm output buffer for prefill tail")?;
        let pre_feedforward_norm_args = pre_feedforward_norm_args
            .as_ref()
            .ok_or("missing pre-feedforward norm args for prefill tail")?;
        let mlp_gate_args = mlp_gate_args
            .as_ref()
            .ok_or("missing mlp gate args for prefill tail")?;
        let mlp_gate_bindings = mlp_gate_bindings
            .as_ref()
            .ok_or("missing mlp gate bindings for prefill tail")?;
        let mlp_up_args = mlp_up_args
            .as_ref()
            .ok_or("missing mlp up args for prefill tail")?;
        let mlp_up_bindings = mlp_up_bindings
            .as_ref()
            .ok_or("missing mlp up bindings for prefill tail")?;
        let geglu_pipeline = geglu_pipeline
            .as_ref()
            .ok_or("missing geglu pipeline for prefill tail")?;
        let geglu_args = geglu_args
            .as_ref()
            .ok_or("missing geglu args for prefill tail")?;
        let geglu_bindings = geglu_bindings
            .as_ref()
            .ok_or("missing geglu bindings for prefill tail")?;
        let mlp_down_args = mlp_down_args
            .as_ref()
            .ok_or("missing mlp down args for prefill tail")?;
        let mlp_down_bindings = mlp_down_bindings
            .as_ref()
            .ok_or("missing mlp down bindings for prefill tail")?;
        let router_scale_pipeline = router_scale_pipeline
            .as_ref()
            .ok_or("missing router scale pipeline for prefill tail")?;
        let router_scale_args = router_scale_args
            .as_ref()
            .ok_or("missing router scale args for prefill tail")?;
        let router_scale_bindings = router_scale_bindings
            .as_ref()
            .ok_or("missing router scale bindings for prefill tail")?;
        let router_proj_args = router_proj_args
            .as_ref()
            .ok_or("missing router proj args for prefill tail")?;
        let router_proj_bindings = router_proj_bindings
            .as_ref()
            .ok_or("missing router proj bindings for prefill tail")?;
        let router_scaled_out_buf = router_scaled_out_buf
            .as_ref()
            .ok_or("missing router scaled output buffer for prefill tail")?;
        let router_proj_out_buf = router_proj_out_buf
            .as_ref()
            .ok_or("missing router proj output buffer for prefill tail")?;
        let router_probs_out_buf = router_probs_out_buf
            .as_ref()
            .ok_or("missing router probs output buffer for prefill tail")?;
        let router_per_expert_scale_buf = router_per_expert_scale_buf
            .as_ref()
            .ok_or("missing router per-expert scale buffer for prefill tail")?;
        let router_softmax_args = router_softmax_args
            .as_ref()
            .ok_or("missing router softmax args for prefill tail")?;
        let router_topk_args = router_topk_args
            .as_ref()
            .ok_or("missing router top-k args for prefill tail")?;
        let router_topk_pipeline = router_topk_pipeline
            .as_ref()
            .ok_or("missing router top-k pipeline for prefill tail")?;
        let pre_feedforward_norm2_args = pre_feedforward_norm2_args
            .as_ref()
            .ok_or("missing pre-feedforward norm2 args for prefill tail")?;
        let pre_feedforward_norm2_bindings = pre_feedforward_norm2_bindings
            .as_ref()
            .ok_or("missing pre-feedforward norm2 bindings for prefill tail")?;
        let selected_expert_proj_pipeline = selected_expert_proj_pipeline
            .as_ref()
            .ok_or("missing selected expert projection pipeline for prefill tail")?;
        let expert_gate_selected_args = expert_gate_selected_args
            .as_ref()
            .ok_or("missing expert gate args for prefill tail")?;
        let expert_gate_selected_bindings = expert_gate_selected_bindings
            .as_ref()
            .ok_or("missing expert gate bindings for prefill tail")?;
        let expert_up_selected_args = expert_up_selected_args
            .as_ref()
            .ok_or("missing expert up args for prefill tail")?;
        let expert_up_selected_bindings = expert_up_selected_bindings
            .as_ref()
            .ok_or("missing expert up bindings for prefill tail")?;
        let moe_expert_geglu_args = moe_expert_geglu_args
            .as_ref()
            .ok_or("missing expert geglu args for prefill tail")?;
        let moe_expert_geglu_bindings = moe_expert_geglu_bindings
            .as_ref()
            .ok_or("missing expert geglu bindings for prefill tail")?;
        let expert_down_selected_args = expert_down_selected_args
            .as_ref()
            .ok_or("missing expert down args for prefill tail")?;
        let expert_down_selected_bindings = expert_down_selected_bindings
            .as_ref()
            .ok_or("missing expert down bindings for prefill tail")?;
        let moe_top_k_indices_buf = moe_top_k_indices_buf
            .as_ref()
            .ok_or("missing moe top-k index buffer for prefill tail")?;
        let moe_top_k_weights_buf = moe_top_k_weights_buf
            .as_ref()
            .ok_or("missing moe top-k weight buffer for prefill tail")?;
        let expert_down_out_buf = expert_down_out_buf
            .as_ref()
            .ok_or("missing expert down output buffer for prefill tail")?;
        let post_ffn_norm1_args = post_ffn_norm1_args
            .as_ref()
            .ok_or("missing post-ffn norm1 args for prefill tail")?;
        let post_ffn_norm1_bindings = post_ffn_norm1_bindings
            .as_ref()
            .ok_or("missing post-ffn norm1 bindings for prefill tail")?;
        let moe_post_ffn_norm2_args = moe_post_ffn_norm2_args
            .as_ref()
            .ok_or("missing moe post-ffn norm2 args for prefill tail")?;
        let moe_post_ffn_norm2_bindings = moe_post_ffn_norm2_bindings
            .as_ref()
            .ok_or("missing moe post-ffn norm2 bindings for prefill tail")?;
        let moe_merge_bindings = moe_merge_bindings
            .as_ref()
            .ok_or("missing moe merge bindings for prefill tail")?;
        let post_ffn_residual_bindings = post_ffn_residual_bindings
            .as_ref()
            .ok_or("missing post-ffn residual bindings for prefill tail")?;
        let moe_weighted_out_buf = moe_weighted_out_buf
            .as_ref()
            .ok_or("missing weighted moe output buffer for prefill tail")?;
        let post_ffn_residual_out_buf = post_ffn_residual_out_buf
            .as_ref()
            .ok_or("missing post-ffn residual output buffer for prefill tail")?;

        let prefill_x_bytes = bytes_from_bf16_words(&prefill_x_words);
        let prefill_attn_out_words = bf16_words_from_f32_bits(prefill_attention_out_bits);
        let prefill_attn_out_bytes = bytes_from_bf16_words(&prefill_attn_out_words);
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
                buffer: &x_buf,
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
        let pre_feedforward_norm_bindings = [
            MetalBufferBindingRef {
                index: 1,
                buffer: residual_out_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 2,
                buffer: pre_feedforward_norm_weight_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 3,
                buffer: pre_feedforward_norm_out_buf,
                offset_bytes: 0,
            },
        ];

        runtime.write_buffer(&x_buf, 0, &prefill_x_bytes)?;
        runtime.write_buffer(attn_out_buf, 0, &prefill_attn_out_bytes)?;
        runtime.begin_command_batch()?;
        dispatch_exact_mlx_qmv_row(
            &runtime,
            &proj_pipeline,
            o_proj_fast_pipeline,
            *o_proj_layout,
            o_proj_args,
            &o_proj_bindings,
            o_proj_threadgroups,
            o_proj_threads_per_threadgroup,
        )?;
        runtime.memory_barrier_buffers()?;
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
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            &rms_pipeline,
            bytes_of(pre_feedforward_norm_args),
            &pre_feedforward_norm_bindings,
            &[],
            pre_feedforward_norm_threadgroups,
            rms_threads_per_threadgroup,
        )?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            &proj_pipeline,
            bytes_of(mlp_gate_args),
            mlp_gate_bindings,
            &[],
            mlp_gate_threadgroups,
            mlp_gate_threads_per_threadgroup,
        )?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            &proj_pipeline,
            bytes_of(mlp_up_args),
            mlp_up_bindings,
            &[],
            mlp_up_threadgroups,
            mlp_up_threads_per_threadgroup,
        )?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            geglu_pipeline,
            bytes_of(geglu_args),
            geglu_bindings,
            &[],
            geglu_threadgroups,
            geglu_threads_per_threadgroup,
        )?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            &proj_pipeline,
            bytes_of(mlp_down_args),
            mlp_down_bindings,
            &[],
            mlp_down_threadgroups,
            mlp_down_threads_per_threadgroup,
        )?;
        runtime.memory_barrier_buffers()?;
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
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            &attention_softmax_pipeline,
            bytes_of(router_softmax_args),
            &[
                MetalBufferBindingRef {
                    index: 1,
                    buffer: router_proj_out_buf,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: router_probs_out_buf,
                    offset_bytes: 0,
                },
            ],
            &[],
            router_softmax_threadgroups,
            mlx_softmax_threads_per_threadgroup(
                router_out_len,
                attention_softmax_pipeline.max_threads_per_threadgroup,
            )?,
        )?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            router_topk_pipeline,
            bytes_of(router_topk_args),
            &[
                MetalBufferBindingRef {
                    index: 1,
                    buffer: router_proj_out_buf,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: router_probs_out_buf,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: router_per_expert_scale_buf,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 4,
                    buffer: moe_top_k_indices_buf,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 5,
                    buffer: moe_top_k_weights_buf,
                    offset_bytes: 0,
                },
            ],
            &[],
            router_topk_threadgroups,
            router_topk_threads_per_threadgroup,
        )?;
        runtime.end_command_batch()?;
        runtime.wait_idle()?;

        let router_output = read_router_output_from_device(
            &runtime,
            router_scaled_out_buf,
            router_proj_out_buf,
            router_probs_out_buf,
            moe_top_k_indices_buf,
            moe_top_k_weights_buf,
            post_attention_norm_len,
            router_out_len,
            ROUTER_TOP_K,
        )?;
        runtime.begin_command_batch()?;
        runtime.dispatch_compute(
            &rms_pipeline,
            bytes_of(pre_feedforward_norm2_args),
            pre_feedforward_norm2_bindings,
            &[],
            rms_threadgroups,
            rms_threads_per_threadgroup,
        )?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            selected_expert_proj_pipeline,
            bytes_of(expert_gate_selected_args),
            expert_gate_selected_bindings,
            &[],
            expert_gate_selected_threadgroups,
            expert_gate_threads_per_threadgroup,
        )?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            selected_expert_proj_pipeline,
            bytes_of(expert_up_selected_args),
            expert_up_selected_bindings,
            &[],
            expert_up_selected_threadgroups,
            expert_up_threads_per_threadgroup,
        )?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            geglu_pipeline,
            bytes_of(moe_expert_geglu_args),
            moe_expert_geglu_bindings,
            &[],
            moe_expert_geglu_threadgroups,
            geglu_threads_per_threadgroup,
        )?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            selected_expert_proj_pipeline,
            bytes_of(expert_down_selected_args),
            expert_down_selected_bindings,
            &[],
            expert_down_selected_threadgroups,
            expert_down_threads_per_threadgroup,
        )?;
        runtime.end_command_batch()?;
        runtime.wait_idle()?;

        let expert_down_bits = decode_bf16_buffer_bits(
            &runtime.read_buffer(expert_down_out_buf, ROUTER_TOP_K * expert_down_out_len * 2)?,
        );
        let weighted_bits = moe_weighted_expert_out_bits(
            &expert_down_bits,
            &router_output.top_k_weights_bits,
            expert_down_out_len,
        )?;
        let weighted_words = bf16_words_from_f32_bits(&weighted_bits);
        let weighted_bytes = bytes_from_bf16_words(&weighted_words);
        runtime.write_buffer(moe_weighted_out_buf, 0, &weighted_bytes)?;
        runtime.begin_command_batch()?;
        runtime.dispatch_compute(
            &rms_pipeline,
            bytes_of(post_ffn_norm1_args),
            post_ffn_norm1_bindings,
            &[],
            rms_threadgroups,
            rms_threads_per_threadgroup,
        )?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            &rms_pipeline,
            bytes_of(moe_post_ffn_norm2_args),
            moe_post_ffn_norm2_bindings,
            &[],
            rms_threadgroups,
            rms_threads_per_threadgroup,
        )?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            residual_pipeline,
            bytes_of(residual_args),
            moe_merge_bindings,
            &[],
            residual_threadgroups,
            residual_threads_per_threadgroup,
        )?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            residual_pipeline,
            bytes_of(residual_args),
            post_ffn_residual_bindings,
            &[],
            residual_threadgroups,
            residual_threads_per_threadgroup,
        )?;
        runtime.end_command_batch()?;
        runtime.wait_idle()?;
        Some(decode_bf16_buffer_bits(&runtime.read_buffer(
            post_ffn_residual_out_buf,
            post_feedforward_norm1_len * 2,
        )?))
    } else {
        None
    };

    if layer_idx == 0 && validate_against_oracle && prefill_input_words_list.len() == 1 {
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
        if let Some(moe_expert_gate_bits) = &moe_expert_gate_bits {
            validate_hash_and_prefix(
                "attention_moe_expert_gate",
                moe_expert_gate_bits,
                EXPECTED_MOE_EXPERT_GATE_HASH,
                &EXPECTED_MOE_EXPERT_GATE_FIRST16_BITS,
            )?;
        }
        if let Some(moe_expert_up_bits) = &moe_expert_up_bits {
            validate_hash_and_prefix(
                "attention_moe_expert_up",
                moe_expert_up_bits,
                EXPECTED_MOE_EXPERT_UP_HASH,
                &EXPECTED_MOE_EXPERT_UP_FIRST16_BITS,
            )?;
        }
        if let Some(moe_expert_geglu_bits) = &moe_expert_geglu_bits {
            validate_hash_and_prefix(
                "attention_moe_expert_geglu",
                moe_expert_geglu_bits,
                EXPECTED_MOE_EXPERT_GEGLU_HASH,
                &EXPECTED_MOE_EXPERT_GEGLU_FIRST16_BITS,
            )?;
        }
        if let Some(moe_expert_down_bits) = &moe_expert_down_bits {
            validate_hash_and_prefix(
                "attention_moe_expert_down",
                moe_expert_down_bits,
                EXPECTED_MOE_EXPERT_DOWN_HASH,
                &EXPECTED_MOE_EXPERT_DOWN_FIRST16_BITS,
            )?;
        }
        if let Some(post_ffn_norm1_bits) = &post_ffn_norm1_bits {
            validate_hash_and_prefix(
                "attention_post_ffn_norm1",
                post_ffn_norm1_bits,
                EXPECTED_POST_FFN_NORM1_HASH,
                &EXPECTED_POST_FFN_NORM1_FIRST16_BITS,
            )?;
        }
        if let Some(moe_expert_out_bits) = &moe_expert_out_bits {
            validate_hash_and_prefix(
                "attention_moe_expert_out",
                moe_expert_out_bits,
                EXPECTED_MOE_EXPERT_OUT_HASH,
                &EXPECTED_MOE_EXPERT_OUT_FIRST16_BITS,
            )?;
        }
        if let Some(moe_post_ffn_norm2_bits) = &moe_post_ffn_norm2_bits {
            validate_hash_and_prefix(
                "attention_moe_post_ffn_norm2",
                moe_post_ffn_norm2_bits,
                EXPECTED_MOE_POST_FFN_NORM2_HASH,
                &EXPECTED_MOE_POST_FFN_NORM2_FIRST16_BITS,
            )?;
        }
        if let Some(moe_merge_bits) = &moe_merge_bits {
            validate_hash_and_prefix(
                "attention_moe_merge",
                moe_merge_bits,
                EXPECTED_MOE_MERGE_HASH,
                &EXPECTED_MOE_MERGE_FIRST16_BITS,
            )?;
        }
        if let Some(post_ffn_residual_bits) = &post_ffn_residual_bits {
            validate_hash_and_prefix(
                "attention_post_ffn_residual",
                post_ffn_residual_bits,
                EXPECTED_POST_FFN_RESIDUAL_HASH,
                &EXPECTED_POST_FFN_RESIDUAL_FIRST16_BITS,
            )?;
        }
    }

    let (post_attention_norm_bits, post_attention_residual_bits, pre_feedforward_norm_bits) =
        match post_attention_stage_bits {
            Some((post_attention_norm_bits, residual_bits, pre_feedforward_norm_bits)) => (
                Some(post_attention_norm_bits),
                Some(residual_bits),
                pre_feedforward_norm_bits,
            ),
            None => (None, None, None),
        };

    Ok(Layer0CachedArtifacts {
        backend_name: runtime.backend_info().name.to_string(),
        model_path,
        layer_idx,
        selected_stage: plan.display_stage(),
        prefill_rope_offset,
        decode_rope_offset,
        q_head_count,
        k_head_count,
        v_head_count,
        q_heads_per_kv,
        head_dim,
        prefill_input_norm_bits,
        prefill_v_proj_bits,
        prefill_q_bits,
        prefill_k_bits,
        prefill_v_bits,
        decode_input_norm_bits,
        decode_v_proj_bits,
        decode_q_bits,
        decode_k_bits,
        decode_v_bits,
        full_k_bits,
        full_v_bits,
        attention_score_bits,
        attention_prob_bits,
        attention_out_bits,
        attention_oproj_bits,
        post_attention_norm_bits,
        post_attention_residual_bits,
        pre_feedforward_norm_bits,
        dense_gate_bits,
        dense_up_bits,
        dense_geglu_bits,
        dense_down_bits,
        router_output,
        moe_expert_gate_bits,
        moe_expert_up_bits,
        moe_expert_geglu_bits,
        moe_expert_down_bits,
        post_ffn_norm1_bits,
        moe_expert_out_bits,
        moe_post_ffn_norm2_bits,
        moe_merge_bits,
        prefill_post_ffn_residual_bits,
        post_ffn_residual_bits,
    })
}

pub fn run_layer_sequence(
    model_path: PathBuf,
    layer_indices: &[usize],
    plan: Layer0CachedPlan,
) -> Result<Vec<Layer0CachedArtifacts>, Box<dyn Error>> {
    run_layer_sequence_from_inputs(
        model_path,
        layer_indices,
        CachedLayerInputs::synthetic_case(),
        plan,
    )
}

pub fn run_layer_sequence_from_inputs(
    model_path: PathBuf,
    layer_indices: &[usize],
    mut inputs: CachedLayerInputs,
    plan: Layer0CachedPlan,
) -> Result<Vec<Layer0CachedArtifacts>, Box<dyn Error>> {
    let mut session = LayerExecutionSession::load(model_path)?;
    let mut outputs = Vec::with_capacity(layer_indices.len());
    for &layer_idx in layer_indices {
        let artifacts = run_layer_plan_with_session(&mut session, layer_idx, inputs, plan)?;
        let next_prefill = artifacts
            .prefill_layer_output_bf16_words()
            .ok_or("layer sequence requires prefill post-ffn residual output")?;
        let next_decode = artifacts
            .bf16_words_for_stage(Layer0CachedStage::PostFfnResidual)
            .ok_or("layer sequence requires decode post-ffn residual output")?;
        inputs = CachedLayerInputs {
            prefill_input_words: next_prefill,
            decode_input_words: next_decode,
            prefill_rope_offset: artifacts.prefill_rope_offset,
            decode_rope_offset: artifacts.decode_rope_offset,
            validate_against_oracle: false,
        };
        outputs.push(artifacts);
    }
    Ok(outputs)
}


#[cfg(test)]
#[path = "../tests/layer0_cached_case.rs"]
mod tests;
