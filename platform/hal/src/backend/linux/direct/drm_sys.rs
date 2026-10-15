//! libdrm bindings used by KHR_display mode selection and console restoration.
#![allow(non_snake_case)]
use std::ffi::{c_char, c_int, c_uint};
#[link(name = "drm")]
extern "C" {
    pub fn drmModeGetResources(fd: c_int) -> *mut Resources;
    pub fn drmModeFreeResources(resources: *mut Resources);
    pub fn drmModeGetConnector(fd: c_int, connector: u32) -> *mut Connector;
    pub fn drmModeFreeConnector(connector: *mut Connector);
    pub fn drmModeGetEncoder(fd: c_int, encoder: u32) -> *mut Encoder;
    pub fn drmModeFreeEncoder(encoder: *mut Encoder);
    pub fn drmModeGetCrtc(fd: c_int, crtc: u32) -> *mut Crtc;
    pub fn drmModeFreeCrtc(crtc: *mut Crtc);
    pub fn drmModeSetCrtc(fd: c_int, crtc: u32, buffer: u32, x: u32, y: u32,
        connectors: *mut u32, count: c_int, mode: *mut Mode) -> c_int;
    pub fn drmSetMaster(fd: c_int) -> c_int;
    pub fn drmDropMaster(fd: c_int) -> c_int;
}
#[derive(Clone, Copy)]
#[repr(C)]
pub struct Crtc {
    pub crtc_id: u32,
    pub buffer_id: u32,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub mode_valid: c_int,
    pub mode: Mode,
    pub gamma_size: c_int,
}
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct Encoder {
    pub encoder_id: u32,
    pub encoder_type: u32,
    pub crtc_id: u32,
    pub possible_crtcs: u32,
    pub possible_clones: u32,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct Resources {
    pub count_fbs: c_int,
    pub fbs: *mut u32,
    pub count_crtcs: c_int,
    pub crtcs: *mut u32,
    pub count_connectors: c_int,
    pub connectors: *mut u32,
    pub count_encoders: c_int,
    pub encoders: *mut u32,
    pub min_width: u32,
    pub max_width: u32,
    pub min_height: u32,
    pub max_height: u32,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct Connector {
    pub connector_id: u32,
    pub encoder_id: u32,
    pub connector_type: u32,
    pub connector_type_id: u32,
    pub connection: c_uint,
    pub mmWidth: u32,
    pub mmHeight: u32,
    pub subpixel: c_uint,
    pub count_modes: c_int,
    pub modes: *mut Mode,
    pub count_props: c_int,
    pub props: *mut u32,
    pub prop_values: *mut u64,
    pub count_encoders: c_int,
    pub encoders: *mut u32,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct Mode {
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
    pub name: [c_char; 32usize],
}
