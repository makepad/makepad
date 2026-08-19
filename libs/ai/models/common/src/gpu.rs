//! Device-resident `gpu_*` surface.
//!
//! On linux/windows with compiled kernels this is `makepad-ai-cuda::launch`.
//! Elsewhere (macOS, or a host without nvcc) the same names resolve to the
//! Metal-backed fallback so family crates can keep a single import path.

#[cfg(all(any(target_os = "linux", target_os = "windows"), makepad_ai_cuda_kernels))]
pub use makepad_ai_cuda::launch::*;

#[cfg(not(all(any(target_os = "linux", target_os = "windows"), makepad_ai_cuda_kernels)))]
mod imp {
    use makepad_ai_cuda::accel::{AffineQuantizedMatmulRowsSpec, AffineQuantizedMatmulSpec};

    pub struct CudaBuffer;
    pub struct CudaMappedHostU32Buffer;
    pub struct CudaGraph;
    pub struct CudaGraphExec;

    impl CudaBuffer {
        pub fn size_bytes(&self) -> usize {
            0
        }

        pub fn device_u32_ptr(&self) -> *const u32 {
            std::ptr::null()
        }

        pub fn device_u32_mut_ptr(&self) -> *mut u32 {
            std::ptr::null_mut()
        }
    }

    impl CudaMappedHostU32Buffer {
        pub fn device_u32_ptr(&self) -> *const u32 {
            std::ptr::null()
        }

        pub fn device_u32_mut_ptr(&self) -> *mut u32 {
            std::ptr::null_mut()
        }

        pub fn write_u32(&self, _index: usize, _value: u32) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn read_u32(&self, _index: usize) -> Result<u32, String> {
            Err("CUDA runtime is unavailable".to_string())
        }
    }

    impl CudaGraph {
        pub fn instantiate(self) -> Result<CudaGraphExec, String> {
            Err("CUDA runtime is unavailable".to_string())
        }
    }

    pub struct CudaRuntime;

    impl CudaRuntime {
        pub fn load() -> Result<Self, String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn alloc_bytes(&self, _size_bytes: usize) -> Result<CudaBuffer, String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn alloc_f32(&self, _len: usize) -> Result<CudaBuffer, String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn alloc_u32(&self, _len: usize) -> Result<CudaBuffer, String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn alloc_mapped_u32(&self, _len: usize) -> Result<CudaMappedHostU32Buffer, String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn load_bytes(&self, _bytes: &[u8]) -> Result<CudaBuffer, String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn write_bytes(&self, _buffer: &CudaBuffer, _bytes: &[u8]) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn zero_bytes(&self, _buffer: &CudaBuffer, _len: usize) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn write_u32(&self, _buffer: &CudaBuffer, _value: u32) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn read_u32(&self, _buffer: &CudaBuffer) -> Result<u32, String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn read_u32s(&self, _buffer: &CudaBuffer, _len: usize) -> Result<Vec<u32>, String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn read_f32s(&self, _buffer: &CudaBuffer, _len: usize) -> Result<Vec<f32>, String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn read_f32s_offset(
            &self,
            _buffer: &CudaBuffer,
            _offset_elems: usize,
            _len: usize,
        ) -> Result<Vec<f32>, String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn read_bytes(&self, _buffer: &CudaBuffer, _len: usize) -> Result<Vec<u8>, String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn matmul_nt_f32(
            &self,
            _a: &CudaBuffer,
            _bt: &CudaBuffer,
            _out: &CudaBuffer,
            _m: usize,
            _k: usize,
            _n: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn matmul_nn_f32(
            &self,
            _a: &CudaBuffer,
            _b: &CudaBuffer,
            _out: &CudaBuffer,
            _m: usize,
            _k: usize,
            _n: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn synchronize(&self) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn begin_capture(&self) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn end_capture(&self) -> Result<CudaGraph, String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn launch_graph(&self, _graph: &CudaGraphExec) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn nvfp4_get_row_f32(
            &self,
            _weights_nvfp4: &CudaBuffer,
            _output_f32: &CudaBuffer,
            _n_cols: usize,
            _row_index: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn nvfp4_get_row_f32_offset(
            &self,
            _weights_nvfp4: &CudaBuffer,
            _output_f32: &CudaBuffer,
            _output_offset_elems: usize,
            _n_cols: usize,
            _row_index: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn nvfp4_get_row_f32_device_u32(
            &self,
            _weights_nvfp4: &CudaBuffer,
            _output_f32: &CudaBuffer,
            _n_cols: usize,
            _row_index_device_u32: &CudaBuffer,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn nvfp4_get_row_f32_device_u32_ptr(
            &self,
            _weights_nvfp4: &CudaBuffer,
            _output_f32: &CudaBuffer,
            _n_cols: usize,
            _row_index_device_u32: *const u32,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn nvfp4_get_rows_f32_device_u32(
            &self,
            _weights_nvfp4: &CudaBuffer,
            _row_indices_device_u32: &CudaBuffer,
            _output_f32: &CudaBuffer,
            _n_cols: usize,
            _row_count: usize,
            _output_row_stride: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn nvfp4_get_rows_f32_device_u32_ptr(
            &self,
            _weights_nvfp4: &CudaBuffer,
            _row_indices_device_u32: *const u32,
            _output_f32: &CudaBuffer,
            _n_cols: usize,
            _row_count: usize,
            _output_row_stride: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn quantize_q8_1_f32(
            &self,
            _input_f32: &CudaBuffer,
            _output_q8_1: &CudaBuffer,
            _n: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn quantize_q8_1_mmq_f32(
            &self,
            _input_f32: &CudaBuffer,
            _output_q8_1_mmq: &CudaBuffer,
            _n_cols: usize,
            _n_rows: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn quantize_q8_1_mmq_f32_padded(
            &self,
            _input_f32: &CudaBuffer,
            _output_q8_1_mmq: &CudaBuffer,
            _n_cols: usize,
            _n_rows: usize,
            _padded_rows: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn nvfp4_q8_1_mmq_fixup_f32_len(&self) -> Result<usize, String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn quantize_nvfp4_f32(
            &self,
            _input_f32: &CudaBuffer,
            _input_scale: f32,
            _output_nvfp4: &CudaBuffer,
            _n: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn nvfp4_q8_1_matvec(
            &self,
            _input_q8_1: &CudaBuffer,
            _packed_weights_nvfp4: &CudaBuffer,
            _output_f32: &CudaBuffer,
            _q8_1_blocks: usize,
            _out_rows: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn nvfp4_q8_1_matmul_batched(
            &self,
            _input_q8_1: &CudaBuffer,
            _packed_weights_nvfp4: &CudaBuffer,
            _output_f32: &CudaBuffer,
            _q8_1_blocks: usize,
            _out_rows: usize,
            _input_rows: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn nvfp4_nvfp4_matvec(
            &self,
            _input_nvfp4: &CudaBuffer,
            _packed_weights_nvfp4: &CudaBuffer,
            _input_scale: f32,
            _output_f32: &CudaBuffer,
            _nvfp4_blocks: usize,
            _out_rows: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn nvfp4_nvfp4_matmul_batched(
            &self,
            _input_nvfp4: &CudaBuffer,
            _packed_weights_nvfp4: &CudaBuffer,
            _input_scale: f32,
            _output_f32: &CudaBuffer,
            _nvfp4_blocks: usize,
            _out_rows: usize,
            _input_rows: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn nvfp4_q8_1_mmq_matmul_batched(
            &self,
            _input_q8_1_mmq: &CudaBuffer,
            _packed_weights_nvfp4: &CudaBuffer,
            _output_f32: &CudaBuffer,
            _tmp_fixup_f32: &CudaBuffer,
            _tmp_fixup_f32_len: usize,
            _n_cols: usize,
            _out_rows: usize,
            _input_rows: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn scale_f32_inplace(
            &self,
            _values: &CudaBuffer,
            _scale: f32,
            _n: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn scale_f32_inplace_device_f32_index(
            &self,
            _values: &CudaBuffer,
            _scales: &CudaBuffer,
            _scale_index: usize,
            _n: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn f32_to_bf16(
            &self,
            _input: &CudaBuffer,
            _output: &CudaBuffer,
            _n: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn layer_norm_mul_add_f32(
            &self,
            _input: &CudaBuffer,
            _gamma: &CudaBuffer,
            _beta: &CudaBuffer,
            _output: &CudaBuffer,
            _row_count: usize,
            _cols: usize,
            _eps: f32,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn add_f32_precise(
            &self,
            _left: &CudaBuffer,
            _right: &CudaBuffer,
            _output: &CudaBuffer,
            _n: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn mul_f32_precise(
            &self,
            _left: &CudaBuffer,
            _right: &CudaBuffer,
            _output: &CudaBuffer,
            _n: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn mul_rows_vec_f32(
            &self,
            _input: &CudaBuffer,
            _vec: &CudaBuffer,
            _output: &CudaBuffer,
            _row_count: usize,
            _cols: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn gelu_f32_precise(
            &self,
            _input: &CudaBuffer,
            _output: &CudaBuffer,
            _n: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn softmax_rows_precise_f32(
            &self,
            _logits: &CudaBuffer,
            _probs: &CudaBuffer,
            _row_count: usize,
            _row_stride: usize,
            _seq_len: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn conv2d_planar_f32(
            &self,
            _input: &CudaBuffer,
            _weights: &CudaBuffer,
            _bias: &CudaBuffer,
            _output: &CudaBuffer,
            _width: usize,
            _height: usize,
            _in_channels: usize,
            _out_channels: usize,
            _kw: usize,
            _kh: usize,
            _pad_x: usize,
            _pad_y: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn group_norm_planar_f32(
            &self,
            _input: &CudaBuffer,
            _gamma: &CudaBuffer,
            _beta: &CudaBuffer,
            _stats: &CudaBuffer,
            _output: &CudaBuffer,
            _width: usize,
            _height: usize,
            _channels: usize,
            _groups: usize,
            _eps: f32,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn silu_f32_precise(
            &self,
            _input: &CudaBuffer,
            _output: &CudaBuffer,
            _n: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn rms_norm_row_weighted_f32_input_offset_f32weights(
            &self,
            _input: &CudaBuffer,
            _input_offset_elems: usize,
            _weights_f32: &CudaBuffer,
            _output: &CudaBuffer,
            _n: usize,
            _eps: f32,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn affine_qmv_bf16_to_f32_select_planes_fixed8_known_valid_precise(
            &self,
            _input_bf16: &CudaBuffer,
            _packed_weights_u32: &CudaBuffer,
            _scales_bf16: &CudaBuffer,
            _biases_bf16: &CudaBuffer,
            _plane_indices_u32: &CudaBuffer,
            _output_f32: &CudaBuffer,
            _n_in: usize,
            _weight_words_per_row: usize,
            _qparams_per_row: usize,
            _out_rows: usize,
            _weight_words_per_plane: usize,
            _qparams_words_per_plane: usize,
            _bits: u32,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn affine_qmv_bf16_to_f32_select_planes_input_offsets_fixed8_known_valid_precise(
            &self,
            _input_bf16: &CudaBuffer,
            _input_words_per_slot: usize,
            _packed_weights_u32: &CudaBuffer,
            _scales_bf16: &CudaBuffer,
            _biases_bf16: &CudaBuffer,
            _plane_indices_u32: &CudaBuffer,
            _output_f32: &CudaBuffer,
            _n_in: usize,
            _weight_words_per_row: usize,
            _qparams_per_row: usize,
            _out_rows: usize,
            _weight_words_per_plane: usize,
            _qparams_words_per_plane: usize,
            _bits: u32,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn affine_qmv_bf16_to_f32(
            &self,
            _input_bf16: &CudaBuffer,
            _packed_weights_u32: &CudaBuffer,
            _scales_bf16: &CudaBuffer,
            _biases_bf16: &CudaBuffer,
            _output_f32: &CudaBuffer,
            _n_in: usize,
            _weight_words_per_row: usize,
            _qparams_per_row: usize,
            _out_rows: usize,
            _bits: u32,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn affine_qmv_bf16_to_f32_offsets(
            &self,
            _input_bf16: &CudaBuffer,
            _packed_weights_u32: &CudaBuffer,
            _packed_weight_word_offset: usize,
            _scales_bf16: &CudaBuffer,
            _scale_word_offset: usize,
            _biases_bf16: &CudaBuffer,
            _bias_word_offset: usize,
            _output_f32: &CudaBuffer,
            _n_in: usize,
            _weight_words_per_row: usize,
            _qparams_per_row: usize,
            _out_rows: usize,
            _bits: u32,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn affine_qmv_bf16_to_f32_precise(
            &self,
            _input_bf16: &CudaBuffer,
            _packed_weights_u32: &CudaBuffer,
            _scales_bf16: &CudaBuffer,
            _biases_bf16: &CudaBuffer,
            _output_f32: &CudaBuffer,
            _n_in: usize,
            _weight_words_per_row: usize,
            _qparams_per_row: usize,
            _out_rows: usize,
            _bits: u32,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn affine_qmv_bf16_to_f32_rows_precise(
            &self,
            _input_bf16: &CudaBuffer,
            _packed_weights_u32: &CudaBuffer,
            _scales_bf16: &CudaBuffer,
            _biases_bf16: &CudaBuffer,
            _output_f32: &CudaBuffer,
            _n_in: usize,
            _weight_words_per_row: usize,
            _qparams_per_row: usize,
            _out_rows: usize,
            _input_rows: usize,
            _bits: u32,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn affine_qmv_bf16_to_f32_offsets_precise(
            &self,
            _input_bf16: &CudaBuffer,
            _packed_weights_u32: &CudaBuffer,
            _packed_weight_word_offset: usize,
            _scales_bf16: &CudaBuffer,
            _scale_word_offset: usize,
            _biases_bf16: &CudaBuffer,
            _bias_word_offset: usize,
            _output_f32: &CudaBuffer,
            _n_in: usize,
            _weight_words_per_row: usize,
            _qparams_per_row: usize,
            _out_rows: usize,
            _bits: u32,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn affine_qmv_q8_1_to_f32_precise(
            &self,
            _input_bf16: &CudaBuffer,
            _input_q8_1: &CudaBuffer,
            _packed_weights_u32: &CudaBuffer,
            _packed_weight_word_offset: usize,
            _scales_bf16: &CudaBuffer,
            _scale_word_offset: usize,
            _biases_bf16: &CudaBuffer,
            _bias_word_offset: usize,
            _output_f32: &CudaBuffer,
            _n_in: usize,
            _weight_words_per_row: usize,
            _qparams_per_row: usize,
            _out_rows: usize,
            _bits: u32,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn affine_qmv_bf16_to_f32_select_plane_precise(
            &self,
            _input_bf16: &CudaBuffer,
            _packed_weights_u32: &CudaBuffer,
            _scales_bf16: &CudaBuffer,
            _biases_bf16: &CudaBuffer,
            _plane_indices_u32: &CudaBuffer,
            _plane_slot: usize,
            _output_f32: &CudaBuffer,
            _n_in: usize,
            _weight_words_per_row: usize,
            _qparams_per_row: usize,
            _out_rows: usize,
            _weight_words_per_plane: usize,
            _qparams_words_per_plane: usize,
            _plane_count: usize,
            _bits: u32,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn affine_qmv_bf16_to_f32_select_plane_precise_offsets(
            &self,
            _input_bf16: &CudaBuffer,
            _input_word_offset: usize,
            _packed_weights_u32: &CudaBuffer,
            _scales_bf16: &CudaBuffer,
            _biases_bf16: &CudaBuffer,
            _plane_indices_u32: &CudaBuffer,
            _plane_slot: usize,
            _output_f32: &CudaBuffer,
            _output_float_offset: usize,
            _n_in: usize,
            _weight_words_per_row: usize,
            _qparams_per_row: usize,
            _out_rows: usize,
            _weight_words_per_plane: usize,
            _qparams_words_per_plane: usize,
            _plane_count: usize,
            _bits: u32,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn affine_qmv_bf16_to_f32_select_plane_rows_precise(
            &self,
            _input_bf16: &CudaBuffer,
            _packed_weights_u32: &CudaBuffer,
            _scales_bf16: &CudaBuffer,
            _biases_bf16: &CudaBuffer,
            _plane_indices_u32: &CudaBuffer,
            _plane_indices_row_stride: usize,
            _plane_slot: usize,
            _output_f32: &CudaBuffer,
            _n_in: usize,
            _weight_words_per_row: usize,
            _qparams_per_row: usize,
            _out_rows: usize,
            _input_rows: usize,
            _weight_words_per_plane: usize,
            _qparams_words_per_plane: usize,
            _plane_count: usize,
            _bits: u32,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn affine_qmv_bf16_to_f32_select_plane_rows_precise_offsets(
            &self,
            _input_bf16: &CudaBuffer,
            _packed_weights_u32: &CudaBuffer,
            _scales_bf16: &CudaBuffer,
            _biases_bf16: &CudaBuffer,
            _plane_indices_u32: &CudaBuffer,
            _plane_indices_row_stride: usize,
            _plane_slot: usize,
            _output_f32: &CudaBuffer,
            _output_float_offset: usize,
            _n_in: usize,
            _weight_words_per_row: usize,
            _qparams_per_row: usize,
            _out_rows: usize,
            _input_rows: usize,
            _weight_words_per_plane: usize,
            _qparams_words_per_plane: usize,
            _plane_count: usize,
            _bits: u32,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn affine_qmv_bf16_to_f32_select_planes_precise(
            &self,
            _input_bf16: &CudaBuffer,
            _packed_weights_u32: &CudaBuffer,
            _scales_bf16: &CudaBuffer,
            _biases_bf16: &CudaBuffer,
            _plane_indices_u32: &CudaBuffer,
            _selected_count: usize,
            _output_f32: &CudaBuffer,
            _n_in: usize,
            _weight_words_per_row: usize,
            _qparams_per_row: usize,
            _out_rows: usize,
            _weight_words_per_plane: usize,
            _qparams_words_per_plane: usize,
            _plane_count: usize,
            _bits: u32,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn affine_qmv_bf16_to_f32_select_planes_input_offsets_precise(
            &self,
            _input_bf16: &CudaBuffer,
            _input_words_per_slot: usize,
            _packed_weights_u32: &CudaBuffer,
            _scales_bf16: &CudaBuffer,
            _biases_bf16: &CudaBuffer,
            _plane_indices_u32: &CudaBuffer,
            _selected_count: usize,
            _output_f32: &CudaBuffer,
            _n_in: usize,
            _weight_words_per_row: usize,
            _qparams_per_row: usize,
            _out_rows: usize,
            _weight_words_per_plane: usize,
            _qparams_words_per_plane: usize,
            _plane_count: usize,
            _bits: u32,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn affine_get_row_f32(
            &self,
            _packed_weights_u32: &CudaBuffer,
            _scales_bf16: &CudaBuffer,
            _biases_bf16: &CudaBuffer,
            _output_f32: &CudaBuffer,
            _weight_words_per_row: usize,
            _qparams_per_row: usize,
            _row_index: usize,
            _bits: u32,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn affine_get_row_f32_device_u32(
            &self,
            _packed_weights_u32: &CudaBuffer,
            _scales_bf16: &CudaBuffer,
            _biases_bf16: &CudaBuffer,
            _output_f32: &CudaBuffer,
            _weight_words_per_row: usize,
            _qparams_per_row: usize,
            _row_index_device_u32: &CudaBuffer,
            _bits: u32,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn affine_get_row_f32_device_u32_ptr(
            &self,
            _packed_weights_u32: &CudaBuffer,
            _scales_bf16: &CudaBuffer,
            _biases_bf16: &CudaBuffer,
            _output_f32: &CudaBuffer,
            _weight_words_per_row: usize,
            _qparams_per_row: usize,
            _row_index_device_u32: *const u32,
            _bits: u32,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn add_f32(
            &self,
            _left: &CudaBuffer,
            _right: &CudaBuffer,
            _out: &CudaBuffer,
            _n: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn copy_f32(
            &self,
            _input: &CudaBuffer,
            _input_offset_elems: usize,
            _output: &CudaBuffer,
            _output_offset_elems: usize,
            _n: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn weighted_sum_rows_f32(
            &self,
            _batched_inputs: &CudaBuffer,
            _weights: &CudaBuffer,
            _output: &CudaBuffer,
            _row_count: usize,
            _input_count: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn weighted_sum_rows_grouped_f32(
            &self,
            _batched_inputs: &CudaBuffer,
            _weights: &CudaBuffer,
            _output: &CudaBuffer,
            _row_count: usize,
            _row_stride: usize,
            _input_count: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn add_scaled_rows_f32(
            &self,
            _input: &CudaBuffer,
            _scales: &CudaBuffer,
            _output: &CudaBuffer,
            _row_count: usize,
            _row_stride: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn add_scaled_rows_f32_indexed(
            &self,
            _input: &CudaBuffer,
            _scales: &CudaBuffer,
            _output: &CudaBuffer,
            _row_count: usize,
            _row_stride: usize,
            _scale_row_stride: usize,
            _scale_column: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn mul_f32(
            &self,
            _left: &CudaBuffer,
            _right: &CudaBuffer,
            _out: &CudaBuffer,
            _n: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn gelu_f32(
            &self,
            _input: &CudaBuffer,
            _out: &CudaBuffer,
            _n: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn geglu_split_f32(
            &self,
            _gate_up: &CudaBuffer,
            _out: &CudaBuffer,
            _n: usize,
            _split_offset: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn geglu_split_f32_rows(
            &self,
            _gate_up: &CudaBuffer,
            _out: &CudaBuffer,
            _row_count: usize,
            _row_stride: usize,
            _n: usize,
            _split_offset: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn ssm_conv_f32(
            &self,
            _src0: &CudaBuffer,
            _src1: &CudaBuffer,
            _dst: &CudaBuffer,
            _d_conv: usize,
            _d_inner: usize,
            _n_tokens: usize,
            _n_seqs: usize,
            _src0_token_stride: usize,
            _src0_seq_stride: usize,
            _src1_inner_stride: usize,
            _dst_token_stride: usize,
            _dst_seq_stride: usize,
            _apply_silu: bool,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn gated_delta_net_f32(
            &self,
            _q: &CudaBuffer,
            _k: &CudaBuffer,
            _v: &CudaBuffer,
            _g: &CudaBuffer,
            _beta: &CudaBuffer,
            _state: &CudaBuffer,
            _dst: &CudaBuffer,
            _sv: usize,
            _h: usize,
            _n_tokens: usize,
            _n_seqs: usize,
            _sq1: usize,
            _sq2: usize,
            _sq3: usize,
            _sv1: usize,
            _sv2: usize,
            _sv3: usize,
            _sb1: usize,
            _sb2: usize,
            _sb3: usize,
            _neqk1: usize,
            _rq3: usize,
            _kda: bool,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn gated_delta_net_f32_state_offset(
            &self,
            _q: &CudaBuffer,
            _k: &CudaBuffer,
            _v: &CudaBuffer,
            _g: &CudaBuffer,
            _beta: &CudaBuffer,
            _state_and_dst: &CudaBuffer,
            _state_offset_elems: usize,
            _sv: usize,
            _h: usize,
            _n_tokens: usize,
            _n_seqs: usize,
            _sq1: usize,
            _sq2: usize,
            _sq3: usize,
            _sv1: usize,
            _sv2: usize,
            _sv3: usize,
            _sb1: usize,
            _sb2: usize,
            _sb3: usize,
            _neqk1: usize,
            _rq3: usize,
            _kda: bool,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn rms_norm_row_weighted_f32(
            &self,
            _input: &CudaBuffer,
            _weights_bf16: &CudaBuffer,
            _output: &CudaBuffer,
            _n: usize,
            _eps: f32,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn rms_norm_row_weighted_f32_f32weights(
            &self,
            _input: &CudaBuffer,
            _weights_f32: &CudaBuffer,
            _output: &CudaBuffer,
            _n: usize,
            _eps: f32,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn rms_norm_row_weighted_f32_f32weights_precise(
            &self,
            _input: &CudaBuffer,
            _weights_f32: &CudaBuffer,
            _output: &CudaBuffer,
            _n: usize,
            _eps: f32,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn rms_norm_row_weighted_f32_input_offset(
            &self,
            _input: &CudaBuffer,
            _input_offset_elems: usize,
            _weights_bf16: &CudaBuffer,
            _output: &CudaBuffer,
            _n: usize,
            _eps: f32,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn rms_norm_rows_weighted_f32(
            &self,
            _input: &CudaBuffer,
            _weights_bf16: &CudaBuffer,
            _output: &CudaBuffer,
            _row_count: usize,
            _row_stride: usize,
            _n: usize,
            _eps: f32,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn rms_norm_rows_weighted_f32_f32weights(
            &self,
            _input: &CudaBuffer,
            _weights_f32: &CudaBuffer,
            _output: &CudaBuffer,
            _row_count: usize,
            _row_stride: usize,
            _n: usize,
            _eps: f32,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn rms_norm_rows_weighted_f32_f32weights_precise(
            &self,
            _input: &CudaBuffer,
            _weights_f32: &CudaBuffer,
            _output: &CudaBuffer,
            _row_count: usize,
            _row_stride: usize,
            _n: usize,
            _eps: f32,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn rms_norm_rows_weighted_f32_offset_f32weights(
            &self,
            _input: &CudaBuffer,
            _input_offset_elems: usize,
            _weights_f32: &CudaBuffer,
            _output: &CudaBuffer,
            _output_offset_elems: usize,
            _row_count: usize,
            _row_stride: usize,
            _n: usize,
            _eps: f32,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn rms_norm_rows_weighted_f32_offset(
            &self,
            _input: &CudaBuffer,
            _input_offset_elems: usize,
            _weights_bf16: &CudaBuffer,
            _output: &CudaBuffer,
            _output_offset_elems: usize,
            _row_count: usize,
            _row_stride: usize,
            _n: usize,
            _eps: f32,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn rms_norm_rows_no_scale_f32(
            &self,
            _input: &CudaBuffer,
            _output: &CudaBuffer,
            _row_count: usize,
            _row_stride: usize,
            _n: usize,
            _eps: f32,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn rms_norm_rows_no_scale_f32_precise(
            &self,
            _input: &CudaBuffer,
            _output: &CudaBuffer,
            _row_count: usize,
            _row_stride: usize,
            _n: usize,
            _eps: f32,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn rms_norm_rows_no_scale_f32_offset(
            &self,
            _input: &CudaBuffer,
            _input_offset_elems: usize,
            _output: &CudaBuffer,
            _output_offset_elems: usize,
            _row_count: usize,
            _row_stride: usize,
            _n: usize,
            _eps: f32,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn rope_rows_f32(
            &self,
            _input: &CudaBuffer,
            _output: &CudaBuffer,
            _row_count: usize,
            _row_stride: usize,
            _head_dim: usize,
            _rotary_dim: usize,
            _base: f32,
            _position: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn rope_rows_f32_device_u32(
            &self,
            _input: &CudaBuffer,
            _output: &CudaBuffer,
            _row_count: usize,
            _row_stride: usize,
            _head_dim: usize,
            _rotary_dim: usize,
            _base: f32,
            _position_device_u32: &CudaBuffer,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn kv_append_f32(
            &self,
            _keys: &CudaBuffer,
            _values: &CudaBuffer,
            _key_cache: &CudaBuffer,
            _value_cache: &CudaBuffer,
            _kv_head_count: usize,
            _head_dim: usize,
            _max_tokens: usize,
            _slot: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn kv_append_f32_offsets(
            &self,
            _keys: &CudaBuffer,
            _key_offset_elems: usize,
            _values: &CudaBuffer,
            _value_offset_elems: usize,
            _key_cache: &CudaBuffer,
            _value_cache: &CudaBuffer,
            _kv_head_count: usize,
            _head_dim: usize,
            _max_tokens: usize,
            _slot: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn kv_append_f32_device_u32(
            &self,
            _keys: &CudaBuffer,
            _values: &CudaBuffer,
            _key_cache: &CudaBuffer,
            _value_cache: &CudaBuffer,
            _kv_head_count: usize,
            _head_dim: usize,
            _max_tokens: usize,
            _slot_device_u32: &CudaBuffer,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn kv_append_f32_device_u32_ptr(
            &self,
            _keys: &CudaBuffer,
            _values: &CudaBuffer,
            _key_cache: &CudaBuffer,
            _value_cache: &CudaBuffer,
            _kv_head_count: usize,
            _head_dim: usize,
            _max_tokens: usize,
            _slot_device_u32: *const u32,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn kv_append_f32_device_u32_ptr_offsets(
            &self,
            _keys: &CudaBuffer,
            _key_offset_elems: usize,
            _values: &CudaBuffer,
            _value_offset_elems: usize,
            _key_cache: &CudaBuffer,
            _value_cache: &CudaBuffer,
            _kv_head_count: usize,
            _head_dim: usize,
            _max_tokens: usize,
            _slot_device_u32: *const u32,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn qkv_norm_rope_cache_f32(
            &self,
            _qkv: &CudaBuffer,
            _q_weights_bf16: &CudaBuffer,
            _k_weights_bf16: &CudaBuffer,
            _q_out: &CudaBuffer,
            _key_cache: &CudaBuffer,
            _value_cache: &CudaBuffer,
            _q_head_count: usize,
            _k_head_count: usize,
            _head_dim: usize,
            _q_offset: usize,
            _k_offset: usize,
            _v_offset: usize,
            _rotary_dim: usize,
            _base: f32,
            _position: usize,
            _eps: f32,
            _max_tokens: usize,
            _slot: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn qkv_norm_rope_cache_rows_f32(
            &self,
            _qkv: &CudaBuffer,
            _q_weights_bf16: &CudaBuffer,
            _k_weights_bf16: &CudaBuffer,
            _q_out: &CudaBuffer,
            _key_cache: &CudaBuffer,
            _value_cache: &CudaBuffer,
            _q_head_count: usize,
            _k_head_count: usize,
            _head_dim: usize,
            _qkv_row_stride: usize,
            _q_out_row_stride: usize,
            _q_offset: usize,
            _k_offset: usize,
            _v_offset: usize,
            _rotary_dim: usize,
            _base: f32,
            _start_position: usize,
            _eps: f32,
            _max_tokens: usize,
            _start_slot: usize,
            _row_count: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn qkv_norm_rope_cache_f32_device_u32(
            &self,
            _qkv: &CudaBuffer,
            _q_weights_bf16: &CudaBuffer,
            _k_weights_bf16: &CudaBuffer,
            _q_out: &CudaBuffer,
            _key_cache: &CudaBuffer,
            _value_cache: &CudaBuffer,
            _q_head_count: usize,
            _k_head_count: usize,
            _head_dim: usize,
            _q_offset: usize,
            _k_offset: usize,
            _v_offset: usize,
            _rotary_dim: usize,
            _base: f32,
            _position_device_u32: &CudaBuffer,
            _eps: f32,
            _max_tokens: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn qkv_norm_rope_cache_f32_device_u32_ptr(
            &self,
            _qkv: &CudaBuffer,
            _q_weights_bf16: &CudaBuffer,
            _k_weights_bf16: &CudaBuffer,
            _q_out: &CudaBuffer,
            _key_cache: &CudaBuffer,
            _value_cache: &CudaBuffer,
            _q_head_count: usize,
            _k_head_count: usize,
            _head_dim: usize,
            _q_offset: usize,
            _k_offset: usize,
            _v_offset: usize,
            _rotary_dim: usize,
            _base: f32,
            _position_device_u32: *const u32,
            _eps: f32,
            _max_tokens: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn qkv_norm_rope_cache_rows_f32_device_u32(
            &self,
            _qkv: &CudaBuffer,
            _q_weights_bf16: &CudaBuffer,
            _k_weights_bf16: &CudaBuffer,
            _q_out: &CudaBuffer,
            _key_cache: &CudaBuffer,
            _value_cache: &CudaBuffer,
            _q_head_count: usize,
            _k_head_count: usize,
            _head_dim: usize,
            _qkv_row_stride: usize,
            _q_out_row_stride: usize,
            _q_offset: usize,
            _k_offset: usize,
            _v_offset: usize,
            _rotary_dim: usize,
            _base: f32,
            _start_position_device_u32: &CudaBuffer,
            _eps: f32,
            _max_tokens: usize,
            _start_slot_device_u32: &CudaBuffer,
            _row_count: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn attention_logits_seq_f32(
            &self,
            _q: &CudaBuffer,
            _key_cache: &CudaBuffer,
            _logits: &CudaBuffer,
            _q_head_count: usize,
            _q_heads_per_kv: usize,
            _head_dim: usize,
            _kv_row_stride: usize,
            _seq_len: usize,
            _start_slot: usize,
            _capacity: usize,
            _logits_row_stride: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn attention_logits_seq_f32_device_u32(
            &self,
            _q: &CudaBuffer,
            _key_cache: &CudaBuffer,
            _logits: &CudaBuffer,
            _q_head_count: usize,
            _q_heads_per_kv: usize,
            _head_dim: usize,
            _kv_row_stride: usize,
            _seq_len_device_u32: &CudaBuffer,
            _start_slot_device_u32: &CudaBuffer,
            _capacity: usize,
            _logits_row_stride: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn attention_logits_seq_f32_device_u32_ptr(
            &self,
            _q: &CudaBuffer,
            _key_cache: &CudaBuffer,
            _logits: &CudaBuffer,
            _q_head_count: usize,
            _q_heads_per_kv: usize,
            _head_dim: usize,
            _kv_row_stride: usize,
            _seq_len_device_u32: *const u32,
            _start_slot_device_u32: *const u32,
            _capacity: usize,
            _logits_row_stride: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn softmax_rows_f32(
            &self,
            _logits: &CudaBuffer,
            _probs: &CudaBuffer,
            _row_count: usize,
            _row_stride: usize,
            _seq_len: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn softmax_rows_f32_device_u32(
            &self,
            _logits: &CudaBuffer,
            _probs: &CudaBuffer,
            _row_count: usize,
            _row_stride: usize,
            _seq_len_device_u32: &CudaBuffer,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn attention_weighted_sum_f32(
            &self,
            _probs: &CudaBuffer,
            _value_cache: &CudaBuffer,
            _out: &CudaBuffer,
            _q_head_count: usize,
            _q_heads_per_kv: usize,
            _head_dim: usize,
            _kv_row_stride: usize,
            _seq_len: usize,
            _start_slot: usize,
            _capacity: usize,
            _probs_row_stride: usize,
            _out_row_stride: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn attention_weighted_sum_f32_output_offset(
            &self,
            _probs: &CudaBuffer,
            _value_cache: &CudaBuffer,
            _out: &CudaBuffer,
            _output_offset_elems: usize,
            _q_head_count: usize,
            _q_heads_per_kv: usize,
            _head_dim: usize,
            _kv_row_stride: usize,
            _seq_len: usize,
            _start_slot: usize,
            _capacity: usize,
            _probs_row_stride: usize,
            _out_row_stride: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn attention_weighted_sum_f32_device_u32(
            &self,
            _probs: &CudaBuffer,
            _value_cache: &CudaBuffer,
            _out: &CudaBuffer,
            _q_head_count: usize,
            _q_heads_per_kv: usize,
            _head_dim: usize,
            _kv_row_stride: usize,
            _seq_len_device_u32: &CudaBuffer,
            _capacity: usize,
            _probs_row_stride: usize,
            _out_row_stride: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn attention_softmax_weighted_sum_f32(
            &self,
            _logits: &CudaBuffer,
            _value_cache: &CudaBuffer,
            _out: &CudaBuffer,
            _q_head_count: usize,
            _q_heads_per_kv: usize,
            _head_dim: usize,
            _kv_row_stride: usize,
            _seq_len: usize,
            _start_slot: usize,
            _capacity: usize,
            _logits_row_stride: usize,
            _out_row_stride: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn attention_softmax_weighted_sum_f32_output_offset(
            &self,
            _logits: &CudaBuffer,
            _value_cache: &CudaBuffer,
            _out: &CudaBuffer,
            _output_offset_elems: usize,
            _q_head_count: usize,
            _q_heads_per_kv: usize,
            _head_dim: usize,
            _kv_row_stride: usize,
            _seq_len: usize,
            _start_slot: usize,
            _capacity: usize,
            _logits_row_stride: usize,
            _out_row_stride: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn attention_softmax_weighted_sum_f32_device_u32(
            &self,
            _logits: &CudaBuffer,
            _value_cache: &CudaBuffer,
            _out: &CudaBuffer,
            _q_head_count: usize,
            _q_heads_per_kv: usize,
            _head_dim: usize,
            _kv_row_stride: usize,
            _seq_len_device_u32: &CudaBuffer,
            _start_slot_device_u32: &CudaBuffer,
            _capacity: usize,
            _logits_row_stride: usize,
            _out_row_stride: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn attention_softmax_weighted_sum_f32_device_u32_ptr(
            &self,
            _logits: &CudaBuffer,
            _value_cache: &CudaBuffer,
            _out: &CudaBuffer,
            _q_head_count: usize,
            _q_heads_per_kv: usize,
            _head_dim: usize,
            _kv_row_stride: usize,
            _seq_len_device_u32: *const u32,
            _start_slot_device_u32: *const u32,
            _capacity: usize,
            _logits_row_stride: usize,
            _out_row_stride: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn attention_seq_softmax_weighted_sum_f32(
            &self,
            _q: &CudaBuffer,
            _key_cache: &CudaBuffer,
            _value_cache: &CudaBuffer,
            _out: &CudaBuffer,
            _q_head_count: usize,
            _q_heads_per_kv: usize,
            _head_dim: usize,
            _kv_row_stride: usize,
            _seq_len: usize,
            _start_slot: usize,
            _capacity: usize,
            _out_row_stride: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn attention_seq_softmax_weighted_sum_rows_f32(
            &self,
            _q: &CudaBuffer,
            _key_cache: &CudaBuffer,
            _value_cache: &CudaBuffer,
            _out: &CudaBuffer,
            _query_count: usize,
            _q_head_count: usize,
            _q_heads_per_kv: usize,
            _head_dim: usize,
            _kv_row_stride: usize,
            _q_row_stride: usize,
            _out_row_stride: usize,
            _base_seq_len: usize,
            _capacity: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn attention_seq_softmax_weighted_sum_rows_blas_f32(
            &self,
            _q: &CudaBuffer,
            _q_bf16: &CudaBuffer,
            _key_cache: &CudaBuffer,
            _value_cache: &CudaBuffer,
            _logits: &CudaBuffer,
            _probs_bf16: &CudaBuffer,
            _out: &CudaBuffer,
            _query_count: usize,
            _q_head_count: usize,
            _q_heads_per_kv: usize,
            _head_dim: usize,
            _kv_row_stride: usize,
            _q_row_stride: usize,
            _out_row_stride: usize,
            _base_seq_len: usize,
            _capacity: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn attention_seq_softmax_weighted_sum_rows_blas_f32_vision(
            &self,
            _q: &CudaBuffer,
            _q_bf16: &CudaBuffer,
            _key_cache: &CudaBuffer,
            _value_cache: &CudaBuffer,
            _logits: &CudaBuffer,
            _probs_bf16: &CudaBuffer,
            _out: &CudaBuffer,
            _query_count: usize,
            _q_head_count: usize,
            _q_heads_per_kv: usize,
            _head_dim: usize,
            _kv_row_stride: usize,
            _q_row_stride: usize,
            _out_row_stride: usize,
            _base_seq_len: usize,
            _capacity: usize,
            _chunk_start_position: usize,
            _vision_start_position: usize,
            _vision_end_position: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn attention_seq_softmax_weighted_sum_rows_blas_f32_device_u32(
            &self,
            _q: &CudaBuffer,
            _q_bf16: &CudaBuffer,
            _key_cache: &CudaBuffer,
            _value_cache: &CudaBuffer,
            _logits: &CudaBuffer,
            _probs_bf16: &CudaBuffer,
            _out: &CudaBuffer,
            _query_count: usize,
            _q_head_count: usize,
            _q_heads_per_kv: usize,
            _head_dim: usize,
            _kv_row_stride: usize,
            _q_row_stride: usize,
            _out_row_stride: usize,
            _base_seq_len_device_u32: &CudaBuffer,
            _capacity: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn attention_seq_softmax_weighted_sum_rows_blas_f32_device_u32_ptr(
            &self,
            _q: &CudaBuffer,
            _q_bf16: &CudaBuffer,
            _key_cache: &CudaBuffer,
            _value_cache: &CudaBuffer,
            _logits: &CudaBuffer,
            _probs_bf16: &CudaBuffer,
            _out: &CudaBuffer,
            _query_count: usize,
            _q_head_count: usize,
            _q_heads_per_kv: usize,
            _head_dim: usize,
            _kv_row_stride: usize,
            _q_row_stride: usize,
            _out_row_stride: usize,
            _base_seq_len_device_u32: *const u32,
            _capacity: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn attention_seq_softmax_weighted_sum_f32_output_offset(
            &self,
            _q: &CudaBuffer,
            _key_cache: &CudaBuffer,
            _value_cache: &CudaBuffer,
            _out: &CudaBuffer,
            _output_offset_elems: usize,
            _q_head_count: usize,
            _q_heads_per_kv: usize,
            _head_dim: usize,
            _kv_row_stride: usize,
            _seq_len: usize,
            _start_slot: usize,
            _capacity: usize,
            _out_row_stride: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn attention_seq_softmax_weighted_sum_f32_device_u32(
            &self,
            _q: &CudaBuffer,
            _key_cache: &CudaBuffer,
            _value_cache: &CudaBuffer,
            _out: &CudaBuffer,
            _q_head_count: usize,
            _q_heads_per_kv: usize,
            _head_dim: usize,
            _kv_row_stride: usize,
            _seq_len_device_u32: &CudaBuffer,
            _capacity: usize,
            _out_row_stride: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn attention_seq_softmax_weighted_sum_f32_device_u32_ptr(
            &self,
            _q: &CudaBuffer,
            _key_cache: &CudaBuffer,
            _value_cache: &CudaBuffer,
            _out: &CudaBuffer,
            _q_head_count: usize,
            _q_heads_per_kv: usize,
            _head_dim: usize,
            _kv_row_stride: usize,
            _seq_len_device_u32: *const u32,
            _capacity: usize,
            _out_row_stride: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn attention_seq_softmax_weighted_sum_rows_f32_device_u32(
            &self,
            _q: &CudaBuffer,
            _key_cache: &CudaBuffer,
            _value_cache: &CudaBuffer,
            _out: &CudaBuffer,
            _query_count: usize,
            _q_head_count: usize,
            _q_heads_per_kv: usize,
            _head_dim: usize,
            _kv_row_stride: usize,
            _q_row_stride: usize,
            _out_row_stride: usize,
            _base_seq_len_device_u32: &CudaBuffer,
            _capacity: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn argmax_f32(
            &self,
            _logits: &CudaBuffer,
            _out_index: &CudaBuffer,
            _n: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn argmax_f32_ptr(
            &self,
            _logits: &CudaBuffer,
            _out_index_device_u32: *mut u32,
            _n: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn masked_argmax_f32(
            &self,
            _logits: &CudaBuffer,
            _disallowed_token_ids: &CudaBuffer,
            _disallowed_count: usize,
            _out_index: &CudaBuffer,
            _n: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn masked_argmax_f32_device_u32(
            &self,
            _logits: &CudaBuffer,
            _disallowed_token_ids: &CudaBuffer,
            _disallowed_count_device_u32: &CudaBuffer,
            _out_index: &CudaBuffer,
            _n: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn masked_argmax_f32_device_u32_ptr(
            &self,
            _logits: &CudaBuffer,
            _disallowed_token_ids: &CudaBuffer,
            _disallowed_count_device_u32: *const u32,
            _out_index: *mut u32,
            _n: usize,
        ) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }
    }

    pub fn supports_affine_quantized_matmul(_bits: u32, _group_size: u64) -> bool {
        false
    }

    pub fn is_available() -> bool {
        false
    }

    pub fn try_affine_quantized_matmul_bf16<FW, FS, FB>(
        _spec: AffineQuantizedMatmulSpec<'_>,
        _weight_cache_key: &str,
        _scales_cache_key: &str,
        _biases_cache_key: &str,
        _load_weight_bytes: FW,
        _load_scales_bytes: FS,
        _load_biases_bytes: FB,
    ) -> Result<Vec<f32>, String>
    where
        FW: FnOnce() -> Result<Vec<u8>, String>,
        FS: FnOnce() -> Result<Vec<u8>, String>,
        FB: FnOnce() -> Result<Vec<u8>, String>,
    {
        Err("CUDA affine backend is unavailable".to_string())
    }

    pub fn try_affine_quantized_matmul_bf16_rows<FW, FS, FB>(
        _spec: AffineQuantizedMatmulRowsSpec<'_>,
        _weight_cache_key: &str,
        _scales_cache_key: &str,
        _biases_cache_key: &str,
        _load_weight_bytes: FW,
        _load_scales_bytes: FS,
        _load_biases_bytes: FB,
    ) -> Result<Vec<f32>, String>
    where
        FW: FnOnce() -> Result<Vec<u8>, String>,
        FS: FnOnce() -> Result<Vec<u8>, String>,
        FB: FnOnce() -> Result<Vec<u8>, String>,
    {
        Err("CUDA affine backend is unavailable".to_string())
    }

    pub fn try_matmul_nt_ggml_bytes_cached<F>(
        _a: &[f32],
        _bt_ggml_type: u32,
        _m: usize,
        _k: usize,
        _n: usize,
        _cache_namespace: &str,
        _bt_cache_key: &str,
        _load_bt_bytes: F,
    ) -> Result<Vec<f32>, String>
    where
        F: FnOnce() -> Result<Vec<u8>, String>,
    {
        Err("CUDA ggml matmul backend is unavailable".to_string())
    }

    pub fn try_matmul_nt_ggml_bytes(
        _a: &[f32],
        _bt_bytes: &[u8],
        _bt_ggml_type: u32,
        _m: usize,
        _k: usize,
        _n: usize,
    ) -> Option<Vec<f32>> {
        None
    }

    pub fn try_flash_attn_f32_packed(
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

    pub fn try_matmul_nt_ggml_bytes_cached_bf16_words<F>(
        _input_bf16_words: &[u16],
        _bt_ggml_type: u32,
        _m: usize,
        _k: usize,
        _n: usize,
        _cache_namespace: &str,
        _bt_cache_key: &str,
        _load_bt_bytes: F,
    ) -> Result<Vec<f32>, String>
    where
        F: FnOnce() -> Result<Vec<u8>, String>,
    {
        Err("CUDA ggml matmul backend is unavailable".to_string())
    }

    pub fn try_get_rows_ggml_bytes_cached<F>(
        _src_ggml_type: u32,
        _n_cols: usize,
        _n_rows: usize,
        _row_indices: &[i32],
        _cache_namespace: &str,
        _src_cache_key: &str,
        _load_src_bytes: F,
    ) -> Result<Vec<f32>, String>
    where
        F: FnOnce() -> Result<Vec<u8>, String>,
    {
        Err("CUDA ggml get_rows backend is unavailable".to_string())
    }

    // Device-resident tensor API. On macOS the handle is Metal's GpuTensor
    // so the fallbacks below type-check against `gpu_tensor::*`.

    #[cfg(target_os = "macos")]
    pub use makepad_ai_metal::{GpuLinearPart, GpuTensor};

    #[cfg(not(target_os = "macos"))]
    pub struct GpuTensor {
        pub(crate) rows: usize,
        pub(crate) cols: usize,
        pub(crate) data: std::cell::RefCell<Vec<f32>>,
        pub(crate) u32s: std::cell::RefCell<Vec<u32>>,
    }

    #[cfg(not(target_os = "macos"))]
    impl GpuTensor {
        pub fn rows(&self) -> usize {
            self.rows
        }

        pub fn cols(&self) -> usize {
            self.cols
        }

        pub fn is_half(&self) -> bool {
            false
        }
    }

    #[cfg(not(target_os = "macos"))]
    pub struct GpuLinearPart<'a> {
        pub bt_ggml_type: u32,
        pub n: usize,
        pub cache_key: &'a str,
        pub bytes: &'a [u8],
    }

    const GPU_UNAVAILABLE: &str = "CUDA device tensor backend is unavailable";

    pub fn gpu_device_available() -> bool {
        #[cfg(target_os = "macos")]
        {
            return makepad_ai_metal::gpu_tensor::available();
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }

    pub fn gpu_cudnn_available() -> bool {
        false
    }

    pub fn gpu_act_f16_enabled() -> bool {
        false
    }

    pub fn gpu_upload(_values: &[f32], _rows: usize, _cols: usize) -> Result<GpuTensor, String> {
        #[cfg(target_os = "macos")]
        {
            return makepad_ai_metal::gpu_tensor::upload(_values, _rows, _cols);
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(GPU_UNAVAILABLE.to_string())
        }
    }

    pub fn gpu_skintokens_michelangelo_fourier(
        _condition: &GpuTensor,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_attention_packed_flash_bf16(
        _q: &GpuTensor,
        _k: &GpuTensor,
        _v: &GpuTensor,
        _head_count: usize,
        _scale: f32,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_sdpa_flash_f16(
        _q: &GpuTensor,
        _k: &GpuTensor,
        _v: &GpuTensor,
        _batch: usize,
        _heads: usize,
        _scale: f32,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_sdpa_flash_f16_wide_v(
        _q: &GpuTensor,
        _k: &GpuTensor,
        _v_alb: &GpuTensor,
        _v_mr: &GpuTensor,
        _batch: usize,
        _heads: usize,
        _scale: f32,
    ) -> Result<(GpuTensor, GpuTensor), String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_attention_packed_composite_bf16(
        _q: &GpuTensor,
        _k: &GpuTensor,
        _v: &GpuTensor,
        _head_count: usize,
        _scale: f32,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_attention_packed_composite_f32(
        _q: &GpuTensor,
        _k: &GpuTensor,
        _v: &GpuTensor,
        _head_count: usize,
        _scale: f32,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_attention_gqa_decode_bf16(
        _q: &GpuTensor,
        _k: &GpuTensor,
        _v: &GpuTensor,
        _query_heads: usize,
        _kv_heads: usize,
        _scale: f32,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_attention_gqa_decode_pair_bf16(
        _q: &GpuTensor,
        _k_cond: &GpuTensor,
        _v_cond: &GpuTensor,
        _k_uncond: &GpuTensor,
        _v_uncond: &GpuTensor,
        _sequence: usize,
        _query_heads: usize,
        _kv_heads: usize,
        _scale: f32,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_attention_packed_cross_bf16(
        _q: &GpuTensor,
        _k: &GpuTensor,
        _v: &GpuTensor,
        _head_count: usize,
        _scale: f32,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_attention_packed_cross_composite_bf16(
        _q: &GpuTensor,
        _k: &GpuTensor,
        _v: &GpuTensor,
        _head_count: usize,
        _scale: f32,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_linear_nt_cached_f16_f32acc(
        _x: &GpuTensor,
        _cache_namespace: &str,
        _parts: &[GpuLinearPart<'_>],
        _bias: &[f32],
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_rms_norm_mul_bf16(
        _x: &GpuTensor,
        _group_cols: usize,
        _cache_namespace: &str,
        _cache_key: &str,
        _scale: &[f32],
        _eps: f32,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_rms_norm_qwen3(
        _x: &GpuTensor,
        _group_cols: usize,
        _cache_namespace: &str,
        _cache_key: &str,
        _scale: &[f32],
        _eps: f32,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_linear_nt_cached_bf16_f32acc(
        _x: &GpuTensor,
        _cache_namespace: &str,
        _parts: &[GpuLinearPart<'_>],
        _bias: &[f32],
    ) -> Result<GpuTensor, String> {
        #[cfg(target_os = "macos")]
        {
            return makepad_ai_metal::gpu_tensor::linear_nt(_x, _cache_namespace, _parts, _bias);
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(GPU_UNAVAILABLE.to_string())
        }
    }

    pub fn gpu_linear_nt_cached_bf16_mm(
        _x: &GpuTensor,
        _cache_namespace: &str,
        _parts: &[GpuLinearPart<'_>],
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_linear_nt_cached_bf16_bias_epilogue(
        _x: &GpuTensor,
        _cache_namespace: &str,
        _parts: &[GpuLinearPart<'_>],
        _bias: &[f32],
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_download(_tensor: &GpuTensor) -> Result<Vec<f32>, String> {
        #[cfg(target_os = "macos")]
        {
            return makepad_ai_metal::gpu_tensor::download(_tensor);
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(GPU_UNAVAILABLE.to_string())
        }
    }

    pub fn gpu_to_f32(_tensor: &GpuTensor) -> Result<GpuTensor, String> {
        #[cfg(target_os = "macos")]
        {
            return makepad_ai_metal::gpu_tensor::to_f32(_tensor);
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(GPU_UNAVAILABLE.to_string())
        }
    }

    pub struct GpuStepGraph {}

    pub fn gpu_graph_capture<T>(
        _run: impl FnOnce() -> Result<T, String>,
    ) -> Result<(GpuStepGraph, T), String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_graph_launch(_graph: &GpuStepGraph) -> Result<(), String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_upload_into(_tensor: &GpuTensor, _values: &[f32]) -> Result<(), String> {
        #[cfg(target_os = "macos")]
        {
            return makepad_ai_metal::gpu_tensor::upload_into(_tensor, _values);
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(GPU_UNAVAILABLE.to_string())
        }
    }

    pub fn gpu_copy_into(_src: &GpuTensor, _dst: &GpuTensor) -> Result<(), String> {
        #[cfg(target_os = "macos")]
        {
            return makepad_ai_metal::gpu_tensor::copy_into(_src, _dst);
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(GPU_UNAVAILABLE.to_string())
        }
    }

    pub fn gpu_to_f16(_tensor: &GpuTensor) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_linear_nt_cached(
        _x: &GpuTensor,
        _cache_namespace: &str,
        _parts: &[GpuLinearPart<'_>],
        _bias: &[f32],
    ) -> Result<GpuTensor, String> {
        #[cfg(target_os = "macos")]
        {
            return makepad_ai_metal::gpu_tensor::linear_nt(_x, _cache_namespace, _parts, _bias);
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(GPU_UNAVAILABLE.to_string())
        }
    }

    pub fn gpu_linear_nt_cached_f16(
        _x: &GpuTensor,
        _cache_namespace: &str,
        _parts: &[GpuLinearPart<'_>],
        _bias: &[f32],
    ) -> Result<GpuTensor, String> {
        #[cfg(target_os = "macos")]
        {
            return makepad_ai_metal::gpu_tensor::linear_nt(_x, _cache_namespace, _parts, _bias);
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(GPU_UNAVAILABLE.to_string())
        }
    }

    pub fn gpu_gelu_bias_f16(
        _x: &GpuTensor,
        _cache_namespace: &str,
        _cache_key: &str,
        _bias: &[f32],
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_layer_norm_mod_f16(
        _x: &GpuTensor,
        _mods: &GpuTensor,
        _scale_off: usize,
        _shift_off: usize,
        _eps: f32,
    ) -> Result<GpuTensor, String> {
        #[cfg(target_os = "macos")]
        {
            return makepad_ai_metal::gpu_tensor::layer_norm_mod(
                _x, _mods, _scale_off, _shift_off, _eps,
            );
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(GPU_UNAVAILABLE.to_string())
        }
    }

    pub fn gpu_layer_norm_mul_add(
        _x: &GpuTensor,
        _mul: &[f32],
        _add: &[f32],
        _eps: f32,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_layer_norm_mul_add_cached(
        _x: &GpuTensor,
        _cache_namespace: &str,
        _cache_key: &str,
        _mul: &[f32],
        _add: &[f32],
        _eps: f32,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_layer_norm_pytorch(
        _x: &GpuTensor,
        _scale: &[f32],
        _bias: &[f32],
        _eps: f32,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_layer_norm_mul_add_grouped(
        _x: &GpuTensor,
        _group_cols: usize,
        _mul: &[f32],
        _add: &[f32],
        _eps: f32,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_rms_norm_mul(
        _x: &GpuTensor,
        _group_cols: usize,
        _cache_namespace: &str,
        _cache_key: &str,
        _scale: &[f32],
        _eps: f32,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_rms_norm_mul_perhead(
        _x: &GpuTensor,
        _heads: usize,
        _head_dim: usize,
        _cache_namespace: &str,
        _cache_key: &str,
        _scale: &[f32],
        _eps: f32,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_rms_norm_mul_round_bf16(
        _x: &GpuTensor,
        _group_cols: usize,
        _cache_namespace: &str,
        _cache_key: &str,
        _scale: &[f32],
        _eps: f32,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_adaln_mod(
        _normed: &GpuTensor,
        _mods: &GpuTensor,
        _scale_off: usize,
        _shift_off: usize,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_gated_residual_round_add_bf16(
        _h: &GpuTensor,
        _update: &GpuTensor,
        _mods: &GpuTensor,
        _gate_off: usize,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_silu_round_mul_round_bf16(
        _gate: &GpuTensor,
        _up: &GpuTensor,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_rope_half_round_bf16(
        _x: &GpuTensor,
        _head_count: usize,
        _rot_half: usize,
        _cos_table: &GpuTensor,
        _sin_table: &GpuTensor,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_gelu_erf(_x: &GpuTensor) -> Result<GpuTensor, String> {
        #[cfg(target_os = "macos")]
        {
            return makepad_ai_metal::gpu_tensor::gelu_erf(_x);
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(GPU_UNAVAILABLE.to_string())
        }
    }

    pub fn gpu_bf16_round(_x: &GpuTensor) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn gpu_dyt(
        _x: &GpuTensor,
        _width: usize,
        _cache_namespace: &str,
        _cache_key: &str,
        _gamma: &[f32],
        _beta: &[f32],
        _alpha: f32,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_geglu_tanh_value_gate(_x: &GpuTensor) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_attention_packed_softcap(
        _q: &GpuTensor,
        _k: &GpuTensor,
        _v: &GpuTensor,
        _head_count: usize,
        _scale: f32,
        _softcap: f32,
        _key_mask: Option<&GpuTensor>,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_gather_rows_colblock(
        _src: &GpuTensor,
        _row_idx: &GpuTensor,
        _colblock_idx: Option<&GpuTensor>,
        _block_cols: usize,
    ) -> Result<GpuTensor, String> {
        #[cfg(target_os = "macos")]
        {
            return makepad_ai_metal::gpu_tensor::gather_rows_colblock(
                _src, _row_idx, _colblock_idx, _block_cols,
            );
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(GPU_UNAVAILABLE.to_string())
        }
    }

    pub fn gpu_gather_cols(_x: &GpuTensor, _indices: &[u32]) -> Result<GpuTensor, String> {
        #[cfg(target_os = "macos")]
        {
            return makepad_ai_metal::gpu_tensor::gather_cols(_x, _indices);
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(GPU_UNAVAILABLE.to_string())
        }
    }

    pub fn gpu_attention_packed_cross(
        _q: &GpuTensor,
        _k: &GpuTensor,
        _v: &GpuTensor,
        _head_count: usize,
        _scale: f32,
    ) -> Result<GpuTensor, String> {
        #[cfg(target_os = "macos")]
        {
            return makepad_ai_metal::gpu_tensor::attention_packed(
                _q, _k, _v, _head_count, _scale,
            );
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(GPU_UNAVAILABLE.to_string())
        }
    }

    pub fn gpu_reshape(_x: GpuTensor, _rows: usize, _cols: usize) -> Result<GpuTensor, String> {
        #[cfg(target_os = "macos")]
        {
            return makepad_ai_metal::gpu_tensor::reshape(&_x, _rows, _cols);
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(GPU_UNAVAILABLE.to_string())
        }
    }

    pub fn gpu_sam3_sine_embed(_ref_points: &GpuTensor, _half: usize) -> Result<GpuTensor, String> {
        #[cfg(target_os = "macos")]
        {
            return makepad_ai_metal::gpu_tensor::sam3_sine_embed(_ref_points, _half);
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(GPU_UNAVAILABLE.to_string())
        }
    }

    pub fn gpu_sam3_rpb_axial(
        _ref_points: &GpuTensor,
        _width: usize,
        _height: usize,
    ) -> Result<(GpuTensor, GpuTensor), String> {
        #[cfg(target_os = "macos")]
        {
            return makepad_ai_metal::gpu_tensor::sam3_rpb_axial(_ref_points, _width, _height);
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(GPU_UNAVAILABLE.to_string())
        }
    }

    pub fn gpu_sam3_refine_boxes(
        _ref_points: &GpuTensor,
        _delta: &GpuTensor,
    ) -> Result<GpuTensor, String> {
        #[cfg(target_os = "macos")]
        {
            return makepad_ai_metal::gpu_tensor::sam3_refine_boxes(_ref_points, _delta);
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(GPU_UNAVAILABLE.to_string())
        }
    }

    pub fn gpu_attention_packed_flash2_d64(
        _q: &GpuTensor,
        _k: &GpuTensor,
        _v: &GpuTensor,
        _head_count: usize,
        _scale: f32,
    ) -> Result<GpuTensor, String> {
        #[cfg(target_os = "macos")]
        {
            return makepad_ai_metal::gpu_tensor::attention_packed(
                _q, _k, _v, _head_count, _scale,
            );
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(GPU_UNAVAILABLE.to_string())
        }
    }

    pub fn gpu_rpb_expand(
        _ry: &GpuTensor,
        _rx: &GpuTensor,
        _height: usize,
        _width: usize,
        _queries: usize,
        _heads: usize,
    ) -> Result<GpuTensor, String> {
        #[cfg(target_os = "macos")]
        {
            return makepad_ai_metal::gpu_tensor::rpb_expand(
                _ry, _rx, _height, _width, _queries, _heads,
            );
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(GPU_UNAVAILABLE.to_string())
        }
    }

    pub fn gpu_attention_packed_cross_bias(
        _q: &GpuTensor,
        _k: &GpuTensor,
        _v: &GpuTensor,
        _head_count: usize,
        _scale: f32,
        _bias: &GpuTensor,
    ) -> Result<GpuTensor, String> {
        #[cfg(target_os = "macos")]
        {
            return makepad_ai_metal::gpu_tensor::attention_packed_cross_bias(
                _q, _k, _v, _head_count, _scale, _bias,
            );
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(GPU_UNAVAILABLE.to_string())
        }
    }

    pub fn gpu_attention_packed_causal_bf16(
        _q: &GpuTensor,
        _k: &GpuTensor,
        _v: &GpuTensor,
        _head_count: usize,
        _scale: f32,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_attention_packed_causal_f16(
        _q: &GpuTensor,
        _k: &GpuTensor,
        _v: &GpuTensor,
        _head_count: usize,
        _scale: f32,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_sparse_conv27(
        _x: &GpuTensor,
        _neighbors: &GpuTensor,
        _cache_namespace: &str,
        _weight_cache_key: &str,
        _co: usize,
        _bias: &[f32],
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_attention_cross_fused_enabled() -> bool {
        false
    }

    #[derive(Default, Clone, Copy)]
    pub struct GpuPerfStats {
        pub weight_evict_events: u64,
        pub weight_stream_count: u64,
        pub weight_stream_bytes: u64,
        pub pool_oom_clears: u64,
        pub pool_fresh_alloc_count: u64,
        pub pool_fresh_alloc_bytes: u64,
        pub pool_overcap_free_bytes: u64,
        pub mem_free_bytes: u64,
        pub mem_total_bytes: u64,
    }

    pub fn gpu_perf_stats(_reset: bool) -> GpuPerfStats {
        GpuPerfStats::default()
    }

    pub fn gpu_layer_norm_mod(
        _x: &GpuTensor,
        _mods: &GpuTensor,
        _scale_off: usize,
        _shift_off: usize,
        _eps: f32,
    ) -> Result<GpuTensor, String> {
        #[cfg(target_os = "macos")]
        {
            return makepad_ai_metal::gpu_tensor::layer_norm_mod(
                _x, _mods, _scale_off, _shift_off, _eps,
            );
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(GPU_UNAVAILABLE.to_string())
        }
    }

    pub fn gpu_gated_residual_mod(
        _residual: &GpuTensor,
        _update: &GpuTensor,
        _mods: &GpuTensor,
        _gate_off: usize,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_add_bf16(_a: &GpuTensor, _b: &GpuTensor) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_rope_interleaved(
        _x: &GpuTensor,
        _head_count: usize,
        _cos_table: &GpuTensor,
        _sin_table: &GpuTensor,
    ) -> Result<GpuTensor, String> {
        #[cfg(target_os = "macos")]
        {
            return makepad_ai_metal::gpu_tensor::rope_interleaved(
                _x, _head_count, _cos_table, _sin_table,
            );
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(GPU_UNAVAILABLE.to_string())
        }
    }

    pub fn gpu_upload_u32(_values: &[u32]) -> Result<GpuTensor, String> {
        #[cfg(target_os = "macos")]
        {
            return makepad_ai_metal::gpu_tensor::upload_u32(_values);
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(GPU_UNAVAILABLE.to_string())
        }
    }

    pub fn gpu_gemm_f16acc_enabled() -> bool {
        false
    }

    pub fn gpu_rope_half(
        _x: &GpuTensor,
        _head_count: usize,
        _rot_half: usize,
        _cos_table: &GpuTensor,
        _sin_table: &GpuTensor,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_rope_half_bf16(
        _x: &GpuTensor,
        _head_count: usize,
        _rot_half: usize,
        _cos_table: &GpuTensor,
        _sin_table: &GpuTensor,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn gpu_rms_norm_mod_indexed(
        _x: &GpuTensor,
        _weight: &GpuTensor,
        _table: &GpuTensor,
        _idx: &GpuTensor,
        _table_stride: usize,
        _scale_off: usize,
        _shift_off: usize,
        _eps: f32,
        _out_half: bool,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_gated_residual_indexed(
        _residual: &GpuTensor,
        _update: &GpuTensor,
        _table: &GpuTensor,
        _idx: &GpuTensor,
        _table_stride: usize,
        _gate_off: usize,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_swiglu_gate_first(
        _x: &GpuTensor,
        _gate_offset: usize,
        _n: usize,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub struct GpuBf16Buf {
        rows: usize,
        cols: usize,
    }

    impl GpuBf16Buf {
        pub fn rows(&self) -> usize {
            self.rows
        }

        pub fn cols(&self) -> usize {
            self.cols
        }
    }

    pub fn gpu_linear_nt_cached_bf16_mm_from_buf(
        _x: &GpuBf16Buf,
        _cache_namespace: &str,
        _parts: &[GpuLinearPart<'_>],
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_linear_nt_cached_bf16_mm_from_buf_to_buf(
        _x: &GpuBf16Buf,
        _cache_namespace: &str,
        _parts: &[GpuLinearPart<'_>],
    ) -> Result<GpuBf16Buf, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_linear_nt_cached_f8_mm(
        _x: &GpuTensor,
        _cache_namespace: &str,
        _parts: &[GpuLinearPart<'_>],
        _weight_scale: f32,
        _input_scale: Option<f32>,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_linear_nt_cached_f8_mm_from_buf(
        _x: &GpuBf16Buf,
        _cache_namespace: &str,
        _parts: &[GpuLinearPart<'_>],
        _weight_scale: f32,
        _input_scale: Option<f32>,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_linear_nt_cached_f8_mm_from_buf_to_buf(
        _x: &GpuBf16Buf,
        _cache_namespace: &str,
        _parts: &[GpuLinearPart<'_>],
        _weight_scale: f32,
        _input_scale: Option<f32>,
    ) -> Result<GpuBf16Buf, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_stream_ring_setup(_groups: Vec<Vec<(String, Vec<u8>)>>) -> Result<(), String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_stream_ring_active() -> bool {
        false
    }

    pub fn gpu_stream_ring_release_slots() -> Result<(), String> {
        Ok(())
    }

    pub fn gpu_stream_ring_prime() -> Result<(), String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_stream_ring_advance(_group: usize) -> Result<(), String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_layer_norm_mod_to_bf16buf(
        _x: &GpuTensor,
        _mods: &GpuTensor,
        _scale_off: usize,
        _shift_off: usize,
        _eps: f32,
    ) -> Result<GpuBf16Buf, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_bf16buf_slab_to_f32(
        _x: &GpuBf16Buf,
        _col_off: usize,
        _cols: usize,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn gpu_rms_norm_mul_from_bf16_slab(
        _x: &GpuBf16Buf,
        _col_off: usize,
        _cols: usize,
        _group_cols: usize,
        _cache_namespace: &str,
        _cache_key: &str,
        _scale: &[f32],
        _eps: f32,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_swiglu_gate_first_from_bf16(
        _x: &GpuBf16Buf,
        _gate_offset: usize,
        _n: usize,
    ) -> Result<GpuBf16Buf, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_concat_f32rn_bf16buf(
        _a: &GpuTensor,
        _b: &GpuBf16Buf,
    ) -> Result<GpuBf16Buf, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_gated_residual_mod_round_bf16(
        _residual: &GpuTensor,
        _update: &GpuTensor,
        _mods: &GpuTensor,
        _gate_off: usize,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_swiglu_value_gate(_x: &GpuTensor) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_wavenet_gate(_x: &GpuTensor) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_alias_snake_updown2x(
        _x: &GpuTensor,
        _params: &GpuTensor,
        _input_scale: f32,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_linear_f32_resident(
        _x: &GpuTensor,
        _weight: &GpuTensor,
        _bias: Option<&GpuTensor>,
    ) -> Result<GpuTensor, String> {
        #[cfg(target_os = "macos")]
        {
            return makepad_ai_metal::gpu_tensor::linear_f32_resident(_x, _weight, _bias);
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(GPU_UNAVAILABLE.to_string())
        }
    }

    pub fn gpu_weight_cache_ensure<F>(
        _cache_namespace: &str,
        _cache_key: &str,
        _bt_ggml_type: u32,
        _n: usize,
        _k: usize,
        _want_a16: bool,
        _load: F,
    ) -> Result<(), String>
    where
        F: FnOnce() -> Result<Vec<u8>, String>,
    {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_quant_linear_type_supported(_ggml_type: u32) -> bool {
        false
    }

    pub fn gpu_weight_cache_ensure_quant<F>(
        _cache_namespace: &str,
        _cache_key: &str,
        _bt_ggml_type: u32,
        _n: usize,
        _k: usize,
        _load: F,
    ) -> Result<(), String>
    where
        F: FnOnce() -> Result<Vec<u8>, String>,
    {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_weight_cache_protect_prefixes(_prefixes: Vec<String>) -> Result<(), String> {
        // No device cache exists without CUDA; protection is a no-op so the
        // pipeline's residency registration never fails on stub builds.
        Ok(())
    }

    pub fn gpu_weight_cache_evict_prefix(_prefix: &str) -> Result<usize, String> {
        #[cfg(target_os = "macos")]
        {
            return makepad_ai_metal::gpu_tensor::evict_prefix(_prefix);
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(GPU_UNAVAILABLE.to_string())
        }
    }

    pub fn gpu_weight_cache_evict_prefix_if_loaded(_prefix: &str) -> Result<usize, String> {
        Ok(0)
    }

    pub fn gpu_runtime_trim() -> Result<(), String> {
        Ok(())
    }

    pub fn gpu_attention_packed_causal(
        _q: &GpuTensor,
        _k: &GpuTensor,
        _v: &GpuTensor,
        _head_count: usize,
        _scale: f32,
    ) -> Result<GpuTensor, String> {
        #[cfg(target_os = "macos")]
        {
            return makepad_ai_metal::gpu_tensor::attention_packed(
                _q, _k, _v, _head_count, _scale,
            );
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(GPU_UNAVAILABLE.to_string())
        }
    }

    pub fn gpu_attention_packed_causal_f32(
        _q: &GpuTensor,
        _k: &GpuTensor,
        _v: &GpuTensor,
        _head_count: usize,
        _scale: f32,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_attention_packed_causal_flash(
        _q: &GpuTensor,
        _k: &GpuTensor,
        _v: &GpuTensor,
        _head_count: usize,
        _scale: f32,
        _bf16: bool,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_attention_packed_flash_cross(
        _q: &GpuTensor,
        _k: &GpuTensor,
        _v: &GpuTensor,
        _head_count: usize,
        _scale: f32,
        _bf16: bool,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_attention_packed_flash_cross_bf16_rn(
        _q: &GpuTensor,
        _k: &GpuTensor,
        _v: &GpuTensor,
        _head_count: usize,
        _scale: f32,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_attention_packed_flash_cross_bf16pre_f16(
        _q: &GpuTensor,
        _k: &GpuTensor,
        _v: &GpuTensor,
        _head_count: usize,
        _scale: f32,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_attention_packed_motion_text(
        _q: &GpuTensor,
        _k: &GpuTensor,
        _v: &GpuTensor,
        _head_count: usize,
        _scale: f32,
        _motion_tokens: usize,
        _band_radius: usize,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_attention_packed(
        _q: &GpuTensor,
        _k: &GpuTensor,
        _v: &GpuTensor,
        _head_count: usize,
        _scale: f32,
    ) -> Result<GpuTensor, String> {
        #[cfg(target_os = "macos")]
        {
            return makepad_ai_metal::gpu_tensor::attention_packed(
                _q, _k, _v, _head_count, _scale,
            );
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(GPU_UNAVAILABLE.to_string())
        }
    }

    pub fn gpu_attention_packed_f32(
        _q: &GpuTensor,
        _k: &GpuTensor,
        _v: &GpuTensor,
        _head_count: usize,
        _scale: f32,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_attention_packed_sliding(
        _q: &GpuTensor,
        _k: &GpuTensor,
        _v: &GpuTensor,
        _head_count: usize,
        _scale: f32,
        _window: usize,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_attention_packed_fa2_bf16(
        _q: &GpuTensor,
        _k: &GpuTensor,
        _v: &GpuTensor,
        _head_count: usize,
        _scale: f32,
        _window: usize,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_attention_packed_sliding_bf16(
        _q: &GpuTensor,
        _k: &GpuTensor,
        _v: &GpuTensor,
        _head_count: usize,
        _scale: f32,
        _window: usize,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_attention_packed_cross_f32(
        _q: &GpuTensor,
        _k: &GpuTensor,
        _v: &GpuTensor,
        _head_count: usize,
        _scale: f32,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_attention_packed_bf16(
        _q: &GpuTensor,
        _k: &GpuTensor,
        _v: &GpuTensor,
        _head_count: usize,
        _scale: f32,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_gelu(_x: &GpuTensor) -> Result<GpuTensor, String> {
        #[cfg(target_os = "macos")]
        {
            return makepad_ai_metal::gpu_tensor::gelu(_x);
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(GPU_UNAVAILABLE.to_string())
        }
    }

    pub fn gpu_add(_a: &GpuTensor, _b: &GpuTensor) -> Result<GpuTensor, String> {
        #[cfg(target_os = "macos")]
        {
            return makepad_ai_metal::gpu_tensor::add(_a, _b);
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(GPU_UNAVAILABLE.to_string())
        }
    }

    pub fn gpu_snake(
        _x: &GpuTensor,
        _alpha: &[f32],
        _inv_beta: &[f32],
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_tconv_stitch(
        _y_hi: &GpuTensor,
        _y_lo: &GpuTensor,
        _in_len: usize,
        _out_ch: usize,
        _stride: usize,
        _padding: usize,
        _k: usize,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_add_into(
        _a: &GpuTensor,
        _b: &GpuTensor,
        _out: &GpuTensor,
    ) -> Result<(), String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_mul(_a: &GpuTensor, _b: &GpuTensor) -> Result<GpuTensor, String> {
        #[cfg(target_os = "macos")]
        {
            return makepad_ai_metal::gpu_tensor::mul(_a, _b);
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(GPU_UNAVAILABLE.to_string())
        }
    }

    pub fn gpu_add_rows_broadcast(
        _x: &GpuTensor,
        _row_bias: &GpuTensor,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_gated_residual(
        _residual: &GpuTensor,
        _update: &GpuTensor,
        _gate: &[f32],
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_slice_cols(_x: &GpuTensor, _start: usize, _len: usize) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_snake_cols(_x: &GpuTensor, _alpha: &GpuTensor) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_concat_cols(_parts: &[&GpuTensor]) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_slice_rows(_x: &GpuTensor, _start: usize, _len: usize) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_concat_rows(_a: &GpuTensor, _b: &GpuTensor) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_concat_rows_many(_parts: &[&GpuTensor]) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_beam_cache_reorder_append(
        _prior: &GpuTensor,
        _step: &GpuTensor,
        _parents: &[u32],
        _prior_beams: usize,
        _prior_sequence: usize,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_silu(_x: &GpuTensor) -> Result<GpuTensor, String> {
        #[cfg(target_os = "macos")]
        {
            return makepad_ai_metal::gpu_tensor::silu(_x);
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(GPU_UNAVAILABLE.to_string())
        }
    }

    pub fn gpu_planar_to_nchw(
        _x: &GpuTensor,
        _batch: usize,
        _width: usize,
        _height: usize,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_nchw_to_planar(
        _x: &GpuTensor,
        _channels: usize,
        _width: usize,
        _height: usize,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_group_norm_nchw(
        _x: &GpuTensor,
        _channels: usize,
        _width: usize,
        _height: usize,
        _groups: usize,
        _cache_namespace: &str,
        _cache_key: &str,
        _gamma: &[f32],
        _beta: &[f32],
        _eps: f32,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_nchw_group_norm(
        _x: &GpuTensor,
        _channels: usize,
        _width: usize,
        _height: usize,
        _groups: usize,
        _cache_namespace: &str,
        _cache_key: &str,
        _gamma: &[f32],
        _beta: &[f32],
        _eps: f32,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_nchw_to_tokens(
        _x: &GpuTensor,
        _channels: usize,
        _width: usize,
        _height: usize,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_tokens_to_nchw(
        _x: &GpuTensor,
        _batch: usize,
        _width: usize,
        _height: usize,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_nhwc_add_channel(
        _x: &GpuTensor,
        _bias: &GpuTensor,
        _batch: usize,
        _channels: usize,
        _width: usize,
        _height: usize,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_nchw_add_channel(
        _x: &GpuTensor,
        _bias: &GpuTensor,
        _channels: usize,
        _width: usize,
        _height: usize,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_nchw_add_channel_inplace(
        _x: &GpuTensor,
        _bias: &GpuTensor,
        _channels: usize,
        _width: usize,
        _height: usize,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn gpu_conv2d_nchw_packed(
        _x: &GpuTensor,
        _channels: usize,
        _width: usize,
        _height: usize,
        _out_width: usize,
        _out_height: usize,
        _cache_namespace: &str,
        _weight_cache_key: &str,
        _weights: &[f32],
        _bias: &[f32],
        _out_channels: usize,
        _kw: usize,
        _kh: usize,
        _pad_x: usize,
        _pad_y: usize,
        _stride_x: usize,
        _stride_y: usize,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn gpu_conv2d_nchw_cached(
        _x: &GpuTensor,
        _batch: usize,
        _width: usize,
        _height: usize,
        _cache_namespace: &str,
        _weight_cache_key: &str,
        _weights: &[f32],
        _bias: &[f32],
        _out_channels: usize,
        _kw: usize,
        _kh: usize,
        _pad_x: usize,
        _pad_y: usize,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn gpu_conv2d_nhwc_cached(
        _x: &GpuTensor,
        _batch: usize,
        _width: usize,
        _height: usize,
        _cache_namespace: &str,
        _weight_cache_key: &str,
        _weights: &[f32],
        _bias: &[f32],
        _out_channels: usize,
        _kw: usize,
        _kh: usize,
        _pad_x: usize,
        _pad_y: usize,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn gpu_conv2d_nhwc_ex(
        _x: &GpuTensor,
        _batch: usize,
        _width: usize,
        _height: usize,
        _out_width: usize,
        _out_height: usize,
        _cache_namespace: &str,
        _weight_cache_key: &str,
        _weights: &[f32],
        _bias: &[f32],
        _out_channels: usize,
        _kw: usize,
        _kh: usize,
        _pad_x: usize,
        _pad_y: usize,
        _stride_x: usize,
        _stride_y: usize,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn gpu_conv2d_nchw_ex(
        _x: &GpuTensor,
        _batch: usize,
        _width: usize,
        _height: usize,
        _out_width: usize,
        _out_height: usize,
        _cache_namespace: &str,
        _weight_cache_key: &str,
        _weights: &[f32],
        _bias: &[f32],
        _out_channels: usize,
        _kw: usize,
        _kh: usize,
        _pad_x: usize,
        _pad_y: usize,
        _stride_x: usize,
        _stride_y: usize,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_conv2d_planar_cached(
        _x: &GpuTensor,
        _width: usize,
        _height: usize,
        _cache_namespace: &str,
        _weight_cache_key: &str,
        _weights: &[f32],
        _bias: &[f32],
        _out_channels: usize,
        _kw: usize,
        _kh: usize,
        _pad_x: usize,
        _pad_y: usize,
    ) -> Result<GpuTensor, String> {
        #[cfg(target_os = "macos")]
        {
            let _ = (_cache_namespace, _weight_cache_key);
            return makepad_ai_metal::gpu_tensor::conv2d_planar(
                _x,
                _width,
                _height,
                _weights,
                _bias,
                _out_channels,
                _kw,
                _kh,
                _pad_x,
                _pad_y,
            );
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(GPU_UNAVAILABLE.to_string())
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn gpu_conv2d_planar_strided(
        _x: &GpuTensor,
        _in_width: usize,
        _in_height: usize,
        _out_width: usize,
        _out_height: usize,
        _cache_namespace: &str,
        _weight_cache_key: &str,
        _weights: &[f32],
        _bias: &[f32],
        _out_channels: usize,
        _kw: usize,
        _kh: usize,
        _pad_x: usize,
        _pad_y: usize,
        _stride_x: usize,
        _stride_y: usize,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_birefnet_relu(_x: &GpuTensor) -> Result<GpuTensor, String> {
        #[cfg(target_os = "macos")]
        {
            return makepad_ai_metal::gpu_tensor::relu(_x);
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(GPU_UNAVAILABLE.to_string())
        }
    }

    pub fn gpu_realesrgan_lrelu(_x: &GpuTensor, _slope: f32) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_realesrgan_scale_add(
        _base: &GpuTensor,
        _delta: &GpuTensor,
        _scale: f32,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_realesrgan_alloc_f16(_rows: usize, _cols: usize) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    // -- Practical-RIFE v4.26 (libs/ai/cuda/kernels/rife.cu) --

    pub fn gpu_rife_warp(
        _x: &GpuTensor,
        _flow: &GpuTensor,
        _width: usize,
        _height: usize,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn gpu_rife_conv_transpose2d(
        _x: &GpuTensor,
        _in_width: usize,
        _in_height: usize,
        _cache_namespace: &str,
        _weight_cache_key: &str,
        _weights: &[f32],
        _bias: &[f32],
        _out_channels: usize,
        _kw: usize,
        _kh: usize,
        _pad: usize,
        _stride: usize,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_rife_res_conv(
        _conv: &GpuTensor,
        _residual: &GpuTensor,
        _beta: &[f32],
        _slope: f32,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_rife_scale(_x: &GpuTensor, _scale: f32) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_rife_fill(_rows: usize, _cols: usize, _value: f32) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn gpu_splat_repo3d_tables(
        _delta: &GpuTensor,
        _freqs: &GpuTensor,
        _head_count: usize,
        _pairs: usize,
        _dim0: usize,
        _dim1: usize,
    ) -> Result<(GpuTensor, GpuTensor), String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_splat_rope_pairs_per_head(
        _x: &GpuTensor,
        _head_count: usize,
        _cos_table: &GpuTensor,
        _sin_table: &GpuTensor,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn gpu_rife_merge_rgb8(
        _warped0: &GpuTensor,
        _warped1: &GpuTensor,
        _mask: &GpuTensor,
        _padded_width: usize,
        _padded_height: usize,
        _width: usize,
        _height: usize,
    ) -> Result<Vec<u8>, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn gpu_realesrgan_conv3x3_f16(
        _input: &GpuTensor,
        _in_channels: usize,
        _width: usize,
        _height: usize,
        _cache_namespace: &str,
        _weight_cache_key: &str,
        _weights: &[f32],
        _out_channels: usize,
        _output: &GpuTensor,
        _out_row_offset: usize,
    ) -> Result<(), String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn gpu_realesrgan_bias_lrelu_f16(
        _tensor: &GpuTensor,
        _row_offset: usize,
        _channels: usize,
        _cache_namespace: &str,
        _bias_cache_key: &str,
        _bias: &[f32],
        _slope: f32,
    ) -> Result<(), String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn gpu_realesrgan_conv3x3_f32(
        _input: &GpuTensor,
        _in_channels: usize,
        _width: usize,
        _height: usize,
        _cache_namespace: &str,
        _weight_cache_key: &str,
        _weights: &[f32],
        _out_channels: usize,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_realesrgan_bias_lrelu_f32(
        _tensor: &GpuTensor,
        _cache_namespace: &str,
        _bias_cache_key: &str,
        _bias: &[f32],
        _slope: f32,
    ) -> Result<(), String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_realesrgan_quantize_rgb8_f32(_x: &GpuTensor) -> Result<Vec<u8>, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_realesrgan_alloc_f32(_rows: usize, _cols: usize) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn gpu_realesrgan_spine_axpb(
        _base: &GpuTensor,
        _delta32: Option<&GpuTensor>,
        _delta16: Option<(&GpuTensor, usize)>,
        _dst32: &GpuTensor,
        _dst16: Option<(&GpuTensor, usize)>,
        _channels: usize,
        _cache_namespace: &str,
        _bias_cache_key: &str,
        _bias: &[f32],
        _scale: f32,
    ) -> Result<(), String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_birefnet_resize_bilinear(
        _x: &GpuTensor,
        _in_width: usize,
        _in_height: usize,
        _out_width: usize,
        _out_height: usize,
        _align_corners: bool,
    ) -> Result<GpuTensor, String> {
        #[cfg(target_os = "macos")]
        {
            return makepad_ai_metal::gpu_tensor::resize_bilinear(
                _x, _in_width, _in_height, _out_width, _out_height, _align_corners,
            );
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(GPU_UNAVAILABLE.to_string())
        }
    }

    pub fn gpu_birefnet_tokens_to_planar(_x: &GpuTensor) -> Result<GpuTensor, String> {
        #[cfg(target_os = "macos")]
        {
            return makepad_ai_metal::gpu_tensor::tokens_to_planar(_x);
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(GPU_UNAVAILABLE.to_string())
        }
    }

    pub fn gpu_planar_tokens_transpose(_x: &GpuTensor) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_paint_ref_attn_wide_v(
        _q: &GpuTensor,
        _k: &GpuTensor,
        _v_alb: &GpuTensor,
        _v_mr: &GpuTensor,
        _heads: usize,
        _scale: f32,
    ) -> Result<(GpuTensor, GpuTensor), String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_paint_attn_batched_self(
        _q: &GpuTensor,
        _k: &GpuTensor,
        _v: &GpuTensor,
        _batch: usize,
        _heads: usize,
        _scale: f32,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_paint_scale(_x: &GpuTensor, _scale: f32) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_paint_pose_rope(
        _x: &GpuTensor,
        _xyz: &[u32],
        _heads: usize,
        _voxel_res: usize,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_paint_group_norm_batched(
        _x: &GpuTensor,
        _width: usize,
        _height: usize,
        _batch: usize,
        _groups: usize,
        _cache_namespace: &str,
        _cache_key: &str,
        _gamma: &[f32],
        _beta: &[f32],
        _eps: f32,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_paint_pose_rope_dev(
        _x: &GpuTensor,
        _xyz: &GpuTensor,
        _heads: usize,
        _voxel_res: usize,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_pixel_shuffle_planar(
        _x: &GpuTensor,
        _in_width: usize,
        _in_height: usize,
        _out_channels: usize,
        _scale: usize,
        _bias: &[f32],
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn gpu_pixel_shuffle_planar_cached(
        _x: &GpuTensor,
        _in_width: usize,
        _in_height: usize,
        _out_channels: usize,
        _scale: usize,
        _cache_namespace: &str,
        _bias_cache_key: &str,
        _bias: &[f32],
    ) -> Result<GpuTensor, String> {
        #[cfg(target_os = "macos")]
        {
            let _ = (_out_channels, _cache_namespace, _bias_cache_key, _bias);
            return makepad_ai_metal::gpu_tensor::pixel_shuffle(
                _x, _in_width, _in_height, _scale,
            );
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(GPU_UNAVAILABLE.to_string())
        }
    }

    pub fn gpu_birefnet_image_to_patches(
        _image: &GpuTensor,
        _image_width: usize,
        _image_height: usize,
        _out_width: usize,
        _out_height: usize,
    ) -> Result<GpuTensor, String> {
        #[cfg(target_os = "macos")]
        {
            return makepad_ai_metal::gpu_tensor::image_to_patches(
                _image, _image_width, _image_height, _out_width, _out_height,
            );
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(GPU_UNAVAILABLE.to_string())
        }
    }

    pub fn gpu_birefnet_global_avg_pool(_x: &GpuTensor) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_birefnet_broadcast(
        _x: &GpuTensor,
        _plane: usize,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_birefnet_mul_sigmoid_mask(
        _x: &GpuTensor,
        _logits: &GpuTensor,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn gpu_birefnet_swin_attention(
        _q: &GpuTensor,
        _k: &GpuTensor,
        _v: &GpuTensor,
        _cache_namespace: &str,
        _cache_key: &str,
        _relative_bias: &[f32],
        _regions: Option<&GpuTensor>,
        _windows: usize,
        _heads: usize,
        _window_tokens: usize,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn gpu_birefnet_deform_conv2d_cached(
        _x: &GpuTensor,
        _offset: &GpuTensor,
        _modulator: &GpuTensor,
        _width: usize,
        _height: usize,
        _cache_namespace: &str,
        _weight_cache_key: &str,
        _weights: &[f32],
        _bias: &[f32],
        _out_channels: usize,
        _kernel: usize,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn gpu_group_norm_planar(
        _x: &GpuTensor,
        _width: usize,
        _height: usize,
        _groups: usize,
        _cache_namespace: &str,
        _cache_key: &str,
        _gamma: &[f32],
        _beta: &[f32],
        _eps: f32,
    ) -> Result<GpuTensor, String> {
        #[cfg(target_os = "macos")]
        {
            let _ = (_cache_namespace, _cache_key);
            return makepad_ai_metal::gpu_tensor::group_norm_planar(
                _x, _width, _height, _groups, _gamma, _beta, _eps,
            );
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(GPU_UNAVAILABLE.to_string())
        }
    }

    pub fn gpu_upsample_nearest2x(
        _x: &GpuTensor,
        _width: usize,
        _height: usize,
    ) -> Result<GpuTensor, String> {
        #[cfg(target_os = "macos")]
        {
            return makepad_ai_metal::gpu_tensor::upsample_nearest2x(_x, _width, _height);
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(GPU_UNAVAILABLE.to_string())
        }
    }

    pub fn gpu_attention_planar_single(
        _q: &GpuTensor,
        _k: &GpuTensor,
        _v: &GpuTensor,
        _scale: f32,
    ) -> Result<GpuTensor, String> {
        Err(GPU_UNAVAILABLE.to_string())
    }

    pub fn gpu_pool_clear() {}

    pub fn gpu_pool_cap_override(_bytes: Option<usize>) {}
}

#[cfg(not(all(any(target_os = "linux", target_os = "windows"), makepad_ai_cuda_kernels)))]
pub use imp::*;
