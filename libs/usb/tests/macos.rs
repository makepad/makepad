#![cfg(target_os = "macos")]

use makepad_usb::{devices, Class, Device};
use std::collections::HashSet;
use std::ffi::{c_char, c_void, CStr};
use std::ptr::null;

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IORegistryEntryIDMatching(id: u64) -> *mut c_void;
    fn IOServiceGetMatchingService(port: u32, matching: *const c_void) -> u32;
    fn IORegistryEntryGetRegistryEntryID(entry: u32, id: *mut u64) -> i32;
    fn IORegistryEntryCreateCFProperty(entry: u32, key: *const c_void, allocator: *const c_void, options: u32) -> *const c_void;
    fn IOObjectRelease(object: u32) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFStringCreateWithCString(allocator: *const c_void, text: *const c_char, encoding: u32) -> *const c_void;
    fn CFGetTypeID(value: *const c_void) -> usize;
    fn CFNumberGetTypeID() -> usize;
    fn CFNumberGetValue(value: *const c_void, kind: isize, out: *mut c_void) -> u8;
    fn CFRelease(value: *const c_void);
}

struct Entry(u32);

impl Drop for Entry {
    fn drop(&mut self) {
        // SAFETY: this guard owns the nonzero reference returned by the matching lookup.
        unsafe { IOObjectRelease(self.0); }
    }
}

struct Property(*const c_void);

impl Drop for Property {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: each successful Create returns one reference owned by this guard.
            unsafe { CFRelease(self.0); }
        }
    }
}

fn number(service: &Entry, name: &CStr) -> u64 {
    // SAFETY: name is NUL-terminated; the allocator is the default and UTF-8 is a valid encoding.
    let key = Property(unsafe { CFStringCreateWithCString(null(), name.as_ptr(), 0x0800_0100) });
    assert!(!key.0.is_null(), "the registry property key is allocated");
    // SAFETY: service and key are retained; Create returns an owned property reference.
    let value = Property(unsafe { IORegistryEntryCreateCFProperty(service.0, key.0, null(), 0) });
    assert!(!value.0.is_null(), "listed device has registry property {}", name.to_string_lossy());
    let mut number = 0i64;
    // SAFETY: CF type is checked before requesting kCFNumberSInt64Type into writable i64 storage.
    unsafe {
        assert_eq!(CFGetTypeID(value.0), CFNumberGetTypeID(), "USB descriptor registry properties are numbers");
        assert_ne!(CFNumberGetValue(value.0, 4, (&mut number as *mut i64).cast()), 0, "USB registry number fits i64");
    }
    u64::try_from(number).expect("USB descriptor fields are nonnegative")
}

fn require_send<T: Send>() {}

#[test]
fn enumeration_agrees_with_registry_without_opening_devices() {
    require_send::<Device>();
    let listed = devices().expect("macOS USB enumeration succeeds without prompts or privileges");
    let mut ids = HashSet::new();
    for device in listed {
        assert!(ids.insert(device.id), "each listed DeviceId is distinct: {}", device.id);
        // The fields stay private: Display exposes the registry id without adding an accessor.
        let display = device.id.to_string();
        let id = u64::from_str_radix(display.strip_prefix("usb registry 0x")
            .expect("macOS DeviceId displays its registry entry id"), 16).expect("registry id is hexadecimal");
        // SAFETY: the numeric id creates a retained matching dictionary, consumed by the lookup below.
        let matching = unsafe { IORegistryEntryIDMatching(id) };
        assert!(!matching.is_null(), "registry id matching dictionary is allocated");
        // SAFETY: matching is valid and consumed; this lookup returns a service reference, never a user client.
        let service = unsafe { IOServiceGetMatchingService(0, matching) };
        assert_ne!(service, 0, "listed device remains registered: {}", device.id);
        let service = Entry(service);
        let mut registry_id = 0;
        // SAFETY: service is retained and registry_id is writable u64 storage.
        assert_eq!(unsafe { IORegistryEntryGetRegistryEntryID(service.0, &mut registry_id) }, 0,
            "listed service has a registry entry id");
        assert_eq!(registry_id, id, "DeviceId resolves to the same registry entry");
        assert_eq!(device.vendor as u64, number(&service, c"idVendor"), "vendor matches the registry");
        assert_eq!(device.product as u64, number(&service, c"idProduct"), "product matches the registry");
        assert_eq!(device.class, Class {
            class: number(&service, c"bDeviceClass") as u8,
            subclass: number(&service, c"bDeviceSubClass") as u8,
            protocol: number(&service, c"bDeviceProtocol") as u8,
        }, "device class triple matches the registry");
        assert!(device.interfaces.iter().all(|interface| interface.endpoints.is_empty()),
            "registry-only enumeration leaves endpoints for Device::configuration");
        assert_eq!(device.location, device.port, "{}: a macOS location id is already a path", device.id);
        for port in &device.serial_ports {
            assert!(port.starts_with("/dev/cu."), "{} serial callout path must start with /dev/cu., got {port}", device.id);
        }
        for pair in device.serial_ports.windows(2) {
            assert!(pair[0] < pair[1], "{} serial ports must be sorted and unique, got {pair:?}", device.id);
        }
    }
    // No device/user client is opened and no driver is detached by this test.
}
