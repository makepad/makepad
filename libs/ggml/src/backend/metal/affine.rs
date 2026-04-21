use std::cell::RefCell;
use std::collections::HashMap;
use std::mem::size_of;
use std::slice;

use crate::backend::{AffineQuantizedMatmulRowsSpec, AffineQuantizedMatmulSpec};

use super::{
    BufferStorageMode, MetalBuffer, MetalBufferBindingRef, MetalPipeline, MetalPipelineDescriptor,
    MetalRuntime, MetalSize,
};

#[derive(Clone, Copy)]
#[repr(C)]
struct AffineQmvArgs {
    n_in: u32,
    weight_words_per_row: u32,
    qparams_per_row: u32,
    out_rows: u32,
}

struct AffineMetalBackend {
    runtime: MetalRuntime,
    current_scope: Option<String>,
    tensor_buffers: HashMap<String, MetalBuffer>,
    qmv_pipelines: HashMap<u32, MetalPipeline>,
    input_buffer: Option<MetalBuffer>,
    input_capacity_words: usize,
    output_buffer: Option<MetalBuffer>,
    output_capacity_words: usize,
}

impl AffineMetalBackend {
    fn load() -> Result<Self, String> {
        let runtime =
            MetalRuntime::new().map_err(|err| format!("MetalRuntime::new failed: {err}"))?;
        if !runtime.features().has_bfloat {
            return Err("Metal device does not report BF16 support".to_string());
        }
        Ok(Self {
            runtime,
            current_scope: None,
            tensor_buffers: HashMap::new(),
            qmv_pipelines: HashMap::new(),
            input_buffer: None,
            input_capacity_words: 0,
            output_buffer: None,
            output_capacity_words: 0,
        })
    }

    fn prepare_scope(&mut self, scope: &str) {
        if self.current_scope.as_deref() != Some(scope) {
            self.current_scope = Some(scope.to_owned());
            self.tensor_buffers.clear();
        }
    }

    fn ensure_input_buffer(&mut self, len_words: usize) -> Result<MetalBuffer, String> {
        if self.input_capacity_words < len_words || self.input_buffer.is_none() {
            self.input_buffer = Some(
                self.runtime
                    .create_buffer(len_words * size_of::<u16>(), BufferStorageMode::Shared)
                    .map_err(|err| format!("create input buffer failed: {err}"))?,
            );
            self.input_capacity_words = len_words;
        }
        self.input_buffer
            .as_ref()
            .cloned()
            .ok_or_else(|| "missing affine metal input buffer".to_string())
    }

    fn ensure_output_buffer(&mut self, len_words: usize) -> Result<MetalBuffer, String> {
        if self.output_capacity_words < len_words || self.output_buffer.is_none() {
            self.output_buffer = Some(
                self.runtime
                    .create_buffer(len_words * size_of::<u16>(), BufferStorageMode::Shared)
                    .map_err(|err| format!("create output buffer failed: {err}"))?,
            );
            self.output_capacity_words = len_words;
        }
        self.output_buffer
            .as_ref()
            .cloned()
            .ok_or_else(|| "missing affine metal output buffer".to_string())
    }

    fn qmv_pipeline_name(bits: u32) -> Option<&'static str> {
        match bits {
            4 => Some("kernel_mlx_affine_qmv_row_bf16"),
            8 => Some("kernel_mlx_affine_qmv_row_bf16_q8"),
            _ => None,
        }
    }

    fn qmv_pipeline(&mut self, bits: u32) -> Result<MetalPipeline, String> {
        if let Some(pipeline) = self.qmv_pipelines.get(&bits) {
            return Ok(pipeline.clone());
        }
        let name = Self::qmv_pipeline_name(bits)
            .ok_or_else(|| format!("no affine Metal qmv pipeline for {bits}-bit weights"))?;
        let pipeline = self
            .runtime
            .get_or_compile_pipeline(&MetalPipelineDescriptor {
                cache_name: name.to_string(),
                base_name: name.to_string(),
                constants: Vec::new(),
                smem_bytes: 0,
                nr0: 0,
                nr1: 0,
                nsg: 0,
            })
            .map_err(|err| format!("compile pipeline {name} failed: {err}"))?;
        self.qmv_pipelines.insert(bits, pipeline.clone());
        Ok(pipeline)
    }

    fn cached_tensor_buffer<F>(&mut self, key: &str, load_bytes: F) -> Result<MetalBuffer, String>
    where
        F: FnOnce() -> Result<Vec<u8>, String>,
    {
        if let Some(buffer) = self.tensor_buffers.get(key) {
            return Ok(buffer.clone());
        }
        let bytes = load_bytes()?;
        let buffer = self
            .runtime
            .create_buffer_with_bytes(&bytes, BufferStorageMode::Private)
            .map_err(|err| format!("upload tensor buffer failed: {err}"))?;
        self.tensor_buffers.insert(key.to_owned(), buffer.clone());
        Ok(buffer)
    }

    fn dispatch_qmv(
        &mut self,
        bits: u32,
        args: &AffineQmvArgs,
        input_buffer: &MetalBuffer,
        weight_buffer: &MetalBuffer,
        scales_buffer: &MetalBuffer,
        biases_buffer: &MetalBuffer,
        output_buffer: &MetalBuffer,
    ) -> Result<(), String> {
        let pipeline = self.qmv_pipeline(bits)?;
        self.runtime
            .begin_command_batch()
            .map_err(|err| format!("begin affine metal batch failed: {err}"))?;
        let dispatch_result = self.runtime.dispatch_compute_tracked(
            &pipeline,
            bytes_of_val(args),
            &[
                MetalBufferBindingRef {
                    index: 1,
                    buffer: input_buffer,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: weight_buffer,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: scales_buffer,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 4,
                    buffer: biases_buffer,
                    offset_bytes: 0,
                },
            ],
            &[MetalBufferBindingRef {
                index: 5,
                buffer: output_buffer,
                offset_bytes: 0,
            }],
            &[],
            MetalSize {
                width: 1,
                height: (args.out_rows as u64).div_ceil(8),
                depth: 1,
            },
            MetalSize {
                width: 32,
                height: 2,
                depth: 1,
            },
        );
        if let Err(err) = dispatch_result {
            let _ = self.runtime.discard_command_batch();
            return Err(format!("affine metal qmv dispatch failed: {err}"));
        }
        self.runtime
            .end_command_batch()
            .map_err(|err| format!("end affine metal batch failed: {err}"))?;
        self.runtime
            .wait_idle()
            .map_err(|err| format!("wait affine metal idle failed: {err}"))?;
        Ok(())
    }

    fn dispatch_qmv_tracked(
        &mut self,
        bits: u32,
        args: &AffineQmvArgs,
        input_buffer: &MetalBuffer,
        input_offset_bytes: usize,
        weight_buffer: &MetalBuffer,
        scales_buffer: &MetalBuffer,
        biases_buffer: &MetalBuffer,
        output_buffer: &MetalBuffer,
        output_offset_bytes: usize,
    ) -> Result<(), String> {
        let pipeline = self.qmv_pipeline(bits)?;
        self.runtime
            .dispatch_compute_tracked(
                &pipeline,
                bytes_of_val(args),
                &[
                    MetalBufferBindingRef {
                        index: 1,
                        buffer: input_buffer,
                        offset_bytes: input_offset_bytes,
                    },
                    MetalBufferBindingRef {
                        index: 2,
                        buffer: weight_buffer,
                        offset_bytes: 0,
                    },
                    MetalBufferBindingRef {
                        index: 3,
                        buffer: scales_buffer,
                        offset_bytes: 0,
                    },
                    MetalBufferBindingRef {
                        index: 4,
                        buffer: biases_buffer,
                        offset_bytes: 0,
                    },
                ],
                &[MetalBufferBindingRef {
                    index: 5,
                    buffer: output_buffer,
                    offset_bytes: output_offset_bytes,
                }],
                &[],
                MetalSize {
                    width: 1,
                    height: (args.out_rows as u64).div_ceil(8),
                    depth: 1,
                },
                MetalSize {
                    width: 32,
                    height: 2,
                    depth: 1,
                },
            )
            .map_err(|err| format!("affine metal qmv dispatch failed: {err}"))?;
        Ok(())
    }

    fn read_output_f32(
        &self,
        output_buffer: &MetalBuffer,
        len_words: usize,
    ) -> Result<Vec<f32>, String> {
        let bytes = self
            .runtime
            .read_buffer(output_buffer, len_words * size_of::<u16>())
            .map_err(|err| format!("read affine metal output failed: {err}"))?;
        if bytes.len() != len_words * size_of::<u16>() {
            return Err(format!(
                "affine metal output byte length mismatch: got {} expected {}",
                bytes.len(),
                len_words * size_of::<u16>()
            ));
        }
        let mut out = Vec::with_capacity(len_words);
        for chunk in bytes.chunks_exact(size_of::<u16>()) {
            out.push(bf16_word_to_f32(u16::from_le_bytes([chunk[0], chunk[1]])));
        }
        Ok(out)
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
        self.prepare_scope(spec.cache_namespace);

        if spec.out_rows == 0 {
            return Ok(Vec::new());
        }

        let input_buffer = self.ensure_input_buffer(spec.input_bf16_words.len())?;
        let output_buffer = self.ensure_output_buffer(spec.out_rows)?;
        self.runtime
            .write_buffer(
                &input_buffer,
                0,
                u16_words_as_le_bytes(spec.input_bf16_words),
            )
            .map_err(|err| format!("write affine metal input failed: {err}"))?;

        let weight_buffer = self.cached_tensor_buffer(weight_cache_key, load_weight_bytes)?;
        let scales_buffer = self.cached_tensor_buffer(scales_cache_key, load_scales_bytes)?;
        let biases_buffer = self.cached_tensor_buffer(biases_cache_key, load_biases_bytes)?;
        let args = AffineQmvArgs {
            n_in: spec.input_bf16_words.len() as u32,
            weight_words_per_row: spec.weight_words_per_row as u32,
            qparams_per_row: spec.qparams_per_row as u32,
            out_rows: spec.out_rows as u32,
        };
        self.dispatch_qmv(
            spec.bits,
            &args,
            &input_buffer,
            &weight_buffer,
            &scales_buffer,
            &biases_buffer,
            &output_buffer,
        )?;
        self.read_output_f32(&output_buffer, spec.out_rows)
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
        self.prepare_scope(spec.cache_namespace);

        if spec.input_rows == 0 || spec.out_rows == 0 {
            return Ok(Vec::new());
        }
        if spec.input_bf16_words.len() % spec.input_rows != 0 {
            return Err(format!(
                "affine metal batched input length {} is not divisible by input_rows {}",
                spec.input_bf16_words.len(),
                spec.input_rows
            ));
        }
        let input_row_words = spec.input_bf16_words.len() / spec.input_rows;
        let output_words = spec
            .out_rows
            .checked_mul(spec.input_rows)
            .ok_or_else(|| "affine metal batched output size overflow".to_string())?;
        let input_buffer = self.ensure_input_buffer(spec.input_bf16_words.len())?;
        let output_buffer = self.ensure_output_buffer(output_words)?;
        self.runtime
            .write_buffer(
                &input_buffer,
                0,
                u16_words_as_le_bytes(spec.input_bf16_words),
            )
            .map_err(|err| format!("write affine metal batched input failed: {err}"))?;

        let weight_buffer = self.cached_tensor_buffer(weight_cache_key, load_weight_bytes)?;
        let scales_buffer = self.cached_tensor_buffer(scales_cache_key, load_scales_bytes)?;
        let biases_buffer = self.cached_tensor_buffer(biases_cache_key, load_biases_bytes)?;
        let args = AffineQmvArgs {
            n_in: input_row_words as u32,
            weight_words_per_row: spec.weight_words_per_row as u32,
            qparams_per_row: spec.qparams_per_row as u32,
            out_rows: spec.out_rows as u32,
        };

        self.runtime
            .begin_command_batch()
            .map_err(|err| format!("begin affine metal batched batch failed: {err}"))?;
        for row_idx in 0..spec.input_rows {
            let input_offset_bytes = row_idx
                .checked_mul(input_row_words)
                .and_then(|words| words.checked_mul(size_of::<u16>()))
                .ok_or_else(|| "affine metal batched input offset overflow".to_string())?;
            let output_offset_bytes = row_idx
                .checked_mul(spec.out_rows)
                .and_then(|words| words.checked_mul(size_of::<u16>()))
                .ok_or_else(|| "affine metal batched output offset overflow".to_string())?;
            if let Err(err) = self.dispatch_qmv_tracked(
                spec.bits,
                &args,
                &input_buffer,
                input_offset_bytes,
                &weight_buffer,
                &scales_buffer,
                &biases_buffer,
                &output_buffer,
                output_offset_bytes,
            ) {
                let _ = self.runtime.discard_command_batch();
                return Err(err);
            }
        }
        self.runtime
            .end_command_batch()
            .map_err(|err| format!("end affine metal batched batch failed: {err}"))?;
        self.read_output_f32(&output_buffer, output_words)
    }
}

pub fn supports_affine_quantized_matmul(bits: u32, group_size: u64) -> bool {
    matches!(bits, 4 | 8) && group_size == 64 && MetalRuntime::is_available()
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
        static AFFINE_METAL_BACKEND: RefCell<Option<AffineMetalBackend>> = const { RefCell::new(None) };
    }

    AFFINE_METAL_BACKEND.with(|backend| {
        let mut backend = backend.borrow_mut();
        if backend.is_none() {
            *backend = Some(AffineMetalBackend::load()?);
        }
        backend
            .as_mut()
            .expect("affine metal backend was just initialized")
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
        static AFFINE_METAL_BACKEND: RefCell<Option<AffineMetalBackend>> = const { RefCell::new(None) };
    }

    AFFINE_METAL_BACKEND.with(|backend| {
        let mut backend = backend.borrow_mut();
        if backend.is_none() {
            *backend = Some(AffineMetalBackend::load()?);
        }
        backend
            .as_mut()
            .expect("affine metal backend was just initialized")
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

fn bytes_of_val<T>(value: &T) -> &[u8] {
    unsafe { slice::from_raw_parts((value as *const T).cast::<u8>(), size_of::<T>()) }
}

fn u16_words_as_le_bytes(words: &[u16]) -> &[u8] {
    #[cfg(target_endian = "little")]
    unsafe {
        slice::from_raw_parts(words.as_ptr().cast::<u8>(), words.len() * size_of::<u16>())
    }

    #[cfg(not(target_endian = "little"))]
    {
        unreachable!("u16 byte reinterpreting currently assumes little-endian targets")
    }
}

fn bf16_word_to_f32(word: u16) -> f32 {
    f32::from_bits((word as u32) << 16)
}
