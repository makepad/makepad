//! The real CUDA executor: device arena mirroring the ggml `Context`,
//! a prepare pass (validate ops/layouts, plan activation offsets with
//! interval reuse), and per-node kernel dispatch on one stream.
//!
//! Quantized weights (Q4_K/Q5_K/Q6_K) execute directly from the GGUF block
//! stream: mat-vec kernels dequantize in registers; the prefill GEMM path
//! dequantizes bounded transient slabs to bf16 for cuBLAS and releases them
//! stream-ordered — there is no persistent float expansion of any weight.
//! A fused packed-quant MMQ kernel is compiled and kept available; it is
//! not the production prefill path until it matches slab+cuBLAS speed.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::ffi::c_void;
use std::rc::Rc;
use std::time::Instant;

use makepad_ai_cuda::{
    begin_stream_capture, end_stream_capture, cublasCreate_v2, cublasDestroy_v2, cublasGemmEx,
    cublasHandle_t, cublasSetStream_v2, cublasSgemm_v2, cudaFree, cudaGetDeviceCount,
    cudaFreeHost, cudaGetErrorString, cudaHostAlloc, cudaMalloc, cudaMemGetInfo, cudaMemcpyAsync,
    cudaMemsetAsync, cudaSetDevice,
    cudaStreamCreateWithFlags, cudaStreamDestroy, cudaStreamSynchronize, cudaStream_t, CudaGraphExec,
    CUBLAS_COMPUTE_32F, CUBLAS_GEMM_DEFAULT_TENSOR_OP, CUBLAS_OP_N, CUBLAS_OP_T,
    CUBLAS_STATUS_SUCCESS, CUDA_MEMCPY_DEVICE_TO_HOST, CUDA_MEMCPY_HOST_TO_DEVICE, CUDA_R_16BF,
    CUDA_R_32F, CUDA_STREAM_CAPTURE_MODE_RELAXED, CUDA_STREAM_NON_BLOCKING, CUDA_SUCCESS,
};
use makepad_ai_cuda::llm_ops::{
    binary, cast_f32_bf16, copy_strided, dequant_rows_bf16, device_info, fattn_mma_f16,
    fattn_mma_fixup_bytes, fattn_vec_f16, fattn_vec_tmp_bytes, flash_decode, gated_delta_net,
    get_rows_f32, get_rows_quant, glu, mmq_q4k, mmq_q5k, mmq_q6k, mmq_quant, mmq_quant_q81,
    mmv_f32, mmv_quant, mmv_quant_q81, mmv_quant_q81_swiglu, mul_mat_batched, norm,
    quantize_mmq_d4, quantize_mmq_ds4, quantize_q81, quantize_q81_batched, rms_norm_mul,
    rope_multi, set_rows, softmax_mask, ssm_conv, unary, unary_mul, QUANT_Q4K as MKLLM_QUANT_Q4K,
    QUANT_Q5K as MKLLM_QUANT_Q5K, QUANT_Q6K as MKLLM_QUANT_Q6K, QUANT_Q80 as MKLLM_QUANT_Q80,
};
use crate::{
    ggml_row_size_for_type, Context, Op, Tensor, TensorId, TensorType, GGML_ROPE_TYPE_IMROPE,
    GGML_ROPE_TYPE_MROPE,
};

use super::CudaDeviceFeatures;
use crate::error::{LlamaError, Result};
use crate::runtime::{
    build_hybrid_decode_graph_with_attention_key_count, build_hybrid_decode_writes,
    collect_hybrid_decode_run, debug_trace_outputs, import_hybrid_graph_context,
    validate_hybrid_decode_layout,
    HybridDecodeBatchLayout, HybridDecodeGraph, HybridDecodeOutputConfig, HybridDecodeRun,
    HybridDecodeSpec, HybridSharedCacheTensorIds, ImportedHybridGraphContext, LogitsProbeInput,
};
use crate::weights::LoadedGgufWeights;

// cudaMemcpyKind::cudaMemcpyDeviceToDevice. Official ggml-cuda cpy.cu:418.
const CUDA_MEMCPY_DEVICE_TO_DEVICE: i32 = 3;

const COMPILED_ARCH: &str = match option_env!("MAKEPAD_LLAMA_CUDA_ARCH") {
    Some(arch) => arch,
    None => "unknown",
};
/// Mat-vec kernels handle this many activation columns; larger M takes the
/// slab-dequant cuBLAS GEMM path (prefill).
const MMV_MAX_COLUMNS: usize = 8;
/// Transient bf16 weight-slab bound for the GEMM path.
const GEMM_SLAB_BYTES: usize = 256 << 20;
/// Stable scratch reserved before CUDA-graph capture so `cudaMalloc`
/// cannot happen on the capturing stream. Official ggml-alloc + cuBLAS
/// workspace is allocated before ggml-cuda.cu:4149 BeginCapture. Prefill
/// GEMM needs the 256 MiB slab plus DS4/fattn fixup in acts.
const GRAPH_ACT_SCRATCH: usize = 64 << 20;
const GRAPH_WEIGHT_SCRATCH: usize = GEMM_SLAB_BYTES;
/// VRAM the arena refuses to consume (driver/display/allocator slack).
const VRAM_RESERVE_BYTES: u64 = 1 << 30;

/// llama-bench.cpp:2026 test_gen host vs GPU split. Env-gated so the
/// official pp/tg line stays free of extra event records.
fn host_split_enabled() -> bool {
    std::env::var_os("MAKEPAD_LLAMA_HOST_SPLIT").is_some()
}

/// llama-bench.cpp:2036-2042 test_gen never calls llama_get_logits.
/// Decode + synchronize + rand; logits stay on device. Our collect of
/// vocab=248320 was 0.57 ms/tok of host-after-sync that they elide.
fn skip_logits_readback() -> bool {
    std::env::var_os("MAKEPAD_LLAMA_SKIP_LOGITS").is_some()
}

#[derive(Default, Clone, Debug)]
pub struct HostSplit {
    pub tokens: u64,
    pub write_bytes: u64,
    pub host_pre_ms: f64,
    pub host_h2d_ms: f64,
    pub host_launch_cpu_ms: f64,
    pub gpu_graph_ms: f64,
    pub gpu_d2h_ms: f64,
    pub host_collect_ms: f64,
    pub wall_ms: f64,
}

impl HostSplit {
    pub fn per_token(&self) -> HostSplit {
        let n = self.tokens.max(1) as f64;
        HostSplit {
            tokens: self.tokens,
            write_bytes: if self.tokens == 0 {
                0
            } else {
                self.write_bytes / self.tokens
            },
            host_pre_ms: self.host_pre_ms / n,
            host_h2d_ms: self.host_h2d_ms / n,
            host_launch_cpu_ms: self.host_launch_cpu_ms / n,
            gpu_graph_ms: self.gpu_graph_ms / n,
            gpu_d2h_ms: self.gpu_d2h_ms / n,
            host_collect_ms: self.host_collect_ms / n,
            wall_ms: self.wall_ms / n,
        }
    }

    pub fn host_outside_gpu_ms(&self) -> f64 {
        (self.wall_ms - self.gpu_graph_ms - self.gpu_d2h_ms).max(0.0)
    }

    pub fn report_line(&self) -> String {
        let pt = self.per_token();
        format!(
            "host.split: tokens={} write_b/tok={} host_pre={:.3} h2d_cpu={:.3} launch_cpu={:.3} \
             gpu_graph={:.3} gpu_d2h={:.3} host_collect={:.3} host_outside={:.3} wall={:.3} ms/tok",
            pt.tokens,
            pt.write_bytes,
            pt.host_pre_ms,
            pt.host_h2d_ms,
            pt.host_launch_cpu_ms,
            pt.gpu_graph_ms,
            pt.gpu_d2h_ms,
            pt.host_collect_ms,
            pt.host_outside_gpu_ms(),
            pt.wall_ms,
        )
    }
}

thread_local! {
    static HOST_SPLIT: RefCell<HostSplit> = RefCell::new(HostSplit::default());
}

pub fn host_split_reset() {
    HOST_SPLIT.with(|slot| *slot.borrow_mut() = HostSplit::default());
}

pub fn host_split_snapshot() -> HostSplit {
    HOST_SPLIT.with(|slot| slot.borrow().clone())
}

const BIN_ADD: i32 = 0;
const BIN_SUB: i32 = 1;
const BIN_MUL: i32 = 2;
const BIN_DIV: i32 = 3;

type CudaErr = i32;
type CudaEvent = *mut c_void;

extern "C" {
    fn cudaEventCreate(event: *mut CudaEvent) -> CudaErr;
    fn cudaEventDestroy(event: CudaEvent) -> CudaErr;
    fn cudaEventRecord(event: CudaEvent, stream: cudaStream_t) -> CudaErr;
    fn cudaEventElapsedTime(ms: *mut f32, start: CudaEvent, end: CudaEvent) -> CudaErr;
}

fn cuda_err_str(err: CudaErr) -> String {
    unsafe {
        let ptr = cudaGetErrorString(err);
        if ptr.is_null() {
            format!("cuda error {err}")
        } else {
            std::ffi::CStr::from_ptr(ptr).to_string_lossy().into_owned()
        }
    }
}

fn check(err: CudaErr, what: &str) -> Result<()> {
    if err != CUDA_SUCCESS {
        return Err(LlamaError::format(format!(
            "cuda {what} failed: {}",
            cuda_err_str(err)
        )));
    }
    Ok(())
}

struct EventTimeline {
    stream: cudaStream_t,
    events: Vec<CudaEvent>,
}

impl EventTimeline {
    fn new(stream: cudaStream_t) -> Result<Self> {
        let mut timeline = Self {
            stream,
            events: Vec::new(),
        };
        timeline.mark()?;
        Ok(timeline)
    }

    fn mark(&mut self) -> Result<()> {
        let mut event = std::ptr::null_mut();
        check(unsafe { cudaEventCreate(&mut event) }, "profile event create")?;
        if let Err(err) = check(
            unsafe { cudaEventRecord(event, self.stream) },
            "profile event record",
        ) {
            unsafe {
                cudaEventDestroy(event);
            }
            return Err(err);
        }
        self.events.push(event);
        Ok(())
    }

    fn elapsed_ms(&self, index: usize) -> Result<f32> {
        let mut ms = 0.0f32;
        check(
            unsafe {
                cudaEventElapsedTime(&mut ms, self.events[index], self.events[index + 1])
            },
            "profile event elapsed",
        )?;
        Ok(ms)
    }
}

impl Drop for EventTimeline {
    fn drop(&mut self) {
        for event in self.events.drain(..) {
            unsafe {
                cudaEventDestroy(event);
            }
        }
    }
}

struct QuantStage {
    start: usize,
    end: usize,
    kind: i32,
    name: &'static str,
}

struct QuantTimeline {
    stream: cudaStream_t,
    events: Vec<CudaEvent>,
    stages: Vec<QuantStage>,
}

impl QuantTimeline {
    fn new(stream: cudaStream_t) -> Self {
        Self {
            stream,
            events: Vec::new(),
            stages: Vec::new(),
        }
    }

    fn mark(&mut self) -> Result<usize> {
        let mut event = std::ptr::null_mut();
        check(unsafe { cudaEventCreate(&mut event) }, "quant profile event create")?;
        if let Err(err) = check(
            unsafe { cudaEventRecord(event, self.stream) },
            "quant profile event record",
        ) {
            unsafe {
                cudaEventDestroy(event);
            }
            return Err(err);
        }
        let index = self.events.len();
        self.events.push(event);
        Ok(index)
    }

    fn finish(&mut self, start: usize, kind: i32, name: &'static str) -> Result<usize> {
        let end = self.mark()?;
        self.stages.push(QuantStage {
            start,
            end,
            kind,
            name,
        });
        Ok(end)
    }

    fn elapsed_ms(&self, stage: &QuantStage) -> Result<f32> {
        let mut ms = 0.0f32;
        check(
            unsafe {
                cudaEventElapsedTime(&mut ms, self.events[stage.start], self.events[stage.end])
            },
            "quant profile event elapsed",
        )?;
        Ok(ms)
    }
}

impl Drop for QuantTimeline {
    fn drop(&mut self) {
        for event in self.events.drain(..) {
            unsafe {
                cudaEventDestroy(event);
            }
        }
    }
}

struct DeviceState {
    stream: cudaStream_t,
    blas: cublasHandle_t,
    features: CudaDeviceFeatures,
    scratch_weights: RefCell<Scratch>,
    scratch_acts: RefCell<Scratch>,
    // llama-context.cpp:1943-1950: logits land in the device host buffer
    // (pinned) so D2H is DMA, not a WDDM bounce through a fresh Vec.
    scratch_host: RefCell<HostScratch>,
    last_q81_src: Cell<*const f32>,
    last_q81_k: Cell<usize>,
    split_ev0: Cell<CudaEvent>,
    split_ev1: Cell<CudaEvent>,
    split_ev2: Cell<CudaEvent>,
}

struct Scratch {
    ptr: *mut c_void,
    size: usize,
}

impl Scratch {
    fn ensure(&mut self, size: usize, what: &str) -> Result<*mut c_void> {
        if self.size < size {
            unsafe {
                if !self.ptr.is_null() {
                    cudaFree(self.ptr);
                    self.ptr = std::ptr::null_mut();
                    self.size = 0;
                }
                let mut ptr = std::ptr::null_mut();
                check(cudaMalloc(&mut ptr, size), what)?;
                self.ptr = ptr;
                self.size = size;
            }
        }
        Ok(self.ptr)
    }
}

struct HostScratch {
    ptr: *mut c_void,
    size: usize,
}

impl HostScratch {
    fn ensure(&mut self, size: usize, what: &str) -> Result<*mut u8> {
        if self.size < size {
            unsafe {
                if !self.ptr.is_null() {
                    cudaFreeHost(self.ptr);
                    self.ptr = std::ptr::null_mut();
                    self.size = 0;
                }
                let mut ptr = std::ptr::null_mut();
                check(cudaHostAlloc(&mut ptr, size, 0), what)?;
                self.ptr = ptr;
                self.size = size;
            }
        }
        Ok(self.ptr as *mut u8)
    }
}

impl Drop for DeviceState {
    fn drop(&mut self) {
        unsafe {
            let weights = self.scratch_weights.get_mut();
            if !weights.ptr.is_null() {
                cudaFree(weights.ptr);
            }
            let acts = self.scratch_acts.get_mut();
            if !acts.ptr.is_null() {
                cudaFree(acts.ptr);
            }
            let host = self.scratch_host.get_mut();
            if !host.ptr.is_null() {
                cudaFreeHost(host.ptr);
            }
            for ev in [self.split_ev0.get(), self.split_ev1.get(), self.split_ev2.get()] {
                if !ev.is_null() {
                    cudaEventDestroy(ev);
                }
            }
            cublasDestroy_v2(self.blas);
            cudaStreamDestroy(self.stream);
        }
    }
}

pub(super) struct Runtime {
    state: Rc<DeviceState>,
}

pub(super) struct Arena {
    state: Rc<DeviceState>,
    ro_dev: *mut c_void,
    ro_split: usize,
    main_dev: *mut c_void,
    main_size: usize,
}

impl Arena {
    fn ptr_at(&self, offset: usize, len: usize) -> Result<*mut c_void> {
        if offset < self.ro_split {
            if offset + len > self.ro_split {
                return Err(LlamaError::format(
                    "device range straddles the read-only weight region",
                ));
            }
            Ok(unsafe { (self.ro_dev as *mut u8).add(offset) } as *mut c_void)
        } else {
            let rel = offset - self.ro_split;
            if rel + len > self.main_size {
                return Err(LlamaError::format(format!(
                    "device range [{}..+{}) exceeds arena size {}",
                    offset, len, self.main_size
                )));
            }
            Ok(unsafe { (self.main_dev as *mut u8).add(rel) } as *mut c_void)
        }
    }
}

impl Drop for Arena {
    fn drop(&mut self) {
        unsafe {
            cudaStreamSynchronize(self.state.stream);
            if !self.ro_dev.is_null() {
                cudaFree(self.ro_dev);
            }
            if !self.main_dev.is_null() {
                cudaFree(self.main_dev);
            }
        }
    }
}

impl Runtime {
    pub(super) fn new() -> Result<Self> {
        unsafe {
            let mut count = 0;
            check(cudaGetDeviceCount(&mut count), "device count")?;
            if count <= 0 {
                return Err(LlamaError::unsupported("no CUDA device present"));
            }
            let device: i32 = std::env::var("MAKEPAD_LLAMA_CUDA_DEVICE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            check(cudaSetDevice(device), "set device")?;

            let mut name = [0u8; 256];
            let (mut cc_major, mut cc_minor, mut sm_count) = (0i32, 0i32, 0i32);
            let mut total_mem = 0usize;
            check(
                device_info(
                    device,
                    name.as_mut_ptr(),
                    name.len() as i32,
                    &mut cc_major,
                    &mut cc_minor,
                    &mut total_mem,
                    &mut sm_count,
                ),
                "device info",
            )?;
            let name_len = name.iter().position(|&b| b == 0).unwrap_or(0);
            let device_name = String::from_utf8_lossy(&name[..name_len]).into_owned();

            // SASS is compiled on the target box for that box
            // (MAKEPAD_GGML_CUDA_ARCH=86/89/120a). Not a fat binary.
            // Same-major, >= minor can run a given build.
            let arch = COMPILED_ARCH.trim_end_matches(|c: char| c.is_ascii_alphabetic());
            let (arch_major, arch_minor) = if arch.len() >= 2 {
                let (major, minor) = arch.split_at(arch.len() - 1);
                (
                    major.parse::<i32>().unwrap_or(0),
                    minor.parse::<i32>().unwrap_or(0),
                )
            } else {
                (0, 0)
            };
            if cc_major != arch_major || cc_minor < arch_minor {
                return Err(LlamaError::unsupported(format!(
                    "CUDA device '{device_name}' is sm_{cc_major}{cc_minor}, but this build's \
                     kernels target sm_{COMPILED_ARCH}; rebuild with MAKEPAD_GGML_CUDA_ARCH={cc_major}{cc_minor}"
                )));
            }

            let (mut free, mut total) = (0usize, 0usize);
            check(cudaMemGetInfo(&mut free, &mut total), "mem info")?;

            let mut stream: cudaStream_t = std::ptr::null_mut();
            check(
                cudaStreamCreateWithFlags(&mut stream, CUDA_STREAM_NON_BLOCKING),
                "stream create",
            )?;
            let mut blas: cublasHandle_t = std::ptr::null_mut();
            if cublasCreate_v2(&mut blas) != CUBLAS_STATUS_SUCCESS {
                cudaStreamDestroy(stream);
                return Err(LlamaError::unsupported("cublas handle creation failed"));
            }
            if cublasSetStream_v2(blas, stream) != CUBLAS_STATUS_SUCCESS {
                cublasDestroy_v2(blas);
                cudaStreamDestroy(stream);
                return Err(LlamaError::unsupported("cublas stream binding failed"));
            }

            let mut split_ev0 = std::ptr::null_mut();
            let mut split_ev1 = std::ptr::null_mut();
            let mut split_ev2 = std::ptr::null_mut();
            if host_split_enabled() {
                if cudaEventCreate(&mut split_ev0) != CUDA_SUCCESS
                    || cudaEventCreate(&mut split_ev1) != CUDA_SUCCESS
                    || cudaEventCreate(&mut split_ev2) != CUDA_SUCCESS
                {
                    if !split_ev0.is_null() {
                        cudaEventDestroy(split_ev0);
                    }
                    if !split_ev1.is_null() {
                        cudaEventDestroy(split_ev1);
                    }
                    if !split_ev2.is_null() {
                        cudaEventDestroy(split_ev2);
                    }
                    split_ev0 = std::ptr::null_mut();
                    split_ev1 = std::ptr::null_mut();
                    split_ev2 = std::ptr::null_mut();
                }
            }

            Ok(Self {
                state: Rc::new(DeviceState {
                    stream,
                    blas,
                    features: CudaDeviceFeatures {
                        device_name,
                        compute_capability: (cc_major as u32, cc_minor as u32),
                        total_vram_bytes: total as u64,
                        free_vram_bytes: free as u64,
                        sm_count: sm_count as u32,
                        compiled_arch: COMPILED_ARCH,
                    },
                    scratch_weights: RefCell::new(Scratch {
                        ptr: std::ptr::null_mut(),
                        size: 0,
                    }),
                    scratch_acts: RefCell::new(Scratch {
                        ptr: std::ptr::null_mut(),
                        size: 0,
                    }),
                    scratch_host: RefCell::new(HostScratch {
                        ptr: std::ptr::null_mut(),
                        size: 0,
                    }),
                    last_q81_src: Cell::new(std::ptr::null()),
                    last_q81_k: Cell::new(0),
                    split_ev0: Cell::new(split_ev0),
                    split_ev1: Cell::new(split_ev1),
                    split_ev2: Cell::new(split_ev2),
                }),
            })
        }
    }

    pub(super) fn features(&self) -> CudaDeviceFeatures {
        let mut features = self.state.features.clone();
        let (mut free, mut total) = (0usize, 0usize);
        unsafe {
            if cudaMemGetInfo(&mut free, &mut total) == CUDA_SUCCESS {
                features.free_vram_bytes = free as u64;
                features.total_vram_bytes = total as u64;
            }
        }
        features
    }

    pub(super) fn reserve_main_buffer_size(
        &self,
        weights: &LoadedGgufWeights,
        spec: &HybridDecodeSpec,
        shared_cache: Option<&HybridSharedCacheTensorIds>,
        n_tokens: usize,
        n_outputs: usize,
    ) -> Result<usize> {
        let ImportedHybridGraphContext {
            mut ctx,
            tensor_ids,
            shared_cache,
        } = import_hybrid_graph_context(weights, shared_cache, false)?;
        let key_count = default_reserve_key_count(spec);
        let decode = build_hybrid_decode_graph_with_attention_key_count(
            &mut ctx,
            &tensor_ids,
            spec,
            shared_cache.as_ref(),
            n_tokens,
            n_outputs,
            key_count,
        )?;
        let plan = plan_graph(&ctx, &decode)?;
        Ok(plan.required_size)
    }

    pub(super) fn create_context_arena(&self, ctx: &Context) -> Result<Arena> {
        self.create_context_arena_with_progress(ctx, &mut |_, _| {})
    }

    pub(super) fn create_context_arena_with_progress(
        &self,
        ctx: &Context,
        progress: &mut dyn FnMut(usize, usize),
    ) -> Result<Arena> {
        let ro_split = ctx.ro_split();
        let main_size = ctx.mem_size() - ro_split;
        let required = (ro_split + main_size) as u64;
        let features = self.features();
        if required + VRAM_RESERVE_BYTES > features.free_vram_bytes {
            return Err(LlamaError::unsupported(format!(
                "model needs {:.2} GiB device memory (+{:.1} GiB reserve) but only {:.2} GiB \
                 of {:.2} GiB VRAM is free on {}",
                required as f64 / (1u64 << 30) as f64,
                VRAM_RESERVE_BYTES as f64 / (1u64 << 30) as f64,
                features.free_vram_bytes as f64 / (1u64 << 30) as f64,
                features.total_vram_bytes as f64 / (1u64 << 30) as f64,
                features.device_name,
            )));
        }

        unsafe {
            let stream = self.state.stream;
            let mut ro_dev: *mut c_void = std::ptr::null_mut();
            if ro_split > 0 {
                check(cudaMalloc(&mut ro_dev, ro_split), "weights region alloc")?;
            }
            let mut main_dev: *mut c_void = std::ptr::null_mut();
            let alloc = cudaMalloc(&mut main_dev, main_size.max(1));
            if alloc != CUDA_SUCCESS {
                if !ro_dev.is_null() {
                    cudaFree(ro_dev);
                }
                return Err(LlamaError::format(format!(
                    "cuda arena alloc failed: {}",
                    cuda_err_str(alloc)
                )));
            }
            let arena = Arena {
                state: self.state.clone(),
                ro_dev,
                ro_split,
                main_dev,
                main_size,
            };

            // Upload the weight region once (chunked; source is the mmap so
            // clean pages stream straight from the file cache), zero the
            // dirty region, then upload the resident dirty prefix (caches,
            // and the whole weight arena for non-mmap contexts).
            const CHUNK: usize = 256 << 20;
            let mut offset = 0usize;
            progress(0, ro_split);
            while offset < ro_split {
                let len = CHUNK.min(ro_split - offset);
                let host = ctx.data_at(offset, len).map_err(LlamaError::format)?;
                check(
                    cudaMemcpyAsync(
                        arena.ptr_at(offset, len)?,
                        host.as_ptr() as *const c_void,
                        len,
                        CUDA_MEMCPY_HOST_TO_DEVICE,
                        stream,
                    ),
                    "weights upload",
                )?;
                // Bounded in-flight window: one chunk at a time.
                check(cudaStreamSynchronize(stream), "weights upload sync")?;
                offset += len;
                progress(offset, ro_split);
            }
            check(
                cudaMemsetAsync(arena.main_dev, 0, arena.main_size.max(1), stream),
                "dirty region clear",
            )?;
            let dirty_used = ctx.used_mem().saturating_sub(ro_split);
            let total = ro_split.saturating_add(dirty_used);
            let mut offset = 0usize;
            while offset < dirty_used {
                let len = CHUNK.min(dirty_used - offset);
                let host = ctx
                    .data_at(ro_split + offset, len)
                    .map_err(LlamaError::format)?;
                check(
                    cudaMemcpyAsync(
                        arena.ptr_at(ro_split + offset, len)?,
                        host.as_ptr() as *const c_void,
                        len,
                        CUDA_MEMCPY_HOST_TO_DEVICE,
                        stream,
                    ),
                    "state upload",
                )?;
                check(cudaStreamSynchronize(stream), "state upload sync")?;
                offset += len;
                progress(ro_split + offset, total.max(1));
            }
            check(cudaStreamSynchronize(stream), "arena upload sync")?;
            Ok(arena)
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn compile_hybrid_decode(
        &self,
        weights: &mut LoadedGgufWeights,
        spec: &HybridDecodeSpec,
        shared_cache: &HybridSharedCacheTensorIds,
        arena: &Arena,
        n_tokens: usize,
        n_outputs: usize,
        attention_key_count: usize,
    ) -> Result<Compiled> {
        let ImportedHybridGraphContext {
            mut ctx,
            tensor_ids,
            shared_cache,
        } = import_hybrid_graph_context(weights, Some(shared_cache), false)?;
        let decode = build_hybrid_decode_graph_with_attention_key_count(
            &mut ctx,
            &tensor_ids,
            spec,
            shared_cache.as_ref(),
            n_tokens,
            n_outputs,
            attention_key_count,
        )?;
        let plan = plan_graph(&ctx, &decode)?;
        if plan.required_size > ctx.mem_size() {
            return Err(LlamaError::format(format!(
                "context out of memory allocating {} bytes for graph activations \
                 (context arena is {} bytes)",
                plan.required_size - ctx.used_mem(),
                ctx.mem_size(),
            )));
        }
        // Official ggml_cuda_graph_check_compability (ggml-cuda.cu:2930)
        // captures cuBLAS GEMM. Scratch is reserved in execute_with_graph
        // before BeginCapture so cudaMalloc cannot ACCESS_VIOLATION.
        Ok(Compiled {
            state: self.state.clone(),
            arena: ArenaRef {
                ro_dev: arena.ro_dev,
                ro_split: arena.ro_split,
                main_dev: arena.main_dev,
                main_size: arena.main_size,
                _keep: arena.state.clone(),
            },
            spec: spec.clone(),
            ctx,
            decode,
            plan,
            graph_exec: RefCell::new(None),
            graph_disabled: Cell::new(
                std::env::var_os("MKLLM_DISABLE_CUDA_GRAPH")
                    .map(|v| v == "1")
                    .unwrap_or(false),
            ),
            graph_warmup: Cell::new(false),
        })
    }

    /// Test/validation entry: execute an arbitrary graph over `ctx` through
    /// the exact planner/dispatch path the session uses, returning the
    /// requested tensors' bytes. Creates (and frees) a private device arena.
    pub(super) fn execute_raw_graph(
        &self,
        ctx: &Context,
        graph: &crate::Graph,
        pinned: &[TensorId],
        writes: &[(TensorId, Vec<u8>)],
        wanted: &[TensorId],
    ) -> Result<BTreeMap<TensorId, Vec<u8>>> {
        let plan = plan_raw_graph(ctx, graph, pinned)?;
        if plan.required_size > ctx.mem_size() {
            return Err(LlamaError::format(format!(
                "context out of memory allocating {} bytes for graph activations",
                plan.required_size - ctx.used_mem(),
            )));
        }
        let arena = self.create_context_arena(ctx)?;
        let arena_ref = ArenaRef {
            ro_dev: arena.ro_dev,
            ro_split: arena.ro_split,
            main_dev: arena.main_dev,
            main_size: arena.main_size,
            _keep: arena.state.clone(),
        };
        let view = ExecView {
            state: &self.state,
            arena: &arena_ref,
            ctx,
            plan: &plan,
        };
        view.run(writes, wanted)
    }

    pub(super) fn read_arena_bytes(
        &self,
        arena: &Arena,
        offset: usize,
        len: usize,
    ) -> Result<Vec<u8>> {
        let mut out = vec![0u8; len];
        unsafe {
            check(
                cudaMemcpyAsync(
                    out.as_mut_ptr() as *mut c_void,
                    arena.ptr_at(offset, len)?,
                    len,
                    CUDA_MEMCPY_DEVICE_TO_HOST,
                    self.state.stream,
                ),
                "arena readback",
            )?;
            check(cudaStreamSynchronize(self.state.stream), "arena readback sync")?;
        }
        Ok(out)
    }

    pub(super) fn clear_arena_ranges(
        &self,
        arena: &Arena,
        ranges: &[(usize, usize)],
    ) -> Result<()> {
        unsafe {
            for &(offset, len) in ranges {
                if len == 0 {
                    continue;
                }
                if offset < arena.ro_split {
                    return Err(LlamaError::format(format!(
                        "refusing to clear read-only CUDA arena range [{}..+{}) below split {}",
                        offset, len, arena.ro_split,
                    )));
                }
                check(
                    cudaMemsetAsync(
                        arena.ptr_at(offset, len)?,
                        0,
                        len,
                        self.state.stream,
                    ),
                    "state reset",
                )?;
            }
            check(cudaStreamSynchronize(self.state.stream), "state reset sync")?;
        }
        Ok(())
    }
}

fn default_reserve_key_count(spec: &HybridDecodeSpec) -> usize {
    use crate::runtime::HybridLayerSpec;
    for layer in &spec.layers {
        if let HybridLayerSpec::Attention { decode, .. } = layer {
            return decode.cache.max_context as usize;
        }
    }
    1
}

// ---------------------------------------------------------------------------
// Planning
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
enum KernelSel {
    Skip,
    GetRowsF32,
    GetRowsQuant(i32),
    SetRows { f16: bool },
    SoftMaxMasked,
    FlashDecode,
    FlashMma,
    FlashVec,
    Norm { l2: bool },
    // llama.cpp ggml-cuda.cu:3994-4004 RMS_NORM + MUL (+ ADD)
    NormMul { add: bool },
    RopeMulti,
    Unary(i32),
    // llama.cpp ggml-cuda.cu:4012-4017 UNARY(SILU/SIGMOID/SOFTPLUS) + MUL
    UnaryMul(i32),
    Glu(i32),
    Binary(i32),
    MmvQuant(i32),
    MmvqFusedSwiglu { kind: i32 },
    GemmQuant(i32),
    MmvF32,
    GemmF32,
    MulMatBatched { a_f16: bool },
    CopyStrided,
    // llama.cpp qwen35.cpp:276 / ggml-cuda.cu:2520: CPY view -> cache, not CONT+SET_ROWS
    CopyViewTo,
    Concat,
    SsmConv,
    // llama.cpp ggml-cuda.cu:4006 SSM_CONV + SILU
    SsmConvSilu,
    GatedDeltaNet,
}

struct PlannedNode {
    node_id: TensorId,
    kernel: KernelSel,
}

struct GraphPlan {
    nodes: Vec<PlannedNode>,
    bindings: BTreeMap<TensorId, usize>,
    required_size: usize,
    fused_mmvq: usize,
    fused_rms: usize,
    fused_unary: usize,
    fused_ssm: usize,
}

fn resolve_root(tensors: &[Tensor], id: TensorId) -> Result<(TensorId, usize)> {
    let mut current = id;
    let mut extra = 0usize;
    loop {
        let t = tensors
            .get(current)
            .ok_or_else(|| LlamaError::format(format!("invalid tensor id {current}")))?;
        if let Some(src) = t.view_src {
            extra += t.view_offs;
            current = src;
        } else {
            return Ok((current, extra));
        }
    }
}

fn quant_kind(ty: TensorType) -> Option<i32> {
    match ty {
        TensorType::Q4K => Some(MKLLM_QUANT_Q4K),
        TensorType::Q5K => Some(MKLLM_QUANT_Q5K),
        TensorType::Q6K => Some(MKLLM_QUANT_Q6K),
        TensorType::Q8_0 => Some(MKLLM_QUANT_Q80),
        _ => None,
    }
}

fn unsupported_node(t: &Tensor, why: &str) -> LlamaError {
    LlamaError::unsupported(format!(
        "CUDA executor cannot run node '{}' ({:?}): {}",
        t.name().unwrap_or("<unnamed>"),
        t.op,
        why
    ))
}

fn select_kernel(tensors: &[Tensor], t: &Tensor) -> Result<KernelSel> {
    let src = |i: usize| -> Result<&Tensor> {
        t.src[i]
            .and_then(|id| tensors.get(id))
            .ok_or_else(|| unsupported_node(t, &format!("missing src{i}")))
    };
    Ok(match t.op {
        Op::None | Op::View | Op::Reshape | Op::Permute | Op::Transpose => KernelSel::Skip,
        Op::Cont | Op::Cpy | Op::Dup => {
            let s = src(0)?;
            if s.desc.ty != t.desc.ty {
                return Err(unsupported_node(t, "type-converting copy"));
            }
            match t.desc.ty {
                TensorType::F32 | TensorType::F16 => KernelSel::CopyStrided,
                other => return Err(unsupported_node(t, &format!("copy of {}", other.name()))),
            }
        }
        Op::Concat => {
            if t.desc.ty != TensorType::F32 {
                return Err(unsupported_node(t, "non-f32 concat"));
            }
            KernelSel::Concat
        }
        Op::GetRows => {
            let s = src(0)?;
            let rows = src(1)?;
            if rows.desc.ty != TensorType::I32 {
                return Err(unsupported_node(t, "non-i32 row indices"));
            }
            match s.desc.ty {
                TensorType::F32 => KernelSel::GetRowsF32,
                ty => match quant_kind(ty) {
                    Some(kind) => KernelSel::GetRowsQuant(kind),
                    None => {
                        return Err(unsupported_node(
                            t,
                            &format!("get_rows from {}", ty.name()),
                        ))
                    }
                },
            }
        }
        Op::SetRows => {
            let s = src(0)?;
            let rows = src(1)?;
            if rows.desc.ty != TensorType::I32 {
                return Err(unsupported_node(t, "non-i32 row indices"));
            }
            if s.desc.ty != TensorType::F32 {
                return Err(unsupported_node(t, "non-f32 source rows"));
            }
            if t.ne[2] != 1 || t.ne[3] != 1 {
                return Err(unsupported_node(t, "batched set_rows"));
            }
            match t.desc.ty {
                TensorType::F16 => KernelSel::SetRows { f16: true },
                TensorType::F32 => KernelSel::SetRows { f16: false },
                other => {
                    return Err(unsupported_node(
                        t,
                        &format!("set_rows into {}", other.name()),
                    ))
                }
            }
        }
        Op::SoftMax => {
            if t.op_param_f32(1) != 0.0 {
                return Err(unsupported_node(t, "ALiBi max_bias"));
            }
            if let Some(mask_id) = t.src[1] {
                let mask = tensors
                    .get(mask_id)
                    .ok_or_else(|| unsupported_node(t, "missing mask"))?;
                if mask.desc.ty != TensorType::F32 {
                    return Err(unsupported_node(t, "non-f32 softmax mask"));
                }
            }
            KernelSel::SoftMaxMasked
        }
        Op::FlashAttnExt => {
            let q = src(0)?;
            let k = src(1)?;
            let v = src(2)?;
            if t.src[3].is_none() {
                return Err(unsupported_node(t, "flash attention without mask"));
            }
            if t.src[4].is_some() {
                return Err(unsupported_node(t, "flash attention sinks"));
            }
            if k.desc.ty != TensorType::F16 || v.desc.ty != TensorType::F16 {
                return Err(unsupported_node(t, "non-f16 flash K/V"));
            }
            if q.desc.ty != TensorType::F32 {
                return Err(unsupported_node(t, "non-f32 flash Q"));
            }
            if t.op_param_f32(1) != 0.0 || t.op_param_f32(2) != 0.0 {
                return Err(unsupported_node(t, "flash max_bias/softcap"));
            }
            if q.ne[3] != 1 {
                return Err(unsupported_node(t, "multi-stream flash attention"));
            }
            if q.ne[2] % k.ne[2] != 0 {
                return Err(unsupported_node(t, "non-integer GQA ratio"));
            }
            // llama.cpp fattn.cu get_best_fattn_kernel + switch_ncols*:
            // D=256 Turing+: MMA_F16; GQA>4 + n_tokens>4 -> ncols1=8,ncols2=8.
            // Ada+ n_q==1 F16 K/V, not (GQA>4 && KV>=8192) -> VEC cols=1
            // (fattn.cu:402-404, fattn-vec.cuh:543-547).
            // K.ne[1] must be the live padded n_kv (llama-kv-cache.cpp:1121),
            // not the 8192 allocation, or this falls through to FlashDecode.
            // MKLLM_DISABLE_FATTN_MMA=1 / MKLLM_DISABLE_FATTN_VEC=1 roll back.
            let fattn_disabled = std::env::var_os("MKLLM_DISABLE_FATTN_MMA")
                .map(|v| v == "1")
                .unwrap_or(false);
            let fattn_vec_disabled = std::env::var_os("MKLLM_DISABLE_FATTN_VEC")
                .map(|v| v == "1")
                .unwrap_or(false);
            let gqa = q.ne[2] / k.ne[2];
            let aligned = k.ne[1] % 256 == 0
                && q.nb[1] % 16 == 0
                && q.nb[2] % 16 == 0
                && k.nb[1] % 16 == 0
                && k.nb[2] % 16 == 0
                && v.nb[1] % 16 == 0
                && v.nb[2] % 16 == 0;
            if !fattn_disabled && q.ne[0] == 256 && v.ne[0] == 256 && aligned {
                if !fattn_vec_disabled
                    && q.ne[1] == 1
                    && q.ne[3] == 1
                    && !(gqa > 4 && k.ne[1] >= 8192)
                {
                    KernelSel::FlashVec
                } else if q.ne[1] >= 20 && gqa > 4 {
                    KernelSel::FlashMma
                } else {
                    KernelSel::FlashDecode
                }
            } else {
                KernelSel::FlashDecode
            }
        }
        Op::RmsNorm => KernelSel::Norm { l2: false },
        Op::L2Norm => KernelSel::Norm { l2: true },
        Op::Rope => {
            // Official ggml_cuda_should_fuse_rope_set_rows (ggml-cuda.cu:3157)
            // only accepts NORMAL/NEOX. Qwen3.5 is IMROPE, so they also emit
            // rope_multi then SET_ROWS. Do not invent an IMROPE+SET_ROWS fuse.
            let mode = t.op_param_i32(2);
            if mode != GGML_ROPE_TYPE_IMROPE && (mode & GGML_ROPE_TYPE_MROPE) == 0 {
                return Err(unsupported_node(t, &format!("rope mode {mode}")));
            }
            if t.src[2].is_some() {
                return Err(unsupported_node(t, "rope freq_factors"));
            }
            if t.desc.ty != TensorType::F32 {
                return Err(unsupported_node(t, "non-f32 rope"));
            }
            KernelSel::RopeMulti
        }
        Op::Unary => {
            let op = t.op_param_i32(0);
            if !(0..=16).contains(&op) {
                return Err(unsupported_node(t, &format!("unary op {op}")));
            }
            let s = src(0)?;
            if !s.is_contiguous() || !t.is_contiguous() {
                return Err(unsupported_node(t, "strided unary"));
            }
            KernelSel::Unary(op)
        }
        Op::Glu => {
            if t.src[1].is_none() {
                return Err(unsupported_node(t, "fused-split glu"));
            }
            let op = t.op_param_i32(0);
            let s0 = src(0)?;
            let s1 = src(1)?;
            if !s0.is_contiguous() || !s1.is_contiguous() || !t.is_contiguous() {
                return Err(unsupported_node(t, "strided glu"));
            }
            KernelSel::Glu(op)
        }
        Op::Add | Op::Sub | Op::Mul | Op::Div => {
            let s0 = src(0)?;
            let s1 = src(1)?;
            if t.desc.ty != TensorType::F32
                || s0.desc.ty != TensorType::F32
                || s1.desc.ty != TensorType::F32
            {
                return Err(unsupported_node(t, "non-f32 binary op"));
            }
            KernelSel::Binary(match t.op {
                Op::Add => BIN_ADD,
                Op::Sub => BIN_SUB,
                Op::Mul => BIN_MUL,
                _ => BIN_DIV,
            })
        }
        Op::MulMat => {
            let a = src(0)?;
            let b = src(1)?;
            if b.desc.ty != TensorType::F32 {
                return Err(unsupported_node(t, "non-f32 activations"));
            }
            let m_total = (b.ne[1] * b.ne[2] * b.ne[3]) as usize;
            match a.desc.ty {
                TensorType::F16 => KernelSel::MulMatBatched { a_f16: true },
                TensorType::F32 => {
                    if a.ne[2] != 1 || a.ne[3] != 1 || b.ne[2] != 1 || b.ne[3] != 1 {
                        KernelSel::MulMatBatched { a_f16: false }
                    } else if m_total <= MMV_MAX_COLUMNS {
                        KernelSel::MmvF32
                    } else {
                        KernelSel::GemmF32
                    }
                }
                ty => {
                    let kind = quant_kind(ty).ok_or_else(|| {
                        unsupported_node(t, &format!("matmul with {} weights", ty.name()))
                    })?;
                    if a.ne[2] != 1 || a.ne[3] != 1 || b.ne[2] != 1 || b.ne[3] != 1 {
                        return Err(unsupported_node(t, "batched quantized matmul"));
                    }
                    if a.ne[0] % 256 != 0 {
                        return Err(unsupported_node(t, "K not a multiple of the superblock"));
                    }
                    if m_total <= MMV_MAX_COLUMNS {
                        KernelSel::MmvQuant(kind)
                    } else {
                        KernelSel::GemmQuant(kind)
                    }
                }
            }
        }
        Op::SsmConv => KernelSel::SsmConv,
        Op::GatedDeltaNet => {
            let g = src(3)?;
            let beta = src(4)?;
            let v = src(2)?;
            let sv = v.ne[0];
            let g_n = g.ne.iter().product::<i64>();
            let b_n = beta.ne.iter().product::<i64>();
            if g_n != b_n && g_n != b_n * sv {
                return Err(unsupported_node(t, "unrecognized gate layout"));
            }
            KernelSel::GatedDeltaNet
        }
        other => {
            return Err(unsupported_node(
                t,
                &format!("op {other:?} has no CUDA kernel"),
            ))
        }
    })
}

const GLU_SWIGLU: i32 = 2;

fn mmvq_q81_shape_ok(t: &Tensor, act: &Tensor, kind: i32) -> bool {
    let k = t.ne[0];
    let m = act.ne[1] * act.ne[2] * act.ne[3];
    m == 1
        && k % 256 == 0
        && t.ne[2] == 1
        && t.ne[3] == 1
        && act.ne[2] == 1
        && act.ne[3] == 1
        && act.nb[0] == 4
        && (kind == MKLLM_QUANT_Q4K || kind == MKLLM_QUANT_Q5K || kind == MKLLM_QUANT_Q6K)
}

fn tensor_used_elsewhere(tensors: &[Tensor], nodes: &[TensorId], id: TensorId, except: TensorId) -> bool {
    for &node_id in nodes {
        if node_id == except {
            continue;
        }
        if let Some(t) = tensors.get(node_id) {
            if t.src.iter().flatten().any(|&src| src == id) {
                return true;
            }
        }
    }
    false
}

// ggml-cuda.cu:3678 skips VIEW/RESHAPE/TRANSPOSE/PERMUTE/NONE before fusion.
// Official qwen35 FFN is adjacent MUL_MAT+MUL_MAT+GLU; our expand inserts
// those noops between compute nodes. Walk past Skip only — never past a
// launched kernel. ggml_can_fuse (ggml-impl.h:693) still requires the
// compute ops themselves to be the next launched nodes.
fn next_compute(nodes: &[PlannedNode], from: usize) -> Option<usize> {
    (from + 1..nodes.len()).find(|&i| !matches!(nodes[i].kernel, KernelSel::Skip))
}

// llama.cpp ggml_cuda_should_fuse_mul_mat_vec_q: MUL_MAT + MUL_MAT + GLU
// with shared activations, same weight type/shape, SiLU/SwiGLU, M=1.
// Evaluate: ggml-cuda.cu:3887-3921.
fn fuse_mmvq_swiglu(tensors: &[Tensor], graph_nodes: &[TensorId], nodes: &mut [PlannedNode]) -> usize {
    if std::env::var_os("MKLLM_DISABLE_MMVQ_FUSION")
        .map(|v| v == "1")
        .unwrap_or(false)
        || std::env::var_os("MKLLM_DISABLE_Q81_MMVQ")
            .map(|v| v == "1")
            .unwrap_or(false)
    {
        return 0;
    }
    let mut fused = 0usize;
    let mut i = 0;
    while i < nodes.len() {
        let KernelSel::MmvQuant(kind0) = nodes[i].kernel else {
            i += 1;
            continue;
        };
        let Some(j) = next_compute(nodes, i) else {
            break;
        };
        let Some(k) = next_compute(nodes, j) else {
            break;
        };
        let (kind1, glu_op) = match (nodes[j].kernel, nodes[k].kernel) {
            (KernelSel::MmvQuant(kind1), KernelSel::Glu(op)) => (kind1, op),
            _ => {
                i += 1;
                continue;
            }
        };
        if kind0 != kind1 || glu_op != GLU_SWIGLU {
            i += 1;
            continue;
        }
        let a_id = nodes[i].node_id;
        let b_id = nodes[j].node_id;
        let glu_id = nodes[k].node_id;
        let glu = &tensors[glu_id];
        if glu.op_param_i32(1) != 0 {
            i += 1;
            continue;
        }
        let (gate_id, up_id) = match (glu.src[0], glu.src[1]) {
            (Some(g), Some(u)) if g == a_id && u == b_id => (a_id, b_id),
            (Some(g), Some(u)) if g == b_id && u == a_id => (b_id, a_id),
            _ => {
                i += 1;
                continue;
            }
        };
        let gate = &tensors[gate_id];
        let up = &tensors[up_id];
        let (Some(gate_w), Some(gate_act)) = (gate.src[0], gate.src[1]) else {
            i += 1;
            continue;
        };
        let (Some(up_w), Some(up_act)) = (up.src[0], up.src[1]) else {
            i += 1;
            continue;
        };
        if gate_act != up_act {
            i += 1;
            continue;
        }
        let gate_w_t = &tensors[gate_w];
        let up_w_t = &tensors[up_w];
        let act = &tensors[up_act];
        if gate_w_t.ne != up_w_t.ne
            || gate_w_t.nb != up_w_t.nb
            || !mmvq_q81_shape_ok(up_w_t, act, kind0)
        {
            i += 1;
            continue;
        }
        if tensor_used_elsewhere(tensors, graph_nodes, gate_id, glu_id)
            || tensor_used_elsewhere(tensors, graph_nodes, up_id, glu_id)
        {
            i += 1;
            continue;
        }
        nodes[i].kernel = KernelSel::Skip;
        nodes[j].kernel = KernelSel::Skip;
        nodes[k].kernel = KernelSel::MmvqFusedSwiglu { kind: kind0 };
        fused += 1;
        i = k + 1;
    }
    fused
}

const UNARY_SIGMOID: i32 = 7;
const UNARY_SILU: i32 = 10;
const UNARY_SOFTPLUS: i32 = 15;

fn fusion_disabled(var: &str) -> bool {
    std::env::var_os(var).map(|v| v == "1").unwrap_or(false)
}

// llama.cpp ggml-cuda.cu:3371-3410 / 3994-4004: RMS_NORM + MUL (+ ADD).
// Intermediate nodes must have a single use (ggml_can_fuse_ext n_uses==1).
fn fuse_rms_norm_mul(tensors: &[Tensor], graph_nodes: &[TensorId], nodes: &mut [PlannedNode]) -> usize {
    if fusion_disabled("MKLLM_DISABLE_RMS_FUSION") {
        return 0;
    }
    let mut fused = 0usize;
    let mut i = 0;
    while i < nodes.len() {
        if !matches!(nodes[i].kernel, KernelSel::Norm { l2: false }) {
            i += 1;
            continue;
        }
        let Some(j) = next_compute(nodes, i) else {
            break;
        };
        if !matches!(nodes[j].kernel, KernelSel::Binary(BIN_MUL)) {
            i += 1;
            continue;
        }
        let rms_id = nodes[i].node_id;
        let mul_id = nodes[j].node_id;
        let mul = &tensors[mul_id];
        let (Some(a), Some(b)) = (mul.src[0], mul.src[1]) else {
            i += 1;
            continue;
        };
        let (other_id, rms_is_b) = if a == rms_id {
            (b, false)
        } else if b == rms_id {
            (a, true)
        } else {
            i += 1;
            continue;
        };
        let other = &tensors[other_id];
        if rms_is_b && !other.are_same_shape(&tensors[rms_id]) {
            i += 1;
            continue;
        }
        if !tensors[a].is_contiguous_rows() || !tensors[b].is_contiguous_rows() {
            i += 1;
            continue;
        }
        if tensor_used_elsewhere(tensors, graph_nodes, rms_id, mul_id) {
            i += 1;
            continue;
        }
        let add_idx = next_compute(nodes, j).filter(|&k| {
            matches!(nodes[k].kernel, KernelSel::Binary(BIN_ADD)) && {
                let add_id = nodes[k].node_id;
                let add = &tensors[add_id];
                match (add.src[0], add.src[1]) {
                    (Some(aa), Some(ab)) if aa == mul_id || ab == mul_id => {
                        let add0 = &tensors[aa];
                        let add1 = &tensors[ab];
                        add0.is_contiguous()
                            && add1.is_contiguous_rows()
                            && !tensor_used_elsewhere(tensors, graph_nodes, mul_id, add_id)
                    }
                    _ => false,
                }
            }
        });
        nodes[i].kernel = KernelSel::Skip;
        if let Some(k) = add_idx {
            nodes[j].kernel = KernelSel::Skip;
            nodes[k].kernel = KernelSel::NormMul { add: true };
            fused += 1;
            i = k + 1;
        } else {
            nodes[j].kernel = KernelSel::NormMul { add: false };
            fused += 1;
            i = j + 1;
        }
    }
    fused
}

// llama.cpp ggml-cuda.cu:3425-3450 / 4012-4017: UNARY + MUL.
fn fuse_unary_mul(tensors: &[Tensor], graph_nodes: &[TensorId], nodes: &mut [PlannedNode]) -> usize {
    if fusion_disabled("MKLLM_DISABLE_UNARY_MUL_FUSION") {
        return 0;
    }
    let mut fused = 0usize;
    let mut i = 0;
    while i < nodes.len() {
        let op = match nodes[i].kernel {
            KernelSel::Unary(op)
                if op == UNARY_SILU || op == UNARY_SIGMOID || op == UNARY_SOFTPLUS =>
            {
                op
            }
            _ => {
                i += 1;
                continue;
            }
        };
        let Some(j) = next_compute(nodes, i) else {
            break;
        };
        if !matches!(nodes[j].kernel, KernelSel::Binary(BIN_MUL)) {
            i += 1;
            continue;
        }
        let unary_id = nodes[i].node_id;
        let mul_id = nodes[j].node_id;
        let unary = &tensors[unary_id];
        let mul = &tensors[mul_id];
        let Some(unary_src_id) = unary.src[0] else {
            i += 1;
            continue;
        };
        let (Some(a), Some(b)) = (mul.src[0], mul.src[1]) else {
            i += 1;
            continue;
        };
        if a != unary_id && b != unary_id {
            i += 1;
            continue;
        }
        let other_id = if a == unary_id { b } else { a };
        let unary_src = &tensors[unary_src_id];
        let other = &tensors[other_id];
        if !unary_src.is_contiguous_1()
            || !other.is_contiguous_1()
            || !other.are_same_shape(unary)
            || tensor_used_elsewhere(tensors, graph_nodes, unary_id, mul_id)
        {
            i += 1;
            continue;
        }
        nodes[i].kernel = KernelSel::Skip;
        nodes[j].kernel = KernelSel::UnaryMul(op);
        fused += 1;
        i = j + 1;
    }
    fused
}

// llama.cpp ggml-cuda.cu:3413-3423 / 4006-4009: SSM_CONV + SILU.
fn fuse_ssm_conv_silu(tensors: &[Tensor], graph_nodes: &[TensorId], nodes: &mut [PlannedNode]) -> usize {
    if fusion_disabled("MKLLM_DISABLE_SSM_SILU_FUSION") {
        return 0;
    }
    let mut fused = 0usize;
    let mut i = 0;
    while i < nodes.len() {
        if !matches!(nodes[i].kernel, KernelSel::SsmConv) {
            i += 1;
            continue;
        }
        let Some(j) = next_compute(nodes, i) else {
            break;
        };
        if !matches!(nodes[j].kernel, KernelSel::Unary(UNARY_SILU)) {
            i += 1;
            continue;
        }
        let conv_id = nodes[i].node_id;
        let silu_id = nodes[j].node_id;
        let silu = &tensors[silu_id];
        if silu.src[0] != Some(conv_id)
            || tensor_used_elsewhere(tensors, graph_nodes, conv_id, silu_id)
        {
            i += 1;
            continue;
        }
        nodes[i].kernel = KernelSel::Skip;
        nodes[j].kernel = KernelSel::SsmConvSilu;
        fused += 1;
        i = j + 1;
    }
    fused
}

// llama.cpp writes recurrent conv state with one CPY into a cache view
// (qwen35.cpp:267-276, ggml-cuda.cu:2520). We still build CONT+SET_ROWS;
// collapse that pair to the same single strided copy.
fn fuse_cont_set_rows(tensors: &[Tensor], graph_nodes: &[TensorId], nodes: &mut [PlannedNode]) {
    if fusion_disabled("MKLLM_DISABLE_CPY_FUSION") {
        return;
    }
    let mut i = 0;
    while i + 1 < nodes.len() {
        match (nodes[i].kernel, nodes[i + 1].kernel) {
            (KernelSel::CopyStrided, KernelSel::SetRows { .. }) => {}
            _ => {
                i += 1;
                continue;
            }
        }
        let cont_id = nodes[i].node_id;
        let set_id = nodes[i + 1].node_id;
        let set = &tensors[set_id];
        if set.src[0] != Some(cont_id) {
            i += 1;
            continue;
        }
        let cont = &tensors[cont_id];
        if cont.src[0].is_none()
            || cont.desc.ty != set.desc.ty
            || tensor_used_elsewhere(tensors, graph_nodes, cont_id, set_id)
        {
            i += 1;
            continue;
        }
        let src_elems = cont.ne.iter().product::<i64>();
        let dst_elems = set.ne.iter().product::<i64>();
        if src_elems != dst_elems || src_elems <= 0 {
            i += 1;
            continue;
        }
        nodes[i].kernel = KernelSel::Skip;
        nodes[i + 1].kernel = KernelSel::CopyViewTo;
        i += 2;
    }
}

// llama.cpp writes recurrent S-state with one CPY into a cache view
// (qwen35.cpp:346, ggml-cuda.cu:2520). We still build SET_ROWS of that
// view; collapse a same-type same-size F32 write to the same single copy.
// Official SET_ROWS remains only the attn K/V f16 path.
fn fuse_set_rows_cpy(tensors: &[Tensor], nodes: &mut [PlannedNode]) {
    if fusion_disabled("MKLLM_DISABLE_CPY_FUSION") {
        return;
    }
    for node in nodes.iter_mut() {
        let KernelSel::SetRows { f16: false } = node.kernel else {
            continue;
        };
        let set = &tensors[node.node_id];
        let Some(src_id) = set.src[0] else {
            continue;
        };
        let src = &tensors[src_id];
        if src.desc.ty != set.desc.ty {
            continue;
        }
        let src_elems = src.ne.iter().product::<i64>();
        let dst_elems = set.ne.iter().product::<i64>();
        if src_elems != dst_elems || src_elems <= 0 {
            continue;
        }
        node.kernel = KernelSel::CopyViewTo;
    }
}

fn dump_plan(nodes: &[PlannedNode], tensors: &[Tensor]) {
    if std::env::var_os("MAKEPAD_LLAMA_CUDA_PROFILE").is_none()
        && std::env::var_os("MAKEPAD_LLAMA_CUDA_DUMP_PLAN").is_none()
    {
        return;
    }
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut launched = 0usize;
    for node in nodes {
        *counts.entry(format!("{:?}", node.kernel)).or_default() += 1;
        if !matches!(node.kernel, KernelSel::Skip) {
            launched += 1;
        }
    }
    eprintln!(
        "cuda.plan: nodes={} launched={} skipped={}",
        nodes.len(),
        launched,
        nodes.len() - launched
    );
    for (kernel, count) in &counts {
        eprintln!("cuda.plan.kernel: {kernel} count={count}");
    }
    let mut shown = 0usize;
    for (index, node) in nodes.iter().enumerate() {
        if matches!(node.kernel, KernelSel::Skip) {
            continue;
        }
        if shown >= 96 {
            eprintln!("cuda.plan.seq: ... {} more launched", launched - shown);
            break;
        }
        let tensor = &tensors[node.node_id];
        eprintln!(
            "cuda.plan.seq: i={index} {:?} op={:?} name={} ne={:?}",
            node.kernel,
            tensor.op,
            tensor.name().unwrap_or("<unnamed>"),
            tensor.ne
        );
        shown += 1;
    }
    for (index, node) in nodes.iter().enumerate() {
        match node.kernel {
            KernelSel::FlashDecode | KernelSel::FlashVec | KernelSel::FlashMma => {
                let tensor = &tensors[node.node_id];
                let k_ne = tensor
                    .src
                    .get(1)
                    .and_then(|id| *id)
                    .and_then(|id| tensors.get(id))
                    .map(|k| k.ne);
                let q_ne = tensor
                    .src
                    .get(0)
                    .and_then(|id| *id)
                    .and_then(|id| tensors.get(id))
                    .map(|q| q.ne);
                eprintln!(
                    "cuda.plan.flash: i={index} {:?} name={} q_ne={:?} k_ne={:?} dst_ne={:?}",
                    node.kernel,
                    tensor.name().unwrap_or("<unnamed>"),
                    q_ne,
                    k_ne,
                    tensor.ne
                );
            }
            KernelSel::CopyStrided
            | KernelSel::CopyViewTo
            | KernelSel::SetRows { .. }
            | KernelSel::RopeMulti
            | KernelSel::Norm { .. }
            | KernelSel::Unary(_)
            | KernelSel::Glu(_) => {
                let tensor = &tensors[node.node_id];
                eprintln!(
                    "cuda.plan.extra: i={index} {:?} op={:?} name={} ne={:?} nb={:?}",
                    node.kernel,
                    tensor.op,
                    tensor.name().unwrap_or("<unnamed>"),
                    tensor.ne,
                    tensor.nb
                );
            }
            _ => {}
        }
    }
}

fn plan_graph(ctx: &Context, decode: &HybridDecodeGraph) -> Result<GraphPlan> {
    let mut pinned = vec![decode.result_logits, decode.result_hidden];
    pinned.extend(decode.moe_selected_experts.iter().map(|s| s.selected_experts));
    pinned.extend(decode.state_updates.iter().copied());
    pinned.extend(decode.debug_outputs.iter().copied());
    plan_raw_graph(ctx, &decode.graph, &pinned)
}

fn plan_raw_graph(
    ctx: &Context,
    graph: &crate::Graph,
    pinned: &[TensorId],
) -> Result<GraphPlan> {
    let tensors = ctx.tensors();

    // Kernel selection + validation first: fail closed before any memory
    // planning if an op/layout/type is not executable.
    let mut nodes = Vec::with_capacity(graph.nodes.len());
    for &node_id in &graph.nodes {
        let t = tensors
            .get(node_id)
            .ok_or_else(|| LlamaError::format(format!("invalid graph node {node_id}")))?;
        nodes.push(PlannedNode {
            node_id,
            kernel: select_kernel(tensors, t)?,
        });
    }
    let fused_mmvq = fuse_mmvq_swiglu(tensors, &graph.nodes, &mut nodes);
    let fused_rms = fuse_rms_norm_mul(tensors, &graph.nodes, &mut nodes);
    let fused_unary = fuse_unary_mul(tensors, &graph.nodes, &mut nodes);
    let fused_ssm = fuse_ssm_conv_silu(tensors, &graph.nodes, &mut nodes);
    fuse_cont_set_rows(tensors, &graph.nodes, &mut nodes);
    fuse_set_rows_cpy(tensors, &mut nodes);
    dump_plan(&nodes, tensors);

    // Lifetimes over root storage: def = first producing node, last_use =
    // last reading node; graph outputs are pinned alive to the end.
    let mut needed: Vec<TensorId> = graph.nodes.clone();
    needed.extend(graph.leafs.iter().copied());
    needed.sort_unstable();
    needed.dedup();

    let mut last_use: BTreeMap<TensorId, usize> = BTreeMap::new();
    let mut def: BTreeMap<TensorId, usize> = BTreeMap::new();
    for (index, &node_id) in graph.nodes.iter().enumerate() {
        let t = &tensors[node_id];
        for src in t.src.iter().flatten() {
            let (root, _) = resolve_root(tensors, *src)?;
            last_use.insert(root, index);
        }
        let (out_root, _) = resolve_root(tensors, node_id)?;
        def.entry(out_root).or_insert(index);
        // A node writing through a view also READS the root layout; keep
        // the root alive through this node.
        last_use.insert(out_root, index.max(last_use.get(&out_root).copied().unwrap_or(0)));
    }
    let end = graph.nodes.len();
    for &pin in pinned {
        let (root, _) = resolve_root(tensors, pin)?;
        last_use.insert(root, end);
    }
    // CopyViewTo reads the CONT source view at the SET_ROWS index.
    for (index, node) in nodes.iter().enumerate() {
        if !matches!(node.kernel, KernelSel::CopyViewTo) {
            continue;
        }
        let set = &tensors[node.node_id];
        let Some(cont_id) = set.src[0] else {
            continue;
        };
        let Some(view_id) = tensors[cont_id].src[0] else {
            continue;
        };
        let (root, _) = resolve_root(tensors, view_id)?;
        last_use.insert(root, index.max(last_use.get(&root).copied().unwrap_or(0)));
    }

    // Offset assignment: resident tensors keep their logical offsets; leaf
    // inputs allocate up front; node outputs allocate at their def index
    // with first-fit reuse of blocks whose lifetime ended strictly before.
    let base = align_up(ctx.used_mem());
    let mut cursor = base;
    let mut max_end = ctx.used_mem();
    let mut free: Vec<(usize, usize)> = Vec::new(); // (offset, size)
    let mut planned: BTreeMap<TensorId, usize> = BTreeMap::new();

    let allocate = |root: TensorId,
                        size: usize,
                        free: &mut Vec<(usize, usize)>,
                        cursor: &mut usize,
                        max_end: &mut usize,
                        planned: &mut BTreeMap<TensorId, usize>| {
        let size = align_up(size);
        // Best-fit over freed blocks.
        let mut best: Option<usize> = None;
        for (index, &(_, free_size)) in free.iter().enumerate() {
            if free_size >= size && best.map_or(true, |b| free[b].1 > free_size) {
                best = Some(index);
            }
        }
        let offset = if let Some(index) = best {
            let (offset, free_size) = free.remove(index);
            if free_size > size {
                free.push((offset + size, free_size - size));
            }
            offset
        } else {
            let offset = *cursor;
            *cursor += size;
            offset
        };
        *max_end = (*max_end).max(offset + size);
        planned.insert(root, offset);
        offset
    };

    // Leaf inputs (token ids, positions, masks, ...) with no resident data.
    for &leaf_id in &graph.leafs {
        let (root, _) = resolve_root(tensors, leaf_id)?;
        let root_t = &tensors[root];
        if root_t.data_offset.is_none() && !planned.contains_key(&root) {
            allocate(
                root,
                root_t.nbytes(),
                &mut free,
                &mut cursor,
                &mut max_end,
                &mut planned,
            );
        }
    }

    // Expiry index: node index -> roots whose last use is that index.
    let mut expiry: BTreeMap<usize, Vec<TensorId>> = BTreeMap::new();
    for (&root, &use_end) in &last_use {
        expiry.entry(use_end).or_default().push(root);
    }

    for (index, &node_id) in graph.nodes.iter().enumerate() {
        let (out_root, _) = resolve_root(tensors, node_id)?;
        let root_t = &tensors[out_root];
        if root_t.data_offset.is_none()
            && !planned.contains_key(&out_root)
            && def.get(&out_root) == Some(&index)
        {
            allocate(
                out_root,
                root_t.nbytes(),
                &mut free,
                &mut cursor,
                &mut max_end,
                &mut planned,
            );
        }
        // Free blocks that die at this node (after its output allocation so
        // outputs never alias this node's own inputs).
        // MAKEPAD_LLAMA_CUDA_NO_REUSE=1 disables reuse entirely (pure bump
        // allocation) — the A/B switch for isolating aliasing bugs.
        if std::env::var_os("MAKEPAD_LLAMA_CUDA_NO_REUSE").is_none() {
            if let Some(dead) = expiry.get(&index) {
                for root in dead {
                    if let Some(&offset) = planned.get(root) {
                        let size = align_up(tensors[*root].nbytes());
                        free.push((offset, size));
                    }
                }
            }
        }
    }

    // Final logical offsets for every needed tensor (views resolved).
    let mut bindings = BTreeMap::new();
    for tensor_id in needed {
        let t = &tensors[tensor_id];
        let (root, view_extra) = resolve_root(tensors, tensor_id)?;
        let root_t = &tensors[root];
        let root_offset = if let Some(offset) = root_t.data_offset {
            offset
        } else if let Some(&offset) = planned.get(&root) {
            offset
        } else {
            return Err(LlamaError::format(format!(
                "tensor '{}' has no planned storage",
                t.name().unwrap_or("<unnamed>")
            )));
        };
        bindings.insert(tensor_id, root_offset + view_extra);
    }

    Ok(GraphPlan {
        nodes,
        bindings,
        required_size: align_up(max_end),
        fused_mmvq,
        fused_rms,
        fused_unary,
        fused_ssm,
    })
}

fn align_up(v: usize) -> usize {
    (v + 63) & !63
}

// ---------------------------------------------------------------------------
// Compiled graph + execution
// ---------------------------------------------------------------------------

struct ArenaRef {
    ro_dev: *mut c_void,
    ro_split: usize,
    main_dev: *mut c_void,
    main_size: usize,
    _keep: Rc<DeviceState>,
}

impl ArenaRef {
    fn ptr_at(&self, offset: usize, len: usize) -> Result<*mut c_void> {
        if offset < self.ro_split {
            if offset + len > self.ro_split {
                return Err(LlamaError::format(
                    "device range straddles the read-only weight region",
                ));
            }
            Ok(unsafe { (self.ro_dev as *mut u8).add(offset) } as *mut c_void)
        } else {
            let rel = offset - self.ro_split;
            if rel + len > self.main_size {
                return Err(LlamaError::format(format!(
                    "device range [{}..+{}) exceeds arena size {}",
                    offset, len, self.main_size
                )));
            }
            Ok(unsafe { (self.main_dev as *mut u8).add(rel) } as *mut c_void)
        }
    }
}

pub(super) struct Compiled {
    state: Rc<DeviceState>,
    arena: ArenaRef,
    spec: HybridDecodeSpec,
    ctx: Context,
    decode: HybridDecodeGraph,
    plan: GraphPlan,
    graph_exec: RefCell<Option<CudaGraphExec>>,
    graph_disabled: Cell<bool>,
    // ggml-cuda.cu:4125-4133: first matching call is eager so cuBLAS can
    // allocate workspace; the next identical call is captured.
    graph_warmup: Cell<bool>,
}

impl Compiled {
    pub(super) fn decode(&self) -> &HybridDecodeGraph {
        &self.decode
    }

    pub(super) fn execute(
        &mut self,
        input: LogitsProbeInput<'_>,
        layout: &HybridDecodeBatchLayout,
        capture_hidden: bool,
    ) -> Result<HybridDecodeRun> {
        let split = host_split_enabled();
        let t_wall = split.then(Instant::now);
        let t_pre = split.then(Instant::now);
        validate_hybrid_decode_layout(&self.ctx, &self.spec, &self.decode, layout)?;
        let writes = build_hybrid_decode_writes(&self.ctx, &self.spec, &self.decode, input, layout)?;
        let host_pre_ms = t_pre
            .map(|start| start.elapsed().as_secs_f64() * 1e3)
            .unwrap_or(0.0);
        let write_bytes: u64 = writes.iter().map(|(_, bytes)| bytes.len() as u64).sum();

        let view = ExecView {
            state: &self.state,
            arena: &self.arena,
            ctx: &self.ctx,
            plan: &self.plan,
        };
        let mut wanted = vec![self.decode.result_logits];
        if capture_hidden {
            wanted.push(self.decode.result_hidden);
        }
        wanted.extend(self.decode.debug_outputs.iter().copied());
        let skip_logits = !capture_hidden && skip_logits_readback();
        let wanted_read = if skip_logits { &[][..] } else { wanted.as_slice() };
        let outputs = if self.graph_disabled.get()
            || std::env::var_os("MAKEPAD_LLAMA_CUDA_PROFILE").is_some()
        {
            view.run(&writes, wanted_read)?
        } else {
            Self::execute_with_graph(self, &view, &writes, wanted_read)?
        };

        if skip_logits {
            if split {
                let wall_ms = t_wall
                    .map(|start| start.elapsed().as_secs_f64() * 1e3)
                    .unwrap_or(0.0);
                HOST_SPLIT.with(|slot| {
                    let mut acc = slot.borrow_mut();
                    acc.tokens += 1;
                    acc.write_bytes += write_bytes;
                    acc.host_pre_ms += host_pre_ms;
                    acc.wall_ms += wall_ms;
                });
            }
            return Ok(HybridDecodeRun {
                hidden: Vec::new(),
                logits: Vec::new(),
                n_tokens: layout.positions.len().max(1),
                hidden_size: 0,
                vocab_size: 0,
                selected_experts: Vec::new(),
            });
        }

        debug_trace_outputs("cuda", &self.ctx, &self.decode, &outputs)?;

        let output_config = if capture_hidden {
            HybridDecodeOutputConfig::FULL
        } else {
            HybridDecodeOutputConfig::LOGITS_ONLY
        };
        let t_collect = split.then(Instant::now);
        let run = collect_hybrid_decode_run(&self.ctx, &self.decode, output_config, &outputs)?;
        if split {
            let host_collect_ms = t_collect
                .map(|start| start.elapsed().as_secs_f64() * 1e3)
                .unwrap_or(0.0);
            let wall_ms = t_wall
                .map(|start| start.elapsed().as_secs_f64() * 1e3)
                .unwrap_or(0.0);
            HOST_SPLIT.with(|slot| {
                let mut acc = slot.borrow_mut();
                acc.tokens += 1;
                acc.write_bytes += write_bytes;
                acc.host_pre_ms += host_pre_ms;
                acc.host_collect_ms += host_collect_ms;
                acc.wall_ms += wall_ms;
            });
        }
        Ok(run)
    }

    fn execute_with_graph(
        &self,
        view: &ExecView<'_>,
        writes: &[(TensorId, Vec<u8>)],
        wanted: &[TensorId],
    ) -> Result<BTreeMap<TensorId, Vec<u8>>> {
        let split = host_split_enabled();
        let ev0 = self.state.split_ev0.get();
        let ev1 = self.state.split_ev1.get();
        let ev2 = self.state.split_ev2.get();
        let can_events = split && !ev0.is_null() && !ev1.is_null() && !ev2.is_null();
        let t_h2d = split.then(Instant::now);
        view.write_inputs(writes)?;
        let host_h2d_ms = t_h2d
            .map(|start| start.elapsed().as_secs_f64() * 1e3)
            .unwrap_or(0.0);
        self.state
            .scratch_acts
            .borrow_mut()
            .ensure(GRAPH_ACT_SCRATCH, "graph act scratch")?;
        self.state
            .scratch_weights
            .borrow_mut()
            .ensure(GRAPH_WEIGHT_SCRATCH, "graph weight scratch")?;
        let stream = self.state.stream;
        let t_launch = split.then(Instant::now);
        let replayed = {
            let guard = self.graph_exec.borrow();
            if let Some(exec) = guard.as_ref() {
                if can_events {
                    check(
                        unsafe { cudaEventRecord(ev0, stream) },
                        "host split ev0",
                    )?;
                }
                exec.launch(stream)
                    .map_err(|err| LlamaError::format(format!("cuda graph launch: {err}")))?;
                if can_events {
                    check(
                        unsafe { cudaEventRecord(ev1, stream) },
                        "host split ev1",
                    )?;
                }
                true
            } else {
                false
            }
        };
        let host_launch_cpu_ms = t_launch
            .map(|start| start.elapsed().as_secs_f64() * 1e3)
            .unwrap_or(0.0);
        if !replayed {
            // ggml-cuda.cu:4125-4133: first call after compile is eager.
            if !self.graph_warmup.get() {
                view.dispatch_all(None)?;
                self.graph_warmup.set(true);
            } else {
                // last_q81 reuse is CPU-side; a leftover hit from the warmup
                // token would omit quantize from the captured graph.
                self.state.last_q81_src.set(std::ptr::null());
                self.state.last_q81_k.set(0);
                begin_stream_capture(stream, CUDA_STREAM_CAPTURE_MODE_RELAXED)
                    .map_err(|err| LlamaError::format(format!("cuda graph begin capture: {err}")))?;
                let dispatch = view.dispatch_all(None);
                if let Err(err) = dispatch {
                    let _ = end_stream_capture(stream);
                    return Err(err);
                }
                match end_stream_capture(stream).and_then(makepad_ai_cuda::CudaGraph::instantiate) {
                    Ok(exec) => {
                        exec.launch(stream).map_err(|err| {
                            LlamaError::format(format!("cuda graph first launch: {err}"))
                        })?;
                        let launched = self
                            .plan
                            .nodes
                            .iter()
                            .filter(|node| !matches!(node.kernel, KernelSel::Skip))
                            .count();
                        let mut extras: BTreeMap<String, usize> = BTreeMap::new();
                        for node in &self.plan.nodes {
                            match node.kernel {
                                KernelSel::Skip => {}
                                KernelSel::CopyStrided
                                | KernelSel::CopyViewTo
                                | KernelSel::SetRows { .. }
                                | KernelSel::Unary(_)
                                | KernelSel::Glu(_)
                                | KernelSel::Norm { .. }
                                | KernelSel::Concat
                                | KernelSel::GetRowsF32
                                | KernelSel::GetRowsQuant(_) => {
                                    *extras.entry(format!("{:?}", node.kernel)).or_default() += 1;
                                }
                                _ => {}
                            }
                        }
                        let mut glu_prev = Vec::new();
                        for (index, node) in self.plan.nodes.iter().enumerate() {
                            if !matches!(node.kernel, KernelSel::Glu(_)) || glu_prev.len() >= 4 {
                                continue;
                            }
                            let prev: Vec<String> = self.plan.nodes[..index]
                                .iter()
                                .rev()
                                .filter(|n| !matches!(n.kernel, KernelSel::Skip))
                                .take(2)
                                .map(|n| format!("{:?}", n.kernel))
                                .collect();
                            glu_prev.push(prev);
                        }
                        eprintln!(
                            "cuda.graph: captured launched={} plan_nodes={} \
                             fused_mmvq={} fused_rms={} fused_unary={} fused_ssm={} \
                             extras={:?} glu_prev={:?}",
                            launched,
                            self.plan.nodes.len(),
                            self.plan.fused_mmvq,
                            self.plan.fused_rms,
                            self.plan.fused_unary,
                            self.plan.fused_ssm,
                            extras,
                            glu_prev
                        );
                        *self.graph_exec.borrow_mut() = Some(exec);
                    }
                    Err(err) => {
                        eprintln!("cuda.graph: capture failed ({err}); staying on eager dispatch");
                        self.graph_disabled.set(true);
                        view.dispatch_all(None)?;
                    }
                }
            }
        }
        let after_copy = if can_events && replayed {
            Some(ev2)
        } else {
            None
        };
        let outputs = view.read_outputs(wanted, after_copy)?;
        if can_events && replayed {
            let mut graph_ms = 0.0f32;
            let mut d2h_ms = 0.0f32;
            check(
                unsafe { cudaEventElapsedTime(&mut graph_ms, ev0, ev1) },
                "host split graph elapsed",
            )?;
            check(
                unsafe { cudaEventElapsedTime(&mut d2h_ms, ev1, ev2) },
                "host split d2h elapsed",
            )?;
            HOST_SPLIT.with(|slot| {
                let mut acc = slot.borrow_mut();
                acc.host_h2d_ms += host_h2d_ms;
                acc.host_launch_cpu_ms += host_launch_cpu_ms;
                acc.gpu_graph_ms += f64::from(graph_ms);
                acc.gpu_d2h_ms += f64::from(d2h_ms);
            });
        }
        Ok(outputs)
    }
}

/// Borrowed execution view shared by the session path and the raw-graph
/// test path — one dispatch implementation, byte-identical behavior.
struct ExecView<'a> {
    state: &'a DeviceState,
    arena: &'a ArenaRef,
    ctx: &'a Context,
    plan: &'a GraphPlan,
}

impl ExecView<'_> {
    fn write_inputs(&self, writes: &[(TensorId, Vec<u8>)]) -> Result<()> {
        let stream = self.state.stream;
        let total: usize = writes.iter().map(|(_, bytes)| bytes.len()).sum();
        if total == 0 {
            return Ok(());
        }
        // llama.cpp host inputs live in cudaMallocHost buffers
        // (ggml-cuda.cu:1147). Pageable Vec H2D is a WDDM bounce; stage
        // through the same pinned host scratch used for logits D2H.
        let pinned = self
            .state
            .scratch_host
            .borrow_mut()
            .ensure(total, "pinned input host")?;
        unsafe {
            let mut cursor = 0usize;
            for (_, bytes) in writes {
                if !bytes.is_empty() {
                    std::ptr::copy_nonoverlapping(
                        bytes.as_ptr(),
                        pinned.add(cursor),
                        bytes.len(),
                    );
                    cursor += bytes.len();
                }
            }
            cursor = 0;
            for (tensor_id, bytes) in writes {
                if bytes.is_empty() {
                    continue;
                }
                let offset = self.binding(*tensor_id)?;
                check(
                    cudaMemcpyAsync(
                        self.arena.ptr_at(offset, bytes.len())?,
                        pinned.add(cursor) as *const c_void,
                        bytes.len(),
                        CUDA_MEMCPY_HOST_TO_DEVICE,
                        stream,
                    ),
                    "input write",
                )?;
                cursor += bytes.len();
            }
        }
        Ok(())
    }

    fn dispatch_all(&self, mut quant_profile: Option<&mut QuantTimeline>) -> Result<()> {
        for index in 0..self.plan.nodes.len() {
            self.dispatch_node(index, quant_profile.as_deref_mut())
                .map_err(|err| LlamaError::format(format!("node {index}: {err:?}")))?;
        }
        Ok(())
    }

    fn read_outputs(
        &self,
        wanted: &[TensorId],
        after_copy: Option<CudaEvent>,
    ) -> Result<BTreeMap<TensorId, Vec<u8>>> {
        let stream = self.state.stream;
        let mut outputs: BTreeMap<TensorId, Vec<u8>> = BTreeMap::new();
        let total: usize = wanted
            .iter()
            .map(|&tensor_id| {
                self.ctx
                    .tensor(tensor_id)
                    .map(|t| t.nbytes())
                    .unwrap_or(0)
            })
            .sum();
        let pinned = self
            .state
            .scratch_host
            .borrow_mut()
            .ensure(total.max(1), "pinned logits host")?;
        unsafe {
            let mut cursor = 0usize;
            for &tensor_id in wanted {
                let t = self
                    .ctx
                    .tensor(tensor_id)
                    .ok_or_else(|| LlamaError::format("invalid output tensor"))?;
                let len = t.nbytes();
                let offset = self.binding(tensor_id)?;
                check(
                    cudaMemcpyAsync(
                        pinned.add(cursor) as *mut c_void,
                        self.arena.ptr_at(offset, len)?,
                        len,
                        CUDA_MEMCPY_DEVICE_TO_HOST,
                        stream,
                    ),
                    "output read",
                )?;
                cursor += len;
            }
            if let Some(event) = after_copy {
                check(cudaEventRecord(event, stream), "host split ev2")?;
            }
            check(cudaStreamSynchronize(stream), "execute sync")?;
            cursor = 0;
            for &tensor_id in wanted {
                let t = self
                    .ctx
                    .tensor(tensor_id)
                    .ok_or_else(|| LlamaError::format("invalid output tensor"))?;
                let len = t.nbytes();
                let mut host = Vec::with_capacity(len);
                if len > 0 {
                    std::ptr::copy_nonoverlapping(pinned.add(cursor), host.as_mut_ptr(), len);
                    host.set_len(len);
                }
                outputs.insert(tensor_id, host);
                cursor += len;
            }
        }
        Ok(outputs)
    }

    fn run(
        &self,
        writes: &[(TensorId, Vec<u8>)],
        wanted: &[TensorId],
    ) -> Result<BTreeMap<TensorId, Vec<u8>>> {
        self.write_inputs(writes)?;
        let stream = self.state.stream;
        let mut profile = std::env::var_os("MAKEPAD_LLAMA_CUDA_PROFILE")
            .map(|_| EventTimeline::new(stream))
            .transpose()?;
        let mut quant_profile =
            std::env::var_os("MAKEPAD_LLAMA_CUDA_PROFILE").map(|_| QuantTimeline::new(stream));

        for index in 0..self.plan.nodes.len() {
            self.dispatch_node(index, quant_profile.as_mut())
                .map_err(|err| LlamaError::format(format!("node {index}: {err:?}")))?;
            if let Some(timeline) = profile.as_mut() {
                timeline.mark()?;
            }
        }

        let outputs = self.read_outputs(wanted, None)?;
        if let Some(timeline) = profile.as_ref() {
            self.report_profile(timeline)?;
        }
        if let Some(timeline) = quant_profile.as_ref() {
            self.report_quant_profile(timeline)?;
        }
        Ok(outputs)
    }

    fn report_quant_profile(&self, timeline: &QuantTimeline) -> Result<()> {
        let mut aggregate: BTreeMap<(i32, &'static str), (usize, f64, f32)> = BTreeMap::new();
        for stage in &timeline.stages {
            let ms = timeline.elapsed_ms(stage)?;
            let entry = aggregate
                .entry((stage.kind, stage.name))
                .or_insert((0, 0.0, 0.0));
            entry.0 += 1;
            entry.1 += f64::from(ms);
            entry.2 = entry.2.max(ms);
        }
        let mut aggregate = aggregate.into_iter().collect::<Vec<_>>();
        aggregate.sort_by(|lhs, rhs| (rhs.1).1.total_cmp(&(lhs.1).1));
        for ((kind, name), (count, sum_ms, max_ms)) in aggregate {
            eprintln!(
                "cuda.profile.quant: kind={kind} stage={name} count={count} total_ms={sum_ms:.3} avg_ms={:.4} max_ms={max_ms:.3}",
                sum_ms / count as f64,
            );
        }
        Ok(())
    }

    fn report_profile(&self, timeline: &EventTimeline) -> Result<()> {
        if timeline.events.len() != self.plan.nodes.len() + 1 {
            return Err(LlamaError::format("CUDA profile timeline length mismatch"));
        }

        let mut aggregate: BTreeMap<String, (usize, f64, f32)> = BTreeMap::new();
        let mut nodes = Vec::with_capacity(self.plan.nodes.len());
        let mut total_ms = 0.0f64;
        for (index, node) in self.plan.nodes.iter().enumerate() {
            let ms = timeline.elapsed_ms(index)?;
            total_ms += f64::from(ms);
            let key = format!("{:?}", node.kernel);
            let entry = aggregate.entry(key.clone()).or_insert((0, 0.0, 0.0));
            entry.0 += 1;
            entry.1 += f64::from(ms);
            entry.2 = entry.2.max(ms);
            let tensor = self.ctx.tensor(node.node_id).ok_or_else(|| {
                LlamaError::format(format!("invalid profiled tensor id {}", node.node_id))
            })?;
            nodes.push((
                ms,
                index,
                key,
                tensor.op,
                tensor.name().unwrap_or("<unnamed>").to_string(),
            ));
        }

        let mut aggregate = aggregate.into_iter().collect::<Vec<_>>();
        aggregate.sort_by(|lhs, rhs| (rhs.1).1.total_cmp(&(lhs.1).1));
        eprintln!(
            "cuda.profile: nodes={} total_ms={total_ms:.3}",
            self.plan.nodes.len()
        );
        for (kernel, (count, sum_ms, max_ms)) in aggregate {
            let percent = if total_ms > 0.0 {
                100.0 * sum_ms / total_ms
            } else {
                0.0
            };
            eprintln!(
                "cuda.profile.kernel: {kernel} count={count} total_ms={sum_ms:.3} pct={percent:.2} avg_ms={:.4} max_ms={max_ms:.3}",
                sum_ms / count as f64,
            );
        }

        nodes.sort_by(|lhs, rhs| rhs.0.total_cmp(&lhs.0));
        for (ms, index, kernel, op, name) in nodes.into_iter().take(20) {
            let extra = self
                .ctx
                .tensor(self.plan.nodes[index].node_id)
                .and_then(|tensor| {
                    let a = tensor.src[0].and_then(|id| self.ctx.tensor(id))?;
                    let b = tensor.src[1].and_then(|id| self.ctx.tensor(id))?;
                    Some(format!(
                        " w={} K={} N={} M={}",
                        a.desc.ty.name(),
                        a.ne[0],
                        a.ne[1],
                        b.ne[1] * b.ne[2] * b.ne[3]
                    ))
                })
                .unwrap_or_default();
            eprintln!(
                "cuda.profile.node: index={index} ms={ms:.3} kernel={kernel} op={op:?} name={name}{extra}"
            );
        }
        Ok(())
    }

    fn binding(&self, tensor_id: TensorId) -> Result<usize> {
        self.plan.bindings.get(&tensor_id).copied().ok_or_else(|| {
            LlamaError::format(format!("tensor {tensor_id} has no device binding"))
        })
    }

    fn tensor(&self, id: TensorId) -> Result<&Tensor> {
        self.ctx
            .tensor(id)
            .ok_or_else(|| LlamaError::format(format!("invalid tensor id {id}")))
    }

    fn src_id(&self, t: &Tensor, index: usize) -> Result<TensorId> {
        t.src[index]
            .ok_or_else(|| LlamaError::format(format!("node is missing src{index}")))
    }

    fn ptr_of(&self, id: TensorId) -> Result<*mut c_void> {
        let t = self.tensor(id)?;
        let len = t.nbytes();
        self.arena.ptr_at(self.binding(id)?, len)
    }

    fn dispatch_node(
        &self,
        index: usize,
        mut quant_profile: Option<&mut QuantTimeline>,
    ) -> Result<()> {
        let PlannedNode { node_id, kernel } = self.plan.nodes[index];
        let stream = self.state.stream;
        let t = self.tensor(node_id)?.clone();
        let tensors = self.ctx.tensors();

        match kernel {
            KernelSel::Skip => Ok(()),
            KernelSel::CopyStrided | KernelSel::CopyViewTo => {
                let (s, src_ptr) = if matches!(kernel, KernelSel::CopyViewTo) {
                    let mid_id = self.src_id(&t, 0)?;
                    let mid = &tensors[mid_id];
                    // CONT+SET_ROWS: skip the CONT and copy the view
                    // (qwen35.cpp:276). S-state SET_ROWS src is already
                    // the view (qwen35.cpp:346).
                    let src_id = match mid.op {
                        Op::Cont | Op::Cpy | Op::Dup => mid.src[0].ok_or_else(|| {
                            LlamaError::format("fused cpy missing view source")
                        })?,
                        _ => mid_id,
                    };
                    (&tensors[src_id], self.ptr_of(src_id)?)
                } else {
                    let s_id = self.src_id(&t, 0)?;
                    (&tensors[s_id], self.ptr_of(s_id)?)
                };
                let dst_ptr = self.ptr_of(node_id)?;
                let label = if matches!(kernel, KernelSel::CopyViewTo) {
                    "copy_view_to"
                } else {
                    "copy_strided"
                };
                // llama.cpp cpy.cu:410-418: same-type contiguous -> memcpy.
                if s.desc.ty == t.desc.ty && s.is_contiguous() && t.is_contiguous() {
                    let bytes = s.nbytes();
                    if bytes != t.nbytes() {
                        return Err(LlamaError::format(format!(
                            "{label}: contiguous cpy size mismatch {bytes} vs {}",
                            t.nbytes()
                        )));
                    }
                    return check(
                        unsafe {
                            cudaMemcpyAsync(
                                dst_ptr,
                                src_ptr,
                                bytes,
                                CUDA_MEMCPY_DEVICE_TO_DEVICE,
                                stream,
                            )
                        },
                        label,
                    );
                }
                let elem = if t.desc.ty == TensorType::F16 { 2 } else { 4 };
                check(
                    unsafe {
                        copy_strided(
                            elem,
                            src_ptr,
                            dst_ptr,
                            s.ne[0] as i32,
                            s.ne[1] as i32,
                            s.ne[2] as i32,
                            s.ne[3] as i32,
                            t.ne[0] as i32,
                            t.ne[1] as i32,
                            t.ne[2] as i32,
                            t.ne[3] as i32,
                            s.nb[0],
                            s.nb[1],
                            s.nb[2],
                            s.nb[3],
                            t.nb[0],
                            t.nb[1],
                            t.nb[2],
                            t.nb[3],
                            stream,
                        )
                    },
                    label,
                )
            }
            KernelSel::Concat => {
                // Lowered to two strided copies into the destination halves.
                let a_id = self.src_id(&t, 0)?;
                let b_id = self.src_id(&t, 1)?;
                let a = tensors[a_id].clone();
                let b = tensors[b_id].clone();
                let mut axis = None;
                for dim in 0..4 {
                    if a.ne[dim] + b.ne[dim] == t.ne[dim] {
                        axis = Some(dim);
                        break;
                    }
                }
                let axis = axis
                    .ok_or_else(|| LlamaError::format("concat axis could not be derived"))?;
                for (part, part_id) in [(&a, a_id), (&b, b_id)] {
                    let dst_extra = if std::ptr::eq(part, &b) {
                        a.ne[axis] as usize * t.nb[axis]
                    } else {
                        0
                    };
                    let dst_ptr =
                        unsafe { (self.ptr_of(node_id)? as *mut u8).add(dst_extra) } as *mut c_void;
                    check(
                        unsafe {
                            copy_strided(
                                4,
                                self.ptr_of(part_id)?,
                                dst_ptr,
                                part.ne[0] as i32,
                                part.ne[1] as i32,
                                part.ne[2] as i32,
                                part.ne[3] as i32,
                                part.ne[0] as i32,
                                part.ne[1] as i32,
                                part.ne[2] as i32,
                                part.ne[3] as i32,
                                part.nb[0],
                                part.nb[1],
                                part.nb[2],
                                part.nb[3],
                                t.nb[0],
                                t.nb[1],
                                t.nb[2],
                                t.nb[3],
                                stream,
                            )
                        },
                        "concat copy",
                    )?;
                }
                Ok(())
            }
            KernelSel::GetRowsF32 | KernelSel::GetRowsQuant(_) => {
                let s_id = self.src_id(&t, 0)?;
                let rows_id = self.src_id(&t, 1)?;
                let s = &tensors[s_id];
                let rows = &tensors[rows_id];
                let nrows = (rows.ne[0] * rows.ne[1] * rows.ne[2]) as i32;
                let err = unsafe {
                    match kernel {
                        KernelSel::GetRowsF32 => get_rows_f32(
                            self.ptr_of(s_id)?,
                            self.ptr_of(rows_id)? as *const i32,
                            self.ptr_of(node_id)?,
                            t.ne[0] as i32,
                            nrows,
                            s.nb[1],
                            t.nb[1],
                            stream,
                        ),
                        KernelSel::GetRowsQuant(kind) => get_rows_quant(
                            kind,
                            self.ptr_of(s_id)?,
                            self.ptr_of(rows_id)? as *const i32,
                            self.ptr_of(node_id)?,
                            t.ne[0] as i32,
                            nrows,
                            s.nb[1],
                            t.nb[1],
                            stream,
                        ),
                        _ => unreachable!(),
                    }
                };
                check(err, "get_rows")
            }
            KernelSel::SetRows { f16 } => {
                let s_id = self.src_id(&t, 0)?;
                let rows_id = self.src_id(&t, 1)?;
                let s = &tensors[s_id];
                check(
                    unsafe {
                        set_rows(
                            i32::from(f16),
                            self.ptr_of(s_id)?,
                            self.ptr_of(rows_id)? as *const i32,
                            self.ptr_of(node_id)?,
                            s.ne[0] as i32,
                            s.ne[1] as i32,
                            s.nb[1],
                            t.nb[1],
                            stream,
                        )
                    },
                    "set_rows",
                )
            }
            KernelSel::SoftMaxMasked => {
                let x_id = self.src_id(&t, 0)?;
                let x = &tensors[x_id];
                let scale = t.op_param_f32(0);
                let (mask_ptr, mask_nb1) = if let Some(mask_id) = t.src[1] {
                    let mask = &tensors[mask_id];
                    (self.ptr_of(mask_id)?, mask.nb[1])
                } else {
                    (std::ptr::null_mut(), 0)
                };
                check(
                    unsafe {
                        softmax_mask(
                            self.ptr_of(x_id)?,
                            mask_ptr,
                            self.ptr_of(node_id)?,
                            x.ne[0] as i32,
                            x.ne[1] as i32,
                            (x.ne[2] * x.ne[3]) as i32,
                            scale,
                            x.nb[1],
                            x.nb[2],
                            mask_nb1,
                            t.nb[1],
                            t.nb[2],
                            stream,
                        )
                    },
                    "softmax",
                )
            }
            KernelSel::FlashMma => {
                let q_id = self.src_id(&t, 0)?;
                let k_id = self.src_id(&t, 1)?;
                let v_id = self.src_id(&t, 2)?;
                let m_id = self.src_id(&t, 3)?;
                let q = &tensors[q_id];
                let k = &tensors[k_id];
                let v = &tensors[v_id];
                let mask = &tensors[m_id];
                let scale = t.op_param_f32(0);
                let nsm = i32::try_from(self.state.features.sm_count.max(1))
                    .map_err(|_| LlamaError::format("fattn nsm exceeds i32"))?;
                let cc = (self.state.features.compute_capability.0 as i32) * 100
                    + (self.state.features.compute_capability.1 as i32) * 10;
                let fixup_bytes = fattn_mma_fixup_bytes(nsm);
                let scratch = self
                    .state
                    .scratch_acts
                    .borrow_mut()
                    .ensure(fixup_bytes.max(1), "fattn mma fixup")?;
                check(
                    unsafe {
                        fattn_mma_f16(
                            self.ptr_of(q_id)?,
                            self.ptr_of(k_id)?,
                            self.ptr_of(v_id)?,
                            self.ptr_of(m_id)?,
                            self.ptr_of(node_id)?,
                            q.ne[0] as i32,
                            v.ne[0] as i32,
                            k.ne[1] as i32,
                            q.ne[1] as i32,
                            q.ne[2] as i32,
                            k.ne[2] as i32,
                            scale,
                            q.nb[1],
                            q.nb[2],
                            k.nb[1],
                            k.nb[2],
                            v.nb[1],
                            v.nb[2],
                            mask.nb[1],
                            t.nb[1],
                            t.nb[2],
                            nsm,
                            cc,
                            scratch as *mut f32,
                            stream,
                        )
                    },
                    "fattn_mma",
                )
            }
            KernelSel::FlashVec => {
                let q_id = self.src_id(&t, 0)?;
                let k_id = self.src_id(&t, 1)?;
                let v_id = self.src_id(&t, 2)?;
                let m_id = self.src_id(&t, 3)?;
                let q = &tensors[q_id];
                let k = &tensors[k_id];
                let v = &tensors[v_id];
                let mask = &tensors[m_id];
                let scale = t.op_param_f32(0);
                let nsm = i32::try_from(self.state.features.sm_count.max(1))
                    .map_err(|_| LlamaError::format("fattn vec nsm exceeds i32"))?;
                let tmp_bytes =
                    fattn_vec_tmp_bytes(q.ne[1] as i32, q.ne[2] as i32, k.ne[1] as i32);
                let scratch = self
                    .state
                    .scratch_acts
                    .borrow_mut()
                    .ensure(tmp_bytes.max(1), "fattn vec tmp")?;
                check(
                    unsafe {
                        fattn_vec_f16(
                            self.ptr_of(q_id)?,
                            self.ptr_of(k_id)?,
                            self.ptr_of(v_id)?,
                            self.ptr_of(m_id)?,
                            self.ptr_of(node_id)?,
                            q.ne[0] as i32,
                            v.ne[0] as i32,
                            k.ne[1] as i32,
                            q.ne[1] as i32,
                            q.ne[2] as i32,
                            k.ne[2] as i32,
                            scale,
                            q.nb[1],
                            q.nb[2],
                            k.nb[1],
                            k.nb[2],
                            v.nb[1],
                            v.nb[2],
                            mask.nb[1],
                            t.nb[1],
                            t.nb[2],
                            nsm,
                            scratch as *mut f32,
                            stream,
                        )
                    },
                    "fattn_vec",
                )
            }
            KernelSel::FlashDecode => {
                let q_id = self.src_id(&t, 0)?;
                let k_id = self.src_id(&t, 1)?;
                let v_id = self.src_id(&t, 2)?;
                let m_id = self.src_id(&t, 3)?;
                let q = &tensors[q_id];
                let k = &tensors[k_id];
                let v = &tensors[v_id];
                let mask = &tensors[m_id];
                let scale = t.op_param_f32(0);
                check(
                    unsafe {
                        flash_decode(
                            self.ptr_of(q_id)?,
                            self.ptr_of(k_id)?,
                            self.ptr_of(v_id)?,
                            self.ptr_of(m_id)?,
                            self.ptr_of(node_id)?,
                            q.ne[0] as i32,
                            v.ne[0] as i32,
                            k.ne[1] as i32,
                            q.ne[1] as i32,
                            q.ne[2] as i32,
                            k.ne[2] as i32,
                            scale,
                            q.nb[1],
                            q.nb[2],
                            k.nb[1],
                            k.nb[2],
                            v.nb[1],
                            v.nb[2],
                            mask.nb[1],
                            t.nb[1],
                            t.nb[2],
                            stream,
                        )
                    },
                    "flash_decode",
                )
            }
            KernelSel::Norm { l2 } => {
                let s_id = self.src_id(&t, 0)?;
                let s = &tensors[s_id];
                let eps = t.op_param_f32(0);
                check(
                    unsafe {
                        norm(
                            i32::from(l2),
                            self.ptr_of(s_id)?,
                            self.ptr_of(node_id)?,
                            s.ne[0] as i32,
                            s.ne[1] as i32,
                            s.ne[2] as i32,
                            s.ne[3] as i32,
                            eps,
                            s.nb[1],
                            s.nb[2],
                            s.nb[3],
                            t.nb[1],
                            t.nb[2],
                            t.nb[3],
                            stream,
                        )
                    },
                    "norm",
                )
            }
            KernelSel::NormMul { add } => {
                self.rms_norm_mul(&t, tensors, add)
            }
            KernelSel::RopeMulti => {
                let s_id = self.src_id(&t, 0)?;
                let pos_id = self.src_id(&t, 1)?;
                let s = &tensors[s_id];
                let n_dims = t.op_param_i32(1);
                let mode = t.op_param_i32(2);
                let n_ctx_orig = t.op_param_i32(4);
                let freq_base = t.op_param_f32(5);
                let freq_scale = t.op_param_f32(6);
                let ext_factor = t.op_param_f32(7);
                let attn_factor = t.op_param_f32(8);
                let beta_fast = t.op_param_f32(9);
                let beta_slow = t.op_param_f32(10);
                let sections = [
                    t.op_param_i32(11),
                    t.op_param_i32(12),
                    t.op_param_i32(13),
                    t.op_param_i32(14),
                ];
                let corr_factor = |n_rot: f32| -> f32 {
                    n_dims as f32
                        * ((n_ctx_orig as f32 / (n_rot * 2.0 * std::f32::consts::PI)).ln())
                        / (2.0 * freq_base.ln())
                };
                let corr0 = corr_factor(beta_fast).floor().max(0.0);
                let corr1 = corr_factor(beta_slow).ceil().min(n_dims as f32 - 1.0);
                check(
                    unsafe {
                        rope_multi(
                            self.ptr_of(s_id)?,
                            self.ptr_of(pos_id)? as *const i32,
                            self.ptr_of(node_id)?,
                            s.ne[0] as i32,
                            s.ne[1] as i32,
                            s.ne[2] as i32,
                            s.ne[3] as i32,
                            i32::from(mode == GGML_ROPE_TYPE_IMROPE),
                            n_dims,
                            sections[0],
                            sections[1],
                            sections[2],
                            sections[3],
                            freq_base,
                            freq_scale,
                            ext_factor,
                            attn_factor,
                            corr0,
                            corr1,
                            s.nb[0],
                            s.nb[1],
                            s.nb[2],
                            s.nb[3],
                            t.nb[0],
                            t.nb[1],
                            t.nb[2],
                            t.nb[3],
                            stream,
                        )
                    },
                    "rope",
                )
            }
            KernelSel::Unary(op) => {
                let s_id = self.src_id(&t, 0)?;
                check(
                    unsafe {
                        unary(
                            self.ptr_of(s_id)?,
                            self.ptr_of(node_id)?,
                            t.ne.iter().product::<i64>() as usize,
                            op,
                            stream,
                        )
                    },
                    "unary",
                )
            }
            KernelSel::UnaryMul(op) => {
                let unary_id = t
                    .src
                    .iter()
                    .flatten()
                    .copied()
                    .find(|&id| tensors[id].op == Op::Unary)
                    .ok_or_else(|| LlamaError::format("fused unary_mul missing unary"))?;
                let other_id = t
                    .src
                    .iter()
                    .flatten()
                    .copied()
                    .find(|&id| id != unary_id)
                    .ok_or_else(|| LlamaError::format("fused unary_mul missing other"))?;
                let unary = &tensors[unary_id];
                let unary_src_id = unary.src[0].ok_or_else(|| {
                    LlamaError::format("fused unary_mul missing unary src")
                })?;
                let unary_src = &tensors[unary_src_id];
                let other = &tensors[other_id];
                let n = unary_src.ne[0] as i32;
                let k = t.ne.iter().product::<i64>() as usize;
                let o0 = unary_src.nb[1] / 4;
                let o1 = other.nb[1] / 4;
                check(
                    unsafe {
                        unary_mul(
                            self.ptr_of(unary_src_id)?,
                            self.ptr_of(other_id)?,
                            self.ptr_of(node_id)?,
                            k,
                            n,
                            op,
                            o0,
                            o1,
                            stream,
                        )
                    },
                    "unary_mul",
                )
            }
            KernelSel::Glu(op) => {
                let a_id = self.src_id(&t, 0)?;
                let b_id = self.src_id(&t, 1)?;
                check(
                    unsafe {
                        glu(
                            self.ptr_of(a_id)?,
                            self.ptr_of(b_id)?,
                            self.ptr_of(node_id)?,
                            t.ne.iter().product::<i64>() as usize,
                            op,
                            stream,
                        )
                    },
                    "glu",
                )
            }
            KernelSel::Binary(op) => {
                let a_id = self.src_id(&t, 0)?;
                let b_id = self.src_id(&t, 1)?;
                let a = &tensors[a_id];
                let b = &tensors[b_id];
                check(
                    unsafe {
                        binary(
                            op,
                            self.ptr_of(a_id)?,
                            self.ptr_of(b_id)?,
                            self.ptr_of(node_id)?,
                            t.ne[0] as i32,
                            t.ne[1] as i32,
                            t.ne[2] as i32,
                            t.ne[3] as i32,
                            b.ne[0] as i32,
                            b.ne[1] as i32,
                            b.ne[2] as i32,
                            b.ne[3] as i32,
                            a.nb[0],
                            a.nb[1],
                            a.nb[2],
                            a.nb[3],
                            b.nb[0],
                            b.nb[1],
                            b.nb[2],
                            b.nb[3],
                            t.nb[0],
                            t.nb[1],
                            t.nb[2],
                            t.nb[3],
                            stream,
                        )
                    },
                    "binary",
                )
            }
            KernelSel::MmvqFusedSwiglu { kind } => {
                self.mmv_quant_q81_swiglu(&t, kind, quant_profile.as_deref_mut())
            }
            KernelSel::MmvQuant(kind) => {
                let a_id = self.src_id(&t, 0)?;
                let b_id = self.src_id(&t, 1)?;
                let a = &tensors[a_id];
                let b = &tensors[b_id];
                let k = a.ne[0] as usize;
                let n = a.ne[1] as usize;
                let m = (b.ne[1] * b.ne[2] * b.ne[3]) as usize;
                let row_bytes =
                    ggml_row_size_for_type(a.desc.ty, a.ne[0]).map_err(LlamaError::format)?;
                let cc = self.state.features.compute_capability;
                // Faithful llama.cpp Ada MMVQ: block_q8_1 half2(d,sum(xi)),
                // 4 warps/row, integer vecdots. Test-only until 32/128-token
                // parity holds. The previous f32-scale/2-warp path is gone.
                let q81_disabled = std::env::var_os("MKLLM_DISABLE_Q81_MMVQ")
                    .map(|v| v == "1")
                    .unwrap_or(false);
                let q81_ok = !q81_disabled
                    && m == 1
                    && (k % 256) == 0
                    && a.ne[2] == 1
                    && a.ne[3] == 1
                    && b.ne[2] == 1
                    && b.ne[3] == 1
                    && b.nb[0] == 4
                    && (kind == MKLLM_QUANT_Q4K
                        || kind == MKLLM_QUANT_Q5K
                        || kind == MKLLM_QUANT_Q6K)
                    && (cc.0 > 6 || (cc.0 == 6 && cc.1 >= 1));
                if q81_ok {
                    self.mmv_quant_q81(&t, kind, k, n, row_bytes, quant_profile.as_deref_mut())
                } else {
                    check(
                        unsafe {
                            mmv_quant(
                                kind,
                                self.ptr_of(a_id)?,
                                self.ptr_of(b_id)? as *const f32,
                                self.ptr_of(node_id)? as *mut f32,
                                k as i32,
                                n as i32,
                                m as i32,
                                row_bytes,
                                b.nb[1] / 4,
                                t.nb[1] / 4,
                                stream,
                            )
                        },
                        "mmv_quant",
                    )
                }
            }
            KernelSel::MmvF32 => {
                let a_id = self.src_id(&t, 0)?;
                let b_id = self.src_id(&t, 1)?;
                let a = &tensors[a_id];
                let b = &tensors[b_id];
                check(
                    unsafe {
                        mmv_f32(
                            self.ptr_of(a_id)? as *const f32,
                            self.ptr_of(b_id)? as *const f32,
                            self.ptr_of(node_id)? as *mut f32,
                            a.ne[0] as i32,
                            a.ne[1] as i32,
                            b.ne[1] as i32,
                            a.nb[1] / 4,
                            b.nb[1] / 4,
                            t.nb[1] / 4,
                            stream,
                        )
                    },
                    "mmv_f32",
                )
            }
            KernelSel::MulMatBatched { a_f16 } => {
                let a_id = self.src_id(&t, 0)?;
                let b_id = self.src_id(&t, 1)?;
                let a = &tensors[a_id];
                let b = &tensors[b_id];
                check(
                    unsafe {
                        mul_mat_batched(
                            i32::from(a_f16),
                            self.ptr_of(a_id)?,
                            self.ptr_of(b_id)?,
                            self.ptr_of(node_id)?,
                            a.ne[0] as i32,
                            a.ne[1] as i32,
                            b.ne[1] as i32,
                            b.ne[2] as i32,
                            b.ne[3] as i32,
                            a.ne[2] as i32,
                            a.ne[3] as i32,
                            a.nb[0],
                            a.nb[1],
                            a.nb[2],
                            a.nb[3],
                            b.nb[0],
                            b.nb[1],
                            b.nb[2],
                            b.nb[3],
                            t.nb[0],
                            t.nb[1],
                            t.nb[2],
                            t.nb[3],
                            stream,
                        )
                    },
                    "mul_mat_batched",
                )
            }
            KernelSel::GemmF32 => {
                let a_id = self.src_id(&t, 0)?;
                let b_id = self.src_id(&t, 1)?;
                let a = &tensors[a_id];
                let b = &tensors[b_id];
                if !a.is_contiguous() || !b.is_contiguous() || !t.is_contiguous() {
                    return Err(LlamaError::unsupported("strided f32 GEMM"));
                }
                let (m, n, k) = (a.ne[1] as i32, b.ne[1] as i32, a.ne[0] as i32);
                let alpha = 1.0f32;
                let beta = 0.0f32;
                let status = unsafe {
                    cublasSgemm_v2(
                        self.state.blas,
                        CUBLAS_OP_T,
                        CUBLAS_OP_N,
                        m,
                        n,
                        k,
                        &alpha,
                        self.ptr_of(a_id)? as *const f32,
                        k,
                        self.ptr_of(b_id)? as *const f32,
                        k,
                        &beta,
                        self.ptr_of(node_id)? as *mut f32,
                        m,
                    )
                };
                if status != CUBLAS_STATUS_SUCCESS {
                    return Err(LlamaError::format(format!("cublas sgemm failed: {status}")));
                }
                Ok(())
            }
            KernelSel::GemmQuant(kind) => {
                self.gemm_quant(&t, kind, quant_profile.as_deref_mut())
            }
            KernelSel::SsmConv | KernelSel::SsmConvSilu => {
                self.ssm_conv(&t, tensors, matches!(kernel, KernelSel::SsmConvSilu))
            }
            KernelSel::GatedDeltaNet => {
                let q_id = self.src_id(&t, 0)?;
                let k_id = self.src_id(&t, 1)?;
                let v_id = self.src_id(&t, 2)?;
                let g_id = self.src_id(&t, 3)?;
                let beta_id = self.src_id(&t, 4)?;
                let state_id = self.src_id(&t, 5)?;
                let q = &tensors[q_id];
                let k = &tensors[k_id];
                let v = &tensors[v_id];
                let g = &tensors[g_id];
                let sv = v.ne[0];
                if sv % 32 != 0 || sv > 256 {
                    return Err(LlamaError::unsupported(format!(
                        "gated_delta_net value dim {sv} is unsupported (needs 32..=256, /32)"
                    )));
                }
                check(
                    unsafe {
                        gated_delta_net(
                            self.ptr_of(q_id)?,
                            self.ptr_of(k_id)?,
                            self.ptr_of(v_id)?,
                            self.ptr_of(g_id)?,
                            self.ptr_of(beta_id)?,
                            self.ptr_of(state_id)?,
                            self.ptr_of(node_id)?,
                            sv as i32,
                            v.ne[1] as i32,
                            v.ne[2] as i32,
                            v.ne[3] as i32,
                            g.ne[0] as i32,
                            q.ne[1] as i32,
                            k.ne[1] as i32,
                            q.nb[1] / 4,
                            q.nb[2] / 4,
                            q.nb[3] / 4,
                            k.nb[1] / 4,
                            k.nb[2] / 4,
                            k.nb[3] / 4,
                            v.nb[1] / 4,
                            v.nb[2] / 4,
                            v.nb[3] / 4,
                            stream,
                        )
                    },
                    "gated_delta_net",
                )
            }
        }
    }

    fn rms_norm_mul(&self, t: &Tensor, tensors: &[Tensor], add: bool) -> Result<()> {
        let stream = self.state.stream;
        let (rms, mul_src, add_src) = if add {
            let mul_id = t
                .src
                .iter()
                .flatten()
                .copied()
                .find(|&id| tensors[id].op == Op::Mul)
                .ok_or_else(|| LlamaError::format("fused rms_norm_mul_add missing mul"))?;
            let add_other = t
                .src
                .iter()
                .flatten()
                .copied()
                .find(|&id| id != mul_id)
                .ok_or_else(|| LlamaError::format("fused rms_norm_mul_add missing add src"))?;
            let mul = &tensors[mul_id];
            let rms_id = mul
                .src
                .iter()
                .flatten()
                .copied()
                .find(|&id| tensors[id].op == Op::RmsNorm)
                .ok_or_else(|| LlamaError::format("fused rms_norm_mul_add missing rms"))?;
            let mul_other = mul
                .src
                .iter()
                .flatten()
                .copied()
                .find(|&id| id != rms_id)
                .ok_or_else(|| LlamaError::format("fused rms_norm_mul_add missing scale"))?;
            (&tensors[rms_id], &tensors[mul_other], Some(&tensors[add_other]))
        } else {
            let rms_id = t
                .src
                .iter()
                .flatten()
                .copied()
                .find(|&id| tensors[id].op == Op::RmsNorm)
                .ok_or_else(|| LlamaError::format("fused rms_norm_mul missing rms"))?;
            let mul_other = t
                .src
                .iter()
                .flatten()
                .copied()
                .find(|&id| id != rms_id)
                .ok_or_else(|| LlamaError::format("fused rms_norm_mul missing scale"))?;
            (&tensors[rms_id], &tensors[mul_other], None)
        };
        let x_id = rms.src[0].ok_or_else(|| LlamaError::format("fused rms missing src"))?;
        let x = &tensors[x_id];
        let eps = rms.op_param_f32(0);
        let (add_ptr, add_nb, add_ne) = match add_src {
            Some(add_t) => (
                self.ptr_of(add_t.id)?,
                [add_t.nb[1], add_t.nb[2], add_t.nb[3]],
                [add_t.ne[0] as i32, add_t.ne[1] as i32, add_t.ne[2] as i32, add_t.ne[3] as i32],
            ),
            None => (std::ptr::null_mut(), [0, 0, 0], [0, 0, 0, 0]),
        };
        check(
            unsafe {
                rms_norm_mul(
                    self.ptr_of(x_id)?,
                    self.ptr_of(mul_src.id)?,
                    add_ptr,
                    self.ptr_of(t.id)?,
                    x.ne[0] as i32,
                    x.ne[1] as i32,
                    x.ne[2] as i32,
                    x.ne[3] as i32,
                    eps,
                    x.nb[1],
                    x.nb[2],
                    x.nb[3],
                    t.nb[1],
                    t.nb[2],
                    t.nb[3],
                    mul_src.nb[1],
                    mul_src.nb[2],
                    mul_src.nb[3],
                    mul_src.ne[0] as i32,
                    mul_src.ne[1] as i32,
                    mul_src.ne[2] as i32,
                    mul_src.ne[3] as i32,
                    add_nb[0],
                    add_nb[1],
                    add_nb[2],
                    add_ne[0],
                    add_ne[1],
                    add_ne[2],
                    add_ne[3],
                    stream,
                )
            },
            if add { "rms_norm_mul_add" } else { "rms_norm_mul" },
        )
    }

    fn ssm_conv(&self, t: &Tensor, tensors: &[Tensor], silu: bool) -> Result<()> {
        let stream = self.state.stream;
        let conv = if silu {
            let conv_id = self.src_id(t, 0)?;
            &tensors[conv_id]
        } else {
            t
        };
        let s_id = conv.src[0].ok_or_else(|| LlamaError::format("ssm_conv missing src0"))?;
        let w_id = conv.src[1].ok_or_else(|| LlamaError::format("ssm_conv missing src1"))?;
        let s = &tensors[s_id];
        let w = &tensors[w_id];
        check(
            unsafe {
                ssm_conv(
                    self.ptr_of(s_id)?,
                    self.ptr_of(w_id)?,
                    self.ptr_of(t.id)?,
                    w.ne[0] as i32,
                    w.ne[1] as i32,
                    t.ne[1] as i32,
                    t.ne[2] as i32,
                    i32::from(silu),
                    s.nb[0],
                    s.nb[1],
                    s.nb[2],
                    w.nb[1],
                    t.nb[0],
                    t.nb[1],
                    t.nb[2],
                    stream,
                )
            },
            if silu { "ssm_conv_silu" } else { "ssm_conv" },
        )
    }

    fn mmv_quant_q81_swiglu(
        &self,
        t: &Tensor,
        kind: i32,
        mut profile: Option<&mut QuantTimeline>,
    ) -> Result<()> {
        let tensors = self.ctx.tensors();
        let gate_mm_id = self.src_id(t, 0)?;
        let up_mm_id = self.src_id(t, 1)?;
        let gate_mm = &tensors[gate_mm_id];
        let up_mm = &tensors[up_mm_id];
        let gate_w = gate_mm.src[0].ok_or_else(|| {
            LlamaError::format("fused mmvq missing gate weight")
        })?;
        let up_w = up_mm.src[0].ok_or_else(|| {
            LlamaError::format("fused mmvq missing up weight")
        })?;
        let act_id = up_mm.src[1].ok_or_else(|| {
            LlamaError::format("fused mmvq missing activations")
        })?;
        let gate_w_t = &tensors[gate_w];
        let up_w_t = &tensors[up_w];
        let k = up_w_t.ne[0] as usize;
        let n = up_w_t.ne[1] as usize;
        let up_row_bytes =
            ggml_row_size_for_type(up_w_t.desc.ty, up_w_t.ne[0]).map_err(LlamaError::format)?;
        let gate_row_bytes =
            ggml_row_size_for_type(gate_w_t.desc.ty, gate_w_t.ne[0]).map_err(LlamaError::format)?;
        let stream = self.state.stream;
        let nblk = k / 32;
        let y_bytes = nblk
            .checked_mul(36)
            .ok_or_else(|| LlamaError::format("q81 fused mmvq scratch overflow"))?;
        let scratch = self
            .state
            .scratch_acts
            .borrow_mut()
            .ensure(y_bytes, "q81 fused activation scratch")?;
        let src1 = self.ptr_of(act_id)? as *const f32;
        let reuse_q81 =
            src1 == self.state.last_q81_src.get() && k == self.state.last_q81_k.get();
        let mut stage_start = profile.as_mut().map(|timeline| timeline.mark()).transpose()?;
        if !reuse_q81 {
            check(
                unsafe { quantize_q81(src1, scratch, k as i32, stream) },
                "q81 fused quantize",
            )?;
            self.state.last_q81_src.set(src1);
            self.state.last_q81_k.set(k);
            if let (Some(timeline), Some(start)) = (profile.as_mut(), stage_start) {
                stage_start = Some(timeline.finish(start, kind, "q81_quant")?);
            }
        }
        check(
            unsafe {
                mmv_quant_q81_swiglu(
                    kind,
                    self.ptr_of(up_w)?,
                    self.ptr_of(gate_w)?,
                    scratch,
                    self.ptr_of(t.id)? as *mut f32,
                    k as i32,
                    n as i32,
                    up_row_bytes,
                    gate_row_bytes,
                    stream,
                )
            },
            "mmv_q81_swiglu",
        )?;
        if let (Some(timeline), Some(start)) = (profile.as_mut(), stage_start) {
            timeline.finish(start, kind, "mmv_q81_swiglu")?;
        }
        Ok(())
    }

    fn mmv_quant_q81(
        &self,
        t: &Tensor,
        kind: i32,
        k: usize,
        n: usize,
        row_bytes: usize,
        mut profile: Option<&mut QuantTimeline>,
    ) -> Result<()> {
        let a_id = self.src_id(t, 0)?;
        let b_id = self.src_id(t, 1)?;
        let stream = self.state.stream;
        let nblk = k / 32;
        let y_bytes = nblk
            .checked_mul(36)
            .ok_or_else(|| LlamaError::format("q81 mmvq scratch overflow"))?;
        let scratch = self
            .state
            .scratch_acts
            .borrow_mut()
            .ensure(y_bytes, "q81 activation scratch")?;
        let src1 = self.ptr_of(b_id)? as *const f32;
        let reuse_q81 =
            src1 == self.state.last_q81_src.get() && k == self.state.last_q81_k.get();
        let mut stage_start = profile.as_mut().map(|timeline| timeline.mark()).transpose()?;
        if !reuse_q81 {
            check(
                unsafe { quantize_q81(src1, scratch, k as i32, stream) },
                "q81 quantize",
            )?;
            self.state.last_q81_src.set(src1);
            self.state.last_q81_k.set(k);
            if let (Some(timeline), Some(start)) = (profile.as_mut(), stage_start) {
                stage_start = Some(timeline.finish(start, kind, "q81_quant")?);
            }
        }
        check(
            unsafe {
                mmv_quant_q81(
                    kind,
                    self.ptr_of(a_id)?,
                    scratch,
                    self.ptr_of(t.id)? as *mut f32,
                    k as i32,
                    n as i32,
                    row_bytes,
                    t.nb[1] / 4,
                    stream,
                )
            },
            "mmv_q81",
        )?;
        if let (Some(timeline), Some(start)) = (profile.as_mut(), stage_start) {
            timeline.finish(start, kind, "mmv_q81")?;
        }
        Ok(())
    }

    fn gemm_quant_q81(
        &self,
        t: &Tensor,
        kind: i32,
        k: usize,
        n: usize,
        m: usize,
        row_bytes: usize,
        mut profile: Option<&mut QuantTimeline>,
    ) -> Result<()> {
        let tensors = self.ctx.tensors();
        let a_id = self.src_id(t, 0)?;
        let b_id = self.src_id(t, 1)?;
        let b = &tensors[b_id];
        let stream = self.state.stream;
        let nblk = k / 32;
        let q_bytes = m * k;
        let d_off = (q_bytes + 15) & !15;
        let scratch_bytes = d_off + m * nblk * 4;
        let scratch = self
            .state
            .scratch_acts
            .borrow_mut()
            .ensure(scratch_bytes, "q81 mmq activation scratch")?;
        let q_ptr = scratch as *mut i8;
        let d_ptr = unsafe { (scratch as *mut u8).add(d_off) as *mut f32 };
        let mut stage_start = profile.as_mut().map(|timeline| timeline.mark()).transpose()?;
        check(
            unsafe {
                quantize_q81_batched(
                    self.ptr_of(b_id)? as *const f32,
                    q_ptr,
                    d_ptr,
                    k as i32,
                    m as i32,
                    b.nb[1] / 4,
                    stream,
                )
            },
            "q81 mmq quantize",
        )?;
        if let (Some(timeline), Some(start)) = (profile.as_mut(), stage_start) {
            stage_start = Some(timeline.finish(start, kind, "q81_quant")?);
        }
        check(
            unsafe {
                mmq_quant_q81(
                    kind,
                    self.ptr_of(a_id)?,
                    q_ptr,
                    d_ptr,
                    self.ptr_of(t.id)? as *mut f32,
                    k as i32,
                    n as i32,
                    m as i32,
                    row_bytes,
                    t.nb[1] / 4,
                    stream,
                )
            },
            "mmq_q81",
        )?;
        if let (Some(timeline), Some(start)) = (profile.as_mut(), stage_start) {
            timeline.finish(start, kind, "mmq_q81")?;
        }
        Ok(())
    }

    fn gemm_quant_q4k_j128(
        &self,
        t: &Tensor,
        kind: i32,
        k: usize,
        n: usize,
        m: usize,
        stride_row_x: usize,
        act_stride_elems: usize,
        mut profile: Option<&mut QuantTimeline>,
    ) -> Result<()> {
        let a_id = self.src_id(t, 0)?;
        let b_id = self.src_id(t, 1)?;
        let stream = self.state.stream;
        let n_q8_blocks = k / 128;
        let stream_k = std::env::var_os("MKLLM_DISABLE_STREAM_K")
            .map(|v| v != "1")
            .unwrap_or(true);
        let nsm = if stream_k {
            i32::try_from(self.state.features.sm_count.max(1))
                .map_err(|_| LlamaError::format("q4k mmq nsm exceeds i32"))?
        } else {
            0
        };
        let fixup_elems = if nsm > 0 {
            (nsm as usize)
                .checked_mul(128 * 128)
                .ok_or_else(|| LlamaError::format("q4k mmq fixup overflow"))?
        } else {
            0
        };
        let y_bytes = m
            .checked_mul(n_q8_blocks)
            .and_then(|bytes| bytes.checked_mul(144))
            .ok_or_else(|| LlamaError::format("q4k mmq ds4 scratch overflow"))?;
        let scratch_bytes = y_bytes
            .checked_add(fixup_elems.saturating_mul(4))
            .ok_or_else(|| LlamaError::format("q4k mmq scratch overflow"))?;
        let k_i = i32::try_from(k).map_err(|_| LlamaError::format("q4k mmq k exceeds i32"))?;
        let n_i = i32::try_from(n).map_err(|_| LlamaError::format("q4k mmq n exceeds i32"))?;
        let m_i = i32::try_from(m).map_err(|_| LlamaError::format("q4k mmq m exceeds i32"))?;
        let stride_col = i32::try_from(act_stride_elems)
            .map_err(|_| LlamaError::format("q4k mmq act stride exceeds i32"))?;
        let stride_row_x_i = i32::try_from(stride_row_x)
            .map_err(|_| LlamaError::format("q4k mmq weight stride exceeds i32"))?;
        let stride_col_dst = i32::try_from(t.nb[1] / 4)
            .map_err(|_| LlamaError::format("q4k mmq dst stride exceeds i32"))?;
        let scratch = self
            .state
            .scratch_acts
            .borrow_mut()
            .ensure(scratch_bytes, "q4k mmq ds4 scratch")?;
        let tmp_fixup = if nsm > 0 {
            unsafe { (scratch as *mut u8).add(y_bytes) as *mut f32 }
        } else {
            std::ptr::null_mut()
        };
        let mut stage_start = profile.as_mut().map(|timeline| timeline.mark()).transpose()?;
        check(
            unsafe {
                quantize_mmq_ds4(
                    self.ptr_of(b_id)? as *const f32,
                    scratch,
                    k_i,
                    m_i,
                    stride_col,
                    stream,
                )
            },
            "q4k mmq ds4",
        )?;
        if let (Some(timeline), Some(start)) = (profile.as_mut(), stage_start) {
            stage_start = Some(timeline.finish(start, kind, "q4k_ds4")?);
        }
        check(
            unsafe {
                mmq_q4k(
                    self.ptr_of(a_id)?,
                    scratch,
                    self.ptr_of(t.id)? as *mut f32,
                    k_i,
                    n_i,
                    m_i,
                    stride_row_x_i,
                    stride_col_dst,
                    nsm,
                    tmp_fixup,
                    stream,
                )
            },
            "mmq_q4k_j128",
        )?;
        if let (Some(timeline), Some(start)) = (profile.as_mut(), stage_start) {
            timeline.finish(start, kind, "mmq_q4k_j128")?;
        }
        Ok(())
    }

    fn gemm_quant_q5k_j128(
        &self,
        t: &Tensor,
        kind: i32,
        k: usize,
        n: usize,
        m: usize,
        stride_row_x: usize,
        act_stride_elems: usize,
        mut profile: Option<&mut QuantTimeline>,
    ) -> Result<()> {
        let a_id = self.src_id(t, 0)?;
        let b_id = self.src_id(t, 1)?;
        let stream = self.state.stream;
        let n_q8_blocks = k / 128;
        let stream_k = std::env::var_os("MKLLM_DISABLE_STREAM_K")
            .map(|v| v != "1")
            .unwrap_or(true);
        let nsm = if stream_k {
            i32::try_from(self.state.features.sm_count.max(1))
                .map_err(|_| LlamaError::format("q5k mmq nsm exceeds i32"))?
        } else {
            0
        };
        let fixup_elems = if nsm > 0 {
            (nsm as usize)
                .checked_mul(128 * 128)
                .ok_or_else(|| LlamaError::format("q5k mmq fixup overflow"))?
        } else {
            0
        };
        let y_bytes = m
            .checked_mul(n_q8_blocks)
            .and_then(|bytes| bytes.checked_mul(144))
            .ok_or_else(|| LlamaError::format("q5k mmq ds4 scratch overflow"))?;
        let scratch_bytes = y_bytes
            .checked_add(fixup_elems.saturating_mul(4))
            .ok_or_else(|| LlamaError::format("q5k mmq scratch overflow"))?;
        let k_i = i32::try_from(k).map_err(|_| LlamaError::format("q5k mmq k exceeds i32"))?;
        let n_i = i32::try_from(n).map_err(|_| LlamaError::format("q5k mmq n exceeds i32"))?;
        let m_i = i32::try_from(m).map_err(|_| LlamaError::format("q5k mmq m exceeds i32"))?;
        let stride_col = i32::try_from(act_stride_elems)
            .map_err(|_| LlamaError::format("q5k mmq act stride exceeds i32"))?;
        let stride_row_x_i = i32::try_from(stride_row_x)
            .map_err(|_| LlamaError::format("q5k mmq weight stride exceeds i32"))?;
        let stride_col_dst = i32::try_from(t.nb[1] / 4)
            .map_err(|_| LlamaError::format("q5k mmq dst stride exceeds i32"))?;
        let scratch = self
            .state
            .scratch_acts
            .borrow_mut()
            .ensure(scratch_bytes, "q5k mmq ds4 scratch")?;
        let tmp_fixup = if nsm > 0 {
            unsafe { (scratch as *mut u8).add(y_bytes) as *mut f32 }
        } else {
            std::ptr::null_mut()
        };
        let mut stage_start = profile.as_mut().map(|timeline| timeline.mark()).transpose()?;
        check(
            unsafe {
                quantize_mmq_ds4(
                    self.ptr_of(b_id)? as *const f32,
                    scratch,
                    k_i,
                    m_i,
                    stride_col,
                    stream,
                )
            },
            "q5k mmq ds4",
        )?;
        if let (Some(timeline), Some(start)) = (profile.as_mut(), stage_start) {
            stage_start = Some(timeline.finish(start, kind, "q5k_ds4")?);
        }
        check(
            unsafe {
                mmq_q5k(
                    self.ptr_of(a_id)?,
                    scratch,
                    self.ptr_of(t.id)? as *mut f32,
                    k_i,
                    n_i,
                    m_i,
                    stride_row_x_i,
                    stride_col_dst,
                    nsm,
                    tmp_fixup,
                    stream,
                )
            },
            "mmq_q5k_j128",
        )?;
        if let (Some(timeline), Some(start)) = (profile.as_mut(), stage_start) {
            timeline.finish(start, kind, "mmq_q5k_j128")?;
        }
        Ok(())
    }

    fn gemm_quant_q6k_j128(
        &self,
        t: &Tensor,
        kind: i32,
        k: usize,
        n: usize,
        m: usize,
        stride_row_x: usize,
        act_stride_elems: usize,
        mut profile: Option<&mut QuantTimeline>,
    ) -> Result<()> {
        let a_id = self.src_id(t, 0)?;
        let b_id = self.src_id(t, 1)?;
        let stream = self.state.stream;
        let n_q8_blocks = k / 128;
        let stream_k = std::env::var_os("MKLLM_DISABLE_STREAM_K")
            .map(|v| v != "1")
            .unwrap_or(true);
        let nsm = if stream_k {
            i32::try_from(self.state.features.sm_count.max(1))
                .map_err(|_| LlamaError::format("q6k mmq nsm exceeds i32"))?
        } else {
            0
        };
        let fixup_elems = if nsm > 0 {
            (nsm as usize)
                .checked_mul(128 * 128)
                .ok_or_else(|| LlamaError::format("q6k mmq fixup overflow"))?
        } else {
            0
        };
        let y_bytes = m
            .checked_mul(n_q8_blocks)
            .and_then(|bytes| bytes.checked_mul(144))
            .ok_or_else(|| LlamaError::format("q6k mmq d4 scratch overflow"))?;
        let scratch_bytes = y_bytes
            .checked_add(fixup_elems.saturating_mul(4))
            .ok_or_else(|| LlamaError::format("q6k mmq scratch overflow"))?;
        let k_i = i32::try_from(k).map_err(|_| LlamaError::format("q6k mmq k exceeds i32"))?;
        let n_i = i32::try_from(n).map_err(|_| LlamaError::format("q6k mmq n exceeds i32"))?;
        let m_i = i32::try_from(m).map_err(|_| LlamaError::format("q6k mmq m exceeds i32"))?;
        let stride_col = i32::try_from(act_stride_elems)
            .map_err(|_| LlamaError::format("q6k mmq act stride exceeds i32"))?;
        let stride_row_x_i = i32::try_from(stride_row_x)
            .map_err(|_| LlamaError::format("q6k mmq weight stride exceeds i32"))?;
        let stride_col_dst = i32::try_from(t.nb[1] / 4)
            .map_err(|_| LlamaError::format("q6k mmq dst stride exceeds i32"))?;
        let scratch = self
            .state
            .scratch_acts
            .borrow_mut()
            .ensure(scratch_bytes, "q6k mmq d4 scratch")?;
        let tmp_fixup = if nsm > 0 {
            unsafe { (scratch as *mut u8).add(y_bytes) as *mut f32 }
        } else {
            std::ptr::null_mut()
        };
        let mut stage_start = profile.as_mut().map(|timeline| timeline.mark()).transpose()?;
        check(
            unsafe {
                quantize_mmq_d4(
                    self.ptr_of(b_id)? as *const f32,
                    scratch,
                    k_i,
                    m_i,
                    stride_col,
                    stream,
                )
            },
            "q6k mmq d4",
        )?;
        if let (Some(timeline), Some(start)) = (profile.as_mut(), stage_start) {
            stage_start = Some(timeline.finish(start, kind, "q6k_d4")?);
        }
        check(
            unsafe {
                mmq_q6k(
                    self.ptr_of(a_id)?,
                    scratch,
                    self.ptr_of(t.id)? as *mut f32,
                    k_i,
                    n_i,
                    m_i,
                    stride_row_x_i,
                    stride_col_dst,
                    nsm,
                    tmp_fixup,
                    stream,
                )
            },
            "mmq_q6k_j128",
        )?;
        if let (Some(timeline), Some(start)) = (profile.as_mut(), stage_start) {
            timeline.finish(start, kind, "mmq_q6k_j128")?;
        }
        Ok(())
    }

    fn gemm_quant(
        &self,
        t: &Tensor,
        kind: i32,
        mut profile: Option<&mut QuantTimeline>,
    ) -> Result<()> {
        let tensors = self.ctx.tensors();
        let a_id = self.src_id(t, 0)?;
        let b_id = self.src_id(t, 1)?;
        let a = &tensors[a_id];
        let b = &tensors[b_id];
        if !t.is_contiguous() {
            return Err(LlamaError::unsupported("strided quantized GEMM output"));
        }
        if b.nb[0] != 4 {
            return Err(LlamaError::unsupported("non-K-contiguous GEMM activations"));
        }
        let k = a.ne[0] as usize;
        let n = a.ne[1] as usize;
        let m = b.ne[1] as usize;
        let row_bytes = ggml_row_size_for_type(a.desc.ty, a.ne[0]).map_err(LlamaError::format)?;
        let stream = self.state.stream;
        let mut stage_start = profile.as_mut().map(|timeline| timeline.mark()).transpose()?;

        // Packed Q8_1 MMQ is compiled but disabled: p512 prefill was 84.6
        // vs 865.7 slab+cublas and the 128-token greedy stream changed.
        let mmq_s8_ok = false
            && (k % 256) == 0
            && m >= 64
            && a.ne[2] == 1
            && a.ne[3] == 1
            && b.ne[2] == 1
            && b.ne[3] == 1
            && (kind == MKLLM_QUANT_Q4K
                || kind == MKLLM_QUANT_Q5K
                || kind == MKLLM_QUANT_Q6K
                || kind == MKLLM_QUANT_Q80);
        if mmq_s8_ok {
            return self.gemm_quant_q81(t, kind, k, n, m, row_bytes, profile);
        }

        // The lossless packed-dequant + BF16 WMMA experiment is compiled but
        // disabled: p512 prefill measured 84.6 tok/s versus ~865.7 for the
        // accepted slab+cuBLAS path. Keep the kernel for the next optimization
        // pass without routing production work through the rejected A/B.
        let fused_bf16_ok = false
            && (k % 256) == 0
            && m >= 32
            && a.ne[2] == 1
            && a.ne[3] == 1
            && b.ne[2] == 1
            && b.ne[3] == 1
            && (kind == MKLLM_QUANT_Q4K
                || kind == MKLLM_QUANT_Q5K
                || kind == MKLLM_QUANT_Q6K
                || kind == MKLLM_QUANT_Q80);
        if fused_bf16_ok {
            check(
                unsafe {
                    mmq_quant(
                        kind,
                        self.ptr_of(a_id)?,
                        self.ptr_of(b_id)? as *const f32,
                        self.ptr_of(t.id)? as *mut f32,
                        k as i32,
                        n as i32,
                        m as i32,
                        row_bytes,
                        b.nb[1] / 4,
                        t.nb[1] / 4,
                        stream,
                    )
                },
                "mmq_bf16",
            )?;
            if let (Some(timeline), Some(start)) = (profile.as_mut(), stage_start) {
                timeline.finish(start, kind, "mmq_bf16")?;
            }
            return Ok(());
        }

        // llama.cpp Q4_K J=128 MMA. Default-on for full J tiles (M>=128,
        // M%128==0). Does not touch the shared ggml CUDA backend used by
        // diffusion / other models. MKLLM_DISABLE_Q4K_MMQ=1 restores slab.
        let cc = self.state.features.compute_capability;
        let act_stride = b.nb[1] / 4;
        let q4k_mmq_disabled = std::env::var_os("MKLLM_DISABLE_Q4K_MMQ")
            .map(|v| v == "1")
            .unwrap_or(false);
        let q4_row_blocks = k / 256;
        let q4_canonical = a.nb[0] == 144
            && a.nb[1] == row_bytes
            && row_bytes % 144 == 0
            && row_bytes / 144 >= q4_row_blocks;
        let q4_validated_stride =
            a.nb[0] == 144 && a.nb[1] % 144 == 0 && a.nb[1] / 144 >= q4_row_blocks;
        let q4k_mmq_ok = !q4k_mmq_disabled
            && kind == MKLLM_QUANT_Q4K
            && cc.0 >= 8
            && (k % 256) == 0
            && m >= 128
            && (m % 128) == 0
            && a.ne[2] == 1
            && a.ne[3] == 1
            && b.ne[2] == 1
            && b.ne[3] == 1
            && b.nb[0] == 4
            && act_stride == k
            && (q4_canonical || q4_validated_stride)
            && t.nb[1] / 4 >= n;
        if q4k_mmq_ok {
            return self.gemm_quant_q4k_j128(
                t,
                kind,
                k,
                n,
                m,
                a.nb[1] / 144,
                act_stride,
                profile,
            );
        }

        // llama.cpp Q5_K J=128: load_tiles_q5_K + the same q8_1 MMA as Q4_K.
        // MKLLM_DISABLE_Q5K_MMQ=1 restores slab.
        let q5k_mmq_disabled = std::env::var_os("MKLLM_DISABLE_Q5K_MMQ")
            .map(|v| v == "1")
            .unwrap_or(false);
        let q5_row_blocks = k / 256;
        let q5_canonical = a.nb[0] == 176
            && a.nb[1] == row_bytes
            && row_bytes % 176 == 0
            && row_bytes / 176 >= q5_row_blocks;
        let q5_validated_stride =
            a.nb[0] == 176 && a.nb[1] % 176 == 0 && a.nb[1] / 176 >= q5_row_blocks;
        let q5k_mmq_ok = !q5k_mmq_disabled
            && kind == MKLLM_QUANT_Q5K
            && cc.0 >= 8
            && (k % 256) == 0
            && m >= 128
            && (m % 128) == 0
            && a.ne[2] == 1
            && a.ne[3] == 1
            && b.ne[2] == 1
            && b.ne[3] == 1
            && b.nb[0] == 4
            && act_stride == k
            && (q5_canonical || q5_validated_stride)
            && t.nb[1] / 4 >= n;
        if q5k_mmq_ok {
            return self.gemm_quant_q5k_j128(
                t,
                kind,
                k,
                n,
                m,
                a.nb[1] / 176,
                act_stride,
                profile,
            );
        }

        // llama.cpp Q6_K J=128 MMA (D4 activations, m16n8k16). Default-on
        // for the same full J tiles as Q4. Lives only in this executor so
        // Fable's shared ggml CUDA path is untouched.
        // MKLLM_DISABLE_Q6K_MMQ=1 restores slab.
        let q6k_mmq_disabled = std::env::var_os("MKLLM_DISABLE_Q6K_MMQ")
            .map(|v| v == "1")
            .unwrap_or(false);
        let q6_row_blocks = k / 256;
        let q6_canonical = a.nb[0] == 210
            && a.nb[1] == row_bytes
            && row_bytes % 210 == 0
            && row_bytes / 210 >= q6_row_blocks;
        let q6_validated_stride =
            a.nb[0] == 210 && a.nb[1] % 210 == 0 && a.nb[1] / 210 >= q6_row_blocks;
        let q6k_mmq_ok = !q6k_mmq_disabled
            && kind == MKLLM_QUANT_Q6K
            && cc.0 >= 8
            && (k % 256) == 0
            && m >= 128
            && (m % 128) == 0
            && a.ne[2] == 1
            && a.ne[3] == 1
            && b.ne[2] == 1
            && b.ne[3] == 1
            && b.nb[0] == 4
            && act_stride == k
            && (q6_canonical || q6_validated_stride)
            && t.nb[1] / 4 >= n;
        if q6k_mmq_ok {
            return self.gemm_quant_q6k_j128(
                t,
                kind,
                k,
                n,
                m,
                a.nb[1] / 210,
                act_stride,
                profile,
            );
        }

        // Activations f32 -> bf16 once.
        let act_bytes = k * m * 2;
        let act_ptr = self
            .state
            .scratch_acts
            .borrow_mut()
            .ensure(act_bytes, "activation scratch")?;
        check(
            unsafe {
                cast_f32_bf16(
                    self.ptr_of(b_id)?,
                    act_ptr,
                    k as i32,
                    m as i32,
                    b.nb[0],
                    b.nb[1],
                    stream,
                )
            },
            "activation cast",
        )?;
        if let (Some(timeline), Some(start)) = (profile.as_mut(), stage_start) {
            stage_start = Some(timeline.finish(start, kind, "activation_cast")?);
        }

        // Row-slab loop: dequant a bounded slab to bf16, GEMM into the
        // output row range, reuse the slab. Peak transient = one slab.
        let slab_rows = (GEMM_SLAB_BYTES / (k * 2)).clamp(1, n);
        let slab_ptr = self
            .state
            .scratch_weights
            .borrow_mut()
            .ensure(slab_rows * k * 2, "weight slab scratch")?;
        let a_base = self.ptr_of(a_id)?;
        let d_base = self.ptr_of(t.id)?;
        let mut r0 = 0usize;
        while r0 < n {
            let rc = slab_rows.min(n - r0);
            check(
                unsafe {
                    dequant_rows_bf16(
                        kind,
                        (a_base as *const u8).add(r0 * row_bytes) as *const c_void,
                        slab_ptr,
                        rc as i32,
                        k as i32,
                        row_bytes,
                        stream,
                    )
                },
                "slab dequant",
            )?;
            if let (Some(timeline), Some(start)) = (profile.as_mut(), stage_start) {
                stage_start = Some(timeline.finish(start, kind, "weight_dequant")?);
            }
            let alpha = 1.0f32;
            let beta_scalar = 0.0f32;
            let status = unsafe {
                cublasGemmEx(
                    self.state.blas,
                    CUBLAS_OP_T,
                    CUBLAS_OP_N,
                    rc as i32,
                    m as i32,
                    k as i32,
                    &alpha as *const f32 as *const c_void,
                    slab_ptr,
                    CUDA_R_16BF,
                    k as i32,
                    act_ptr,
                    CUDA_R_16BF,
                    k as i32,
                    &beta_scalar as *const f32 as *const c_void,
                    (d_base as *mut f32).add(r0) as *mut c_void,
                    CUDA_R_32F,
                    n as i32,
                    CUBLAS_COMPUTE_32F,
                    CUBLAS_GEMM_DEFAULT_TENSOR_OP,
                )
            };
            if status != CUBLAS_STATUS_SUCCESS {
                return Err(LlamaError::format(format!(
                    "cublas quantized GEMM failed: {status}"
                )));
            }
            if let (Some(timeline), Some(start)) = (profile.as_mut(), stage_start) {
                stage_start = Some(timeline.finish(start, kind, "cublas_gemm")?);
            }
            r0 += rc;
        }
        Ok(())
    }
}
