//! Raw USB for a worker that owns a device: every call blocks.
//! Enumeration reads the OS's cached descriptors and strings without opening
//! device nodes. Only an explicit [`Device::detach_driver`] detaches a driver.
//! Timeouts apply per transfer: finite values round up to milliseconds, at
//! least one; [`Duration::MAX`] and values above `u32::MAX` milliseconds wait
//! without a deadline. Transfers submit the caller's buffer in one request.
use std::cell::Cell;
use std::fmt;
use std::io;
use std::marker::PhantomData;
use std::time::Duration;

pub mod descriptor;

/// A device for this session: stable while it stays plugged in; a replug gives a new id.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct DeviceId {
    #[cfg(target_os = "macos")]
    registry: u64,
    #[cfg(not(target_os = "macos"))]
    bus: u32,
    #[cfg(not(target_os = "macos"))]
    address: u32,
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        #[cfg(target_os = "macos")]
        return write!(f, "usb registry 0x{:x}", self.registry);
        #[cfg(not(target_os = "macos"))]
        write!(f, "usb {}-{}", self.bus, self.address)
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DeviceInfo {
    pub id: DeviceId,
    pub vendor: u16,
    pub product: u16,
    /// bcdDevice, in binary-coded decimal.
    pub release: u16,
    /// bcdUSB, in binary-coded decimal.
    pub usb: u16,
    pub class: Class,
    pub manufacturer: Option<String>,
    /// The product string.
    pub name: Option<String>,
    pub serial: Option<String>,
    pub speed: Option<Speed>,
    /// The topology path (`1-2.3`, or a macOS location id such as `0x14100000`).
    /// A replug on the same port keeps it.
    pub port: String,
    /// The port as a path that bus renumbering does not change: on Linux the device's path below
    /// `/sys/devices` (its controller, then the port chain); on macOS the location id, as `port`.
    pub location: String,
    /// The active configuration value, or None when unconfigured.
    pub configuration: Option<u8>,
    /// Every alternate setting of the active configuration; on macOS, only the
    /// interface services currently in the registry. See [`Device::configuration`].
    pub interfaces: Vec<InterfaceInfo>,
    /// The serial port nodes the OS made for this device's interfaces (macOS callout devices such
    /// as `/dev/cu.usbmodem1101`; Linux `/dev/ttyACM0`, `/dev/ttyUSB0`), read from the registry or
    /// sysfs: enumeration never opens them. Sorted; empty when no serial driver is bound.
    pub serial_ports: Vec<String>,
    pub access: Access,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Class {
    pub class: u8,
    pub subclass: u8,
    pub protocol: u8,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Speed { Low, Full, High, Super, SuperPlus }

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct InterfaceInfo {
    pub number: u8,
    pub alternate: u8,
    pub class: Class,
    /// Present only when the OS already has this setting's string.
    pub name: Option<String>,
    /// Empty in macOS enumeration: the registry does not contain endpoints.
    /// Use [`Device::configuration`] on an open device to read them.
    pub endpoints: Vec<EndpointInfo>,
    /// The bound OS driver's name, or None when free.
    pub driver: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct EndpointInfo {
    pub address: u8,
    pub transfer: Transfer,
    /// Bytes per packet: wMaxPacketSize bits 0..10, without the high-bandwidth bits 11..12.
    pub max_packet: u16,
    pub interval: u8,
}

impl EndpointInfo {
    pub fn is_in(&self) -> bool {
        self.address & 0x80 != 0
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Transfer { Control, Isochronous, Bulk, Interrupt }

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Access {
    Ok,
    /// The user may not open it; the text says what is missing, in words for a person.
    Denied(String),
    /// Another driver holds an interface, named in the text.
    Busy(String),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Setup {
    pub request_type: u8,
    pub request: u8,
    pub value: u16,
    pub index: u16,
}

// Only these Linux architectures use the asm-generic ioctl ABI declared in ffi.
// Android needs the platform's permission flow and remains unsupported here.
macro_rules! usb {
    ({ $($item:item)* } else { $($other:item)* }) => {
        $(#[cfg(all(
            target_os = "linux",
            any(
                target_arch = "x86",
                target_arch = "x86_64",
                target_arch = "arm",
                target_arch = "aarch64",
                target_arch = "riscv32",
                target_arch = "riscv64",
                target_arch = "loongarch64"
            )
        ))]
        $item)*
        $(#[cfg(not(all(
            target_os = "linux",
            any(
                target_arch = "x86",
                target_arch = "x86_64",
                target_arch = "arm",
                target_arch = "aarch64",
                target_arch = "riscv32",
                target_arch = "riscv64",
                target_arch = "loongarch64"
            )
        )))]
        $other)*
    };
}

usb! {
    {
        mod ffi;
        #[path = "linux.rs"]
        mod backend;
    } else {
        #[cfg(not(target_os = "macos"))]
        #[path = "unsupported.rs"]
        mod backend;
    }
}

#[cfg(target_os = "macos")]
mod apple_ffi;
#[cfg(target_os = "macos")]
#[path = "macos.rs"]
mod backend;

/// Every USB device the OS lists, without opening any of them (no permission needed to list).
/// Targets without a backend return [`io::ErrorKind::Unsupported`].
pub fn devices() -> io::Result<Vec<DeviceInfo>> {
    backend::devices()
}

/// Open a device for transfers. Fails with PermissionDenied (the same words as
/// [`Access::Denied`]) or NotFound when it is gone. On macOS this only retains
/// the registry service; user-client access is checked by claim or a device operation.
pub fn open(id: DeviceId) -> io::Result<Device> {
    Ok(Device { inner: backend::open(id)?, _not_sync: PhantomData })
}

/// An owned USB device. Send, but not Sync: one worker owns its transfers.
/// Dropping it releases every claimed interface, without reattaching drivers.
pub struct Device {
    inner: backend::Device,
    _not_sync: PhantomData<Cell<()>>,
}

impl Device {
    /// Read every alternate setting and endpoint of the active configuration.
    /// On macOS this uses a claimed interface, or opens a device user client with
    /// no capture or seize options; a bound device driver can prevent that open.
    pub fn configuration(&mut self) -> io::Result<Vec<InterfaceInfo>> {
        self.inner.configuration()
    }

    /// Claim an interface; ResourceBusy names its driver when one is bound.
    pub fn claim(&mut self, interface: u8) -> io::Result<()> {
        self.inner.claim(interface)
    }

    pub fn release(&mut self, interface: u8) -> io::Result<()> {
        self.inner.release(interface)
    }

    pub fn set_alternate(&mut self, interface: u8, alternate: u8) -> io::Result<()> {
        self.inner.set_alternate(interface, alternate)
    }

    /// Detach the OS driver bound to `interface` so it can be claimed. A system change the user
    /// sees (the OS loses the device until it is replugged): the caller asks the person first.
    /// Never called by this crate on its own. Always Unsupported on macOS, where
    /// detaching needs an entitlement this app does not have.
    pub fn detach_driver(&mut self, interface: u8) -> io::Result<()> {
        self.inner.detach_driver(interface)
    }

    /// The setup direction bit must be IN. The buffer length becomes wLength.
    pub fn control_in(&mut self, setup: Setup, data: &mut [u8], timeout: Duration) -> io::Result<usize> {
        self.inner.control_in(setup, data, timeout)
    }

    /// The setup direction bit must be OUT. The buffer length becomes wLength.
    pub fn control_out(&mut self, setup: Setup, data: &[u8], timeout: Duration) -> io::Result<usize> {
        self.inner.control_out(setup, data, timeout)
    }

    pub fn bulk_in(&mut self, endpoint: u8, data: &mut [u8], timeout: Duration) -> io::Result<usize> {
        self.inner.bulk_in(endpoint, data, timeout)
    }

    pub fn bulk_out(&mut self, endpoint: u8, data: &[u8], timeout: Duration) -> io::Result<usize> {
        self.inner.bulk_out(endpoint, data, timeout)
    }

    pub fn interrupt_in(&mut self, endpoint: u8, data: &mut [u8], timeout: Duration) -> io::Result<usize> {
        self.inner.interrupt_in(endpoint, data, timeout)
    }

    pub fn interrupt_out(&mut self, endpoint: u8, data: &[u8], timeout: Duration) -> io::Result<usize> {
        self.inner.interrupt_out(endpoint, data, timeout)
    }

    pub fn clear_halt(&mut self, endpoint: u8) -> io::Result<()> {
        self.inner.clear_halt(endpoint)
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        self.inner.release_all();
    }
}
