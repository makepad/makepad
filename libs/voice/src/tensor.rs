use crate::quant::*;
use std::cell::RefCell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};

thread_local! {
    static Q8_ACT_SCRATCH: RefCell<Vec<u8>> = RefCell::new(Vec::new());
}

fn quant_gpu_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("MAKEPAD_VOICE_METAL_QUANT")
            .ok()
            .map(|v| {
                let v = v.trim().to_ascii_lowercase();
                !(v.is_empty() || v == "0" || v == "false" || v == "no" || v == "off")
            })
            .unwrap_or(true)
    })
}

/// Wrapper to pass raw *mut f32 across thread boundaries.
/// Stores the pointer as usize which is always Send+Sync.
/// Safety: callers must ensure non-overlapping writes and valid lifetime.
#[derive(Clone, Copy)]
pub struct SendPtr(usize);

impl SendPtr {
    pub fn new(p: *mut f32) -> Self {
        SendPtr(p as usize)
    }
    #[inline]
    pub fn ptr(&self) -> *mut f32 {
        self.0 as *mut f32
    }
}

#[derive(Clone, Copy)]
struct SendPtrU8(usize);

impl SendPtrU8 {
    fn new(p: *mut u8) -> Self {
        SendPtrU8(p as usize)
    }
    #[inline]
    fn ptr(&self) -> *mut u8 {
        self.0 as *mut u8
    }
}

/// A simple persistent thread pool.
/// Workers park on a Condvar and wake when work is submitted.
struct ThreadPool {
    n_workers: usize,
    // Work descriptor — protected by mutex for epoch synchronization
    work: Mutex<WorkDesc>,
    wake: Condvar,
    // Atomic work counter — accessed without mutex during work-steal
    counter: AtomicUsize,
    // Completion tracking
    done_count: AtomicUsize,
    done_mutex: Mutex<bool>,
    done_cv: Condvar,
}

struct WorkDesc {
    fn_data: usize,
    fn_vtable: usize,
    total: usize,
    epoch: usize,
}

impl ThreadPool {
    fn new(n: usize) -> &'static Self {
        let pool = Box::leak(Box::new(ThreadPool {
            n_workers: n,
            work: Mutex::new(WorkDesc {
                fn_data: 0,
                fn_vtable: 0,
                total: 0,
                epoch: 0,
            }),
            wake: Condvar::new(),
            counter: AtomicUsize::new(0),
            done_count: AtomicUsize::new(0),
            done_mutex: Mutex::new(false),
            done_cv: Condvar::new(),
        }));

        for _ in 0..n {
            let pool_ptr = pool as *const ThreadPool as usize;
            std::thread::spawn(move || {
                let pool = unsafe { &*(pool_ptr as *const ThreadPool) };
                let mut last_epoch = 0;
                loop {
                    // Wait for new work
                    let (fn_data, fn_vtable, total) = {
                        let mut w = pool.work.lock().unwrap();
                        while w.epoch == last_epoch {
                            w = pool.wake.wait(w).unwrap();
                        }
                        last_epoch = w.epoch;
                        (w.fn_data, w.fn_vtable, w.total)
                    };

                    // Reconstruct the &dyn Fn(usize) from raw parts
                    let f: &dyn Fn(usize) = unsafe { std::mem::transmute([fn_data, fn_vtable]) };

                    // Work-steal loop
                    loop {
                        let i = pool.counter.fetch_add(1, Ordering::Relaxed);
                        if i >= total {
                            break;
                        }
                        f(i);
                    }

                    // Signal completion
                    if pool.done_count.fetch_add(1, Ordering::AcqRel) + 1 == pool.n_workers {
                        let mut done = pool.done_mutex.lock().unwrap();
                        *done = true;
                        pool.done_cv.notify_one();
                    }
                }
            });
        }

        pool
    }

    /// Submit work and block until complete.
    /// SAFETY: `f` must remain valid until all workers are done (guaranteed by blocking).
    fn run(&self, total: usize, f: &dyn Fn(usize)) {
        // Encode the fat pointer as two usizes
        let raw: [usize; 2] = unsafe { std::mem::transmute(f) };

        // Reset completion and counter
        self.done_count.store(0, Ordering::Relaxed);
        self.counter.store(0, Ordering::Relaxed);
        *self.done_mutex.lock().unwrap() = false;

        // Set up work and wake all workers
        {
            let mut w = self.work.lock().unwrap();
            w.fn_data = raw[0];
            w.fn_vtable = raw[1];
            w.total = total;
            w.epoch += 1;
        }
        self.wake.notify_all();

        // Wait for all workers to finish
        let mut done = self.done_mutex.lock().unwrap();
        while !*done {
            done = self.done_cv.wait(done).unwrap();
        }
    }
}

fn get_pool() -> &'static ThreadPool {
    static POOL: std::sync::OnceLock<&'static ThreadPool> = std::sync::OnceLock::new();
    *POOL.get_or_init(|| {
        let hw = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        let max_threads = hw.max(1);
        let default_threads = if cfg!(any(
            target_os = "macos",
            target_os = "windows",
            target_os = "linux"
        )) {
            hw.min(4).max(1)
        } else {
            hw.min(8).max(4).min(max_threads)
        };
        let n = std::env::var("MAKEPAD_VOICE_THREADS")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .map(|v| v.clamp(1, max_threads))
            .unwrap_or(default_threads);
        ThreadPool::new(n)
    })
}

#[inline]
fn use_act_q8() -> bool {
    static USE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *USE.get_or_init(|| {
        let default_on = cfg!(target_arch = "aarch64");
        let on = std::env::var("MAKEPAD_VOICE_ACT_Q8")
            .ok()
            .map(|v| v != "0" && v.to_lowercase() != "false")
            .unwrap_or(default_on);
        on
    })
}

/// Run `n_jobs` tasks in parallel, each receiving its job index.
#[inline]
pub fn parallel_for(n_jobs: usize, f: impl Fn(usize) + Send + Sync) {
    if n_jobs == 0 {
        return;
    }
    if n_jobs == 1 {
        f(0);
        return;
    }
    get_pool().run(n_jobs, &f);
}

/// Work-stealing parallel loop: distribute `total` work items across threads.
#[inline]
pub fn parallel_work_steal(total: usize, f: impl Fn(usize) + Send + Sync) {
    if total == 0 {
        return;
    }
    if total < 64 || get_pool().n_workers <= 1 {
        for i in 0..total {
            f(i);
        }
        return;
    }
    get_pool().run(total, &f);
}

// SIMD dot product: compute dot product of two f32 slices
#[inline]
fn dot_f32(a: &[f32], b: &[f32]) -> f32 {
    dot_f32_simd(a, b)
}

#[cfg(target_arch = "aarch64")]
#[inline]
fn dot_f32_simd(a: &[f32], b: &[f32]) -> f32 {
    use std::arch::aarch64::*;
    let n = a.len().min(b.len());
    let mut i = 0;
    unsafe {
        let mut sum0 = vdupq_n_f32(0.0);
        let mut sum1 = vdupq_n_f32(0.0);
        let mut sum2 = vdupq_n_f32(0.0);
        let mut sum3 = vdupq_n_f32(0.0);
        // Process 16 elements per iteration (4 NEON registers x 4 floats)
        while i + 15 < n {
            let a0 = vld1q_f32(a.as_ptr().add(i));
            let b0 = vld1q_f32(b.as_ptr().add(i));
            sum0 = vfmaq_f32(sum0, a0, b0);
            let a1 = vld1q_f32(a.as_ptr().add(i + 4));
            let b1 = vld1q_f32(b.as_ptr().add(i + 4));
            sum1 = vfmaq_f32(sum1, a1, b1);
            let a2 = vld1q_f32(a.as_ptr().add(i + 8));
            let b2 = vld1q_f32(b.as_ptr().add(i + 8));
            sum2 = vfmaq_f32(sum2, a2, b2);
            let a3 = vld1q_f32(a.as_ptr().add(i + 12));
            let b3 = vld1q_f32(b.as_ptr().add(i + 12));
            sum3 = vfmaq_f32(sum3, a3, b3);
            i += 16;
        }
        // Process 4 elements at a time
        while i + 3 < n {
            let va = vld1q_f32(a.as_ptr().add(i));
            let vb = vld1q_f32(b.as_ptr().add(i));
            sum0 = vfmaq_f32(sum0, va, vb);
            i += 4;
        }
        sum0 = vaddq_f32(sum0, sum1);
        sum2 = vaddq_f32(sum2, sum3);
        sum0 = vaddq_f32(sum0, sum2);
        let mut result = vaddvq_f32(sum0);
        // Scalar tail
        while i < n {
            result += a[i] * b[i];
            i += 1;
        }
        result
    }
}

#[cfg(target_arch = "x86_64")]
#[inline]
fn dot_f32_simd(a: &[f32], b: &[f32]) -> f32 {
    if is_x86_feature_detected!("avx") && is_x86_feature_detected!("fma") {
        unsafe { dot_f32_avx_fma(a, b) }
    } else {
        dot_f32_scalar(a, b)
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx,fma")]
unsafe fn dot_f32_avx_fma(a: &[f32], b: &[f32]) -> f32 {
    use std::arch::x86_64::*;
    let n = a.len().min(b.len());
    let mut i = 0;
    let mut sum0 = _mm256_setzero_ps();
    let mut sum1 = _mm256_setzero_ps();
    let mut sum2 = _mm256_setzero_ps();
    let mut sum3 = _mm256_setzero_ps();
    // Process 32 elements per iteration (4 AVX registers x 8 floats)
    while i + 31 < n {
        let a0 = _mm256_loadu_ps(a.as_ptr().add(i));
        let b0 = _mm256_loadu_ps(b.as_ptr().add(i));
        sum0 = _mm256_fmadd_ps(a0, b0, sum0);
        let a1 = _mm256_loadu_ps(a.as_ptr().add(i + 8));
        let b1 = _mm256_loadu_ps(b.as_ptr().add(i + 8));
        sum1 = _mm256_fmadd_ps(a1, b1, sum1);
        let a2 = _mm256_loadu_ps(a.as_ptr().add(i + 16));
        let b2 = _mm256_loadu_ps(b.as_ptr().add(i + 16));
        sum2 = _mm256_fmadd_ps(a2, b2, sum2);
        let a3 = _mm256_loadu_ps(a.as_ptr().add(i + 24));
        let b3 = _mm256_loadu_ps(b.as_ptr().add(i + 24));
        sum3 = _mm256_fmadd_ps(a3, b3, sum3);
        i += 32;
    }
    // Process 8 elements at a time
    while i + 7 < n {
        let va = _mm256_loadu_ps(a.as_ptr().add(i));
        let vb = _mm256_loadu_ps(b.as_ptr().add(i));
        sum0 = _mm256_fmadd_ps(va, vb, sum0);
        i += 8;
    }
    sum0 = _mm256_add_ps(sum0, sum1);
    sum2 = _mm256_add_ps(sum2, sum3);
    sum0 = _mm256_add_ps(sum0, sum2);
    // Horizontal sum of 8 floats
    let hi = _mm256_extractf128_ps(sum0, 1);
    let lo = _mm256_castps256_ps128(sum0);
    let sum128 = _mm_add_ps(lo, hi);
    let shuf = _mm_movehdup_ps(sum128);
    let sums = _mm_add_ps(sum128, shuf);
    let shuf2 = _mm_movehl_ps(sums, sums);
    let sums2 = _mm_add_ss(sums, shuf2);
    let mut result = _mm_cvtss_f32(sums2);
    // Scalar tail
    while i < n {
        result += a[i] * b[i];
        i += 1;
    }
    result
}

#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
#[inline]
fn dot_f32_simd(a: &[f32], b: &[f32]) -> f32 {
    dot_f32_scalar(a, b)
}

#[inline]
#[cfg(not(target_arch = "aarch64"))]
fn dot_f32_scalar(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len().min(b.len());
    let mut sum = 0.0f32;
    let mut i = 0;
    while i + 3 < n {
        sum += a[i] * b[i] + a[i + 1] * b[i + 1] + a[i + 2] * b[i + 2] + a[i + 3] * b[i + 3];
        i += 4;
    }
    while i < n {
        sum += a[i] * b[i];
        i += 1;
    }
    sum
}

// ---- f16 × f32 dot product (widening FMA, no pre-dequant) ----

/// Dot product of f16 weights (raw bytes, 2 bytes per element) with f32 activations.
/// `w_f16` is &[u8] with length n*2, `x_f32` is &[f32] with length n.
#[inline]
fn dot_f16_f32(w_f16: &[u8], x_f32: &[f32]) -> f32 {
    dot_f16_f32_simd(w_f16, x_f32)
}

#[cfg(target_arch = "aarch64")]
#[inline]
fn dot_f16_f32_simd(w_f16: &[u8], x_f32: &[f32]) -> f32 {
    use std::arch::aarch64::*;
    let n = x_f32.len();
    let w_ptr = w_f16.as_ptr() as *const u16;
    let x_ptr = x_f32.as_ptr();
    let mut i = 0;
    unsafe {
        let mut sum0 = vdupq_n_f32(0.0);
        let mut sum1 = vdupq_n_f32(0.0);
        let mut sum2 = vdupq_n_f32(0.0);
        let mut sum3 = vdupq_n_f32(0.0);

        // Use inline asm for FCVTL (f16->f32 widening) since the intrinsic is unstable
        // FCVTL converts 4 f16 in Vn.4H (lower half of 64-bit D register) to 4 f32 in Vd.4S
        #[inline(always)]
        unsafe fn f16x4_to_f32x4(h: uint16x4_t) -> float32x4_t {
            let out: float32x4_t;
            std::arch::asm!(
                "fcvtl {out:v}.4s, {inp:v}.4h",
                inp = in(vreg) h,
                out = out(vreg) out,
                options(pure, nomem, nostack)
            );
            out
        }

        // Process 16 elements per iteration
        while i + 15 < n {
            let h0 = vld1_u16(w_ptr.add(i));
            let w0 = f16x4_to_f32x4(h0);
            let x0 = vld1q_f32(x_ptr.add(i));
            sum0 = vfmaq_f32(sum0, w0, x0);

            let h1 = vld1_u16(w_ptr.add(i + 4));
            let w1 = f16x4_to_f32x4(h1);
            let x1 = vld1q_f32(x_ptr.add(i + 4));
            sum1 = vfmaq_f32(sum1, w1, x1);

            let h2 = vld1_u16(w_ptr.add(i + 8));
            let w2 = f16x4_to_f32x4(h2);
            let x2 = vld1q_f32(x_ptr.add(i + 8));
            sum2 = vfmaq_f32(sum2, w2, x2);

            let h3 = vld1_u16(w_ptr.add(i + 12));
            let w3 = f16x4_to_f32x4(h3);
            let x3 = vld1q_f32(x_ptr.add(i + 12));
            sum3 = vfmaq_f32(sum3, w3, x3);

            i += 16;
        }
        while i + 3 < n {
            let h = vld1_u16(w_ptr.add(i));
            let w = f16x4_to_f32x4(h);
            let x = vld1q_f32(x_ptr.add(i));
            sum0 = vfmaq_f32(sum0, w, x);
            i += 4;
        }
        sum0 = vaddq_f32(sum0, sum1);
        sum2 = vaddq_f32(sum2, sum3);
        sum0 = vaddq_f32(sum0, sum2);
        let mut result = vaddvq_f32(sum0);
        while i < n {
            result += f16_to_f32(*w_ptr.add(i)) * x_f32[i];
            i += 1;
        }
        result
    }
}

#[cfg(not(target_arch = "aarch64"))]
#[inline]
fn dot_f16_f32_simd(w_f16: &[u8], x_f32: &[f32]) -> f32 {
    let n = x_f32.len();
    let mut sum = 0.0f32;
    for i in 0..n {
        let h = u16::from_le_bytes([w_f16[i * 2], w_f16[i * 2 + 1]]);
        sum += f16_to_f32(h) * x_f32[i];
    }
    sum
}

/// A tensor storing f32 data with shape information.
/// All computation happens in f32. Quantized weights are dequantized on the fly during matmul.
#[derive(Clone)]
pub struct Tensor {
    pub data: Vec<f32>,
    pub shape: Vec<usize>, // e.g. [rows, cols] for 2D
}

/// Raw weight tensor - stores quantized (or f16/f32) bytes as loaded from the model file.
/// Dequantized on the fly during matrix multiply.
pub struct RawTensor {
    pub data: Vec<u8>,
    pub shape: Vec<usize>,
    pub ggml_type: u32,
}

impl RawTensor {
    /// Dequantize the entire tensor to f32.
    pub fn to_f32(&self) -> Tensor {
        let n = self.shape.iter().product::<usize>();
        let mut out = vec![0.0f32; n];
        match self.ggml_type {
            GGML_TYPE_F32 => {
                for i in 0..n {
                    let off = i * 4;
                    out[i] = f32::from_le_bytes([
                        self.data[off],
                        self.data[off + 1],
                        self.data[off + 2],
                        self.data[off + 3],
                    ]);
                }
            }
            GGML_TYPE_F16 => {
                for i in 0..n {
                    let off = i * 2;
                    out[i] = f16_to_f32(u16::from_le_bytes([self.data[off], self.data[off + 1]]));
                }
            }
            GGML_TYPE_Q4_0 => {
                let bs = block_size(GGML_TYPE_Q4_0);
                let mut tmp = [0.0f32; QK];
                let nb = n / QK;
                for b in 0..nb {
                    dequantize_q4_0(&self.data[b * bs..], &mut tmp);
                    out[b * QK..(b + 1) * QK].copy_from_slice(&tmp);
                }
            }
            GGML_TYPE_Q4_1 => {
                let bs = block_size(GGML_TYPE_Q4_1);
                let mut tmp = [0.0f32; QK];
                let nb = n / QK;
                for b in 0..nb {
                    dequantize_q4_1(&self.data[b * bs..], &mut tmp);
                    out[b * QK..(b + 1) * QK].copy_from_slice(&tmp);
                }
            }
            GGML_TYPE_Q5_0 => {
                let bs = block_size(GGML_TYPE_Q5_0);
                let mut tmp = [0.0f32; QK];
                let nb = n / QK;
                for b in 0..nb {
                    dequantize_q5_0(&self.data[b * bs..], &mut tmp);
                    out[b * QK..(b + 1) * QK].copy_from_slice(&tmp);
                }
            }
            GGML_TYPE_Q5_1 => {
                let bs = block_size(GGML_TYPE_Q5_1);
                let mut tmp = [0.0f32; QK];
                let nb = n / QK;
                for b in 0..nb {
                    dequantize_q5_1(&self.data[b * bs..], &mut tmp);
                    out[b * QK..(b + 1) * QK].copy_from_slice(&tmp);
                }
            }
            GGML_TYPE_Q8_0 => {
                let bs = block_size(GGML_TYPE_Q8_0);
                let mut tmp = [0.0f32; QK];
                let nb = n / QK;
                for b in 0..nb {
                    dequantize_q8_0(&self.data[b * bs..], &mut tmp);
                    out[b * QK..(b + 1) * QK].copy_from_slice(&tmp);
                }
            }
            t => panic!("unsupported dequantize type {}", t),
        }
        // Reverse shape from GGML column-major to row-major order.
        let mut shape = self.shape.clone();
        shape.reverse();
        Tensor { data: out, shape }
    }

    /// Number of rows (product of all dims except dim0).
    /// In GGML layout, dim0 is contiguous. Rows = dim1 * dim2 * ...
    pub fn n_rows(&self) -> usize {
        if self.shape.len() <= 1 {
            1
        } else {
            self.shape[1..].iter().product()
        }
    }

    /// Number of columns (dim0 = contiguous/inner dimension in GGML layout)
    pub fn n_cols(&self) -> usize {
        self.shape.first().copied().unwrap_or(1)
    }

    /// Convert this RawTensor to Q8_0 quantization (from F16 or F32).
    /// If already Q8_0 or another quantized type, returns self unchanged.
    pub fn to_q8_0(self) -> RawTensor {
        match self.ggml_type {
            GGML_TYPE_F16 => {
                let n_elements: usize = self.shape.iter().product();
                let data = quantize_f16_to_q8_0(&self.data, n_elements);
                RawTensor {
                    data,
                    shape: self.shape,
                    ggml_type: GGML_TYPE_Q8_0,
                }
            }
            GGML_TYPE_F32 => {
                let n_elements: usize = self.shape.iter().product();
                // First convert to f32 vec, then quantize
                let mut f32_data = vec![0.0f32; n_elements];
                for i in 0..n_elements {
                    let off = i * 4;
                    f32_data[i] = f32::from_le_bytes([
                        self.data[off],
                        self.data[off + 1],
                        self.data[off + 2],
                        self.data[off + 3],
                    ]);
                }
                let data = quantize_f32_to_q8_0(&f32_data);
                RawTensor {
                    data,
                    shape: self.shape,
                    ggml_type: GGML_TYPE_Q8_0,
                }
            }
            _ => self, // already quantized, leave as-is
        }
    }

    /// Bytes per row
    pub fn row_bytes(&self) -> usize {
        let cols = self.n_cols();
        let be = block_elements(self.ggml_type);
        let bs = block_size(self.ggml_type);
        (cols / be) * bs
    }

    /// Get raw bytes for a specific row
    pub fn row_data(&self, row: usize) -> &[u8] {
        let rb = self.row_bytes();
        &self.data[row * rb..(row + 1) * rb]
    }
}

impl Tensor {
    pub fn zeros(shape: &[usize]) -> Self {
        let n: usize = shape.iter().product();
        Tensor {
            data: vec![0.0; n],
            shape: shape.to_vec(),
        }
    }

    /// SIMD-accelerated dot product of two f32 slices.
    #[inline]
    pub fn dot_f32(a: &[f32], b: &[f32]) -> f32 {
        dot_f32(a, b)
    }

    pub fn numel(&self) -> usize {
        self.shape.iter().product()
    }

    /// Reshape (no copy, just changes shape metadata). Panics if numel differs.
    pub fn reshape(&self, new_shape: &[usize]) -> Tensor {
        let n: usize = new_shape.iter().product();
        assert_eq!(
            n,
            self.numel(),
            "reshape: element count mismatch {} vs {}",
            n,
            self.numel()
        );
        Tensor {
            data: self.data.clone(),
            shape: new_shape.to_vec(),
        }
    }

    /// 2D shape helpers
    pub fn rows(&self) -> usize {
        if self.shape.len() == 1 {
            1
        } else {
            self.shape[0]
        }
    }
    pub fn cols(&self) -> usize {
        *self.shape.last().unwrap()
    }

    // ---- Element-wise operations ----

    /// out = a + b (broadcasting b if it's smaller)
    pub fn add(a: &Tensor, b: &Tensor) -> Tensor {
        if let Some(out) = crate::metal_backend::try_add_f32(&a.data, &a.shape, &b.data, &b.shape) {
            return Tensor {
                data: out,
                shape: a.shape.clone(),
            };
        }

        if a.numel() == b.numel() {
            let data: Vec<f32> = a.data.iter().zip(&b.data).map(|(x, y)| x + y).collect();
            return Tensor {
                data,
                shape: a.shape.clone(),
            };
        }
        // broadcast b along leading dims
        let bn = b.numel();
        let data: Vec<f32> = a
            .data
            .iter()
            .enumerate()
            .map(|(i, x)| x + b.data[i % bn])
            .collect();
        Tensor {
            data,
            shape: a.shape.clone(),
        }
    }

    /// out = a + b, in-place on a
    pub fn add_inplace(a: &mut Tensor, b: &Tensor) {
        let bn = b.numel();
        for (i, x) in a.data.iter_mut().enumerate() {
            *x += b.data[i % bn];
        }
    }

    /// out = a * b element-wise (broadcasting b)
    pub fn mul(a: &Tensor, b: &Tensor) -> Tensor {
        if let Some(out) = crate::metal_backend::try_mul_f32(&a.data, &a.shape, &b.data, &b.shape) {
            return Tensor {
                data: out,
                shape: a.shape.clone(),
            };
        }

        if a.numel() == b.numel() {
            let data: Vec<f32> = a.data.iter().zip(&b.data).map(|(x, y)| x * y).collect();
            return Tensor {
                data,
                shape: a.shape.clone(),
            };
        }
        let bn = b.numel();
        let data: Vec<f32> = a
            .data
            .iter()
            .enumerate()
            .map(|(i, x)| x * b.data[i % bn])
            .collect();
        Tensor {
            data,
            shape: a.shape.clone(),
        }
    }

    /// out = a * scalar
    pub fn scale(a: &Tensor, s: f32) -> Tensor {
        let data: Vec<f32> = a.data.iter().map(|x| x * s).collect();
        Tensor {
            data,
            shape: a.shape.clone(),
        }
    }

    pub fn scale_inplace(a: &mut Tensor, s: f32) {
        for x in &mut a.data {
            *x *= s;
        }
    }

    /// GELU activation: x * 0.5 * (1 + tanh(sqrt(2/pi) * (x + 0.044715 * x^3)))
    pub fn gelu(a: &Tensor) -> Tensor {
        if let Some(out) = crate::metal_backend::try_gelu_f32(&a.data, &a.shape) {
            return Tensor {
                data: out,
                shape: a.shape.clone(),
            };
        }

        const SQRT_2_PI: f32 = 0.7978845608;
        let data: Vec<f32> = a
            .data
            .iter()
            .map(|&x| {
                let t = (SQRT_2_PI * (x + 0.044715 * x * x * x)).tanh();
                0.5 * x * (1.0 + t)
            })
            .collect();
        Tensor {
            data,
            shape: a.shape.clone(),
        }
    }

    /// Layer normalization along the last dimension.
    /// out[i] = (x[i] - mean) / sqrt(var + eps)
    pub fn layer_norm(x: &Tensor, eps: f32) -> Tensor {
        if let Some(out) = crate::metal_backend::try_layer_norm_f32(&x.data, &x.shape, eps) {
            return Tensor {
                data: out,
                shape: x.shape.clone(),
            };
        }

        let n = *x.shape.last().unwrap();
        let batches = x.numel() / n;
        let mut out = vec![0.0f32; x.numel()];

        for b in 0..batches {
            let off = b * n;
            let slice = &x.data[off..off + n];

            let mean: f32 = slice.iter().sum::<f32>() / n as f32;
            let var: f32 = slice.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / n as f32;
            let inv_std = 1.0 / (var + eps).sqrt();

            for i in 0..n {
                out[off + i] = (slice[i] - mean) * inv_std;
            }
        }

        Tensor {
            data: out,
            shape: x.shape.clone(),
        }
    }

    /// Layer normalization followed by affine transform.
    /// out = layer_norm(x, eps) * mul + add
    pub fn layer_norm_mul_add(x: &Tensor, mul: &Tensor, add: &Tensor, eps: f32) -> Tensor {
        if let Some(out) = crate::metal_backend::try_layer_norm_mul_add_f32(
            &x.data, &x.shape, &mul.data, &mul.shape, &add.data, &add.shape, eps,
        ) {
            return Tensor {
                data: out,
                shape: x.shape.clone(),
            };
        }

        let norm = Self::layer_norm(x, eps);
        Self::add(&Self::mul(&norm, mul), add)
    }

    /// Softmax along last dimension, with optional additive mask and scale.
    /// out[i] = exp((x[i]*scale + mask[i]) - max) / sum
    pub fn softmax(x: &Tensor, mask: Option<&Tensor>, scale: f32) -> Tensor {
        let n = *x.shape.last().unwrap();
        let batches = x.numel() / n;
        let mut out = vec![0.0f32; x.numel()];

        for b in 0..batches {
            let off = b * n;
            let slice = &x.data[off..off + n];

            let mut max_val = f32::NEG_INFINITY;
            for i in 0..n {
                let mut v = slice[i] * scale;
                if let Some(m) = mask {
                    v += m.data[off % m.numel() + i % m.data.len().min(n)];
                }
                if v > max_val {
                    max_val = v;
                }
            }

            let mut sum = 0.0f32;
            for i in 0..n {
                let mut v = slice[i] * scale;
                if let Some(m) = mask {
                    v += m.data[off % m.numel() + i % m.data.len().min(n)];
                }
                let e = (v - max_val).exp();
                out[off + i] = e;
                sum += e;
            }

            let inv_sum = 1.0 / sum;
            for i in 0..n {
                out[off + i] *= inv_sum;
            }
        }

        Tensor {
            data: out,
            shape: x.shape.clone(),
        }
    }

    /// Softmax with a mask tensor that has shape [n_kv] applied per-row,
    /// for the decoder attention mask pattern.
    pub fn softmax_masked(x: &Tensor, mask: &[f32], scale: f32, n_kv: usize) -> Tensor {
        let n = *x.shape.last().unwrap();
        assert_eq!(n, n_kv);
        let batches = x.numel() / n;
        let mut out = vec![0.0f32; x.numel()];

        for b in 0..batches {
            let off = b * n;
            let mask_off = (b % (mask.len() / n)) * n;

            let mut max_val = f32::NEG_INFINITY;
            for i in 0..n {
                let v = x.data[off + i] * scale + mask[mask_off + i];
                if v > max_val {
                    max_val = v;
                }
            }

            let mut sum = 0.0f32;
            for i in 0..n {
                let v = x.data[off + i] * scale + mask[mask_off + i];
                let e = (v - max_val).exp();
                out[off + i] = e;
                sum += e;
            }

            if sum > 0.0 {
                let inv = 1.0 / sum;
                for i in 0..n {
                    out[off + i] *= inv;
                }
            }
        }

        Tensor {
            data: out,
            shape: x.shape.clone(),
        }
    }

    /// Transpose a 2D tensor: [rows, cols] -> [cols, rows]
    pub fn transpose_2d(a: &Tensor) -> Tensor {
        assert_eq!(a.shape.len(), 2);
        let (r, c) = (a.shape[0], a.shape[1]);
        let mut out = vec![0.0f32; r * c];
        for i in 0..r {
            for j in 0..c {
                out[j * r + i] = a.data[i * c + j];
            }
        }
        Tensor {
            data: out,
            shape: vec![c, r],
        }
    }

    /// Get rows (embedding lookup): weight[indices[i]] for each index
    /// weight: [n_vocab, dim], indices: list of i32 -> out: [n_tokens, dim]
    pub fn get_rows(weight: &Tensor, indices: &[i32]) -> Tensor {
        let dim = weight.shape[weight.shape.len() - 1];
        let n = indices.len();
        let mut out = vec![0.0f32; n * dim];
        for (i, &idx) in indices.iter().enumerate() {
            let src_off = idx as usize * dim;
            out[i * dim..(i + 1) * dim].copy_from_slice(&weight.data[src_off..src_off + dim]);
        }
        Tensor {
            data: out,
            shape: vec![n, dim],
        }
    }

    // ---- Matrix multiply ----

    /// Matrix multiply: a[M, K] @ b[K, N] -> out[M, N]
    /// This is the f32 x f32 path. Transposes b for SIMD-friendly dot products.
    pub fn matmul(a: &Tensor, b: &Tensor) -> Tensor {
        assert_eq!(a.shape.len(), 2);
        assert_eq!(b.shape.len(), 2);
        let m = a.shape[0];
        let k = a.shape[1];
        assert_eq!(b.shape[0], k);
        let n = b.shape[1];

        if let Some(out) = crate::metal_backend::try_matmul_nn_f32(&a.data, &b.data, m, k, n) {
            return Tensor {
                data: out,
                shape: vec![m, n],
            };
        }

        // Transpose b to [N, K] so each output column is a contiguous row
        let mut bt = vec![0.0f32; n * k];
        for i in 0..k {
            for j in 0..n {
                bt[j * k + i] = b.data[i * n + j];
            }
        }

        let mut out = vec![0.0f32; m * n];
        for i in 0..m {
            let a_row = &a.data[i * k..(i + 1) * k];
            for j in 0..n {
                let b_row = &bt[j * k..(j + 1) * k];
                out[i * n + j] = dot_f32(a_row, b_row);
            }
        }
        Tensor {
            data: out,
            shape: vec![m, n],
        }
    }

    /// Matrix multiply: x[M, K] @ w^T where w is [N, K] (transposed weight).
    /// Both x and w are f32 Tensors. w has layout [out_features, in_features]
    /// where each row is a contiguous weight vector (ideal for SIMD dot products).
    /// Result is [M, N]. Uses multiple threads for large matmuls.
    pub fn matmul_t(x: &Tensor, w: &Tensor) -> Tensor {
        let _t = std::time::Instant::now();
        let in_features = w.cols();
        let out_features = w.rows();
        let batch = x.numel() / in_features;
        assert_eq!(x.numel(), batch * in_features, "matmul_t: x size mismatch");

        if let Some(out) = crate::metal_backend::try_matmul_nt_f32(
            &x.data,
            &w.data,
            batch,
            in_features,
            out_features,
        ) {
            crate::PROF_MATMUL_T.fetch_add(
                _t.elapsed().as_nanos() as u64,
                std::sync::atomic::Ordering::Relaxed,
            );
            crate::PROF_MATMUL_T_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return Tensor {
                data: out,
                shape: vec![batch, out_features],
            };
        }

        let mut out = vec![0.0f32; batch * out_features];
        let out_ptr = SendPtr::new(out.as_mut_ptr());
        let x_data = &x.data;
        let w_data = &w.data;
        parallel_work_steal(out_features, |o| {
            let w_row = &w_data[o * in_features..(o + 1) * in_features];
            for b in 0..batch {
                let x_row = &x_data[b * in_features..(b + 1) * in_features];
                unsafe {
                    *out_ptr.ptr().add(b * out_features + o) = dot_f32(x_row, w_row);
                }
            }
        });

        crate::PROF_MATMUL_T.fetch_add(
            _t.elapsed().as_nanos() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
        crate::PROF_MATMUL_T_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Tensor {
            data: out,
            shape: vec![batch, out_features],
        }
    }

    #[inline]
    pub fn activation_q8_enabled() -> bool {
        use_act_q8()
    }

    /// Quantize all activation rows of x (shape [batch, in_features]) into GGML Q8_0 blocks.
    pub fn prequantize_rows_q8(x: &Tensor, in_features: usize) -> Vec<u8> {
        assert_eq!(
            in_features % QK,
            0,
            "prequantize_rows_q8: K must be multiple of {}",
            QK
        );
        let batch = x.numel() / in_features;
        assert_eq!(
            x.numel(),
            batch * in_features,
            "prequantize_rows_q8: x size mismatch"
        );
        let bs = block_size(GGML_TYPE_Q8_0);
        let nb = in_features / QK;
        let rb = nb * bs;
        let mut out = vec![0u8; batch * rb];
        let x_data = &x.data;
        let out_ptr = SendPtrU8::new(out.as_mut_ptr());
        parallel_work_steal(batch, |b| {
            let x_row = &x_data[b * in_features..(b + 1) * in_features];
            let out_row = unsafe { std::slice::from_raw_parts_mut(out_ptr.ptr().add(b * rb), rb) };
            for blk in 0..nb {
                quantize_q8_0_block(
                    &x_row[blk * QK..(blk + 1) * QK],
                    &mut out_row[blk * bs..(blk + 1) * bs],
                );
            }
        });
        out
    }

    /// Matrix multiply with a quantized weight matrix.
    /// weight (RawTensor) is [out_features, in_features] (row-major quantized)
    /// x is [batch, in_features] (f32)
    /// result is [batch, out_features] (f32)
    /// This computes x @ weight^T (each weight row dots with each x row)
    pub fn matmul_raw(x: &Tensor, weight: &RawTensor) -> Tensor {
        Self::matmul_raw_with_prequant(x, weight, None)
    }

    /// Fused linear layer: x @ weight^T + bias.
    /// Uses fused Metal matmul+add when available.
    pub fn linear_raw(x: &Tensor, weight: &RawTensor, bias: &Tensor) -> Tensor {
        let in_features = weight.n_cols();
        let out_features = weight.n_rows();
        let batch = x.numel() / in_features;
        assert_eq!(
            x.numel(),
            batch * in_features,
            "linear_raw: x size mismatch"
        );

        if bias.numel() == out_features {
            if let Some(out) = crate::metal_backend::try_matmul_nt_ggml_bytes_add_bias(
                &x.data,
                &weight.data,
                weight.ggml_type,
                batch,
                in_features,
                out_features,
                &bias.data,
            ) {
                return Tensor {
                    data: out,
                    shape: vec![batch, out_features],
                };
            }
        }

        Tensor::add(&Self::matmul_raw(x, weight), bias)
    }

    /// Same as linear_raw but allows pre-quantized activation rows for CPU fallback path.
    pub fn linear_raw_with_prequant(
        x: &Tensor,
        weight: &RawTensor,
        xq_rows: Option<&[u8]>,
        bias: &Tensor,
    ) -> Tensor {
        let in_features = weight.n_cols();
        let out_features = weight.n_rows();
        let batch = x.numel() / in_features;
        assert_eq!(
            x.numel(),
            batch * in_features,
            "linear_raw_with_prequant: x size mismatch"
        );

        if bias.numel() == out_features {
            if let Some(out) = crate::metal_backend::try_matmul_nt_ggml_bytes_add_bias(
                &x.data,
                &weight.data,
                weight.ggml_type,
                batch,
                in_features,
                out_features,
                &bias.data,
            ) {
                return Tensor {
                    data: out,
                    shape: vec![batch, out_features],
                };
            }
        }

        Tensor::add(&Self::matmul_raw_with_prequant(x, weight, xq_rows), bias)
    }

    /// Same as matmul_raw but optionally accepts pre-quantized activation rows in Q8_0 layout.
    /// `xq_rows` is [batch, row_bytes] where row_bytes = (in_features/QK)*34.
    pub fn matmul_raw_with_prequant(
        x: &Tensor,
        weight: &RawTensor,
        xq_rows: Option<&[u8]>,
    ) -> Tensor {
        let _t = std::time::Instant::now();
        if std::env::var("MAKEPAD_VOICE_BACKEND")
            .ok()
            .map(|v| v.trim().eq_ignore_ascii_case("metal"))
            .unwrap_or(false)
        {
            static LOG_FIRST: std::sync::OnceLock<()> = std::sync::OnceLock::new();
            if LOG_FIRST.set(()).is_ok() {
                eprintln!(
                    "[voice][debug] matmul_raw first-call weight.ggml_type={} shape={:?}",
                    weight.ggml_type, weight.shape
                );
            }
        }
        let in_features = weight.n_cols();
        let out_features = weight.n_rows();
        let batch = x.numel() / in_features;
        assert_eq!(
            x.numel(),
            batch * in_features,
            "matmul_raw: x size mismatch"
        );

        if weight.ggml_type == GGML_TYPE_F32 {
            if let Some(out) = crate::metal_backend::try_matmul_nt_f32_bytes(
                &x.data,
                &weight.data,
                batch,
                in_features,
                out_features,
            ) {
                crate::PROF_MATMUL_RAW.fetch_add(
                    _t.elapsed().as_nanos() as u64,
                    std::sync::atomic::Ordering::Relaxed,
                );
                crate::PROF_MATMUL_RAW_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                return Tensor {
                    data: out,
                    shape: vec![batch, out_features],
                };
            }
        } else if weight.ggml_type == GGML_TYPE_F16 {
            if let Some(out) = crate::metal_backend::try_matmul_nt_f16_bytes(
                &x.data,
                &weight.data,
                batch,
                in_features,
                out_features,
            ) {
                crate::PROF_MATMUL_RAW.fetch_add(
                    _t.elapsed().as_nanos() as u64,
                    std::sync::atomic::Ordering::Relaxed,
                );
                crate::PROF_MATMUL_RAW_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                return Tensor {
                    data: out,
                    shape: vec![batch, out_features],
                };
            }
        } else if quant_gpu_enabled() {
            match weight.ggml_type {
                GGML_TYPE_Q4_0 | GGML_TYPE_Q4_1 | GGML_TYPE_Q5_0 | GGML_TYPE_Q5_1
                | GGML_TYPE_Q8_0 => {
                    if let Some(out) = crate::metal_backend::try_matmul_nt_ggml_bytes(
                        &x.data,
                        &weight.data,
                        weight.ggml_type,
                        batch,
                        in_features,
                        out_features,
                    ) {
                        crate::PROF_MATMUL_RAW.fetch_add(
                            _t.elapsed().as_nanos() as u64,
                            std::sync::atomic::Ordering::Relaxed,
                        );
                        crate::PROF_MATMUL_RAW_CALLS
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        return Tensor {
                            data: out,
                            shape: vec![batch, out_features],
                        };
                    }
                }
                _ => {}
            }
        }

        let mut out = vec![0.0f32; batch * out_features];

        match weight.ggml_type {
            GGML_TYPE_F32 => {
                let w = weight.to_f32();
                for b in 0..batch {
                    let x_row = &x.data[b * in_features..(b + 1) * in_features];
                    for o in 0..out_features {
                        let w_row = &w.data[o * in_features..(o + 1) * in_features];
                        out[b * out_features + o] = dot_f32(x_row, w_row);
                    }
                }
            }
            GGML_TYPE_F16 => {
                // Direct f16×f32 dot product — no pre-dequant, half the bandwidth
                let row_bytes = in_features * 2; // 2 bytes per f16
                let out_ptr = SendPtr::new(out.as_mut_ptr());
                let x_data = &x.data;
                let w_data = &weight.data;
                parallel_work_steal(out_features, |o| {
                    let w_row = &w_data[o * row_bytes..(o + 1) * row_bytes];
                    for b in 0..batch {
                        let x_row = &x_data[b * in_features..(b + 1) * in_features];
                        unsafe {
                            *out_ptr.ptr().add(b * out_features + o) = dot_f16_f32(w_row, x_row);
                        }
                    }
                });
            }
            GGML_TYPE_Q4_0 => {
                let bs = block_size(GGML_TYPE_Q4_0);
                let nb = in_features / QK;
                let rb = nb * bs;
                for b in 0..batch {
                    let x_row = &x.data[b * in_features..(b + 1) * in_features];
                    for o in 0..out_features {
                        let w_row = &weight.data[o * rb..];
                        let mut sum = 0.0f32;
                        for blk in 0..nb {
                            sum += vec_dot_q4_0_f32(&w_row[blk * bs..], &x_row[blk * QK..]);
                        }
                        out[b * out_features + o] = sum;
                    }
                }
            }
            GGML_TYPE_Q5_0 => {
                let bs = block_size(GGML_TYPE_Q5_0);
                let nb = in_features / QK;
                let rb = nb * bs;
                let out_ptr = SendPtr::new(out.as_mut_ptr());
                let x_data = &x.data;
                let w_data = &weight.data;
                let use_q8_act = use_act_q8() && in_features % QK == 0 && nb > 0;

                if use_q8_act && batch > 2 {
                    let xq_bs = block_size(GGML_TYPE_Q8_0);
                    let xq_rb = nb * xq_bs;

                    if let Some(xq_all) = xq_rows {
                        assert!(
                            xq_all.len() >= batch * xq_rb,
                            "matmul_raw_with_prequant: xq_rows too small: {} < {}",
                            xq_all.len(),
                            batch * xq_rb
                        );

                        parallel_work_steal(batch, |b| {
                            let xq = &xq_all[b * xq_rb..(b + 1) * xq_rb];
                            let out_row_ptr = unsafe { out_ptr.ptr().add(b * out_features) };
                            for o in 0..out_features {
                                let w_row = &w_data[o * rb..(o + 1) * rb];
                                let mut sum = 0.0f32;
                                let mut blk = 0;
                                while blk + 3 < nb {
                                    sum +=
                                        vec_dot_q5_0_q8_0(&w_row[blk * bs..], &xq[blk * xq_bs..]);
                                    sum += vec_dot_q5_0_q8_0(
                                        &w_row[(blk + 1) * bs..],
                                        &xq[(blk + 1) * xq_bs..],
                                    );
                                    sum += vec_dot_q5_0_q8_0(
                                        &w_row[(blk + 2) * bs..],
                                        &xq[(blk + 2) * xq_bs..],
                                    );
                                    sum += vec_dot_q5_0_q8_0(
                                        &w_row[(blk + 3) * bs..],
                                        &xq[(blk + 3) * xq_bs..],
                                    );
                                    blk += 4;
                                }
                                while blk < nb {
                                    sum +=
                                        vec_dot_q5_0_q8_0(&w_row[blk * bs..], &xq[blk * xq_bs..]);
                                    blk += 1;
                                }
                                unsafe {
                                    *out_row_ptr.add(o) = sum;
                                }
                            }
                        });
                    } else {
                        parallel_work_steal(batch, |b| {
                            let x_row = &x_data[b * in_features..(b + 1) * in_features];
                            let out_row_ptr = unsafe { out_ptr.ptr().add(b * out_features) };
                            Q8_ACT_SCRATCH.with(|scratch| {
                                let mut scratch = scratch.borrow_mut();
                                if scratch.len() < xq_rb {
                                    scratch.resize(xq_rb, 0);
                                }
                                let xq = &mut scratch[..xq_rb];
                                for blk in 0..nb {
                                    quantize_q8_0_block(
                                        &x_row[blk * QK..(blk + 1) * QK],
                                        &mut xq[blk * xq_bs..(blk + 1) * xq_bs],
                                    );
                                }

                                for o in 0..out_features {
                                    let w_row = &w_data[o * rb..(o + 1) * rb];
                                    let mut sum = 0.0f32;
                                    let mut blk = 0;
                                    while blk + 3 < nb {
                                        sum += vec_dot_q5_0_q8_0(
                                            &w_row[blk * bs..],
                                            &xq[blk * xq_bs..],
                                        );
                                        sum += vec_dot_q5_0_q8_0(
                                            &w_row[(blk + 1) * bs..],
                                            &xq[(blk + 1) * xq_bs..],
                                        );
                                        sum += vec_dot_q5_0_q8_0(
                                            &w_row[(blk + 2) * bs..],
                                            &xq[(blk + 2) * xq_bs..],
                                        );
                                        sum += vec_dot_q5_0_q8_0(
                                            &w_row[(blk + 3) * bs..],
                                            &xq[(blk + 3) * xq_bs..],
                                        );
                                        blk += 4;
                                    }
                                    while blk < nb {
                                        sum += vec_dot_q5_0_q8_0(
                                            &w_row[blk * bs..],
                                            &xq[blk * xq_bs..],
                                        );
                                        blk += 1;
                                    }
                                    unsafe {
                                        *out_row_ptr.add(o) = sum;
                                    }
                                }
                            });
                        });
                    }
                } else if batch > 2 {
                    parallel_work_steal(batch, |b| {
                        let x_row = &x_data[b * in_features..(b + 1) * in_features];
                        let out_row_ptr = unsafe { out_ptr.ptr().add(b * out_features) };
                        for o in 0..out_features {
                            let w_row = &w_data[o * rb..(o + 1) * rb];
                            let mut sum = 0.0f32;
                            let mut blk = 0;
                            while blk + 3 < nb {
                                sum += vec_dot_q5_0_f32(&w_row[blk * bs..], &x_row[blk * QK..]);
                                sum += vec_dot_q5_0_f32(
                                    &w_row[(blk + 1) * bs..],
                                    &x_row[(blk + 1) * QK..],
                                );
                                sum += vec_dot_q5_0_f32(
                                    &w_row[(blk + 2) * bs..],
                                    &x_row[(blk + 2) * QK..],
                                );
                                sum += vec_dot_q5_0_f32(
                                    &w_row[(blk + 3) * bs..],
                                    &x_row[(blk + 3) * QK..],
                                );
                                blk += 4;
                            }
                            while blk < nb {
                                sum += vec_dot_q5_0_f32(&w_row[blk * bs..], &x_row[blk * QK..]);
                                blk += 1;
                            }
                            unsafe {
                                *out_row_ptr.add(o) = sum;
                            }
                        }
                    });
                } else {
                    parallel_work_steal(out_features, |o| {
                        let w_row = &w_data[o * rb..(o + 1) * rb];
                        for b in 0..batch {
                            let x_row = &x_data[b * in_features..(b + 1) * in_features];
                            let mut sum = 0.0f32;
                            let mut blk = 0;
                            while blk + 3 < nb {
                                sum += vec_dot_q5_0_f32(&w_row[blk * bs..], &x_row[blk * QK..]);
                                sum += vec_dot_q5_0_f32(
                                    &w_row[(blk + 1) * bs..],
                                    &x_row[(blk + 1) * QK..],
                                );
                                sum += vec_dot_q5_0_f32(
                                    &w_row[(blk + 2) * bs..],
                                    &x_row[(blk + 2) * QK..],
                                );
                                sum += vec_dot_q5_0_f32(
                                    &w_row[(blk + 3) * bs..],
                                    &x_row[(blk + 3) * QK..],
                                );
                                blk += 4;
                            }
                            while blk < nb {
                                sum += vec_dot_q5_0_f32(&w_row[blk * bs..], &x_row[blk * QK..]);
                                blk += 1;
                            }
                            unsafe {
                                *out_ptr.ptr().add(b * out_features + o) = sum;
                            }
                        }
                    });
                }
            }
            GGML_TYPE_Q8_0 => {
                let bs = block_size(GGML_TYPE_Q8_0);
                let nb = in_features / QK;
                let rb = nb * bs;
                let out_ptr = SendPtr::new(out.as_mut_ptr());
                let x_data = &x.data;
                let w_data = &weight.data;
                let use_q8_act = use_act_q8() && in_features % QK == 0 && nb > 0;
                if use_q8_act && batch > 2 {
                    if let Some(xq_all) = xq_rows {
                        assert!(
                            xq_all.len() >= batch * rb,
                            "matmul_raw_with_prequant: xq_rows too small: {} < {}",
                            xq_all.len(),
                            batch * rb
                        );
                        parallel_work_steal(batch, |b| {
                            let xq = &xq_all[b * rb..(b + 1) * rb];
                            let out_row_ptr = unsafe { out_ptr.ptr().add(b * out_features) };
                            for o in 0..out_features {
                                let w_row = &w_data[o * rb..(o + 1) * rb];
                                let mut sum = 0.0f32;
                                let mut blk = 0;
                                while blk + 3 < nb {
                                    sum += vec_dot_q8_0_q8_0(&w_row[blk * bs..], &xq[blk * bs..]);
                                    sum += vec_dot_q8_0_q8_0(
                                        &w_row[(blk + 1) * bs..],
                                        &xq[(blk + 1) * bs..],
                                    );
                                    sum += vec_dot_q8_0_q8_0(
                                        &w_row[(blk + 2) * bs..],
                                        &xq[(blk + 2) * bs..],
                                    );
                                    sum += vec_dot_q8_0_q8_0(
                                        &w_row[(blk + 3) * bs..],
                                        &xq[(blk + 3) * bs..],
                                    );
                                    blk += 4;
                                }
                                while blk < nb {
                                    sum += vec_dot_q8_0_q8_0(&w_row[blk * bs..], &xq[blk * bs..]);
                                    blk += 1;
                                }
                                unsafe {
                                    *out_row_ptr.add(o) = sum;
                                }
                            }
                        });
                    } else {
                        // Aggressive CPU fast path: quantize activations to Q8 per-row once, then Q8xQ8 dots.
                        parallel_work_steal(batch, |b| {
                            let x_row = &x_data[b * in_features..(b + 1) * in_features];
                            let out_row_ptr = unsafe { out_ptr.ptr().add(b * out_features) };
                            Q8_ACT_SCRATCH.with(|scratch| {
                                let mut scratch = scratch.borrow_mut();
                                if scratch.len() < rb {
                                    scratch.resize(rb, 0);
                                }
                                let xq = &mut scratch[..rb];
                                for blk in 0..nb {
                                    quantize_q8_0_block(
                                        &x_row[blk * QK..(blk + 1) * QK],
                                        &mut xq[blk * bs..(blk + 1) * bs],
                                    );
                                }
                                for o in 0..out_features {
                                    let w_row = &w_data[o * rb..(o + 1) * rb];
                                    let mut sum = 0.0f32;
                                    let mut blk = 0;
                                    while blk + 3 < nb {
                                        sum +=
                                            vec_dot_q8_0_q8_0(&w_row[blk * bs..], &xq[blk * bs..]);
                                        sum += vec_dot_q8_0_q8_0(
                                            &w_row[(blk + 1) * bs..],
                                            &xq[(blk + 1) * bs..],
                                        );
                                        sum += vec_dot_q8_0_q8_0(
                                            &w_row[(blk + 2) * bs..],
                                            &xq[(blk + 2) * bs..],
                                        );
                                        sum += vec_dot_q8_0_q8_0(
                                            &w_row[(blk + 3) * bs..],
                                            &xq[(blk + 3) * bs..],
                                        );
                                        blk += 4;
                                    }
                                    while blk < nb {
                                        sum +=
                                            vec_dot_q8_0_q8_0(&w_row[blk * bs..], &xq[blk * bs..]);
                                        blk += 1;
                                    }
                                    unsafe {
                                        *out_row_ptr.add(o) = sum;
                                    }
                                }
                            });
                        });
                    }
                } else if batch > 2 {
                    // For larger batches, keep each x row hot in cache and sweep all output rows.
                    parallel_work_steal(batch, |b| {
                        let x_row = &x_data[b * in_features..(b + 1) * in_features];
                        let out_row_ptr = unsafe { out_ptr.ptr().add(b * out_features) };
                        for o in 0..out_features {
                            let w_row = &w_data[o * rb..(o + 1) * rb];
                            let mut sum = 0.0f32;
                            let mut blk = 0;
                            while blk + 3 < nb {
                                sum += vec_dot_q8_0_f32(&w_row[blk * bs..], &x_row[blk * QK..]);
                                sum += vec_dot_q8_0_f32(
                                    &w_row[(blk + 1) * bs..],
                                    &x_row[(blk + 1) * QK..],
                                );
                                sum += vec_dot_q8_0_f32(
                                    &w_row[(blk + 2) * bs..],
                                    &x_row[(blk + 2) * QK..],
                                );
                                sum += vec_dot_q8_0_f32(
                                    &w_row[(blk + 3) * bs..],
                                    &x_row[(blk + 3) * QK..],
                                );
                                blk += 4;
                            }
                            while blk < nb {
                                sum += vec_dot_q8_0_f32(&w_row[blk * bs..], &x_row[blk * QK..]);
                                blk += 1;
                            }
                            unsafe {
                                *out_row_ptr.add(o) = sum;
                            }
                        }
                    });
                } else {
                    // For tiny batches (decoder path), parallelize over output rows.
                    parallel_work_steal(out_features, |o| {
                        let w_row = &w_data[o * rb..(o + 1) * rb];
                        for b in 0..batch {
                            let x_row = &x_data[b * in_features..(b + 1) * in_features];
                            let mut sum = 0.0f32;
                            let mut blk = 0;
                            while blk + 3 < nb {
                                sum += vec_dot_q8_0_f32(&w_row[blk * bs..], &x_row[blk * QK..]);
                                sum += vec_dot_q8_0_f32(
                                    &w_row[(blk + 1) * bs..],
                                    &x_row[(blk + 1) * QK..],
                                );
                                sum += vec_dot_q8_0_f32(
                                    &w_row[(blk + 2) * bs..],
                                    &x_row[(blk + 2) * QK..],
                                );
                                sum += vec_dot_q8_0_f32(
                                    &w_row[(blk + 3) * bs..],
                                    &x_row[(blk + 3) * QK..],
                                );
                                blk += 4;
                            }
                            while blk < nb {
                                sum += vec_dot_q8_0_f32(&w_row[blk * bs..], &x_row[blk * QK..]);
                                blk += 1;
                            }
                            unsafe {
                                *out_ptr.ptr().add(b * out_features + o) = sum;
                            }
                        }
                    });
                }
            }
            _t => {
                // Fallback: dequantize the whole weight and do f32 matmul
                let w = weight.to_f32();
                for b in 0..batch {
                    let x_row = &x.data[b * in_features..(b + 1) * in_features];
                    for o in 0..out_features {
                        let w_row = &w.data[o * in_features..(o + 1) * in_features];
                        out[b * out_features + o] = dot_f32(x_row, w_row);
                    }
                }
            }
        }

        crate::PROF_MATMUL_RAW.fetch_add(
            _t.elapsed().as_nanos() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
        crate::PROF_MATMUL_RAW_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Tensor {
            data: out,
            shape: vec![batch, out_features],
        }
    }

    /// 1D convolution with padding=kernel_size/2 (same padding), given stride.
    /// input: [channels_in, length]
    /// weight: [channels_out, channels_in, kernel_size] (stored as RawTensor)
    /// bias: [channels_out] (f32 Tensor, shaped [1, channels_out])
    /// output: [channels_out, length / stride]
    pub fn conv1d(input: &Tensor, weight: &Tensor, bias: &Tensor, stride: usize) -> Tensor {
        let ch_out = weight.shape[0];
        let ch_in = weight.shape[1];
        let ksize = weight.shape[2];
        let in_len = input.shape[1];
        let pad = ksize / 2;
        let out_len = (in_len + 2 * pad - ksize) / stride + 1;

        if let Some(im2col) =
            crate::metal_backend::try_im2col_1d_f32(&input.data, ch_in, in_len, ksize, stride, pad)
        {
            let k = ch_in * ksize;
            if let Some(mm_out) =
                crate::metal_backend::try_matmul_nt_f32(&im2col, &weight.data, out_len, k, ch_out)
            {
                let mut out = vec![0.0f32; ch_out * out_len];
                let out_ptr = SendPtr::new(out.as_mut_ptr());
                let b_data = &bias.data;
                parallel_for(ch_out, |co| {
                    let b = b_data[co];
                    for t in 0..out_len {
                        unsafe {
                            *out_ptr.ptr().add(co * out_len + t) = mm_out[t * ch_out + co] + b;
                        }
                    }
                });

                return Tensor {
                    data: out,
                    shape: vec![ch_out, out_len],
                };
            }
        }

        let mut out = vec![0.0f32; ch_out * out_len];
        let out_ptr = SendPtr::new(out.as_mut_ptr());
        let in_data = &input.data;
        let w_data = &weight.data;
        let b_data = &bias.data;
        parallel_for(ch_out, |co| {
            let b = b_data[co];
            let w_co = &w_data[co * ch_in * ksize..(co + 1) * ch_in * ksize];
            for t in 0..out_len {
                let mut sum = b;
                let in_start = t * stride;
                for ci in 0..ch_in {
                    let in_row_off = ci * in_len;
                    let w_ci = &w_co[ci * ksize..(ci + 1) * ksize];
                    for k in 0..ksize {
                        let in_pos = in_start + k;
                        let in_pos = in_pos as isize - pad as isize;
                        if in_pos >= 0 && (in_pos as usize) < in_len {
                            sum += in_data[in_row_off + in_pos as usize] * w_ci[k];
                        }
                    }
                }
                unsafe {
                    *out_ptr.ptr().add(co * out_len + t) = sum;
                }
            }
        });

        Tensor {
            data: out,
            shape: vec![ch_out, out_len],
        }
    }

    /// Same as conv1d but with RawTensor weight (dequantize first)
    pub fn conv1d_raw(input: &Tensor, weight: &RawTensor, bias: &Tensor, stride: usize) -> Tensor {
        if weight.shape.len() == 3 {
            let ksize = weight.shape[0];
            let ch_in = weight.shape[1];
            let ch_out = weight.shape[2];
            let in_len = input.shape[1];
            let pad = ksize / 2;
            let out_len = (in_len + 2 * pad - ksize) / stride + 1;

            if let Some(im2col) = crate::metal_backend::try_im2col_1d_f32(
                &input.data,
                ch_in,
                in_len,
                ksize,
                stride,
                pad,
            ) {
                let k = ch_in * ksize;
                let mm_out = match weight.ggml_type {
                    GGML_TYPE_F32 => crate::metal_backend::try_matmul_nt_f32_bytes(
                        &im2col,
                        &weight.data,
                        out_len,
                        k,
                        ch_out,
                    ),
                    GGML_TYPE_F16 => crate::metal_backend::try_matmul_nt_f16_bytes(
                        &im2col,
                        &weight.data,
                        out_len,
                        k,
                        ch_out,
                    ),
                    GGML_TYPE_Q4_0 | GGML_TYPE_Q4_1 | GGML_TYPE_Q5_0 | GGML_TYPE_Q5_1
                    | GGML_TYPE_Q8_0
                        if quant_gpu_enabled() =>
                    {
                        crate::metal_backend::try_matmul_nt_ggml_bytes(
                            &im2col,
                            &weight.data,
                            weight.ggml_type,
                            out_len,
                            k,
                            ch_out,
                        )
                    }
                    _ => None,
                };

                if let Some(mm_out) = mm_out {
                    let mut out = vec![0.0f32; ch_out * out_len];
                    let out_ptr = SendPtr::new(out.as_mut_ptr());
                    let b_data = &bias.data;
                    parallel_for(ch_out, |co| {
                        let b = b_data[co];
                        for t in 0..out_len {
                            unsafe {
                                *out_ptr.ptr().add(co * out_len + t) = mm_out[t * ch_out + co] + b;
                            }
                        }
                    });
                    return Tensor {
                        data: out,
                        shape: vec![ch_out, out_len],
                    };
                }
            }
        }

        let w = weight.to_f32();
        Self::conv1d(input, &w, bias, stride)
    }
}
