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
