#![allow(non_camel_case_types)]

use std::ffi::{c_char, c_int, c_uint, c_void, CStr};
use std::fmt;
use std::ptr::{self, NonNull};

pub type cudaError_t = c_int;
pub type cudaStream_t = *mut c_void;
pub type cudaEvent_t = *mut c_void;
pub type cudaGraph_t = *mut c_void;
pub type cudaGraphExec_t = *mut c_void;
pub type cudaStreamCaptureMode = c_int;
pub type cublasStatus_t = c_int;
pub type cublasHandle_t = *mut c_void;
pub type cublasOperation_t = c_int;
pub type cudaDataType = c_int;
pub type cublasComputeType_t = c_int;
pub type cublasGemmAlgo_t = c_int;
pub type cublasLtHandle_t = *mut c_void;
pub type cublasLtMatmulDesc_t = *mut c_void;
pub type cublasLtMatrixLayout_t = *mut c_void;
pub type cublasLtMatmulPreference_t = *mut c_void;
pub type cublasLtMatmulDescAttributes_t = c_int;
pub type cublasLtMatmulPreferenceAttributes_t = c_int;
pub type cublasLtEpilogue_t = c_uint;

/// cuBLASLt deliberately exposes its selected algorithm as a fixed-size,
/// semi-opaque value. Keep this definition byte-for-byte compatible with
/// `cublasLt.h`; callers may cache it only for the same cuBLAS version.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct cublasLtMatmulAlgo_t {
    pub data: [u64; 8],
}

/// Result record filled by `cublasLtMatmulAlgoGetHeuristic`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct cublasLtMatmulHeuristicResult_t {
    pub algo: cublasLtMatmulAlgo_t,
    pub workspace_size: usize,
    pub state: cublasStatus_t,
    pub waves_count: f32,
    pub reserved: [c_int; 4],
}

pub const CUDA_SUCCESS: cudaError_t = 0;
pub const CUDA_STREAM_NON_BLOCKING: c_uint = 1;
pub const CUDA_STREAM_CAPTURE_MODE_GLOBAL: cudaStreamCaptureMode = 0;
pub const CUDA_STREAM_CAPTURE_MODE_THREAD_LOCAL: cudaStreamCaptureMode = 1;
pub const CUDA_STREAM_CAPTURE_MODE_RELAXED: cudaStreamCaptureMode = 2;
pub const CUDA_HOST_ALLOC_MAPPED: c_uint = 2;

pub const CUDA_MEMCPY_HOST_TO_DEVICE: c_int = 1;
pub const CUDA_MEMCPY_DEVICE_TO_HOST: c_int = 2;
pub const CUBLAS_STATUS_SUCCESS: cublasStatus_t = 0;
pub const CUBLAS_OP_N: cublasOperation_t = 0;
pub const CUBLAS_OP_T: cublasOperation_t = 1;
pub const CUDA_R_32F: cudaDataType = 0;
pub const CUDA_R_16F: cudaDataType = 2;
pub const CUDA_R_16BF: cudaDataType = 14;
/// Signed FP8 E4M3 (torch float8_e4m3fn) — cuda.h `CUDA_R_8F_E4M3`.
pub const CUDA_R_8F_E4M3: cudaDataType = 28;
pub const CUBLAS_COMPUTE_16F: cublasComputeType_t = 64;
pub const CUBLAS_COMPUTE_32F: cublasComputeType_t = 68;
pub const CUBLAS_COMPUTE_32F_FAST_16BF: cublasComputeType_t = 75;
pub const CUBLAS_GEMM_DEFAULT: cublasGemmAlgo_t = -1;
pub const CUBLAS_GEMM_DEFAULT_TENSOR_OP: cublasGemmAlgo_t = 99;

// cuBLASLt descriptor constants from the CUDA 12 cublasLt.h ABI. These
// values have remained stable since the corresponding attributes landed.
pub const CUBLASLT_MATMUL_DESC_TRANSA: cublasLtMatmulDescAttributes_t = 3;
pub const CUBLASLT_MATMUL_DESC_TRANSB: cublasLtMatmulDescAttributes_t = 4;
pub const CUBLASLT_MATMUL_DESC_EPILOGUE: cublasLtMatmulDescAttributes_t = 7;
pub const CUBLASLT_MATMUL_DESC_BIAS_POINTER: cublasLtMatmulDescAttributes_t = 8;
/// Device f32 dequant-scale pointers for narrow-precision (FP8) matmuls:
/// D = alpha * a_scale * b_scale * (A x B). cublasLt.h enum values 17/18.
pub const CUBLASLT_MATMUL_DESC_A_SCALE_POINTER: cublasLtMatmulDescAttributes_t = 17;
pub const CUBLASLT_MATMUL_DESC_B_SCALE_POINTER: cublasLtMatmulDescAttributes_t = 18;
pub const CUBLASLT_EPILOGUE_BIAS: cublasLtEpilogue_t = 4;
pub const CUBLASLT_MATMUL_PREF_MAX_WORKSPACE_BYTES:
    cublasLtMatmulPreferenceAttributes_t = 1;
pub const CUBLASLT_MATMUL_PREF_MIN_ALIGNMENT_A_BYTES:
    cublasLtMatmulPreferenceAttributes_t = 5;
pub const CUBLASLT_MATMUL_PREF_MIN_ALIGNMENT_B_BYTES:
    cublasLtMatmulPreferenceAttributes_t = 6;
pub const CUBLASLT_MATMUL_PREF_MIN_ALIGNMENT_C_BYTES:
    cublasLtMatmulPreferenceAttributes_t = 7;
pub const CUBLASLT_MATMUL_PREF_MIN_ALIGNMENT_D_BYTES:
    cublasLtMatmulPreferenceAttributes_t = 8;

unsafe extern "C" {
    pub fn cudaGetDeviceCount(count: *mut c_int) -> cudaError_t;
    pub fn cudaMemGetInfo(free: *mut usize, total: *mut usize) -> cudaError_t;
    pub fn cudaSetDevice(device: c_int) -> cudaError_t;
    pub fn cudaGetDevice(device: *mut c_int) -> cudaError_t;
    pub fn cudaMalloc(dev_ptr: *mut *mut c_void, size: usize) -> cudaError_t;
    pub fn cudaFree(dev_ptr: *mut c_void) -> cudaError_t;
    pub fn cudaHostAlloc(host_ptr: *mut *mut c_void, size: usize, flags: c_uint) -> cudaError_t;
    pub fn cudaFreeHost(ptr: *mut c_void) -> cudaError_t;
    pub fn cudaHostGetDevicePointer(
        device_ptr: *mut *mut c_void,
        host_ptr: *mut c_void,
        flags: c_uint,
    ) -> cudaError_t;
    pub fn cudaMemcpyAsync(
        dst: *mut c_void,
        src: *const c_void,
        count: usize,
        kind: c_int,
        stream: cudaStream_t,
    ) -> cudaError_t;
    pub fn cudaMemsetAsync(
        dst: *mut c_void,
        value: c_int,
        count: usize,
        stream: cudaStream_t,
    ) -> cudaError_t;
    pub fn cudaMemcpy(
        dst: *mut c_void,
        src: *const c_void,
        count: usize,
        kind: c_int,
    ) -> cudaError_t;
    pub fn cudaStreamCreateWithFlags(stream: *mut cudaStream_t, flags: c_uint) -> cudaError_t;
    pub fn cudaStreamDestroy(stream: cudaStream_t) -> cudaError_t;
    pub fn cudaStreamSynchronize(stream: cudaStream_t) -> cudaError_t;
    pub fn cudaEventCreate(event: *mut cudaEvent_t) -> cudaError_t;
    pub fn cudaEventDestroy(event: cudaEvent_t) -> cudaError_t;
    pub fn cudaEventRecord(event: cudaEvent_t, stream: cudaStream_t) -> cudaError_t;
    pub fn cudaStreamWaitEvent(
        stream: cudaStream_t,
        event: cudaEvent_t,
        flags: c_uint,
    ) -> cudaError_t;
    pub fn cudaEventSynchronize(event: cudaEvent_t) -> cudaError_t;
    pub fn cudaEventElapsedTime(ms: *mut f32, start: cudaEvent_t, end: cudaEvent_t) -> cudaError_t;
    pub fn cudaStreamBeginCapture(stream: cudaStream_t, mode: cudaStreamCaptureMode)
        -> cudaError_t;
    pub fn cudaStreamEndCapture(stream: cudaStream_t, graph: *mut cudaGraph_t) -> cudaError_t;
    pub fn cudaDeviceSynchronize() -> cudaError_t;
    pub fn cudaGraphInstantiate(
        graph_exec: *mut cudaGraphExec_t,
        graph: cudaGraph_t,
        flags: u64,
    ) -> cudaError_t;
    pub fn cudaGraphLaunch(graph_exec: cudaGraphExec_t, stream: cudaStream_t) -> cudaError_t;
    pub fn cudaGraphDestroy(graph: cudaGraph_t) -> cudaError_t;
    pub fn cudaGraphExecDestroy(graph_exec: cudaGraphExec_t) -> cudaError_t;
    pub fn cudaGetErrorString(error: cudaError_t) -> *const c_char;

    pub fn cublasCreate_v2(handle: *mut cublasHandle_t) -> cublasStatus_t;
    pub fn cublasDestroy_v2(handle: cublasHandle_t) -> cublasStatus_t;
    pub fn cublasSetStream_v2(handle: cublasHandle_t, stream: cudaStream_t) -> cublasStatus_t;
    pub fn cublasSetWorkspace_v2(
        handle: cublasHandle_t,
        workspace: *mut c_void,
        workspace_size_in_bytes: usize,
    ) -> cublasStatus_t;
    pub fn cublasSgemm_v2(
        handle: cublasHandle_t,
        transa: cublasOperation_t,
        transb: cublasOperation_t,
        m: c_int,
        n: c_int,
        k: c_int,
        alpha: *const f32,
        a: *const f32,
        lda: c_int,
        b: *const f32,
        ldb: c_int,
        beta: *const f32,
        c: *mut f32,
        ldc: c_int,
    ) -> cublasStatus_t;
    pub fn cublasGemmStridedBatchedEx(
        handle: cublasHandle_t,
        transa: cublasOperation_t,
        transb: cublasOperation_t,
        m: c_int,
        n: c_int,
        k: c_int,
        alpha: *const c_void,
        a: *const c_void,
        atype: cudaDataType,
        lda: c_int,
        stride_a: i64,
        b: *const c_void,
        btype: cudaDataType,
        ldb: c_int,
        stride_b: i64,
        beta: *const c_void,
        c: *mut c_void,
        ctype: cudaDataType,
        ldc: c_int,
        stride_c: i64,
        batch_count: c_int,
        compute_type: cublasComputeType_t,
        algo: cublasGemmAlgo_t,
    ) -> cublasStatus_t;
    pub fn cublasGemmEx(
        handle: cublasHandle_t,
        transa: cublasOperation_t,
        transb: cublasOperation_t,
        m: c_int,
        n: c_int,
        k: c_int,
        alpha: *const c_void,
        a: *const c_void,
        atype: cudaDataType,
        lda: c_int,
        b: *const c_void,
        btype: cudaDataType,
        ldb: c_int,
        beta: *const c_void,
        c: *mut c_void,
        ctype: cudaDataType,
        ldc: c_int,
        compute_type: cublasComputeType_t,
        algo: cublasGemmAlgo_t,
    ) -> cublasStatus_t;

    pub fn cublasLtCreate(handle: *mut cublasLtHandle_t) -> cublasStatus_t;
    pub fn cublasLtDestroy(handle: cublasLtHandle_t) -> cublasStatus_t;
    pub fn cublasLtMatmulDescCreate(
        desc: *mut cublasLtMatmulDesc_t,
        compute_type: cublasComputeType_t,
        scale_type: cudaDataType,
    ) -> cublasStatus_t;
    pub fn cublasLtMatmulDescDestroy(desc: cublasLtMatmulDesc_t) -> cublasStatus_t;
    pub fn cublasLtMatmulDescSetAttribute(
        desc: cublasLtMatmulDesc_t,
        attr: cublasLtMatmulDescAttributes_t,
        value: *const c_void,
        size_in_bytes: usize,
    ) -> cublasStatus_t;
    pub fn cublasLtMatrixLayoutCreate(
        layout: *mut cublasLtMatrixLayout_t,
        data_type: cudaDataType,
        rows: u64,
        cols: u64,
        ld: i64,
    ) -> cublasStatus_t;
    pub fn cublasLtMatrixLayoutDestroy(layout: cublasLtMatrixLayout_t) -> cublasStatus_t;
    pub fn cublasLtMatmulPreferenceCreate(
        preference: *mut cublasLtMatmulPreference_t,
    ) -> cublasStatus_t;
    pub fn cublasLtMatmulPreferenceDestroy(
        preference: cublasLtMatmulPreference_t,
    ) -> cublasStatus_t;
    pub fn cublasLtMatmulPreferenceSetAttribute(
        preference: cublasLtMatmulPreference_t,
        attr: cublasLtMatmulPreferenceAttributes_t,
        value: *const c_void,
        size_in_bytes: usize,
    ) -> cublasStatus_t;
    pub fn cublasLtMatmulAlgoGetHeuristic(
        handle: cublasLtHandle_t,
        operation_desc: cublasLtMatmulDesc_t,
        a_desc: cublasLtMatrixLayout_t,
        b_desc: cublasLtMatrixLayout_t,
        c_desc: cublasLtMatrixLayout_t,
        d_desc: cublasLtMatrixLayout_t,
        preference: cublasLtMatmulPreference_t,
        requested_algo_count: c_int,
        heuristic_results: *mut cublasLtMatmulHeuristicResult_t,
        returned_algo_count: *mut c_int,
    ) -> cublasStatus_t;
    pub fn cublasLtMatmul(
        handle: cublasLtHandle_t,
        operation_desc: cublasLtMatmulDesc_t,
        alpha: *const c_void,
        a: *const c_void,
        a_desc: cublasLtMatrixLayout_t,
        b: *const c_void,
        b_desc: cublasLtMatrixLayout_t,
        beta: *const c_void,
        c: *const c_void,
        c_desc: cublasLtMatrixLayout_t,
        d: *mut c_void,
        d_desc: cublasLtMatrixLayout_t,
        algo: *const cublasLtMatmulAlgo_t,
        workspace: *mut c_void,
        workspace_size_in_bytes: usize,
        stream: cudaStream_t,
    ) -> cublasStatus_t;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CudaError {
    code: cudaError_t,
}

impl CudaError {
    pub fn code(self) -> cudaError_t {
        self.code
    }

    pub fn message(self) -> String {
        unsafe {
            let ptr = cudaGetErrorString(self.code);
            if ptr.is_null() {
                return format!("CUDA error {}", self.code);
            }
            CStr::from_ptr(ptr).to_string_lossy().into_owned()
        }
    }
}

impl fmt::Display for CudaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (code {})", self.message(), self.code)
    }
}

impl std::error::Error for CudaError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CublasError {
    code: cublasStatus_t,
}

impl CublasError {
    pub fn code(self) -> cublasStatus_t {
        self.code
    }
}

impl fmt::Display for CublasError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cuBLAS error code {}", self.code)
    }
}

impl std::error::Error for CublasError {}

#[inline]
pub fn check(status: cudaError_t) -> Result<(), CudaError> {
    if status == CUDA_SUCCESS {
        Ok(())
    } else {
        Err(CudaError { code: status })
    }
}

#[inline]
pub fn check_cublas(status: cublasStatus_t) -> Result<(), CublasError> {
    if status == CUBLAS_STATUS_SUCCESS {
        Ok(())
    } else {
        Err(CublasError { code: status })
    }
}

pub fn device_count() -> Result<i32, CudaError> {
    let mut count = 0;
    unsafe {
        check(cudaGetDeviceCount(&mut count))?;
    }
    Ok(count)
}

pub fn is_available() -> bool {
    device_count().is_ok_and(|count| count > 0)
}

/// (free, total) device memory in bytes for the current device.
pub fn mem_get_info() -> Result<(usize, usize), CudaError> {
    let mut free = 0usize;
    let mut total = 0usize;
    unsafe {
        check(cudaMemGetInfo(&mut free, &mut total))?;
    }
    Ok((free, total))
}

pub fn current_device() -> Result<i32, CudaError> {
    let mut device = 0;
    unsafe {
        check(cudaGetDevice(&mut device))?;
    }
    Ok(device)
}

pub fn set_device(device: i32) -> Result<(), CudaError> {
    unsafe { check(cudaSetDevice(device)) }
}

pub fn create_non_blocking_stream() -> Result<cudaStream_t, CudaError> {
    let mut stream = ptr::null_mut();
    unsafe {
        check(cudaStreamCreateWithFlags(
            &mut stream,
            CUDA_STREAM_NON_BLOCKING,
        ))?;
    }
    Ok(stream)
}

pub fn destroy_stream(stream: cudaStream_t) -> Result<(), CudaError> {
    unsafe { check(cudaStreamDestroy(stream)) }
}

pub fn synchronize_stream(stream: cudaStream_t) -> Result<(), CudaError> {
    unsafe { check(cudaStreamSynchronize(stream)) }
}

pub fn begin_stream_capture(
    stream: cudaStream_t,
    mode: cudaStreamCaptureMode,
) -> Result<(), CudaError> {
    unsafe { check(cudaStreamBeginCapture(stream, mode)) }
}

pub fn end_stream_capture(stream: cudaStream_t) -> Result<CudaGraph, CudaError> {
    let mut graph = ptr::null_mut();
    unsafe {
        check(cudaStreamEndCapture(stream, &mut graph))?;
    }
    Ok(CudaGraph { inner: graph })
}

pub fn cublas_create() -> Result<cublasHandle_t, CublasError> {
    let mut handle = ptr::null_mut();
    unsafe {
        check_cublas(cublasCreate_v2(&mut handle))?;
    }
    Ok(handle)
}

pub fn cublas_destroy(handle: cublasHandle_t) -> Result<(), CublasError> {
    unsafe { check_cublas(cublasDestroy_v2(handle)) }
}

pub fn cublas_set_stream(handle: cublasHandle_t, stream: cudaStream_t) -> Result<(), CublasError> {
    unsafe { check_cublas(cublasSetStream_v2(handle, stream)) }
}

pub fn cublas_set_workspace(
    handle: cublasHandle_t,
    workspace: NonNull<c_void>,
    workspace_size_in_bytes: usize,
) -> Result<(), CublasError> {
    unsafe {
        check_cublas(cublasSetWorkspace_v2(
            handle,
            workspace.as_ptr(),
            workspace_size_in_bytes,
        ))
    }
}

pub fn cublas_lt_create() -> Result<cublasLtHandle_t, CublasError> {
    let mut handle = ptr::null_mut();
    unsafe {
        check_cublas(cublasLtCreate(&mut handle))?;
    }
    Ok(handle)
}

pub fn cublas_lt_destroy(handle: cublasLtHandle_t) -> Result<(), CublasError> {
    unsafe { check_cublas(cublasLtDestroy(handle)) }
}

pub fn cublas_lt_matmul_desc_create(
    compute_type: cublasComputeType_t,
    scale_type: cudaDataType,
) -> Result<cublasLtMatmulDesc_t, CublasError> {
    let mut desc = ptr::null_mut();
    unsafe {
        check_cublas(cublasLtMatmulDescCreate(
            &mut desc,
            compute_type,
            scale_type,
        ))?;
    }
    Ok(desc)
}

pub fn cublas_lt_matmul_desc_destroy(
    desc: cublasLtMatmulDesc_t,
) -> Result<(), CublasError> {
    unsafe { check_cublas(cublasLtMatmulDescDestroy(desc)) }
}

/// Set a cuBLASLt operation attribute using the ABI size of `T`.
///
/// The cuBLASLt API validates that this size matches the selected attribute;
/// callers should use the exact documented type (`i32`, `u32`, or a device
/// pointer). Passing a mismatched type returns `CUBLAS_STATUS_INVALID_VALUE`.
pub fn cublas_lt_matmul_desc_set_attribute<T>(
    desc: cublasLtMatmulDesc_t,
    attr: cublasLtMatmulDescAttributes_t,
    value: &T,
) -> Result<(), CublasError> {
    unsafe {
        check_cublas(cublasLtMatmulDescSetAttribute(
            desc,
            attr,
            (value as *const T).cast::<c_void>(),
            std::mem::size_of::<T>(),
        ))
    }
}

pub fn cublas_lt_matrix_layout_create(
    data_type: cudaDataType,
    rows: u64,
    cols: u64,
    ld: i64,
) -> Result<cublasLtMatrixLayout_t, CublasError> {
    let mut layout = ptr::null_mut();
    unsafe {
        check_cublas(cublasLtMatrixLayoutCreate(
            &mut layout,
            data_type,
            rows,
            cols,
            ld,
        ))?;
    }
    Ok(layout)
}

pub fn cublas_lt_matrix_layout_destroy(
    layout: cublasLtMatrixLayout_t,
) -> Result<(), CublasError> {
    unsafe { check_cublas(cublasLtMatrixLayoutDestroy(layout)) }
}

pub fn cublas_lt_matmul_preference_create(
) -> Result<cublasLtMatmulPreference_t, CublasError> {
    let mut preference = ptr::null_mut();
    unsafe {
        check_cublas(cublasLtMatmulPreferenceCreate(&mut preference))?;
    }
    Ok(preference)
}

pub fn cublas_lt_matmul_preference_destroy(
    preference: cublasLtMatmulPreference_t,
) -> Result<(), CublasError> {
    unsafe { check_cublas(cublasLtMatmulPreferenceDestroy(preference)) }
}

/// Set a cuBLASLt heuristic preference using the ABI size of `T`.
pub fn cublas_lt_matmul_preference_set_attribute<T>(
    preference: cublasLtMatmulPreference_t,
    attr: cublasLtMatmulPreferenceAttributes_t,
    value: &T,
) -> Result<(), CublasError> {
    unsafe {
        check_cublas(cublasLtMatmulPreferenceSetAttribute(
            preference,
            attr,
            (value as *const T).cast::<c_void>(),
            std::mem::size_of::<T>(),
        ))
    }
}

#[allow(clippy::too_many_arguments)]
pub fn cublas_lt_matmul_algo_get_heuristic(
    handle: cublasLtHandle_t,
    operation_desc: cublasLtMatmulDesc_t,
    a_desc: cublasLtMatrixLayout_t,
    b_desc: cublasLtMatrixLayout_t,
    c_desc: cublasLtMatrixLayout_t,
    d_desc: cublasLtMatrixLayout_t,
    preference: cublasLtMatmulPreference_t,
) -> Result<Option<cublasLtMatmulHeuristicResult_t>, CublasError> {
    let mut result = cublasLtMatmulHeuristicResult_t::default();
    let mut returned = 0;
    unsafe {
        check_cublas(cublasLtMatmulAlgoGetHeuristic(
            handle,
            operation_desc,
            a_desc,
            b_desc,
            c_desc,
            d_desc,
            preference,
            1,
            &mut result,
            &mut returned,
        ))?;
    }
    Ok((returned != 0).then_some(result))
}

#[allow(clippy::too_many_arguments)]
pub unsafe fn cublas_lt_matmul(
    handle: cublasLtHandle_t,
    operation_desc: cublasLtMatmulDesc_t,
    alpha: *const c_void,
    a: *const c_void,
    a_desc: cublasLtMatrixLayout_t,
    b: *const c_void,
    b_desc: cublasLtMatrixLayout_t,
    beta: *const c_void,
    c: *const c_void,
    c_desc: cublasLtMatrixLayout_t,
    d: *mut c_void,
    d_desc: cublasLtMatrixLayout_t,
    algo: &cublasLtMatmulAlgo_t,
    workspace: *mut c_void,
    workspace_size_in_bytes: usize,
    stream: cudaStream_t,
) -> Result<(), CublasError> {
    unsafe {
        check_cublas(cublasLtMatmul(
            handle,
            operation_desc,
            alpha,
            a,
            a_desc,
            b,
            b_desc,
            beta,
            c,
            c_desc,
            d,
            d_desc,
            algo,
            workspace,
            workspace_size_in_bytes,
            stream,
        ))
    }
}

/// Match PyTorch's cuBLASLt heuristic alignment calculation: report the
/// largest power-of-two alignment up to 256 bytes for a device pointer.
pub fn cublas_lt_pointer_alignment(pointer: *const c_void) -> u32 {
    let address = pointer as usize;
    let mut alignment = 256u32;
    loop {
        if address % alignment as usize == 0 {
            return alignment;
        }
        alignment /= 2;
    }
}

#[allow(clippy::too_many_arguments)]
pub fn cublas_sgemm(
    handle: cublasHandle_t,
    transa: cublasOperation_t,
    transb: cublasOperation_t,
    m: i32,
    n: i32,
    k: i32,
    alpha: &f32,
    a: *const f32,
    lda: i32,
    b: *const f32,
    ldb: i32,
    beta: &f32,
    c: *mut f32,
    ldc: i32,
) -> Result<(), CublasError> {
    unsafe {
        check_cublas(cublasSgemm_v2(
            handle, transa, transb, m, n, k, alpha, a, lda, b, ldb, beta, c, ldc,
        ))
    }
}

#[allow(clippy::too_many_arguments)]
pub unsafe fn cublas_gemm_strided_batched_ex(
    handle: cublasHandle_t,
    transa: cublasOperation_t,
    transb: cublasOperation_t,
    m: i32,
    n: i32,
    k: i32,
    alpha: &f32,
    a: *const c_void,
    atype: cudaDataType,
    lda: i32,
    stride_a: i64,
    b: *const c_void,
    btype: cudaDataType,
    ldb: i32,
    stride_b: i64,
    beta: &f32,
    c: *mut c_void,
    ctype: cudaDataType,
    ldc: i32,
    stride_c: i64,
    batch_count: i32,
    compute_type: cublasComputeType_t,
    algo: cublasGemmAlgo_t,
) -> Result<(), CublasError> {
    unsafe {
        check_cublas(cublasGemmStridedBatchedEx(
            handle,
            transa,
            transb,
            m,
            n,
            k,
            alpha as *const f32 as *const c_void,
            a,
            atype,
            lda,
            stride_a,
            b,
            btype,
            ldb,
            stride_b,
            beta as *const f32 as *const c_void,
            c,
            ctype,
            ldc,
            stride_c,
            batch_count,
            compute_type,
            algo,
        ))
    }
}

/// Direct `cublasGemmEx` entry point used when a framework's observable
/// numeric contract differs from the nominally equivalent batch-count-one
/// `cublasGemmStridedBatchedEx` call.
#[allow(clippy::too_many_arguments)]
pub unsafe fn cublas_gemm_ex(
    handle: cublasHandle_t,
    transa: cublasOperation_t,
    transb: cublasOperation_t,
    m: i32,
    n: i32,
    k: i32,
    alpha: &f32,
    a: *const c_void,
    atype: cudaDataType,
    lda: i32,
    b: *const c_void,
    btype: cudaDataType,
    ldb: i32,
    beta: &f32,
    c: *mut c_void,
    ctype: cudaDataType,
    ldc: i32,
    compute_type: cublasComputeType_t,
    algo: cublasGemmAlgo_t,
) -> Result<(), CublasError> {
    unsafe {
        check_cublas(cublasGemmEx(
            handle,
            transa,
            transb,
            m,
            n,
            k,
            alpha as *const f32 as *const c_void,
            a,
            atype,
            lda,
            b,
            btype,
            ldb,
            beta as *const f32 as *const c_void,
            c,
            ctype,
            ldc,
            compute_type,
            algo,
        ))
    }
}

/// cublasGemmStridedBatchedEx with caller-typed alpha/beta scalars: compute
/// types whose scaling factors are not f32 (e.g. CUBLAS_COMPUTE_16F wants
/// __half alpha/beta) need raw pointers instead of the &f32 of the wrapper
/// above.
#[allow(clippy::too_many_arguments)]
pub unsafe fn cublas_gemm_strided_batched_ex_raw(
    handle: cublasHandle_t,
    transa: cublasOperation_t,
    transb: cublasOperation_t,
    m: i32,
    n: i32,
    k: i32,
    alpha: *const c_void,
    a: *const c_void,
    atype: cudaDataType,
    lda: i32,
    stride_a: i64,
    b: *const c_void,
    btype: cudaDataType,
    ldb: i32,
    stride_b: i64,
    beta: *const c_void,
    c: *mut c_void,
    ctype: cudaDataType,
    ldc: i32,
    stride_c: i64,
    batch_count: i32,
    compute_type: cublasComputeType_t,
    algo: cublasGemmAlgo_t,
) -> Result<(), CublasError> {
    unsafe {
        check_cublas(cublasGemmStridedBatchedEx(
            handle,
            transa,
            transb,
            m,
            n,
            k,
            alpha,
            a,
            atype,
            lda,
            stride_a,
            b,
            btype,
            ldb,
            stride_b,
            beta,
            c,
            ctype,
            ldc,
            stride_c,
            batch_count,
            compute_type,
            algo,
        ))
    }
}

pub fn device_synchronize() -> Result<(), CudaError> {
    unsafe { check(cudaDeviceSynchronize()) }
}

pub unsafe fn malloc(size: usize) -> Result<NonNull<c_void>, CudaError> {
    let mut ptr = ptr::null_mut();
    check(cudaMalloc(&mut ptr, size))?;
    NonNull::new(ptr).ok_or(CudaError { code: -1 })
}

pub unsafe fn free(ptr: NonNull<c_void>) -> Result<(), CudaError> {
    check(cudaFree(ptr.as_ptr()))
}

pub unsafe fn host_alloc_mapped(size: usize) -> Result<NonNull<c_void>, CudaError> {
    let mut ptr = ptr::null_mut();
    check(cudaHostAlloc(&mut ptr, size, CUDA_HOST_ALLOC_MAPPED))?;
    NonNull::new(ptr).ok_or(CudaError { code: -1 })
}

/// Plain pinned (page-locked) host memory for fast async H2D staging.
pub unsafe fn host_alloc_pinned(size: usize) -> Result<NonNull<c_void>, CudaError> {
    let mut ptr = ptr::null_mut();
    check(cudaHostAlloc(&mut ptr, size, 0))?;
    NonNull::new(ptr).ok_or(CudaError { code: -1 })
}

pub fn event_create() -> Result<cudaEvent_t, CudaError> {
    let mut event: cudaEvent_t = ptr::null_mut();
    unsafe { check(cudaEventCreate(&mut event))? };
    Ok(event)
}

pub fn event_destroy(event: cudaEvent_t) -> Result<(), CudaError> {
    unsafe { check(cudaEventDestroy(event)) }
}

pub fn event_record(event: cudaEvent_t, stream: cudaStream_t) -> Result<(), CudaError> {
    unsafe { check(cudaEventRecord(event, stream)) }
}

/// Make `stream` wait for `event` (cross-stream ordering, no host sync).
pub fn stream_wait_event(stream: cudaStream_t, event: cudaEvent_t) -> Result<(), CudaError> {
    unsafe { check(cudaStreamWaitEvent(stream, event, 0)) }
}

pub unsafe fn free_host(ptr: NonNull<c_void>) -> Result<(), CudaError> {
    check(cudaFreeHost(ptr.as_ptr()))
}

pub unsafe fn host_get_device_pointer(
    host_ptr: NonNull<c_void>,
) -> Result<NonNull<c_void>, CudaError> {
    let mut device_ptr = ptr::null_mut();
    check(cudaHostGetDevicePointer(
        &mut device_ptr,
        host_ptr.as_ptr(),
        0,
    ))?;
    NonNull::new(device_ptr).ok_or(CudaError { code: -1 })
}

pub unsafe fn memcpy_async_host_to_device(
    dst: NonNull<c_void>,
    src: *const c_void,
    size: usize,
    stream: cudaStream_t,
) -> Result<(), CudaError> {
    check(cudaMemcpyAsync(
        dst.as_ptr(),
        src,
        size,
        CUDA_MEMCPY_HOST_TO_DEVICE,
        stream,
    ))
}

pub unsafe fn memcpy_async_device_to_host(
    dst: *mut c_void,
    src: NonNull<c_void>,
    size: usize,
    stream: cudaStream_t,
) -> Result<(), CudaError> {
    check(cudaMemcpyAsync(
        dst,
        src.as_ptr(),
        size,
        CUDA_MEMCPY_DEVICE_TO_HOST,
        stream,
    ))
}

pub unsafe fn memset_async(
    dst: NonNull<c_void>,
    value: c_int,
    size: usize,
    stream: cudaStream_t,
) -> Result<(), CudaError> {
    check(cudaMemsetAsync(dst.as_ptr(), value, size, stream))
}

pub struct CudaGraph {
    inner: cudaGraph_t,
}

impl CudaGraph {
    pub fn instantiate(self) -> Result<CudaGraphExec, CudaError> {
        let mut graph_exec = ptr::null_mut();
        unsafe {
            check(cudaGraphInstantiate(&mut graph_exec, self.inner, 0))?;
        }
        Ok(CudaGraphExec { inner: graph_exec })
    }
}

impl Drop for CudaGraph {
    fn drop(&mut self) {
        let _ = unsafe { check(cudaGraphDestroy(self.inner)) };
    }
}

pub struct CudaGraphExec {
    inner: cudaGraphExec_t,
}

impl CudaGraphExec {
    pub fn launch(&self, stream: cudaStream_t) -> Result<(), CudaError> {
        unsafe { check(cudaGraphLaunch(self.inner, stream)) }
    }
}

impl Drop for CudaGraphExec {
    fn drop(&mut self) {
        let _ = unsafe { check(cudaGraphExecDestroy(self.inner)) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cublas_lt_abi_records_match_cuda_header() {
        assert_eq!(std::mem::size_of::<cublasLtMatmulAlgo_t>(), 64);
        assert_eq!(std::mem::align_of::<cublasLtMatmulAlgo_t>(), 8);
        assert_eq!(std::mem::size_of::<cublasLtMatmulHeuristicResult_t>(), 96);
        assert_eq!(std::mem::align_of::<cublasLtMatmulHeuristicResult_t>(), 8);
    }

    #[test]
    fn cublas_lt_pointer_alignment_matches_pytorch_contract() {
        assert_eq!(cublas_lt_pointer_alignment(0x1000usize as *const c_void), 256);
        assert_eq!(cublas_lt_pointer_alignment(0x1080usize as *const c_void), 128);
        assert_eq!(cublas_lt_pointer_alignment(0x1040usize as *const c_void), 64);
        assert_eq!(cublas_lt_pointer_alignment(0x1002usize as *const c_void), 2);
        assert_eq!(cublas_lt_pointer_alignment(0x1001usize as *const c_void), 1);
    }

    #[test]
    fn cublas_default_tensor_op_matches_cuda_header() {
        assert_eq!(CUBLAS_GEMM_DEFAULT_TENSOR_OP, 99);
    }
}
