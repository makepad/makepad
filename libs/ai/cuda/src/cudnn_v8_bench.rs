//! Isolated cuDNN v7 vs v8 frontend microbench. Not used by the UNet walk.

use crate::cudnn::{
    algo_id, convolution_forward_f16, math_type, prepare_nchw_contiguous, workspace_bytes,
    CUDNN_DATA_FLOAT, CUDNN_DATA_HALF, CUDNN_TENSOR_OP_MATH, CUDNN_TENSOR_OP_MATH_ALLOW_CONVERSION,
};
use crate::{
    check, cudaEventCreate, cudaEventDestroy, cudaEventElapsedTime, cudaEventRecord,
    cudaEventSynchronize, cudaFree, cudaMalloc, cudaMemsetAsync, cudaStreamCreateWithFlags,
    cudaStreamDestroy, cudaStreamSynchronize, cudaEvent_t, cudaStream_t, CUDA_STREAM_NON_BLOCKING,
};
use std::ffi::{c_int, c_void};
use std::ptr;

const WARM: i32 = 10;
const ITERS: i32 = 100;

type Desc = *mut c_void;
type Status = c_int;

const CUDNN_SUCCESS: Status = 0;
const CUDNN_BACKEND_CONVOLUTION_DESCRIPTOR: i64 = 1;
const CUDNN_BACKEND_ENGINECFG_DESCRIPTOR: i64 = 3;
const CUDNN_BACKEND_ENGINEHEUR_DESCRIPTOR: i64 = 4;
const CUDNN_BACKEND_EXECUTION_PLAN_DESCRIPTOR: i64 = 5;
const CUDNN_BACKEND_OPERATION_CONVOLUTION_FORWARD_DESCRIPTOR: i64 = 10;
const CUDNN_BACKEND_OPERATIONGRAPH_DESCRIPTOR: i64 = 15;
const CUDNN_BACKEND_VARIANT_PACK_DESCRIPTOR: i64 = 16;
const CUDNN_BACKEND_TENSOR_DESCRIPTOR: i64 = 17;

const CUDNN_TYPE_HANDLE: i64 = 0;
const CUDNN_TYPE_DATA_TYPE: i64 = 1;
const CUDNN_TYPE_INT64: i64 = 3;
const CUDNN_TYPE_VOID_PTR: i64 = 6;
const CUDNN_TYPE_CONVOLUTION_MODE: i64 = 7;
const CUDNN_TYPE_HEUR_MODE: i64 = 8;
const CUDNN_TYPE_BACKEND_DESCRIPTOR: i64 = 15;

const CUDNN_HEUR_MODE_INSTANT: i64 = 0;
const CUDNN_CROSS_CORRELATION_I64: i64 = 1;

const CUDNN_ATTR_CONVOLUTION_COMP_TYPE: i64 = 100;
const CUDNN_ATTR_CONVOLUTION_CONV_MODE: i64 = 101;
const CUDNN_ATTR_CONVOLUTION_DILATIONS: i64 = 102;
const CUDNN_ATTR_CONVOLUTION_FILTER_STRIDES: i64 = 103;
const CUDNN_ATTR_CONVOLUTION_POST_PADDINGS: i64 = 104;
const CUDNN_ATTR_CONVOLUTION_PRE_PADDINGS: i64 = 105;
const CUDNN_ATTR_CONVOLUTION_SPATIAL_DIMS: i64 = 106;
const CUDNN_ATTR_ENGINEHEUR_MODE: i64 = 200;
const CUDNN_ATTR_ENGINEHEUR_OPERATION_GRAPH: i64 = 201;
const CUDNN_ATTR_ENGINEHEUR_RESULTS: i64 = 202;
const CUDNN_ATTR_EXECUTION_PLAN_HANDLE: i64 = 400;
const CUDNN_ATTR_EXECUTION_PLAN_ENGINE_CONFIG: i64 = 401;
const CUDNN_ATTR_EXECUTION_PLAN_WORKSPACE_SIZE: i64 = 402;
const CUDNN_ATTR_OPERATION_CONVOLUTION_FORWARD_ALPHA: i64 = 700;
const CUDNN_ATTR_OPERATION_CONVOLUTION_FORWARD_BETA: i64 = 701;
const CUDNN_ATTR_OPERATION_CONVOLUTION_FORWARD_CONV_DESC: i64 = 702;
const CUDNN_ATTR_OPERATION_CONVOLUTION_FORWARD_W: i64 = 703;
const CUDNN_ATTR_OPERATION_CONVOLUTION_FORWARD_X: i64 = 704;
const CUDNN_ATTR_OPERATION_CONVOLUTION_FORWARD_Y: i64 = 705;
const CUDNN_ATTR_OPERATIONGRAPH_HANDLE: i64 = 800;
const CUDNN_ATTR_OPERATIONGRAPH_OPS: i64 = 801;
const CUDNN_ATTR_TENSOR_BYTE_ALIGNMENT: i64 = 900;
const CUDNN_ATTR_TENSOR_DATA_TYPE: i64 = 901;
const CUDNN_ATTR_TENSOR_DIMENSIONS: i64 = 902;
const CUDNN_ATTR_TENSOR_STRIDES: i64 = 903;
const CUDNN_ATTR_TENSOR_UNIQUE_ID: i64 = 906;
const CUDNN_ATTR_VARIANT_PACK_UNIQUE_IDS: i64 = 1000;
const CUDNN_ATTR_VARIANT_PACK_DATA_POINTERS: i64 = 1001;
const CUDNN_ATTR_VARIANT_PACK_WORKSPACE: i64 = 1003;

type FnCreate = unsafe extern "C" fn(*mut *mut c_void) -> Status;
type FnDestroy = unsafe extern "C" fn(*mut c_void) -> Status;
type FnSetStream = unsafe extern "C" fn(*mut c_void, cudaStream_t) -> Status;
type FnBeCreate = unsafe extern "C" fn(i64, *mut Desc) -> Status;
type FnBeDestroy = unsafe extern "C" fn(Desc) -> Status;
type FnBeSet = unsafe extern "C" fn(Desc, i64, i64, i64, *const c_void) -> Status;
type FnBeGet = unsafe extern "C" fn(Desc, i64, i64, i64, *mut i64, *mut c_void) -> Status;
type FnBeFin = unsafe extern "C" fn(Desc) -> Status;
type FnBeExec = unsafe extern "C" fn(*mut c_void, Desc, Desc) -> Status;

struct V8 {
    create: FnCreate,
    destroy: FnDestroy,
    set_stream: FnSetStream,
    be_create: FnBeCreate,
    be_destroy: FnBeDestroy,
    be_set: FnBeSet,
    be_get: FnBeGet,
    be_fin: FnBeFin,
    be_exec: FnBeExec,
}

fn load_v8() -> Result<V8, String> {
    // Reuse the already-loaded walk library (PATH + SetDllDirectory already done
    // if prepare_nchw_contiguous ran). Probe backend symbols independently.
    let _ = crate::cudnn::available();
    let paths = [
        "cudnn64_9.dll",
        "cudnn_graph64_9.dll",
        "libcudnn.so.9",
        "libcudnn.so",
    ];
    let mut last = "no backend candidate".to_string();
    for path in paths {
        match try_load_v8(path) {
            Ok(v) => {
                eprintln!("PBR_CUDNN_V8_LOAD {path}");
                return Ok(v);
            }
            Err(e) => last = e,
        }
    }
    Err(last)
}

fn try_load_v8(path: &str) -> Result<V8, String> {
    let lib = unsafe { crate::cudnn::dynlib_load(path) };
    if lib.is_null() {
        return Err(format!("load {path} failed"));
    }
    unsafe {
        Ok(V8 {
            create: std::mem::transmute(sym(lib, "cudnnCreate")?),
            destroy: std::mem::transmute(sym(lib, "cudnnDestroy")?),
            set_stream: std::mem::transmute(sym(lib, "cudnnSetStream")?),
            be_create: std::mem::transmute(sym(lib, "cudnnBackendCreateDescriptor")?),
            be_destroy: std::mem::transmute(sym(lib, "cudnnBackendDestroyDescriptor")?),
            be_set: std::mem::transmute(sym(lib, "cudnnBackendSetAttribute")?),
            be_get: std::mem::transmute(sym(lib, "cudnnBackendGetAttribute")?),
            be_fin: std::mem::transmute(sym(lib, "cudnnBackendFinalize")?),
            be_exec: std::mem::transmute(sym(lib, "cudnnBackendExecute")?),
        })
    }
}

fn sym(lib: *mut c_void, name: &str) -> Result<*mut c_void, String> {
    let p = unsafe { crate::cudnn::dynlib_sym(lib, name) };
    if p.is_null() {
        Err(format!("missing {name}"))
    } else {
        Ok(p)
    }
}

struct Shape {
    name: &'static str,
    n: i32,
    c: i32,
    h: i32,
    w: i32,
    half: bool,
}

fn time_events(
    stream: cudaStream_t,
    mut launch: impl FnMut() -> Result<(), String>,
) -> Result<f32, String> {
    for _ in 0..WARM {
        launch()?;
    }
    check(unsafe { cudaStreamSynchronize(stream) }).map_err(|e| e.to_string())?;
    let mut start: cudaEvent_t = ptr::null_mut();
    let mut stop: cudaEvent_t = ptr::null_mut();
    check(unsafe { cudaEventCreate(&mut start) }).map_err(|e| e.to_string())?;
    check(unsafe { cudaEventCreate(&mut stop) }).map_err(|e| e.to_string())?;
    check(unsafe { cudaEventRecord(start, stream) }).map_err(|e| e.to_string())?;
    for _ in 0..ITERS {
        launch()?;
    }
    check(unsafe { cudaEventRecord(stop, stream) }).map_err(|e| e.to_string())?;
    check(unsafe { cudaEventSynchronize(stop) }).map_err(|e| e.to_string())?;
    let mut ms = 0.0f32;
    check(unsafe { cudaEventElapsedTime(&mut ms, start, stop) }).map_err(|e| e.to_string())?;
    unsafe {
        let _ = cudaEventDestroy(start);
        let _ = cudaEventDestroy(stop);
    }
    Ok(ms / ITERS as f32)
}

fn bench_v7(shape: &Shape, stream: cudaStream_t) -> Result<(f32, i32, i32, usize), String> {
    let dtype = if shape.half {
        CUDNN_DATA_HALF
    } else {
        CUDNN_DATA_FLOAT
    };
    let desc = prepare_nchw_contiguous(
        shape.n, shape.c, shape.c, shape.h, shape.w, shape.h, shape.w, 3, 3, 1, 1, 1, 1, dtype,
        stream,
    )?;
    let elem = if shape.half { 2usize } else { 4 };
    let x_bytes = (shape.n * shape.c * shape.h * shape.w) as usize * elem;
    let w_bytes = (shape.c * shape.c * 3 * 3) as usize * elem;
    let y_bytes = x_bytes;
    let mut x = ptr::null_mut();
    let mut w = ptr::null_mut();
    let mut y = ptr::null_mut();
    check(unsafe { cudaMalloc(&mut x, x_bytes) }).map_err(|e| e.to_string())?;
    check(unsafe { cudaMalloc(&mut w, w_bytes) }).map_err(|e| e.to_string())?;
    check(unsafe { cudaMalloc(&mut y, y_bytes) }).map_err(|e| e.to_string())?;
    check(unsafe { cudaMemsetAsync(x, 0, x_bytes, stream) }).map_err(|e| e.to_string())?;
    check(unsafe { cudaMemsetAsync(w, 0, w_bytes, stream) }).map_err(|e| e.to_string())?;
    check(unsafe { cudaMemsetAsync(y, 0, y_bytes, stream) }).map_err(|e| e.to_string())?;
    let ws_bytes = workspace_bytes(&desc);
    let mut ws = ptr::null_mut();
    if ws_bytes > 0 {
        check(unsafe { cudaMalloc(&mut ws, ws_bytes) }).map_err(|e| e.to_string())?;
    }
    let ms = time_events(stream, || {
        convolution_forward_f16(&desc, x, w, y, ws, stream)
    })?;
    unsafe {
        if !ws.is_null() {
            let _ = cudaFree(ws);
        }
        let _ = cudaFree(x);
        let _ = cudaFree(w);
        let _ = cudaFree(y);
    }
    Ok((ms, algo_id(&desc), math_type(&desc), ws_bytes))
}

fn be_chk(api: &V8, st: Status, what: &str) -> Result<(), String> {
    if st == CUDNN_SUCCESS {
        Ok(())
    } else {
        Err(format!("{what}: cudnn status {st}"))
    }
}

fn be_set<T>(api: &V8, d: Desc, attr: i64, ty: i64, vals: &[T]) -> Result<(), String> {
    be_chk(
        api,
        unsafe {
            (api.be_set)(
                d,
                attr,
                ty,
                vals.len() as i64,
                vals.as_ptr().cast(),
            )
        },
        "backend set",
    )
}

fn make_tensor(
    api: &V8,
    uid: i64,
    dtype: i32,
    dims: [i64; 4],
    strides: [i64; 4],
) -> Result<Desc, String> {
    let mut d = ptr::null_mut();
    be_chk(
        api,
        unsafe { (api.be_create)(CUDNN_BACKEND_TENSOR_DESCRIPTOR, &mut d) },
        "create tensor",
    )?;
    let align: i64 = 16;
    be_set(api, d, CUDNN_ATTR_TENSOR_UNIQUE_ID, CUDNN_TYPE_INT64, &[uid])?;
    be_set(api, d, CUDNN_ATTR_TENSOR_DATA_TYPE, CUDNN_TYPE_DATA_TYPE, &[dtype])?;
    be_set(api, d, CUDNN_ATTR_TENSOR_BYTE_ALIGNMENT, CUDNN_TYPE_INT64, &[align])?;
    be_set(api, d, CUDNN_ATTR_TENSOR_DIMENSIONS, CUDNN_TYPE_INT64, &dims)?;
    be_set(api, d, CUDNN_ATTR_TENSOR_STRIDES, CUDNN_TYPE_INT64, &strides)?;
    be_chk(api, unsafe { (api.be_fin)(d) }, "finalize tensor")?;
    Ok(d)
}

fn bench_v8(api: &V8, shape: &Shape, stream: cudaStream_t) -> Result<(f32, i64), String> {
    let mut handle = ptr::null_mut();
    be_chk(api, unsafe { (api.create)(&mut handle) }, "cudnnCreate")?;
    be_chk(
        api,
        unsafe { (api.set_stream)(handle, stream) },
        "cudnnSetStream",
    )?;
    let dtype = if shape.half {
        CUDNN_DATA_HALF
    } else {
        CUDNN_DATA_FLOAT
    };
    let n = i64::from(shape.n);
    let c = i64::from(shape.c);
    let h = i64::from(shape.h);
    let w = i64::from(shape.w);
    let x_dims = [n, c, h, w];
    let x_st = [c * h * w, h * w, w, 1];
    let f_dims = [c, c, 3, 3];
    let f_st = [c * 9, 9, 3, 1];
    let x_desc = make_tensor(api, 0, dtype, x_dims, x_st)?;
    let w_desc = make_tensor(api, 1, dtype, f_dims, f_st)?;
    let y_desc = make_tensor(api, 2, dtype, x_dims, x_st)?;

    let mut conv = ptr::null_mut();
    be_chk(
        api,
        unsafe { (api.be_create)(CUDNN_BACKEND_CONVOLUTION_DESCRIPTOR, &mut conv) },
        "create conv",
    )?;
    let spat: i64 = 2;
    let ones = [1i64, 1];
    be_set(
        api,
        conv,
        CUDNN_ATTR_CONVOLUTION_COMP_TYPE,
        CUDNN_TYPE_DATA_TYPE,
        &[CUDNN_DATA_FLOAT],
    )?;
    be_set(
        api,
        conv,
        CUDNN_ATTR_CONVOLUTION_CONV_MODE,
        CUDNN_TYPE_CONVOLUTION_MODE,
        &[1i32],
    )?;
    be_set(api, conv, CUDNN_ATTR_CONVOLUTION_SPATIAL_DIMS, CUDNN_TYPE_INT64, &[spat])?;
    be_set(api, conv, CUDNN_ATTR_CONVOLUTION_DILATIONS, CUDNN_TYPE_INT64, &ones)?;
    be_set(api, conv, CUDNN_ATTR_CONVOLUTION_FILTER_STRIDES, CUDNN_TYPE_INT64, &ones)?;
    be_set(api, conv, CUDNN_ATTR_CONVOLUTION_PRE_PADDINGS, CUDNN_TYPE_INT64, &ones)?;
    be_set(api, conv, CUDNN_ATTR_CONVOLUTION_POST_PADDINGS, CUDNN_TYPE_INT64, &ones)?;
    be_chk(api, unsafe { (api.be_fin)(conv) }, "finalize conv")?;

    let mut op = ptr::null_mut();
    be_chk(
        api,
        unsafe { (api.be_create)(CUDNN_BACKEND_OPERATION_CONVOLUTION_FORWARD_DESCRIPTOR, &mut op) },
        "create fwd op",
    )?;
    let alpha = 1.0f32;
    let beta = 0.0f32;
    be_chk(
        api,
        unsafe {
            (api.be_set)(
                op,
                CUDNN_ATTR_OPERATION_CONVOLUTION_FORWARD_X,
                CUDNN_TYPE_BACKEND_DESCRIPTOR,
                1,
                (&x_desc as *const Desc).cast(),
            )
        },
        "op x",
    )?;
    be_chk(
        api,
        unsafe {
            (api.be_set)(
                op,
                CUDNN_ATTR_OPERATION_CONVOLUTION_FORWARD_W,
                CUDNN_TYPE_BACKEND_DESCRIPTOR,
                1,
                (&w_desc as *const Desc).cast(),
            )
        },
        "op w",
    )?;
    be_chk(
        api,
        unsafe {
            (api.be_set)(
                op,
                CUDNN_ATTR_OPERATION_CONVOLUTION_FORWARD_Y,
                CUDNN_TYPE_BACKEND_DESCRIPTOR,
                1,
                (&y_desc as *const Desc).cast(),
            )
        },
        "op y",
    )?;
    be_chk(
        api,
        unsafe {
            (api.be_set)(
                op,
                CUDNN_ATTR_OPERATION_CONVOLUTION_FORWARD_CONV_DESC,
                CUDNN_TYPE_BACKEND_DESCRIPTOR,
                1,
                (&conv as *const Desc).cast(),
            )
        },
        "op conv",
    )?;
    be_chk(
        api,
        unsafe {
            (api.be_set)(
                op,
                CUDNN_ATTR_OPERATION_CONVOLUTION_FORWARD_ALPHA,
                4, // CUDNN_TYPE_FLOAT
                1,
                (&alpha as *const f32).cast(),
            )
        },
        "op alpha",
    )?;
    be_chk(
        api,
        unsafe {
            (api.be_set)(
                op,
                CUDNN_ATTR_OPERATION_CONVOLUTION_FORWARD_BETA,
                4,
                1,
                (&beta as *const f32).cast(),
            )
        },
        "op beta",
    )?;
    be_chk(api, unsafe { (api.be_fin)(op) }, "finalize op")?;

    let mut graph = ptr::null_mut();
    be_chk(
        api,
        unsafe { (api.be_create)(CUDNN_BACKEND_OPERATIONGRAPH_DESCRIPTOR, &mut graph) },
        "create graph",
    )?;
    be_chk(
        api,
        unsafe {
            (api.be_set)(
                graph,
                CUDNN_ATTR_OPERATIONGRAPH_HANDLE,
                CUDNN_TYPE_HANDLE,
                1,
                (&handle as *const *mut c_void).cast(),
            )
        },
        "graph handle",
    )?;
    be_chk(
        api,
        unsafe {
            (api.be_set)(
                graph,
                CUDNN_ATTR_OPERATIONGRAPH_OPS,
                CUDNN_TYPE_BACKEND_DESCRIPTOR,
                1,
                (&op as *const Desc).cast(),
            )
        },
        "graph ops",
    )?;
    be_chk(api, unsafe { (api.be_fin)(graph) }, "finalize graph")?;

    let mut heur = ptr::null_mut();
    be_chk(
        api,
        unsafe { (api.be_create)(CUDNN_BACKEND_ENGINEHEUR_DESCRIPTOR, &mut heur) },
        "create heur",
    )?;
    be_chk(
        api,
        unsafe {
            (api.be_set)(
                heur,
                CUDNN_ATTR_ENGINEHEUR_OPERATION_GRAPH,
                CUDNN_TYPE_BACKEND_DESCRIPTOR,
                1,
                (&graph as *const Desc).cast(),
            )
        },
        "heur graph",
    )?;
    be_set(
        api,
        heur,
        CUDNN_ATTR_ENGINEHEUR_MODE,
        CUDNN_TYPE_HEUR_MODE,
        &[0i32],
    )?;
    be_chk(api, unsafe { (api.be_fin)(heur) }, "finalize heur")?;

    let mut cfg = ptr::null_mut();
    be_chk(
        api,
        unsafe { (api.be_create)(CUDNN_BACKEND_ENGINECFG_DESCRIPTOR, &mut cfg) },
        "create cfg",
    )?;
    let mut returned = 0i64;
    be_chk(
        api,
        unsafe {
            (api.be_get)(
                heur,
                CUDNN_ATTR_ENGINEHEUR_RESULTS,
                CUDNN_TYPE_BACKEND_DESCRIPTOR,
                1,
                &mut returned,
                (&mut cfg as *mut Desc).cast(),
            )
        },
        "heur results",
    )?;
    if returned < 1 {
        return Err("v8 heur returned 0 engine configs".into());
    }

    let mut plan = ptr::null_mut();
    be_chk(
        api,
        unsafe { (api.be_create)(CUDNN_BACKEND_EXECUTION_PLAN_DESCRIPTOR, &mut plan) },
        "create plan",
    )?;
    be_chk(
        api,
        unsafe {
            (api.be_set)(
                plan,
                CUDNN_ATTR_EXECUTION_PLAN_HANDLE,
                CUDNN_TYPE_HANDLE,
                1,
                (&handle as *const *mut c_void).cast(),
            )
        },
        "plan handle",
    )?;
    be_chk(
        api,
        unsafe {
            (api.be_set)(
                plan,
                CUDNN_ATTR_EXECUTION_PLAN_ENGINE_CONFIG,
                CUDNN_TYPE_BACKEND_DESCRIPTOR,
                1,
                (&cfg as *const Desc).cast(),
            )
        },
        "plan cfg",
    )?;
    be_chk(api, unsafe { (api.be_fin)(plan) }, "finalize plan")?;
    let mut ws_size: i64 = 0;
    let mut nret = 0i64;
    be_chk(
        api,
        unsafe {
            (api.be_get)(
                plan,
                CUDNN_ATTR_EXECUTION_PLAN_WORKSPACE_SIZE,
                CUDNN_TYPE_INT64,
                1,
                &mut nret,
                (&mut ws_size as *mut i64).cast(),
            )
        },
        "plan workspace",
    )?;

    let elem = if shape.half { 2usize } else { 4 };
    let x_bytes = (shape.n * shape.c * shape.h * shape.w) as usize * elem;
    let w_bytes = (shape.c * shape.c * 9) as usize * elem;
    let mut x = ptr::null_mut();
    let mut filt = ptr::null_mut();
    let mut y = ptr::null_mut();
    check(unsafe { cudaMalloc(&mut x, x_bytes) }).map_err(|e| e.to_string())?;
    check(unsafe { cudaMalloc(&mut filt, w_bytes) }).map_err(|e| e.to_string())?;
    check(unsafe { cudaMalloc(&mut y, x_bytes) }).map_err(|e| e.to_string())?;
    check(unsafe { cudaMemsetAsync(x, 0, x_bytes, stream) }).map_err(|e| e.to_string())?;
    check(unsafe { cudaMemsetAsync(filt, 0, w_bytes, stream) }).map_err(|e| e.to_string())?;
    check(unsafe { cudaMemsetAsync(y, 0, x_bytes, stream) }).map_err(|e| e.to_string())?;
    let mut ws = ptr::null_mut();
    if ws_size > 0 {
        check(unsafe { cudaMalloc(&mut ws, ws_size as usize) }).map_err(|e| e.to_string())?;
    }

    let mut var = ptr::null_mut();
    be_chk(
        api,
        unsafe { (api.be_create)(CUDNN_BACKEND_VARIANT_PACK_DESCRIPTOR, &mut var) },
        "create varpack",
    )?;
    let uids = [0i64, 1, 2];
    let ptrs = [x, filt, y];
    be_set(api, var, CUDNN_ATTR_VARIANT_PACK_UNIQUE_IDS, CUDNN_TYPE_INT64, &uids)?;
    be_chk(
        api,
        unsafe {
            (api.be_set)(
                var,
                CUDNN_ATTR_VARIANT_PACK_DATA_POINTERS,
                CUDNN_TYPE_VOID_PTR,
                3,
                ptrs.as_ptr().cast(),
            )
        },
        "var ptrs",
    )?;
    if !ws.is_null() {
        be_chk(
            api,
            unsafe {
                (api.be_set)(
                    var,
                    CUDNN_ATTR_VARIANT_PACK_WORKSPACE,
                    CUDNN_TYPE_VOID_PTR,
                    1,
                    (&ws as *const *mut c_void).cast(),
                )
            },
            "var ws",
        )?;
    }
    be_chk(api, unsafe { (api.be_fin)(var) }, "finalize varpack")?;

    let ms = time_events(stream, || {
        be_chk(api, unsafe { (api.be_exec)(handle, plan, var) }, "v8 execute")
    })?;

    unsafe {
        let _ = (api.be_destroy)(var);
        let _ = (api.be_destroy)(plan);
        let _ = (api.be_destroy)(cfg);
        let _ = (api.be_destroy)(heur);
        let _ = (api.be_destroy)(graph);
        let _ = (api.be_destroy)(op);
        let _ = (api.be_destroy)(conv);
        let _ = (api.be_destroy)(x_desc);
        let _ = (api.be_destroy)(w_desc);
        let _ = (api.be_destroy)(y_desc);
        let _ = (api.destroy)(handle);
        if !ws.is_null() {
            let _ = cudaFree(ws);
        }
        let _ = cudaFree(x);
        let _ = cudaFree(filt);
        let _ = cudaFree(y);
    }
    Ok((ms, ws_size))
}

/// Run the isolated v7 vs v8 table. Safe to call from a dedicated bin only.
pub fn run() -> Result<(), String> {
    if !crate::cudnn::available() {
        return Err("cuDNN unavailable".into());
    }
    let mut stream: cudaStream_t = ptr::null_mut();
    check(unsafe { cudaStreamCreateWithFlags(&mut stream, CUDA_STREAM_NON_BLOCKING) })
        .map_err(|e| e.to_string())?;
    // Touch the walk loader so PATH/SetDllDirectory match production.
    let _ = prepare_nchw_contiguous(
        1, 8, 8, 8, 8, 8, 8, 3, 3, 1, 1, 1, 1, CUDNN_DATA_FLOAT, stream,
    );

    let shapes = [
        Shape {
            name: "36x320x16x16",
            n: 36,
            c: 320,
            h: 16,
            w: 16,
            half: false,
        },
        Shape {
            name: "36x640x8x8",
            n: 36,
            c: 640,
            h: 8,
            w: 8,
            half: false,
        },
        Shape {
            name: "36x320x16x16",
            n: 36,
            c: 320,
            h: 16,
            w: 16,
            half: true,
        },
        Shape {
            name: "36x640x8x8",
            n: 36,
            c: 640,
            h: 8,
            w: 8,
            half: true,
        },
    ];

    let v8 = match load_v8() {
        Ok(v) => {
            eprintln!("PBR_CUDNN_V8_OK");
            Some(v)
        }
        Err(e) => {
            eprintln!("PBR_CUDNN_V8_MISSING {e}");
            None
        }
    };

    println!("PBR_CUDNN_BENCH_HDR shape dtype v7_ms v7_algo v7_math v7_ws v8_ms v8_ws");
    for s in &shapes {
        let dt = if s.half { "f16" } else { "f32" };
        let want_math = if s.half {
            CUDNN_TENSOR_OP_MATH
        } else {
            CUDNN_TENSOR_OP_MATH_ALLOW_CONVERSION
        };
        let (v7_ms, algo, math, ws) = bench_v7(s, stream)?;
        let (v8_s, v8_ws) = if let Some(api) = &v8 {
            match bench_v8(api, s, stream) {
                Ok((ms, wss)) => (format!("{ms:.4}"), wss.to_string()),
                Err(e) => {
                    eprintln!("PBR_CUDNN_V8_FAIL {} {dt} {e}", s.name);
                    ("ERR".into(), "-".into())
                }
            }
        } else {
            ("NA".into(), "-".into())
        };
        println!(
            "PBR_CUDNN_BENCH {} {dt} {v7_ms:.4} {algo} {math} {ws} {v8_s} {v8_ws} want_math={want_math}",
            s.name
        );
    }
    unsafe {
        let _ = cudaStreamDestroy(stream);
    }
    Ok(())
}
