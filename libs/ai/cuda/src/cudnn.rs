//! Dynamic cuDNN loader. Same library PyTorch uses; no new kernels.

use crate::{cudaStream_t, CUDA_SUCCESS};
use std::ffi::{c_char, c_int, c_void, CString};
use std::ptr;
use std::sync::OnceLock;

pub type cudnnHandle_t = *mut c_void;
pub type cudnnTensorDescriptor_t = *mut c_void;
pub type cudnnFilterDescriptor_t = *mut c_void;
pub type cudnnConvolutionDescriptor_t = *mut c_void;
pub type cudnnStatus_t = c_int;
pub type cudnnDataType_t = c_int;
pub type cudnnTensorFormat_t = c_int;
pub type cudnnConvolutionMode_t = c_int;
pub type cudnnConvolutionFwdAlgo_t = c_int;
pub type cudnnMathType_t = c_int;
pub type cudnnDeterminism_t = c_int;

pub const CUDNN_STATUS_SUCCESS: cudnnStatus_t = 0;
pub const CUDNN_DATA_FLOAT: cudnnDataType_t = 0;
pub const CUDNN_DATA_HALF: cudnnDataType_t = 2;
pub const CUDNN_TENSOR_NCHW: cudnnTensorFormat_t = 0;
pub const CUDNN_TENSOR_NHWC: cudnnTensorFormat_t = 1;
pub const CUDNN_CROSS_CORRELATION: cudnnConvolutionMode_t = 1;
pub const CUDNN_CONVOLUTION_FWD_ALGO_IMPLICIT_PRECOMP_GEMM: cudnnConvolutionFwdAlgo_t = 1;
pub const CUDNN_CONVOLUTION_FWD_ALGO_WINOGRAD: cudnnConvolutionFwdAlgo_t = 6;
pub const CUDNN_CONVOLUTION_FWD_ALGO_WINOGRAD_NONFUSED: cudnnConvolutionFwdAlgo_t = 7;
pub const CUDNN_TENSOR_OP_MATH: cudnnMathType_t = 1;
pub const CUDNN_TENSOR_OP_MATH_ALLOW_CONVERSION: cudnnMathType_t = 2;
pub const CUDNN_FMA_MATH: cudnnMathType_t = 3;
pub const CUDNN_ACTIVATION_IDENTITY: c_int = 5;
pub const CUDNN_ACTIVATION_SWISH: c_int = 6;
pub const CUDNN_NORM_PER_CHANNEL: c_int = 1;
pub const CUDNN_NORM_OPS_NORM: c_int = 0;
pub const CUDNN_NORM_ALGO_STANDARD: c_int = 0;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct cudnnConvolutionFwdAlgoPerf_t {
    pub algo: cudnnConvolutionFwdAlgo_t,
    pub status: cudnnStatus_t,
    pub time: f32,
    pub memory: usize,
    pub determinism: cudnnDeterminism_t,
    pub mathType: cudnnMathType_t,
    pub reserved: [c_int; 3],
}

type FnCreate = unsafe extern "C" fn(*mut cudnnHandle_t) -> cudnnStatus_t;
type FnDestroy = unsafe extern "C" fn(cudnnHandle_t) -> cudnnStatus_t;
type FnSetStream = unsafe extern "C" fn(cudnnHandle_t, cudaStream_t) -> cudnnStatus_t;
type FnCreateTensor = unsafe extern "C" fn(*mut cudnnTensorDescriptor_t) -> cudnnStatus_t;
type FnDestroyTensor = unsafe extern "C" fn(cudnnTensorDescriptor_t) -> cudnnStatus_t;
type FnSetTensor4d = unsafe extern "C" fn(
    cudnnTensorDescriptor_t,
    cudnnTensorFormat_t,
    cudnnDataType_t,
    c_int,
    c_int,
    c_int,
    c_int,
) -> cudnnStatus_t;
type FnSetTensorNd = unsafe extern "C" fn(
    cudnnTensorDescriptor_t,
    cudnnDataType_t,
    c_int,
    *const c_int,
    *const c_int,
) -> cudnnStatus_t;
type FnCreateFilter = unsafe extern "C" fn(*mut cudnnFilterDescriptor_t) -> cudnnStatus_t;
type FnDestroyFilter = unsafe extern "C" fn(cudnnFilterDescriptor_t) -> cudnnStatus_t;
type FnSetFilter4d = unsafe extern "C" fn(
    cudnnFilterDescriptor_t,
    cudnnDataType_t,
    cudnnTensorFormat_t,
    c_int,
    c_int,
    c_int,
    c_int,
) -> cudnnStatus_t;
type FnCreateConv = unsafe extern "C" fn(*mut cudnnConvolutionDescriptor_t) -> cudnnStatus_t;
type FnDestroyConv = unsafe extern "C" fn(cudnnConvolutionDescriptor_t) -> cudnnStatus_t;
type FnSetConv2d = unsafe extern "C" fn(
    cudnnConvolutionDescriptor_t,
    c_int,
    c_int,
    c_int,
    c_int,
    c_int,
    c_int,
    cudnnConvolutionMode_t,
    cudnnDataType_t,
) -> cudnnStatus_t;
type FnSetConvMath = unsafe extern "C" fn(
    cudnnConvolutionDescriptor_t,
    cudnnMathType_t,
) -> cudnnStatus_t;
type FnGetAlgoV7 = unsafe extern "C" fn(
    cudnnHandle_t,
    cudnnTensorDescriptor_t,
    cudnnFilterDescriptor_t,
    cudnnConvolutionDescriptor_t,
    cudnnTensorDescriptor_t,
    c_int,
    *mut c_int,
    *mut cudnnConvolutionFwdAlgoPerf_t,
) -> cudnnStatus_t;
type FnGetWorkspace = unsafe extern "C" fn(
    cudnnHandle_t,
    cudnnTensorDescriptor_t,
    cudnnFilterDescriptor_t,
    cudnnConvolutionDescriptor_t,
    cudnnTensorDescriptor_t,
    cudnnConvolutionFwdAlgo_t,
    *mut usize,
) -> cudnnStatus_t;
type FnConvForward = unsafe extern "C" fn(
    cudnnHandle_t,
    *const c_void,
    cudnnTensorDescriptor_t,
    *const c_void,
    cudnnFilterDescriptor_t,
    *const c_void,
    cudnnConvolutionDescriptor_t,
    cudnnConvolutionFwdAlgo_t,
    *mut c_void,
    usize,
    *const c_void,
    cudnnTensorDescriptor_t,
    *mut c_void,
) -> cudnnStatus_t;
type FnTransformTensor = unsafe extern "C" fn(
    cudnnHandle_t,
    *const c_void,
    cudnnTensorDescriptor_t,
    *const c_void,
    *const c_void,
    cudnnTensorDescriptor_t,
    *mut c_void,
) -> cudnnStatus_t;
type FnGetErrorString = unsafe extern "C" fn(cudnnStatus_t) -> *const c_char;
type FnCreateAct = unsafe extern "C" fn(*mut *mut c_void) -> cudnnStatus_t;
type FnSetAct = unsafe extern "C" fn(*mut c_void, c_int, c_int, f64) -> cudnnStatus_t;
type FnDestroyAct = unsafe extern "C" fn(*mut c_void) -> cudnnStatus_t;
type FnActForward = unsafe extern "C" fn(
    cudnnHandle_t,
    *mut c_void,
    *const c_void,
    cudnnTensorDescriptor_t,
    *const c_void,
    *const c_void,
    cudnnTensorDescriptor_t,
    *mut c_void,
) -> cudnnStatus_t;
type FnConvBiasAct = unsafe extern "C" fn(
    cudnnHandle_t,
    *const c_void,
    cudnnTensorDescriptor_t,
    *const c_void,
    cudnnFilterDescriptor_t,
    *const c_void,
    cudnnConvolutionDescriptor_t,
    cudnnConvolutionFwdAlgo_t,
    *mut c_void,
    usize,
    *const c_void,
    cudnnTensorDescriptor_t,
    *const c_void,
    cudnnTensorDescriptor_t,
    *const c_void,
    *mut c_void,
    cudnnTensorDescriptor_t,
    *mut c_void,
) -> cudnnStatus_t;
type FnAddTensor = unsafe extern "C" fn(
    cudnnHandle_t,
    *const c_void,
    cudnnTensorDescriptor_t,
    *const c_void,
    *const c_void,
    cudnnTensorDescriptor_t,
    *mut c_void,
) -> cudnnStatus_t;
type FnNormFwdInf = unsafe extern "C" fn(
    cudnnHandle_t,
    c_int,
    c_int,
    c_int,
    *const c_void,
    *const c_void,
    cudnnTensorDescriptor_t,
    *const c_void,
    cudnnTensorDescriptor_t,
    *const c_void,
    *const c_void,
    cudnnTensorDescriptor_t,
    *const c_void,
    *const c_void,
    cudnnTensorDescriptor_t,
    *const c_void,
    *mut c_void,
    cudnnTensorDescriptor_t,
    *mut c_void,
    f64,
    c_int,
) -> cudnnStatus_t;
type FnBnFwdTrain = unsafe extern "C" fn(
    cudnnHandle_t,
    c_int,
    *const c_void,
    *const c_void,
    cudnnTensorDescriptor_t,
    *const c_void,
    cudnnTensorDescriptor_t,
    *mut c_void,
    cudnnTensorDescriptor_t,
    *const c_void,
    *const c_void,
    f64,
    *mut c_void,
    *mut c_void,
    f64,
    *mut c_void,
    *mut c_void,
) -> cudnnStatus_t;
type FnCreateOp = unsafe extern "C" fn(*mut *mut c_void) -> cudnnStatus_t;
type FnSetOp = unsafe extern "C" fn(*mut c_void, c_int, cudnnDataType_t, c_int) -> cudnnStatus_t;
type FnDestroyOp = unsafe extern "C" fn(*mut c_void) -> cudnnStatus_t;
type FnOpTensor = unsafe extern "C" fn(
    cudnnHandle_t,
    *mut c_void,
    *const c_void,
    cudnnTensorDescriptor_t,
    *const c_void,
    *const c_void,
    cudnnTensorDescriptor_t,
    *const c_void,
    *const c_void,
    cudnnTensorDescriptor_t,
    *mut c_void,
) -> cudnnStatus_t;

pub const CUDNN_BATCHNORM_SPATIAL: c_int = 1;
pub const CUDNN_OP_TENSOR_MUL: c_int = 1;
pub const CUDNN_NOT_PROPAGATE_NAN: c_int = 0;

struct CudnnApi {
    create: FnCreate,
    destroy: FnDestroy,
    set_stream: FnSetStream,
    create_tensor: FnCreateTensor,
    destroy_tensor: FnDestroyTensor,
    set_tensor4d: FnSetTensor4d,
    set_tensor_nd: FnSetTensorNd,
    create_filter: FnCreateFilter,
    destroy_filter: FnDestroyFilter,
    set_filter4d: FnSetFilter4d,
    create_conv: FnCreateConv,
    destroy_conv: FnDestroyConv,
    set_conv2d: FnSetConv2d,
    set_conv_math: FnSetConvMath,
    get_algo_v7: FnGetAlgoV7,
    find_algo: Option<FnGetAlgoV7>,
    get_workspace: FnGetWorkspace,
    conv_forward: FnConvForward,
    conv_bias_act: Option<FnConvBiasAct>,
    create_act: Option<FnCreateAct>,
    set_act: Option<FnSetAct>,
    destroy_act: Option<FnDestroyAct>,
    act_forward: Option<FnActForward>,
    transform_tensor: FnTransformTensor,
    add_tensor: FnAddTensor,
    norm_fwd_inf: Option<FnNormFwdInf>,
    bn_fwd_train: Option<FnBnFwdTrain>,
    create_op: Option<FnCreateOp>,
    set_op: Option<FnSetOp>,
    destroy_op: Option<FnDestroyOp>,
    op_tensor: Option<FnOpTensor>,
    get_error_string: FnGetErrorString,
}

fn status_err(api: &CudnnApi, status: cudnnStatus_t, what: &str) -> String {
    let ptr = unsafe { (api.get_error_string)(status) };
    let msg = if ptr.is_null() {
        format!("cudnn status {status}")
    } else {
        unsafe { std::ffi::CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned()
    };
    format!("{what}: {msg} ({status})")
}

fn check(api: &CudnnApi, status: cudnnStatus_t, what: &str) -> Result<(), String> {
    if status == CUDNN_STATUS_SUCCESS {
        Ok(())
    } else {
        Err(status_err(api, status, what))
    }
}

struct Lib(*mut c_void);
unsafe impl Send for Lib {}
unsafe impl Sync for Lib {}

static CUDNN: OnceLock<Result<(Lib, CudnnApi), String>> = OnceLock::new();

fn load_sym(lib: *mut c_void, name: &str) -> Result<*mut c_void, String> {
    let ptr = unsafe { dynlib::sym(lib, name) };
    if ptr.is_null() {
        Err(format!("cuDNN missing symbol {name}"))
    } else {
        Ok(ptr)
    }
}

unsafe fn load_ops_sym(lib: *mut c_void, path: &str, name: &str) -> Option<*mut c_void> {
    if let Ok(ptr) = load_sym(lib, name) {
        return Some(ptr);
    }
    let ops = std::path::Path::new(path)
        .parent()
        .map(|dir| dir.join("cudnn_ops64_9.dll"));
    if let Some(ops) = ops {
        let ops_s = ops.to_string_lossy();
        let extra = dynlib::load(&ops_s);
        if !extra.is_null() {
            if let Ok(ptr) = load_sym(extra, name) {
                return Some(ptr);
            }
        }
    }
    None
}

unsafe fn load_act_forward(lib: *mut c_void, path: &str) -> Option<FnActForward> {
    load_ops_sym(lib, path, "cudnnActivationForward").map(|ptr| std::mem::transmute(ptr))
}

unsafe fn load_norm_fwd_inf(lib: *mut c_void, path: &str) -> Option<FnNormFwdInf> {
    if let Ok(ptr) = load_sym(lib, "cudnnNormalizationForwardInference") {
        return Some(std::mem::transmute(ptr));
    }
    // cuDNN 9 splits ops into cudnn_ops64_9.dll next to the stub.
    let ops = std::path::Path::new(path)
        .parent()
        .map(|dir| dir.join("cudnn_ops64_9.dll"));
    if let Some(ops) = ops {
        let ops_s = ops.to_string_lossy();
        let extra = dynlib::load(&ops_s);
        if !extra.is_null() {
            if let Ok(ptr) = load_sym(extra, "cudnnNormalizationForwardInference") {
                eprintln!("PBR_CUDNN_NORM {ops_s}");
                return Some(std::mem::transmute(ptr));
            }
        }
    }
    eprintln!("PBR_CUDNN_NORM_MISSING");
    None
}

fn prepend_lib_dir(path: &str) {
    let dir = std::path::Path::new(path).parent();
    let Some(dir) = dir.filter(|d| d.is_dir()) else {
        return;
    };
    let mut new_path = dir.as_os_str().to_os_string();
    if let Some(cur) = std::env::var_os("PATH") {
        new_path.push(";");
        new_path.push(cur);
    }
    std::env::set_var("PATH", new_path);
    #[cfg(windows)]
    unsafe {
        dynlib::set_dll_dir(dir);
    }
}

fn try_load_path(path: &str) -> Result<(Lib, CudnnApi), String> {
    prepend_lib_dir(path);
    let lib = unsafe { dynlib::load(path) };
    if lib.is_null() {
        return Err(format!("failed to load {path}"));
    }
    unsafe {
        let api = CudnnApi {
            create: std::mem::transmute(load_sym(lib, "cudnnCreate")?),
            destroy: std::mem::transmute(load_sym(lib, "cudnnDestroy")?),
            set_stream: std::mem::transmute(load_sym(lib, "cudnnSetStream")?),
            create_tensor: std::mem::transmute(load_sym(lib, "cudnnCreateTensorDescriptor")?),
            destroy_tensor: std::mem::transmute(load_sym(lib, "cudnnDestroyTensorDescriptor")?),
            set_tensor4d: std::mem::transmute(load_sym(lib, "cudnnSetTensor4dDescriptor")?),
            set_tensor_nd: std::mem::transmute(load_sym(lib, "cudnnSetTensorNdDescriptor")?),
            create_filter: std::mem::transmute(load_sym(lib, "cudnnCreateFilterDescriptor")?),
            destroy_filter: std::mem::transmute(load_sym(lib, "cudnnDestroyFilterDescriptor")?),
            set_filter4d: std::mem::transmute(load_sym(lib, "cudnnSetFilter4dDescriptor")?),
            create_conv: std::mem::transmute(load_sym(lib, "cudnnCreateConvolutionDescriptor")?),
            destroy_conv: std::mem::transmute(load_sym(lib, "cudnnDestroyConvolutionDescriptor")?),
            set_conv2d: std::mem::transmute(load_sym(lib, "cudnnSetConvolution2dDescriptor")?),
            set_conv_math: std::mem::transmute(load_sym(lib, "cudnnSetConvolutionMathType")?),
            get_algo_v7: std::mem::transmute(load_sym(lib, "cudnnGetConvolutionForwardAlgorithm_v7")?),
            find_algo: load_sym(lib, "cudnnFindConvolutionForwardAlgorithm")
                .ok()
                .map(|ptr| std::mem::transmute(ptr)),
            get_workspace: std::mem::transmute(load_sym(
                lib,
                "cudnnGetConvolutionForwardWorkspaceSize",
            )?),
            conv_forward: std::mem::transmute(load_sym(lib, "cudnnConvolutionForward")?),
            conv_bias_act: load_sym(lib, "cudnnConvolutionBiasActivationForward")
                .ok()
                .map(|ptr| std::mem::transmute(ptr)),
            create_act: load_sym(lib, "cudnnCreateActivationDescriptor")
                .ok()
                .map(|ptr| std::mem::transmute(ptr)),
            set_act: load_sym(lib, "cudnnSetActivationDescriptor")
                .ok()
                .map(|ptr| std::mem::transmute(ptr)),
            destroy_act: load_sym(lib, "cudnnDestroyActivationDescriptor")
                .ok()
                .map(|ptr| std::mem::transmute(ptr)),
            act_forward: load_act_forward(lib, path),
            transform_tensor: std::mem::transmute(load_sym(lib, "cudnnTransformTensor")?),
            add_tensor: std::mem::transmute(load_sym(lib, "cudnnAddTensor")?),
            norm_fwd_inf: load_norm_fwd_inf(lib, path),
            bn_fwd_train: load_sym(lib, "cudnnBatchNormalizationForwardTraining")
                .ok()
                .map(|p| std::mem::transmute(p)),
            create_op: load_sym(lib, "cudnnCreateOpTensorDescriptor")
                .ok()
                .map(|p| std::mem::transmute(p)),
            set_op: load_sym(lib, "cudnnSetOpTensorDescriptor")
                .ok()
                .map(|p| std::mem::transmute(p)),
            destroy_op: load_sym(lib, "cudnnDestroyOpTensorDescriptor")
                .ok()
                .map(|p| std::mem::transmute(p)),
            op_tensor: load_sym(lib, "cudnnOpTensor")
                .ok()
                .map(|p| std::mem::transmute(p)),
            get_error_string: std::mem::transmute(load_sym(lib, "cudnnGetErrorString")?),
        };
        Ok((Lib(lib), api))
    }
}

fn candidate_paths() -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(p) = std::env::var("CUDNN_PATH") {
        out.push(p);
    }
    #[cfg(windows)]
    {
        const DLLS: &[&str] = &["cudnn64_9.dll", "cudnn64_8.dll", "cudnn64_7.dll"];
        for name in DLLS {
            out.push((*name).to_string());
        }
        for key in ["CUDA_PATH", "CUDA_HOME"] {
            if let Ok(root) = std::env::var(key) {
                for name in DLLS {
                    out.push(format!("{root}\\bin\\{name}"));
                }
            }
        }
        // Official Hunyuan paint venv + other local torch installs.
        const TORCH_LIBS: &[&str] = &[
            r"C:\ai\venv_paint\Lib\site-packages\torch\lib",
            r"C:\ai\venv\Lib\site-packages\torch\lib",
            r"C:\Users\playe\Documents\ComfyUI\.venv\Lib\site-packages\torch\lib",
        ];
        for dir in TORCH_LIBS {
            for name in DLLS {
                out.push(format!("{dir}\\{name}"));
            }
        }
        if let Ok(home) = std::env::var("USERPROFILE") {
            for name in DLLS {
                out.push(format!(
                    "{home}\\AppData\\Local\\Programs\\Python\\Python311\\Lib\\site-packages\\torch\\lib\\{name}"
                ));
            }
        }
    }
    #[cfg(not(windows))]
    {
        const SOS: &[&str] = &[
            "libcudnn.so.9",
            "libcudnn.so.8",
            "libcudnn.so",
        ];
        for name in SOS {
            out.push((*name).to_string());
        }
        for key in ["CUDA_HOME", "CUDA_PATH"] {
            if let Ok(root) = std::env::var(key) {
                for name in SOS {
                    out.push(format!("{root}/lib64/{name}"));
                    out.push(format!("{root}/lib/{name}"));
                }
            }
        }
        out.push("/usr/local/cuda/lib64/libcudnn.so.9".into());
        out.push("/usr/local/cuda/lib64/libcudnn.so.8".into());
        out.push("/usr/local/cuda/lib64/libcudnn.so".into());
    }
    out
}

fn api() -> Result<&'static CudnnApi, String> {
    match CUDNN.get_or_init(|| {
        let mut last = "no cuDNN candidate".to_string();
        for path in candidate_paths() {
            match try_load_path(&path) {
                Ok(loaded) => {
                    eprintln!("PBR_CUDNN_LOAD {path}");
                    return Ok(loaded);
                }
                Err(err) => last = format!("{path}: {err}"),
            }
        }
        Err(last)
    }) {
        Ok((_, api)) => Ok(api),
        Err(err) => Err(err.clone()),
    }
}

pub fn available() -> bool {
    if std::env::var("MAKEPAD_PBR_CUDNN").as_deref() == Ok("0") {
        return false;
    }
    api().is_ok()
}

pub struct ConvDesc {
    handle: cudnnHandle_t,
    x: cudnnTensorDescriptor_t,
    y: cudnnTensorDescriptor_t,
    w: cudnnFilterDescriptor_t,
    conv: cudnnConvolutionDescriptor_t,
    bias: cudnnTensorDescriptor_t,
    algo: cudnnConvolutionFwdAlgo_t,
    math_type: cudnnMathType_t,
    workspace: usize,
}

impl Drop for ConvDesc {
    fn drop(&mut self) {
        if let Ok(api) = api() {
            unsafe {
                let _ = (api.destroy_tensor)(self.x);
                let _ = (api.destroy_tensor)(self.y);
                let _ = (api.destroy_filter)(self.w);
                let _ = (api.destroy_conv)(self.conv);
                if !self.bias.is_null() {
                    let _ = (api.destroy_tensor)(self.bias);
                }
                let _ = (api.destroy)(self.handle);
            }
        }
    }
}

/// Planar `[C, N*H*W]` viewed as NCHW `[N,C,H,W]` with swapped N/C strides.
#[allow(clippy::too_many_arguments)]
pub fn prepare_nchw_strided(
    n: i32,
    c_in: i32,
    c_out: i32,
    h: i32,
    w: i32,
    out_h: i32,
    out_w: i32,
    kh: i32,
    kw: i32,
    pad_h: i32,
    pad_w: i32,
    stride_h: i32,
    stride_w: i32,
    data_type: cudnnDataType_t,
    stream: cudaStream_t,
) -> Result<ConvDesc, String> {
    prepare_nchw_strided_math(
        n,
        c_in,
        c_out,
        h,
        w,
        out_h,
        out_w,
        kh,
        kw,
        pad_h,
        pad_w,
        stride_h,
        stride_w,
        data_type,
        CUDNN_TENSOR_OP_MATH_ALLOW_CONVERSION,
        stream,
    )
}

#[allow(clippy::too_many_arguments)]
fn prepare_nchw_strided_math(
    n: i32,
    c_in: i32,
    c_out: i32,
    h: i32,
    w: i32,
    out_h: i32,
    out_w: i32,
    kh: i32,
    kw: i32,
    pad_h: i32,
    pad_w: i32,
    stride_h: i32,
    stride_w: i32,
    data_type: cudnnDataType_t,
    math: cudnnMathType_t,
    stream: cudaStream_t,
) -> Result<ConvDesc, String> {
    let api = api()?;
    let mut handle = ptr::null_mut();
    check(api, unsafe { (api.create)(&mut handle) }, "cudnnCreate")?;
    check(
        api,
        unsafe { (api.set_stream)(handle, stream) },
        "cudnnSetStream",
    )?;
    let mut x = ptr::null_mut();
    let mut y = ptr::null_mut();
    let mut filt = ptr::null_mut();
    let mut conv = ptr::null_mut();
    check(
        api,
        unsafe { (api.create_tensor)(&mut x) },
        "cudnnCreateTensorDescriptor x",
    )?;
    check(
        api,
        unsafe { (api.create_tensor)(&mut y) },
        "cudnnCreateTensorDescriptor y",
    )?;
    check(
        api,
        unsafe { (api.create_filter)(&mut filt) },
        "cudnnCreateFilterDescriptor",
    )?;
    check(
        api,
        unsafe { (api.create_conv)(&mut conv) },
        "cudnnCreateConvolutionDescriptor",
    )?;
    let x_plane = n * h * w;
    let y_plane = n * out_h * out_w;
    let x_dims = [n, c_in, h, w];
    let y_dims = [n, c_out, out_h, out_w];
    let x_strides = [h * w, x_plane, w, 1];
    let y_strides = [out_h * out_w, y_plane, out_w, 1];
    check(
        api,
        unsafe { (api.set_tensor_nd)(x, data_type, 4, x_dims.as_ptr(), x_strides.as_ptr()) },
        "set x NCHW strided",
    )?;
    check(
        api,
        unsafe { (api.set_tensor_nd)(y, data_type, 4, y_dims.as_ptr(), y_strides.as_ptr()) },
        "set y NCHW strided",
    )?;
    check(
        api,
        unsafe { (api.set_filter4d)(filt, data_type, CUDNN_TENSOR_NCHW, c_out, c_in, kh, kw) },
        "set filter NCHW",
    )?;
    check(
        api,
        unsafe {
            (api.set_conv2d)(
                conv,
                pad_h,
                pad_w,
                stride_h,
                stride_w,
                1,
                1,
                CUDNN_CROSS_CORRELATION,
                CUDNN_DATA_FLOAT,
            )
        },
        "set conv2d",
    )?;
    check(
        api,
        unsafe { (api.set_conv_math)(conv, math) },
        "set conv math",
    )?;
    let (algo, workspace, math_type) = pick_algo(api, handle, x, filt, conv, y)?;
    let bias = make_bias_desc(api, CUDNN_TENSOR_NCHW, data_type, c_out)?;
    Ok(ConvDesc {
        handle,
        x,
        y,
        w: filt,
        conv,
        bias,
        algo,
        math_type,
        workspace,
    })
}

pub fn prepare_nchw_strided_f16(
    n: i32,
    c_in: i32,
    c_out: i32,
    h: i32,
    w: i32,
    kh: i32,
    kw: i32,
    pad_h: i32,
    pad_w: i32,
    stream: cudaStream_t,
) -> Result<ConvDesc, String> {
    prepare_nchw_strided(
        n, c_in, c_out, h, w, h, w, kh, kw, pad_h, pad_w, 1, 1, CUDNN_DATA_HALF, stream,
    )
}

pub fn prepare_nchw_strided_f32(
    n: i32,
    c_in: i32,
    c_out: i32,
    h: i32,
    w: i32,
    kh: i32,
    kw: i32,
    pad_h: i32,
    pad_w: i32,
    stream: cudaStream_t,
) -> Result<ConvDesc, String> {
    prepare_nchw_strided(
        n, c_in, c_out, h, w, h, w, kh, kw, pad_h, pad_w, 1, 1, CUDNN_DATA_FLOAT, stream,
    )
}

/// Planar-strided f32 conv pinned to FMA math: no TF32 down-conversion, true
/// f32 multiply-accumulate.  For precision-critical layers whose rounding
/// would land directly in a locked output envelope.
#[allow(clippy::too_many_arguments)]
pub fn prepare_nchw_strided_f32_fma(
    n: i32,
    c_in: i32,
    c_out: i32,
    h: i32,
    w: i32,
    kh: i32,
    kw: i32,
    pad_h: i32,
    pad_w: i32,
    stream: cudaStream_t,
) -> Result<ConvDesc, String> {
    prepare_nchw_strided_math(
        n,
        c_in,
        c_out,
        h,
        w,
        h,
        w,
        kh,
        kw,
        pad_h,
        pad_w,
        1,
        1,
        CUDNN_DATA_FLOAT,
        CUDNN_FMA_MATH,
        stream,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn prepare_nchw_strided_ex(
    n: i32,
    c_in: i32,
    c_out: i32,
    h: i32,
    w: i32,
    out_h: i32,
    out_w: i32,
    kh: i32,
    kw: i32,
    pad_h: i32,
    pad_w: i32,
    stride_h: i32,
    stride_w: i32,
    data_type: cudnnDataType_t,
    stream: cudaStream_t,
) -> Result<ConvDesc, String> {
    prepare_nchw_strided(
        n, c_in, c_out, h, w, out_h, out_w, kh, kw, pad_h, pad_w, stride_h, stride_w, data_type,
        stream,
    )
}

/// Packed NCHW `[N,C,H,W]` (strides `C*H*W, H*W, W, 1`). This is what
/// official PyTorch feeds cuDNN; the planar-strided view cannot pick
/// tensor-core algos.
#[allow(clippy::too_many_arguments)]
pub fn prepare_nchw_contiguous(
    n: i32,
    c_in: i32,
    c_out: i32,
    h: i32,
    w: i32,
    out_h: i32,
    out_w: i32,
    kh: i32,
    kw: i32,
    pad_h: i32,
    pad_w: i32,
    stride_h: i32,
    stride_w: i32,
    data_type: cudnnDataType_t,
    stream: cudaStream_t,
) -> Result<ConvDesc, String> {
    let api = api()?;
    let mut handle = ptr::null_mut();
    check(api, unsafe { (api.create)(&mut handle) }, "cudnnCreate")?;
    check(
        api,
        unsafe { (api.set_stream)(handle, stream) },
        "cudnnSetStream",
    )?;
    let mut x = ptr::null_mut();
    let mut y = ptr::null_mut();
    let mut filt = ptr::null_mut();
    let mut conv = ptr::null_mut();
    check(
        api,
        unsafe { (api.create_tensor)(&mut x) },
        "cudnnCreateTensorDescriptor x",
    )?;
    check(
        api,
        unsafe { (api.create_tensor)(&mut y) },
        "cudnnCreateTensorDescriptor y",
    )?;
    check(
        api,
        unsafe { (api.create_filter)(&mut filt) },
        "cudnnCreateFilterDescriptor",
    )?;
    check(
        api,
        unsafe { (api.create_conv)(&mut conv) },
        "cudnnCreateConvolutionDescriptor",
    )?;
    check(
        api,
        unsafe { (api.set_tensor4d)(x, CUDNN_TENSOR_NCHW, data_type, n, c_in, h, w) },
        "set x NCHW contig",
    )?;
    check(
        api,
        unsafe { (api.set_tensor4d)(y, CUDNN_TENSOR_NCHW, data_type, n, c_out, out_h, out_w) },
        "set y NCHW contig",
    )?;
    check(
        api,
        unsafe { (api.set_filter4d)(filt, data_type, CUDNN_TENSOR_NCHW, c_out, c_in, kh, kw) },
        "set filter NCHW",
    )?;
    check(
        api,
        unsafe {
            (api.set_conv2d)(
                conv,
                pad_h,
                pad_w,
                stride_h,
                stride_w,
                1,
                1,
                CUDNN_CROSS_CORRELATION,
                CUDNN_DATA_FLOAT,
            )
        },
        "set conv2d",
    )?;
    let math = if data_type == CUDNN_DATA_HALF {
        CUDNN_TENSOR_OP_MATH
    } else {
        CUDNN_TENSOR_OP_MATH_ALLOW_CONVERSION
    };
    check(
        api,
        unsafe { (api.set_conv_math)(conv, math) },
        "set conv math",
    )?;
    // Official nn.Conv2d fp16 is HALF x/w/y + TENSOR_OP, not Find.
    // Find picks WINOGRAD (0.04ms isolated, ~4ms in-walk). Pin both dtypes.
    let (algo, workspace, math_type) =
        pin_implicit_precomp(api, handle, x, filt, conv, y, math)?;
    let bias = make_bias_desc(api, CUDNN_TENSOR_NCHW, data_type, c_out)?;
    Ok(ConvDesc {
        handle,
        x,
        y,
        w: filt,
        conv,
        bias,
        algo,
        math_type,
        workspace,
    })
}

pub fn prepare_nchw_contiguous_f32(
    n: i32,
    c_in: i32,
    c_out: i32,
    h: i32,
    w: i32,
    kh: i32,
    kw: i32,
    pad_h: i32,
    pad_w: i32,
    stream: cudaStream_t,
) -> Result<ConvDesc, String> {
    prepare_nchw_contiguous(
        n, c_in, c_out, h, w, h, w, kh, kw, pad_h, pad_w, 1, 1, CUDNN_DATA_FLOAT, stream,
    )
}

fn find_per_shape(
    api: &CudnnApi,
    handle: cudnnHandle_t,
    x: cudnnTensorDescriptor_t,
    filt: cudnnFilterDescriptor_t,
    conv: cudnnConvolutionDescriptor_t,
    y: cudnnTensorDescriptor_t,
    math: cudnnMathType_t,
) -> Result<(cudnnConvolutionFwdAlgo_t, usize, cudnnMathType_t), String> {
    let mut perf = [cudnnConvolutionFwdAlgoPerf_t {
        algo: 0,
        status: -1,
        time: 0.0,
        memory: 0,
        determinism: 0,
        mathType: 0,
        reserved: [0; 3],
    }; 8];
    let mut returned = 0i32;
    let finder = api.find_algo.unwrap_or(api.get_algo_v7);
    check(
        api,
        unsafe {
            finder(
                handle,
                x,
                filt,
                conv,
                y,
                perf.len() as i32,
                &mut returned,
                perf.as_mut_ptr(),
            )
        },
        "cudnnFind/GetConvolutionForwardAlgorithm",
    )?;
    let ok: Vec<_> = perf
        .iter()
        .take(returned.max(0) as usize)
        .filter(|p| {
            p.status == CUDNN_STATUS_SUCCESS
                && p.memory < 512 * 1024 * 1024
                && p.algo != 6
                && p.algo != 7
        })
        .copied()
        .collect();
    let chosen = ok
        .iter()
        .filter(|p| p.time > 0.0)
        .min_by(|a, b| a.time.partial_cmp(&b.time).unwrap_or(std::cmp::Ordering::Equal))
        .or(ok.first())
        .copied();
    let (algo, mut workspace, math_type) = match chosen {
        Some(p) => (p.algo, p.memory, p.mathType),
        None => (
            CUDNN_CONVOLUTION_FWD_ALGO_IMPLICIT_PRECOMP_GEMM,
            0,
            math,
        ),
    };
    let mut ws = 0usize;
    if unsafe { (api.get_workspace)(handle, x, filt, conv, y, algo, &mut ws) }
        == CUDNN_STATUS_SUCCESS
    {
        workspace = ws;
    }
    static LOG_N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let i = LOG_N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if i < 12 {
        eprintln!(
            "PBR_CUDNN_ALGO id={algo} mathType={math_type} time_ms={:.3} workspace={workspace} shape#{i}",
            chosen.map(|p| p.time).unwrap_or(-1.0)
        );
        if i < 3 {
            for p in &ok {
                eprintln!(
                    "PBR_CUDNN_ALGO_TABLE[{i}] id={} time_ms={:.3} workspace={} math={}",
                    p.algo, p.time, p.memory, p.mathType
                );
            }
        }
    }
    Ok((algo, workspace, math_type))
}

fn pin_implicit_precomp(
    api: &CudnnApi,
    handle: cudnnHandle_t,
    x: cudnnTensorDescriptor_t,
    filt: cudnnFilterDescriptor_t,
    conv: cudnnConvolutionDescriptor_t,
    y: cudnnTensorDescriptor_t,
    math: cudnnMathType_t,
) -> Result<(cudnnConvolutionFwdAlgo_t, usize, cudnnMathType_t), String> {
    let algo = CUDNN_CONVOLUTION_FWD_ALGO_IMPLICIT_PRECOMP_GEMM;
    let mut workspace = 0usize;
    if unsafe { (api.get_workspace)(handle, x, filt, conv, y, algo, &mut workspace) }
        != CUDNN_STATUS_SUCCESS
    {
        workspace = 0;
    }
    static ALGO_LOG: std::sync::Once = std::sync::Once::new();
    ALGO_LOG.call_once(|| {
        eprintln!(
            "PBR_CUDNN_ALGO id={algo} mathType={math} workspace={workspace} pinned=1"
        );
    });
    Ok((algo, workspace, math))
}

fn pick_algo(
    api: &CudnnApi,
    handle: cudnnHandle_t,
    x: cudnnTensorDescriptor_t,
    filt: cudnnFilterDescriptor_t,
    conv: cudnnConvolutionDescriptor_t,
    y: cudnnTensorDescriptor_t,
) -> Result<(cudnnConvolutionFwdAlgo_t, usize, cudnnMathType_t), String> {
    let mut perf = [cudnnConvolutionFwdAlgoPerf_t {
        algo: 0,
        status: -1,
        time: 0.0,
        memory: 0,
        determinism: 0,
        mathType: 0,
        reserved: [0; 3],
    }; 8];
    let mut returned = 0i32;
    check(
        api,
        unsafe {
            (api.get_algo_v7)(
                handle,
                x,
                filt,
                conv,
                y,
                perf.len() as i32,
                &mut returned,
                perf.as_mut_ptr(),
            )
        },
        "cudnnGetConvolutionForwardAlgorithm_v7",
    )?;
    let ok: Vec<_> = perf
        .iter()
        .take(returned.max(0) as usize)
        .filter(|p| p.status == CUDNN_STATUS_SUCCESS && p.memory < 512 * 1024 * 1024)
        .copied()
        .collect();
    static ALGO_LOG: std::sync::Once = std::sync::Once::new();
    ALGO_LOG.call_once(|| {
        for p in &ok {
            eprintln!(
                "PBR_CUDNN_ALGO_TABLE id={} status={} time_ms={:.3} workspace={} math={}",
                p.algo, p.status, p.time, p.memory, p.mathType
            );
        }
        if let Some(p) = ok.first() {
            eprintln!(
                "PBR_CUDNN_ALGO id={} mathType={} time_ms={:.3} workspace={}",
                p.algo, p.mathType, p.time, p.memory
            );
        }
    });
    let chosen = ok.first().copied();
    let (algo, mut workspace, math_type) = match chosen {
        Some(p) => (p.algo, p.memory, p.mathType),
        None => (CUDNN_CONVOLUTION_FWD_ALGO_IMPLICIT_PRECOMP_GEMM, 0, 0),
    };
    let mut ws = 0usize;
    if unsafe { (api.get_workspace)(handle, x, filt, conv, y, algo, &mut ws) }
        == CUDNN_STATUS_SUCCESS
    {
        workspace = ws;
    }
    Ok((algo, workspace, math_type))
}

/// Build NHWC fp16 same-pad conv descriptors and pick a tensor-core algo.
pub fn prepare_nhwc_f16(
    n: i32,
    c_in: i32,
    c_out: i32,
    h: i32,
    w: i32,
    kh: i32,
    kw: i32,
    pad_h: i32,
    pad_w: i32,
    stream: cudaStream_t,
) -> Result<ConvDesc, String> {
    prepare_nhwc_ex(n, c_in, c_out, h, w, h, w, kh, kw, pad_h, pad_w, 1, 1, stream)
}

#[allow(clippy::too_many_arguments)]
pub fn prepare_nhwc_ex(
    n: i32,
    c_in: i32,
    c_out: i32,
    h: i32,
    w: i32,
    out_h: i32,
    out_w: i32,
    kh: i32,
    kw: i32,
    pad_h: i32,
    pad_w: i32,
    stride_h: i32,
    stride_w: i32,
    stream: cudaStream_t,
) -> Result<ConvDesc, String> {
    let api = api()?;
    let mut handle = ptr::null_mut();
    check(api, unsafe { (api.create)(&mut handle) }, "cudnnCreate")?;
    check(
        api,
        unsafe { (api.set_stream)(handle, stream) },
        "cudnnSetStream",
    )?;
    let mut x = ptr::null_mut();
    let mut y = ptr::null_mut();
    let mut filt = ptr::null_mut();
    let mut conv = ptr::null_mut();
    check(
        api,
        unsafe { (api.create_tensor)(&mut x) },
        "cudnnCreateTensorDescriptor x",
    )?;
    check(
        api,
        unsafe { (api.create_tensor)(&mut y) },
        "cudnnCreateTensorDescriptor y",
    )?;
    check(
        api,
        unsafe { (api.create_filter)(&mut filt) },
        "cudnnCreateFilterDescriptor",
    )?;
    check(
        api,
        unsafe { (api.create_conv)(&mut conv) },
        "cudnnCreateConvolutionDescriptor",
    )?;
    check(
        api,
        unsafe { (api.set_tensor4d)(x, CUDNN_TENSOR_NHWC, CUDNN_DATA_HALF, n, c_in, h, w) },
        "set x NHWC",
    )?;
    check(
        api,
        unsafe { (api.set_tensor4d)(y, CUDNN_TENSOR_NHWC, CUDNN_DATA_HALF, n, c_out, out_h, out_w) },
        "set y NHWC",
    )?;
    check(
        api,
        unsafe {
            (api.set_filter4d)(
                filt,
                CUDNN_DATA_HALF,
                CUDNN_TENSOR_NHWC,
                c_out,
                c_in,
                kh,
                kw,
            )
        },
        "set filter NHWC",
    )?;
    check(
        api,
        unsafe {
            (api.set_conv2d)(
                conv,
                pad_h,
                pad_w,
                stride_h,
                stride_w,
                1,
                1,
                CUDNN_CROSS_CORRELATION,
                CUDNN_DATA_FLOAT,
            )
        },
        "set conv2d",
    )?;
    check(
        api,
        unsafe { (api.set_conv_math)(conv, CUDNN_TENSOR_OP_MATH) },
        "set conv math",
    )?;
    let (algo, workspace, math_type) =
        find_per_shape(api, handle, x, filt, conv, y, CUDNN_TENSOR_OP_MATH)?;
    let bias = make_bias_desc(api, CUDNN_TENSOR_NHWC, CUDNN_DATA_HALF, c_out)?;
    Ok(ConvDesc {
        handle,
        x,
        y,
        w: filt,
        conv,
        bias,
        algo,
        math_type,
        workspace,
    })
}

fn make_bias_desc(
    api: &CudnnApi,
    format: cudnnTensorFormat_t,
    data_type: cudnnDataType_t,
    c_out: i32,
) -> Result<cudnnTensorDescriptor_t, String> {
    let mut bias = ptr::null_mut();
    check(
        api,
        unsafe { (api.create_tensor)(&mut bias) },
        "create bias desc",
    )?;
    if let Err(err) = check(
        api,
        unsafe { (api.set_tensor4d)(bias, format, data_type, 1, c_out, 1, 1) },
        "set bias desc",
    ) {
        unsafe {
            let _ = (api.destroy_tensor)(bias);
        }
        return Err(err);
    }
    Ok(bias)
}

pub fn workspace_bytes(desc: &ConvDesc) -> usize {
    desc.workspace
}

fn identity_act() -> Result<*mut c_void, String> {
    thread_local! {
        static ACT: std::cell::Cell<*mut c_void> = const { std::cell::Cell::new(std::ptr::null_mut()) };
    }
    let api = api()?;
    let create = api
        .create_act
        .ok_or_else(|| "cudnnCreateActivationDescriptor missing".to_string())?;
    let set = api
        .set_act
        .ok_or_else(|| "cudnnSetActivationDescriptor missing".to_string())?;
    ACT.with(|cell| {
        let mut h = cell.get();
        if h.is_null() {
            check(api, unsafe { create(&mut h) }, "cudnnCreateActivationDescriptor")?;
            check(
                api,
                unsafe { set(h, CUDNN_ACTIVATION_IDENTITY, CUDNN_NOT_PROPAGATE_NAN, 0.0) },
                "cudnnSetActivationDescriptor IDENTITY",
            )?;
            cell.set(h);
        }
        Ok(h)
    })
}

fn swish_act() -> Result<*mut c_void, String> {
    thread_local! {
        static ACT: std::cell::Cell<*mut c_void> = const { std::cell::Cell::new(std::ptr::null_mut()) };
    }
    let api = api()?;
    let create = api
        .create_act
        .ok_or_else(|| "cudnnCreateActivationDescriptor missing".to_string())?;
    let set = api
        .set_act
        .ok_or_else(|| "cudnnSetActivationDescriptor missing".to_string())?;
    ACT.with(|cell| {
        let mut h = cell.get();
        if h.is_null() {
            check(api, unsafe { create(&mut h) }, "cudnnCreateActivationDescriptor")?;
            // Official nn.SiLU is swish with beta=1.
            check(
                api,
                unsafe { set(h, CUDNN_ACTIVATION_SWISH, CUDNN_NOT_PROPAGATE_NAN, 1.0) },
                "cudnnSetActivationDescriptor SWISH",
            )?;
            cell.set(h);
        }
        Ok(h)
    })
}

/// Official `F.silu` / `nn.SiLU` on a contiguous HALF buffer. Same cuDNN pile.
pub fn silu_half(
    x: *const c_void,
    y: *mut c_void,
    n_elem: i32,
    stream: cudaStream_t,
) -> Result<(), String> {
    let api = api()?;
    let act_fwd = api
        .act_forward
        .ok_or_else(|| "cudnnActivationForward missing".to_string())?;
    let handle = op_handle(stream)?;
    let act = swish_act()?;
    let mut desc = ptr::null_mut();
    check(api, unsafe { (api.create_tensor)(&mut desc) }, "create silu desc")?;
    let result = (|| {
        check(
            api,
            unsafe { (api.set_tensor4d)(desc, CUDNN_TENSOR_NCHW, CUDNN_DATA_HALF, 1, 1, 1, n_elem) },
            "set silu desc",
        )?;
        let alpha = 1.0f32;
        let beta = 0.0f32;
        check(
            api,
            unsafe {
                act_fwd(
                    handle,
                    act,
                    (&alpha as *const f32).cast(),
                    desc,
                    x,
                    (&beta as *const f32).cast(),
                    desc,
                    y,
                )
            },
            "cudnnActivationForward SWISH",
        )
    })();
    unsafe {
        let _ = (api.destroy_tensor)(desc);
    }
    result
}

/// Fused conv + bias, IDENTITY activation. `bias` is `[1,C,1,1]`. `z` unused (alpha2=0).
pub fn convolution_bias_activation_identity(
    desc: &ConvDesc,
    x: *const c_void,
    w: *const c_void,
    bias: *const c_void,
    y: *mut c_void,
    workspace: *mut c_void,
    n: i32,
    c_out: i32,
    out_h: i32,
    out_w: i32,
    data_type: cudnnDataType_t,
    format: cudnnTensorFormat_t,
    stream: cudaStream_t,
) -> Result<(), String> {
    let api = api()?;
    let conv_bias = api
        .conv_bias_act
        .ok_or_else(|| "cudnnConvolutionBiasActivationForward missing".to_string())?;
    let act = identity_act()?;
    check(
        api,
        unsafe { (api.set_stream)(desc.handle, stream) },
        "cudnnSetStream",
    )?;
    let alpha1 = 1.0f32;
    let alpha2 = 0.0f32;
    let _ = (n, c_out, out_h, out_w, data_type, format);
    check(
        api,
        unsafe {
            conv_bias(
                desc.handle,
                (&alpha1 as *const f32).cast(),
                desc.x,
                x,
                desc.w,
                w,
                desc.conv,
                desc.algo,
                workspace,
                desc.workspace,
                (&alpha2 as *const f32).cast(),
                desc.y,
                y,
                desc.bias,
                bias,
                act,
                desc.y,
                y,
            )
        },
        "cudnnConvolutionBiasActivationForward",
    )
}

/// `x`/`y`/`w` are fp16 device pointers. `x`/`y` are NHWC, `w` is NHWC filter.
pub fn convolution_forward_f16(
    desc: &ConvDesc,
    x: *const c_void,
    w: *const c_void,
    y: *mut c_void,
    workspace: *mut c_void,
    stream: cudaStream_t,
) -> Result<(), String> {
    let api = api()?;
    check(
        api,
        unsafe { (api.set_stream)(desc.handle, stream) },
        "cudnnSetStream",
    )?;
    let alpha = 1.0f32;
    let beta = 0.0f32;
    check(
        api,
        unsafe {
            (api.conv_forward)(
                desc.handle,
                (&alpha as *const f32).cast(),
                desc.x,
                x,
                desc.w,
                w,
                desc.conv,
                desc.algo,
                workspace,
                desc.workspace,
                (&beta as *const f32).cast(),
                desc.y,
                y,
            )
        },
        "cudnnConvolutionForward",
    )
}

/// Copy planar `[C, N*H*W]` ↔ packed NCHW `[N, C*H*W]`.
pub fn transform_planar_nchw_f32(
    n: i32,
    c: i32,
    h: i32,
    w: i32,
    src: *const c_void,
    dst: *mut c_void,
    to_nchw: bool,
    stream: cudaStream_t,
) -> Result<(), String> {
    transform_planar_nchw(n, c, h, w, src, dst, to_nchw, CUDNN_DATA_FLOAT, stream)
}

pub fn transform_planar_nchw(
    n: i32,
    c: i32,
    h: i32,
    w: i32,
    src: *const c_void,
    dst: *mut c_void,
    to_nchw: bool,
    data_type: cudnnDataType_t,
    stream: cudaStream_t,
) -> Result<(), String> {
    let api = api()?;
    let handle = op_handle(stream)?;
    let mut a = ptr::null_mut();
    let mut b = ptr::null_mut();
    check(api, unsafe { (api.create_tensor)(&mut a) }, "create src desc")?;
    check(api, unsafe { (api.create_tensor)(&mut b) }, "create dst desc")?;
    let plane = n * h * w;
    let dims = [n, c, h, w];
    let planar_strides = [h * w, plane, w, 1];
    let nchw_strides = [c * h * w, h * w, w, 1];
    let (src_st, dst_st) = if to_nchw {
        (planar_strides, nchw_strides)
    } else {
        (nchw_strides, planar_strides)
    };
    let result = (|| {
        check(
            api,
            unsafe { (api.set_tensor_nd)(a, data_type, 4, dims.as_ptr(), src_st.as_ptr()) },
            "set src nd",
        )?;
        check(
            api,
            unsafe { (api.set_tensor_nd)(b, data_type, 4, dims.as_ptr(), dst_st.as_ptr()) },
            "set dst nd",
        )?;
        let alpha = 1.0f32;
        let beta = 0.0f32;
        check(
            api,
            unsafe {
                (api.transform_tensor)(
                    handle,
                    (&alpha as *const f32).cast(),
                    a,
                    src,
                    (&beta as *const f32).cast(),
                    b,
                    dst,
                )
            },
            "cudnnTransformTensor",
        )
    })();
    unsafe {
        let _ = (api.destroy_tensor)(a);
        let _ = (api.destroy_tensor)(b);
    }
    result
}

pub fn group_norm_available() -> bool {
    api().ok().and_then(|a| a.norm_fwd_inf).is_some()
}

pub fn algo_id(desc: &ConvDesc) -> i32 {
    desc.algo
}

pub fn math_type(desc: &ConvDesc) -> i32 {
    desc.math_type
}

/// Packed NCHW `[N,C,H,W]` ↔ NHWC `[N*H*W, C]` (token matrix).
pub fn transform_nchw_nhwc_f32(
    n: i32,
    c: i32,
    h: i32,
    w: i32,
    src: *const c_void,
    dst: *mut c_void,
    to_nhwc: bool,
    stream: cudaStream_t,
) -> Result<(), String> {
    transform_nchw_nhwc(n, c, h, w, src, dst, to_nhwc, CUDNN_DATA_FLOAT, stream)
}

pub fn transform_nchw_nhwc(
    n: i32,
    c: i32,
    h: i32,
    w: i32,
    src: *const c_void,
    dst: *mut c_void,
    to_nhwc: bool,
    data_type: cudnnDataType_t,
    stream: cudaStream_t,
) -> Result<(), String> {
    let api = api()?;
    let handle = op_handle(stream)?;
    let mut a = ptr::null_mut();
    let mut b = ptr::null_mut();
    check(api, unsafe { (api.create_tensor)(&mut a) }, "nchw-nhwc src")?;
    check(api, unsafe { (api.create_tensor)(&mut b) }, "nchw-nhwc dst")?;
    let dims = [n, c, h, w];
    let nchw = [c * h * w, h * w, w, 1];
    let nhwc = [h * w * c, 1, w * c, c];
    let (ss, ds) = if to_nhwc {
        (nchw, nhwc)
    } else {
        (nhwc, nchw)
    };
    let result = (|| {
        check(
            api,
            unsafe { (api.set_tensor_nd)(a, data_type, 4, dims.as_ptr(), ss.as_ptr()) },
            "nchw-nhwc set src",
        )?;
        check(
            api,
            unsafe { (api.set_tensor_nd)(b, data_type, 4, dims.as_ptr(), ds.as_ptr()) },
            "nchw-nhwc set dst",
        )?;
        let alpha = 1.0f32;
        let beta = 0.0f32;
        check(
            api,
            unsafe {
                (api.transform_tensor)(
                    handle,
                    (&alpha as *const f32).cast(),
                    a,
                    src,
                    (&beta as *const f32).cast(),
                    b,
                    dst,
                )
            },
            "cudnnTransformTensor nchw-nhwc",
        )
    })();
    unsafe {
        let _ = (api.destroy_tensor)(a);
        let _ = (api.destroy_tensor)(b);
    }
    result
}

/// Broadcast `A[1,C,1,1]` onto packed NCHW `y[N,C,H,W]` (f32).
pub fn add_bias_nchw_f32(
    bias: *const c_void,
    y: *mut c_void,
    n: i32,
    c: i32,
    h: i32,
    w: i32,
    stream: cudaStream_t,
) -> Result<(), String> {
    add_broadcast_f32(
        bias,
        1,
        c,
        1,
        1,
        &[c, 1, 1, 1],
        y,
        n,
        c,
        h,
        w,
        &[c * h * w, h * w, w, 1],
        1.0,
        1.0,
        stream,
    )
}

/// Same `y += bias[c]` as [`add_bias_nchw_f32`], using the per-shape
/// `ConvDesc` bias/y descriptors (no create/destroy). Keeps `desc.handle`
/// — do not switch to the shared op_handle (that regression was +473ms).
pub fn add_bias_nchw_from_desc(
    desc: &ConvDesc,
    bias: *const c_void,
    y: *mut c_void,
) -> Result<(), String> {
    if desc.bias.is_null() {
        return Err("add_bias_nchw_from_desc: missing bias desc".into());
    }
    let api = api()?;
    let alpha = 1.0f32;
    let beta = 1.0f32;
    check(
        api,
        unsafe {
            (api.add_tensor)(
                desc.handle,
                (&alpha as *const f32).cast(),
                desc.bias,
                bias,
                (&beta as *const f32).cast(),
                desc.y,
                y,
            )
        },
        "cudnnAddTensor cached",
    )
}

#[allow(clippy::too_many_arguments)]
fn add_broadcast_f32(
    a: *const c_void,
    an: i32,
    ac: i32,
    ah: i32,
    aw: i32,
    a_strides: &[i32; 4],
    c: *mut c_void,
    cn: i32,
    cc: i32,
    ch: i32,
    cw: i32,
    c_strides: &[i32; 4],
    alpha: f32,
    beta: f32,
    stream: cudaStream_t,
) -> Result<(), String> {
    let api = api()?;
    let handle = op_handle(stream)?;
    let mut a_desc = ptr::null_mut();
    let mut c_desc = ptr::null_mut();
    check(api, unsafe { (api.create_tensor)(&mut a_desc) }, "add32 a")?;
    check(api, unsafe { (api.create_tensor)(&mut c_desc) }, "add32 c")?;
    let a_dims = [an, ac, ah, aw];
    let c_dims = [cn, cc, ch, cw];
    let result = (|| {
        check(
            api,
            unsafe {
                (api.set_tensor_nd)(
                    a_desc,
                    CUDNN_DATA_FLOAT,
                    4,
                    a_dims.as_ptr(),
                    a_strides.as_ptr(),
                )
            },
            "add32 set a",
        )?;
        check(
            api,
            unsafe {
                (api.set_tensor_nd)(
                    c_desc,
                    CUDNN_DATA_FLOAT,
                    4,
                    c_dims.as_ptr(),
                    c_strides.as_ptr(),
                )
            },
            "add32 set c",
        )?;
        check(
            api,
            unsafe {
                (api.add_tensor)(
                    handle,
                    (&alpha as *const f32).cast(),
                    a_desc,
                    a,
                    (&beta as *const f32).cast(),
                    c_desc,
                    c,
                )
            },
            "cudnnAddTensor f32",
        )
    })();
    unsafe {
        let _ = (api.destroy_tensor)(a_desc);
        let _ = (api.destroy_tensor)(c_desc);
    }
    result
}

/// Packed NCHW GroupNorm. `x`/`y` are `[N,C,H,W]` contiguous. `scale`/`bias` are `C`.
pub fn group_norm_nchw_f32(
    n: i32,
    c: i32,
    h: i32,
    w: i32,
    groups: i32,
    eps: f32,
    x: *const c_void,
    scale: *const c_void,
    bias: *const c_void,
    y: *mut c_void,
    stream: cudaStream_t,
) -> Result<(), String> {
    group_norm_nchw(
        n, c, h, w, groups, eps, x, scale, bias, y, CUDNN_DATA_FLOAT, stream,
    )
}

pub fn group_norm_nchw(
    n: i32,
    c: i32,
    h: i32,
    w: i32,
    groups: i32,
    eps: f32,
    x: *const c_void,
    scale: *const c_void,
    bias: *const c_void,
    y: *mut c_void,
    data_type: cudnnDataType_t,
    stream: cudaStream_t,
) -> Result<(), String> {
    let api = api()?;
    let norm = api
        .norm_fwd_inf
        .ok_or_else(|| "cudnnNormalizationForwardInference not in this cuDNN".to_string())?;
    let handle = op_handle(stream)?;
    let mut x_desc = ptr::null_mut();
    let mut y_desc = ptr::null_mut();
    let mut sb_desc = ptr::null_mut();
    check(api, unsafe { (api.create_tensor)(&mut x_desc) }, "gn x")?;
    check(api, unsafe { (api.create_tensor)(&mut y_desc) }, "gn y")?;
    check(api, unsafe { (api.create_tensor)(&mut sb_desc) }, "gn sb")?;
    let result = (|| {
        check(
            api,
            unsafe { (api.set_tensor4d)(x_desc, CUDNN_TENSOR_NCHW, data_type, n, c, h, w) },
            "gn x nchw",
        )?;
        check(
            api,
            unsafe { (api.set_tensor4d)(y_desc, CUDNN_TENSOR_NCHW, data_type, n, c, h, w) },
            "gn y nchw",
        )?;
        check(
            api,
            unsafe { (api.set_tensor4d)(sb_desc, CUDNN_TENSOR_NCHW, data_type, 1, c, 1, 1) },
            "gn scale",
        )?;
        let alpha = 1.0f32;
        let beta = 0.0f32;
        check(
            api,
            unsafe {
                norm(
                    handle,
                    CUDNN_NORM_PER_CHANNEL,
                    CUDNN_NORM_OPS_NORM,
                    CUDNN_NORM_ALGO_STANDARD,
                    (&alpha as *const f32).cast(),
                    (&beta as *const f32).cast(),
                    x_desc,
                    x,
                    sb_desc,
                    scale,
                    bias,
                    sb_desc,
                    ptr::null(),
                    ptr::null(),
                    ptr::null_mut(),
                    ptr::null(),
                    ptr::null_mut(),
                    y_desc,
                    y,
                    f64::from(eps),
                    groups,
                )
            },
            "cudnnNormalizationForwardInference",
        )
    })();
    unsafe {
        let _ = (api.destroy_tensor)(x_desc);
        let _ = (api.destroy_tensor)(y_desc);
        let _ = (api.destroy_tensor)(sb_desc);
    }
    result
}

/// GroupNorm as spatial BN on `[1, N*G, (C/G)*H, W]` (identity affine) then
/// per-channel gamma/beta. Scratch `ones`/`zeros`/`mean`/`var` are `N*G` f32.
#[allow(clippy::too_many_arguments)]
pub fn group_norm_bn_nchw_f32(
    n: i32,
    c: i32,
    h: i32,
    w: i32,
    groups: i32,
    eps: f32,
    x: *const c_void,
    y: *mut c_void,
    gamma: *const c_void,
    beta: *const c_void,
    ones: *mut c_void,
    zeros: *mut c_void,
    mean: *mut c_void,
    var: *mut c_void,
    stream: cudaStream_t,
) -> Result<(), String> {
    group_norm_bn_nchw(
        n, c, h, w, groups, eps, x, y, gamma, beta, ones, zeros, mean, var,
        CUDNN_DATA_FLOAT, stream,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn group_norm_bn_nchw(
    n: i32,
    c: i32,
    h: i32,
    w: i32,
    groups: i32,
    eps: f32,
    x: *const c_void,
    y: *mut c_void,
    gamma: *const c_void,
    beta: *const c_void,
    ones: *mut c_void,
    zeros: *mut c_void,
    mean: *mut c_void,
    var: *mut c_void,
    data_type: cudnnDataType_t,
    stream: cudaStream_t,
) -> Result<(), String> {
    let api = api()?;
    let bn = api
        .bn_fwd_train
        .ok_or_else(|| "cudnnBatchNormalizationForwardTraining missing".to_string())?;
    let create_op = api.create_op.ok_or_else(|| "cudnnCreateOpTensorDescriptor missing".to_string())?;
    let set_op = api.set_op.ok_or_else(|| "cudnnSetOpTensorDescriptor missing".to_string())?;
    let destroy_op = api.destroy_op.ok_or_else(|| "cudnnDestroyOpTensorDescriptor missing".to_string())?;
    let op_tensor = api.op_tensor.ok_or_else(|| "cudnnOpTensor missing".to_string())?;
    if groups <= 0 || c % groups != 0 {
        return Err("group_norm_bn bad groups".into());
    }
    let handle = op_handle(stream)?;
    let ng = n * groups;
    let cg = c / groups;
    let mut x_desc = ptr::null_mut();
    let mut y_desc = ptr::null_mut();
    let mut sb_desc = ptr::null_mut();
    let mut ch_desc = ptr::null_mut();
    let mut op_desc = ptr::null_mut();
    check(api, unsafe { (api.create_tensor)(&mut x_desc) }, "bn x")?;
    check(api, unsafe { (api.create_tensor)(&mut y_desc) }, "bn y")?;
    check(api, unsafe { (api.create_tensor)(&mut sb_desc) }, "bn sb")?;
    check(api, unsafe { (api.create_tensor)(&mut ch_desc) }, "bn ch")?;
    check(api, unsafe { create_op(&mut op_desc) }, "bn op")?;
    let result = (|| {
        check(
            api,
            unsafe {
                (api.set_tensor4d)(
                    x_desc,
                    CUDNN_TENSOR_NCHW,
                    data_type,
                    1,
                    ng,
                    cg * h,
                    w,
                )
            },
            "bn x reshape",
        )?;
        check(
            api,
            unsafe {
                (api.set_tensor4d)(
                    y_desc,
                    CUDNN_TENSOR_NCHW,
                    data_type,
                    1,
                    ng,
                    cg * h,
                    w,
                )
            },
            "bn y reshape",
        )?;
        check(
            api,
            unsafe {
                (api.set_tensor4d)(sb_desc, CUDNN_TENSOR_NCHW, CUDNN_DATA_FLOAT, 1, ng, 1, 1)
            },
            "bn sb",
        )?;
        let alpha = 1.0f32;
        let beta0 = 0.0f32;
        check(
            api,
            unsafe {
                bn(
                    handle,
                    CUDNN_BATCHNORM_SPATIAL,
                    (&alpha as *const f32).cast(),
                    (&beta0 as *const f32).cast(),
                    x_desc,
                    x,
                    y_desc,
                    y,
                    sb_desc,
                    ones,
                    zeros,
                    0.0,
                    mean,
                    var,
                    f64::from(eps),
                    mean,
                    var,
                )
            },
            "cudnnBatchNormalizationForwardTraining",
        )?;
        // Affine in original NCHW: y *= gamma[c]; y += beta[c].
        check(
            api,
            unsafe {
                (api.set_tensor4d)(y_desc, CUDNN_TENSOR_NCHW, data_type, n, c, h, w)
            },
            "bn y nchw",
        )?;
        check(
            api,
            unsafe { (api.set_tensor4d)(ch_desc, CUDNN_TENSOR_NCHW, data_type, 1, c, 1, 1) },
            "bn gamma",
        )?;
        check(
            api,
            unsafe { set_op(op_desc, CUDNN_OP_TENSOR_MUL, CUDNN_DATA_FLOAT, CUDNN_NOT_PROPAGATE_NAN) },
            "bn set mul",
        )?;
        check(
            api,
            unsafe {
                op_tensor(
                    handle,
                    op_desc,
                    (&alpha as *const f32).cast(),
                    y_desc,
                    y,
                    (&alpha as *const f32).cast(),
                    ch_desc,
                    gamma,
                    (&beta0 as *const f32).cast(),
                    y_desc,
                    y,
                )
            },
            "cudnnOpTensor MUL gamma",
        )?;
        if data_type == CUDNN_DATA_HALF {
            add_bias_nchw_packed_f16(beta, y, n, c, h, w, stream)
        } else {
            add_bias_nchw_f32(beta, y, n, c, h, w, stream)
        }
    })();
    unsafe {
        if !op_desc.is_null() {
            let _ = destroy_op(op_desc);
        }
        let _ = (api.destroy_tensor)(x_desc);
        let _ = (api.destroy_tensor)(y_desc);
        let _ = (api.destroy_tensor)(sb_desc);
        let _ = (api.destroy_tensor)(ch_desc);
    }
    result
}

thread_local! {
    static OP_HANDLE: std::cell::Cell<cudnnHandle_t> = const { std::cell::Cell::new(std::ptr::null_mut()) };
}

fn op_handle(stream: cudaStream_t) -> Result<cudnnHandle_t, String> {
    let api = api()?;
    OP_HANDLE.with(|cell| {
        let mut h = cell.get();
        if h.is_null() {
            check(api, unsafe { (api.create)(&mut h) }, "cudnnCreate op")?;
            cell.set(h);
        }
        check(api, unsafe { (api.set_stream)(h, stream) }, "cudnnSetStream op")?;
        Ok(h)
    })
}

/// `C = alpha * A + beta * C` for two same-shaped contiguous tensors.
pub fn add_same_f16(
    a: *const c_void,
    c: *mut c_void,
    n_elem: i32,
    alpha: f32,
    beta: f32,
    stream: cudaStream_t,
) -> Result<(), String> {
    add_broadcast_f16(
        a,
        1,
        1,
        1,
        n_elem,
        &[n_elem, n_elem, n_elem, 1],
        c,
        1,
        1,
        1,
        n_elem,
        &[n_elem, n_elem, n_elem, 1],
        alpha,
        beta,
        stream,
    )
}

/// Broadcast `A[1,C,1,1]` onto packed NHWC `y[N,H,W,C]` (fp16).
pub fn add_bias_nhwc_f16(
    bias: *const c_void,
    y: *mut c_void,
    n: i32,
    c: i32,
    h: i32,
    w: i32,
    stream: cudaStream_t,
) -> Result<(), String> {
    add_broadcast_f16(
        bias,
        1,
        c,
        1,
        1,
        &[c, 1, 1, 1],
        y,
        n,
        c,
        h,
        w,
        &[h * w * c, 1, w * c, c],
        1.0,
        1.0,
        stream,
    )
}

/// Broadcast `A[1,C,1,1]` onto planar-as-NCHW `C[N,C,H,W]` (fp16).
pub fn add_bias_nchw_f16(
    bias: *const c_void,
    y: *mut c_void,
    n: i32,
    c: i32,
    h: i32,
    w: i32,
    stream: cudaStream_t,
) -> Result<(), String> {
    let plane = n * h * w;
    add_broadcast_f16(
        bias,
        1,
        c,
        1,
        1,
        &[c, 1, 1, 1],
        y,
        n,
        c,
        h,
        w,
        &[h * w, plane, w, 1],
        1.0,
        1.0,
        stream,
    )
}

/// Broadcast `A[1,C,1,1]` onto packed contiguous NCHW `y[N,C,H,W]` (fp16).
pub fn add_bias_nchw_packed_f16(
    bias: *const c_void,
    y: *mut c_void,
    n: i32,
    c: i32,
    h: i32,
    w: i32,
    stream: cudaStream_t,
) -> Result<(), String> {
    add_broadcast_f16(
        bias,
        1,
        c,
        1,
        1,
        &[c, 1, 1, 1],
        y,
        n,
        c,
        h,
        w,
        &[c * h * w, h * w, w, 1],
        1.0,
        1.0,
        stream,
    )
}

#[allow(clippy::too_many_arguments)]
fn add_broadcast_f16(
    a: *const c_void,
    an: i32,
    ac: i32,
    ah: i32,
    aw: i32,
    a_strides: &[i32; 4],
    c: *mut c_void,
    cn: i32,
    cc: i32,
    ch: i32,
    cw: i32,
    c_strides: &[i32; 4],
    alpha: f32,
    beta: f32,
    stream: cudaStream_t,
) -> Result<(), String> {
    let api = api()?;
    let handle = op_handle(stream)?;
    let mut a_desc = ptr::null_mut();
    let mut c_desc = ptr::null_mut();
    check(api, unsafe { (api.create_tensor)(&mut a_desc) }, "add a desc")?;
    check(api, unsafe { (api.create_tensor)(&mut c_desc) }, "add c desc")?;
    let a_dims = [an, ac, ah, aw];
    let c_dims = [cn, cc, ch, cw];
    let result = (|| {
        check(
            api,
            unsafe {
                (api.set_tensor_nd)(a_desc, CUDNN_DATA_HALF, 4, a_dims.as_ptr(), a_strides.as_ptr())
            },
            "add set a",
        )?;
        check(
            api,
            unsafe {
                (api.set_tensor_nd)(c_desc, CUDNN_DATA_HALF, 4, c_dims.as_ptr(), c_strides.as_ptr())
            },
            "add set c",
        )?;
        check(
            api,
            unsafe {
                (api.add_tensor)(
                    handle,
                    (&alpha as *const f32).cast(),
                    a_desc,
                    a,
                    (&beta as *const f32).cast(),
                    c_desc,
                    c,
                )
            },
            "cudnnAddTensor",
        )
    })();
    unsafe {
        let _ = (api.destroy_tensor)(a_desc);
        let _ = (api.destroy_tensor)(c_desc);
    }
    result
}

/// Host [OC, IC, KH, KW] f32 → NHWC [OC, KH, KW, IC] f16 words.
pub fn pack_nhwc_f16(weights: &[f32], oc: usize, ic: usize, kh: usize, kw: usize) -> Vec<u16> {
    let mut out = vec![0u16; oc * ic * kh * kw];
    for o in 0..oc {
        for i in 0..ic {
            for y in 0..kh {
                for x in 0..kw {
                    let src = ((o * ic + i) * kh + y) * kw + x;
                    let dst = ((o * kh + y) * kw + x) * ic + i;
                    out[dst] = crate_f32_to_f16(weights[src]);
                }
            }
        }
    }
    out
}

fn crate_f32_to_f16(v: f32) -> u16 {
    // IEEE754 rne, matches the ggml host helper enough for weight cache.
    let bits = v.to_bits();
    let sign = (bits >> 16) & 0x8000;
    let mantissa = bits & 0x7fffff;
    let exp = ((bits >> 23) & 0xff) as i32;
    if exp == 255 {
        return (sign | 0x7c00 | (mantissa >> 13) as u32) as u16;
    }
    let exp = exp - 127 + 15;
    if exp >= 31 {
        return (sign | 0x7c00) as u16;
    }
    if exp <= 0 {
        if exp < -10 {
            return sign as u16;
        }
        let mant = mantissa | 0x800000;
        let shift = (1 - exp) as u32;
        let mut half = mant >> (shift + 13);
        let leftover = mant >> (shift + 12);
        if leftover & 1 != 0 {
            half += 1;
        }
        return (sign | half) as u16;
    }
    let mut half = ((exp as u32) << 10) | (mantissa >> 13);
    if mantissa & 0x1000 != 0 {
        half += 1;
    }
    (sign | half) as u16
}

#[cfg(windows)]
mod dynlib {
    use std::ffi::{c_char, c_void, CString, OsStr};
    use std::os::windows::ffi::OsStrExt;

    const LOAD_WITH_ALTERED_SEARCH_PATH: u32 = 0x0000_0008;

    extern "system" {
        fn LoadLibraryExW(name: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
        fn SetDllDirectoryW(name: *const u16) -> i32;
        fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
    }

    pub unsafe fn set_dll_dir(dir: &std::path::Path) {
        use std::os::windows::ffi::OsStrExt;
        let mut wide: Vec<u16> = dir.as_os_str().encode_wide().collect();
        wide.push(0);
        let _ = SetDllDirectoryW(wide.as_ptr());
    }

    pub unsafe fn load(name: &str) -> *mut c_void {
        let mut wide: Vec<u16> = OsStr::new(name).encode_wide().collect();
        wide.push(0);
        let flags = if name.contains('\\') || name.contains('/') {
            LOAD_WITH_ALTERED_SEARCH_PATH
        } else {
            0
        };
        LoadLibraryExW(wide.as_ptr(), std::ptr::null_mut(), flags)
    }

    pub unsafe fn sym(lib: *mut c_void, name: &str) -> *mut c_void {
        let c = CString::new(name).unwrap_or_default();
        GetProcAddress(lib, c.as_ptr())
    }
}

#[cfg(not(windows))]
mod dynlib {
    use std::ffi::{c_char, c_int, c_void, CString};

    const RTLD_NOW: c_int = 2;

    extern "C" {
        fn dlopen(filename: *const c_char, flag: c_int) -> *mut c_void;
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    }

    pub unsafe fn load(name: &str) -> *mut c_void {
        let c = CString::new(name).unwrap_or_default();
        dlopen(c.as_ptr(), RTLD_NOW)
    }

    pub unsafe fn sym(lib: *mut c_void, name: &str) -> *mut c_void {
        let c = CString::new(name).unwrap_or_default();
        dlsym(lib, c.as_ptr())
    }
}

// Keep CUDA_SUCCESS referenced so a Windows-only crate still typechecks.
#[allow(dead_code)]
fn _cuda_ok(status: crate::cudaError_t) -> bool {
    status == CUDA_SUCCESS
}

pub(crate) unsafe fn dynlib_load(name: &str) -> *mut std::ffi::c_void {
    dynlib::load(name)
}

pub(crate) unsafe fn dynlib_sym(lib: *mut std::ffi::c_void, name: &str) -> *mut std::ffi::c_void {
    dynlib::sym(lib, name)
}
