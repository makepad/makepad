#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct gbm_device {
    _unused: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct gbm_surface {
    _unused: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct gbm_bo {
    _unused: [u8; 0],
} 

#[repr(C)]
#[derive(Copy, Clone)]
pub union gbm_bo_handle {
    pub ptr: *mut ::std::os::raw::c_void,
    pub s32: i32,
    pub u32_: u32,
    pub s64: i64,
    pub u64_: u64,
}

const fn gbm_four_char_as_u32(s: &str) -> u32 {
    let b = s.as_bytes();
    ((b[3] as u32) << 24)
        | ((b[2] as u32) << 16)
        | ((b[1] as u32) << 8)
        | ((b[0] as u32))
}

pub const GBM_FORMAT_XRGB8888: u32 = gbm_four_char_as_u32("XR24");
pub const GBM_BO_USE_SCANOUT: u32 = 1 << 0;
pub const GBM_BO_USE_RENDERING: u32 = 1 << 2;

#[link(name = "gbm")]
extern "C" {
    pub fn gbm_create_device(fd: ::std::os::raw::c_int) -> *mut gbm_device;
    pub fn gbm_surface_create(
        gbm: *mut gbm_device,
        width: u32,
        height: u32,
        format: u32,
        flags: u32,
    ) -> *mut gbm_surface;
    pub fn gbm_bo_get_stride(bo: *mut gbm_bo) -> u32;
    pub fn gbm_surface_lock_front_buffer(surface: *mut gbm_surface) -> *mut gbm_bo;
    pub fn gbm_surface_release_buffer(surface: *mut gbm_surface, bo: *mut gbm_bo);
    pub fn gbm_bo_get_user_data(bo: *mut gbm_bo) -> *mut ::std::os::raw::c_void;
    pub fn gbm_bo_get_handle(bo: *mut gbm_bo) -> gbm_bo_handle;
}

