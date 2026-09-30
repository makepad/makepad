//! The training surface (kernels/train.cu): device buffers from a size-class
//! pool, a row-major TF32 GEMM, and the `mkt_*` kernels (forward, backward,
//! optimiser, STFT family, monotonic alignment) the singing model's `nn`
//! GPU backend drives. Everything runs on the default stream, so a host copy
//! is the only synchronisation point.

use crate::driver::*;
use std::collections::HashMap;
use std::ffi::{c_int, c_void};
use std::sync::Mutex;

/// cublas_api.h `CUBLAS_COMPUTE_32F_FAST_TF32`: f32 storage, tensor-core math.
pub const CUBLAS_COMPUTE_32F_FAST_TF32: cublasComputeType_t = 77;

type St = cudaStream_t;

cuda_ffi! {
    pub fn mkt_act_fwd(x: *const f32, y: *mut f32, n: usize, op: c_int, st: St) -> cudaError_t;
    pub fn mkt_act_bwd(x: *const f32, d: *const f32, dx: *mut f32, n: usize, op: c_int, st: St) -> cudaError_t;
    pub fn mkt_add(a: *const f32, b: *const f32, y: *mut f32, n: usize, st: St) -> cudaError_t;
    pub fn mkt_mul(a: *const f32, b: *const f32, y: *mut f32, n: usize, st: St) -> cudaError_t;
    pub fn mkt_scale(x: *const f32, y: *mut f32, s: f32, n: usize, st: St) -> cudaError_t;
    pub fn mkt_axpy(x: *const f32, y: *mut f32, s: f32, n: usize, st: St) -> cudaError_t;
    pub fn mkt_mul_acc(x: *const f32, o: *const f32, y: *mut f32, n: usize, st: St) -> cudaError_t;
    pub fn mkt_fill(y: *mut f32, v: f32, n: usize, st: St) -> cudaError_t;
    pub fn mkt_axpy_scalar(x: *const f32, y: *mut f32, s: f32, st: St) -> cudaError_t;
    pub fn mkt_log_eps(x: *const f32, y: *mut f32, eps: f32, n: usize, st: St) -> cudaError_t;
    pub fn mkt_log_eps_bwd(x: *const f32, d: *const f32, dx: *mut f32, eps: f32, n: usize, st: St) -> cudaError_t;
    pub fn mkt_add_row(x: *const f32, v: *const f32, y: *mut f32, rows: usize, cols: usize, st: St) -> cudaError_t;
    pub fn mkt_mul_row(x: *const f32, v: *const f32, y: *mut f32, rows: usize, cols: usize, st: St) -> cudaError_t;
    pub fn mkt_mul_row_bwd_x(d: *const f32, v: *const f32, dx: *mut f32, rows: usize, cols: usize, st: St) -> cudaError_t;
    pub fn mkt_mul_col(x: *const f32, s: *const f32, y: *mut f32, rows: usize, cols: usize, st: St) -> cudaError_t;
    pub fn mkt_mul_col_bwd_x(d: *const f32, s: *const f32, dx: *mut f32, rows: usize, cols: usize, st: St) -> cudaError_t;
    pub fn mkt_col_dot_acc(d: *const f32, x: *const f32, dv: *mut f32, rows: usize, cols: usize, st: St) -> cudaError_t;
    pub fn mkt_row_dot_acc(d: *const f32, x: *const f32, ds: *mut f32, rows: usize, cols: usize, st: St) -> cudaError_t;
    pub fn mkt_copy_cols(src: *const f32, src_cols: usize, src_off: usize, dst: *mut f32, dst_cols: usize, dst_off: usize, rows: usize, width: usize, acc: c_int, st: St) -> cudaError_t;
    pub fn mkt_gather_rows(x: *const f32, idx: *const u32, y: *mut f32, rows_out: usize, cols: usize, st: St) -> cudaError_t;
    pub fn mkt_scatter_rows_acc(d: *const f32, idx: *const u32, dx: *mut f32, rows_out: usize, cols: usize, st: St) -> cudaError_t;
    pub fn mkt_im2col(x: *const f32, col: *mut f32, rows: usize, c: usize, k: c_int, dil: c_int, seg: usize, st: St) -> cudaError_t;
    pub fn mkt_col2im_acc(dcol: *const f32, dx: *mut f32, rows: usize, c: usize, k: c_int, dil: c_int, seg: usize, st: St) -> cudaError_t;
    pub fn mkt_dwconv(x: *const f32, w: *const f32, y: *mut f32, rows: usize, c: usize, k: c_int, seg: usize, st: St) -> cudaError_t;
    pub fn mkt_dwconv_bwd_x(d: *const f32, w: *const f32, dx: *mut f32, rows: usize, c: usize, k: c_int, seg: usize, st: St) -> cudaError_t;
    pub fn mkt_dwconv_bwd_w(d: *const f32, x: *const f32, dw: *mut f32, rows: usize, c: usize, k: c_int, seg: usize, st: St) -> cudaError_t;
    pub fn mkt_layer_norm(x: *const f32, xhat: *mut f32, rstd: *mut f32, rows: usize, cols: usize, st: St) -> cudaError_t;
    pub fn mkt_layer_norm_bwd(d: *const f32, xhat: *const f32, rstd: *const f32, dx: *mut f32, rows: usize, cols: usize, st: St) -> cudaError_t;
    pub fn mkt_rope(x: *const f32, y: *mut f32, rows: usize, c: usize, heads: c_int, seg: usize, sign: f32, acc: c_int, st: St) -> cudaError_t;
    pub fn mkt_split_heads(x: *const f32, y: *mut f32, b: usize, t: usize, h: c_int, dh: c_int, st: St) -> cudaError_t;
    pub fn mkt_merge_heads(x: *const f32, y: *mut f32, b: usize, t: usize, h: c_int, dh: c_int, acc: c_int, st: St) -> cudaError_t;
    pub fn mkt_softmax_rows(s: *mut f32, rows: usize, t: usize, s_len: usize, h: c_int, key_len: *const u32, scale: f32, st: St) -> cudaError_t;
    pub fn mkt_softmax_bwd_rows(p: *const f32, dp: *mut f32, rows: usize, s_len: usize, scale: f32, st: St) -> cudaError_t;
    pub fn mkt_loss(x: *const f32, t: *const f32, w: *const f32, col_lim: *const u32, seg: usize, g: *mut f32, out: *mut f32, rows: usize, cols: usize, inv_denom: f32, sq: c_int, st: St) -> cudaError_t;
    pub fn mkt_scaled_acc(g: *const f32, d: *const f32, dx: *mut f32, n: usize, st: St) -> cudaError_t;
    pub fn mkt_bce(x: *const f32, t: *const f32, g: *mut f32, out: *mut f32, n: usize, st: St) -> cudaError_t;
    pub fn mkt_hn_filter(gh: *const f32, gn: *const f32, pa: *const f32, pb: *const f32, hr: *const f32, hi: *const f32, nr: *const f32, ni: *const f32, y: *mut f32, rows: usize, f: usize, st: St) -> cudaError_t;
    pub fn mkt_hn_filter_bwd(d: *const f32, gh: *const f32, gn: *const f32, pa: *const f32, pb: *const f32, hr: *const f32, hi: *const f32, nr: *const f32, ni: *const f32, dgh: *mut f32, dgn: *mut f32, dpa: *mut f32, dpb: *mut f32, rows: usize, f: usize, st: St) -> cudaError_t;
    pub fn mkt_fft(re: *mut f32, im: *mut f32, batch: usize, n: c_int, inverse: c_int, st: St) -> cudaError_t;
    pub fn mkt_frame(x: *const f32, gain: *const f32, win: *const f32, re: *mut f32, im: *mut f32, b: usize, len: usize, frames: usize, n: c_int, hop: c_int, st: St) -> cudaError_t;
    pub fn mkt_half(fr: *const f32, fi: *const f32, hr: *mut f32, hi: *mut f32, rows: usize, n: c_int, scale: f32, st: St) -> cudaError_t;
    pub fn mkt_half_c(fr: *const f32, fi: *const f32, hr: *mut f32, hi: *mut f32, rows: usize, n: c_int, c_edge: f32, c_mid: f32, st: St) -> cudaError_t;
    pub fn mkt_hermitian(hr: *const f32, hi: *const f32, fr: *mut f32, fi: *mut f32, rows: usize, n: c_int, c_edge: f32, c_mid: f32, st: St) -> cudaError_t;
    pub fn mkt_overlap_add(fr: *const f32, win: *const f32, norm: *const f32, y: *mut f32, b: usize, len: usize, frames: usize, n: c_int, hop: c_int, scale: f32, acc: c_int, st: St) -> cudaError_t;
    pub fn mkt_mag(re: *const f32, im: *const f32, m: *mut f32, n: usize, st: St) -> cudaError_t;
    pub fn mkt_mag_bwd(d: *const f32, re: *const f32, im: *const f32, m: *const f32, dre: *mut f32, dim: *mut f32, n: usize, st: St) -> cudaError_t;
    pub fn mkt_split_ri(y: *const f32, re: *mut f32, im: *mut f32, rows: usize, f: usize, st: St) -> cudaError_t;
    pub fn mkt_join_ri_acc(re: *const f32, im: *const f32, y: *mut f32, rows: usize, f: usize, st: St) -> cudaError_t;
    pub fn mkt_sumsq(g: *const f32, n: usize, out: *mut f32, st: St) -> cudaError_t;
    pub fn mkt_adamw(p: *mut f32, g: *const f32, m: *mut f32, v: *mut f32, n: usize, lr: f32, b1: f32, b2: f32, eps: f32, wd: f32, bc1: f32, bc2: f32, gscale: *const f32, st: St) -> cudaError_t;
    pub fn mkt_clip_scale(sumsq: *const f32, max_norm: f32, scale: *mut f32, st: St) -> cudaError_t;
    pub fn mkt_ema(e: *mut f32, p: *const f32, decay: f32, n: usize, st: St) -> cudaError_t;
    pub fn mkt_randn(y: *mut f32, n: usize, seed: u64, st: St) -> cudaError_t;
    pub fn mkt_mas_logp(dot: *mut f32, mu: *const f32, mel: *const f32, b: usize, n: c_int, t: c_int, d: c_int, st: St) -> cudaError_t;
    pub fn mkt_dur_to_idx(dur: *const u32, idx: *mut u32, logdur: *mut f32, b: usize, n: c_int, t: c_int, st: St) -> cudaError_t;
    pub fn mkt_harmonic(ph: *const f32, hz: *const f32, amp: *const f32, y: *mut f32, n: usize, top: f32, level: f32, st: St) -> cudaError_t;
    pub fn mkt_mas(logp: *mut f32, n_tok: *const u32, n_frame: *const u32, dur: *mut u32, b: usize, n: c_int, t: c_int, st: St) -> cudaError_t;
}

/// The default stream.
pub fn stream() -> St {
    std::ptr::null_mut()
}

/// Panic with the kernel's name on a CUDA error: training cannot continue.
#[track_caller]
pub fn ck(what: &str, e: cudaError_t) {
    if let Err(err) = check(e) {
        panic!("CUDA {what}: {}", err.message());
    }
}

// ---------------------------------------------------------------------------
// Buffers
// ---------------------------------------------------------------------------

/// Freed allocations by byte capacity (powers of two), reused before cudaMalloc.
static POOL: Mutex<Option<HashMap<usize, Vec<usize>>>> = Mutex::new(None);

fn class(bytes: usize) -> usize {
    bytes.max(256).next_power_of_two()
}

fn raw_alloc(bytes: usize) -> *mut c_void {
    let cap = class(bytes);
    if let Some(p) = POOL.lock().unwrap().get_or_insert_with(HashMap::new).get_mut(&cap).and_then(|v| v.pop()) {
        return p as *mut c_void;
    }
    let mut p: *mut c_void = std::ptr::null_mut();
    let e = unsafe { cudaMalloc(&mut p, cap) };
    if e != CUDA_SUCCESS {
        // Out of memory: return the pooled blocks to the driver and retry once.
        release_pool();
        ck("cudaMalloc", unsafe { cudaMalloc(&mut p, cap) });
    }
    p
}

fn raw_free(p: *mut c_void, bytes: usize) {
    POOL.lock().unwrap().get_or_insert_with(HashMap::new).entry(class(bytes)).or_default().push(p as usize);
}

/// Give every pooled block back to the driver.
pub fn release_pool() {
    let mut g = POOL.lock().unwrap();
    if let Some(map) = g.as_mut() {
        for (_, v) in map.drain() {
            for p in v {
                unsafe { cudaFree(p as *mut c_void) };
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Asynchronous uploads through pinned staging
// ---------------------------------------------------------------------------

/// Two pinned arenas used in turn, one per training step: an upload copies
/// into the current arena and queues an async copy on the default stream, so
/// the host never waits for the device except when an arena comes round
/// again (the step before last must have finished its copies).
struct Arena {
    ptr: *mut u8,
    cap: usize,
    used: usize,
    event: cudaEvent_t,
    armed: bool,
}

struct Staging {
    arenas: [Arena; 2],
    cur: usize,
}

unsafe impl Send for Staging {}

static STAGING: Mutex<Option<Staging>> = Mutex::new(None);

/// Enable staged uploads with two arenas of `bytes` each.
pub fn staging_init(bytes: usize) {
    let mk = || {
        let mut p: *mut c_void = std::ptr::null_mut();
        ck("cudaHostAlloc", unsafe { cudaHostAlloc(&mut p, bytes, 0) });
        Arena { ptr: p as *mut u8, cap: bytes, used: 0, event: event_create().unwrap_or_else(|e| panic!("event: {}", e.message())), armed: false }
    };
    *STAGING.lock().unwrap() = Some(Staging { arenas: [mk(), mk()], cur: 0 });
}

/// Start a step: switch arenas, waiting for the step that last used the new
/// one to finish its copies.
pub fn staging_begin_step() {
    let mut g = STAGING.lock().unwrap();
    let Some(s) = g.as_mut() else { return };
    s.cur ^= 1;
    let a = &mut s.arenas[s.cur];
    if a.armed {
        ck("event sync", unsafe { cudaEventSynchronize(a.event) });
    }
    a.used = 0;
    a.armed = false;
}

/// End a step: mark the current arena's copies.
pub fn staging_end_step() {
    let mut g = STAGING.lock().unwrap();
    let Some(s) = g.as_mut() else { return };
    let a = &mut s.arenas[s.cur];
    if let Err(e) = event_record(a.event, stream()) {
        panic!("event record: {}", e.message());
    }
    a.armed = true;
}

/// Copy `bytes` to `dst` asynchronously if staging is on and has room.
fn staged_copy(dst: *mut c_void, src: *const u8, bytes: usize) -> bool {
    let mut g = STAGING.lock().unwrap();
    let Some(s) = g.as_mut() else { return false };
    let a = &mut s.arenas[s.cur];
    let at = (a.used + 255) & !255;
    if at + bytes > a.cap {
        return false;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(src, a.ptr.add(at), bytes);
        ck("memcpy async", cudaMemcpyAsync(dst, a.ptr.add(at) as *const c_void, bytes, CUDA_MEMCPY_HOST_TO_DEVICE, stream()));
    }
    a.used = at + bytes;
    true
}

fn upload_bytes(dst: *mut c_void, src: *const u8, bytes: usize) {
    if bytes == 0 || staged_copy(dst, src, bytes) {
        return;
    }
    ck("upload", unsafe { cudaMemcpy(dst, src as *const c_void, bytes, CUDA_MEMCPY_HOST_TO_DEVICE) });
}

/// An f32 device buffer. GPU memory is used through raw pointers, so a shared
/// reference can be written by a kernel (the graph orders all writes).
pub struct DevBuf {
    ptr: *mut f32,
    len: usize,
}

unsafe impl Send for DevBuf {}
unsafe impl Sync for DevBuf {}

impl DevBuf {
    /// Uninitialised.
    pub fn new(len: usize) -> DevBuf {
        DevBuf { ptr: raw_alloc(len * 4) as *mut f32, len }
    }
    pub fn zeros(len: usize) -> DevBuf {
        let b = DevBuf::new(len);
        b.fill(0.0);
        b
    }
    pub fn from_host(v: &[f32]) -> DevBuf {
        let b = DevBuf::new(v.len());
        b.upload(v);
        b
    }
    pub fn upload(&self, v: &[f32]) {
        assert!(v.len() <= self.len);
        upload_bytes(self.ptr as *mut c_void, v.as_ptr() as *const u8, v.len() * 4);
    }
    pub fn to_host(&self) -> Vec<f32> {
        let mut v = vec![0.0f32; self.len];
        if self.len > 0 {
            ck("download", unsafe { cudaMemcpy(v.as_mut_ptr() as *mut c_void, self.ptr as *const c_void, self.len * 4, CUDA_MEMCPY_DEVICE_TO_HOST) });
        }
        v
    }
    pub fn fill(&self, v: f32) {
        if self.len > 0 {
            ck("fill", unsafe { mkt_fill(self.ptr, v, self.len, stream()) });
        }
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn ptr(&self) -> *const f32 {
        self.ptr
    }
    pub fn mptr(&self) -> *mut f32 {
        self.ptr
    }
}

impl Drop for DevBuf {
    fn drop(&mut self) {
        raw_free(self.ptr as *mut c_void, self.len * 4);
    }
}

/// A u32 device buffer (indices, lengths).
pub struct DevU32 {
    ptr: *mut u32,
    len: usize,
}

unsafe impl Send for DevU32 {}
unsafe impl Sync for DevU32 {}

impl DevU32 {
    pub fn from_host(v: &[u32]) -> DevU32 {
        let ptr = raw_alloc(v.len().max(1) * 4) as *mut u32;
        upload_bytes(ptr as *mut c_void, v.as_ptr() as *const u8, v.len() * 4);
        DevU32 { ptr, len: v.len() }
    }
    pub fn zeros(len: usize) -> DevU32 {
        DevU32::from_host(&vec![0; len])
    }
    pub fn to_host(&self) -> Vec<u32> {
        let mut v = vec![0u32; self.len];
        if self.len > 0 {
            ck("download u32", unsafe { cudaMemcpy(v.as_mut_ptr() as *mut c_void, self.ptr as *const c_void, self.len * 4, CUDA_MEMCPY_DEVICE_TO_HOST) });
        }
        v
    }
    pub fn ptr(&self) -> *const u32 {
        self.ptr
    }
    pub fn mptr(&self) -> *mut u32 {
        self.ptr
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl Drop for DevU32 {
    fn drop(&mut self) {
        raw_free(self.ptr as *mut c_void, self.len.max(1) * 4);
    }
}

// ---------------------------------------------------------------------------
// GEMM
// ---------------------------------------------------------------------------

thread_local! {
    static HANDLE: std::cell::Cell<cublasHandle_t> = const { std::cell::Cell::new(std::ptr::null_mut()) };
}

fn handle() -> cublasHandle_t {
    HANDLE.with(|h| {
        if h.get().is_null() {
            h.set(cublas_create().unwrap_or_else(|e| panic!("cublasCreate: {e:?}")));
        }
        h.get()
    })
}

/// Whether GEMMs use TF32 tensor cores (f32 storage and accumulation).
static TF32: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

pub fn set_tf32(on: bool) {
    TF32.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn tf32() -> bool {
    TF32.load(std::sync::atomic::Ordering::Relaxed)
}

/// Row-major, strided-batched: C[m,n] = alpha op(A) op(B) + beta C, where
/// op(A) is [m,k] (A stored [k,m] when `ta`) and op(B) is [k,n] (B stored
/// [n,k] when `tb`); lda/ldb/ldc are the stored row lengths.
#[allow(clippy::too_many_arguments)]
pub fn gemm(ta: bool, tb: bool, m: usize, n: usize, k: usize, alpha: f32, a: *const f32, lda: usize, sa: i64, b: *const f32, ldb: usize, sb: i64, beta: f32, c: *mut f32, ldc: usize, sc: i64, batch: usize) {
    if m == 0 || n == 0 || batch == 0 {
        return;
    }
    // Row-major C = op(A) op(B)  <=>  column-major C^T = op(B)^T op(A)^T,
    // and a row-major matrix read column-major is its transpose.
    let opb = if tb { CUBLAS_OP_T } else { CUBLAS_OP_N };
    let opa = if ta { CUBLAS_OP_T } else { CUBLAS_OP_N };
    let compute = if TF32.load(std::sync::atomic::Ordering::Relaxed) { CUBLAS_COMPUTE_32F_FAST_TF32 } else { CUBLAS_COMPUTE_32F };
    let r = unsafe {
        cublas_gemm_strided_batched_ex(
            handle(), opb, opa, n as i32, m as i32, k as i32, &alpha,
            b as *const c_void, CUDA_R_32F, ldb as i32, sb,
            a as *const c_void, CUDA_R_32F, lda as i32, sa,
            &beta, c as *mut c_void, CUDA_R_32F, ldc as i32, sc,
            batch as i32, compute, CUBLAS_GEMM_DEFAULT,
        )
    };
    if let Err(e) = r {
        panic!("cuBLAS gemm {m}x{n}x{k} (batch {batch}): {e:?}");
    }
}

/// Wait for the device (only for timing and tests).
pub fn sync() {
    ck("sync", unsafe { cudaDeviceSynchronize() });
}

/// (free, total) device memory in bytes.
pub fn mem() -> (usize, usize) {
    mem_get_info().unwrap_or((0, 0))
}
