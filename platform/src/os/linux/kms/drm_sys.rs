#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

#[link(name = "drm")]
extern "C" {
    pub fn drmGetDevices2(
        flags: u32,
        devices: *mut drmDevicePtr,
        max_devices: ::std::os::raw::c_int,
    ) -> ::std::os::raw::c_int;
    pub fn drmModeGetResources(fd: ::std::os::raw::c_int) -> drmModeResPtr;
    pub fn drmModeGetConnector(fd: ::std::os::raw::c_int, connectorId: u32) -> drmModeConnectorPtr;
    pub fn drmModeFreeConnector(ptr: drmModeConnectorPtr);
    pub fn drmModeFreeResources(ptr: drmModeResPtr);
    pub fn drmModeGetEncoder(fd: ::std::os::raw::c_int, encoder_id: u32) -> drmModeEncoderPtr;
    pub fn drmModeFreeEncoder(ptr: drmModeEncoderPtr);
    pub fn drmModeAddFB2(
        fd: ::std::os::raw::c_int,
        width: u32,
        height: u32,
        pixel_format: u32,
        bo_handles: *const u32,
        pitches: *const u32,
        offsets: *const u32,
        buf_id: *mut u32,
        flags: u32,
    ) -> ::std::os::raw::c_int;
    pub fn drmModeSetCrtc(
        fd: ::std::os::raw::c_int,
        crtcId: u32,
        bufferId: u32,
        x: u32,
        y: u32,
        connectors: *mut u32,
        count: ::std::os::raw::c_int,
        mode: drmModeModeInfoPtr,
    ) -> ::std::os::raw::c_int;
    pub fn drmModePageFlip(
        fd: ::std::os::raw::c_int,
        crtc_id: u32,
        fb_id: u32,
        flags: u32,
        user_data: *mut ::std::os::raw::c_void,
    ) -> ::std::os::raw::c_int;
    pub fn drmHandleEvent(
        fd: ::std::os::raw::c_int,
        evctx: drmEventContextPtr,
    ) -> ::std::os::raw::c_int;
}

pub const MAX_DRM_DEVICES: usize = 64;
pub const DRM_NODE_PRIMARY: u32 = 0;
pub const DRM_MODE_CONNECTED: drmModeConnection = 1;
pub const DRM_MODE_PAGE_FLIP_EVENT: u32 = 1;

pub type drmDevice = _drmDevice;
pub type drmDevicePtr = *mut _drmDevice;
pub type drmPciBusInfo = _drmPciBusInfo;
pub type drmPciBusInfoPtr = *mut _drmPciBusInfo;
pub type drmUsbBusInfo = _drmUsbBusInfo;
pub type drmUsbBusInfoPtr = *mut _drmUsbBusInfo;
pub type drmPlatformBusInfo = _drmPlatformBusInfo;
pub type drmPlatformBusInfoPtr = *mut _drmPlatformBusInfo;
pub type drmHost1xBusInfo = _drmHost1xBusInfo;
pub type drmHost1xBusInfoPtr = *mut _drmHost1xBusInfo;
pub type drmPciDeviceInfo = _drmPciDeviceInfo;
pub type drmPciDeviceInfoPtr = *mut _drmPciDeviceInfo;
pub type drmUsbDeviceInfo = _drmUsbDeviceInfo;
pub type drmUsbDeviceInfoPtr = *mut _drmUsbDeviceInfo;
pub type drmPlatformDeviceInfo = _drmPlatformDeviceInfo;
pub type drmPlatformDeviceInfoPtr = *mut _drmPlatformDeviceInfo;
pub type drmHost1xDeviceInfo = _drmHost1xDeviceInfo;
pub type drmHost1xDeviceInfoPtr = *mut _drmHost1xDeviceInfo;
pub type drmModeRes = _drmModeRes;
pub type drmModeResPtr = *mut _drmModeRes;
pub type drmModeConnector = _drmModeConnector;
pub type drmModeConnectorPtr = *mut _drmModeConnector;
pub type drmModeConnection = ::std::os::raw::c_uint;
pub type drmModeSubPixel = ::std::os::raw::c_uint;
pub type drmModeModeInfo = _drmModeModeInfo;
pub type drmModeModeInfoPtr = *mut _drmModeModeInfo;
pub type drmModeEncoder = _drmModeEncoder;
pub type drmModeEncoderPtr = *mut _drmModeEncoder;
pub type drmEventContext = _drmEventContext;
pub type drmEventContextPtr = *mut _drmEventContext;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _drmModeEncoder {
    pub encoder_id: u32,
    pub encoder_type: u32,
    pub crtc_id: u32,
    pub possible_crtcs: u32,
    pub possible_clones: u32,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _drmHost1xDeviceInfo {
    pub compatible: *mut *mut ::std::os::raw::c_char,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _drmPlatformDeviceInfo {
    pub compatible: *mut *mut ::std::os::raw::c_char,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _drmUsbDeviceInfo {
    pub vendor: u16,
    pub product: u16,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct _drmHost1xBusInfo {
    pub fullname: [::std::os::raw::c_char; 512usize],
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct _drmPlatformBusInfo {
    pub fullname: [::std::os::raw::c_char; 512usize],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _drmUsbBusInfo {
    pub bus: u8,
    pub dev: u8,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _drmPciBusInfo {
    pub domain: u16,
    pub bus: u8,
    pub dev: u8,
    pub func: u8,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct _drmDevice {
    pub nodes: *mut *mut ::std::os::raw::c_char,
    pub available_nodes: ::std::os::raw::c_int,
    pub bustype: ::std::os::raw::c_int,
    pub businfo: _drmDevice__bindgen_ty_1,
    pub deviceinfo: _drmDevice__bindgen_ty_2,
}
#[repr(C)]
#[derive(Copy, Clone)]
pub union _drmDevice__bindgen_ty_1 {
    pub pci: drmPciBusInfoPtr,
    pub usb: drmUsbBusInfoPtr,
    pub platform: drmPlatformBusInfoPtr,
    pub host1x: drmHost1xBusInfoPtr,
}
#[repr(C)]
#[derive(Copy, Clone)]
pub union _drmDevice__bindgen_ty_2 {
    pub pci: drmPciDeviceInfoPtr,
    pub usb: drmUsbDeviceInfoPtr,
    pub platform: drmPlatformDeviceInfoPtr,
    pub host1x: drmHost1xDeviceInfoPtr,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _drmPciDeviceInfo {
    pub vendor_id: u16,
    pub device_id: u16,
    pub subvendor_id: u16,
    pub subdevice_id: u16,
    pub revision_id: u8,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _drmModeRes {
    pub count_fbs: ::std::os::raw::c_int,
    pub fbs: *mut u32,
    pub count_crtcs: ::std::os::raw::c_int,
    pub crtcs: *mut u32,
    pub count_connectors: ::std::os::raw::c_int,
    pub connectors: *mut u32,
    pub count_encoders: ::std::os::raw::c_int,
    pub encoders: *mut u32,
    pub min_width: u32,
    pub max_width: u32,
    pub min_height: u32,
    pub max_height: u32,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _drmModeConnector {
    pub connector_id: u32,
    pub encoder_id: u32,
    pub connector_type: u32,
    pub connector_type_id: u32,
    pub connection: drmModeConnection,
    pub mmWidth: u32,
    pub mmHeight: u32,
    pub subpixel: drmModeSubPixel,
    pub count_modes: ::std::os::raw::c_int,
    pub modes: drmModeModeInfoPtr,
    pub count_props: ::std::os::raw::c_int,
    pub props: *mut u32,
    pub prop_values: *mut u64,
    pub count_encoders: ::std::os::raw::c_int,
    pub encoders: *mut u32,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _drmModeModeInfo {
    pub clock: u32,
    pub hdisplay: u16,
    pub hsync_start: u16,
    pub hsync_end: u16,
    pub htotal: u16,
    pub hskew: u16,
    pub vdisplay: u16,
    pub vsync_start: u16,
    pub vsync_end: u16,
    pub vtotal: u16,
    pub vscan: u16,
    pub vrefresh: u32,
    pub flags: u32,
    pub type_: u32,
    pub name: [::std::os::raw::c_char; 32usize],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _drmEventContext {
    pub version: ::std::os::raw::c_int,
    pub vblank_handler: ::std::option::Option<
        unsafe extern "C" fn(
            fd: ::std::os::raw::c_int,
            sequence: ::std::os::raw::c_uint,
            tv_sec: ::std::os::raw::c_uint,
            tv_usec: ::std::os::raw::c_uint,
            user_data: *mut ::std::os::raw::c_void,
        ),
    >,
    pub page_flip_handler: ::std::option::Option<
        unsafe extern "C" fn(
            fd: ::std::os::raw::c_int,
            sequence: ::std::os::raw::c_uint,
            tv_sec: ::std::os::raw::c_uint,
            tv_usec: ::std::os::raw::c_uint,
            user_data: *mut ::std::os::raw::c_void,
        ),
    >,
    pub page_flip_handler2: ::std::option::Option<
        unsafe extern "C" fn(
            fd: ::std::os::raw::c_int,
            sequence: ::std::os::raw::c_uint,
            tv_sec: ::std::os::raw::c_uint,
            tv_usec: ::std::os::raw::c_uint,
            crtc_id: ::std::os::raw::c_uint,
            user_data: *mut ::std::os::raw::c_void,
        ),
    >,
    pub sequence_handler: ::std::option::Option<
        unsafe extern "C" fn(fd: ::std::os::raw::c_int, sequence: u64, ns: u64, user_data: u64),
    >,
}
