#[cfg(test)]
use crate::layer0_cached_case::ExactMetalGenerationCursor;
use crate::layer0_cached_case::{
    run_layer_sequence_from_inputs, CachedLayerInputs, ExactMetalGenerationGraph,
    ExactMetalGenerationStopReason, ExactMetalTextRuntimeSession, Layer0CachedArtifacts,
    Layer0CachedPlan, Layer0CachedStage,
};
use crate::GemmaKvCacheLayout;
use crate::{GemmaKvCacheSet, KvTensor, KvTensorShape};
use makepad_ggml::backend::metal::{
    try_matmul_nt_ggml_bytes, BufferStorageMode, MetalBuffer, MetalBufferBindingRef,
    MetalPipeline, MetalPipelineDescriptor, MetalRuntime, MetalRuntimeCounters, MetalSize,
};
use makepad_ggml::quant::GGML_TYPE_BF16;
use crate::{MlxDType, MlxGemmaMoeExpertOutput, MlxRouterTopKOutput};
use crate::{MlxGreedyToken, MlxIndexedSafetensors, MlxTokenizer};
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::error::Error;
use std::mem::size_of;
use std::path::{Path, PathBuf};
use std::slice;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const EMBED_TOKENS_WEIGHT_NAME: &str = "language_model.model.embed_tokens.weight";
const EMBED_TOKENS_SCALES_NAME: &str = "language_model.model.embed_tokens.scales";
const EMBED_TOKENS_BIASES_NAME: &str = "language_model.model.embed_tokens.biases";

#[derive(Clone, Copy)]
#[repr(C)]
struct MlxAffineQprojRowArgs {
    n_in: u32,
    weight_words_per_row: u32,
    qparams_per_row: u32,
    out_rows: u32,
}

struct MlxAffineMetalBackend {
    runtime: MetalRuntime,
    current_root: Option<PathBuf>,
    tensor_buffers: HashMap<String, MetalBuffer>,
    qmv_pipelines: HashMap<u32, MetalPipeline>,
    input_buffer: Option<MetalBuffer>,
    input_capacity_words: usize,
    output_buffer: Option<MetalBuffer>,
    output_capacity_words: usize,
}

impl MlxAffineMetalBackend {
    fn load() -> Result<Self, String> {
        let runtime = MetalRuntime::new().map_err(|err| format!("MetalRuntime::new failed: {err}"))?;
        if !runtime.features().has_bfloat {
            return Err("Metal device does not report BF16 support".to_string());
        }
        Ok(Self {
            runtime,
            current_root: None,
            tensor_buffers: HashMap::new(),
            qmv_pipelines: HashMap::new(),
            input_buffer: None,
            input_capacity_words: 0,
            output_buffer: None,
            output_capacity_words: 0,
        })
    }

    fn prepare_model(&mut self, weights: &MlxIndexedSafetensors) {
        let root = &weights.snapshot.paths.root_dir;
        if self.current_root.as_ref() != Some(root) {
            self.current_root = Some(root.clone());
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

    fn cached_tensor_buffer<F>(&mut self, key: String, load_bytes: F) -> Result<MetalBuffer, String>
    where
        F: FnOnce() -> Result<Vec<u8>, String>,
    {
        if let Some(buffer) = self.tensor_buffers.get(&key) {
            return Ok(buffer.clone());
        }
        let bytes = load_bytes()?;
        let buffer = self
            .runtime
            .create_buffer_with_bytes(&bytes, BufferStorageMode::Private)
            .map_err(|err| format!("upload tensor buffer failed: {err}"))?;
        self.tensor_buffers.insert(key, buffer.clone());
        Ok(buffer)
    }

    fn dispatch_qmv(
        &mut self,
        bits: u32,
        args: &MlxAffineQprojRowArgs,
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
        args: &MlxAffineQprojRowArgs,
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

    fn read_output_f32(&self, output_buffer: &MetalBuffer, len_words: usize) -> Result<Vec<f32>, String> {
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

    fn matmul_rank2(
        &mut self,
        weights: &MlxIndexedSafetensors,
        input_words: &[u16],
        weight_name: &str,
        scales_name: &str,
        biases_name: &str,
    ) -> Result<Vec<f32>, String> {
        self.prepare_model(weights);

        let bits = weights.snapshot.config.quantization.bits;
        let group_size = weights.snapshot.config.quantization.group_size as u64;
        let weight_entry = weights.tensor(weight_name).map_err(|err| err.to_string())?;
        let scales_entry = weights.tensor(scales_name).map_err(|err| err.to_string())?;
        let biases_entry = weights.tensor(biases_name).map_err(|err| err.to_string())?;
        validate_affine_rank2_shapes(
            input_words,
            weight_name,
            scales_name,
            biases_name,
            weight_entry,
            scales_entry,
            biases_entry,
            bits,
            group_size,
        )?;

        let rows = weight_entry.shape[0] as usize;
        if rows == 0 {
            return Ok(Vec::new());
        }
        let input_buffer = self.ensure_input_buffer(input_words.len())?;
        let output_buffer = self.ensure_output_buffer(rows)?;
        self.runtime
            .write_buffer(&input_buffer, 0, u16_words_as_le_bytes(input_words))
            .map_err(|err| format!("write affine metal input failed: {err}"))?;

        let root = weights.snapshot.paths.root_dir.to_string_lossy();
        let weight_key = format!("{root}:{weight_name}");
        let scales_key = format!("{root}:{scales_name}");
        let biases_key = format!("{root}:{biases_name}");
        let weight_buffer = self.cached_tensor_buffer(weight_key, || {
            weights.read_tensor_bytes(weight_name).map_err(|err| err.to_string())
        })?;
        let scales_buffer = self.cached_tensor_buffer(scales_key, || {
            weights.read_tensor_bytes(scales_name).map_err(|err| err.to_string())
        })?;
        let biases_buffer = self.cached_tensor_buffer(biases_key, || {
            weights.read_tensor_bytes(biases_name).map_err(|err| err.to_string())
        })?;
        let args = MlxAffineQprojRowArgs {
            n_in: input_words.len() as u32,
            weight_words_per_row: weight_entry.shape[1] as u32,
            qparams_per_row: scales_entry.shape[1] as u32,
            out_rows: rows as u32,
        };
        self.dispatch_qmv(
            bits,
            &args,
            &input_buffer,
            &weight_buffer,
            &scales_buffer,
            &biases_buffer,
            &output_buffer,
        )?;
        self.read_output_f32(&output_buffer, rows)
    }

    fn matmul_rank2_rows(
        &mut self,
        weights: &MlxIndexedSafetensors,
        input_words: &[u16],
        input_rows: usize,
        weight_name: &str,
        scales_name: &str,
        biases_name: &str,
    ) -> Result<Vec<f32>, String> {
        self.prepare_model(weights);

        let bits = weights.snapshot.config.quantization.bits;
        let group_size = weights.snapshot.config.quantization.group_size as u64;
        let weight_entry = weights.tensor(weight_name).map_err(|err| err.to_string())?;
        let scales_entry = weights.tensor(scales_name).map_err(|err| err.to_string())?;
        let biases_entry = weights.tensor(biases_name).map_err(|err| err.to_string())?;
        let values_per_word = 32 / bits as u64;
        let input_row_words = usize::try_from(weight_entry.shape[1] * values_per_word)
            .map_err(|_| "affine metal batched input row width overflow".to_string())?;
        let sample_row = if input_rows == 0 {
            &[][..]
        } else {
            if input_words.len() % input_rows != 0 {
                return Err(format!(
                    "affine metal batched input length {} is not divisible by input_rows {}",
                    input_words.len(),
                    input_rows
                ));
            }
            &input_words[..input_row_words]
        };
        validate_affine_rank2_shapes(
            sample_row,
            weight_name,
            scales_name,
            biases_name,
            weight_entry,
            scales_entry,
            biases_entry,
            bits,
            group_size,
        )?;
        if input_rows == 0 {
            return Ok(Vec::new());
        }
        let expected_words = input_row_words
            .checked_mul(input_rows)
            .ok_or_else(|| "affine metal batched input size overflow".to_string())?;
        if input_words.len() != expected_words {
            return Err(format!(
                "affine metal batched input length mismatch: got {} expected {}",
                input_words.len(),
                expected_words
            ));
        }

        let output_rows = weight_entry.shape[0] as usize;
        if output_rows == 0 {
            return Ok(Vec::new());
        }
        let output_words = output_rows
            .checked_mul(input_rows)
            .ok_or_else(|| "affine metal batched output size overflow".to_string())?;
        let input_buffer = self.ensure_input_buffer(input_words.len())?;
        let output_buffer = self.ensure_output_buffer(output_words)?;
        self.runtime
            .write_buffer(&input_buffer, 0, u16_words_as_le_bytes(input_words))
            .map_err(|err| format!("write affine metal batched input failed: {err}"))?;

        let root = weights.snapshot.paths.root_dir.to_string_lossy();
        let weight_key = format!("{root}:{weight_name}");
        let scales_key = format!("{root}:{scales_name}");
        let biases_key = format!("{root}:{biases_name}");
        let weight_buffer = self.cached_tensor_buffer(weight_key, || {
            weights.read_tensor_bytes(weight_name).map_err(|err| err.to_string())
        })?;
        let scales_buffer = self.cached_tensor_buffer(scales_key, || {
            weights.read_tensor_bytes(scales_name).map_err(|err| err.to_string())
        })?;
        let biases_buffer = self.cached_tensor_buffer(biases_key, || {
            weights.read_tensor_bytes(biases_name).map_err(|err| err.to_string())
        })?;
        let args = MlxAffineQprojRowArgs {
            n_in: input_row_words as u32,
            weight_words_per_row: weight_entry.shape[1] as u32,
            qparams_per_row: scales_entry.shape[1] as u32,
            out_rows: output_rows as u32,
        };

        self.runtime
            .begin_command_batch()
            .map_err(|err| format!("begin affine metal batched batch failed: {err}"))?;
        for row_idx in 0..input_rows {
            let input_offset_bytes = row_idx
                .checked_mul(input_row_words)
                .and_then(|words| words.checked_mul(size_of::<u16>()))
                .ok_or_else(|| "affine metal batched input offset overflow".to_string())?;
            let output_offset_bytes = row_idx
                .checked_mul(output_rows)
                .and_then(|words| words.checked_mul(size_of::<u16>()))
                .ok_or_else(|| "affine metal batched output offset overflow".to_string())?;
            if let Err(err) = self.dispatch_qmv_tracked(
                bits,
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

    fn matmul_rank3_plane(
        &mut self,
        weights: &MlxIndexedSafetensors,
        input_words: &[u16],
        weight_name: &str,
        scales_name: &str,
        biases_name: &str,
        plane: u64,
    ) -> Result<Vec<f32>, String> {
        self.prepare_model(weights);

        let bits = weights.snapshot.config.quantization.bits;
        let group_size = weights.snapshot.config.quantization.group_size as u64;
        let weight_entry = weights.tensor(weight_name).map_err(|err| err.to_string())?;
        let scales_entry = weights.tensor(scales_name).map_err(|err| err.to_string())?;
        let biases_entry = weights.tensor(biases_name).map_err(|err| err.to_string())?;
        validate_affine_rank3_plane_shapes(
            input_words,
            weight_name,
            scales_name,
            biases_name,
            weight_entry,
            scales_entry,
            biases_entry,
            bits,
            group_size,
            plane,
        )?;

        let rows = weight_entry.shape[1] as usize;
        if rows == 0 {
            return Ok(Vec::new());
        }
        let input_buffer = self.ensure_input_buffer(input_words.len())?;
        let output_buffer = self.ensure_output_buffer(rows)?;
        self.runtime
            .write_buffer(&input_buffer, 0, u16_words_as_le_bytes(input_words))
            .map_err(|err| format!("write affine metal plane input failed: {err}"))?;

        let root = weights.snapshot.paths.root_dir.to_string_lossy();
        let weight_key = format!("{root}:{weight_name}@{plane}");
        let scales_key = format!("{root}:{scales_name}@{plane}");
        let biases_key = format!("{root}:{biases_name}@{plane}");
        let weight_buffer = self.cached_tensor_buffer(weight_key, || {
            let header = weights.header_for_tensor(weight_name).map_err(|err| err.to_string())?;
            let words = header
                .read_rank3_plane_u32_words(weight_name, plane)
                .map_err(|err| err.to_string())?;
            Ok(u32_words_as_le_bytes(words.as_slice()).to_vec())
        })?;
        let scales_buffer = self.cached_tensor_buffer(scales_key, || {
            let header = weights.header_for_tensor(scales_name).map_err(|err| err.to_string())?;
            let words = header
                .read_rank3_plane_bf16_words(scales_name, plane)
                .map_err(|err| err.to_string())?;
            Ok(u16_words_as_le_bytes(words.as_slice()).to_vec())
        })?;
        let biases_buffer = self.cached_tensor_buffer(biases_key, || {
            let header = weights.header_for_tensor(biases_name).map_err(|err| err.to_string())?;
            let words = header
                .read_rank3_plane_bf16_words(biases_name, plane)
                .map_err(|err| err.to_string())?;
            Ok(u16_words_as_le_bytes(words.as_slice()).to_vec())
        })?;
        let args = MlxAffineQprojRowArgs {
            n_in: input_words.len() as u32,
            weight_words_per_row: weight_entry.shape[2] as u32,
            qparams_per_row: scales_entry.shape[2] as u32,
            out_rows: rows as u32,
        };
        self.dispatch_qmv(
            bits,
            &args,
            &input_buffer,
            &weight_buffer,
            &scales_buffer,
            &biases_buffer,
            &output_buffer,
        )?;
        self.read_output_f32(&output_buffer, rows)
    }
}

fn validate_affine_rank2_shapes(
    input_words: &[u16],
    weight_name: &str,
    scales_name: &str,
    biases_name: &str,
    weight_entry: &crate::MlxTensorEntry,
    scales_entry: &crate::MlxTensorEntry,
    biases_entry: &crate::MlxTensorEntry,
    bits: u32,
    group_size: u64,
) -> Result<(), String> {
    if weight_entry.dtype != MlxDType::U32 {
        return Err(format!(
            "tensor {weight_name} expected U32 for affine Metal matmul, got {:?}",
            weight_entry.dtype
        ));
    }
    if scales_entry.dtype != MlxDType::BF16 || biases_entry.dtype != MlxDType::BF16 {
        return Err(format!(
            "tensors {scales_name} / {biases_name} expected BF16, got {:?} / {:?}",
            scales_entry.dtype, biases_entry.dtype
        ));
    }
    if weight_entry.shape.len() != 2 || scales_entry.shape.len() != 2 || biases_entry.shape.len() != 2 {
        return Err(format!(
            "affine Metal matmul expects rank-2 tensors, got {:?} {:?} {:?}",
            weight_entry.shape, scales_entry.shape, biases_entry.shape
        ));
    }
    if scales_entry.shape != biases_entry.shape {
        return Err(format!(
            "affine Metal scale/bias shape mismatch: {:?} vs {:?}",
            scales_entry.shape, biases_entry.shape
        ));
    }
    if weight_entry.shape[0] != scales_entry.shape[0] {
        return Err(format!(
            "affine Metal weight/scales outer shape mismatch: {:?} vs {:?}",
            weight_entry.shape, scales_entry.shape
        ));
    }
    let values_per_word = 32 / bits as u64;
    let inner_dim = weight_entry.shape[1] * values_per_word;
    if inner_dim != scales_entry.shape[1] * group_size {
        return Err(format!(
            "affine Metal packed/scales mismatch for group_size={group_size} bits={bits}"
        ));
    }
    if input_words.len() as u64 != inner_dim {
        return Err(format!(
            "affine Metal activation length mismatch: got {} expected {inner_dim}",
            input_words.len()
        ));
    }
    Ok(())
}

fn validate_affine_rank3_plane_shapes(
    input_words: &[u16],
    weight_name: &str,
    scales_name: &str,
    biases_name: &str,
    weight_entry: &crate::MlxTensorEntry,
    scales_entry: &crate::MlxTensorEntry,
    biases_entry: &crate::MlxTensorEntry,
    bits: u32,
    group_size: u64,
    plane: u64,
) -> Result<(), String> {
    if weight_entry.dtype != MlxDType::U32 {
        return Err(format!(
            "tensor {weight_name} expected U32 for affine Metal plane matmul, got {:?}",
            weight_entry.dtype
        ));
    }
    if scales_entry.dtype != MlxDType::BF16 || biases_entry.dtype != MlxDType::BF16 {
        return Err(format!(
            "tensors {scales_name} / {biases_name} expected BF16, got {:?} / {:?}",
            scales_entry.dtype, biases_entry.dtype
        ));
    }
    if weight_entry.shape.len() != 3 || scales_entry.shape.len() != 3 || biases_entry.shape.len() != 3 {
        return Err(format!(
            "affine Metal plane matmul expects rank-3 tensors, got {:?} {:?} {:?}",
            weight_entry.shape, scales_entry.shape, biases_entry.shape
        ));
    }
    if scales_entry.shape != biases_entry.shape {
        return Err(format!(
            "affine Metal plane scale/bias shape mismatch: {:?} vs {:?}",
            scales_entry.shape, biases_entry.shape
        ));
    }
    if weight_entry.shape[0] != scales_entry.shape[0] || weight_entry.shape[1] != scales_entry.shape[1] {
        return Err(format!(
            "affine Metal plane weight/scales outer mismatch: {:?} vs {:?}",
            weight_entry.shape, scales_entry.shape
        ));
    }
    if plane >= weight_entry.shape[0] {
        return Err(format!(
            "plane {plane} out of range for affine Metal tensor {weight_name} with {} planes",
            weight_entry.shape[0]
        ));
    }
    let values_per_word = 32 / bits as u64;
    let inner_dim = weight_entry.shape[2] * values_per_word;
    if inner_dim != scales_entry.shape[2] * group_size {
        return Err(format!(
            "affine Metal plane packed/scales mismatch for group_size={group_size} bits={bits}"
        ));
    }
    if input_words.len() as u64 != inner_dim {
        return Err(format!(
            "affine Metal plane activation length mismatch: got {} expected {inner_dim}",
            input_words.len()
        ));
    }
    Ok(())
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

fn u32_words_as_le_bytes(words: &[u32]) -> &[u8] {
    #[cfg(target_endian = "little")]
    unsafe {
        slice::from_raw_parts(words.as_ptr().cast::<u8>(), words.len() * size_of::<u32>())
    }

    #[cfg(not(target_endian = "little"))]
    {
        unreachable!("u32 byte reinterpreting currently assumes little-endian targets")
    }
}

fn with_affine_metal_backend<T, F>(f: F) -> Option<Result<T, String>>
where
    F: FnOnce(&mut MlxAffineMetalBackend) -> Result<T, String>,
{
    thread_local! {
        static AFFINE_METAL_BACKEND: RefCell<Option<MlxAffineMetalBackend>> = const { RefCell::new(None) };
    }
    AFFINE_METAL_BACKEND.with(|backend| {
        let mut backend = backend.borrow_mut();
        if backend.is_none() {
            match MlxAffineMetalBackend::load() {
                Ok(loaded) => *backend = Some(loaded),
                Err(_) => return None,
            }
        }
        Some(f(
            backend
                .as_mut()
                .expect("affine metal backend was just initialized"),
        ))
    })
}

fn try_affine_quantized_matmul_tensor_metal(
    weights: &MlxIndexedSafetensors,
    input_words: &[u16],
    weight_name: &str,
    scales_name: &str,
    biases_name: &str,
) -> Option<Result<Vec<f32>, String>> {
    if weights.snapshot.config.quantization.mode != "affine" {
        return None;
    }
    match weights.snapshot.config.quantization.bits {
        4 | 8 => {}
        _ => return None,
    }
    if weights.snapshot.config.quantization.group_size != 64 {
        return None;
    }
    with_affine_metal_backend(|backend| {
        backend.matmul_rank2(weights, input_words, weight_name, scales_name, biases_name)
    })
}

pub(crate) fn try_affine_quantized_matmul_rows_metal(
    weights: &MlxIndexedSafetensors,
    input_words: &[u16],
    input_rows: usize,
    weight_name: &str,
    scales_name: &str,
    biases_name: &str,
) -> Option<Result<Vec<f32>, String>> {
    if weights.snapshot.config.quantization.mode != "affine" {
        return None;
    }
    match weights.snapshot.config.quantization.bits {
        4 | 8 => {}
        _ => return None,
    }
    if weights.snapshot.config.quantization.group_size != 64 {
        return None;
    }
    with_affine_metal_backend(|backend| {
        backend.matmul_rank2_rows(
            weights,
            input_words,
            input_rows,
            weight_name,
            scales_name,
            biases_name,
        )
    })
}

fn try_affine_quantized_matmul_rank3_plane_metal(
    weights: &MlxIndexedSafetensors,
    input_words: &[u16],
    weight_name: &str,
    scales_name: &str,
    biases_name: &str,
    plane: u64,
) -> Option<Result<Vec<f32>, String>> {
    if weights.snapshot.config.quantization.mode != "affine" {
        return None;
    }
    match weights.snapshot.config.quantization.bits {
        4 | 8 => {}
        _ => return None,
    }
    if weights.snapshot.config.quantization.group_size != 64 {
        return None;
    }
    with_affine_metal_backend(|backend| {
        backend.matmul_rank3_plane(
            weights,
            input_words,
            weight_name,
            scales_name,
            biases_name,
            plane,
        )
    })
}

include!("text_runtime/api.rs");
include!("text_runtime/reference.rs");

#[cfg(test)]
#[path = "../tests/text_runtime.rs"]
mod tests;
