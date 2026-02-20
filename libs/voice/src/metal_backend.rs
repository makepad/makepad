use std::sync::OnceLock;

fn env_truthy(key: &str) -> Option<bool> {
    std::env::var(key).ok().map(|v| {
        let v = v.trim().to_ascii_lowercase();
        !(v.is_empty() || v == "0" || v == "false" || v == "no" || v == "off")
    })
}

fn metal_requested() -> bool {
    static USE_METAL: OnceLock<bool> = OnceLock::new();
    *USE_METAL.get_or_init(|| {
        if let Ok(backend) = std::env::var("MAKEPAD_VOICE_BACKEND") {
            let backend = backend.trim().to_ascii_lowercase();
            if backend == "cpu" {
                return false;
            }
            if backend == "metal" {
                return true;
            }
        }
        env_truthy("MAKEPAD_VOICE_METAL").unwrap_or(cfg!(target_os = "macos"))
    })
}

pub(crate) fn try_matmul_nn_f32(
    a: &[f32],
    b: &[f32],
    m: usize,
    k: usize,
    n: usize,
) -> Option<Vec<f32>> {
    if !metal_requested() {
        return None;
    }
    imp::try_matmul_nn_f32(a, b, m, k, n)
}

pub(crate) fn try_matmul_nt_f32(
    a: &[f32],
    bt: &[f32],
    m: usize,
    k: usize,
    n: usize,
) -> Option<Vec<f32>> {
    if !metal_requested() {
        return None;
    }
    imp::try_matmul_nt_f32(a, bt, m, k, n)
}

pub(crate) fn try_matmul_nt_f32_bytes(
    a: &[f32],
    bt_bytes: &[u8],
    m: usize,
    k: usize,
    n: usize,
) -> Option<Vec<f32>> {
    if !metal_requested() {
        return None;
    }
    imp::try_matmul_nt_f32_bytes(a, bt_bytes, m, k, n)
}

pub(crate) fn try_matmul_nt_f16_bytes(
    a: &[f32],
    bt_f16_bytes: &[u8],
    m: usize,
    k: usize,
    n: usize,
) -> Option<Vec<f32>> {
    if !metal_requested() {
        return None;
    }
    imp::try_matmul_nt_f16_bytes(a, bt_f16_bytes, m, k, n)
}

pub(crate) fn try_matmul_nt_ggml_bytes(
    a: &[f32],
    bt_bytes: &[u8],
    bt_ggml_type: u32,
    m: usize,
    k: usize,
    n: usize,
) -> Option<Vec<f32>> {
    if !metal_requested() {
        return None;
    }
    imp::try_matmul_nt_ggml_bytes(a, bt_bytes, bt_ggml_type, m, k, n)
}

pub(crate) fn try_matmul_nt_ggml_bytes_add_bias(
    a: &[f32],
    bt_bytes: &[u8],
    bt_ggml_type: u32,
    m: usize,
    k: usize,
    n: usize,
    bias: &[f32],
) -> Option<Vec<f32>> {
    if !metal_requested() {
        return None;
    }
    imp::try_matmul_nt_ggml_bytes_add_bias(a, bt_bytes, bt_ggml_type, m, k, n, bias)
}

#[allow(dead_code)]
pub(crate) fn try_flash_attn_f32(
    q: &[f32],
    k: &[f32],
    v: &[f32],
    n_q: usize,
    d: usize,
    scale: f32,
) -> Option<Vec<f32>> {
    if !metal_requested() {
        return None;
    }
    imp::try_flash_attn_f32(q, k, v, n_q, d, scale)
}

pub(crate) fn try_flash_attn_f32_packed(
    q: &[f32],
    k: &[f32],
    v: &[f32],
    n_q: usize,
    n_kv: usize,
    n_head: usize,
    d: usize,
    scale: f32,
) -> Option<Vec<f32>> {
    if !metal_requested() {
        return None;
    }
    imp::try_flash_attn_f32_packed(q, k, v, n_q, n_kv, n_head, d, scale)
}

pub(crate) fn clear_decoder_kv_cache() {
    if !metal_requested() {
        return;
    }
    imp::clear_decoder_kv_cache();
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn try_flash_attn_f32_self_kv_cache(
    layer: usize,
    q: &[f32],
    k_all: &[f32],
    v_all: &[f32],
    n_kv: usize,
    n_head: usize,
    d: usize,
    scale: f32,
) -> Option<Vec<f32>> {
    if !metal_requested() {
        return None;
    }
    imp::try_flash_attn_f32_self_kv_cache(layer, q, k_all, v_all, n_kv, n_head, d, scale)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn try_flash_attn_f32_cross_kv_cache(
    layer: usize,
    q: &[f32],
    k_cross: &[f32],
    v_cross: &[f32],
    n_q: usize,
    n_kv: usize,
    n_head: usize,
    d: usize,
    scale: f32,
) -> Option<Vec<f32>> {
    if !metal_requested() {
        return None;
    }
    imp::try_flash_attn_f32_cross_kv_cache(
        layer, q, k_cross, v_cross, n_q, n_kv, n_head, d, scale,
    )
}

pub(crate) fn try_add_f32(
    a: &[f32],
    a_shape: &[usize],
    b: &[f32],
    b_shape: &[usize],
) -> Option<Vec<f32>> {
    if !metal_requested() {
        return None;
    }
    imp::try_add_f32(a, a_shape, b, b_shape)
}

pub(crate) fn try_mul_f32(
    a: &[f32],
    a_shape: &[usize],
    b: &[f32],
    b_shape: &[usize],
) -> Option<Vec<f32>> {
    if !metal_requested() {
        return None;
    }
    imp::try_mul_f32(a, a_shape, b, b_shape)
}

pub(crate) fn try_gelu_f32(a: &[f32], shape: &[usize]) -> Option<Vec<f32>> {
    if !metal_requested() {
        return None;
    }
    imp::try_gelu_f32(a, shape)
}

pub(crate) fn try_layer_norm_f32(x: &[f32], shape: &[usize], eps: f32) -> Option<Vec<f32>> {
    if !metal_requested() {
        return None;
    }
    imp::try_layer_norm_f32(x, shape, eps)
}

pub(crate) fn try_layer_norm_mul_add_f32(
    x: &[f32],
    x_shape: &[usize],
    mul: &[f32],
    mul_shape: &[usize],
    add: &[f32],
    add_shape: &[usize],
    eps: f32,
) -> Option<Vec<f32>> {
    if !metal_requested() {
        return None;
    }
    imp::try_layer_norm_mul_add_f32(x, x_shape, mul, mul_shape, add, add_shape, eps)
}

pub(crate) fn try_im2col_1d_f32(
    input: &[f32],
    ic: usize,
    iw: usize,
    kw: usize,
    stride: usize,
    pad: usize,
) -> Option<Vec<f32>> {
    if !metal_requested() {
        return None;
    }
    imp::try_im2col_1d_f32(input, ic, iw, kw, stride, pad)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn try_encoder_attn_block_f32(
    x: &[f32],
    seq_len: usize,
    n_state: usize,
    n_head: usize,
    ln_w: &[f32],
    ln_b: &[f32],
    q_w_bytes: &[u8],
    q_w_ggml_type: u32,
    q_b: &[f32],
    k_w_bytes: &[u8],
    k_w_ggml_type: u32,
    v_w_bytes: &[u8],
    v_w_ggml_type: u32,
    v_b: &[f32],
    out_w_bytes: &[u8],
    out_w_ggml_type: u32,
    out_b: &[f32],
) -> Option<Vec<f32>> {
    if !metal_requested() {
        return None;
    }
    imp::try_encoder_attn_block_f32(
        x,
        seq_len,
        n_state,
        n_head,
        ln_w,
        ln_b,
        q_w_bytes,
        q_w_ggml_type,
        q_b,
        k_w_bytes,
        k_w_ggml_type,
        v_w_bytes,
        v_w_ggml_type,
        v_b,
        out_w_bytes,
        out_w_ggml_type,
        out_b,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn try_encoder_ffn_block_f32(
    x: &[f32],
    seq_len: usize,
    n_state: usize,
    ln_w: &[f32],
    ln_b: &[f32],
    w0_bytes: &[u8],
    w0_ggml_type: u32,
    b0: &[f32],
    w1_bytes: &[u8],
    w1_ggml_type: u32,
    b1: &[f32],
) -> Option<Vec<f32>> {
    if !metal_requested() {
        return None;
    }
    imp::try_encoder_ffn_block_f32(
        x,
        seq_len,
        n_state,
        ln_w,
        ln_b,
        w0_bytes,
        w0_ggml_type,
        b0,
        w1_bytes,
        w1_ggml_type,
        b1,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn try_encoder_layer_f32(
    x: &[f32],
    seq_len: usize,
    n_state: usize,
    n_head: usize,
    attn_ln_w: &[f32],
    attn_ln_b: &[f32],
    q_w_bytes: &[u8],
    q_w_ggml_type: u32,
    q_b: &[f32],
    k_w_bytes: &[u8],
    k_w_ggml_type: u32,
    v_w_bytes: &[u8],
    v_w_ggml_type: u32,
    v_b: &[f32],
    out_w_bytes: &[u8],
    out_w_ggml_type: u32,
    out_b: &[f32],
    mlp_ln_w: &[f32],
    mlp_ln_b: &[f32],
    w0_bytes: &[u8],
    w0_ggml_type: u32,
    b0: &[f32],
    w1_bytes: &[u8],
    w1_ggml_type: u32,
    b1: &[f32],
) -> Option<Vec<f32>> {
    if !metal_requested() {
        return None;
    }
    imp::try_encoder_layer_f32(
        x,
        seq_len,
        n_state,
        n_head,
        attn_ln_w,
        attn_ln_b,
        q_w_bytes,
        q_w_ggml_type,
        q_b,
        k_w_bytes,
        k_w_ggml_type,
        v_w_bytes,
        v_w_ggml_type,
        v_b,
        out_w_bytes,
        out_w_ggml_type,
        out_b,
        mlp_ln_w,
        mlp_ln_b,
        w0_bytes,
        w0_ggml_type,
        b0,
        w1_bytes,
        w1_ggml_type,
        b1,
    )
}

pub(crate) fn try_encoder_stack_f32(
    x: &[f32],
    seq_len: usize,
    n_state: usize,
    n_head: usize,
    layers: &[crate::model::EncoderLayer],
    final_ln_w: &[f32],
    final_ln_b: &[f32],
) -> Option<Vec<f32>> {
    if !metal_requested() {
        return None;
    }
    imp::try_encoder_stack_f32(
        x, seq_len, n_state, n_head, layers, final_ln_w, final_ln_b,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn try_decoder_cross_ffn_step_f32(
    layer_idx: usize,
    x: &[f32],
    n_state: usize,
    n_head: usize,
    k_cross: &[f32],
    v_cross: &[f32],
    n_audio_ctx: usize,
    layer: &crate::model::DecoderLayer,
) -> Option<Vec<f32>> {
    if !metal_requested() {
        return None;
    }
    imp::try_decoder_cross_ffn_step_f32(
        layer_idx, x, n_state, n_head, k_cross, v_cross, n_audio_ctx, layer,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn try_decoder_self_cross_ffn_step_f32(
    layer_idx: usize,
    x: &[f32],
    q_self: &[f32],
    k_all: &[f32],
    v_all: &[f32],
    n_kv: usize,
    n_state: usize,
    n_head: usize,
    k_cross: &[f32],
    v_cross: &[f32],
    n_audio_ctx: usize,
    layer: &crate::model::DecoderLayer,
) -> Option<Vec<f32>> {
    if !metal_requested() {
        return None;
    }
    imp::try_decoder_self_cross_ffn_step_f32(
        layer_idx, x, q_self, k_all, v_all, n_kv, n_state, n_head, k_cross, v_cross, n_audio_ctx, layer,
    )
}

pub(crate) fn is_requested() -> bool {
    metal_requested()
}

#[cfg(not(target_os = "macos"))]
mod imp {
    pub(super) fn try_matmul_nn_f32(
        _a: &[f32],
        _b: &[f32],
        _m: usize,
        _k: usize,
        _n: usize,
    ) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn try_matmul_nt_f32(
        _a: &[f32],
        _bt: &[f32],
        _m: usize,
        _k: usize,
        _n: usize,
    ) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn try_matmul_nt_f32_bytes(
        _a: &[f32],
        _bt_bytes: &[u8],
        _m: usize,
        _k: usize,
        _n: usize,
    ) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn try_matmul_nt_f16_bytes(
        _a: &[f32],
        _bt_f16_bytes: &[u8],
        _m: usize,
        _k: usize,
        _n: usize,
    ) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn try_matmul_nt_ggml_bytes(
        _a: &[f32],
        _bt_bytes: &[u8],
        _bt_ggml_type: u32,
        _m: usize,
        _k: usize,
        _n: usize,
    ) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn try_matmul_nt_ggml_bytes_add_bias(
        _a: &[f32],
        _bt_bytes: &[u8],
        _bt_ggml_type: u32,
        _m: usize,
        _k: usize,
        _n: usize,
        _bias: &[f32],
    ) -> Option<Vec<f32>> {
        None
    }

    #[allow(dead_code)]
    pub(super) fn try_flash_attn_f32(
        _q: &[f32],
        _k: &[f32],
        _v: &[f32],
        _n_q: usize,
        _d: usize,
        _scale: f32,
    ) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn try_flash_attn_f32_packed(
        _q: &[f32],
        _k: &[f32],
        _v: &[f32],
        _n_q: usize,
        _n_kv: usize,
        _n_head: usize,
        _d: usize,
        _scale: f32,
    ) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn clear_decoder_kv_cache() {}

    #[allow(clippy::too_many_arguments)]
    pub(super) fn try_flash_attn_f32_self_kv_cache(
        _layer: usize,
        _q: &[f32],
        _k_all: &[f32],
        _v_all: &[f32],
        _n_kv: usize,
        _n_head: usize,
        _d: usize,
        _scale: f32,
    ) -> Option<Vec<f32>> {
        None
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn try_flash_attn_f32_cross_kv_cache(
        _layer: usize,
        _q: &[f32],
        _k_cross: &[f32],
        _v_cross: &[f32],
        _n_q: usize,
        _n_kv: usize,
        _n_head: usize,
        _d: usize,
        _scale: f32,
    ) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn try_add_f32(
        _a: &[f32],
        _a_shape: &[usize],
        _b: &[f32],
        _b_shape: &[usize],
    ) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn try_mul_f32(
        _a: &[f32],
        _a_shape: &[usize],
        _b: &[f32],
        _b_shape: &[usize],
    ) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn try_gelu_f32(_a: &[f32], _shape: &[usize]) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn try_layer_norm_f32(_x: &[f32], _shape: &[usize], _eps: f32) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn try_layer_norm_mul_add_f32(
        _x: &[f32],
        _x_shape: &[usize],
        _mul: &[f32],
        _mul_shape: &[usize],
        _add: &[f32],
        _add_shape: &[usize],
        _eps: f32,
    ) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn try_im2col_1d_f32(
        _input: &[f32],
        _ic: usize,
        _iw: usize,
        _kw: usize,
        _stride: usize,
        _pad: usize,
    ) -> Option<Vec<f32>> {
        None
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn try_encoder_attn_block_f32(
        _x: &[f32],
        _seq_len: usize,
        _n_state: usize,
        _n_head: usize,
        _ln_w: &[f32],
        _ln_b: &[f32],
        _q_w_bytes: &[u8],
        _q_w_ggml_type: u32,
        _q_b: &[f32],
        _k_w_bytes: &[u8],
        _k_w_ggml_type: u32,
        _v_w_bytes: &[u8],
        _v_w_ggml_type: u32,
        _v_b: &[f32],
        _out_w_bytes: &[u8],
        _out_w_ggml_type: u32,
        _out_b: &[f32],
    ) -> Option<Vec<f32>> {
        None
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn try_encoder_ffn_block_f32(
        _x: &[f32],
        _seq_len: usize,
        _n_state: usize,
        _ln_w: &[f32],
        _ln_b: &[f32],
        _w0_bytes: &[u8],
        _w0_ggml_type: u32,
        _b0: &[f32],
        _w1_bytes: &[u8],
        _w1_ggml_type: u32,
        _b1: &[f32],
    ) -> Option<Vec<f32>> {
        None
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn try_encoder_layer_f32(
        _x: &[f32],
        _seq_len: usize,
        _n_state: usize,
        _n_head: usize,
        _attn_ln_w: &[f32],
        _attn_ln_b: &[f32],
        _q_w_bytes: &[u8],
        _q_w_ggml_type: u32,
        _q_b: &[f32],
        _k_w_bytes: &[u8],
        _k_w_ggml_type: u32,
        _v_w_bytes: &[u8],
        _v_w_ggml_type: u32,
        _v_b: &[f32],
        _out_w_bytes: &[u8],
        _out_w_ggml_type: u32,
        _out_b: &[f32],
        _mlp_ln_w: &[f32],
        _mlp_ln_b: &[f32],
        _w0_bytes: &[u8],
        _w0_ggml_type: u32,
        _b0: &[f32],
        _w1_bytes: &[u8],
        _w1_ggml_type: u32,
        _b1: &[f32],
    ) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn try_encoder_stack_f32(
        _x: &[f32],
        _seq_len: usize,
        _n_state: usize,
        _n_head: usize,
        _layers: &[crate::model::EncoderLayer],
        _final_ln_w: &[f32],
        _final_ln_b: &[f32],
    ) -> Option<Vec<f32>> {
        None
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn try_decoder_cross_ffn_step_f32(
        _layer_idx: usize,
        _x: &[f32],
        _n_state: usize,
        _n_head: usize,
        _k_cross: &[f32],
        _v_cross: &[f32],
        _n_audio_ctx: usize,
        _layer: &crate::model::DecoderLayer,
    ) -> Option<Vec<f32>> {
        None
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn try_decoder_self_cross_ffn_step_f32(
        _layer_idx: usize,
        _x: &[f32],
        _q_self: &[f32],
        _k_all: &[f32],
        _v_all: &[f32],
        _n_kv: usize,
        _n_state: usize,
        _n_head: usize,
        _k_cross: &[f32],
        _v_cross: &[f32],
        _n_audio_ctx: usize,
        _layer: &crate::model::DecoderLayer,
    ) -> Option<Vec<f32>> {
        None
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use crate::model::{DecoderLayer, EncoderLayer};
    use crate::quant::{
        block_size, GGML_TYPE_F16, GGML_TYPE_F32, GGML_TYPE_Q4_0, GGML_TYPE_Q4_1, GGML_TYPE_Q5_0,
        GGML_TYPE_Q5_1, GGML_TYPE_Q8_0,
    };
    use makepad_objc_sys::runtime::{nil, ObjcId, Object, YES};
    use makepad_objc_sys::{class, msg_send, sel, sel_impl};
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::ffi::{c_char, c_void, CStr};
    use std::ptr::NonNull;
    use std::sync::OnceLock;

    const UTF8_ENCODING: u64 = 4;
    const MTL_RESOURCE_STORAGE_MODE_SHARED: u64 = 0;
    const MTL_GPU_FAMILY_APPLE6: u64 = 1006;
    const MTL_GPU_FAMILY_METAL3: u64 = 5001;
    const MTL_GPU_FAMILY_METAL4: u64 = 5002;

    const MTL_DATA_TYPE_INT: u64 = 29;
    const MTL_DATA_TYPE_SHORT: u64 = 37;
    const MTL_DATA_TYPE_BOOL: u64 = 53;

    const FC_FLASH_ATTN_EXT_PAD: i32 = 100;
    const FC_FLASH_ATTN_EXT_BLK: i32 = 200;
    const FC_FLASH_ATTN_EXT: i32 = 300;
    const FC_FLASH_ATTN_EXT_VEC: i32 = 400;
    const FC_FLASH_ATTN_EXT_VEC_REDUCE: i32 = 500;
    const FC_MUL_MV: i32 = 600;
    const FC_MUL_MM: i32 = 700;
    const FC_UNARY: i32 = 1200;
    const FC_BIN: i32 = 1300;
    const OP_FLASH_ATTN_EXT_NQPSG: i32 = 8;
    const OP_FLASH_ATTN_EXT_NCPSG: i32 = 64;
    const OP_FLASH_ATTN_EXT_VEC_NQPSG: i32 = 1;
    const OP_FLASH_ATTN_EXT_VEC_NCPSG: i32 = 32;
    const OP_UNARY_NUM_GELU: i16 = 103;
    const SCRATCH_FLASH_PAD: u8 = 1;
    const SCRATCH_FLASH_BLK: u8 = 2;
    const SCRATCH_FLASH_TMP: u8 = 3;
    const SCRATCH_FLASH_OUT: u8 = 4;
    const SCRATCH_ENC_NORM0: u8 = 10;
    const SCRATCH_ENC_NORM1: u8 = 11;
    const SCRATCH_DEC_NORM0: u8 = 12;
    const SCRATCH_DEC_NORM1: u8 = 13;

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

    const _GGML_METAL_SOURCE_RAW: &str =
        include_str!("../../../local/whisper.cpp/ggml/src/ggml-metal/ggml-metal.metal");
    const _GGML_COMMON_H: &str = include_str!("../../../local/whisper.cpp/ggml/src/ggml-common.h");
    const _GGML_METAL_IMPL_H: &str =
        include_str!("../../../local/whisper.cpp/ggml/src/ggml-metal/ggml-metal-impl.h");
    const _GGML_METALLIB_BYTES: &[u8] = include_bytes!(env!("MAKEPAD_VOICE_GGML_METALLIB"));

    #[link(name = "Metal", kind = "framework")]
    extern "C" {
        fn MTLCreateSystemDefaultDevice() -> ObjcId;
        fn MTLCopyAllDevices() -> ObjcId;
    }

    #[link(name = "Foundation", kind = "framework")]
    extern "C" {}

    #[repr(C)]
    #[derive(Copy, Clone)]
    struct MTLSize {
        width: u64,
        height: u64,
        depth: u64,
    }

    #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
    struct BufferKey {
        ptr: usize,
        len: usize,
        tag: u8,
    }

    #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
    enum Src0Type {
        F32,
        F16,
        Q4_0,
        Q4_1,
        Q5_0,
        Q5_1,
        Q8_0,
    }

    #[derive(Clone, Copy, Debug)]
    enum FunctionConstantValue {
        Int32(i32),
        Int16(i16),
        Bool(bool),
    }

    #[derive(Clone, Copy, Debug)]
    struct FunctionConstant {
        idx: i32,
        value: FunctionConstantValue,
    }

    #[repr(C)]
    #[derive(Copy, Clone)]
    struct KArgsMulMm {
        ne00: i32,
        ne02: i32,
        nb01: u64,
        nb02: u64,
        nb03: u64,
        ne12: i32,
        nb10: u64,
        nb11: u64,
        nb12: u64,
        nb13: u64,
        ne0: i32,
        ne1: i32,
        r2: i16,
        r3: i16,
    }

    #[repr(C)]
    #[derive(Copy, Clone)]
    struct KArgsMulMv {
        ne00: i32,
        ne01: i32,
        ne02: i32,
        nb00: u64,
        nb01: u64,
        nb02: u64,
        nb03: u64,
        ne10: i32,
        ne11: i32,
        ne12: i32,
        nb10: u64,
        nb11: u64,
        nb12: u64,
        nb13: u64,
        ne0: i32,
        ne1: i32,
        nr0: i32,
        r2: i16,
        r3: i16,
    }

    #[repr(C)]
    #[derive(Copy, Clone)]
    struct KArgsMulMvExt {
        ne00: i32,
        ne01: i32,
        ne02: i32,
        nb00: u64,
        nb01: u64,
        nb02: u64,
        nb03: u64,
        ne10: i32,
        ne11: i32,
        ne12: i32,
        nb10: u64,
        nb11: u64,
        nb12: u64,
        nb13: u64,
        ne0: i32,
        ne1: i32,
        r2: i16,
        r3: i16,
    }

    #[repr(C)]
    #[derive(Copy, Clone)]
    struct KArgsFlashAttnExtPad {
        ne11: i32,
        ne_12_2: i32,
        ne_12_3: i32,
        nb11: u64,
        nb12: u64,
        nb13: u64,
        nb21: u64,
        nb22: u64,
        nb23: u64,
        ne31: i32,
        ne32: i32,
        ne33: i32,
        nb31: u64,
        nb32: u64,
        nb33: u64,
    }

    #[repr(C)]
    #[derive(Copy, Clone)]
    struct KArgsFlashAttnExtBlk {
        ne01: i32,
        ne30: i32,
        ne31: i32,
        ne32: i32,
        ne33: i32,
        nb31: u64,
        nb32: u64,
        nb33: u64,
    }

    #[repr(C)]
    #[derive(Copy, Clone)]
    struct KArgsFlashAttnExt {
        ne01: i32,
        ne02: i32,
        ne03: i32,
        nb01: u64,
        nb02: u64,
        nb03: u64,
        ne11: i32,
        ne_12_2: i32,
        ne_12_3: i32,
        ns10: i32,
        nb11: u64,
        nb12: u64,
        nb13: u64,
        ns20: i32,
        nb21: u64,
        nb22: u64,
        nb23: u64,
        ne31: i32,
        ne32: i32,
        ne33: i32,
        nb31: u64,
        nb32: u64,
        nb33: u64,
        ne1: i32,
        ne2: i32,
        ne3: i32,
        scale: f32,
        max_bias: f32,
        m0: f32,
        m1: f32,
        n_head_log2: i32,
        logit_softcap: f32,
    }

    #[repr(C)]
    #[derive(Copy, Clone)]
    struct KArgsFlashAttnExtVec {
        ne01: i32,
        ne02: i32,
        ne03: i32,
        nb01: u64,
        nb02: u64,
        nb03: u64,
        ne11: i32,
        ne_12_2: i32,
        ne_12_3: i32,
        ns10: i32,
        nb11: u64,
        nb12: u64,
        nb13: u64,
        ns20: i32,
        nb21: u64,
        nb22: u64,
        nb23: u64,
        ne31: i32,
        ne32: i32,
        ne33: i32,
        nb31: u64,
        nb32: u64,
        nb33: u64,
        ne1: i32,
        ne2: i32,
        ne3: i32,
        scale: f32,
        max_bias: f32,
        m0: f32,
        m1: f32,
        n_head_log2: i32,
        logit_softcap: f32,
    }

    #[repr(C)]
    #[derive(Copy, Clone)]
    struct KArgsFlashAttnExtVecReduce {
        nrows: i32,
    }

    #[repr(C)]
    #[derive(Copy, Clone)]
    struct KArgsUnary {
        ne00: i32,
        ne01: i32,
        ne02: i32,
        ne03: i32,
        nb00: u64,
        nb01: u64,
        nb02: u64,
        nb03: u64,
        ne0: i32,
        ne1: i32,
        ne2: i32,
        ne3: i32,
        nb0: u64,
        nb1: u64,
        nb2: u64,
        nb3: u64,
        slope: f32,
        scale: f32,
        bias: f32,
        val: f32,
        min: f32,
        max: f32,
    }

    #[repr(C)]
    #[derive(Copy, Clone)]
    struct KArgsBin {
        ne00: i32,
        ne01: i32,
        ne02: i32,
        ne03: i32,
        nb00: u64,
        nb01: u64,
        nb02: u64,
        nb03: u64,
        ne10: i32,
        ne11: i32,
        ne12: i32,
        ne13: i32,
        nb10: u64,
        nb11: u64,
        nb12: u64,
        nb13: u64,
        ne0: i32,
        ne1: i32,
        ne2: i32,
        ne3: i32,
        nb0: u64,
        nb1: u64,
        nb2: u64,
        nb3: u64,
        offs: u64,
        o1: [u64; 8],
    }

    #[repr(C)]
    #[derive(Copy, Clone)]
    struct KArgsNorm {
        ne00: i32,
        ne00_t: i32,
        nb1: u64,
        nb2: u64,
        nb3: u64,
        eps: f32,
        nef1: [i32; 3],
        nef2: [i32; 3],
        nef3: [i32; 3],
        nbf1: [u64; 3],
        nbf2: [u64; 3],
        nbf3: [u64; 3],
    }

    #[repr(C)]
    #[derive(Copy, Clone)]
    struct KArgsIm2Col {
        ofs0: u64,
        ofs1: u64,
        iw: i32,
        ih: i32,
        chw: i32,
        s0: i32,
        s1: i32,
        p0: i32,
        p1: i32,
        d0: i32,
        d1: i32,
        n: i32,
        kh: i32,
        kw: i32,
        khw: i32,
    }

    #[derive(Copy, Clone)]
    struct Shape4 {
        ne: [i32; 4],
        nb: [u64; 4],
        numel: usize,
    }

    struct StrongId(NonNull<Object>);

    impl StrongId {
        unsafe fn from_owned(id: ObjcId) -> Option<Self> {
            NonNull::new(id).map(Self)
        }

        unsafe fn from_unowned(id: ObjcId) -> Option<Self> {
            if id.is_null() {
                return None;
            }
            let _: () = msg_send![id, retain];
            NonNull::new(id).map(Self)
        }

        fn as_id(&self) -> ObjcId {
            self.0.as_ptr()
        }
    }

    impl Drop for StrongId {
        fn drop(&mut self) {
            unsafe {
                let _: () = msg_send![self.0.as_ptr(), release];
            }
        }
    }

    struct AutoreleasePool(ObjcId);

    impl AutoreleasePool {
        fn new() -> Self {
            let pool: ObjcId = unsafe { msg_send![class!(NSAutoreleasePool), new] };
            Self(pool)
        }
    }

    impl Drop for AutoreleasePool {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    let _: () = msg_send![self.0, release];
                }
            }
        }
    }

    fn nsstring_to_string(ns_string: ObjcId) -> String {
        if ns_string.is_null() {
            return String::new();
        }
        unsafe {
            let utf8_ptr: *const c_char = msg_send![ns_string, UTF8String];
            if utf8_ptr.is_null() {
                return String::new();
            }
            CStr::from_ptr(utf8_ptr).to_string_lossy().into_owned()
        }
    }

    fn str_to_nsstring_owned(s: &str) -> ObjcId {
        unsafe {
            let ns_string: ObjcId = msg_send![class!(NSString), alloc];
            if ns_string.is_null() {
                return nil;
            }
            msg_send![
                ns_string,
                initWithBytes: s.as_ptr() as *const c_void
                length: s.len() as u64
                encoding: UTF8_ENCODING
            ]
        }
    }

    fn ns_error_to_string(error: ObjcId) -> String {
        if error.is_null() {
            return "unknown Metal error".to_string();
        }
        unsafe {
            let desc: ObjcId = msg_send![error, localizedDescription];
            nsstring_to_string(desc)
        }
    }

    fn device_supports_family(device: ObjcId, family: u64) -> bool {
        unsafe { msg_send![device, supportsFamily: family] }
    }

    fn metal_compile_feature_macros(device: ObjcId) -> (bool, bool) {
        let mut has_bfloat = device_supports_family(device, MTL_GPU_FAMILY_METAL3)
            || device_supports_family(device, MTL_GPU_FAMILY_APPLE6);
        if std::env::var("GGML_METAL_BF16_DISABLE").is_ok() {
            has_bfloat = false;
        }

        let mut has_tensor = device_supports_family(device, MTL_GPU_FAMILY_METAL4);
        if std::env::var("GGML_METAL_TENSOR_DISABLE").is_ok() {
            has_tensor = false;
        }

        if std::env::var("GGML_METAL_TENSOR_ENABLE").is_err() && has_tensor {
            let dev_name_obj: ObjcId = unsafe { msg_send![device, name] };
            let dev_name = nsstring_to_string(dev_name_obj);
            let tensor_whitelisted = dev_name.contains("M5")
                || dev_name.contains("M6")
                || dev_name.contains("A19")
                || dev_name.contains("A20");
            if !tensor_whitelisted {
                has_tensor = false;
            }
        }

        (has_bfloat, has_tensor)
    }

    fn build_ggml_source() -> String {
        let mut src = _GGML_METAL_SOURCE_RAW.to_string();
        src = src.replace("__embed_ggml-common.h__", _GGML_COMMON_H);
        src = src.replace("#include \"ggml-common.h\"", _GGML_COMMON_H);
        src = src.replace("#include \"ggml-metal-impl.h\"", _GGML_METAL_IMPL_H);
        src
    }

    fn src0_type_from_ggml(t: u32) -> Option<Src0Type> {
        match t {
            GGML_TYPE_F32 => Some(Src0Type::F32),
            GGML_TYPE_F16 => Some(Src0Type::F16),
            GGML_TYPE_Q4_0 => Some(Src0Type::Q4_0),
            GGML_TYPE_Q4_1 => Some(Src0Type::Q4_1),
            GGML_TYPE_Q5_0 => Some(Src0Type::Q5_0),
            GGML_TYPE_Q5_1 => Some(Src0Type::Q5_1),
            GGML_TYPE_Q8_0 => Some(Src0Type::Q8_0),
            _ => None,
        }
    }

    fn src0_type_name(t: Src0Type) -> &'static str {
        match t {
            Src0Type::F32 => "f32",
            Src0Type::F16 => "f16",
            Src0Type::Q4_0 => "q4_0",
            Src0Type::Q4_1 => "q4_1",
            Src0Type::Q5_0 => "q5_0",
            Src0Type::Q5_1 => "q5_1",
            Src0Type::Q8_0 => "q8_0",
        }
    }

    fn src0_layout_bytes_per_row(t: Src0Type, k: usize) -> Result<(usize, u64), String> {
        match t {
            Src0Type::F32 => Ok((
                k.checked_mul(4)
                    .ok_or_else(|| "overflow computing f32 row bytes".to_string())?,
                4,
            )),
            Src0Type::F16 => Ok((
                k.checked_mul(2)
                    .ok_or_else(|| "overflow computing f16 row bytes".to_string())?,
                2,
            )),
            Src0Type::Q4_0 | Src0Type::Q4_1 | Src0Type::Q5_0 | Src0Type::Q5_1 | Src0Type::Q8_0 => {
                if k % 32 != 0 {
                    return Err(format!(
                        "quantized kernel requires K multiple of 32, got {}",
                        k
                    ));
                }
                let ggml_type = match t {
                    Src0Type::Q4_0 => GGML_TYPE_Q4_0,
                    Src0Type::Q4_1 => GGML_TYPE_Q4_1,
                    Src0Type::Q5_0 => GGML_TYPE_Q5_0,
                    Src0Type::Q5_1 => GGML_TYPE_Q5_1,
                    Src0Type::Q8_0 => GGML_TYPE_Q8_0,
                    _ => unreachable!(),
                };
                let bs = block_size(ggml_type);
                let row = (k / 32)
                    .checked_mul(bs)
                    .ok_or_else(|| "overflow computing quantized row bytes".to_string())?;
                Ok((row, bs as u64))
            }
        }
    }

    fn shape4_from_row_major(shape: &[usize], elem_bytes: u64) -> Result<Shape4, String> {
        if shape.is_empty() {
            return Err("shape must be non-empty".to_string());
        }
        if shape.len() > 4 {
            return Err(format!(
                "shape rank > 4 is unsupported in metal elementwise path: {:?}",
                shape
            ));
        }
        let mut ne = [1i32; 4];
        for (i, &d) in shape.iter().rev().enumerate() {
            ne[i] = i32::try_from(d).map_err(|_| format!("shape dim too large: {}", d))?;
        }
        let mut nb = [0u64; 4];
        nb[0] = elem_bytes;
        for i in 1..4 {
            nb[i] = nb[i - 1]
                .checked_mul(ne[i - 1] as u64)
                .ok_or_else(|| "overflow computing strides".to_string())?;
        }
        let numel = shape.iter().try_fold(1usize, |acc, &d| {
            acc.checked_mul(d)
                .ok_or_else(|| "overflow computing tensor numel".to_string())
        })?;
        Ok(Shape4 { ne, nb, numel })
    }

    fn nrows(s: &Shape4) -> usize {
        (s.ne[1] as usize)
            .saturating_mul(s.ne[2] as usize)
            .saturating_mul(s.ne[3] as usize)
    }

    fn f32_slice_as_bytes(s: &[f32]) -> &[u8] {
        unsafe {
            std::slice::from_raw_parts(
                s.as_ptr() as *const u8,
                s.len() * std::mem::size_of::<f32>(),
            )
        }
    }

    fn flash_attn_supported_head_dim(d: usize) -> bool {
        matches!(
            d,
            32 | 40 | 48 | 64 | 72 | 80 | 96 | 112 | 128 | 192 | 256 | 576
        )
    }

    fn flash_attn_use_vec(n_q: usize, d: usize) -> bool {
        n_q < 20 && d % 32 == 0
    }

    #[derive(Clone, Copy, Default)]
    struct FlashAttnExtParams {
        has_mask: bool,
        has_sinks: bool,
        max_bias: f32,
        logit_softcap: f32,
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

        // Matches ggml-metal FATTN_SMEM() for non-quantized f32 K/V.
        let words = nqptg.saturating_mul(dk + 2 * pad_to(dv, 64) + 2 * (2 * ncpsg));
        pad_to(words.saturating_mul(std::mem::size_of::<f32>() / 2), 16)
    }

    fn flash_attn_vec_smem_bytes(dk: usize, dv: usize, nsg: i32) -> usize {
        let ncpsg = OP_FLASH_ATTN_EXT_VEC_NCPSG as usize;
        let words = (pad_to(dk, 128) + 4 * ncpsg + 2 * pad_to(dv, 128))
            .saturating_mul(nsg.max(1) as usize);
        pad_to(words.saturating_mul(std::mem::size_of::<f32>() / 2), 16)
    }

    fn flash_attn_ext_extra_pad_bytes(
        n_q: usize,
        n_kv: usize,
        n_head: usize,
        d: usize,
        has_mask: bool,
        use_vec: bool,
    ) -> Result<usize, String> {
        // Match ggml-metal: reserve non-vec sized padding space, but gate by the active kernel kvpad.
        let reserve_ncpsg = OP_FLASH_ATTN_EXT_NCPSG as usize;
        let active_ncpsg = if use_vec {
            OP_FLASH_ATTN_EXT_VEC_NCPSG as usize
        } else {
            OP_FLASH_ATTN_EXT_NCPSG as usize
        };
        let has_kvpad = n_kv % active_ncpsg != 0;
        if !has_kvpad {
            return Ok(0);
        }

        let n_state = n_head
            .checked_mul(d)
            .ok_or_else(|| "overflow computing flash n_state".to_string())?;
        let nb11 = n_state
            .checked_mul(std::mem::size_of::<f32>())
            .ok_or_else(|| "overflow computing flash nb11".to_string())?;
        let nb21 = nb11;

        let k_term = nb11
            .checked_mul(n_head)
            .ok_or_else(|| "overflow computing flash extra pad K bytes".to_string())?;
        let v_term = nb21
            .checked_mul(n_head)
            .ok_or_else(|| "overflow computing flash extra pad V bytes".to_string())?;
        let mask_term = if has_mask {
            std::mem::size_of::<u16>()
                .checked_mul(n_q)
                .ok_or_else(|| "overflow computing flash extra pad mask bytes".to_string())?
        } else {
            0
        };

        reserve_ncpsg
            .checked_mul(
                k_term
                    .checked_add(v_term)
                    .and_then(|v| v.checked_add(mask_term))
                    .ok_or_else(|| "overflow computing flash extra pad size".to_string())?,
            )
            .ok_or_else(|| "overflow computing flash extra pad size".to_string())
    }

    fn flash_attn_ext_extra_blk_bytes(
        n_q: usize,
        n_kv: usize,
        has_mask: bool,
        use_vec: bool,
    ) -> Result<usize, String> {
        if !has_mask {
            return Ok(0);
        }

        let nqptg = if use_vec {
            OP_FLASH_ATTN_EXT_VEC_NQPSG as usize
        } else {
            OP_FLASH_ATTN_EXT_NQPSG as usize
        };
        let ncpsg = if use_vec {
            OP_FLASH_ATTN_EXT_VEC_NCPSG as usize
        } else {
            OP_FLASH_ATTN_EXT_NCPSG as usize
        };

        let ne1 = (n_q + nqptg - 1) / nqptg;
        let ne0 = (n_kv + ncpsg - 1) / ncpsg;
        let raw = ne0
            .checked_mul(ne1)
            .ok_or_else(|| "overflow computing flash extra blk size".to_string())?;

        Ok(pad_to(raw, 32))
    }

    fn flash_attn_ext_extra_tmp_bytes(
        n_q: usize,
        n_head: usize,
        d: usize,
        nwg: usize,
    ) -> Result<usize, String> {
        let ne01_max = n_q.min(32);
        std::mem::size_of::<f32>()
            .checked_mul(ne01_max)
            .and_then(|v| v.checked_mul(n_head))
            .and_then(|v| v.checked_mul(nwg))
            .and_then(|v| v.checked_mul(d + 2))
            .ok_or_else(|| "overflow computing flash extra tmp size".to_string())
    }

    fn can_use_mul_mv_ext(src0: Src0Type, ne00: i32, ne11: i32) -> bool {
        if ne00 % 128 != 0 {
            return false;
        }
        if !(2..=8).contains(&ne11) {
            return false;
        }
        matches!(
            src0,
            Src0Type::F32
                | Src0Type::F16
                | Src0Type::Q4_0
                | Src0Type::Q4_1
                | Src0Type::Q5_0
                | Src0Type::Q5_1
                | Src0Type::Q8_0
        )
    }

    struct PipelineState {
        obj: StrongId,
        smem: usize,
        nsg: i32,
        nr0: i32,
        nr1: i32,
    }

    struct DecoderKvLayer {
        k: StrongId,
        v: StrongId,
        n_state: usize,
        cap_rows: usize,
        len_rows: usize,
    }

    struct CrossKvLayer {
        k: StrongId,
        v: StrongId,
        n_state: usize,
        n_rows: usize,
        src_k_ptr: usize,
        src_v_ptr: usize,
        src_k_len: usize,
        src_v_len: usize,
    }

    struct ScratchBuffer {
        buf: StrongId,
        cap_bytes: usize,
    }

    struct MetalContext {
        device: StrongId,
        command_queue: StrongId,
        library: StrongId,
        pipeline_cache: HashMap<String, PipelineState>,
        cached_weight_buffers: HashMap<BufferKey, StrongId>,
        scratch_buffers: HashMap<u8, ScratchBuffer>,
        matmul_out_buffers: HashMap<u8, ScratchBuffer>,
        decoder_kv_layers: HashMap<usize, DecoderKvLayer>,
        cross_kv_layers: HashMap<usize, CrossKvLayer>,
        batch_depth: usize,
        batch_command_buffer: Option<StrongId>,
        batch_encoder: Option<StrongId>,
        last_command_buffer: Option<StrongId>,
    }

    impl MetalContext {
        fn create_device() -> Option<StrongId> {
            unsafe {
                let dev = MTLCreateSystemDefaultDevice();
                if let Some(dev) = StrongId::from_owned(dev) {
                    return Some(dev);
                }

                let all = MTLCopyAllDevices();
                if all.is_null() {
                    return None;
                }

                let count: u64 = msg_send![all, count];
                let first: ObjcId = if count > 0 {
                    msg_send![all, objectAtIndex: 0u64]
                } else {
                    nil
                };
                let _: () = msg_send![all, release];

                StrongId::from_unowned(first)
            }
        }

        fn new() -> Result<Self, String> {
            let _pool = AutoreleasePool::new();

            let device = Self::create_device().ok_or_else(|| {
                "unable to create Metal device (MTLCreateSystemDefaultDevice and MTLCopyAllDevices returned nil)"
                    .to_string()
            })?;

            let command_queue_obj: ObjcId = unsafe { msg_send![device.as_id(), newCommandQueue] };
            let command_queue = unsafe { StrongId::from_owned(command_queue_obj) }
                .ok_or_else(|| "newCommandQueue returned nil".to_string())?;

            let library = match Self::load_library_from_metallib(device.as_id()) {
                Ok(Some(lib)) => lib,
                Ok(None) => {
                    let source = build_ggml_source();
                    Self::compile_library(device.as_id(), &source)?
                }
                Err(err) => {
                    eprintln!(
                        "[voice][metal] precompiled metallib load failed, compiling source: {}",
                        err
                    );
                    let source = build_ggml_source();
                    Self::compile_library(device.as_id(), &source)?
                }
            };

            eprintln!("[voice][metal] backend initialized (ggml kernels)");

            Ok(Self {
                device,
                command_queue,
                library,
                pipeline_cache: HashMap::new(),
                cached_weight_buffers: HashMap::new(),
                scratch_buffers: HashMap::new(),
                matmul_out_buffers: HashMap::new(),
                decoder_kv_layers: HashMap::new(),
                cross_kv_layers: HashMap::new(),
                batch_depth: 0,
                batch_command_buffer: None,
                batch_encoder: None,
                last_command_buffer: None,
            })
        }

        fn load_library_from_metallib(device: ObjcId) -> Result<Option<StrongId>, String> {
            if _GGML_METALLIB_BYTES.is_empty() {
                return Ok(None);
            }

            let _pool = AutoreleasePool::new();

            let data_obj: ObjcId = unsafe {
                msg_send![
                    class!(NSData),
                    dataWithBytes: _GGML_METALLIB_BYTES.as_ptr() as *const c_void
                    length: _GGML_METALLIB_BYTES.len() as u64
                ]
            };
            if data_obj.is_null() {
                return Err("NSData::dataWithBytes returned nil".to_string());
            }

            let mut error: ObjcId = nil;
            let library_obj: ObjcId =
                unsafe { msg_send![device, newLibraryWithData: data_obj error: &mut error] };
            if library_obj.is_null() {
                return Err(format!(
                    "newLibraryWithData failed: {}",
                    ns_error_to_string(error)
                ));
            }

            let library = unsafe { StrongId::from_owned(library_obj) }
                .ok_or_else(|| "newLibraryWithData returned nil".to_string())?;
            Ok(Some(library))
        }

        fn compile_library(device: ObjcId, source: &str) -> Result<StrongId, String> {
            let _pool = AutoreleasePool::new();

            let options_obj: ObjcId = unsafe { msg_send![class!(MTLCompileOptions), new] };
            let options = unsafe { StrongId::from_owned(options_obj) }
                .ok_or_else(|| "MTLCompileOptions::new returned nil".to_string())?;
            unsafe {
                let _: () = msg_send![options.as_id(), setFastMathEnabled: YES];
            }

            let (has_bfloat, has_tensor) = metal_compile_feature_macros(device);
            if has_bfloat || has_tensor {
                let prep_obj: ObjcId = unsafe { msg_send![class!(NSMutableDictionary), dictionary] };
                if !prep_obj.is_null() {
                    if has_bfloat {
                        let key_obj = str_to_nsstring_owned("GGML_METAL_HAS_BF16");
                        let val_obj = str_to_nsstring_owned("1");
                        let key = unsafe { StrongId::from_owned(key_obj) }
                            .ok_or_else(|| "failed to build metal macro key".to_string())?;
                        let val = unsafe { StrongId::from_owned(val_obj) }
                            .ok_or_else(|| "failed to build metal macro value".to_string())?;
                        unsafe {
                            let _: () =
                                msg_send![prep_obj, setObject: val.as_id() forKey: key.as_id()];
                        }
                    }
                    if has_tensor {
                        let key_obj = str_to_nsstring_owned("GGML_METAL_HAS_TENSOR");
                        let val_obj = str_to_nsstring_owned("1");
                        let key = unsafe { StrongId::from_owned(key_obj) }
                            .ok_or_else(|| "failed to build metal macro key".to_string())?;
                        let val = unsafe { StrongId::from_owned(val_obj) }
                            .ok_or_else(|| "failed to build metal macro value".to_string())?;
                        unsafe {
                            let _: () =
                                msg_send![prep_obj, setObject: val.as_id() forKey: key.as_id()];
                        }
                    }
                    unsafe {
                        let _: () = msg_send![options.as_id(), setPreprocessorMacros: prep_obj];
                    }
                }
            }

            let source_obj = str_to_nsstring_owned(source);
            let source_obj = unsafe { StrongId::from_owned(source_obj) }
                .ok_or_else(|| "failed to create NSString for Metal source".to_string())?;

            let mut error: ObjcId = nil;
            let library_obj: ObjcId = unsafe {
                msg_send![
                    device,
                    newLibraryWithSource: source_obj.as_id()
                    options: options.as_id()
                    error: &mut error
                ]
            };

            unsafe { StrongId::from_owned(library_obj) }.ok_or_else(|| {
                format!("newLibraryWithSource failed: {}", ns_error_to_string(error))
            })
        }

        fn new_buffer_with_bytes(&self, bytes: &[u8]) -> Result<StrongId, String> {
            let obj: ObjcId = unsafe {
                msg_send![
                    self.device.as_id(),
                    newBufferWithBytes: bytes.as_ptr() as *const c_void
                    length: bytes.len() as u64
                    options: MTL_RESOURCE_STORAGE_MODE_SHARED
                ]
            };
            unsafe { StrongId::from_owned(obj) }
                .ok_or_else(|| format!("newBufferWithBytes failed for {} bytes", bytes.len()))
        }

        fn new_buffer_with_length(&self, byte_len: usize) -> Result<StrongId, String> {
            let obj: ObjcId = unsafe {
                msg_send![
                    self.device.as_id(),
                    newBufferWithLength: byte_len as u64
                    options: MTL_RESOURCE_STORAGE_MODE_SHARED
                ]
            };
            unsafe { StrongId::from_owned(obj) }
                .ok_or_else(|| format!("newBufferWithLength failed for {} bytes", byte_len))
        }

        fn get_or_create_scratch_buffer(&mut self, kind: u8, need_bytes: usize) -> Result<ObjcId, String> {
            let need_bytes = need_bytes.max(1);
            if let Some(entry) = self.scratch_buffers.get(&kind) {
                if entry.cap_bytes >= need_bytes {
                    return Ok(entry.buf.as_id());
                }
            }

            let buf = self.new_buffer_with_length(need_bytes)?;
            self.scratch_buffers.insert(
                kind,
                ScratchBuffer {
                    buf,
                    cap_bytes: need_bytes,
                },
            );

            Ok(self.scratch_buffers.get(&kind).unwrap().buf.as_id())
        }

        fn get_or_create_matmul_out_buffer(
            &mut self,
            tag: u8,
            need_bytes: usize,
        ) -> Result<ObjcId, String> {
            let need_bytes = need_bytes.max(1);
            if let Some(entry) = self.matmul_out_buffers.get(&tag) {
                if entry.cap_bytes >= need_bytes {
                    return Ok(entry.buf.as_id());
                }
            }

            let buf = self.new_buffer_with_length(need_bytes)?;
            self.matmul_out_buffers.insert(
                tag,
                ScratchBuffer {
                    buf,
                    cap_bytes: need_bytes,
                },
            );

            Ok(self.matmul_out_buffers.get(&tag).unwrap().buf.as_id())
        }

        fn read_f32_buffer(&self, buffer: ObjcId, elems: usize) -> Result<Vec<f32>, String> {
            self.wait_queue_idle()?;

            let out_ptr: *const c_void = unsafe { msg_send![buffer, contents] };
            if out_ptr.is_null() {
                return Err("output buffer contents returned null".to_string());
            }

            let mut out = vec![0.0f32; elems];
            unsafe {
                std::ptr::copy_nonoverlapping(out_ptr as *const f32, out.as_mut_ptr(), elems);
            }
            Ok(out)
        }

        fn get_or_create_cached_f32_buffer(
            &mut self,
            data: &[f32],
            tag: u8,
        ) -> Result<ObjcId, String> {
            let bytes = f32_slice_as_bytes(data);
            let key = BufferKey {
                ptr: data.as_ptr() as usize,
                len: bytes.len(),
                tag,
            };
            self.get_or_create_weight_buffer(key, bytes)
        }

        fn get_or_create_weight_buffer(
            &mut self,
            key: BufferKey,
            bytes: &[u8],
        ) -> Result<ObjcId, String> {
            if !self.cached_weight_buffers.contains_key(&key) {
                let buf = self.new_buffer_with_bytes(bytes)?;
                self.cached_weight_buffers.insert(key, buf);
            }
            Ok(self.cached_weight_buffers.get(&key).unwrap().as_id())
        }

        fn clear_decoder_kv_cache(&mut self) {
            self.decoder_kv_layers.clear();
            self.cross_kv_layers.clear();
        }

        fn ensure_decoder_kv_layer(
            &mut self,
            layer: usize,
            n_state: usize,
            need_rows: usize,
        ) -> Result<(ObjcId, ObjcId), String> {
            let need_rows = need_rows.max(1);

            if let Some(entry) = self.decoder_kv_layers.get(&layer) {
                if entry.n_state == n_state && entry.cap_rows >= need_rows {
                    return Ok((entry.k.as_id(), entry.v.as_id()));
                }
            }

            let row_bytes = n_state
                .checked_mul(std::mem::size_of::<f32>())
                .ok_or_else(|| "overflow computing decoder kv row bytes".to_string())?;

            let old = self.decoder_kv_layers.remove(&layer);
            let cap_rows = if let Some(ref old) = old {
                if old.n_state == n_state {
                    old.cap_rows.saturating_mul(2).max(need_rows).max(32)
                } else {
                    need_rows.max(32)
                }
            } else {
                need_rows.max(32)
            };
            let total_bytes = cap_rows
                .checked_mul(row_bytes)
                .ok_or_else(|| "overflow computing decoder kv bytes".to_string())?;

            let new_k = self.new_buffer_with_length(total_bytes)?;
            let new_v = self.new_buffer_with_length(total_bytes)?;

            let mut len_rows = 0usize;
            if let Some(old) = old {
                if old.n_state == n_state && old.len_rows > 0 {
                    let copy_rows = old.len_rows.min(cap_rows);
                    let copy_bytes = copy_rows
                        .checked_mul(row_bytes)
                        .ok_or_else(|| "overflow computing decoder kv copy bytes".to_string())?;
                    let old_k_ptr: *const u8 = unsafe { msg_send![old.k.as_id(), contents] };
                    let old_v_ptr: *const u8 = unsafe { msg_send![old.v.as_id(), contents] };
                    let new_k_ptr: *mut u8 = unsafe { msg_send![new_k.as_id(), contents] };
                    let new_v_ptr: *mut u8 = unsafe { msg_send![new_v.as_id(), contents] };
                    if old_k_ptr.is_null()
                        || old_v_ptr.is_null()
                        || new_k_ptr.is_null()
                        || new_v_ptr.is_null()
                    {
                        return Err("decoder kv buffer contents returned null".to_string());
                    }
                    unsafe {
                        std::ptr::copy_nonoverlapping(old_k_ptr, new_k_ptr, copy_bytes);
                        std::ptr::copy_nonoverlapping(old_v_ptr, new_v_ptr, copy_bytes);
                    }
                    len_rows = copy_rows;
                }
            }

            self.decoder_kv_layers.insert(
                layer,
                DecoderKvLayer {
                    k: new_k,
                    v: new_v,
                    n_state,
                    cap_rows,
                    len_rows,
                },
            );
            let entry = self
                .decoder_kv_layers
                .get(&layer)
                .ok_or_else(|| "decoder kv layer insertion failed".to_string())?;
            Ok((entry.k.as_id(), entry.v.as_id()))
        }

        fn ensure_cross_kv_layer(
            &mut self,
            layer: usize,
            n_state: usize,
            n_rows: usize,
            k_cross: &[f32],
            v_cross: &[f32],
        ) -> Result<(ObjcId, ObjcId), String> {
            let need = n_rows
                .checked_mul(n_state)
                .ok_or_else(|| "overflow computing cross kv size".to_string())?;
            if k_cross.len() != need || v_cross.len() != need {
                return Err(format!(
                    "cross kv len mismatch: k={}, v={}, expected={}",
                    k_cross.len(),
                    v_cross.len(),
                    need
                ));
            }

            let src_k_ptr = k_cross.as_ptr() as usize;
            let src_v_ptr = v_cross.as_ptr() as usize;
            let src_k_len = k_cross.len();
            let src_v_len = v_cross.len();

            if let Some(entry) = self.cross_kv_layers.get(&layer) {
                if entry.n_state == n_state
                    && entry.n_rows == n_rows
                    && entry.src_k_ptr == src_k_ptr
                    && entry.src_v_ptr == src_v_ptr
                    && entry.src_k_len == src_k_len
                    && entry.src_v_len == src_v_len
                {
                    return Ok((entry.k.as_id(), entry.v.as_id()));
                }
            }

            let k_buf = self.new_buffer_with_bytes(f32_slice_as_bytes(k_cross))?;
            let v_buf = self.new_buffer_with_bytes(f32_slice_as_bytes(v_cross))?;
            self.cross_kv_layers.insert(
                layer,
                CrossKvLayer {
                    k: k_buf,
                    v: v_buf,
                    n_state,
                    n_rows,
                    src_k_ptr,
                    src_v_ptr,
                    src_k_len,
                    src_v_len,
                },
            );

            let entry = self
                .cross_kv_layers
                .get(&layer)
                .ok_or_else(|| "cross kv layer insertion failed".to_string())?;
            Ok((entry.k.as_id(), entry.v.as_id()))
        }

        fn compile_pipeline(
            &self,
            base_name: &str,
            constants: &[FunctionConstant],
        ) -> Result<StrongId, String> {
            let _pool = AutoreleasePool::new();

            let base_obj = str_to_nsstring_owned(base_name);
            let base_obj = unsafe { StrongId::from_owned(base_obj) }
                .ok_or_else(|| format!("failed to create NSString for function '{}'", base_name))?;

            let mut error: ObjcId = nil;
            let func_obj: ObjcId = if constants.is_empty() {
                unsafe { msg_send![self.library.as_id(), newFunctionWithName: base_obj.as_id()] }
            } else {
                let cv_obj: ObjcId = unsafe { msg_send![class!(MTLFunctionConstantValues), new] };
                let cv = unsafe { StrongId::from_owned(cv_obj) }
                    .ok_or_else(|| "MTLFunctionConstantValues::new returned nil".to_string())?;

                for c in constants {
                    unsafe {
                        match c.value {
                            FunctionConstantValue::Int32(v) => {
                                let _: () = msg_send![
                                    cv.as_id(),
                                    setConstantValue: &v as *const i32 as *const c_void
                                    type: MTL_DATA_TYPE_INT
                                    atIndex: c.idx as u64
                                ];
                            }
                            FunctionConstantValue::Int16(v) => {
                                let _: () = msg_send![
                                    cv.as_id(),
                                    setConstantValue: &v as *const i16 as *const c_void
                                    type: MTL_DATA_TYPE_SHORT
                                    atIndex: c.idx as u64
                                ];
                            }
                            FunctionConstantValue::Bool(v) => {
                                let b: u8 = if v { 1 } else { 0 };
                                let _: () = msg_send![
                                    cv.as_id(),
                                    setConstantValue: &b as *const u8 as *const c_void
                                    type: MTL_DATA_TYPE_BOOL
                                    atIndex: c.idx as u64
                                ];
                            }
                        }
                    }
                }

                unsafe {
                    msg_send![
                        self.library.as_id(),
                        newFunctionWithName: base_obj.as_id()
                        constantValues: cv.as_id()
                        error: &mut error
                    ]
                }
            };

            let func = unsafe { StrongId::from_owned(func_obj) }.ok_or_else(|| {
                format!(
                    "newFunctionWithName('{}') failed: {}",
                    base_name,
                    ns_error_to_string(error)
                )
            })?;

            let mut error: ObjcId = nil;
            let pipeline_obj: ObjcId = unsafe {
                msg_send![
                    self.device.as_id(),
                    newComputePipelineStateWithFunction: func.as_id()
                    error: &mut error
                ]
            };

            unsafe { StrongId::from_owned(pipeline_obj) }.ok_or_else(|| {
                format!(
                    "newComputePipelineStateWithFunction('{}') failed: {}",
                    base_name,
                    ns_error_to_string(error)
                )
            })
        }

        fn get_or_compile_cached_pipeline(
            &mut self,
            cache_name: String,
            base_name: &str,
            constants: &[FunctionConstant],
            smem: usize,
            nr0: i32,
            nr1: i32,
            nsg: i32,
        ) -> Result<(ObjcId, usize, i32, i32, i32), String> {
            if !self.pipeline_cache.contains_key(&cache_name) {
                let compiled = self.compile_pipeline(base_name, constants)?;
                self.pipeline_cache.insert(
                    cache_name.clone(),
                    PipelineState {
                        obj: compiled,
                        smem,
                        nsg,
                        nr0,
                        nr1,
                    },
                );
            }

            let p = self.pipeline_cache.get(&cache_name).unwrap();
            Ok((p.obj.as_id(), p.smem, p.nr0, p.nr1, p.nsg))
        }

        fn pipeline_max_threads(pipeline: ObjcId) -> u64 {
            unsafe { msg_send![pipeline, maxTotalThreadsPerThreadgroup] }
        }

        fn begin_batch(&mut self) -> Result<(), String> {
            if self.batch_depth == 0 {
                let command_buffer_obj: ObjcId =
                    unsafe { msg_send![self.command_queue.as_id(), commandBuffer] };
                let command_buffer = unsafe { StrongId::from_unowned(command_buffer_obj) }
                    .ok_or_else(|| "commandBuffer returned nil".to_string())?;

                let encoder_obj: ObjcId =
                    unsafe { msg_send![command_buffer.as_id(), computeCommandEncoder] };
                let encoder = unsafe { StrongId::from_unowned(encoder_obj) }
                    .ok_or_else(|| "computeCommandEncoder returned nil".to_string())?;

                self.batch_command_buffer = Some(command_buffer);
                self.batch_encoder = Some(encoder);
            }
            self.batch_depth += 1;
            Ok(())
        }

        fn end_batch(&mut self) -> Result<(), String> {
            if self.batch_depth == 0 {
                return Err("end_batch called with no active batch".to_string());
            }

            self.batch_depth -= 1;
            if self.batch_depth == 0 {
                let command_buffer = self
                    .batch_command_buffer
                    .take()
                    .ok_or_else(|| "batch command buffer missing".to_string())?;
                let encoder = self
                    .batch_encoder
                    .take()
                    .ok_or_else(|| "batch encoder missing".to_string())?;

                unsafe {
                    let _: () = msg_send![encoder.as_id(), endEncoding];
                    let _: () = msg_send![command_buffer.as_id(), commit];
                }

                self.last_command_buffer = Some(command_buffer);
            }

            Ok(())
        }

        fn with_batch<T, F>(&mut self, f: F) -> Result<T, String>
        where
            F: FnOnce(&mut Self) -> Result<T, String>,
        {
            self.begin_batch()?;
            let out = f(self);
            let end_res = self.end_batch();
            match (out, end_res) {
                (Ok(v), Ok(())) => Ok(v),
                (Err(e), Ok(())) => Err(e),
                (Ok(_), Err(e)) => Err(e),
                (Err(e), Err(_)) => Err(e),
            }
        }

        fn begin_command_encoder(
            &self,
        ) -> Result<(ObjcId, ObjcId, Option<(StrongId, StrongId)>), String> {
            if self.batch_depth > 0 {
                let command_buffer = self
                    .batch_command_buffer
                    .as_ref()
                    .ok_or_else(|| "batch command buffer missing".to_string())?;
                let encoder = self
                    .batch_encoder
                    .as_ref()
                    .ok_or_else(|| "batch encoder missing".to_string())?;
                return Ok((command_buffer.as_id(), encoder.as_id(), None));
            }

            let command_buffer_obj: ObjcId =
                unsafe { msg_send![self.command_queue.as_id(), commandBuffer] };
            let command_buffer = unsafe { StrongId::from_unowned(command_buffer_obj) }
                .ok_or_else(|| "commandBuffer returned nil".to_string())?;

            let encoder_obj: ObjcId =
                unsafe { msg_send![command_buffer.as_id(), computeCommandEncoder] };
            let encoder = unsafe { StrongId::from_unowned(encoder_obj) }
                .ok_or_else(|| "computeCommandEncoder returned nil".to_string())?;

            Ok((
                command_buffer.as_id(),
                encoder.as_id(),
                Some((command_buffer, encoder)),
            ))
        }

        fn wait_queue_idle(&self) -> Result<(), String> {
            if self.batch_depth > 0 {
                return Err("wait_queue_idle called while command batch is active".to_string());
            }

            if let Some(command_buffer) = self.last_command_buffer.as_ref() {
                let command_buffer_id = command_buffer.as_id();
                unsafe {
                    let _: () = msg_send![command_buffer_id, waitUntilCompleted];
                }
                let status: u64 = unsafe { msg_send![command_buffer_id, status] };
                if status == 5 {
                    let error: ObjcId = unsafe { msg_send![command_buffer_id, error] };
                    return Err(format!(
                        "Metal command buffer error (queue idle wait): {}",
                        ns_error_to_string(error)
                    ));
                }
                return Ok(());
            }

            let command_buffer_obj: ObjcId =
                unsafe { msg_send![self.command_queue.as_id(), commandBuffer] };
            let command_buffer = unsafe { StrongId::from_unowned(command_buffer_obj) }
                .ok_or_else(|| "commandBuffer returned nil".to_string())?;
            unsafe {
                let _: () = msg_send![command_buffer.as_id(), commit];
                let _: () = msg_send![command_buffer.as_id(), waitUntilCompleted];
            }
            let status: u64 = unsafe { msg_send![command_buffer.as_id(), status] };
            if status == 5 {
                let error: ObjcId = unsafe { msg_send![command_buffer.as_id(), error] };
                return Err(format!(
                    "Metal command buffer error (queue idle wait): {}",
                    ns_error_to_string(error)
                ));
            }
            Ok(())
        }

        fn end_command_encoder(&mut self, handles: Option<(StrongId, StrongId)>) -> Result<(), String> {
            let Some((command_buffer, encoder)) = handles else {
                return Ok(());
            };

            unsafe {
                let _: () = msg_send![encoder.as_id(), endEncoding];
                let _: () = msg_send![command_buffer.as_id(), commit];
            }

            self.last_command_buffer = Some(command_buffer);

            Ok(())
        }

        fn dispatch_mul_mv_ext(
            &mut self,
            src0: Src0Type,
            src0_id: ObjcId,
            src1_id: ObjcId,
            dst_id: ObjcId,
            ne00: i32,
            ne01: i32,
            ne10: i32,
            ne11: i32,
            nb00: u64,
            nb01: u64,
            nb10: u64,
            nb11: u64,
            ne0: i32,
            ne1: i32,
        ) -> Result<(), String> {
            static LOG_ONCE: OnceLock<()> = OnceLock::new();
            if LOG_ONCE.set(()).is_ok() {
                eprintln!("[voice][metal] mul_mat dispatch: mul_mv_ext");
            }

            let nsg = 2i32;
            let nxpsg = if ne00 % 256 == 0 && ne11 < 3 {
                16i32
            } else if ne00 % 128 == 0 {
                8i32
            } else {
                4i32
            };
            let nypsg = 32 / nxpsg;
            let r0ptg = nypsg * nsg;
            let r1ptg = match ne11 {
                2 => 2,
                3 | 6 => 3,
                4 | 7 | 8 => 4,
                5 => 5,
                _ => return Err(format!("unsupported ne11 for mul_mv_ext: {}", ne11)),
            };

            let base = format!(
                "kernel_mul_mv_ext_{}_{}_r1_{}",
                src0_type_name(src0),
                "f32",
                r1ptg
            );
            let name = format!("{}_nsg={}_nxpsg={}", base, nsg, nxpsg);

            let constants = [
                FunctionConstant {
                    idx: FC_MUL_MV + 0,
                    value: FunctionConstantValue::Int16(nsg as i16),
                },
                FunctionConstant {
                    idx: FC_MUL_MV + 1,
                    value: FunctionConstantValue::Int16(nxpsg as i16),
                },
            ];

            let (pipeline, _smem, _nr0, _nr1, _pnsg) =
                self.get_or_compile_cached_pipeline(name, &base, &constants, 0, 0, 0, nsg)?;

            let args = KArgsMulMvExt {
                ne00,
                ne01,
                ne02: 1,
                nb00,
                nb01,
                nb02: nb01 * ne01 as u64,
                nb03: nb01 * ne01 as u64,
                ne10,
                ne11,
                ne12: 1,
                nb10,
                nb11,
                nb12: nb11 * ne11 as u64,
                nb13: nb11 * ne11 as u64,
                ne0,
                ne1,
                r2: 1,
                r3: 1,
            };

            let (_command_buffer, encoder, encoder_handles) = self.begin_command_encoder()?;
            unsafe {
                let _: () = msg_send![encoder, setComputePipelineState: pipeline];
                let _: () = msg_send![
                    encoder,
                    setBytes: &args as *const KArgsMulMvExt as *const c_void
                    length: std::mem::size_of::<KArgsMulMvExt>() as u64
                    atIndex: 0u64
                ];
                let _: () =
                    msg_send![encoder, setBuffer: src0_id offset: 0u64 atIndex: 1u64];
                let _: () =
                    msg_send![encoder, setBuffer: src1_id offset: 0u64 atIndex: 2u64];
                let _: () =
                    msg_send![encoder, setBuffer: dst_id offset: 0u64 atIndex: 3u64];

                let tgs = MTLSize {
                    width: ((ne01 + r0ptg - 1) / r0ptg) as u64,
                    height: ((ne11 + r1ptg - 1) / r1ptg) as u64,
                    depth: 1,
                };
                let tpg = MTLSize {
                    width: 32,
                    height: nsg as u64,
                    depth: 1,
                };
                let _: () = msg_send![
                    encoder,
                    dispatchThreadgroups: tgs
                    threadsPerThreadgroup: tpg
                ];
            }

            self.end_command_encoder(encoder_handles)
        }

        fn dispatch_mul_mm(
            &mut self,
            src0: Src0Type,
            src0_id: ObjcId,
            src1_id: ObjcId,
            dst_id: ObjcId,
            ne00: i32,
            ne01: i32,
            nb01: u64,
            ne12: i32,
            nb10: u64,
            nb11: u64,
            ne0: i32,
            ne1: i32,
        ) -> Result<(), String> {
            static LOG_ONCE: OnceLock<()> = OnceLock::new();
            if LOG_ONCE.set(()).is_ok() {
                eprintln!("[voice][metal] mul_mat dispatch: mul_mm");
            }

            let bc_inp = ne00 % 32 != 0;
            let bc_out = ne0 % 64 != 0 || ne1 % 32 != 0;

            let base = format!("kernel_mul_mm_{}_{}", src0_type_name(src0), "f32");
            let name = format!("{}_bci={}_bco={}", base, bc_inp as i32, bc_out as i32);

            let smem = if bc_out {
                8192usize
            } else {
                4096usize + 2048usize
            };
            let constants = [
                FunctionConstant {
                    idx: FC_MUL_MM + 0,
                    value: FunctionConstantValue::Bool(bc_inp),
                },
                FunctionConstant {
                    idx: FC_MUL_MM + 1,
                    value: FunctionConstantValue::Bool(bc_out),
                },
            ];

            let (pipeline, pipeline_smem, _nr0, _nr1, _nsg) =
                self.get_or_compile_cached_pipeline(name, &base, &constants, smem, 0, 0, 0)?;

            let args = KArgsMulMm {
                ne00,
                ne02: 1,
                nb01,
                nb02: nb01 * ne01 as u64,
                nb03: nb01 * ne01 as u64,
                ne12,
                nb10,
                nb11,
                nb12: nb11 * ne1 as u64,
                nb13: nb11 * ne1 as u64,
                ne0,
                ne1,
                r2: 1,
                r3: 1,
            };

            let (_command_buffer, encoder, encoder_handles) = self.begin_command_encoder()?;
            unsafe {
                let _: () = msg_send![encoder, setComputePipelineState: pipeline];
                let _: () = msg_send![
                    encoder,
                    setBytes: &args as *const KArgsMulMm as *const c_void
                    length: std::mem::size_of::<KArgsMulMm>() as u64
                    atIndex: 0u64
                ];
                let _: () =
                    msg_send![encoder, setBuffer: src0_id offset: 0u64 atIndex: 1u64];
                let _: () =
                    msg_send![encoder, setBuffer: src1_id offset: 0u64 atIndex: 2u64];
                let _: () =
                    msg_send![encoder, setBuffer: dst_id offset: 0u64 atIndex: 3u64];
                let _: () = msg_send![
                    encoder,
                    setThreadgroupMemoryLength: pipeline_smem as u64
                    atIndex: 0u64
                ];

                let tgs = MTLSize {
                    width: ((ne1 + 31) / 32) as u64,
                    height: ((ne01 + 63) / 64) as u64,
                    depth: ne12 as u64,
                };
                let tpg = MTLSize {
                    width: 128,
                    height: 1,
                    depth: 1,
                };
                let _: () = msg_send![
                    encoder,
                    dispatchThreadgroups: tgs
                    threadsPerThreadgroup: tpg
                ];
            }

            self.end_command_encoder(encoder_handles)
        }

        fn dispatch_mul_mv(
            &mut self,
            src0: Src0Type,
            src0_id: ObjcId,
            src1_id: ObjcId,
            dst_id: ObjcId,
            ne00: i32,
            ne01: i32,
            ne10: i32,
            ne11: i32,
            nb00: u64,
            nb01: u64,
            nb10: u64,
            nb11: u64,
            ne0: i32,
            ne1: i32,
        ) -> Result<(), String> {
            static LOG_ONCE: OnceLock<()> = OnceLock::new();
            if LOG_ONCE.set(()).is_ok() {
                eprintln!("[voice][metal] mul_mat dispatch: mul_mv");
            }

            let (nsg, nr0, nr1, smem, suffix) = match src0 {
                Src0Type::F32 | Src0Type::F16 => {
                    if ne00 < 32 {
                        (1, 32, 1, 0usize, "_short")
                    } else {
                        let nsg = ((ne00 + 127) / 128).min(4);
                        let nr0 = 2;
                        let smem = 32usize * std::mem::size_of::<f32>() * nr0 as usize;
                        let suffix = if ne00 % 4 == 0 { "_4" } else { "" };
                        (nsg, nr0, 1, smem, suffix)
                    }
                }
                Src0Type::Q4_0 => (N_SG_Q4_0, N_R0_Q4_0, 1, 0usize, ""),
                Src0Type::Q4_1 => (N_SG_Q4_1, N_R0_Q4_1, 1, 0usize, ""),
                Src0Type::Q5_0 => (N_SG_Q5_0, N_R0_Q5_0, 1, 0usize, ""),
                Src0Type::Q5_1 => (N_SG_Q5_1, N_R0_Q5_1, 1, 0usize, ""),
                Src0Type::Q8_0 => (
                    N_SG_Q8_0,
                    N_R0_Q8_0,
                    1,
                    32usize * std::mem::size_of::<f32>() * N_R0_Q8_0 as usize,
                    "",
                ),
            };

            let base = format!("kernel_mul_mv_{}_{}{}", src0_type_name(src0), "f32", suffix);
            let name = format!("{}_nsg={}", base, nsg);
            let constants = [FunctionConstant {
                idx: FC_MUL_MV + 0,
                value: FunctionConstantValue::Int16(nsg as i16),
            }];

            let (pipeline, _pipeline_smem, pn0, pn1, pnsg) =
                self.get_or_compile_cached_pipeline(name, &base, &constants, smem, nr0, nr1, nsg)?;

            let args = KArgsMulMv {
                ne00,
                ne01,
                ne02: 1,
                nb00,
                nb01,
                nb02: nb01 * ne01 as u64,
                nb03: nb01 * ne01 as u64,
                ne10,
                ne11,
                ne12: 1,
                nb10,
                nb11,
                nb12: nb11 * ne11 as u64,
                nb13: nb11 * ne11 as u64,
                ne0,
                ne1,
                nr0: pn0,
                r2: 1,
                r3: 1,
            };

            let (_command_buffer, encoder, encoder_handles) = self.begin_command_encoder()?;
            unsafe {
                let _: () = msg_send![encoder, setComputePipelineState: pipeline];
                let _: () = msg_send![
                    encoder,
                    setBytes: &args as *const KArgsMulMv as *const c_void
                    length: std::mem::size_of::<KArgsMulMv>() as u64
                    atIndex: 0u64
                ];
                let _: () =
                    msg_send![encoder, setBuffer: src0_id offset: 0u64 atIndex: 1u64];
                let _: () =
                    msg_send![encoder, setBuffer: src1_id offset: 0u64 atIndex: 2u64];
                let _: () =
                    msg_send![encoder, setBuffer: dst_id offset: 0u64 atIndex: 3u64];

                if smem > 0 {
                    let _: () = msg_send![
                        encoder,
                        setThreadgroupMemoryLength: smem as u64
                        atIndex: 0u64
                    ];
                }

                let tg_x = if matches!(src0, Src0Type::F32 | Src0Type::F16 | Src0Type::Q8_0) {
                    (ne01 + pn0 - 1) / pn0
                } else {
                    (ne01 + pn0 * pnsg - 1) / (pn0 * pnsg)
                };
                let tg_y = (ne11 + pn1 - 1) / pn1;

                let tgs = MTLSize {
                    width: tg_x as u64,
                    height: tg_y as u64,
                    depth: 1,
                };
                let tpg = MTLSize {
                    width: 32,
                    height: pnsg as u64,
                    depth: 1,
                };
                let _: () = msg_send![
                    encoder,
                    dispatchThreadgroups: tgs
                    threadsPerThreadgroup: tpg
                ];
            }

            self.end_command_encoder(encoder_handles)
        }

        fn dispatch_unary_f32(
            &mut self,
            op_num: i16,
            src0_id: ObjcId,
            dst_id: ObjcId,
            shape: &Shape4,
        ) -> Result<(), String> {
            let is_c4 = shape.ne[0] % 4 == 0;
            let is_cnt = shape.numel < 32768;

            let base = if is_c4 {
                "kernel_unary_f32_f32_4"
            } else {
                "kernel_unary_f32_f32"
            };
            let name = format!("{}_op={}_cnt={}", base, op_num, is_cnt as i32);

            let constants = [
                FunctionConstant {
                    idx: FC_UNARY + 0,
                    value: FunctionConstantValue::Int16(op_num),
                },
                FunctionConstant {
                    idx: FC_UNARY + 1,
                    value: FunctionConstantValue::Bool(is_cnt),
                },
            ];

            let (pipeline, _smem, _nr0, _nr1, _nsg) =
                self.get_or_compile_cached_pipeline(name, base, &constants, 0, 0, 0, 0)?;

            let mut args = KArgsUnary {
                ne00: shape.ne[0],
                ne01: shape.ne[1],
                ne02: shape.ne[2],
                ne03: shape.ne[3],
                nb00: shape.nb[0],
                nb01: shape.nb[1],
                nb02: shape.nb[2],
                nb03: shape.nb[3],
                ne0: shape.ne[0],
                ne1: shape.ne[1],
                ne2: shape.ne[2],
                ne3: shape.ne[3],
                nb0: shape.nb[0],
                nb1: shape.nb[1],
                nb2: shape.nb[2],
                nb3: shape.nb[3],
                slope: 0.0,
                scale: 0.0,
                bias: 0.0,
                val: 0.0,
                min: 0.0,
                max: 0.0,
            };

            if is_c4 {
                args.ne00 /= 4;
                args.ne0 /= 4;
            }

            let (_command_buffer, encoder, encoder_handles) = self.begin_command_encoder()?;
            unsafe {
                let _: () = msg_send![encoder, setComputePipelineState: pipeline];
                let _: () = msg_send![
                    encoder,
                    setBytes: &args as *const KArgsUnary as *const c_void
                    length: std::mem::size_of::<KArgsUnary>() as u64
                    atIndex: 0u64
                ];
                let _: () =
                    msg_send![encoder, setBuffer: src0_id offset: 0u64 atIndex: 1u64];
                let _: () =
                    msg_send![encoder, setBuffer: dst_id offset: 0u64 atIndex: 2u64];

                if is_cnt {
                    let n = if is_c4 { shape.numel / 4 } else { shape.numel };
                    let tgs = MTLSize {
                        width: n as u64,
                        height: 1,
                        depth: 1,
                    };
                    let tpg = MTLSize {
                        width: 1,
                        height: 1,
                        depth: 1,
                    };
                    let _: () = msg_send![
                        encoder,
                        dispatchThreadgroups: tgs
                        threadsPerThreadgroup: tpg
                    ];
                } else {
                    let nth_max =
                        std::cmp::min(256u64, Self::pipeline_max_threads(pipeline)).max(1u64);
                    let nth = std::cmp::min(args.ne00 as u64, nth_max).max(1u64);
                    let nk0 = ((args.ne00 as u64) + nth - 1) / nth;

                    let tgs = MTLSize {
                        width: nk0.saturating_mul(shape.ne[1] as u64),
                        height: shape.ne[2] as u64,
                        depth: shape.ne[3] as u64,
                    };
                    let tpg = MTLSize {
                        width: nth,
                        height: 1,
                        depth: 1,
                    };
                    let _: () = msg_send![
                        encoder,
                        dispatchThreadgroups: tgs
                        threadsPerThreadgroup: tpg
                    ];
                }
            }

            self.end_command_encoder(encoder_handles)
        }

        fn dispatch_bin_f32(
            &mut self,
            op_num: i16,
            src0_id: ObjcId,
            src1_id: ObjcId,
            dst_id: ObjcId,
            src0_shape: &Shape4,
            src1_shape: &Shape4,
        ) -> Result<(), String> {
            for d in 0..4 {
                let b = src1_shape.ne[d];
                let a = src0_shape.ne[d];
                if b != 1 && b != a {
                    return Err(format!(
                        "binary broadcast mismatch at dim {}: lhs={}, rhs={}",
                        d, a, b
                    ));
                }
            }

            let is_c4 = src0_shape.ne[0] % 4 == 0 && src1_shape.ne[0] % 4 == 0;
            let is_rb = nrows(src1_shape) == 1 && src0_shape.numel < 65536;

            let base = if is_c4 {
                "kernel_bin_fuse_f32_f32_f32_4"
            } else {
                "kernel_bin_fuse_f32_f32_f32"
            };
            let name = format!("{}_op={}_nf=1_rb={}", base, op_num, is_rb as i32);

            let constants = [
                FunctionConstant {
                    idx: FC_BIN + 0,
                    value: FunctionConstantValue::Int16(op_num),
                },
                FunctionConstant {
                    idx: FC_BIN + 1,
                    value: FunctionConstantValue::Int16(1),
                },
                FunctionConstant {
                    idx: FC_BIN + 2,
                    value: FunctionConstantValue::Bool(is_rb),
                },
            ];

            let (pipeline, _smem, _nr0, _nr1, _nsg) =
                self.get_or_compile_cached_pipeline(name, base, &constants, 0, 0, 0, 0)?;

            let mut args = KArgsBin {
                ne00: src0_shape.ne[0],
                ne01: src0_shape.ne[1],
                ne02: src0_shape.ne[2],
                ne03: src0_shape.ne[3],
                nb00: src0_shape.nb[0],
                nb01: src0_shape.nb[1],
                nb02: src0_shape.nb[2],
                nb03: src0_shape.nb[3],
                ne10: src1_shape.ne[0],
                ne11: src1_shape.ne[1],
                ne12: src1_shape.ne[2],
                ne13: src1_shape.ne[3],
                nb10: src1_shape.nb[0],
                nb11: src1_shape.nb[1],
                nb12: src1_shape.nb[2],
                nb13: src1_shape.nb[3],
                ne0: src0_shape.ne[0],
                ne1: src0_shape.ne[1],
                ne2: src0_shape.ne[2],
                ne3: src0_shape.ne[3],
                nb0: src0_shape.nb[0],
                nb1: src0_shape.nb[1],
                nb2: src0_shape.nb[2],
                nb3: src0_shape.nb[3],
                offs: 0,
                o1: [0u64; 8],
            };

            if is_c4 {
                args.ne00 /= 4;
                args.ne10 /= 4;
                args.ne0 /= 4;
            }

            let (_command_buffer, encoder, encoder_handles) = self.begin_command_encoder()?;
            unsafe {
                let _: () = msg_send![encoder, setComputePipelineState: pipeline];
                let _: () = msg_send![
                    encoder,
                    setBytes: &args as *const KArgsBin as *const c_void
                    length: std::mem::size_of::<KArgsBin>() as u64
                    atIndex: 0u64
                ];
                let _: () =
                    msg_send![encoder, setBuffer: src0_id offset: 0u64 atIndex: 1u64];
                let _: () =
                    msg_send![encoder, setBuffer: src1_id offset: 0u64 atIndex: 2u64];
                let _: () =
                    msg_send![encoder, setBuffer: dst_id offset: 0u64 atIndex: 3u64];

                if is_rb {
                    let n = if is_c4 {
                        src0_shape.numel / 4
                    } else {
                        src0_shape.numel
                    };
                    let tgs = MTLSize {
                        width: n as u64,
                        height: 1,
                        depth: 1,
                    };
                    let tpg = MTLSize {
                        width: 1,
                        height: 1,
                        depth: 1,
                    };
                    let _: () = msg_send![
                        encoder,
                        dispatchThreadgroups: tgs
                        threadsPerThreadgroup: tpg
                    ];
                } else {
                    let nth_max =
                        std::cmp::min(256u64, Self::pipeline_max_threads(pipeline)).max(1u64);
                    let mut nth = 1u64;
                    while 2 * nth < args.ne0 as u64 && nth < nth_max {
                        nth *= 2;
                    }
                    let tgs = MTLSize {
                        width: src0_shape.ne[1] as u64,
                        height: src0_shape.ne[2] as u64,
                        depth: src0_shape.ne[3] as u64,
                    };
                    let tpg = MTLSize {
                        width: nth,
                        height: 1,
                        depth: 1,
                    };
                    let _: () = msg_send![
                        encoder,
                        dispatchThreadgroups: tgs
                        threadsPerThreadgroup: tpg
                    ];
                }
            }

            self.end_command_encoder(encoder_handles)
        }

        #[allow(clippy::too_many_arguments)]
        fn dispatch_norm_f32(
            &mut self,
            src0_id: ObjcId,
            src1_0_id: ObjcId,
            src1_1_id: ObjcId,
            dst_id: ObjcId,
            src0_shape: &Shape4,
            src1_0_shape: &Shape4,
            src1_1_shape: &Shape4,
            eps: f32,
            n_fuse: i32,
        ) -> Result<(), String> {
            if src0_shape.ne[0] <= 0 {
                return Err("norm ne0 must be positive".to_string());
            }

            let is_c4 = src0_shape.ne[0] % 4 == 0;
            let suffix = if is_c4 { "_4" } else { "" };
            let base = match n_fuse {
                1 => format!("kernel_norm_f32{}", suffix),
                2 => format!("kernel_norm_mul_f32{}", suffix),
                3 => format!("kernel_norm_mul_add_f32{}", suffix),
                _ => return Err(format!("unsupported norm fuse level: {}", n_fuse)),
            };

            let (pipeline, pipeline_smem, _nr0, _nr1, _nsg) =
                self.get_or_compile_cached_pipeline(base.clone(), &base, &[], 32 * 4, 0, 0, 0)?;

            let ne00_t = if is_c4 {
                src0_shape.ne[0] / 4
            } else {
                src0_shape.ne[0]
            };
            let args = KArgsNorm {
                ne00: src0_shape.ne[0],
                ne00_t,
                nb1: src0_shape.nb[1],
                nb2: src0_shape.nb[2],
                nb3: src0_shape.nb[3],
                eps,
                nef1: [src0_shape.ne[1], src1_0_shape.ne[1], src1_1_shape.ne[1]],
                nef2: [src0_shape.ne[2], src1_0_shape.ne[2], src1_1_shape.ne[2]],
                nef3: [src0_shape.ne[3], src1_0_shape.ne[3], src1_1_shape.ne[3]],
                nbf1: [src0_shape.nb[1], src1_0_shape.nb[1], src1_1_shape.nb[1]],
                nbf2: [src0_shape.nb[2], src1_0_shape.nb[2], src1_1_shape.nb[2]],
                nbf3: [src0_shape.nb[3], src1_0_shape.nb[3], src1_1_shape.nb[3]],
            };

            let mut nth = 32u64;
            let nth_max = Self::pipeline_max_threads(pipeline).max(1u64);
            while nth < args.ne00_t as u64 && nth < nth_max {
                nth *= 2;
            }
            nth = std::cmp::min(nth, nth_max);
            nth = std::cmp::min(nth, args.ne00_t.max(1) as u64);

            let (_command_buffer, encoder, encoder_handles) = self.begin_command_encoder()?;
            unsafe {
                let _: () = msg_send![encoder, setComputePipelineState: pipeline];
                let _: () = msg_send![
                    encoder,
                    setBytes: &args as *const KArgsNorm as *const c_void
                    length: std::mem::size_of::<KArgsNorm>() as u64
                    atIndex: 0u64
                ];
                let _: () =
                    msg_send![encoder, setBuffer: src0_id offset: 0u64 atIndex: 1u64];
                let _: () =
                    msg_send![encoder, setBuffer: src1_0_id offset: 0u64 atIndex: 2u64];
                let _: () =
                    msg_send![encoder, setBuffer: src1_1_id offset: 0u64 atIndex: 3u64];
                let _: () =
                    msg_send![encoder, setBuffer: dst_id offset: 0u64 atIndex: 4u64];
                let _: () = msg_send![
                    encoder,
                    setThreadgroupMemoryLength: pipeline_smem as u64
                    atIndex: 0u64
                ];

                let tgs = MTLSize {
                    width: src0_shape.ne[1] as u64,
                    height: src0_shape.ne[2] as u64,
                    depth: src0_shape.ne[3] as u64,
                };
                let tpg = MTLSize {
                    width: nth,
                    height: 1,
                    depth: 1,
                };
                let _: () = msg_send![
                    encoder,
                    dispatchThreadgroups: tgs
                    threadsPerThreadgroup: tpg
                ];
            }

            self.end_command_encoder(encoder_handles)
        }

        fn dispatch_im2col_1d_f32(
            &mut self,
            src_id: ObjcId,
            dst_id: ObjcId,
            ic: usize,
            iw: usize,
            kw: usize,
            stride: usize,
            pad: usize,
            ow: usize,
        ) -> Result<(), String> {
            let base = "kernel_im2col_f32";
            let (pipeline, _smem, _nr0, _nr1, _nsg) =
                self.get_or_compile_cached_pipeline(base.to_string(), base, &[], 0, 0, 0, 0)?;

            let ic_i32 = i32::try_from(ic).map_err(|_| format!("ic too large: {}", ic))?;
            let iw_i32 = i32::try_from(iw).map_err(|_| format!("iw too large: {}", iw))?;
            let kw_i32 = i32::try_from(kw).map_err(|_| format!("kw too large: {}", kw))?;
            let ow_i32 = i32::try_from(ow).map_err(|_| format!("ow too large: {}", ow))?;
            let stride_i32 =
                i32::try_from(stride).map_err(|_| format!("stride too large: {}", stride))?;
            let pad_i32 = i32::try_from(pad).map_err(|_| format!("pad too large: {}", pad))?;

            let chw = ic
                .checked_mul(kw)
                .ok_or_else(|| "overflow computing im2col CHW".to_string())?;
            let ofs0 = ic
                .checked_mul(iw)
                .ok_or_else(|| "overflow computing im2col ofs0".to_string())?;

            let args = KArgsIm2Col {
                ofs0: ofs0 as u64,
                ofs1: iw as u64,
                iw: iw_i32,
                ih: 1,
                chw: i32::try_from(chw).map_err(|_| format!("CHW too large: {}", chw))?,
                s0: stride_i32,
                s1: 1,
                p0: pad_i32,
                p1: 0,
                d0: 1,
                d1: 1,
                n: 1,
                kh: 1,
                kw: kw_i32,
                khw: kw_i32,
            };

            let max_threads = Self::pipeline_max_threads(pipeline);
            let khkw = kw_i32 as u64;
            if khkw == 0 || khkw > max_threads {
                return Err(format!(
                    "invalid im2col thread shape: kh*kw={} max={}",
                    khkw, max_threads
                ));
            }
            let ntptg0 = (max_threads / khkw).min(1).max(1);

            let (_command_buffer, encoder, encoder_handles) = self.begin_command_encoder()?;
            unsafe {
                let _: () = msg_send![encoder, setComputePipelineState: pipeline];
                let _: () = msg_send![
                    encoder,
                    setBytes: &args as *const KArgsIm2Col as *const c_void
                    length: std::mem::size_of::<KArgsIm2Col>() as u64
                    atIndex: 0u64
                ];
                let _: () =
                    msg_send![encoder, setBuffer: src_id offset: 0u64 atIndex: 1u64];
                let _: () =
                    msg_send![encoder, setBuffer: dst_id offset: 0u64 atIndex: 2u64];

                let tgs = MTLSize {
                    width: ic_i32 as u64,
                    height: 1,
                    depth: ow_i32 as u64,
                };
                let tpg = MTLSize {
                    width: ntptg0,
                    height: 1,
                    depth: kw_i32 as u64,
                };
                let _: () = msg_send![
                    encoder,
                    dispatchThreadgroups: tgs
                    threadsPerThreadgroup: tpg
                ];
            }

            self.end_command_encoder(encoder_handles)
        }

        #[allow(clippy::too_many_arguments)]
        fn dispatch_flash_attn_ext_pad(
            &mut self,
            k_id: ObjcId,
            v_id: ObjcId,
            mask_id: ObjcId,
            pad_id: ObjcId,
            has_mask: bool,
            ncpsg: i32,
            ne11: i32,
            ne_12_2: i32,
            ne_12_3: i32,
            nb11: u64,
            nb12: u64,
            nb13: u64,
            nb21: u64,
            nb22: u64,
            nb23: u64,
            ne31: i32,
            ne32: i32,
            ne33: i32,
            nb31: u64,
            nb32: u64,
            nb33: u64,
        ) -> Result<(), String> {
            let base = "kernel_flash_attn_ext_pad";
            let name = format!("{}_mask={}_ncpsg={}", base, has_mask as i32, ncpsg);
            let constants = [
                FunctionConstant {
                    idx: FC_FLASH_ATTN_EXT_PAD + 0,
                    value: FunctionConstantValue::Bool(has_mask),
                },
                FunctionConstant {
                    idx: FC_FLASH_ATTN_EXT_PAD + 25,
                    value: FunctionConstantValue::Int32(ncpsg),
                },
            ];
            let (pipeline, _smem, _nr0, _nr1, _nsg) =
                self.get_or_compile_cached_pipeline(name, base, &constants, 0, 0, 0, 0)?;

            let args = KArgsFlashAttnExtPad {
                ne11,
                ne_12_2,
                ne_12_3,
                nb11,
                nb12,
                nb13,
                nb21,
                nb22,
                nb23,
                ne31,
                ne32,
                ne33,
                nb31,
                nb32,
                nb33,
            };

            let (_command_buffer, encoder, encoder_handles) = self.begin_command_encoder()?;
            unsafe {
                let _: () = msg_send![encoder, setComputePipelineState: pipeline];
                let _: () = msg_send![
                    encoder,
                    setBytes: &args as *const KArgsFlashAttnExtPad as *const c_void
                    length: std::mem::size_of::<KArgsFlashAttnExtPad>() as u64
                    atIndex: 0u64
                ];
                let _: () = msg_send![encoder, setBuffer: k_id offset: 0u64 atIndex: 1u64];
                let _: () = msg_send![encoder, setBuffer: v_id offset: 0u64 atIndex: 2u64];
                let _: () =
                    msg_send![encoder, setBuffer: mask_id offset: 0u64 atIndex: 3u64];
                let _: () =
                    msg_send![encoder, setBuffer: pad_id offset: 0u64 atIndex: 4u64];

                let tgs = MTLSize {
                    width: ncpsg as u64,
                    height: ne_12_2.max(ne32) as u64,
                    depth: ne_12_3.max(ne33) as u64,
                };
                let tpg = MTLSize {
                    width: 32,
                    height: 1,
                    depth: 1,
                };
                let _: () = msg_send![
                    encoder,
                    dispatchThreadgroups: tgs
                    threadsPerThreadgroup: tpg
                ];
            }

            self.end_command_encoder(encoder_handles)
        }

        #[allow(clippy::too_many_arguments)]
        fn dispatch_flash_attn_ext_blk(
            &mut self,
            mask_id: ObjcId,
            blk_id: ObjcId,
            n_q: usize,
            n_kv: usize,
            ne31: i32,
            ne32: i32,
            ne33: i32,
            nb31: u64,
            nb32: u64,
            nb33: u64,
            nqptg: i32,
            ncpsg: i32,
        ) -> Result<(), String> {
            let base = "kernel_flash_attn_ext_blk";
            let name = format!("{}_nqptg={}_ncpsg={}", base, nqptg, ncpsg);
            let constants = [
                FunctionConstant {
                    idx: FC_FLASH_ATTN_EXT_BLK + 24,
                    value: FunctionConstantValue::Int32(nqptg),
                },
                FunctionConstant {
                    idx: FC_FLASH_ATTN_EXT_BLK + 25,
                    value: FunctionConstantValue::Int32(ncpsg),
                },
            ];
            let (pipeline, _smem, _nr0, _nr1, _nsg) =
                self.get_or_compile_cached_pipeline(name, base, &constants, 0, 0, 0, 0)?;

            let ne01 = i32::try_from(n_q).map_err(|_| format!("n_q too large: {}", n_q))?;
            let ne30 = i32::try_from(n_kv).map_err(|_| format!("n_kv too large: {}", n_kv))?;
            let args = KArgsFlashAttnExtBlk {
                ne01,
                ne30,
                ne31,
                ne32,
                ne33,
                nb31,
                nb32,
                nb33,
            };

            let (_command_buffer, encoder, encoder_handles) = self.begin_command_encoder()?;
            unsafe {
                let _: () = msg_send![encoder, setComputePipelineState: pipeline];
                let _: () = msg_send![
                    encoder,
                    setBytes: &args as *const KArgsFlashAttnExtBlk as *const c_void
                    length: std::mem::size_of::<KArgsFlashAttnExtBlk>() as u64
                    atIndex: 0u64
                ];
                let _: () =
                    msg_send![encoder, setBuffer: mask_id offset: 0u64 atIndex: 1u64];
                let _: () =
                    msg_send![encoder, setBuffer: blk_id offset: 0u64 atIndex: 2u64];

                let nblk1 = ((ne01 + nqptg - 1) / nqptg) as u64;
                let nblk0 = ((ne30 + ncpsg - 1) / ncpsg) as u64;
                let tgs = MTLSize {
                    width: nblk0,
                    height: nblk1,
                    depth: (ne32 * ne33) as u64,
                };
                let tpg = MTLSize {
                    width: 32,
                    height: 1,
                    depth: 1,
                };
                let _: () = msg_send![
                    encoder,
                    dispatchThreadgroups: tgs
                    threadsPerThreadgroup: tpg
                ];
            }

            self.end_command_encoder(encoder_handles)
        }

        fn dispatch_flash_attn_ext_f32(
            &mut self,
            q_id: ObjcId,
            k_id: ObjcId,
            v_id: ObjcId,
            mask_id: ObjcId,
            sinks_id: ObjcId,
            pad_id: ObjcId,
            blk_id: ObjcId,
            dst_id: ObjcId,
            n_q: usize,
            n_kv: usize,
            n_head: usize,
            d: usize,
            scale: f32,
            has_mask: bool,
            has_sinks: bool,
            max_bias: f32,
            logit_softcap: f32,
        ) -> Result<(), String> {
            static LOG_ONCE: OnceLock<()> = OnceLock::new();
            if LOG_ONCE.set(()).is_ok() {
                eprintln!("[voice][metal] flash_attn dispatch: flash_attn_ext_f32");
            }

            if !flash_attn_supported_head_dim(d) {
                return Err(format!(
                    "unsupported flash-attn head dim for f32 kernel: {}",
                    d
                ));
            }
            if d % 4 != 0 {
                return Err(format!(
                    "flash-attn requires head dim divisible by 4 (float4 store), got {}",
                    d
                ));
            }

            let nsg = if d >= 512 { 8 } else { 4 };
            let nqptg = OP_FLASH_ATTN_EXT_NQPSG;
            let ncpsg = OP_FLASH_ATTN_EXT_NCPSG;
            let has_kvpad = n_kv % (ncpsg as usize) != 0;
            let has_bias = max_bias != 0.0;
            let has_scap = logit_softcap != 0.0;

            let ne01 = i32::try_from(n_q).map_err(|_| format!("n_q too large: {}", n_q))?;
            let ne11 = i32::try_from(n_kv).map_err(|_| format!("n_kv too large: {}", n_kv))?;
            let ne02 =
                i32::try_from(n_head).map_err(|_| format!("n_head too large: {}", n_head))?;
            let n_state = n_head
                .checked_mul(d)
                .ok_or_else(|| "overflow computing flash n_state".to_string())?;
            let n_state_i32 =
                i32::try_from(n_state).map_err(|_| format!("n_state too large: {}", n_state))?;

            let nb01 = (n_state as u64)
                .checked_mul(4)
                .ok_or_else(|| "overflow computing flash nb01".to_string())?;
            let nb02 = (d as u64)
                .checked_mul(4)
                .ok_or_else(|| "overflow computing flash nb02".to_string())?;
            let nb03 = nb01
                .checked_mul(n_q as u64)
                .ok_or_else(|| "overflow computing flash nb03".to_string())?;

            let nb11 = (n_state as u64)
                .checked_mul(4)
                .ok_or_else(|| "overflow computing flash nb11".to_string())?;
            let nb12 = (d as u64)
                .checked_mul(4)
                .ok_or_else(|| "overflow computing flash nb12".to_string())?;
            let nb13 = nb11
                .checked_mul(n_kv as u64)
                .ok_or_else(|| "overflow computing flash nb13".to_string())?;

            let nb21 = nb11;
            let nb22 = nb12;
            let nb23 = nb13;

            let ne31 = ne01;
            let ne32 = 1i32;
            let ne33 = 1i32;
            let nb31 = (n_kv as u64)
                .checked_mul(2)
                .ok_or_else(|| "overflow computing flash nb31".to_string())?;
            let nb32 = nb31
                .checked_mul(n_q as u64)
                .ok_or_else(|| "overflow computing flash nb32".to_string())?;
            let nb33 = nb32;

            let n_head_log2 = if n_head <= 1 {
                1i32
            } else {
                let p = (usize::BITS - 1) - (n_head as u32).leading_zeros();
                (1u32 << p) as i32
            };
            let m0 = (2.0f32).powf(-(max_bias) / (n_head_log2 as f32));
            let m1 = (2.0f32).powf(-(max_bias / 2.0) / (n_head_log2 as f32));
            let scale_k = if has_scap {
                scale / logit_softcap
            } else {
                scale
            };

            if has_kvpad {
                self.dispatch_flash_attn_ext_pad(
                    k_id,
                    v_id,
                    mask_id,
                    pad_id,
                    has_mask,
                    ncpsg,
                    ne11,
                    ne02,
                    1,
                    nb11,
                    nb12,
                    nb13,
                    nb21,
                    nb22,
                    nb23,
                    ne31,
                    ne32,
                    ne33,
                    nb31,
                    nb32,
                    nb33,
                )?;
            }
            if has_mask {
                self.dispatch_flash_attn_ext_blk(
                    mask_id, blk_id, n_q, n_kv, ne31, ne32, ne33, nb31, nb32, nb33, nqptg, ncpsg,
                )?;
            }

            let base = format!("kernel_flash_attn_ext_f32_dk{}_dv{}", d, d);
            let name = format!(
                "{}_mask={}_sinks={}_bias={}_scap={}_kvpad={}_bcm=0_ns10={}_ns20={}_nsg={}",
                base,
                has_mask as i32,
                has_sinks as i32,
                has_bias as i32,
                has_scap as i32,
                has_kvpad as i32,
                n_state_i32,
                n_state_i32,
                nsg
            );
            let constants = [
                FunctionConstant {
                    idx: FC_FLASH_ATTN_EXT + 0,
                    value: FunctionConstantValue::Bool(has_mask),
                },
                FunctionConstant {
                    idx: FC_FLASH_ATTN_EXT + 1,
                    value: FunctionConstantValue::Bool(has_sinks),
                },
                FunctionConstant {
                    idx: FC_FLASH_ATTN_EXT + 2,
                    value: FunctionConstantValue::Bool(has_bias),
                },
                FunctionConstant {
                    idx: FC_FLASH_ATTN_EXT + 3,
                    value: FunctionConstantValue::Bool(has_scap),
                },
                FunctionConstant {
                    idx: FC_FLASH_ATTN_EXT + 4,
                    value: FunctionConstantValue::Bool(has_kvpad),
                },
                FunctionConstant {
                    idx: FC_FLASH_ATTN_EXT + 10,
                    value: FunctionConstantValue::Bool(false),
                },
                FunctionConstant {
                    idx: FC_FLASH_ATTN_EXT + 20,
                    value: FunctionConstantValue::Int32(n_state_i32),
                },
                FunctionConstant {
                    idx: FC_FLASH_ATTN_EXT + 21,
                    value: FunctionConstantValue::Int32(n_state_i32),
                },
                FunctionConstant {
                    idx: FC_FLASH_ATTN_EXT + 22,
                    value: FunctionConstantValue::Int32(nsg),
                },
            ];

            let smem = flash_attn_smem_bytes(d, d, nsg);
            let (pipeline, pipeline_smem, _nr0, _nr1, _pnsg) =
                self.get_or_compile_cached_pipeline(name, &base, &constants, smem, 0, 0, nsg)?;

            let args = KArgsFlashAttnExt {
                ne01,
                ne02,
                ne03: 1,
                nb01,
                nb02,
                nb03,
                ne11,
                ne_12_2: ne02,
                ne_12_3: 1,
                ns10: n_state_i32,
                nb11,
                nb12,
                nb13,
                ns20: n_state_i32,
                nb21,
                nb22,
                nb23,
                ne31,
                ne32,
                ne33,
                nb31,
                nb32,
                nb33,
                ne1: ne02,
                ne2: ne01,
                ne3: 1,
                scale: scale_k,
                max_bias,
                m0,
                m1,
                n_head_log2,
                logit_softcap,
            };

            let (_command_buffer, encoder, encoder_handles) = self.begin_command_encoder()?;
            unsafe {
                let _: () = msg_send![encoder, setComputePipelineState: pipeline];
                let _: () = msg_send![
                    encoder,
                    setBytes: &args as *const KArgsFlashAttnExt as *const c_void
                    length: std::mem::size_of::<KArgsFlashAttnExt>() as u64
                    atIndex: 0u64
                ];
                let _: () = msg_send![encoder, setBuffer: q_id offset: 0u64 atIndex: 1u64];
                let _: () = msg_send![encoder, setBuffer: k_id offset: 0u64 atIndex: 2u64];
                let _: () = msg_send![encoder, setBuffer: v_id offset: 0u64 atIndex: 3u64];
                let _: () =
                    msg_send![encoder, setBuffer: mask_id offset: 0u64 atIndex: 4u64];
                let _: () =
                    msg_send![encoder, setBuffer: sinks_id offset: 0u64 atIndex: 5u64];
                let _: () =
                    msg_send![encoder, setBuffer: pad_id offset: 0u64 atIndex: 6u64];
                let _: () =
                    msg_send![encoder, setBuffer: blk_id offset: 0u64 atIndex: 7u64];
                let _: () =
                    msg_send![encoder, setBuffer: dst_id offset: 0u64 atIndex: 8u64];
                let _: () = msg_send![
                    encoder,
                    setThreadgroupMemoryLength: pipeline_smem as u64
                    atIndex: 0u64
                ];

                let tgs = MTLSize {
                    width: ((n_q as i32 + nqptg - 1) / nqptg) as u64,
                    height: n_head as u64,
                    depth: 1,
                };
                let tpg = MTLSize {
                    width: 32,
                    height: nsg as u64,
                    depth: 1,
                };
                let _: () = msg_send![
                    encoder,
                    dispatchThreadgroups: tgs
                    threadsPerThreadgroup: tpg
                ];
            }

            self.end_command_encoder(encoder_handles)
        }

        fn dispatch_flash_attn_ext_vec_reduce_f32(
            &mut self,
            tmp_id: ObjcId,
            dst_id: ObjcId,
            nrows: usize,
            d: usize,
            nwg: i32,
        ) -> Result<(), String> {
            let base = "kernel_flash_attn_ext_vec_reduce";
            let name = format!("{}_dv={}_nwg={}", base, d, nwg);
            let d_i32 = i32::try_from(d).map_err(|_| format!("d too large for vec reduce: {}", d))?;
            let constants = [
                FunctionConstant {
                    idx: FC_FLASH_ATTN_EXT_VEC_REDUCE + 0,
                    value: FunctionConstantValue::Int32(d_i32),
                },
                FunctionConstant {
                    idx: FC_FLASH_ATTN_EXT_VEC_REDUCE + 1,
                    value: FunctionConstantValue::Int32(nwg),
                },
            ];
            let (pipeline, _smem, _nr0, _nr1, _pnsg) =
                self.get_or_compile_cached_pipeline(name, base, &constants, 0, 0, 0, 0)?;

            let nrows_i32 = i32::try_from(nrows)
                .map_err(|_| format!("nrows too large for vec reduce: {}", nrows))?;
            let args = KArgsFlashAttnExtVecReduce { nrows: nrows_i32 };

            let tpg_width = (32i32)
                .checked_mul(nwg)
                .ok_or_else(|| "overflow computing vec reduce tpg width".to_string())?;
            let max_threads = Self::pipeline_max_threads(pipeline);
            if tpg_width as u64 > max_threads {
                return Err(format!(
                    "vec reduce threadsPerThreadgroup={} exceeds max={}",
                    tpg_width, max_threads
                ));
            }

            let (_command_buffer, encoder, encoder_handles) = self.begin_command_encoder()?;
            unsafe {
                let _: () = msg_send![encoder, setComputePipelineState: pipeline];
                let _: () = msg_send![
                    encoder,
                    setBytes: &args as *const KArgsFlashAttnExtVecReduce as *const c_void
                    length: std::mem::size_of::<KArgsFlashAttnExtVecReduce>() as u64
                    atIndex: 0u64
                ];
                let _: () = msg_send![encoder, setBuffer: tmp_id offset: 0u64 atIndex: 1u64];
                let _: () = msg_send![encoder, setBuffer: dst_id offset: 0u64 atIndex: 2u64];

                let tgs = MTLSize {
                    width: nrows as u64,
                    height: 1,
                    depth: 1,
                };
                let tpg = MTLSize {
                    width: tpg_width as u64,
                    height: 1,
                    depth: 1,
                };
                let _: () = msg_send![
                    encoder,
                    dispatchThreadgroups: tgs
                    threadsPerThreadgroup: tpg
                ];
            }

            self.end_command_encoder(encoder_handles)
        }

        fn dispatch_flash_attn_ext_vec_f32(
            &mut self,
            q_id: ObjcId,
            k_id: ObjcId,
            v_id: ObjcId,
            mask_id: ObjcId,
            sinks_id: ObjcId,
            pad_id: ObjcId,
            tmp_id: Option<ObjcId>,
            dst_id: ObjcId,
            n_q: usize,
            n_kv: usize,
            n_head: usize,
            d: usize,
            scale: f32,
            has_mask: bool,
            has_sinks: bool,
            max_bias: f32,
            logit_softcap: f32,
        ) -> Result<(), String> {
            static LOG_ONCE: OnceLock<()> = OnceLock::new();
            if LOG_ONCE.set(()).is_ok() {
                eprintln!("[voice][metal] flash_attn dispatch: flash_attn_ext_vec_f32");
            }

            if d % 32 != 0 {
                return Err(format!(
                    "flash-attn vec requires head dim divisible by 32, got {}",
                    d
                ));
            }
            if !flash_attn_supported_head_dim(d) {
                return Err(format!(
                    "unsupported flash-attn vec head dim for f32 kernel: {}",
                    d
                ));
            }

            let nqptg = OP_FLASH_ATTN_EXT_VEC_NQPSG;
            let ncpsg = OP_FLASH_ATTN_EXT_VEC_NCPSG;
            let nhptg = 1i32;
            let has_kvpad = n_kv % (ncpsg as usize) != 0;
            let has_bias = max_bias != 0.0;
            let has_scap = logit_softcap != 0.0;

            let nwg = 32i32;
            let mut nsg = 1i32;
            while (2i64)
                .saturating_mul(nwg as i64)
                .saturating_mul(nsg as i64)
                .saturating_mul(ncpsg as i64)
                < n_kv as i64
                && nsg < 4
            {
                nsg *= 2;
            }

            let ne01 = i32::try_from(n_q).map_err(|_| format!("n_q too large: {}", n_q))?;
            let ne11 = i32::try_from(n_kv).map_err(|_| format!("n_kv too large: {}", n_kv))?;
            let ne02 =
                i32::try_from(n_head).map_err(|_| format!("n_head too large: {}", n_head))?;
            let n_state = n_head
                .checked_mul(d)
                .ok_or_else(|| "overflow computing flash n_state".to_string())?;
            let n_state_i32 =
                i32::try_from(n_state).map_err(|_| format!("n_state too large: {}", n_state))?;

            let nb01 = (n_state as u64)
                .checked_mul(4)
                .ok_or_else(|| "overflow computing flash nb01".to_string())?;
            let nb02 = (d as u64)
                .checked_mul(4)
                .ok_or_else(|| "overflow computing flash nb02".to_string())?;
            let nb03 = nb01
                .checked_mul(n_q as u64)
                .ok_or_else(|| "overflow computing flash nb03".to_string())?;

            let nb11 = (n_state as u64)
                .checked_mul(4)
                .ok_or_else(|| "overflow computing flash nb11".to_string())?;
            let nb12 = (d as u64)
                .checked_mul(4)
                .ok_or_else(|| "overflow computing flash nb12".to_string())?;
            let nb13 = nb11
                .checked_mul(n_kv as u64)
                .ok_or_else(|| "overflow computing flash nb13".to_string())?;

            let nb21 = nb11;
            let nb22 = nb12;
            let nb23 = nb13;

            let ne31 = ne01;
            let ne32 = 1i32;
            let ne33 = 1i32;
            let nb31 = (n_kv as u64)
                .checked_mul(2)
                .ok_or_else(|| "overflow computing flash nb31".to_string())?;
            let nb32 = nb31
                .checked_mul(n_q as u64)
                .ok_or_else(|| "overflow computing flash nb32".to_string())?;
            let nb33 = nb32;

            let n_head_log2 = if n_head <= 1 {
                1i32
            } else {
                let p = (usize::BITS - 1) - (n_head as u32).leading_zeros();
                (1u32 << p) as i32
            };
            let m0 = (2.0f32).powf(-(max_bias) / (n_head_log2 as f32));
            let m1 = (2.0f32).powf(-(max_bias / 2.0) / (n_head_log2 as f32));
            let scale_k = if has_scap {
                scale / logit_softcap
            } else {
                scale
            };

            if has_kvpad {
                self.dispatch_flash_attn_ext_pad(
                    k_id,
                    v_id,
                    mask_id,
                    pad_id,
                    has_mask,
                    ncpsg,
                    ne11,
                    ne02,
                    1,
                    nb11,
                    nb12,
                    nb13,
                    nb21,
                    nb22,
                    nb23,
                    ne31,
                    ne32,
                    ne33,
                    nb31,
                    nb32,
                    nb33,
                )?;
            }

            let base = format!("kernel_flash_attn_ext_vec_f32_dk{}_dv{}", d, d);
            let name = format!(
                "{}_mask={}_sink={}_bias={}_scap={}_kvpad={}_ns10={}_ns20={}_nsg={}_nwg={}",
                base,
                has_mask as i32,
                has_sinks as i32,
                has_bias as i32,
                has_scap as i32,
                has_kvpad as i32,
                n_state_i32,
                n_state_i32,
                nsg,
                nwg
            );
            let constants = [
                FunctionConstant {
                    idx: FC_FLASH_ATTN_EXT_VEC + 0,
                    value: FunctionConstantValue::Bool(has_mask),
                },
                FunctionConstant {
                    idx: FC_FLASH_ATTN_EXT_VEC + 1,
                    value: FunctionConstantValue::Bool(has_sinks),
                },
                FunctionConstant {
                    idx: FC_FLASH_ATTN_EXT_VEC + 2,
                    value: FunctionConstantValue::Bool(has_bias),
                },
                FunctionConstant {
                    idx: FC_FLASH_ATTN_EXT_VEC + 3,
                    value: FunctionConstantValue::Bool(has_scap),
                },
                FunctionConstant {
                    idx: FC_FLASH_ATTN_EXT_VEC + 4,
                    value: FunctionConstantValue::Bool(has_kvpad),
                },
                FunctionConstant {
                    idx: FC_FLASH_ATTN_EXT_VEC + 20,
                    value: FunctionConstantValue::Int32(n_state_i32),
                },
                FunctionConstant {
                    idx: FC_FLASH_ATTN_EXT_VEC + 21,
                    value: FunctionConstantValue::Int32(n_state_i32),
                },
                FunctionConstant {
                    idx: FC_FLASH_ATTN_EXT_VEC + 22,
                    value: FunctionConstantValue::Int32(nsg),
                },
                FunctionConstant {
                    idx: FC_FLASH_ATTN_EXT_VEC + 23,
                    value: FunctionConstantValue::Int32(nwg),
                },
            ];

            let smem = flash_attn_vec_smem_bytes(d, d, nsg);
            let (pipeline, pipeline_smem, _nr0, _nr1, _pnsg) =
                self.get_or_compile_cached_pipeline(name, &base, &constants, smem, 0, 0, nsg)?;
            let max_threads = Self::pipeline_max_threads(pipeline);
            let thread_width = (32i32)
                .checked_mul(nsg)
                .ok_or_else(|| "overflow computing vec thread width".to_string())?;
            if thread_width as u64 > max_threads {
                return Err(format!(
                    "flash-attn vec threadsPerThreadgroup={} exceeds max={}",
                    thread_width, max_threads
                ));
            }

            let args = KArgsFlashAttnExtVec {
                ne01,
                ne02,
                ne03: 1,
                nb01,
                nb02,
                nb03,
                ne11,
                ne_12_2: ne02,
                ne_12_3: 1,
                ns10: n_state_i32,
                nb11,
                nb12,
                nb13,
                ns20: n_state_i32,
                nb21,
                nb22,
                nb23,
                ne31,
                ne32,
                ne33,
                nb31,
                nb32,
                nb33,
                ne1: ne02,
                ne2: ne01,
                ne3: 1,
                scale: scale_k,
                max_bias,
                m0,
                m1,
                n_head_log2,
                logit_softcap,
            };

            let nrows = n_q
                .checked_mul(n_head)
                .ok_or_else(|| "overflow computing flash vec nrows".to_string())?;

            let (_command_buffer, encoder, encoder_handles) = self.begin_command_encoder()?;
            unsafe {
                let _: () = msg_send![encoder, setComputePipelineState: pipeline];
                let _: () = msg_send![
                    encoder,
                    setBytes: &args as *const KArgsFlashAttnExtVec as *const c_void
                    length: std::mem::size_of::<KArgsFlashAttnExtVec>() as u64
                    atIndex: 0u64
                ];
                let _: () = msg_send![encoder, setBuffer: q_id offset: 0u64 atIndex: 1u64];
                let _: () = msg_send![encoder, setBuffer: k_id offset: 0u64 atIndex: 2u64];
                let _: () = msg_send![encoder, setBuffer: v_id offset: 0u64 atIndex: 3u64];
                let _: () =
                    msg_send![encoder, setBuffer: mask_id offset: 0u64 atIndex: 4u64];
                let _: () =
                    msg_send![encoder, setBuffer: sinks_id offset: 0u64 atIndex: 5u64];
                let _: () = msg_send![encoder, setBuffer: pad_id offset: 0u64 atIndex: 6u64];
                if nwg == 1 {
                    let _: () =
                        msg_send![encoder, setBuffer: dst_id offset: 0u64 atIndex: 7u64];
                } else {
                    let tmp_id = tmp_id.ok_or_else(|| {
                        "flash-attn vec requires tmp buffer when nwg > 1".to_string()
                    })?;
                    let _: () =
                        msg_send![encoder, setBuffer: tmp_id offset: 0u64 atIndex: 7u64];
                }

                let _: () = msg_send![
                    encoder,
                    setThreadgroupMemoryLength: pipeline_smem as u64
                    atIndex: 0u64
                ];

                let tgs = MTLSize {
                    width: ((n_q as i32 + nqptg - 1) / nqptg) as u64,
                    height: ((n_head as i32 + nhptg - 1) / nhptg) as u64,
                    depth: nwg as u64,
                };
                let tpg = MTLSize {
                    width: 32,
                    height: nsg as u64,
                    depth: 1,
                };
                let _: () = msg_send![
                    encoder,
                    dispatchThreadgroups: tgs
                    threadsPerThreadgroup: tpg
                ];
            }

            self.end_command_encoder(encoder_handles)?;

            if nwg > 1 {
                let tmp_id = tmp_id.ok_or_else(|| {
                    "flash-attn vec requires tmp buffer when nwg > 1".to_string()
                })?;
                self.dispatch_flash_attn_ext_vec_reduce_f32(tmp_id, dst_id, nrows, d, nwg)?;
            }

            Ok(())
        }

        fn flash_attn_f32_packed(
            &mut self,
            q: &[f32],
            k: &[f32],
            v: &[f32],
            n_q: usize,
            n_kv: usize,
            n_head: usize,
            d: usize,
            scale: f32,
        ) -> Result<Vec<f32>, String> {
            if n_q == 0 || n_kv == 0 || n_head == 0 || d == 0 {
                return Ok(Vec::new());
            }

            let q_need = n_q
                .checked_mul(n_head)
                .and_then(|v| v.checked_mul(d))
                .ok_or_else(|| "overflow computing flash q size".to_string())?;
            if q.len() != q_need {
                return Err(format!(
                    "flash q len mismatch: got {}, expected {}",
                    q.len(),
                    q_need
                ));
            }

            let kv_need = n_kv
                .checked_mul(n_head)
                .and_then(|v| v.checked_mul(d))
                .ok_or_else(|| "overflow computing flash kv size".to_string())?;
            if k.len() != kv_need {
                return Err(format!(
                    "flash k len mismatch: got {}, expected {}",
                    k.len(),
                    kv_need
                ));
            }
            if v.len() != kv_need {
                return Err(format!(
                    "flash v len mismatch: got {}, expected {}",
                    v.len(),
                    kv_need
                ));
            }

            let out_elems = n_q
                .checked_mul(n_head)
                .and_then(|v| v.checked_mul(d))
                .ok_or_else(|| "overflow computing flash output size".to_string())?;
            let out_bytes = out_elems
                .checked_mul(std::mem::size_of::<f32>())
                .ok_or_else(|| "overflow computing flash output bytes".to_string())?;

            let q_bytes = unsafe {
                std::slice::from_raw_parts(
                    q.as_ptr() as *const u8,
                    q.len() * std::mem::size_of::<f32>(),
                )
            };
            let k_bytes = unsafe {
                std::slice::from_raw_parts(
                    k.as_ptr() as *const u8,
                    k.len() * std::mem::size_of::<f32>(),
                )
            };
            let v_bytes = unsafe {
                std::slice::from_raw_parts(
                    v.as_ptr() as *const u8,
                    v.len() * std::mem::size_of::<f32>(),
                )
            };

            let q_buf = self.new_buffer_with_bytes(q_bytes)?;
            let k_buf = self.new_buffer_with_bytes(k_bytes)?;
            let v_buf = self.new_buffer_with_bytes(v_bytes)?;
            let dst_id = self.get_or_create_scratch_buffer(SCRATCH_FLASH_OUT, out_bytes)?;

            let params = FlashAttnExtParams::default();
            let use_vec = flash_attn_use_vec(n_q, d);
            let pad_bytes =
                flash_attn_ext_extra_pad_bytes(n_q, n_kv, n_head, d, params.has_mask, use_vec)?;
            let pad_id = self.get_or_create_scratch_buffer(SCRATCH_FLASH_PAD, pad_bytes)?;

            if use_vec {
                let tmp_bytes = flash_attn_ext_extra_tmp_bytes(n_q, n_head, d, 32)?;
                let tmp_id = self.get_or_create_scratch_buffer(SCRATCH_FLASH_TMP, tmp_bytes)?;

                self.dispatch_flash_attn_ext_vec_f32(
                    q_buf.as_id(),
                    k_buf.as_id(),
                    v_buf.as_id(),
                    q_buf.as_id(), // unused when has_mask=false
                    q_buf.as_id(), // unused when has_sinks=false
                    pad_id,
                    Some(tmp_id),
                    dst_id,
                    n_q,
                    n_kv,
                    n_head,
                    d,
                    scale,
                    params.has_mask,
                    params.has_sinks,
                    params.max_bias,
                    params.logit_softcap,
                )?;
            } else {
                let blk_bytes = flash_attn_ext_extra_blk_bytes(n_q, n_kv, params.has_mask, false)?;
                let blk_id = self.get_or_create_scratch_buffer(SCRATCH_FLASH_BLK, blk_bytes)?;

                self.dispatch_flash_attn_ext_f32(
                    q_buf.as_id(),
                    k_buf.as_id(),
                    v_buf.as_id(),
                    q_buf.as_id(), // unused when has_mask=false
                    q_buf.as_id(), // unused when has_sinks=false
                    pad_id,
                    blk_id,
                    dst_id,
                    n_q,
                    n_kv,
                    n_head,
                    d,
                    scale,
                    params.has_mask,
                    params.has_sinks,
                    params.max_bias,
                    params.logit_softcap,
                )?;
            }

            self.read_f32_buffer(dst_id, out_elems)
        }

        #[allow(clippy::too_many_arguments)]
        fn flash_attn_f32_self_kv_cache(
            &mut self,
            layer: usize,
            q: &[f32],
            k_all: &[f32],
            v_all: &[f32],
            n_kv: usize,
            n_head: usize,
            d: usize,
            scale: f32,
        ) -> Result<Vec<f32>, String> {
            if n_kv == 0 || n_head == 0 || d == 0 {
                return Ok(Vec::new());
            }
            let n_state = n_head
                .checked_mul(d)
                .ok_or_else(|| "overflow computing n_state".to_string())?;
            let kv_need = n_kv
                .checked_mul(n_state)
                .ok_or_else(|| "overflow computing decoder self kv size".to_string())?;
            if q.len() != n_state || k_all.len() != kv_need || v_all.len() != kv_need {
                return Err(format!(
                    "decoder self kv len mismatch: q={}, k_all={}, v_all={}, expected q={}, kv={}",
                    q.len(),
                    k_all.len(),
                    v_all.len(),
                    n_state,
                    kv_need
                ));
            }

            let (k_id, v_id) = self.ensure_decoder_kv_layer(layer, n_state, n_kv)?;
            let row_bytes = n_state
                .checked_mul(std::mem::size_of::<f32>())
                .ok_or_else(|| "overflow computing decoder kv row bytes".to_string())?;
            let start_row = self
                .decoder_kv_layers
                .get(&layer)
                .map(|e| e.len_rows.min(n_kv))
                .unwrap_or(0);
            if start_row < n_kv {
                let copy_rows = n_kv - start_row;
                let copy_bytes = copy_rows
                    .checked_mul(row_bytes)
                    .ok_or_else(|| "overflow computing decoder kv copy bytes".to_string())?;
                let offset = start_row
                    .checked_mul(row_bytes)
                    .ok_or_else(|| "overflow computing decoder kv copy offset".to_string())?;
                let src_k = f32_slice_as_bytes(&k_all[start_row * n_state..n_kv * n_state]);
                let src_v = f32_slice_as_bytes(&v_all[start_row * n_state..n_kv * n_state]);
                let dst_k: *mut u8 = unsafe { msg_send![k_id, contents] };
                let dst_v: *mut u8 = unsafe { msg_send![v_id, contents] };
                if dst_k.is_null() || dst_v.is_null() {
                    return Err("decoder kv buffer contents returned null".to_string());
                }
                unsafe {
                    std::ptr::copy_nonoverlapping(src_k.as_ptr(), dst_k.add(offset), copy_bytes);
                    std::ptr::copy_nonoverlapping(src_v.as_ptr(), dst_v.add(offset), copy_bytes);
                }
                if let Some(entry) = self.decoder_kv_layers.get_mut(&layer) {
                    entry.len_rows = n_kv;
                }
            }

            let q_buf = self.new_buffer_with_bytes(f32_slice_as_bytes(q))?;
            let out_buf =
                self.flash_attn_f32_from_buffers(q_buf.as_id(), k_id, v_id, 1, n_kv, n_head, d, scale)?;
            self.read_f32_buffer(out_buf.as_id(), n_state)
        }

        #[allow(clippy::too_many_arguments)]
        fn flash_attn_f32_cross_kv_cache(
            &mut self,
            layer: usize,
            q: &[f32],
            k_cross: &[f32],
            v_cross: &[f32],
            n_q: usize,
            n_kv: usize,
            n_head: usize,
            d: usize,
            scale: f32,
        ) -> Result<Vec<f32>, String> {
            if n_q == 0 || n_kv == 0 || n_head == 0 || d == 0 {
                return Ok(Vec::new());
            }
            let n_state = n_head
                .checked_mul(d)
                .ok_or_else(|| "overflow computing n_state".to_string())?;
            let q_need = n_q
                .checked_mul(n_state)
                .ok_or_else(|| "overflow computing cross q size".to_string())?;
            if q.len() != q_need {
                return Err(format!(
                    "cross q len mismatch: got {}, expected {}",
                    q.len(),
                    q_need
                ));
            }

            let (k_id, v_id) =
                self.ensure_cross_kv_layer(layer, n_state, n_kv, k_cross, v_cross)?;
            let q_buf = self.new_buffer_with_bytes(f32_slice_as_bytes(q))?;
            let out_buf =
                self.flash_attn_f32_from_buffers(q_buf.as_id(), k_id, v_id, n_q, n_kv, n_head, d, scale)?;
            self.read_f32_buffer(out_buf.as_id(), q_need)
        }

        fn flash_attn_f32_from_buffers(
            &mut self,
            q_id: ObjcId,
            k_id: ObjcId,
            v_id: ObjcId,
            n_q: usize,
            n_kv: usize,
            n_head: usize,
            d: usize,
            scale: f32,
        ) -> Result<StrongId, String> {
            let out_elems = n_q
                .checked_mul(n_head)
                .and_then(|v| v.checked_mul(d))
                .ok_or_else(|| "overflow computing flash output size".to_string())?;
            let out_bytes = out_elems
                .checked_mul(std::mem::size_of::<f32>())
                .ok_or_else(|| "overflow computing flash output bytes".to_string())?;

            let dst_id = self.get_or_create_scratch_buffer(SCRATCH_FLASH_OUT, out_bytes)?;

            let params = FlashAttnExtParams::default();
            let use_vec = flash_attn_use_vec(n_q, d);
            let pad_bytes =
                flash_attn_ext_extra_pad_bytes(n_q, n_kv, n_head, d, params.has_mask, use_vec)?;
            let pad_id = self.get_or_create_scratch_buffer(SCRATCH_FLASH_PAD, pad_bytes)?;

            if use_vec {
                let tmp_bytes = flash_attn_ext_extra_tmp_bytes(n_q, n_head, d, 32)?;
                let tmp_id = self.get_or_create_scratch_buffer(SCRATCH_FLASH_TMP, tmp_bytes)?;

                self.dispatch_flash_attn_ext_vec_f32(
                    q_id,
                    k_id,
                    v_id,
                    q_id, // unused when has_mask=false
                    q_id, // unused when has_sinks=false
                    pad_id,
                    Some(tmp_id),
                    dst_id,
                    n_q,
                    n_kv,
                    n_head,
                    d,
                    scale,
                    params.has_mask,
                    params.has_sinks,
                    params.max_bias,
                    params.logit_softcap,
                )?;
            } else {
                let blk_bytes = flash_attn_ext_extra_blk_bytes(n_q, n_kv, params.has_mask, false)?;
                let blk_id = self.get_or_create_scratch_buffer(SCRATCH_FLASH_BLK, blk_bytes)?;

                self.dispatch_flash_attn_ext_f32(
                    q_id,
                    k_id,
                    v_id,
                    q_id, // unused when has_mask=false
                    q_id, // unused when has_sinks=false
                    pad_id,
                    blk_id,
                    dst_id,
                    n_q,
                    n_kv,
                    n_head,
                    d,
                    scale,
                    params.has_mask,
                    params.has_sinks,
                    params.max_bias,
                    params.logit_softcap,
                )?;
            }

            unsafe { StrongId::from_unowned(dst_id) }
                .ok_or_else(|| "flash-attn output buffer returned nil".to_string())
        }

        #[allow(clippy::too_many_arguments)]
        fn linear_from_src_buffer(
            &mut self,
            src_id: ObjcId,
            m: usize,
            k: usize,
            w_bytes: &[u8],
            w_ggml_type: u32,
            n: usize,
            bias: Option<&[f32]>,
            weight_tag: u8,
            bias_tag: u8,
        ) -> Result<StrongId, String> {
            let dst = self.matmul_nt_ggml_from_src1_buffer(
                src_id,
                w_bytes,
                w_ggml_type,
                m,
                k,
                n,
                Some(weight_tag),
            )?;

            if let Some(bias) = bias {
                if bias.len() != n {
                    return Err(format!(
                        "linear bias len mismatch: got {}, expected {}",
                        bias.len(),
                        n
                    ));
                }
                let bias_id = self.get_or_create_cached_f32_buffer(bias, bias_tag)?;
                let dst_shape = shape4_from_row_major(&[m, n], 4)?;
                let bias_shape = shape4_from_row_major(&[n], 4)?;
                self.dispatch_bin_f32(
                    0,
                    dst.as_id(),
                    bias_id,
                    dst.as_id(),
                    &dst_shape,
                    &bias_shape,
                )?;
            }

            Ok(dst)
        }

        #[allow(clippy::too_many_arguments)]
        fn encoder_attn_block_f32(
            &mut self,
            x: &[f32],
            seq_len: usize,
            n_state: usize,
            n_head: usize,
            ln_w: &[f32],
            ln_b: &[f32],
            q_w_bytes: &[u8],
            q_w_ggml_type: u32,
            q_b: &[f32],
            k_w_bytes: &[u8],
            k_w_ggml_type: u32,
            v_w_bytes: &[u8],
            v_w_ggml_type: u32,
            v_b: &[f32],
            out_w_bytes: &[u8],
            out_w_ggml_type: u32,
            out_b: &[f32],
        ) -> Result<Vec<f32>, String> {
            if n_state == 0 || seq_len == 0 || n_head == 0 || n_state % n_head != 0 {
                return Err(format!(
                    "invalid attn dimensions: seq_len={}, n_state={}, n_head={}",
                    seq_len, n_state, n_head
                ));
            }
            let x_need = seq_len
                .checked_mul(n_state)
                .ok_or_else(|| "overflow computing x size".to_string())?;
            if x.len() != x_need {
                return Err(format!(
                    "x len mismatch: got {}, expected {}",
                    x.len(),
                    x_need
                ));
            }
            if ln_w.len() != n_state || ln_b.len() != n_state {
                return Err("layernorm affine size mismatch".to_string());
            }
            if q_b.len() != n_state || v_b.len() != n_state || out_b.len() != n_state {
                return Err("attention bias size mismatch".to_string());
            }

            let x_buf = self.new_buffer_with_bytes(f32_slice_as_bytes(x))?;
            let x_shape = shape4_from_row_major(&[seq_len, n_state], 4)?;
            let ln_shape = shape4_from_row_major(&[n_state], 4)?;
            let ln_w_id = self.get_or_create_cached_f32_buffer(ln_w, 110)?;
            let ln_b_id = self.get_or_create_cached_f32_buffer(ln_b, 111)?;

            let norm_buf = self.new_buffer_with_length(x_need * std::mem::size_of::<f32>())?;
            self.dispatch_norm_f32(
                x_buf.as_id(),
                ln_w_id,
                ln_b_id,
                norm_buf.as_id(),
                &x_shape,
                &ln_shape,
                &ln_shape,
                1e-5f32,
                3,
            )?;

            let q_buf = self.linear_from_src_buffer(
                norm_buf.as_id(),
                seq_len,
                n_state,
                q_w_bytes,
                q_w_ggml_type,
                n_state,
                Some(q_b),
                112,
                113,
            )?;
            let k_buf = self.linear_from_src_buffer(
                norm_buf.as_id(),
                seq_len,
                n_state,
                k_w_bytes,
                k_w_ggml_type,
                n_state,
                None,
                114,
                0,
            )?;
            let v_buf = self.linear_from_src_buffer(
                norm_buf.as_id(),
                seq_len,
                n_state,
                v_w_bytes,
                v_w_ggml_type,
                n_state,
                Some(v_b),
                115,
                116,
            )?;

            let d = n_state / n_head;
            let scale = 1.0f32 / (d as f32).sqrt();
            let attn_buf = self.flash_attn_f32_from_buffers(
                q_buf.as_id(),
                k_buf.as_id(),
                v_buf.as_id(),
                seq_len,
                seq_len,
                n_head,
                d,
                scale,
            )?;

            let proj_buf = self.linear_from_src_buffer(
                attn_buf.as_id(),
                seq_len,
                n_state,
                out_w_bytes,
                out_w_ggml_type,
                n_state,
                Some(out_b),
                117,
                118,
            )?;

            self.dispatch_bin_f32(
                0,
                proj_buf.as_id(),
                x_buf.as_id(),
                proj_buf.as_id(),
                &x_shape,
                &x_shape,
            )?;

            self.read_f32_buffer(proj_buf.as_id(), x_need)
        }

        #[allow(clippy::too_many_arguments)]
        fn encoder_ffn_block_f32(
            &mut self,
            x: &[f32],
            seq_len: usize,
            n_state: usize,
            ln_w: &[f32],
            ln_b: &[f32],
            w0_bytes: &[u8],
            w0_ggml_type: u32,
            b0: &[f32],
            w1_bytes: &[u8],
            w1_ggml_type: u32,
            b1: &[f32],
        ) -> Result<Vec<f32>, String> {
            if n_state == 0 || seq_len == 0 {
                return Err(format!(
                    "invalid ffn dimensions: seq_len={}, n_state={}",
                    seq_len, n_state
                ));
            }
            let x_need = seq_len
                .checked_mul(n_state)
                .ok_or_else(|| "overflow computing x size".to_string())?;
            if x.len() != x_need {
                return Err(format!(
                    "x len mismatch: got {}, expected {}",
                    x.len(),
                    x_need
                ));
            }
            if ln_w.len() != n_state || ln_b.len() != n_state {
                return Err("layernorm affine size mismatch".to_string());
            }
            let n_ff = b0.len();
            if n_ff == 0 || b1.len() != n_state {
                return Err("ffn bias size mismatch".to_string());
            }

            let x_buf = self.new_buffer_with_bytes(f32_slice_as_bytes(x))?;
            let x_shape = shape4_from_row_major(&[seq_len, n_state], 4)?;
            let ln_shape = shape4_from_row_major(&[n_state], 4)?;
            let ln_w_id = self.get_or_create_cached_f32_buffer(ln_w, 120)?;
            let ln_b_id = self.get_or_create_cached_f32_buffer(ln_b, 121)?;

            let norm_buf = self.new_buffer_with_length(x_need * std::mem::size_of::<f32>())?;
            self.dispatch_norm_f32(
                x_buf.as_id(),
                ln_w_id,
                ln_b_id,
                norm_buf.as_id(),
                &x_shape,
                &ln_shape,
                &ln_shape,
                1e-5f32,
                3,
            )?;

            let ff0_buf = self.linear_from_src_buffer(
                norm_buf.as_id(),
                seq_len,
                n_state,
                w0_bytes,
                w0_ggml_type,
                n_ff,
                Some(b0),
                122,
                123,
            )?;

            let ff0_shape = shape4_from_row_major(&[seq_len, n_ff], 4)?;
            self.dispatch_unary_f32(
                OP_UNARY_NUM_GELU,
                ff0_buf.as_id(),
                ff0_buf.as_id(),
                &ff0_shape,
            )?;

            let ff1_buf = self.linear_from_src_buffer(
                ff0_buf.as_id(),
                seq_len,
                n_ff,
                w1_bytes,
                w1_ggml_type,
                n_state,
                Some(b1),
                124,
                125,
            )?;

            self.dispatch_bin_f32(
                0,
                ff1_buf.as_id(),
                x_buf.as_id(),
                ff1_buf.as_id(),
                &x_shape,
                &x_shape,
            )?;

            self.read_f32_buffer(ff1_buf.as_id(), x_need)
        }

        #[allow(clippy::too_many_arguments)]
        fn encoder_layer_from_buffer_f32(
            &mut self,
            x_id: ObjcId,
            seq_len: usize,
            n_state: usize,
            n_head: usize,
            attn_ln_w: &[f32],
            attn_ln_b: &[f32],
            q_w_bytes: &[u8],
            q_w_ggml_type: u32,
            q_b: &[f32],
            k_w_bytes: &[u8],
            k_w_ggml_type: u32,
            v_w_bytes: &[u8],
            v_w_ggml_type: u32,
            v_b: &[f32],
            out_w_bytes: &[u8],
            out_w_ggml_type: u32,
            out_b: &[f32],
            mlp_ln_w: &[f32],
            mlp_ln_b: &[f32],
            w0_bytes: &[u8],
            w0_ggml_type: u32,
            b0: &[f32],
            w1_bytes: &[u8],
            w1_ggml_type: u32,
            b1: &[f32],
            tag_base: u8,
        ) -> Result<StrongId, String> {
            if n_state == 0 || seq_len == 0 || n_head == 0 || n_state % n_head != 0 {
                return Err(format!(
                    "invalid encoder layer dimensions: seq_len={}, n_state={}, n_head={}",
                    seq_len, n_state, n_head
                ));
            }
            if attn_ln_w.len() != n_state
                || attn_ln_b.len() != n_state
                || mlp_ln_w.len() != n_state
                || mlp_ln_b.len() != n_state
            {
                return Err("layernorm affine size mismatch".to_string());
            }
            if q_b.len() != n_state || v_b.len() != n_state || out_b.len() != n_state {
                return Err("attention bias size mismatch".to_string());
            }
            let n_ff = b0.len();
            if n_ff == 0 || b1.len() != n_state {
                return Err("ffn bias size mismatch".to_string());
            }

            let x_shape = shape4_from_row_major(&[seq_len, n_state], 4)?;
            let ln_shape = shape4_from_row_major(&[n_state], 4)?;

            // Attention sub-block
            let attn_ln_w_id = self.get_or_create_cached_f32_buffer(attn_ln_w, tag_base.wrapping_add(0))?;
            let attn_ln_b_id = self.get_or_create_cached_f32_buffer(attn_ln_b, tag_base.wrapping_add(1))?;
            let norm_bytes = x_shape
                .numel
                .checked_mul(std::mem::size_of::<f32>())
                .ok_or_else(|| "overflow computing encoder norm buffer bytes".to_string())?;
            let norm0_id = self.get_or_create_scratch_buffer(SCRATCH_ENC_NORM0, norm_bytes)?;
            self.dispatch_norm_f32(
                x_id,
                attn_ln_w_id,
                attn_ln_b_id,
                norm0_id,
                &x_shape,
                &ln_shape,
                &ln_shape,
                1e-5f32,
                3,
            )?;

            let q_buf = self.linear_from_src_buffer(
                norm0_id,
                seq_len,
                n_state,
                q_w_bytes,
                q_w_ggml_type,
                n_state,
                Some(q_b),
                tag_base.wrapping_add(2),
                tag_base.wrapping_add(3),
            )?;
            let k_buf = self.linear_from_src_buffer(
                norm0_id,
                seq_len,
                n_state,
                k_w_bytes,
                k_w_ggml_type,
                n_state,
                None,
                tag_base.wrapping_add(4),
                0,
            )?;
            let v_buf = self.linear_from_src_buffer(
                norm0_id,
                seq_len,
                n_state,
                v_w_bytes,
                v_w_ggml_type,
                n_state,
                Some(v_b),
                tag_base.wrapping_add(5),
                tag_base.wrapping_add(6),
            )?;

            let d = n_state / n_head;
            let scale = 1.0f32 / (d as f32).sqrt();
            let attn_buf = self.flash_attn_f32_from_buffers(
                q_buf.as_id(),
                k_buf.as_id(),
                v_buf.as_id(),
                seq_len,
                seq_len,
                n_head,
                d,
                scale,
            )?;
            let attn_res_buf = self.linear_from_src_buffer(
                attn_buf.as_id(),
                seq_len,
                n_state,
                out_w_bytes,
                out_w_ggml_type,
                n_state,
                Some(out_b),
                tag_base.wrapping_add(7),
                tag_base.wrapping_add(8),
            )?;
            self.dispatch_bin_f32(
                0,
                attn_res_buf.as_id(),
                x_id,
                attn_res_buf.as_id(),
                &x_shape,
                &x_shape,
            )?;

            // FFN sub-block
            let mlp_ln_w_id = self.get_or_create_cached_f32_buffer(mlp_ln_w, tag_base.wrapping_add(9))?;
            let mlp_ln_b_id = self.get_or_create_cached_f32_buffer(mlp_ln_b, tag_base.wrapping_add(10))?;
            let norm1_id = self.get_or_create_scratch_buffer(SCRATCH_ENC_NORM1, norm_bytes)?;
            self.dispatch_norm_f32(
                attn_res_buf.as_id(),
                mlp_ln_w_id,
                mlp_ln_b_id,
                norm1_id,
                &x_shape,
                &ln_shape,
                &ln_shape,
                1e-5f32,
                3,
            )?;

            let ff0_buf = self.linear_from_src_buffer(
                norm1_id,
                seq_len,
                n_state,
                w0_bytes,
                w0_ggml_type,
                n_ff,
                Some(b0),
                tag_base.wrapping_add(11),
                tag_base.wrapping_add(12),
            )?;
            let ff0_shape = shape4_from_row_major(&[seq_len, n_ff], 4)?;
            self.dispatch_unary_f32(
                OP_UNARY_NUM_GELU,
                ff0_buf.as_id(),
                ff0_buf.as_id(),
                &ff0_shape,
            )?;
            let ff1_buf = self.linear_from_src_buffer(
                ff0_buf.as_id(),
                seq_len,
                n_ff,
                w1_bytes,
                w1_ggml_type,
                n_state,
                Some(b1),
                tag_base.wrapping_add(13),
                tag_base.wrapping_add(14),
            )?;
            self.dispatch_bin_f32(
                0,
                ff1_buf.as_id(),
                attn_res_buf.as_id(),
                ff1_buf.as_id(),
                &x_shape,
                &x_shape,
            )?;

            Ok(ff1_buf)
        }

        #[allow(clippy::too_many_arguments)]
        fn encoder_layer_f32(
            &mut self,
            x: &[f32],
            seq_len: usize,
            n_state: usize,
            n_head: usize,
            attn_ln_w: &[f32],
            attn_ln_b: &[f32],
            q_w_bytes: &[u8],
            q_w_ggml_type: u32,
            q_b: &[f32],
            k_w_bytes: &[u8],
            k_w_ggml_type: u32,
            v_w_bytes: &[u8],
            v_w_ggml_type: u32,
            v_b: &[f32],
            out_w_bytes: &[u8],
            out_w_ggml_type: u32,
            out_b: &[f32],
            mlp_ln_w: &[f32],
            mlp_ln_b: &[f32],
            w0_bytes: &[u8],
            w0_ggml_type: u32,
            b0: &[f32],
            w1_bytes: &[u8],
            w1_ggml_type: u32,
            b1: &[f32],
        ) -> Result<Vec<f32>, String> {
            let x_need = seq_len
                .checked_mul(n_state)
                .ok_or_else(|| "overflow computing x size".to_string())?;
            if x.len() != x_need {
                return Err(format!(
                    "x len mismatch: got {}, expected {}",
                    x.len(),
                    x_need
                ));
            }
            let x_buf = self.new_buffer_with_bytes(f32_slice_as_bytes(x))?;

            let ff1_buf = self.encoder_layer_from_buffer_f32(
                x_buf.as_id(),
                seq_len,
                n_state,
                n_head,
                attn_ln_w,
                attn_ln_b,
                q_w_bytes,
                q_w_ggml_type,
                q_b,
                k_w_bytes,
                k_w_ggml_type,
                v_w_bytes,
                v_w_ggml_type,
                v_b,
                out_w_bytes,
                out_w_ggml_type,
                out_b,
                mlp_ln_w,
                mlp_ln_b,
                w0_bytes,
                w0_ggml_type,
                b0,
                w1_bytes,
                w1_ggml_type,
                b1,
                130,
            )?;
            self.read_f32_buffer(ff1_buf.as_id(), x_need)
        }

        fn encoder_stack_f32(
            &mut self,
            x: &[f32],
            seq_len: usize,
            n_state: usize,
            n_head: usize,
            layers: &[EncoderLayer],
            final_ln_w: &[f32],
            final_ln_b: &[f32],
        ) -> Result<Vec<f32>, String> {
            if n_state == 0 || seq_len == 0 || n_head == 0 || n_state % n_head != 0 {
                return Err(format!(
                    "invalid encoder stack dimensions: seq_len={}, n_state={}, n_head={}",
                    seq_len, n_state, n_head
                ));
            }
            let x_need = seq_len
                .checked_mul(n_state)
                .ok_or_else(|| "overflow computing encoder stack x size".to_string())?;
            if x.len() != x_need {
                return Err(format!(
                    "encoder stack x len mismatch: got {}, expected {}",
                    x.len(),
                    x_need
                ));
            }
            if final_ln_w.len() != n_state || final_ln_b.len() != n_state {
                return Err("encoder stack final layernorm affine size mismatch".to_string());
            }

            let x_shape = shape4_from_row_major(&[seq_len, n_state], 4)?;
            let ln_shape = shape4_from_row_major(&[n_state], 4)?;

            let mut cur_buf = self.new_buffer_with_bytes(f32_slice_as_bytes(x))?;

            for (il, layer) in layers.iter().enumerate() {
                let tag_base = (il as u8).wrapping_mul(16).wrapping_add(160);
                let cur_id = cur_buf.as_id();
                cur_buf = self.with_batch(|ctx| {
                    ctx.encoder_layer_from_buffer_f32(
                        cur_id,
                        seq_len,
                        n_state,
                        n_head,
                        &layer.attn_ln_0_w.data,
                        &layer.attn_ln_0_b.data,
                        &layer.attn_q_w.data,
                        layer.attn_q_w.ggml_type,
                        &layer.attn_q_b.data,
                        &layer.attn_k_w.data,
                        layer.attn_k_w.ggml_type,
                        &layer.attn_v_w.data,
                        layer.attn_v_w.ggml_type,
                        &layer.attn_v_b.data,
                        &layer.attn_ln_1_w.data,
                        layer.attn_ln_1_w.ggml_type,
                        &layer.attn_ln_1_b.data,
                        &layer.mlp_ln_w.data,
                        &layer.mlp_ln_b.data,
                        &layer.mlp_0_w.data,
                        layer.mlp_0_w.ggml_type,
                        &layer.mlp_0_b.data,
                        &layer.mlp_1_w.data,
                        layer.mlp_1_w.ggml_type,
                        &layer.mlp_1_b.data,
                        tag_base,
                    )
                })?;
            }

            let ln_w_id = self.get_or_create_cached_f32_buffer(final_ln_w, 120)?;
            let ln_b_id = self.get_or_create_cached_f32_buffer(final_ln_b, 121)?;
            let out_buf = self.new_buffer_with_length(x_need * std::mem::size_of::<f32>())?;
            self.dispatch_norm_f32(
                cur_buf.as_id(),
                ln_w_id,
                ln_b_id,
                out_buf.as_id(),
                &x_shape,
                &ln_shape,
                &ln_shape,
                1e-5f32,
                3,
            )?;

            self.read_f32_buffer(out_buf.as_id(), x_need)
        }

        #[allow(clippy::too_many_arguments)]
        fn decoder_self_cross_ffn_step_f32(
            &mut self,
            layer_idx: usize,
            x: &[f32],
            q_self: &[f32],
            k_all: &[f32],
            v_all: &[f32],
            n_kv: usize,
            n_state: usize,
            n_head: usize,
            k_cross: &[f32],
            v_cross: &[f32],
            n_audio_ctx: usize,
            layer: &DecoderLayer,
        ) -> Result<Vec<f32>, String> {
            if n_state == 0 || n_head == 0 || n_state % n_head != 0 {
                return Err(format!(
                    "invalid decoder dimensions: n_state={}, n_head={}",
                    n_state, n_head
                ));
            }
            if x.len() != n_state || q_self.len() != n_state {
                return Err(format!(
                    "decoder self-cross x/q len mismatch: x={}, q={}, expected={}",
                    x.len(),
                    q_self.len(),
                    n_state
                ));
            }
            let kv_need = n_kv
                .checked_mul(n_state)
                .ok_or_else(|| "overflow computing decoder self kv size".to_string())?;
            if k_all.len() != kv_need || v_all.len() != kv_need {
                return Err(format!(
                    "decoder self kv len mismatch: k_all={}, v_all={}, expected={}",
                    k_all.len(),
                    v_all.len(),
                    kv_need
                ));
            }
            if layer.attn_ln_1_b.data.len() != n_state
                || layer.cross_attn_ln_0_w.data.len() != n_state
                || layer.cross_attn_ln_0_b.data.len() != n_state
                || layer.cross_attn_q_b.data.len() != n_state
                || layer.cross_attn_ln_1_b.data.len() != n_state
                || layer.mlp_ln_w.data.len() != n_state
                || layer.mlp_ln_b.data.len() != n_state
                || layer.mlp_1_b.data.len() != n_state
            {
                return Err("decoder self/cross/ffn affine size mismatch".to_string());
            }

            let n_ff = layer.mlp_0_b.data.len();
            if n_ff == 0 {
                return Err("decoder ffn hidden size is zero".to_string());
            }

            let d = n_state / n_head;
            let scale = 1.0f32 / (d as f32).sqrt();

            let out_buf = self.with_batch(|ctx| {
                let x_shape = shape4_from_row_major(&[1, n_state], 4)?;
                let ln_shape = shape4_from_row_major(&[n_state], 4)?;

                let x_buf = ctx.new_buffer_with_bytes(f32_slice_as_bytes(x))?;

                let (k_self_id, v_self_id) = ctx.ensure_decoder_kv_layer(layer_idx, n_state, n_kv)?;
                let row_bytes = n_state
                    .checked_mul(std::mem::size_of::<f32>())
                    .ok_or_else(|| "overflow computing decoder kv row bytes".to_string())?;
                let start_row = ctx
                    .decoder_kv_layers
                    .get(&layer_idx)
                    .map(|e| e.len_rows.min(n_kv))
                    .unwrap_or(0);
                if start_row < n_kv {
                    let copy_rows = n_kv - start_row;
                    let copy_bytes = copy_rows
                        .checked_mul(row_bytes)
                        .ok_or_else(|| "overflow computing decoder kv copy bytes".to_string())?;
                    let offset = start_row
                        .checked_mul(row_bytes)
                        .ok_or_else(|| "overflow computing decoder kv copy offset".to_string())?;
                    let src_k = f32_slice_as_bytes(&k_all[start_row * n_state..n_kv * n_state]);
                    let src_v = f32_slice_as_bytes(&v_all[start_row * n_state..n_kv * n_state]);
                    let dst_k: *mut u8 = unsafe { msg_send![k_self_id, contents] };
                    let dst_v: *mut u8 = unsafe { msg_send![v_self_id, contents] };
                    if dst_k.is_null() || dst_v.is_null() {
                        return Err("decoder kv buffer contents returned null".to_string());
                    }
                    unsafe {
                        std::ptr::copy_nonoverlapping(src_k.as_ptr(), dst_k.add(offset), copy_bytes);
                        std::ptr::copy_nonoverlapping(src_v.as_ptr(), dst_v.add(offset), copy_bytes);
                    }
                    if let Some(entry) = ctx.decoder_kv_layers.get_mut(&layer_idx) {
                        entry.len_rows = n_kv;
                    }
                }

                let q_self_buf = ctx.new_buffer_with_bytes(f32_slice_as_bytes(q_self))?;
                let self_attn_buf = ctx.flash_attn_f32_from_buffers(
                    q_self_buf.as_id(),
                    k_self_id,
                    v_self_id,
                    1,
                    n_kv,
                    n_head,
                    d,
                    scale,
                )?;

                let self_proj_buf = ctx.linear_from_src_buffer(
                    self_attn_buf.as_id(),
                    1,
                    n_state,
                    &layer.attn_ln_1_w.data,
                    layer.attn_ln_1_w.ggml_type,
                    n_state,
                    Some(&layer.attn_ln_1_b.data),
                    212,
                    213,
                )?;

                ctx.dispatch_bin_f32(
                    0,
                    self_proj_buf.as_id(),
                    x_buf.as_id(),
                    self_proj_buf.as_id(),
                    &x_shape,
                    &x_shape,
                )?;

                let (k_cross_id, v_cross_id) =
                    ctx.ensure_cross_kv_layer(layer_idx, n_state, n_audio_ctx, k_cross, v_cross)?;

                let cross_ln_w_id = ctx.get_or_create_cached_f32_buffer(&layer.cross_attn_ln_0_w.data, 214)?;
                let cross_ln_b_id = ctx.get_or_create_cached_f32_buffer(&layer.cross_attn_ln_0_b.data, 215)?;
                let norm_bytes = n_state
                    .checked_mul(std::mem::size_of::<f32>())
                    .ok_or_else(|| "overflow computing decoder norm bytes".to_string())?;
                let norm0_id = ctx.get_or_create_scratch_buffer(SCRATCH_DEC_NORM0, norm_bytes)?;
                ctx.dispatch_norm_f32(
                    self_proj_buf.as_id(),
                    cross_ln_w_id,
                    cross_ln_b_id,
                    norm0_id,
                    &x_shape,
                    &ln_shape,
                    &ln_shape,
                    1e-5f32,
                    3,
                )?;

                let q_cross_buf = ctx.linear_from_src_buffer(
                    norm0_id,
                    1,
                    n_state,
                    &layer.cross_attn_q_w.data,
                    layer.cross_attn_q_w.ggml_type,
                    n_state,
                    Some(&layer.cross_attn_q_b.data),
                    216,
                    217,
                )?;

                let cross_attn_buf = ctx.flash_attn_f32_from_buffers(
                    q_cross_buf.as_id(),
                    k_cross_id,
                    v_cross_id,
                    1,
                    n_audio_ctx,
                    n_head,
                    d,
                    scale,
                )?;

                let cross_proj_buf = ctx.linear_from_src_buffer(
                    cross_attn_buf.as_id(),
                    1,
                    n_state,
                    &layer.cross_attn_ln_1_w.data,
                    layer.cross_attn_ln_1_w.ggml_type,
                    n_state,
                    Some(&layer.cross_attn_ln_1_b.data),
                    218,
                    219,
                )?;

                ctx.dispatch_bin_f32(
                    0,
                    cross_proj_buf.as_id(),
                    self_proj_buf.as_id(),
                    cross_proj_buf.as_id(),
                    &x_shape,
                    &x_shape,
                )?;

                let mlp_ln_w_id = ctx.get_or_create_cached_f32_buffer(&layer.mlp_ln_w.data, 220)?;
                let mlp_ln_b_id = ctx.get_or_create_cached_f32_buffer(&layer.mlp_ln_b.data, 221)?;
                let norm1_id = ctx.get_or_create_scratch_buffer(SCRATCH_DEC_NORM1, norm_bytes)?;
                ctx.dispatch_norm_f32(
                    cross_proj_buf.as_id(),
                    mlp_ln_w_id,
                    mlp_ln_b_id,
                    norm1_id,
                    &x_shape,
                    &ln_shape,
                    &ln_shape,
                    1e-5f32,
                    3,
                )?;

                let ff0_buf = ctx.linear_from_src_buffer(
                    norm1_id,
                    1,
                    n_state,
                    &layer.mlp_0_w.data,
                    layer.mlp_0_w.ggml_type,
                    n_ff,
                    Some(&layer.mlp_0_b.data),
                    222,
                    223,
                )?;

                let ff0_shape = shape4_from_row_major(&[1, n_ff], 4)?;
                ctx.dispatch_unary_f32(
                    OP_UNARY_NUM_GELU,
                    ff0_buf.as_id(),
                    ff0_buf.as_id(),
                    &ff0_shape,
                )?;

                let ff1_buf = ctx.linear_from_src_buffer(
                    ff0_buf.as_id(),
                    1,
                    n_ff,
                    &layer.mlp_1_w.data,
                    layer.mlp_1_w.ggml_type,
                    n_state,
                    Some(&layer.mlp_1_b.data),
                    224,
                    225,
                )?;

                ctx.dispatch_bin_f32(
                    0,
                    ff1_buf.as_id(),
                    cross_proj_buf.as_id(),
                    ff1_buf.as_id(),
                    &x_shape,
                    &x_shape,
                )?;

                Ok(ff1_buf)
            })?;

            self.read_f32_buffer(out_buf.as_id(), n_state)
        }

        #[allow(clippy::too_many_arguments)]
        fn decoder_cross_ffn_step_f32(
            &mut self,
            layer_idx: usize,
            x: &[f32],
            n_state: usize,
            n_head: usize,
            k_cross: &[f32],
            v_cross: &[f32],
            n_audio_ctx: usize,
            layer: &DecoderLayer,
        ) -> Result<Vec<f32>, String> {
            if n_state == 0 || n_head == 0 || n_state % n_head != 0 {
                return Err(format!(
                    "invalid decoder dimensions: n_state={}, n_head={}",
                    n_state, n_head
                ));
            }
            if x.len() != n_state {
                return Err(format!(
                    "decoder x len mismatch: got {}, expected {}",
                    x.len(),
                    n_state
                ));
            }
            if layer.cross_attn_ln_0_w.data.len() != n_state
                || layer.cross_attn_ln_0_b.data.len() != n_state
                || layer.cross_attn_q_b.data.len() != n_state
                || layer.cross_attn_ln_1_b.data.len() != n_state
                || layer.mlp_ln_w.data.len() != n_state
                || layer.mlp_ln_b.data.len() != n_state
                || layer.mlp_1_b.data.len() != n_state
            {
                return Err("decoder cross/ffn affine size mismatch".to_string());
            }

            let n_ff = layer.mlp_0_b.data.len();
            if n_ff == 0 {
                return Err("decoder ffn hidden size is zero".to_string());
            }

            let d = n_state / n_head;
            let scale = 1.0f32 / (d as f32).sqrt();

            let out_buf = self.with_batch(|ctx| {
                let x_shape = shape4_from_row_major(&[1, n_state], 4)?;
                let ln_shape = shape4_from_row_major(&[n_state], 4)?;

                let x_buf = ctx.new_buffer_with_bytes(f32_slice_as_bytes(x))?;
                let (k_cross_id, v_cross_id) =
                    ctx.ensure_cross_kv_layer(layer_idx, n_state, n_audio_ctx, k_cross, v_cross)?;

                // Cross-attention block
                let cross_ln_w_id = ctx.get_or_create_cached_f32_buffer(&layer.cross_attn_ln_0_w.data, 200)?;
                let cross_ln_b_id = ctx.get_or_create_cached_f32_buffer(&layer.cross_attn_ln_0_b.data, 201)?;
                let norm_bytes = n_state
                    .checked_mul(std::mem::size_of::<f32>())
                    .ok_or_else(|| "overflow computing decoder norm bytes".to_string())?;
                let norm0_id = ctx.get_or_create_scratch_buffer(SCRATCH_DEC_NORM0, norm_bytes)?;
                ctx.dispatch_norm_f32(
                    x_buf.as_id(),
                    cross_ln_w_id,
                    cross_ln_b_id,
                    norm0_id,
                    &x_shape,
                    &ln_shape,
                    &ln_shape,
                    1e-5f32,
                    3,
                )?;

                let q_buf = ctx.linear_from_src_buffer(
                    norm0_id,
                    1,
                    n_state,
                    &layer.cross_attn_q_w.data,
                    layer.cross_attn_q_w.ggml_type,
                    n_state,
                    Some(&layer.cross_attn_q_b.data),
                    202,
                    203,
                )?;

                let attn_buf = ctx.flash_attn_f32_from_buffers(
                    q_buf.as_id(),
                    k_cross_id,
                    v_cross_id,
                    1,
                    n_audio_ctx,
                    n_head,
                    d,
                    scale,
                )?;

                let cross_proj_buf = ctx.linear_from_src_buffer(
                    attn_buf.as_id(),
                    1,
                    n_state,
                    &layer.cross_attn_ln_1_w.data,
                    layer.cross_attn_ln_1_w.ggml_type,
                    n_state,
                    Some(&layer.cross_attn_ln_1_b.data),
                    204,
                    205,
                )?;

                ctx.dispatch_bin_f32(
                    0,
                    cross_proj_buf.as_id(),
                    x_buf.as_id(),
                    cross_proj_buf.as_id(),
                    &x_shape,
                    &x_shape,
                )?;

                // FFN block
                let mlp_ln_w_id = ctx.get_or_create_cached_f32_buffer(&layer.mlp_ln_w.data, 206)?;
                let mlp_ln_b_id = ctx.get_or_create_cached_f32_buffer(&layer.mlp_ln_b.data, 207)?;
                let norm1_id = ctx.get_or_create_scratch_buffer(SCRATCH_DEC_NORM1, norm_bytes)?;
                ctx.dispatch_norm_f32(
                    cross_proj_buf.as_id(),
                    mlp_ln_w_id,
                    mlp_ln_b_id,
                    norm1_id,
                    &x_shape,
                    &ln_shape,
                    &ln_shape,
                    1e-5f32,
                    3,
                )?;

                let ff0_buf = ctx.linear_from_src_buffer(
                    norm1_id,
                    1,
                    n_state,
                    &layer.mlp_0_w.data,
                    layer.mlp_0_w.ggml_type,
                    n_ff,
                    Some(&layer.mlp_0_b.data),
                    208,
                    209,
                )?;

                let ff0_shape = shape4_from_row_major(&[1, n_ff], 4)?;
                ctx.dispatch_unary_f32(
                    OP_UNARY_NUM_GELU,
                    ff0_buf.as_id(),
                    ff0_buf.as_id(),
                    &ff0_shape,
                )?;

                let ff1_buf = ctx.linear_from_src_buffer(
                    ff0_buf.as_id(),
                    1,
                    n_ff,
                    &layer.mlp_1_w.data,
                    layer.mlp_1_w.ggml_type,
                    n_state,
                    Some(&layer.mlp_1_b.data),
                    210,
                    211,
                )?;

                ctx.dispatch_bin_f32(
                    0,
                    ff1_buf.as_id(),
                    cross_proj_buf.as_id(),
                    ff1_buf.as_id(),
                    &x_shape,
                    &x_shape,
                )?;

                Ok(ff1_buf)
            })?;

            self.read_f32_buffer(out_buf.as_id(), n_state)
        }

        fn bin_f32(
            &mut self,
            op_num: i16,
            a: &[f32],
            a_shape: &[usize],
            b: &[f32],
            b_shape: &[usize],
        ) -> Result<Vec<f32>, String> {
            let a_s = shape4_from_row_major(a_shape, 4)?;
            let b_s = shape4_from_row_major(b_shape, 4)?;
            if a.len() != a_s.numel {
                return Err(format!(
                    "lhs len mismatch: got {}, expected {}",
                    a.len(),
                    a_s.numel
                ));
            }
            if b.len() != b_s.numel {
                return Err(format!(
                    "rhs len mismatch: got {}, expected {}",
                    b.len(),
                    b_s.numel
                ));
            }

            let a_bytes = unsafe {
                std::slice::from_raw_parts(
                    a.as_ptr() as *const u8,
                    a.len() * std::mem::size_of::<f32>(),
                )
            };
            let b_bytes = unsafe {
                std::slice::from_raw_parts(
                    b.as_ptr() as *const u8,
                    b.len() * std::mem::size_of::<f32>(),
                )
            };

            let a_buf = self.new_buffer_with_bytes(a_bytes)?;
            let b_buf = self.new_buffer_with_bytes(b_bytes)?;
            let dst_buf = self.new_buffer_with_length(a_s.numel * std::mem::size_of::<f32>())?;

            self.dispatch_bin_f32(
                op_num,
                a_buf.as_id(),
                b_buf.as_id(),
                dst_buf.as_id(),
                &a_s,
                &b_s,
            )?;
            self.read_f32_buffer(dst_buf.as_id(), a_s.numel)
        }

        fn unary_gelu_f32(&mut self, a: &[f32], shape: &[usize]) -> Result<Vec<f32>, String> {
            let s = shape4_from_row_major(shape, 4)?;
            if a.len() != s.numel {
                return Err(format!(
                    "unary len mismatch: got {}, expected {}",
                    a.len(),
                    s.numel
                ));
            }

            let a_bytes = unsafe {
                std::slice::from_raw_parts(
                    a.as_ptr() as *const u8,
                    a.len() * std::mem::size_of::<f32>(),
                )
            };

            let a_buf = self.new_buffer_with_bytes(a_bytes)?;
            let dst_buf = self.new_buffer_with_length(s.numel * std::mem::size_of::<f32>())?;
            self.dispatch_unary_f32(OP_UNARY_NUM_GELU, a_buf.as_id(), dst_buf.as_id(), &s)?;
            self.read_f32_buffer(dst_buf.as_id(), s.numel)
        }

        fn norm_f32(&mut self, x: &[f32], x_shape: &[usize], eps: f32) -> Result<Vec<f32>, String> {
            let s = shape4_from_row_major(x_shape, 4)?;
            if x.len() != s.numel {
                return Err(format!(
                    "norm len mismatch: got {}, expected {}",
                    x.len(),
                    s.numel
                ));
            }

            let x_bytes = unsafe {
                std::slice::from_raw_parts(
                    x.as_ptr() as *const u8,
                    x.len() * std::mem::size_of::<f32>(),
                )
            };

            let x_buf = self.new_buffer_with_bytes(x_bytes)?;
            let dst_buf = self.new_buffer_with_length(s.numel * std::mem::size_of::<f32>())?;
            self.dispatch_norm_f32(
                x_buf.as_id(),
                x_buf.as_id(),
                x_buf.as_id(),
                dst_buf.as_id(),
                &s,
                &s,
                &s,
                eps,
                1,
            )?;
            self.read_f32_buffer(dst_buf.as_id(), s.numel)
        }

        #[allow(clippy::too_many_arguments)]
        fn norm_mul_add_f32(
            &mut self,
            x: &[f32],
            x_shape: &[usize],
            mul: &[f32],
            mul_shape: &[usize],
            add: &[f32],
            add_shape: &[usize],
            eps: f32,
        ) -> Result<Vec<f32>, String> {
            let x_s = shape4_from_row_major(x_shape, 4)?;
            let m_s = shape4_from_row_major(mul_shape, 4)?;
            let a_s = shape4_from_row_major(add_shape, 4)?;

            if x.len() != x_s.numel {
                return Err(format!(
                    "norm src len mismatch: got {}, expected {}",
                    x.len(),
                    x_s.numel
                ));
            }
            if mul.len() != m_s.numel {
                return Err(format!(
                    "norm mul len mismatch: got {}, expected {}",
                    mul.len(),
                    m_s.numel
                ));
            }
            if add.len() != a_s.numel {
                return Err(format!(
                    "norm add len mismatch: got {}, expected {}",
                    add.len(),
                    a_s.numel
                ));
            }

            if m_s.ne[0] != x_s.ne[0] || a_s.ne[0] != x_s.ne[0] {
                return Err(format!(
                    "norm fuse ne0 mismatch: x={} mul={} add={}",
                    x_s.ne[0], m_s.ne[0], a_s.ne[0]
                ));
            }
            for d in 1..4 {
                if (m_s.ne[d] != 1 && m_s.ne[d] != x_s.ne[d])
                    || (a_s.ne[d] != 1 && a_s.ne[d] != x_s.ne[d])
                {
                    return Err("norm fuse broadcast mismatch".to_string());
                }
            }

            let x_bytes = unsafe {
                std::slice::from_raw_parts(
                    x.as_ptr() as *const u8,
                    x.len() * std::mem::size_of::<f32>(),
                )
            };
            let mul_bytes = unsafe {
                std::slice::from_raw_parts(
                    mul.as_ptr() as *const u8,
                    mul.len() * std::mem::size_of::<f32>(),
                )
            };
            let add_bytes = unsafe {
                std::slice::from_raw_parts(
                    add.as_ptr() as *const u8,
                    add.len() * std::mem::size_of::<f32>(),
                )
            };

            let x_buf = self.new_buffer_with_bytes(x_bytes)?;
            let mul_buf = self.new_buffer_with_bytes(mul_bytes)?;
            let add_buf = self.new_buffer_with_bytes(add_bytes)?;
            let dst_buf = self.new_buffer_with_length(x_s.numel * std::mem::size_of::<f32>())?;

            self.dispatch_norm_f32(
                x_buf.as_id(),
                mul_buf.as_id(),
                add_buf.as_id(),
                dst_buf.as_id(),
                &x_s,
                &m_s,
                &a_s,
                eps,
                3,
            )?;
            self.read_f32_buffer(dst_buf.as_id(), x_s.numel)
        }

        fn im2col_1d_f32(
            &mut self,
            input: &[f32],
            ic: usize,
            iw: usize,
            kw: usize,
            stride: usize,
            pad: usize,
        ) -> Result<Vec<f32>, String> {
            if kw == 0 || stride == 0 {
                return Err("im2col requires kw>0 and stride>0".to_string());
            }
            let expect = ic
                .checked_mul(iw)
                .ok_or_else(|| "overflow computing input size".to_string())?;
            if input.len() != expect {
                return Err(format!(
                    "im2col input len mismatch: got {}, expected {}",
                    input.len(),
                    expect
                ));
            }
            let num = iw
                .checked_add(pad.saturating_mul(2))
                .ok_or_else(|| "overflow computing im2col output numerator".to_string())?;
            if num < kw {
                return Ok(Vec::new());
            }
            let ow = (num - kw) / stride + 1;
            let chw = ic
                .checked_mul(kw)
                .ok_or_else(|| "overflow computing im2col CHW".to_string())?;
            let out_elems = ow
                .checked_mul(chw)
                .ok_or_else(|| "overflow computing im2col output size".to_string())?;

            let input_bytes = unsafe {
                std::slice::from_raw_parts(
                    input.as_ptr() as *const u8,
                    input.len() * std::mem::size_of::<f32>(),
                )
            };
            let src_buf = self.new_buffer_with_bytes(input_bytes)?;
            let dst_buf = self.new_buffer_with_length(out_elems * std::mem::size_of::<f32>())?;
            self.dispatch_im2col_1d_f32(
                src_buf.as_id(),
                dst_buf.as_id(),
                ic,
                iw,
                kw,
                stride,
                pad,
                ow,
            )?;
            self.read_f32_buffer(dst_buf.as_id(), out_elems)
        }

        fn matmul_nt_ggml_from_src1_buffer(
            &mut self,
            src1_id: ObjcId,
            bt_bytes: &[u8],
            bt_ggml_type: u32,
            m: usize,
            k: usize,
            n: usize,
            weight_cache_tag: Option<u8>,
        ) -> Result<StrongId, String> {
            if m == 0 || k == 0 || n == 0 {
                return self
                    .new_buffer_with_length(m.saturating_mul(n) * std::mem::size_of::<f32>());
            }

            let src0 = src0_type_from_ggml(bt_ggml_type).ok_or_else(|| {
                format!(
                    "unsupported src0 ggml_type for metal matmul: {}",
                    bt_ggml_type
                )
            })?;
            let (src0_row_bytes, nb00) = src0_layout_bytes_per_row(src0, k)?;
            let expected_src0 = n
                .checked_mul(src0_row_bytes)
                .ok_or_else(|| "matmul overflow computing src0 bytes".to_string())?;
            if bt_bytes.len() != expected_src0 {
                return Err(format!(
                    "rhs len mismatch: got {}, expected {} (type {:?}, k={}, n={})",
                    bt_bytes.len(),
                    expected_src0,
                    src0,
                    k,
                    n
                ));
            }

            let ne00 = i32::try_from(k).map_err(|_| format!("k too large: {}", k))?;
            let ne01 = i32::try_from(n).map_err(|_| format!("n too large: {}", n))?;
            let ne10 = i32::try_from(k).map_err(|_| format!("k too large: {}", k))?;
            let ne11 = i32::try_from(m).map_err(|_| format!("m too large: {}", m))?;
            let ne0 = ne01;
            let ne1 = ne11;
            let nb01 = src0_row_bytes as u64;
            let nb10 = 4u64;
            let nb11 = (k as u64)
                .checked_mul(4)
                .ok_or_else(|| "overflow computing nb11".to_string())?;
            let mn = m
                .checked_mul(n)
                .ok_or_else(|| "matmul overflow computing m*n".to_string())?;

            let _pool = AutoreleasePool::new();

            let mut src0_temp = None;
            let src0_id = if let Some(tag) = weight_cache_tag {
                let key = BufferKey {
                    ptr: bt_bytes.as_ptr() as usize,
                    len: bt_bytes.len(),
                    tag,
                };
                self.get_or_create_weight_buffer(key, bt_bytes)?
            } else {
                let b = self.new_buffer_with_bytes(bt_bytes)?;
                let id = b.as_id();
                src0_temp = Some(b);
                id
            };

            let dst_bytes = mn
                .checked_mul(std::mem::size_of::<f32>())
                .ok_or_else(|| "matmul overflow computing dst bytes".to_string())?;
            let mut dst_temp = None;
            let dst_id = if let Some(tag) = weight_cache_tag {
                self.get_or_create_matmul_out_buffer(tag, dst_bytes)?
            } else {
                let b = self.new_buffer_with_length(dst_bytes)?;
                let id = b.as_id();
                dst_temp = Some(b);
                id
            };
            let used_mul_mv_ext = can_use_mul_mv_ext(src0, ne00, ne11);
            let used_mul_mm = ne00 >= 64 && ne11 > 8;

            let compute_res = if used_mul_mv_ext {
                self.dispatch_mul_mv_ext(
                    src0,
                    src0_id,
                    src1_id,
                    dst_id,
                    ne00,
                    ne01,
                    ne10,
                    ne11,
                    nb00,
                    nb01,
                    nb10,
                    nb11,
                    ne0,
                    ne1,
                )
            } else if used_mul_mm {
                match self.dispatch_mul_mm(
                    src0,
                    src0_id,
                    src1_id,
                    dst_id,
                    ne00,
                    ne01,
                    nb01,
                    1,
                    nb10,
                    nb11,
                    ne0,
                    ne1,
                ) {
                    Ok(()) => Ok(()),
                    Err(e) => {
                        eprintln!(
                            "[voice][metal] mul_mm failed for type {:?}, falling back to mul_mv: {}",
                            src0, e
                        );
                        self.dispatch_mul_mv(
                            src0,
                            src0_id,
                            src1_id,
                            dst_id,
                            ne00,
                            ne01,
                            ne10,
                            ne11,
                            nb00,
                            nb01,
                            nb10,
                            nb11,
                            ne0,
                            ne1,
                        )
                    }
                }
            } else {
                self.dispatch_mul_mv(
                    src0,
                    src0_id,
                    src1_id,
                    dst_id,
                    ne00,
                    ne01,
                    ne10,
                    ne11,
                    nb00,
                    nb01,
                    nb10,
                    nb11,
                    ne0,
                    ne1,
                )
            };
            compute_res?;
            drop(src0_temp);
            if let Some(dst_buffer) = dst_temp {
                Ok(dst_buffer)
            } else {
                unsafe { StrongId::from_unowned(dst_id) }
                    .ok_or_else(|| "matmul scratch output buffer returned nil".to_string())
            }
        }

        fn matmul_nt_ggml_bytes_impl(
            &mut self,
            a: &[f32],
            bt_bytes: &[u8],
            bt_ggml_type: u32,
            m: usize,
            k: usize,
            n: usize,
            weight_cache_tag: Option<u8>,
        ) -> Result<(StrongId, usize, usize), String> {
            let mk = m
                .checked_mul(k)
                .ok_or_else(|| "matmul overflow computing m*k".to_string())?;
            if a.len() != mk {
                return Err(format!(
                    "lhs len mismatch: got {}, expected {}",
                    a.len(),
                    mk
                ));
            }

            let a_bytes = unsafe {
                std::slice::from_raw_parts(
                    a.as_ptr() as *const u8,
                    a.len() * std::mem::size_of::<f32>(),
                )
            };
            let src1_buffer = self.new_buffer_with_bytes(a_bytes)?;
            let dst_buffer = self.matmul_nt_ggml_from_src1_buffer(
                src1_buffer.as_id(),
                bt_bytes,
                bt_ggml_type,
                m,
                k,
                n,
                weight_cache_tag,
            )?;

            Ok((dst_buffer, m, n))
        }

        fn matmul_nn_f32(
            &mut self,
            a: &[f32],
            b: &[f32],
            m: usize,
            k: usize,
            n: usize,
        ) -> Result<Vec<f32>, String> {
            let mk = m
                .checked_mul(k)
                .ok_or_else(|| "overflow computing m*k".to_string())?;
            let kn = k
                .checked_mul(n)
                .ok_or_else(|| "overflow computing k*n".to_string())?;
            if a.len() != mk {
                return Err(format!(
                    "lhs len mismatch: got {}, expected {}",
                    a.len(),
                    mk
                ));
            }
            if b.len() != kn {
                return Err(format!(
                    "rhs len mismatch: got {}, expected {}",
                    b.len(),
                    kn
                ));
            }

            let mut bt = vec![0.0f32; n * k];
            for i in 0..k {
                for j in 0..n {
                    bt[j * k + i] = b[i * n + j];
                }
            }

            let bt_bytes = unsafe {
                std::slice::from_raw_parts(
                    bt.as_ptr() as *const u8,
                    bt.len() * std::mem::size_of::<f32>(),
                )
            };

            let (dst, mr, nr) =
                self.matmul_nt_ggml_bytes_impl(a, bt_bytes, GGML_TYPE_F32, m, k, n, None)?;
            self.read_f32_buffer(dst.as_id(), mr * nr)
        }

        fn matmul_nt_f32(
            &mut self,
            a: &[f32],
            bt: &[f32],
            m: usize,
            k: usize,
            n: usize,
        ) -> Result<Vec<f32>, String> {
            let bt_bytes = unsafe {
                std::slice::from_raw_parts(
                    bt.as_ptr() as *const u8,
                    bt.len() * std::mem::size_of::<f32>(),
                )
            };
            let cache_tag = Some(1u8);
            let (dst, mr, nr) =
                self.matmul_nt_ggml_bytes_impl(a, bt_bytes, GGML_TYPE_F32, m, k, n, cache_tag)?;
            self.read_f32_buffer(dst.as_id(), mr * nr)
        }

        fn matmul_nt_f32_bytes(
            &mut self,
            a: &[f32],
            bt_bytes: &[u8],
            m: usize,
            k: usize,
            n: usize,
        ) -> Result<Vec<f32>, String> {
            let (dst, mr, nr) =
                self.matmul_nt_ggml_bytes_impl(a, bt_bytes, GGML_TYPE_F32, m, k, n, Some(2u8))?;
            self.read_f32_buffer(dst.as_id(), mr * nr)
        }

        fn matmul_nt_f16_bytes(
            &mut self,
            a: &[f32],
            bt_f16_bytes: &[u8],
            m: usize,
            k: usize,
            n: usize,
        ) -> Result<Vec<f32>, String> {
            let (dst, mr, nr) =
                self.matmul_nt_ggml_bytes_impl(a, bt_f16_bytes, GGML_TYPE_F16, m, k, n, Some(3u8))?;
            self.read_f32_buffer(dst.as_id(), mr * nr)
        }

        fn matmul_nt_ggml_bytes(
            &mut self,
            a: &[f32],
            bt_bytes: &[u8],
            bt_ggml_type: u32,
            m: usize,
            k: usize,
            n: usize,
        ) -> Result<Vec<f32>, String> {
            let tag = match bt_ggml_type {
                GGML_TYPE_F32 => 2u8,
                GGML_TYPE_F16 => 3u8,
                GGML_TYPE_Q4_0 => 4u8,
                GGML_TYPE_Q4_1 => 5u8,
                GGML_TYPE_Q5_0 => 6u8,
                GGML_TYPE_Q5_1 => 7u8,
                GGML_TYPE_Q8_0 => 8u8,
                _ => 0u8,
            };
            let (dst, mr, nr) =
                self.matmul_nt_ggml_bytes_impl(a, bt_bytes, bt_ggml_type, m, k, n, Some(tag))?;
            self.read_f32_buffer(dst.as_id(), mr * nr)
        }

        fn matmul_nt_ggml_bytes_add_bias(
            &mut self,
            a: &[f32],
            bt_bytes: &[u8],
            bt_ggml_type: u32,
            m: usize,
            k: usize,
            n: usize,
            bias: &[f32],
        ) -> Result<Vec<f32>, String> {
            if bias.len() != n {
                return Err(format!(
                    "bias len mismatch for matmul+add: got {}, expected {}",
                    bias.len(),
                    n
                ));
            }

            let tag = match bt_ggml_type {
                GGML_TYPE_F32 => 2u8,
                GGML_TYPE_F16 => 3u8,
                GGML_TYPE_Q4_0 => 4u8,
                GGML_TYPE_Q4_1 => 5u8,
                GGML_TYPE_Q5_0 => 6u8,
                GGML_TYPE_Q5_1 => 7u8,
                GGML_TYPE_Q8_0 => 8u8,
                _ => 0u8,
            };

            let (dst, mr, nr) =
                self.matmul_nt_ggml_bytes_impl(a, bt_bytes, bt_ggml_type, m, k, n, Some(tag))?;

            let bias_shape = shape4_from_row_major(&[n], 4)?;
            let dst_shape = shape4_from_row_major(&[m, n], 4)?;
            let bias_bytes = unsafe {
                std::slice::from_raw_parts(
                    bias.as_ptr() as *const u8,
                    bias.len() * std::mem::size_of::<f32>(),
                )
            };
            let bias_buf = self.new_buffer_with_bytes(bias_bytes)?;
            self.dispatch_bin_f32(
                0,
                dst.as_id(),
                bias_buf.as_id(),
                dst.as_id(),
                &dst_shape,
                &bias_shape,
            )?;

            self.read_f32_buffer(dst.as_id(), mr * nr)
        }
    }

    fn with_context<T>(f: impl FnOnce(&mut MetalContext) -> Result<T, String>) -> Option<T> {
        enum ContextState {
            Uninitialized,
            Disabled,
            Ready(MetalContext),
        }

        thread_local! {
            static CONTEXT: RefCell<ContextState> = const { RefCell::new(ContextState::Uninitialized) };
        }

        CONTEXT.with(|ctx| {
            let mut ctx = ctx.borrow_mut();

            if matches!(&*ctx, ContextState::Uninitialized) {
                *ctx = match MetalContext::new() {
                    Ok(created) => ContextState::Ready(created),
                    Err(err) => {
                        eprintln!("[voice][metal] backend disabled: {}", err);
                        ContextState::Disabled
                    }
                };
            }

            let ctx = match &mut *ctx {
                ContextState::Ready(ctx) => ctx,
                ContextState::Disabled | ContextState::Uninitialized => return None,
            };
            match f(ctx) {
                Ok(v) => Some(v),
                Err(err) => {
                    eprintln!("[voice][metal] compute failed: {}", err);
                    None
                }
            }
        })
    }

    pub(super) fn try_matmul_nn_f32(
        a: &[f32],
        b: &[f32],
        m: usize,
        k: usize,
        n: usize,
    ) -> Option<Vec<f32>> {
        with_context(|ctx| ctx.matmul_nn_f32(a, b, m, k, n))
    }

    pub(super) fn try_matmul_nt_f32(
        a: &[f32],
        bt: &[f32],
        m: usize,
        k: usize,
        n: usize,
    ) -> Option<Vec<f32>> {
        with_context(|ctx| ctx.matmul_nt_f32(a, bt, m, k, n))
    }

    pub(super) fn try_matmul_nt_f32_bytes(
        a: &[f32],
        bt_bytes: &[u8],
        m: usize,
        k: usize,
        n: usize,
    ) -> Option<Vec<f32>> {
        with_context(|ctx| ctx.matmul_nt_f32_bytes(a, bt_bytes, m, k, n))
    }

    pub(super) fn try_matmul_nt_f16_bytes(
        a: &[f32],
        bt_f16_bytes: &[u8],
        m: usize,
        k: usize,
        n: usize,
    ) -> Option<Vec<f32>> {
        with_context(|ctx| ctx.matmul_nt_f16_bytes(a, bt_f16_bytes, m, k, n))
    }

    pub(super) fn try_matmul_nt_ggml_bytes(
        a: &[f32],
        bt_bytes: &[u8],
        bt_ggml_type: u32,
        m: usize,
        k: usize,
        n: usize,
    ) -> Option<Vec<f32>> {
        with_context(|ctx| ctx.matmul_nt_ggml_bytes(a, bt_bytes, bt_ggml_type, m, k, n))
    }

    pub(super) fn try_matmul_nt_ggml_bytes_add_bias(
        a: &[f32],
        bt_bytes: &[u8],
        bt_ggml_type: u32,
        m: usize,
        k: usize,
        n: usize,
        bias: &[f32],
    ) -> Option<Vec<f32>> {
        with_context(|ctx| {
            ctx.matmul_nt_ggml_bytes_add_bias(a, bt_bytes, bt_ggml_type, m, k, n, bias)
        })
    }

    #[allow(dead_code)]
    pub(super) fn try_flash_attn_f32(
        q: &[f32],
        k: &[f32],
        v: &[f32],
        n_q: usize,
        d: usize,
        scale: f32,
    ) -> Option<Vec<f32>> {
        if d == 0 || k.len() % d != 0 {
            return None;
        }
        let n_kv = k.len() / d;
        with_context(|ctx| ctx.flash_attn_f32_packed(q, k, v, n_q, n_kv, 1, d, scale))
    }

    pub(super) fn try_flash_attn_f32_packed(
        q: &[f32],
        k: &[f32],
        v: &[f32],
        n_q: usize,
        n_kv: usize,
        n_head: usize,
        d: usize,
        scale: f32,
    ) -> Option<Vec<f32>> {
        with_context(|ctx| ctx.flash_attn_f32_packed(q, k, v, n_q, n_kv, n_head, d, scale))
    }

    pub(super) fn clear_decoder_kv_cache() {
        let _ = with_context(|ctx| {
            ctx.clear_decoder_kv_cache();
            Ok(())
        });
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn try_flash_attn_f32_self_kv_cache(
        layer: usize,
        q: &[f32],
        k_all: &[f32],
        v_all: &[f32],
        n_kv: usize,
        n_head: usize,
        d: usize,
        scale: f32,
    ) -> Option<Vec<f32>> {
        with_context(|ctx| {
            ctx.flash_attn_f32_self_kv_cache(layer, q, k_all, v_all, n_kv, n_head, d, scale)
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn try_flash_attn_f32_cross_kv_cache(
        layer: usize,
        q: &[f32],
        k_cross: &[f32],
        v_cross: &[f32],
        n_q: usize,
        n_kv: usize,
        n_head: usize,
        d: usize,
        scale: f32,
    ) -> Option<Vec<f32>> {
        with_context(|ctx| {
            ctx.flash_attn_f32_cross_kv_cache(
                layer, q, k_cross, v_cross, n_q, n_kv, n_head, d, scale,
            )
        })
    }

    pub(super) fn try_add_f32(
        a: &[f32],
        a_shape: &[usize],
        b: &[f32],
        b_shape: &[usize],
    ) -> Option<Vec<f32>> {
        with_context(|ctx| ctx.bin_f32(0, a, a_shape, b, b_shape))
    }

    pub(super) fn try_mul_f32(
        a: &[f32],
        a_shape: &[usize],
        b: &[f32],
        b_shape: &[usize],
    ) -> Option<Vec<f32>> {
        with_context(|ctx| ctx.bin_f32(2, a, a_shape, b, b_shape))
    }

    pub(super) fn try_gelu_f32(a: &[f32], shape: &[usize]) -> Option<Vec<f32>> {
        with_context(|ctx| ctx.unary_gelu_f32(a, shape))
    }

    pub(super) fn try_layer_norm_f32(x: &[f32], shape: &[usize], eps: f32) -> Option<Vec<f32>> {
        with_context(|ctx| ctx.norm_f32(x, shape, eps))
    }

    pub(super) fn try_layer_norm_mul_add_f32(
        x: &[f32],
        x_shape: &[usize],
        mul: &[f32],
        mul_shape: &[usize],
        add: &[f32],
        add_shape: &[usize],
        eps: f32,
    ) -> Option<Vec<f32>> {
        with_context(|ctx| ctx.norm_mul_add_f32(x, x_shape, mul, mul_shape, add, add_shape, eps))
    }

    pub(super) fn try_im2col_1d_f32(
        input: &[f32],
        ic: usize,
        iw: usize,
        kw: usize,
        stride: usize,
        pad: usize,
    ) -> Option<Vec<f32>> {
        with_context(|ctx| ctx.im2col_1d_f32(input, ic, iw, kw, stride, pad))
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn try_encoder_attn_block_f32(
        x: &[f32],
        seq_len: usize,
        n_state: usize,
        n_head: usize,
        ln_w: &[f32],
        ln_b: &[f32],
        q_w_bytes: &[u8],
        q_w_ggml_type: u32,
        q_b: &[f32],
        k_w_bytes: &[u8],
        k_w_ggml_type: u32,
        v_w_bytes: &[u8],
        v_w_ggml_type: u32,
        v_b: &[f32],
        out_w_bytes: &[u8],
        out_w_ggml_type: u32,
        out_b: &[f32],
    ) -> Option<Vec<f32>> {
        with_context(|ctx| {
            ctx.encoder_attn_block_f32(
                x,
                seq_len,
                n_state,
                n_head,
                ln_w,
                ln_b,
                q_w_bytes,
                q_w_ggml_type,
                q_b,
                k_w_bytes,
                k_w_ggml_type,
                v_w_bytes,
                v_w_ggml_type,
                v_b,
                out_w_bytes,
                out_w_ggml_type,
                out_b,
            )
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn try_encoder_ffn_block_f32(
        x: &[f32],
        seq_len: usize,
        n_state: usize,
        ln_w: &[f32],
        ln_b: &[f32],
        w0_bytes: &[u8],
        w0_ggml_type: u32,
        b0: &[f32],
        w1_bytes: &[u8],
        w1_ggml_type: u32,
        b1: &[f32],
    ) -> Option<Vec<f32>> {
        with_context(|ctx| {
            ctx.encoder_ffn_block_f32(
                x,
                seq_len,
                n_state,
                ln_w,
                ln_b,
                w0_bytes,
                w0_ggml_type,
                b0,
                w1_bytes,
                w1_ggml_type,
                b1,
            )
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn try_encoder_layer_f32(
        x: &[f32],
        seq_len: usize,
        n_state: usize,
        n_head: usize,
        attn_ln_w: &[f32],
        attn_ln_b: &[f32],
        q_w_bytes: &[u8],
        q_w_ggml_type: u32,
        q_b: &[f32],
        k_w_bytes: &[u8],
        k_w_ggml_type: u32,
        v_w_bytes: &[u8],
        v_w_ggml_type: u32,
        v_b: &[f32],
        out_w_bytes: &[u8],
        out_w_ggml_type: u32,
        out_b: &[f32],
        mlp_ln_w: &[f32],
        mlp_ln_b: &[f32],
        w0_bytes: &[u8],
        w0_ggml_type: u32,
        b0: &[f32],
        w1_bytes: &[u8],
        w1_ggml_type: u32,
        b1: &[f32],
    ) -> Option<Vec<f32>> {
        with_context(|ctx| {
            ctx.encoder_layer_f32(
                x,
                seq_len,
                n_state,
                n_head,
                attn_ln_w,
                attn_ln_b,
                q_w_bytes,
                q_w_ggml_type,
                q_b,
                k_w_bytes,
                k_w_ggml_type,
                v_w_bytes,
                v_w_ggml_type,
                v_b,
                out_w_bytes,
                out_w_ggml_type,
                out_b,
                mlp_ln_w,
                mlp_ln_b,
                w0_bytes,
                w0_ggml_type,
                b0,
                w1_bytes,
                w1_ggml_type,
                b1,
            )
        })
    }

    pub(super) fn try_encoder_stack_f32(
        x: &[f32],
        seq_len: usize,
        n_state: usize,
        n_head: usize,
        layers: &[EncoderLayer],
        final_ln_w: &[f32],
        final_ln_b: &[f32],
    ) -> Option<Vec<f32>> {
        with_context(|ctx| {
            ctx.encoder_stack_f32(x, seq_len, n_state, n_head, layers, final_ln_w, final_ln_b)
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn try_decoder_cross_ffn_step_f32(
        layer_idx: usize,
        x: &[f32],
        n_state: usize,
        n_head: usize,
        k_cross: &[f32],
        v_cross: &[f32],
        n_audio_ctx: usize,
        layer: &DecoderLayer,
    ) -> Option<Vec<f32>> {
        with_context(|ctx| {
            ctx.decoder_cross_ffn_step_f32(
                layer_idx,
                x,
                n_state,
                n_head,
                k_cross,
                v_cross,
                n_audio_ctx,
                layer,
            )
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn try_decoder_self_cross_ffn_step_f32(
        layer_idx: usize,
        x: &[f32],
        q_self: &[f32],
        k_all: &[f32],
        v_all: &[f32],
        n_kv: usize,
        n_state: usize,
        n_head: usize,
        k_cross: &[f32],
        v_cross: &[f32],
        n_audio_ctx: usize,
        layer: &DecoderLayer,
    ) -> Option<Vec<f32>> {
        with_context(|ctx| {
            ctx.decoder_self_cross_ffn_step_f32(
                layer_idx,
                x,
                q_self,
                k_all,
                v_all,
                n_kv,
                n_state,
                n_head,
                k_cross,
                v_cross,
                n_audio_ctx,
                layer,
            )
        })
    }
}
