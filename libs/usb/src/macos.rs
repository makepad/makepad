use super::*;
use crate::apple_ffi::{self as ffi, message, Bool, IOReturn, Object, Sel};
use std::collections::BTreeMap;
use std::ffi::{c_void, CStr};
use std::ptr::{null, null_mut};
use std::sync::{Arc, Condvar, Mutex};

struct Service(ffi::Io);

impl Drop for Service {
    fn drop(&mut self) {
        // SAFETY: this wrapper owns exactly one nonzero IOKit object reference.
        unsafe { ffi::IOObjectRelease(self.0); }
    }
}

impl Service {
    fn conforms(&self, name: &CStr) -> bool {
        // SAFETY: the object is retained and name is NUL-terminated.
        unsafe { ffi::IOObjectConformsTo(self.0, name.as_ptr()) != 0 }
    }

    fn class(&self) -> io::Result<String> {
        let mut name = [0; 128]; // io_name_t
        // SAFETY: name is writable storage for the SDK's 128-byte io_name_t.
        check("IOObjectGetClass", unsafe { ffi::IOObjectGetClass(self.0, name.as_mut_ptr()) })?;
        let bytes: Vec<u8> = name.iter().take_while(|byte| **byte != 0).map(|byte| *byte as u8).collect();
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    fn id(&self) -> io::Result<DeviceId> {
        let mut registry = 0;
        // SAFETY: the retained entry and writable u64 match the SDK declaration.
        check("IORegistryEntryGetRegistryEntryID", unsafe { ffi::IORegistryEntryGetRegistryEntryID(self.0, &mut registry) })?;
        Ok(DeviceId { registry })
    }

    fn children(&self) -> io::Result<Vec<Service>> {
        let mut iterator = 0;
        // SAFETY: the entry is retained, the plane is NUL-terminated, and iterator is writable.
        check("IORegistryEntryGetChildIterator", unsafe {
            ffi::IORegistryEntryGetChildIterator(self.0, c"IOService".as_ptr(), &mut iterator)
        })?;
        Ok(collect(Service(iterator)))
    }

    fn property(&self, name: &CStr) -> Option<Property> {
        // SAFETY: name is NUL-terminated; Create returns an owned CFString or null.
        let key = Property(unsafe { ffi::CFStringCreateWithCString(null(), name.as_ptr(), ffi::UTF8) });
        if key.0.is_null() { return None }
        // SAFETY: entry and key are live; Create returns an owned property or null.
        let property = unsafe { ffi::IORegistryEntryCreateCFProperty(self.0, key.0, null(), 0) };
        if property.is_null() { None } else { Some(Property(property)) }
    }

    fn number(&self, name: &CStr) -> Option<u64> {
        let property = self.property(name)?;
        let mut value = 0i64;
        // SAFETY: the CF type is checked before reading into a correctly sized signed integer.
        unsafe {
            if ffi::CFGetTypeID(property.0) != ffi::CFNumberGetTypeID()
                || ffi::CFNumberGetValue(property.0, ffi::SINT64, (&mut value as *mut i64).cast()) == 0
            {
                return None;
            }
        }
        u64::try_from(value).ok()
    }

    fn required(&self, name: &CStr) -> io::Result<u64> {
        self.number(name).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData,
            format!("missing USB registry property {}", name.to_string_lossy())))
    }

    fn text(&self, name: &CStr) -> Option<String> {
        let property = self.property(name)?;
        // SAFETY: the type is checked and the UTF-8 buffer includes space for the terminator.
        unsafe {
            if ffi::CFGetTypeID(property.0) != ffi::CFStringGetTypeID() { return None }
            let size = ffi::CFStringGetMaximumSizeForEncoding(ffi::CFStringGetLength(property.0), ffi::UTF8).checked_add(1)?;
            let mut bytes = vec![0u8; usize::try_from(size).ok()?];
            if ffi::CFStringGetCString(property.0, bytes.as_mut_ptr().cast(), size, ffi::UTF8) == 0 { return None }
            let end = bytes.iter().position(|byte| *byte == 0)?;
            if end == 0 { None } else { Some(String::from_utf8_lossy(&bytes[..end]).into_owned()) }
        }
    }
}

struct Property(ffi::Cf);

impl Drop for Property {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: this wrapper owns the Create reference, released exactly once.
            unsafe { ffi::CFRelease(self.0); }
        }
    }
}

fn collect(iterator: Service) -> Vec<Service> {
    let mut entries = Vec::new();
    loop {
        // SAFETY: iterator stays retained; each nonzero result transfers an owned reference.
        let entry = unsafe { ffi::IOIteratorNext(iterator.0) };
        if entry == 0 { break }
        entries.push(Service(entry));
    }
    entries
}

fn find(id: DeviceId) -> io::Result<Option<Service>> {
    // SAFETY: the integer registry id requires no pointer arguments; the dictionary is owned.
    let matching = unsafe { ffi::IORegistryEntryIDMatching(id.registry) };
    if matching.is_null() { return Err(io::Error::other("IORegistryEntryIDMatching: allocation failed")) }
    // SAFETY: matching is consumed by the lookup; the default main port needs no allocation.
    let service = unsafe { ffi::IOServiceGetMatchingService(0, matching) };
    Ok(if service == 0 { None } else { Some(Service(service)) })
}

fn composite(service: &Service) -> bool {
    service.conforms(c"AppleUSBCompositeDevice") || service.conforms(c"IOUSBCompositeDriver")
}

fn interface_client(service: &Service) -> bool {
    service.conforms(c"IOUSBHostInterfaceUserClient") || service.conforms(c"AppleUSBHostInterfaceUserClient")
}

fn interface_services(service: &Service) -> io::Result<Vec<Service>> {
    let mut interfaces = Vec::new();
    for child in service.children()? {
        if child.conforms(c"IOUSBHostInterface") {
            interfaces.push(child);
        } else if composite(&child) {
            // Some OS versions publish interface services below the composite driver.
            interfaces.extend(interface_services(&child)?);
        }
    }
    Ok(interfaces)
}

fn interface_driver(service: &Service, include_user_client: bool) -> io::Result<Option<String>> {
    for child in service.children()? {
        if child.conforms(c"IOService") && (include_user_client || !interface_client(&child)) {
            return child.class().map(Some);
        }
    }
    Ok(None)
}

fn device_driver(service: &Service) -> io::Result<Option<String>> {
    for child in service.children()? {
        if child.conforms(c"IOService") && !child.conforms(c"IOUSBHostInterface") && !composite(&child) {
            return child.class().map(Some);
        }
    }
    Ok(None)
}

fn busy(number: u8, driver: &str) -> String {
    format!("interface {number} is bound to the {driver} driver")
}

fn interface_info(service: &Service) -> io::Result<InterfaceInfo> {
    Ok(InterfaceInfo {
        number: service.required(c"bInterfaceNumber")? as u8,
        alternate: service.required(c"bAlternateSetting")? as u8,
        class: Class {
            class: service.required(c"bInterfaceClass")? as u8,
            subclass: service.required(c"bInterfaceSubClass")? as u8,
            protocol: service.required(c"bInterfaceProtocol")? as u8,
        },
        name: service.text(c"kUSBString"), // kUSBInterfaceString
        endpoints: Vec::new(),
        driver: interface_driver(service, false)?,
    })
}

/// The callout devices of the serial clients below `service`. A device behind a hub is a child of
/// the hub's device service: its ports are its own, so the walk stops at another USB device.
fn collect_serial_ports(service: &Service, ports: &mut Vec<String>) {
    let Ok(children) = service.children() else { return };
    for child in children {
        if child.conforms(c"IOUSBHostDevice") {
            continue;
        }
        if child.conforms(c"IOSerialBSDClient") {
            ports.extend(child.text(c"IOCalloutDevice"));
        }
        collect_serial_ports(&child, ports);
    }
}

fn read_device(service: &Service) -> io::Result<DeviceInfo> {
    let mut interfaces = interface_services(service)?.iter().map(interface_info).collect::<io::Result<Vec<_>>>()?;
    interfaces.sort_by_key(|interface| (interface.number, interface.alternate));
    let port = format!("0x{:08x}", service.required(c"locationID")?);
    let mut serial_ports = Vec::new();
    collect_serial_ports(service, &mut serial_ports);
    serial_ports.sort_unstable();
    serial_ports.dedup();
    let access = if let Some(interface) = interfaces.iter().find(|interface| interface.driver.is_some()) {
        Access::Busy(busy(interface.number, interface.driver.as_deref().unwrap_or_default()))
    } else if let Some(driver) = device_driver(service)? {
        Access::Busy(format!("device is bound to the {driver} driver"))
    } else {
        Access::Ok
    };
    Ok(DeviceInfo {
        id: service.id()?,
        vendor: service.required(c"idVendor")? as u16,
        product: service.required(c"idProduct")? as u16,
        release: service.required(c"bcdDevice")? as u16,
        usb: service.required(c"bcdUSB")? as u16,
        class: Class {
            class: service.required(c"bDeviceClass")? as u8,
            subclass: service.required(c"bDeviceSubClass")? as u8,
            protocol: service.required(c"bDeviceProtocol")? as u8,
        },
        manufacturer: service.text(c"kUSBVendorString"),
        name: service.text(c"kUSBProductString"),
        serial: service.text(c"kUSBSerialNumberString"),
        speed: match service.number(c"USBSpeed").or_else(|| service.number(c"Device Speed")) {
            Some(0) => Some(Speed::Low),
            Some(1) => Some(Speed::Full),
            Some(2) => Some(Speed::High),
            Some(3) => Some(Speed::Super),
            Some(4 | 5) => Some(Speed::SuperPlus),
            _ => None,
        },
        location: port.clone(),
        port,
        configuration: service.number(c"kUSBCurrentConfiguration")
            .or_else(|| service.number(c"bConfigurationValue"))
            .and_then(|value| u8::try_from(value).ok()).filter(|value| *value != 0),
        interfaces,
        serial_ports,
        access,
    })
}

pub fn devices() -> io::Result<Vec<DeviceInfo>> {
    // SAFETY: the class name is NUL-terminated; matching returns an owned dictionary.
    let matching = unsafe { ffi::IOServiceMatching(c"IOUSBHostDevice".as_ptr()) };
    if matching.is_null() { return Err(io::Error::other("IOServiceMatching: allocation failed")) }
    let mut iterator = 0;
    // SAFETY: matching is consumed and iterator is writable; no user client is opened.
    check("IOServiceGetMatchingServices", unsafe { ffi::IOServiceGetMatchingServices(0, matching, &mut iterator) })?;
    let mut devices = Vec::new();
    for service in collect(Service(iterator)) {
        // An unplugged or incomplete registry entry must not hide other devices.
        if let Ok(info) = read_device(&service) { devices.push(info) }
    }
    devices.sort_by_key(|device| device.id.registry);
    Ok(devices)
}

pub fn open(id: DeviceId) -> io::Result<Device> {
    let service = find(id)?.filter(|service| service.conforms(c"IOUSBHostDevice"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("open {id}: the device is gone")))?;
    Ok(Device { claims: BTreeMap::new(), client: None, service, id })
}

struct Pool(*mut c_void);

impl Pool {
    fn new() -> Self {
        // SAFETY: push creates a pool for this thread, popped by this non-Send guard.
        Self(unsafe { ffi::objc_autoreleasePoolPush() })
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        // SAFETY: the pool is popped on the same thread in nested stack order.
        unsafe { ffi::objc_autoreleasePoolPop(self.0); }
    }
}

struct Owned(Object);

impl Drop for Owned {
    fn drop(&mut self) {
        let _pool = Pool::new();
        // SAFETY: this is one owned alloc/init or copy reference; release takes no arguments.
        unsafe { message!(unsafe extern "C" fn(Object, Sel), self.0, c"release"); }
    }
}

struct Host(Owned);

impl Host {
    fn new(service: &Service, class: &CStr) -> Result<Self, IOReturn> {
        let _pool = Pool::new();
        let mut error = null_mut();
        // SAFETY: the class and selectors have SDK signatures; init consumes alloc and retains the service.
        let object = unsafe {
            let class = ffi::objc_getClass(class.as_ptr());
            if class.is_null() { return Err(ffi::ERROR) }
            let allocated = message!(unsafe extern "C" fn(Object, Sel) -> Object, class, c"alloc");
            message!(unsafe extern "C" fn(Object, Sel, ffi::Io, usize, Object, *mut Object, Object) -> Object,
                allocated, c"initWithIOService:options:queue:error:interestHandler:",
                service.0, ffi::INIT_NONE, null_mut(), &mut error, null_mut())
        };
        if object.is_null() { Err(error_code(error)) } else { Ok(Self(Owned(object))) }
    }

    fn object(&self) -> Object { self.0.0 }
}

impl Drop for Host {
    fn drop(&mut self) {
        let _pool = Pool::new();
        // SAFETY: destroy closes this owned user client; no request remains outstanding here.
        unsafe { message!(unsafe extern "C" fn(Object, Sel), self.object(), c"destroy"); }
    }
}

fn error_code(error: Object) -> IOReturn {
    if error.is_null() { return ffi::ERROR }
    // SAFETY: callers supply a live NSError within their autorelease pool; code returns NSInteger.
    unsafe { message!(unsafe extern "C" fn(Object, Sel) -> isize, error, c"code") as IOReturn }
}

fn call_error(call: &str, status: IOReturn, endpoint: Option<u8>, gone: bool) -> io::Error {
    let (kind, reason) = match status {
        ffi::TIMEOUT | ffi::TRANSACTION_TIMEOUT => (io::ErrorKind::TimedOut, "transfer timed out".to_owned()),
        ffi::NO_DEVICE | ffi::NOT_RESPONDING => (io::ErrorKind::NotConnected, "the device is gone".to_owned()),
        ffi::ABORTED if gone => (io::ErrorKind::NotConnected, "the device is gone".to_owned()),
        ffi::PIPE_STALLED => (io::ErrorKind::BrokenPipe, match endpoint {
            Some(endpoint) => format!("endpoint 0x{endpoint:02x} stalled; clear_halt resets it"),
            None => "pipe stalled; clear_halt resets it".to_owned(),
        }),
        ffi::NOT_PRIVILEGED => (io::ErrorKind::PermissionDenied,
            "access denied: macOS refused the user client (kIOReturnNotPrivileged)".to_owned()),
        ffi::BUSY | ffi::EXCLUSIVE_ACCESS => (io::ErrorKind::ResourceBusy, "already held by another client".to_owned()),
        _ => (io::ErrorKind::Other, "USB call failed".to_owned()),
    };
    io::Error::new(kind, format!("{call}: {reason} (IOReturn 0x{:08x})", status as u32))
}

fn check(call: &str, status: IOReturn) -> io::Result<()> {
    if status == 0 { Ok(()) } else { Err(call_error(call, status, None, false)) }
}

fn timeout_ms(timeout: Duration) -> u32 {
    if timeout == Duration::MAX { return 0 }
    let millis = timeout.as_millis() + u128::from(timeout.subsec_nanos() % 1_000_000 != 0);
    if millis > u32::MAX as u128 { 0 } else { millis.max(1) as u32 }
}

struct Claim {
    // Drop pipes before destroying the interface they retain.
    pipes: BTreeMap<u8, Owned>,
    host: Host,
    endpoints: Vec<EndpointInfo>,
}

pub struct Device {
    claims: BTreeMap<u8, Claim>,
    client: Option<Host>,
    service: Service,
    id: DeviceId,
}

// SAFETY: ownership moves between workers; requests complete before return, callbacks only touch Arc state, and the public Device is !Sync.
unsafe impl Send for Device {}

// The transfer runs on an NSMutableData of its own: NSMutableData copies even from its no-copy
// initializer, so OUT is copied in and IN is copied back out after the request completes.
enum Buffer<'a> { In(&'a mut [u8]), Out(&'a [u8]) }

impl Buffer<'_> {
    fn len(&self) -> usize {
        match self { Self::In(data) => data.len(), Self::Out(data) => data.len() }
    }

    fn check_direction(&self, address: u8, call: &str) -> io::Result<()> {
        if (address & 0x80 != 0) == matches!(self, Self::In(_)) { return Ok(()) }
        Err(io::Error::new(io::ErrorKind::InvalidInput, format!("{call}: direction bit does not match the call")))
    }

    fn data(&self) -> io::Result<Owned> {
        let _pool = Pool::new();
        // SAFETY: IN gets zeroed storage of its length; OUT is copied by init; signatures match NSData.h.
        let object = unsafe {
            let class = ffi::objc_getClass(c"NSMutableData".as_ptr());
            let allocated = message!(unsafe extern "C" fn(Object, Sel) -> Object, class, c"alloc");
            match self {
                Self::In(data) => message!(unsafe extern "C" fn(Object, Sel, usize) -> Object,
                    allocated, c"initWithLength:", data.len()),
                Self::Out(data) => message!(unsafe extern "C" fn(Object, Sel, *const c_void, usize) -> Object,
                    allocated, c"initWithBytes:length:", data.as_ptr().cast(), data.len()),
            }
        };
        if object.is_null() { return Err(io::Error::other("NSMutableData init: allocation failed")) }
        Ok(Owned(object))
    }

    /// Copy an IN transfer's bytes from its NSMutableData into the caller's buffer.
    fn finish(&mut self, data: &Owned, transferred: usize) -> usize {
        let Self::In(buffer) = self else { return transferred };
        let _pool = Pool::new();
        // SAFETY: data is a live NSMutableData of buffer.len() bytes; bytes returns its storage pointer.
        let bytes = unsafe { message!(unsafe extern "C" fn(Object, Sel) -> *const u8, data.0, c"bytes") };
        let count = transferred.min(buffer.len());
        if count > 0 && !bytes.is_null() {
            // SAFETY: both regions hold at least count bytes and are distinct allocations.
            unsafe { std::ptr::copy_nonoverlapping(bytes, buffer.as_mut_ptr(), count); }
        }
        count
    }
}

fn configuration(host: &Host) -> io::Result<Vec<InterfaceInfo>> {
    let _pool = Pool::new();
    // SAFETY: host retains the SDK's descriptor cache; both accessors return pointers, not structures by value.
    let (device, config) = unsafe {
        (message!(unsafe extern "C" fn(Object, Sel) -> *const u8, host.object(), c"deviceDescriptor"),
         message!(unsafe extern "C" fn(Object, Sel) -> *const u8, host.object(), c"configurationDescriptor"))
    };
    if device.is_null() { return Err(io::Error::other("deviceDescriptor: no descriptor available")) }
    if config.is_null() { return Ok(Vec::new()) }
    // SAFETY: the SDK owns a full 18-byte device descriptor and a configuration allocation of wTotalLength bytes while host is live.
    let bytes = unsafe {
        let device = std::slice::from_raw_parts(device, 18);
        let header = std::slice::from_raw_parts(config, 9);
        let length = u16::from_le_bytes([header[2], header[3]]) as usize;
        if length < 9 { return Err(io::Error::new(io::ErrorKind::InvalidData, "configurationDescriptor: wTotalLength is shorter than nine")) }
        let mut bytes = device.to_vec();
        bytes.extend_from_slice(std::slice::from_raw_parts(config, length));
        bytes
    };
    let parsed = descriptor::parse_device(&bytes).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    Ok(parsed.configurations.into_iter().next().map(|config| config.interfaces).unwrap_or_default())
}

fn endpoints(host: &Host, number: u8) -> io::Result<Vec<EndpointInfo>> {
    let _pool = Pool::new();
    // SAFETY: this host is an IOUSBHostInterface and retains its full interface descriptor.
    let pointer = unsafe { message!(unsafe extern "C" fn(Object, Sel) -> *const u8, host.object(), c"interfaceDescriptor") };
    if pointer.is_null() { return Err(io::Error::other("interfaceDescriptor: no descriptor available")) }
    // SAFETY: bAlternateSetting is byte three of the SDK-owned nine-byte interface descriptor.
    let alternate = unsafe { *pointer.add(3) };
    configuration(host)?.into_iter().find(|interface| interface.number == number && interface.alternate == alternate)
        .map(|interface| interface.endpoints)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "claimed interface has no configuration descriptors"))
}

impl Device {
    fn error(&self, call: &str, status: IOReturn, endpoint: Option<u8>) -> io::Error {
        let gone = status == ffi::ABORTED && matches!(find(self.id), Ok(None));
        call_error(call, status, endpoint, gone)
    }

    fn host(&mut self) -> io::Result<&Host> {
        if let Some((&number, _)) = self.claims.first_key_value() {
            return Ok(&self.claims[&number].host);
        }
        if self.client.is_none() {
            if let Some(driver) = device_driver(&self.service)? {
                return Err(io::Error::new(io::ErrorKind::ResourceBusy, format!("open device user client: device is bound to the {driver} driver")));
            }
            self.client = Some(Host::new(&self.service, c"IOUSBHostDevice").map_err(|status| {
                if matches!(status, ffi::BUSY | ffi::EXCLUSIVE_ACCESS) {
                    if let Ok(Some(driver)) = device_driver(&self.service) {
                        return io::Error::new(io::ErrorKind::ResourceBusy,
                            format!("initWithIOService IOUSBHostDevice: device is bound to the {driver} driver (IOReturn 0x{:08x})", status as u32));
                    }
                }
                self.error("initWithIOService IOUSBHostDevice", status, None)
            })?);
        }
        Ok(self.client.as_ref().expect("device client was initialized"))
    }

    pub fn configuration(&mut self) -> io::Result<Vec<InterfaceInfo>> {
        let mut interfaces = configuration(self.host()?)?;
        for service in interface_services(&self.service)? {
            let info = interface_info(&service)?;
            for interface in interfaces.iter_mut().filter(|interface| interface.number == info.number) {
                interface.driver = info.driver.clone();
                if interface.alternate == info.alternate { interface.name = info.name.clone() }
            }
        }
        Ok(interfaces)
    }

    pub fn claim(&mut self, number: u8) -> io::Result<()> {
        if self.claims.contains_key(&number) { return Ok(()) }
        let service = interface_services(&self.service)?.into_iter()
            .find(|service| service.number(c"bInterfaceNumber") == Some(number as u64))
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("claim {number}: interface is not present")))?;
        let host = Host::new(&service, c"IOUSBHostInterface").map_err(|status| {
            if matches!(status, ffi::BUSY | ffi::EXCLUSIVE_ACCESS) {
                let reason = match interface_driver(&service, true) {
                    Ok(Some(driver)) => busy(number, &driver),
                    _ => match device_driver(&self.service) {
                        Ok(Some(driver)) => format!("device is bound to the {driver} driver"),
                        _ => format!("interface {number} is already claimed"),
                    },
                };
                io::Error::new(io::ErrorKind::ResourceBusy, format!("claim {number}: {reason} (IOReturn 0x{:08x})", status as u32))
            } else {
                self.error(&format!("claim {number}: initWithIOService IOUSBHostInterface"), status, None)
            }
        })?;
        let endpoints = endpoints(&host, number)?;
        self.claims.insert(number, Claim { pipes: BTreeMap::new(), host, endpoints });
        Ok(())
    }

    pub fn release(&mut self, number: u8) -> io::Result<()> {
        self.claims.remove(&number).ok_or_else(|| io::Error::new(io::ErrorKind::NotFound,
            format!("release {number}: interface is not claimed")))?;
        Ok(())
    }

    pub fn release_all(&mut self) {
        self.claims.clear();
        self.client = None;
    }

    pub fn detach_driver(&mut self, _interface: u8) -> io::Result<()> {
        Err(io::Error::new(io::ErrorKind::Unsupported,
            "detaching a driver on macOS needs the com.apple.vm.device-access entitlement, which this app does not have"))
    }

    pub fn set_alternate(&mut self, number: u8, alternate: u8) -> io::Result<()> {
        let _pool = Pool::new();
        let claim = self.claims.get_mut(&number).ok_or_else(|| io::Error::new(io::ErrorKind::NotFound,
            format!("set_alternate {number}: interface is not claimed")))?;
        claim.pipes.clear();
        claim.endpoints.clear();
        let mut error = null_mut();
        // SAFETY: the interface is owned, no I/O is pending, and the selector takes NSUInteger and NSError**.
        let success = unsafe { message!(unsafe extern "C" fn(Object, Sel, usize, *mut Object) -> Bool,
            claim.host.object(), c"selectAlternateSetting:error:", alternate as usize, &mut error) };
        // Refresh even on failure: the old pipes may have been invalidated before it failed.
        let refreshed = endpoints(&claim.host, number);
        if let Ok(endpoints) = &refreshed { claim.endpoints = endpoints.clone() }
        if success as u8 == 0 { return Err(self.error("selectAlternateSetting", error_code(error), None)) }
        refreshed.map(|_| ())
    }

    fn pipe(&mut self, address: u8, transfer: Option<Transfer>) -> io::Result<Object> {
        let _pool = Pool::new();
        let number = self.claims.iter().find_map(|(number, claim)| {
            claim.endpoints.iter().any(|endpoint| endpoint.address == address).then_some(*number)
        }).ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("endpoint 0x{address:02x} has no claimed interface")))?;
        let claim = self.claims.get_mut(&number).expect("claimed interface was found");
        if let Some(transfer) = transfer {
            if !claim.endpoints.iter().any(|endpoint| endpoint.address == address && endpoint.transfer == transfer) {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("endpoint 0x{address:02x} is not {transfer:?}")));
            }
        }
        if let Some(pipe) = claim.pipes.get(&address) { return Ok(pipe.0) }
        let mut error = null_mut();
        // SAFETY: the live interface owns this endpoint; copy returns an owned pipe or nil.
        let pipe = unsafe { message!(unsafe extern "C" fn(Object, Sel, usize, *mut Object) -> Object,
            claim.host.object(), c"copyPipeWithAddress:error:", address as usize, &mut error) };
        if pipe.is_null() { return Err(self.error("copyPipeWithAddress", error_code(error), Some(address))) }
        claim.pipes.insert(address, Owned(pipe));
        Ok(pipe)
    }

    fn control(&mut self, call: &str, setup: Setup, mut buffer: Buffer<'_>, timeout: Duration) -> io::Result<usize> {
        buffer.check_direction(setup.request_type, call)?;
        let length = u16::try_from(buffer.len()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput,
            format!("{call}: buffer length does not fit USB's 16-bit wLength")))?;
        let request = ffi::DeviceRequest { request_type: setup.request_type, request: setup.request,
            value: setup.value.to_le(), index: setup.index.to_le(), length: length.to_le() };
        let _pool = Pool::new();
        let data = buffer.data()?;
        let host = self.host()?.object();
        let mut transferred = 0;
        let mut error = null_mut();
        // SAFETY: the packed request is passed by value; NSData holds the borrow through this synchronous request.
        let success = unsafe { message!(unsafe extern "C" fn(Object, Sel, ffi::DeviceRequest, Object, *mut usize, f64, *mut Object) -> Bool,
            host, c"sendDeviceRequest:data:bytesTransferred:completionTimeout:error:",
            request, data.0, &mut transferred, timeout_ms(timeout) as f64 / 1000.0, &mut error) };
        if success as u8 == 0 { return Err(self.error(call, error_code(error), Some(setup.request_type & 0x80))) }
        Ok(buffer.finish(&data, transferred))
    }

    fn transfer(&mut self, call: &str, address: u8, mut buffer: Buffer<'_>, timeout: Duration, transfer: Transfer) -> io::Result<usize> {
        buffer.check_direction(address, call)?;
        let pipe = self.pipe(address, Some(transfer))?;
        let _pool = Pool::new();
        let data = buffer.data()?;
        if transfer == Transfer::Interrupt {
            let transferred = interrupt(pipe, &data, timeout).map_err(|status| self.error(call, status, Some(address)))?;
            return Ok(buffer.finish(&data, transferred));
        }
        let mut transferred = 0;
        let mut error = null_mut();
        // SAFETY: pipe is retained in the claim and NSData holds the borrow until the synchronous request completes.
        let success = unsafe { message!(unsafe extern "C" fn(Object, Sel, Object, *mut usize, f64, *mut Object) -> Bool,
            pipe, c"sendIORequestWithData:bytesTransferred:completionTimeout:error:",
            data.0, &mut transferred, timeout_ms(timeout) as f64 / 1000.0, &mut error) };
        if success as u8 == 0 { return Err(self.error(call, error_code(error), Some(address))) }
        Ok(buffer.finish(&data, transferred))
    }

    pub fn control_in(&mut self, setup: Setup, data: &mut [u8], timeout: Duration) -> io::Result<usize> {
        self.control("control_in", setup, Buffer::In(data), timeout)
    }

    pub fn control_out(&mut self, setup: Setup, data: &[u8], timeout: Duration) -> io::Result<usize> {
        self.control("control_out", setup, Buffer::Out(data), timeout)
    }

    pub fn bulk_in(&mut self, endpoint: u8, data: &mut [u8], timeout: Duration) -> io::Result<usize> {
        self.transfer("bulk_in", endpoint, Buffer::In(data), timeout, Transfer::Bulk)
    }

    pub fn bulk_out(&mut self, endpoint: u8, data: &[u8], timeout: Duration) -> io::Result<usize> {
        self.transfer("bulk_out", endpoint, Buffer::Out(data), timeout, Transfer::Bulk)
    }

    pub fn interrupt_in(&mut self, endpoint: u8, data: &mut [u8], timeout: Duration) -> io::Result<usize> {
        self.transfer("interrupt_in", endpoint, Buffer::In(data), timeout, Transfer::Interrupt)
    }

    pub fn interrupt_out(&mut self, endpoint: u8, data: &[u8], timeout: Duration) -> io::Result<usize> {
        self.transfer("interrupt_out", endpoint, Buffer::Out(data), timeout, Transfer::Interrupt)
    }

    pub fn clear_halt(&mut self, endpoint: u8) -> io::Result<()> {
        let pipe = self.pipe(endpoint, None)?;
        let _pool = Pool::new();
        let mut error = null_mut();
        // SAFETY: the cached pipe is live and has no outstanding request; NSError** points to writable storage.
        let success = unsafe { message!(unsafe extern "C" fn(Object, Sel, *mut Object) -> Bool,
            pipe, c"clearStallWithError:", &mut error) };
        if success as u8 == 0 { Err(self.error("clearStallWithError", error_code(error), Some(endpoint))) } else { Ok(()) }
    }
}

struct Completion {
    done: Mutex<Option<(IOReturn, usize)>>,
    wake: Condvar,
}

#[repr(C)]
struct Literal {
    isa: *const c_void,
    flags: i32,
    reserved: i32,
    invoke: extern "C" fn(*mut Literal, IOReturn, usize),
    descriptor: *const ffi::BlockDescriptor,
    completion: *const Completion,
}

static BLOCK_DESCRIPTOR: ffi::BlockDescriptor = ffi::BlockDescriptor { reserved: 0, size: std::mem::size_of::<Literal>() };

const _: () = {
    assert!(std::mem::offset_of!(Literal, flags) == std::mem::size_of::<*const c_void>());
    assert!(std::mem::offset_of!(Literal, invoke) == std::mem::size_of::<*const c_void>() + 8);
    assert!(std::mem::offset_of!(Literal, completion) == 3 * std::mem::size_of::<*const c_void>() + 8);
    assert!(std::mem::size_of::<Literal>() == 4 * std::mem::size_of::<*const c_void>() + 8);
};

extern "C" fn completed(block: *mut Literal, status: IOReturn, transferred: usize) {
    // SAFETY: enqueue copied this Block ABI literal; exactly one completion consumes its one raw Arc reference.
    let completion = unsafe { Arc::from_raw((*block).completion) };
    let mut done = completion.done.lock().unwrap_or_else(|error| error.into_inner());
    *done = Some((status, transferred));
    completion.wake.notify_one();
}

fn interrupt(pipe: Object, data: &Owned, timeout: Duration) -> Result<usize, IOReturn> {
    let completion = Arc::new(Completion { done: Mutex::new(None), wake: Condvar::new() });
    let captured = Arc::into_raw(Arc::clone(&completion));
    let mut block = Literal {
        // SAFETY: libSystem's concrete stack-block class is an external static with process lifetime.
        isa: unsafe { ffi::_NSConcreteStackBlock.as_ptr().cast() },
        flags: 0,
        reserved: 0,
        invoke: completed,
        descriptor: &BLOCK_DESCRIPTOR,
        completion: captured,
    };
    let mut error = null_mut();
    // SAFETY: the API copies the stack block; its Arc survives until completion, and data stays borrowed until completion is observed.
    let success = unsafe { message!(unsafe extern "C" fn(Object, Sel, Object, f64, *mut Object, *mut Literal) -> Bool,
        pipe, c"enqueueIORequestWithData:completionTimeout:error:completionHandler:", data.0, 0.0, &mut error, &mut block) };
    if success as u8 == 0 {
        // SAFETY: enqueue failure schedules no completion, so the caller reclaims the unconsumed raw Arc.
        unsafe { drop(Arc::from_raw(captured)); }
        return Err(error_code(error));
    }
    let millis = timeout_ms(timeout);
    let done = completion.done.lock().unwrap_or_else(|error| error.into_inner());
    let (mut done, timed_out) = if millis == 0 {
        (completion.wake.wait_while(done, |done| done.is_none()).unwrap_or_else(|error| error.into_inner()), false)
    } else {
        let (done, _) = completion.wake.wait_timeout_while(done, Duration::from_millis(millis as u64), |done| done.is_none())
            .unwrap_or_else(|error| error.into_inner());
        let timed_out = done.is_none();
        (done, timed_out)
    };
    if timed_out {
        // Do not hold the completion lock while synchronous abort waits for its handler.
        drop(done);
        let mut abort_error = null_mut();
        // SAFETY: pipe is retained and only this request is pending; synchronous abort completes its handler before buffer release.
        unsafe { message!(unsafe extern "C" fn(Object, Sel, usize, *mut Object) -> Bool,
            pipe, c"abortWithOption:error:", ffi::ABORT_SYNCHRONOUS, &mut abort_error); }
        done = completion.done.lock().unwrap_or_else(|error| error.into_inner());
        // Even an abort failure cannot allow the borrowed buffer to escape while I/O is pending.
        done = completion.wake.wait_while(done, |done| done.is_none()).unwrap_or_else(|error| error.into_inner());
    }
    let (status, transferred) = done.expect("completion was observed");
    if timed_out { Err(ffi::TIMEOUT) } else if status == 0 { Ok(transferred) } else { Err(status) }
}
