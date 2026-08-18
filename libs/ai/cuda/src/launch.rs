#[cfg(all(any(target_os = "linux", target_os = "windows"), makepad_ai_cuda_kernels))]
mod imp {
    use crate::accel::{AffineQuantizedMatmulRowsSpec, AffineQuantizedMatmulSpec};
    use crate::quant::{
        block_elements, block_size, h3_nvfp4_pairs_bytes, quantize_bf16_to_q8_1,
        quantize_f32_to_q8_1, GGML_TYPE_BF16, GGML_TYPE_F16, GGML_TYPE_H3_NVFP4_PAIRS,
        GGML_TYPE_F8_E4M3, GGML_TYPE_H3_NVFP4_PAIRS_PRESCALE, GGML_TYPE_NVFP4,
        GGML_TYPE_Q4_0, GGML_TYPE_Q4_K, GGML_TYPE_Q6_K, QK, QK_NVFP4,
    };
    use crate::{cudaError_t, cudaStream_t};
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::ffi::c_void;
    use std::ptr::NonNull;
    use std::rc::Rc;

    pub use crate::{CudaGraph, CudaGraphExec};

    unsafe extern "C" {
        fn makepad_cuda_affine_qmv_bf16(
            input_bf16_words: *const u16,
            packed_weights_u32: *const u32,
            scales_bf16_words: *const u16,
            biases_bf16_words: *const u16,
            output_bf16_words: *mut u16,
            n_in: u32,
            weight_words_per_row: u32,
            qparams_per_row: u32,
            out_rows: u32,
            bits: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_affine_qmv_f32(
            input_bf16_words: *const u16,
            packed_weights_u32: *const u32,
            scales_bf16_words: *const u16,
            biases_bf16_words: *const u16,
            output_f32: *mut f32,
            n_in: u32,
            weight_words_per_row: u32,
            qparams_per_row: u32,
            out_rows: u32,
            bits: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_affine_qmv_f32_precise(
            input_bf16_words: *const u16,
            packed_weights_u32: *const u32,
            scales_bf16_words: *const u16,
            biases_bf16_words: *const u16,
            output_f32: *mut f32,
            n_in: u32,
            weight_words_per_row: u32,
            qparams_per_row: u32,
            out_rows: u32,
            bits: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_affine_qmv_f32_rows_precise(
            input_bf16_words: *const u16,
            packed_weights_u32: *const u32,
            scales_bf16_words: *const u16,
            biases_bf16_words: *const u16,
            output_f32: *mut f32,
            n_in: u32,
            weight_words_per_row: u32,
            qparams_per_row: u32,
            out_rows: u32,
            input_rows: u32,
            bits: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_affine_q8_1_qmv_f32_precise(
            input_bf16_words: *const u16,
            input_q8_1_bytes: *const u8,
            packed_weights_u32: *const u32,
            scales_bf16_words: *const u16,
            biases_bf16_words: *const u16,
            output_f32: *mut f32,
            n_in: u32,
            weight_words_per_row: u32,
            qparams_per_row: u32,
            out_rows: u32,
            bits: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_affine_qmv_f32_select_plane_precise(
            input_bf16_words: *const u16,
            packed_weights_u32: *const u32,
            scales_bf16_words: *const u16,
            biases_bf16_words: *const u16,
            plane_indices_u32: *const u32,
            plane_slot: u32,
            output_f32: *mut f32,
            n_in: u32,
            weight_words_per_row: u32,
            qparams_per_row: u32,
            out_rows: u32,
            weight_words_per_plane: u32,
            qparams_words_per_plane: u32,
            plane_count: u32,
            bits: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_affine_qmv_f32_select_plane_rows_precise(
            input_bf16_words: *const u16,
            packed_weights_u32: *const u32,
            scales_bf16_words: *const u16,
            biases_bf16_words: *const u16,
            plane_indices_u32: *const u32,
            plane_indices_row_stride: u32,
            plane_slot: u32,
            output_f32: *mut f32,
            n_in: u32,
            weight_words_per_row: u32,
            qparams_per_row: u32,
            out_rows: u32,
            input_rows: u32,
            weight_words_per_plane: u32,
            qparams_words_per_plane: u32,
            plane_count: u32,
            bits: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_affine_qmv_f32_select_planes_precise(
            input_bf16_words: *const u16,
            packed_weights_u32: *const u32,
            scales_bf16_words: *const u16,
            biases_bf16_words: *const u16,
            plane_indices_u32: *const u32,
            selected_count: u32,
            output_f32: *mut f32,
            n_in: u32,
            weight_words_per_row: u32,
            qparams_per_row: u32,
            out_rows: u32,
            weight_words_per_plane: u32,
            qparams_words_per_plane: u32,
            plane_count: u32,
            bits: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_affine_qmv_f32_select_planes_fixed8_known_valid_precise(
            input_bf16_words: *const u16,
            packed_weights_u32: *const u32,
            scales_bf16_words: *const u16,
            biases_bf16_words: *const u16,
            plane_indices_u32: *const u32,
            output_f32: *mut f32,
            n_in: u32,
            weight_words_per_row: u32,
            qparams_per_row: u32,
            out_rows: u32,
            weight_words_per_plane: u32,
            qparams_words_per_plane: u32,
            bits: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_affine_qmv_f32_select_planes_input_offsets_precise(
            input_bf16_words: *const u16,
            input_words_per_slot: u32,
            packed_weights_u32: *const u32,
            scales_bf16_words: *const u16,
            biases_bf16_words: *const u16,
            plane_indices_u32: *const u32,
            selected_count: u32,
            output_f32: *mut f32,
            n_in: u32,
            weight_words_per_row: u32,
            qparams_per_row: u32,
            out_rows: u32,
            weight_words_per_plane: u32,
            qparams_words_per_plane: u32,
            plane_count: u32,
            bits: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_affine_qmv_f32_select_planes_input_offsets_fixed8_known_valid_precise(
            input_bf16_words: *const u16,
            input_words_per_slot: u32,
            packed_weights_u32: *const u32,
            scales_bf16_words: *const u16,
            biases_bf16_words: *const u16,
            plane_indices_u32: *const u32,
            output_f32: *mut f32,
            n_in: u32,
            weight_words_per_row: u32,
            qparams_per_row: u32,
            out_rows: u32,
            weight_words_per_plane: u32,
            qparams_words_per_plane: u32,
            bits: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_affine_get_row_f32(
            packed_weights_u32: *const u32,
            scales_bf16_words: *const u16,
            biases_bf16_words: *const u16,
            output_f32: *mut f32,
            weight_words_per_row: u32,
            qparams_per_row: u32,
            row_index: u32,
            bits: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_affine_get_row_f32_device_u32(
            packed_weights_u32: *const u32,
            scales_bf16_words: *const u16,
            biases_bf16_words: *const u16,
            output_f32: *mut f32,
            weight_words_per_row: u32,
            qparams_per_row: u32,
            row_index_device_u32: *const u32,
            bits: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_nvfp4_q8_1_matvec(
            input_q8_1_bytes: *const u8,
            packed_weights_nvfp4_bytes: *const u8,
            output_f32: *mut f32,
            q8_1_blocks: u32,
            out_rows: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_nvfp4_q8_1_matmul(
            input_q8_1_bytes: *const u8,
            packed_weights_nvfp4_bytes: *const u8,
            output_f32: *mut f32,
            q8_1_blocks: u32,
            out_rows: u32,
            input_rows: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_nvfp4_nvfp4_matvec(
            input_nvfp4_bytes: *const u8,
            packed_weights_nvfp4_bytes: *const u8,
            input_scale: f32,
            output_f32: *mut f32,
            nvfp4_blocks: u32,
            out_rows: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_nvfp4_nvfp4_matmul(
            input_nvfp4_bytes: *const u8,
            packed_weights_nvfp4_bytes: *const u8,
            input_scale: f32,
            output_f32: *mut f32,
            nvfp4_blocks: u32,
            out_rows: u32,
            input_rows: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_nvfp4_q8_1_mmq_matmul(
            input_q8_1_mmq_bytes: *const u8,
            packed_weights_nvfp4_bytes: *const u8,
            output_f32: *mut f32,
            tmp_fixup_f32: *mut f32,
            tmp_fixup_f32_len: u32,
            n_cols: u32,
            out_rows: u32,
            input_rows: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_nvfp4_q8_1_mmq_fixup_f32_len(len_out: *mut u32) -> cudaError_t;

        fn makepad_cuda_nvfp4_get_row_f32(
            packed_weights_nvfp4_bytes: *const u8,
            output_f32: *mut f32,
            n_cols: u32,
            row_index: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_nvfp4_get_row_f32_device_u32(
            packed_weights_nvfp4_bytes: *const u8,
            output_f32: *mut f32,
            n_cols: u32,
            row_index_device_u32: *const u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_nvfp4_get_rows_f32_device_u32(
            packed_weights_nvfp4_bytes: *const u8,
            row_indices_device_u32: *const u32,
            output_f32: *mut f32,
            n_cols: u32,
            row_count: u32,
            output_row_stride: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_quantize_q8_1_f32(
            input_f32: *const f32,
            output_q8_1_bytes: *mut u8,
            n: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_quantize_q8_1_mmq_f32(
            input_f32: *const f32,
            output_q8_1_mmq_bytes: *mut u8,
            n_cols: u32,
            n_rows: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_quantize_q8_1_mmq_f32_padded(
            input_f32: *const f32,
            output_q8_1_mmq_bytes: *mut u8,
            n_cols: u32,
            n_rows: u32,
            padded_rows: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_quantize_nvfp4_f32(
            input_f32: *const f32,
            input_scale: f32,
            output_nvfp4_bytes: *mut u8,
            n: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        // kquants.cu: bulk dense dequantization into bf16 scratch for the
        // quantized H3 linear path (see gpu_linear_nt_impl).
        fn makepad_cuda_dequant_q4_k_bf16(
            src_blocks: *const std::ffi::c_void,
            dst_bf16: *mut std::ffi::c_void,
            n_super_blocks: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_dequant_q6_k_bf16(
            src_blocks: *const std::ffi::c_void,
            dst_bf16: *mut std::ffi::c_void,
            n_super_blocks: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_dequant_q4_0_bf16(
            src_blocks: *const std::ffi::c_void,
            dst_bf16: *mut std::ffi::c_void,
            n_blocks: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_dequant_nvfp4_pairs_bf16(
            packed_blob: *const std::ffi::c_void,
            dst_bf16: *mut std::ffi::c_void,
            rows: u32,
            cols: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        // kquants.cu: raw signed E4M3FN scalars -> bf16 scratch (dense linear
        // dequant, exact conversion) and gathered f32 embedding rows for the
        // FLUX combined-FP8 checkpoints.
        fn makepad_cuda_dequant_f8_e4m3_bf16(
            src_bytes: *const std::ffi::c_void,
            dst_bf16: *mut std::ffi::c_void,
            count: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_quant_bf16_f8_e4m3(
            src_bf16: *const std::ffi::c_void,
            dst_bytes: *mut std::ffi::c_void,
            inv_scale: f32,
            count: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_get_rows_f8_e4m3_f32(
            src_bytes: *const std::ffi::c_void,
            row_indices_i32: *const std::ffi::c_void,
            dst_f32: *mut std::ffi::c_void,
            n_cols: u32,
            n_take: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_scale_f32_inplace(
            values: *mut f32,
            scale: f32,
            n: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_scale_f32_inplace_device_f32_index(
            values: *mut f32,
            scales: *const f32,
            scale_index: u32,
            n: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_f32_to_bf16(
            input: *const f32,
            output: *mut u16,
            n: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_bf16_to_f32(
            input: *const u16,
            output: *mut f32,
            n: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_skintokens_michelangelo_fourier_f32(
            condition: *const f32,
            output: *mut f32,
            rows: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_add_f32(
            left: *const f32,
            right: *const f32,
            out: *mut f32,
            n: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_copy_f32(
            input: *const f32,
            output: *mut f32,
            n: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_weighted_sum_rows_f32(
            batched_inputs: *const f32,
            weights: *const f32,
            output: *mut f32,
            row_count: u32,
            input_count: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_weighted_sum_rows_grouped_f32(
            batched_inputs: *const f32,
            weights: *const f32,
            output: *mut f32,
            row_count: u32,
            row_stride: u32,
            input_count: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_add_scaled_rows_f32(
            input: *const f32,
            scales: *const f32,
            output: *mut f32,
            row_count: u32,
            row_stride: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_add_scaled_rows_f32_indexed(
            input: *const f32,
            scales: *const f32,
            output: *mut f32,
            row_count: u32,
            row_stride: u32,
            scale_row_stride: u32,
            scale_column: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_mul_f32(
            left: *const f32,
            right: *const f32,
            out: *mut f32,
            n: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_gelu_f32(
            input: *const f32,
            out: *mut f32,
            n: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_geglu_split_f32(
            gate_up: *const f32,
            out: *mut f32,
            n: u32,
            split_offset: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_geglu_split_f32_rows(
            gate_up: *const f32,
            out: *mut f32,
            row_count: u32,
            row_stride: u32,
            n: u32,
            split_offset: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_ssm_conv_f32(
            src0: *const f32,
            src1: *const f32,
            dst: *mut f32,
            d_conv: u32,
            d_inner: u32,
            n_tokens: u32,
            n_seqs: u32,
            src0_token_stride: u32,
            src0_seq_stride: u32,
            src1_inner_stride: u32,
            dst_token_stride: u32,
            dst_seq_stride: u32,
            apply_silu: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_gated_delta_net_f32(
            q: *const f32,
            k: *const f32,
            v: *const f32,
            g: *const f32,
            beta: *const f32,
            state: *const f32,
            dst: *mut f32,
            sv: u32,
            h: u32,
            n_tokens: u32,
            n_seqs: u32,
            sq1: u32,
            sq2: u32,
            sq3: u32,
            sv1: u32,
            sv2: u32,
            sv3: u32,
            sb1: u32,
            sb2: u32,
            sb3: u32,
            neqk1: u32,
            rq3: u32,
            kda: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_rms_norm_row_weighted_f32(
            input: *const f32,
            weights_bf16: *const u16,
            output: *mut f32,
            n: u32,
            eps: f32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_rms_norm_row_weighted_f32_f32weights(
            input: *const f32,
            weights_f32: *const f32,
            output: *mut f32,
            n: u32,
            eps: f32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_rms_norm_row_weighted_f32_f32weights_precise(
            input: *const f32,
            weights_f32: *const f32,
            output: *mut f32,
            n: u32,
            eps: f32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_rms_norm_rows_weighted_f32(
            input: *const f32,
            weights_bf16: *const u16,
            output: *mut f32,
            row_count: u32,
            row_stride: u32,
            n: u32,
            eps: f32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_rms_norm_rows_weighted_f32_f32weights(
            input: *const f32,
            weights_f32: *const f32,
            output: *mut f32,
            row_count: u32,
            row_stride: u32,
            n: u32,
            eps: f32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_rms_norm_rows_weighted_f32_f32weights_precise(
            input: *const f32,
            weights_f32: *const f32,
            output: *mut f32,
            row_count: u32,
            row_stride: u32,
            n: u32,
            eps: f32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_rms_norm_qwen3(
            input: *const f32,
            weights_f32: *const f32,
            output: *mut f32,
            row_count: u32,
            row_stride: u32,
            n: u32,
            eps: f32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_rms_norm_rows_no_scale_f32(
            input: *const f32,
            output: *mut f32,
            row_count: u32,
            row_stride: u32,
            n: u32,
            eps: f32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_rms_norm_rows_no_scale_f32_precise(
            input: *const f32,
            output: *mut f32,
            row_count: u32,
            row_stride: u32,
            n: u32,
            eps: f32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_rope_rows_f32(
            input: *const f32,
            output: *mut f32,
            row_count: u32,
            row_stride: u32,
            head_dim: u32,
            rotary_dim: u32,
            base: f32,
            position: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_rope_rows_f32_device_u32(
            input: *const f32,
            output: *mut f32,
            row_count: u32,
            row_stride: u32,
            head_dim: u32,
            rotary_dim: u32,
            base: f32,
            position_device_u32: *const u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_kv_append_f32(
            keys: *const f32,
            values: *const f32,
            key_cache: *mut u16,
            value_cache: *mut u16,
            kv_head_count: u32,
            head_dim: u32,
            max_tokens: u32,
            slot: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_kv_append_f32_device_u32(
            keys: *const f32,
            values: *const f32,
            key_cache: *mut u16,
            value_cache: *mut u16,
            kv_head_count: u32,
            head_dim: u32,
            max_tokens: u32,
            slot_device_u32: *const u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_qkv_norm_rope_cache_f32(
            qkv: *const f32,
            q_weights_bf16: *const u16,
            k_weights_bf16: *const u16,
            q_out: *mut f32,
            key_cache: *mut u16,
            value_cache: *mut u16,
            q_head_count: u32,
            k_head_count: u32,
            head_dim: u32,
            q_offset: u32,
            k_offset: u32,
            v_offset: u32,
            rotary_dim: u32,
            base: f32,
            position: u32,
            eps: f32,
            max_tokens: u32,
            slot: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_qkv_norm_rope_cache_rows_f32(
            qkv: *const f32,
            q_weights_bf16: *const u16,
            k_weights_bf16: *const u16,
            q_out: *mut f32,
            key_cache: *mut u16,
            value_cache: *mut u16,
            q_head_count: u32,
            k_head_count: u32,
            head_dim: u32,
            qkv_row_stride: u32,
            q_out_row_stride: u32,
            q_offset: u32,
            k_offset: u32,
            v_offset: u32,
            rotary_dim: u32,
            base: f32,
            start_position: u32,
            eps: f32,
            max_tokens: u32,
            start_slot: u32,
            row_count: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_qkv_norm_rope_cache_f32_device_u32(
            qkv: *const f32,
            q_weights_bf16: *const u16,
            k_weights_bf16: *const u16,
            q_out: *mut f32,
            key_cache: *mut u16,
            value_cache: *mut u16,
            q_head_count: u32,
            k_head_count: u32,
            head_dim: u32,
            q_offset: u32,
            k_offset: u32,
            v_offset: u32,
            rotary_dim: u32,
            base: f32,
            position_device_u32: *const u32,
            eps: f32,
            max_tokens: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_qkv_norm_rope_cache_rows_f32_device_u32(
            qkv: *const f32,
            q_weights_bf16: *const u16,
            k_weights_bf16: *const u16,
            q_out: *mut f32,
            key_cache: *mut u16,
            value_cache: *mut u16,
            q_head_count: u32,
            k_head_count: u32,
            head_dim: u32,
            qkv_row_stride: u32,
            q_out_row_stride: u32,
            q_offset: u32,
            k_offset: u32,
            v_offset: u32,
            rotary_dim: u32,
            base: f32,
            start_position_device_u32: *const u32,
            eps: f32,
            max_tokens: u32,
            start_slot_device_u32: *const u32,
            row_count: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_attention_logits_seq_f32(
            q: *const f32,
            key_cache: *const u16,
            logits: *mut f32,
            q_head_count: u32,
            q_heads_per_kv: u32,
            head_dim: u32,
            kv_row_stride: u32,
            seq_len: u32,
            start_slot: u32,
            capacity: u32,
            logits_row_stride: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_attention_logits_seq_f32_device_u32(
            q: *const f32,
            key_cache: *const u16,
            logits: *mut f32,
            q_head_count: u32,
            q_heads_per_kv: u32,
            head_dim: u32,
            kv_row_stride: u32,
            seq_len_device_u32: *const u32,
            start_slot_device_u32: *const u32,
            capacity: u32,
            logits_row_stride: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_softmax_rows_f32(
            logits: *const f32,
            probs: *mut f32,
            row_count: u32,
            row_stride: u32,
            seq_len: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_softmax_rows_f32_device_u32(
            logits: *const f32,
            probs: *mut f32,
            row_count: u32,
            row_stride: u32,
            seq_len_device_u32: *const u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_softmax_rows_causal_f32(
            logits: *mut f32,
            query_count: u32,
            row_count: u32,
            row_stride: u32,
            base_seq_len: u32,
            max_seq_len: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_softmax_rows_causal_f32_device_u32(
            logits: *mut f32,
            query_count: u32,
            row_count: u32,
            row_stride: u32,
            base_seq_len_device_u32: *const u32,
            max_seq_len: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_softmax_rows_causal_bf16(
            logits: *const f32,
            probs: *mut u16,
            query_count: u32,
            row_count: u32,
            row_stride: u32,
            base_seq_len: u32,
            max_seq_len: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_softmax_rows_causal_bf16_device_u32(
            logits: *const f32,
            probs: *mut u16,
            query_count: u32,
            row_count: u32,
            row_stride: u32,
            base_seq_len_device_u32: *const u32,
            max_seq_len: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_softmax_rows_causal_vision_bf16(
            logits: *const f32,
            probs: *mut u16,
            query_count: u32,
            row_count: u32,
            row_stride: u32,
            base_seq_len: u32,
            max_seq_len: u32,
            chunk_start_position: u32,
            vision_start_position: u32,
            vision_end_position: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_attention_weighted_sum_f32(
            probs: *const f32,
            value_cache: *const u16,
            out: *mut f32,
            q_head_count: u32,
            q_heads_per_kv: u32,
            head_dim: u32,
            kv_row_stride: u32,
            seq_len: u32,
            start_slot: u32,
            capacity: u32,
            probs_row_stride: u32,
            out_row_stride: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_attention_softmax_weighted_sum_f32(
            logits: *const f32,
            value_cache: *const u16,
            out: *mut f32,
            q_head_count: u32,
            q_heads_per_kv: u32,
            head_dim: u32,
            kv_row_stride: u32,
            seq_len: u32,
            start_slot: u32,
            capacity: u32,
            logits_row_stride: u32,
            out_row_stride: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_attention_weighted_sum_f32_device_u32(
            probs: *const f32,
            value_cache: *const u16,
            out: *mut f32,
            q_head_count: u32,
            q_heads_per_kv: u32,
            head_dim: u32,
            kv_row_stride: u32,
            seq_len_device_u32: *const u32,
            capacity: u32,
            probs_row_stride: u32,
            out_row_stride: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_attention_softmax_weighted_sum_f32_device_u32(
            logits: *const f32,
            value_cache: *const u16,
            out: *mut f32,
            q_head_count: u32,
            q_heads_per_kv: u32,
            head_dim: u32,
            kv_row_stride: u32,
            seq_len_device_u32: *const u32,
            start_slot_device_u32: *const u32,
            capacity: u32,
            logits_row_stride: u32,
            out_row_stride: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_attention_seq_softmax_weighted_sum_f32(
            q: *const f32,
            key_cache: *const u16,
            value_cache: *const u16,
            out: *mut f32,
            q_head_count: u32,
            q_heads_per_kv: u32,
            head_dim: u32,
            kv_row_stride: u32,
            seq_len: u32,
            start_slot: u32,
            capacity: u32,
            out_row_stride: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_attention_seq_softmax_weighted_sum_rows_f32(
            q: *const f32,
            key_cache: *const u16,
            value_cache: *const u16,
            out: *mut f32,
            query_count: u32,
            q_head_count: u32,
            q_heads_per_kv: u32,
            head_dim: u32,
            kv_row_stride: u32,
            q_row_stride: u32,
            out_row_stride: u32,
            base_seq_len: u32,
            capacity: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_attention_seq_softmax_weighted_sum_f32_device_u32(
            q: *const f32,
            key_cache: *const u16,
            value_cache: *const u16,
            out: *mut f32,
            q_head_count: u32,
            q_heads_per_kv: u32,
            head_dim: u32,
            kv_row_stride: u32,
            seq_len_device_u32: *const u32,
            capacity: u32,
            out_row_stride: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_attention_seq_softmax_weighted_sum_rows_f32_device_u32(
            q: *const f32,
            key_cache: *const u16,
            value_cache: *const u16,
            out: *mut f32,
            query_count: u32,
            q_head_count: u32,
            q_heads_per_kv: u32,
            head_dim: u32,
            kv_row_stride: u32,
            q_row_stride: u32,
            out_row_stride: u32,
            base_seq_len_device_u32: *const u32,
            capacity: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_flash_attn_f32_packed(
            q: *const f32,
            k: *const f32,
            v: *const f32,
            out: *mut f32,
            seq_len: u32,
            num_heads: u32,
            head_dim: u32,
            scale: f32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_argmax_f32(
            logits: *const f32,
            out_index: *mut u32,
            n: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_masked_argmax_f32(
            logits: *const f32,
            disallowed_token_ids: *const u32,
            disallowed_count: u32,
            out_index: *mut u32,
            n: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_masked_argmax_f32_device_u32(
            logits: *const f32,
            disallowed_token_ids: *const u32,
            disallowed_count_device_u32: *const u32,
            out_index: *mut u32,
            n: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        // diffusion_ops.cu — precise f32 kernels for the diffusion lazy path.
        fn makepad_cuda_layer_norm_mul_add_f32(
            input: *const f32,
            gamma: *const f32,
            beta: *const f32,
            output: *mut f32,
            row_count: u32,
            cols: u32,
            eps: f32,
            gamma_add: f32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_layer_norm_pytorch_f32(
            input: *const f32,
            gamma: *const f32,
            beta: *const f32,
            output: *mut f32,
            row_count: u32,
            cols: u32,
            eps: f32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_gated_residual_vec_f32(
            residual: *const f32,
            update: *const f32,
            gate: *const f32,
            output: *mut f32,
            row_count: u32,
            cols: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_f32_to_bf16_rn_f16(
            input: *const f32,
            output: *mut u16,
            n: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_gated_residual_vec_round_bf16_f32(
            residual: *const f32,
            update: *const f32,
            gate: *const f32,
            output: *mut f32,
            row_count: u32,
            cols: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_snake_cols_f32(
            input: *const f32,
            alpha: *const f32,
            output: *mut f32,
            row_count: u32,
            cols: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_rpb_expand_f32(
            ry: *const f32,
            rx: *const f32,
            bias: *mut f32,
            queries: u32,
            height: u32,
            width: u32,
            heads: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_add_f32_precise(
            left: *const f32,
            right: *const f32,
            output: *mut f32,
            n: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_mul_f32_precise(
            left: *const f32,
            right: *const f32,
            output: *mut f32,
            n: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_mul_rows_vec_f32(
            input: *const f32,
            vec: *const f32,
            output: *mut f32,
            row_count: u32,
            cols: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_gelu_f32_precise(
            input: *const f32,
            output: *mut f32,
            n: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_softmax_rows_precise_f32(
            logits: *const f32,
            probs: *mut f32,
            row_count: u32,
            row_stride: u32,
            seq_len: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_softmax_rows_motion_text_f32(
            logits: *const f32,
            probs: *mut f32,
            row_count: u32,
            row_stride: u32,
            seq_len: u32,
            motion_tokens: u32,
            band_radius: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_softmax_rows_sliding_f32(
            logits: *const f32,
            probs: *mut f32,
            row_count: u32,
            row_stride: u32,
            seq_len: u32,
            window: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_snake_rows_f32(
            input: *const f32,
            alpha: *const f32,
            inv_beta: *const f32,
            output: *mut f32,
            rows: u32,
            cols: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_tconv_stitch_f32(
            y_hi: *const f32,
            y_lo: *const f32,
            output: *mut f32,
            in_len: u32,
            out_len: u32,
            out_ch: u32,
            stride: u32,
            padding: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_f32_to_f16(
            input: *const f32,
            output: *mut u16,
            n: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_add_rows_vec_f32(
            input: *const f32,
            vec: *const f32,
            output: *mut f32,
            row_count: u32,
            cols: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_bf16_round_f32(
            input: *const f32,
            output: *mut f32,
            n: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_rms_norm_weighted_precise_round_bf16(
            input: *const f32,
            weights_f32: *const f32,
            output: *mut f32,
            row_count: u32,
            row_stride: u32,
            n: u32,
            eps: f32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_adaln_mod_f32(
            normed: *const f32,
            mods_scale: *const f32,
            mods_shift: *const f32,
            output: *mut f32,
            row_count: u32,
            cols: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_rope_half_round_bf16_f32(
            input: *const f32,
            cos_table: *const f32,
            sin_table: *const f32,
            output: *mut f32,
            token_count: u32,
            head_count: u32,
            head_dim: u32,
            rot_half: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_gated_residual_round_add_bf16_f32(
            h: *const f32,
            update: *const f32,
            gate: *const f32,
            output: *mut f32,
            row_count: u32,
            cols: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_silu_round_mul_round_bf16_f32(
            gate: *const f32,
            up: *const f32,
            output: *mut f32,
            n: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_f32_to_bf16_rn(
            input: *const f32,
            output: *mut u16,
            n: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_add_bf16_f32(
            left: *const f32,
            right: *const f32,
            output: *mut f32,
            n: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_rope_interleaved_f32(
            input: *const f32,
            cos_table: *const f32,
            sin_table: *const f32,
            output: *mut f32,
            token_count: u32,
            head_count: u32,
            half_dim: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_copy_submatrix_f32(
            src: *const f32,
            dst: *mut f32,
            src_stride: u32,
            dst_stride: u32,
            row_count: u32,
            cols: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_rms_norm_perhead_f32(
            input: *const f32,
            weights: *const f32,
            output: *mut f32,
            row_count: u32,
            n: u32,
            heads: u32,
            eps: f32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_gelu_erf_f32(
            input: *const f32,
            output: *mut f32,
            total: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_gather_rows_colblock_f32(
            src: *const f32,
            row_idx: *const u32,
            colblock_idx: *const u32,
            output: *mut f32,
            out_rows: u32,
            src_row_stride: u32,
            block_cols: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_gather_cols_f32(
            src: *const f32,
            col_idx: *const u32,
            output: *mut f32,
            rows: u32,
            src_cols: u32,
            out_cols: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_rope_half_f32(
            input: *const f32,
            cos_table: *const f32,
            sin_table: *const f32,
            output: *mut f32,
            token_count: u32,
            head_count: u32,
            head_dim: u32,
            rot_half: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_rope_half_bf16_f32(
            input: *const f32,
            cos_table: *const f32,
            sin_table: *const f32,
            output: *mut f32,
            token_count: u32,
            head_count: u32,
            head_dim: u32,
            rot_half: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_beam_cache_reorder_append_f32(
            prior: *const f32,
            step: *const f32,
            parents: *const u32,
            output: *mut f32,
            prior_sequence: u32,
            output_beams: u32,
            cols: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_attention_gqa_decode_bf16_f32(
            query: *const f32,
            key: *const f32,
            value: *const f32,
            output: *mut f32,
            beams: u32,
            sequence: u32,
            query_heads: u32,
            kv_heads: u32,
            head_dim: u32,
            scale: f32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_attention_gqa_decode_pair_bf16_f32(
            query: *const f32,
            key0: *const f32,
            value0: *const f32,
            key1: *const f32,
            value1: *const f32,
            dots: *mut f32,
            output: *mut f32,
            sequence: u32,
            query_heads: u32,
            kv_heads: u32,
            head_dim: u32,
            scale: f32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_geglu_tanh_value_gate_f32(
            input: *const f32,
            output: *mut f32,
            rows: u32,
            n: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_dyt_f32(
            input: *const f32,
            gamma: *const f32,
            beta: *const f32,
            output: *mut f32,
            group_rows: u32,
            width: u32,
            alpha: f32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_softcap_addmask_f32(
            scores: *mut f32,
            key_mask: *const f32,
            rows: u32,
            cols: u32,
            softcap: f32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_rope_half_f16(
            input: *const u16,
            cos_table: *const f32,
            sin_table: *const f32,
            output: *mut u16,
            token_count: u32,
            head_count: u32,
            head_dim: u32,
            rot_half: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_rms_norm_mod_indexed_f32(
            input: *const f32,
            weight: *const f32,
            table: *const f32,
            idx: *const u32,
            output: *mut f32,
            rows: u32,
            cols: u32,
            table_stride: u32,
            scale_off: u32,
            shift_off: u32,
            eps: f32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_rms_norm_mod_indexed_out16(
            input: *const f32,
            weight: *const f32,
            table: *const f32,
            idx: *const u32,
            output: *mut u16,
            rows: u32,
            cols: u32,
            table_stride: u32,
            scale_off: u32,
            shift_off: u32,
            eps: f32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_gated_residual_indexed_f32(
            residual: *const f32,
            update: *const f32,
            table: *const f32,
            idx: *const u32,
            output: *mut f32,
            rows: u32,
            cols: u32,
            table_stride: u32,
            gate_off: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_swiglu_value_gate_f32(
            input: *const f32,
            output: *mut f32,
            rows: u32,
            n: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_swiglu_value_gate_f16(
            input: *const u16,
            output: *mut u16,
            rows: u32,
            n: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_layer_norm_mul_add_f32_out_bf16(
            input: *const f32,
            gamma: *const f32,
            beta: *const f32,
            output: *mut u16,
            row_count: u32,
            cols: u32,
            eps: f32,
            gamma_add: f32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_bf16_slab_to_f32(
            input: *const u16,
            output: *mut f32,
            rows: u32,
            in_stride: u32,
            col_off: u32,
            cols: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_swiglu_gate_first_bf16slab(
            input: *const u16,
            output: *mut u16,
            rows: u32,
            in_stride: u32,
            gate_offset: u32,
            n: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_concat_f32rn_bf16(
            a: *const f32,
            b: *const u16,
            output: *mut u16,
            rows: u32,
            a_cols: u32,
            b_cols: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_rms_norm_weighted_bf16slab_f32(
            input: *const u16,
            weights_f32: *const f32,
            output: *mut f32,
            group_count: u32,
            groups_per_row: u32,
            in_stride: u32,
            col_off: u32,
            n: u32,
            eps: f32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_swiglu_gate_first_strided_f32(
            input: *const f32,
            output: *mut f32,
            rows: u32,
            in_stride: u32,
            gate_offset: u32,
            n: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_wavenet_gate_f32(
            input: *const f32,
            output: *mut f32,
            rows: u32,
            n: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_alias_snake_updown2x_f32(
            input: *const f32,
            params: *const f32,
            output: *mut f32,
            t_in: u32,
            ch: u32,
            input_scale: f32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_conv2d_planar_f32(
            input: *const f32,
            weights: *const f32,
            bias: *const f32,
            output: *mut f32,
            width: u32,
            height: u32,
            in_channels: u32,
            out_channels: u32,
            kw: u32,
            kh: u32,
            pad_x: u32,
            pad_y: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_conv2d_planar_strided_f32(
            input: *const f32,
            weights: *const f32,
            bias: *const f32,
            output: *mut f32,
            in_width: u32,
            in_height: u32,
            out_width: u32,
            out_height: u32,
            in_channels: u32,
            out_channels: u32,
            kw: u32,
            kh: u32,
            pad_x: u32,
            pad_y: u32,
            stride_x: u32,
            stride_y: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_group_norm_planar_f32(
            input: *const f32,
            gamma: *const f32,
            beta: *const f32,
            stats: *mut f32,
            output: *mut f32,
            width: u32,
            height: u32,
            channels: u32,
            groups: u32,
            eps: f32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_silu_f32_precise(
            input: *const f32,
            output: *mut f32,
            n: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_upsample2x_planar_f32(
            src: *const f32,
            dst: *mut f32,
            width: u32,
            height: u32,
            channels: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_pad_planar_f32_to_f16(
            src: *const f32,
            dst: *mut u16,
            width: u32,
            height: u32,
            channels: u32,
            pad_x: u32,
            pad_y: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_add_planes_vec_f32(
            data: *mut f32,
            vec: *const f32,
            plane: u32,
            channels: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_conv_extract_bias_f32(
            acc: *const f32,
            bias: *const f32,
            out: *mut f32,
            width: u32,
            height: u32,
            padded_width: u32,
            padded_plane: u32,
            out_channels: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_f16_to_f32_precise(
            input: *const u16,
            output: *mut f32,
            n: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_f16_bias_to_f32(
            input: *const u16,
            bias: *const f32,
            output: *mut f32,
            row_count: u32,
            cols: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_flash_attention_f32(
            q: *const u16,
            k: *const u16,
            v: *const u16,
            out: *mut f32,
            seq: u32,
            head_count: u32,
            hidden: u32,
            scale: f32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_flash_attention2_f32(
            q: *const u16,
            k: *const u16,
            v: *const u16,
            out: *mut f32,
            seq: u32,
            head_count: u32,
            hidden: u32,
            scale: f32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_flash_attention2_cross_f32(
            q: *const u16,
            k: *const u16,
            v: *const u16,
            out: *mut f32,
            q_len: u32,
            kv_len: u32,
            head_count: u32,
            hidden: u32,
            scale: f32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_flash_attention2_causal_f32(
            q: *const u16,
            k: *const u16,
            v: *const u16,
            out: *mut f32,
            seq: u32,
            head_count: u32,
            hidden: u32,
            scale: f32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_flash_attention2_causal_bf16(
            q: *const u16,
            k: *const u16,
            v: *const u16,
            out: *mut f32,
            seq: u32,
            head_count: u32,
            hidden: u32,
            scale: f32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_flash_attention2_sliding_bf16(
            q: *const u16,
            k: *const u16,
            v: *const u16,
            out: *mut f32,
            seq: u32,
            head_count: u32,
            hidden: u32,
            scale: f32,
            window: i32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_flash_attention2_cross_bf16(
            q: *const u16,
            k: *const u16,
            v: *const u16,
            out: *mut f32,
            q_len: u32,
            kv_len: u32,
            head_count: u32,
            hidden: u32,
            scale: f32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_flash_attention_bf16_d64_f32(
            q: *const u16,
            k: *const u16,
            v: *const u16,
            out: *mut f32,
            q_len: u32,
            kv_len: u32,
            head_count: u32,
            hidden: u32,
            scale: f32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_sdpa_flash_f16_d64(
            q: *const u16,
            k: *const u16,
            v: *const u16,
            out: *mut u16,
            batch: u32,
            q_len: u32,
            kv_len: u32,
            heads: u32,
            q_batch_stride: u32,
            k_batch_stride: u32,
            v_batch_stride: u32,
            o_batch_stride: u32,
            q_head_stride: u32,
            k_head_stride: u32,
            v_head_stride: u32,
            o_head_stride: u32,
            q_row_stride: u32,
            k_row_stride: u32,
            v_row_stride: u32,
            o_row_stride: u32,
            scale: f32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_sdpa_flash_f16_d64v128(
            q: *const u16,
            k: *const u16,
            v: *const u16,
            o0: *mut u16,
            o1: *mut u16,
            batch: u32,
            q_len: u32,
            kv_len: u32,
            heads: u32,
            q_batch_stride: u32,
            k_batch_stride: u32,
            v_batch_stride: u32,
            o_batch_stride: u32,
            q_head_stride: u32,
            k_head_stride: u32,
            v_head_stride: u32,
            o_head_stride: u32,
            q_row_stride: u32,
            k_row_stride: u32,
            v_row_stride: u32,
            o_row_stride: u32,
            scale: f32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_gather27_f16(
            src: *const f32,
            neighbors: *const u32,
            out: *mut u16,
            row0: u32,
            rows: u32,
            n_total: u32,
            ci: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_rms_norm_rows_weighted_f16(
            input: *const u16,
            weights_f32: *const f32,
            output: *mut u16,
            row_count: u32,
            row_stride: u32,
            n: u32,
            eps: f32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_rope_interleaved_f16(
            input: *const u16,
            cos_table: *const f32,
            sin_table: *const f32,
            output: *mut u16,
            token_count: u32,
            head_count: u32,
            half_dim: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_layer_norm_mul_add_f32_out16(
            input: *const f32,
            gamma: *const f32,
            beta: *const f32,
            output: *mut u16,
            row_count: u32,
            cols: u32,
            eps: f32,
            gamma_add: f32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_gelu_f16(
            input: *const u16,
            bias: *const f32,
            output: *mut u16,
            row_count: u32,
            cols: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_f16_bias_inplace(
            data: *mut u16,
            bias: *const f32,
            row_count: u32,
            cols: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_im2col_planar_f32_to_f16(
            input: *const f32,
            output: *mut u16,
            width: u32,
            height: u32,
            kw: u32,
            kh: u32,
            pad_x: u32,
            pad_y: u32,
            p0: u32,
            m_chunk: u32,
            k_total: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_birefnet_relu_f32(
            input: *const f32,
            output: *mut f32,
            n: usize,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_realesrgan_lrelu_f32(
            input: *const f32,
            output: *mut f32,
            n: usize,
            slope: f32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_realesrgan_scale_add_f32(
            base: *const f32,
            delta: *const f32,
            output: *mut f32,
            n: usize,
            scale: f32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_realesrgan_bias_lrelu_f16(
            data: *mut u16,
            bias: *const f32,
            plane: usize,
            n: usize,
            slope: f32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_realesrgan_bias_lrelu_f32(
            data: *mut f32,
            bias: *const f32,
            plane: usize,
            n: usize,
            slope: f32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_realesrgan_spine_axpb(
            base: *const f32,
            delta32: *const f32,
            delta16: *const u16,
            bias: *const f32,
            dst32: *mut f32,
            dst16: *mut u16,
            plane: usize,
            n: usize,
            scale: f32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_realesrgan_quantize_rgb8_f32(
            input: *const f32,
            output: *mut u8,
            plane: usize,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_birefnet_resize_bilinear_f32(
            input: *const f32,
            output: *mut f32,
            in_width: u32,
            in_height: u32,
            out_width: u32,
            out_height: u32,
            channels: u32,
            align_corners: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_birefnet_tokens_to_planar_f32(
            input: *const f32,
            output: *mut f32,
            tokens: u32,
            channels: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_paint_transpose_f32(
            input: *const f32,
            output: *mut f32,
            rows: u32,
            cols: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_paint_ref_attn_wide_v_f32(
            q: *const f32,
            k: *const f32,
            v_alb: *const f32,
            v_mr: *const f32,
            o_alb: *mut f32,
            o_mr: *mut f32,
            q_len: u32,
            kv_len: u32,
            hidden: u32,
            heads: u32,
            scale: f32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_paint_attn_batched_self_f32(
            q: *const f32,
            k: *const f32,
            v: *const f32,
            out: *mut f32,
            batch: u32,
            seq: u32,
            hidden: u32,
            heads: u32,
            scale: f32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_paint_scale_f32(
            input: *const f32,
            output: *mut f32,
            scale: f32,
            n: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_paint_pose_rope_f32(
            x: *const f32,
            xyz: *const u32,
            out: *mut f32,
            seq: u32,
            hidden: u32,
            heads: u32,
            voxel_res: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_paint_pack_heads_f32(
            input: *const f32,
            output: *mut f32,
            batch: u32,
            seq: u32,
            heads: u32,
            head_dim: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_paint_unpack_heads_f32(
            input: *const f32,
            output: *mut f32,
            batch: u32,
            seq: u32,
            heads: u32,
            head_dim: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_paint_gn_batched_f32(
            input: *const f32,
            gamma: *const f32,
            beta: *const f32,
            stats: *mut f32,
            output: *mut f32,
            width: u32,
            height: u32,
            channels: u32,
            groups: u32,
            batch: u32,
            eps: f32,
            stream: cudaStream_t,
        ) -> cudaError_t;



        fn makepad_cuda_pixel_shuffle_planar_f32(
            input: *const f32,
            bias: *const f32,
            output: *mut f32,
            in_width: u32,
            in_height: u32,
            out_channels: u32,
            scale: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_birefnet_image_to_patches_f32(
            input: *const f32,
            output: *mut f32,
            image_width: u32,
            image_height: u32,
            out_width: u32,
            out_height: u32,
            channels: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_birefnet_global_avg_pool_f32(
            input: *const f32,
            output: *mut f32,
            plane: u32,
            channels: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_birefnet_broadcast_f32(
            input: *const f32,
            output: *mut f32,
            plane: u32,
            channels: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_birefnet_mul_sigmoid_mask_f32(
            input: *const f32,
            logits: *const f32,
            output: *mut f32,
            plane: u32,
            channels: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_birefnet_deform_im2col_f32_to_f16(
            input: *const f32,
            offset: *const f32,
            modulator: *const f32,
            output: *mut u16,
            width: u32,
            height: u32,
            channels: u32,
            kernel: u32,
            padding: u32,
            p0: u32,
            m_chunk: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_birefnet_swin_attention_f32(
            q: *const f32,
            k: *const f32,
            v: *const f32,
            relative_bias: *const f32,
            regions: *const u32,
            output: *mut f32,
            windows: u32,
            heads: u32,
            window_tokens: u32,
            head_dim: u32,
            scale: f32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_group_norm_planar_multi_f32(
            input: *const f32,
            gamma: *const f32,
            beta: *const f32,
            partials: *mut f64,
            stats: *mut f32,
            output: *mut f32,
            width: u32,
            height: u32,
            channels: u32,
            groups: u32,
            chunk_count: u32,
            eps: f32,
            stream: cudaStream_t,
        ) -> cudaError_t;
    }

    struct DeviceBuffer {
        ptr: NonNull<c_void>,
        size_bytes: usize,
    }

    impl DeviceBuffer {
        fn new(size_bytes: usize) -> Result<Self, String> {
            let ptr = unsafe { crate::malloc(size_bytes) }.map_err(|err| err.to_string())?;
            Ok(Self { ptr, size_bytes })
        }

        fn write(&self, bytes: &[u8], stream: cudaStream_t) -> Result<(), String> {
            self.write_at(0, bytes, stream)
        }

        fn write_at(&self, offset_bytes: usize, bytes: &[u8], stream: cudaStream_t) -> Result<(), String> {
            let end = offset_bytes
                .checked_add(bytes.len())
                .ok_or_else(|| "CUDA buffer write offset overflow".to_string())?;
            if end > self.size_bytes {
                return Err(format!(
                    "CUDA buffer overflow on write: {end} > {}",
                    self.size_bytes
                ));
            }
            unsafe {
                let dst = NonNull::new_unchecked(self.ptr.as_ptr().cast::<u8>().add(offset_bytes).cast::<c_void>());
                crate::memcpy_async_host_to_device(
                    dst,
                    bytes.as_ptr().cast::<c_void>(),
                    bytes.len(),
                    stream,
                )
                .map_err(|err| err.to_string())
            }
        }

        fn read_u16_words(
            &self,
            len_words: usize,
            stream: cudaStream_t,
        ) -> Result<Vec<u16>, String> {
            let len_bytes = len_words
                .checked_mul(size_of::<u16>())
                .ok_or_else(|| "CUDA output byte count overflow".to_string())?;
            if len_bytes > self.size_bytes {
                return Err(format!(
                    "CUDA buffer overflow on read: {} > {}",
                    len_bytes, self.size_bytes
                ));
            }
            let mut out = vec![0u16; len_words];
            unsafe {
                crate::memcpy_async_device_to_host(
                    out.as_mut_ptr().cast::<c_void>(),
                    self.ptr,
                    len_bytes,
                    stream,
                )
                .map_err(|err| err.to_string())?;
                crate::synchronize_stream(stream).map_err(|err| err.to_string())?;
            }
            Ok(out)
        }

        fn read_f32s(&self, len_values: usize, stream: cudaStream_t) -> Result<Vec<f32>, String> {
            self.read_f32s_at(0, len_values, stream)
        }

        fn read_f32s_at(
            &self,
            offset_values: usize,
            len_values: usize,
            stream: cudaStream_t,
        ) -> Result<Vec<f32>, String> {
            let offset_bytes = offset_values
                .checked_mul(size_of::<f32>())
                .ok_or_else(|| "CUDA output offset overflow".to_string())?;
            let len_bytes = len_values
                .checked_mul(size_of::<f32>())
                .ok_or_else(|| "CUDA output byte count overflow".to_string())?;
            let end = offset_bytes
                .checked_add(len_bytes)
                .ok_or_else(|| "CUDA output range overflow".to_string())?;
            if end > self.size_bytes {
                return Err(format!(
                    "CUDA buffer overflow on read: {end} > {}",
                    self.size_bytes
                ));
            }
            let mut out = vec![0f32; len_values];
            unsafe {
                let src = NonNull::new_unchecked(self.ptr.as_ptr().cast::<u8>().add(offset_bytes).cast::<c_void>());
                crate::memcpy_async_device_to_host(
                    out.as_mut_ptr().cast::<c_void>(),
                    src,
                    len_bytes,
                    stream,
                )
                .map_err(|err| err.to_string())?;
                crate::synchronize_stream(stream).map_err(|err| err.to_string())?;
            }
            Ok(out)
        }

        fn read_bytes(&self, len_bytes: usize, stream: cudaStream_t) -> Result<Vec<u8>, String> {
            if len_bytes > self.size_bytes {
                return Err(format!(
                    "CUDA buffer overflow on read: {} > {}",
                    len_bytes, self.size_bytes
                ));
            }
            let mut out = vec![0u8; len_bytes];
            unsafe {
                crate::memcpy_async_device_to_host(
                    out.as_mut_ptr().cast::<c_void>(),
                    self.ptr,
                    len_bytes,
                    stream,
                )
                .map_err(|err| err.to_string())?;
                crate::synchronize_stream(stream).map_err(|err| err.to_string())?;
            }
            Ok(out)
        }

        fn read_u32s(&self, len_values: usize, stream: cudaStream_t) -> Result<Vec<u32>, String> {
            let len_bytes = len_values
                .checked_mul(size_of::<u32>())
                .ok_or_else(|| "CUDA output byte count overflow".to_string())?;
            if len_bytes > self.size_bytes {
                return Err(format!(
                    "CUDA buffer overflow on read: {} > {}",
                    len_bytes, self.size_bytes
                ));
            }
            let mut out = vec![0u32; len_values];
            unsafe {
                crate::memcpy_async_device_to_host(
                    out.as_mut_ptr().cast::<c_void>(),
                    self.ptr,
                    len_bytes,
                    stream,
                )
                .map_err(|err| err.to_string())?;
                crate::synchronize_stream(stream).map_err(|err| err.to_string())?;
            }
            Ok(out)
        }
    }

    impl Drop for DeviceBuffer {
        fn drop(&mut self) {
            let _ = unsafe { crate::free(self.ptr) };
        }
    }

    pub struct CudaMappedHostU32Buffer {
        host_ptr: NonNull<u32>,
        device_ptr: NonNull<c_void>,
        len: usize,
    }

    impl CudaMappedHostU32Buffer {
        fn new(len: usize) -> Result<Self, String> {
            let size_bytes = len
                .checked_mul(size_of::<u32>())
                .ok_or_else(|| "CUDA mapped u32 buffer size overflow".to_string())?;
            let host_ptr = unsafe { crate::host_alloc_mapped(size_bytes) }
                .map_err(|err| err.to_string())?;
            let device_ptr = unsafe { crate::host_get_device_pointer(host_ptr) }
                .map_err(|err| err.to_string())?;
            unsafe {
                std::ptr::write_bytes(host_ptr.as_ptr(), 0, len);
            }
            Ok(Self {
                host_ptr: host_ptr.cast::<u32>(),
                device_ptr,
                len,
            })
        }

        pub fn device_u32_ptr(&self) -> *const u32 {
            self.device_ptr.as_ptr().cast::<u32>()
        }

        pub fn device_u32_mut_ptr(&self) -> *mut u32 {
            self.device_ptr.as_ptr().cast::<u32>()
        }

        pub fn write_u32(&self, index: usize, value: u32) -> Result<(), String> {
            if index >= self.len {
                return Err(format!(
                    "CUDA mapped u32 buffer overflow on write: {} >= {}",
                    index, self.len
                ));
            }
            unsafe {
                *self.host_ptr.as_ptr().add(index) = value;
            }
            Ok(())
        }

        pub fn read_u32(&self, index: usize) -> Result<u32, String> {
            if index >= self.len {
                return Err(format!(
                    "CUDA mapped u32 buffer overflow on read: {} >= {}",
                    index, self.len
                ));
            }
            Ok(unsafe { *self.host_ptr.as_ptr().add(index) })
        }
    }

    impl Drop for CudaMappedHostU32Buffer {
        fn drop(&mut self) {
            let _ = unsafe { crate::free_host(self.host_ptr.cast::<c_void>()) };
        }
    }

    struct CudaAffineBackend {
        device: i32,
        stream: cudaStream_t,
        current_scope: Option<String>,
        tensor_buffers: HashMap<String, DeviceBuffer>,
        input_buffer: Option<DeviceBuffer>,
        input_capacity_words: usize,
        output_buffer: Option<DeviceBuffer>,
        output_capacity_words: usize,
    }

    impl CudaAffineBackend {
        fn load() -> Result<Self, String> {
            let device_count = crate::device_count().map_err(|err| err.to_string())?;
            if device_count <= 0 {
                return Err("CUDA reported zero devices".to_string());
            }
            let device = 0;
            crate::set_device(device).map_err(|err| err.to_string())?;
            let stream =
                crate::create_non_blocking_stream().map_err(|err| err.to_string())?;
            Ok(Self {
                device,
                stream,
                current_scope: None,
                tensor_buffers: HashMap::new(),
                input_buffer: None,
                input_capacity_words: 0,
                output_buffer: None,
                output_capacity_words: 0,
            })
        }

        fn prepare_device(&self) -> Result<(), String> {
            crate::set_device(self.device).map_err(|err| err.to_string())
        }

        fn prepare_scope(&mut self, scope: &str) {
            if self.current_scope.as_deref() != Some(scope) {
                self.current_scope = Some(scope.to_owned());
                self.tensor_buffers.clear();
            }
        }

        fn ensure_input_buffer(&mut self, len_words: usize) -> Result<&DeviceBuffer, String> {
            if self.input_capacity_words < len_words || self.input_buffer.is_none() {
                self.input_buffer = Some(DeviceBuffer::new(
                    len_words
                        .checked_mul(size_of::<u16>())
                        .ok_or_else(|| "CUDA input buffer size overflow".to_string())?,
                )?);
                self.input_capacity_words = len_words;
            }
            self.input_buffer
                .as_ref()
                .ok_or_else(|| "missing CUDA affine input buffer".to_string())
        }

        fn ensure_output_buffer(&mut self, len_words: usize) -> Result<&DeviceBuffer, String> {
            if self.output_capacity_words < len_words || self.output_buffer.is_none() {
                self.output_buffer = Some(DeviceBuffer::new(
                    len_words
                        .checked_mul(size_of::<u16>())
                        .ok_or_else(|| "CUDA output buffer size overflow".to_string())?,
                )?);
                self.output_capacity_words = len_words;
            }
            self.output_buffer
                .as_ref()
                .ok_or_else(|| "missing CUDA affine output buffer".to_string())
        }

        fn cached_tensor_buffer<F>(
            &mut self,
            key: &str,
            load_bytes: F,
        ) -> Result<&DeviceBuffer, String>
        where
            F: FnOnce() -> Result<Vec<u8>, String>,
        {
            if !self.tensor_buffers.contains_key(key) {
                let bytes = load_bytes()?;
                let buffer = DeviceBuffer::new(bytes.len())?;
                buffer.write(&bytes, self.stream)?;
                self.tensor_buffers.insert(key.to_owned(), buffer);
            }
            self.tensor_buffers
                .get(key)
                .ok_or_else(|| format!("missing cached CUDA tensor buffer {key}"))
        }

        fn matmul<FW, FS, FB>(
            &mut self,
            spec: AffineQuantizedMatmulSpec<'_>,
            weight_cache_key: &str,
            scales_cache_key: &str,
            biases_cache_key: &str,
            load_weight_bytes: FW,
            load_scales_bytes: FS,
            load_biases_bytes: FB,
        ) -> Result<Vec<f32>, String>
        where
            FW: FnOnce() -> Result<Vec<u8>, String>,
            FS: FnOnce() -> Result<Vec<u8>, String>,
            FB: FnOnce() -> Result<Vec<u8>, String>,
        {
            self.prepare_device()?;
            self.prepare_scope(spec.cache_namespace);
            let stream = self.stream;

            if spec.out_rows == 0 {
                return Ok(Vec::new());
            }

            let input_ptr = {
                let input_buffer = self.ensure_input_buffer(spec.input_bf16_words.len())?;
                input_buffer.write(u16_words_as_le_bytes(spec.input_bf16_words), stream)?;
                input_buffer.ptr.as_ptr().cast::<u16>()
            };
            let weight_ptr = {
                self.cached_tensor_buffer(weight_cache_key, load_weight_bytes)?
                    .ptr
                    .as_ptr()
                    .cast::<u32>()
            };
            let scales_ptr = {
                self.cached_tensor_buffer(scales_cache_key, load_scales_bytes)?
                    .ptr
                    .as_ptr()
                    .cast::<u16>()
            };
            let biases_ptr = {
                self.cached_tensor_buffer(biases_cache_key, load_biases_bytes)?
                    .ptr
                    .as_ptr()
                    .cast::<u16>()
            };
            let output_ptr = {
                self.ensure_output_buffer(spec.out_rows)?
                    .ptr
                    .as_ptr()
                    .cast::<u16>()
            };

            let status = unsafe {
                makepad_cuda_affine_qmv_bf16(
                    input_ptr,
                    weight_ptr,
                    scales_ptr,
                    biases_ptr,
                    output_ptr,
                    spec.input_bf16_words.len() as u32,
                    spec.weight_words_per_row as u32,
                    spec.qparams_per_row as u32,
                    spec.out_rows as u32,
                    spec.bits,
                    stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())?;
            let output_words = self
                .ensure_output_buffer(spec.out_rows)?
                .read_u16_words(spec.out_rows, stream)?;
            Ok(output_words.into_iter().map(bf16_word_to_f32).collect())
        }

        fn matmul_rows<FW, FS, FB>(
            &mut self,
            spec: AffineQuantizedMatmulRowsSpec<'_>,
            weight_cache_key: &str,
            scales_cache_key: &str,
            biases_cache_key: &str,
            load_weight_bytes: FW,
            load_scales_bytes: FS,
            load_biases_bytes: FB,
        ) -> Result<Vec<f32>, String>
        where
            FW: FnOnce() -> Result<Vec<u8>, String>,
            FS: FnOnce() -> Result<Vec<u8>, String>,
            FB: FnOnce() -> Result<Vec<u8>, String>,
        {
            self.prepare_device()?;
            self.prepare_scope(spec.cache_namespace);
            let stream = self.stream;

            if spec.input_rows == 0 || spec.out_rows == 0 {
                return Ok(Vec::new());
            }
            if spec.input_bf16_words.len() % spec.input_rows != 0 {
                return Err(format!(
                    "CUDA batched input length {} is not divisible by input_rows {}",
                    spec.input_bf16_words.len(),
                    spec.input_rows
                ));
            }

            let input_row_words = spec.input_bf16_words.len() / spec.input_rows;
            let total_output_words = spec
                .out_rows
                .checked_mul(spec.input_rows)
                .ok_or_else(|| "CUDA batched output size overflow".to_string())?;
            let input_ptr = {
                let input_buffer = self.ensure_input_buffer(spec.input_bf16_words.len())?;
                input_buffer.write(u16_words_as_le_bytes(spec.input_bf16_words), stream)?;
                input_buffer.ptr.as_ptr().cast::<u16>()
            };
            let weight_ptr = {
                self.cached_tensor_buffer(weight_cache_key, load_weight_bytes)?
                    .ptr
                    .as_ptr()
                    .cast::<u32>()
            };
            let scales_ptr = {
                self.cached_tensor_buffer(scales_cache_key, load_scales_bytes)?
                    .ptr
                    .as_ptr()
                    .cast::<u16>()
            };
            let biases_ptr = {
                self.cached_tensor_buffer(biases_cache_key, load_biases_bytes)?
                    .ptr
                    .as_ptr()
                    .cast::<u16>()
            };
            let output_ptr = {
                self.ensure_output_buffer(total_output_words)?
                    .ptr
                    .as_ptr()
                    .cast::<u16>()
            };

            for row_idx in 0..spec.input_rows {
                let status = unsafe {
                    makepad_cuda_affine_qmv_bf16(
                        input_ptr.add(row_idx * input_row_words),
                        weight_ptr,
                        scales_ptr,
                        biases_ptr,
                        output_ptr.add(row_idx * spec.out_rows),
                        input_row_words as u32,
                        spec.weight_words_per_row as u32,
                        spec.qparams_per_row as u32,
                        spec.out_rows as u32,
                        spec.bits,
                        stream,
                    )
                };
                crate::check(status).map_err(|err| err.to_string())?;
            }

            let output_words = self
                .ensure_output_buffer(total_output_words)?
                .read_u16_words(total_output_words, stream)?;
            Ok(output_words.into_iter().map(bf16_word_to_f32).collect())
        }
    }

    impl Drop for CudaAffineBackend {
        fn drop(&mut self) {
            let _ = crate::destroy_stream(self.stream);
        }
    }

    struct CudaGgmlBackend {
        device: i32,
        stream: cudaStream_t,
        current_scope: Option<String>,
        tensor_buffers: HashMap<String, DeviceBuffer>,
        input_buffer: Option<DeviceBuffer>,
        input_capacity_bytes: usize,
        output_buffer: Option<DeviceBuffer>,
        output_capacity_f32: usize,
    }

    impl CudaGgmlBackend {
        fn load() -> Result<Self, String> {
            let device_count = crate::device_count().map_err(|err| err.to_string())?;
            if device_count <= 0 {
                return Err("CUDA reported zero devices".to_string());
            }
            let device = 0;
            crate::set_device(device).map_err(|err| err.to_string())?;
            let stream =
                crate::create_non_blocking_stream().map_err(|err| err.to_string())?;
            Ok(Self {
                device,
                stream,
                current_scope: None,
                tensor_buffers: HashMap::new(),
                input_buffer: None,
                input_capacity_bytes: 0,
                output_buffer: None,
                output_capacity_f32: 0,
            })
        }

        fn prepare_device(&self) -> Result<(), String> {
            crate::set_device(self.device).map_err(|err| err.to_string())
        }

        fn prepare_scope(&mut self, scope: &str) {
            if self.current_scope.as_deref() != Some(scope) {
                self.current_scope = Some(scope.to_owned());
                self.tensor_buffers.clear();
            }
        }

        fn ensure_input_buffer_bytes(&mut self, len_bytes: usize) -> Result<&DeviceBuffer, String> {
            if self.input_capacity_bytes < len_bytes || self.input_buffer.is_none() {
                self.input_buffer = Some(DeviceBuffer::new(len_bytes)?);
                self.input_capacity_bytes = len_bytes;
            }
            self.input_buffer
                .as_ref()
                .ok_or_else(|| "missing CUDA ggml input buffer".to_string())
        }

        fn ensure_output_buffer_f32(&mut self, len_values: usize) -> Result<&DeviceBuffer, String> {
            if self.output_capacity_f32 < len_values || self.output_buffer.is_none() {
                self.output_buffer = Some(DeviceBuffer::new(
                    len_values
                        .checked_mul(size_of::<f32>())
                        .ok_or_else(|| "CUDA ggml output buffer size overflow".to_string())?,
                )?);
                self.output_capacity_f32 = len_values;
            }
            self.output_buffer
                .as_ref()
                .ok_or_else(|| "missing CUDA ggml output buffer".to_string())
        }

        fn cached_tensor_buffer<F>(
            &mut self,
            key: &str,
            load_bytes: F,
        ) -> Result<&DeviceBuffer, String>
        where
            F: FnOnce() -> Result<Vec<u8>, String>,
        {
            if !self.tensor_buffers.contains_key(key) {
                let bytes = load_bytes()?;
                let buffer = DeviceBuffer::new(bytes.len())?;
                buffer.write(&bytes, self.stream)?;
                self.tensor_buffers.insert(key.to_owned(), buffer);
            }
            self.tensor_buffers
                .get(key)
                .ok_or_else(|| format!("missing cached CUDA tensor buffer {key}"))
        }

        fn matmul_nt_ggml_bytes_cached<F>(
            &mut self,
            a: &[f32],
            bt_ggml_type: u32,
            m: usize,
            k: usize,
            n: usize,
            cache_namespace: &str,
            bt_cache_key: &str,
            load_bt_bytes: F,
        ) -> Result<Vec<f32>, String>
        where
            F: FnOnce() -> Result<Vec<u8>, String>,
        {
            if bt_ggml_type != GGML_TYPE_NVFP4 {
                return Err("CUDA ggml matmul only supports NVFP4 today".to_string());
            }
            if m != 1 {
                return Err(format!("CUDA NVFP4 matmul expects m=1, got {m}"));
            }
            if a.len() != k {
                return Err(format!(
                    "CUDA NVFP4 matmul activation length mismatch: got {} expected {k}",
                    a.len()
                ));
            }
            if k == 0 || n == 0 {
                return Ok(Vec::new());
            }
            if k % QK != 0 || k % QK_NVFP4 != 0 {
                return Err(format!(
                    "CUDA NVFP4 matmul expects k divisible by 64, got {k}"
                ));
            }

            self.prepare_device()?;
            self.prepare_scope(cache_namespace);
            let stream = self.stream;

            let input_q8_1 = quantize_f32_to_q8_1(a);
            let input_ptr = {
                let input_buffer = self.ensure_input_buffer_bytes(input_q8_1.len())?;
                input_buffer.write(&input_q8_1, stream)?;
                input_buffer.ptr.as_ptr().cast::<u8>()
            };
            let weight_ptr = {
                self.cached_tensor_buffer(bt_cache_key, load_bt_bytes)?
                    .ptr
                    .as_ptr()
                    .cast::<u8>()
            };
            let output_ptr = { self.ensure_output_buffer_f32(n)?.ptr.as_ptr().cast::<f32>() };
            let q8_1_blocks = k / QK;

            let status = unsafe {
                makepad_cuda_nvfp4_q8_1_matvec(
                    input_ptr,
                    weight_ptr,
                    output_ptr,
                    q8_1_blocks as u32,
                    n as u32,
                    stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())?;
            self.ensure_output_buffer_f32(n)?.read_f32s(n, stream)
        }

        fn matmul_nt_ggml_bytes_cached_bf16_words<F>(
            &mut self,
            input_bf16_words: &[u16],
            bt_ggml_type: u32,
            m: usize,
            k: usize,
            n: usize,
            cache_namespace: &str,
            bt_cache_key: &str,
            load_bt_bytes: F,
        ) -> Result<Vec<f32>, String>
        where
            F: FnOnce() -> Result<Vec<u8>, String>,
        {
            if bt_ggml_type != GGML_TYPE_NVFP4 {
                return Err("CUDA ggml matmul only supports NVFP4 today".to_string());
            }
            if m != 1 {
                return Err(format!("CUDA NVFP4 matmul expects m=1, got {m}"));
            }
            if input_bf16_words.len() != k {
                return Err(format!(
                    "CUDA NVFP4 matmul activation length mismatch: got {} expected {k}",
                    input_bf16_words.len()
                ));
            }
            if k == 0 || n == 0 {
                return Ok(Vec::new());
            }
            if k % QK != 0 || k % QK_NVFP4 != 0 {
                return Err(format!(
                    "CUDA NVFP4 matmul expects k divisible by 64, got {k}"
                ));
            }

            self.prepare_device()?;
            self.prepare_scope(cache_namespace);
            let stream = self.stream;

            let input_q8_1 = quantize_bf16_to_q8_1(input_bf16_words);
            let input_ptr = {
                let input_buffer = self.ensure_input_buffer_bytes(input_q8_1.len())?;
                input_buffer.write(&input_q8_1, stream)?;
                input_buffer.ptr.as_ptr().cast::<u8>()
            };
            let weight_ptr = {
                self.cached_tensor_buffer(bt_cache_key, load_bt_bytes)?
                    .ptr
                    .as_ptr()
                    .cast::<u8>()
            };
            let output_ptr = { self.ensure_output_buffer_f32(n)?.ptr.as_ptr().cast::<f32>() };
            let q8_1_blocks = k / QK;

            let status = unsafe {
                makepad_cuda_nvfp4_q8_1_matvec(
                    input_ptr,
                    weight_ptr,
                    output_ptr,
                    q8_1_blocks as u32,
                    n as u32,
                    stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())?;
            self.ensure_output_buffer_f32(n)?.read_f32s(n, stream)
        }

        fn get_rows_ggml_bytes_cached<F>(
            &mut self,
            src_ggml_type: u32,
            n_cols: usize,
            n_rows: usize,
            row_indices: &[i32],
            cache_namespace: &str,
            src_cache_key: &str,
            load_src_bytes: F,
        ) -> Result<Vec<f32>, String>
        where
            F: FnOnce() -> Result<Vec<u8>, String>,
        {
            if src_ggml_type != GGML_TYPE_NVFP4 {
                return Err("CUDA ggml get_rows only supports NVFP4 today".to_string());
            }
            if n_cols % QK_NVFP4 != 0 {
                return Err(format!(
                    "CUDA NVFP4 get_rows expects n_cols divisible by 64, got {n_cols}"
                ));
            }
            if row_indices.is_empty() {
                return Ok(Vec::new());
            }

            self.prepare_device()?;
            self.prepare_scope(cache_namespace);
            let stream = self.stream;

            let weight_ptr = {
                self.cached_tensor_buffer(src_cache_key, load_src_bytes)?
                    .ptr
                    .as_ptr()
                    .cast::<u8>()
            };
            let total_output = n_cols
                .checked_mul(row_indices.len())
                .ok_or_else(|| "CUDA NVFP4 get_rows output size overflow".to_string())?;
            let output_ptr = {
                self.ensure_output_buffer_f32(total_output)?
                    .ptr
                    .as_ptr()
                    .cast::<f32>()
            };

            for (row_slot, &row_index) in row_indices.iter().enumerate() {
                let row_index = usize::try_from(row_index)
                    .map_err(|_| format!("negative row index {}", row_index))?;
                if row_index >= n_rows {
                    return Err(format!(
                        "CUDA NVFP4 get_rows row {} out of range for {} rows",
                        row_index, n_rows
                    ));
                }
                let status = unsafe {
                    makepad_cuda_nvfp4_get_row_f32(
                        weight_ptr,
                        output_ptr.add(row_slot * n_cols),
                        n_cols as u32,
                        row_index as u32,
                        stream,
                    )
                };
                crate::check(status).map_err(|err| err.to_string())?;
            }

            self.ensure_output_buffer_f32(total_output)?
                .read_f32s(total_output, stream)
        }
    }

    impl Drop for CudaGgmlBackend {
        fn drop(&mut self) {
            let _ = crate::destroy_stream(self.stream);
        }
    }

    /// Dense (unquantized) cached-weight linear backend for the diffusion
    /// lazy path: BF16/F16 safetensors weights stay resident on the device
    /// keyed by `namespace::key`, activations are converted to the weight's
    /// half type on-device, and the matmul runs through cuBLAS GemmEx with
    /// f32 accumulation. Unlike `CudaGgmlBackend` this cache is NOT cleared
    /// on namespace switches (the flux warm loop interleaves clip/t5/unet
    /// namespaces and the unet upload is ~24GB); on an allocation failure the
    /// whole cache is dropped once and the allocation retried.
    struct CudaDenseLinearBackend {
        device: i32,
        stream: cudaStream_t,
        blas: crate::cublasHandle_t,
        blas_lt: crate::cublasLtHandle_t,
        weight_buffers: HashMap<String, DeviceBuffer>,
        input_f32: Option<DeviceBuffer>,
        input_f32_capacity_bytes: usize,
        input_half: Option<DeviceBuffer>,
        input_half_capacity_bytes: usize,
        output_f32: Option<DeviceBuffer>,
        output_f32_capacity: usize,
        /// Grow-to-max bf16 scratch for per-call dequantization of resident
        /// raw F8_E4M3 weights (the 24GB FLUX tier keeps weights 1-byte on
        /// device; the executing matrix expands transiently here only).
        dequant_bf16: Option<DeviceBuffer>,
        dequant_bf16_capacity_bytes: usize,
        /// Double-buffered pinned-host weight streamer (flux2-dev: the 8
        /// double blocks don't co-fit with the resident singles on 32GB).
        stream_ring: Option<FluxStreamRing>,
        /// Weight-cache key prefixes that must survive allocation-failure
        /// recovery: the resident FLUX checkpoint namespaces. OOM handling
        /// may drop scratch and unprotected weights, never these — a silent
        /// re-stream would violate the persistent-residency contract.
        protected_prefixes: Vec<String>,
    }

    impl CudaDenseLinearBackend {
        fn load() -> Result<Self, String> {
            let device_count = crate::device_count().map_err(|err| err.to_string())?;
            if device_count <= 0 {
                return Err("CUDA reported zero devices".to_string());
            }
            let device = 0;
            crate::set_device(device).map_err(|err| err.to_string())?;
            let stream =
                crate::create_non_blocking_stream().map_err(|err| err.to_string())?;
            let blas = match crate::cublas_create() {
                Ok(handle) => handle,
                Err(err) => {
                    let _ = crate::destroy_stream(stream);
                    return Err(format!("cuBLAS create failed: {err}"));
                }
            };
            if let Err(err) = crate::cublas_set_stream(blas, stream) {
                let _ = crate::cublas_destroy(blas);
                let _ = crate::destroy_stream(stream);
                return Err(format!("cuBLAS set stream failed: {err}"));
            }
            let blas_lt = match crate::cublas_lt_create() {
                Ok(handle) => handle,
                Err(err) => {
                    let _ = crate::cublas_destroy(blas);
                    let _ = crate::destroy_stream(stream);
                    return Err(format!("cuBLASLt create failed: {err}"));
                }
            };
            Ok(Self {
                device,
                stream,
                blas,
                blas_lt,
                weight_buffers: HashMap::new(),
                input_f32: None,
                input_f32_capacity_bytes: 0,
                input_half: None,
                input_half_capacity_bytes: 0,
                output_f32: None,
                output_f32_capacity: 0,
                dequant_bf16: None,
                dequant_bf16_capacity_bytes: 0,
                stream_ring: None,
                protected_prefixes: Vec::new(),
            })
        }

        fn prepare_device(&self) -> Result<(), String> {
            crate::set_device(self.device).map_err(|err| err.to_string())
        }

        fn alloc_with_evict(&mut self, size_bytes: usize) -> Result<DeviceBuffer, String> {
            match DeviceBuffer::new(size_bytes) {
                Ok(buffer) => Ok(buffer),
                Err(_) => {
                    // Likely device OOM. Recovery ladder: (1) idle activation
                    // pool + local scratch first — cheap, never touches model
                    // residency; (2) unprotected cached weights; protected
                    // prefixes (the resident FLUX checkpoint) are NEVER
                    // dropped here — silently re-streaming them would break
                    // the persistent-residency contract, so if freeing
                    // everything else is not enough the caller gets the
                    // allocation error and the job fails explicitly.
                    gpu_pool_clear();
                    self.input_f32 = None;
                    self.input_f32_capacity_bytes = 0;
                    self.input_half = None;
                    self.input_half_capacity_bytes = 0;
                    self.output_f32 = None;
                    self.output_f32_capacity = 0;
                    self.dequant_bf16 = None;
                    self.dequant_bf16_capacity_bytes = 0;
                    if let Ok(buffer) = DeviceBuffer::new(size_bytes) {
                        return Ok(buffer);
                    }
                    perf_count(&PERF_WEIGHT_EVICT_EVENTS, 1);
                    if self.protected_prefixes.is_empty() {
                        self.weight_buffers.clear();
                    } else {
                        let protected = std::mem::take(&mut self.protected_prefixes);
                        self.weight_buffers
                            .retain(|key, _| protected.iter().any(|p| key.starts_with(p.as_str())));
                        self.protected_prefixes = protected;
                    }
                    DeviceBuffer::new(size_bytes)
                }
            }
        }

        fn cached_weight_buffer<F>(
            &mut self,
            qualified_key: &str,
            expected_bytes: usize,
            load_bytes: F,
        ) -> Result<(), String>
        where
            F: FnOnce() -> Result<Vec<u8>, String>,
        {
            // Ring-streamed weights are device-resident via their slot; the
            // cache must not upload a duplicate copy.
            if self.ring_weight_ptr(qualified_key).is_some() {
                return Ok(());
            }
            if let Some(existing) = self.weight_buffers.get(qualified_key) {
                if existing.size_bytes == expected_bytes {
                    return Ok(());
                }
                self.weight_buffers.remove(qualified_key);
            }
            let bytes = load_bytes()?;
            if bytes.len() != expected_bytes {
                return Err(format!(
                    "CUDA dense matmul weight byte length mismatch for {qualified_key}: got {} expected {expected_bytes}",
                    bytes.len()
                ));
            }
            perf_count(&PERF_WEIGHT_STREAM_COUNT, 1);
            perf_count(&PERF_WEIGHT_STREAM_BYTES, bytes.len() as u64);
            let buffer = self.alloc_with_evict(bytes.len())?;
            buffer.write(&bytes, self.stream)?;
            // The weight bytes Vec is dropped after the async H2D copy is
            // issued; pageable-memcpy staging makes this safe (see
            // DeviceBuffer::write callers elsewhere in this module), but we
            // synchronize anyway because weight uploads are rare and large.
            crate::synchronize_stream(self.stream).map_err(|err| err.to_string())?;
            self.weight_buffers
                .insert(qualified_key.to_owned(), buffer);
            Ok(())
        }

        fn ensure_input_f32(&mut self, len_bytes: usize) -> Result<(), String> {
            if self.input_f32_capacity_bytes < len_bytes || self.input_f32.is_none() {
                let buffer = self.alloc_with_evict(len_bytes)?;
                self.input_f32 = Some(buffer);
                self.input_f32_capacity_bytes = len_bytes;
            }
            Ok(())
        }

        fn ensure_input_half(&mut self, len_bytes: usize) -> Result<(), String> {
            if self.input_half_capacity_bytes < len_bytes || self.input_half.is_none() {
                let buffer = self.alloc_with_evict(len_bytes)?;
                self.input_half = Some(buffer);
                self.input_half_capacity_bytes = len_bytes;
            }
            Ok(())
        }

        fn ensure_output_f32(&mut self, len_values: usize) -> Result<(), String> {
            if self.output_f32_capacity < len_values || self.output_f32.is_none() {
                let len_bytes = len_values
                    .checked_mul(size_of::<f32>())
                    .ok_or_else(|| "CUDA dense matmul output size overflow".to_string())?;
                let buffer = self.alloc_with_evict(len_bytes)?;
                self.output_f32 = Some(buffer);
                self.output_f32_capacity = len_values;
            }
            Ok(())
        }

        fn ensure_dequant_bf16(&mut self, len_bytes: usize) -> Result<(), String> {
            if self.dequant_bf16_capacity_bytes < len_bytes || self.dequant_bf16.is_none() {
                let buffer = self.alloc_with_evict(len_bytes)?;
                self.dequant_bf16 = Some(buffer);
                self.dequant_bf16_capacity_bytes = len_bytes;
            }
            Ok(())
        }

        #[allow(clippy::too_many_arguments)]
        fn matmul_nt_half_cached<F>(
            &mut self,
            a_f32: Option<&[f32]>,
            a_bf16_words: Option<&[u16]>,
            bt_ggml_type: u32,
            m: usize,
            k: usize,
            n: usize,
            cache_namespace: &str,
            bt_cache_key: &str,
            load_bt_bytes: F,
        ) -> Result<Vec<f32>, String>
        where
            F: FnOnce() -> Result<Vec<u8>, String>,
        {
            let half_type = match bt_ggml_type {
                GGML_TYPE_BF16 => crate::CUDA_R_16BF,
                GGML_TYPE_F16 => crate::CUDA_R_16F,
                // Raw signed E4M3FN stays 1-byte resident; the executing
                // matrix dequantizes into transient bf16 scratch below and
                // the gemm consumes the scratch as a bf16 operand.
                GGML_TYPE_F8_E4M3 => crate::CUDA_R_16BF,
                _ => {
                    return Err(format!(
                        "CUDA dense matmul: unsupported ggml type {bt_ggml_type}"
                    ));
                }
            };
            let weight_is_f8 = bt_ggml_type == GGML_TYPE_F8_E4M3;
            if m == 0 || n == 0 {
                return Ok(Vec::new());
            }
            if k == 0 {
                return Ok(vec![0.0; m * n]);
            }
            let input_len = m
                .checked_mul(k)
                .ok_or_else(|| "CUDA dense matmul input size overflow".to_string())?;
            if let Some(a) = a_f32 {
                if a.len() != input_len {
                    return Err(format!(
                        "CUDA dense matmul activation length mismatch: got {} expected {input_len}",
                        a.len()
                    ));
                }
            }
            if let Some(words) = a_bf16_words {
                if words.len() != input_len {
                    return Err(format!(
                        "CUDA dense matmul activation length mismatch: got {} expected {input_len}",
                        words.len()
                    ));
                }
                if half_type != crate::CUDA_R_16BF {
                    return Err(
                        "CUDA dense matmul: unsupported ggml type for bf16 activations"
                            .to_string(),
                    );
                }
            }
            let weight_elems = n
                .checked_mul(k)
                .ok_or_else(|| "CUDA dense matmul weight size overflow".to_string())?;
            let weight_bytes = if weight_is_f8 {
                weight_elems
            } else {
                weight_elems
                    .checked_mul(size_of::<u16>())
                    .ok_or_else(|| "CUDA dense matmul weight size overflow".to_string())?
            };

            let prof_on = crate::prof::enabled();
            let t_weight = std::time::Instant::now();
            self.prepare_device()?;
            // The dtype rides in the F8 key so a layout change can never
            // silently reuse a stale same-size half buffer (and vice versa).
            let qualified_key = if weight_is_f8 {
                format!("{cache_namespace}::{bt_cache_key}::f8")
            } else {
                format!("{cache_namespace}::{bt_cache_key}")
            };
            self.cached_weight_buffer(&qualified_key, weight_bytes, load_bt_bytes)?;
            crate::prof::record(
                crate::prof::CAT_DENSE_WEIGHT_UPLOAD,
                t_weight,
                0,
            );

            let t_upload = std::time::Instant::now();
            let h2d_bytes = if a_f32.is_some() {
                input_len * size_of::<f32>()
            } else {
                input_len * size_of::<u16>()
            };
            let input_half_bytes = input_len
                .checked_mul(size_of::<u16>())
                .ok_or_else(|| "CUDA dense matmul input size overflow".to_string())?;
            self.ensure_input_half(input_half_bytes)?;
            if let Some(a) = a_f32 {
                let input_f32_bytes = input_len * size_of::<f32>();
                self.ensure_input_f32(input_f32_bytes)?;
                let input_f32 = self
                    .input_f32
                    .as_ref()
                    .ok_or_else(|| "missing CUDA dense matmul f32 input buffer".to_string())?;
                let raw = unsafe {
                    std::slice::from_raw_parts(a.as_ptr().cast::<u8>(), input_f32_bytes)
                };
                input_f32.write(raw, self.stream)?;
                let input_half = self
                    .input_half
                    .as_ref()
                    .ok_or_else(|| "missing CUDA dense matmul half input buffer".to_string())?;
                let status = unsafe {
                    if half_type == crate::CUDA_R_16BF {
                        makepad_cuda_f32_to_bf16(
                            input_f32.ptr.as_ptr().cast::<f32>(),
                            input_half.ptr.as_ptr().cast::<u16>(),
                            input_len as u32,
                            self.stream,
                        )
                    } else {
                        makepad_cuda_f32_to_f16(
                            input_f32.ptr.as_ptr().cast::<f32>(),
                            input_half.ptr.as_ptr().cast::<u16>(),
                            input_len as u32,
                            self.stream,
                        )
                    }
                };
                crate::check(status).map_err(|err| err.to_string())?;
            } else if let Some(words) = a_bf16_words {
                let input_half = self
                    .input_half
                    .as_ref()
                    .ok_or_else(|| "missing CUDA dense matmul half input buffer".to_string())?;
                let raw = unsafe {
                    std::slice::from_raw_parts(words.as_ptr().cast::<u8>(), input_half_bytes)
                };
                input_half.write(raw, self.stream)?;
            } else {
                return Err("CUDA dense matmul requires an activation slice".to_string());
            }
            if prof_on {
                crate::synchronize_stream(self.stream).map_err(|err| err.to_string())?;
            }
            crate::prof::record(
                crate::prof::CAT_DENSE_UPLOAD,
                t_upload,
                h2d_bytes as u64,
            );

            let out_len = m
                .checked_mul(n)
                .ok_or_else(|| "CUDA dense matmul output size overflow".to_string())?;
            self.ensure_output_f32(out_len)?;
            if weight_is_f8 {
                // Scratch is (re)ensured BEFORE the borrows below; on an OOM
                // recovery this may drop unprotected weights, in which case
                // the lookup that follows fails explicitly (never silently).
                let scratch_bytes = weight_elems
                    .checked_mul(size_of::<u16>())
                    .ok_or_else(|| "CUDA dense matmul dequant scratch overflow".to_string())?;
                self.ensure_dequant_bf16(scratch_bytes)?;
            }

            let weight = self
                .weight_buffers
                .get(&qualified_key)
                .ok_or_else(|| format!("missing cached CUDA weight buffer {qualified_key}"))?;
            let input_half = self
                .input_half
                .as_ref()
                .ok_or_else(|| "missing CUDA dense matmul half input buffer".to_string())?;
            let output = self
                .output_f32
                .as_ref()
                .ok_or_else(|| "missing CUDA dense matmul output buffer".to_string())?;

            // Resident raw F8 expands into the transient bf16 scratch that
            // the gemm then reads as its (bf16) A operand; bf16/f16 weights
            // are consumed in place.
            let a_operand: *const std::ffi::c_void = if weight_is_f8 {
                let scratch = self
                    .dequant_bf16
                    .as_ref()
                    .ok_or_else(|| "missing CUDA dense matmul dequant scratch".to_string())?;
                let count = u32::try_from(weight_elems)
                    .map_err(|_| "f8_e4m3 dequant count exceeds u32".to_string())?;
                let status = unsafe {
                    makepad_cuda_dequant_f8_e4m3_bf16(
                        weight.ptr.as_ptr().cast_const(),
                        scratch.ptr.as_ptr(),
                        count,
                        self.stream,
                    )
                };
                crate::check(status).map_err(|err| err.to_string())?;
                scratch.ptr.as_ptr().cast_const()
            } else {
                weight.ptr.as_ptr().cast_const()
            };

            // Row-major C[m][n] = A[m][k] * B[n][k]^T expressed in cuBLAS
            // column-major terms, mirroring CudaRuntime::matmul_nt_f32:
            // gemm(OP_T, OP_N, m=n, n=m, k=k, A=weight ld=k, B=input ld=k,
            // C ld=n), with half inputs and f32 accumulate/output.
            let alpha = 1.0f32;
            let beta = 0.0f32;
            let t_gemm = std::time::Instant::now();
            unsafe {
                crate::cublas_gemm_strided_batched_ex(
                    self.blas,
                    crate::CUBLAS_OP_T,
                    crate::CUBLAS_OP_N,
                    n as i32,
                    m as i32,
                    k as i32,
                    &alpha,
                    a_operand,
                    half_type,
                    k as i32,
                    0,
                    input_half.ptr.as_ptr(),
                    half_type,
                    k as i32,
                    0,
                    &beta,
                    output.ptr.as_ptr(),
                    crate::CUDA_R_32F,
                    n as i32,
                    0,
                    1,
                    crate::CUBLAS_COMPUTE_32F,
                    crate::CUBLAS_GEMM_DEFAULT,
                )
                .map_err(|err| {
                    format!("cuBLAS dense NT gemm failed: m={m} k={k} n={n}: {err}")
                })?;
            }
            if prof_on {
                crate::synchronize_stream(self.stream).map_err(|err| err.to_string())?;
            }
            crate::prof::record(crate::prof::CAT_DENSE_GEMM, t_gemm, 0);

            let t_download = std::time::Instant::now();
            let result = self
                .output_f32
                .as_ref()
                .ok_or_else(|| "missing CUDA dense matmul output buffer".to_string())?
                .read_f32s(out_len, self.stream);
            crate::prof::record(
                crate::prof::CAT_DENSE_DOWNLOAD,
                t_download,
                (out_len * size_of::<f32>()) as u64,
            );
            result
        }

        /// Gathered f32 rows from a device-resident raw F8_E4M3 matrix (the
        /// T5 token embedding in the FLUX combined checkpoints): the payload
        /// stays cached 1-byte under the same `::f8` key the dense matmul
        /// uses, so warm calls upload only the row indices.
        fn get_rows_f8_cached<F>(
            &mut self,
            n_cols: usize,
            n_rows: usize,
            row_indices: &[i32],
            cache_namespace: &str,
            src_cache_key: &str,
            load_src_bytes: F,
        ) -> Result<Vec<f32>, String>
        where
            F: FnOnce() -> Result<Vec<u8>, String>,
        {
            if row_indices.is_empty() {
                return Ok(Vec::new());
            }
            for &row in row_indices {
                let row = usize::try_from(row)
                    .map_err(|_| format!("negative embedding row index {row}"))?;
                if row >= n_rows {
                    return Err(format!(
                        "embedding row {row} out of range for {n_rows} rows"
                    ));
                }
            }
            let payload_bytes = n_rows
                .checked_mul(n_cols)
                .ok_or_else(|| "f8 embedding payload overflow".to_string())?;
            let total_output = n_cols
                .checked_mul(row_indices.len())
                .ok_or_else(|| "f8 embedding gather output overflow".to_string())?;

            self.prepare_device()?;
            let qualified_key = format!("{cache_namespace}::{src_cache_key}::f8");
            self.cached_weight_buffer(&qualified_key, payload_bytes, load_src_bytes)?;
            self.ensure_output_f32(total_output)?;

            let indices_bytes = row_indices.len() * size_of::<i32>();
            let indices_buffer = self.alloc_with_evict(indices_bytes)?;
            let indices_raw = unsafe {
                std::slice::from_raw_parts(row_indices.as_ptr().cast::<u8>(), indices_bytes)
            };
            indices_buffer.write(indices_raw, self.stream)?;

            let weight = self
                .weight_buffers
                .get(&qualified_key)
                .ok_or_else(|| format!("missing cached CUDA weight buffer {qualified_key}"))?;
            let output = self
                .output_f32
                .as_ref()
                .ok_or_else(|| "missing CUDA embedding gather output buffer".to_string())?;
            let status = unsafe {
                makepad_cuda_get_rows_f8_e4m3_f32(
                    weight.ptr.as_ptr().cast_const(),
                    indices_buffer.ptr.as_ptr().cast_const(),
                    output.ptr.as_ptr(),
                    n_cols as u32,
                    row_indices.len() as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())?;
            // read_f32s synchronizes the stream, so dropping the transient
            // indices buffer afterwards is ordered behind the gather kernel.
            self.output_f32
                .as_ref()
                .ok_or_else(|| "missing CUDA embedding gather output buffer".to_string())?
                .read_f32s(total_output, self.stream)
        }
    }

    impl Drop for CudaDenseLinearBackend {
        fn drop(&mut self) {
            let _ = crate::cublas_lt_destroy(self.blas_lt);
            let _ = crate::cublas_destroy(self.blas);
            let _ = crate::destroy_stream(self.stream);
        }
    }

    // ------------------------------------------------------------------
    // Device-resident tensor API (flux device path).
    //
    // Activations stay on the GPU between ops; the only host traffic is the
    // initial upload, tiny per-block modulation vectors, and the final
    // download. Everything runs on the dense-linear backend's stream and
    // shares its BF16/F16 weight cache, so the warm host path and this path
    // never duplicate the multi-GB weight uploads.
    // ------------------------------------------------------------------

    thread_local! {
        static GPU_TENSOR_POOL: RefCell<std::collections::BTreeMap<usize, Vec<DeviceBuffer>>> =
            RefCell::new(std::collections::BTreeMap::new());
        static GPU_TENSOR_POOL_BYTES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    // ------------------------------------------------------------------
    // Perf instrumentation: eviction / weight-streaming / pool-pressure
    // counters. Cheap atomics, always on; snapshot per pipeline stage to
    // prove (or rule out) weight-cache re-streaming and VRAM-cliff churn.
    // ------------------------------------------------------------------
    static PERF_WEIGHT_EVICT_EVENTS: std::sync::atomic::AtomicU64 =
        std::sync::atomic::AtomicU64::new(0);
    static PERF_WEIGHT_STREAM_COUNT: std::sync::atomic::AtomicU64 =
        std::sync::atomic::AtomicU64::new(0);
    static PERF_WEIGHT_STREAM_BYTES: std::sync::atomic::AtomicU64 =
        std::sync::atomic::AtomicU64::new(0);
    static PERF_POOL_OOM_CLEARS: std::sync::atomic::AtomicU64 =
        std::sync::atomic::AtomicU64::new(0);
    static PERF_POOL_FRESH_ALLOC_COUNT: std::sync::atomic::AtomicU64 =
        std::sync::atomic::AtomicU64::new(0);
    static PERF_POOL_FRESH_ALLOC_BYTES: std::sync::atomic::AtomicU64 =
        std::sync::atomic::AtomicU64::new(0);
    static PERF_POOL_OVERCAP_FREE_BYTES: std::sync::atomic::AtomicU64 =
        std::sync::atomic::AtomicU64::new(0);

    fn perf_count(counter: &std::sync::atomic::AtomicU64, value: u64) {
        counter.fetch_add(value, std::sync::atomic::Ordering::Relaxed);
    }

    /// Snapshot of the backend perf counters + live device memory.
    #[derive(Default, Clone, Copy)]
    pub struct GpuPerfStats {
        /// alloc_with_evict OOM fallbacks: each one DROPS THE ENTIRE WEIGHT
        /// CACHE (every subsequent gemm re-streams its weight from host).
        pub weight_evict_events: u64,
        /// Weight-cache misses (uploads); a warm pass should stream nothing.
        pub weight_stream_count: u64,
        pub weight_stream_bytes: u64,
        /// Pool acquire OOM fallbacks (idle activation pool dropped + retry).
        pub pool_oom_clears: u64,
        /// Fresh cudaMalloc traffic through the activation pool (churn --
        /// near the WDDM residency cliff each of these can page).
        pub pool_fresh_alloc_count: u64,
        pub pool_fresh_alloc_bytes: u64,
        /// Real frees forced by the idle-pool cap.
        pub pool_overcap_free_bytes: u64,
        pub mem_free_bytes: u64,
        pub mem_total_bytes: u64,
    }

    /// Read (and optionally reset) the perf counters; mem_free/mem_total are
    /// live cudaMemGetInfo values.
    pub fn gpu_perf_stats(reset: bool) -> GpuPerfStats {
        use std::sync::atomic::Ordering::Relaxed;
        let read = |counter: &std::sync::atomic::AtomicU64| {
            if reset {
                counter.swap(0, Relaxed)
            } else {
                counter.load(Relaxed)
            }
        };
        let (mem_free_bytes, mem_total_bytes) = crate::mem_get_info()
            .map(|(free, total)| (free as u64, total as u64))
            .unwrap_or((0, 0));
        GpuPerfStats {
            weight_evict_events: read(&PERF_WEIGHT_EVICT_EVENTS),
            weight_stream_count: read(&PERF_WEIGHT_STREAM_COUNT),
            weight_stream_bytes: read(&PERF_WEIGHT_STREAM_BYTES),
            pool_oom_clears: read(&PERF_POOL_OOM_CLEARS),
            pool_fresh_alloc_count: read(&PERF_POOL_FRESH_ALLOC_COUNT),
            pool_fresh_alloc_bytes: read(&PERF_POOL_FRESH_ALLOC_BYTES),
            pool_overcap_free_bytes: read(&PERF_POOL_OVERCAP_FREE_BYTES),
            mem_free_bytes,
            mem_total_bytes,
        }
    }

    const GPU_POOL_BIG_STEP: usize = 256 * 1024 * 1024;

    thread_local! {
        static GPU_POOL_CAP_OVERRIDE: std::cell::Cell<Option<usize>> =
            const { std::cell::Cell::new(None) };
    }

    /// Temporarily override the idle-pool cap (bytes); None restores the env
    /// default. The VAE decode phase clamps the cap low: its multi-GB planes
    /// on top of the resident weights sit right at the 32GB WDDM residency
    /// cliff, where every pooled-idle GB costs ~3x in paging stalls (measured:
    /// VAE @1024 1438ms at cap 6144 -> 406ms at cap 2048), while the denoise
    /// phase runs ~3% faster with the roomier default.
    ///
    /// Lowering the cap SHRINKS the idle pool immediately (largest buffers
    /// first): a phase that ran with a roomier cap must not leave its idle
    /// GBs pinned while the next phase allocates near the VRAM ceiling
    /// (each cudaMalloc there costs ~20ms in VidMm eviction scans).
    pub fn gpu_pool_cap_override(bytes: Option<usize>) {
        GPU_POOL_CAP_OVERRIDE.with(|cell| cell.set(bytes));
        let cap = gpu_pool_cap_bytes();
        let pooled = GPU_TENSOR_POOL_BYTES.with(|total| total.get());
        if pooled <= cap {
            return;
        }
        let mut freed = 0usize;
        let _ = GPU_TENSOR_POOL.try_with(|pool| {
            let mut pool = pool.borrow_mut();
            let need = pooled - cap;
            let mut to_drop: Vec<DeviceBuffer> = Vec::new();
            let sizes: Vec<usize> = pool.keys().rev().copied().collect();
            'outer: for bucket in sizes {
                if let Some(buffers) = pool.get_mut(&bucket) {
                    let mut index = 0;
                    while index < buffers.len() {
                        if gpu_graph_pinned(&buffers[index]) {
                            index += 1;
                            continue;
                        }
                        to_drop.push(buffers.remove(index));
                        freed += bucket;
                        if freed >= need {
                            break 'outer;
                        }
                    }
                }
            }
            pool.retain(|_, buffers| !buffers.is_empty());
            drop(to_drop);
        });
        perf_count(&PERF_POOL_OVERCAP_FREE_BYTES, freed as u64);
        GPU_TENSOR_POOL_BYTES.with(|total| total.set(pooled.saturating_sub(freed)));
    }

    /// Idle (pooled) bytes above this cap are freed for real on release —
    /// without it the denoise + VAE phases together pin enough spare
    /// activation buckets to oversubscribe 32GB cards at 1024 (WDDM paging).
    fn gpu_pool_cap_bytes() -> usize {
        if let Some(value) = GPU_POOL_CAP_OVERRIDE.with(|cell| cell.get()) {
            return value;
        }
        thread_local! {
            static CAP: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
        }
        CAP.with(|cap| {
            if let Some(value) = cap.get() {
                return value;
            }
            let value = std::env::var("FLUX_GPU_POOL_CAP_MB")
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(6144)
                * 1024
                * 1024;
            cap.set(Some(value));
            value
        })
    }

    fn gpu_pool_round(min_bytes: usize) -> usize {
        let min_bytes = min_bytes.max(256);
        if min_bytes <= GPU_POOL_BIG_STEP {
            min_bytes.next_power_of_two()
        } else {
            // Gentler rounding for the multi-hundred-MB activation planes:
            // pow2 would waste up to ~2x right where VRAM is scarcest.
            min_bytes.div_ceil(GPU_POOL_BIG_STEP) * GPU_POOL_BIG_STEP
        }
    }

    thread_local! {
        // Addresses of pool buffers referenced by a live captured CUDA graph:
        // they may be REUSED freely (replays rewrite them front to back) but
        // must never be cudaFree'd while the graph exec exists.
        static GPU_GRAPH_PINNED: RefCell<std::collections::HashSet<usize>> =
            RefCell::new(std::collections::HashSet::new());
        static GPU_GRAPH_CAPTURING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    fn gpu_graph_pinned(buffer: &DeviceBuffer) -> bool {
        // try_with: during thread teardown this TLS may already be destroyed
        // (a graph-state destructor releasing tensors) — treat as unpinned.
        GPU_GRAPH_PINNED
            .try_with(|set| set.borrow().contains(&(buffer.ptr.as_ptr() as usize)))
            .unwrap_or(false)
    }

    fn gpu_pool_acquire(min_bytes: usize) -> Result<DeviceBuffer, String> {
        let rounded = gpu_pool_round(min_bytes);
        // BEST-FIT: take the smallest pooled buffer in [rounded, 2*rounded)
        // instead of requiring an exact size match. The VAE decode cycles
        // through many distinct multi-hundred-MB plane sizes; exact-match
        // pooling made nearly every op there pay a WDDM cudaMalloc+cudaFree
        // of ~1GB once the idle cap was reached (the 1024 VAE anomaly).
        let limit = rounded.saturating_mul(2);
        let reused = GPU_TENSOR_POOL.with(|pool| {
            let mut pool = pool.borrow_mut();
            let size = pool
                .range(rounded..limit)
                .find(|(_, buffers)| buffers.iter().any(|b| !gpu_graph_pinned(b)))
                .map(|(size, _)| *size);
            size.and_then(|size| {
                let buffers = pool.get_mut(&size)?;
                let idx = buffers.iter().rposition(|b| !gpu_graph_pinned(b))?;
                let buffer = buffers.remove(idx);
                if buffers.is_empty() {
                    pool.remove(&size);
                }
                Some(buffer)
            })
        });
        let buffer = if let Some(buffer) = reused {
            GPU_TENSOR_POOL_BYTES
                .with(|total| total.set(total.get().saturating_sub(buffer.size_bytes)));
            buffer
        } else {
            perf_count(&PERF_POOL_FRESH_ALLOC_COUNT, 1);
            perf_count(&PERF_POOL_FRESH_ALLOC_BYTES, rounded as u64);
            match DeviceBuffer::new(rounded) {
                Ok(buffer) => buffer,
                Err(_) => {
                    // Likely device OOM: drop every pooled activation buffer
                    // and retry once before giving up.
                    perf_count(&PERF_POOL_OOM_CLEARS, 1);
                    gpu_pool_clear();
                    DeviceBuffer::new(rounded)?
                }
            }
        };
        if GPU_GRAPH_CAPTURING.with(|flag| flag.get()) {
            GPU_GRAPH_PINNED
                .with(|set| set.borrow_mut().insert(buffer.ptr.as_ptr() as usize));
        }
        Ok(buffer)
    }

    fn gpu_pool_release(buffer: DeviceBuffer) {
        let size = buffer.size_bytes;
        let mut pooled = GPU_TENSOR_POOL_BYTES.with(|total| total.get());
        let cap = gpu_pool_cap_bytes();
        if pooled.saturating_add(size) > cap && !gpu_graph_pinned(&buffer) {
            // Over the idle cap. Freeing the INCOMING buffer forever turns
            // the pool into a revolving door once stale sizes fill the cap:
            // every hot-size acquire/release pays a real WDDM
            // cudaMalloc+cudaFree (measured on TRELLIS HR: 2549 fresh allocs
            // / 326GB churn in one warm stage). Instead, evict pooled
            // buffers largest-first until the incoming one fits — the pool
            // adapts to the current phase's working set.
            if size > cap {
                perf_count(&PERF_POOL_OVERCAP_FREE_BYTES, size as u64);
                drop(buffer);
                return;
            }
            let mut freed = 0usize;
            let evicted = GPU_TENSOR_POOL.try_with(|pool| {
                let mut pool = pool.borrow_mut();
                let need = (pooled + size).saturating_sub(cap);
                let mut to_drop: Vec<DeviceBuffer> = Vec::new();
                let sizes: Vec<usize> = pool.keys().rev().copied().collect();
                'outer: for bucket in sizes {
                    if let Some(buffers) = pool.get_mut(&bucket) {
                        let mut index = 0;
                        while index < buffers.len() {
                            if gpu_graph_pinned(&buffers[index]) {
                                index += 1;
                                continue;
                            }
                            to_drop.push(buffers.remove(index));
                            freed += bucket;
                            if freed >= need {
                                break 'outer;
                            }
                        }
                    }
                }
                pool.retain(|_, buffers| !buffers.is_empty());
                drop(to_drop); // the real cudaFrees
            });
            if evicted.is_err() {
                // TLS teardown: no pool to evict from, free for real.
                perf_count(&PERF_POOL_OVERCAP_FREE_BYTES, size as u64);
                drop(buffer);
                return;
            }
            perf_count(&PERF_POOL_OVERCAP_FREE_BYTES, freed as u64);
            pooled = pooled.saturating_sub(freed);
            GPU_TENSOR_POOL_BYTES.with(|total| total.set(pooled));
            if pooled.saturating_add(size) > cap {
                // Everything left is pinned: free the incoming buffer.
                drop(buffer);
                return;
            }
        }
        // try_with: releases can run from TLS destructors at thread teardown
        // (graph-state tensors), after the pool map itself is destroyed —
        // fall through to a plain cudaFree (the buffer drops here).
        let mut slot = Some(buffer);
        let _ = GPU_TENSOR_POOL.try_with(|pool| {
            if let Some(buffer) = slot.take() {
                pool.borrow_mut().entry(size).or_default().push(buffer);
            }
        });
        if slot.is_none() {
            GPU_TENSOR_POOL_BYTES.with(|total| total.set(pooled + size));
        }
    }

    /// Drop every idle pooled activation buffer (used at phase boundaries —
    /// e.g. denoise -> VAE decode — so the two phases' working sets never
    /// coexist in VRAM). Buffers pinned by a live captured graph are kept.
    pub fn gpu_pool_clear() {
        let has_pins = GPU_GRAPH_PINNED.with(|set| !set.borrow().is_empty());
        if has_pins {
            GPU_TENSOR_POOL.with(|pool| {
                let mut pool = pool.borrow_mut();
                let mut kept_bytes = 0usize;
                for (_, buffers) in pool.iter_mut() {
                    buffers.retain(|buffer| {
                        let keep = gpu_graph_pinned(buffer);
                        if keep {
                            kept_bytes += buffer.size_bytes;
                        }
                        keep
                    });
                }
                pool.retain(|_, buffers| !buffers.is_empty());
                GPU_TENSOR_POOL_BYTES.with(|total| total.set(kept_bytes));
            });
        } else {
            GPU_TENSOR_POOL.with(|pool| pool.borrow_mut().clear());
            GPU_TENSOR_POOL_BYTES.with(|total| total.set(0));
        }
        CONV_SCRATCH.with(|cell| *cell.borrow_mut() = [None, None]);
    }

    /// A captured denoise-step graph: replaying it re-runs the step's whole
    /// kernel sequence with one launch. Dropping it unpins the pool buffers
    /// it references.
    pub struct GpuStepGraph {
        exec: crate::CudaGraphExec,
    }

    impl Drop for GpuStepGraph {
        fn drop(&mut self) {
            // try_with: the graph state can be destroyed by TLS teardown
            // after the pinned set is already gone.
            let _ = GPU_GRAPH_PINNED.try_with(|set| set.borrow_mut().clear());
        }
    }

    /// Capture `run` (a full device-resident step whose inputs live in
    /// persistent tensors) as a CUDA graph on the dense backend's stream.
    /// The run itself is NOT executed — launch the returned graph for real
    /// work. Requirements: everything inside must be async device work on the
    /// backend stream (no gpu_download / gpu_upload of transient host data).
    pub fn gpu_graph_capture<T>(
        run: impl FnOnce() -> Result<T, String>,
    ) -> Result<(GpuStepGraph, T), String> {
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            crate::begin_stream_capture(
                backend.stream,
                crate::CUDA_STREAM_CAPTURE_MODE_RELAXED,
            )
            .map_err(|err| format!("graph capture begin failed: {err}"))
        })?;
        GPU_GRAPH_CAPTURING.with(|flag| flag.set(true));
        let result = run();
        GPU_GRAPH_CAPTURING.with(|flag| flag.set(false));
        let graph = with_dense_linear_backend(|backend| {
            crate::end_stream_capture(backend.stream)
                .map_err(|err| format!("graph capture end failed: {err}"))
        });
        match (result, graph) {
            (Ok(value), Ok(graph)) => {
                let exec = graph
                    .instantiate()
                    .map_err(|err| format!("graph instantiate failed: {err}"))?;
                Ok((GpuStepGraph { exec }, value))
            }
            (result, graph) => {
                GPU_GRAPH_PINNED.with(|set| set.borrow_mut().clear());
                drop(graph);
                match result {
                    Err(err) => Err(format!("graph capture run failed: {err}")),
                    Ok(_) => Err("graph capture failed".to_string()),
                }
            }
        }
    }

    pub fn gpu_graph_launch(graph: &GpuStepGraph) -> Result<(), String> {
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            graph
                .exec
                .launch(backend.stream)
                .map_err(|err| format!("graph launch failed: {err}"))
        })
    }

    /// Overwrite a persistent f32 tensor's contents (graph step inputs).
    pub fn gpu_upload_into(tensor: &GpuTensor, values: &[f32]) -> Result<(), String> {
        if tensor.half {
            return Err("gpu_upload_into expects an f32 tensor".to_string());
        }
        if values.len() != tensor.rows * tensor.cols {
            return Err(format!(
                "gpu_upload_into shape mismatch: {} values for {}x{}",
                values.len(),
                tensor.rows,
                tensor.cols
            ));
        }
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let raw = unsafe {
                std::slice::from_raw_parts(
                    values.as_ptr().cast::<u8>(),
                    values.len() * size_of::<f32>(),
                )
            };
            tensor
                .buf
                .as_ref()
                .ok_or_else(|| "GPU tensor already released".to_string())?
                .write_at(tensor.offset_units * size_of::<f32>(), raw, backend.stream)
        })
    }

    /// Device-to-device copy into a persistent tensor (same shape).
    pub fn gpu_copy_into(src: &GpuTensor, dst: &GpuTensor) -> Result<(), String> {
        if src.half || dst.half {
            return Err("gpu_copy_into expects f32 tensors".to_string());
        }
        if src.rows != dst.rows || src.cols != dst.cols {
            return Err(format!(
                "gpu_copy_into shape mismatch: {}x{} vs {}x{}",
                src.rows, src.cols, dst.rows, dst.cols
            ));
        }
        let bytes = src.rows * src.cols * size_of::<f32>();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            crate::check(unsafe {
                crate::cudaMemcpyAsync(
                    dst.device_ptr()?.cast(),
                    src.device_ptr()?.cast_const().cast(),
                    bytes,
                    3, // cudaMemcpyDeviceToDevice
                    backend.stream,
                )
            })
            .map_err(|err| err.to_string())
        })
    }

    thread_local! {
        static CONV_SCRATCH: RefCell<[Option<DeviceBuffer>; 2]> = const { RefCell::new([None, None]) };
    }

    /// Persistent grow-to-max scratch for the implicit-GEMM conv (padded
    /// input + accumulator): per-call pool traffic on these multi-hundred-MB
    /// buffers was pure cudaMalloc churn at the big decode stages.
    fn conv_scratch_ptr(slot: usize, min_bytes: usize) -> Result<*mut c_void, String> {
        CONV_SCRATCH.with(|cell| {
            let mut slots = cell.borrow_mut();
            let need_new = match &slots[slot] {
                Some(buffer) => buffer.size_bytes < min_bytes,
                None => true,
            };
            if need_new {
                slots[slot] = None;
                let rounded = gpu_pool_round(min_bytes);
                let buffer = match DeviceBuffer::new(rounded) {
                    Ok(buffer) => buffer,
                    Err(_) => {
                        GPU_TENSOR_POOL.with(|pool| pool.borrow_mut().clear());
                        GPU_TENSOR_POOL_BYTES.with(|total| total.set(0));
                        DeviceBuffer::new(rounded)?
                    }
                };
                slots[slot] = Some(buffer);
            }
            Ok(slots[slot]
                .as_ref()
                .expect("conv scratch was just ensured")
                .ptr
                .as_ptr())
        })
    }

    /// MAKEPAD_GPU_PROF=1 attribution for the device-resident ops: sync the
    /// stream so the elapsed time covers the op's GPU work, then record.
    fn gpu_prof(stream: cudaStream_t, cat: usize, start: std::time::Instant, bytes: u64) {
        if crate::prof::enabled() {
            let _ = crate::synchronize_stream(stream);
            crate::prof::record(cat, start, bytes);
        }
    }

    /// Opaque device-resident f32 matrix (row-major rows x cols).
    /// Row slices share the parent allocation (torch-style views).
    pub struct GpuTensor {
        buf: Option<Rc<DeviceBuffer>>,
        rows: usize,
        cols: usize,
        // f16 storage: activations on the gemm->attention/gelu spine stay
        // half-precision end to end (they get rounded to f16 at every gemm
        // input anyway); f32 remains the default for everything else.
        half: bool,
        /// Offset from the allocation start in storage units (f32 or f16).
        offset_units: usize,
    }

    // Rc is only for intra-thread views. Asset-ai moves a backend onto the
    // worker thread as a whole; a live GpuTensor is never shared across
    // threads, which is why ContentBackend is Send and not Sync.
    unsafe impl Send for GpuTensor {}

    impl GpuTensor {
        pub fn rows(&self) -> usize {
            self.rows
        }

        pub fn cols(&self) -> usize {
            self.cols
        }

        pub fn is_half(&self) -> bool {
            self.half
        }

        fn device_ptr(&self) -> Result<*mut f32, String> {
            let base = self
                .buf
                .as_ref()
                .ok_or_else(|| "GPU tensor already released".to_string())?
                .ptr
                .as_ptr()
                .cast::<f32>();
            Ok(unsafe { base.add(self.offset_units) })
        }

        fn device_ptr_u16(&self) -> Result<*mut u16, String> {
            let base = self
                .buf
                .as_ref()
                .ok_or_else(|| "GPU tensor already released".to_string())?
                .ptr
                .as_ptr()
                .cast::<u16>();
            Ok(unsafe { base.add(self.offset_units) })
        }

        /// Pointer at the view origin in storage units, typed as f32* for the
        /// copy kernels that take a byte-stride in those units.
        fn storage_ptr(&self) -> Result<*mut f32, String> {
            if self.half {
                Ok(self.device_ptr_u16()?.cast::<f32>())
            } else {
                self.device_ptr()
            }
        }

        fn from_pool(rows: usize, cols: usize) -> Result<Self, String> {
            let bytes = rows
                .checked_mul(cols)
                .and_then(|len| len.checked_mul(size_of::<f32>()))
                .ok_or_else(|| "GPU tensor size overflow".to_string())?;
            Ok(Self {
                buf: Some(Rc::new(gpu_pool_acquire(bytes)?)),
                rows,
                cols,
                half: false,
                offset_units: 0,
            })
        }

        fn from_pool_half(rows: usize, cols: usize) -> Result<Self, String> {
            let bytes = rows
                .checked_mul(cols)
                .and_then(|len| len.checked_mul(size_of::<u16>()))
                .ok_or_else(|| "GPU tensor size overflow".to_string())?;
            Ok(Self {
                buf: Some(Rc::new(gpu_pool_acquire(bytes)?)),
                rows,
                cols,
                half: true,
                offset_units: 0,
            })
        }

        fn same_storage(&self, other: &Self) -> bool {
            match (&self.buf, &other.buf) {
                (Some(a), Some(b)) => {
                    Rc::ptr_eq(a, b) && self.half == other.half && self.cols == other.cols
                }
                _ => false,
            }
        }
    }

    impl Drop for GpuTensor {
        fn drop(&mut self) {
            if let Some(buf) = self.buf.take() {
                if let Ok(buffer) = Rc::try_unwrap(buf) {
                    gpu_pool_release(buffer);
                }
            }
        }
    }

    /// Opaque bf16 activation buffer for the flux2 linear spine. The bf16
    /// dense gemms round every f32 input RN-even and emit bf16 outputs; a
    /// linear-to-linear segment held in this type carries bit-identical
    /// values with half the traffic and no standalone conversion passes.
    /// Deliberately NOT a GpuTensor so no generic f32/f16 op can misread
    /// the storage.
    pub struct GpuBf16Buf {
        buf: Option<Rc<DeviceBuffer>>,
        rows: usize,
        cols: usize,
    }

    impl GpuBf16Buf {
        fn from_pool(rows: usize, cols: usize) -> Result<Self, String> {
            let bytes = rows
                .checked_mul(cols)
                .and_then(|len| len.checked_mul(size_of::<u16>()))
                .ok_or_else(|| "GPU bf16 buffer size overflow".to_string())?;
            Ok(Self {
                buf: Some(Rc::new(gpu_pool_acquire(bytes)?)),
                rows,
                cols,
            })
        }

        fn device_ptr_u16(&self) -> Result<*mut u16, String> {
            Ok(self
                .buf
                .as_ref()
                .ok_or_else(|| "GPU bf16 buffer already released".to_string())?
                .ptr
                .as_ptr()
                .cast::<u16>())
        }

        pub fn rows(&self) -> usize {
            self.rows
        }

        pub fn cols(&self) -> usize {
            self.cols
        }
    }

    impl Drop for GpuBf16Buf {
        fn drop(&mut self) {
            if let Some(buf) = self.buf.take() {
                if let Ok(buffer) = Rc::try_unwrap(buf) {
                    gpu_pool_release(buffer);
                }
            }
        }
    }

    /// One weight matrix part of a (possibly column-concatenated) linear
    /// layer: `n` output features whose BF16/F16 bytes live at `bytes` and
    /// cache under `cache_key`.
    pub struct GpuLinearPart<'a> {
        pub bt_ggml_type: u32,
        pub n: usize,
        pub cache_key: &'a str,
        pub bytes: &'a [u8],
    }

    pub fn gpu_device_available() -> bool {
        is_available()
    }

    pub fn gpu_cudnn_available() -> bool {
        crate::cudnn::available()
    }

    fn gpu_check(status: cudaError_t) -> Result<(), String> {
        crate::check(status).map_err(|err| err.to_string())
    }

    /// Upload a small host vector into a pooled device buffer.
    fn gpu_upload_small(
        backend: &CudaDenseLinearBackend,
        values: &[f32],
    ) -> Result<DeviceBuffer, String> {
        let buffer = gpu_pool_acquire(values.len() * size_of::<f32>())?;
        let raw = unsafe {
            std::slice::from_raw_parts(
                values.as_ptr().cast::<u8>(),
                values.len() * size_of::<f32>(),
            )
        };
        buffer.write(raw, backend.stream)?;
        Ok(buffer)
    }

    pub fn gpu_upload(values: &[f32], rows: usize, cols: usize) -> Result<GpuTensor, String> {
        if values.len() != rows * cols {
            return Err(format!(
                "gpu_upload shape mismatch: {} values for {rows}x{cols}",
                values.len()
            ));
        }
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let tensor = GpuTensor::from_pool(rows, cols)?;
            let raw = unsafe {
                std::slice::from_raw_parts(
                    values.as_ptr().cast::<u8>(),
                    values.len() * size_of::<f32>(),
                )
            };
            tensor
                .buf
                .as_ref()
                .expect("freshly pooled GPU tensor")
                .write(raw, backend.stream)?;
            Ok(tensor)
        })
    }

    /// CUDA execution of the released Michelangelo Fourier embedder followed
    /// by normal concatenation. Keeping this model-specific primitive additive
    /// prevents CUDA libdevice trig differences from leaking into generic
    /// positional-embedding behavior.
    pub fn gpu_skintokens_michelangelo_fourier(
        condition: &GpuTensor,
    ) -> Result<GpuTensor, String> {
        if condition.half || condition.cols != 6 {
            return Err(format!(
                "gpu_skintokens_michelangelo_fourier expects f32 Nx6, got {}x{} half={}",
                condition.rows, condition.cols, condition.half,
            ));
        }
        let rows = u32::try_from(condition.rows)
            .map_err(|_| "Michelangelo Fourier row count exceeds u32".to_string())?;
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let output = GpuTensor::from_pool(condition.rows, 54)?;
            let status = unsafe {
                makepad_cuda_skintokens_michelangelo_fourier_f32(
                    condition.device_ptr()?,
                    output.device_ptr()?,
                    rows,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            Ok(output)
        })
    }

    pub fn gpu_download(tensor: &GpuTensor) -> Result<Vec<f32>, String> {
        if tensor.half {
            // Convert on device first: nothing on the host path expects f16.
            let full = gpu_to_f32(tensor)?;
            return gpu_download(&full);
        }
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            tensor
                .buf
                .as_ref()
                .ok_or_else(|| "GPU tensor already released".to_string())?
                .read_f32s_at(
                    tensor.offset_units,
                    tensor.rows * tensor.cols,
                    backend.stream,
                )
        })
    }

    /// f32 copy of an f16 tensor (device-side convert).
    pub fn gpu_to_f32(tensor: &GpuTensor) -> Result<GpuTensor, String> {
        if !tensor.half {
            return Err("gpu_to_f32 expects an f16 tensor".to_string());
        }
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(tensor.rows, tensor.cols)?;
            let status = unsafe {
                makepad_cuda_f16_to_f32_precise(
                    tensor.device_ptr_u16()?,
                    out.device_ptr()?,
                    (tensor.rows * tensor.cols) as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            Ok(out)
        })
    }

    /// f16 copy of an f32 tensor (device-side convert).
    pub fn gpu_to_f16(tensor: &GpuTensor) -> Result<GpuTensor, String> {
        if tensor.half {
            return Err("gpu_to_f16 expects an f32 tensor".to_string());
        }
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool_half(tensor.rows, tensor.cols)?;
            let status = unsafe {
                makepad_cuda_f32_to_f16(
                    tensor.device_ptr()?,
                    out.device_ptr_u16()?,
                    (tensor.rows * tensor.cols) as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            Ok(out)
        })
    }

    /// The f16 activation spine: qkv/mlp activations stay f16 between the
    /// f16-accumulate gemms and the attention/gelu consumers, skipping the
    /// convert passes and halving slice/concat/norm/rope traffic.
    /// FLUX_ACT_F16=0 restores f32 activations everywhere (requires the
    /// f16acc gemms, so FLUX_GEMM_F16ACC=0 also disables it).
    pub fn gpu_act_f16_enabled() -> bool {
        if !gpu_gemm_f16acc_enabled() {
            return false;
        }
        match std::env::var("FLUX_ACT_F16") {
            Ok(value) => value != "0",
            Err(_) => true,
        }
    }

    /// C = X @ concat(parts)^T + bias, all resident. Multi-part weights write
    /// straight into their column ranges via the gemm ldc, so no concat pass
    /// is needed.
    pub fn gpu_linear_nt_cached(
        x: &GpuTensor,
        cache_namespace: &str,
        parts: &[GpuLinearPart<'_>],
        bias: &[f32],
    ) -> Result<GpuTensor, String> {
        gpu_linear_nt_impl(x, cache_namespace, parts, bias, false, false, false)
    }

    /// BF16-weight linear with BF16 GEMM operands and f32 accumulation/output,
    /// independent of the process-wide Flux f16-acceleration switch. This is
    /// the stable execution contract for BF16 language encoders such as
    /// Qwen3, whose weights must not be silently converted to f16.
    pub fn gpu_linear_nt_cached_bf16_f32acc(
        x: &GpuTensor,
        cache_namespace: &str,
        parts: &[GpuLinearPart<'_>],
        bias: &[f32],
    ) -> Result<GpuTensor, String> {
        if parts
            .iter()
            .any(|part| part.bt_ggml_type != GGML_TYPE_BF16)
        {
            return Err("gpu_linear_nt_cached_bf16_f32acc requires BF16 weights".to_string());
        }
        gpu_linear_nt_impl(x, cache_namespace, parts, bias, false, true, false)
    }

    /// PyTorch 2.7's bias-free BF16 `nn.Linear` contract: the contiguous 3-D
    /// activation is folded to 2-D `mm`, which reaches `cublasGemmEx` with
    /// BF16 A/B/C, f32 accumulation, and `CUBLAS_GEMM_DEFAULT_TENSOR_OP`.
    /// The result is expanded to f32 storage without changing its BF16 bits.
    ///
    /// This is additive and single-part so the generic dense path and packed
    /// multi-projection geometry retain their established behavior.
    pub fn gpu_linear_nt_cached_bf16_mm(
        x: &GpuTensor,
        cache_namespace: &str,
        parts: &[GpuLinearPart<'_>],
    ) -> Result<GpuTensor, String> {
        if x.half {
            return Err("gpu_linear_nt_cached_bf16_mm expects f32 storage".to_string());
        }
        if parts.len() != 1 || parts[0].bt_ggml_type != GGML_TYPE_BF16 {
            return Err(
                "gpu_linear_nt_cached_bf16_mm requires one BF16 weight part".to_string(),
            );
        }
        let part = &parts[0];
        let m = x.rows;
        let k = x.cols;
        let n = part.n;
        if m == 0 || k == 0 || n == 0 {
            return Err(format!(
                "gpu_linear_nt_cached_bf16_mm empty shape: x={m}x{k} n={n}",
            ));
        }

        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let weight_bytes = n
                .checked_mul(k)
                .and_then(|len| len.checked_mul(size_of::<u16>()))
                .ok_or_else(|| "PyTorch BF16 mm weight size overflow".to_string())?;
            let weight_key = format!("{cache_namespace}::{}", part.cache_key);
            backend.cached_weight_buffer(&weight_key, weight_bytes, || {
                Ok(part.bytes.to_vec())
            })?;

            let output_values = m
                .checked_mul(n)
                .ok_or_else(|| "PyTorch BF16 mm output size overflow".to_string())?;
            let output_bf16 = gpu_pool_acquire(output_values * size_of::<u16>())?;
            let output = GpuTensor::from_pool(m, n)?;

            backend.ensure_input_half(m * k * size_of::<u16>())?;
            let input_bf16_ptr = backend
                .input_half
                .as_ref()
                .ok_or_else(|| "missing PyTorch BF16 mm input buffer".to_string())?
                .ptr
                .as_ptr();
            // RN-even straight to bf16 words (bit-identical to round-then-truncate).
            let status = unsafe {
                makepad_cuda_f32_to_bf16_rn(
                    x.device_ptr()?,
                    input_bf16_ptr.cast::<u16>(),
                    (m * k) as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;

            let (weight_ptr, _) = backend.weight_ptr(&weight_key)?;
            let alpha = 1.0f32;
            let beta = 0.0f32;
            // Row-major X[M,K] @ W^T[K,N] is column-major W^T[N,K] @
            // X^T[K,M] -> D^T[N,M], matching Torch's adjusted BLAS geometry.
            unsafe {
                crate::cublas_gemm_ex(
                    backend.blas,
                    crate::CUBLAS_OP_T,
                    crate::CUBLAS_OP_N,
                    n as i32,
                    m as i32,
                    k as i32,
                    &alpha,
                    weight_ptr,
                    crate::CUDA_R_16BF,
                    k as i32,
                    input_bf16_ptr,
                    crate::CUDA_R_16BF,
                    k as i32,
                    &beta,
                    output_bf16.ptr.as_ptr(),
                    crate::CUDA_R_16BF,
                    n as i32,
                    crate::CUDA_R_32F,
                    crate::CUBLAS_GEMM_DEFAULT_TENSOR_OP,
                )
            }
            .map_err(|err| format!("PyTorch BF16 mm failed: m={m} k={k} n={n}: {err}"))?;
            let status = unsafe {
                makepad_cuda_bf16_to_f32(
                    output_bf16.ptr.as_ptr().cast::<u16>(),
                    output.device_ptr()?,
                    output_values as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_pool_release(output_bf16);
            gpu_prof(
                backend.stream,
                crate::prof::CAT_DENSE_TOTAL,
                prof_start,
                0,
            );
            Ok(output)
        })
    }

    /// PyTorch 2.7's CUDA `nn.Linear` contract for a BF16 activation, weight,
    /// and bias: cuBLASLt `gemm_and_bias<BFloat16>` with a BF16 D tensor and
    /// fused bias epilogue. The returned tensor expands that BF16 D tensor to
    /// f32 storage without changing its values so it composes with the rest of
    /// the device-resident API.
    ///
    /// This is deliberately additive and single-part. Concatenating several
    /// independent linears would change Lt's N geometry and therefore its
    /// selected heuristic; bias-free PyTorch linears use the existing GemmEx
    /// path instead.
    pub fn gpu_linear_nt_cached_bf16_bias_epilogue(
        x: &GpuTensor,
        cache_namespace: &str,
        parts: &[GpuLinearPart<'_>],
        bias: &[f32],
    ) -> Result<GpuTensor, String> {
        if x.half {
            return Err(
                "gpu_linear_nt_cached_bf16_bias_epilogue expects f32 storage"
                    .to_string(),
            );
        }
        if parts.len() != 1 || parts[0].bt_ggml_type != GGML_TYPE_BF16 {
            return Err(
                "gpu_linear_nt_cached_bf16_bias_epilogue requires one BF16 weight part"
                    .to_string(),
            );
        }
        let part = &parts[0];
        let m = x.rows;
        let k = x.cols;
        let n = part.n;
        if m == 0 || k == 0 || n == 0 || bias.len() != n {
            return Err(format!(
                "gpu_linear_nt_cached_bf16_bias_epilogue shape mismatch: x={m}x{k} n={n} bias={}",
                bias.len(),
            ));
        }

        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let weight_bytes = n
                .checked_mul(k)
                .and_then(|len| len.checked_mul(size_of::<u16>()))
                .ok_or_else(|| "cuBLASLt BF16 weight size overflow".to_string())?;
            let weight_key = format!("{cache_namespace}::{}", part.cache_key);
            backend.cached_weight_buffer(&weight_key, weight_bytes, || {
                Ok(part.bytes.to_vec())
            })?;

            // The checkpoint bias is BF16. Keep a separate BF16 cache entry
            // from the generic path's expanded-f32 bias buffer.
            let bias_key = format!("{cache_namespace}::{}::b16", part.cache_key);
            backend.cached_weight_buffer(&bias_key, n * size_of::<u16>(), || {
                let mut bytes = Vec::with_capacity(n * size_of::<u16>());
                for &value in bias {
                    let bits = value.to_bits();
                    let rounded = bits.wrapping_add(0x7fff + ((bits >> 16) & 1));
                    bytes.extend_from_slice(&((rounded >> 16) as u16).to_le_bytes());
                }
                Ok(bytes)
            })?;

            let output_values = m
                .checked_mul(n)
                .ok_or_else(|| "cuBLASLt BF16 output size overflow".to_string())?;
            let output_bf16 = gpu_pool_acquire(output_values * size_of::<u16>())?;
            let workspace_size = 1024 * 1024;
            let workspace = gpu_pool_acquire(workspace_size)?;
            let output = GpuTensor::from_pool(m, n)?;

            // Allocate/convert only after every weight and transient buffer is
            // resident: a dense-cache eviction clears this staging buffer.
            backend.ensure_input_half(m * k * size_of::<u16>())?;
            let input_bf16 = backend
                .input_half
                .as_ref()
                .ok_or_else(|| "missing cuBLASLt BF16 input buffer".to_string())?;
            // RN-even straight to bf16 words (bit-identical to round-then-truncate).
            let status = unsafe {
                makepad_cuda_f32_to_bf16_rn(
                    x.device_ptr()?,
                    input_bf16.ptr.as_ptr().cast::<u16>(),
                    (m * k) as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;

            let weight = backend
                .weight_buffers
                .get(&weight_key)
                .ok_or_else(|| format!("missing cached CUDA weight buffer {weight_key}"))?;
            let bias = backend
                .weight_buffers
                .get(&bias_key)
                .ok_or_else(|| format!("missing cached CUDA bias buffer {bias_key}"))?;

            // PyTorch exposes row-major [M,K] @ [K,N], then expresses it to
            // column-major cuBLASLt as op_T(W[K,N]) @ X[K,M] -> D[N,M].
            let operation = crate::cublas_lt_matmul_desc_create(
                crate::CUBLAS_COMPUTE_32F,
                crate::CUDA_R_32F,
            )
            .map_err(|error| format!("cuBLASLt descriptor create failed: {error}"))?;
            let a_desc = match crate::cublas_lt_matrix_layout_create(
                crate::CUDA_R_16BF,
                k as u64,
                n as u64,
                k as i64,
            ) {
                Ok(desc) => desc,
                Err(error) => {
                    let _ = crate::cublas_lt_matmul_desc_destroy(operation);
                    return Err(format!("cuBLASLt A layout create failed: {error}"));
                }
            };
            let b_desc = match crate::cublas_lt_matrix_layout_create(
                crate::CUDA_R_16BF,
                k as u64,
                m as u64,
                k as i64,
            ) {
                Ok(desc) => desc,
                Err(error) => {
                    let _ = crate::cublas_lt_matrix_layout_destroy(a_desc);
                    let _ = crate::cublas_lt_matmul_desc_destroy(operation);
                    return Err(format!("cuBLASLt B layout create failed: {error}"));
                }
            };
            let c_desc = match crate::cublas_lt_matrix_layout_create(
                crate::CUDA_R_16BF,
                n as u64,
                m as u64,
                n as i64,
            ) {
                Ok(desc) => desc,
                Err(error) => {
                    let _ = crate::cublas_lt_matrix_layout_destroy(b_desc);
                    let _ = crate::cublas_lt_matrix_layout_destroy(a_desc);
                    let _ = crate::cublas_lt_matmul_desc_destroy(operation);
                    return Err(format!("cuBLASLt C layout create failed: {error}"));
                }
            };
            let preference = match crate::cublas_lt_matmul_preference_create() {
                Ok(desc) => desc,
                Err(error) => {
                    let _ = crate::cublas_lt_matrix_layout_destroy(c_desc);
                    let _ = crate::cublas_lt_matrix_layout_destroy(b_desc);
                    let _ = crate::cublas_lt_matrix_layout_destroy(a_desc);
                    let _ = crate::cublas_lt_matmul_desc_destroy(operation);
                    return Err(format!("cuBLASLt preference create failed: {error}"));
                }
            };

            let run = (|| -> Result<(), String> {
                let transpose_a = crate::CUBLAS_OP_T;
                let transpose_b = crate::CUBLAS_OP_N;
                let epilogue = crate::CUBLASLT_EPILOGUE_BIAS;
                let bias_ptr = bias.ptr.as_ptr().cast::<c_void>();
                crate::cublas_lt_matmul_desc_set_attribute(
                    operation,
                    crate::CUBLASLT_MATMUL_DESC_TRANSA,
                    &transpose_a,
                )
                .map_err(|error| format!("cuBLASLt transA attribute failed: {error}"))?;
                crate::cublas_lt_matmul_desc_set_attribute(
                    operation,
                    crate::CUBLASLT_MATMUL_DESC_TRANSB,
                    &transpose_b,
                )
                .map_err(|error| format!("cuBLASLt transB attribute failed: {error}"))?;
                crate::cublas_lt_matmul_desc_set_attribute(
                    operation,
                    crate::CUBLASLT_MATMUL_DESC_EPILOGUE,
                    &epilogue,
                )
                .map_err(|error| format!("cuBLASLt epilogue attribute failed: {error}"))?;
                crate::cublas_lt_matmul_desc_set_attribute(
                    operation,
                    crate::CUBLASLT_MATMUL_DESC_BIAS_POINTER,
                    &bias_ptr,
                )
                .map_err(|error| format!("cuBLASLt bias attribute failed: {error}"))?;

                crate::cublas_lt_matmul_preference_set_attribute(
                    preference,
                    crate::CUBLASLT_MATMUL_PREF_MAX_WORKSPACE_BYTES,
                    &workspace_size,
                )
                .map_err(|error| format!("cuBLASLt workspace preference failed: {error}"))?;
                for (attribute, pointer) in [
                    (
                        crate::CUBLASLT_MATMUL_PREF_MIN_ALIGNMENT_A_BYTES,
                        weight.ptr.as_ptr().cast_const(),
                    ),
                    (
                        crate::CUBLASLT_MATMUL_PREF_MIN_ALIGNMENT_B_BYTES,
                        input_bf16.ptr.as_ptr().cast_const(),
                    ),
                    (
                        crate::CUBLASLT_MATMUL_PREF_MIN_ALIGNMENT_C_BYTES,
                        output_bf16.ptr.as_ptr().cast_const(),
                    ),
                    (
                        crate::CUBLASLT_MATMUL_PREF_MIN_ALIGNMENT_D_BYTES,
                        // PyTorch 2.7 deliberately derives Lt's D-alignment
                        // preference from the bias pointer (not result_ptr).
                        bias.ptr.as_ptr().cast_const(),
                    ),
                ] {
                    let alignment = crate::cublas_lt_pointer_alignment(pointer);
                    crate::cublas_lt_matmul_preference_set_attribute(
                        preference,
                        attribute,
                        &alignment,
                    )
                    .map_err(|error| {
                        format!("cuBLASLt alignment preference failed: {error}")
                    })?;
                }

                let heuristic = crate::cublas_lt_matmul_algo_get_heuristic(
                    backend.blas_lt,
                    operation,
                    a_desc,
                    b_desc,
                    c_desc,
                    c_desc,
                    preference,
                )
                .map_err(|error| format!("cuBLASLt heuristic query failed: {error}"))?
                .ok_or_else(|| "cuBLASLt found no BF16+bias algorithm".to_string())?;
                let alpha = 1.0f32;
                let beta = 0.0f32;
                unsafe {
                    crate::cublas_lt_matmul(
                        backend.blas_lt,
                        operation,
                        (&alpha as *const f32).cast::<c_void>(),
                        weight.ptr.as_ptr(),
                        a_desc,
                        input_bf16.ptr.as_ptr(),
                        b_desc,
                        (&beta as *const f32).cast::<c_void>(),
                        output_bf16.ptr.as_ptr(),
                        c_desc,
                        output_bf16.ptr.as_ptr(),
                        c_desc,
                        &heuristic.algo,
                        workspace.ptr.as_ptr(),
                        workspace_size,
                        backend.stream,
                    )
                }
                .map_err(|error| format!("cuBLASLt BF16+bias matmul failed: {error}"))?;
                let status = unsafe {
                    makepad_cuda_bf16_to_f32(
                        output_bf16.ptr.as_ptr().cast::<u16>(),
                        output.device_ptr()?,
                        output_values as u32,
                        backend.stream,
                    )
                };
                gpu_check(status)
            })();

            let _ = crate::cublas_lt_matmul_preference_destroy(preference);
            let _ = crate::cublas_lt_matrix_layout_destroy(c_desc);
            let _ = crate::cublas_lt_matrix_layout_destroy(b_desc);
            let _ = crate::cublas_lt_matrix_layout_destroy(a_desc);
            let _ = crate::cublas_lt_matmul_desc_destroy(operation);
            run?;
            gpu_pool_release(workspace);
            gpu_pool_release(output_bf16);
            gpu_prof(
                backend.stream,
                crate::prof::CAT_DENSE_TOTAL,
                prof_start,
                0,
            );
            Ok(output)
        })
    }

    /// BF16 checkpoint weights converted to F16 GEMM operands with f32
    /// accumulation/output. This contract is explicit and does not inherit
    /// the process-wide Flux precision switches.
    pub fn gpu_linear_nt_cached_f16_f32acc(
        x: &GpuTensor,
        cache_namespace: &str,
        parts: &[GpuLinearPart<'_>],
        bias: &[f32],
    ) -> Result<GpuTensor, String> {
        if parts
            .iter()
            .any(|part| part.bt_ggml_type != GGML_TYPE_BF16)
        {
            return Err("gpu_linear_nt_cached_f16_f32acc requires BF16 weights".to_string());
        }
        gpu_linear_nt_impl(x, cache_namespace, parts, bias, false, false, true)
    }

    /// Like gpu_linear_nt_cached but the output STAYS f16 (the gemm's C is
    /// the result; bias is broadcast in place). Requires the f16acc gemms.
    /// Pass an empty bias to defer it into the consumer (e.g. fused gelu).
    pub fn gpu_linear_nt_cached_f16(
        x: &GpuTensor,
        cache_namespace: &str,
        parts: &[GpuLinearPart<'_>],
        bias: &[f32],
    ) -> Result<GpuTensor, String> {
        gpu_linear_nt_impl(x, cache_namespace, parts, bias, true, false, false)
    }

    fn gpu_linear_nt_impl(
        x: &GpuTensor,
        cache_namespace: &str,
        parts: &[GpuLinearPart<'_>],
        bias: &[f32],
        out_half: bool,
        force_native_bf16: bool,
        force_f16_operands: bool,
    ) -> Result<GpuTensor, String> {
        let m = x.rows;
        let k = x.cols;
        let n_total: usize = parts.iter().map(|part| part.n).sum();
        if n_total == 0 || m == 0 || k == 0 {
            return Err("gpu_linear empty shape".to_string());
        }
        if !bias.is_empty() && bias.len() != n_total {
            return Err(format!(
                "gpu_linear bias mismatch: bias={} n={n_total}",
                bias.len()
            ));
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let input_len = m * k;
            let native_type = match parts.first().map(|part| part.bt_ggml_type) {
                Some(GGML_TYPE_BF16) => crate::CUDA_R_16BF,
                Some(GGML_TYPE_F16) => crate::CUDA_R_16F,
                // Quantized weights dequantize into bf16 scratch just before
                // the gemm: the operand class (and thus the activation
                // conversion) is bf16, matching the H3 f32-accumulate spine.
                Some(ty) if gpu_quant_linear_type_supported(ty) => crate::CUDA_R_16BF,
                other => {
                    return Err(format!("gpu_linear unsupported ggml type {other:?}"));
                }
            };
            if parts
                .iter()
                .any(|part| part.bt_ggml_type != parts[0].bt_ggml_type)
            {
                return Err("gpu_linear mixed weight types".to_string());
            }
            let quant_call = gpu_quant_linear_type_supported(parts[0].bt_ggml_type);
            // f16-accumulate for the token-parallel gemms only: m == 1 is the
            // modulation projection, whose scale/shift/gate outputs are the
            // most rounding-sensitive values in the step — keep those f32.
            // Quantized parts never take the f16acc path: quantization noise
            // plus f16 accumulation was never validated for H3, and the
            // dequant scratch is bf16.
            let f16acc = !quant_call
                && !force_native_bf16
                && (force_f16_operands || gpu_gemm_f16acc_enabled())
                && m > 1;
            if (x.half || out_half) && !f16acc {
                return Err("gpu_linear f16 activations require f16acc gemms".to_string());
            }
            let half_type = if f16acc {
                crate::CUDA_R_16F
            } else {
                native_type
            };
            let part_key = |part: &GpuLinearPart<'_>| {
                if quant_call {
                    quant_part_key(cache_namespace, part.cache_key, part.bt_ggml_type)
                } else if f16acc {
                    format!("{cache_namespace}::{}::a16", part.cache_key)
                } else {
                    format!("{cache_namespace}::{}", part.cache_key)
                }
            };

            // Warm every weight part FIRST: a cold-weight upload can trigger
            // alloc_with_evict, which clears input_half — converting the
            // activations before that would silently lose them.
            for part in parts {
                let weight_bytes = if quant_call {
                    quant_linear_payload_bytes(part.bt_ggml_type, part.n, k)?
                } else {
                    part.n
                        .checked_mul(k)
                        .and_then(|len| len.checked_mul(size_of::<u16>()))
                        .ok_or_else(|| "gpu_linear weight size overflow".to_string())?
                };
                let qualified_key = part_key(part);
                let part_bytes = part.bytes;
                let needs_convert = f16acc && part.bt_ggml_type == GGML_TYPE_BF16;
                backend.cached_weight_buffer(&qualified_key, weight_bytes, || {
                    if needs_convert {
                        // bf16 -> f16 (value-preserving over the weight range).
                        let mut converted = vec![0u8; part_bytes.len()];
                        for (i, chunk) in part_bytes.chunks_exact(2).enumerate() {
                            let word = u16::from_le_bytes([chunk[0], chunk[1]]);
                            let half = crate::quant::f32_to_f16(f32::from_bits(
                                (word as u32) << 16,
                            ));
                            converted[i * 2..i * 2 + 2].copy_from_slice(&half.to_le_bytes());
                        }
                        Ok(converted)
                    } else {
                        Ok(part_bytes.to_vec())
                    }
                })?;
            }

            let out = if out_half {
                GpuTensor::from_pool_half(m, n_total)?
            } else {
                GpuTensor::from_pool(m, n_total)?
            };
            // COMPUTE_16F requires an f16 C matrix: an f16 output IS the C
            // matrix; an f32 output gemms into a pooled half buffer and
            // converts (+bias) in one pass afterwards.
            let c_half = if f16acc && !out_half {
                let bytes = m
                    .checked_mul(n_total)
                    .and_then(|len| len.checked_mul(size_of::<u16>()))
                    .ok_or_else(|| "gpu_linear c_half size overflow".to_string())?;
                Some(gpu_pool_acquire(bytes)?)
            } else {
                None
            };
            let c16_ptr: *mut u16 = if out_half {
                out.device_ptr_u16()?
            } else if let Some(c_half) = &c_half {
                c_half.ptr.as_ptr().cast::<u16>()
            } else {
                std::ptr::null_mut()
            };
            let out_ptr = if out_half {
                std::ptr::null_mut()
            } else {
                out.device_ptr()?
            };

            // The gemm's activation operand, computed only after every other
            // allocation: an eviction pass clears the input_half staging
            // buffer, so nothing may allocate between the convert and the
            // gemms. An f16 tensor is consumed directly (the point of the
            // f16 spine); f32 activations convert into the staging buffer.
            let b_operand: *const std::ffi::c_void = if x.half {
                x.device_ptr_u16()?.cast_const().cast::<std::ffi::c_void>()
            } else {
                backend.ensure_input_half(input_len * size_of::<u16>())?;
                let x_ptr = x.device_ptr()?;
                let input_half = backend
                    .input_half
                    .as_ref()
                    .ok_or_else(|| "missing gpu_linear half input buffer".to_string())?;
                let status = unsafe {
                    if half_type == crate::CUDA_R_16BF {
                        makepad_cuda_f32_to_bf16(
                            x_ptr,
                            input_half.ptr.as_ptr().cast::<u16>(),
                            input_len as u32,
                            backend.stream,
                        )
                    } else {
                        makepad_cuda_f32_to_f16(
                            x_ptr,
                            input_half.ptr.as_ptr().cast::<u16>(),
                            input_len as u32,
                            backend.stream,
                        )
                    }
                };
                gpu_check(status)?;
                input_half.ptr.as_ptr().cast_const()
            };
            let mut col_off = 0usize;
            for part in parts {
                let qualified_key = part_key(part);
                let weight = backend
                    .weight_buffers
                    .get(&qualified_key)
                    .ok_or_else(|| format!("missing cached CUDA weight buffer {qualified_key}"))?;
                // Quantized part: bulk-dequantize the cached payload into
                // pooled bf16 scratch; the gemm below then reads the scratch
                // as a regular bf16 weight. Scratch peaks at ONE part at a
                // time (largest H3 part: 28672x5376 bf16 = 308 MB), released
                // stream-ordered right after the gemm submission.
                let (a_operand, dequant_scratch): (*const std::ffi::c_void, Option<DeviceBuffer>) =
                    if quant_call {
                        let scratch_bytes = part
                            .n
                            .checked_mul(k)
                            .and_then(|len| len.checked_mul(size_of::<u16>()))
                            .ok_or_else(|| "gpu_linear dequant scratch overflow".to_string())?;
                        let scratch = gpu_pool_acquire(scratch_bytes)?;
                        let src = weight.ptr.as_ptr().cast_const();
                        let dst = scratch.ptr.as_ptr();
                        let status = unsafe {
                            match part.bt_ggml_type {
                                GGML_TYPE_Q4_K => makepad_cuda_dequant_q4_k_bf16(
                                    src,
                                    dst,
                                    (part.n * k / 256) as u32,
                                    backend.stream,
                                ),
                                GGML_TYPE_Q6_K => makepad_cuda_dequant_q6_k_bf16(
                                    src,
                                    dst,
                                    (part.n * k / 256) as u32,
                                    backend.stream,
                                ),
                                GGML_TYPE_Q4_0 => makepad_cuda_dequant_q4_0_bf16(
                                    src,
                                    dst,
                                    (part.n * k / 32) as u32,
                                    backend.stream,
                                ),
                                GGML_TYPE_H3_NVFP4_PAIRS
                                | GGML_TYPE_H3_NVFP4_PAIRS_PRESCALE => {
                                    makepad_cuda_dequant_nvfp4_pairs_bf16(
                                        src,
                                        dst,
                                        part.n as u32,
                                        k as u32,
                                        backend.stream,
                                    )
                                }
                                GGML_TYPE_F8_E4M3 => {
                                    let count = u32::try_from(part.n * k).map_err(|_| {
                                        "f8_e4m3 dequant count exceeds u32".to_string()
                                    })?;
                                    makepad_cuda_dequant_f8_e4m3_bf16(
                                        src,
                                        dst,
                                        count,
                                        backend.stream,
                                    )
                                }
                                other => {
                                    return Err(format!(
                                        "gpu_linear quant dequant unsupported type {other}"
                                    ))
                                }
                            }
                        };
                        gpu_check(status)?;
                        (dst.cast_const(), Some(scratch))
                    } else {
                        (weight.ptr.as_ptr().cast_const(), None)
                    };
                if f16acc {
                    const F16_ONE: u16 = 0x3C00;
                    const F16_ZERO: u16 = 0x0000;
                    unsafe {
                        crate::cublas_gemm_strided_batched_ex_raw(
                            backend.blas,
                            crate::CUBLAS_OP_T,
                            crate::CUBLAS_OP_N,
                            part.n as i32,
                            m as i32,
                            k as i32,
                            (&F16_ONE as *const u16).cast::<std::ffi::c_void>(),
                            weight.ptr.as_ptr(),
                            crate::CUDA_R_16F,
                            k as i32,
                            0,
                            b_operand,
                            crate::CUDA_R_16F,
                            k as i32,
                            0,
                            (&F16_ZERO as *const u16).cast::<std::ffi::c_void>(),
                            c16_ptr.add(col_off).cast::<std::ffi::c_void>(),
                            crate::CUDA_R_16F,
                            n_total as i32,
                            0,
                            1,
                            crate::CUBLAS_COMPUTE_16F,
                            crate::CUBLAS_GEMM_DEFAULT,
                        )
                        .map_err(|err| {
                            format!(
                                "gpu_linear f16acc gemm failed: m={m} k={k} n={}: {err}",
                                part.n
                            )
                        })?;
                    }
                } else {
                    let alpha = 1.0f32;
                    let beta = 0.0f32;
                    unsafe {
                        crate::cublas_gemm_strided_batched_ex(
                            backend.blas,
                            crate::CUBLAS_OP_T,
                            crate::CUBLAS_OP_N,
                            part.n as i32,
                            m as i32,
                            k as i32,
                            &alpha,
                            a_operand,
                            half_type,
                            k as i32,
                            0,
                            b_operand,
                            half_type,
                            k as i32,
                            0,
                            &beta,
                            out_ptr.add(col_off).cast::<std::ffi::c_void>(),
                            crate::CUDA_R_32F,
                            n_total as i32,
                            0,
                            1,
                            crate::CUBLAS_COMPUTE_32F,
                            crate::CUBLAS_GEMM_DEFAULT,
                        )
                        .map_err(|err| {
                            format!("gpu_linear gemm failed: m={m} k={k} n={}: {err}", part.n)
                        })?;
                    }
                }
                if let Some(scratch) = dequant_scratch {
                    // Same-stream discipline makes this safe: any pool reuse
                    // writes are ordered after the gemm submitted above.
                    gpu_pool_release(scratch);
                }
                col_off += part.n;
            }
            // The bias is model-constant: device-cache it (keyed off the
            // first weight part) instead of a pageable upload per call.
            let bias_key = if bias.is_empty() {
                None
            } else {
                let key = format!("{cache_namespace}::{}::b", parts[0].cache_key);
                let bias_bytes = bias.len() * size_of::<f32>();
                backend.cached_weight_buffer(&key, bias_bytes, || {
                    let raw = unsafe {
                        std::slice::from_raw_parts(bias.as_ptr().cast::<u8>(), bias_bytes)
                    };
                    Ok(raw.to_vec())
                })?;
                Some(key)
            };
            let bias_ptr = match &bias_key {
                Some(key) => Some(
                    backend
                        .weight_buffers
                        .get(key)
                        .ok_or_else(|| format!("missing cached CUDA bias buffer {key}"))?
                        .ptr
                        .as_ptr()
                        .cast::<f32>(),
                ),
                None => None,
            };

            if out_half {
                // The f16 C is the result; bias (if any) broadcasts in place.
                if let Some(bias_ptr) = bias_ptr {
                    let status = unsafe {
                        makepad_cuda_f16_bias_inplace(
                            c16_ptr,
                            bias_ptr,
                            m as u32,
                            n_total as u32,
                            backend.stream,
                        )
                    };
                    gpu_check(status)?;
                }
            } else if let Some(c_half) = c_half {
                // Fused C-convert + bias broadcast: one pass instead of a
                // convert pass plus a read-modify-write bias pass.
                let status = match bias_ptr {
                    Some(bias_ptr) => unsafe {
                        makepad_cuda_f16_bias_to_f32(
                            c_half.ptr.as_ptr().cast::<u16>(),
                            bias_ptr,
                            out_ptr,
                            m as u32,
                            n_total as u32,
                            backend.stream,
                        )
                    },
                    None => unsafe {
                        makepad_cuda_f16_to_f32_precise(
                            c_half.ptr.as_ptr().cast::<u16>(),
                            out_ptr,
                            (m * n_total) as u32,
                            backend.stream,
                        )
                    },
                };
                gpu_check(status)?;
                gpu_pool_release(c_half);
            } else if let Some(bias_ptr) = bias_ptr {
                let status = unsafe {
                    makepad_cuda_add_rows_vec_f32(
                        out_ptr,
                        bias_ptr,
                        out_ptr,
                        m as u32,
                        n_total as u32,
                        backend.stream,
                    )
                };
                gpu_check(status)?;
            }
            gpu_prof(backend.stream, crate::prof::CAT_DENSE_TOTAL, prof_start, 0);
            Ok(out)
        })
    }

    /// LayerNorm over the last dim followed by `* mul + add`, with mul/add
    /// broadcast per column (mul already contains the +1 the flux modulation
    /// wants).
    pub fn gpu_layer_norm_mul_add(
        x: &GpuTensor,
        mul: &[f32],
        add: &[f32],
        eps: f32,
    ) -> Result<GpuTensor, String> {
        if mul.len() != x.cols || add.len() != x.cols {
            return Err("gpu_layer_norm_mul_add width mismatch".to_string());
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let mul_buf = gpu_upload_small(backend, mul)?;
            let add_buf = gpu_upload_small(backend, add)?;
            let out = GpuTensor::from_pool(x.rows, x.cols)?;
            let status = unsafe {
                makepad_cuda_layer_norm_mul_add_f32(
                    x.device_ptr()?,
                    mul_buf.ptr.as_ptr().cast::<f32>(),
                    add_buf.ptr.as_ptr().cast::<f32>(),
                    out.device_ptr()?,
                    x.rows as u32,
                    x.cols as u32,
                    eps,
                    0.0,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_pool_release(mul_buf);
            gpu_pool_release(add_buf);
            gpu_prof(backend.stream, crate::prof::CAT_LAYER_NORM, prof_start, 0);
            Ok(out)
        })
    }

    /// Same kernel as [`gpu_layer_norm_mul_add`]; affine vectors stay resident
    /// under `{namespace}::{key}::ln` instead of H2D every call.
    pub fn gpu_layer_norm_mul_add_cached(
        x: &GpuTensor,
        cache_namespace: &str,
        cache_key: &str,
        mul: &[f32],
        add: &[f32],
        eps: f32,
    ) -> Result<GpuTensor, String> {
        if mul.len() != x.cols || add.len() != x.cols {
            return Err("gpu_layer_norm_mul_add width mismatch".to_string());
        }
        if x.half {
            return Err("gpu_layer_norm_mul_add_cached is f32-only".into());
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let vec_key = format!("{cache_namespace}::{cache_key}::ln");
            let vec_bytes = 2 * mul.len() * size_of::<f32>();
            backend.cached_weight_buffer(&vec_key, vec_bytes, || {
                let mut concat = Vec::with_capacity(mul.len() + add.len());
                concat.extend_from_slice(mul);
                concat.extend_from_slice(add);
                let raw = unsafe {
                    std::slice::from_raw_parts(concat.as_ptr().cast::<u8>(), vec_bytes)
                };
                Ok(raw.to_vec())
            })?;
            let vec_buf = backend
                .weight_buffers
                .get(&vec_key)
                .ok_or_else(|| format!("missing cached LN {vec_key}"))?;
            let mul_ptr = vec_buf.ptr.as_ptr().cast::<f32>();
            let add_ptr = unsafe { mul_ptr.add(mul.len()) };
            let out = GpuTensor::from_pool(x.rows, x.cols)?;
            let status = unsafe {
                makepad_cuda_layer_norm_mul_add_f32(
                    x.device_ptr()?,
                    mul_ptr,
                    add_ptr,
                    out.device_ptr()?,
                    x.rows as u32,
                    x.cols as u32,
                    eps,
                    0.0,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_prof(backend.stream, crate::prof::CAT_LAYER_NORM, prof_start, 0);
            Ok(out)
        })
    }

    /// Numerically follows PyTorch 2.7's vectorized CUDA LayerNorm fast path:
    /// float4 online-Welford moments, its fixed 4-warp reduction tree, and
    /// identical affine-operation association. SkinTokens uses this where the
    /// released Torch checkpoint is the numeric oracle.
    pub fn gpu_layer_norm_pytorch(
        x: &GpuTensor,
        scale: &[f32],
        bias: &[f32],
        eps: f32,
    ) -> Result<GpuTensor, String> {
        if scale.len() != x.cols || bias.len() != x.cols || x.cols % 4 != 0 {
            return Err("gpu_layer_norm_pytorch width mismatch".to_string());
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let scale_buf = gpu_upload_small(backend, scale)?;
            let bias_buf = gpu_upload_small(backend, bias)?;
            let out = GpuTensor::from_pool(x.rows, x.cols)?;
            let status = unsafe {
                makepad_cuda_layer_norm_pytorch_f32(
                    x.device_ptr()?,
                    scale_buf.ptr.as_ptr().cast::<f32>(),
                    bias_buf.ptr.as_ptr().cast::<f32>(),
                    out.device_ptr()?,
                    x.rows as u32,
                    x.cols as u32,
                    eps,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_pool_release(scale_buf);
            gpu_pool_release(bias_buf);
            gpu_prof(backend.stream, crate::prof::CAT_LAYER_NORM, prof_start, 0);
            Ok(out)
        })
    }

    /// LayerNorm over consecutive `group_cols` groups while retaining the
    /// input tensor's outer shape. HY-Motion uses this for the text refiner's
    /// affine per-head Q/K LayerNorm (64 values per group).
    pub fn gpu_layer_norm_mul_add_grouped(
        x: &GpuTensor,
        group_cols: usize,
        mul: &[f32],
        add: &[f32],
        eps: f32,
    ) -> Result<GpuTensor, String> {
        if x.half
            || group_cols == 0
            || (x.rows * x.cols) % group_cols != 0
            || mul.len() != group_cols
            || add.len() != group_cols
        {
            return Err("gpu_layer_norm_mul_add_grouped shape mismatch".to_string());
        }
        let grouped_rows = (x.rows * x.cols) / group_cols;
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let mul_buf = gpu_upload_small(backend, mul)?;
            let add_buf = gpu_upload_small(backend, add)?;
            let out = GpuTensor::from_pool(x.rows, x.cols)?;
            let status = unsafe {
                makepad_cuda_layer_norm_mul_add_f32(
                    x.device_ptr()?,
                    mul_buf.ptr.as_ptr().cast::<f32>(),
                    add_buf.ptr.as_ptr().cast::<f32>(),
                    out.device_ptr()?,
                    grouped_rows as u32,
                    group_cols as u32,
                    eps,
                    0.0,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_pool_release(mul_buf);
            gpu_pool_release(add_buf);
            gpu_prof(
                backend.stream,
                crate::prof::CAT_LAYER_NORM,
                prof_start,
                0,
            );
            Ok(out)
        })
    }

    /// Modulated layer norm with DEVICE-RESIDENT scale/shift: `mods` is the
    /// per-step modulation row and scale/shift live at element offsets in it
    /// (the kernel adds the +1 to scale) — no per-call host uploads and no
    /// per-step modulation download.
    pub fn gpu_layer_norm_mod(
        x: &GpuTensor,
        mods: &GpuTensor,
        scale_off: usize,
        shift_off: usize,
        eps: f32,
    ) -> Result<GpuTensor, String> {
        let mods_len = mods.rows * mods.cols;
        if scale_off + x.cols > mods_len || shift_off + x.cols > mods_len {
            return Err("gpu_layer_norm_mod offset out of range".to_string());
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(x.rows, x.cols)?;
            let mods_ptr = mods.device_ptr()?;
            let status = unsafe {
                makepad_cuda_layer_norm_mul_add_f32(
                    x.device_ptr()?,
                    mods_ptr.add(scale_off),
                    mods_ptr.add(shift_off),
                    out.device_ptr()?,
                    x.rows as u32,
                    x.cols as u32,
                    eps,
                    1.0,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_prof(backend.stream, crate::prof::CAT_LAYER_NORM, prof_start, 0);
            Ok(out)
        })
    }

    /// residual + gate*update with a DEVICE-RESIDENT gate vector at an
    /// element offset into the modulation row, fused into one kernel.
    pub fn gpu_gated_residual_mod(
        residual: &GpuTensor,
        update: &GpuTensor,
        mods: &GpuTensor,
        gate_off: usize,
    ) -> Result<GpuTensor, String> {
        if residual.rows != update.rows || residual.cols != update.cols {
            return Err("gpu_gated_residual_mod shape mismatch".to_string());
        }
        if gate_off + update.cols > mods.rows * mods.cols {
            return Err("gpu_gated_residual_mod offset out of range".to_string());
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(residual.rows, residual.cols)?;
            let status = unsafe {
                makepad_cuda_gated_residual_vec_f32(
                    residual.device_ptr()?,
                    update.device_ptr()?,
                    mods.device_ptr()?.add(gate_off),
                    out.device_ptr()?,
                    residual.rows as u32,
                    residual.cols as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_prof(backend.stream, crate::prof::CAT_ELEMENTWISE, prof_start, 0);
            Ok(out)
        })
    }

    /// `gpu_gated_residual_mod` with the bf16-RN boundary folded onto the
    /// store — bit-identical to gated_residual_mod followed by
    /// gpu_bf16_round, one memory pass instead of two.
    pub fn gpu_gated_residual_mod_round_bf16(
        residual: &GpuTensor,
        update: &GpuTensor,
        mods: &GpuTensor,
        gate_off: usize,
    ) -> Result<GpuTensor, String> {
        if residual.rows != update.rows || residual.cols != update.cols {
            return Err("gpu_gated_residual_mod_round_bf16 shape mismatch".to_string());
        }
        if gate_off + update.cols > mods.rows * mods.cols {
            return Err("gpu_gated_residual_mod_round_bf16 offset out of range".to_string());
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(residual.rows, residual.cols)?;
            let status = unsafe {
                makepad_cuda_gated_residual_vec_round_bf16_f32(
                    residual.device_ptr()?,
                    update.device_ptr()?,
                    mods.device_ptr()?.add(gate_off),
                    out.device_ptr()?,
                    residual.rows as u32,
                    residual.cols as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_prof(backend.stream, crate::prof::CAT_ELEMENTWISE, prof_start, 0);
            Ok(out)
        })
    }

    /// RMS-norm with weight over groups of `group_cols` values (the flux
    /// per-head QK norm: rows' = rows*cols/group_cols).
    /// snake(x) = x + (1/(alpha+1e-9)) * sin^2(alpha*x), alpha per column
    /// (MOSS DAC device path; planes are time-major rows=samples).
    pub fn gpu_snake_cols(x: &GpuTensor, alpha: &GpuTensor) -> Result<GpuTensor, String> {
        if alpha.rows * alpha.cols != x.cols {
            return Err("gpu_snake_cols alpha width mismatch".to_string());
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(x.rows, x.cols)?;
            let status = unsafe {
                makepad_cuda_snake_cols_f32(
                    x.device_ptr()?,
                    alpha.device_ptr()?,
                    out.device_ptr()?,
                    x.rows as u32,
                    x.cols as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_prof(backend.stream, crate::prof::CAT_ELEMENTWISE, prof_start, 0);
            Ok(out)
        })
    }

    pub fn gpu_rms_norm_mul(
        x: &GpuTensor,
        group_cols: usize,
        cache_namespace: &str,
        cache_key: &str,
        scale: &[f32],
        eps: f32,
    ) -> Result<GpuTensor, String> {
        if group_cols == 0 || (x.rows * x.cols) % group_cols != 0 || scale.len() != group_cols {
            return Err("gpu_rms_norm_mul shape mismatch".to_string());
        }
        let group_rows = (x.rows * x.cols) / group_cols;
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            // The scale vector is model-constant: device-cache it instead of
            // re-uploading on every one of the ~600 calls per image.
            let vec_key = format!("{cache_namespace}::{cache_key}::rms");
            let vec_bytes = scale.len() * size_of::<f32>();
            backend.cached_weight_buffer(&vec_key, vec_bytes, || {
                let raw = unsafe {
                    std::slice::from_raw_parts(scale.as_ptr().cast::<u8>(), vec_bytes)
                };
                Ok(raw.to_vec())
            })?;
            let out = if x.half {
                GpuTensor::from_pool_half(x.rows, x.cols)?
            } else {
                GpuTensor::from_pool(x.rows, x.cols)?
            };
            let scale_buf = backend
                .weight_buffers
                .get(&vec_key)
                .ok_or_else(|| format!("missing cached CUDA rms scale buffer {vec_key}"))?;
            let status = if x.half {
                unsafe {
                    makepad_cuda_rms_norm_rows_weighted_f16(
                        x.device_ptr_u16()?,
                        scale_buf.ptr.as_ptr().cast::<f32>(),
                        out.device_ptr_u16()?,
                        group_rows as u32,
                        group_cols as u32,
                        group_cols as u32,
                        eps,
                        backend.stream,
                    )
                }
            } else {
                unsafe {
                    makepad_cuda_rms_norm_rows_weighted_f32_f32weights_precise(
                        x.device_ptr()?,
                        scale_buf.ptr.as_ptr().cast::<f32>(),
                        out.device_ptr()?,
                        group_rows as u32,
                        group_cols as u32,
                        group_cols as u32,
                        eps,
                        backend.stream,
                    )
                }
            };
            gpu_check(status)?;
            gpu_prof(backend.stream, crate::prof::CAT_RMS_NORM, prof_start, 0);
            Ok(out)
        })
    }

    /// Fused `gpu_rms_norm_mul` (f32 precise kernel) + `gpu_bf16_round`:
    /// identical reduction and normalize/scale arithmetic under the identical
    /// launch geometry, with the bf16 round applied at the store instead of
    /// in a second full-tensor pass. Bit-identical to the two-kernel recipe.
    pub fn gpu_rms_norm_mul_round_bf16(
        x: &GpuTensor,
        group_cols: usize,
        cache_namespace: &str,
        cache_key: &str,
        scale: &[f32],
        eps: f32,
    ) -> Result<GpuTensor, String> {
        if x.half {
            return Err("gpu_rms_norm_mul_round_bf16 expects f32 storage".to_string());
        }
        if group_cols == 0 || (x.rows * x.cols) % group_cols != 0 || scale.len() != group_cols {
            return Err("gpu_rms_norm_mul_round_bf16 shape mismatch".to_string());
        }
        let group_rows = (x.rows * x.cols) / group_cols;
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let vec_key = format!("{cache_namespace}::{cache_key}::rmsrnd");
            let vec_bytes = scale.len() * size_of::<f32>();
            backend.cached_weight_buffer(&vec_key, vec_bytes, || {
                let raw = unsafe {
                    std::slice::from_raw_parts(scale.as_ptr().cast::<u8>(), vec_bytes)
                };
                Ok(raw.to_vec())
            })?;
            let out = GpuTensor::from_pool(x.rows, x.cols)?;
            let scale_buf = backend
                .weight_buffers
                .get(&vec_key)
                .ok_or_else(|| format!("missing cached CUDA rms scale buffer {vec_key}"))?;
            let status = unsafe {
                makepad_cuda_rms_norm_weighted_precise_round_bf16(
                    x.device_ptr()?,
                    scale_buf.ptr.as_ptr().cast::<f32>(),
                    out.device_ptr()?,
                    group_rows as u32,
                    group_cols as u32,
                    group_cols as u32,
                    eps,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_prof(backend.stream, crate::prof::CAT_RMS_NORM, prof_start, 0);
            Ok(out)
        })
    }

    /// Fused AdaLN modulation:
    /// `out = fmaf(mods[shift_off+c], 1.0, fmaf(1.0 + mods[scale_off+c], normed, 0.0))`
    /// — bit-identical to the slice + add-ones + two `gpu_gated_residual_mod`
    /// launch chain it replaces.
    pub fn gpu_adaln_mod(
        normed: &GpuTensor,
        mods: &GpuTensor,
        scale_off: usize,
        shift_off: usize,
    ) -> Result<GpuTensor, String> {
        if normed.half || mods.half {
            return Err("gpu_adaln_mod expects f32 storage".to_string());
        }
        let mods_len = mods.rows * mods.cols;
        if scale_off + normed.cols > mods_len || shift_off + normed.cols > mods_len {
            return Err("gpu_adaln_mod offset out of range".to_string());
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(normed.rows, normed.cols)?;
            let status = unsafe {
                makepad_cuda_adaln_mod_f32(
                    normed.device_ptr()?,
                    mods.device_ptr()?.add(scale_off),
                    mods.device_ptr()?.add(shift_off),
                    out.device_ptr()?,
                    normed.rows as u32,
                    normed.cols as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_prof(backend.stream, crate::prof::CAT_ELEMENTWISE, prof_start, 0);
            Ok(out)
        })
    }

    /// Fused gate-mul + bf16 round + residual add — the ACE per-block
    /// `gpu_gated_residual_mod(zeros, update, mods, off)` → `gpu_bf16_round`
    /// → `gpu_add_bf16(h, ...)` chain in one pass, bit-identical.
    pub fn gpu_gated_residual_round_add_bf16(
        h: &GpuTensor,
        update: &GpuTensor,
        mods: &GpuTensor,
        gate_off: usize,
    ) -> Result<GpuTensor, String> {
        if h.half || update.half || mods.half {
            return Err("gpu_gated_residual_round_add_bf16 expects f32 storage".to_string());
        }
        if h.rows != update.rows || h.cols != update.cols {
            return Err("gpu_gated_residual_round_add_bf16 shape mismatch".to_string());
        }
        if gate_off + update.cols > mods.rows * mods.cols {
            return Err("gpu_gated_residual_round_add_bf16 offset out of range".to_string());
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(h.rows, h.cols)?;
            let status = unsafe {
                makepad_cuda_gated_residual_round_add_bf16_f32(
                    h.device_ptr()?,
                    update.device_ptr()?,
                    mods.device_ptr()?.add(gate_off),
                    out.device_ptr()?,
                    h.rows as u32,
                    h.cols as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_prof(backend.stream, crate::prof::CAT_ELEMENTWISE, prof_start, 0);
            Ok(out)
        })
    }

    /// Fused SwiGLU chain `silu → bf16 round → mul → bf16 round` (the exact
    /// ACE FFN recipe: `gpu_silu` + `gpu_bf16_round` + `gpu_mul` +
    /// `gpu_bf16_round`), bit-identical in one pass.
    pub fn gpu_silu_round_mul_round_bf16(
        gate: &GpuTensor,
        up: &GpuTensor,
    ) -> Result<GpuTensor, String> {
        if gate.half || up.half || gate.rows != up.rows || gate.cols != up.cols {
            return Err("gpu_silu_round_mul_round_bf16 expects matching f32 tensors".to_string());
        }
        let len = gate
            .rows
            .checked_mul(gate.cols)
            .ok_or_else(|| "gpu_silu_round_mul_round_bf16 element count overflow".to_string())?;
        let len_u32 = u32::try_from(len)
            .map_err(|_| "gpu_silu_round_mul_round_bf16 element count exceeds CUDA limit".to_string())?;
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(gate.rows, gate.cols)?;
            let status = unsafe {
                makepad_cuda_silu_round_mul_round_bf16_f32(
                    gate.device_ptr()?,
                    up.device_ptr()?,
                    out.device_ptr()?,
                    len_u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_prof(backend.stream, crate::prof::CAT_ELEMENTWISE, prof_start, 0);
            Ok(out)
        })
    }


    /// Qwen-style BF16 RMSNorm. Inputs are held in f32 buffers for convenient
    /// composition, but normalization, scale multiplication, and output are
    /// rounded exactly to BF16 at this operator boundary.
    pub fn gpu_rms_norm_mul_bf16(
        x: &GpuTensor,
        group_cols: usize,
        cache_namespace: &str,
        cache_key: &str,
        scale: &[f32],
        eps: f32,
    ) -> Result<GpuTensor, String> {
        if x.half
            || group_cols == 0
            || (x.rows * x.cols) % group_cols != 0
            || scale.len() != group_cols
        {
            return Err("gpu_rms_norm_mul_bf16 shape mismatch".to_string());
        }
        let group_rows = (x.rows * x.cols) / group_cols;
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let vec_key = format!("{cache_namespace}::{cache_key}::rmsbf16");
            let vec_bytes = scale.len() * size_of::<f32>();
            backend.cached_weight_buffer(&vec_key, vec_bytes, || {
                let raw = unsafe {
                    std::slice::from_raw_parts(scale.as_ptr().cast::<u8>(), vec_bytes)
                };
                Ok(raw.to_vec())
            })?;
            let out = GpuTensor::from_pool(x.rows, x.cols)?;
            let scale_buf = backend
                .weight_buffers
                .get(&vec_key)
                .ok_or_else(|| format!("missing cached CUDA rms scale buffer {vec_key}"))?;
            let status = unsafe {
                makepad_cuda_rms_norm_rows_weighted_f32_f32weights(
                    x.device_ptr()?,
                    scale_buf.ptr.as_ptr().cast::<f32>(),
                    out.device_ptr()?,
                    group_rows as u32,
                    group_cols as u32,
                    group_cols as u32,
                    eps,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            Ok(out)
        })
    }

    /// Official Qwen3RMSNorm: `w * xhat.to(bf16)` after f32 variance/rsqrt.
    /// Unlike `gpu_rms_norm_mul_bf16`, the product is not rounded to bf16.
    pub fn gpu_rms_norm_qwen3(
        x: &GpuTensor,
        group_cols: usize,
        cache_namespace: &str,
        cache_key: &str,
        scale: &[f32],
        eps: f32,
    ) -> Result<GpuTensor, String> {
        if x.half
            || group_cols == 0
            || (x.rows * x.cols) % group_cols != 0
            || scale.len() != group_cols
        {
            return Err("gpu_rms_norm_qwen3 shape mismatch".to_string());
        }
        let group_rows = (x.rows * x.cols) / group_cols;
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let vec_key = format!("{cache_namespace}::{cache_key}::rmsqwen3");
            let vec_bytes = scale.len() * size_of::<f32>();
            backend.cached_weight_buffer(&vec_key, vec_bytes, || {
                let raw = unsafe {
                    std::slice::from_raw_parts(scale.as_ptr().cast::<u8>(), vec_bytes)
                };
                Ok(raw.to_vec())
            })?;
            let out = GpuTensor::from_pool(x.rows, x.cols)?;
            let scale_buf = backend
                .weight_buffers
                .get(&vec_key)
                .ok_or_else(|| format!("missing cached CUDA rms scale buffer {vec_key}"))?;
            let status = unsafe {
                makepad_cuda_rms_norm_qwen3(
                    x.device_ptr()?,
                    scale_buf.ptr.as_ptr().cast::<f32>(),
                    out.device_ptr()?,
                    group_rows as u32,
                    group_cols as u32,
                    group_cols as u32,
                    eps,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            Ok(out)
        })
    }

    /// RMS-norm over per-head groups with a DISTINCT weight vector per head
    /// (the TRELLIS MultiHeadRMSNorm): x is (rows, heads*head_dim), scale is
    /// heads*head_dim, each (row, head) group normalizes over head_dim and
    /// multiplies by scale[head*head_dim..]. f32 only.
    pub fn gpu_rms_norm_mul_perhead(
        x: &GpuTensor,
        heads: usize,
        head_dim: usize,
        cache_namespace: &str,
        cache_key: &str,
        scale: &[f32],
        eps: f32,
    ) -> Result<GpuTensor, String> {
        if heads == 0
            || head_dim == 0
            || x.cols != heads * head_dim
            || scale.len() != heads * head_dim
        {
            return Err("gpu_rms_norm_mul_perhead shape mismatch".to_string());
        }
        if x.half {
            return Err("gpu_rms_norm_mul_perhead is f32-only".to_string());
        }
        let group_rows = x.rows * heads;
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let vec_key = format!("{cache_namespace}::{cache_key}::rmsph");
            let vec_bytes = scale.len() * size_of::<f32>();
            backend.cached_weight_buffer(&vec_key, vec_bytes, || {
                let raw = unsafe {
                    std::slice::from_raw_parts(scale.as_ptr().cast::<u8>(), vec_bytes)
                };
                Ok(raw.to_vec())
            })?;
            let out = GpuTensor::from_pool(x.rows, x.cols)?;
            let scale_buf = backend
                .weight_buffers
                .get(&vec_key)
                .ok_or_else(|| format!("missing cached CUDA rms scale buffer {vec_key}"))?;
            let status = unsafe {
                makepad_cuda_rms_norm_perhead_f32(
                    x.device_ptr()?,
                    scale_buf.ptr.as_ptr().cast::<f32>(),
                    out.device_ptr()?,
                    group_rows as u32,
                    head_dim as u32,
                    heads as u32,
                    eps,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_prof(backend.stream, crate::prof::CAT_RMS_NORM, prof_start, 0);
            Ok(out)
        })
    }

    /// GeGLU (tanh gelu) value/gate over packed rows [value(n), gate(n)]
    /// (the SA3 T5Gemma MLP): out = value * gelu_tanh(gate).
    pub fn gpu_geglu_tanh_value_gate(x: &GpuTensor) -> Result<GpuTensor, String> {
        if x.cols % 2 != 0 {
            return Err("gpu_geglu_tanh_value_gate odd column count".to_string());
        }
        if x.half {
            return Err("gpu_geglu_tanh_value_gate is f32-only".to_string());
        }
        let n = x.cols / 2;
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(x.rows, n)?;
            let status = unsafe {
                makepad_cuda_geglu_tanh_value_gate_f32(
                    x.device_ptr()?,
                    out.device_ptr()?,
                    x.rows as u32,
                    n as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_prof(backend.stream, crate::prof::CAT_ELEMENTWISE, prof_start, 0);
            Ok(out)
        })
    }

    /// DynamicTanh norm over groups of `width` values (SA3 SAME-S AE):
    /// y = tanh(alpha*x)*gamma + beta. gamma/beta cached device-side.
    #[allow(clippy::too_many_arguments)]
    pub fn gpu_dyt(
        x: &GpuTensor,
        width: usize,
        cache_namespace: &str,
        cache_key: &str,
        gamma: &[f32],
        beta: &[f32],
        alpha: f32,
    ) -> Result<GpuTensor, String> {
        if width == 0
            || (x.rows * x.cols) % width != 0
            || gamma.len() != width
            || beta.len() != width
        {
            return Err("gpu_dyt shape mismatch".to_string());
        }
        if x.half {
            return Err("gpu_dyt is f32-only".to_string());
        }
        let group_rows = (x.rows * x.cols) / width;
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let key = format!("{cache_namespace}::{cache_key}::dyt");
            let vec_bytes = 2 * width * size_of::<f32>();
            backend.cached_weight_buffer(&key, vec_bytes, || {
                let mut raw = Vec::with_capacity(vec_bytes);
                for value in gamma.iter().chain(beta) {
                    raw.extend_from_slice(&value.to_le_bytes());
                }
                Ok(raw)
            })?;
            let out = GpuTensor::from_pool(x.rows, x.cols)?;
            let vec_buf = backend
                .weight_buffers
                .get(&key)
                .ok_or_else(|| format!("missing cached CUDA dyt buffer {key}"))?;
            let gamma_ptr = vec_buf.ptr.as_ptr().cast::<f32>();
            let status = unsafe {
                makepad_cuda_dyt_f32(
                    x.device_ptr()?,
                    gamma_ptr,
                    gamma_ptr.add(width),
                    out.device_ptr()?,
                    group_rows as u32,
                    width as u32,
                    alpha,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_prof(backend.stream, crate::prof::CAT_RMS_NORM, prof_start, 0);
            Ok(out)
        })
    }

    /// Packed attention with Gemma-style logit softcapping and an optional
    /// additive key mask (0 valid / -inf padded), f32 composite path only —
    /// the SA3 T5Gemma encoder (bidirectional, 256 tokens). `key_mask` is a
    /// device tensor of `kv_len` values.
    pub fn gpu_attention_packed_softcap(
        q: &GpuTensor,
        k: &GpuTensor,
        v: &GpuTensor,
        head_count: usize,
        scale: f32,
        softcap: f32,
        key_mask: Option<&GpuTensor>,
    ) -> Result<GpuTensor, String> {
        let seq = q.rows;
        let kv_len = k.rows;
        let hidden = q.cols;
        if k.cols != hidden || v.cols != hidden || v.rows != kv_len {
            return Err("gpu_attention_softcap shape mismatch".to_string());
        }
        if head_count == 0 || hidden % head_count != 0 {
            return Err("gpu_attention_softcap head mismatch".to_string());
        }
        if q.half || k.half || v.half {
            return Err("gpu_attention_softcap is f32-only".to_string());
        }
        if let Some(mask) = key_mask {
            if mask.rows * mask.cols != kv_len {
                return Err("gpu_attention_softcap mask length mismatch".to_string());
            }
        }
        let head_dim = hidden / head_count;
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let scores_len = head_count
                .checked_mul(seq)
                .and_then(|len| len.checked_mul(kv_len))
                .ok_or_else(|| "gpu_attention_softcap scores overflow".to_string())?;
            let scores = gpu_pool_acquire(scores_len * size_of::<f32>())?;
            let scores_ptr = scores.ptr.as_ptr().cast::<f32>();
            let beta = 0.0f32;
            let one = 1.0f32;
            unsafe {
                // scores[h][i][j] = scale * sum_d q[i][h][d] * k[j][h][d]
                crate::cublas_gemm_strided_batched_ex(
                    backend.blas,
                    crate::CUBLAS_OP_T,
                    crate::CUBLAS_OP_N,
                    kv_len as i32,
                    seq as i32,
                    head_dim as i32,
                    &scale,
                    k.device_ptr()?.cast::<std::ffi::c_void>(),
                    crate::CUDA_R_32F,
                    hidden as i32,
                    head_dim as i64,
                    q.device_ptr()?.cast::<std::ffi::c_void>(),
                    crate::CUDA_R_32F,
                    hidden as i32,
                    head_dim as i64,
                    &beta,
                    scores_ptr.cast::<std::ffi::c_void>(),
                    crate::CUDA_R_32F,
                    kv_len as i32,
                    (seq * kv_len) as i64,
                    head_count as i32,
                    crate::CUBLAS_COMPUTE_32F,
                    crate::CUBLAS_GEMM_DEFAULT,
                )
                .map_err(|err| format!("gpu_attention_softcap qk gemm failed: {err}"))?;
            }
            let mask_ptr = match key_mask {
                Some(mask) => mask.device_ptr()?,
                None => std::ptr::null(),
            };
            let status = unsafe {
                makepad_cuda_softcap_addmask_f32(
                    scores_ptr,
                    mask_ptr,
                    (head_count * seq) as u32,
                    kv_len as u32,
                    softcap,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            let status = unsafe {
                makepad_cuda_softmax_rows_precise_f32(
                    scores_ptr,
                    scores_ptr,
                    (head_count * seq) as u32,
                    kv_len as u32,
                    kv_len as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            let out = GpuTensor::from_pool(seq, hidden)?;
            unsafe {
                // out[i][h][d] = sum_j probs[h][i][j] * v[j][h][d]
                crate::cublas_gemm_strided_batched_ex(
                    backend.blas,
                    crate::CUBLAS_OP_N,
                    crate::CUBLAS_OP_N,
                    head_dim as i32,
                    seq as i32,
                    kv_len as i32,
                    &one,
                    v.device_ptr()?.cast::<std::ffi::c_void>(),
                    crate::CUDA_R_32F,
                    hidden as i32,
                    head_dim as i64,
                    scores_ptr.cast::<std::ffi::c_void>(),
                    crate::CUDA_R_32F,
                    kv_len as i32,
                    (seq * kv_len) as i64,
                    &beta,
                    out.device_ptr()?.cast::<std::ffi::c_void>(),
                    crate::CUDA_R_32F,
                    hidden as i32,
                    head_dim as i64,
                    head_count as i32,
                    crate::CUBLAS_COMPUTE_32F,
                    crate::CUBLAS_GEMM_DEFAULT,
                )
                .map_err(|err| format!("gpu_attention_softcap pv gemm failed: {err}"))?;
            }
            gpu_pool_release(scores);
            gpu_prof(backend.stream, crate::prof::CAT_FLASH_ATTN, prof_start, 0);
            Ok(out)
        })
    }

    /// Row gather with optional per-row column-block select:
    /// `out[i] = src[row_idx[i]][block*cols .. block*cols + cols]` where
    /// `block = colblock_idx[i]` (or 0 when None). `row_idx == u32::MAX`
    /// yields a zero row (out-of-grid conv neighbors). Index buffers are
    /// device-resident u32 tensors from `gpu_upload_u32`.
    pub fn gpu_gather_rows_colblock(
        src: &GpuTensor,
        row_idx: &GpuTensor,
        colblock_idx: Option<&GpuTensor>,
        block_cols: usize,
    ) -> Result<GpuTensor, String> {
        if src.half {
            return Err("gpu_gather_rows_colblock is f32-only".to_string());
        }
        if block_cols == 0 || src.cols % block_cols != 0 {
            return Err("gpu_gather_rows_colblock block width mismatch".to_string());
        }
        let out_rows = row_idx.rows * row_idx.cols;
        if let Some(blocks) = colblock_idx {
            if blocks.rows * blocks.cols != out_rows {
                return Err("gpu_gather_rows_colblock index length mismatch".to_string());
            }
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(out_rows, block_cols)?;
            let colblock_ptr: *const u32 = match colblock_idx {
                Some(blocks) => blocks.device_ptr()?.cast::<u32>(),
                None => std::ptr::null(),
            };
            let status = unsafe {
                makepad_cuda_gather_rows_colblock_f32(
                    src.device_ptr()?,
                    row_idx.device_ptr()?.cast::<u32>(),
                    colblock_ptr,
                    out.device_ptr()?,
                    out_rows as u32,
                    src.cols as u32,
                    block_cols as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_prof(backend.stream, crate::prof::CAT_ELEMENTWISE, prof_start, 0);
            Ok(out)
        })
    }

    /// Column gather shared across every row: out[r][j] = x[r][indices[j]].
    /// On planar [channel][y*w+x] tensors one host-computed index table
    /// re-addresses every channel at once — composes reflect padding, the
    /// valid-region crop after a pad-0 conv, and stride-2 subsampling for
    /// the H3 VAE encoder (gpu_conv2d_planar_cached is zero-pad stride-1).
    pub fn gpu_gather_cols(x: &GpuTensor, indices: &[u32]) -> Result<GpuTensor, String> {
        if x.half {
            return Err("gpu_gather_cols is f32-only".to_string());
        }
        if indices.is_empty() {
            return Err("gpu_gather_cols: empty index table".to_string());
        }
        for &index in indices {
            if index as usize >= x.cols {
                return Err(format!(
                    "gpu_gather_cols index {index} out of range (cols {})",
                    x.cols
                ));
            }
        }
        let idx = gpu_upload_u32(indices)?;
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(x.rows, indices.len())?;
            let status = unsafe {
                makepad_cuda_gather_cols_f32(
                    x.device_ptr()?,
                    idx.device_ptr()?.cast::<u32>(),
                    out.device_ptr()?,
                    x.rows as u32,
                    x.cols as u32,
                    indices.len() as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_prof(backend.stream, crate::prof::CAT_ELEMENTWISE, prof_start, 0);
            Ok(out)
        })
    }

    /// Submanifold 3^3 sparse conv as chunked im2col + ONE tensor-core gemm
    /// per chunk (replaces 27 x (gather + gemm + add) = 81 launches and the
    /// 27 full-size accumulate passes). The slab is (rows, 27*ci) f16,
    /// tap-major columns, zero where a neighbor is absent — exactly the
    /// checkpoint's [co, 27*ci] flex_gemm weight layout, which the caller
    /// must already have ensured (f16, unconverted) under
    /// `{namespace}::{weight_cache_key}`. f32 accumulate, f32 out (+bias).
    /// `neighbors` is tap-major u32: word t*n + voxel (u32::MAX = absent).
    pub fn gpu_sparse_conv27(
        x: &GpuTensor,
        neighbors: &GpuTensor,
        cache_namespace: &str,
        weight_cache_key: &str,
        co: usize,
        bias: &[f32],
    ) -> Result<GpuTensor, String> {
        if x.half {
            return Err("gpu_sparse_conv27 expects f32 activations".to_string());
        }
        let n = x.rows;
        let ci = x.cols;
        if n == 0 || ci == 0 || co == 0 {
            return Err("gpu_sparse_conv27 empty shape".to_string());
        }
        if neighbors.rows * neighbors.cols != 27 * n {
            return Err(format!(
                "gpu_sparse_conv27 neighbor words {} != 27*{n}",
                neighbors.rows * neighbors.cols
            ));
        }
        if !bias.is_empty() && bias.len() != co {
            return Err("gpu_sparse_conv27 bias mismatch".to_string());
        }
        let k_total = 27 * ci;
        // Slab cap: bounds the transient at ~512MB even at the 5M-voxel
        // final stage (chunking keeps every gemm fat enough to saturate).
        const SLAB_CAP_BYTES: usize = 512 * 1024 * 1024;
        let max_rows = (SLAB_CAP_BYTES / (k_total * size_of::<u16>())).clamp(1, n);
        let qualified_key = format!("{cache_namespace}::{weight_cache_key}");
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let weight_ptr = backend
                .weight_buffers
                .get(&qualified_key)
                .ok_or_else(|| {
                    format!("gpu_sparse_conv27 weight '{qualified_key}' not in device cache")
                })?
                .ptr
                .as_ptr();
            let out = GpuTensor::from_pool(n, co)?;
            let slab = gpu_pool_acquire(max_rows * k_total * size_of::<u16>())?;
            let mut r0 = 0usize;
            while r0 < n {
                let rows = max_rows.min(n - r0);
                let status = unsafe {
                    makepad_cuda_gather27_f16(
                        x.device_ptr()?,
                        neighbors.device_ptr()?.cast::<u32>(),
                        slab.ptr.as_ptr().cast::<u16>(),
                        r0 as u32,
                        rows as u32,
                        n as u32,
                        ci as u32,
                        backend.stream,
                    )
                };
                gpu_check(status)?;
                // Row-major C(rows, co) at row r0 is column-major (co, rows):
                // C_col = op_T(W_col(k, co)) * op_N(Slab_col(k, rows)).
                let alpha = 1.0f32;
                let beta = 0.0f32;
                unsafe {
                    crate::cublas_gemm_strided_batched_ex(
                        backend.blas,
                        crate::CUBLAS_OP_T,
                        crate::CUBLAS_OP_N,
                        co as i32,
                        rows as i32,
                        k_total as i32,
                        &alpha,
                        weight_ptr.cast::<std::ffi::c_void>(),
                        crate::CUDA_R_16F,
                        k_total as i32,
                        0,
                        slab.ptr.as_ptr().cast::<std::ffi::c_void>(),
                        crate::CUDA_R_16F,
                        k_total as i32,
                        0,
                        &beta,
                        out.device_ptr()?
                            .add(r0 * co)
                            .cast::<std::ffi::c_void>(),
                        crate::CUDA_R_32F,
                        co as i32,
                        0,
                        1,
                        crate::CUBLAS_COMPUTE_32F,
                        crate::CUBLAS_GEMM_DEFAULT,
                    )
                    .map_err(|err| format!("gpu_sparse_conv27 gemm failed: {err}"))?;
                }
                r0 += rows;
            }
            gpu_pool_release(slab);
            if !bias.is_empty() {
                let bias_dev = gpu_upload_small(backend, bias)?;
                let status = unsafe {
                    makepad_cuda_add_rows_vec_f32(
                        out.device_ptr()?,
                        bias_dev.ptr.as_ptr().cast::<f32>(),
                        out.device_ptr()?,
                        n as u32,
                        co as u32,
                        backend.stream,
                    )
                };
                gpu_check(status)?;
                gpu_pool_release(bias_dev);
            }
            gpu_prof(backend.stream, crate::prof::CAT_DENSE_GEMM, prof_start, 0);
            Ok(out)
        })
    }

    /// Exact (erf) GELU — the DINOv3 MLP activation. f32 only.
    pub fn gpu_gelu_erf(x: &GpuTensor) -> Result<GpuTensor, String> {
        if x.half {
            return Err("gpu_gelu_erf is f32-only".to_string());
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(x.rows, x.cols)?;
            let status = unsafe {
                makepad_cuda_gelu_erf_f32(
                    x.device_ptr()?,
                    out.device_ptr()?,
                    (x.rows * x.cols) as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_prof(backend.stream, crate::prof::CAT_ELEMENTWISE, prof_start, 0);
            Ok(out)
        })
    }

    /// Cross-attention over packed rows with kv length independent of the
    /// query length (TRELLIS image-cond cross-attn, DINOv3 would also fit).
    /// Composite path: per q-chunk batched QK^T (f32 accum) -> row softmax ->
    /// batched PV, chunked so the scores buffer stays bounded. f32 only.
    pub fn gpu_attention_packed_cross(
        q: &GpuTensor,
        k: &GpuTensor,
        v: &GpuTensor,
        head_count: usize,
        scale: f32,
    ) -> Result<GpuTensor, String> {
        let hidden = q.cols;
        if head_count == 0 || hidden % head_count != 0 {
            return Err("gpu_attention_cross head mismatch".to_string());
        }
        let head_dim = hidden / head_count;
        if gpu_attention_cross_fused_enabled() && head_dim == 128 {
            let fused = gpu_attention_packed_cross_fused(q, k, v, head_count, scale)?;
            if gpu_attention_compare_enabled() {
                let reference = gpu_attention_packed_cross_composite(
                    q,
                    k,
                    v,
                    head_count,
                    scale,
                    PackedAttentionPrecision::Environment,
                )?;
                let fused_host = gpu_download(&fused)?;
                let reference_host = gpu_download(&reference)?;
                let mut max_abs_diff = 0.0f32;
                let mut max_ref = 0.0f32;
                for (a, b) in fused_host.iter().zip(&reference_host) {
                    max_abs_diff = max_abs_diff.max((a - b).abs());
                    max_ref = max_ref.max(b.abs());
                }
                eprintln!(
                    "FLUX_ATTN_COMPARE cross q={} kv={} heads={head_count} \
                     max_abs_diff={max_abs_diff:.3e} max_ref={max_ref:.3e}",
                    q.rows, k.rows
                );
            }
            return Ok(fused);
        }
        gpu_attention_packed_cross_composite(
            q,
            k,
            v,
            head_count,
            scale,
            PackedAttentionPrecision::Environment,
        )
    }

    /// Expand axial box-RPB `ry[Q,H,heads] + rx[Q,W,heads]` into
    /// `[heads, Q+1, H*W]` (presence row 0 is left zero).
    /// Reinterpret a contiguous f32 tensor as `[rows, cols]` without
    /// copying. The element count must match; the buffer is reused as-is.
    pub fn gpu_reshape(mut x: GpuTensor, rows: usize, cols: usize) -> Result<GpuTensor, String> {
        if x.half {
            return Err("gpu_reshape is f32-only".to_string());
        }
        let src = x.rows.checked_mul(x.cols);
        let dst = rows.checked_mul(cols);
        if src.is_none() || src != dst {
            return Err("gpu_reshape element count mismatch".to_string());
        }
        x.rows = rows;
        x.cols = cols;
        Ok(x)
    }

    unsafe extern "C" {
        fn makepad_cuda_sam3_sine_embed_f32(
            ref_points: *const f32,
            out: *mut f32,
            queries: u32,
            half: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_sam3_rpb_axial_f32(
            ref_points: *const f32,
            dx: *mut f32,
            dy: *mut f32,
            queries: u32,
            width: u32,
            height: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_cuda_sam3_refine_boxes_f32(
            ref_points: *const f32,
            delta: *const f32,
            out: *mut f32,
            n: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;
    }

    /// SAM3 DETR sine query embedding: sigmoid-space boxes `[Q, 4]` to
    /// interleaved sin/cos `[Q, 4*half]` (half = detector dim / 2).
    pub fn gpu_sam3_sine_embed(ref_points: &GpuTensor, half: usize) -> Result<GpuTensor, String> {
        if ref_points.half {
            return Err("gpu_sam3_sine_embed is f32-only".to_string());
        }
        if ref_points.cols != 4 || half == 0 || half % 2 != 0 {
            return Err("gpu_sam3_sine_embed shape mismatch".to_string());
        }
        let queries = ref_points.rows;
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(queries, 4 * half)?;
            let status = unsafe {
                makepad_cuda_sam3_sine_embed_f32(
                    ref_points.device_ptr()?,
                    out.device_ptr()?,
                    queries as u32,
                    half as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            Ok(out)
        })
    }

    /// SAM3 axial box-RPB inputs from sigmoid-space boxes: log-scaled
    /// signed distances `dx [Q*W, 2]`, `dy [Q*H, 2]`.
    pub fn gpu_sam3_rpb_axial(
        ref_points: &GpuTensor,
        width: usize,
        height: usize,
    ) -> Result<(GpuTensor, GpuTensor), String> {
        if ref_points.half {
            return Err("gpu_sam3_rpb_axial is f32-only".to_string());
        }
        if ref_points.cols != 4 || width == 0 || height == 0 {
            return Err("gpu_sam3_rpb_axial shape mismatch".to_string());
        }
        let queries = ref_points.rows;
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let dx = GpuTensor::from_pool(queries * width, 2)?;
            let dy = GpuTensor::from_pool(queries * height, 2)?;
            let status = unsafe {
                makepad_cuda_sam3_rpb_axial_f32(
                    ref_points.device_ptr()?,
                    dx.device_ptr()?,
                    dy.device_ptr()?,
                    queries as u32,
                    width as u32,
                    height as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            Ok((dx, dy))
        })
    }

    /// SAM3 box refinement: `sigmoid(inverse_sigmoid(ref) + delta)`
    /// elementwise over sigmoid-space boxes.
    pub fn gpu_sam3_refine_boxes(
        ref_points: &GpuTensor,
        delta: &GpuTensor,
    ) -> Result<GpuTensor, String> {
        if ref_points.half || delta.half {
            return Err("gpu_sam3_refine_boxes is f32-only".to_string());
        }
        if ref_points.rows != delta.rows || ref_points.cols != delta.cols {
            return Err("gpu_sam3_refine_boxes shape mismatch".to_string());
        }
        let n = ref_points.rows * ref_points.cols;
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(ref_points.rows, ref_points.cols)?;
            let status = unsafe {
                makepad_cuda_sam3_refine_boxes_f32(
                    ref_points.device_ptr()?,
                    delta.device_ptr()?,
                    out.device_ptr()?,
                    n as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            Ok(out)
        })
    }

    unsafe extern "C" {
        fn makepad_cuda_flash_attention2_d64_f32(
            q: *const u16,
            k: *const u16,
            v: *const u16,
            out: *mut f32,
            seq: u32,
            kv_len: u32,
            head_count: u32,
            hidden: u32,
            scale: f32,
            stream: cudaStream_t,
        ) -> cudaError_t;
    }

    /// head_dim-64 FA2 flash attention: register-resident online softmax,
    /// f16 gemm operands, f32 softmax and accumulators (same numerics class
    /// as the d128 FA2 kernel and torch SDPA on f16 models). Q may differ
    /// from K/V in row count; self-attention passes the same tensor.
    pub fn gpu_attention_packed_flash2_d64(
        q: &GpuTensor,
        k: &GpuTensor,
        v: &GpuTensor,
        head_count: usize,
        scale: f32,
    ) -> Result<GpuTensor, String> {
        let q_len = q.rows;
        let kv_len = k.rows;
        let hidden = q.cols;
        if q_len == 0
            || kv_len == 0
            || head_count == 0
            || hidden % head_count != 0
            || hidden / head_count != 64
            || k.cols != hidden
            || v.cols != hidden
            || v.rows != kv_len
        {
            return Err("gpu_attention_packed_flash2_d64 shape mismatch".to_string());
        }
        if q.half || k.half || v.half {
            return Err("gpu_attention_packed_flash2_d64 expects f32 storage".to_string());
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let mut transients = Vec::with_capacity(3);
            let mut as_f16 = |tensor: &GpuTensor| -> Result<*const u16, String> {
                let elems = tensor
                    .rows
                    .checked_mul(tensor.cols)
                    .ok_or_else(|| "flash2_d64 input overflow".to_string())?;
                let buffer = gpu_pool_acquire(elems * size_of::<u16>())?;
                let status = unsafe {
                    makepad_cuda_f32_to_f16(
                        tensor.device_ptr()?,
                        buffer.ptr.as_ptr().cast::<u16>(),
                        elems as u32,
                        backend.stream,
                    )
                };
                gpu_check(status)?;
                let ptr = buffer.ptr.as_ptr().cast::<u16>().cast_const();
                transients.push(buffer);
                Ok(ptr)
            };
            let q_ptr = as_f16(q)?;
            let k_ptr = as_f16(k)?;
            let v_ptr = as_f16(v)?;
            let out = GpuTensor::from_pool(q_len, hidden)?;
            let status = unsafe {
                makepad_cuda_flash_attention2_d64_f32(
                    q_ptr,
                    k_ptr,
                    v_ptr,
                    out.device_ptr()?,
                    q_len as u32,
                    kv_len as u32,
                    head_count as u32,
                    hidden as u32,
                    scale,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            for transient in transients {
                gpu_pool_release(transient);
            }
            gpu_prof(
                backend.stream,
                crate::prof::CAT_FLASH_ATTN,
                prof_start,
                0,
            );
            Ok(out)
        })
    }

    pub fn gpu_rpb_expand(
        ry: &GpuTensor,
        rx: &GpuTensor,
        height: usize,
        width: usize,
        queries: usize,
        heads: usize,
    ) -> Result<GpuTensor, String> {
        if ry.cols != heads || rx.cols != heads {
            return Err("gpu_rpb_expand head mismatch".to_string());
        }
        if ry.rows != queries * height || rx.rows != queries * width {
            return Err("gpu_rpb_expand spatial mismatch".to_string());
        }
        if ry.half || rx.half {
            return Err("gpu_rpb_expand is f32-only".to_string());
        }
        let q1 = queries + 1;
        let hw = height * width;
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(heads * q1, hw)?;
            let status = unsafe {
                makepad_cuda_rpb_expand_f32(
                    ry.device_ptr()?,
                    rx.device_ptr()?,
                    out.device_ptr()?,
                    queries as u32,
                    height as u32,
                    width as u32,
                    heads as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            Ok(out)
        })
    }

    /// Same composite packed-cross path as [`gpu_attention_packed_cross`],
    /// plus an additive score bias of shape `[heads * q_len, kv_len]`
    /// (row-major `h, q, k`). Reuses the existing QK GEMM / softmax / PV
    /// kernels; no new ISA. Used by SAM3 box-RPB decoder attention.
    pub fn gpu_attention_packed_cross_bias(
        q: &GpuTensor,
        k: &GpuTensor,
        v: &GpuTensor,
        head_count: usize,
        scale: f32,
        bias: &GpuTensor,
    ) -> Result<GpuTensor, String> {
        let q_len = q.rows;
        let kv_len = k.rows;
        let hidden = q.cols;
        if k.cols != hidden || v.cols != hidden || v.rows != kv_len {
            return Err("gpu_attention_cross_bias shape mismatch".to_string());
        }
        if head_count == 0 || hidden % head_count != 0 {
            return Err("gpu_attention_cross_bias head mismatch".to_string());
        }
        if q.half || k.half || v.half || bias.half {
            return Err("gpu_attention_cross_bias is f32-only".to_string());
        }
        if bias.rows * bias.cols != head_count * q_len * kv_len {
            return Err("gpu_attention_cross_bias bias length mismatch".to_string());
        }
        let head_dim = hidden / head_count;
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let scores_len = head_count * q_len * kv_len;
            let scores = gpu_pool_acquire(scores_len * size_of::<f32>())?;
            let scores_ptr = scores.ptr.as_ptr().cast::<f32>();
            let beta = 0.0f32;
            let one = 1.0f32;
            unsafe {
                crate::cublas_gemm_strided_batched_ex(
                    backend.blas,
                    crate::CUBLAS_OP_T,
                    crate::CUBLAS_OP_N,
                    kv_len as i32,
                    q_len as i32,
                    head_dim as i32,
                    &scale,
                    k.device_ptr()?.cast::<std::ffi::c_void>(),
                    crate::CUDA_R_32F,
                    hidden as i32,
                    head_dim as i64,
                    q.device_ptr()?.cast::<std::ffi::c_void>(),
                    crate::CUDA_R_32F,
                    hidden as i32,
                    head_dim as i64,
                    &beta,
                    scores_ptr.cast::<std::ffi::c_void>(),
                    crate::CUDA_R_32F,
                    kv_len as i32,
                    (q_len * kv_len) as i64,
                    head_count as i32,
                    crate::CUBLAS_COMPUTE_32F,
                    crate::CUBLAS_GEMM_DEFAULT,
                )
                .map_err(|err| format!("gpu_attention_cross_bias qk gemm failed: {err}"))?;
            }
            let status = unsafe {
                makepad_cuda_add_f32_precise(
                    scores_ptr,
                    bias.device_ptr()?,
                    scores_ptr,
                    scores_len as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            let status = unsafe {
                makepad_cuda_softmax_rows_precise_f32(
                    scores_ptr,
                    scores_ptr,
                    (head_count * q_len) as u32,
                    kv_len as u32,
                    kv_len as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            let out = GpuTensor::from_pool(q_len, hidden)?;
            unsafe {
                crate::cublas_gemm_strided_batched_ex(
                    backend.blas,
                    crate::CUBLAS_OP_N,
                    crate::CUBLAS_OP_N,
                    head_dim as i32,
                    q_len as i32,
                    kv_len as i32,
                    &one,
                    v.device_ptr()?.cast::<std::ffi::c_void>(),
                    crate::CUDA_R_32F,
                    hidden as i32,
                    head_dim as i64,
                    scores_ptr.cast::<std::ffi::c_void>(),
                    crate::CUDA_R_32F,
                    kv_len as i32,
                    (q_len * kv_len) as i64,
                    &beta,
                    out.device_ptr()?.cast::<std::ffi::c_void>(),
                    crate::CUDA_R_32F,
                    hidden as i32,
                    head_dim as i64,
                    head_count as i32,
                    crate::CUBLAS_COMPUTE_32F,
                    crate::CUBLAS_GEMM_DEFAULT,
                )
                .map_err(|err| format!("gpu_attention_cross_bias pv gemm failed: {err}"))?;
            }
            gpu_pool_release(scores);
            gpu_prof(backend.stream, crate::prof::CAT_FLASH_ATTN, prof_start, 0);
            Ok(out)
        })
    }

    /// Cross-attention with explicit BF16 Q/K/V and probability GEMM
    /// operands, f32 accumulation, and f32 output. This follows PyTorch
    /// autocast BF16 independently of the process-wide Flux precision flags.
    pub fn gpu_attention_packed_cross_bf16(
        q: &GpuTensor,
        k: &GpuTensor,
        v: &GpuTensor,
        head_count: usize,
        scale: f32,
    ) -> Result<GpuTensor, String> {
        let hidden = q.cols;
        if k.cols != hidden || v.cols != hidden || v.rows != k.rows {
            return Err("gpu_attention_cross shape mismatch".to_string());
        }
        if head_count == 0 || hidden % head_count != 0 {
            return Err("gpu_attention_cross head mismatch".to_string());
        }
        if q.half || k.half || v.half {
            return Err("gpu_attention_packed_cross_bf16 expects f32 storage".to_string());
        }
        if hidden / head_count == 64 {
            return gpu_attention_packed_flash_bf16(q, k, v, head_count, scale);
        }
        gpu_attention_packed_cross_composite(
            q,
            k,
            v,
            head_count,
            scale,
            PackedAttentionPrecision::Bf16,
        )
    }

    /// Cross-attention through the materialized BF16 cuBLAS/softmax/cuBLAS
    /// implementation, bypassing the head-dimension-64 flash route. This is
    /// an explicit parity-gate entry point and does not change generic routing.
    pub fn gpu_attention_packed_cross_composite_bf16(
        q: &GpuTensor,
        k: &GpuTensor,
        v: &GpuTensor,
        head_count: usize,
        scale: f32,
    ) -> Result<GpuTensor, String> {
        let hidden = q.cols;
        if k.cols != hidden || v.cols != hidden || v.rows != k.rows {
            return Err("gpu_attention_cross shape mismatch".to_string());
        }
        if head_count == 0 || hidden % head_count != 0 {
            return Err("gpu_attention_cross head mismatch".to_string());
        }
        if q.half || k.half || v.half {
            return Err(
                "gpu_attention_packed_cross_composite_bf16 expects f32 storage".to_string(),
            );
        }
        gpu_attention_packed_cross_composite(
            q,
            k,
            v,
            head_count,
            scale,
            PackedAttentionPrecision::Bf16,
        )
    }

    /// Cross-capable BF16 flash attention for head dimension 64. Q/K/V use
    /// token-major f32 storage whose values are explicitly converted to BF16;
    /// the kernel uses BF16 tensor-core products with online f32 softmax and
    /// accumulation. This is shared by SkinTokens VAE, Michelangelo, and skin
    /// decoder self/cross blocks.
    pub fn gpu_attention_packed_flash_bf16(
        q: &GpuTensor,
        k: &GpuTensor,
        v: &GpuTensor,
        head_count: usize,
        scale: f32,
    ) -> Result<GpuTensor, String> {
        let q_len = q.rows;
        let kv_len = k.rows;
        let hidden = q.cols;
        if q_len == 0
            || kv_len == 0
            || head_count == 0
            || hidden / head_count != 64
            || hidden % head_count != 0
            || k.cols != hidden
            || v.cols != hidden
            || v.rows != kv_len
        {
            return Err("gpu_attention_packed_flash_bf16 shape mismatch".to_string());
        }
        if q.half || k.half || v.half {
            return Err("gpu_attention_packed_flash_bf16 expects f32 storage".to_string());
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let mut transients = Vec::with_capacity(3);
            let mut as_bf16 = |tensor: &GpuTensor| -> Result<*const u16, String> {
                let elems = tensor
                    .rows
                    .checked_mul(tensor.cols)
                    .ok_or_else(|| "BF16 flash attention input overflow".to_string())?;
                let buffer = gpu_pool_acquire(elems * size_of::<u16>())?;
                let status = unsafe {
                    makepad_cuda_f32_to_bf16(
                        tensor.device_ptr()?,
                        buffer.ptr.as_ptr().cast::<u16>(),
                        elems as u32,
                        backend.stream,
                    )
                };
                gpu_check(status)?;
                let ptr = buffer.ptr.as_ptr().cast::<u16>().cast_const();
                transients.push(buffer);
                Ok(ptr)
            };
            let q_ptr = as_bf16(q)?;
            let k_ptr = as_bf16(k)?;
            let v_ptr = as_bf16(v)?;
            let out = GpuTensor::from_pool(q_len, hidden)?;
            let status = unsafe {
                makepad_cuda_flash_attention_bf16_d64_f32(
                    q_ptr,
                    k_ptr,
                    v_ptr,
                    out.device_ptr()?,
                    q_len as u32,
                    kv_len as u32,
                    head_count as u32,
                    hidden as u32,
                    scale,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            for transient in transients {
                gpu_pool_release(transient);
            }
            gpu_prof(
                backend.stream,
                crate::prof::CAT_FLASH_ATTN,
                prof_start,
                0,
            );
            Ok(out)
        })
    }

    /// Official `F.sdpa` host: one FLASH launch over `[B,H,Sq,64] x [B,H,Sk,64]`.
    /// `q` is token-major `[B*Sq, H*64]` (same as `view(B,Sq,H,D).transpose`).
    pub fn gpu_sdpa_flash_f16(
        q: &GpuTensor,
        k: &GpuTensor,
        v: &GpuTensor,
        batch: usize,
        heads: usize,
        scale: f32,
    ) -> Result<GpuTensor, String> {
        if batch == 0 || heads == 0 || q.cols != heads * 64 || q.rows % batch != 0 {
            return Err(format!(
                "gpu_sdpa_flash_f16 shape {}x{} batch {batch} heads {heads}",
                q.rows, q.cols
            ));
        }
        let sq = q.rows / batch;
        if k.rows % batch != 0 || v.rows != k.rows || k.cols != q.cols || v.cols != q.cols {
            return Err("gpu_sdpa_flash_f16 kv shape".into());
        }
        let sk = k.rows / batch;
        let q16;
        let k16;
        let v16;
        let q_use = if q.half {
            q
        } else {
            q16 = gpu_to_f16(q)?;
            &q16
        };
        let k_use = if k.half {
            k
        } else {
            k16 = gpu_to_f16(k)?;
            &k16
        };
        let v_use = if v.half {
            v
        } else {
            v16 = gpu_to_f16(v)?;
            &v16
        };
        let hidden = heads * 64;
        let q_row = hidden;
        let k_row = hidden;
        let v_row = hidden;
        let q_batch = sq * hidden;
        let k_batch = sk * hidden;
        let v_batch = sk * hidden;
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool_half(q.rows, q.cols)?;
            gpu_check(unsafe {
                makepad_cuda_sdpa_flash_f16_d64(
                    q_use.device_ptr_u16()?,
                    k_use.device_ptr_u16()?,
                    v_use.device_ptr_u16()?,
                    out.device_ptr_u16()?,
                    batch as u32,
                    sq as u32,
                    sk as u32,
                    heads as u32,
                    q_batch as u32,
                    k_batch as u32,
                    v_batch as u32,
                    q_batch as u32,
                    64,
                    64,
                    64,
                    64,
                    q_row as u32,
                    k_row as u32,
                    v_row as u32,
                    q_row as u32,
                    scale,
                    backend.stream,
                )
            })?;
            gpu_prof(
                backend.stream,
                crate::prof::CAT_FLASH_ATTN,
                prof_start,
                0,
            );
            Ok(out)
        })
    }

    /// Official RA `F.sdpa`: cat(V_alb, V_mr) on last dim so `head_dim_v=128`,
    /// one FLASH, split the 128-wide output back to two hidden packs.
    pub fn gpu_sdpa_flash_f16_wide_v(
        q: &GpuTensor,
        k: &GpuTensor,
        v_alb: &GpuTensor,
        v_mr: &GpuTensor,
        batch: usize,
        heads: usize,
        scale: f32,
    ) -> Result<(GpuTensor, GpuTensor), String> {
        if batch == 0 || heads == 0 || q.cols != heads * 64 || q.rows % batch != 0 {
            return Err(format!(
                "gpu_sdpa_flash_f16_wide_v shape {}x{} batch {batch} heads {heads}",
                q.rows, q.cols
            ));
        }
        if k.rows % batch != 0
            || v_alb.rows != k.rows
            || v_mr.rows != k.rows
            || k.cols != q.cols
            || v_alb.cols != q.cols
            || v_mr.cols != q.cols
        {
            return Err("gpu_sdpa_flash_f16_wide_v kv shape".into());
        }
        let sq = q.rows / batch;
        let sk = k.rows / batch;
        let q16;
        let k16;
        let va16;
        let vm16;
        let q_use = if q.half {
            q
        } else {
            q16 = gpu_to_f16(q)?;
            &q16
        };
        let k_use = if k.half {
            k
        } else {
            k16 = gpu_to_f16(k)?;
            &k16
        };
        let va_use = if v_alb.half {
            v_alb
        } else {
            va16 = gpu_to_f16(v_alb)?;
            &va16
        };
        let vm_use = if v_mr.half {
            v_mr
        } else {
            vm16 = gpu_to_f16(v_mr)?;
            &vm16
        };
        let v_wide = gpu_concat_cols(&[va_use, vm_use])?;
        let hidden = heads * 64;
        let v_hidden = hidden * 2;
        let q_row = hidden;
        let k_row = hidden;
        let v_row = v_hidden;
        let o_row = hidden;
        let q_batch = sq * hidden;
        let k_batch = sk * hidden;
        let v_batch = sk * v_hidden;
        let o_batch = sq * hidden;
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let o_alb = GpuTensor::from_pool_half(q.rows, hidden)?;
            let o_mr = GpuTensor::from_pool_half(q.rows, hidden)?;
            gpu_check(unsafe {
                makepad_cuda_sdpa_flash_f16_d64v128(
                    q_use.device_ptr_u16()?,
                    k_use.device_ptr_u16()?,
                    v_wide.device_ptr_u16()?,
                    o_alb.device_ptr_u16()?,
                    o_mr.device_ptr_u16()?,
                    batch as u32,
                    sq as u32,
                    sk as u32,
                    heads as u32,
                    q_batch as u32,
                    k_batch as u32,
                    v_batch as u32,
                    o_batch as u32,
                    64,
                    64,
                    128,
                    64,
                    q_row as u32,
                    k_row as u32,
                    v_row as u32,
                    o_row as u32,
                    scale,
                    backend.stream,
                )
            })?;
            gpu_prof(
                backend.stream,
                crate::prof::CAT_FLASH_ATTN,
                prof_start,
                0,
            );
            Ok((o_alb, o_mr))
        })
    }

    /// Beam-major one-token grouped-query attention for autoregressive decode.
    /// Q is `[beams, query_heads * head_dim]`; K/V are flattened
    /// `[beams * sequence, kv_heads * head_dim]` so no beam can attend across
    /// another beam's history. All values use f32 storage containing BF16.
    pub fn gpu_attention_gqa_decode_bf16(
        q: &GpuTensor,
        k: &GpuTensor,
        v: &GpuTensor,
        query_heads: usize,
        kv_heads: usize,
        scale: f32,
    ) -> Result<GpuTensor, String> {
        if q.half || k.half || v.half {
            return Err("gpu_attention_gqa_decode_bf16 expects f32 storage".to_string());
        }
        if q.rows == 0
            || query_heads == 0
            || kv_heads == 0
            || query_heads % kv_heads != 0
            || q.cols % query_heads != 0
            || k.cols != v.cols
            || k.rows != v.rows
            || k.cols != kv_heads * (q.cols / query_heads)
            || k.rows % q.rows != 0
        {
            return Err("gpu_attention_gqa_decode_bf16 shape mismatch".to_string());
        }
        let sequence = k.rows / q.rows;
        let head_dim = q.cols / query_heads;
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(q.rows, q.cols)?;
            let status = unsafe {
                makepad_cuda_attention_gqa_decode_bf16_f32(
                    q.device_ptr()?,
                    k.device_ptr()?,
                    v.device_ptr()?,
                    out.device_ptr()?,
                    q.rows as u32,
                    sequence as u32,
                    query_heads as u32,
                    kv_heads as u32,
                    head_dim as u32,
                    scale,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_prof(
                backend.stream,
                crate::prof::CAT_FLASH_ATTN,
                prof_start,
                0,
            );
            Ok(out)
        })
    }

    /// Pair (cond, uncond) GQA decode reading rows `0..sequence` of the four
    /// separate `[cap, kv_heads*head_dim]` caches in place — no per-frame
    /// concat. Byte-identical to `gpu_attention_gqa_decode_bf16` on the
    /// row-concatenated caches; parallel over positions instead of serial.
    /// `q` is `[2, query_heads*head_dim]` (cond row 0, uncond row 1).
    pub fn gpu_attention_gqa_decode_pair_bf16(
        q: &GpuTensor,
        k_cond: &GpuTensor,
        v_cond: &GpuTensor,
        k_uncond: &GpuTensor,
        v_uncond: &GpuTensor,
        sequence: usize,
        query_heads: usize,
        kv_heads: usize,
        scale: f32,
    ) -> Result<GpuTensor, String> {
        if q.half || k_cond.half || v_cond.half || k_uncond.half || v_uncond.half {
            return Err("gpu_attention_gqa_decode_pair_bf16 expects f32 storage".to_string());
        }
        if q.rows != 2
            || sequence == 0
            || query_heads == 0
            || kv_heads == 0
            || query_heads % kv_heads != 0
            || q.cols % query_heads != 0
        {
            return Err("gpu_attention_gqa_decode_pair_bf16 shape mismatch".to_string());
        }
        let head_dim = q.cols / query_heads;
        let kv_width = kv_heads * head_dim;
        for cache in [k_cond, v_cond, k_uncond, v_uncond] {
            if cache.cols != kv_width || cache.rows < sequence {
                return Err("gpu_attention_gqa_decode_pair_bf16 cache shape".to_string());
            }
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            // Pad the per-call dots scratch to a 256-position bucket: the AR
            // loop grows `sequence` every frame, and an exact-size request
            // would miss the pool (one fresh device alloc per frame, ~1500
            // dead sizes over a song). The kernel indexes rows by the real
            // `sequence`, so the slack bytes are never read.
            let dots_cols = sequence.div_ceil(256) * 256;
            let dots = GpuTensor::from_pool(2 * query_heads, dots_cols)?;
            let out = GpuTensor::from_pool(2, q.cols)?;
            let status = unsafe {
                makepad_cuda_attention_gqa_decode_pair_bf16_f32(
                    q.device_ptr()?,
                    k_cond.device_ptr()?,
                    v_cond.device_ptr()?,
                    k_uncond.device_ptr()?,
                    v_uncond.device_ptr()?,
                    dots.device_ptr()?,
                    out.device_ptr()?,
                    sequence as u32,
                    query_heads as u32,
                    kv_heads as u32,
                    head_dim as u32,
                    scale,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_prof(
                backend.stream,
                crate::prof::CAT_FLASH_ATTN,
                prof_start,
                0,
            );
            Ok(out)
        })
    }

    /// FLUX_ATTN_CROSS=0 falls back to the composite cross path (also
    /// disabled whenever the fused kernels are off globally). Public so
    /// callers holding per-stage KV caches can store them f16 (the fused
    /// kernel's native operand type) only when the fused path will run.
    pub fn gpu_attention_cross_fused_enabled() -> bool {
        if !gpu_attention_fused_enabled() {
            return false;
        }
        match std::env::var("FLUX_ATTN_CROSS") {
            Ok(value) => value != "0",
            Err(_) => true,
        }
    }

    /// The fused FA2 cross kernel (head_dim 128, kv_len independent of
    /// q_len): no materialized scores — the composite path writes + reads
    /// heads*q_len*kv_len floats three times per call, which at TRELLIS HR
    /// scale (13.4k q x 4.1k kv x 12 heads x 30 blocks) is the cross-attn
    /// whale. f32 inputs convert to f16 transients (the kernel's native
    /// operand type); f16 inputs feed straight through.
    fn gpu_attention_packed_cross_fused(
        q: &GpuTensor,
        k: &GpuTensor,
        v: &GpuTensor,
        head_count: usize,
        scale: f32,
    ) -> Result<GpuTensor, String> {
        let q_len = q.rows;
        let kv_len = k.rows;
        let hidden = q.cols;
        if k.cols != hidden || v.cols != hidden || v.rows != kv_len {
            return Err("gpu_attention_cross shape mismatch".to_string());
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let mut transient: Vec<DeviceBuffer> = Vec::new();
            let mut as_half = |tensor: &GpuTensor| -> Result<*const u16, String> {
                if tensor.half {
                    return tensor.device_ptr_u16().map(|ptr| ptr.cast_const());
                }
                let elems = tensor.rows * tensor.cols;
                let buffer = gpu_pool_acquire(elems * size_of::<u16>())?;
                let status = unsafe {
                    makepad_cuda_f32_to_f16(
                        tensor.device_ptr()?,
                        buffer.ptr.as_ptr().cast::<u16>(),
                        elems as u32,
                        backend.stream,
                    )
                };
                gpu_check(status)?;
                let ptr = buffer.ptr.as_ptr().cast::<u16>().cast_const();
                transient.push(buffer);
                Ok(ptr)
            };
            let q_ptr = as_half(q)?;
            let k_ptr = as_half(k)?;
            let v_ptr = as_half(v)?;
            let out = GpuTensor::from_pool(q_len, hidden)?;
            let status = unsafe {
                makepad_cuda_flash_attention2_cross_f32(
                    q_ptr,
                    k_ptr,
                    v_ptr,
                    out.device_ptr()?,
                    q_len as u32,
                    kv_len as u32,
                    head_count as u32,
                    hidden as u32,
                    scale,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            for buffer in transient {
                gpu_pool_release(buffer);
            }
            gpu_prof(backend.stream, crate::prof::CAT_FLASH_ATTN, prof_start, 0);
            Ok(out)
        })
    }

    fn gpu_attention_packed_cross_composite(
        q: &GpuTensor,
        k: &GpuTensor,
        v: &GpuTensor,
        head_count: usize,
        scale: f32,
        precision: PackedAttentionPrecision,
    ) -> Result<GpuTensor, String> {
        let q_len = q.rows;
        let kv_len = k.rows;
        let hidden = q.cols;
        if k.cols != hidden || v.cols != hidden || v.rows != kv_len {
            return Err("gpu_attention_cross shape mismatch".to_string());
        }
        if head_count == 0 || hidden % head_count != 0 {
            return Err("gpu_attention_cross head mismatch".to_string());
        }
        if q.half || k.half || v.half {
            return Err("gpu_attention_cross is f32-only".to_string());
        }
        if precision == PackedAttentionPrecision::F16 {
            return Err("gpu_attention_cross explicit f16 is not implemented".to_string());
        }
        let head_dim = hidden / head_count;
        let use_bf16 = precision == PackedAttentionPrecision::Bf16;
        // Bound the scores buffer at ~64M floats (256MB).
        const MAX_SCORE_ELEMS: usize = 64 * 1024 * 1024;
        let chunk_rows = (MAX_SCORE_ELEMS / (head_count * kv_len).max(1))
            .clamp(1, q_len.max(1));
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(q_len, hidden)?;
            let scores = gpu_pool_acquire(head_count * chunk_rows * kv_len * size_of::<f32>())?;
            let scores_ptr = scores.ptr.as_ptr().cast::<f32>();
            let mut bf16: Option<(DeviceBuffer, DeviceBuffer, DeviceBuffer, DeviceBuffer)> = None;
            if use_bf16 {
                let q16 = gpu_pool_acquire(q_len * hidden * size_of::<u16>())?;
                let k16 = gpu_pool_acquire(kv_len * hidden * size_of::<u16>())?;
                let v16 = gpu_pool_acquire(kv_len * hidden * size_of::<u16>())?;
                let p16 = gpu_pool_acquire(
                    head_count * chunk_rows * kv_len * size_of::<u16>(),
                )?;
                for (src, dst, elems) in [
                    (q, &q16, q_len * hidden),
                    (k, &k16, kv_len * hidden),
                    (v, &v16, kv_len * hidden),
                ] {
                    let status = unsafe {
                        makepad_cuda_f32_to_bf16(
                            src.device_ptr()?,
                            dst.ptr.as_ptr().cast::<u16>(),
                            elems as u32,
                            backend.stream,
                        )
                    };
                    gpu_check(status)?;
                }
                bf16 = Some((q16, k16, v16, p16));
            }
            let beta = 0.0f32;
            let one = 1.0f32;
            let mut start = 0usize;
            while start < q_len {
                let rows = chunk_rows.min(q_len - start);
                let (k_ptr, q_ptr, input_type) = if let Some((q16, k16, _, _)) = &bf16 {
                    (
                        k16.ptr.as_ptr(),
                        unsafe { q16.ptr.as_ptr().cast::<u16>().add(start * hidden) }
                            .cast::<std::ffi::c_void>(),
                        crate::CUDA_R_16BF,
                    )
                } else {
                    (
                        k.device_ptr()?.cast::<std::ffi::c_void>(),
                        unsafe { q.device_ptr()?.add(start * hidden) }
                            .cast::<std::ffi::c_void>(),
                        crate::CUDA_R_32F,
                    )
                };
                unsafe {
                    // scores[h][i][j] = scale * sum_d q[start+i][h][d] * k[j][h][d]
                    crate::cublas_gemm_strided_batched_ex(
                        backend.blas,
                        crate::CUBLAS_OP_T,
                        crate::CUBLAS_OP_N,
                        kv_len as i32,
                        rows as i32,
                        head_dim as i32,
                        &scale,
                        k_ptr,
                        input_type,
                        hidden as i32,
                        head_dim as i64,
                        q_ptr,
                        input_type,
                        hidden as i32,
                        head_dim as i64,
                        &beta,
                        scores_ptr.cast::<std::ffi::c_void>(),
                        crate::CUDA_R_32F,
                        kv_len as i32,
                        (rows * kv_len) as i64,
                        head_count as i32,
                        crate::CUBLAS_COMPUTE_32F,
                        crate::CUBLAS_GEMM_DEFAULT,
                    )
                    .map_err(|err| format!("gpu_attention_cross qk gemm failed: {err}"))?;
                }
                let status = unsafe {
                    makepad_cuda_softmax_rows_precise_f32(
                        scores_ptr,
                        scores_ptr,
                        (head_count * rows) as u32,
                        kv_len as u32,
                        kv_len as u32,
                        backend.stream,
                    )
                };
                gpu_check(status)?;
                if let Some((_, _, v16, p16)) = &bf16 {
                    let score_elems = head_count * rows * kv_len;
                    let status = unsafe {
                        makepad_cuda_f32_to_bf16(
                            scores_ptr,
                            p16.ptr.as_ptr().cast::<u16>(),
                            score_elems as u32,
                            backend.stream,
                        )
                    };
                    gpu_check(status)?;
                    unsafe {
                        // out[start+i][h][d] = sum_j probs[h][i][j] * v[j][h][d]
                        crate::cublas_gemm_strided_batched_ex(
                            backend.blas,
                            crate::CUBLAS_OP_N,
                            crate::CUBLAS_OP_N,
                            head_dim as i32,
                            rows as i32,
                            kv_len as i32,
                            &one,
                            v16.ptr.as_ptr(),
                            crate::CUDA_R_16BF,
                            hidden as i32,
                            head_dim as i64,
                            p16.ptr.as_ptr(),
                            crate::CUDA_R_16BF,
                            kv_len as i32,
                            (rows * kv_len) as i64,
                            &beta,
                            out.device_ptr()?.add(start * hidden).cast::<std::ffi::c_void>(),
                            crate::CUDA_R_32F,
                            hidden as i32,
                            head_dim as i64,
                            head_count as i32,
                            crate::CUBLAS_COMPUTE_32F,
                            crate::CUBLAS_GEMM_DEFAULT,
                        )
                        .map_err(|err| format!("gpu_attention_cross pv gemm failed: {err}"))?;
                    }
                } else {
                    unsafe {
                    // out[start+i][h][d] = sum_j probs[h][i][j] * v[j][h][d]
                    crate::cublas_gemm_strided_batched_ex(
                        backend.blas,
                        crate::CUBLAS_OP_N,
                        crate::CUBLAS_OP_N,
                        head_dim as i32,
                        rows as i32,
                        kv_len as i32,
                        &one,
                        v.device_ptr()?.cast::<std::ffi::c_void>(),
                        crate::CUDA_R_32F,
                        hidden as i32,
                        head_dim as i64,
                        scores_ptr.cast::<std::ffi::c_void>(),
                        crate::CUDA_R_32F,
                        kv_len as i32,
                        (rows * kv_len) as i64,
                        &beta,
                        out.device_ptr()?.add(start * hidden).cast::<std::ffi::c_void>(),
                        crate::CUDA_R_32F,
                        hidden as i32,
                        head_dim as i64,
                        head_count as i32,
                        crate::CUBLAS_COMPUTE_32F,
                        crate::CUBLAS_GEMM_DEFAULT,
                    )
                    .map_err(|err| format!("gpu_attention_cross pv gemm failed: {err}"))?;
                    }
                }
                start += rows;
            }
            if let Some((q16, k16, v16, p16)) = bf16 {
                gpu_pool_release(q16);
                gpu_pool_release(k16);
                gpu_pool_release(v16);
                gpu_pool_release(p16);
            }
            gpu_pool_release(scores);
            gpu_prof(backend.stream, crate::prof::CAT_FLASH_ATTN, prof_start, 0);
            Ok(out)
        })
    }

    /// Interleaved-pair RoPE on token-major [token][head][dim] data.
    pub fn gpu_rope_interleaved(
        x: &GpuTensor,
        head_count: usize,
        cos_table: &GpuTensor,
        sin_table: &GpuTensor,
    ) -> Result<GpuTensor, String> {
        if head_count == 0 || x.cols % head_count != 0 {
            return Err("gpu_rope head mismatch".to_string());
        }
        let head_dim = x.cols / head_count;
        if head_dim % 2 != 0 {
            return Err("gpu_rope odd head dim".to_string());
        }
        let half_dim = head_dim / 2;
        if cos_table.rows != x.rows
            || cos_table.cols != half_dim
            || sin_table.rows != x.rows
            || sin_table.cols != half_dim
        {
            return Err("gpu_rope table mismatch".to_string());
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = if x.half {
                GpuTensor::from_pool_half(x.rows, x.cols)?
            } else {
                GpuTensor::from_pool(x.rows, x.cols)?
            };
            let status = if x.half {
                unsafe {
                    makepad_cuda_rope_interleaved_f16(
                        x.device_ptr_u16()?,
                        cos_table.device_ptr()?,
                        sin_table.device_ptr()?,
                        out.device_ptr_u16()?,
                        x.rows as u32,
                        head_count as u32,
                        half_dim as u32,
                        backend.stream,
                    )
                }
            } else {
                unsafe {
                    makepad_cuda_rope_interleaved_f32(
                        x.device_ptr()?,
                        cos_table.device_ptr()?,
                        sin_table.device_ptr()?,
                        out.device_ptr()?,
                        x.rows as u32,
                        head_count as u32,
                        half_dim as u32,
                        backend.stream,
                    )
                }
            };
            gpu_check(status)?;
            gpu_prof(backend.stream, crate::prof::CAT_ELEMENTWISE, prof_start, 0);
            Ok(out)
        })
    }

    // --- MiniMax H3 device ops --------------------------------------------

    /// Upload raw u32 words (row indices for the indexed AdaLN kernels) into
    /// a pooled device buffer. The tensor's rows/cols track the word count.
    pub fn gpu_upload_u32(values: &[u32]) -> Result<GpuTensor, String> {
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let tensor = GpuTensor::from_pool(values.len().max(1), 1)?;
            let raw = unsafe {
                std::slice::from_raw_parts(
                    values.as_ptr().cast::<u8>(),
                    values.len() * size_of::<u32>(),
                )
            };
            tensor
                .buf
                .as_ref()
                .expect("freshly pooled GPU tensor")
                .write(raw, backend.stream)?;
            Ok(tensor)
        })
    }

    /// Rotate-half RoPE over the leading `2 * rot_half` channels of every
    /// head (H3 DiT rotates 96 of 128; the Qwen3-VL text encoder rotates all
    /// 128). cos/sin tables are (rows, rot_half); both rotated halves share
    /// one table entry.
    pub fn gpu_rope_half(
        x: &GpuTensor,
        head_count: usize,
        rot_half: usize,
        cos_table: &GpuTensor,
        sin_table: &GpuTensor,
    ) -> Result<GpuTensor, String> {
        if head_count == 0 || x.cols % head_count != 0 {
            return Err("gpu_rope_half head mismatch".to_string());
        }
        let head_dim = x.cols / head_count;
        if rot_half * 2 > head_dim {
            return Err("gpu_rope_half rotary span exceeds head dim".to_string());
        }
        if cos_table.rows != x.rows
            || cos_table.cols != rot_half
            || sin_table.rows != x.rows
            || sin_table.cols != rot_half
        {
            return Err("gpu_rope_half table mismatch".to_string());
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = if x.half {
                GpuTensor::from_pool_half(x.rows, x.cols)?
            } else {
                GpuTensor::from_pool(x.rows, x.cols)?
            };
            let status = if x.half {
                unsafe {
                    makepad_cuda_rope_half_f16(
                        x.device_ptr_u16()?,
                        cos_table.device_ptr()?,
                        sin_table.device_ptr()?,
                        out.device_ptr_u16()?,
                        x.rows as u32,
                        head_count as u32,
                        head_dim as u32,
                        rot_half as u32,
                        backend.stream,
                    )
                }
            } else {
                unsafe {
                    makepad_cuda_rope_half_f32(
                        x.device_ptr()?,
                        cos_table.device_ptr()?,
                        sin_table.device_ptr()?,
                        out.device_ptr()?,
                        x.rows as u32,
                        head_count as u32,
                        head_dim as u32,
                        rot_half as u32,
                        backend.stream,
                    )
                }
            };
            gpu_check(status)?;
            gpu_prof(backend.stream, crate::prof::CAT_ELEMENTWISE, prof_start, 0);
            Ok(out)
        })
    }

    /// Fused `gpu_rope_half` (f32 kernel) + `gpu_bf16_round`: identical
    /// rotation expressions with the bf16 round moved onto the stores
    /// (pass-through channels rounded too, matching the separate full-tensor
    /// round pass). Bit-identical to the two-kernel recipe.
    pub fn gpu_rope_half_round_bf16(
        x: &GpuTensor,
        head_count: usize,
        rot_half: usize,
        cos_table: &GpuTensor,
        sin_table: &GpuTensor,
    ) -> Result<GpuTensor, String> {
        if x.half || cos_table.half || sin_table.half {
            return Err("gpu_rope_half_round_bf16 expects f32 storage".to_string());
        }
        if head_count == 0 || x.cols % head_count != 0 {
            return Err("gpu_rope_half_round_bf16 head mismatch".to_string());
        }
        let head_dim = x.cols / head_count;
        if rot_half * 2 > head_dim {
            return Err("gpu_rope_half_round_bf16 rotary span exceeds head dim".to_string());
        }
        if cos_table.rows != x.rows
            || cos_table.cols != rot_half
            || sin_table.rows != x.rows
            || sin_table.cols != rot_half
        {
            return Err("gpu_rope_half_round_bf16 table mismatch".to_string());
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(x.rows, x.cols)?;
            let status = unsafe {
                makepad_cuda_rope_half_round_bf16_f32(
                    x.device_ptr()?,
                    cos_table.device_ptr()?,
                    sin_table.device_ptr()?,
                    out.device_ptr()?,
                    x.rows as u32,
                    head_count as u32,
                    head_dim as u32,
                    rot_half as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_prof(backend.stream, crate::prof::CAT_ELEMENTWISE, prof_start, 0);
            Ok(out)
        })
    }


    /// Rotate-half RoPE with PyTorch BF16 expression boundaries: both
    /// products round to BF16 before add/subtract and the result rounds once
    /// more. Inputs/tables/outputs use f32 storage containing BF16 values.
    pub fn gpu_rope_half_bf16(
        x: &GpuTensor,
        head_count: usize,
        rot_half: usize,
        cos_table: &GpuTensor,
        sin_table: &GpuTensor,
    ) -> Result<GpuTensor, String> {
        if x.half || cos_table.half || sin_table.half {
            return Err("gpu_rope_half_bf16 expects f32 storage".to_string());
        }
        if head_count == 0 || x.cols % head_count != 0 {
            return Err("gpu_rope_half_bf16 head mismatch".to_string());
        }
        let head_dim = x.cols / head_count;
        if rot_half * 2 > head_dim {
            return Err("gpu_rope_half_bf16 rotary span exceeds head dim".to_string());
        }
        if cos_table.rows != x.rows
            || cos_table.cols != rot_half
            || sin_table.rows != x.rows
            || sin_table.cols != rot_half
        {
            return Err("gpu_rope_half_bf16 table mismatch".to_string());
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(x.rows, x.cols)?;
            let status = unsafe {
                makepad_cuda_rope_half_bf16_f32(
                    x.device_ptr()?,
                    cos_table.device_ptr()?,
                    sin_table.device_ptr()?,
                    out.device_ptr()?,
                    x.rows as u32,
                    head_count as u32,
                    head_dim as u32,
                    rot_half as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_prof(backend.stream, crate::prof::CAT_ELEMENTWISE, prof_start, 0);
            Ok(out)
        })
    }

    /// RMS norm (weighted, f32 math) + per-row AdaLN modulation selected from
    /// a device table: y = rmsnorm(x)*w*(1+scale[idx[row]]) + shift[idx[row]].
    /// `table_stride` is the element stride of one table row; `scale_off` /
    /// `shift_off` are element offsets of the scale/shift chunks in that row.
    #[allow(clippy::too_many_arguments)]
    pub fn gpu_rms_norm_mod_indexed(
        x: &GpuTensor,
        weight: &GpuTensor,
        table: &GpuTensor,
        idx: &GpuTensor,
        table_stride: usize,
        scale_off: usize,
        shift_off: usize,
        eps: f32,
        out_half: bool,
    ) -> Result<GpuTensor, String> {
        if x.half {
            return Err("gpu_rms_norm_mod_indexed expects f32 input".to_string());
        }
        if weight.rows * weight.cols != x.cols {
            return Err("gpu_rms_norm_mod_indexed weight mismatch".to_string());
        }
        if idx.rows * idx.cols < x.rows {
            return Err("gpu_rms_norm_mod_indexed idx mismatch".to_string());
        }
        if scale_off + x.cols > table_stride || shift_off + x.cols > table_stride {
            return Err("gpu_rms_norm_mod_indexed offset out of range".to_string());
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = if out_half {
                GpuTensor::from_pool_half(x.rows, x.cols)?
            } else {
                GpuTensor::from_pool(x.rows, x.cols)?
            };
            let status = if out_half {
                unsafe {
                    makepad_cuda_rms_norm_mod_indexed_out16(
                        x.device_ptr()?,
                        weight.device_ptr()?,
                        table.device_ptr()?,
                        idx.device_ptr()?.cast::<u32>(),
                        out.device_ptr_u16()?,
                        x.rows as u32,
                        x.cols as u32,
                        table_stride as u32,
                        scale_off as u32,
                        shift_off as u32,
                        eps,
                        backend.stream,
                    )
                }
            } else {
                unsafe {
                    makepad_cuda_rms_norm_mod_indexed_f32(
                        x.device_ptr()?,
                        weight.device_ptr()?,
                        table.device_ptr()?,
                        idx.device_ptr()?.cast::<u32>(),
                        out.device_ptr()?,
                        x.rows as u32,
                        x.cols as u32,
                        table_stride as u32,
                        scale_off as u32,
                        shift_off as u32,
                        eps,
                        backend.stream,
                    )
                }
            };
            gpu_check(status)?;
            gpu_prof(backend.stream, crate::prof::CAT_RMS_NORM, prof_start, 0);
            Ok(out)
        })
    }

    /// residual + gate[idx[row]] * update with the gate selected per row from
    /// a device AdaLN table.
    pub fn gpu_gated_residual_indexed(
        residual: &GpuTensor,
        update: &GpuTensor,
        table: &GpuTensor,
        idx: &GpuTensor,
        table_stride: usize,
        gate_off: usize,
    ) -> Result<GpuTensor, String> {
        if residual.half || update.half {
            return Err("gpu_gated_residual_indexed expects f32 tensors".to_string());
        }
        if residual.rows != update.rows || residual.cols != update.cols {
            return Err("gpu_gated_residual_indexed shape mismatch".to_string());
        }
        if gate_off + residual.cols > table_stride {
            return Err("gpu_gated_residual_indexed offset out of range".to_string());
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(residual.rows, residual.cols)?;
            let status = unsafe {
                makepad_cuda_gated_residual_indexed_f32(
                    residual.device_ptr()?,
                    update.device_ptr()?,
                    table.device_ptr()?,
                    idx.device_ptr()?.cast::<u32>(),
                    out.device_ptr()?,
                    residual.rows as u32,
                    residual.cols as u32,
                    table_stride as u32,
                    gate_off as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_prof(backend.stream, crate::prof::CAT_ELEMENTWISE, prof_start, 0);
            Ok(out)
        })
    }

    /// Value-first SwiGLU on (rows, 2n): out = x[:, :n] * silu(x[:, n:]).
    pub fn gpu_swiglu_value_gate(x: &GpuTensor) -> Result<GpuTensor, String> {
        if x.cols % 2 != 0 {
            return Err("gpu_swiglu_value_gate odd column count".to_string());
        }
        let n = x.cols / 2;
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = if x.half {
                GpuTensor::from_pool_half(x.rows, n)?
            } else {
                GpuTensor::from_pool(x.rows, n)?
            };
            let status = if x.half {
                unsafe {
                    makepad_cuda_swiglu_value_gate_f16(
                        x.device_ptr_u16()?,
                        out.device_ptr_u16()?,
                        x.rows as u32,
                        n as u32,
                        backend.stream,
                    )
                }
            } else {
                unsafe {
                    makepad_cuda_swiglu_value_gate_f32(
                        x.device_ptr()?,
                        out.device_ptr()?,
                        x.rows as u32,
                        n as u32,
                        backend.stream,
                    )
                }
            };
            gpu_check(status)?;
            gpu_prof(backend.stream, crate::prof::CAT_ELEMENTWISE, prof_start, 0);
            Ok(out)
        })
    }

    /// Gate-first SwiGLU on a column slab of `x` read in place:
    /// `out[r, c] = silu(x[r, gate_offset + c]) * x[r, gate_offset + n + c]`
    /// with the exact silu/mul arithmetic of `gpu_swiglu_value_gate`.
    /// Replaces slice+slice+swap-concat+swiglu (4 launches, ~3 extra
    /// full-tensor passes) with one strided read. f32 only.
    pub fn gpu_swiglu_gate_first(
        x: &GpuTensor,
        gate_offset: usize,
        n: usize,
    ) -> Result<GpuTensor, String> {
        if x.half {
            return Err("gpu_swiglu_gate_first expects f32 storage".to_string());
        }
        if n == 0 || gate_offset + 2 * n > x.cols {
            return Err(format!(
                "gpu_swiglu_gate_first slab {gate_offset}+2*{n} outside {} cols",
                x.cols
            ));
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(x.rows, n)?;
            let status = unsafe {
                makepad_cuda_swiglu_gate_first_strided_f32(
                    x.device_ptr()?,
                    out.device_ptr()?,
                    x.rows as u32,
                    x.cols as u32,
                    gate_offset as u32,
                    n as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_prof(backend.stream, crate::prof::CAT_ELEMENTWISE, prof_start, 0);
            Ok(out)
        })
    }

    /// Dense bf16 linear consuming a GpuBf16Buf input (no staging pass) and
    /// expanding the bf16 gemm output to f32 — bit-identical to staging the
    /// same values from f32 storage.
    pub fn gpu_linear_nt_cached_bf16_mm_from_buf(
        x: &GpuBf16Buf,
        cache_namespace: &str,
        parts: &[GpuLinearPart<'_>],
    ) -> Result<GpuTensor, String> {
        if parts.len() != 1 || parts[0].bt_ggml_type != GGML_TYPE_BF16 {
            return Err(
                "gpu_linear_nt_cached_bf16_mm_from_buf requires one BF16 weight part".to_string(),
            );
        }
        let part = &parts[0];
        let (m, k, n) = (x.rows, x.cols, part.n);
        if m == 0 || k == 0 || n == 0 {
            return Err(format!(
                "gpu_linear_nt_cached_bf16_mm_from_buf empty shape: x={m}x{k} n={n}",
            ));
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let weight_bytes = n
                .checked_mul(k)
                .and_then(|len| len.checked_mul(size_of::<u16>()))
                .ok_or_else(|| "PyTorch BF16 mm weight size overflow".to_string())?;
            let weight_key = format!("{cache_namespace}::{}", part.cache_key);
            backend.cached_weight_buffer(&weight_key, weight_bytes, || {
                Ok(part.bytes.to_vec())
            })?;
            let output_values = m
                .checked_mul(n)
                .ok_or_else(|| "PyTorch BF16 mm output size overflow".to_string())?;
            let output_bf16 = gpu_pool_acquire(output_values * size_of::<u16>())?;
            let output = GpuTensor::from_pool(m, n)?;
            let (weight_ptr, _) = backend.weight_ptr(&weight_key)?;
            let alpha = 1.0f32;
            let beta = 0.0f32;
            unsafe {
                crate::cublas_gemm_ex(
                    backend.blas,
                    crate::CUBLAS_OP_T,
                    crate::CUBLAS_OP_N,
                    n as i32,
                    m as i32,
                    k as i32,
                    &alpha,
                    weight_ptr,
                    crate::CUDA_R_16BF,
                    k as i32,
                    x.device_ptr_u16()?.cast::<std::ffi::c_void>(),
                    crate::CUDA_R_16BF,
                    k as i32,
                    &beta,
                    output_bf16.ptr.as_ptr(),
                    crate::CUDA_R_16BF,
                    n as i32,
                    crate::CUDA_R_32F,
                    crate::CUBLAS_GEMM_DEFAULT_TENSOR_OP,
                )
            }
            .map_err(|err| format!("PyTorch BF16 mm failed: m={m} k={k} n={n}: {err}"))?;
            let status = unsafe {
                makepad_cuda_bf16_to_f32(
                    output_bf16.ptr.as_ptr().cast::<u16>(),
                    output.device_ptr()?,
                    output_values as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_pool_release(output_bf16);
            gpu_prof(
                backend.stream,
                crate::prof::CAT_DENSE_TOTAL,
                prof_start,
                0,
            );
            Ok(output)
        })
    }

    /// Dense bf16 linear from GpuBf16Buf input straight to a GpuBf16Buf
    /// output: no staging, no expansion — the gemm's own bf16 C is the
    /// result.
    pub fn gpu_linear_nt_cached_bf16_mm_from_buf_to_buf(
        x: &GpuBf16Buf,
        cache_namespace: &str,
        parts: &[GpuLinearPart<'_>],
    ) -> Result<GpuBf16Buf, String> {
        if parts.len() != 1 || parts[0].bt_ggml_type != GGML_TYPE_BF16 {
            return Err(
                "gpu_linear_nt_cached_bf16_mm_from_buf_to_buf requires one BF16 weight part"
                    .to_string(),
            );
        }
        let part = &parts[0];
        let (m, k, n) = (x.rows, x.cols, part.n);
        if m == 0 || k == 0 || n == 0 {
            return Err(format!(
                "gpu_linear_nt_cached_bf16_mm_from_buf_to_buf empty shape: x={m}x{k} n={n}",
            ));
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let weight_bytes = n
                .checked_mul(k)
                .and_then(|len| len.checked_mul(size_of::<u16>()))
                .ok_or_else(|| "PyTorch BF16 mm weight size overflow".to_string())?;
            let weight_key = format!("{cache_namespace}::{}", part.cache_key);
            backend.cached_weight_buffer(&weight_key, weight_bytes, || {
                Ok(part.bytes.to_vec())
            })?;
            let output = GpuBf16Buf::from_pool(m, n)?;
            let (weight_ptr, _) = backend.weight_ptr(&weight_key)?;
            let alpha = 1.0f32;
            let beta = 0.0f32;
            unsafe {
                crate::cublas_gemm_ex(
                    backend.blas,
                    crate::CUBLAS_OP_T,
                    crate::CUBLAS_OP_N,
                    n as i32,
                    m as i32,
                    k as i32,
                    &alpha,
                    weight_ptr,
                    crate::CUDA_R_16BF,
                    k as i32,
                    x.device_ptr_u16()?.cast::<std::ffi::c_void>(),
                    crate::CUDA_R_16BF,
                    k as i32,
                    &beta,
                    output.device_ptr_u16()?.cast::<std::ffi::c_void>(),
                    crate::CUDA_R_16BF,
                    n as i32,
                    crate::CUDA_R_32F,
                    crate::CUBLAS_GEMM_DEFAULT_TENSOR_OP,
                )
            }
            .map_err(|err| format!("PyTorch BF16 mm failed: m={m} k={k} n={n}: {err}"))?;
            gpu_prof(
                backend.stream,
                crate::prof::CAT_DENSE_TOTAL,
                prof_start,
                0,
            );
            Ok(output)
        })
    }

    /// One streamed weight: qualified cache key + pinned host copy.
    struct RingTensor {
        key: String,
        len: usize,
        host: std::ptr::NonNull<c_void>,
    }

    // The pinned pointers are only touched from the dense-backend thread
    // (thread-local backend), but the struct must be Send-safe for the
    // OnceLock plumbing types; uploads never alias.
    unsafe impl Send for RingTensor {}

    /// Double-buffered weight streamer: N groups of identically-shaped
    /// tensors rotate through 2 device slot-sets; group g lives in slot g%2.
    /// `advance(g)` records g's compute-done fence on the main stream, then
    /// prefetches group (g+2)%N into the freed slot on a dedicated copy
    /// stream — the upload of the NEXT same-parity group overlaps the
    /// compute of everything in between (the other-parity block plus, at
    /// step boundaries, the whole single-block phase).
    struct FluxStreamRing {
        groups: Vec<Vec<RingTensor>>,
        /// Per-tensor byte lengths (identical across groups) — slots re-alloc
        /// from this after a release.
        shape: Vec<usize>,
        slots: [Vec<DeviceBuffer>; 2],
        copy_stream: cudaStream_t,
        upload_done: [crate::cudaEvent_t; 2],
        compute_done: [crate::cudaEvent_t; 2],
        /// Group currently (or being) uploaded per slot; -1 = none.
        resident: [i64; 2],
        /// Whether the compute stream has already waited this slot's upload.
        upload_waited: [bool; 2],
    }

    impl CudaDenseLinearBackend {
        /// Device pointer for `key` when it is ring-resident.
        fn ring_weight_ptr(&self, key: &str) -> Option<(*mut c_void, usize)> {
            let ring = self.stream_ring.as_ref()?;
            for slot in 0..2 {
                let group = ring.resident[slot];
                if group < 0 {
                    continue;
                }
                for (index, tensor) in ring.groups[group as usize].iter().enumerate() {
                    if tensor.key == key {
                        return Some((
                            ring.slots[slot][index].ptr.as_ptr(),
                            tensor.len,
                        ));
                    }
                }
            }
            None
        }

        /// GEMM-side weight lookup: ring slots first (waiting the slot's
        /// upload fence once), then the ordinary cache.
        fn weight_ptr(&mut self, key: &str) -> Result<(*mut c_void, usize), String> {
            let mut ring_hit: Option<((usize, usize), usize)> = None;
            if let Some(ring) = self.stream_ring.as_ref() {
                'outer: for slot in 0..2 {
                    let group = ring.resident[slot];
                    if group < 0 {
                        continue;
                    }
                    for (index, tensor) in ring.groups[group as usize].iter().enumerate() {
                        if tensor.key == key {
                            ring_hit = Some(((slot, index), tensor.len));
                            break 'outer;
                        }
                    }
                }
            }
            if let Some(((slot, index), len)) = ring_hit {
                let ring = self.stream_ring.as_mut().expect("ring present");
                if !ring.upload_waited[slot] {
                    crate::stream_wait_event(self.stream, ring.upload_done[slot])
                        .map_err(|err| format!("ring upload wait: {err}"))?;
                    ring.upload_waited[slot] = true;
                }
                let ring = self.stream_ring.as_ref().expect("ring present");
                return Ok((ring.slots[slot][index].ptr.as_ptr(), len));
            }
            self.weight_buffers
                .get(key)
                .map(|buffer| (buffer.ptr.as_ptr(), buffer.size_bytes))
                .ok_or_else(|| format!("missing cached CUDA weight buffer {key}"))
        }

        fn ring_prefetch(&mut self, target: usize) -> Result<(), String> {
            // Slots may have been released for a VRAM-hungry phase (VAE
            // decode); re-allocate before borrowing the ring mutably.
            let needs_slots = self
                .stream_ring
                .as_ref()
                .is_some_and(|ring| ring.slots[target % 2].is_empty());
            if needs_slots {
                let shape = self
                    .stream_ring
                    .as_ref()
                    .map(|ring| ring.shape.clone())
                    .unwrap_or_default();
                let mut buffers = Vec::with_capacity(shape.len());
                for len in &shape {
                    buffers.push(self.alloc_with_evict(*len)?);
                }
                if let Some(ring) = self.stream_ring.as_mut() {
                    ring.slots[target % 2] = buffers;
                }
            }
            let ring = self
                .stream_ring
                .as_mut()
                .ok_or_else(|| "stream ring not set up".to_string())?;
            let slot = target % 2;
            if ring.resident[slot] == target as i64 {
                return Ok(());
            }
            // The copy must not overwrite weights still referenced by
            // enqueued compute: wait the slot's last compute fence.
            crate::stream_wait_event(ring.copy_stream, ring.compute_done[slot])
                .map_err(|err| format!("ring compute-fence wait: {err}"))?;
            for (index, tensor) in ring.groups[target].iter().enumerate() {
                unsafe {
                    crate::memcpy_async_host_to_device(
                        ring.slots[slot][index].ptr,
                        tensor.host.as_ptr().cast_const(),
                        tensor.len,
                        ring.copy_stream,
                    )
                }
                .map_err(|err| format!("ring H2D {}: {err}", tensor.key))?;
            }
            crate::event_record(ring.upload_done[slot], ring.copy_stream)
                .map_err(|err| format!("ring upload record: {err}"))?;
            ring.resident[slot] = target as i64;
            ring.upload_waited[slot] = false;
            Ok(())
        }
    }

    /// Register the streamed groups (pinned host copies + 2 device slot-sets)
    /// and prime groups 0 and 1. Replaces any previous ring.
    pub fn gpu_stream_ring_setup(groups: Vec<Vec<(String, Vec<u8>)>>) -> Result<(), String> {
        if groups.len() < 3 {
            return Err("stream ring needs at least 3 groups".to_string());
        }
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let shape: Vec<usize> = groups[0].iter().map(|(_, bytes)| bytes.len()).collect();
            for group in &groups {
                let lens: Vec<usize> = group.iter().map(|(_, bytes)| bytes.len()).collect();
                if lens != shape {
                    return Err("stream ring groups must share tensor shapes".to_string());
                }
            }
            let mut ring_groups = Vec::with_capacity(groups.len());
            for group in groups {
                let mut tensors = Vec::with_capacity(group.len());
                for (key, bytes) in group {
                    let host = unsafe { crate::host_alloc_pinned(bytes.len()) }
                        .map_err(|err| format!("ring pinned alloc: {err}"))?;
                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            bytes.as_ptr(),
                            host.as_ptr().cast::<u8>(),
                            bytes.len(),
                        );
                    }
                    tensors.push(RingTensor {
                        key,
                        len: bytes.len(),
                        host,
                    });
                }
                ring_groups.push(tensors);
            }
            let mut slots: [Vec<DeviceBuffer>; 2] = [Vec::new(), Vec::new()];
            for slot in &mut slots {
                for len in &shape {
                    slot.push(backend.alloc_with_evict(*len)?);
                }
            }
            let copy_stream =
                crate::create_non_blocking_stream().map_err(|err| err.to_string())?;
            let events = || -> Result<crate::cudaEvent_t, String> {
                crate::event_create().map_err(|err| err.to_string())
            };
            backend.stream_ring = Some(FluxStreamRing {
                groups: ring_groups,
                shape,
                slots,
                copy_stream,
                upload_done: [events()?, events()?],
                compute_done: [events()?, events()?],
                resident: [-1, -1],
                upload_waited: [true, true],
            });
            backend.ring_prefetch(0)?;
            backend.ring_prefetch(1)?;
            Ok(())
        })
    }

    /// Free both device slot-sets (the ~2.6GB the VAE-decode phase needs as
    /// headroom on a 32GB card). Pinned host copies stay; the next
    /// prime/prefetch re-allocates and re-uploads.
    pub fn gpu_stream_ring_release_slots() -> Result<(), String> {
        with_dense_linear_backend(|backend| {
            let Some(ring) = backend.stream_ring.as_mut() else {
                return Ok(());
            };
            // In-flight prefetches write into the slots on the copy stream;
            // enqueued compute reads them on the main stream.
            crate::synchronize_stream(ring.copy_stream).map_err(|err| err.to_string())?;
            crate::synchronize_stream(backend.stream).map_err(|err| err.to_string())?;
            let ring = backend.stream_ring.as_mut().expect("ring present");
            ring.slots = [Vec::new(), Vec::new()];
            ring.resident = [-1, -1];
            ring.upload_waited = [true, true];
            Ok(())
        })
    }

    /// Ensure groups 0 and 1 are (re-)uploaded — cheap no-op when resident.
    pub fn gpu_stream_ring_prime() -> Result<(), String> {
        with_dense_linear_backend(|backend| {
            if backend.stream_ring.is_none() {
                return Err("stream ring not set up".to_string());
            }
            backend.ring_prefetch(0)?;
            backend.ring_prefetch(1)
        })
    }

    pub fn gpu_stream_ring_active() -> bool {
        with_dense_linear_backend(|backend| Ok(backend.stream_ring.is_some())).unwrap_or(false)
    }

    /// Compute-stream fence + prefetch of the next same-parity group. Call
    /// right after enqueueing the last op that reads `group`'s weights.
    pub fn gpu_stream_ring_advance(group: usize) -> Result<(), String> {
        with_dense_linear_backend(|backend| {
            let stream = backend.stream;
            let (target, slot) = {
                let ring = backend
                    .stream_ring
                    .as_ref()
                    .ok_or_else(|| "stream ring not set up".to_string())?;
                let count = ring.groups.len();
                ((group + 2) % count, group % 2)
            };
            {
                let ring = backend.stream_ring.as_mut().expect("ring present");
                crate::event_record(ring.compute_done[slot], stream)
                    .map_err(|err| format!("ring compute record: {err}"))?;
            }
            backend.ring_prefetch(target)
        })
    }

    /// The fp8 scaled-mm switch: default ON (the fp8mixed reference's own
    /// arithmetic class — torch `_scaled_mm` with static activation
    /// quantization); `MAKEPAD_FLUX2_FP8MM=0` forces the bf16-dequant
    /// reference path everywhere.
    fn f8_scaled_mm_enabled() -> bool {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ON.get_or_init(|| {
            !matches!(std::env::var("MAKEPAD_FLUX2_FP8MM").as_deref(), Ok("0"))
        })
    }

    /// Shared body of the F8_E4M3-resident dense linears (flux2-dev): the
    /// weight stays 1-byte in the cache (key suffix `::f8` so a layout change
    /// can never reuse a stale half buffer).
    ///
    /// Two arithmetic paths:
    /// - `input_scale` present (and fp8-mm enabled): TRUE fp8 tensor-core
    ///   matmul — the bf16 activation quantizes to E4M3 with the static
    ///   `1/input_scale` (SATFINITE, torch cast parity), and cuBLASLt runs
    ///   e4m3 x e4m3 -> bf16 D with the per-tensor `weight_scale` /
    ///   `input_scale` dequant pointers applied post-accumulate — exactly
    ///   the reference `_scaled_mm` recipe. Falls back to the dequant path
    ///   on any Lt refusal (odd shapes etc.).
    /// - otherwise: dequant into pooled bf16 scratch (exact e4m3->bf16
    ///   kernel) + bf16 GEMM f32-accumulate with alpha = weight_scale.
    #[allow(clippy::too_many_arguments)]
    fn f8_linear_gemm(
        backend: &mut CudaDenseLinearBackend,
        input_bf16_ptr: *const std::ffi::c_void,
        cache_namespace: &str,
        part: &GpuLinearPart<'_>,
        weight_scale: f32,
        input_scale: Option<f32>,
        m: usize,
        k: usize,
        out_bf16_ptr: *const std::ffi::c_void,
    ) -> Result<(), String> {
        let n = part.n;
        let weight_bytes = n
            .checked_mul(k)
            .ok_or_else(|| "f8 mm weight size overflow".to_string())?;
        let weight_key = format!("{cache_namespace}::{}::f8", part.cache_key);
        backend.cached_weight_buffer(&weight_key, weight_bytes, || Ok(part.bytes.to_vec()))?;

        if let Some(input_scale) = input_scale.filter(|_| f8_scaled_mm_enabled()) {
            match f8_scaled_mm(
                backend,
                input_bf16_ptr,
                &weight_key,
                weight_scale,
                input_scale,
                m,
                k,
                n,
                out_bf16_ptr,
            ) {
                Ok(()) => return Ok(()),
                Err(err) => {
                    static WARNED: std::sync::Once = std::sync::Once::new();
                    WARNED.call_once(|| {
                        eprintln!("f8 scaled-mm unavailable ({err}); using bf16 dequant path");
                    });
                }
            }
        }

        let (weight_ptr, _) = backend.weight_ptr(&weight_key)?;
        let scratch = gpu_pool_acquire(weight_bytes * size_of::<u16>())?;
        let count = u32::try_from(weight_bytes)
            .map_err(|_| "f8_e4m3 dequant count exceeds u32".to_string())?;
        let status = unsafe {
            makepad_cuda_dequant_f8_e4m3_bf16(
                weight_ptr.cast_const(),
                scratch.ptr.as_ptr(),
                count,
                backend.stream,
            )
        };
        gpu_check(status)?;
        let alpha = weight_scale;
        let beta = 0.0f32;
        let result = unsafe {
            crate::cublas_gemm_ex(
                backend.blas,
                crate::CUBLAS_OP_T,
                crate::CUBLAS_OP_N,
                n as i32,
                m as i32,
                k as i32,
                &alpha,
                scratch.ptr.as_ptr(),
                crate::CUDA_R_16BF,
                k as i32,
                input_bf16_ptr,
                crate::CUDA_R_16BF,
                k as i32,
                &beta,
                out_bf16_ptr.cast_mut(),
                crate::CUDA_R_16BF,
                n as i32,
                crate::CUDA_R_32F,
                crate::CUBLAS_GEMM_DEFAULT_TENSOR_OP,
            )
        }
        .map_err(|err| format!("f8 mm failed: m={m} k={k} n={n}: {err}"));
        gpu_pool_release(scratch);
        result
    }

    /// Round-to-nearest-even bf16 grid value of `1/scale` — the reference
    /// computes `(1.0/scale).to(bf16)` before multiplying.
    fn bf16_grid(value: f32) -> f32 {
        let bits = value.to_bits();
        let rounding = 0x7fffu32 + ((bits >> 16) & 1);
        f32::from_bits(bits.wrapping_add(rounding) & 0xffff_0000)
    }

    /// The cuBLASLt e4m3 x e4m3 -> bf16 matmul with device dequant-scale
    /// pointers (see `f8_linear_gemm`). Weight is the cached raw fp8 buffer;
    /// the activation is quantized transiently into pooled scratch.
    #[allow(clippy::too_many_arguments)]
    fn f8_scaled_mm(
        backend: &mut CudaDenseLinearBackend,
        input_bf16_ptr: *const std::ffi::c_void,
        weight_key: &str,
        weight_scale: f32,
        input_scale: f32,
        m: usize,
        k: usize,
        n: usize,
        out_bf16_ptr: *const std::ffi::c_void,
    ) -> Result<(), String> {
        use std::ffi::c_void;
        // Per-tensor f32 scales live in tiny cached device buffers (Lt wants
        // device pointers); keys ride beside the weight so eviction drops
        // them together.
        let wscale_key = format!("{weight_key}::wscale");
        backend.cached_weight_buffer(&wscale_key, 4, || Ok(weight_scale.to_le_bytes().to_vec()))?;
        let iscale_key = format!("{weight_key}::iscale");
        backend.cached_weight_buffer(&iscale_key, 4, || Ok(input_scale.to_le_bytes().to_vec()))?;

        let input_values = m
            .checked_mul(k)
            .ok_or_else(|| "f8 scaled mm input size overflow".to_string())?;
        let input_f8 = gpu_pool_acquire(input_values)?;
        let inv_input_scale = bf16_grid(1.0 / input_scale);
        let status = unsafe {
            makepad_cuda_quant_bf16_f8_e4m3(
                input_bf16_ptr,
                input_f8.ptr.as_ptr(),
                inv_input_scale,
                input_values as u32,
                backend.stream,
            )
        };
        if let Err(err) = gpu_check(status) {
            gpu_pool_release(input_f8);
            return Err(err);
        }

        let workspace_size = 32usize * 1024 * 1024;
        let workspace = match gpu_pool_acquire(workspace_size) {
            Ok(buffer) => buffer,
            Err(err) => {
                gpu_pool_release(input_f8);
                return Err(err);
            }
        };

        let run = (|| -> Result<(), String> {
            let (weight_ptr, _) = backend.weight_ptr(weight_key)?;
            let wscale = backend
                .weight_buffers
                .get(&wscale_key)
                .ok_or_else(|| "missing f8 weight scale buffer".to_string())?;
            let iscale = backend
                .weight_buffers
                .get(&iscale_key)
                .ok_or_else(|| "missing f8 input scale buffer".to_string())?;

            let operation = crate::cublas_lt_matmul_desc_create(
                crate::CUBLAS_COMPUTE_32F,
                crate::CUDA_R_32F,
            )
            .map_err(|error| format!("f8 Lt desc create: {error}"))?;
            let a_desc = crate::cublas_lt_matrix_layout_create(
                crate::CUDA_R_8F_E4M3,
                k as u64,
                n as u64,
                k as i64,
            );
            let b_desc = crate::cublas_lt_matrix_layout_create(
                crate::CUDA_R_8F_E4M3,
                k as u64,
                m as u64,
                k as i64,
            );
            let d_desc = crate::cublas_lt_matrix_layout_create(
                crate::CUDA_R_16BF,
                n as u64,
                m as u64,
                n as i64,
            );
            let preference = crate::cublas_lt_matmul_preference_create();
            let inner = (|| -> Result<(), String> {
                let a_desc = a_desc
                    .as_ref()
                    .map_err(|error| format!("f8 Lt A layout: {error}"))?;
                let b_desc = b_desc
                    .as_ref()
                    .map_err(|error| format!("f8 Lt B layout: {error}"))?;
                let d_desc = d_desc
                    .as_ref()
                    .map_err(|error| format!("f8 Lt D layout: {error}"))?;
                let preference = preference
                    .as_ref()
                    .map_err(|error| format!("f8 Lt preference: {error}"))?;
                let transpose_a = crate::CUBLAS_OP_T;
                let transpose_b = crate::CUBLAS_OP_N;
                crate::cublas_lt_matmul_desc_set_attribute(
                    operation,
                    crate::CUBLASLT_MATMUL_DESC_TRANSA,
                    &transpose_a,
                )
                .map_err(|error| format!("f8 Lt transA: {error}"))?;
                crate::cublas_lt_matmul_desc_set_attribute(
                    operation,
                    crate::CUBLASLT_MATMUL_DESC_TRANSB,
                    &transpose_b,
                )
                .map_err(|error| format!("f8 Lt transB: {error}"))?;
                let a_scale_ptr = wscale.ptr.as_ptr().cast::<c_void>();
                let b_scale_ptr = iscale.ptr.as_ptr().cast::<c_void>();
                crate::cublas_lt_matmul_desc_set_attribute(
                    operation,
                    crate::CUBLASLT_MATMUL_DESC_A_SCALE_POINTER,
                    &a_scale_ptr,
                )
                .map_err(|error| format!("f8 Lt a_scale: {error}"))?;
                crate::cublas_lt_matmul_desc_set_attribute(
                    operation,
                    crate::CUBLASLT_MATMUL_DESC_B_SCALE_POINTER,
                    &b_scale_ptr,
                )
                .map_err(|error| format!("f8 Lt b_scale: {error}"))?;
                crate::cublas_lt_matmul_preference_set_attribute(
                    *preference,
                    crate::CUBLASLT_MATMUL_PREF_MAX_WORKSPACE_BYTES,
                    &workspace_size,
                )
                .map_err(|error| format!("f8 Lt workspace pref: {error}"))?;
                let heuristic = crate::cublas_lt_matmul_algo_get_heuristic(
                    backend.blas_lt,
                    operation,
                    *a_desc,
                    *b_desc,
                    *d_desc,
                    *d_desc,
                    *preference,
                )
                .map_err(|error| format!("f8 Lt heuristic: {error}"))?
                .ok_or_else(|| format!("f8 Lt no algorithm for m={m} k={k} n={n}"))?;
                let alpha = 1.0f32;
                let beta = 0.0f32;
                unsafe {
                    crate::cublas_lt_matmul(
                        backend.blas_lt,
                        operation,
                        (&alpha as *const f32).cast::<c_void>(),
                        weight_ptr,
                        *a_desc,
                        input_f8.ptr.as_ptr(),
                        *b_desc,
                        (&beta as *const f32).cast::<c_void>(),
                        out_bf16_ptr,
                        *d_desc,
                        out_bf16_ptr.cast_mut(),
                        *d_desc,
                        &heuristic.algo,
                        workspace.ptr.as_ptr(),
                        workspace_size,
                        backend.stream,
                    )
                }
                .map_err(|error| format!("f8 Lt matmul m={m} k={k} n={n}: {error}"))
            })();
            if let Ok(desc) = preference {
                let _ = crate::cublas_lt_matmul_preference_destroy(desc);
            }
            if let Ok(desc) = d_desc {
                let _ = crate::cublas_lt_matrix_layout_destroy(desc);
            }
            if let Ok(desc) = b_desc {
                let _ = crate::cublas_lt_matrix_layout_destroy(desc);
            }
            if let Ok(desc) = a_desc {
                let _ = crate::cublas_lt_matrix_layout_destroy(desc);
            }
            let _ = crate::cublas_lt_matmul_desc_destroy(operation);
            inner
        })();
        gpu_pool_release(workspace);
        gpu_pool_release(input_f8);
        run
    }

    fn require_one_f8_part<'a, 'b>(
        parts: &'b [GpuLinearPart<'a>],
        who: &str,
    ) -> Result<&'b GpuLinearPart<'a>, String> {
        if parts.len() != 1 || parts[0].bt_ggml_type != GGML_TYPE_F8_E4M3 {
            return Err(format!("{who} requires one F8_E4M3 weight part"));
        }
        Ok(&parts[0])
    }

    /// F8_E4M3-resident dense linear, f32 activation in / f32 out (values on
    /// the bf16 grid like the bf16_mm twin: bf16 D, expanded losslessly).
    pub fn gpu_linear_nt_cached_f8_mm(
        x: &GpuTensor,
        cache_namespace: &str,
        parts: &[GpuLinearPart<'_>],
        weight_scale: f32,
        input_scale: Option<f32>,
    ) -> Result<GpuTensor, String> {
        if x.half {
            return Err("gpu_linear_nt_cached_f8_mm expects f32 storage".to_string());
        }
        let part = require_one_f8_part(parts, "gpu_linear_nt_cached_f8_mm")?;
        let (m, k, n) = (x.rows, x.cols, part.n);
        if m == 0 || k == 0 || n == 0 {
            return Err(format!("gpu_linear_nt_cached_f8_mm empty shape: x={m}x{k} n={n}"));
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            backend.ensure_input_half(m * k * size_of::<u16>())?;
            let status = unsafe {
                makepad_cuda_f32_to_bf16_rn(
                    x.device_ptr()?,
                    backend
                        .input_half
                        .as_ref()
                        .ok_or_else(|| "missing f8 mm input buffer".to_string())?
                        .ptr
                        .as_ptr()
                        .cast::<u16>(),
                    (m * k) as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            let output_values = m
                .checked_mul(n)
                .ok_or_else(|| "f8 mm output size overflow".to_string())?;
            let output_bf16 = gpu_pool_acquire(output_values * size_of::<u16>())?;
            let output = GpuTensor::from_pool(m, n)?;
            let input_ptr = backend
                .input_half
                .as_ref()
                .expect("ensured above")
                .ptr
                .as_ptr()
                .cast_const();
            f8_linear_gemm(
                backend,
                input_ptr,
                cache_namespace,
                part,
                weight_scale,
                input_scale,
                m,
                k,
                output_bf16.ptr.as_ptr().cast_const(),
            )?;
            let status = unsafe {
                makepad_cuda_bf16_to_f32(
                    output_bf16.ptr.as_ptr().cast::<u16>(),
                    output.device_ptr()?,
                    output_values as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_pool_release(output_bf16);
            gpu_prof(backend.stream, crate::prof::CAT_DENSE_TOTAL, prof_start, 0);
            Ok(output)
        })
    }

    /// F8_E4M3-resident dense linear from a bf16 buffer, f32 out.
    pub fn gpu_linear_nt_cached_f8_mm_from_buf(
        x: &GpuBf16Buf,
        cache_namespace: &str,
        parts: &[GpuLinearPart<'_>],
        weight_scale: f32,
        input_scale: Option<f32>,
    ) -> Result<GpuTensor, String> {
        let part = require_one_f8_part(parts, "gpu_linear_nt_cached_f8_mm_from_buf")?;
        let (m, k, n) = (x.rows, x.cols, part.n);
        if m == 0 || k == 0 || n == 0 {
            return Err(format!(
                "gpu_linear_nt_cached_f8_mm_from_buf empty shape: x={m}x{k} n={n}"
            ));
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let output_values = m
                .checked_mul(n)
                .ok_or_else(|| "f8 mm output size overflow".to_string())?;
            let output_bf16 = gpu_pool_acquire(output_values * size_of::<u16>())?;
            let output = GpuTensor::from_pool(m, n)?;
            f8_linear_gemm(
                backend,
                x.device_ptr_u16()?.cast::<std::ffi::c_void>().cast_const(),
                cache_namespace,
                part,
                weight_scale,
                input_scale,
                m,
                k,
                output_bf16.ptr.as_ptr().cast_const(),
            )?;
            let status = unsafe {
                makepad_cuda_bf16_to_f32(
                    output_bf16.ptr.as_ptr().cast::<u16>(),
                    output.device_ptr()?,
                    output_values as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_pool_release(output_bf16);
            gpu_prof(backend.stream, crate::prof::CAT_DENSE_TOTAL, prof_start, 0);
            Ok(output)
        })
    }

    /// F8_E4M3-resident dense linear from a bf16 buffer straight to a bf16
    /// buffer (the gemm's own bf16 D is the result).
    pub fn gpu_linear_nt_cached_f8_mm_from_buf_to_buf(
        x: &GpuBf16Buf,
        cache_namespace: &str,
        parts: &[GpuLinearPart<'_>],
        weight_scale: f32,
        input_scale: Option<f32>,
    ) -> Result<GpuBf16Buf, String> {
        let part = require_one_f8_part(parts, "gpu_linear_nt_cached_f8_mm_from_buf_to_buf")?;
        let (m, k, n) = (x.rows, x.cols, part.n);
        if m == 0 || k == 0 || n == 0 {
            return Err(format!(
                "gpu_linear_nt_cached_f8_mm_from_buf_to_buf empty shape: x={m}x{k} n={n}"
            ));
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let output = GpuBf16Buf::from_pool(m, n)?;
            f8_linear_gemm(
                backend,
                x.device_ptr_u16()?.cast::<std::ffi::c_void>().cast_const(),
                cache_namespace,
                part,
                weight_scale,
                input_scale,
                m,
                k,
                output.device_ptr_u16()?.cast::<std::ffi::c_void>().cast_const(),
            )?;
            gpu_prof(backend.stream, crate::prof::CAT_DENSE_TOTAL, prof_start, 0);
            Ok(output)
        })
    }

    /// LayerNorm+mod storing bf16-RN — the bits the next linear's staging
    /// would produce from the f32 result.
    pub fn gpu_layer_norm_mod_to_bf16buf(
        x: &GpuTensor,
        mods: &GpuTensor,
        scale_off: usize,
        shift_off: usize,
        eps: f32,
    ) -> Result<GpuBf16Buf, String> {
        if x.half {
            return Err("gpu_layer_norm_mod_to_bf16buf expects f32 storage".to_string());
        }
        let mods_len = mods.rows * mods.cols;
        if scale_off + x.cols > mods_len || shift_off + x.cols > mods_len {
            return Err("gpu_layer_norm_mod_to_bf16buf offset out of range".to_string());
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuBf16Buf::from_pool(x.rows, x.cols)?;
            let mods_ptr = mods.device_ptr()?;
            let status = unsafe {
                makepad_cuda_layer_norm_mul_add_f32_out_bf16(
                    x.device_ptr()?,
                    mods_ptr.add(scale_off),
                    mods_ptr.add(shift_off),
                    out.device_ptr_u16()?,
                    x.rows as u32,
                    x.cols as u32,
                    eps,
                    1.0,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_prof(backend.stream, crate::prof::CAT_LAYER_NORM, prof_start, 0);
            Ok(out)
        })
    }

    /// Expand a bf16 column slab into a contiguous f32 tensor (lossless).
    pub fn gpu_bf16buf_slab_to_f32(
        x: &GpuBf16Buf,
        col_off: usize,
        cols: usize,
    ) -> Result<GpuTensor, String> {
        if cols == 0 || col_off + cols > x.cols {
            return Err(format!(
                "gpu_bf16buf_slab_to_f32 slab {col_off}+{cols} outside {} cols",
                x.cols
            ));
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(x.rows, cols)?;
            let status = unsafe {
                makepad_cuda_bf16_slab_to_f32(
                    x.device_ptr_u16()?,
                    out.device_ptr()?,
                    x.rows as u32,
                    x.cols as u32,
                    col_off as u32,
                    cols as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_prof(backend.stream, crate::prof::CAT_ELEMENTWISE, prof_start, 0);
            Ok(out)
        })
    }

    /// Per-head weighted RMS-norm reading its groups out of a bf16 slab and
    /// writing contiguous f32 — bit-identical to slice+expand+rms (same
    /// values, same reduction).
    #[allow(clippy::too_many_arguments)]
    pub fn gpu_rms_norm_mul_from_bf16_slab(
        x: &GpuBf16Buf,
        col_off: usize,
        cols: usize,
        group_cols: usize,
        cache_namespace: &str,
        cache_key: &str,
        scale: &[f32],
        eps: f32,
    ) -> Result<GpuTensor, String> {
        if group_cols == 0
            || cols % group_cols != 0
            || col_off + cols > x.cols
            || scale.len() != group_cols
        {
            return Err("gpu_rms_norm_mul_from_bf16_slab shape mismatch".to_string());
        }
        let groups_per_row = cols / group_cols;
        let group_count = x.rows * groups_per_row;
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let vec_key = format!("{cache_namespace}::{cache_key}::rms");
            let vec_bytes = scale.len() * size_of::<f32>();
            backend.cached_weight_buffer(&vec_key, vec_bytes, || {
                let raw = unsafe {
                    std::slice::from_raw_parts(scale.as_ptr().cast::<u8>(), vec_bytes)
                };
                Ok(raw.to_vec())
            })?;
            let out = GpuTensor::from_pool(x.rows, cols)?;
            let scale_buf = backend
                .weight_buffers
                .get(&vec_key)
                .ok_or_else(|| format!("missing cached CUDA rms scale buffer {vec_key}"))?;
            let status = unsafe {
                makepad_cuda_rms_norm_weighted_bf16slab_f32(
                    x.device_ptr_u16()?,
                    scale_buf.ptr.as_ptr().cast::<f32>(),
                    out.device_ptr()?,
                    group_count as u32,
                    groups_per_row as u32,
                    x.cols as u32,
                    col_off as u32,
                    group_cols as u32,
                    eps,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_prof(backend.stream, crate::prof::CAT_RMS_NORM, prof_start, 0);
            Ok(out)
        })
    }

    /// Gate-first SwiGLU over a bf16 slab, storing bf16-RN.
    pub fn gpu_swiglu_gate_first_from_bf16(
        x: &GpuBf16Buf,
        gate_offset: usize,
        n: usize,
    ) -> Result<GpuBf16Buf, String> {
        if n == 0 || gate_offset + 2 * n > x.cols {
            return Err(format!(
                "gpu_swiglu_gate_first_from_bf16 slab {gate_offset}+2*{n} outside {} cols",
                x.cols
            ));
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuBf16Buf::from_pool(x.rows, n)?;
            let status = unsafe {
                makepad_cuda_swiglu_gate_first_bf16slab(
                    x.device_ptr_u16()?,
                    out.device_ptr_u16()?,
                    x.rows as u32,
                    x.cols as u32,
                    gate_offset as u32,
                    n as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_prof(backend.stream, crate::prof::CAT_ELEMENTWISE, prof_start, 0);
            Ok(out)
        })
    }

    /// `[rn_bf16(a) | b]` — the [attn | mlp_act] concat staged straight into
    /// the down-projection's bf16 input layout.
    pub fn gpu_concat_f32rn_bf16buf(
        a: &GpuTensor,
        b: &GpuBf16Buf,
    ) -> Result<GpuBf16Buf, String> {
        if a.half {
            return Err("gpu_concat_f32rn_bf16buf expects f32 left input".to_string());
        }
        if a.rows != b.rows {
            return Err("gpu_concat_f32rn_bf16buf row mismatch".to_string());
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuBf16Buf::from_pool(a.rows, a.cols + b.cols)?;
            let status = unsafe {
                makepad_cuda_concat_f32rn_bf16(
                    a.device_ptr()?,
                    b.device_ptr_u16()?,
                    out.device_ptr_u16()?,
                    a.rows as u32,
                    a.cols as u32,
                    b.cols as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_prof(backend.stream, crate::prof::CAT_ELEMENTWISE, prof_start, 0);
            Ok(out)
        })
    }

    /// WaveNet cond gate on (rows, 2n), biases already applied by the
    /// producing GEMM: out = tanh(x[:, :n]) * sigmoid(x[:, n:]). f32 only.
    pub fn gpu_wavenet_gate(x: &GpuTensor) -> Result<GpuTensor, String> {
        if x.half {
            return Err("gpu_wavenet_gate expects f32 input".to_string());
        }
        if x.cols % 2 != 0 {
            return Err("gpu_wavenet_gate odd column count".to_string());
        }
        let n = x.cols / 2;
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(x.rows, n)?;
            let status = unsafe {
                makepad_cuda_wavenet_gate_f32(
                    x.device_ptr()?,
                    out.device_ptr()?,
                    x.rows as u32,
                    n as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_prof(backend.stream, crate::prof::CAT_ELEMENTWISE, prof_start, 0);
            Ok(out)
        })
    }

    /// Fused anti-aliased SnakeBeta (BigVGAN Activation1d, ratio 2, 12-tap
    /// Kaiser filters) on time-major (t, ch) rows. `params` is the combined
    /// per-activation buffer [alpha(ch) | inv_beta(ch) | up_filter(12) |
    /// down_filter(12)] — alpha/inv_beta preexponentiated, up taps carrying
    /// the ratio gain. `input_scale` multiplies inputs as loaded (before the
    /// snake). f32 only; output has the input's shape.
    pub fn gpu_alias_snake_updown2x(
        x: &GpuTensor,
        params: &GpuTensor,
        input_scale: f32,
    ) -> Result<GpuTensor, String> {
        if x.half || params.half {
            return Err("gpu_alias_snake_updown2x expects f32 tensors".to_string());
        }
        if params.rows * params.cols != 2 * x.cols + 24 {
            return Err(format!(
                "gpu_alias_snake_updown2x params len {} != 2*{} + 24 (12-tap up/down)",
                params.rows * params.cols,
                x.cols
            ));
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(x.rows, x.cols)?;
            let status = unsafe {
                makepad_cuda_alias_snake_updown2x_f32(
                    x.device_ptr()?,
                    params.device_ptr()?,
                    out.device_ptr()?,
                    x.rows as u32,
                    x.cols as u32,
                    input_scale,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_prof(backend.stream, crate::prof::CAT_ELEMENTWISE, prof_start, 0);
            Ok(out)
        })
    }

    /// Dense f32 linear against a RESIDENT f32 weight tensor (n rows of k
    /// cols): C(m,n) = X(m,k) @ W^T. Used for H3's deliberately-f32 modules
    /// (patch projections and output heads).
    pub fn gpu_linear_f32_resident(
        x: &GpuTensor,
        weight: &GpuTensor,
        bias: Option<&GpuTensor>,
    ) -> Result<GpuTensor, String> {
        if x.half || weight.half {
            return Err("gpu_linear_f32_resident expects f32 tensors".to_string());
        }
        if weight.cols != x.cols {
            return Err(format!(
                "gpu_linear_f32_resident k mismatch: x {}x{}, w {}x{}",
                x.rows, x.cols, weight.rows, weight.cols
            ));
        }
        let (m, k, n) = (x.rows, x.cols, weight.rows);
        if let Some(bias) = bias {
            if bias.rows * bias.cols != n {
                return Err("gpu_linear_f32_resident bias mismatch".to_string());
            }
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(m, n)?;
            // Row-major C(m,n) is column-major (n,m):
            // C_col(n,m) = op_T(W_col(k,n)) * op_N(X_col(k,m)).
            let alpha = 1.0f32;
            let beta = 0.0f32;
            crate::cublas_sgemm(
                backend.blas,
                crate::CUBLAS_OP_T,
                crate::CUBLAS_OP_N,
                n as i32,
                m as i32,
                k as i32,
                &alpha,
                weight.device_ptr()?,
                k as i32,
                x.device_ptr()?,
                k as i32,
                &beta,
                out.device_ptr()?,
                n as i32,
            )
            .map_err(|err| err.to_string())?;
            if let Some(bias) = bias {
                let status = unsafe {
                    makepad_cuda_add_rows_vec_f32(
                        out.device_ptr()?,
                        bias.device_ptr()?,
                        out.device_ptr()?,
                        m as u32,
                        n as u32,
                        backend.stream,
                    )
                };
                gpu_check(status)?;
            }
            gpu_prof(backend.stream, crate::prof::CAT_DENSE_GEMM, prof_start, 0);
            Ok(out)
        })
    }

    /// Pre-fill the device weight cache for one linear part, streaming the
    /// bytes through `load` only on a cache miss. Reproduces the exact key
    /// and bf16->f16 conversion logic of gpu_linear_nt_cached, so parts can
    /// afterwards be passed with EMPTY bytes (a miss with empty bytes fails
    /// loudly instead of uploading garbage). This is the H3 streaming-load
    /// path: model weights never need a whole-model host-RAM copy.
    pub fn gpu_weight_cache_ensure<F>(
        cache_namespace: &str,
        cache_key: &str,
        bt_ggml_type: u32,
        n: usize,
        k: usize,
        want_a16: bool,
        load: F,
    ) -> Result<(), String>
    where
        F: FnOnce() -> Result<Vec<u8>, String>,
    {
        // F8_E4M3 stays 1-byte resident under the same `::f8` key suffix the
        // f8 mm family looks up (see f8_linear_gemm).
        let elem_bytes = if bt_ggml_type == GGML_TYPE_F8_E4M3 {
            1
        } else {
            size_of::<u16>()
        };
        let weight_bytes = n
            .checked_mul(k)
            .and_then(|len| len.checked_mul(elem_bytes))
            .ok_or_else(|| "gpu_weight_cache_ensure size overflow".to_string())?;
        let qualified_key = if want_a16 {
            format!("{cache_namespace}::{cache_key}::a16")
        } else if bt_ggml_type == GGML_TYPE_F8_E4M3 {
            format!("{cache_namespace}::{cache_key}::f8")
        } else {
            format!("{cache_namespace}::{cache_key}")
        };
        let needs_convert = want_a16 && bt_ggml_type == GGML_TYPE_BF16;
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            backend.cached_weight_buffer(&qualified_key, weight_bytes, || {
                let raw = load()?;
                if raw.len() != weight_bytes {
                    return Err(format!(
                        "gpu_weight_cache_ensure {qualified_key}: got {} bytes, expected {weight_bytes}",
                        raw.len()
                    ));
                }
                if needs_convert {
                    let mut converted = vec![0u8; raw.len()];
                    for (i, chunk) in raw.chunks_exact(2).enumerate() {
                        let word = u16::from_le_bytes([chunk[0], chunk[1]]);
                        let half =
                            crate::quant::f32_to_f16(f32::from_bits((word as u32) << 16));
                        converted[i * 2..i * 2 + 2].copy_from_slice(&half.to_le_bytes());
                    }
                    Ok(converted)
                } else {
                    Ok(raw)
                }
            })
        })
    }

    /// True for the quantized weight formats the dense linear path serves by
    /// bulk-dequantizing into pooled bf16 scratch right before the gemm.
    pub fn gpu_quant_linear_type_supported(ggml_type: u32) -> bool {
        matches!(
            ggml_type,
            GGML_TYPE_Q4_K
                | GGML_TYPE_Q6_K
                | GGML_TYPE_Q4_0
                | GGML_TYPE_H3_NVFP4_PAIRS
                | GGML_TYPE_H3_NVFP4_PAIRS_PRESCALE
                | GGML_TYPE_F8_E4M3
        )
    }

    /// Device payload size of one quantized `(n out-rows, k in-cols)` linear.
    fn quant_linear_payload_bytes(ggml_type: u32, n: usize, k: usize) -> Result<usize, String> {
        match ggml_type {
            GGML_TYPE_Q4_K | GGML_TYPE_Q6_K | GGML_TYPE_Q4_0 => {
                let elems = block_elements(ggml_type);
                if k == 0 || k % elems != 0 {
                    return Err(format!(
                        "quant linear k={k} not divisible by {elems} (ggml type {ggml_type})"
                    ));
                }
                n.checked_mul(k / elems)
                    .and_then(|blocks| blocks.checked_mul(block_size(ggml_type)))
                    .ok_or_else(|| "quant linear size overflow".to_string())
            }
            GGML_TYPE_H3_NVFP4_PAIRS => h3_nvfp4_pairs_bytes(n, k, false)
                .ok_or_else(|| format!("nvfp4 pairs shape invalid: n={n} k={k}")),
            GGML_TYPE_H3_NVFP4_PAIRS_PRESCALE => h3_nvfp4_pairs_bytes(n, k, true)
                .ok_or_else(|| format!("nvfp4 pairs shape invalid: n={n} k={k}")),
            // Scalar signed E4M3FN: 1 byte per element, resident raw — the
            // 24GB FLUX tier depends on weights never expanding in cache.
            GGML_TYPE_F8_E4M3 => n
                .checked_mul(k)
                .ok_or_else(|| "f8_e4m3 linear size overflow".to_string()),
            other => Err(format!("not a quantized linear type: {other}")),
        }
    }

    fn quant_part_key(cache_namespace: &str, cache_key: &str, ggml_type: u32) -> String {
        // The type rides in the key so a format change can never silently
        // reuse a stale same-size buffer cached under another layout.
        format!("{cache_namespace}::{cache_key}::q{ggml_type}")
    }

    /// `gpu_weight_cache_ensure` twin for quantized linears: uploads the raw
    /// quantized payload verbatim (K-quant block stream / NVFP4 pairs blob)
    /// under the type-suffixed key the quantized gemm path looks up.
    pub fn gpu_weight_cache_ensure_quant<F>(
        cache_namespace: &str,
        cache_key: &str,
        bt_ggml_type: u32,
        n: usize,
        k: usize,
        load: F,
    ) -> Result<(), String>
    where
        F: FnOnce() -> Result<Vec<u8>, String>,
    {
        let payload_bytes = quant_linear_payload_bytes(bt_ggml_type, n, k)?;
        let qualified_key = quant_part_key(cache_namespace, cache_key, bt_ggml_type);
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            backend.cached_weight_buffer(&qualified_key, payload_bytes, || {
                let raw = load()?;
                if raw.len() != payload_bytes {
                    return Err(format!(
                        "gpu_weight_cache_ensure_quant {qualified_key}: got {} bytes, expected {payload_bytes}",
                        raw.len()
                    ));
                }
                Ok(raw)
            })
        })
    }

    /// Marks weight-cache key prefixes that must survive allocation-failure
    /// recovery on this thread (the resident FLUX checkpoint namespaces).
    /// Replaces the previous protected set; pass an empty vec to clear.
    /// Protection only guards the implicit OOM eviction ladder — explicit
    /// `gpu_weight_cache_evict_prefix` calls (model switch/unload) still
    /// evict protected keys, keeping residency reporting truthful.
    pub fn gpu_weight_cache_protect_prefixes(prefixes: Vec<String>) -> Result<(), String> {
        with_dense_linear_backend(|backend| {
            backend.protected_prefixes = prefixes;
            Ok(())
        })
    }

    /// Drop every cached weight buffer whose qualified key starts with
    /// `prefix` (e.g. the H3 text-encoder namespace after its encode pass).
    /// Returns the number of buffers freed.
    pub fn gpu_weight_cache_evict_prefix(prefix: &str) -> Result<usize, String> {
        with_dense_linear_backend(|backend| {
            let keys: Vec<String> = backend
                .weight_buffers
                .keys()
                .filter(|key| key.starts_with(prefix))
                .cloned()
                .collect();
            let count = keys.len();
            for key in keys {
                backend.weight_buffers.remove(&key);
            }
            Ok(count)
        })
    }

    /// Drop cached dense-linear weights matching `prefix` without creating a
    /// CUDA backend when this thread has never used one. Lifecycle teardown
    /// uses this variant: unloading a cold/CPU model must not initialize a
    /// device merely to discover that there is nothing to release.
    pub fn gpu_weight_cache_evict_prefix_if_loaded(prefix: &str) -> Result<usize, String> {
        DENSE_LINEAR_BACKEND.with(|slot| {
            let mut slot = slot.borrow_mut();
            let Some(backend) = slot.as_mut() else {
                return Ok(0);
            };
            backend.prepare_device()?;
            let keys: Vec<String> = backend
                .weight_buffers
                .keys()
                .filter(|key| key.starts_with(prefix))
                .cloned()
                .collect();
            let count = keys.len();
            for key in keys {
                backend.weight_buffers.remove(&key);
            }
            Ok(count)
        })
    }

    /// Synchronize the already-created dense CUDA runtime and release its
    /// reusable activation/scratch buffers on this thread. Model-specific
    /// weight caches are left alone; callers evict their namespaces first.
    /// Like the conditional eviction helper, this is a no-op when the thread
    /// has never initialized the dense backend.
    pub fn gpu_runtime_trim() -> Result<(), String> {
        let result = DENSE_LINEAR_BACKEND.with(|slot| {
            let mut slot = slot.borrow_mut();
            let Some(backend) = slot.as_mut() else {
                return Ok(());
            };
            backend.prepare_device()?;
            let sync = crate::synchronize_stream(backend.stream)
                .map_err(|error| error.to_string());
            // cudaFree provides the final release barrier even when the
            // explicit stream sync reported an error. Clear every reusable
            // dense buffer before returning either result.
            backend.input_f32 = None;
            backend.input_f32_capacity_bytes = 0;
            backend.input_half = None;
            backend.input_half_capacity_bytes = 0;
            backend.output_f32 = None;
            backend.output_f32_capacity = 0;
            sync
        });
        gpu_pool_clear();
        result
    }

    fn gpu_attention_f16_enabled() -> bool {
        match std::env::var("FLUX_ATTN_F16") {
            Ok(value) => value != "0",
            Err(_) => true,
        }
    }

    fn gpu_attention_fused_enabled() -> bool {
        match std::env::var("FLUX_ATTN_FUSED") {
            Ok(value) => value != "0",
            Err(_) => true,
        }
    }

    /// The register-level FA2 kernel (raw mma.sync, fragment-resident
    /// softmax). FLUX_ATTN_MMA=0 falls back to the wmma flash kernel.
    fn gpu_attention_mma_enabled() -> bool {
        match std::env::var("FLUX_ATTN_MMA") {
            Ok(value) => value != "0",
            Err(_) => true,
        }
    }

    /// f16-accumulate dense gemms (CUBLAS_COMPUTE_16F): consumer GeForce runs
    /// f16xf16->f16 tensor ops at TWICE the f32-accumulate rate, and this is
    /// the same reduced-precision-reduction torch enables by default for f16
    /// models. FLUX_GEMM_F16ACC=0 restores f32 accumulation everywhere.
    pub fn gpu_gemm_f16acc_enabled() -> bool {
        match std::env::var("FLUX_GEMM_F16ACC") {
            Ok(value) => value != "0",
            Err(_) => true,
        }
    }

    fn gpu_attention_compare_enabled() -> bool {
        matches!(std::env::var("FLUX_ATTN_COMPARE"), Ok(value) if value == "1")
    }

    /// Full bidirectional self attention on token-major packed q/k/v.
    ///
    /// Default path (head_dim 128): the fused flash-attention kernel —
    /// online-softmax tiling on tensor cores, no materialized score tensor
    /// (f16 gemm inputs, f32 softmax and accumulators). FLUX_ATTN_FUSED=0
    /// (or an unsupported head_dim) falls back to the cublas composite:
    /// two strided-batched gemms around an in-place f32 row softmax, gemm
    /// inputs in f16 unless FLUX_ATTN_F16=0. FLUX_ATTN_COMPARE=1 runs BOTH
    /// paths and prints the max/mean abs difference per call (validation).
    pub fn gpu_attention_packed(
        q: &GpuTensor,
        k: &GpuTensor,
        v: &GpuTensor,
        head_count: usize,
        scale: f32,
    ) -> Result<GpuTensor, String> {
        let seq = q.rows;
        let hidden = q.cols;
        if k.rows != seq || v.rows != seq || k.cols != hidden || v.cols != hidden {
            return Err("gpu_attention shape mismatch".to_string());
        }
        if head_count == 0 || hidden % head_count != 0 {
            return Err("gpu_attention head mismatch".to_string());
        }
        let head_dim = hidden / head_count;
        if gpu_attention_fused_enabled() && head_dim == 128 {
            let fused = gpu_attention_packed_fused(q, k, v, head_count, scale)?;
            if gpu_attention_compare_enabled() {
                let reference = gpu_attention_packed_composite(
                    q,
                    k,
                    v,
                    head_count,
                    scale,
                    PackedAttentionMask::None,
                    PackedAttentionPrecision::Environment,
                )?;
                let fused_host = gpu_download(&fused)?;
                let reference_host = gpu_download(&reference)?;
                let mut max_abs_diff = 0.0f32;
                let mut sum_abs_diff = 0.0f64;
                let mut max_ref = 0.0f32;
                for (a, b) in fused_host.iter().zip(&reference_host) {
                    let diff = (a - b).abs();
                    max_abs_diff = max_abs_diff.max(diff);
                    sum_abs_diff += diff as f64;
                    max_ref = max_ref.max(b.abs());
                }
                let mean_abs_diff = sum_abs_diff / fused_host.len().max(1) as f64;
                eprintln!(
                    "FLUX_ATTN_COMPARE seq={seq} heads={head_count} \
                     max_abs_diff={max_abs_diff:.3e} mean_abs_diff={mean_abs_diff:.3e} \
                     max_ref={max_ref:.3e}"
                );
            }
            return Ok(fused);
        }
        gpu_attention_packed_composite(
            q,
            k,
            v,
            head_count,
            scale,
            PackedAttentionMask::None,
            PackedAttentionPrecision::Environment,
        )
    }

    /// Bidirectional self-attention with f32 QK/PV GEMMs and f32 softmax.
    /// Used by ACE DiT so Flux's default f16/fused FA2 path cannot leak in.
    pub fn gpu_attention_packed_f32(
        q: &GpuTensor,
        k: &GpuTensor,
        v: &GpuTensor,
        head_count: usize,
        scale: f32,
    ) -> Result<GpuTensor, String> {
        gpu_attention_packed_composite(
            q,
            k,
            v,
            head_count,
            scale,
            PackedAttentionMask::None,
            PackedAttentionPrecision::F32,
        )
    }

    /// Bidirectional sliding-window self-attention (`|i-j| <= window`) in
    /// explicit f32. Matches ACE's CPU `ace_gqa_attention` mask.
    pub fn gpu_attention_packed_sliding(
        q: &GpuTensor,
        k: &GpuTensor,
        v: &GpuTensor,
        head_count: usize,
        scale: f32,
        window: usize,
    ) -> Result<GpuTensor, String> {
        let seq = q.rows;
        if window + 1 >= seq {
            return gpu_attention_packed_f32(q, k, v, head_count, scale);
        }
        gpu_attention_packed_composite(
            q,
            k,
            v,
            head_count,
            scale,
            PackedAttentionMask::Sliding { window },
            PackedAttentionPrecision::F32,
        )
    }

    /// Bidirectional FA2 with BF16 tensor-core operands. `window = 0` is
    /// full attention; `window > 0` keeps `|i-j| <= window`.
    pub fn gpu_attention_packed_fa2_bf16(
        q: &GpuTensor,
        k: &GpuTensor,
        v: &GpuTensor,
        head_count: usize,
        scale: f32,
        window: usize,
    ) -> Result<GpuTensor, String> {
        let seq = q.rows;
        let hidden = q.cols;
        if k.rows != seq || v.rows != seq || k.cols != hidden || v.cols != hidden {
            return Err("gpu_attention_packed_fa2_bf16 shape mismatch".to_string());
        }
        if head_count == 0 || hidden % head_count != 0 || hidden / head_count != 128 {
            return Err("gpu_attention_packed_fa2_bf16 wants head_dim 128".to_string());
        }
        if q.half || k.half || v.half {
            return Err("gpu_attention_packed_fa2_bf16 expects f32 storage".to_string());
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let elems = seq * hidden;
            let q16 = gpu_pool_acquire(elems * size_of::<u16>())?;
            let k16 = gpu_pool_acquire(elems * size_of::<u16>())?;
            let v16 = gpu_pool_acquire(elems * size_of::<u16>())?;
            for (src, dst) in [(q, &q16), (k, &k16), (v, &v16)] {
                let status = unsafe {
                    makepad_cuda_f32_to_bf16(
                        src.device_ptr()?,
                        dst.ptr.as_ptr().cast::<u16>(),
                        elems as u32,
                        backend.stream,
                    )
                };
                gpu_check(status)?;
            }
            let out = GpuTensor::from_pool(seq, hidden)?;
            let win = if window + 1 >= seq { 0 } else { window as i32 };
            let status = unsafe {
                makepad_cuda_flash_attention2_sliding_bf16(
                    q16.ptr.as_ptr().cast::<u16>(),
                    k16.ptr.as_ptr().cast::<u16>(),
                    v16.ptr.as_ptr().cast::<u16>(),
                    out.device_ptr()?,
                    seq as u32,
                    head_count as u32,
                    hidden as u32,
                    scale,
                    win,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_pool_release(q16);
            gpu_pool_release(k16);
            gpu_pool_release(v16);
            gpu_prof(backend.stream, crate::prof::CAT_FLASH_ATTN, prof_start, 0);
            Ok(out)
        })
    }

    /// Sliding-window self-attention with BF16 Q/K/V and probability GEMM
    /// operands (f32 softmax). Matches official ACE-Step autocast.
    pub fn gpu_attention_packed_sliding_bf16(
        q: &GpuTensor,
        k: &GpuTensor,
        v: &GpuTensor,
        head_count: usize,
        scale: f32,
        window: usize,
    ) -> Result<GpuTensor, String> {
        let seq = q.rows;
        if window + 1 >= seq {
            return gpu_attention_packed_composite_bf16(q, k, v, head_count, scale);
        }
        gpu_attention_packed_composite(
            q,
            k,
            v,
            head_count,
            scale,
            PackedAttentionMask::Sliding { window },
            PackedAttentionPrecision::Bf16,
        )
    }

    /// Cross-attention with f32 QK/PV GEMMs and f32 softmax.
    pub fn gpu_attention_packed_cross_f32(
        q: &GpuTensor,
        k: &GpuTensor,
        v: &GpuTensor,
        head_count: usize,
        scale: f32,
    ) -> Result<GpuTensor, String> {
        gpu_attention_packed_cross_composite(
            q,
            k,
            v,
            head_count,
            scale,
            PackedAttentionPrecision::F32,
        )
    }

    /// Full bidirectional self-attention with explicit BF16 Q/K/V and
    /// probability GEMM operands, f32 accumulation, and f32 output. This is
    /// the precision contract used by SkinTokens under CUDA autocast.
    pub fn gpu_attention_packed_bf16(
        q: &GpuTensor,
        k: &GpuTensor,
        v: &GpuTensor,
        head_count: usize,
        scale: f32,
    ) -> Result<GpuTensor, String> {
        let seq = q.rows;
        let hidden = q.cols;
        if k.rows != seq || v.rows != seq || k.cols != hidden || v.cols != hidden {
            return Err("gpu_attention shape mismatch".to_string());
        }
        if head_count == 0 || hidden % head_count != 0 {
            return Err("gpu_attention head mismatch".to_string());
        }
        if q.half || k.half || v.half {
            return Err("gpu_attention_packed_bf16 expects f32 storage".to_string());
        }
        if hidden / head_count == 64 {
            return gpu_attention_packed_flash_bf16(q, k, v, head_count, scale);
        }
        gpu_attention_packed_composite(
            q,
            k,
            v,
            head_count,
            scale,
            PackedAttentionMask::None,
            PackedAttentionPrecision::Bf16,
        )
    }

    /// Full bidirectional BF16 self-attention through the materialized
    /// cuBLAS/softmax/cuBLAS implementation, bypassing the head-dimension-64
    /// flash route. This explicit entry point is useful for operator parity
    /// gates that need to distinguish flash reduction order from the shared
    /// BF16 input/output contract.
    pub fn gpu_attention_packed_composite_bf16(
        q: &GpuTensor,
        k: &GpuTensor,
        v: &GpuTensor,
        head_count: usize,
        scale: f32,
    ) -> Result<GpuTensor, String> {
        let seq = q.rows;
        let hidden = q.cols;
        if k.rows != seq || v.rows != seq || k.cols != hidden || v.cols != hidden {
            return Err("gpu_attention shape mismatch".to_string());
        }
        if head_count == 0 || hidden % head_count != 0 {
            return Err("gpu_attention head mismatch".to_string());
        }
        if q.half || k.half || v.half {
            return Err("gpu_attention_packed_composite_bf16 expects f32 storage".to_string());
        }
        gpu_attention_packed_composite(
            q,
            k,
            v,
            head_count,
            scale,
            PackedAttentionMask::None,
            PackedAttentionPrecision::Bf16,
        )
    }

    /// Materialized QK/softmax/PV with f32 operands and f32 accumulation.
    /// Closest in-repo match to torch MATH/EFFICIENT SDPA when flash is absent.
    pub fn gpu_attention_packed_composite_f32(
        q: &GpuTensor,
        k: &GpuTensor,
        v: &GpuTensor,
        head_count: usize,
        scale: f32,
    ) -> Result<GpuTensor, String> {
        let seq = q.rows;
        let hidden = q.cols;
        if k.rows != seq || v.rows != seq || k.cols != hidden || v.cols != hidden {
            return Err("gpu_attention shape mismatch".to_string());
        }
        if head_count == 0 || hidden % head_count != 0 {
            return Err("gpu_attention head mismatch".to_string());
        }
        if q.half || k.half || v.half {
            return Err("gpu_attention_packed_composite_f32 expects f32 storage".to_string());
        }
        gpu_attention_packed_composite(
            q,
            k,
            v,
            head_count,
            scale,
            PackedAttentionMask::None,
            PackedAttentionPrecision::F32,
        )
    }

    /// Causal packed FA2 (head_dim 128): online softmax matching official
    /// PyTorch SDPA/FlashAttention. `bf16` selects bf16 tensor-core operands
    /// (Qwen3 Music3) vs f16 (Flux-style). Falls back to the composite
    /// causal path when fused attention is off or head_dim != 128.
    pub fn gpu_attention_packed_causal_flash(
        q: &GpuTensor,
        k: &GpuTensor,
        v: &GpuTensor,
        head_count: usize,
        scale: f32,
        bf16: bool,
    ) -> Result<GpuTensor, String> {
        let seq = q.rows;
        let hidden = q.cols;
        if k.rows != seq || v.rows != seq || k.cols != hidden || v.cols != hidden {
            return Err("gpu_attention_packed_causal_flash shape mismatch".to_string());
        }
        if head_count == 0 || hidden % head_count != 0 {
            return Err("gpu_attention_packed_causal_flash head mismatch".to_string());
        }
        let head_dim = hidden / head_count;
        if !gpu_attention_fused_enabled() || head_dim != 128 {
            return gpu_attention_packed_composite(
                q,
                k,
                v,
                head_count,
                scale,
                PackedAttentionMask::Causal,
                if bf16 {
                    PackedAttentionPrecision::Bf16
                } else {
                    PackedAttentionPrecision::F16
                },
            );
        }
        if q.half != k.half || q.half != v.half {
            return Err("gpu_attention_packed_causal_flash mixed input dtypes".to_string());
        }
        if q.half && bf16 {
            return Err("gpu_attention_packed_causal_flash bf16 wants f32 storage".to_string());
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let elems = seq * hidden;
            let mut halves: Option<(DeviceBuffer, DeviceBuffer, DeviceBuffer)> = None;
            if !q.half {
                let q16 = gpu_pool_acquire(elems * size_of::<u16>())?;
                let k16 = gpu_pool_acquire(elems * size_of::<u16>())?;
                let v16 = gpu_pool_acquire(elems * size_of::<u16>())?;
                for (src, dst) in [(q, &q16), (k, &k16), (v, &v16)] {
                    let status = unsafe {
                        if bf16 {
                            makepad_cuda_f32_to_bf16(
                                src.device_ptr()?,
                                dst.ptr.as_ptr().cast::<u16>(),
                                elems as u32,
                                backend.stream,
                            )
                        } else {
                            makepad_cuda_f32_to_f16(
                                src.device_ptr()?,
                                dst.ptr.as_ptr().cast::<u16>(),
                                elems as u32,
                                backend.stream,
                            )
                        }
                    };
                    gpu_check(status)?;
                }
                halves = Some((q16, k16, v16));
            }
            let (q_ptr, k_ptr, v_ptr): (*const u16, *const u16, *const u16) = match &halves {
                Some((q16, k16, v16)) => (
                    q16.ptr.as_ptr().cast::<u16>(),
                    k16.ptr.as_ptr().cast::<u16>(),
                    v16.ptr.as_ptr().cast::<u16>(),
                ),
                None => (
                    q.device_ptr_u16()?,
                    k.device_ptr_u16()?,
                    v.device_ptr_u16()?,
                ),
            };
            let out = GpuTensor::from_pool(seq, hidden)?;
            let status = unsafe {
                if bf16 {
                    makepad_cuda_flash_attention2_causal_bf16(
                        q_ptr,
                        k_ptr,
                        v_ptr,
                        out.device_ptr()?,
                        seq as u32,
                        head_count as u32,
                        hidden as u32,
                        scale,
                        backend.stream,
                    )
                } else {
                    makepad_cuda_flash_attention2_causal_f32(
                        q_ptr,
                        k_ptr,
                        v_ptr,
                        out.device_ptr()?,
                        seq as u32,
                        head_count as u32,
                        hidden as u32,
                        scale,
                        backend.stream,
                    )
                }
            };
            gpu_check(status)?;
            if let Some((q16, k16, v16)) = halves {
                gpu_pool_release(q16);
                gpu_pool_release(k16);
                gpu_pool_release(v16);
            }
            gpu_prof(backend.stream, crate::prof::CAT_FLASH_ATTN, prof_start, 0);
            Ok(out)
        })
    }

    /// Decode / cross FA2: `q` is `[q_len, H*D]`, `k`/`v` are `[kv_len, H*D]`.
    /// One-token GQA decode uses q_len=1 (all KV allowed). Same fused kernel
    /// as causal prefill so Music3 decode stays in the official SDPA family.
    pub fn gpu_attention_packed_flash_cross(
        q: &GpuTensor,
        k: &GpuTensor,
        v: &GpuTensor,
        head_count: usize,
        scale: f32,
        bf16: bool,
    ) -> Result<GpuTensor, String> {
        let q_len = q.rows;
        let kv_len = k.rows;
        let hidden = q.cols;
        if v.rows != kv_len || k.cols != hidden || v.cols != hidden {
            return Err("gpu_attention_packed_flash_cross shape mismatch".to_string());
        }
        if head_count == 0 || hidden % head_count != 0 {
            return Err("gpu_attention_packed_flash_cross head mismatch".to_string());
        }
        let head_dim = hidden / head_count;
        if !gpu_attention_fused_enabled() || head_dim != 128 {
            return Err("gpu_attention_packed_flash_cross needs fused head_dim 128".to_string());
        }
        if q.half != k.half || q.half != v.half {
            return Err("gpu_attention_packed_flash_cross mixed input dtypes".to_string());
        }
        if q.half && bf16 {
            return Err("gpu_attention_packed_flash_cross bf16 wants f32 storage".to_string());
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let q_elems = q_len * hidden;
            let kv_elems = kv_len * hidden;
            let mut halves: Option<(DeviceBuffer, DeviceBuffer, DeviceBuffer)> = None;
            if !q.half {
                let q16 = gpu_pool_acquire(q_elems * size_of::<u16>())?;
                let k16 = gpu_pool_acquire(kv_elems * size_of::<u16>())?;
                let v16 = gpu_pool_acquire(kv_elems * size_of::<u16>())?;
                for (src, dst, elems) in [
                    (q, &q16, q_elems),
                    (k, &k16, kv_elems),
                    (v, &v16, kv_elems),
                ] {
                    let status = unsafe {
                        if bf16 {
                            makepad_cuda_f32_to_bf16(
                                src.device_ptr()?,
                                dst.ptr.as_ptr().cast::<u16>(),
                                elems as u32,
                                backend.stream,
                            )
                        } else {
                            makepad_cuda_f32_to_f16(
                                src.device_ptr()?,
                                dst.ptr.as_ptr().cast::<u16>(),
                                elems as u32,
                                backend.stream,
                            )
                        }
                    };
                    gpu_check(status)?;
                }
                halves = Some((q16, k16, v16));
            }
            let (q_ptr, k_ptr, v_ptr): (*const u16, *const u16, *const u16) = match &halves {
                Some((q16, k16, v16)) => (
                    q16.ptr.as_ptr().cast::<u16>(),
                    k16.ptr.as_ptr().cast::<u16>(),
                    v16.ptr.as_ptr().cast::<u16>(),
                ),
                None => (
                    q.device_ptr_u16()?,
                    k.device_ptr_u16()?,
                    v.device_ptr_u16()?,
                ),
            };
            let out = GpuTensor::from_pool(q_len, hidden)?;
            let status = unsafe {
                if bf16 {
                    makepad_cuda_flash_attention2_cross_bf16(
                        q_ptr,
                        k_ptr,
                        v_ptr,
                        out.device_ptr()?,
                        q_len as u32,
                        kv_len as u32,
                        head_count as u32,
                        hidden as u32,
                        scale,
                        backend.stream,
                    )
                } else {
                    makepad_cuda_flash_attention2_cross_f32(
                        q_ptr,
                        k_ptr,
                        v_ptr,
                        out.device_ptr()?,
                        q_len as u32,
                        kv_len as u32,
                        head_count as u32,
                        hidden as u32,
                        scale,
                        backend.stream,
                    )
                }
            };
            gpu_check(status)?;
            if let Some((q16, k16, v16)) = halves {
                gpu_pool_release(q16);
                gpu_pool_release(k16);
                gpu_pool_release(v16);
            }
            gpu_prof(backend.stream, crate::prof::CAT_FLASH_ATTN, prof_start, 0);
            Ok(out)
        })
    }

    /// FA2 cross/self attention (head_dim 128, bf16 tensor cores) with
    /// **round-to-nearest-even** input staging. The wrapper above stages
    /// through the legacy truncating f32->bf16 converter; a bf16 PyTorch
    /// model rounds RN-even at every op boundary, so truncation biases
    /// q/k/v systematically low. Additive on purpose: Flux2 Klein is the
    /// only caller, the truncating variant keeps its validated numerics
    /// for the other lanes.
    pub fn gpu_attention_packed_flash_cross_bf16_rn(
        q: &GpuTensor,
        k: &GpuTensor,
        v: &GpuTensor,
        head_count: usize,
        scale: f32,
    ) -> Result<GpuTensor, String> {
        let q_len = q.rows;
        let kv_len = k.rows;
        let hidden = q.cols;
        if v.rows != kv_len || k.cols != hidden || v.cols != hidden {
            return Err("gpu_attention_packed_flash_cross_bf16_rn shape mismatch".to_string());
        }
        if head_count == 0 || hidden % head_count != 0 || hidden / head_count != 128 {
            return Err("gpu_attention_packed_flash_cross_bf16_rn needs head_dim 128".to_string());
        }
        if q.half || k.half || v.half {
            return Err("gpu_attention_packed_flash_cross_bf16_rn expects f32 storage".to_string());
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let q_elems = q_len * hidden;
            let kv_elems = kv_len * hidden;
            let q16 = gpu_pool_acquire(q_elems * size_of::<u16>())?;
            let k16 = gpu_pool_acquire(kv_elems * size_of::<u16>())?;
            let v16 = gpu_pool_acquire(kv_elems * size_of::<u16>())?;
            for (src, dst, elems) in [
                (q, &q16, q_elems),
                (k, &k16, kv_elems),
                (v, &v16, kv_elems),
            ] {
                let status = unsafe {
                    makepad_cuda_f32_to_bf16_rn(
                        src.device_ptr()?,
                        dst.ptr.as_ptr().cast::<u16>(),
                        elems as u32,
                        backend.stream,
                    )
                };
                gpu_check(status)?;
            }
            let out = GpuTensor::from_pool(q_len, hidden)?;
            let status = unsafe {
                makepad_cuda_flash_attention2_cross_bf16(
                    q16.ptr.as_ptr().cast::<u16>(),
                    k16.ptr.as_ptr().cast::<u16>(),
                    v16.ptr.as_ptr().cast::<u16>(),
                    out.device_ptr()?,
                    q_len as u32,
                    kv_len as u32,
                    head_count as u32,
                    hidden as u32,
                    scale,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_pool_release(q16);
            gpu_pool_release(k16);
            gpu_pool_release(v16);
            gpu_prof(backend.stream, crate::prof::CAT_FLASH_ATTN, prof_start, 0);
            Ok(out)
        })
    }

    /// FA2 cross/self attention where q/k/v are first rounded to the
    /// oracle's bf16 value grid (RN-even) and then carried EXACTLY in f16
    /// (bf16 mantissa 7 <= f16 mantissa 10; the range fits flux2's post-
    /// norm activations): the QK products are computed over the oracle's
    /// own operand values with f32 accumulation, while P keeps f16's finer
    /// rounding for the PV mma.
    pub fn gpu_attention_packed_flash_cross_bf16pre_f16(
        q: &GpuTensor,
        k: &GpuTensor,
        v: &GpuTensor,
        head_count: usize,
        scale: f32,
    ) -> Result<GpuTensor, String> {
        let q_len = q.rows;
        let kv_len = k.rows;
        let hidden = q.cols;
        if v.rows != kv_len || k.cols != hidden || v.cols != hidden {
            return Err("gpu_attention_packed_flash_cross_bf16pre_f16 shape mismatch".to_string());
        }
        if head_count == 0 || hidden % head_count != 0 || hidden / head_count != 128 {
            return Err(
                "gpu_attention_packed_flash_cross_bf16pre_f16 needs head_dim 128".to_string(),
            );
        }
        if q.half || k.half || v.half {
            return Err(
                "gpu_attention_packed_flash_cross_bf16pre_f16 expects f32 storage".to_string(),
            );
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let q_elems = q_len * hidden;
            let kv_elems = kv_len * hidden;
            let q16 = gpu_pool_acquire(q_elems * size_of::<u16>())?;
            let k16 = gpu_pool_acquire(kv_elems * size_of::<u16>())?;
            let v16 = gpu_pool_acquire(kv_elems * size_of::<u16>())?;
            for (src, dst, elems) in [
                (q, &q16, q_elems),
                (k, &k16, kv_elems),
                (v, &v16, kv_elems),
            ] {
                let status = unsafe {
                    makepad_cuda_f32_to_bf16_rn_f16(
                        src.device_ptr()?,
                        dst.ptr.as_ptr().cast::<u16>(),
                        elems as u32,
                        backend.stream,
                    )
                };
                gpu_check(status)?;
            }
            let out = GpuTensor::from_pool(q_len, hidden)?;
            let status = unsafe {
                makepad_cuda_flash_attention2_cross_f32(
                    q16.ptr.as_ptr().cast::<u16>(),
                    k16.ptr.as_ptr().cast::<u16>(),
                    v16.ptr.as_ptr().cast::<u16>(),
                    out.device_ptr()?,
                    q_len as u32,
                    kv_len as u32,
                    head_count as u32,
                    hidden as u32,
                    scale,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_pool_release(q16);
            gpu_pool_release(k16);
            gpu_pool_release(v16);
            gpu_prof(backend.stream, crate::prof::CAT_FLASH_ATTN, prof_start, 0);
            Ok(out)
        })
    }

    /// CAUSAL packed attention (decoder-LM semantics — the H3 text encoder).
    /// Always the composite path: the fused FA2 kernel is unmasked-only.
    /// Music3 Qwen3 prefill uses `gpu_attention_packed_causal_flash` instead.
    pub fn gpu_attention_packed_causal(
        q: &GpuTensor,
        k: &GpuTensor,
        v: &GpuTensor,
        head_count: usize,
        scale: f32,
    ) -> Result<GpuTensor, String> {
        let seq = q.rows;
        let hidden = q.cols;
        if k.rows != seq || v.rows != seq || k.cols != hidden || v.cols != hidden {
            return Err("gpu_attention shape mismatch".to_string());
        }
        if head_count == 0 || hidden % head_count != 0 {
            return Err("gpu_attention head mismatch".to_string());
        }
        gpu_attention_packed_composite(
            q,
            k,
            v,
            head_count,
            scale,
            PackedAttentionMask::Causal,
            PackedAttentionPrecision::Environment,
        )
    }

    /// Torch SDPA MATH: f32 QK GEMM, f32 causal softmax, f32 PV. Independent
    /// of FLUX_ATTN_F16 (which otherwise makes Environment composite f16).
    pub fn gpu_attention_packed_causal_f32(
        q: &GpuTensor,
        k: &GpuTensor,
        v: &GpuTensor,
        head_count: usize,
        scale: f32,
    ) -> Result<GpuTensor, String> {
        let seq = q.rows;
        let hidden = q.cols;
        if k.rows != seq || v.rows != seq || k.cols != hidden || v.cols != hidden {
            return Err("gpu_attention shape mismatch".to_string());
        }
        if head_count == 0 || hidden % head_count != 0 {
            return Err("gpu_attention head mismatch".to_string());
        }
        if q.half || k.half || v.half {
            return Err("gpu_attention_packed_causal_f32 expects f32 storage".to_string());
        }
        gpu_attention_packed_composite(
            q,
            k,
            v,
            head_count,
            scale,
            PackedAttentionMask::Causal,
            PackedAttentionPrecision::F32,
        )
    }

    /// Causal packed attention with BF16 Q/K/V and probability GEMM operands,
    /// f32 accumulation, and f32 output. Unlike the general diffusion path,
    /// this never changes precision in response to Flux environment flags.
    pub fn gpu_attention_packed_causal_bf16(
        q: &GpuTensor,
        k: &GpuTensor,
        v: &GpuTensor,
        head_count: usize,
        scale: f32,
    ) -> Result<GpuTensor, String> {
        let seq = q.rows;
        let hidden = q.cols;
        if k.rows != seq || v.rows != seq || k.cols != hidden || v.cols != hidden {
            return Err("gpu_attention shape mismatch".to_string());
        }
        if head_count == 0 || hidden % head_count != 0 {
            return Err("gpu_attention head mismatch".to_string());
        }
        if q.half || k.half || v.half {
            return Err("gpu_attention_packed_causal_bf16 expects f32 storage".to_string());
        }
        gpu_attention_packed_composite(
            q,
            k,
            v,
            head_count,
            scale,
            PackedAttentionMask::Causal,
            PackedAttentionPrecision::Bf16,
        )
    }

    /// Causal packed attention with explicit F16 Q/K/V and probability GEMM
    /// operands plus f32 accumulation/output. This is independent of the
    /// process-wide Flux attention precision switch.
    pub fn gpu_attention_packed_causal_f16(
        q: &GpuTensor,
        k: &GpuTensor,
        v: &GpuTensor,
        head_count: usize,
        scale: f32,
    ) -> Result<GpuTensor, String> {
        let seq = q.rows;
        let hidden = q.cols;
        if k.rows != seq || v.rows != seq || k.cols != hidden || v.cols != hidden {
            return Err("gpu_attention shape mismatch".to_string());
        }
        if head_count == 0 || hidden % head_count != 0 {
            return Err("gpu_attention head mismatch".to_string());
        }
        if q.half || k.half || v.half {
            return Err("gpu_attention_packed_causal_f16 expects f32 storage".to_string());
        }
        gpu_attention_packed_composite(
            q,
            k,
            v,
            head_count,
            scale,
            PackedAttentionMask::Causal,
            PackedAttentionPrecision::F16,
        )
    }

    /// Packed attention with HY-Motion's asymmetric two-stream mask.
    ///
    /// The first `motion_tokens` rows are motion. Their queries attend to a
    /// +/- `band_radius` motion window and every following text row. Text
    /// queries attend only to text rows. Padding rows must already have been
    /// removed by the caller.
    pub fn gpu_attention_packed_motion_text(
        q: &GpuTensor,
        k: &GpuTensor,
        v: &GpuTensor,
        head_count: usize,
        scale: f32,
        motion_tokens: usize,
        band_radius: usize,
    ) -> Result<GpuTensor, String> {
        let seq = q.rows;
        let hidden = q.cols;
        if k.rows != seq || v.rows != seq || k.cols != hidden || v.cols != hidden {
            return Err("gpu_attention_motion_text shape mismatch".to_string());
        }
        if head_count == 0 || hidden % head_count != 0 {
            return Err("gpu_attention_motion_text head mismatch".to_string());
        }
        if motion_tokens == 0 || motion_tokens >= seq {
            return Err(format!(
                "gpu_attention_motion_text requires 0 < motion_tokens < seq, got {motion_tokens}/{seq}"
            ));
        }
        gpu_attention_packed_composite(
            q,
            k,
            v,
            head_count,
            scale,
            PackedAttentionMask::MotionText {
                motion_tokens,
                band_radius,
            },
            PackedAttentionPrecision::Environment,
        )
    }

    /// The fused flash-attention kernel path (head_dim 128 only): q/k/v are
    /// converted to f16 once (the kernel's cp.async tile staging copies raw
    /// bytes), then one kernel launch.
    fn gpu_attention_packed_fused(
        q: &GpuTensor,
        k: &GpuTensor,
        v: &GpuTensor,
        head_count: usize,
        scale: f32,
    ) -> Result<GpuTensor, String> {
        let seq = q.rows;
        let hidden = q.cols;
        if q.half != k.half || q.half != v.half {
            return Err("gpu_attention mixed input dtypes".to_string());
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let elems = seq * hidden;
            // f16 spine inputs feed the kernel directly; f32 inputs are
            // converted into transient pool buffers first.
            let mut halves: Option<(DeviceBuffer, DeviceBuffer, DeviceBuffer)> = None;
            if !q.half {
                let q16 = gpu_pool_acquire(elems * size_of::<u16>())?;
                let k16 = gpu_pool_acquire(elems * size_of::<u16>())?;
                let v16 = gpu_pool_acquire(elems * size_of::<u16>())?;
                for (src, dst) in [(q, &q16), (k, &k16), (v, &v16)] {
                    let status = unsafe {
                        makepad_cuda_f32_to_f16(
                            src.device_ptr()?,
                            dst.ptr.as_ptr().cast::<u16>(),
                            elems as u32,
                            backend.stream,
                        )
                    };
                    gpu_check(status)?;
                }
                halves = Some((q16, k16, v16));
            }
            let (q_ptr, k_ptr, v_ptr): (*const u16, *const u16, *const u16) = match &halves {
                Some((q16, k16, v16)) => (
                    q16.ptr.as_ptr().cast::<u16>(),
                    k16.ptr.as_ptr().cast::<u16>(),
                    v16.ptr.as_ptr().cast::<u16>(),
                ),
                None => (
                    q.device_ptr_u16()?,
                    k.device_ptr_u16()?,
                    v.device_ptr_u16()?,
                ),
            };
            let out = GpuTensor::from_pool(seq, hidden)?;
            let status = if gpu_attention_mma_enabled() {
                unsafe {
                    makepad_cuda_flash_attention2_f32(
                        q_ptr,
                        k_ptr,
                        v_ptr,
                        out.device_ptr()?,
                        seq as u32,
                        head_count as u32,
                        hidden as u32,
                        scale,
                        backend.stream,
                    )
                }
            } else {
                unsafe {
                    makepad_cuda_flash_attention_f32(
                        q_ptr,
                        k_ptr,
                        v_ptr,
                        out.device_ptr()?,
                        seq as u32,
                        head_count as u32,
                        hidden as u32,
                        scale,
                        backend.stream,
                    )
                }
            };
            gpu_check(status)?;
            if let Some((q16, k16, v16)) = halves {
                gpu_pool_release(q16);
                gpu_pool_release(k16);
                gpu_pool_release(v16);
            }
            gpu_prof(backend.stream, crate::prof::CAT_FLASH_ATTN, prof_start, 0);
            Ok(out)
        })
    }

    /// The cublas composite fallback (see gpu_attention_packed).
    #[derive(Clone, Copy)]
    enum PackedAttentionMask {
        None,
        Causal,
        Sliding {
            window: usize,
        },
        MotionText {
            motion_tokens: usize,
            band_radius: usize,
        },
    }

    #[derive(Clone, Copy, Eq, PartialEq)]
    enum PackedAttentionPrecision {
        Environment,
        Bf16,
        F16,
        /// Torch SDPA MATH: f32 QK / softmax / PV. Official dump60 on 169.
        F32,
    }

    fn gpu_attention_packed_composite(
        q: &GpuTensor,
        k: &GpuTensor,
        v: &GpuTensor,
        head_count: usize,
        scale: f32,
        mask: PackedAttentionMask,
        precision: PackedAttentionPrecision,
    ) -> Result<GpuTensor, String> {
        let seq = q.rows;
        let hidden = q.cols;
        let head_dim = hidden / head_count;
        let use_half = match precision {
            PackedAttentionPrecision::F32 => false,
            PackedAttentionPrecision::Environment => gpu_attention_f16_enabled(),
            PackedAttentionPrecision::Bf16 | PackedAttentionPrecision::F16 => true,
        };
        let half_type = if precision == PackedAttentionPrecision::Bf16 {
            crate::CUDA_R_16BF
        } else {
            crate::CUDA_R_16F
        };
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let scores_len = head_count
                .checked_mul(seq)
                .and_then(|len| len.checked_mul(seq))
                .ok_or_else(|| "gpu_attention scores overflow".to_string())?;
            let scores = gpu_pool_acquire(scores_len * size_of::<f32>())?;
            let scores_ptr = scores.ptr.as_ptr().cast::<f32>();
            let beta = 0.0f32;
            let one = 1.0f32;

            // Optional f16 copies of the gemm inputs. f16 spine inputs skip
            // the copies entirely (they only ever need the probs buffer).
            let elems = seq * hidden;
            if q.half
                && (!use_half
                    || precision == PackedAttentionPrecision::Bf16
                    || q.half != k.half
                    || q.half != v.half)
            {
                return Err(
                    "gpu_attention composite f32 path cannot consume f16 inputs".to_string(),
                );
            }
            let mut halves: Option<(DeviceBuffer, DeviceBuffer, DeviceBuffer, DeviceBuffer)> =
                None;
            let mut p16_spine: Option<DeviceBuffer> = None;
            if q.half {
                p16_spine = Some(gpu_pool_acquire(scores_len * size_of::<u16>())?);
            } else if use_half {
                let q16 = gpu_pool_acquire(elems * size_of::<u16>())?;
                let k16 = gpu_pool_acquire(elems * size_of::<u16>())?;
                let v16 = gpu_pool_acquire(elems * size_of::<u16>())?;
                let p16 = gpu_pool_acquire(scores_len * size_of::<u16>())?;
                for (src, dst) in [(q, &q16), (k, &k16), (v, &v16)] {
                    let status = unsafe {
                        if precision == PackedAttentionPrecision::Bf16 {
                            makepad_cuda_f32_to_bf16(
                                src.device_ptr()?,
                                dst.ptr.as_ptr().cast::<u16>(),
                                elems as u32,
                                backend.stream,
                            )
                        } else {
                            makepad_cuda_f32_to_f16(
                                src.device_ptr()?,
                                dst.ptr.as_ptr().cast::<u16>(),
                                elems as u32,
                                backend.stream,
                            )
                        }
                    };
                    gpu_check(status)?;
                }
                halves = Some((q16, k16, v16, p16));
            }
            let (qk_a, qk_b, qk_type) = if q.half {
                (
                    k.device_ptr_u16()?.cast::<std::ffi::c_void>(),
                    q.device_ptr_u16()?.cast::<std::ffi::c_void>(),
                    crate::CUDA_R_16F,
                )
            } else {
                match &halves {
                    Some((q16, k16, _, _)) => (
                        k16.ptr.as_ptr(),
                        q16.ptr.as_ptr(),
                        half_type,
                    ),
                    None => (
                        k.device_ptr()?.cast::<std::ffi::c_void>(),
                        q.device_ptr()?.cast::<std::ffi::c_void>(),
                        crate::CUDA_R_32F,
                    ),
                }
            };
            unsafe {
                // scores[h][i][j] = scale * sum_d q[i][h][d] * k[j][h][d]
                crate::cublas_gemm_strided_batched_ex(
                    backend.blas,
                    crate::CUBLAS_OP_T,
                    crate::CUBLAS_OP_N,
                    seq as i32,
                    seq as i32,
                    head_dim as i32,
                    &scale,
                    qk_a,
                    qk_type,
                    hidden as i32,
                    head_dim as i64,
                    qk_b,
                    qk_type,
                    hidden as i32,
                    head_dim as i64,
                    &beta,
                    scores_ptr.cast::<std::ffi::c_void>(),
                    crate::CUDA_R_32F,
                    seq as i32,
                    (seq * seq) as i64,
                    head_count as i32,
                    crate::CUBLAS_COMPUTE_32F,
                    crate::CUBLAS_GEMM_DEFAULT,
                )
                .map_err(|err| format!("gpu_attention qk gemm failed: {err}"))?;
            }
            let status = match mask {
                PackedAttentionMask::Causal => unsafe {
                    makepad_cuda_softmax_rows_causal_f32(
                        scores_ptr,
                        seq as u32,
                        (head_count * seq) as u32,
                        seq as u32,
                        0,
                        seq as u32,
                        backend.stream,
                    )
                },
                PackedAttentionMask::MotionText {
                    motion_tokens,
                    band_radius,
                } => unsafe {
                    makepad_cuda_softmax_rows_motion_text_f32(
                        scores_ptr,
                        scores_ptr,
                        (head_count * seq) as u32,
                        seq as u32,
                        seq as u32,
                        motion_tokens as u32,
                        band_radius as u32,
                        backend.stream,
                    )
                },
                PackedAttentionMask::None => unsafe {
                    makepad_cuda_softmax_rows_precise_f32(
                        scores_ptr,
                        scores_ptr,
                        (head_count * seq) as u32,
                        seq as u32,
                        seq as u32,
                        backend.stream,
                    )
                },
                PackedAttentionMask::Sliding { window } => unsafe {
                    makepad_cuda_softmax_rows_sliding_f32(
                        scores_ptr,
                        scores_ptr,
                        (head_count * seq) as u32,
                        seq as u32,
                        seq as u32,
                        window as u32,
                        backend.stream,
                    )
                },
            };
            gpu_check(status)?;
            let out = GpuTensor::from_pool(seq, hidden)?;
            let pv_f16: Option<(*const std::ffi::c_void, *const std::ffi::c_void)> =
                if let Some(p16) = &p16_spine {
                    Some((
                        v.device_ptr_u16()?.cast::<std::ffi::c_void>(),
                        p16.ptr.as_ptr().cast::<u16>().cast::<std::ffi::c_void>(),
                    ))
                } else if let Some((_, _, v16, p16)) = &halves {
                    Some((
                        v16.ptr.as_ptr().cast_const(),
                        p16.ptr.as_ptr().cast_const(),
                    ))
                } else {
                    None
                };
            if let Some((v_ptr, p_ptr)) = pv_f16 {
                let p16_dst: *mut u16 = p_ptr.cast_mut().cast::<u16>();
                let status = unsafe {
                    if precision == PackedAttentionPrecision::Bf16 {
                        makepad_cuda_f32_to_bf16(
                            scores_ptr,
                            p16_dst,
                            scores_len as u32,
                            backend.stream,
                        )
                    } else {
                        makepad_cuda_f32_to_f16(
                            scores_ptr,
                            p16_dst,
                            scores_len as u32,
                            backend.stream,
                        )
                    }
                };
                gpu_check(status)?;
                unsafe {
                    // out[i][h][d] = sum_j probs[h][i][j] * v[j][h][d]
                    crate::cublas_gemm_strided_batched_ex(
                        backend.blas,
                        crate::CUBLAS_OP_N,
                        crate::CUBLAS_OP_N,
                        head_dim as i32,
                        seq as i32,
                        seq as i32,
                        &one,
                        v_ptr.cast_mut(),
                        half_type,
                        hidden as i32,
                        head_dim as i64,
                        p_ptr.cast_mut(),
                        half_type,
                        seq as i32,
                        (seq * seq) as i64,
                        &beta,
                        out.device_ptr()?.cast::<std::ffi::c_void>(),
                        crate::CUDA_R_32F,
                        hidden as i32,
                        head_dim as i64,
                        head_count as i32,
                        crate::CUBLAS_COMPUTE_32F,
                        crate::CUBLAS_GEMM_DEFAULT,
                    )
                    .map_err(|err| format!("gpu_attention pv gemm failed: {err}"))?;
                }
            } else {
                unsafe {
                    // out[i][h][d] = sum_j probs[h][i][j] * v[j][h][d]
                    crate::cublas_gemm_strided_batched_ex(
                        backend.blas,
                        crate::CUBLAS_OP_N,
                        crate::CUBLAS_OP_N,
                        head_dim as i32,
                        seq as i32,
                        seq as i32,
                        &one,
                        v.device_ptr()?.cast::<std::ffi::c_void>(),
                        crate::CUDA_R_32F,
                        hidden as i32,
                        head_dim as i64,
                        scores_ptr.cast::<std::ffi::c_void>(),
                        crate::CUDA_R_32F,
                        seq as i32,
                        (seq * seq) as i64,
                        &beta,
                        out.device_ptr()?.cast::<std::ffi::c_void>(),
                        crate::CUDA_R_32F,
                        hidden as i32,
                        head_dim as i64,
                        head_count as i32,
                        crate::CUBLAS_COMPUTE_32F,
                        crate::CUBLAS_GEMM_DEFAULT,
                    )
                    .map_err(|err| format!("gpu_attention pv gemm failed: {err}"))?;
                }
            }
            if let Some((q16, k16, v16, p16)) = halves {
                gpu_pool_release(q16);
                gpu_pool_release(k16);
                gpu_pool_release(v16);
                gpu_pool_release(p16);
            }
            if let Some(p16) = p16_spine {
                gpu_pool_release(p16);
            }
            gpu_pool_release(scores);
            gpu_prof(backend.stream, crate::prof::CAT_FLASH_ATTN, prof_start, 0);
            Ok(out)
        })
    }

    pub fn gpu_gelu(x: &GpuTensor) -> Result<GpuTensor, String> {
        if x.half {
            return gpu_gelu_f16_inner(x, None);
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(x.rows, x.cols)?;
            let status = unsafe {
                makepad_cuda_gelu_f32_precise(
                    x.device_ptr()?,
                    out.device_ptr()?,
                    (x.rows * x.cols) as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_prof(backend.stream, crate::prof::CAT_ELEMENTWISE, prof_start, 0);
            Ok(out)
        })
    }

    /// GELU on f16 storage with an optional cached f32 bias folded in (a
    /// deferred linear bias, so the mlp.0 C never leaves f16).
    fn gpu_gelu_f16_inner(
        x: &GpuTensor,
        bias_ptr: Option<*const f32>,
    ) -> Result<GpuTensor, String> {
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool_half(x.rows, x.cols)?;
            let status = unsafe {
                makepad_cuda_gelu_f16(
                    x.device_ptr_u16()?,
                    bias_ptr.unwrap_or(std::ptr::null()),
                    out.device_ptr_u16()?,
                    x.rows as u32,
                    x.cols as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_prof(backend.stream, crate::prof::CAT_ELEMENTWISE, prof_start, 0);
            Ok(out)
        })
    }

    /// gelu(x + bias) on an f16 tensor; the bias is model-constant and
    /// device-cached under the given key.
    pub fn gpu_gelu_bias_f16(
        x: &GpuTensor,
        cache_namespace: &str,
        cache_key: &str,
        bias: &[f32],
    ) -> Result<GpuTensor, String> {
        if !x.half {
            return Err("gpu_gelu_bias_f16 expects an f16 tensor".to_string());
        }
        if bias.len() != x.cols {
            return Err("gpu_gelu_bias_f16 bias mismatch".to_string());
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let key = format!("{cache_namespace}::{cache_key}::b");
            let bias_bytes = bias.len() * size_of::<f32>();
            backend.cached_weight_buffer(&key, bias_bytes, || {
                let raw = unsafe {
                    std::slice::from_raw_parts(bias.as_ptr().cast::<u8>(), bias_bytes)
                };
                Ok(raw.to_vec())
            })?;
            let out = GpuTensor::from_pool_half(x.rows, x.cols)?;
            let bias_ptr = backend
                .weight_buffers
                .get(&key)
                .ok_or_else(|| format!("missing cached CUDA bias buffer {key}"))?
                .ptr
                .as_ptr()
                .cast::<f32>()
                .cast_const();
            let status = unsafe {
                makepad_cuda_gelu_f16(
                    x.device_ptr_u16()?,
                    bias_ptr,
                    out.device_ptr_u16()?,
                    x.rows as u32,
                    x.cols as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_prof(backend.stream, crate::prof::CAT_ELEMENTWISE, prof_start, 0);
            Ok(out)
        })
    }

    /// gpu_layer_norm_mod with an f16 output (feeds the next linear's f16
    /// activation operand directly).
    pub fn gpu_layer_norm_mod_f16(
        x: &GpuTensor,
        mods: &GpuTensor,
        scale_off: usize,
        shift_off: usize,
        eps: f32,
    ) -> Result<GpuTensor, String> {
        let mods_len = mods.rows * mods.cols;
        if scale_off + x.cols > mods_len || shift_off + x.cols > mods_len {
            return Err("gpu_layer_norm_mod_f16 offset out of range".to_string());
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool_half(x.rows, x.cols)?;
            let mods_ptr = mods.device_ptr()?;
            let status = unsafe {
                makepad_cuda_layer_norm_mul_add_f32_out16(
                    x.device_ptr()?,
                    mods_ptr.add(scale_off),
                    mods_ptr.add(shift_off),
                    out.device_ptr_u16()?,
                    x.rows as u32,
                    x.cols as u32,
                    eps,
                    1.0,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_prof(backend.stream, crate::prof::CAT_LAYER_NORM, prof_start, 0);
            Ok(out)
        })
    }

    /// Elementwise f32 multiply with exact shape matching.
    pub fn gpu_mul(a: &GpuTensor, b: &GpuTensor) -> Result<GpuTensor, String> {
        if a.half || b.half || a.rows != b.rows || a.cols != b.cols {
            return Err("gpu_mul expects matching f32 tensors".to_string());
        }
        let len = a
            .rows
            .checked_mul(a.cols)
            .ok_or_else(|| "gpu_mul element count overflow".to_string())?;
        let len_u32 = u32::try_from(len)
            .map_err(|_| "gpu_mul element count exceeds CUDA limit".to_string())?;
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(a.rows, a.cols)?;
            let status = unsafe {
                makepad_cuda_mul_f32_precise(
                    a.device_ptr()?,
                    b.device_ptr()?,
                    out.device_ptr()?,
                    len_u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            Ok(out)
        })
    }

    /// Add one f32 bias value per row: `out[r, c] = x[r, c] + row_bias[r]`.
    ///
    /// The underlying planar-bias kernel uses the same memory layout when a
    /// matrix row is treated as one plane. The input is copied first so this
    /// public operation does not mutate either operand.
    pub fn gpu_add_rows_broadcast(
        x: &GpuTensor,
        row_bias: &GpuTensor,
    ) -> Result<GpuTensor, String> {
        let bias_len = row_bias
            .rows
            .checked_mul(row_bias.cols)
            .ok_or_else(|| "gpu_add_rows_broadcast bias size overflow".to_string())?;
        if x.half || row_bias.half || bias_len != x.rows {
            return Err(
                "gpu_add_rows_broadcast expects f32 input and one bias per row".to_string(),
            );
        }
        let len = x
            .rows
            .checked_mul(x.cols)
            .ok_or_else(|| "gpu_add_rows_broadcast element count overflow".to_string())?;
        let len_u32 = u32::try_from(len).map_err(|_| {
            "gpu_add_rows_broadcast element count exceeds CUDA limit".to_string()
        })?;
        let rows_u32 = u32::try_from(x.rows)
            .map_err(|_| "gpu_add_rows_broadcast row count exceeds CUDA limit".to_string())?;
        let cols_u32 = u32::try_from(x.cols)
            .map_err(|_| "gpu_add_rows_broadcast column count exceeds CUDA limit".to_string())?;
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(x.rows, x.cols)?;
            let copy_status = unsafe {
                makepad_cuda_copy_f32(
                    x.device_ptr()?,
                    out.device_ptr()?,
                    len_u32,
                    backend.stream,
                )
            };
            gpu_check(copy_status)?;
            let add_status = unsafe {
                makepad_cuda_add_planes_vec_f32(
                    out.device_ptr()?,
                    row_bias.device_ptr()?,
                    cols_u32,
                    rows_u32,
                    backend.stream,
                )
            };
            gpu_check(add_status)?;
            Ok(out)
        })
    }

    pub fn gpu_add(a: &GpuTensor, b: &GpuTensor) -> Result<GpuTensor, String> {
        if a.rows != b.rows || a.cols != b.cols || a.half != b.half {
            return Err(format!(
                "gpu_add shape mismatch {}x{} half={} vs {}x{} half={}",
                a.rows, a.cols, a.half, b.rows, b.cols, b.half
            ));
        }
        if a.half {
            return gpu_add_f16(a, b);
        }
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(a.rows, a.cols)?;
            let status = unsafe {
                makepad_cuda_add_f32_precise(
                    a.device_ptr()?,
                    b.device_ptr()?,
                    out.device_ptr()?,
                    (a.rows * a.cols) as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            Ok(out)
        })
    }

    /// Time-major snake: `y[t,c] = x[t,c] + inv_beta[c] * sin(alpha[c] * x[t,c])^2`.
    pub fn gpu_snake(x: &GpuTensor, alpha: &[f32], inv_beta: &[f32]) -> Result<GpuTensor, String> {
        if x.half {
            return Err("gpu_snake expects f32 storage".to_string());
        }
        if alpha.len() != x.cols || inv_beta.len() != x.cols {
            return Err(format!(
                "gpu_snake channel mismatch cols={} alpha={} inv_beta={}",
                x.cols,
                alpha.len(),
                inv_beta.len()
            ));
        }
        let alpha_g = gpu_upload(alpha, 1, x.cols)?;
        let inv_g = gpu_upload(inv_beta, 1, x.cols)?;
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(x.rows, x.cols)?;
            let status = unsafe {
                makepad_cuda_snake_rows_f32(
                    x.device_ptr()?,
                    alpha_g.device_ptr()?,
                    inv_g.device_ptr()?,
                    out.device_ptr()?,
                    x.rows as u32,
                    x.cols as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            Ok(out)
        })
    }

    /// Fold two strided transposed-conv GEMM outputs into time-major audio.
    /// `y_hi`/`y_lo` are `[in_len, stride * out_ch]`.
    pub fn gpu_tconv_stitch(
        y_hi: &GpuTensor,
        y_lo: &GpuTensor,
        in_len: usize,
        out_ch: usize,
        stride: usize,
        padding: usize,
        k: usize,
    ) -> Result<GpuTensor, String> {
        if y_hi.half || y_lo.half {
            return Err("gpu_tconv_stitch expects f32 storage".to_string());
        }
        if stride == 0 || y_hi.rows != in_len || y_lo.rows != in_len {
            return Err("gpu_tconv_stitch in_len mismatch".to_string());
        }
        if y_hi.cols != stride * out_ch || y_lo.cols != stride * out_ch {
            return Err("gpu_tconv_stitch channel mismatch".to_string());
        }
        let out_len = (in_len - 1) * stride + k - 2 * padding;
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(out_len, out_ch)?;
            let status = unsafe {
                makepad_cuda_tconv_stitch_f32(
                    y_hi.device_ptr()?,
                    y_lo.device_ptr()?,
                    out.device_ptr()?,
                    in_len as u32,
                    out_len as u32,
                    out_ch as u32,
                    stride as u32,
                    padding as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            Ok(out)
        })
    }

    fn gpu_add_f16(a: &GpuTensor, b: &GpuTensor) -> Result<GpuTensor, String> {
        let n_elem = i32::try_from(a.rows * a.cols)
            .map_err(|_| "gpu_add_f16 too large".to_string())?;
        let bytes = (a.rows * a.cols) * size_of::<u16>();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool_half(a.rows, a.cols)?;
            crate::check(unsafe {
                crate::cudaMemcpyAsync(
                    out.device_ptr_u16()?.cast(),
                    a.device_ptr_u16()?.cast_const().cast(),
                    bytes,
                    3,
                    backend.stream,
                )
            })
            .map_err(|err| err.to_string())?;
            crate::cudnn::add_same_f16(
                b.device_ptr_u16()?.cast(),
                out.device_ptr_u16()?.cast(),
                n_elem,
                1.0,
                1.0,
                backend.stream,
            )?;
            Ok(out)
        })
    }

    /// Residual add into an existing tensor (including a row view).
    pub fn gpu_add_into(a: &GpuTensor, b: &GpuTensor, out: &GpuTensor) -> Result<(), String> {
        if a.half || b.half || out.half {
            return Err("gpu_add_into expects f32 tensors".to_string());
        }
        if a.rows != b.rows || a.cols != b.cols || out.rows != a.rows || out.cols != a.cols {
            return Err("gpu_add_into shape mismatch".to_string());
        }
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let status = unsafe {
                makepad_cuda_add_f32_precise(
                    a.device_ptr()?,
                    b.device_ptr()?,
                    out.device_ptr()?,
                    (a.rows * a.cols) as u32,
                    backend.stream,
                )
            };
            gpu_check(status)
        })
    }

    /// Elementwise BF16 residual add, with an f32 storage result whose values
    /// are exactly representable as BF16.
    pub fn gpu_add_bf16(a: &GpuTensor, b: &GpuTensor) -> Result<GpuTensor, String> {
        if a.half || b.half || a.rows != b.rows || a.cols != b.cols {
            return Err("gpu_add_bf16 shape mismatch".to_string());
        }
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(a.rows, a.cols)?;
            let status = unsafe {
                makepad_cuda_add_bf16_f32(
                    a.device_ptr()?,
                    b.device_ptr()?,
                    out.device_ptr()?,
                    (a.rows * a.cols) as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            Ok(out)
        })
    }

    /// Round f32 storage values to BF16 (round-to-nearest-even) and expand
    /// them back into f32 storage for the device-tensor API.
    pub fn gpu_bf16_round(x: &GpuTensor) -> Result<GpuTensor, String> {
        if x.half {
            return Err("gpu_bf16_round expects f32 storage".to_string());
        }
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(x.rows, x.cols)?;
            let status = unsafe {
                makepad_cuda_bf16_round_f32(
                    x.device_ptr()?,
                    out.device_ptr()?,
                    (x.rows * x.cols) as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            Ok(out)
        })
    }

    /// residual + gate*update with gate broadcast per column.
    pub fn gpu_gated_residual(
        residual: &GpuTensor,
        update: &GpuTensor,
        gate: &[f32],
    ) -> Result<GpuTensor, String> {
        if residual.rows != update.rows
            || residual.cols != update.cols
            || gate.len() != update.cols
        {
            return Err("gpu_gated_residual shape mismatch".to_string());
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let gate_buf = gpu_upload_small(backend, gate)?;
            let out = GpuTensor::from_pool(residual.rows, residual.cols)?;
            let out_ptr = out.device_ptr()?;
            let status = unsafe {
                makepad_cuda_mul_rows_vec_f32(
                    update.device_ptr()?,
                    gate_buf.ptr.as_ptr().cast::<f32>(),
                    out_ptr,
                    update.rows as u32,
                    update.cols as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            let status = unsafe {
                makepad_cuda_add_f32_precise(
                    residual.device_ptr()?,
                    out_ptr,
                    out_ptr,
                    (residual.rows * residual.cols) as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_pool_release(gate_buf);
            gpu_prof(backend.stream, crate::prof::CAT_ELEMENTWISE, prof_start, 0);
            Ok(out)
        })
    }

    /// Column quantities of an f16 tensor expressed in f32 copy units: even
    /// f16 column counts halve into f32 elements, so the one f32 submatrix
    /// copy kernel serves both dtypes (every spine width is a multiple of the
    /// 3072 hidden size, so evenness always holds).
    fn gpu_copy_units(half: bool, cols: usize, what: &str) -> Result<usize, String> {
        if !half {
            return Ok(cols);
        }
        if cols % 2 != 0 {
            return Err(format!("{what}: odd f16 column count {cols}"));
        }
        Ok(cols / 2)
    }

    pub fn gpu_slice_cols(x: &GpuTensor, start: usize, len: usize) -> Result<GpuTensor, String> {
        if start + len > x.cols {
            return Err("gpu_slice_cols out of range".to_string());
        }
        let start_u = gpu_copy_units(x.half, start, "gpu_slice_cols")?;
        let len_u = gpu_copy_units(x.half, len, "gpu_slice_cols")?;
        let stride_u = gpu_copy_units(x.half, x.cols, "gpu_slice_cols")?;
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = if x.half {
                GpuTensor::from_pool_half(x.rows, len)?
            } else {
                GpuTensor::from_pool(x.rows, len)?
            };
            let status = unsafe {
                makepad_cuda_copy_submatrix_f32(
                    x.storage_ptr()?.add(start_u),
                    out.storage_ptr()?,
                    stride_u as u32,
                    len_u as u32,
                    x.rows as u32,
                    len_u as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_prof(backend.stream, crate::prof::CAT_ELEMENTWISE, prof_start, 0);
            Ok(out)
        })
    }

    pub fn gpu_concat_cols(parts: &[&GpuTensor]) -> Result<GpuTensor, String> {
        let rows = parts.first().map(|part| part.rows).unwrap_or(0);
        let half = parts.first().map(|part| part.half).unwrap_or(false);
        if rows == 0
            || parts
                .iter()
                .any(|part| part.rows != rows || part.half != half)
        {
            return Err("gpu_concat_cols shape mismatch".to_string());
        }
        let total_cols: usize = parts.iter().map(|part| part.cols).sum();
        let total_u = gpu_copy_units(half, total_cols, "gpu_concat_cols")?;
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = if half {
                GpuTensor::from_pool_half(rows, total_cols)?
            } else {
                GpuTensor::from_pool(rows, total_cols)?
            };
            let out_ptr = out.storage_ptr()?;
            let mut col_off = 0usize;
            for part in parts {
                let part_u = gpu_copy_units(half, part.cols, "gpu_concat_cols")?;
                let status = unsafe {
                    makepad_cuda_copy_submatrix_f32(
                        part.storage_ptr()?,
                        out_ptr.add(col_off),
                        part_u as u32,
                        total_u as u32,
                        rows as u32,
                        part_u as u32,
                        backend.stream,
                    )
                };
                gpu_check(status)?;
                col_off += part_u;
            }
            Ok(out)
        })
    }

    pub fn gpu_slice_rows(x: &GpuTensor, start: usize, len: usize) -> Result<GpuTensor, String> {
        if start + len > x.rows {
            return Err("gpu_slice_rows out of range".to_string());
        }
        let cols_u = gpu_copy_units(x.half, x.cols, "gpu_slice_rows")?;
        let buf = x
            .buf
            .clone()
            .ok_or_else(|| "GPU tensor already released".to_string())?;
        Ok(GpuTensor {
            buf: Some(buf),
            rows: len,
            cols: x.cols,
            half: x.half,
            offset_units: x.offset_units + start * cols_u,
        })
    }

    pub fn gpu_concat_rows(a: &GpuTensor, b: &GpuTensor) -> Result<GpuTensor, String> {
        if a.cols != b.cols || a.half != b.half {
            return Err("gpu_concat_rows shape mismatch".to_string());
        }
        let cols_u = gpu_copy_units(a.half, a.cols, "gpu_concat_rows")?;
        if a.same_storage(b) {
            if a.offset_units + a.rows * cols_u == b.offset_units {
                return Ok(GpuTensor {
                    buf: a.buf.clone(),
                    rows: a.rows + b.rows,
                    cols: a.cols,
                    half: a.half,
                    offset_units: a.offset_units,
                });
            }
            if b.offset_units + b.rows * cols_u == a.offset_units {
                return Ok(GpuTensor {
                    buf: b.buf.clone(),
                    rows: a.rows + b.rows,
                    cols: a.cols,
                    half: a.half,
                    offset_units: b.offset_units,
                });
            }
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = if a.half {
                GpuTensor::from_pool_half(a.rows + b.rows, a.cols)?
            } else {
                GpuTensor::from_pool(a.rows + b.rows, a.cols)?
            };
            let out_ptr = out.storage_ptr()?;
            for (part, row_off) in [(a, 0usize), (b, a.rows)] {
                let status = unsafe {
                    makepad_cuda_copy_submatrix_f32(
                        part.storage_ptr()?,
                        out_ptr.add(row_off * cols_u),
                        cols_u as u32,
                        cols_u as u32,
                        part.rows as u32,
                        cols_u as u32,
                        backend.stream,
                    )
                };
                gpu_check(status)?;
            }
            gpu_prof(backend.stream, crate::prof::CAT_ELEMENTWISE, prof_start, 0);
            Ok(out)
        })
    }

    /// Concat many row-blocks in one allocation. Adjacent views of the same
    /// storage collapse to a single view (no copy).
    pub fn gpu_concat_rows_many(parts: &[&GpuTensor]) -> Result<GpuTensor, String> {
        if parts.is_empty() {
            return Err("gpu_concat_rows_many empty".to_string());
        }
        if parts.len() == 1 {
            return gpu_slice_rows(parts[0], 0, parts[0].rows);
        }
        let cols = parts[0].cols;
        let half = parts[0].half;
        if parts.iter().any(|p| p.cols != cols || p.half != half) {
            return Err("gpu_concat_rows_many shape mismatch".to_string());
        }
        let cols_u = gpu_copy_units(half, cols, "gpu_concat_rows_many")?;
        let mut adjacent = true;
        let mut next_off = parts[0].offset_units;
        for part in parts {
            if !part.same_storage(parts[0]) || part.offset_units != next_off {
                adjacent = false;
                break;
            }
            next_off += part.rows * cols_u;
        }
        if adjacent {
            let rows: usize = parts.iter().map(|p| p.rows).sum();
            return Ok(GpuTensor {
                buf: parts[0].buf.clone(),
                rows,
                cols,
                half,
                offset_units: parts[0].offset_units,
            });
        }
        let rows: usize = parts.iter().map(|p| p.rows).sum();
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = if half {
                GpuTensor::from_pool_half(rows, cols)?
            } else {
                GpuTensor::from_pool(rows, cols)?
            };
            let out_ptr = out.storage_ptr()?;
            let mut row_off = 0usize;
            for part in parts {
                let status = unsafe {
                    makepad_cuda_copy_submatrix_f32(
                        part.storage_ptr()?,
                        out_ptr.add(row_off * cols_u),
                        cols_u as u32,
                        cols_u as u32,
                        part.rows as u32,
                        cols_u as u32,
                        backend.stream,
                    )
                };
                gpu_check(status)?;
                row_off += part.rows;
            }
            gpu_prof(
                backend.stream,
                crate::prof::CAT_ELEMENTWISE,
                prof_start,
                0,
            );
            Ok(out)
        })
    }

    /// Reorder beam-major prior cache rows by `parents`, then append one row
    /// from `step` to each output beam. Output layout remains beam-major with
    /// sequence length `prior_sequence + 1`.
    pub fn gpu_beam_cache_reorder_append(
        prior: &GpuTensor,
        step: &GpuTensor,
        parents: &[u32],
        prior_beams: usize,
        prior_sequence: usize,
    ) -> Result<GpuTensor, String> {
        if prior.half
            || step.half
            || parents.is_empty()
            || prior_beams == 0
            || prior.rows != prior_beams * prior_sequence
            || step.rows != parents.len()
            || step.cols != prior.cols
            || parents.iter().any(|&parent| parent as usize >= prior_beams)
        {
            return Err("gpu_beam_cache_reorder_append shape mismatch".to_string());
        }
        let parent_gpu = gpu_upload_u32(parents)?;
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(parents.len() * (prior_sequence + 1), prior.cols)?;
            let status = unsafe {
                makepad_cuda_beam_cache_reorder_append_f32(
                    prior.device_ptr()?,
                    step.device_ptr()?,
                    parent_gpu.device_ptr()?.cast::<u32>(),
                    out.device_ptr()?,
                    prior_sequence as u32,
                    parents.len() as u32,
                    prior.cols as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_prof(
                backend.stream,
                crate::prof::CAT_ELEMENTWISE,
                prof_start,
                0,
            );
            Ok(out)
        })
    }

    // ------------------------------------------------------------------
    // Device-resident planar ops (flux VAE decode). Planar tensors reuse
    // GpuTensor with rows = channels, cols = width*height ([c][y][x] flat).
    // ------------------------------------------------------------------

    pub fn gpu_silu(x: &GpuTensor) -> Result<GpuTensor, String> {
        if x.half {
            return gpu_to_f16(&gpu_silu(&gpu_to_f32(x)?)?);
        }
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(x.rows, x.cols)?;
            let status = unsafe {
                makepad_cuda_silu_f32_precise(
                    x.device_ptr()?,
                    out.device_ptr()?,
                    (x.rows * x.cols) as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            Ok(out)
        })
    }

    fn gpu_conv_gemm_enabled() -> bool {
        match std::env::var("FLUX_VAE_CONV_GEMM") {
            Ok(value) => value != "0",
            Err(_) => true,
        }
    }

    fn gpu_conv_im2col_enabled() -> bool {
        match std::env::var("FLUX_VAE_CONV_IM2COL") {
            Ok(value) => value != "0",
            Err(_) => true,
        }
    }

    /// Stride-1 "same" planar conv2d; weights are device-cached under
    /// `{cache_namespace}::{weight_cache_key}` so warm decodes upload nothing.
    ///
    /// Default path (FLUX_VAE_CONV_GEMM!=0, odd kernels with matching "same"
    /// padding): implicit GEMM — the input is zero-padded + converted to f16
    /// once, then kh*kw strided-batched cuBLAS gemms (batch = output rows,
    /// f16 inputs, f32 accumulate) accumulate the shifted contributions; the
    /// weight cache holds a per-shift (ic x oc) f16 repack. Fallback: the
    /// direct f32 kernel with an f32 weight cache.
    #[allow(clippy::too_many_arguments)]
    pub fn gpu_conv2d_planar_cached(
        x: &GpuTensor,
        width: usize,
        height: usize,
        cache_namespace: &str,
        weight_cache_key: &str,
        weights: &[f32],
        bias: &[f32],
        out_channels: usize,
        kw: usize,
        kh: usize,
        pad_x: usize,
        pad_y: usize,
    ) -> Result<GpuTensor, String> {
        let in_channels = x.rows;
        let plane = width
            .checked_mul(height)
            .ok_or_else(|| "gpu_conv2d plane overflow".to_string())?;
        if x.cols != plane {
            return Err(format!(
                "gpu_conv2d plane mismatch: cols={} width*height={plane}",
                x.cols
            ));
        }
        if weights.len() != out_channels * in_channels * kw * kh || bias.len() != out_channels {
            return Err("gpu_conv2d weight/bias shape mismatch".to_string());
        }
        if gpu_conv_gemm_enabled() && kw == 1 && kh == 1 && pad_x == 0 && pad_y == 0 {
            return gpu_conv2d_1x1_gemm(
                x,
                width,
                height,
                cache_namespace,
                weight_cache_key,
                weights,
                bias,
                out_channels,
            );
        }
        if gpu_conv_gemm_enabled() && kw == 2 * pad_x + 1 && kh == 2 * pad_y + 1 {
            if gpu_conv_im2col_enabled() {
                return gpu_conv2d_planar_im2col(
                    x,
                    width,
                    height,
                    cache_namespace,
                    weight_cache_key,
                    weights,
                    bias,
                    out_channels,
                    kw,
                    kh,
                    pad_x,
                    pad_y,
                );
            }
            return gpu_conv2d_planar_gemm(
                x,
                width,
                height,
                cache_namespace,
                weight_cache_key,
                weights,
                bias,
                out_channels,
                kw,
                kh,
                pad_x,
                pad_y,
            );
        }
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let weight_bytes = weights.len() * size_of::<f32>();
            let qualified_key = format!("{cache_namespace}::{weight_cache_key}");
            backend.cached_weight_buffer(&qualified_key, weight_bytes, || {
                let raw = unsafe {
                    std::slice::from_raw_parts(weights.as_ptr().cast::<u8>(), weight_bytes)
                };
                Ok(raw.to_vec())
            })?;
            let bias_buf = gpu_upload_small(backend, bias)?;
            let out = GpuTensor::from_pool(out_channels, plane)?;
            let weight = backend
                .weight_buffers
                .get(&qualified_key)
                .ok_or_else(|| format!("missing cached CUDA conv weight buffer {qualified_key}"))?;
            let status = unsafe {
                makepad_cuda_conv2d_planar_f32(
                    x.device_ptr()?,
                    weight.ptr.as_ptr().cast::<f32>(),
                    bias_buf.ptr.as_ptr().cast::<f32>(),
                    out.device_ptr()?,
                    width as u32,
                    height as u32,
                    in_channels as u32,
                    out_channels as u32,
                    kw as u32,
                    kh as u32,
                    pad_x as u32,
                    pad_y as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_pool_release(bias_buf);
            Ok(out)
        })
    }

    /// Planar `[C, N*H*W]` → packed NCHW `[N, C*H*W]`.
    pub fn gpu_planar_to_nchw(
        x: &GpuTensor,
        batch: usize,
        width: usize,
        height: usize,
    ) -> Result<GpuTensor, String> {
        gpu_transform_planar_nchw(x, x.rows, batch, width, height, true)
    }

    /// Packed NCHW `[N, C*H*W]` → planar `[C, N*H*W]`.
    pub fn gpu_nchw_to_planar(
        x: &GpuTensor,
        channels: usize,
        width: usize,
        height: usize,
    ) -> Result<GpuTensor, String> {
        gpu_transform_planar_nchw(x, channels, x.rows, width, height, false)
    }

    fn gpu_transform_planar_nchw(
        x: &GpuTensor,
        channels: usize,
        batch: usize,
        width: usize,
        height: usize,
        to_nchw: bool,
    ) -> Result<GpuTensor, String> {
        let plane = batch
            .checked_mul(width)
            .and_then(|v| v.checked_mul(height))
            .ok_or_else(|| "gpu_transform_planar_nchw overflow".to_string())?;
        if to_nchw {
            if x.rows != channels || x.cols != plane {
                return Err(format!(
                    "gpu_planar_to_nchw {}x{} vs C={channels} nHW={plane}",
                    x.rows, x.cols
                ));
            }
        } else if x.rows != batch || x.cols != channels * width * height {
            return Err(format!(
                "gpu_nchw_to_planar {}x{} vs n={batch} C*HW={}",
                x.rows,
                x.cols,
                channels * width * height
            ));
        }
        if !crate::cudnn::available() {
            return Err("cuDNN unavailable for planar↔NCHW transform".into());
        }
        let dtype = if x.half {
            crate::cudnn::CUDNN_DATA_HALF
        } else {
            crate::cudnn::CUDNN_DATA_FLOAT
        };
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = if to_nchw {
                if x.half {
                    GpuTensor::from_pool_half(batch, channels * width * height)?
                } else {
                    GpuTensor::from_pool(batch, channels * width * height)?
                }
            } else if x.half {
                GpuTensor::from_pool_half(channels, plane)?
            } else {
                GpuTensor::from_pool(channels, plane)?
            };
            crate::cudnn::transform_planar_nchw(
                batch as i32,
                channels as i32,
                height as i32,
                width as i32,
                x.storage_ptr()?.cast(),
                out.storage_ptr()?.cast(),
                to_nchw,
                dtype,
                backend.stream,
            )?;
            Ok(out)
        })
    }

    /// Packed NCHW GroupNorm. `x` is `[N, C*H*W]`.
    pub fn gpu_nchw_group_norm(
        x: &GpuTensor,
        channels: usize,
        width: usize,
        height: usize,
        groups: usize,
        cache_namespace: &str,
        cache_key: &str,
        gamma: &[f32],
        beta: &[f32],
        eps: f32,
    ) -> Result<GpuTensor, String> {
        let batch = x.rows;
        let hw = width * height;
        if x.half || x.cols != channels * hw || gamma.len() != channels || beta.len() != channels {
            return Err("gpu_nchw_group_norm shape mismatch".into());
        }
        if groups == 0 || channels % groups != 0 {
            return Err(format!("gpu_nchw_group_norm C={channels} groups={groups}"));
        }
        if !crate::cudnn::group_norm_available() {
            let planar = gpu_nchw_to_planar(x, channels, width, height)?;
            let gn = gpu_paint_group_norm_batched(
                &planar,
                width,
                height,
                batch,
                groups,
                cache_namespace,
                cache_key,
                gamma,
                beta,
                eps,
            )?;
            return gpu_planar_to_nchw(&gn, batch, width, height);
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let g_key = format!("{cache_namespace}::{cache_key}::gn_w");
            let b_key = format!("{cache_namespace}::{cache_key}::gn_b");
            backend.cached_weight_buffer(&g_key, gamma.len() * size_of::<f32>(), || {
                let raw = unsafe {
                    std::slice::from_raw_parts(
                        gamma.as_ptr().cast::<u8>(),
                        gamma.len() * size_of::<f32>(),
                    )
                };
                Ok(raw.to_vec())
            })?;
            backend.cached_weight_buffer(&b_key, beta.len() * size_of::<f32>(), || {
                let raw = unsafe {
                    std::slice::from_raw_parts(
                        beta.as_ptr().cast::<u8>(),
                        beta.len() * size_of::<f32>(),
                    )
                };
                Ok(raw.to_vec())
            })?;
            let g = backend
                .weight_buffers
                .get(&g_key)
                .ok_or_else(|| format!("missing GN gamma {g_key}"))?;
            let b = backend
                .weight_buffers
                .get(&b_key)
                .ok_or_else(|| format!("missing GN beta {b_key}"))?;
            let out = GpuTensor::from_pool(batch, channels * hw)?;
            crate::cudnn::group_norm_nchw_f32(
                batch as i32,
                channels as i32,
                height as i32,
                width as i32,
                groups as i32,
                eps,
                x.device_ptr()?.cast(),
                g.ptr.as_ptr(),
                b.ptr.as_ptr(),
                out.device_ptr()?.cast(),
                backend.stream,
            )?;
            gpu_prof(
                backend.stream,
                crate::prof::CAT_GROUP_NORM,
                prof_start,
                0,
            );
            Ok(out)
        })
    }

    /// Packed NCHW `[N, C*H*W]` → tokens `[N*H*W, C]`.
    pub fn gpu_nchw_to_tokens(
        x: &GpuTensor,
        channels: usize,
        width: usize,
        height: usize,
    ) -> Result<GpuTensor, String> {
        let batch = x.rows;
        let hw = width * height;
        if x.cols != channels * hw {
            return Err("gpu_nchw_to_tokens shape mismatch".into());
        }
        let dtype = if x.half {
            crate::cudnn::CUDNN_DATA_HALF
        } else {
            crate::cudnn::CUDNN_DATA_FLOAT
        };
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = if x.half {
                GpuTensor::from_pool_half(batch * hw, channels)?
            } else {
                GpuTensor::from_pool(batch * hw, channels)?
            };
            crate::cudnn::transform_nchw_nhwc(
                batch as i32,
                channels as i32,
                height as i32,
                width as i32,
                x.storage_ptr()?.cast(),
                out.storage_ptr()?.cast(),
                true,
                dtype,
                backend.stream,
            )?;
            Ok(out)
        })
    }

    /// Tokens `[N*H*W, C]` → packed NCHW `[N, C*H*W]`.
    pub fn gpu_tokens_to_nchw(
        x: &GpuTensor,
        batch: usize,
        width: usize,
        height: usize,
    ) -> Result<GpuTensor, String> {
        let channels = x.cols;
        let hw = width * height;
        if x.rows != batch * hw {
            return Err("gpu_tokens_to_nchw shape mismatch".into());
        }
        let dtype = if x.half {
            crate::cudnn::CUDNN_DATA_HALF
        } else {
            crate::cudnn::CUDNN_DATA_FLOAT
        };
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = if x.half {
                GpuTensor::from_pool_half(batch, channels * hw)?
            } else {
                GpuTensor::from_pool(batch, channels * hw)?
            };
            crate::cudnn::transform_nchw_nhwc(
                batch as i32,
                channels as i32,
                height as i32,
                width as i32,
                x.storage_ptr()?.cast(),
                out.storage_ptr()?.cast(),
                false,
                dtype,
                backend.stream,
            )?;
            Ok(out)
        })
    }

    /// `y[n,h,w,c] += bias[c]` on packed NHWC `[N*H*W, C]`.
    pub fn gpu_nhwc_add_channel(
        x: &GpuTensor,
        bias: &GpuTensor,
        batch: usize,
        channels: usize,
        width: usize,
        height: usize,
    ) -> Result<GpuTensor, String> {
        let hw = width * height;
        let b_len = bias.rows * bias.cols;
        if x.rows != batch * hw || x.cols != channels || b_len != channels {
            return Err("gpu_nhwc_add_channel shape mismatch".into());
        }
        if !x.half {
            return Err("gpu_nhwc_add_channel expects f16".into());
        }
        let bias16;
        let bias_use = if bias.half {
            bias
        } else {
            bias16 = gpu_to_f16(bias)?;
            &bias16
        };
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool_half(x.rows, x.cols)?;
            let bytes = x.rows * x.cols * size_of::<u16>();
            crate::check(unsafe {
                crate::cudaMemcpyAsync(
                    out.storage_ptr()?.cast(),
                    x.storage_ptr()?.cast_const().cast(),
                    bytes,
                    3,
                    backend.stream,
                )
            })
            .map_err(|err| err.to_string())?;
            crate::cudnn::add_bias_nhwc_f16(
                bias_use.storage_ptr()?.cast(),
                out.storage_ptr()?.cast(),
                batch as i32,
                channels as i32,
                height as i32,
                width as i32,
                backend.stream,
            )?;
            Ok(out)
        })
    }

    /// `y[n,c,h,w] += bias[c]` on packed NCHW `[N, C*H*W]`.
    pub fn gpu_nchw_add_channel(
        x: &GpuTensor,
        bias: &GpuTensor,
        channels: usize,
        width: usize,
        height: usize,
    ) -> Result<GpuTensor, String> {
        let batch = x.rows;
        let hw = width * height;
        let b_len = bias.rows * bias.cols;
        if x.cols != channels * hw || b_len != channels {
            return Err("gpu_nchw_add_channel shape mismatch".into());
        }
        let bias16;
        let bias_use = if x.half && !bias.half {
            bias16 = gpu_to_f16(bias)?;
            &bias16
        } else {
            bias
        };
        if x.half != bias_use.half {
            return Err("gpu_nchw_add_channel dtype mismatch".into());
        }
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = if x.half {
                GpuTensor::from_pool_half(batch, channels * hw)?
            } else {
                GpuTensor::from_pool(batch, channels * hw)?
            };
            let elem = if x.half { size_of::<u16>() } else { size_of::<f32>() };
            let bytes = batch * channels * hw * elem;
            crate::check(unsafe {
                crate::cudaMemcpyAsync(
                    out.storage_ptr()?.cast(),
                    x.storage_ptr()?.cast_const().cast(),
                    bytes,
                    3,
                    backend.stream,
                )
            })
            .map_err(|err| err.to_string())?;
            if x.half {
                crate::cudnn::add_bias_nchw_packed_f16(
                    bias_use.storage_ptr()?.cast(),
                    out.storage_ptr()?.cast(),
                    batch as i32,
                    channels as i32,
                    height as i32,
                    width as i32,
                    backend.stream,
                )?;
            } else {
                crate::cudnn::add_bias_nchw_f32(
                    bias_use.device_ptr()?.cast(),
                    out.device_ptr()?.cast(),
                    batch as i32,
                    channels as i32,
                    height as i32,
                    width as i32,
                    backend.stream,
                )?;
            }
            Ok(out)
        })
    }

    /// In-place `x[n,c,h,w] += bias[c]`. Caller must own `x` (fresh conv out).
    pub fn gpu_nchw_add_channel_inplace(
        x: &GpuTensor,
        bias: &GpuTensor,
        channels: usize,
        width: usize,
        height: usize,
    ) -> Result<GpuTensor, String> {
        let batch = x.rows;
        let hw = width * height;
        let b_len = bias.rows * bias.cols;
        if x.cols != channels * hw || b_len != channels {
            return Err("gpu_nchw_add_channel_inplace shape mismatch".into());
        }
        if x.half || bias.half {
            return Err("gpu_nchw_add_channel_inplace is f32-only".into());
        }
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            crate::cudnn::add_bias_nchw_f32(
                bias.device_ptr()?.cast(),
                x.device_ptr()?.cast(),
                batch as i32,
                channels as i32,
                height as i32,
                width as i32,
                backend.stream,
            )?;
            gpu_slice_rows(x, 0, x.rows)
        })
    }

    /// Packed NCHW `[N, Cin*H*W]` same-pad/strided conv → `[N, Cout*oH*oW]`.
    #[allow(clippy::too_many_arguments)]
    pub fn gpu_conv2d_nchw_packed(
        x: &GpuTensor,
        channels: usize,
        width: usize,
        height: usize,
        out_width: usize,
        out_height: usize,
        cache_namespace: &str,
        weight_cache_key: &str,
        weights: &[f32],
        bias: &[f32],
        out_channels: usize,
        kw: usize,
        kh: usize,
        pad_x: usize,
        pad_y: usize,
        stride_x: usize,
        stride_y: usize,
    ) -> Result<GpuTensor, String> {
        let batch = x.rows;
        let hw = width * height;
        if x.cols != channels * hw {
            return Err(format!(
                "gpu_conv2d_nchw_packed {}x{} vs n C*HW={channels}*{hw}",
                x.rows, x.cols
            ));
        }
        if weights.len() != out_channels * channels * kw * kh || bias.len() != out_channels {
            return Err("gpu_conv2d_nchw_packed weight/bias mismatch".into());
        }
        if x.half
            && kw == 1
            && kh == 1
            && pad_x == 0
            && pad_y == 0
            && stride_x == 1
            && stride_y == 1
        {
            return gpu_conv2d_1x1_nchw_packed_f16(
                x,
                channels,
                width,
                height,
                cache_namespace,
                weight_cache_key,
                weights,
                bias,
                out_channels,
            );
        }
        if !crate::cudnn::available() {
            let planar = gpu_nchw_to_planar(x, channels, width, height)?;
            let y = gpu_conv2d_nchw_ex(
                &planar,
                batch,
                width,
                height,
                out_width,
                out_height,
                cache_namespace,
                weight_cache_key,
                weights,
                bias,
                out_channels,
                kw,
                kh,
                pad_x,
                pad_y,
                stride_x,
                stride_y,
            )?;
            return gpu_planar_to_nchw(&y, batch, out_width, out_height);
        }
        let dtype = if x.half {
            crate::cudnn::CUDNN_DATA_HALF
        } else {
            crate::cudnn::CUDNN_DATA_FLOAT
        };
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let key = (
                batch as u32,
                channels as u32,
                out_channels as u32,
                height as u32,
                width as u32,
                out_height as u32,
                kh as u32,
                pad_x as u32,
                stride_x as u32,
                0x20u32,
                dtype as u32,
            );
            CUDNN_DESCS.with(|cell: &RefCell<CudnnDescMap>| {
                let mut map = cell.borrow_mut();
                if !map.contains_key(&key) {
                    let desc = crate::cudnn::prepare_nchw_contiguous(
                        batch as i32,
                        channels as i32,
                        out_channels as i32,
                        height as i32,
                        width as i32,
                        out_height as i32,
                        out_width as i32,
                        kh as i32,
                        kw as i32,
                        pad_y as i32,
                        pad_x as i32,
                        stride_y as i32,
                        stride_x as i32,
                        dtype,
                        backend.stream,
                    )?;
                    static PACKED_LOG: std::sync::Once = std::sync::Once::new();
                    PACKED_LOG.call_once(|| {
                        eprintln!(
                            "PBR_CUDNN_ON nchw_packed_{} algo={} mathType={}",
                            if x.half { "f16" } else { "f32" },
                            crate::cudnn::algo_id(&desc),
                            crate::cudnn::math_type(&desc)
                        );
                    });
                    map.insert(key, desc);
                }
                let desc = map.get(&key).expect("just inserted");
                let ws_bytes = crate::cudnn::workspace_bytes(desc);
                let ws_ptr = if ws_bytes == 0 {
                    std::ptr::null_mut()
                } else {
                    CUDNN_WS.with(|ws| -> Result<*mut std::ffi::c_void, String> {
                        let mut slot = ws.borrow_mut();
                        let need_new = match slot.as_ref() {
                            Some(buf) => buf.size_bytes < ws_bytes,
                            None => true,
                        };
                        if need_new {
                            *slot = Some(DeviceBuffer::new(gpu_pool_round(ws_bytes.max(1)))?);
                        }
                        Ok(slot.as_ref().unwrap().ptr.as_ptr())
                    })?
                };
                let w_key = if x.half {
                    format!("{cache_namespace}::{weight_cache_key}::nchw16")
                } else {
                    format!("{cache_namespace}::{weight_cache_key}")
                };
                let bias_key = if x.half {
                    format!("{cache_namespace}::{weight_cache_key}::bias16")
                } else {
                    format!("{cache_namespace}::{weight_cache_key}::bias")
                };
                if x.half {
                    backend.cached_weight_buffer(
                        &w_key,
                        weights.len() * size_of::<u16>(),
                        || Ok(pack_f16_bytes(weights)),
                    )?;
                    backend.cached_weight_buffer(
                        &bias_key,
                        bias.len() * size_of::<u16>(),
                        || Ok(pack_f16_bytes(bias)),
                    )?;
                } else {
                    backend.cached_weight_buffer(
                        &w_key,
                        weights.len() * size_of::<f32>(),
                        || {
                            let raw = unsafe {
                                std::slice::from_raw_parts(
                                    weights.as_ptr().cast::<u8>(),
                                    weights.len() * size_of::<f32>(),
                                )
                            };
                            Ok(raw.to_vec())
                        },
                    )?;
                    backend.cached_weight_buffer(
                        &bias_key,
                        bias.len() * size_of::<f32>(),
                        || {
                            let raw = unsafe {
                                std::slice::from_raw_parts(
                                    bias.as_ptr().cast::<u8>(),
                                    bias.len() * size_of::<f32>(),
                                )
                            };
                            Ok(raw.to_vec())
                        },
                    )?;
                }
                let weight_ptr = backend
                    .weight_buffers
                    .get(&w_key)
                    .ok_or_else(|| format!("missing packed conv {w_key}"))?
                    .ptr
                    .as_ptr();
                let bias_ptr = backend
                    .weight_buffers
                    .get(&bias_key)
                    .ok_or_else(|| format!("missing packed bias {bias_key}"))?
                    .ptr
                    .as_ptr();
                let out = if x.half {
                    GpuTensor::from_pool_half(batch, out_channels * out_width * out_height)?
                } else {
                    GpuTensor::from_pool(batch, out_channels * out_width * out_height)?
                };
                crate::cudnn::convolution_forward_f16(
                    desc,
                    x.storage_ptr()?.cast(),
                    weight_ptr,
                    out.storage_ptr()?.cast(),
                    ws_ptr,
                    backend.stream,
                )?;
                crate::cudnn::add_bias_nchw_from_desc(
                    desc,
                    bias_ptr,
                    if x.half {
                        out.storage_ptr()?.cast()
                    } else {
                        out.device_ptr()?.cast()
                    },
                )?;
                gpu_prof(backend.stream, crate::prof::CAT_CONV2D, prof_start, 0);
                Ok(out)
            })
        })
    }

    fn pack_f16_bytes(values: &[f32]) -> Vec<u8> {
        let mut packed = vec![0u16; values.len()];
        for (dst, src) in packed.iter_mut().zip(values) {
            *dst = crate::quant::f32_to_f16(*src);
        }
        unsafe {
            std::slice::from_raw_parts(packed.as_ptr().cast::<u8>(), packed.len() * size_of::<u16>())
                .to_vec()
        }
    }

    /// Round-to-nearest-even f16 packing — bit-matches torch `.half()`
    /// weight casts.  The RealESRGAN parity lock is calibrated against it;
    /// the truncating [`pack_f16_bytes`] stays for consumers whose dumps
    /// were recorded with it.
    fn pack_f16_bytes_rn(values: &[f32]) -> Vec<u8> {
        let mut packed = vec![0u16; values.len()];
        for (dst, src) in packed.iter_mut().zip(values) {
            *dst = crate::quant::f32_to_f16_rn(*src);
        }
        unsafe {
            std::slice::from_raw_parts(packed.as_ptr().cast::<u8>(), packed.len() * size_of::<u16>())
                .to_vec()
        }
    }

    fn gpu_conv2d_1x1_nchw_packed_f16(
        x: &GpuTensor,
        channels: usize,
        width: usize,
        height: usize,
        cache_namespace: &str,
        weight_cache_key: &str,
        weights: &[f32],
        bias: &[f32],
        out_channels: usize,
    ) -> Result<GpuTensor, String> {
        if !x.half {
            return Err("gpu_conv2d_1x1_nchw_packed_f16 expects f16".into());
        }
        let batch = x.rows;
        let hw = width * height;
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let w_key = format!("{cache_namespace}::{weight_cache_key}::nchw16");
            backend.cached_weight_buffer(&w_key, weights.len() * size_of::<u16>(), || {
                Ok(pack_f16_bytes(weights))
            })?;
            let weight = backend
                .weight_buffers
                .get(&w_key)
                .ok_or_else(|| format!("missing packed 1x1 {w_key}"))?;
            let out = GpuTensor::from_pool_half(batch, out_channels * hw)?;
            let alpha = 1.0f32;
            let beta = 0.0f32;
            // Per-n: Y[oc, hw] = W[oc, ic] @ X[ic, hw].
            unsafe {
                crate::cublas_gemm_strided_batched_ex(
                    backend.blas,
                    crate::CUBLAS_OP_N,
                    crate::CUBLAS_OP_N,
                    hw as i32,
                    out_channels as i32,
                    channels as i32,
                    &alpha,
                    x.device_ptr_u16()?.cast::<std::ffi::c_void>(),
                    crate::CUDA_R_16F,
                    hw as i32,
                    (channels * hw) as i64,
                    weight.ptr.as_ptr().cast::<std::ffi::c_void>(),
                    crate::CUDA_R_16F,
                    channels as i32,
                    0,
                    &beta,
                    out.device_ptr_u16()?.cast::<std::ffi::c_void>(),
                    crate::CUDA_R_16F,
                    hw as i32,
                    (out_channels * hw) as i64,
                    batch as i32,
                    crate::CUBLAS_COMPUTE_32F,
                    crate::CUBLAS_GEMM_DEFAULT,
                )
                .map_err(|err| format!("packed nchw 1x1 gemm: {err}"))?;
            }
            let bias_key = format!("{cache_namespace}::{weight_cache_key}::bias16");
            backend.cached_weight_buffer(&bias_key, bias.len() * size_of::<u16>(), || {
                Ok(pack_f16_bytes(bias))
            })?;
            let bias_buf = backend
                .weight_buffers
                .get(&bias_key)
                .ok_or_else(|| format!("missing packed 1x1 bias {bias_key}"))?;
            crate::cudnn::add_bias_nchw_packed_f16(
                bias_buf.ptr.as_ptr(),
                out.storage_ptr()?.cast(),
                batch as i32,
                out_channels as i32,
                height as i32,
                width as i32,
                backend.stream,
            )?;
            gpu_prof(backend.stream, crate::prof::CAT_CONV2D, prof_start, 0);
            Ok(out)
        })
    }

    /// Contiguous NHWC conv. `x` is `[N*H*W, C]` (official packed layout).
    #[allow(clippy::too_many_arguments)]
    pub fn gpu_conv2d_nhwc_cached(
        x: &GpuTensor,
        batch: usize,
        width: usize,
        height: usize,
        cache_namespace: &str,
        weight_cache_key: &str,
        weights: &[f32],
        bias: &[f32],
        out_channels: usize,
        kw: usize,
        kh: usize,
        pad_x: usize,
        pad_y: usize,
    ) -> Result<GpuTensor, String> {
        gpu_conv2d_nhwc_ex(
            x,
            batch,
            width,
            height,
            width,
            height,
            cache_namespace,
            weight_cache_key,
            weights,
            bias,
            out_channels,
            kw,
            kh,
            pad_x,
            pad_y,
            1,
            1,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn gpu_conv2d_nhwc_ex(
        x: &GpuTensor,
        batch: usize,
        width: usize,
        height: usize,
        out_width: usize,
        out_height: usize,
        cache_namespace: &str,
        weight_cache_key: &str,
        weights: &[f32],
        bias: &[f32],
        out_channels: usize,
        kw: usize,
        kh: usize,
        pad_x: usize,
        pad_y: usize,
        stride_x: usize,
        stride_y: usize,
    ) -> Result<GpuTensor, String> {
        let in_plane = batch * width * height;
        if x.rows != in_plane {
            return Err(format!(
                "gpu_conv2d_nhwc rows {} vs n*H*W {in_plane}",
                x.rows
            ));
        }
        let in_channels = x.cols;
        if weights.len() != out_channels * in_channels * kw * kh || bias.len() != out_channels {
            return Err("gpu_conv2d_nhwc weight/bias shape mismatch".to_string());
        }
        if !x.half || !crate::cudnn::available() {
            return Err("gpu_conv2d_nhwc wants f16 + cuDNN".to_string());
        }
        if kw == 1 && kh == 1 && pad_x == 0 && pad_y == 0 && stride_x == 1 && stride_y == 1 {
            return gpu_conv2d_1x1_nhwc_f16(
                x,
                cache_namespace,
                weight_cache_key,
                weights,
                bias,
                out_channels,
            );
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let key = (
                batch as u32,
                in_channels as u32,
                out_channels as u32,
                height as u32,
                width as u32,
                out_height as u32,
                kh as u32,
                pad_x as u32,
                stride_x as u32,
                0x10u32,
                crate::cudnn::CUDNN_DATA_HALF as u32,
            );
            CUDNN_DESCS.with(|cell: &RefCell<CudnnDescMap>| {
                let mut map = cell.borrow_mut();
                if !map.contains_key(&key) {
                    let desc = crate::cudnn::prepare_nhwc_ex(
                        batch as i32,
                        in_channels as i32,
                        out_channels as i32,
                        height as i32,
                        width as i32,
                        out_height as i32,
                        out_width as i32,
                        kh as i32,
                        kw as i32,
                        pad_y as i32,
                        pad_x as i32,
                        stride_y as i32,
                        stride_x as i32,
                        backend.stream,
                    )?;
                    static NHWC_LOG: std::sync::Once = std::sync::Once::new();
                    NHWC_LOG.call_once(|| {
                        eprintln!(
                            "PBR_CUDNN_ON nhwc_packed_f16 algo={} mathType={}",
                            crate::cudnn::algo_id(&desc),
                            crate::cudnn::math_type(&desc)
                        );
                    });
                    map.insert(key, desc);
                }
                let desc = map.get(&key).expect("just inserted");
                let ws_bytes = crate::cudnn::workspace_bytes(desc);
                let ws_ptr = if ws_bytes == 0 {
                    std::ptr::null_mut()
                } else {
                    CUDNN_WS.with(|ws| -> Result<*mut std::ffi::c_void, String> {
                        let mut slot = ws.borrow_mut();
                        let need_new = match slot.as_ref() {
                            Some(buf) => buf.size_bytes < ws_bytes,
                            None => true,
                        };
                        if need_new {
                            *slot = Some(DeviceBuffer::new(gpu_pool_round(ws_bytes.max(1)))?);
                        }
                        Ok(slot.as_ref().unwrap().ptr.as_ptr())
                    })?
                };
                let w_key = format!("{cache_namespace}::{weight_cache_key}::nhwc16");
                backend.cached_weight_buffer(&w_key, weights.len() * size_of::<u16>(), || {
                    let packed = pack_filter_nhwc_f16(weights, out_channels, in_channels, kh, kw);
                    let raw = unsafe {
                        std::slice::from_raw_parts(
                            packed.as_ptr().cast::<u8>(),
                            packed.len() * size_of::<u16>(),
                        )
                    };
                    Ok(raw.to_vec())
                })?;
                let weight = backend
                    .weight_buffers
                    .get(&w_key)
                    .ok_or_else(|| format!("missing NHWC filter {w_key}"))?;
                let out_plane = batch * out_width * out_height;
                let out = GpuTensor::from_pool_half(out_plane, out_channels)?;
                crate::cudnn::convolution_forward_f16(
                    desc,
                    x.device_ptr_u16()?.cast(),
                    weight.ptr.as_ptr(),
                    out.device_ptr_u16()?.cast(),
                    ws_ptr,
                    backend.stream,
                )?;
                let bias_key = format!("{cache_namespace}::{weight_cache_key}::bias16");
                backend.cached_weight_buffer(&bias_key, bias.len() * size_of::<u16>(), || {
                    let mut packed = vec![0u16; bias.len()];
                    for (dst, src) in packed.iter_mut().zip(bias) {
                        *dst = crate::quant::f32_to_f16(*src);
                    }
                    let raw = unsafe {
                        std::slice::from_raw_parts(
                            packed.as_ptr().cast::<u8>(),
                            packed.len() * size_of::<u16>(),
                        )
                    };
                    Ok(raw.to_vec())
                })?;
                let bias_buf = backend
                    .weight_buffers
                    .get(&bias_key)
                    .ok_or_else(|| format!("missing NHWC bias {bias_key}"))?;
                crate::cudnn::add_bias_nhwc_f16(
                    bias_buf.ptr.as_ptr(),
                    out.device_ptr_u16()?.cast(),
                    batch as i32,
                    out_channels as i32,
                    out_height as i32,
                    out_width as i32,
                    backend.stream,
                )?;
                gpu_prof(backend.stream, crate::prof::CAT_CONV2D, prof_start, 0);
                Ok(out)
            })
        })
    }

    fn gpu_conv2d_1x1_nhwc_f16(
        x: &GpuTensor,
        cache_namespace: &str,
        weight_cache_key: &str,
        weights: &[f32],
        bias: &[f32],
        out_channels: usize,
    ) -> Result<GpuTensor, String> {
        let rows = x.rows;
        let in_channels = x.cols;
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            // W NHWC 1x1 is [oc, ic] row-major = same as [oc, ic] planar.
            let w_key = format!("{cache_namespace}::{weight_cache_key}::imcf16");
            backend.cached_weight_buffer(&w_key, weights.len() * size_of::<u16>(), || {
                let mut packed = vec![0u16; weights.len()];
                for (dst, src) in packed.iter_mut().zip(weights) {
                    *dst = crate::quant::f32_to_f16(*src);
                }
                let raw = unsafe {
                    std::slice::from_raw_parts(
                        packed.as_ptr().cast::<u8>(),
                        packed.len() * size_of::<u16>(),
                    )
                };
                Ok(raw.to_vec())
            })?;
            let weight = backend
                .weight_buffers
                .get(&w_key)
                .ok_or_else(|| format!("missing 1x1 nhwc {w_key}"))?;
            let out = GpuTensor::from_pool_half(rows, out_channels)?;
            let alpha = 1.0f32;
            let beta = 0.0f32;
            // C[rows, oc] = X[rows, ic] @ W[ic, oc]
            // W stored [oc, ic] row-major = [ic, oc] col-major. X row-major = col-major [ic, rows].
            // col-major: C(oc x rows) = W^T (oc x ic) @ X^T (ic x rows) 
            // Use OP_T on W (ldb=ic) and OP_N on X^T... 
            // Row-major Y = X @ W^T : col-major Y^T = W @ X^T
            // W [oc, ic] row-major = [ic, oc] col-major. 
            // cublas: C = A @ B with A = X^T (ic x rows col = rows x ic row), ...
            unsafe {
                crate::cublas_gemm_strided_batched_ex(
                    backend.blas,
                    crate::CUBLAS_OP_T,
                    crate::CUBLAS_OP_N,
                    out_channels as i32,
                    rows as i32,
                    in_channels as i32,
                    &alpha,
                    weight.ptr.as_ptr().cast::<std::ffi::c_void>(),
                    crate::CUDA_R_16F,
                    in_channels as i32,
                    0,
                    x.device_ptr_u16()?.cast::<std::ffi::c_void>(),
                    crate::CUDA_R_16F,
                    in_channels as i32,
                    0,
                    &beta,
                    out.device_ptr_u16()?.cast::<std::ffi::c_void>(),
                    crate::CUDA_R_16F,
                    out_channels as i32,
                    0,
                    1,
                    crate::CUBLAS_COMPUTE_32F,
                    crate::CUBLAS_GEMM_DEFAULT,
                )
                .map_err(|err| format!("nhwc 1x1 gemm: {err}"))?;
            }
            let bias_key = format!("{cache_namespace}::{weight_cache_key}::bias16");
            backend.cached_weight_buffer(&bias_key, bias.len() * size_of::<u16>(), || {
                let mut packed = vec![0u16; bias.len()];
                for (dst, src) in packed.iter_mut().zip(bias) {
                    *dst = crate::quant::f32_to_f16(*src);
                }
                let raw = unsafe {
                    std::slice::from_raw_parts(
                        packed.as_ptr().cast::<u8>(),
                        packed.len() * size_of::<u16>(),
                    )
                };
                Ok(raw.to_vec())
            })?;
            let bias_buf = backend
                .weight_buffers
                .get(&bias_key)
                .ok_or_else(|| format!("missing 1x1 nhwc bias {bias_key}"))?;
            crate::cudnn::add_bias_nhwc_f16(
                bias_buf.ptr.as_ptr(),
                out.device_ptr_u16()?.cast(),
                1,
                out_channels as i32,
                1,
                rows as i32,
                backend.stream,
            )?;
            gpu_prof(backend.stream, crate::prof::CAT_CONV2D, prof_start, 0);
            Ok(out)
        })
    }

    /// Same-pad conv on official NCHW batch `[N,C,H,W]` stored as planar
    /// `[C, N*H*W]`. 3x3 uses cuDNN fp16 NHWC (one `cudnnConvolutionForward`);
    /// 1x1 stays a GEMM. Falls back to the tall-image im2col path if cuDNN
    /// is missing.
    #[allow(clippy::too_many_arguments)]
    pub fn gpu_conv2d_nchw_cached(
        x: &GpuTensor,
        batch: usize,
        width: usize,
        height: usize,
        cache_namespace: &str,
        weight_cache_key: &str,
        weights: &[f32],
        bias: &[f32],
        out_channels: usize,
        kw: usize,
        kh: usize,
        pad_x: usize,
        pad_y: usize,
    ) -> Result<GpuTensor, String> {
        gpu_conv2d_nchw_ex(
            x,
            batch,
            width,
            height,
            width,
            height,
            cache_namespace,
            weight_cache_key,
            weights,
            bias,
            out_channels,
            kw,
            kh,
            pad_x,
            pad_y,
            1,
            1,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn gpu_conv2d_nchw_ex(
        x: &GpuTensor,
        batch: usize,
        width: usize,
        height: usize,
        out_width: usize,
        out_height: usize,
        cache_namespace: &str,
        weight_cache_key: &str,
        weights: &[f32],
        bias: &[f32],
        out_channels: usize,
        kw: usize,
        kh: usize,
        pad_x: usize,
        pad_y: usize,
        stride_x: usize,
        stride_y: usize,
    ) -> Result<GpuTensor, String> {
        if batch == 0 || stride_x == 0 || stride_y == 0 {
            return Err("gpu_conv2d_nchw_ex bad batch/stride".to_string());
        }
        let in_plane = width
            .checked_mul(height)
            .and_then(|p| p.checked_mul(batch))
            .ok_or_else(|| "gpu_conv2d_nchw plane overflow".to_string())?;
        if x.cols != in_plane {
            return Err(format!(
                "gpu_conv2d_nchw plane mismatch: cols={} n*H*W={in_plane}",
                x.cols
            ));
        }
        if kw == 1 && kh == 1 && pad_x == 0 && pad_y == 0 && stride_x == 1 && stride_y == 1 {
            return gpu_conv2d_1x1_maybe_half(
                x,
                width,
                batch * height,
                cache_namespace,
                weight_cache_key,
                weights,
                bias,
                out_channels,
            );
        }
        if batch > 1 && gpu_conv_gemm_enabled() && crate::cudnn::available() {
            static CUDNN_LOG: std::sync::Once = std::sync::Once::new();
            CUDNN_LOG.call_once(|| {
                eprintln!(
                    "PBR_CUDNN_ON nchw_resident_{}",
                    if x.half { "f16" } else { "f32" }
                );
            });
            match gpu_conv2d_nchw_cudnn(
                x,
                batch,
                width,
                height,
                out_width,
                out_height,
                cache_namespace,
                weight_cache_key,
                weights,
                bias,
                out_channels,
                kw,
                kh,
                pad_x,
                pad_y,
                stride_x,
                stride_y,
            ) {
                Ok(out) => return Ok(out),
                Err(err) => {
                    eprintln!("PBR_CUDNN_FALLBACK {weight_cache_key}: {err}");
                }
            }
        }
        if x.half {
            let x32 = gpu_to_f32(x)?;
            let y32 = gpu_conv2d_planar_cached(
                &x32,
                width,
                batch * height,
                cache_namespace,
                weight_cache_key,
                weights,
                bias,
                out_channels,
                kw,
                kh,
                pad_x,
                pad_y,
            )?;
            return gpu_to_f16(&y32);
        }
        gpu_conv2d_planar_cached(
            x,
            width,
            batch * height,
            cache_namespace,
            weight_cache_key,
            weights,
            bias,
            out_channels,
            kw,
            kh,
            pad_x,
            pad_y,
        )
    }

    fn gpu_conv2d_1x1_maybe_half(
        x: &GpuTensor,
        width: usize,
        height: usize,
        cache_namespace: &str,
        weight_cache_key: &str,
        weights: &[f32],
        bias: &[f32],
        out_channels: usize,
    ) -> Result<GpuTensor, String> {
        if x.half {
            gpu_conv2d_1x1_gemm_f16(
                x,
                width,
                height,
                cache_namespace,
                weight_cache_key,
                weights,
                bias,
                out_channels,
            )
        } else {
            gpu_conv2d_1x1_gemm(
                x,
                width,
                height,
                cache_namespace,
                weight_cache_key,
                weights,
                bias,
                out_channels,
            )
        }
    }

    type CudnnDescKey = (u32, u32, u32, u32, u32, u32, u32, u32, u32, u32, u32);
    type CudnnDescMap = HashMap<CudnnDescKey, crate::cudnn::ConvDesc>;
    thread_local! {
        static CUDNN_DESCS: RefCell<CudnnDescMap> = RefCell::new(HashMap::new());
        static CUDNN_WS: RefCell<Option<DeviceBuffer>> = const { RefCell::new(None) };
    }

    #[allow(dead_code)]
    fn pack_filter_nhwc_f16(
        weights: &[f32],
        oc: usize,
        ic: usize,
        kh: usize,
        kw: usize,
    ) -> Vec<u16> {
        let mut out = vec![0u16; oc * ic * kh * kw];
        for o in 0..oc {
            for i in 0..ic {
                for y in 0..kh {
                    for x in 0..kw {
                        let src = ((o * ic + i) * kh + y) * kw + x;
                        let dst = ((o * kh + y) * kw + x) * ic + i;
                        out[dst] = crate::quant::f32_to_f16(weights[src]);
                    }
                }
            }
        }
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn gpu_conv2d_nchw_cudnn(
        x: &GpuTensor,
        batch: usize,
        width: usize,
        height: usize,
        out_width: usize,
        out_height: usize,
        cache_namespace: &str,
        weight_cache_key: &str,
        weights: &[f32],
        bias: &[f32],
        out_channels: usize,
        kw: usize,
        kh: usize,
        pad_x: usize,
        pad_y: usize,
        stride_x: usize,
        stride_y: usize,
    ) -> Result<GpuTensor, String> {
        let in_channels = x.rows;
        let out_plane = batch * out_width * out_height;
        let half = x.half;
        let dtype = if half {
            crate::cudnn::CUDNN_DATA_HALF
        } else {
            crate::cudnn::CUDNN_DATA_FLOAT
        };
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let key = (
                batch as u32,
                in_channels as u32,
                out_channels as u32,
                height as u32,
                width as u32,
                out_height as u32,
                kh as u32,
                pad_x as u32,
                stride_x as u32,
                u32::from(half),
                dtype as u32,
            );
            CUDNN_DESCS.with(|cell: &RefCell<CudnnDescMap>| {
                let mut map = cell.borrow_mut();
                if !map.contains_key(&key) {
                    let desc = crate::cudnn::prepare_nchw_strided_ex(
                        batch as i32,
                        in_channels as i32,
                        out_channels as i32,
                        height as i32,
                        width as i32,
                        out_height as i32,
                        out_width as i32,
                        kh as i32,
                        kw as i32,
                        pad_y as i32,
                        pad_x as i32,
                        stride_y as i32,
                        stride_x as i32,
                        dtype,
                        backend.stream,
                    )?;
                    map.insert(key, desc);
                }
                let desc = map.get(&key).expect("just inserted");
                let ws_bytes = crate::cudnn::workspace_bytes(desc);
                let ws_ptr = if ws_bytes == 0 {
                    std::ptr::null_mut()
                } else {
                    CUDNN_WS.with(|ws| -> Result<*mut std::ffi::c_void, String> {
                        let mut slot = ws.borrow_mut();
                        let need_new = match slot.as_ref() {
                            Some(buf) => buf.size_bytes < ws_bytes,
                            None => true,
                        };
                        if need_new {
                            *slot = Some(DeviceBuffer::new(gpu_pool_round(ws_bytes.max(1)))?);
                        }
                        Ok(slot.as_ref().unwrap().ptr.as_ptr())
                    })?
                };
                let w_key = if half {
                    format!("{cache_namespace}::{weight_cache_key}::imcf16")
                } else {
                    format!("{cache_namespace}::{weight_cache_key}")
                };
                if half {
                    backend.cached_weight_buffer(&w_key, weights.len() * size_of::<u16>(), || {
                        let mut packed = vec![0u16; weights.len()];
                        for (dst, src) in packed.iter_mut().zip(weights) {
                            *dst = crate::quant::f32_to_f16(*src);
                        }
                        let raw = unsafe {
                            std::slice::from_raw_parts(
                                packed.as_ptr().cast::<u8>(),
                                packed.len() * size_of::<u16>(),
                            )
                        };
                        Ok(raw.to_vec())
                    })?;
                } else {
                    backend.cached_weight_buffer(&w_key, weights.len() * size_of::<f32>(), || {
                        let raw = unsafe {
                            std::slice::from_raw_parts(
                                weights.as_ptr().cast::<u8>(),
                                weights.len() * size_of::<f32>(),
                            )
                        };
                        Ok(raw.to_vec())
                    })?;
                }
                let weight = backend
                    .weight_buffers
                    .get(&w_key)
                    .ok_or_else(|| format!("missing cuDNN filter {w_key}"))?;
                let x_ptr = if half {
                    x.device_ptr_u16()?.cast()
                } else {
                    x.device_ptr()?.cast()
                };
                let out = if half {
                    GpuTensor::from_pool_half(out_channels, out_plane)?
                } else {
                    GpuTensor::from_pool(out_channels, out_plane)?
                };
                let out_ptr = if half {
                    out.device_ptr_u16()?.cast()
                } else {
                    out.device_ptr()?.cast()
                };
                crate::cudnn::convolution_forward_f16(
                    desc,
                    x_ptr,
                    weight.ptr.as_ptr(),
                    out_ptr,
                    ws_ptr,
                    backend.stream,
                )?;
                if half {
                    let bias_key = format!("{cache_namespace}::{weight_cache_key}::bias16");
                    backend.cached_weight_buffer(&bias_key, bias.len() * size_of::<u16>(), || {
                        let mut packed = vec![0u16; bias.len()];
                        for (dst, src) in packed.iter_mut().zip(bias) {
                            *dst = crate::quant::f32_to_f16(*src);
                        }
                        let raw = unsafe {
                            std::slice::from_raw_parts(
                                packed.as_ptr().cast::<u8>(),
                                packed.len() * size_of::<u16>(),
                            )
                        };
                        Ok(raw.to_vec())
                    })?;
                    let bias_buf = backend
                        .weight_buffers
                        .get(&bias_key)
                        .ok_or_else(|| format!("missing cuDNN bias {bias_key}"))?;
                    crate::cudnn::add_bias_nchw_f16(
                        bias_buf.ptr.as_ptr(),
                        out.device_ptr_u16()?.cast(),
                        batch as i32,
                        out_channels as i32,
                        out_height as i32,
                        out_width as i32,
                        backend.stream,
                    )?;
                } else {
                    let bias_key = format!("{cache_namespace}::{weight_cache_key}::bias");
                    backend.cached_weight_buffer(&bias_key, bias.len() * size_of::<f32>(), || {
                        let raw = unsafe {
                            std::slice::from_raw_parts(
                                bias.as_ptr().cast::<u8>(),
                                bias.len() * size_of::<f32>(),
                            )
                        };
                        Ok(raw.to_vec())
                    })?;
                    let bias_buf = backend
                        .weight_buffers
                        .get(&bias_key)
                        .ok_or_else(|| format!("missing cached CUDA conv bias buffer {bias_key}"))?;
                    gpu_check(unsafe {
                        makepad_cuda_add_planes_vec_f32(
                            out.device_ptr()?,
                            bias_buf.ptr.as_ptr().cast::<f32>(),
                            out_plane as u32,
                            out_channels as u32,
                            backend.stream,
                        )
                    })?;
                }
                gpu_prof(backend.stream, crate::prof::CAT_CONV2D, prof_start, 0);
                Ok(out)
            })
        })
    }

    /// Planar conv with explicit input/output sizes and stride. Used by the
    /// SD VAE encoder downsamplers (3x3 stride-2 with extra right/bottom pad
    /// encoded as in_w+1 / pad 0 after an explicit pad, or pad_x=0 and
    /// in-kernel OOB zeros). Weights are cached like the stride-1 path.
    #[allow(clippy::too_many_arguments)]
    pub fn gpu_conv2d_planar_strided(
        x: &GpuTensor,
        in_width: usize,
        in_height: usize,
        out_width: usize,
        out_height: usize,
        cache_namespace: &str,
        weight_cache_key: &str,
        weights: &[f32],
        bias: &[f32],
        out_channels: usize,
        kw: usize,
        kh: usize,
        pad_x: usize,
        pad_y: usize,
        stride_x: usize,
        stride_y: usize,
    ) -> Result<GpuTensor, String> {
        let in_channels = x.rows;
        if x.cols != in_width * in_height {
            return Err(format!(
                "gpu_conv2d_strided plane mismatch: cols={} in={}x{}",
                x.cols, in_width, in_height
            ));
        }
        if weights.len() != out_channels * in_channels * kw * kh || bias.len() != out_channels {
            return Err("gpu_conv2d_strided weight/bias shape mismatch".to_string());
        }
        if stride_x == 0 || stride_y == 0 || out_width == 0 || out_height == 0 {
            return Err("gpu_conv2d_strided invalid stride/output size".to_string());
        }
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let weight_bytes = weights.len() * size_of::<f32>();
            let qualified_key = format!("{cache_namespace}::{weight_cache_key}");
            backend.cached_weight_buffer(&qualified_key, weight_bytes, || {
                let raw = unsafe {
                    std::slice::from_raw_parts(weights.as_ptr().cast::<u8>(), weight_bytes)
                };
                Ok(raw.to_vec())
            })?;
            let bias_buf = gpu_upload_small(backend, bias)?;
            let out = GpuTensor::from_pool(out_channels, out_width * out_height)?;
            let weight = backend
                .weight_buffers
                .get(&qualified_key)
                .ok_or_else(|| format!("missing cached CUDA conv weight buffer {qualified_key}"))?;
            let status = unsafe {
                makepad_cuda_conv2d_planar_strided_f32(
                    x.device_ptr()?,
                    weight.ptr.as_ptr().cast::<f32>(),
                    bias_buf.ptr.as_ptr().cast::<f32>(),
                    out.device_ptr()?,
                    in_width as u32,
                    in_height as u32,
                    out_width as u32,
                    out_height as u32,
                    in_channels as u32,
                    out_channels as u32,
                    kw as u32,
                    kh as u32,
                    pad_x as u32,
                    pad_y as u32,
                    stride_x as u32,
                    stride_y as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_pool_release(bias_buf);
            Ok(out)
        })
    }

    /// 1x1 is `W[oc,ic] @ X[ic, plane]` — no spatial im2col.
    fn gpu_conv2d_1x1_gemm(
        x: &GpuTensor,
        width: usize,
        height: usize,
        cache_namespace: &str,
        weight_cache_key: &str,
        weights: &[f32],
        bias: &[f32],
        out_channels: usize,
    ) -> Result<GpuTensor, String> {
        let in_channels = x.rows;
        let plane = width * height;
        if x.half {
            return Err("gpu_conv2d_1x1_gemm expects f32 storage".to_string());
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let qualified_key = format!("{cache_namespace}::{weight_cache_key}::imcf16");
            backend.cached_weight_buffer(&qualified_key, weights.len() * size_of::<u16>(), || {
                let mut packed = vec![0u16; weights.len()];
                for (dst, src) in packed.iter_mut().zip(weights) {
                    *dst = crate::quant::f32_to_f16(*src);
                }
                let raw = unsafe {
                    std::slice::from_raw_parts(
                        packed.as_ptr().cast::<u8>(),
                        packed.len() * size_of::<u16>(),
                    )
                };
                Ok(raw.to_vec())
            })?;
            let weight = backend
                .weight_buffers
                .get(&qualified_key)
                .ok_or_else(|| format!("missing cached CUDA conv 1x1 buffer {qualified_key}"))?;
            let x16 = gpu_pool_acquire(in_channels * plane * size_of::<u16>())?;
            gpu_check(unsafe {
                makepad_cuda_f32_to_f16(
                    x.device_ptr()?,
                    x16.ptr.as_ptr().cast::<u16>(),
                    (in_channels * plane) as u32,
                    backend.stream,
                )
            })?;
            let out = GpuTensor::from_pool(out_channels, plane)?;
            let alpha = 1.0f32;
            let beta = 0.0f32;
            unsafe {
                crate::cublas_gemm_strided_batched_ex(
                    backend.blas,
                    crate::CUBLAS_OP_N,
                    crate::CUBLAS_OP_N,
                    plane as i32,
                    out_channels as i32,
                    in_channels as i32,
                    &alpha,
                    x16.ptr.as_ptr().cast::<std::ffi::c_void>(),
                    crate::CUDA_R_16F,
                    plane as i32,
                    0,
                    weight.ptr.as_ptr().cast::<std::ffi::c_void>(),
                    crate::CUDA_R_16F,
                    in_channels as i32,
                    0,
                    &beta,
                    out.device_ptr()?.cast::<std::ffi::c_void>(),
                    crate::CUDA_R_32F,
                    plane as i32,
                    0,
                    1,
                    crate::CUBLAS_COMPUTE_32F,
                    crate::CUBLAS_GEMM_DEFAULT,
                )
                .map_err(|err| format!("gpu_conv2d 1x1 gemm failed (oc={out_channels} ic={in_channels}): {err}"))?;
            }
            gpu_pool_release(x16);
            let bias_key = format!("{cache_namespace}::{weight_cache_key}::bias");
            backend.cached_weight_buffer(&bias_key, bias.len() * size_of::<f32>(), || {
                let raw = unsafe {
                    std::slice::from_raw_parts(
                        bias.as_ptr().cast::<u8>(),
                        bias.len() * size_of::<f32>(),
                    )
                };
                Ok(raw.to_vec())
            })?;
            let bias_buf = backend
                .weight_buffers
                .get(&bias_key)
                .ok_or_else(|| format!("missing cached CUDA conv bias buffer {bias_key}"))?;
            gpu_check(unsafe {
                makepad_cuda_add_planes_vec_f32(
                    out.device_ptr()?,
                    bias_buf.ptr.as_ptr().cast::<f32>(),
                    plane as u32,
                    out_channels as u32,
                    backend.stream,
                )
            })?;
            gpu_prof(backend.stream, crate::prof::CAT_CONV2D, prof_start, 0);
            Ok(out)
        })
    }

    fn gpu_conv2d_1x1_gemm_f16(
        x: &GpuTensor,
        width: usize,
        height: usize,
        cache_namespace: &str,
        weight_cache_key: &str,
        weights: &[f32],
        bias: &[f32],
        out_channels: usize,
    ) -> Result<GpuTensor, String> {
        if !x.half {
            return Err("gpu_conv2d_1x1_gemm_f16 expects f16 storage".to_string());
        }
        let in_channels = x.rows;
        let plane = width * height;
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let qualified_key = format!("{cache_namespace}::{weight_cache_key}::imcf16");
            backend.cached_weight_buffer(&qualified_key, weights.len() * size_of::<u16>(), || {
                let mut packed = vec![0u16; weights.len()];
                for (dst, src) in packed.iter_mut().zip(weights) {
                    *dst = crate::quant::f32_to_f16(*src);
                }
                let raw = unsafe {
                    std::slice::from_raw_parts(
                        packed.as_ptr().cast::<u8>(),
                        packed.len() * size_of::<u16>(),
                    )
                };
                Ok(raw.to_vec())
            })?;
            let weight = backend
                .weight_buffers
                .get(&qualified_key)
                .ok_or_else(|| format!("missing cached CUDA conv 1x1 f16 {qualified_key}"))?;
            let out = GpuTensor::from_pool_half(out_channels, plane)?;
            let alpha = 1.0f32;
            let beta = 0.0f32;
            unsafe {
                crate::cublas_gemm_strided_batched_ex(
                    backend.blas,
                    crate::CUBLAS_OP_N,
                    crate::CUBLAS_OP_N,
                    plane as i32,
                    out_channels as i32,
                    in_channels as i32,
                    &alpha,
                    x.device_ptr_u16()?.cast::<std::ffi::c_void>(),
                    crate::CUDA_R_16F,
                    plane as i32,
                    0,
                    weight.ptr.as_ptr().cast::<std::ffi::c_void>(),
                    crate::CUDA_R_16F,
                    in_channels as i32,
                    0,
                    &beta,
                    out.device_ptr_u16()?.cast::<std::ffi::c_void>(),
                    crate::CUDA_R_16F,
                    plane as i32,
                    0,
                    1,
                    crate::CUBLAS_COMPUTE_32F,
                    crate::CUBLAS_GEMM_DEFAULT,
                )
                .map_err(|err| {
                    format!("gpu_conv2d 1x1 f16 gemm failed (oc={out_channels} ic={in_channels}): {err}")
                })?;
            }
            let bias_key = format!("{cache_namespace}::{weight_cache_key}::bias16");
            backend.cached_weight_buffer(&bias_key, bias.len() * size_of::<u16>(), || {
                let mut packed = vec![0u16; bias.len()];
                for (dst, src) in packed.iter_mut().zip(bias) {
                    *dst = crate::quant::f32_to_f16(*src);
                }
                let raw = unsafe {
                    std::slice::from_raw_parts(
                        packed.as_ptr().cast::<u8>(),
                        packed.len() * size_of::<u16>(),
                    )
                };
                Ok(raw.to_vec())
            })?;
            let bias_buf = backend
                .weight_buffers
                .get(&bias_key)
                .ok_or_else(|| format!("missing 1x1 f16 bias {bias_key}"))?;
            crate::cudnn::add_bias_nchw_f16(
                bias_buf.ptr.as_ptr(),
                out.device_ptr_u16()?.cast(),
                1,
                out_channels as i32,
                height as i32,
                width as i32,
                backend.stream,
            )?;
            gpu_prof(backend.stream, crate::prof::CAT_CONV2D, prof_start, 0);
            Ok(out)
        })
    }

    /// Ceiling on the materialized im2col slab; bigger planes gemm in chunks.
    const IM2COL_MAX_CHUNK_BYTES: usize = 768 * 1024 * 1024;

    /// im2col + single-gemm planar conv: build a col-major (plane x ic*kh*kw)
    /// f16 im2col matrix (chunked to IM2COL_MAX_CHUNK_BYTES) and run ONE
    /// tensor-core gemm per chunk against the f16 weight matrix — whose
    /// col-major (k x oc) layout is exactly the source [oc][ic][kh][kw] array
    /// flattened, so the cached repack is a pure dtype convert. Replaces the
    /// 9-shift accumulator recipe: no padded input, no f32 accumulator
    /// re-read per shift, no interior extract, and the gemm k dimension is
    /// ic*kh*kw instead of a cublas-hostile k=ic. The gemm writes the planar
    /// [oc][plane] output directly (col-major C with ldc = plane).
    #[allow(clippy::too_many_arguments)]
    fn gpu_conv2d_planar_im2col(
        x: &GpuTensor,
        width: usize,
        height: usize,
        cache_namespace: &str,
        weight_cache_key: &str,
        weights: &[f32],
        bias: &[f32],
        out_channels: usize,
        kw: usize,
        kh: usize,
        pad_x: usize,
        pad_y: usize,
    ) -> Result<GpuTensor, String> {
        let in_channels = x.rows;
        let plane = width * height;
        let k_total = in_channels * kw * kh;
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;

            let qualified_key = format!("{cache_namespace}::{weight_cache_key}::imcf16");
            backend.cached_weight_buffer(&qualified_key, weights.len() * size_of::<u16>(), || {
                let mut packed = vec![0u16; weights.len()];
                for (dst, src) in packed.iter_mut().zip(weights) {
                    *dst = crate::quant::f32_to_f16(*src);
                }
                let raw = unsafe {
                    std::slice::from_raw_parts(
                        packed.as_ptr().cast::<u8>(),
                        packed.len() * size_of::<u16>(),
                    )
                };
                Ok(raw.to_vec())
            })?;
            let weight = backend
                .weight_buffers
                .get(&qualified_key)
                .ok_or_else(|| format!("missing cached CUDA conv im2col buffer {qualified_key}"))?;

            let max_chunk = (IM2COL_MAX_CHUNK_BYTES / (k_total * size_of::<u16>()))
                .max(1)
                .min(plane);
            let chunk_count = plane.div_ceil(max_chunk);
            let m_chunk_cap = plane.div_ceil(chunk_count);
            let slab_ptr =
                conv_scratch_ptr(0, m_chunk_cap * k_total * size_of::<u16>())?.cast::<u16>();

            let out = GpuTensor::from_pool(out_channels, plane)?;
            let alpha = 1.0f32;
            let beta = 0.0f32;
            let mut p0 = 0usize;
            while p0 < plane {
                let m_chunk = m_chunk_cap.min(plane - p0);
                let status = unsafe {
                    makepad_cuda_im2col_planar_f32_to_f16(
                        x.device_ptr()?,
                        slab_ptr,
                        width as u32,
                        height as u32,
                        kw as u32,
                        kh as u32,
                        pad_x as u32,
                        pad_y as u32,
                        p0 as u32,
                        m_chunk as u32,
                        k_total as u32,
                        backend.stream,
                    )
                };
                gpu_check(status)?;
                unsafe {
                    // C rows p0..p0+m_chunk (col-major, ldc = plane) =
                    //   im2col chunk (m_chunk x k, lda = m_chunk)
                    //   * W (k x oc, ldb = k)
                    crate::cublas_gemm_strided_batched_ex(
                        backend.blas,
                        crate::CUBLAS_OP_N,
                        crate::CUBLAS_OP_N,
                        m_chunk as i32,
                        out_channels as i32,
                        k_total as i32,
                        &alpha,
                        slab_ptr.cast::<std::ffi::c_void>(),
                        crate::CUDA_R_16F,
                        m_chunk as i32,
                        0,
                        weight.ptr.as_ptr().cast::<std::ffi::c_void>(),
                        crate::CUDA_R_16F,
                        k_total as i32,
                        0,
                        &beta,
                        out.device_ptr()?.add(p0).cast::<std::ffi::c_void>(),
                        crate::CUDA_R_32F,
                        plane as i32,
                        0,
                        1,
                        crate::CUBLAS_COMPUTE_32F,
                        crate::CUBLAS_GEMM_DEFAULT,
                    )
                    .map_err(|err| {
                        format!(
                            "gpu_conv2d im2col gemm failed (p0={p0} m={m_chunk} \
                             oc={out_channels} k={k_total}): {err}"
                        )
                    })?;
                }
                p0 += m_chunk;
            }

            // The bias is model-constant: cache it on device — every-call
            // pageable uploads hit the WDDM slow path under VRAM pressure.
            let bias_key = format!("{cache_namespace}::{weight_cache_key}::bias");
            backend.cached_weight_buffer(&bias_key, bias.len() * size_of::<f32>(), || {
                let raw = unsafe {
                    std::slice::from_raw_parts(
                        bias.as_ptr().cast::<u8>(),
                        bias.len() * size_of::<f32>(),
                    )
                };
                Ok(raw.to_vec())
            })?;
            let bias_buf = backend
                .weight_buffers
                .get(&bias_key)
                .ok_or_else(|| format!("missing cached CUDA conv bias buffer {bias_key}"))?;
            let status = unsafe {
                makepad_cuda_add_planes_vec_f32(
                    out.device_ptr()?,
                    bias_buf.ptr.as_ptr().cast::<f32>(),
                    plane as u32,
                    out_channels as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_prof(backend.stream, crate::prof::CAT_CONV2D, prof_start, 0);
            Ok(out)
        })
    }

    /// BiRefNet's decoder activation. Kept separate from SiLU/GELU so the
    /// checkpoint graph does not silently change its nonlinearity.
    pub fn gpu_birefnet_relu(x: &GpuTensor) -> Result<GpuTensor, String> {
        if x.half {
            return Err("gpu_birefnet_relu is f32-only".to_string());
        }
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(x.rows, x.cols)?;
            gpu_check(unsafe {
                makepad_cuda_birefnet_relu_f32(
                    x.device_ptr()?,
                    out.device_ptr()?,
                    x.rows * x.cols,
                    backend.stream,
                )
            })?;
            Ok(out)
        })
    }

    /// LeakyReLU with an explicit negative slope (RealESRGAN uses 0.2).
    /// Layout-agnostic elementwise pass over the whole tensor.
    pub fn gpu_realesrgan_lrelu(x: &GpuTensor, slope: f32) -> Result<GpuTensor, String> {
        if x.half {
            return Err("gpu_realesrgan_lrelu is f32-only".to_string());
        }
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(x.rows, x.cols)?;
            gpu_check(unsafe {
                makepad_cuda_realesrgan_lrelu_f32(
                    x.device_ptr()?,
                    out.device_ptr()?,
                    x.rows * x.cols,
                    slope,
                    backend.stream,
                )
            })?;
            Ok(out)
        })
    }

    /// `base + scale * delta` (the RRDB residual): one fused elementwise pass.
    pub fn gpu_realesrgan_scale_add(
        base: &GpuTensor,
        delta: &GpuTensor,
        scale: f32,
    ) -> Result<GpuTensor, String> {
        if base.half || delta.half {
            return Err("gpu_realesrgan_scale_add is f32-only".to_string());
        }
        if base.rows != delta.rows || base.cols != delta.cols {
            return Err(format!(
                "gpu_realesrgan_scale_add {}x{} vs {}x{}",
                base.rows, base.cols, delta.rows, delta.cols
            ));
        }
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(base.rows, base.cols)?;
            gpu_check(unsafe {
                makepad_cuda_realesrgan_scale_add_f32(
                    base.device_ptr()?,
                    delta.device_ptr()?,
                    out.device_ptr()?,
                    base.rows * base.cols,
                    scale,
                    backend.stream,
                )
            })?;
            Ok(out)
        })
    }

    /// Pool-allocated f16 planar tensor for the RealESRGAN fast path (the
    /// wide dense-block buffer and tail feature maps).  Contents undefined.
    pub fn gpu_realesrgan_alloc_f16(rows: usize, cols: usize) -> Result<GpuTensor, String> {
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            GpuTensor::from_pool_half(rows, cols)
        })
    }

    /// cuDNN f16 3x3 stride-1 "same" convolution, batch 1, planar rows.
    /// Reads the first `in_channels` rows of `input` and writes
    /// `out_channels` raw (biasless) rows of `output` starting at
    /// `out_row_offset` — with batch 1 a planar row block is exactly a
    /// contiguous NCHW tensor, so dense-block concats become pointer offsets.
    #[allow(clippy::too_many_arguments)]
    pub fn gpu_realesrgan_conv3x3_f16(
        input: &GpuTensor,
        in_channels: usize,
        width: usize,
        height: usize,
        cache_namespace: &str,
        weight_cache_key: &str,
        weights: &[f32],
        out_channels: usize,
        output: &GpuTensor,
        out_row_offset: usize,
    ) -> Result<(), String> {
        let plane = width * height;
        if !input.half || !output.half {
            return Err("gpu_realesrgan_conv3x3_f16 wants f16 tensors".into());
        }
        if input.cols != plane
            || output.cols != plane
            || in_channels > input.rows
            || out_row_offset + out_channels > output.rows
        {
            return Err(format!(
                "gpu_realesrgan_conv3x3_f16 shapes: in {}x{} (use {in_channels}), \
                 out {}x{} (offset {out_row_offset}+{out_channels}), plane {plane}",
                input.rows, input.cols, output.rows, output.cols
            ));
        }
        if weights.len() != out_channels * in_channels * 9 {
            return Err("gpu_realesrgan_conv3x3_f16 weight length mismatch".into());
        }
        if !crate::cudnn::available() {
            return Err("gpu_realesrgan_conv3x3_f16 requires cuDNN".into());
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let key = (
                1u32,
                in_channels as u32,
                out_channels as u32,
                height as u32,
                width as u32,
                height as u32,
                3u32,
                1u32,
                1u32,
                0x30u32,
                crate::cudnn::CUDNN_DATA_HALF as u32,
            );
            CUDNN_DESCS.with(|cell: &RefCell<CudnnDescMap>| {
                let mut map = cell.borrow_mut();
                if !map.contains_key(&key) {
                    let desc = crate::cudnn::prepare_nchw_strided_f16(
                        1,
                        in_channels as i32,
                        out_channels as i32,
                        height as i32,
                        width as i32,
                        3,
                        3,
                        1,
                        1,
                        backend.stream,
                    )?;
                    map.insert(key, desc);
                }
                let desc = map.get(&key).expect("just inserted");
                let ws_bytes = crate::cudnn::workspace_bytes(desc);
                let ws_ptr = if ws_bytes == 0 {
                    std::ptr::null_mut()
                } else {
                    CUDNN_WS.with(|ws| -> Result<*mut std::ffi::c_void, String> {
                        let mut slot = ws.borrow_mut();
                        let need_new = match slot.as_ref() {
                            Some(buf) => buf.size_bytes < ws_bytes,
                            None => true,
                        };
                        if need_new {
                            *slot = Some(DeviceBuffer::new(gpu_pool_round(ws_bytes.max(1)))?);
                        }
                        Ok(slot.as_ref().unwrap().ptr.as_ptr())
                    })?
                };
                let w_key = format!("{cache_namespace}::{weight_cache_key}::nchw16");
                backend.cached_weight_buffer(
                    &w_key,
                    weights.len() * size_of::<u16>(),
                    || Ok(pack_f16_bytes_rn(weights)),
                )?;
                let weight_ptr = backend
                    .weight_buffers
                    .get(&w_key)
                    .ok_or_else(|| format!("missing packed conv {w_key}"))?
                    .ptr
                    .as_ptr();
                let x_ptr = input.device_ptr_u16()?;
                let y_ptr = unsafe { output.device_ptr_u16()?.add(out_row_offset * plane) };
                crate::cudnn::convolution_forward_f16(
                    desc,
                    x_ptr.cast(),
                    weight_ptr,
                    y_ptr.cast(),
                    ws_ptr,
                    backend.stream,
                )?;
                gpu_prof(backend.stream, crate::prof::CAT_CONV2D, prof_start, 0);
                Ok(())
            })
        })
    }

    /// In-place bias + LeakyReLU over a row region of an f16 planar map
    /// (`slope` 1.0 degenerates to a pure bias add for linear conv tails).
    #[allow(clippy::too_many_arguments)]
    pub fn gpu_realesrgan_bias_lrelu_f16(
        tensor: &GpuTensor,
        row_offset: usize,
        channels: usize,
        cache_namespace: &str,
        bias_cache_key: &str,
        bias: &[f32],
        slope: f32,
    ) -> Result<(), String> {
        if !tensor.half {
            return Err("gpu_realesrgan_bias_lrelu_f16 wants an f16 tensor".into());
        }
        if row_offset + channels > tensor.rows || bias.len() != channels {
            return Err("gpu_realesrgan_bias_lrelu_f16 region mismatch".into());
        }
        let plane = tensor.cols;
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let bias_key = format!("{cache_namespace}::{bias_cache_key}::bias32");
            backend.cached_weight_buffer(&bias_key, bias.len() * size_of::<f32>(), || {
                let raw = unsafe {
                    std::slice::from_raw_parts(
                        bias.as_ptr().cast::<u8>(),
                        bias.len() * size_of::<f32>(),
                    )
                };
                Ok(raw.to_vec())
            })?;
            let bias_ptr = backend
                .weight_buffers
                .get(&bias_key)
                .ok_or_else(|| format!("missing cached bias {bias_key}"))?
                .ptr
                .as_ptr()
                .cast::<f32>();
            let data = unsafe { tensor.device_ptr_u16()?.add(row_offset * plane) };
            gpu_check(unsafe {
                makepad_cuda_realesrgan_bias_lrelu_f16(
                    data,
                    bias_ptr,
                    plane,
                    channels * plane,
                    slope,
                    backend.stream,
                )
            })
        })
    }

    /// cuDNN true-f32 (FMA math, no TF32) 3x3 stride-1 "same" convolution
    /// over planar rows, batch 1.  Returns a fresh biasless output tensor.
    /// The RealESRGAN head runs on this so no head rounding reaches the
    /// locked output envelope.
    #[allow(clippy::too_many_arguments)]
    pub fn gpu_realesrgan_conv3x3_f32(
        input: &GpuTensor,
        in_channels: usize,
        width: usize,
        height: usize,
        cache_namespace: &str,
        weight_cache_key: &str,
        weights: &[f32],
        out_channels: usize,
    ) -> Result<GpuTensor, String> {
        let plane = width * height;
        if input.half {
            return Err("gpu_realesrgan_conv3x3_f32 wants f32 tensors".into());
        }
        if input.cols != plane || in_channels > input.rows {
            return Err(format!(
                "gpu_realesrgan_conv3x3_f32 shapes: in {}x{} (use {in_channels}), plane {plane}",
                input.rows, input.cols
            ));
        }
        if weights.len() != out_channels * in_channels * 9 {
            return Err("gpu_realesrgan_conv3x3_f32 weight length mismatch".into());
        }
        if !crate::cudnn::available() {
            return Err("gpu_realesrgan_conv3x3_f32 requires cuDNN".into());
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(out_channels, plane)?;
            let key = (
                1u32,
                in_channels as u32,
                out_channels as u32,
                height as u32,
                width as u32,
                height as u32,
                3u32,
                1u32,
                1u32,
                0x32u32,
                crate::cudnn::CUDNN_DATA_FLOAT as u32,
            );
            CUDNN_DESCS.with(|cell: &RefCell<CudnnDescMap>| -> Result<(), String> {
                let mut map = cell.borrow_mut();
                if !map.contains_key(&key) {
                    let desc = crate::cudnn::prepare_nchw_strided_f32_fma(
                        1,
                        in_channels as i32,
                        out_channels as i32,
                        height as i32,
                        width as i32,
                        3,
                        3,
                        1,
                        1,
                        backend.stream,
                    )?;
                    map.insert(key, desc);
                }
                let desc = map.get(&key).expect("just inserted");
                let ws_bytes = crate::cudnn::workspace_bytes(desc);
                let ws_ptr = if ws_bytes == 0 {
                    std::ptr::null_mut()
                } else {
                    CUDNN_WS.with(|ws| -> Result<*mut std::ffi::c_void, String> {
                        let mut slot = ws.borrow_mut();
                        let need_new = match slot.as_ref() {
                            Some(buf) => buf.size_bytes < ws_bytes,
                            None => true,
                        };
                        if need_new {
                            *slot = Some(DeviceBuffer::new(gpu_pool_round(ws_bytes.max(1)))?);
                        }
                        Ok(slot.as_ref().unwrap().ptr.as_ptr())
                    })?
                };
                let w_key = format!("{cache_namespace}::{weight_cache_key}::nchw32");
                backend.cached_weight_buffer(
                    &w_key,
                    weights.len() * size_of::<f32>(),
                    || {
                        let raw = unsafe {
                            std::slice::from_raw_parts(
                                weights.as_ptr().cast::<u8>(),
                                weights.len() * size_of::<f32>(),
                            )
                        };
                        Ok(raw.to_vec())
                    },
                )?;
                let weight_ptr = backend
                    .weight_buffers
                    .get(&w_key)
                    .ok_or_else(|| format!("missing packed conv {w_key}"))?
                    .ptr
                    .as_ptr();
                crate::cudnn::convolution_forward_f16(
                    desc,
                    input.device_ptr()?.cast(),
                    weight_ptr,
                    out.device_ptr()?.cast(),
                    ws_ptr,
                    backend.stream,
                )?;
                gpu_prof(backend.stream, crate::prof::CAT_CONV2D, prof_start, 0);
                Ok(())
            })?;
            Ok(out)
        })
    }

    /// In-place bias + LeakyReLU over a full f32 planar map (`slope` 1.0
    /// degenerates to a pure bias add for linear conv tails).
    pub fn gpu_realesrgan_bias_lrelu_f32(
        tensor: &GpuTensor,
        cache_namespace: &str,
        bias_cache_key: &str,
        bias: &[f32],
        slope: f32,
    ) -> Result<(), String> {
        if tensor.half {
            return Err("gpu_realesrgan_bias_lrelu_f32 wants an f32 tensor".into());
        }
        if bias.len() != tensor.rows {
            return Err("gpu_realesrgan_bias_lrelu_f32 bias length mismatch".into());
        }
        let plane = tensor.cols;
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let bias_key = format!("{cache_namespace}::{bias_cache_key}::bias32");
            backend.cached_weight_buffer(&bias_key, bias.len() * size_of::<f32>(), || {
                let raw = unsafe {
                    std::slice::from_raw_parts(
                        bias.as_ptr().cast::<u8>(),
                        bias.len() * size_of::<f32>(),
                    )
                };
                Ok(raw.to_vec())
            })?;
            let bias_ptr = backend
                .weight_buffers
                .get(&bias_key)
                .ok_or_else(|| format!("missing cached bias {bias_key}"))?
                .ptr
                .as_ptr()
                .cast::<f32>();
            gpu_check(unsafe {
                makepad_cuda_realesrgan_bias_lrelu_f32(
                    tensor.device_ptr()?,
                    bias_ptr,
                    plane,
                    tensor.rows * plane,
                    slope,
                    backend.stream,
                )
            })
        })
    }

    /// Pool-allocated f32 planar tensor for the RealESRGAN spine/head.
    /// Contents undefined.
    pub fn gpu_realesrgan_alloc_f32(rows: usize, cols: usize) -> Result<GpuTensor, String> {
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            GpuTensor::from_pool(rows, cols)
        })
    }

    /// f32 spine residual for the RealESRGAN body: `dst32 = base + scale *
    /// (delta + bias)` with the delta read from an f32 tensor or an f16 row
    /// region, optionally mirroring the result into an f16 row region (the
    /// conv-input view).  Exactly one delta source must be given.  Keeping
    /// the residual chain in f32 stops rounding from compounding across the
    /// 23 RRDB blocks.
    #[allow(clippy::too_many_arguments)]
    pub fn gpu_realesrgan_spine_axpb(
        base: &GpuTensor,
        delta32: Option<&GpuTensor>,
        delta16: Option<(&GpuTensor, usize)>,
        dst32: &GpuTensor,
        dst16: Option<(&GpuTensor, usize)>,
        channels: usize,
        cache_namespace: &str,
        bias_cache_key: &str,
        bias: &[f32],
        scale: f32,
    ) -> Result<(), String> {
        let plane = dst32.cols;
        if base.half || dst32.half {
            return Err("gpu_realesrgan_spine_axpb wants f32 base/dst".into());
        }
        if delta32.is_some() == delta16.is_some() {
            return Err("gpu_realesrgan_spine_axpb wants exactly one delta source".into());
        }
        if base.cols != plane
            || channels > base.rows
            || channels > dst32.rows
            || !(bias.is_empty() || bias.len() == channels)
        {
            return Err("gpu_realesrgan_spine_axpb region mismatch".into());
        }
        if let Some(delta) = delta32 {
            if delta.half || delta.cols != plane || channels > delta.rows {
                return Err("gpu_realesrgan_spine_axpb delta32 mismatch".into());
            }
        }
        if let Some((delta, row)) = delta16 {
            if !delta.half || delta.cols != plane || row + channels > delta.rows {
                return Err("gpu_realesrgan_spine_axpb delta16 mismatch".into());
            }
        }
        if let Some((mirror, row)) = dst16 {
            if !mirror.half || mirror.cols != plane || row + channels > mirror.rows {
                return Err("gpu_realesrgan_spine_axpb dst16 mismatch".into());
            }
        }
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let bias_ptr = if bias.is_empty() {
                std::ptr::null()
            } else {
                let bias_key = format!("{cache_namespace}::{bias_cache_key}::bias32");
                backend.cached_weight_buffer(&bias_key, bias.len() * size_of::<f32>(), || {
                    let raw = unsafe {
                        std::slice::from_raw_parts(
                            bias.as_ptr().cast::<u8>(),
                            bias.len() * size_of::<f32>(),
                        )
                    };
                    Ok(raw.to_vec())
                })?;
                backend
                    .weight_buffers
                    .get(&bias_key)
                    .ok_or_else(|| format!("missing cached bias {bias_key}"))?
                    .ptr
                    .as_ptr()
                    .cast::<f32>()
                    .cast_const()
            };
            unsafe {
                let delta32_ptr = match delta32 {
                    Some(delta) => delta.device_ptr()?.cast_const(),
                    None => std::ptr::null(),
                };
                let delta16_ptr = match delta16 {
                    Some((delta, row)) => {
                        delta.device_ptr_u16()?.add(row * plane).cast_const()
                    }
                    None => std::ptr::null(),
                };
                let dst16_ptr = match dst16 {
                    Some((mirror, row)) => mirror.device_ptr_u16()?.add(row * plane),
                    None => std::ptr::null_mut(),
                };
                gpu_check(makepad_cuda_realesrgan_spine_axpb(
                    base.device_ptr()?.cast_const(),
                    delta32_ptr,
                    delta16_ptr,
                    bias_ptr,
                    dst32.device_ptr()?,
                    dst16_ptr,
                    plane,
                    channels * plane,
                    scale,
                    backend.stream,
                ))
            }
        })
    }

    /// f32 twin of [`gpu_realesrgan_quantize_rgb8`].
    pub fn gpu_realesrgan_quantize_rgb8_f32(x: &GpuTensor) -> Result<Vec<u8>, String> {
        if x.half || x.rows != 3 {
            return Err("gpu_realesrgan_quantize_rgb8_f32 wants an f32 [3, plane] tensor".into());
        }
        let plane = x.cols;
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let scratch = gpu_pool_acquire(plane * 3)?;
            gpu_check(unsafe {
                makepad_cuda_realesrgan_quantize_rgb8_f32(
                    x.device_ptr()?,
                    scratch.ptr.as_ptr().cast::<u8>(),
                    plane,
                    backend.stream,
                )
            })?;
            let bytes = scratch.read_bytes(plane * 3, backend.stream)?;
            gpu_pool_release(scratch);
            Ok(bytes)
        })
    }

    /// Bilinear planar resize with PyTorch's align_corners switch.
    #[allow(clippy::too_many_arguments)]
    pub fn gpu_birefnet_resize_bilinear(
        x: &GpuTensor,
        in_width: usize,
        in_height: usize,
        out_width: usize,
        out_height: usize,
        align_corners: bool,
    ) -> Result<GpuTensor, String> {
        if x.half || x.cols != in_width.saturating_mul(in_height) {
            return Err("gpu_birefnet_resize_bilinear shape mismatch".to_string());
        }
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(x.rows, out_width.saturating_mul(out_height))?;
            gpu_check(unsafe {
                makepad_cuda_birefnet_resize_bilinear_f32(
                    x.device_ptr()?,
                    out.device_ptr()?,
                    in_width as u32,
                    in_height as u32,
                    out_width as u32,
                    out_height as u32,
                    x.rows as u32,
                    u32::from(align_corners),
                    backend.stream,
                )
            })?;
            Ok(out)
        })
    }

    /// Transpose `[rows, cols]` into `[cols, rows]`. Planar `[C, HW]` ↔ tokens `[HW, C]`.
    pub fn gpu_planar_tokens_transpose(x: &GpuTensor) -> Result<GpuTensor, String> {
        if x.half {
            return Err("gpu_planar_tokens_transpose is f32-only".into());
        }
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(x.cols, x.rows)?;
            gpu_check(unsafe {
                makepad_cuda_paint_transpose_f32(
                    x.device_ptr()?,
                    out.device_ptr()?,
                    x.rows as u32,
                    x.cols as u32,
                    backend.stream,
                )
            })?;
            Ok(out)
        })
    }

    /// Official Hunyuan RA: softmax over K with Q, apply to cat(V_alb, V_mr)
    /// viewed as `[heads, 2*head_dim]`, split back to two hidden packs.
    pub fn gpu_paint_ref_attn_wide_v(
        q: &GpuTensor,
        k: &GpuTensor,
        v_alb: &GpuTensor,
        v_mr: &GpuTensor,
        heads: usize,
        scale: f32,
    ) -> Result<(GpuTensor, GpuTensor), String> {
        let q_len = q.rows;
        let hidden = q.cols;
        if k.cols != hidden || v_alb.cols != hidden || v_mr.cols != hidden {
            return Err("gpu_paint_ref_attn_wide_v hidden mismatch".into());
        }
        if k.rows != v_alb.rows || k.rows != v_mr.rows {
            return Err("gpu_paint_ref_attn_wide_v kv mismatch".into());
        }
        if heads == 0 || hidden % heads != 0 {
            return Err("gpu_paint_ref_attn_wide_v head mismatch".into());
        }
        if q.half || k.half || v_alb.half || v_mr.half {
            return Err("gpu_paint_ref_attn_wide_v is f32-only".into());
        }
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let o_alb = GpuTensor::from_pool(q_len, hidden)?;
            let o_mr = GpuTensor::from_pool(q_len, hidden)?;
            gpu_check(unsafe {
                makepad_cuda_paint_ref_attn_wide_v_f32(
                    q.device_ptr()?,
                    k.device_ptr()?,
                    v_alb.device_ptr()?,
                    v_mr.device_ptr()?,
                    o_alb.device_ptr()?,
                    o_mr.device_ptr()?,
                    q_len as u32,
                    k.rows as u32,
                    hidden as u32,
                    heads as u32,
                    scale,
                    backend.stream,
                )
            })?;
            Ok((o_alb, o_mr))
        })
    }

    /// Independent self-attention over `batch` sequences packed as
    /// `[batch * seq, hidden]`. Does not mix tokens across the batch.
    /// Inference (`FLUX_ATTN_F16` default on) packs heads and uses one
    /// strided-batched f16 GEMM; tap canaries keep the scalar f32 kernel.
    pub fn gpu_paint_attn_batched_self(
        q: &GpuTensor,
        k: &GpuTensor,
        v: &GpuTensor,
        batch: usize,
        heads: usize,
        scale: f32,
    ) -> Result<GpuTensor, String> {
        if batch == 0 || q.rows % batch != 0 {
            return Err(format!(
                "gpu_paint_attn_batched_self rows {} vs batch {batch}",
                q.rows
            ));
        }
        let seq = q.rows / batch;
        let hidden = q.cols;
        if k.rows != q.rows || v.rows != q.rows || k.cols != hidden || v.cols != hidden {
            return Err("gpu_paint_attn_batched_self shape mismatch".into());
        }
        if heads == 0 || hidden % heads != 0 {
            return Err("gpu_paint_attn_batched_self head mismatch".into());
        }
        if q.half || k.half || v.half {
            return Err("gpu_paint_attn_batched_self is f32-only".into());
        }
        let head_dim = hidden / heads;
        if gpu_attention_f16_enabled() && head_dim == 64 {
            return gpu_paint_attn_batched_self_f16(q, k, v, batch, seq, heads, head_dim, scale);
        }
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(q.rows, hidden)?;
            gpu_check(unsafe {
                makepad_cuda_paint_attn_batched_self_f32(
                    q.device_ptr()?,
                    k.device_ptr()?,
                    v.device_ptr()?,
                    out.device_ptr()?,
                    batch as u32,
                    seq as u32,
                    hidden as u32,
                    heads as u32,
                    scale,
                    backend.stream,
                )
            })?;
            Ok(out)
        })
    }

    fn gpu_paint_attn_batched_self_f16(
        q: &GpuTensor,
        k: &GpuTensor,
        v: &GpuTensor,
        batch: usize,
        seq: usize,
        heads: usize,
        head_dim: usize,
        scale: f32,
    ) -> Result<GpuTensor, String> {
        let pack_rows = batch * heads * seq;
        let bh = batch * heads;
        let scores_len = bh
            .checked_mul(seq)
            .and_then(|n| n.checked_mul(seq))
            .ok_or_else(|| "paint batched self scores overflow".to_string())?;
        let elems = pack_rows * head_dim;
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let q_pack = gpu_pool_acquire(elems * size_of::<f32>())?;
            let k_pack = gpu_pool_acquire(elems * size_of::<f32>())?;
            let v_pack = gpu_pool_acquire(elems * size_of::<f32>())?;
            for (src, dst) in [
                (q, &q_pack),
                (k, &k_pack),
                (v, &v_pack),
            ] {
                gpu_check(unsafe {
                    makepad_cuda_paint_pack_heads_f32(
                        src.device_ptr()?,
                        dst.ptr.as_ptr().cast::<f32>(),
                        batch as u32,
                        seq as u32,
                        heads as u32,
                        head_dim as u32,
                        backend.stream,
                    )
                })?;
            }
            let q16 = gpu_pool_acquire(elems * size_of::<u16>())?;
            let k16 = gpu_pool_acquire(elems * size_of::<u16>())?;
            let v16 = gpu_pool_acquire(elems * size_of::<u16>())?;
            let p16 = gpu_pool_acquire(scores_len * size_of::<u16>())?;
            for (src, dst) in [(&q_pack, &q16), (&k_pack, &k16), (&v_pack, &v16)] {
                gpu_check(unsafe {
                    makepad_cuda_f32_to_f16(
                        src.ptr.as_ptr().cast::<f32>(),
                        dst.ptr.as_ptr().cast::<u16>(),
                        elems as u32,
                        backend.stream,
                    )
                })?;
            }
            let scores = gpu_pool_acquire(scores_len * size_of::<f32>())?;
            let scores_ptr = scores.ptr.as_ptr().cast::<f32>();
            let beta = 0.0f32;
            let one = 1.0f32;
            let stride_q = (seq * head_dim) as i64;
            unsafe {
                crate::cublas_gemm_strided_batched_ex(
                    backend.blas,
                    crate::CUBLAS_OP_T,
                    crate::CUBLAS_OP_N,
                    seq as i32,
                    seq as i32,
                    head_dim as i32,
                    &scale,
                    k16.ptr.as_ptr(),
                    crate::CUDA_R_16F,
                    head_dim as i32,
                    stride_q,
                    q16.ptr.as_ptr(),
                    crate::CUDA_R_16F,
                    head_dim as i32,
                    stride_q,
                    &beta,
                    scores_ptr.cast::<std::ffi::c_void>(),
                    crate::CUDA_R_32F,
                    seq as i32,
                    (seq * seq) as i64,
                    bh as i32,
                    crate::CUBLAS_COMPUTE_32F,
                    crate::CUBLAS_GEMM_DEFAULT,
                )
                .map_err(|err| format!("paint batched self qk gemm failed: {err}"))?;
            }
            gpu_check(unsafe {
                makepad_cuda_softmax_rows_precise_f32(
                    scores_ptr,
                    scores_ptr,
                    (bh * seq) as u32,
                    seq as u32,
                    seq as u32,
                    backend.stream,
                )
            })?;
            gpu_check(unsafe {
                makepad_cuda_f32_to_f16(
                    scores_ptr,
                    p16.ptr.as_ptr().cast::<u16>(),
                    scores_len as u32,
                    backend.stream,
                )
            })?;
            let o_pack = gpu_pool_acquire(elems * size_of::<f32>())?;
            unsafe {
                crate::cublas_gemm_strided_batched_ex(
                    backend.blas,
                    crate::CUBLAS_OP_N,
                    crate::CUBLAS_OP_N,
                    head_dim as i32,
                    seq as i32,
                    seq as i32,
                    &one,
                    v16.ptr.as_ptr(),
                    crate::CUDA_R_16F,
                    head_dim as i32,
                    stride_q,
                    p16.ptr.as_ptr(),
                    crate::CUDA_R_16F,
                    seq as i32,
                    (seq * seq) as i64,
                    &beta,
                    o_pack.ptr.as_ptr(),
                    crate::CUDA_R_32F,
                    head_dim as i32,
                    stride_q,
                    bh as i32,
                    crate::CUBLAS_COMPUTE_32F,
                    crate::CUBLAS_GEMM_DEFAULT,
                )
                .map_err(|err| format!("paint batched self pv gemm failed: {err}"))?;
            }
            let out = GpuTensor::from_pool(q.rows, q.cols)?;
            gpu_check(unsafe {
                makepad_cuda_paint_unpack_heads_f32(
                    o_pack.ptr.as_ptr().cast::<f32>(),
                    out.device_ptr()?,
                    batch as u32,
                    seq as u32,
                    heads as u32,
                    head_dim as u32,
                    backend.stream,
                )
            })?;
            for buf in [q_pack, k_pack, v_pack, q16, k16, v16, p16, scores, o_pack] {
                gpu_pool_release(buf);
            }
            Ok(out)
        })
    }

    /// Independent GroupNorm over `batch` planar images packed as `[C, batch*H*W]`.
    pub fn gpu_paint_group_norm_batched(
        x: &GpuTensor,
        width: usize,
        height: usize,
        batch: usize,
        groups: usize,
        cache_namespace: &str,
        cache_key: &str,
        gamma: &[f32],
        beta: &[f32],
        eps: f32,
    ) -> Result<GpuTensor, String> {
        let channels = x.rows;
        if batch == 0 || groups == 0 || channels % groups != 0 {
            return Err("gpu_paint_group_norm_batched shape mismatch".into());
        }
        if x.cols != batch * width * height || gamma.len() != channels || beta.len() != channels {
            return Err("gpu_paint_group_norm_batched plane mismatch".into());
        }
        if x.half {
            return Err("gpu_paint_group_norm_batched is f32-only".into());
        }
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let vec_key = format!("{cache_namespace}::{cache_key}::gn");
            let vec_bytes = 2 * channels * size_of::<f32>();
            backend.cached_weight_buffer(&vec_key, vec_bytes, || {
                let mut concat = Vec::with_capacity(2 * channels);
                concat.extend_from_slice(gamma);
                concat.extend_from_slice(beta);
                let raw = unsafe {
                    std::slice::from_raw_parts(concat.as_ptr().cast::<u8>(), vec_bytes)
                };
                Ok(raw.to_vec())
            })?;
            let stats = gpu_pool_acquire(batch * groups * 2 * size_of::<f32>())?;
            let out = GpuTensor::from_pool(channels, x.cols)?;
            let vec_buf = backend
                .weight_buffers
                .get(&vec_key)
                .ok_or_else(|| format!("missing cached CUDA group norm buffer {vec_key}"))?;
            let gamma_ptr = vec_buf.ptr.as_ptr().cast::<f32>();
            gpu_check(unsafe {
                makepad_cuda_paint_gn_batched_f32(
                    x.device_ptr()?,
                    gamma_ptr,
                    gamma_ptr.add(channels),
                    stats.ptr.as_ptr().cast::<f32>(),
                    out.device_ptr()?,
                    width as u32,
                    height as u32,
                    channels as u32,
                    groups as u32,
                    batch as u32,
                    eps,
                    backend.stream,
                )
            })?;
            gpu_pool_release(stats);
            Ok(out)
        })
    }

    /// Packed NCHW `[N, C*H*W]` GroupNorm via cuDNN (same op Python uses).
    pub fn gpu_group_norm_nchw(
        x: &GpuTensor,
        channels: usize,
        width: usize,
        height: usize,
        groups: usize,
        cache_namespace: &str,
        cache_key: &str,
        gamma: &[f32],
        beta: &[f32],
        eps: f32,
    ) -> Result<GpuTensor, String> {
        let batch = x.rows;
        if batch == 0 || groups == 0 || channels % groups != 0 {
            return Err("gpu_group_norm_nchw shape mismatch".into());
        }
        if x.cols != channels * width * height || gamma.len() != channels || beta.len() != channels {
            return Err("gpu_group_norm_nchw plane mismatch".into());
        }
        if !crate::cudnn::available() {
            return Err("cuDNN unavailable for NCHW group norm".into());
        }
        let dtype = if x.half {
            crate::cudnn::CUDNN_DATA_HALF
        } else {
            crate::cudnn::CUDNN_DATA_FLOAT
        };
        let gn_result = with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let vec_key = if x.half {
                format!("{cache_namespace}::{cache_key}::gn_nchw16")
            } else {
                format!("{cache_namespace}::{cache_key}::gn_nchw")
            };
            if x.half {
                let mut concat = Vec::with_capacity(2 * channels);
                concat.extend_from_slice(&pack_f16_bytes(gamma));
                concat.extend_from_slice(&pack_f16_bytes(beta));
                backend.cached_weight_buffer(&vec_key, concat.len(), || Ok(concat))?;
            } else {
                let vec_bytes = 2 * channels * size_of::<f32>();
                backend.cached_weight_buffer(&vec_key, vec_bytes, || {
                    let mut concat = Vec::with_capacity(2 * channels);
                    concat.extend_from_slice(gamma);
                    concat.extend_from_slice(beta);
                    let raw = unsafe {
                        std::slice::from_raw_parts(concat.as_ptr().cast::<u8>(), vec_bytes)
                    };
                    Ok(raw.to_vec())
                })?;
            }
            let out = if x.half {
                GpuTensor::from_pool_half(batch, x.cols)?
            } else {
                GpuTensor::from_pool(batch, x.cols)?
            };
            let vec_buf = backend
                .weight_buffers
                .get(&vec_key)
                .ok_or_else(|| format!("missing cached CUDA NCHW group norm {vec_key}"))?;
            let gamma_ptr = vec_buf.ptr.as_ptr();
            let beta_ptr = if x.half {
                unsafe { gamma_ptr.add(channels * size_of::<u16>()) }
            } else {
                unsafe { gamma_ptr.add(channels * size_of::<f32>()) }
            };
            let gn = crate::cudnn::group_norm_nchw(
                batch as i32,
                channels as i32,
                height as i32,
                width as i32,
                groups as i32,
                eps,
                x.storage_ptr()?.cast(),
                gamma_ptr,
                beta_ptr,
                out.storage_ptr()?.cast(),
                dtype,
                backend.stream,
            );
            if let Err(err) = gn {
                static BN_LOG: std::sync::Once = std::sync::Once::new();
                BN_LOG.call_once(|| eprintln!("PBR_CUDNN_GN_BN {err}"));
                let ng = batch * groups;
                let ones_key = format!("paint-unet::gn_bn_ones_{ng}");
                backend.cached_weight_buffer(&ones_key, ng * size_of::<f32>(), || {
                    let v = vec![1.0f32; ng];
                    let raw = unsafe {
                        std::slice::from_raw_parts(v.as_ptr().cast::<u8>(), ng * size_of::<f32>())
                    };
                    Ok(raw.to_vec())
                })?;
                let zeros_key = format!("paint-unet::gn_bn_zeros_{ng}");
                backend.cached_weight_buffer(&zeros_key, ng * size_of::<f32>(), || {
                    Ok(vec![0u8; ng * size_of::<f32>()])
                })?;
                let ones = backend
                    .weight_buffers
                    .get(&ones_key)
                    .ok_or_else(|| "missing GN BN ones".to_string())?;
                let zeros = backend
                    .weight_buffers
                    .get(&zeros_key)
                    .ok_or_else(|| "missing GN BN zeros".to_string())?;
                let scratch = GpuTensor::from_pool(2, ng)?;
                let mean = scratch.device_ptr()?;
                let var = unsafe { mean.add(ng) };
                crate::cudnn::group_norm_bn_nchw(
                    batch as i32,
                    channels as i32,
                    height as i32,
                    width as i32,
                    groups as i32,
                    eps,
                    x.storage_ptr()?.cast(),
                    out.storage_ptr()?.cast(),
                    gamma_ptr,
                    beta_ptr,
                    ones.ptr.as_ptr(),
                    zeros.ptr.as_ptr(),
                    mean.cast(),
                    var.cast(),
                    dtype,
                    backend.stream,
                )?;
            }
            Ok(out)
        });
        match gn_result {
            Ok(out) => Ok(out),
            Err(err) if x.half => {
                static F32_VIEW: std::sync::Once = std::sync::Once::new();
                F32_VIEW.call_once(|| eprintln!("PBR_CUDNN_GN_F32_VIEW {err}"));
                let y = gpu_group_norm_nchw(
                    &gpu_to_f32(x)?,
                    channels,
                    width,
                    height,
                    groups,
                    cache_namespace,
                    cache_key,
                    gamma,
                    beta,
                    eps,
                )?;
                gpu_to_f16(&y)
            }
            Err(err) => Err(err),
        }
    }

    /// PoseRoPE with xyz already on device (`seq * 3` packed u32).
    pub fn gpu_paint_pose_rope_dev(
        x: &GpuTensor,
        xyz: &GpuTensor,
        heads: usize,
        voxel_res: usize,
    ) -> Result<GpuTensor, String> {
        if x.half {
            return Err("gpu_paint_pose_rope_dev is f32-only".into());
        }
        if heads == 0 || x.cols % heads != 0 {
            return Err("gpu_paint_pose_rope_dev head mismatch".into());
        }
        if xyz.rows.saturating_mul(xyz.cols) != x.rows.saturating_mul(3) {
            return Err(format!(
                "gpu_paint_pose_rope_dev xyz {} vs seq {}",
                xyz.rows * xyz.cols,
                x.rows
            ));
        }
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(x.rows, x.cols)?;
            gpu_check(unsafe {
                makepad_cuda_paint_pose_rope_f32(
                    x.device_ptr()?,
                    xyz.device_ptr()?.cast::<u32>(),
                    out.device_ptr()?,
                    x.rows as u32,
                    x.cols as u32,
                    heads as u32,
                    voxel_res as u32,
                    backend.stream,
                )
            })?;
            Ok(out)
        })
    }

    pub fn gpu_paint_scale(x: &GpuTensor, scale: f32) -> Result<GpuTensor, String> {
        if x.half {
            return Err("gpu_paint_scale is f32-only".into());
        }
        let n = x.rows.saturating_mul(x.cols);
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(x.rows, x.cols)?;
            gpu_check(unsafe {
                makepad_cuda_paint_scale_f32(
                    x.device_ptr()?,
                    out.device_ptr()?,
                    scale,
                    n as u32,
                    backend.stream,
                )
            })?;
            Ok(out)
        })
    }

    /// 3D PoseRoPE. `xyz` is `seq * 3` packed u32 bins.
    pub fn gpu_paint_pose_rope(
        x: &GpuTensor,
        xyz: &[u32],
        heads: usize,
        voxel_res: usize,
    ) -> Result<GpuTensor, String> {
        if x.half {
            return Err("gpu_paint_pose_rope is f32-only".into());
        }
        if heads == 0 || x.cols % heads != 0 {
            return Err("gpu_paint_pose_rope head mismatch".into());
        }
        if xyz.len() != x.rows.saturating_mul(3) {
            return Err(format!(
                "gpu_paint_pose_rope xyz {} vs seq {}",
                xyz.len(),
                x.rows
            ));
        }
        let xyz_g = gpu_upload_u32(xyz)?;
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(x.rows, x.cols)?;
            gpu_check(unsafe {
                makepad_cuda_paint_pose_rope_f32(
                    x.device_ptr()?,
                    xyz_g.device_ptr()?.cast::<u32>(),
                    out.device_ptr()?,
                    x.rows as u32,
                    x.cols as u32,
                    heads as u32,
                    voxel_res as u32,
                    backend.stream,
                )
            })?;
            Ok(out)
        })
    }

    /// Convert row-major [token, channel] into planar [channel, token].
    pub fn gpu_birefnet_tokens_to_planar(x: &GpuTensor) -> Result<GpuTensor, String> {
        if x.half {
            return Err("gpu_birefnet_tokens_to_planar is f32-only".to_string());
        }
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(x.cols, x.rows)?;
            gpu_check(unsafe {
                makepad_cuda_birefnet_tokens_to_planar_f32(
                    x.device_ptr()?,
                    out.device_ptr()?,
                    x.rows as u32,
                    x.cols as u32,
                    backend.stream,
                )
            })?;
            Ok(out)
        })
    }

    /// Rearrange `[C*r*r,H,W]` into `[C,H*r,W*r]` and add one bias per
    /// output channel.  This is the layout half of a ConvTranspose2d whose
    /// kernel and stride are both `r` (there is no spatial overlap).
    pub fn gpu_pixel_shuffle_planar(
        x: &GpuTensor,
        in_width: usize,
        in_height: usize,
        out_channels: usize,
        scale: usize,
        bias: &[f32],
    ) -> Result<GpuTensor, String> {
        if x.half
            || scale == 0
            || x.rows != out_channels.saturating_mul(scale).saturating_mul(scale)
            || x.cols != in_width.saturating_mul(in_height)
            || bias.len() != out_channels
        {
            return Err("gpu_pixel_shuffle_planar shape mismatch".to_string());
        }
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out_width = in_width
                .checked_mul(scale)
                .ok_or_else(|| "gpu_pixel_shuffle_planar width overflow".to_string())?;
            let out_height = in_height
                .checked_mul(scale)
                .ok_or_else(|| "gpu_pixel_shuffle_planar height overflow".to_string())?;
            let out = GpuTensor::from_pool(out_channels, out_width * out_height)?;
            let bias_buf = gpu_upload_small(backend, bias)?;
            gpu_check(unsafe {
                makepad_cuda_pixel_shuffle_planar_f32(
                    x.device_ptr()?,
                    bias_buf.ptr.as_ptr().cast::<f32>(),
                    out.device_ptr()?,
                    in_width as u32,
                    in_height as u32,
                    out_channels as u32,
                    scale as u32,
                    backend.stream,
                )
            })?;
            gpu_pool_release(bias_buf);
            Ok(out)
        })
    }

    /// `gpu_pixel_shuffle_planar` with the model-constant bias cached on
    /// device by key, so warm calls issue no host uploads (required for CUDA
    /// graph capture of a whole forward).
    #[allow(clippy::too_many_arguments)]
    pub fn gpu_pixel_shuffle_planar_cached(
        x: &GpuTensor,
        in_width: usize,
        in_height: usize,
        out_channels: usize,
        scale: usize,
        cache_namespace: &str,
        bias_cache_key: &str,
        bias: &[f32],
    ) -> Result<GpuTensor, String> {
        if x.half
            || scale == 0
            || x.rows != out_channels.saturating_mul(scale).saturating_mul(scale)
            || x.cols != in_width.saturating_mul(in_height)
            || bias.len() != out_channels
        {
            return Err("gpu_pixel_shuffle_planar shape mismatch".to_string());
        }
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out_width = in_width
                .checked_mul(scale)
                .ok_or_else(|| "gpu_pixel_shuffle_planar width overflow".to_string())?;
            let out_height = in_height
                .checked_mul(scale)
                .ok_or_else(|| "gpu_pixel_shuffle_planar height overflow".to_string())?;
            let bias_key = format!("{cache_namespace}::{bias_cache_key}::psbias");
            backend.cached_weight_buffer(&bias_key, bias.len() * size_of::<f32>(), || {
                let raw = unsafe {
                    std::slice::from_raw_parts(
                        bias.as_ptr().cast::<u8>(),
                        bias.len() * size_of::<f32>(),
                    )
                };
                Ok(raw.to_vec())
            })?;
            let bias_buf = backend
                .weight_buffers
                .get(&bias_key)
                .ok_or_else(|| format!("missing cached pixel-shuffle bias {bias_key}"))?;
            let out = GpuTensor::from_pool(out_channels, out_width * out_height)?;
            gpu_check(unsafe {
                makepad_cuda_pixel_shuffle_planar_f32(
                    x.device_ptr()?,
                    bias_buf.ptr.as_ptr().cast::<f32>(),
                    out.device_ptr()?,
                    in_width as u32,
                    in_height as u32,
                    out_channels as u32,
                    scale as u32,
                    backend.stream,
                )
            })?;
            Ok(out)
        })
    }

    /// Rearrange `[C,H,W] -> [C*hg*wg,H/hg,W/wg]` exactly like the
    /// BiRefNet decoder's einops image-to-patches expression.
    pub fn gpu_birefnet_image_to_patches(
        image: &GpuTensor,
        image_width: usize,
        image_height: usize,
        out_width: usize,
        out_height: usize,
    ) -> Result<GpuTensor, String> {
        if image.half
            || image.cols != image_width.saturating_mul(image_height)
            || out_width == 0
            || out_height == 0
            || image_width % out_width != 0
            || image_height % out_height != 0
        {
            return Err("gpu_birefnet_image_to_patches shape mismatch".to_string());
        }
        let out_channels = image.rows * (image_width / out_width) * (image_height / out_height);
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(out_channels, out_width * out_height)?;
            gpu_check(unsafe {
                makepad_cuda_birefnet_image_to_patches_f32(
                    image.device_ptr()?,
                    out.device_ptr()?,
                    image_width as u32,
                    image_height as u32,
                    out_width as u32,
                    out_height as u32,
                    image.rows as u32,
                    backend.stream,
                )
            })?;
            Ok(out)
        })
    }

    pub fn gpu_birefnet_global_avg_pool(x: &GpuTensor) -> Result<GpuTensor, String> {
        if x.half || x.cols == 0 {
            return Err("gpu_birefnet_global_avg_pool shape mismatch".to_string());
        }
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(x.rows, 1)?;
            gpu_check(unsafe {
                makepad_cuda_birefnet_global_avg_pool_f32(
                    x.device_ptr()?,
                    out.device_ptr()?,
                    x.cols as u32,
                    x.rows as u32,
                    backend.stream,
                )
            })?;
            Ok(out)
        })
    }

    pub fn gpu_birefnet_broadcast(x: &GpuTensor, plane: usize) -> Result<GpuTensor, String> {
        if x.half || x.cols != 1 || plane == 0 {
            return Err("gpu_birefnet_broadcast shape mismatch".to_string());
        }
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(x.rows, plane)?;
            gpu_check(unsafe {
                makepad_cuda_birefnet_broadcast_f32(
                    x.device_ptr()?,
                    out.device_ptr()?,
                    plane as u32,
                    x.rows as u32,
                    backend.stream,
                )
            })?;
            Ok(out)
        })
    }

    pub fn gpu_birefnet_mul_sigmoid_mask(
        x: &GpuTensor,
        logits: &GpuTensor,
    ) -> Result<GpuTensor, String> {
        if x.half || logits.half || logits.rows != 1 || logits.cols != x.cols {
            return Err("gpu_birefnet_mul_sigmoid_mask shape mismatch".to_string());
        }
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(x.rows, x.cols)?;
            gpu_check(unsafe {
                makepad_cuda_birefnet_mul_sigmoid_mask_f32(
                    x.device_ptr()?,
                    logits.device_ptr()?,
                    out.device_ptr()?,
                    x.cols as u32,
                    x.rows as u32,
                    backend.stream,
                )
            })?;
            Ok(out)
        })
    }

    /// Fused, score-materialization-free Swin window attention. Inputs and
    /// output are `[windows*144, hidden]`; shifted-region labels are optional.
    #[allow(clippy::too_many_arguments)]
    pub fn gpu_birefnet_swin_attention(
        q: &GpuTensor,
        k: &GpuTensor,
        v: &GpuTensor,
        cache_namespace: &str,
        cache_key: &str,
        relative_bias: &[f32],
        regions: Option<&GpuTensor>,
        windows: usize,
        heads: usize,
        window_tokens: usize,
    ) -> Result<GpuTensor, String> {
        if q.half
            || k.half
            || v.half
            || q.rows != k.rows
            || q.rows != v.rows
            || q.cols != k.cols
            || q.cols != v.cols
            || q.rows != windows.saturating_mul(window_tokens)
            || heads == 0
            || q.cols % heads != 0
            || q.cols / heads != 32
            || relative_bias.len() != heads * window_tokens * window_tokens
            || regions.is_some_and(|r| r.rows * r.cols != q.rows)
        {
            return Err("gpu_birefnet_swin_attention shape mismatch".to_string());
        }
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let bias_key = format!("{cache_namespace}::{cache_key}::relbias");
            backend.cached_weight_buffer(
                &bias_key,
                relative_bias.len() * size_of::<f32>(),
                || {
                    let raw = unsafe {
                        std::slice::from_raw_parts(
                            relative_bias.as_ptr().cast::<u8>(),
                            relative_bias.len() * size_of::<f32>(),
                        )
                    };
                    Ok(raw.to_vec())
                },
            )?;
            let bias = backend
                .weight_buffers
                .get(&bias_key)
                .ok_or_else(|| format!("missing cached Swin bias {bias_key}"))?;
            let out = GpuTensor::from_pool(q.rows, q.cols)?;
            let region_ptr = match regions {
                Some(region) => region.device_ptr()?.cast::<u32>(),
                None => std::ptr::null(),
            };
            gpu_check(unsafe {
                makepad_cuda_birefnet_swin_attention_f32(
                    q.device_ptr()?,
                    k.device_ptr()?,
                    v.device_ptr()?,
                    bias.ptr.as_ptr().cast::<f32>(),
                    region_ptr,
                    out.device_ptr()?,
                    windows as u32,
                    heads as u32,
                    window_tokens as u32,
                    (q.cols / heads) as u32,
                    1.0 / (q.cols as f32 / heads as f32).sqrt(),
                    backend.stream,
                )
            })?;
            Ok(out)
        })
    }

    /// torchvision-compatible modulated deformable convolution. Sampling is
    /// fused into an f16 im2col slab and the channel reduction is one cuBLAS
    /// GEMM per slab chunk. The modulator input is raw logits; `2*sigmoid` is
    /// applied in the sampling kernel exactly once.
    #[allow(clippy::too_many_arguments)]
    pub fn gpu_birefnet_deform_conv2d_cached(
        x: &GpuTensor,
        offset: &GpuTensor,
        modulator: &GpuTensor,
        width: usize,
        height: usize,
        cache_namespace: &str,
        weight_cache_key: &str,
        weights: &[f32],
        bias: &[f32],
        out_channels: usize,
        kernel: usize,
    ) -> Result<GpuTensor, String> {
        let plane = width.saturating_mul(height);
        let kernel2 = kernel.saturating_mul(kernel);
        if x.half
            || offset.half
            || modulator.half
            || x.cols != plane
            || offset.rows != 2 * kernel2
            || offset.cols != plane
            || modulator.rows != kernel2
            || modulator.cols != plane
            || weights.len() != out_channels * x.rows * kernel2
            || bias.len() != out_channels
            || kernel == 0
            || kernel % 2 == 0
        {
            return Err("gpu_birefnet_deform_conv2d_cached shape mismatch".to_string());
        }
        let in_channels = x.rows;
        let k_total = in_channels * kernel2;
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let weight_key = format!("{cache_namespace}::{weight_cache_key}::deform-f16");
            backend.cached_weight_buffer(&weight_key, weights.len() * size_of::<u16>(), || {
                let packed: Vec<u16> = weights.iter().map(|&value| crate::quant::f32_to_f16(value)).collect();
                let raw = unsafe {
                    std::slice::from_raw_parts(
                        packed.as_ptr().cast::<u8>(),
                        packed.len() * size_of::<u16>(),
                    )
                };
                Ok(raw.to_vec())
            })?;
            let weight = backend
                .weight_buffers
                .get(&weight_key)
                .ok_or_else(|| format!("missing cached deform weight {weight_key}"))?;
            let max_chunk = (IM2COL_MAX_CHUNK_BYTES / (k_total * size_of::<u16>()))
                .max(1)
                .min(plane);
            let chunk_count = plane.div_ceil(max_chunk);
            let chunk_cap = plane.div_ceil(chunk_count);
            let slab = conv_scratch_ptr(0, chunk_cap * k_total * size_of::<u16>())?.cast::<u16>();
            let out = GpuTensor::from_pool(out_channels, plane)?;
            let alpha = 1.0f32;
            let beta = 0.0f32;
            let mut p0 = 0usize;
            while p0 < plane {
                let rows = chunk_cap.min(plane - p0);
                gpu_check(unsafe {
                    makepad_cuda_birefnet_deform_im2col_f32_to_f16(
                        x.device_ptr()?,
                        offset.device_ptr()?,
                        modulator.device_ptr()?,
                        slab,
                        width as u32,
                        height as u32,
                        in_channels as u32,
                        kernel as u32,
                        (kernel / 2) as u32,
                        p0 as u32,
                        rows as u32,
                        backend.stream,
                    )
                })?;
                unsafe {
                    crate::cublas_gemm_strided_batched_ex(
                        backend.blas,
                        crate::CUBLAS_OP_N,
                        crate::CUBLAS_OP_N,
                        rows as i32,
                        out_channels as i32,
                        k_total as i32,
                        &alpha,
                        slab.cast::<std::ffi::c_void>(),
                        crate::CUDA_R_16F,
                        rows as i32,
                        0,
                        weight.ptr.as_ptr().cast::<std::ffi::c_void>(),
                        crate::CUDA_R_16F,
                        k_total as i32,
                        0,
                        &beta,
                        out.device_ptr()?.add(p0).cast::<std::ffi::c_void>(),
                        crate::CUDA_R_32F,
                        plane as i32,
                        0,
                        1,
                        crate::CUBLAS_COMPUTE_32F,
                        crate::CUBLAS_GEMM_DEFAULT,
                    )
                    .map_err(|err| format!("birefnet deform GEMM failed: {err}"))?;
                }
                p0 += rows;
            }
            let bias_key = format!("{cache_namespace}::{weight_cache_key}::deform-bias");
            backend.cached_weight_buffer(&bias_key, bias.len() * size_of::<f32>(), || {
                let raw = unsafe {
                    std::slice::from_raw_parts(
                        bias.as_ptr().cast::<u8>(),
                        bias.len() * size_of::<f32>(),
                    )
                };
                Ok(raw.to_vec())
            })?;
            let bias_buffer = backend
                .weight_buffers
                .get(&bias_key)
                .ok_or_else(|| format!("missing cached deform bias {bias_key}"))?;
            gpu_check(unsafe {
                makepad_cuda_add_planes_vec_f32(
                    out.device_ptr()?,
                    bias_buffer.ptr.as_ptr().cast::<f32>(),
                    plane as u32,
                    out_channels as u32,
                    backend.stream,
                )
            })?;
            gpu_prof(backend.stream, crate::prof::CAT_CONV2D, prof_start, 0);
            Ok(out)
        })
    }

    /// Implicit-GEMM planar conv (see gpu_conv2d_planar_cached).
    #[allow(clippy::too_many_arguments)]
    fn gpu_conv2d_planar_gemm(
        x: &GpuTensor,
        width: usize,
        height: usize,
        cache_namespace: &str,
        weight_cache_key: &str,
        weights: &[f32],
        bias: &[f32],
        out_channels: usize,
        kw: usize,
        kh: usize,
        pad_x: usize,
        pad_y: usize,
    ) -> Result<GpuTensor, String> {
        let in_channels = x.rows;
        let plane = width * height;
        let padded_width = width + 2 * pad_x;
        let padded_height = height + 2 * pad_y;
        let padded_plane = padded_width * padded_height;
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;

            // Per-shift (ic x oc) col-major f16 weight repack, device-cached.
            let qualified_key = format!("{cache_namespace}::{weight_cache_key}::g9f16");
            let repack_len = kw * kh * in_channels * out_channels;
            backend.cached_weight_buffer(&qualified_key, repack_len * size_of::<u16>(), || {
                let mut packed = vec![0u16; repack_len];
                for oc in 0..out_channels {
                    for ic in 0..in_channels {
                        for ky in 0..kh {
                            for kx in 0..kw {
                                let src = ((oc * in_channels + ic) * kh + ky) * kw + kx;
                                let shift = ky * kw + kx;
                                let dst = shift * in_channels * out_channels
                                    + oc * in_channels
                                    + ic;
                                packed[dst] = crate::quant::f32_to_f16(weights[src]);
                            }
                        }
                    }
                }
                let raw = unsafe {
                    std::slice::from_raw_parts(
                        packed.as_ptr().cast::<u8>(),
                        packed.len() * size_of::<u16>(),
                    )
                };
                Ok(raw.to_vec())
            })?;

            // Zero-padded f16 input, with one spare plane of slack: the
            // largest shift reads up to (kh-1)*padded_width + (kw-1) elements
            // past a channel's plane. Interior spills read the next channel's
            // (valid) data and the last channel spills into the spare plane —
            // all of which only feeds accumulator rows that get discarded.
            let padded_ptr =
                conv_scratch_ptr(0, (in_channels + 1) * padded_plane * size_of::<u16>())?;
            let status = unsafe {
                makepad_cuda_pad_planar_f32_to_f16(
                    x.device_ptr()?,
                    padded_ptr.cast::<u16>(),
                    width as u32,
                    height as u32,
                    in_channels as u32,
                    pad_x as u32,
                    pad_y as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;

            // Padded-plane accumulator: one LARGE gemm per kernel shift
            // (m = padded_plane) instead of per-output-row batches — cuBLAS
            // runs these at real tensor-core rates.
            let acc_ptr = conv_scratch_ptr(1, out_channels * padded_plane * size_of::<f32>())?
                .cast::<f32>();
            let weight = backend
                .weight_buffers
                .get(&qualified_key)
                .ok_or_else(|| format!("missing cached CUDA conv repack buffer {qualified_key}"))?;
            let alpha = 1.0f32;
            for ky in 0..kh {
                for kx in 0..kw {
                    let shift = ky * kw + kx;
                    let beta = if shift == 0 { 0.0f32 } else { 1.0f32 };
                    let a_offset_elems = ky * padded_width + kx;
                    let b_offset_elems = shift * in_channels * out_channels;
                    unsafe {
                        let a_ptr = padded_ptr
                            .cast::<u16>()
                            .add(a_offset_elems)
                            .cast::<std::ffi::c_void>();
                        let b_ptr = weight
                            .ptr
                            .as_ptr()
                            .cast::<u16>()
                            .add(b_offset_elems)
                            .cast::<std::ffi::c_void>();
                        // C (padded_plane x oc, ldc=padded_plane) +=
                        //   A_shift (padded_plane x ic, lda=padded_plane)
                        //   * W_shift (ic x oc, ldb=ic)
                        crate::cublas_gemm_strided_batched_ex(
                            backend.blas,
                            crate::CUBLAS_OP_N,
                            crate::CUBLAS_OP_N,
                            padded_plane as i32,
                            out_channels as i32,
                            in_channels as i32,
                            &alpha,
                            a_ptr,
                            crate::CUDA_R_16F,
                            padded_plane as i32,
                            0,
                            b_ptr,
                            crate::CUDA_R_16F,
                            in_channels as i32,
                            0,
                            &beta,
                            acc_ptr.cast::<std::ffi::c_void>(),
                            crate::CUDA_R_32F,
                            padded_plane as i32,
                            0,
                            1,
                            crate::CUBLAS_COMPUTE_32F,
                            crate::CUBLAS_GEMM_DEFAULT,
                        )
                        .map_err(|err| {
                            format!(
                                "gpu_conv2d gemm failed (shift {shift}, w={width} oc={out_channels} ic={in_channels}): {err}"
                            )
                        })?;
                    }
                }
            }

            let out = GpuTensor::from_pool(out_channels, plane)?;
            let bias_buf = gpu_upload_small(backend, bias)?;
            let status = unsafe {
                makepad_cuda_conv_extract_bias_f32(
                    acc_ptr,
                    bias_buf.ptr.as_ptr().cast::<f32>(),
                    out.device_ptr()?,
                    width as u32,
                    height as u32,
                    padded_width as u32,
                    padded_plane as u32,
                    out_channels as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_pool_release(bias_buf);
            gpu_prof(backend.stream, crate::prof::CAT_CONV2D, prof_start, 0);
            Ok(out)
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn gpu_group_norm_planar(
        x: &GpuTensor,
        width: usize,
        height: usize,
        groups: usize,
        cache_namespace: &str,
        cache_key: &str,
        gamma: &[f32],
        beta: &[f32],
        eps: f32,
    ) -> Result<GpuTensor, String> {
        let channels = x.rows;
        if groups == 0 || channels % groups != 0 || gamma.len() != channels || beta.len() != channels
        {
            return Err("gpu_group_norm shape mismatch".to_string());
        }
        if x.cols != width * height {
            return Err("gpu_group_norm plane mismatch".to_string());
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            // gamma/beta are model-constant: cache them on device as one
            // [gamma || beta] buffer instead of re-uploading every call.
            let vec_key = format!("{cache_namespace}::{cache_key}::gn");
            let vec_bytes = 2 * channels * size_of::<f32>();
            backend.cached_weight_buffer(&vec_key, vec_bytes, || {
                let mut concat = Vec::with_capacity(2 * channels);
                concat.extend_from_slice(gamma);
                concat.extend_from_slice(beta);
                let raw = unsafe {
                    std::slice::from_raw_parts(concat.as_ptr().cast::<u8>(), vec_bytes)
                };
                Ok(raw.to_vec())
            })?;
            // Spread each group's statistics over enough blocks to fill the
            // device (the single-block-per-group kernel serialized ~4M
            // elements per block at the big decode planes), then combine.
            let group_elems = width * height * (channels / groups);
            let chunk_count = group_elems.div_ceil(256 * 64).clamp(1, 1024);
            let partials_buf =
                gpu_pool_acquire(groups * chunk_count * 2 * size_of::<f64>())?;
            let stats_buf = gpu_pool_acquire(groups * 2 * size_of::<f32>())?;
            let out = GpuTensor::from_pool(channels, x.cols)?;
            let vec_buf = backend
                .weight_buffers
                .get(&vec_key)
                .ok_or_else(|| format!("missing cached CUDA group norm buffer {vec_key}"))?;
            let gamma_ptr = vec_buf.ptr.as_ptr().cast::<f32>();
            let status = unsafe {
                makepad_cuda_group_norm_planar_multi_f32(
                    x.device_ptr()?,
                    gamma_ptr,
                    gamma_ptr.add(channels),
                    partials_buf.ptr.as_ptr().cast::<f64>(),
                    stats_buf.ptr.as_ptr().cast::<f32>(),
                    out.device_ptr()?,
                    width as u32,
                    height as u32,
                    channels as u32,
                    groups as u32,
                    chunk_count as u32,
                    eps,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            gpu_pool_release(partials_buf);
            gpu_pool_release(stats_buf);
            gpu_prof(backend.stream, crate::prof::CAT_GROUP_NORM, prof_start, 0);
            Ok(out)
        })
    }

    pub fn gpu_upsample_nearest2x(
        x: &GpuTensor,
        width: usize,
        height: usize,
    ) -> Result<GpuTensor, String> {
        if x.cols != width * height {
            return Err("gpu_upsample plane mismatch".to_string());
        }
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let out = GpuTensor::from_pool(x.rows, width * height * 4)?;
            let status = unsafe {
                makepad_cuda_upsample2x_planar_f32(
                    x.device_ptr()?,
                    out.device_ptr()?,
                    width as u32,
                    height as u32,
                    x.rows as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            Ok(out)
        })
    }

    /// Single-head self attention on planar [c][token] data (the VAE mid
    /// block): scores = qᵀk * scale (softmax over keys), out = v·probs.
    /// Column-major views mean zero transpose passes.
    pub fn gpu_attention_planar_single(
        q: &GpuTensor,
        k: &GpuTensor,
        v: &GpuTensor,
        scale: f32,
    ) -> Result<GpuTensor, String> {
        let channels = q.rows;
        let seq = q.cols;
        if k.rows != channels || v.rows != channels || k.cols != seq || v.cols != seq {
            return Err("gpu_attention_planar shape mismatch".to_string());
        }
        let prof_start = std::time::Instant::now();
        with_dense_linear_backend(|backend| {
            backend.prepare_device()?;
            let scores_len = seq
                .checked_mul(seq)
                .ok_or_else(|| "gpu_attention_planar scores overflow".to_string())?;
            let scores = gpu_pool_acquire(scores_len * size_of::<f32>())?;
            let scores_ptr = scores.ptr.as_ptr().cast::<f32>();
            let beta = 0.0f32;
            let one = 1.0f32;
            unsafe {
                // scores[j][i] = scale * sum_d k[d][j] * q[d][i]
                // planar (seq x c) col-major with ld = seq: A=k opN, B=q opT.
                crate::cublas_gemm_strided_batched_ex(
                    backend.blas,
                    crate::CUBLAS_OP_N,
                    crate::CUBLAS_OP_T,
                    seq as i32,
                    seq as i32,
                    channels as i32,
                    &scale,
                    k.device_ptr()?.cast::<std::ffi::c_void>(),
                    crate::CUDA_R_32F,
                    seq as i32,
                    0,
                    q.device_ptr()?.cast::<std::ffi::c_void>(),
                    crate::CUDA_R_32F,
                    seq as i32,
                    0,
                    &beta,
                    scores_ptr.cast::<std::ffi::c_void>(),
                    crate::CUDA_R_32F,
                    seq as i32,
                    0,
                    1,
                    crate::CUBLAS_COMPUTE_32F,
                    crate::CUBLAS_GEMM_DEFAULT,
                )
                .map_err(|err| format!("gpu_attention_planar qk gemm failed: {err}"))?;
            }
            let status = unsafe {
                makepad_cuda_softmax_rows_precise_f32(
                    scores_ptr,
                    scores_ptr,
                    seq as u32,
                    seq as u32,
                    seq as u32,
                    backend.stream,
                )
            };
            gpu_check(status)?;
            let out = GpuTensor::from_pool(channels, seq)?;
            unsafe {
                // out[d][i] = sum_j probs[j][i] * v[d][j]: A=probs opT, B=v opN
                // giving C (seq x c) col-major = planar out.
                crate::cublas_gemm_strided_batched_ex(
                    backend.blas,
                    crate::CUBLAS_OP_T,
                    crate::CUBLAS_OP_N,
                    seq as i32,
                    channels as i32,
                    seq as i32,
                    &one,
                    scores_ptr.cast::<std::ffi::c_void>(),
                    crate::CUDA_R_32F,
                    seq as i32,
                    0,
                    v.device_ptr()?.cast::<std::ffi::c_void>(),
                    crate::CUDA_R_32F,
                    seq as i32,
                    0,
                    &beta,
                    out.device_ptr()?.cast::<std::ffi::c_void>(),
                    crate::CUDA_R_32F,
                    seq as i32,
                    0,
                    1,
                    crate::CUBLAS_COMPUTE_32F,
                    crate::CUBLAS_GEMM_DEFAULT,
                )
                .map_err(|err| format!("gpu_attention_planar pv gemm failed: {err}"))?;
            }
            gpu_pool_release(scores);
            gpu_prof(backend.stream, crate::prof::CAT_ATTN_SOFTMAX_WS, prof_start, 0);
            Ok(out)
        })
    }

    pub struct CudaBuffer {
        inner: DeviceBuffer,
    }

    impl CudaBuffer {
        pub fn size_bytes(&self) -> usize {
            self.inner.size_bytes
        }

        pub fn device_u32_ptr(&self) -> *const u32 {
            self.inner.ptr.as_ptr().cast::<u32>()
        }

        pub fn device_u32_mut_ptr(&self) -> *mut u32 {
            self.inner.ptr.as_ptr().cast::<u32>()
        }
    }

    pub struct CudaRuntime {
        device: i32,
        stream: cudaStream_t,
        blas: crate::cublasHandle_t,
    }

    impl CudaRuntime {
        pub fn load() -> Result<Self, String> {
            let device_count = crate::device_count().map_err(|err| err.to_string())?;
            if device_count <= 0 {
                return Err("CUDA reported zero devices".to_string());
            }
            let device = 0;
            crate::set_device(device).map_err(|err| err.to_string())?;
            let stream =
                crate::create_non_blocking_stream().map_err(|err| err.to_string())?;
            let blas = match crate::cublas_create() {
                Ok(handle) => handle,
                Err(err) => {
                    let _ = crate::destroy_stream(stream);
                    return Err(format!("cuBLAS create failed: {err}"));
                }
            };
            if let Err(err) = crate::cublas_set_stream(blas, stream) {
                let _ = crate::cublas_destroy(blas);
                let _ = crate::destroy_stream(stream);
                return Err(format!("cuBLAS set stream failed: {err}"));
            }
            Ok(Self {
                device,
                stream,
                blas,
            })
        }

        fn prepare_device(&self) -> Result<(), String> {
            crate::set_device(self.device).map_err(|err| err.to_string())
        }

        pub fn alloc_bytes(&self, size_bytes: usize) -> Result<CudaBuffer, String> {
            self.prepare_device()?;
            Ok(CudaBuffer {
                inner: DeviceBuffer::new(size_bytes)?,
            })
        }

        pub fn alloc_f32(&self, len: usize) -> Result<CudaBuffer, String> {
            self.alloc_bytes(
                len.checked_mul(size_of::<f32>())
                    .ok_or_else(|| "CUDA f32 buffer size overflow".to_string())?,
            )
        }

        pub fn alloc_u32(&self, len: usize) -> Result<CudaBuffer, String> {
            self.alloc_bytes(
                len.checked_mul(size_of::<u32>())
                    .ok_or_else(|| "CUDA u32 buffer size overflow".to_string())?,
            )
        }

        pub fn alloc_mapped_u32(&self, len: usize) -> Result<CudaMappedHostU32Buffer, String> {
            self.prepare_device()?;
            CudaMappedHostU32Buffer::new(len)
        }

        pub fn load_bytes(&self, bytes: &[u8]) -> Result<CudaBuffer, String> {
            let buffer = self.alloc_bytes(bytes.len())?;
            self.write_bytes(&buffer, bytes)?;
            Ok(buffer)
        }

        pub fn write_bytes(&self, buffer: &CudaBuffer, bytes: &[u8]) -> Result<(), String> {
            self.prepare_device()?;
            buffer.inner.write(bytes, self.stream)
        }

        pub fn zero_bytes(&self, buffer: &CudaBuffer, len: usize) -> Result<(), String> {
            self.prepare_device()?;
            if len > buffer.inner.size_bytes {
                return Err(format!(
                    "CUDA buffer overflow on zero: {} > {}",
                    len, buffer.inner.size_bytes
                ));
            }
            unsafe { crate::memset_async(buffer.inner.ptr, 0, len, self.stream) }
                .map_err(|err| err.to_string())
        }

        pub fn write_u32(&self, buffer: &CudaBuffer, value: u32) -> Result<(), String> {
            self.prepare_device()?;
            buffer.inner.write(&value.to_le_bytes(), self.stream)
        }

        pub fn read_u32(&self, buffer: &CudaBuffer) -> Result<u32, String> {
            self.prepare_device()?;
            buffer
                .inner
                .read_u32s(1, self.stream)?
                .into_iter()
                .next()
                .ok_or_else(|| "missing CUDA u32 readback value".to_string())
        }

        pub fn read_u32s(&self, buffer: &CudaBuffer, len: usize) -> Result<Vec<u32>, String> {
            self.prepare_device()?;
            buffer.inner.read_u32s(len, self.stream)
        }

        pub fn read_f32s(&self, buffer: &CudaBuffer, len: usize) -> Result<Vec<f32>, String> {
            self.prepare_device()?;
            buffer.inner.read_f32s(len, self.stream)
        }

        pub fn read_f32s_offset(
            &self,
            buffer: &CudaBuffer,
            offset_elems: usize,
            len: usize,
        ) -> Result<Vec<f32>, String> {
            self.prepare_device()?;
            let mut out = vec![0.0f32; len];
            let byte_len = len
                .checked_mul(std::mem::size_of::<f32>())
                .ok_or_else(|| "CUDA read_f32s_offset byte size overflow".to_string())?;
            unsafe {
                let src = std::ptr::NonNull::new_unchecked(
                    buffer
                        .inner
                        .ptr
                        .as_ptr()
                        .cast::<f32>()
                        .add(offset_elems)
                        .cast::<c_void>(),
                );
                crate::memcpy_async_device_to_host(
                    out.as_mut_ptr().cast::<c_void>(),
                    src,
                    byte_len,
                    self.stream,
                )
                .map_err(|err| err.to_string())?;
            }
            self.synchronize()?;
            Ok(out)
        }

        pub fn read_bytes(&self, buffer: &CudaBuffer, len: usize) -> Result<Vec<u8>, String> {
            self.prepare_device()?;
            buffer.inner.read_bytes(len, self.stream)
        }

        pub fn matmul_nt_f32(
            &self,
            a: &CudaBuffer,
            bt: &CudaBuffer,
            out: &CudaBuffer,
            m: usize,
            k: usize,
            n: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let alpha = 1.0f32;
            let beta = 0.0f32;
            crate::cublas_sgemm(
                self.blas,
                crate::CUBLAS_OP_T,
                crate::CUBLAS_OP_N,
                n as i32,
                m as i32,
                k as i32,
                &alpha,
                bt.inner.ptr.as_ptr().cast::<f32>(),
                k as i32,
                a.inner.ptr.as_ptr().cast::<f32>(),
                k as i32,
                &beta,
                out.inner.ptr.as_ptr().cast::<f32>(),
                n as i32,
            )
            .map_err(|err| format!("cuBLAS matmul_nt_f32 failed: m={m} k={k} n={n}: {err}"))
        }

        pub fn matmul_nn_f32(
            &self,
            a: &CudaBuffer,
            b: &CudaBuffer,
            out: &CudaBuffer,
            m: usize,
            k: usize,
            n: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let alpha = 1.0f32;
            let beta = 0.0f32;
            crate::cublas_sgemm(
                self.blas,
                crate::CUBLAS_OP_N,
                crate::CUBLAS_OP_N,
                n as i32,
                m as i32,
                k as i32,
                &alpha,
                b.inner.ptr.as_ptr().cast::<f32>(),
                n as i32,
                a.inner.ptr.as_ptr().cast::<f32>(),
                k as i32,
                &beta,
                out.inner.ptr.as_ptr().cast::<f32>(),
                n as i32,
            )
            .map_err(|err| format!("cuBLAS matmul_nn_f32 failed: m={m} k={k} n={n}: {err}"))
        }

        pub fn synchronize(&self) -> Result<(), String> {
            self.prepare_device()?;
            crate::synchronize_stream(self.stream).map_err(|err| err.to_string())
        }

        pub fn begin_capture(&self) -> Result<(), String> {
            self.prepare_device()?;
            crate::begin_stream_capture(
                self.stream,
                crate::CUDA_STREAM_CAPTURE_MODE_RELAXED,
            )
            .map_err(|err| err.to_string())
        }

        pub fn end_capture(&self) -> Result<CudaGraph, String> {
            self.prepare_device()?;
            crate::end_stream_capture(self.stream).map_err(|err| err.to_string())
        }

        pub fn launch_graph(&self, graph: &CudaGraphExec) -> Result<(), String> {
            self.prepare_device()?;
            graph.launch(self.stream).map_err(|err| err.to_string())
        }

        pub fn nvfp4_get_row_f32(
            &self,
            weights_nvfp4: &CudaBuffer,
            output_f32: &CudaBuffer,
            n_cols: usize,
            row_index: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_nvfp4_get_row_f32(
                    weights_nvfp4.inner.ptr.as_ptr().cast::<u8>(),
                    output_f32.inner.ptr.as_ptr().cast::<f32>(),
                    n_cols as u32,
                    row_index as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn nvfp4_get_row_f32_offset(
            &self,
            weights_nvfp4: &CudaBuffer,
            output_f32: &CudaBuffer,
            output_offset_elems: usize,
            n_cols: usize,
            row_index: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_nvfp4_get_row_f32(
                    weights_nvfp4.inner.ptr.as_ptr().cast::<u8>(),
                    output_f32
                        .inner
                        .ptr
                        .as_ptr()
                        .cast::<f32>()
                        .add(output_offset_elems),
                    n_cols as u32,
                    row_index as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn nvfp4_get_row_f32_device_u32(
            &self,
            weights_nvfp4: &CudaBuffer,
            output_f32: &CudaBuffer,
            n_cols: usize,
            row_index_device_u32: &CudaBuffer,
        ) -> Result<(), String> {
            self.nvfp4_get_row_f32_device_u32_ptr(
                weights_nvfp4,
                output_f32,
                n_cols,
                row_index_device_u32.inner.ptr.as_ptr().cast::<u32>(),
            )
        }

        pub fn nvfp4_get_row_f32_device_u32_ptr(
            &self,
            weights_nvfp4: &CudaBuffer,
            output_f32: &CudaBuffer,
            n_cols: usize,
            row_index_device_u32: *const u32,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_nvfp4_get_row_f32_device_u32(
                    weights_nvfp4.inner.ptr.as_ptr().cast::<u8>(),
                    output_f32.inner.ptr.as_ptr().cast::<f32>(),
                    n_cols as u32,
                    row_index_device_u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn nvfp4_get_rows_f32_device_u32(
            &self,
            weights_nvfp4: &CudaBuffer,
            row_indices_device_u32: &CudaBuffer,
            output_f32: &CudaBuffer,
            n_cols: usize,
            row_count: usize,
            output_row_stride: usize,
        ) -> Result<(), String> {
            self.nvfp4_get_rows_f32_device_u32_ptr(
                weights_nvfp4,
                row_indices_device_u32.inner.ptr.as_ptr().cast::<u32>(),
                output_f32,
                n_cols,
                row_count,
                output_row_stride,
            )
        }

        pub fn nvfp4_get_rows_f32_device_u32_ptr(
            &self,
            weights_nvfp4: &CudaBuffer,
            row_indices_device_u32: *const u32,
            output_f32: &CudaBuffer,
            n_cols: usize,
            row_count: usize,
            output_row_stride: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_nvfp4_get_rows_f32_device_u32(
                    weights_nvfp4.inner.ptr.as_ptr().cast::<u8>(),
                    row_indices_device_u32,
                    output_f32.inner.ptr.as_ptr().cast::<f32>(),
                    n_cols as u32,
                    row_count as u32,
                    output_row_stride as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn quantize_q8_1_f32(
            &self,
            input_f32: &CudaBuffer,
            output_q8_1: &CudaBuffer,
            n: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_quantize_q8_1_f32(
                    input_f32.inner.ptr.as_ptr().cast::<f32>(),
                    output_q8_1.inner.ptr.as_ptr().cast::<u8>(),
                    n as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn quantize_q8_1_mmq_f32(
            &self,
            input_f32: &CudaBuffer,
            output_q8_1_mmq: &CudaBuffer,
            n_cols: usize,
            n_rows: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_quantize_q8_1_mmq_f32(
                    input_f32.inner.ptr.as_ptr().cast::<f32>(),
                    output_q8_1_mmq.inner.ptr.as_ptr().cast::<u8>(),
                    n_cols as u32,
                    n_rows as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn quantize_q8_1_mmq_f32_padded(
            &self,
            input_f32: &CudaBuffer,
            output_q8_1_mmq: &CudaBuffer,
            n_cols: usize,
            n_rows: usize,
            padded_rows: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_quantize_q8_1_mmq_f32_padded(
                    input_f32.inner.ptr.as_ptr().cast::<f32>(),
                    output_q8_1_mmq.inner.ptr.as_ptr().cast::<u8>(),
                    n_cols as u32,
                    n_rows as u32,
                    padded_rows as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn nvfp4_q8_1_mmq_fixup_f32_len(&self) -> Result<usize, String> {
            self.prepare_device()?;
            let mut len = 0u32;
            let status = unsafe { makepad_cuda_nvfp4_q8_1_mmq_fixup_f32_len(&mut len) };
            crate::check(status).map_err(|err| err.to_string())?;
            Ok(len as usize)
        }

        pub fn quantize_nvfp4_f32(
            &self,
            input_f32: &CudaBuffer,
            input_scale: f32,
            output_nvfp4: &CudaBuffer,
            n: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_quantize_nvfp4_f32(
                    input_f32.inner.ptr.as_ptr().cast::<f32>(),
                    input_scale,
                    output_nvfp4.inner.ptr.as_ptr().cast::<u8>(),
                    n as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn nvfp4_q8_1_matvec(
            &self,
            input_q8_1: &CudaBuffer,
            packed_weights_nvfp4: &CudaBuffer,
            output_f32: &CudaBuffer,
            q8_1_blocks: usize,
            out_rows: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_nvfp4_q8_1_matvec(
                    input_q8_1.inner.ptr.as_ptr().cast::<u8>(),
                    packed_weights_nvfp4.inner.ptr.as_ptr().cast::<u8>(),
                    output_f32.inner.ptr.as_ptr().cast::<f32>(),
                    q8_1_blocks as u32,
                    out_rows as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn nvfp4_q8_1_matmul_batched(
            &self,
            input_q8_1: &CudaBuffer,
            packed_weights_nvfp4: &CudaBuffer,
            output_f32: &CudaBuffer,
            q8_1_blocks: usize,
            out_rows: usize,
            input_rows: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_nvfp4_q8_1_matmul(
                    input_q8_1.inner.ptr.as_ptr().cast::<u8>(),
                    packed_weights_nvfp4.inner.ptr.as_ptr().cast::<u8>(),
                    output_f32.inner.ptr.as_ptr().cast::<f32>(),
                    q8_1_blocks as u32,
                    out_rows as u32,
                    input_rows as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn nvfp4_nvfp4_matvec(
            &self,
            input_nvfp4: &CudaBuffer,
            packed_weights_nvfp4: &CudaBuffer,
            input_scale: f32,
            output_f32: &CudaBuffer,
            nvfp4_blocks: usize,
            out_rows: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_nvfp4_nvfp4_matvec(
                    input_nvfp4.inner.ptr.as_ptr().cast::<u8>(),
                    packed_weights_nvfp4.inner.ptr.as_ptr().cast::<u8>(),
                    input_scale,
                    output_f32.inner.ptr.as_ptr().cast::<f32>(),
                    nvfp4_blocks as u32,
                    out_rows as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn nvfp4_nvfp4_matmul_batched(
            &self,
            input_nvfp4: &CudaBuffer,
            packed_weights_nvfp4: &CudaBuffer,
            input_scale: f32,
            output_f32: &CudaBuffer,
            nvfp4_blocks: usize,
            out_rows: usize,
            input_rows: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_nvfp4_nvfp4_matmul(
                    input_nvfp4.inner.ptr.as_ptr().cast::<u8>(),
                    packed_weights_nvfp4.inner.ptr.as_ptr().cast::<u8>(),
                    input_scale,
                    output_f32.inner.ptr.as_ptr().cast::<f32>(),
                    nvfp4_blocks as u32,
                    out_rows as u32,
                    input_rows as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn nvfp4_q8_1_mmq_matmul_batched(
            &self,
            input_q8_1_mmq: &CudaBuffer,
            packed_weights_nvfp4: &CudaBuffer,
            output_f32: &CudaBuffer,
            tmp_fixup_f32: &CudaBuffer,
            tmp_fixup_f32_len: usize,
            n_cols: usize,
            out_rows: usize,
            input_rows: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_nvfp4_q8_1_mmq_matmul(
                    input_q8_1_mmq.inner.ptr.as_ptr().cast::<u8>(),
                    packed_weights_nvfp4.inner.ptr.as_ptr().cast::<u8>(),
                    output_f32.inner.ptr.as_ptr().cast::<f32>(),
                    tmp_fixup_f32.inner.ptr.as_ptr().cast::<f32>(),
                    tmp_fixup_f32_len as u32,
                    n_cols as u32,
                    out_rows as u32,
                    input_rows as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn scale_f32_inplace(
            &self,
            values: &CudaBuffer,
            scale: f32,
            n: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_scale_f32_inplace(
                    values.inner.ptr.as_ptr().cast::<f32>(),
                    scale,
                    n as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn scale_f32_inplace_device_f32_index(
            &self,
            values: &CudaBuffer,
            scales: &CudaBuffer,
            scale_index: usize,
            n: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_scale_f32_inplace_device_f32_index(
                    values.inner.ptr.as_ptr().cast::<f32>(),
                    scales.inner.ptr.as_ptr().cast::<f32>(),
                    scale_index as u32,
                    n as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn f32_to_bf16(
            &self,
            input: &CudaBuffer,
            output: &CudaBuffer,
            n: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_f32_to_bf16(
                    input.inner.ptr.as_ptr().cast::<f32>(),
                    output.inner.ptr.as_ptr().cast::<u16>(),
                    n as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn layer_norm_mul_add_f32(
            &self,
            input: &CudaBuffer,
            gamma: &CudaBuffer,
            beta: &CudaBuffer,
            output: &CudaBuffer,
            row_count: usize,
            cols: usize,
            eps: f32,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_layer_norm_mul_add_f32(
                    input.inner.ptr.as_ptr().cast::<f32>(),
                    gamma.inner.ptr.as_ptr().cast::<f32>(),
                    beta.inner.ptr.as_ptr().cast::<f32>(),
                    output.inner.ptr.as_ptr().cast::<f32>(),
                    row_count as u32,
                    cols as u32,
                    eps,
                    0.0,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn add_f32_precise(
            &self,
            left: &CudaBuffer,
            right: &CudaBuffer,
            output: &CudaBuffer,
            n: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_add_f32_precise(
                    left.inner.ptr.as_ptr().cast::<f32>(),
                    right.inner.ptr.as_ptr().cast::<f32>(),
                    output.inner.ptr.as_ptr().cast::<f32>(),
                    n as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn mul_f32_precise(
            &self,
            left: &CudaBuffer,
            right: &CudaBuffer,
            output: &CudaBuffer,
            n: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_mul_f32_precise(
                    left.inner.ptr.as_ptr().cast::<f32>(),
                    right.inner.ptr.as_ptr().cast::<f32>(),
                    output.inner.ptr.as_ptr().cast::<f32>(),
                    n as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn mul_rows_vec_f32(
            &self,
            input: &CudaBuffer,
            vec: &CudaBuffer,
            output: &CudaBuffer,
            row_count: usize,
            cols: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_mul_rows_vec_f32(
                    input.inner.ptr.as_ptr().cast::<f32>(),
                    vec.inner.ptr.as_ptr().cast::<f32>(),
                    output.inner.ptr.as_ptr().cast::<f32>(),
                    row_count as u32,
                    cols as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn gelu_f32_precise(
            &self,
            input: &CudaBuffer,
            output: &CudaBuffer,
            n: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_gelu_f32_precise(
                    input.inner.ptr.as_ptr().cast::<f32>(),
                    output.inner.ptr.as_ptr().cast::<f32>(),
                    n as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn softmax_rows_precise_f32(
            &self,
            logits: &CudaBuffer,
            probs: &CudaBuffer,
            row_count: usize,
            row_stride: usize,
            seq_len: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_softmax_rows_precise_f32(
                    logits.inner.ptr.as_ptr().cast::<f32>(),
                    probs.inner.ptr.as_ptr().cast::<f32>(),
                    row_count as u32,
                    row_stride as u32,
                    seq_len as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn conv2d_planar_f32(
            &self,
            input: &CudaBuffer,
            weights: &CudaBuffer,
            bias: &CudaBuffer,
            output: &CudaBuffer,
            width: usize,
            height: usize,
            in_channels: usize,
            out_channels: usize,
            kw: usize,
            kh: usize,
            pad_x: usize,
            pad_y: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_conv2d_planar_f32(
                    input.inner.ptr.as_ptr().cast::<f32>(),
                    weights.inner.ptr.as_ptr().cast::<f32>(),
                    bias.inner.ptr.as_ptr().cast::<f32>(),
                    output.inner.ptr.as_ptr().cast::<f32>(),
                    width as u32,
                    height as u32,
                    in_channels as u32,
                    out_channels as u32,
                    kw as u32,
                    kh as u32,
                    pad_x as u32,
                    pad_y as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn group_norm_planar_f32(
            &self,
            input: &CudaBuffer,
            gamma: &CudaBuffer,
            beta: &CudaBuffer,
            stats: &CudaBuffer,
            output: &CudaBuffer,
            width: usize,
            height: usize,
            channels: usize,
            groups: usize,
            eps: f32,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_group_norm_planar_f32(
                    input.inner.ptr.as_ptr().cast::<f32>(),
                    gamma.inner.ptr.as_ptr().cast::<f32>(),
                    beta.inner.ptr.as_ptr().cast::<f32>(),
                    stats.inner.ptr.as_ptr().cast::<f32>(),
                    output.inner.ptr.as_ptr().cast::<f32>(),
                    width as u32,
                    height as u32,
                    channels as u32,
                    groups as u32,
                    eps,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn silu_f32_precise(
            &self,
            input: &CudaBuffer,
            output: &CudaBuffer,
            n: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_silu_f32_precise(
                    input.inner.ptr.as_ptr().cast::<f32>(),
                    output.inner.ptr.as_ptr().cast::<f32>(),
                    n as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn affine_qmv_bf16_to_f32(
            &self,
            input_bf16: &CudaBuffer,
            packed_weights_u32: &CudaBuffer,
            scales_bf16: &CudaBuffer,
            biases_bf16: &CudaBuffer,
            output_f32: &CudaBuffer,
            n_in: usize,
            weight_words_per_row: usize,
            qparams_per_row: usize,
            out_rows: usize,
            bits: u32,
        ) -> Result<(), String> {
            self.affine_qmv_bf16_to_f32_offsets(
                input_bf16,
                packed_weights_u32,
                0,
                scales_bf16,
                0,
                biases_bf16,
                0,
                output_f32,
                n_in,
                weight_words_per_row,
                qparams_per_row,
                out_rows,
                bits,
            )
        }

        #[allow(clippy::too_many_arguments)]
        pub fn affine_qmv_bf16_to_f32_offsets(
            &self,
            input_bf16: &CudaBuffer,
            packed_weights_u32: &CudaBuffer,
            packed_weight_word_offset: usize,
            scales_bf16: &CudaBuffer,
            scale_word_offset: usize,
            biases_bf16: &CudaBuffer,
            bias_word_offset: usize,
            output_f32: &CudaBuffer,
            n_in: usize,
            weight_words_per_row: usize,
            qparams_per_row: usize,
            out_rows: usize,
            bits: u32,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_affine_qmv_f32(
                    input_bf16.inner.ptr.as_ptr().cast::<u16>(),
                    packed_weights_u32
                        .inner
                        .ptr
                        .as_ptr()
                        .cast::<u32>()
                        .add(packed_weight_word_offset),
                    scales_bf16
                        .inner
                        .ptr
                        .as_ptr()
                        .cast::<u16>()
                        .add(scale_word_offset),
                    biases_bf16
                        .inner
                        .ptr
                        .as_ptr()
                        .cast::<u16>()
                        .add(bias_word_offset),
                    output_f32.inner.ptr.as_ptr().cast::<f32>(),
                    n_in as u32,
                    weight_words_per_row as u32,
                    qparams_per_row as u32,
                    out_rows as u32,
                    bits,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn affine_qmv_bf16_to_f32_precise(
            &self,
            input_bf16: &CudaBuffer,
            packed_weights_u32: &CudaBuffer,
            scales_bf16: &CudaBuffer,
            biases_bf16: &CudaBuffer,
            output_f32: &CudaBuffer,
            n_in: usize,
            weight_words_per_row: usize,
            qparams_per_row: usize,
            out_rows: usize,
            bits: u32,
        ) -> Result<(), String> {
            self.affine_qmv_bf16_to_f32_offsets_precise(
                input_bf16,
                packed_weights_u32,
                0,
                scales_bf16,
                0,
                biases_bf16,
                0,
                output_f32,
                n_in,
                weight_words_per_row,
                qparams_per_row,
                out_rows,
                bits,
            )
        }

        #[allow(clippy::too_many_arguments)]
        pub fn affine_qmv_bf16_to_f32_offsets_precise(
            &self,
            input_bf16: &CudaBuffer,
            packed_weights_u32: &CudaBuffer,
            packed_weight_word_offset: usize,
            scales_bf16: &CudaBuffer,
            scale_word_offset: usize,
            biases_bf16: &CudaBuffer,
            bias_word_offset: usize,
            output_f32: &CudaBuffer,
            n_in: usize,
            weight_words_per_row: usize,
            qparams_per_row: usize,
            out_rows: usize,
            bits: u32,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_affine_qmv_f32_precise(
                    input_bf16.inner.ptr.as_ptr().cast::<u16>(),
                    packed_weights_u32
                        .inner
                        .ptr
                        .as_ptr()
                        .cast::<u32>()
                        .add(packed_weight_word_offset),
                    scales_bf16
                        .inner
                        .ptr
                        .as_ptr()
                        .cast::<u16>()
                        .add(scale_word_offset),
                    biases_bf16
                        .inner
                        .ptr
                        .as_ptr()
                        .cast::<u16>()
                        .add(bias_word_offset),
                    output_f32.inner.ptr.as_ptr().cast::<f32>(),
                    n_in as u32,
                    weight_words_per_row as u32,
                    qparams_per_row as u32,
                    out_rows as u32,
                    bits,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn affine_qmv_bf16_to_f32_rows_precise(
            &self,
            input_bf16: &CudaBuffer,
            packed_weights_u32: &CudaBuffer,
            scales_bf16: &CudaBuffer,
            biases_bf16: &CudaBuffer,
            output_f32: &CudaBuffer,
            n_in: usize,
            weight_words_per_row: usize,
            qparams_per_row: usize,
            out_rows: usize,
            input_rows: usize,
            bits: u32,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_affine_qmv_f32_rows_precise(
                    input_bf16.inner.ptr.as_ptr().cast::<u16>(),
                    packed_weights_u32.inner.ptr.as_ptr().cast::<u32>(),
                    scales_bf16.inner.ptr.as_ptr().cast::<u16>(),
                    biases_bf16.inner.ptr.as_ptr().cast::<u16>(),
                    output_f32.inner.ptr.as_ptr().cast::<f32>(),
                    n_in as u32,
                    weight_words_per_row as u32,
                    qparams_per_row as u32,
                    out_rows as u32,
                    input_rows as u32,
                    bits,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn affine_qmv_q8_1_to_f32_precise(
            &self,
            input_bf16: &CudaBuffer,
            input_q8_1: &CudaBuffer,
            packed_weights_u32: &CudaBuffer,
            packed_weight_word_offset: usize,
            scales_bf16: &CudaBuffer,
            scale_word_offset: usize,
            biases_bf16: &CudaBuffer,
            bias_word_offset: usize,
            output_f32: &CudaBuffer,
            n_in: usize,
            weight_words_per_row: usize,
            qparams_per_row: usize,
            out_rows: usize,
            bits: u32,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_affine_q8_1_qmv_f32_precise(
                    input_bf16.inner.ptr.as_ptr().cast::<u16>(),
                    input_q8_1.inner.ptr.as_ptr().cast::<u8>(),
                    packed_weights_u32
                        .inner
                        .ptr
                        .as_ptr()
                        .cast::<u32>()
                        .add(packed_weight_word_offset),
                    scales_bf16
                        .inner
                        .ptr
                        .as_ptr()
                        .cast::<u16>()
                        .add(scale_word_offset),
                    biases_bf16
                        .inner
                        .ptr
                        .as_ptr()
                        .cast::<u16>()
                        .add(bias_word_offset),
                    output_f32.inner.ptr.as_ptr().cast::<f32>(),
                    n_in as u32,
                    weight_words_per_row as u32,
                    qparams_per_row as u32,
                    out_rows as u32,
                    bits,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn affine_qmv_bf16_to_f32_select_plane_precise(
            &self,
            input_bf16: &CudaBuffer,
            packed_weights_u32: &CudaBuffer,
            scales_bf16: &CudaBuffer,
            biases_bf16: &CudaBuffer,
            plane_indices_u32: &CudaBuffer,
            plane_slot: usize,
            output_f32: &CudaBuffer,
            n_in: usize,
            weight_words_per_row: usize,
            qparams_per_row: usize,
            out_rows: usize,
            weight_words_per_plane: usize,
            qparams_words_per_plane: usize,
            plane_count: usize,
            bits: u32,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_affine_qmv_f32_select_plane_precise(
                    input_bf16.inner.ptr.as_ptr().cast::<u16>(),
                    packed_weights_u32.inner.ptr.as_ptr().cast::<u32>(),
                    scales_bf16.inner.ptr.as_ptr().cast::<u16>(),
                    biases_bf16.inner.ptr.as_ptr().cast::<u16>(),
                    plane_indices_u32.inner.ptr.as_ptr().cast::<u32>(),
                    plane_slot as u32,
                    output_f32.inner.ptr.as_ptr().cast::<f32>(),
                    n_in as u32,
                    weight_words_per_row as u32,
                    qparams_per_row as u32,
                    out_rows as u32,
                    weight_words_per_plane as u32,
                    qparams_words_per_plane as u32,
                    plane_count as u32,
                    bits,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn affine_qmv_bf16_to_f32_select_plane_precise_offsets(
            &self,
            input_bf16: &CudaBuffer,
            input_word_offset: usize,
            packed_weights_u32: &CudaBuffer,
            scales_bf16: &CudaBuffer,
            biases_bf16: &CudaBuffer,
            plane_indices_u32: &CudaBuffer,
            plane_slot: usize,
            output_f32: &CudaBuffer,
            output_float_offset: usize,
            n_in: usize,
            weight_words_per_row: usize,
            qparams_per_row: usize,
            out_rows: usize,
            weight_words_per_plane: usize,
            qparams_words_per_plane: usize,
            plane_count: usize,
            bits: u32,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_affine_qmv_f32_select_plane_precise(
                    input_bf16
                        .inner
                        .ptr
                        .as_ptr()
                        .cast::<u16>()
                        .add(input_word_offset),
                    packed_weights_u32.inner.ptr.as_ptr().cast::<u32>(),
                    scales_bf16.inner.ptr.as_ptr().cast::<u16>(),
                    biases_bf16.inner.ptr.as_ptr().cast::<u16>(),
                    plane_indices_u32.inner.ptr.as_ptr().cast::<u32>(),
                    plane_slot as u32,
                    output_f32
                        .inner
                        .ptr
                        .as_ptr()
                        .cast::<f32>()
                        .add(output_float_offset),
                    n_in as u32,
                    weight_words_per_row as u32,
                    qparams_per_row as u32,
                    out_rows as u32,
                    weight_words_per_plane as u32,
                    qparams_words_per_plane as u32,
                    plane_count as u32,
                    bits,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn affine_qmv_bf16_to_f32_select_plane_rows_precise(
            &self,
            input_bf16: &CudaBuffer,
            packed_weights_u32: &CudaBuffer,
            scales_bf16: &CudaBuffer,
            biases_bf16: &CudaBuffer,
            plane_indices_u32: &CudaBuffer,
            plane_indices_row_stride: usize,
            plane_slot: usize,
            output_f32: &CudaBuffer,
            n_in: usize,
            weight_words_per_row: usize,
            qparams_per_row: usize,
            out_rows: usize,
            input_rows: usize,
            weight_words_per_plane: usize,
            qparams_words_per_plane: usize,
            plane_count: usize,
            bits: u32,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_affine_qmv_f32_select_plane_rows_precise(
                    input_bf16.inner.ptr.as_ptr().cast::<u16>(),
                    packed_weights_u32.inner.ptr.as_ptr().cast::<u32>(),
                    scales_bf16.inner.ptr.as_ptr().cast::<u16>(),
                    biases_bf16.inner.ptr.as_ptr().cast::<u16>(),
                    plane_indices_u32.inner.ptr.as_ptr().cast::<u32>(),
                    plane_indices_row_stride as u32,
                    plane_slot as u32,
                    output_f32.inner.ptr.as_ptr().cast::<f32>(),
                    n_in as u32,
                    weight_words_per_row as u32,
                    qparams_per_row as u32,
                    out_rows as u32,
                    input_rows as u32,
                    weight_words_per_plane as u32,
                    qparams_words_per_plane as u32,
                    plane_count as u32,
                    bits,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn affine_qmv_bf16_to_f32_select_plane_rows_precise_offsets(
            &self,
            input_bf16: &CudaBuffer,
            packed_weights_u32: &CudaBuffer,
            scales_bf16: &CudaBuffer,
            biases_bf16: &CudaBuffer,
            plane_indices_u32: &CudaBuffer,
            plane_indices_row_stride: usize,
            plane_slot: usize,
            output_f32: &CudaBuffer,
            output_float_offset: usize,
            n_in: usize,
            weight_words_per_row: usize,
            qparams_per_row: usize,
            out_rows: usize,
            input_rows: usize,
            weight_words_per_plane: usize,
            qparams_words_per_plane: usize,
            plane_count: usize,
            bits: u32,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_affine_qmv_f32_select_plane_rows_precise(
                    input_bf16.inner.ptr.as_ptr().cast::<u16>(),
                    packed_weights_u32.inner.ptr.as_ptr().cast::<u32>(),
                    scales_bf16.inner.ptr.as_ptr().cast::<u16>(),
                    biases_bf16.inner.ptr.as_ptr().cast::<u16>(),
                    plane_indices_u32.inner.ptr.as_ptr().cast::<u32>(),
                    plane_indices_row_stride as u32,
                    plane_slot as u32,
                    output_f32
                        .inner
                        .ptr
                        .as_ptr()
                        .cast::<f32>()
                        .add(output_float_offset),
                    n_in as u32,
                    weight_words_per_row as u32,
                    qparams_per_row as u32,
                    out_rows as u32,
                    input_rows as u32,
                    weight_words_per_plane as u32,
                    qparams_words_per_plane as u32,
                    plane_count as u32,
                    bits,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn affine_qmv_bf16_to_f32_select_planes_precise(
            &self,
            input_bf16: &CudaBuffer,
            packed_weights_u32: &CudaBuffer,
            scales_bf16: &CudaBuffer,
            biases_bf16: &CudaBuffer,
            plane_indices_u32: &CudaBuffer,
            selected_count: usize,
            output_f32: &CudaBuffer,
            n_in: usize,
            weight_words_per_row: usize,
            qparams_per_row: usize,
            out_rows: usize,
            weight_words_per_plane: usize,
            qparams_words_per_plane: usize,
            plane_count: usize,
            bits: u32,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_affine_qmv_f32_select_planes_precise(
                    input_bf16.inner.ptr.as_ptr().cast::<u16>(),
                    packed_weights_u32.inner.ptr.as_ptr().cast::<u32>(),
                    scales_bf16.inner.ptr.as_ptr().cast::<u16>(),
                    biases_bf16.inner.ptr.as_ptr().cast::<u16>(),
                    plane_indices_u32.inner.ptr.as_ptr().cast::<u32>(),
                    selected_count as u32,
                    output_f32.inner.ptr.as_ptr().cast::<f32>(),
                    n_in as u32,
                    weight_words_per_row as u32,
                    qparams_per_row as u32,
                    out_rows as u32,
                    weight_words_per_plane as u32,
                    qparams_words_per_plane as u32,
                    plane_count as u32,
                    bits,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn affine_qmv_bf16_to_f32_select_planes_fixed8_known_valid_precise(
            &self,
            input_bf16: &CudaBuffer,
            packed_weights_u32: &CudaBuffer,
            scales_bf16: &CudaBuffer,
            biases_bf16: &CudaBuffer,
            plane_indices_u32: &CudaBuffer,
            output_f32: &CudaBuffer,
            n_in: usize,
            weight_words_per_row: usize,
            qparams_per_row: usize,
            out_rows: usize,
            weight_words_per_plane: usize,
            qparams_words_per_plane: usize,
            bits: u32,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_affine_qmv_f32_select_planes_fixed8_known_valid_precise(
                    input_bf16.inner.ptr.as_ptr().cast::<u16>(),
                    packed_weights_u32.inner.ptr.as_ptr().cast::<u32>(),
                    scales_bf16.inner.ptr.as_ptr().cast::<u16>(),
                    biases_bf16.inner.ptr.as_ptr().cast::<u16>(),
                    plane_indices_u32.inner.ptr.as_ptr().cast::<u32>(),
                    output_f32.inner.ptr.as_ptr().cast::<f32>(),
                    n_in as u32,
                    weight_words_per_row as u32,
                    qparams_per_row as u32,
                    out_rows as u32,
                    weight_words_per_plane as u32,
                    qparams_words_per_plane as u32,
                    bits,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn affine_qmv_bf16_to_f32_select_planes_input_offsets_precise(
            &self,
            input_bf16: &CudaBuffer,
            input_words_per_slot: usize,
            packed_weights_u32: &CudaBuffer,
            scales_bf16: &CudaBuffer,
            biases_bf16: &CudaBuffer,
            plane_indices_u32: &CudaBuffer,
            selected_count: usize,
            output_f32: &CudaBuffer,
            n_in: usize,
            weight_words_per_row: usize,
            qparams_per_row: usize,
            out_rows: usize,
            weight_words_per_plane: usize,
            qparams_words_per_plane: usize,
            plane_count: usize,
            bits: u32,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_affine_qmv_f32_select_planes_input_offsets_precise(
                    input_bf16.inner.ptr.as_ptr().cast::<u16>(),
                    input_words_per_slot as u32,
                    packed_weights_u32.inner.ptr.as_ptr().cast::<u32>(),
                    scales_bf16.inner.ptr.as_ptr().cast::<u16>(),
                    biases_bf16.inner.ptr.as_ptr().cast::<u16>(),
                    plane_indices_u32.inner.ptr.as_ptr().cast::<u32>(),
                    selected_count as u32,
                    output_f32.inner.ptr.as_ptr().cast::<f32>(),
                    n_in as u32,
                    weight_words_per_row as u32,
                    qparams_per_row as u32,
                    out_rows as u32,
                    weight_words_per_plane as u32,
                    qparams_words_per_plane as u32,
                    plane_count as u32,
                    bits,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn affine_qmv_bf16_to_f32_select_planes_input_offsets_fixed8_known_valid_precise(
            &self,
            input_bf16: &CudaBuffer,
            input_words_per_slot: usize,
            packed_weights_u32: &CudaBuffer,
            scales_bf16: &CudaBuffer,
            biases_bf16: &CudaBuffer,
            plane_indices_u32: &CudaBuffer,
            output_f32: &CudaBuffer,
            n_in: usize,
            weight_words_per_row: usize,
            qparams_per_row: usize,
            out_rows: usize,
            weight_words_per_plane: usize,
            qparams_words_per_plane: usize,
            bits: u32,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_affine_qmv_f32_select_planes_input_offsets_fixed8_known_valid_precise(
                    input_bf16.inner.ptr.as_ptr().cast::<u16>(),
                    input_words_per_slot as u32,
                    packed_weights_u32.inner.ptr.as_ptr().cast::<u32>(),
                    scales_bf16.inner.ptr.as_ptr().cast::<u16>(),
                    biases_bf16.inner.ptr.as_ptr().cast::<u16>(),
                    plane_indices_u32.inner.ptr.as_ptr().cast::<u32>(),
                    output_f32.inner.ptr.as_ptr().cast::<f32>(),
                    n_in as u32,
                    weight_words_per_row as u32,
                    qparams_per_row as u32,
                    out_rows as u32,
                    weight_words_per_plane as u32,
                    qparams_words_per_plane as u32,
                    bits,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn affine_get_row_f32(
            &self,
            packed_weights_u32: &CudaBuffer,
            scales_bf16: &CudaBuffer,
            biases_bf16: &CudaBuffer,
            output_f32: &CudaBuffer,
            weight_words_per_row: usize,
            qparams_per_row: usize,
            row_index: usize,
            bits: u32,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_affine_get_row_f32(
                    packed_weights_u32.inner.ptr.as_ptr().cast::<u32>(),
                    scales_bf16.inner.ptr.as_ptr().cast::<u16>(),
                    biases_bf16.inner.ptr.as_ptr().cast::<u16>(),
                    output_f32.inner.ptr.as_ptr().cast::<f32>(),
                    weight_words_per_row as u32,
                    qparams_per_row as u32,
                    row_index as u32,
                    bits,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn affine_get_row_f32_device_u32(
            &self,
            packed_weights_u32: &CudaBuffer,
            scales_bf16: &CudaBuffer,
            biases_bf16: &CudaBuffer,
            output_f32: &CudaBuffer,
            weight_words_per_row: usize,
            qparams_per_row: usize,
            row_index_device_u32: &CudaBuffer,
            bits: u32,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_affine_get_row_f32_device_u32(
                    packed_weights_u32.inner.ptr.as_ptr().cast::<u32>(),
                    scales_bf16.inner.ptr.as_ptr().cast::<u16>(),
                    biases_bf16.inner.ptr.as_ptr().cast::<u16>(),
                    output_f32.inner.ptr.as_ptr().cast::<f32>(),
                    weight_words_per_row as u32,
                    qparams_per_row as u32,
                    row_index_device_u32.inner.ptr.as_ptr().cast::<u32>(),
                    bits,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn affine_get_row_f32_device_u32_ptr(
            &self,
            packed_weights_u32: &CudaBuffer,
            scales_bf16: &CudaBuffer,
            biases_bf16: &CudaBuffer,
            output_f32: &CudaBuffer,
            weight_words_per_row: usize,
            qparams_per_row: usize,
            row_index_device_u32: *const u32,
            bits: u32,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_affine_get_row_f32_device_u32(
                    packed_weights_u32.inner.ptr.as_ptr().cast::<u32>(),
                    scales_bf16.inner.ptr.as_ptr().cast::<u16>(),
                    biases_bf16.inner.ptr.as_ptr().cast::<u16>(),
                    output_f32.inner.ptr.as_ptr().cast::<f32>(),
                    weight_words_per_row as u32,
                    qparams_per_row as u32,
                    row_index_device_u32,
                    bits,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn add_f32(
            &self,
            left: &CudaBuffer,
            right: &CudaBuffer,
            out: &CudaBuffer,
            n: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_add_f32(
                    left.inner.ptr.as_ptr().cast::<f32>(),
                    right.inner.ptr.as_ptr().cast::<f32>(),
                    out.inner.ptr.as_ptr().cast::<f32>(),
                    n as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn copy_f32(
            &self,
            input: &CudaBuffer,
            input_offset_elems: usize,
            output: &CudaBuffer,
            output_offset_elems: usize,
            n: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_copy_f32(
                    input.inner.ptr.as_ptr().cast::<f32>().add(input_offset_elems),
                    output
                        .inner
                        .ptr
                        .as_ptr()
                        .cast::<f32>()
                        .add(output_offset_elems),
                    n as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn weighted_sum_rows_f32(
            &self,
            batched_inputs: &CudaBuffer,
            weights: &CudaBuffer,
            output: &CudaBuffer,
            row_count: usize,
            input_count: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_weighted_sum_rows_f32(
                    batched_inputs.inner.ptr.as_ptr().cast::<f32>(),
                    weights.inner.ptr.as_ptr().cast::<f32>(),
                    output.inner.ptr.as_ptr().cast::<f32>(),
                    row_count as u32,
                    input_count as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn weighted_sum_rows_grouped_f32(
            &self,
            batched_inputs: &CudaBuffer,
            weights: &CudaBuffer,
            output: &CudaBuffer,
            row_count: usize,
            row_stride: usize,
            input_count: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_weighted_sum_rows_grouped_f32(
                    batched_inputs.inner.ptr.as_ptr().cast::<f32>(),
                    weights.inner.ptr.as_ptr().cast::<f32>(),
                    output.inner.ptr.as_ptr().cast::<f32>(),
                    row_count as u32,
                    row_stride as u32,
                    input_count as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn add_scaled_rows_f32(
            &self,
            input: &CudaBuffer,
            scales: &CudaBuffer,
            output: &CudaBuffer,
            row_count: usize,
            row_stride: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_add_scaled_rows_f32(
                    input.inner.ptr.as_ptr().cast::<f32>(),
                    scales.inner.ptr.as_ptr().cast::<f32>(),
                    output.inner.ptr.as_ptr().cast::<f32>(),
                    row_count as u32,
                    row_stride as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn add_scaled_rows_f32_indexed(
            &self,
            input: &CudaBuffer,
            scales: &CudaBuffer,
            output: &CudaBuffer,
            row_count: usize,
            row_stride: usize,
            scale_row_stride: usize,
            scale_column: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_add_scaled_rows_f32_indexed(
                    input.inner.ptr.as_ptr().cast::<f32>(),
                    scales.inner.ptr.as_ptr().cast::<f32>(),
                    output.inner.ptr.as_ptr().cast::<f32>(),
                    row_count as u32,
                    row_stride as u32,
                    scale_row_stride as u32,
                    scale_column as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn mul_f32(
            &self,
            left: &CudaBuffer,
            right: &CudaBuffer,
            out: &CudaBuffer,
            n: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_mul_f32(
                    left.inner.ptr.as_ptr().cast::<f32>(),
                    right.inner.ptr.as_ptr().cast::<f32>(),
                    out.inner.ptr.as_ptr().cast::<f32>(),
                    n as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn gelu_f32(
            &self,
            input: &CudaBuffer,
            out: &CudaBuffer,
            n: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_gelu_f32(
                    input.inner.ptr.as_ptr().cast::<f32>(),
                    out.inner.ptr.as_ptr().cast::<f32>(),
                    n as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn geglu_split_f32(
            &self,
            gate_up: &CudaBuffer,
            out: &CudaBuffer,
            n: usize,
            split_offset: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_geglu_split_f32(
                    gate_up.inner.ptr.as_ptr().cast::<f32>(),
                    out.inner.ptr.as_ptr().cast::<f32>(),
                    n as u32,
                    split_offset as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn geglu_split_f32_rows(
            &self,
            gate_up: &CudaBuffer,
            out: &CudaBuffer,
            row_count: usize,
            row_stride: usize,
            n: usize,
            split_offset: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_geglu_split_f32_rows(
                    gate_up.inner.ptr.as_ptr().cast::<f32>(),
                    out.inner.ptr.as_ptr().cast::<f32>(),
                    row_count as u32,
                    row_stride as u32,
                    n as u32,
                    split_offset as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn ssm_conv_f32(
            &self,
            src0: &CudaBuffer,
            src1: &CudaBuffer,
            dst: &CudaBuffer,
            d_conv: usize,
            d_inner: usize,
            n_tokens: usize,
            n_seqs: usize,
            src0_token_stride: usize,
            src0_seq_stride: usize,
            src1_inner_stride: usize,
            dst_token_stride: usize,
            dst_seq_stride: usize,
            apply_silu: bool,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_ssm_conv_f32(
                    src0.inner.ptr.as_ptr().cast::<f32>(),
                    src1.inner.ptr.as_ptr().cast::<f32>(),
                    dst.inner.ptr.as_ptr().cast::<f32>(),
                    d_conv as u32,
                    d_inner as u32,
                    n_tokens as u32,
                    n_seqs as u32,
                    src0_token_stride as u32,
                    src0_seq_stride as u32,
                    src1_inner_stride as u32,
                    dst_token_stride as u32,
                    dst_seq_stride as u32,
                    u32::from(apply_silu),
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn gated_delta_net_f32(
            &self,
            q: &CudaBuffer,
            k: &CudaBuffer,
            v: &CudaBuffer,
            g: &CudaBuffer,
            beta: &CudaBuffer,
            state: &CudaBuffer,
            dst: &CudaBuffer,
            sv: usize,
            h: usize,
            n_tokens: usize,
            n_seqs: usize,
            sq1: usize,
            sq2: usize,
            sq3: usize,
            sv1: usize,
            sv2: usize,
            sv3: usize,
            sb1: usize,
            sb2: usize,
            sb3: usize,
            neqk1: usize,
            rq3: usize,
            kda: bool,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_gated_delta_net_f32(
                    q.inner.ptr.as_ptr().cast::<f32>(),
                    k.inner.ptr.as_ptr().cast::<f32>(),
                    v.inner.ptr.as_ptr().cast::<f32>(),
                    g.inner.ptr.as_ptr().cast::<f32>(),
                    beta.inner.ptr.as_ptr().cast::<f32>(),
                    state.inner.ptr.as_ptr().cast::<f32>(),
                    dst.inner.ptr.as_ptr().cast::<f32>(),
                    sv as u32,
                    h as u32,
                    n_tokens as u32,
                    n_seqs as u32,
                    sq1 as u32,
                    sq2 as u32,
                    sq3 as u32,
                    sv1 as u32,
                    sv2 as u32,
                    sv3 as u32,
                    sb1 as u32,
                    sb2 as u32,
                    sb3 as u32,
                    neqk1 as u32,
                    rq3 as u32,
                    u32::from(kda),
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn gated_delta_net_f32_state_offset(
            &self,
            q: &CudaBuffer,
            k: &CudaBuffer,
            v: &CudaBuffer,
            g: &CudaBuffer,
            beta: &CudaBuffer,
            state_and_dst: &CudaBuffer,
            state_offset_elems: usize,
            sv: usize,
            h: usize,
            n_tokens: usize,
            n_seqs: usize,
            sq1: usize,
            sq2: usize,
            sq3: usize,
            sv1: usize,
            sv2: usize,
            sv3: usize,
            sb1: usize,
            sb2: usize,
            sb3: usize,
            neqk1: usize,
            rq3: usize,
            kda: bool,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_gated_delta_net_f32(
                    q.inner.ptr.as_ptr().cast::<f32>(),
                    k.inner.ptr.as_ptr().cast::<f32>(),
                    v.inner.ptr.as_ptr().cast::<f32>(),
                    g.inner.ptr.as_ptr().cast::<f32>(),
                    beta.inner.ptr.as_ptr().cast::<f32>(),
                    state_and_dst
                        .inner
                        .ptr
                        .as_ptr()
                        .cast::<f32>()
                        .add(state_offset_elems),
                    state_and_dst.inner.ptr.as_ptr().cast::<f32>(),
                    sv as u32,
                    h as u32,
                    n_tokens as u32,
                    n_seqs as u32,
                    sq1 as u32,
                    sq2 as u32,
                    sq3 as u32,
                    sv1 as u32,
                    sv2 as u32,
                    sv3 as u32,
                    sb1 as u32,
                    sb2 as u32,
                    sb3 as u32,
                    neqk1 as u32,
                    rq3 as u32,
                    u32::from(kda),
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn rms_norm_row_weighted_f32(
            &self,
            input: &CudaBuffer,
            weights_bf16: &CudaBuffer,
            output: &CudaBuffer,
            n: usize,
            eps: f32,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_rms_norm_row_weighted_f32(
                    input.inner.ptr.as_ptr().cast::<f32>(),
                    weights_bf16.inner.ptr.as_ptr().cast::<u16>(),
                    output.inner.ptr.as_ptr().cast::<f32>(),
                    n as u32,
                    eps,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn rms_norm_row_weighted_f32_f32weights(
            &self,
            input: &CudaBuffer,
            weights_f32: &CudaBuffer,
            output: &CudaBuffer,
            n: usize,
            eps: f32,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_rms_norm_row_weighted_f32_f32weights(
                    input.inner.ptr.as_ptr().cast::<f32>(),
                    weights_f32.inner.ptr.as_ptr().cast::<f32>(),
                    output.inner.ptr.as_ptr().cast::<f32>(),
                    n as u32,
                    eps,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn rms_norm_row_weighted_f32_f32weights_precise(
            &self,
            input: &CudaBuffer,
            weights_f32: &CudaBuffer,
            output: &CudaBuffer,
            n: usize,
            eps: f32,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_rms_norm_row_weighted_f32_f32weights_precise(
                    input.inner.ptr.as_ptr().cast::<f32>(),
                    weights_f32.inner.ptr.as_ptr().cast::<f32>(),
                    output.inner.ptr.as_ptr().cast::<f32>(),
                    n as u32,
                    eps,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn rms_norm_row_weighted_f32_input_offset(
            &self,
            input: &CudaBuffer,
            input_offset_elems: usize,
            weights_bf16: &CudaBuffer,
            output: &CudaBuffer,
            n: usize,
            eps: f32,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_rms_norm_row_weighted_f32(
                    input
                        .inner
                        .ptr
                        .as_ptr()
                        .cast::<f32>()
                        .add(input_offset_elems),
                    weights_bf16.inner.ptr.as_ptr().cast::<u16>(),
                    output.inner.ptr.as_ptr().cast::<f32>(),
                    n as u32,
                    eps,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn rms_norm_row_weighted_f32_input_offset_f32weights(
            &self,
            input: &CudaBuffer,
            input_offset_elems: usize,
            weights_f32: &CudaBuffer,
            output: &CudaBuffer,
            n: usize,
            eps: f32,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_rms_norm_row_weighted_f32_f32weights(
                    input
                        .inner
                        .ptr
                        .as_ptr()
                        .cast::<f32>()
                        .add(input_offset_elems),
                    weights_f32.inner.ptr.as_ptr().cast::<f32>(),
                    output.inner.ptr.as_ptr().cast::<f32>(),
                    n as u32,
                    eps,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn rms_norm_rows_weighted_f32(
            &self,
            input: &CudaBuffer,
            weights_bf16: &CudaBuffer,
            output: &CudaBuffer,
            row_count: usize,
            row_stride: usize,
            n: usize,
            eps: f32,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_rms_norm_rows_weighted_f32(
                    input.inner.ptr.as_ptr().cast::<f32>(),
                    weights_bf16.inner.ptr.as_ptr().cast::<u16>(),
                    output.inner.ptr.as_ptr().cast::<f32>(),
                    row_count as u32,
                    row_stride as u32,
                    n as u32,
                    eps,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn rms_norm_rows_weighted_f32_f32weights(
            &self,
            input: &CudaBuffer,
            weights_f32: &CudaBuffer,
            output: &CudaBuffer,
            row_count: usize,
            row_stride: usize,
            n: usize,
            eps: f32,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_rms_norm_rows_weighted_f32_f32weights(
                    input.inner.ptr.as_ptr().cast::<f32>(),
                    weights_f32.inner.ptr.as_ptr().cast::<f32>(),
                    output.inner.ptr.as_ptr().cast::<f32>(),
                    row_count as u32,
                    row_stride as u32,
                    n as u32,
                    eps,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn rms_norm_rows_weighted_f32_f32weights_precise(
            &self,
            input: &CudaBuffer,
            weights_f32: &CudaBuffer,
            output: &CudaBuffer,
            row_count: usize,
            row_stride: usize,
            n: usize,
            eps: f32,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_rms_norm_rows_weighted_f32_f32weights_precise(
                    input.inner.ptr.as_ptr().cast::<f32>(),
                    weights_f32.inner.ptr.as_ptr().cast::<f32>(),
                    output.inner.ptr.as_ptr().cast::<f32>(),
                    row_count as u32,
                    row_stride as u32,
                    n as u32,
                    eps,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn rms_norm_rows_weighted_f32_offset(
            &self,
            input: &CudaBuffer,
            input_offset_elems: usize,
            weights_bf16: &CudaBuffer,
            output: &CudaBuffer,
            output_offset_elems: usize,
            row_count: usize,
            row_stride: usize,
            n: usize,
            eps: f32,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_rms_norm_rows_weighted_f32(
                    input
                        .inner
                        .ptr
                        .as_ptr()
                        .cast::<f32>()
                        .add(input_offset_elems),
                    weights_bf16.inner.ptr.as_ptr().cast::<u16>(),
                    output
                        .inner
                        .ptr
                        .as_ptr()
                        .cast::<f32>()
                        .add(output_offset_elems),
                    row_count as u32,
                    row_stride as u32,
                    n as u32,
                    eps,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn rms_norm_rows_weighted_f32_offset_f32weights(
            &self,
            input: &CudaBuffer,
            input_offset_elems: usize,
            weights_f32: &CudaBuffer,
            output: &CudaBuffer,
            output_offset_elems: usize,
            row_count: usize,
            row_stride: usize,
            n: usize,
            eps: f32,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_rms_norm_rows_weighted_f32_f32weights(
                    input
                        .inner
                        .ptr
                        .as_ptr()
                        .cast::<f32>()
                        .add(input_offset_elems),
                    weights_f32.inner.ptr.as_ptr().cast::<f32>(),
                    output
                        .inner
                        .ptr
                        .as_ptr()
                        .cast::<f32>()
                        .add(output_offset_elems),
                    row_count as u32,
                    row_stride as u32,
                    n as u32,
                    eps,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn rms_norm_rows_no_scale_f32(
            &self,
            input: &CudaBuffer,
            output: &CudaBuffer,
            row_count: usize,
            row_stride: usize,
            n: usize,
            eps: f32,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_rms_norm_rows_no_scale_f32(
                    input.inner.ptr.as_ptr().cast::<f32>(),
                    output.inner.ptr.as_ptr().cast::<f32>(),
                    row_count as u32,
                    row_stride as u32,
                    n as u32,
                    eps,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn rms_norm_rows_no_scale_f32_precise(
            &self,
            input: &CudaBuffer,
            output: &CudaBuffer,
            row_count: usize,
            row_stride: usize,
            n: usize,
            eps: f32,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_rms_norm_rows_no_scale_f32_precise(
                    input.inner.ptr.as_ptr().cast::<f32>(),
                    output.inner.ptr.as_ptr().cast::<f32>(),
                    row_count as u32,
                    row_stride as u32,
                    n as u32,
                    eps,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn rms_norm_rows_no_scale_f32_offset(
            &self,
            input: &CudaBuffer,
            input_offset_elems: usize,
            output: &CudaBuffer,
            output_offset_elems: usize,
            row_count: usize,
            row_stride: usize,
            n: usize,
            eps: f32,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_rms_norm_rows_no_scale_f32(
                    input
                        .inner
                        .ptr
                        .as_ptr()
                        .cast::<f32>()
                        .add(input_offset_elems),
                    output
                        .inner
                        .ptr
                        .as_ptr()
                        .cast::<f32>()
                        .add(output_offset_elems),
                    row_count as u32,
                    row_stride as u32,
                    n as u32,
                    eps,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn rope_rows_f32(
            &self,
            input: &CudaBuffer,
            output: &CudaBuffer,
            row_count: usize,
            row_stride: usize,
            head_dim: usize,
            rotary_dim: usize,
            base: f32,
            position: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_rope_rows_f32(
                    input.inner.ptr.as_ptr().cast::<f32>(),
                    output.inner.ptr.as_ptr().cast::<f32>(),
                    row_count as u32,
                    row_stride as u32,
                    head_dim as u32,
                    rotary_dim as u32,
                    base,
                    position as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn rope_rows_f32_device_u32(
            &self,
            input: &CudaBuffer,
            output: &CudaBuffer,
            row_count: usize,
            row_stride: usize,
            head_dim: usize,
            rotary_dim: usize,
            base: f32,
            position_device_u32: &CudaBuffer,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_rope_rows_f32_device_u32(
                    input.inner.ptr.as_ptr().cast::<f32>(),
                    output.inner.ptr.as_ptr().cast::<f32>(),
                    row_count as u32,
                    row_stride as u32,
                    head_dim as u32,
                    rotary_dim as u32,
                    base,
                    position_device_u32.inner.ptr.as_ptr().cast::<u32>(),
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn kv_append_f32(
            &self,
            keys: &CudaBuffer,
            values: &CudaBuffer,
            key_cache: &CudaBuffer,
            value_cache: &CudaBuffer,
            kv_head_count: usize,
            head_dim: usize,
            max_tokens: usize,
            slot: usize,
        ) -> Result<(), String> {
            self.kv_append_f32_offsets(
                keys,
                0,
                values,
                0,
                key_cache,
                value_cache,
                kv_head_count,
                head_dim,
                max_tokens,
                slot,
            )
        }

        #[allow(clippy::too_many_arguments)]
        pub fn kv_append_f32_offsets(
            &self,
            keys: &CudaBuffer,
            key_offset_elems: usize,
            values: &CudaBuffer,
            value_offset_elems: usize,
            key_cache: &CudaBuffer,
            value_cache: &CudaBuffer,
            kv_head_count: usize,
            head_dim: usize,
            max_tokens: usize,
            slot: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_kv_append_f32(
                    keys.inner.ptr.as_ptr().cast::<f32>().add(key_offset_elems),
                    values
                        .inner
                        .ptr
                        .as_ptr()
                        .cast::<f32>()
                        .add(value_offset_elems),
                    key_cache.inner.ptr.as_ptr().cast::<u16>(),
                    value_cache.inner.ptr.as_ptr().cast::<u16>(),
                    kv_head_count as u32,
                    head_dim as u32,
                    max_tokens as u32,
                    slot as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn kv_append_f32_device_u32(
            &self,
            keys: &CudaBuffer,
            values: &CudaBuffer,
            key_cache: &CudaBuffer,
            value_cache: &CudaBuffer,
            kv_head_count: usize,
            head_dim: usize,
            max_tokens: usize,
            slot_device_u32: &CudaBuffer,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_kv_append_f32_device_u32(
                    keys.inner.ptr.as_ptr().cast::<f32>(),
                    values.inner.ptr.as_ptr().cast::<f32>(),
                    key_cache.inner.ptr.as_ptr().cast::<u16>(),
                    value_cache.inner.ptr.as_ptr().cast::<u16>(),
                    kv_head_count as u32,
                    head_dim as u32,
                    max_tokens as u32,
                    slot_device_u32.inner.ptr.as_ptr().cast::<u32>(),
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn kv_append_f32_device_u32_ptr(
            &self,
            keys: &CudaBuffer,
            values: &CudaBuffer,
            key_cache: &CudaBuffer,
            value_cache: &CudaBuffer,
            kv_head_count: usize,
            head_dim: usize,
            max_tokens: usize,
            slot_device_u32: *const u32,
        ) -> Result<(), String> {
            self.kv_append_f32_device_u32_ptr_offsets(
                keys,
                0,
                values,
                0,
                key_cache,
                value_cache,
                kv_head_count,
                head_dim,
                max_tokens,
                slot_device_u32,
            )
        }

        #[allow(clippy::too_many_arguments)]
        pub fn kv_append_f32_device_u32_ptr_offsets(
            &self,
            keys: &CudaBuffer,
            key_offset_elems: usize,
            values: &CudaBuffer,
            value_offset_elems: usize,
            key_cache: &CudaBuffer,
            value_cache: &CudaBuffer,
            kv_head_count: usize,
            head_dim: usize,
            max_tokens: usize,
            slot_device_u32: *const u32,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_kv_append_f32_device_u32(
                    keys.inner.ptr.as_ptr().cast::<f32>().add(key_offset_elems),
                    values
                        .inner
                        .ptr
                        .as_ptr()
                        .cast::<f32>()
                        .add(value_offset_elems),
                    key_cache.inner.ptr.as_ptr().cast::<u16>(),
                    value_cache.inner.ptr.as_ptr().cast::<u16>(),
                    kv_head_count as u32,
                    head_dim as u32,
                    max_tokens as u32,
                    slot_device_u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn qkv_norm_rope_cache_f32(
            &self,
            qkv: &CudaBuffer,
            q_weights_bf16: &CudaBuffer,
            k_weights_bf16: &CudaBuffer,
            q_out: &CudaBuffer,
            key_cache: &CudaBuffer,
            value_cache: &CudaBuffer,
            q_head_count: usize,
            k_head_count: usize,
            head_dim: usize,
            q_offset: usize,
            k_offset: usize,
            v_offset: usize,
            rotary_dim: usize,
            base: f32,
            position: usize,
            eps: f32,
            max_tokens: usize,
            slot: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_qkv_norm_rope_cache_f32(
                    qkv.inner.ptr.as_ptr().cast::<f32>(),
                    q_weights_bf16.inner.ptr.as_ptr().cast::<u16>(),
                    k_weights_bf16.inner.ptr.as_ptr().cast::<u16>(),
                    q_out.inner.ptr.as_ptr().cast::<f32>(),
                    key_cache.inner.ptr.as_ptr().cast::<u16>(),
                    value_cache.inner.ptr.as_ptr().cast::<u16>(),
                    q_head_count as u32,
                    k_head_count as u32,
                    head_dim as u32,
                    q_offset as u32,
                    k_offset as u32,
                    v_offset as u32,
                    rotary_dim as u32,
                    base,
                    position as u32,
                    eps,
                    max_tokens as u32,
                    slot as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn qkv_norm_rope_cache_rows_f32(
            &self,
            qkv: &CudaBuffer,
            q_weights_bf16: &CudaBuffer,
            k_weights_bf16: &CudaBuffer,
            q_out: &CudaBuffer,
            key_cache: &CudaBuffer,
            value_cache: &CudaBuffer,
            q_head_count: usize,
            k_head_count: usize,
            head_dim: usize,
            qkv_row_stride: usize,
            q_out_row_stride: usize,
            q_offset: usize,
            k_offset: usize,
            v_offset: usize,
            rotary_dim: usize,
            base: f32,
            start_position: usize,
            eps: f32,
            max_tokens: usize,
            start_slot: usize,
            row_count: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_qkv_norm_rope_cache_rows_f32(
                    qkv.inner.ptr.as_ptr().cast::<f32>(),
                    q_weights_bf16.inner.ptr.as_ptr().cast::<u16>(),
                    k_weights_bf16.inner.ptr.as_ptr().cast::<u16>(),
                    q_out.inner.ptr.as_ptr().cast::<f32>(),
                    key_cache.inner.ptr.as_ptr().cast::<u16>(),
                    value_cache.inner.ptr.as_ptr().cast::<u16>(),
                    q_head_count as u32,
                    k_head_count as u32,
                    head_dim as u32,
                    qkv_row_stride as u32,
                    q_out_row_stride as u32,
                    q_offset as u32,
                    k_offset as u32,
                    v_offset as u32,
                    rotary_dim as u32,
                    base,
                    start_position as u32,
                    eps,
                    max_tokens as u32,
                    start_slot as u32,
                    row_count as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn qkv_norm_rope_cache_f32_device_u32(
            &self,
            qkv: &CudaBuffer,
            q_weights_bf16: &CudaBuffer,
            k_weights_bf16: &CudaBuffer,
            q_out: &CudaBuffer,
            key_cache: &CudaBuffer,
            value_cache: &CudaBuffer,
            q_head_count: usize,
            k_head_count: usize,
            head_dim: usize,
            q_offset: usize,
            k_offset: usize,
            v_offset: usize,
            rotary_dim: usize,
            base: f32,
            position_device_u32: &CudaBuffer,
            eps: f32,
            max_tokens: usize,
        ) -> Result<(), String> {
            self.qkv_norm_rope_cache_f32_device_u32_ptr(
                qkv,
                q_weights_bf16,
                k_weights_bf16,
                q_out,
                key_cache,
                value_cache,
                q_head_count,
                k_head_count,
                head_dim,
                q_offset,
                k_offset,
                v_offset,
                rotary_dim,
                base,
                position_device_u32.inner.ptr.as_ptr().cast::<u32>(),
                eps,
                max_tokens,
            )
        }

        #[allow(clippy::too_many_arguments)]
        pub fn qkv_norm_rope_cache_f32_device_u32_ptr(
            &self,
            qkv: &CudaBuffer,
            q_weights_bf16: &CudaBuffer,
            k_weights_bf16: &CudaBuffer,
            q_out: &CudaBuffer,
            key_cache: &CudaBuffer,
            value_cache: &CudaBuffer,
            q_head_count: usize,
            k_head_count: usize,
            head_dim: usize,
            q_offset: usize,
            k_offset: usize,
            v_offset: usize,
            rotary_dim: usize,
            base: f32,
            position_device_u32: *const u32,
            eps: f32,
            max_tokens: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_qkv_norm_rope_cache_f32_device_u32(
                    qkv.inner.ptr.as_ptr().cast::<f32>(),
                    q_weights_bf16.inner.ptr.as_ptr().cast::<u16>(),
                    k_weights_bf16.inner.ptr.as_ptr().cast::<u16>(),
                    q_out.inner.ptr.as_ptr().cast::<f32>(),
                    key_cache.inner.ptr.as_ptr().cast::<u16>(),
                    value_cache.inner.ptr.as_ptr().cast::<u16>(),
                    q_head_count as u32,
                    k_head_count as u32,
                    head_dim as u32,
                    q_offset as u32,
                    k_offset as u32,
                    v_offset as u32,
                    rotary_dim as u32,
                    base,
                    position_device_u32,
                    eps,
                    max_tokens as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn qkv_norm_rope_cache_rows_f32_device_u32(
            &self,
            qkv: &CudaBuffer,
            q_weights_bf16: &CudaBuffer,
            k_weights_bf16: &CudaBuffer,
            q_out: &CudaBuffer,
            key_cache: &CudaBuffer,
            value_cache: &CudaBuffer,
            q_head_count: usize,
            k_head_count: usize,
            head_dim: usize,
            qkv_row_stride: usize,
            q_out_row_stride: usize,
            q_offset: usize,
            k_offset: usize,
            v_offset: usize,
            rotary_dim: usize,
            base: f32,
            start_position_device_u32: &CudaBuffer,
            eps: f32,
            max_tokens: usize,
            start_slot_device_u32: &CudaBuffer,
            row_count: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_qkv_norm_rope_cache_rows_f32_device_u32(
                    qkv.inner.ptr.as_ptr().cast::<f32>(),
                    q_weights_bf16.inner.ptr.as_ptr().cast::<u16>(),
                    k_weights_bf16.inner.ptr.as_ptr().cast::<u16>(),
                    q_out.inner.ptr.as_ptr().cast::<f32>(),
                    key_cache.inner.ptr.as_ptr().cast::<u16>(),
                    value_cache.inner.ptr.as_ptr().cast::<u16>(),
                    q_head_count as u32,
                    k_head_count as u32,
                    head_dim as u32,
                    qkv_row_stride as u32,
                    q_out_row_stride as u32,
                    q_offset as u32,
                    k_offset as u32,
                    v_offset as u32,
                    rotary_dim as u32,
                    base,
                    start_position_device_u32.inner.ptr.as_ptr().cast::<u32>(),
                    eps,
                    max_tokens as u32,
                    start_slot_device_u32.inner.ptr.as_ptr().cast::<u32>(),
                    row_count as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn attention_logits_seq_f32(
            &self,
            q: &CudaBuffer,
            key_cache: &CudaBuffer,
            logits: &CudaBuffer,
            q_head_count: usize,
            q_heads_per_kv: usize,
            head_dim: usize,
            kv_row_stride: usize,
            seq_len: usize,
            start_slot: usize,
            capacity: usize,
            logits_row_stride: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_attention_logits_seq_f32(
                    q.inner.ptr.as_ptr().cast::<f32>(),
                    key_cache.inner.ptr.as_ptr().cast::<u16>(),
                    logits.inner.ptr.as_ptr().cast::<f32>(),
                    q_head_count as u32,
                    q_heads_per_kv as u32,
                    head_dim as u32,
                    kv_row_stride as u32,
                    seq_len as u32,
                    start_slot as u32,
                    capacity as u32,
                    logits_row_stride as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn attention_logits_seq_f32_device_u32(
            &self,
            q: &CudaBuffer,
            key_cache: &CudaBuffer,
            logits: &CudaBuffer,
            q_head_count: usize,
            q_heads_per_kv: usize,
            head_dim: usize,
            kv_row_stride: usize,
            seq_len_device_u32: &CudaBuffer,
            start_slot_device_u32: &CudaBuffer,
            capacity: usize,
            logits_row_stride: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_attention_logits_seq_f32_device_u32(
                    q.inner.ptr.as_ptr().cast::<f32>(),
                    key_cache.inner.ptr.as_ptr().cast::<u16>(),
                    logits.inner.ptr.as_ptr().cast::<f32>(),
                    q_head_count as u32,
                    q_heads_per_kv as u32,
                    head_dim as u32,
                    kv_row_stride as u32,
                    seq_len_device_u32.inner.ptr.as_ptr().cast::<u32>(),
                    start_slot_device_u32.inner.ptr.as_ptr().cast::<u32>(),
                    capacity as u32,
                    logits_row_stride as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn attention_logits_seq_f32_device_u32_ptr(
            &self,
            q: &CudaBuffer,
            key_cache: &CudaBuffer,
            logits: &CudaBuffer,
            q_head_count: usize,
            q_heads_per_kv: usize,
            head_dim: usize,
            kv_row_stride: usize,
            seq_len_device_u32: *const u32,
            start_slot_device_u32: *const u32,
            capacity: usize,
            logits_row_stride: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_attention_logits_seq_f32_device_u32(
                    q.inner.ptr.as_ptr().cast::<f32>(),
                    key_cache.inner.ptr.as_ptr().cast::<u16>(),
                    logits.inner.ptr.as_ptr().cast::<f32>(),
                    q_head_count as u32,
                    q_heads_per_kv as u32,
                    head_dim as u32,
                    kv_row_stride as u32,
                    seq_len_device_u32,
                    start_slot_device_u32,
                    capacity as u32,
                    logits_row_stride as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn softmax_rows_f32(
            &self,
            logits: &CudaBuffer,
            probs: &CudaBuffer,
            row_count: usize,
            row_stride: usize,
            seq_len: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_softmax_rows_f32(
                    logits.inner.ptr.as_ptr().cast::<f32>(),
                    probs.inner.ptr.as_ptr().cast::<f32>(),
                    row_count as u32,
                    row_stride as u32,
                    seq_len as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn softmax_rows_f32_device_u32(
            &self,
            logits: &CudaBuffer,
            probs: &CudaBuffer,
            row_count: usize,
            row_stride: usize,
            seq_len_device_u32: &CudaBuffer,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_softmax_rows_f32_device_u32(
                    logits.inner.ptr.as_ptr().cast::<f32>(),
                    probs.inner.ptr.as_ptr().cast::<f32>(),
                    row_count as u32,
                    row_stride as u32,
                    seq_len_device_u32.inner.ptr.as_ptr().cast::<u32>(),
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn softmax_rows_causal_f32(
            &self,
            logits: &CudaBuffer,
            query_count: usize,
            row_count: usize,
            row_stride: usize,
            base_seq_len: usize,
            max_seq_len: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_softmax_rows_causal_f32(
                    logits.inner.ptr.as_ptr().cast::<f32>(),
                    query_count as u32,
                    row_count as u32,
                    row_stride as u32,
                    base_seq_len as u32,
                    max_seq_len as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn softmax_rows_causal_f32_device_u32_ptr(
            &self,
            logits: &CudaBuffer,
            query_count: usize,
            row_count: usize,
            row_stride: usize,
            base_seq_len_device_u32: *const u32,
            max_seq_len: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_softmax_rows_causal_f32_device_u32(
                    logits.inner.ptr.as_ptr().cast::<f32>(),
                    query_count as u32,
                    row_count as u32,
                    row_stride as u32,
                    base_seq_len_device_u32,
                    max_seq_len as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn softmax_rows_causal_bf16(
            &self,
            logits: &CudaBuffer,
            probs: &CudaBuffer,
            query_count: usize,
            row_count: usize,
            row_stride: usize,
            base_seq_len: usize,
            max_seq_len: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_softmax_rows_causal_bf16(
                    logits.inner.ptr.as_ptr().cast::<f32>(),
                    probs.inner.ptr.as_ptr().cast::<u16>(),
                    query_count as u32,
                    row_count as u32,
                    row_stride as u32,
                    base_seq_len as u32,
                    max_seq_len as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn softmax_rows_causal_bf16_device_u32_ptr(
            &self,
            logits: &CudaBuffer,
            probs: &CudaBuffer,
            query_count: usize,
            row_count: usize,
            row_stride: usize,
            base_seq_len_device_u32: *const u32,
            max_seq_len: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_softmax_rows_causal_bf16_device_u32(
                    logits.inner.ptr.as_ptr().cast::<f32>(),
                    probs.inner.ptr.as_ptr().cast::<u16>(),
                    query_count as u32,
                    row_count as u32,
                    row_stride as u32,
                    base_seq_len_device_u32,
                    max_seq_len as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn softmax_rows_causal_vision_bf16(
            &self,
            logits: &CudaBuffer,
            probs: &CudaBuffer,
            query_count: usize,
            row_count: usize,
            row_stride: usize,
            base_seq_len: usize,
            max_seq_len: usize,
            chunk_start_position: usize,
            vision_start_position: usize,
            vision_end_position: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_softmax_rows_causal_vision_bf16(
                    logits.inner.ptr.as_ptr().cast::<f32>(),
                    probs.inner.ptr.as_ptr().cast::<u16>(),
                    query_count as u32,
                    row_count as u32,
                    row_stride as u32,
                    base_seq_len as u32,
                    max_seq_len as u32,
                    chunk_start_position as u32,
                    vision_start_position as u32,
                    vision_end_position as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        #[allow(clippy::too_many_arguments)]
        fn attention_seq_softmax_weighted_sum_rows_blas_f32_inner(
            &self,
            q: &CudaBuffer,
            q_bf16: &CudaBuffer,
            key_cache: &CudaBuffer,
            value_cache: &CudaBuffer,
            logits: &CudaBuffer,
            probs_bf16: &CudaBuffer,
            out: &CudaBuffer,
            query_count: usize,
            q_head_count: usize,
            q_heads_per_kv: usize,
            head_dim: usize,
            kv_row_stride: usize,
            q_row_stride: usize,
            out_row_stride: usize,
            base_seq_len: usize,
            base_seq_len_device_u32: Option<*const u32>,
            capacity: usize,
            max_seq_len: usize,
            vision_mask: Option<(usize, usize, usize)>,
        ) -> Result<(), String> {
            if query_count == 0
                || q_head_count == 0
                || q_heads_per_kv == 0
                || head_dim == 0
                || capacity == 0
                || max_seq_len == 0
                || max_seq_len > capacity
                || q_head_count % q_heads_per_kv != 0
                || q_row_stride < q_head_count * head_dim
                || out_row_stride < q_head_count * head_dim
                || kv_row_stride < capacity * head_dim
            {
                return Err("invalid CUDA BLAS prefill attention shape".to_string());
            }
            let workspace_seq_stride = max_seq_len;
            let logits_len = q_head_count
                .checked_mul(query_count)
                .and_then(|len| len.checked_mul(workspace_seq_stride))
                .ok_or_else(|| "CUDA BLAS prefill attention logits size overflow".to_string())?;
            let logits_bytes = logits_len.checked_mul(size_of::<f32>()).ok_or_else(|| {
                "CUDA BLAS prefill attention logits byte size overflow".to_string()
            })?;
            if logits.size_bytes() < logits_bytes {
                return Err(format!(
                    "CUDA BLAS prefill attention logits buffer too small: {} < {}",
                    logits.size_bytes(),
                    logits_bytes
                ));
            }
            let probs_bytes = logits_len.checked_mul(size_of::<u16>()).ok_or_else(|| {
                "CUDA BLAS prefill attention probs byte size overflow".to_string()
            })?;
            if probs_bf16.size_bytes() < probs_bytes {
                return Err(format!(
                    "CUDA BLAS prefill attention probs buffer too small: {} < {}",
                    probs_bf16.size_bytes(),
                    probs_bytes
                ));
            }
            let q_bf16_len = query_count
                .checked_mul(q_row_stride)
                .ok_or_else(|| "CUDA BLAS prefill attention Q bf16 size overflow".to_string())?;
            let q_bf16_bytes = q_bf16_len.checked_mul(size_of::<u16>()).ok_or_else(|| {
                "CUDA BLAS prefill attention Q bf16 byte size overflow".to_string()
            })?;
            if q_bf16.size_bytes() < q_bf16_bytes {
                return Err(format!(
                    "CUDA BLAS prefill attention Q bf16 buffer too small: {} < {}",
                    q_bf16.size_bytes(),
                    q_bf16_bytes
                ));
            }

            self.prepare_device()?;
            self.f32_to_bf16(q, q_bf16, q_bf16_len)?;
            let alpha = 1.0f32;
            let beta = 0.0f32;
            let k_head_count = q_head_count / q_heads_per_kv;
            let logits_head_stride = query_count
                .checked_mul(workspace_seq_stride)
                .ok_or_else(|| "CUDA BLAS prefill attention head stride overflow".to_string())?;
            let batch_count = q_heads_per_kv as i32;
            let compute_type = crate::CUBLAS_COMPUTE_32F_FAST_16BF;

            for kv_head in 0..k_head_count {
                let q_head_base = kv_head * q_heads_per_kv;
                unsafe {
                    crate::cublas_gemm_strided_batched_ex(
                        self.blas,
                        crate::CUBLAS_OP_T,
                        crate::CUBLAS_OP_N,
                        max_seq_len as i32,
                        query_count as i32,
                        head_dim as i32,
                        &alpha,
                        key_cache
                            .inner
                            .ptr
                            .as_ptr()
                            .cast::<u16>()
                            .add(kv_head * kv_row_stride)
                            .cast::<c_void>() as *const c_void,
                        crate::CUDA_R_16BF,
                        head_dim as i32,
                        0,
                        q_bf16
                            .inner
                            .ptr
                            .as_ptr()
                            .cast::<u16>()
                            .add(q_head_base * head_dim)
                            .cast::<c_void>() as *const c_void,
                        crate::CUDA_R_16BF,
                        q_row_stride as i32,
                        head_dim as i64,
                        &beta,
                        logits
                            .inner
                            .ptr
                            .as_ptr()
                            .cast::<f32>()
                            .add(q_head_base * logits_head_stride)
                            .cast::<c_void>(),
                        crate::CUDA_R_32F,
                        workspace_seq_stride as i32,
                        logits_head_stride as i64,
                        batch_count,
                        compute_type,
                        crate::CUBLAS_GEMM_DEFAULT,
                    )
                    .map_err(|err| {
                        format!(
                            "cuBLAS prefill QK gemm failed: kv_head={kv_head} m={max_seq_len} n={query_count} k={head_dim} batch={q_heads_per_kv}: {err}"
                        )
                    })?;
                }
            }

            let row_count = q_head_count
                .checked_mul(query_count)
                .ok_or_else(|| "CUDA BLAS prefill attention row count overflow".to_string())?;
            if let Some((chunk_start_position, vision_start_position, vision_end_position)) =
                vision_mask
            {
                if base_seq_len_device_u32.is_some() {
                    return Err(
                        "CUDA vision prefill attention does not support device sequence length"
                            .to_string(),
                    );
                }
                self.softmax_rows_causal_vision_bf16(
                    logits,
                    probs_bf16,
                    query_count,
                    row_count,
                    workspace_seq_stride,
                    base_seq_len,
                    max_seq_len,
                    chunk_start_position,
                    vision_start_position,
                    vision_end_position,
                )?;
            } else if let Some(base_seq_len_device_u32) = base_seq_len_device_u32 {
                self.softmax_rows_causal_bf16_device_u32_ptr(
                    logits,
                    probs_bf16,
                    query_count,
                    row_count,
                    workspace_seq_stride,
                    base_seq_len_device_u32,
                    max_seq_len,
                )?;
            } else {
                self.softmax_rows_causal_bf16(
                    logits,
                    probs_bf16,
                    query_count,
                    row_count,
                    workspace_seq_stride,
                    base_seq_len,
                    max_seq_len,
                )?;
            }

            for kv_head in 0..k_head_count {
                let q_head_base = kv_head * q_heads_per_kv;
                unsafe {
                    crate::cublas_gemm_strided_batched_ex(
                        self.blas,
                        crate::CUBLAS_OP_T,
                        crate::CUBLAS_OP_N,
                        head_dim as i32,
                        query_count as i32,
                        max_seq_len as i32,
                        &alpha,
                        value_cache
                            .inner
                            .ptr
                            .as_ptr()
                            .cast::<u16>()
                            .add(kv_head * kv_row_stride)
                            .cast::<c_void>() as *const c_void,
                        crate::CUDA_R_16BF,
                        capacity as i32,
                        0,
                        probs_bf16
                            .inner
                            .ptr
                            .as_ptr()
                            .cast::<u16>()
                            .add(q_head_base * logits_head_stride)
                            .cast::<c_void>() as *const c_void,
                        crate::CUDA_R_16BF,
                        workspace_seq_stride as i32,
                        logits_head_stride as i64,
                        &beta,
                        out.inner
                            .ptr
                            .as_ptr()
                            .cast::<f32>()
                            .add(q_head_base * head_dim)
                            .cast::<c_void>(),
                        crate::CUDA_R_32F,
                        out_row_stride as i32,
                        head_dim as i64,
                        batch_count,
                        compute_type,
                        crate::CUBLAS_GEMM_DEFAULT,
                    )
                    .map_err(|err| {
                        format!(
                            "cuBLAS prefill PV gemm failed: kv_head={kv_head} m={head_dim} n={query_count} k={max_seq_len} batch={q_heads_per_kv}: {err}"
                        )
                    })?;
                }
            }
            Ok(())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn attention_seq_softmax_weighted_sum_rows_blas_f32(
            &self,
            q: &CudaBuffer,
            q_bf16: &CudaBuffer,
            key_cache: &CudaBuffer,
            value_cache: &CudaBuffer,
            logits: &CudaBuffer,
            probs_bf16: &CudaBuffer,
            out: &CudaBuffer,
            query_count: usize,
            q_head_count: usize,
            q_heads_per_kv: usize,
            head_dim: usize,
            kv_row_stride: usize,
            q_row_stride: usize,
            out_row_stride: usize,
            base_seq_len: usize,
            capacity: usize,
        ) -> Result<(), String> {
            let max_seq_len = base_seq_len
                .checked_add(query_count)
                .ok_or_else(|| "CUDA BLAS prefill attention sequence length overflow".to_string())?
                .min(capacity);
            self.attention_seq_softmax_weighted_sum_rows_blas_f32_inner(
                q,
                q_bf16,
                key_cache,
                value_cache,
                logits,
                probs_bf16,
                out,
                query_count,
                q_head_count,
                q_heads_per_kv,
                head_dim,
                kv_row_stride,
                q_row_stride,
                out_row_stride,
                base_seq_len,
                None,
                capacity,
                max_seq_len,
                None,
            )
        }

        #[allow(clippy::too_many_arguments)]
        pub fn attention_seq_softmax_weighted_sum_rows_blas_f32_vision(
            &self,
            q: &CudaBuffer,
            q_bf16: &CudaBuffer,
            key_cache: &CudaBuffer,
            value_cache: &CudaBuffer,
            logits: &CudaBuffer,
            probs_bf16: &CudaBuffer,
            out: &CudaBuffer,
            query_count: usize,
            q_head_count: usize,
            q_heads_per_kv: usize,
            head_dim: usize,
            kv_row_stride: usize,
            q_row_stride: usize,
            out_row_stride: usize,
            base_seq_len: usize,
            capacity: usize,
            chunk_start_position: usize,
            vision_start_position: usize,
            vision_end_position: usize,
        ) -> Result<(), String> {
            let max_seq_len = base_seq_len
                .checked_add(query_count)
                .ok_or_else(|| "CUDA BLAS prefill attention sequence length overflow".to_string())?
                .min(capacity);
            self.attention_seq_softmax_weighted_sum_rows_blas_f32_inner(
                q,
                q_bf16,
                key_cache,
                value_cache,
                logits,
                probs_bf16,
                out,
                query_count,
                q_head_count,
                q_heads_per_kv,
                head_dim,
                kv_row_stride,
                q_row_stride,
                out_row_stride,
                base_seq_len,
                None,
                capacity,
                max_seq_len,
                Some((
                    chunk_start_position,
                    vision_start_position,
                    vision_end_position,
                )),
            )
        }

        #[allow(clippy::too_many_arguments)]
        pub fn attention_seq_softmax_weighted_sum_rows_blas_f32_device_u32(
            &self,
            q: &CudaBuffer,
            q_bf16: &CudaBuffer,
            key_cache: &CudaBuffer,
            value_cache: &CudaBuffer,
            logits: &CudaBuffer,
            probs_bf16: &CudaBuffer,
            out: &CudaBuffer,
            query_count: usize,
            q_head_count: usize,
            q_heads_per_kv: usize,
            head_dim: usize,
            kv_row_stride: usize,
            q_row_stride: usize,
            out_row_stride: usize,
            base_seq_len_device_u32: &CudaBuffer,
            capacity: usize,
        ) -> Result<(), String> {
            self.attention_seq_softmax_weighted_sum_rows_blas_f32_device_u32_ptr(
                q,
                q_bf16,
                key_cache,
                value_cache,
                logits,
                probs_bf16,
                out,
                query_count,
                q_head_count,
                q_heads_per_kv,
                head_dim,
                kv_row_stride,
                q_row_stride,
                out_row_stride,
                base_seq_len_device_u32.inner.ptr.as_ptr().cast::<u32>(),
                capacity,
            )
        }

        #[allow(clippy::too_many_arguments)]
        pub fn attention_seq_softmax_weighted_sum_rows_blas_f32_device_u32_ptr(
            &self,
            q: &CudaBuffer,
            q_bf16: &CudaBuffer,
            key_cache: &CudaBuffer,
            value_cache: &CudaBuffer,
            logits: &CudaBuffer,
            probs_bf16: &CudaBuffer,
            out: &CudaBuffer,
            query_count: usize,
            q_head_count: usize,
            q_heads_per_kv: usize,
            head_dim: usize,
            kv_row_stride: usize,
            q_row_stride: usize,
            out_row_stride: usize,
            base_seq_len_device_u32: *const u32,
            capacity: usize,
        ) -> Result<(), String> {
            self.attention_seq_softmax_weighted_sum_rows_blas_f32_inner(
                q,
                q_bf16,
                key_cache,
                value_cache,
                logits,
                probs_bf16,
                out,
                query_count,
                q_head_count,
                q_heads_per_kv,
                head_dim,
                kv_row_stride,
                q_row_stride,
                out_row_stride,
                0,
                Some(base_seq_len_device_u32),
                capacity,
                capacity,
                None,
            )
        }

        pub fn attention_weighted_sum_f32(
            &self,
            probs: &CudaBuffer,
            value_cache: &CudaBuffer,
            out: &CudaBuffer,
            q_head_count: usize,
            q_heads_per_kv: usize,
            head_dim: usize,
            kv_row_stride: usize,
            seq_len: usize,
            start_slot: usize,
            capacity: usize,
            probs_row_stride: usize,
            out_row_stride: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_attention_weighted_sum_f32(
                    probs.inner.ptr.as_ptr().cast::<f32>(),
                    value_cache.inner.ptr.as_ptr().cast::<u16>(),
                    out.inner.ptr.as_ptr().cast::<f32>(),
                    q_head_count as u32,
                    q_heads_per_kv as u32,
                    head_dim as u32,
                    kv_row_stride as u32,
                    seq_len as u32,
                    start_slot as u32,
                    capacity as u32,
                    probs_row_stride as u32,
                    out_row_stride as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn attention_weighted_sum_f32_output_offset(
            &self,
            probs: &CudaBuffer,
            value_cache: &CudaBuffer,
            out: &CudaBuffer,
            output_offset_elems: usize,
            q_head_count: usize,
            q_heads_per_kv: usize,
            head_dim: usize,
            kv_row_stride: usize,
            seq_len: usize,
            start_slot: usize,
            capacity: usize,
            probs_row_stride: usize,
            out_row_stride: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_attention_weighted_sum_f32(
                    probs.inner.ptr.as_ptr().cast::<f32>(),
                    value_cache.inner.ptr.as_ptr().cast::<u16>(),
                    out.inner
                        .ptr
                        .as_ptr()
                        .cast::<f32>()
                        .add(output_offset_elems),
                    q_head_count as u32,
                    q_heads_per_kv as u32,
                    head_dim as u32,
                    kv_row_stride as u32,
                    seq_len as u32,
                    start_slot as u32,
                    capacity as u32,
                    probs_row_stride as u32,
                    out_row_stride as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn attention_weighted_sum_f32_device_u32(
            &self,
            probs: &CudaBuffer,
            value_cache: &CudaBuffer,
            out: &CudaBuffer,
            q_head_count: usize,
            q_heads_per_kv: usize,
            head_dim: usize,
            kv_row_stride: usize,
            seq_len_device_u32: &CudaBuffer,
            capacity: usize,
            probs_row_stride: usize,
            out_row_stride: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_attention_weighted_sum_f32_device_u32(
                    probs.inner.ptr.as_ptr().cast::<f32>(),
                    value_cache.inner.ptr.as_ptr().cast::<u16>(),
                    out.inner.ptr.as_ptr().cast::<f32>(),
                    q_head_count as u32,
                    q_heads_per_kv as u32,
                    head_dim as u32,
                    kv_row_stride as u32,
                    seq_len_device_u32.inner.ptr.as_ptr().cast::<u32>(),
                    capacity as u32,
                    probs_row_stride as u32,
                    out_row_stride as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn attention_softmax_weighted_sum_f32(
            &self,
            logits: &CudaBuffer,
            value_cache: &CudaBuffer,
            out: &CudaBuffer,
            q_head_count: usize,
            q_heads_per_kv: usize,
            head_dim: usize,
            kv_row_stride: usize,
            seq_len: usize,
            start_slot: usize,
            capacity: usize,
            logits_row_stride: usize,
            out_row_stride: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_attention_softmax_weighted_sum_f32(
                    logits.inner.ptr.as_ptr().cast::<f32>(),
                    value_cache.inner.ptr.as_ptr().cast::<u16>(),
                    out.inner.ptr.as_ptr().cast::<f32>(),
                    q_head_count as u32,
                    q_heads_per_kv as u32,
                    head_dim as u32,
                    kv_row_stride as u32,
                    seq_len as u32,
                    start_slot as u32,
                    capacity as u32,
                    logits_row_stride as u32,
                    out_row_stride as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn attention_softmax_weighted_sum_f32_output_offset(
            &self,
            logits: &CudaBuffer,
            value_cache: &CudaBuffer,
            out: &CudaBuffer,
            output_offset_elems: usize,
            q_head_count: usize,
            q_heads_per_kv: usize,
            head_dim: usize,
            kv_row_stride: usize,
            seq_len: usize,
            start_slot: usize,
            capacity: usize,
            logits_row_stride: usize,
            out_row_stride: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_attention_softmax_weighted_sum_f32(
                    logits.inner.ptr.as_ptr().cast::<f32>(),
                    value_cache.inner.ptr.as_ptr().cast::<u16>(),
                    out.inner
                        .ptr
                        .as_ptr()
                        .cast::<f32>()
                        .add(output_offset_elems),
                    q_head_count as u32,
                    q_heads_per_kv as u32,
                    head_dim as u32,
                    kv_row_stride as u32,
                    seq_len as u32,
                    start_slot as u32,
                    capacity as u32,
                    logits_row_stride as u32,
                    out_row_stride as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn attention_softmax_weighted_sum_f32_device_u32(
            &self,
            logits: &CudaBuffer,
            value_cache: &CudaBuffer,
            out: &CudaBuffer,
            q_head_count: usize,
            q_heads_per_kv: usize,
            head_dim: usize,
            kv_row_stride: usize,
            seq_len_device_u32: &CudaBuffer,
            start_slot_device_u32: &CudaBuffer,
            capacity: usize,
            logits_row_stride: usize,
            out_row_stride: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_attention_softmax_weighted_sum_f32_device_u32(
                    logits.inner.ptr.as_ptr().cast::<f32>(),
                    value_cache.inner.ptr.as_ptr().cast::<u16>(),
                    out.inner.ptr.as_ptr().cast::<f32>(),
                    q_head_count as u32,
                    q_heads_per_kv as u32,
                    head_dim as u32,
                    kv_row_stride as u32,
                    seq_len_device_u32.inner.ptr.as_ptr().cast::<u32>(),
                    start_slot_device_u32.inner.ptr.as_ptr().cast::<u32>(),
                    capacity as u32,
                    logits_row_stride as u32,
                    out_row_stride as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn attention_softmax_weighted_sum_f32_device_u32_ptr(
            &self,
            logits: &CudaBuffer,
            value_cache: &CudaBuffer,
            out: &CudaBuffer,
            q_head_count: usize,
            q_heads_per_kv: usize,
            head_dim: usize,
            kv_row_stride: usize,
            seq_len_device_u32: *const u32,
            start_slot_device_u32: *const u32,
            capacity: usize,
            logits_row_stride: usize,
            out_row_stride: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_attention_softmax_weighted_sum_f32_device_u32(
                    logits.inner.ptr.as_ptr().cast::<f32>(),
                    value_cache.inner.ptr.as_ptr().cast::<u16>(),
                    out.inner.ptr.as_ptr().cast::<f32>(),
                    q_head_count as u32,
                    q_heads_per_kv as u32,
                    head_dim as u32,
                    kv_row_stride as u32,
                    seq_len_device_u32,
                    start_slot_device_u32,
                    capacity as u32,
                    logits_row_stride as u32,
                    out_row_stride as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn attention_seq_softmax_weighted_sum_f32(
            &self,
            q: &CudaBuffer,
            key_cache: &CudaBuffer,
            value_cache: &CudaBuffer,
            out: &CudaBuffer,
            q_head_count: usize,
            q_heads_per_kv: usize,
            head_dim: usize,
            kv_row_stride: usize,
            seq_len: usize,
            start_slot: usize,
            capacity: usize,
            out_row_stride: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_attention_seq_softmax_weighted_sum_f32(
                    q.inner.ptr.as_ptr().cast::<f32>(),
                    key_cache.inner.ptr.as_ptr().cast::<u16>(),
                    value_cache.inner.ptr.as_ptr().cast::<u16>(),
                    out.inner.ptr.as_ptr().cast::<f32>(),
                    q_head_count as u32,
                    q_heads_per_kv as u32,
                    head_dim as u32,
                    kv_row_stride as u32,
                    seq_len as u32,
                    start_slot as u32,
                    capacity as u32,
                    out_row_stride as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn attention_seq_softmax_weighted_sum_rows_f32(
            &self,
            q: &CudaBuffer,
            key_cache: &CudaBuffer,
            value_cache: &CudaBuffer,
            out: &CudaBuffer,
            query_count: usize,
            q_head_count: usize,
            q_heads_per_kv: usize,
            head_dim: usize,
            kv_row_stride: usize,
            q_row_stride: usize,
            out_row_stride: usize,
            base_seq_len: usize,
            capacity: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_attention_seq_softmax_weighted_sum_rows_f32(
                    q.inner.ptr.as_ptr().cast::<f32>(),
                    key_cache.inner.ptr.as_ptr().cast::<u16>(),
                    value_cache.inner.ptr.as_ptr().cast::<u16>(),
                    out.inner.ptr.as_ptr().cast::<f32>(),
                    query_count as u32,
                    q_head_count as u32,
                    q_heads_per_kv as u32,
                    head_dim as u32,
                    kv_row_stride as u32,
                    q_row_stride as u32,
                    out_row_stride as u32,
                    base_seq_len as u32,
                    capacity as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn attention_seq_softmax_weighted_sum_f32_output_offset(
            &self,
            q: &CudaBuffer,
            key_cache: &CudaBuffer,
            value_cache: &CudaBuffer,
            out: &CudaBuffer,
            output_offset_elems: usize,
            q_head_count: usize,
            q_heads_per_kv: usize,
            head_dim: usize,
            kv_row_stride: usize,
            seq_len: usize,
            start_slot: usize,
            capacity: usize,
            out_row_stride: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_attention_seq_softmax_weighted_sum_f32(
                    q.inner.ptr.as_ptr().cast::<f32>(),
                    key_cache.inner.ptr.as_ptr().cast::<u16>(),
                    value_cache.inner.ptr.as_ptr().cast::<u16>(),
                    out.inner
                        .ptr
                        .as_ptr()
                        .cast::<f32>()
                        .add(output_offset_elems),
                    q_head_count as u32,
                    q_heads_per_kv as u32,
                    head_dim as u32,
                    kv_row_stride as u32,
                    seq_len as u32,
                    start_slot as u32,
                    capacity as u32,
                    out_row_stride as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn attention_seq_softmax_weighted_sum_f32_device_u32(
            &self,
            q: &CudaBuffer,
            key_cache: &CudaBuffer,
            value_cache: &CudaBuffer,
            out: &CudaBuffer,
            q_head_count: usize,
            q_heads_per_kv: usize,
            head_dim: usize,
            kv_row_stride: usize,
            seq_len_device_u32: &CudaBuffer,
            capacity: usize,
            out_row_stride: usize,
        ) -> Result<(), String> {
            self.attention_seq_softmax_weighted_sum_f32_device_u32_ptr(
                q,
                key_cache,
                value_cache,
                out,
                q_head_count,
                q_heads_per_kv,
                head_dim,
                kv_row_stride,
                seq_len_device_u32.inner.ptr.as_ptr().cast::<u32>(),
                capacity,
                out_row_stride,
            )
        }

        #[allow(clippy::too_many_arguments)]
        pub fn attention_seq_softmax_weighted_sum_f32_device_u32_ptr(
            &self,
            q: &CudaBuffer,
            key_cache: &CudaBuffer,
            value_cache: &CudaBuffer,
            out: &CudaBuffer,
            q_head_count: usize,
            q_heads_per_kv: usize,
            head_dim: usize,
            kv_row_stride: usize,
            seq_len_device_u32: *const u32,
            capacity: usize,
            out_row_stride: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_attention_seq_softmax_weighted_sum_f32_device_u32(
                    q.inner.ptr.as_ptr().cast::<f32>(),
                    key_cache.inner.ptr.as_ptr().cast::<u16>(),
                    value_cache.inner.ptr.as_ptr().cast::<u16>(),
                    out.inner.ptr.as_ptr().cast::<f32>(),
                    q_head_count as u32,
                    q_heads_per_kv as u32,
                    head_dim as u32,
                    kv_row_stride as u32,
                    seq_len_device_u32,
                    capacity as u32,
                    out_row_stride as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn attention_seq_softmax_weighted_sum_rows_f32_device_u32(
            &self,
            q: &CudaBuffer,
            key_cache: &CudaBuffer,
            value_cache: &CudaBuffer,
            out: &CudaBuffer,
            query_count: usize,
            q_head_count: usize,
            q_heads_per_kv: usize,
            head_dim: usize,
            kv_row_stride: usize,
            q_row_stride: usize,
            out_row_stride: usize,
            base_seq_len_device_u32: &CudaBuffer,
            capacity: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_attention_seq_softmax_weighted_sum_rows_f32_device_u32(
                    q.inner.ptr.as_ptr().cast::<f32>(),
                    key_cache.inner.ptr.as_ptr().cast::<u16>(),
                    value_cache.inner.ptr.as_ptr().cast::<u16>(),
                    out.inner.ptr.as_ptr().cast::<f32>(),
                    query_count as u32,
                    q_head_count as u32,
                    q_heads_per_kv as u32,
                    head_dim as u32,
                    kv_row_stride as u32,
                    q_row_stride as u32,
                    out_row_stride as u32,
                    base_seq_len_device_u32.inner.ptr.as_ptr().cast::<u32>(),
                    capacity as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn flash_attn_f32_packed(
            &self,
            q: &CudaBuffer,
            k: &CudaBuffer,
            v: &CudaBuffer,
            out: &CudaBuffer,
            seq_len: usize,
            num_heads: usize,
            head_dim: usize,
            scale: f32,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_flash_attn_f32_packed(
                    q.inner.ptr.as_ptr().cast::<f32>(),
                    k.inner.ptr.as_ptr().cast::<f32>(),
                    v.inner.ptr.as_ptr().cast::<f32>(),
                    out.inner.ptr.as_ptr().cast::<f32>(),
                    seq_len as u32,
                    num_heads as u32,
                    head_dim as u32,
                    scale,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn argmax_f32(
            &self,
            logits: &CudaBuffer,
            out_index: &CudaBuffer,
            n: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_argmax_f32(
                    logits.inner.ptr.as_ptr().cast::<f32>(),
                    out_index.inner.ptr.as_ptr().cast::<u32>(),
                    n as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn argmax_f32_ptr(
            &self,
            logits: &CudaBuffer,
            out_index_device_u32: *mut u32,
            n: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_argmax_f32(
                    logits.inner.ptr.as_ptr().cast::<f32>(),
                    out_index_device_u32,
                    n as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn masked_argmax_f32(
            &self,
            logits: &CudaBuffer,
            disallowed_token_ids: &CudaBuffer,
            disallowed_count: usize,
            out_index: &CudaBuffer,
            n: usize,
        ) -> Result<(), String> {
            if disallowed_count == 0 {
                return self.argmax_f32(logits, out_index, n);
            }
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_masked_argmax_f32(
                    logits.inner.ptr.as_ptr().cast::<f32>(),
                    disallowed_token_ids.inner.ptr.as_ptr().cast::<u32>(),
                    disallowed_count as u32,
                    out_index.inner.ptr.as_ptr().cast::<u32>(),
                    n as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }

        pub fn masked_argmax_f32_device_u32(
            &self,
            logits: &CudaBuffer,
            disallowed_token_ids: &CudaBuffer,
            disallowed_count_device_u32: &CudaBuffer,
            out_index: &CudaBuffer,
            n: usize,
        ) -> Result<(), String> {
            self.masked_argmax_f32_device_u32_ptr(
                logits,
                disallowed_token_ids,
                disallowed_count_device_u32.inner.ptr.as_ptr().cast::<u32>(),
                out_index.inner.ptr.as_ptr().cast::<u32>(),
                n,
            )
        }

        pub fn masked_argmax_f32_device_u32_ptr(
            &self,
            logits: &CudaBuffer,
            disallowed_token_ids: &CudaBuffer,
            disallowed_count_device_u32: *const u32,
            out_index: *mut u32,
            n: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_cuda_masked_argmax_f32_device_u32(
                    logits.inner.ptr.as_ptr().cast::<f32>(),
                    disallowed_token_ids.inner.ptr.as_ptr().cast::<u32>(),
                    disallowed_count_device_u32,
                    out_index,
                    n as u32,
                    self.stream,
                )
            };
            crate::check(status).map_err(|err| err.to_string())
        }
    }

    impl Drop for CudaRuntime {
        fn drop(&mut self) {
            let _ = crate::cublas_destroy(self.blas);
            let _ = crate::destroy_stream(self.stream);
        }
    }

    pub fn supports_affine_quantized_matmul(bits: u32, group_size: u64) -> bool {
        matches!(bits, 4 | 8) && group_size == 64 && crate::driver::is_available()
    }

    pub fn is_available() -> bool {
        crate::driver::is_available()
    }

    pub fn try_affine_quantized_matmul_bf16<FW, FS, FB>(
        spec: AffineQuantizedMatmulSpec<'_>,
        weight_cache_key: &str,
        scales_cache_key: &str,
        biases_cache_key: &str,
        load_weight_bytes: FW,
        load_scales_bytes: FS,
        load_biases_bytes: FB,
    ) -> Result<Vec<f32>, String>
    where
        FW: FnOnce() -> Result<Vec<u8>, String>,
        FS: FnOnce() -> Result<Vec<u8>, String>,
        FB: FnOnce() -> Result<Vec<u8>, String>,
    {
        thread_local! {
            static AFFINE_CUDA_BACKEND: RefCell<Option<CudaAffineBackend>> = const { RefCell::new(None) };
        }

        AFFINE_CUDA_BACKEND.with(|backend| {
            let mut backend = backend.borrow_mut();
            if backend.is_none() {
                *backend = Some(CudaAffineBackend::load()?);
            }
            backend
                .as_mut()
                .expect("affine CUDA backend was just initialized")
                .matmul(
                    spec,
                    weight_cache_key,
                    scales_cache_key,
                    biases_cache_key,
                    load_weight_bytes,
                    load_scales_bytes,
                    load_biases_bytes,
                )
        })
    }

    pub fn try_affine_quantized_matmul_bf16_rows<FW, FS, FB>(
        spec: AffineQuantizedMatmulRowsSpec<'_>,
        weight_cache_key: &str,
        scales_cache_key: &str,
        biases_cache_key: &str,
        load_weight_bytes: FW,
        load_scales_bytes: FS,
        load_biases_bytes: FB,
    ) -> Result<Vec<f32>, String>
    where
        FW: FnOnce() -> Result<Vec<u8>, String>,
        FS: FnOnce() -> Result<Vec<u8>, String>,
        FB: FnOnce() -> Result<Vec<u8>, String>,
    {
        thread_local! {
            static AFFINE_CUDA_BACKEND: RefCell<Option<CudaAffineBackend>> = const { RefCell::new(None) };
        }

        AFFINE_CUDA_BACKEND.with(|backend| {
            let mut backend = backend.borrow_mut();
            if backend.is_none() {
                *backend = Some(CudaAffineBackend::load()?);
            }
            backend
                .as_mut()
                .expect("affine CUDA backend was just initialized")
                .matmul_rows(
                    spec,
                    weight_cache_key,
                    scales_cache_key,
                    biases_cache_key,
                    load_weight_bytes,
                    load_scales_bytes,
                    load_biases_bytes,
                )
        })
    }

    thread_local! {
        static DENSE_LINEAR_BACKEND: RefCell<Option<CudaDenseLinearBackend>> =
            const { RefCell::new(None) };
    }

    fn with_dense_linear_backend<T, F>(f: F) -> Result<T, String>
    where
        F: FnOnce(&mut CudaDenseLinearBackend) -> Result<T, String>,
    {
        DENSE_LINEAR_BACKEND.with(|backend| {
            let mut backend = backend.borrow_mut();
            if backend.is_none() {
                *backend = Some(CudaDenseLinearBackend::load()?);
            }
            f(backend
                .as_mut()
                .expect("dense CUDA linear backend was just initialized"))
        })
    }

    pub fn try_matmul_nt_ggml_bytes_cached<F>(
        a: &[f32],
        bt_ggml_type: u32,
        m: usize,
        k: usize,
        n: usize,
        cache_namespace: &str,
        bt_cache_key: &str,
        load_bt_bytes: F,
    ) -> Result<Vec<f32>, String>
    where
        F: FnOnce() -> Result<Vec<u8>, String>,
    {
        if matches!(
            bt_ggml_type,
            GGML_TYPE_BF16 | GGML_TYPE_F16 | GGML_TYPE_F8_E4M3
        ) {
            return with_dense_linear_backend(|backend| {
                backend.matmul_nt_half_cached(
                    Some(a),
                    None,
                    bt_ggml_type,
                    m,
                    k,
                    n,
                    cache_namespace,
                    bt_cache_key,
                    load_bt_bytes,
                )
            });
        }

        thread_local! {
            static GGML_CUDA_BACKEND: RefCell<Option<CudaGgmlBackend>> = const { RefCell::new(None) };
        }

        GGML_CUDA_BACKEND.with(|backend| {
            let mut backend = backend.borrow_mut();
            if backend.is_none() {
                *backend = Some(CudaGgmlBackend::load()?);
            }
            backend
                .as_mut()
                .expect("ggml CUDA backend was just initialized")
                .matmul_nt_ggml_bytes_cached(
                    a,
                    bt_ggml_type,
                    m,
                    k,
                    n,
                    cache_namespace,
                    bt_cache_key,
                    load_bt_bytes,
                )
        })
    }

    pub fn try_matmul_nt_ggml_bytes(
        a: &[f32],
        bt_bytes: &[u8],
        bt_ggml_type: u32,
        m: usize,
        k: usize,
        n: usize,
    ) -> Option<Vec<f32>> {
        if bt_ggml_type != GGML_TYPE_BF16 {
            return None;
        }
        if a.len() != m.checked_mul(k)? {
            return None;
        }
        if bt_bytes.len() != n.checked_mul(k)?.checked_mul(size_of::<u16>())? {
            return None;
        }

        thread_local! {
            static F32_CUDA_BACKEND: RefCell<Option<CudaRuntime>> = const { RefCell::new(None) };
        }

        let result = F32_CUDA_BACKEND.with(|backend| -> Result<Vec<f32>, String> {
            let mut backend = backend.borrow_mut();
            if backend.is_none() {
                *backend = Some(CudaRuntime::load()?);
            }
            let cuda = backend
                .as_ref()
                .ok_or_else(|| "CUDA runtime did not initialize".to_string())?;
            let input_bytes = unsafe {
                std::slice::from_raw_parts(a.as_ptr().cast::<u8>(), a.len() * size_of::<f32>())
            };
            let mut bt_f32 = Vec::with_capacity(n * k);
            for bytes in bt_bytes.chunks_exact(size_of::<u16>()) {
                let word = u16::from_le_bytes([bytes[0], bytes[1]]);
                bt_f32.push(bf16_word_to_f32(word));
            }
            let bt_f32_bytes = unsafe {
                std::slice::from_raw_parts(
                    bt_f32.as_ptr().cast::<u8>(),
                    bt_f32.len() * size_of::<f32>(),
                )
            };
            let out_len = m
                .checked_mul(n)
                .ok_or_else(|| "CUDA BF16 matmul output length overflow".to_string())?;
            let input = cuda.load_bytes(input_bytes)?;
            let weight = cuda.load_bytes(bt_f32_bytes)?;
            let output = cuda.alloc_f32(out_len)?;
            cuda.matmul_nt_f32(&input, &weight, &output, m, k, n)?;
            cuda.read_f32s(&output, out_len)
        });
        match result {
            Ok(out) => Some(out),
            Err(err) => {
                if std::env::var_os("MAKEPAD_CUDA_TRACE").is_some() {
                    eprintln!("CUDA BF16 matmul_nt fallback failed: m={m} k={k} n={n}: {err}");
                }
                None
            }
        }
    }

    pub fn try_flash_attn_f32_packed(
        q: &[f32],
        k: &[f32],
        v: &[f32],
        n_q: usize,
        n_kv: usize,
        n_head: usize,
        d: usize,
        scale: f32,
    ) -> Option<Vec<f32>> {
        if n_q != n_kv || q.len() != n_q.checked_mul(n_head)?.checked_mul(d)? {
            return None;
        }
        let len = q.len();
        if k.len() != len || v.len() != len {
            return None;
        }

        thread_local! {
            static FLASH_ATTN_CUDA_BACKEND: RefCell<Option<CudaRuntime>> = const { RefCell::new(None) };
        }

        let result = FLASH_ATTN_CUDA_BACKEND.with(|backend| -> Result<Vec<f32>, String> {
            let mut backend = backend.borrow_mut();
            if backend.is_none() {
                *backend = Some(CudaRuntime::load()?);
            }
            let cuda = backend
                .as_ref()
                .ok_or_else(|| "CUDA runtime did not initialize".to_string())?;
            let q_buf = cuda.load_bytes(f32s_as_bytes(q))?;
            let k_buf = cuda.load_bytes(f32s_as_bytes(k))?;
            let v_buf = cuda.load_bytes(f32s_as_bytes(v))?;
            let out = cuda.alloc_f32(len)?;
            cuda.flash_attn_f32_packed(&q_buf, &k_buf, &v_buf, &out, n_q, n_head, d, scale)?;
            cuda.read_f32s(&out, len)
        });
        match result {
            Ok(out) => Some(out),
            Err(err) => {
                if std::env::var_os("MAKEPAD_CUDA_TRACE").is_some() {
                    eprintln!(
                        "CUDA flash attention fallback failed: n={n_q} heads={n_head} d={d}: {err}"
                    );
                }
                None
            }
        }
    }

    pub fn try_matmul_nt_ggml_bytes_cached_bf16_words<F>(
        input_bf16_words: &[u16],
        bt_ggml_type: u32,
        m: usize,
        k: usize,
        n: usize,
        cache_namespace: &str,
        bt_cache_key: &str,
        load_bt_bytes: F,
    ) -> Result<Vec<f32>, String>
    where
        F: FnOnce() -> Result<Vec<u8>, String>,
    {
        if bt_ggml_type == GGML_TYPE_BF16 {
            return with_dense_linear_backend(|backend| {
                backend.matmul_nt_half_cached(
                    None,
                    Some(input_bf16_words),
                    bt_ggml_type,
                    m,
                    k,
                    n,
                    cache_namespace,
                    bt_cache_key,
                    load_bt_bytes,
                )
            });
        }

        thread_local! {
            static GGML_CUDA_BACKEND: RefCell<Option<CudaGgmlBackend>> = const { RefCell::new(None) };
        }

        GGML_CUDA_BACKEND.with(|backend| {
            let mut backend = backend.borrow_mut();
            if backend.is_none() {
                *backend = Some(CudaGgmlBackend::load()?);
            }
            backend
                .as_mut()
                .expect("ggml CUDA backend was just initialized")
                .matmul_nt_ggml_bytes_cached_bf16_words(
                    input_bf16_words,
                    bt_ggml_type,
                    m,
                    k,
                    n,
                    cache_namespace,
                    bt_cache_key,
                    load_bt_bytes,
                )
        })
    }

    pub fn try_get_rows_ggml_bytes_cached<F>(
        src_ggml_type: u32,
        n_cols: usize,
        n_rows: usize,
        row_indices: &[i32],
        cache_namespace: &str,
        src_cache_key: &str,
        load_src_bytes: F,
    ) -> Result<Vec<f32>, String>
    where
        F: FnOnce() -> Result<Vec<u8>, String>,
    {
        // F8 embeddings ride the DENSE backend: its cross-namespace cache
        // survives the flux warm loop's clip/t5/unet interleave, unlike
        // CudaGgmlBackend's scope-keyed cache which clears on namespace
        // switches (fine for H3's one-shot NVFP4 encode, fatal for a
        // persistent-residency text encoder).
        if src_ggml_type == GGML_TYPE_F8_E4M3 {
            return with_dense_linear_backend(|backend| {
                backend.get_rows_f8_cached(
                    n_cols,
                    n_rows,
                    row_indices,
                    cache_namespace,
                    src_cache_key,
                    load_src_bytes,
                )
            });
        }

        thread_local! {
            static GGML_CUDA_BACKEND: RefCell<Option<CudaGgmlBackend>> = const { RefCell::new(None) };
        }

        GGML_CUDA_BACKEND.with(|backend| {
            let mut backend = backend.borrow_mut();
            if backend.is_none() {
                *backend = Some(CudaGgmlBackend::load()?);
            }
            backend
                .as_mut()
                .expect("ggml CUDA backend was just initialized")
                .get_rows_ggml_bytes_cached(
                    src_ggml_type,
                    n_cols,
                    n_rows,
                    row_indices,
                    cache_namespace,
                    src_cache_key,
                    load_src_bytes,
                )
        })
    }

    fn u16_words_as_le_bytes(words: &[u16]) -> &[u8] {
        #[cfg(target_endian = "little")]
        unsafe {
            std::slice::from_raw_parts(words.as_ptr().cast::<u8>(), words.len() * size_of::<u16>())
        }

        #[cfg(not(target_endian = "little"))]
        {
            unreachable!("u16 byte reinterpreting currently assumes little-endian targets")
        }
    }

    fn f32s_as_bytes(values: &[f32]) -> &[u8] {
        #[cfg(target_endian = "little")]
        unsafe {
            std::slice::from_raw_parts(
                values.as_ptr().cast::<u8>(),
                values.len() * size_of::<f32>(),
            )
        }

        #[cfg(not(target_endian = "little"))]
        {
            unreachable!("f32 byte reinterpreting currently assumes little-endian targets")
        }
    }

    fn bf16_word_to_f32(word: u16) -> f32 {
        f32::from_bits((word as u32) << 16)
    }

    use std::mem::size_of;
}

#[cfg(not(all(any(target_os = "linux", target_os = "windows"), makepad_ai_cuda_kernels)))]
mod imp {}

pub use imp::*;
