#![allow(non_camel_case_types)]

#[link(name = "drm")]
extern "C" {
    pub fn drmGetDevices2(
        flags: u32,
        devices: *mut drmDevicePtr,
        max_devices: ::std::os::raw::c_int,
    ) -> ::std::os::raw::c_int;

}

pub const MAX_DRM_DEVICES:usize = 64;

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
