//! Probe the DRM format modifier used by the VA driver for NV12 surfaces.
//!
//! GStreamer's `vah*dec` DMABuf caps often omit `drm-format`. Importing NVIDIA
//! block-linear buffers as LINEAR causes 花屏. Creating a same-size VA surface
//! and exporting it yields the modifier the decoder actually uses.

use super::libc_sys::{close, open};
use super::module_loader::ModuleLoader;
use std::ffi::{c_int, c_uint, c_void};
use std::os::fd::RawFd;
use std::sync::Mutex;

const VA_STATUS_SUCCESS: i32 = 0;
const VA_RT_FORMAT_YUV420: u32 = 1;
const VA_FOURCC_NV12: u32 = 0x3231_564e;
const VA_PROFILE_H264_MAIN: i32 = 6;
const VA_ENTRYPOINT_VLD: i32 = 1;
const VA_CONFIG_ATTRIB_RT_FORMAT: i32 = 1;
const VA_SURFACE_ATTRIB_PIXEL_FORMAT: i32 = 2;
const VA_SURFACE_ATTRIB_SETTABLE: u32 = 0x0000_0002;
const VA_GENERIC_VALUE_TYPE_INTEGER: i32 = 1;
const VA_SURFACE_ATTRIB_MEM_TYPE_DRM_PRIME_2: u32 = 0x4000_0000;
const VA_EXPORT_SURFACE_READ_ONLY: u32 = 0x0001;
const VA_EXPORT_SURFACE_SEPARATE_LAYERS: u32 = 0x0004;

#[repr(C)]
struct VaConfigAttrib {
    type_: i32,
    value: u32,
}

/// Matches libva: type at 0, union value at 8 (16 bytes total).
#[repr(C)]
struct VaGenericValue {
    type_: i32,
    _pad: u32,
    value_i: i32,
    _pad2: u32,
}

#[repr(C)]
struct VaSurfaceAttrib {
    type_: i32,
    flags: u32,
    value: VaGenericValue,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct VaDrmPrimeSurfaceDescriptorObject {
    fd: i32,
    size: u32,
    drm_format_modifier: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct VaDrmPrimeSurfaceDescriptorLayer {
    drm_format: u32,
    num_planes: u32,
    object_index: [u32; 4],
    offset: [u32; 4],
    pitch: [u32; 4],
}

#[repr(C)]
struct VaDrmPrimeSurfaceDescriptor {
    fourcc: u32,
    width: u32,
    height: u32,
    num_objects: u32,
    objects: [VaDrmPrimeSurfaceDescriptorObject; 4],
    num_layers: u32,
    layers: [VaDrmPrimeSurfaceDescriptorLayer; 4],
}

type VaGetDisplayDrm = unsafe extern "C" fn(RawFd) -> *mut c_void;
type VaInitialize = unsafe extern "C" fn(*mut c_void, *mut c_int, *mut c_int) -> i32;
type VaTerminate = unsafe extern "C" fn(*mut c_void) -> i32;
type VaCreateConfig = unsafe extern "C" fn(
    *mut c_void,
    i32,
    i32,
    *mut VaConfigAttrib,
    c_int,
    *mut u32,
) -> i32;
type VaDestroyConfig = unsafe extern "C" fn(*mut c_void, u32) -> i32;
type VaCreateSurfaces = unsafe extern "C" fn(
    *mut c_void,
    c_uint,
    c_uint,
    c_uint,
    *mut u32,
    c_uint,
    *mut VaSurfaceAttrib,
    c_uint,
) -> i32;
type VaDestroySurfaces = unsafe extern "C" fn(*mut c_void, *mut u32, c_int) -> i32;
type VaExportSurfaceHandle =
    unsafe extern "C" fn(*mut c_void, u32, u32, u32, *mut c_void) -> i32;

struct VaProbeFns {
    _libva: ModuleLoader,
    _libva_drm: ModuleLoader,
    get_display_drm: VaGetDisplayDrm,
    initialize: VaInitialize,
    terminate: VaTerminate,
    create_config: VaCreateConfig,
    destroy_config: VaDestroyConfig,
    create_surfaces: VaCreateSurfaces,
    destroy_surfaces: VaDestroySurfaces,
    export_surface_handle: VaExportSurfaceHandle,
}

fn load_va() -> Option<VaProbeFns> {
    let libva = ModuleLoader::load("libva.so.2").ok()?;
    let libva_drm = ModuleLoader::load("libva-drm.so.2").ok()?;
    Some(VaProbeFns {
        get_display_drm: libva_drm.get_symbol("vaGetDisplayDRM").ok()?,
        initialize: libva.get_symbol("vaInitialize").ok()?,
        terminate: libva.get_symbol("vaTerminate").ok()?,
        create_config: libva.get_symbol("vaCreateConfig").ok()?,
        destroy_config: libva.get_symbol("vaDestroyConfig").ok()?,
        create_surfaces: libva.get_symbol("vaCreateSurfaces").ok()?,
        destroy_surfaces: libva.get_symbol("vaDestroySurfaces").ok()?,
        export_surface_handle: libva.get_symbol("vaExportSurfaceHandle").ok()?,
        _libva: libva,
        _libva_drm: libva_drm,
    })
}

fn open_render_node() -> Option<RawFd> {
    for path in [
        b"/dev/dri/renderD128\0".as_ptr(),
        b"/dev/dri/renderD129\0".as_ptr(),
        b"/dev/dri/card0\0".as_ptr(),
    ] {
        let fd = unsafe { open(path as *const _, 2) };
        if fd >= 0 {
            return Some(fd);
        }
    }
    None
}

static CACHE: Mutex<Option<(u32, u32, Option<u64>)>> = Mutex::new(None);

/// DRM modifier VA uses for an NV12 surface of `width`×`height`.
pub fn probe_nv12_modifier(width: u32, height: u32) -> Option<u64> {
    if width == 0 || height == 0 {
        return None;
    }
    if let Ok(cache) = CACHE.lock() {
        if let Some((w, h, m)) = *cache {
            if w == width && h == height {
                return m;
            }
        }
    }

    let result = (|| {
        let va = load_va()?;
        let drm_fd = match open_render_node() {
            Some(fd) => fd,
            None => {
                crate::log!("VIDEO: open /dev/dri/renderD* failed for VA probe");
                return None;
            }
        };
        let modifier = unsafe { probe_with_va(&va, drm_fd, width, height) };
        unsafe {
            close(drm_fd);
        }
        modifier
    })();

    if let Ok(mut cache) = CACHE.lock() {
        *cache = Some((width, height, result));
    }
    if let Some(m) = result {
        crate::log!(
            "VIDEO: VA-probed NV12 DRM modifier 0x{:x} for {}x{}",
            m,
            width,
            height
        );
    } else {
        crate::log!(
            "VIDEO: VA NV12 modifier probe failed for {}x{} (importing without modifier)",
            width,
            height
        );
    }
    result
}

unsafe fn probe_with_va(va: &VaProbeFns, drm_fd: RawFd, width: u32, height: u32) -> Option<u64> {
    let dpy = (va.get_display_drm)(drm_fd);
    if dpy.is_null() {
        return None;
    }
    let mut maj = 0;
    let mut min = 0;
    if (va.initialize)(dpy, &mut maj, &mut min) != VA_STATUS_SUCCESS {
        return None;
    }

    let mut attr = VaConfigAttrib {
        type_: VA_CONFIG_ATTRIB_RT_FORMAT,
        value: VA_RT_FORMAT_YUV420,
    };
    let mut cfg = 0u32;
    if (va.create_config)(
        dpy,
        VA_PROFILE_H264_MAIN,
        VA_ENTRYPOINT_VLD,
        &mut attr,
        1,
        &mut cfg,
    ) != VA_STATUS_SUCCESS
    {
        (va.terminate)(dpy);
        return None;
    }

    let mut sattr = VaSurfaceAttrib {
        type_: VA_SURFACE_ATTRIB_PIXEL_FORMAT,
        flags: VA_SURFACE_ATTRIB_SETTABLE,
        value: VaGenericValue {
            type_: VA_GENERIC_VALUE_TYPE_INTEGER,
            _pad: 0,
            value_i: VA_FOURCC_NV12 as i32,
            _pad2: 0,
        },
    };
    let mut surf = 0u32;
    let mut modifier = None;
    if (va.create_surfaces)(
        dpy,
        VA_RT_FORMAT_YUV420,
        width,
        height,
        &mut surf,
        1,
        &mut sattr,
        1,
    ) == VA_STATUS_SUCCESS
    {
        let zero_obj = VaDrmPrimeSurfaceDescriptorObject {
            fd: -1,
            size: 0,
            drm_format_modifier: 0,
        };
        let zero_layer = VaDrmPrimeSurfaceDescriptorLayer {
            drm_format: 0,
            num_planes: 0,
            object_index: [0; 4],
            offset: [0; 4],
            pitch: [0; 4],
        };
        let mut desc = VaDrmPrimeSurfaceDescriptor {
            fourcc: 0,
            width: 0,
            height: 0,
            num_objects: 0,
            objects: [zero_obj; 4],
            num_layers: 0,
            layers: [zero_layer; 4],
        };
        let st = (va.export_surface_handle)(
            dpy,
            surf,
            VA_SURFACE_ATTRIB_MEM_TYPE_DRM_PRIME_2,
            VA_EXPORT_SURFACE_READ_ONLY | VA_EXPORT_SURFACE_SEPARATE_LAYERS,
            &mut desc as *mut _ as *mut c_void,
        );
        if st == VA_STATUS_SUCCESS && desc.num_objects > 0 {
            modifier = Some(desc.objects[0].drm_format_modifier);
        }
        for obj in desc.objects.iter().take(desc.num_objects as usize) {
            if obj.fd >= 0 {
                close(obj.fd);
            }
        }
        (va.destroy_surfaces)(dpy, &mut surf, 1);
    }
    (va.destroy_config)(dpy, cfg);
    (va.terminate)(dpy);
    modifier
}
