#[cfg(all(target_os = "linux", makepad_ggml_cuda_kernels))]
mod imp {
    use crate::backend::{AffineQuantizedMatmulRowsSpec, AffineQuantizedMatmulSpec};
    use crate::quant::{
        quantize_bf16_to_q8_1, quantize_f32_to_q8_1, GGML_TYPE_NVFP4, QK, QK_NVFP4,
    };
    use makepad_cuda::{self, cudaError_t, cudaStream_t};
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::ffi::c_void;
    use std::ptr::NonNull;

    unsafe extern "C" {
        fn makepad_ggml_cuda_affine_qmv_bf16(
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

        fn makepad_ggml_cuda_nvfp4_q8_1_matvec(
            input_q8_1_bytes: *const u8,
            packed_weights_nvfp4_bytes: *const u8,
            output_f32: *mut f32,
            q8_1_blocks: u32,
            out_rows: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_ggml_cuda_nvfp4_get_row_f32(
            packed_weights_nvfp4_bytes: *const u8,
            output_f32: *mut f32,
            n_cols: u32,
            row_index: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_ggml_cuda_quantize_q8_1_f32(
            input_f32: *const f32,
            output_q8_1_bytes: *mut u8,
            n: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_ggml_cuda_scale_f32_inplace(
            values: *mut f32,
            scale: f32,
            n: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_ggml_cuda_add_f32(
            left: *const f32,
            right: *const f32,
            out: *mut f32,
            n: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_ggml_cuda_geglu_split_f32(
            gate_up: *const f32,
            out: *mut f32,
            n: u32,
            split_offset: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_ggml_cuda_rms_norm_row_weighted_f32(
            input: *const f32,
            weights_bf16: *const u16,
            output: *mut f32,
            n: u32,
            eps: f32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_ggml_cuda_rms_norm_rows_weighted_f32(
            input: *const f32,
            weights_bf16: *const u16,
            output: *mut f32,
            row_count: u32,
            row_stride: u32,
            n: u32,
            eps: f32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_ggml_cuda_rms_norm_rows_no_scale_f32(
            input: *const f32,
            output: *mut f32,
            row_count: u32,
            row_stride: u32,
            n: u32,
            eps: f32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_ggml_cuda_rope_rows_f32(
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

        fn makepad_ggml_cuda_kv_append_f32(
            keys: *const f32,
            values: *const f32,
            key_cache: *mut f32,
            value_cache: *mut f32,
            kv_head_count: u32,
            head_dim: u32,
            max_tokens: u32,
            slot: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_ggml_cuda_attention_logits_seq_f32(
            q: *const f32,
            key_cache: *const f32,
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

        fn makepad_ggml_cuda_softmax_rows_f32(
            logits: *const f32,
            probs: *mut f32,
            row_count: u32,
            row_stride: u32,
            seq_len: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;

        fn makepad_ggml_cuda_attention_weighted_sum_f32(
            probs: *const f32,
            value_cache: *const f32,
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

        fn makepad_ggml_cuda_argmax_f32(
            logits: *const f32,
            out_index: *mut u32,
            n: u32,
            stream: cudaStream_t,
        ) -> cudaError_t;
    }

    struct DeviceBuffer {
        ptr: NonNull<c_void>,
        size_bytes: usize,
    }

    impl DeviceBuffer {
        fn new(size_bytes: usize) -> Result<Self, String> {
            let ptr = unsafe { makepad_cuda::malloc(size_bytes) }.map_err(|err| err.to_string())?;
            Ok(Self { ptr, size_bytes })
        }

        fn write(&self, bytes: &[u8], stream: cudaStream_t) -> Result<(), String> {
            if bytes.len() > self.size_bytes {
                return Err(format!(
                    "CUDA buffer overflow on write: {} > {}",
                    bytes.len(),
                    self.size_bytes
                ));
            }
            unsafe {
                makepad_cuda::memcpy_async_host_to_device(
                    self.ptr,
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
                makepad_cuda::memcpy_async_device_to_host(
                    out.as_mut_ptr().cast::<c_void>(),
                    self.ptr,
                    len_bytes,
                    stream,
                )
                .map_err(|err| err.to_string())?;
                makepad_cuda::synchronize_stream(stream).map_err(|err| err.to_string())?;
            }
            Ok(out)
        }

        fn read_f32s(&self, len_values: usize, stream: cudaStream_t) -> Result<Vec<f32>, String> {
            let len_bytes = len_values
                .checked_mul(size_of::<f32>())
                .ok_or_else(|| "CUDA output byte count overflow".to_string())?;
            if len_bytes > self.size_bytes {
                return Err(format!(
                    "CUDA buffer overflow on read: {} > {}",
                    len_bytes, self.size_bytes
                ));
            }
            let mut out = vec![0f32; len_values];
            unsafe {
                makepad_cuda::memcpy_async_device_to_host(
                    out.as_mut_ptr().cast::<c_void>(),
                    self.ptr,
                    len_bytes,
                    stream,
                )
                .map_err(|err| err.to_string())?;
                makepad_cuda::synchronize_stream(stream).map_err(|err| err.to_string())?;
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
                makepad_cuda::memcpy_async_device_to_host(
                    out.as_mut_ptr().cast::<c_void>(),
                    self.ptr,
                    len_bytes,
                    stream,
                )
                .map_err(|err| err.to_string())?;
                makepad_cuda::synchronize_stream(stream).map_err(|err| err.to_string())?;
            }
            Ok(out)
        }
    }

    impl Drop for DeviceBuffer {
        fn drop(&mut self) {
            let _ = unsafe { makepad_cuda::free(self.ptr) };
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
            let device_count = makepad_cuda::device_count().map_err(|err| err.to_string())?;
            if device_count <= 0 {
                return Err("CUDA reported zero devices".to_string());
            }
            let device = 0;
            makepad_cuda::set_device(device).map_err(|err| err.to_string())?;
            let stream =
                makepad_cuda::create_non_blocking_stream().map_err(|err| err.to_string())?;
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
            makepad_cuda::set_device(self.device).map_err(|err| err.to_string())
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
                makepad_ggml_cuda_affine_qmv_bf16(
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
            makepad_cuda::check(status).map_err(|err| err.to_string())?;
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
                    makepad_ggml_cuda_affine_qmv_bf16(
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
                makepad_cuda::check(status).map_err(|err| err.to_string())?;
            }

            let output_words = self
                .ensure_output_buffer(total_output_words)?
                .read_u16_words(total_output_words, stream)?;
            Ok(output_words.into_iter().map(bf16_word_to_f32).collect())
        }
    }

    impl Drop for CudaAffineBackend {
        fn drop(&mut self) {
            let _ = makepad_cuda::destroy_stream(self.stream);
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
            let device_count = makepad_cuda::device_count().map_err(|err| err.to_string())?;
            if device_count <= 0 {
                return Err("CUDA reported zero devices".to_string());
            }
            let device = 0;
            makepad_cuda::set_device(device).map_err(|err| err.to_string())?;
            let stream =
                makepad_cuda::create_non_blocking_stream().map_err(|err| err.to_string())?;
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
            makepad_cuda::set_device(self.device).map_err(|err| err.to_string())
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
                return Err(format!("CUDA NVFP4 matmul expects k divisible by 64, got {k}"));
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
            let output_ptr = {
                self.ensure_output_buffer_f32(n)?
                    .ptr
                    .as_ptr()
                    .cast::<f32>()
            };
            let q8_1_blocks = k / QK;

            let status = unsafe {
                makepad_ggml_cuda_nvfp4_q8_1_matvec(
                    input_ptr,
                    weight_ptr,
                    output_ptr,
                    q8_1_blocks as u32,
                    n as u32,
                    stream,
                )
            };
            makepad_cuda::check(status).map_err(|err| err.to_string())?;
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
                return Err(format!("CUDA NVFP4 matmul expects k divisible by 64, got {k}"));
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
            let output_ptr = {
                self.ensure_output_buffer_f32(n)?
                    .ptr
                    .as_ptr()
                    .cast::<f32>()
            };
            let q8_1_blocks = k / QK;

            let status = unsafe {
                makepad_ggml_cuda_nvfp4_q8_1_matvec(
                    input_ptr,
                    weight_ptr,
                    output_ptr,
                    q8_1_blocks as u32,
                    n as u32,
                    stream,
                )
            };
            makepad_cuda::check(status).map_err(|err| err.to_string())?;
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
                    makepad_ggml_cuda_nvfp4_get_row_f32(
                        weight_ptr,
                        output_ptr.add(row_slot * n_cols),
                        n_cols as u32,
                        row_index as u32,
                        stream,
                    )
                };
                makepad_cuda::check(status).map_err(|err| err.to_string())?;
            }

            self.ensure_output_buffer_f32(total_output)?
                .read_f32s(total_output, stream)
        }
    }

    impl Drop for CudaGgmlBackend {
        fn drop(&mut self) {
            let _ = makepad_cuda::destroy_stream(self.stream);
        }
    }

    pub struct CudaBuffer {
        inner: DeviceBuffer,
    }

    impl CudaBuffer {
        pub fn size_bytes(&self) -> usize {
            self.inner.size_bytes
        }
    }

    pub struct CudaRuntime {
        device: i32,
        stream: cudaStream_t,
    }

    impl CudaRuntime {
        pub fn load() -> Result<Self, String> {
            let device_count = makepad_cuda::device_count().map_err(|err| err.to_string())?;
            if device_count <= 0 {
                return Err("CUDA reported zero devices".to_string());
            }
            let device = 0;
            makepad_cuda::set_device(device).map_err(|err| err.to_string())?;
            let stream =
                makepad_cuda::create_non_blocking_stream().map_err(|err| err.to_string())?;
            Ok(Self { device, stream })
        }

        fn prepare_device(&self) -> Result<(), String> {
            makepad_cuda::set_device(self.device).map_err(|err| err.to_string())
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

        pub fn load_bytes(&self, bytes: &[u8]) -> Result<CudaBuffer, String> {
            let buffer = self.alloc_bytes(bytes.len())?;
            self.write_bytes(&buffer, bytes)?;
            Ok(buffer)
        }

        pub fn write_bytes(&self, buffer: &CudaBuffer, bytes: &[u8]) -> Result<(), String> {
            self.prepare_device()?;
            buffer.inner.write(bytes, self.stream)
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

        pub fn synchronize(&self) -> Result<(), String> {
            self.prepare_device()?;
            makepad_cuda::synchronize_stream(self.stream).map_err(|err| err.to_string())
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
                makepad_ggml_cuda_nvfp4_get_row_f32(
                    weights_nvfp4.inner.ptr.as_ptr().cast::<u8>(),
                    output_f32.inner.ptr.as_ptr().cast::<f32>(),
                    n_cols as u32,
                    row_index as u32,
                    self.stream,
                )
            };
            makepad_cuda::check(status).map_err(|err| err.to_string())
        }

        pub fn quantize_q8_1_f32(
            &self,
            input_f32: &CudaBuffer,
            output_q8_1: &CudaBuffer,
            n: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_ggml_cuda_quantize_q8_1_f32(
                    input_f32.inner.ptr.as_ptr().cast::<f32>(),
                    output_q8_1.inner.ptr.as_ptr().cast::<u8>(),
                    n as u32,
                    self.stream,
                )
            };
            makepad_cuda::check(status).map_err(|err| err.to_string())
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
                makepad_ggml_cuda_nvfp4_q8_1_matvec(
                    input_q8_1.inner.ptr.as_ptr().cast::<u8>(),
                    packed_weights_nvfp4.inner.ptr.as_ptr().cast::<u8>(),
                    output_f32.inner.ptr.as_ptr().cast::<f32>(),
                    q8_1_blocks as u32,
                    out_rows as u32,
                    self.stream,
                )
            };
            makepad_cuda::check(status).map_err(|err| err.to_string())
        }

        pub fn scale_f32_inplace(
            &self,
            values: &CudaBuffer,
            scale: f32,
            n: usize,
        ) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_ggml_cuda_scale_f32_inplace(
                    values.inner.ptr.as_ptr().cast::<f32>(),
                    scale,
                    n as u32,
                    self.stream,
                )
            };
            makepad_cuda::check(status).map_err(|err| err.to_string())
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
                makepad_ggml_cuda_add_f32(
                    left.inner.ptr.as_ptr().cast::<f32>(),
                    right.inner.ptr.as_ptr().cast::<f32>(),
                    out.inner.ptr.as_ptr().cast::<f32>(),
                    n as u32,
                    self.stream,
                )
            };
            makepad_cuda::check(status).map_err(|err| err.to_string())
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
                makepad_ggml_cuda_geglu_split_f32(
                    gate_up.inner.ptr.as_ptr().cast::<f32>(),
                    out.inner.ptr.as_ptr().cast::<f32>(),
                    n as u32,
                    split_offset as u32,
                    self.stream,
                )
            };
            makepad_cuda::check(status).map_err(|err| err.to_string())
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
                makepad_ggml_cuda_rms_norm_row_weighted_f32(
                    input.inner.ptr.as_ptr().cast::<f32>(),
                    weights_bf16.inner.ptr.as_ptr().cast::<u16>(),
                    output.inner.ptr.as_ptr().cast::<f32>(),
                    n as u32,
                    eps,
                    self.stream,
                )
            };
            makepad_cuda::check(status).map_err(|err| err.to_string())
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
                makepad_ggml_cuda_rms_norm_rows_weighted_f32(
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
            makepad_cuda::check(status).map_err(|err| err.to_string())
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
                makepad_ggml_cuda_rms_norm_rows_weighted_f32(
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
            makepad_cuda::check(status).map_err(|err| err.to_string())
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
                makepad_ggml_cuda_rms_norm_rows_no_scale_f32(
                    input.inner.ptr.as_ptr().cast::<f32>(),
                    output.inner.ptr.as_ptr().cast::<f32>(),
                    row_count as u32,
                    row_stride as u32,
                    n as u32,
                    eps,
                    self.stream,
                )
            };
            makepad_cuda::check(status).map_err(|err| err.to_string())
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
                makepad_ggml_cuda_rms_norm_rows_no_scale_f32(
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
            makepad_cuda::check(status).map_err(|err| err.to_string())
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
                makepad_ggml_cuda_rope_rows_f32(
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
            makepad_cuda::check(status).map_err(|err| err.to_string())
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
            self.prepare_device()?;
            let status = unsafe {
                makepad_ggml_cuda_kv_append_f32(
                    keys.inner.ptr.as_ptr().cast::<f32>(),
                    values.inner.ptr.as_ptr().cast::<f32>(),
                    key_cache.inner.ptr.as_ptr().cast::<f32>(),
                    value_cache.inner.ptr.as_ptr().cast::<f32>(),
                    kv_head_count as u32,
                    head_dim as u32,
                    max_tokens as u32,
                    slot as u32,
                    self.stream,
                )
            };
            makepad_cuda::check(status).map_err(|err| err.to_string())
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
                makepad_ggml_cuda_attention_logits_seq_f32(
                    q.inner.ptr.as_ptr().cast::<f32>(),
                    key_cache.inner.ptr.as_ptr().cast::<f32>(),
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
            makepad_cuda::check(status).map_err(|err| err.to_string())
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
                makepad_ggml_cuda_softmax_rows_f32(
                    logits.inner.ptr.as_ptr().cast::<f32>(),
                    probs.inner.ptr.as_ptr().cast::<f32>(),
                    row_count as u32,
                    row_stride as u32,
                    seq_len as u32,
                    self.stream,
                )
            };
            makepad_cuda::check(status).map_err(|err| err.to_string())
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
                makepad_ggml_cuda_attention_weighted_sum_f32(
                    probs.inner.ptr.as_ptr().cast::<f32>(),
                    value_cache.inner.ptr.as_ptr().cast::<f32>(),
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
            makepad_cuda::check(status).map_err(|err| err.to_string())
        }

        pub fn argmax_f32(&self, logits: &CudaBuffer, out_index: &CudaBuffer, n: usize) -> Result<(), String> {
            self.prepare_device()?;
            let status = unsafe {
                makepad_ggml_cuda_argmax_f32(
                    logits.inner.ptr.as_ptr().cast::<f32>(),
                    out_index.inner.ptr.as_ptr().cast::<u32>(),
                    n as u32,
                    self.stream,
                )
            };
            makepad_cuda::check(status).map_err(|err| err.to_string())
        }
    }

    impl Drop for CudaRuntime {
        fn drop(&mut self) {
            let _ = makepad_cuda::destroy_stream(self.stream);
        }
    }

    pub fn supports_affine_quantized_matmul(bits: u32, group_size: u64) -> bool {
        matches!(bits, 4 | 8) && group_size == 64 && makepad_cuda::is_available()
    }

    pub fn is_available() -> bool {
        makepad_cuda::is_available()
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

    fn bf16_word_to_f32(word: u16) -> f32 {
        f32::from_bits((word as u32) << 16)
    }

    use std::mem::size_of;
}

#[cfg(not(all(target_os = "linux", makepad_ggml_cuda_kernels)))]
mod imp {
    use crate::backend::{AffineQuantizedMatmulRowsSpec, AffineQuantizedMatmulSpec};

    pub struct CudaBuffer;

    impl CudaBuffer {
        pub fn size_bytes(&self) -> usize {
            0
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

        pub fn load_bytes(&self, _bytes: &[u8]) -> Result<CudaBuffer, String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn write_bytes(&self, _buffer: &CudaBuffer, _bytes: &[u8]) -> Result<(), String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn read_u32(&self, _buffer: &CudaBuffer) -> Result<u32, String> {
            Err("CUDA runtime is unavailable".to_string())
        }

        pub fn synchronize(&self) -> Result<(), String> {
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

        pub fn quantize_q8_1_f32(
            &self,
            _input_f32: &CudaBuffer,
            _output_q8_1: &CudaBuffer,
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

        pub fn scale_f32_inplace(
            &self,
            _values: &CudaBuffer,
            _scale: f32,
            _n: usize,
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

        pub fn geglu_split_f32(
            &self,
            _gate_up: &CudaBuffer,
            _out: &CudaBuffer,
            _n: usize,
            _split_offset: usize,
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

        pub fn argmax_f32(
            &self,
            _logits: &CudaBuffer,
            _out_index: &CudaBuffer,
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
}

pub use imp::*;
