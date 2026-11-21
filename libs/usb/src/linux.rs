use super::*;
use crate::ffi;
use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

const SYSFS: &str = "/sys/bus/usb/devices";

fn node(id: DeviceId) -> String {
    format!("/dev/bus/usb/{:03}/{:03}", id.bus, id.address)
}

fn denied(path: &str) -> String {
    format!("no read-write permission on {path}; a udev rule grants it")
}

fn busy(interface: u8, driver: &str) -> String {
    format!("interface {interface} is bound to the {driver} driver")
}

fn optional_text(path: &Path) -> io::Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => {
            let text = text.trim();
            Ok(if text.is_empty() { None } else { Some(text.to_owned()) })
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

fn decimal<T: std::str::FromStr>(text: &str) -> io::Result<T> {
    text.trim().parse().map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid sysfs number"))
}

fn collect_tty_nodes(tty: &Path, ports: &mut Vec<String>) -> io::Result<()> {
    let entries = match fs::read_dir(tty) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };
    for entry in entries {
        let name = entry?.file_name();
        let name = name.to_str()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid sysfs tty name"))?;
        ports.push(format!("/dev/{name}"));
    }
    Ok(())
}

fn read_device(path: &Path, name: &str) -> io::Result<DeviceInfo> {
    let id = DeviceId {
        bus: decimal(&fs::read_to_string(path.join("busnum"))?)?,
        address: decimal(&fs::read_to_string(path.join("devnum"))?)?,
    };
    let parsed = descriptor::parse_device(&fs::read(path.join("descriptors"))?)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    let active = fs::read_to_string(path.join("bConfigurationValue"))?;
    let configuration = if active.trim().is_empty() {
        None
    } else {
        match decimal::<u8>(&active)? {
            0 => None,
            value => Some(value),
        }
    };
    let port = if name.starts_with("usb") { format!("{}-0", id.bus) } else { name.to_owned() };
    let real = fs::canonicalize(path)?;
    let location = real.strip_prefix("/sys/devices").unwrap_or(&real).to_string_lossy().into_owned();
    let mut interfaces = match configuration {
        Some(value) => parsed.configurations.into_iter().find(|config| config.value == value)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "active configuration has no descriptors"))?
            .interfaces,
        None => Vec::new(),
    };
    let mut serial_ports = Vec::new();
    if let Some(value) = configuration {
        for interface in &mut interfaces {
            // Root hub usbN has interface N-0:config.interface, just like its port path.
            let iface = Path::new(SYSFS).join(format!("{port}:{value}.{}", interface.number));
            interface.driver = match fs::read_link(iface.join("driver")) {
                Ok(driver) => Some(driver.file_name().and_then(|name| name.to_str())
                    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid driver name"))?.to_owned()),
                Err(err) if err.kind() == io::ErrorKind::NotFound => None,
                Err(err) => return Err(err),
            };
            let alternate = optional_text(&iface.join("bAlternateSetting"))?
                .map(|text| decimal::<u8>(&text)).transpose()?;
            if alternate == Some(interface.alternate) {
                interface.name = optional_text(&iface.join("interface"))?;
            }
            collect_tty_nodes(&iface.join("tty"), &mut serial_ports)?;
            let children = match fs::read_dir(&iface) {
                Ok(children) => children,
                Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
                Err(err) => return Err(err),
            };
            for child in children {
                let child = child?;
                if child.file_type()?.is_dir() {
                    collect_tty_nodes(&child.path().join("tty"), &mut serial_ports)?;
                }
            }
        }
    }
    serial_ports.sort_unstable();
    serial_ports.dedup();
    let speed = match optional_text(&path.join("speed"))?.as_deref() {
        Some("1.5") => Some(Speed::Low),
        Some("12") => Some(Speed::Full),
        Some("480") => Some(Speed::High),
        Some("5000") => Some(Speed::Super),
        Some("10000" | "20000") => Some(Speed::SuperPlus),
        _ => None,
    };
    let access = if let Some((number, driver)) = interfaces.iter()
        .find_map(|interface| interface.driver.as_deref().map(|driver| (interface.number, driver)))
    {
        Access::Busy(busy(number, driver))
    } else {
        let path = node(id);
        let c_path = CString::new(path.as_bytes())
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))?;
        // SAFETY: c_path is a live NUL-terminated path; access only checks its permissions.
        if unsafe { ffi::access(c_path.as_ptr(), ffi::R_OK | ffi::W_OK) } == 0 {
            Access::Ok
        } else {
            Access::Denied(denied(&path))
        }
    };
    Ok(DeviceInfo {
        id,
        vendor: parsed.vendor,
        product: parsed.product,
        release: parsed.release,
        usb: parsed.usb,
        class: parsed.class,
        manufacturer: optional_text(&path.join("manufacturer"))?,
        name: optional_text(&path.join("product"))?,
        serial: optional_text(&path.join("serial"))?,
        speed,
        port,
        location,
        configuration,
        interfaces,
        serial_ports,
        access,
    })
}

pub fn devices() -> io::Result<Vec<DeviceInfo>> {
    let entries = fs::read_dir(SYSFS).map_err(|err| call_error("devices", None, err))?;
    let mut devices = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.contains(':') {
            continue;
        }
        // A single unreadable, malformed or unplugged entry must not hide the others.
        if let Ok(device) = read_device(&entry.path(), name) {
            devices.push(device);
        }
    }
    devices.sort_by_key(|device| (device.id.bus, device.id.address));
    Ok(devices)
}

pub fn open(id: DeviceId) -> io::Result<Device> {
    let path = node(id);
    let file = OpenOptions::new().read(true).write(true).custom_flags(ffi::O_CLOEXEC)
        .open(&path).map_err(|err| {
            if err.kind() == io::ErrorKind::PermissionDenied {
                io::Error::new(io::ErrorKind::PermissionDenied, denied(&path))
            } else if err.kind() == io::ErrorKind::NotFound
                || matches!(err.raw_os_error(), Some(ffi::ENODEV | ffi::ESHUTDOWN))
            {
                io::Error::new(io::ErrorKind::NotFound, format!("open {id}: the device is gone"))
            } else {
                call_error(&format!("open {id}"), None, err)
            }
        })?;
    Ok(Device { file, id, claimed: [false; 256] })
}

pub struct Device {
    file: File,
    id: DeviceId,
    claimed: [bool; 256],
}

fn call_error(call: &str, endpoint: Option<u8>, err: io::Error) -> io::Error {
    match (err.raw_os_error(), endpoint) {
        (Some(ffi::ETIMEDOUT), _) => io::Error::new(io::ErrorKind::TimedOut, format!("{call}: transfer timed out")),
        (Some(ffi::ENODEV | ffi::ESHUTDOWN), _) => io::Error::new(io::ErrorKind::NotConnected, format!("{call}: the device is gone")),
        (Some(ffi::EPIPE), Some(endpoint)) => io::Error::new(io::ErrorKind::BrokenPipe,
            format!("{call}: endpoint 0x{endpoint:02x} stalled; clear_halt resets it")),
        (Some(ffi::ENOMEM), Some(_)) => io::Error::new(err.kind(), format!(
            "{call}: the kernel refused the USB memory allocation; the usbfs memory limit is /sys/module/usbcore/parameters/usbfs_memory_mb: {err}")),
        _ => io::Error::new(err.kind(), format!("{call}: {err}")),
    }
}

fn check(rc: ffi::c_int, call: &str, endpoint: Option<u8>) -> io::Result<usize> {
    if rc < 0 {
        Err(call_error(call, endpoint, io::Error::last_os_error()))
    } else {
        Ok(rc as usize)
    }
}

fn timeout_ms(timeout: Duration) -> u32 {
    if timeout == Duration::MAX {
        return 0;
    }
    let millis = timeout.as_millis() + u128::from(timeout.subsec_nanos() % 1_000_000 != 0);
    if millis > u32::MAX as u128 { 0 } else { millis.max(1) as u32 }
}

// Keep the Rust borrow alive through the synchronous ioctl. OUT buffers are
// cast to the UAPI's mutable pointer only after checking the transfer direction.
enum Buffer<'a> {
    In(&'a mut [u8]),
    Out(&'a [u8]),
}

impl Buffer<'_> {
    fn is_in(&self) -> bool { matches!(self, Self::In(_)) }

    fn len(&self) -> usize {
        match self { Self::In(data) => data.len(), Self::Out(data) => data.len() }
    }

    fn pointer(&mut self) -> *mut ffi::c_void {
        match self {
            Self::In(data) => data.as_mut_ptr().cast(),
            Self::Out(data) => data.as_ptr().cast_mut().cast(),
        }
    }

    fn check_direction(&self, address: u8, call: &str) -> io::Result<()> {
        if (address & 0x80 != 0) == self.is_in() {
            Ok(())
        } else {
            Err(io::Error::new(io::ErrorKind::InvalidInput, format!("{call}: direction bit does not match the call")))
        }
    }
}

impl Device {
    pub fn configuration(&mut self) -> io::Result<Vec<InterfaceInfo>> {
        self.file.seek(SeekFrom::Start(0)).map_err(|err| call_error("configuration", None, err))?;
        let mut bytes = Vec::new();
        self.file.read_to_end(&mut bytes).map_err(|err| call_error("configuration", None, err))?;
        let parsed = descriptor::parse_device(&bytes)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        // usbfs returns all configurations. Read the active value and cached strings
        // from this device's sysfs entry without issuing a control request.
        for entry in fs::read_dir(SYSFS)? {
            let path = entry?.path();
            let bus = optional_text(&path.join("busnum"))?.map(|s| decimal::<u32>(&s)).transpose()?;
            let address = optional_text(&path.join("devnum"))?.map(|s| decimal::<u32>(&s)).transpose()?;
            if bus != Some(self.id.bus) || address != Some(self.id.address) { continue }
            let name = path.file_name().and_then(|name| name.to_str())
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid sysfs device name"))?;
            let cached = read_device(&path, name)?;
            let Some(value) = cached.configuration else { return Ok(Vec::new()) };
            let mut interfaces = parsed.configurations.into_iter().find(|config| config.value == value)
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "active configuration has no descriptors"))?
                .interfaces;
            for interface in &mut interfaces {
                if let Some(info) = cached.interfaces.iter().find(|info| info.number == interface.number && info.alternate == interface.alternate) {
                    interface.name = info.name.clone();
                    interface.driver = info.driver.clone();
                }
            }
            return Ok(interfaces);
        }
        Err(io::Error::new(io::ErrorKind::NotConnected, "configuration: the device is gone"))
    }

    fn driver(&self, interface: u8) -> io::Result<String> {
        let mut request = ffi::usbdevfs_getdriver { interface: interface as ffi::c_uint, driver: [0; 256] };
        let call = format!("get_driver {interface}");
        // SAFETY: the fd is live and request is writable storage with the GETDRIVER C layout.
        let rc = unsafe { ffi::ioctl(self.file.as_raw_fd(), ffi::USBDEVFS_GETDRIVER, &mut request) };
        check(rc, &call, None)?;
        let bytes: Vec<u8> = request.driver.iter().take_while(|byte| **byte != 0).map(|byte| *byte as u8).collect();
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    pub fn claim(&mut self, interface: u8) -> io::Result<()> {
        let mut number = interface as ffi::c_uint;
        let call = format!("claim {interface}");
        // SAFETY: the fd is live and number has the unsigned-int layout CLAIMINTERFACE reads.
        let rc = unsafe { ffi::ioctl(self.file.as_raw_fd(), ffi::USBDEVFS_CLAIMINTERFACE, &mut number) };
        if rc < 0 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(ffi::EBUSY) {
                let reason = match self.driver(interface) {
                    Ok(driver) if !driver.is_empty() => busy(interface, &driver),
                    _ => format!("interface {interface} is already claimed"),
                };
                return Err(io::Error::new(io::ErrorKind::ResourceBusy, format!("{call}: {reason}")));
            }
            return Err(call_error(&call, None, err));
        }
        self.claimed[interface as usize] = true;
        Ok(())
    }

    pub fn release(&mut self, interface: u8) -> io::Result<()> {
        let mut number = interface as ffi::c_uint;
        let call = format!("release {interface}");
        // SAFETY: the fd is live and number has the unsigned-int layout RELEASEINTERFACE reads.
        let rc = unsafe { ffi::ioctl(self.file.as_raw_fd(), ffi::USBDEVFS_RELEASEINTERFACE, &mut number) };
        check(rc, &call, None)?;
        self.claimed[interface as usize] = false;
        Ok(())
    }

    pub fn release_all(&mut self) {
        for number in 0..self.claimed.len() {
            if self.claimed[number] {
                // Best effort on drop, including unplug: closing file also releases usbfs claims.
                let _ = self.release(number as u8);
            }
        }
    }

    pub fn set_alternate(&mut self, interface: u8, alternate: u8) -> io::Result<()> {
        let mut request = ffi::usbdevfs_setinterface { interface: interface as ffi::c_uint, altsetting: alternate as ffi::c_uint };
        let call = format!("set_alternate {interface} {alternate}");
        // SAFETY: the fd is live and request has the SETINTERFACE C layout and no pointers.
        let rc = unsafe { ffi::ioctl(self.file.as_raw_fd(), ffi::USBDEVFS_SETINTERFACE, &mut request) };
        check(rc, &call, None).map(|_| ())
    }

    pub fn detach_driver(&mut self, interface: u8) -> io::Result<()> {
        let mut request = ffi::usbdevfs_ioctl {
            ifno: interface as ffi::c_int,
            ioctl_code: ffi::USBDEVFS_DISCONNECT as ffi::c_int,
            data: std::ptr::null_mut(),
        };
        let call = format!("detach_driver {interface}");
        // SAFETY: the fd is live, request has the IOCTL C layout, and DISCONNECT takes no data.
        let rc = unsafe { ffi::ioctl(self.file.as_raw_fd(), ffi::USBDEVFS_IOCTL, &mut request) };
        check(rc, &call, None).map(|_| ())
    }

    fn control(&mut self, call: &str, setup: Setup, mut data: Buffer<'_>, timeout: Duration) -> io::Result<usize> {
        data.check_direction(setup.request_type, call)?;
        let length = u16::try_from(data.len()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput,
            format!("{call}: buffer length does not fit USB's 16-bit wLength")))?;
        let mut request = ffi::usbdevfs_ctrltransfer {
            bRequestType: setup.request_type,
            bRequest: setup.request,
            wValue: setup.value,
            wIndex: setup.index,
            wLength: length,
            timeout: timeout_ms(timeout),
            data: data.pointer(),
        };
        // SAFETY: request has the CONTROL C layout; its live buffer is writable only for checked IN requests, and ioctl is synchronous.
        let rc = unsafe { ffi::ioctl(self.file.as_raw_fd(), ffi::USBDEVFS_CONTROL, &mut request) };
        check(rc, call, Some(setup.request_type & 0x80))
    }

    pub fn control_in(&mut self, setup: Setup, data: &mut [u8], timeout: Duration) -> io::Result<usize> {
        self.control("control_in", setup, Buffer::In(data), timeout)
    }

    pub fn control_out(&mut self, setup: Setup, data: &[u8], timeout: Duration) -> io::Result<usize> {
        self.control("control_out", setup, Buffer::Out(data), timeout)
    }

    fn transfer(&mut self, operation: &str, endpoint: u8, mut data: Buffer<'_>, timeout: Duration) -> io::Result<usize> {
        let call = format!("{operation} 0x{endpoint:02x}");
        data.check_direction(endpoint, &call)?;
        let length = ffi::c_uint::try_from(data.len()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput,
            format!("{call}: buffer length does not fit usbfs's 32-bit length")))?;
        let mut request = ffi::usbdevfs_bulktransfer {
            ep: endpoint as ffi::c_uint,
            len: length,
            timeout: timeout_ms(timeout),
            data: data.pointer(),
        };
        // usbfs chooses bulk or interrupt from the device's endpoint descriptor.
        // SAFETY: request has the BULK C layout; its live buffer is writable only for checked IN endpoints, and ioctl is synchronous.
        let rc = unsafe { ffi::ioctl(self.file.as_raw_fd(), ffi::USBDEVFS_BULK, &mut request) };
        check(rc, &call, Some(endpoint))
    }

    pub fn bulk_in(&mut self, endpoint: u8, data: &mut [u8], timeout: Duration) -> io::Result<usize> {
        self.transfer("bulk_in", endpoint, Buffer::In(data), timeout)
    }

    pub fn bulk_out(&mut self, endpoint: u8, data: &[u8], timeout: Duration) -> io::Result<usize> {
        self.transfer("bulk_out", endpoint, Buffer::Out(data), timeout)
    }

    pub fn interrupt_in(&mut self, endpoint: u8, data: &mut [u8], timeout: Duration) -> io::Result<usize> {
        self.transfer("interrupt_in", endpoint, Buffer::In(data), timeout)
    }

    pub fn interrupt_out(&mut self, endpoint: u8, data: &[u8], timeout: Duration) -> io::Result<usize> {
        self.transfer("interrupt_out", endpoint, Buffer::Out(data), timeout)
    }

    pub fn clear_halt(&mut self, endpoint: u8) -> io::Result<()> {
        let mut address = endpoint as ffi::c_uint;
        let call = format!("clear_halt 0x{endpoint:02x}");
        // SAFETY: the fd is live and address has the unsigned-int layout CLEAR_HALT reads.
        let rc = unsafe { ffi::ioctl(self.file.as_raw_fd(), ffi::USBDEVFS_CLEAR_HALT, &mut address) };
        check(rc, &call, Some(endpoint)).map(|_| ())
    }
}
