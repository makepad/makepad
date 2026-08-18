//! GGUF/ggml dtype metadata: the `TensorType` enum, its GGML_TYPE_* ids, and
//! the tiny per-type size tables (`ggml_type_name`, `block_size`,
//! `block_elements`). Moved out of libs/ggml/src/{tensor,quant}.rs (lane T2,
//! /aiarch.md §1) — this is the slice `formats/gguf.rs` needs to compute
//! tensor byte sizes, and it has zero dependency on the rest of ggml (no
//! Context, no Op, no compute kernels): pure lookup tables. ggml keeps a
//! `pub use makepad_ai_loader::quant::*;` shim at both original locations so
//! `TensorType` stays the same nominal type throughout ggml/llama and every
//! existing `makepad_ggml::{TensorType, block_size, block_elements, ...}`
//! call site keeps compiling unchanged.
//!
//! `QK`/`QK_K`/`QK_NVFP4` are local copies of the same-named constants in
//! ggml's quant.rs (needed by `block_elements` below) — ggml's dequant
//! kernels keep their own definitions, since those are compute code that
//! stays in ggml, not loader's disk layer.

const QK: usize = 32;
const QK_K: usize = 256;
const QK_NVFP4: usize = 64;

/// GGML type constants copied from upstream `ggml.h`.
pub const GGML_TYPE_F32: u32 = 0;
pub const GGML_TYPE_F16: u32 = 1;
pub const GGML_TYPE_Q4_0: u32 = 2;
pub const GGML_TYPE_Q4_1: u32 = 3;
pub const GGML_TYPE_Q5_0: u32 = 6;
pub const GGML_TYPE_Q5_1: u32 = 7;
pub const GGML_TYPE_Q8_0: u32 = 8;
pub const GGML_TYPE_Q8_1: u32 = 9;
pub const GGML_TYPE_Q2_K: u32 = 10;
pub const GGML_TYPE_Q3_K: u32 = 11;
pub const GGML_TYPE_Q4_K: u32 = 12;
pub const GGML_TYPE_Q5_K: u32 = 13;
pub const GGML_TYPE_Q6_K: u32 = 14;
pub const GGML_TYPE_Q8_K: u32 = 15;
pub const GGML_TYPE_IQ2_XXS: u32 = 16;
pub const GGML_TYPE_IQ2_XS: u32 = 17;
pub const GGML_TYPE_IQ3_XXS: u32 = 18;
pub const GGML_TYPE_IQ1_S: u32 = 19;
pub const GGML_TYPE_IQ4_NL: u32 = 20;
pub const GGML_TYPE_IQ3_S: u32 = 21;
pub const GGML_TYPE_IQ2_S: u32 = 22;
pub const GGML_TYPE_IQ4_XS: u32 = 23;
pub const GGML_TYPE_I8: u32 = 24;
pub const GGML_TYPE_I16: u32 = 25;
pub const GGML_TYPE_I32: u32 = 26;
pub const GGML_TYPE_I64: u32 = 27;
pub const GGML_TYPE_F64: u32 = 28;
pub const GGML_TYPE_IQ1_M: u32 = 29;
pub const GGML_TYPE_BF16: u32 = 30;
pub const GGML_TYPE_TQ1_0: u32 = 34;
pub const GGML_TYPE_TQ2_0: u32 = 35;
pub const GGML_TYPE_MXFP4: u32 = 39;
pub const GGML_TYPE_NVFP4: u32 = 40;
/// Makepad extension (not in upstream ggml.h): scalar signed FP8 E4M3FN,
/// 1 byte per element, implicit scale 1.0.
pub const GGML_TYPE_F8_E4M3: u32 = 41;
pub const GGML_TYPE_COUNT: u32 = 42;

pub fn ggml_type_name(ggml_type: u32) -> &'static str {
    match ggml_type {
        GGML_TYPE_F32 => "f32",
        GGML_TYPE_F16 => "f16",
        GGML_TYPE_Q4_0 => "q4_0",
        GGML_TYPE_Q4_1 => "q4_1",
        GGML_TYPE_Q5_0 => "q5_0",
        GGML_TYPE_Q5_1 => "q5_1",
        GGML_TYPE_Q8_0 => "q8_0",
        GGML_TYPE_Q8_1 => "q8_1",
        GGML_TYPE_Q2_K => "q2_K",
        GGML_TYPE_Q3_K => "q3_K",
        GGML_TYPE_Q4_K => "q4_K",
        GGML_TYPE_Q5_K => "q5_K",
        GGML_TYPE_Q6_K => "q6_K",
        GGML_TYPE_Q8_K => "q8_K",
        GGML_TYPE_IQ2_XXS => "iq2_xxs",
        GGML_TYPE_IQ2_XS => "iq2_xs",
        GGML_TYPE_IQ3_XXS => "iq3_xxs",
        GGML_TYPE_IQ1_S => "iq1_s",
        GGML_TYPE_IQ4_NL => "iq4_nl",
        GGML_TYPE_IQ3_S => "iq3_s",
        GGML_TYPE_IQ2_S => "iq2_s",
        GGML_TYPE_IQ4_XS => "iq4_xs",
        GGML_TYPE_I8 => "i8",
        GGML_TYPE_I16 => "i16",
        GGML_TYPE_I32 => "i32",
        GGML_TYPE_I64 => "i64",
        GGML_TYPE_F64 => "f64",
        GGML_TYPE_IQ1_M => "iq1_m",
        GGML_TYPE_BF16 => "bf16",
        GGML_TYPE_TQ1_0 => "tq1_0",
        GGML_TYPE_TQ2_0 => "tq2_0",
        GGML_TYPE_MXFP4 => "mxfp4",
        GGML_TYPE_NVFP4 => "nvfp4",
        GGML_TYPE_F8_E4M3 => "f8_e4m3",
        _ => "unknown",
    }
}

/// Type/block size in bytes for one ggml storage block.
pub fn block_size(ggml_type: u32) -> usize {
    match ggml_type {
        GGML_TYPE_F32 => 4,
        GGML_TYPE_F16 => 2,
        GGML_TYPE_Q4_0 => 18,
        GGML_TYPE_Q4_1 => 20,
        GGML_TYPE_Q5_0 => 22,
        GGML_TYPE_Q5_1 => 24,
        GGML_TYPE_Q8_0 => 34,
        GGML_TYPE_Q8_1 => 36,
        GGML_TYPE_Q2_K => 84,
        GGML_TYPE_Q3_K => 110,
        GGML_TYPE_Q4_K => 144,
        GGML_TYPE_Q5_K => 176,
        GGML_TYPE_Q6_K => 210,
        GGML_TYPE_Q8_K => 292,
        GGML_TYPE_IQ2_XXS => 66,
        GGML_TYPE_IQ2_XS => 74,
        GGML_TYPE_IQ3_XXS => 98,
        GGML_TYPE_IQ1_S => 50,
        GGML_TYPE_IQ4_NL => 18,
        GGML_TYPE_IQ3_S => 110,
        GGML_TYPE_IQ2_S => 82,
        GGML_TYPE_IQ4_XS => 136,
        GGML_TYPE_I8 => 1,
        GGML_TYPE_I16 => 2,
        GGML_TYPE_I32 => 4,
        GGML_TYPE_I64 => 8,
        GGML_TYPE_F64 => 8,
        GGML_TYPE_IQ1_M => 56,
        GGML_TYPE_BF16 => 2,
        GGML_TYPE_TQ1_0 => 54,
        GGML_TYPE_TQ2_0 => 66,
        GGML_TYPE_MXFP4 => 17,
        GGML_TYPE_NVFP4 => 36,
        GGML_TYPE_F8_E4M3 => 1,
        _ => panic!("unsupported ggml type {}", ggml_type),
    }
}

/// Number of dequantized elements represented by one storage block.
pub fn block_elements(ggml_type: u32) -> usize {
    match ggml_type {
        GGML_TYPE_F32 | GGML_TYPE_F16 | GGML_TYPE_I8 | GGML_TYPE_I16 | GGML_TYPE_I32
        | GGML_TYPE_I64 | GGML_TYPE_F64 | GGML_TYPE_BF16 | GGML_TYPE_F8_E4M3 => 1,
        GGML_TYPE_Q4_0 | GGML_TYPE_Q4_1 | GGML_TYPE_Q5_0 | GGML_TYPE_Q5_1 | GGML_TYPE_Q8_0
        | GGML_TYPE_Q8_1 | GGML_TYPE_MXFP4 | GGML_TYPE_IQ4_NL => QK,
        GGML_TYPE_NVFP4 => QK_NVFP4,
        GGML_TYPE_Q2_K | GGML_TYPE_Q3_K | GGML_TYPE_Q4_K | GGML_TYPE_Q5_K | GGML_TYPE_Q6_K
        | GGML_TYPE_Q8_K | GGML_TYPE_IQ2_XXS | GGML_TYPE_IQ2_XS | GGML_TYPE_IQ3_XXS
        | GGML_TYPE_IQ1_S | GGML_TYPE_IQ3_S | GGML_TYPE_IQ2_S | GGML_TYPE_IQ4_XS
        | GGML_TYPE_IQ1_M | GGML_TYPE_TQ1_0 | GGML_TYPE_TQ2_0 => QK_K,
        _ => panic!("unsupported ggml type {}", ggml_type),
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum TensorType {
    F32 = GGML_TYPE_F32,
    F16 = GGML_TYPE_F16,
    Q4_0 = GGML_TYPE_Q4_0,
    Q4_1 = GGML_TYPE_Q4_1,
    Q5_0 = GGML_TYPE_Q5_0,
    Q5_1 = GGML_TYPE_Q5_1,
    Q8_0 = GGML_TYPE_Q8_0,
    Q8_1 = GGML_TYPE_Q8_1,
    Q2K = GGML_TYPE_Q2_K,
    Q3K = GGML_TYPE_Q3_K,
    Q4K = GGML_TYPE_Q4_K,
    Q5K = GGML_TYPE_Q5_K,
    Q6K = GGML_TYPE_Q6_K,
    Q8K = GGML_TYPE_Q8_K,
    IQ2Xxs = GGML_TYPE_IQ2_XXS,
    IQ2Xs = GGML_TYPE_IQ2_XS,
    IQ3Xxs = GGML_TYPE_IQ3_XXS,
    IQ1S = GGML_TYPE_IQ1_S,
    IQ4Nl = GGML_TYPE_IQ4_NL,
    IQ3S = GGML_TYPE_IQ3_S,
    IQ2S = GGML_TYPE_IQ2_S,
    IQ4Xs = GGML_TYPE_IQ4_XS,
    I8 = GGML_TYPE_I8,
    I16 = GGML_TYPE_I16,
    I32 = GGML_TYPE_I32,
    I64 = GGML_TYPE_I64,
    F64 = GGML_TYPE_F64,
    IQ1M = GGML_TYPE_IQ1_M,
    BF16 = GGML_TYPE_BF16,
    TQ1_0 = GGML_TYPE_TQ1_0,
    TQ2_0 = GGML_TYPE_TQ2_0,
    MXFP4 = GGML_TYPE_MXFP4,
    NVFP4 = GGML_TYPE_NVFP4,
    F8E4M3 = GGML_TYPE_F8_E4M3,
}

impl TensorType {
    pub fn from_ggml_type(id: u32) -> Option<Self> {
        Some(match id {
            GGML_TYPE_F32 => Self::F32,
            GGML_TYPE_F16 => Self::F16,
            GGML_TYPE_Q4_0 => Self::Q4_0,
            GGML_TYPE_Q4_1 => Self::Q4_1,
            GGML_TYPE_Q5_0 => Self::Q5_0,
            GGML_TYPE_Q5_1 => Self::Q5_1,
            GGML_TYPE_Q8_0 => Self::Q8_0,
            GGML_TYPE_Q8_1 => Self::Q8_1,
            GGML_TYPE_Q2_K => Self::Q2K,
            GGML_TYPE_Q3_K => Self::Q3K,
            GGML_TYPE_Q4_K => Self::Q4K,
            GGML_TYPE_Q5_K => Self::Q5K,
            GGML_TYPE_Q6_K => Self::Q6K,
            GGML_TYPE_Q8_K => Self::Q8K,
            GGML_TYPE_IQ2_XXS => Self::IQ2Xxs,
            GGML_TYPE_IQ2_XS => Self::IQ2Xs,
            GGML_TYPE_IQ3_XXS => Self::IQ3Xxs,
            GGML_TYPE_IQ1_S => Self::IQ1S,
            GGML_TYPE_IQ4_NL => Self::IQ4Nl,
            GGML_TYPE_IQ3_S => Self::IQ3S,
            GGML_TYPE_IQ2_S => Self::IQ2S,
            GGML_TYPE_IQ4_XS => Self::IQ4Xs,
            GGML_TYPE_I8 => Self::I8,
            GGML_TYPE_I16 => Self::I16,
            GGML_TYPE_I32 => Self::I32,
            GGML_TYPE_I64 => Self::I64,
            GGML_TYPE_F64 => Self::F64,
            GGML_TYPE_IQ1_M => Self::IQ1M,
            GGML_TYPE_BF16 => Self::BF16,
            GGML_TYPE_TQ1_0 => Self::TQ1_0,
            GGML_TYPE_TQ2_0 => Self::TQ2_0,
            GGML_TYPE_MXFP4 => Self::MXFP4,
            GGML_TYPE_NVFP4 => Self::NVFP4,
            GGML_TYPE_F8_E4M3 => Self::F8E4M3,
            _ => return None,
        })
    }

    pub fn ggml_type(self) -> u32 {
        self as u32
    }

    pub fn name(self) -> &'static str {
        ggml_type_name(self.ggml_type())
    }

    pub fn block_size(self) -> usize {
        block_elements(self.ggml_type())
    }

    pub fn scalar_size_bytes(self) -> Option<usize> {
        match self {
            Self::F16 | Self::BF16 | Self::I16 => Some(2),
            Self::F32 | Self::I32 => Some(4),
            Self::F64 | Self::I64 => Some(8),
            Self::I8 | Self::F8E4M3 => Some(1),
            Self::Q4_0
            | Self::Q4_1
            | Self::Q5_0
            | Self::Q5_1
            | Self::Q8_0
            | Self::Q8_1
            | Self::Q2K
            | Self::Q3K
            | Self::Q4K
            | Self::Q5K
            | Self::Q6K
            | Self::Q8K
            | Self::IQ2Xxs
            | Self::IQ2Xs
            | Self::IQ3Xxs
            | Self::IQ1S
            | Self::IQ4Nl
            | Self::IQ3S
            | Self::IQ2S
            | Self::IQ4Xs
            | Self::IQ1M
            | Self::TQ1_0
            | Self::TQ2_0
            | Self::MXFP4
            | Self::NVFP4 => None,
        }
    }

    pub fn is_quantized(self) -> bool {
        self.scalar_size_bytes().is_none()
    }
}
