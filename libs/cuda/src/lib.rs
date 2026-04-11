#![allow(non_camel_case_types)]

use std::ffi::{c_char, c_int, c_uint, c_void, CStr};
use std::fmt;
use std::ptr::{self, NonNull};

pub type cudaError_t = c_int;
pub type cudaStream_t = *mut c_void;

pub const CUDA_SUCCESS: cudaError_t = 0;
pub const CUDA_STREAM_NON_BLOCKING: c_uint = 1;

pub const CUDA_MEMCPY_HOST_TO_DEVICE: c_int = 1;
pub const CUDA_MEMCPY_DEVICE_TO_HOST: c_int = 2;

unsafe extern "C" {
    pub fn cudaGetDeviceCount(count: *mut c_int) -> cudaError_t;
    pub fn cudaSetDevice(device: c_int) -> cudaError_t;
    pub fn cudaGetDevice(device: *mut c_int) -> cudaError_t;
    pub fn cudaMalloc(dev_ptr: *mut *mut c_void, size: usize) -> cudaError_t;
    pub fn cudaFree(dev_ptr: *mut c_void) -> cudaError_t;
    pub fn cudaMemcpyAsync(
        dst: *mut c_void,
        src: *const c_void,
        count: usize,
        kind: c_int,
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
    pub fn cudaDeviceSynchronize() -> cudaError_t;
    pub fn cudaGetErrorString(error: cudaError_t) -> *const c_char;
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

#[inline]
pub fn check(status: cudaError_t) -> Result<(), CudaError> {
    if status == CUDA_SUCCESS {
        Ok(())
    } else {
        Err(CudaError { code: status })
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
