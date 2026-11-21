use super::*;

fn unsupported<T>() -> io::Result<T> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "raw USB is not available on this target yet"))
}

pub fn devices() -> io::Result<Vec<DeviceInfo>> {
    unsupported()
}

pub fn open(_id: DeviceId) -> io::Result<Device> {
    unsupported()
}

pub struct Device;

impl Device {
    pub fn configuration(&mut self) -> io::Result<Vec<InterfaceInfo>> { unsupported() }
    pub fn claim(&mut self, _interface: u8) -> io::Result<()> { unsupported() }
    pub fn release(&mut self, _interface: u8) -> io::Result<()> { unsupported() }
    pub fn set_alternate(&mut self, _interface: u8, _alternate: u8) -> io::Result<()> { unsupported() }
    pub fn detach_driver(&mut self, _interface: u8) -> io::Result<()> { unsupported() }
    pub fn control_in(&mut self, _setup: Setup, _data: &mut [u8], _timeout: Duration) -> io::Result<usize> { unsupported() }
    pub fn control_out(&mut self, _setup: Setup, _data: &[u8], _timeout: Duration) -> io::Result<usize> { unsupported() }
    pub fn bulk_in(&mut self, _endpoint: u8, _data: &mut [u8], _timeout: Duration) -> io::Result<usize> { unsupported() }
    pub fn bulk_out(&mut self, _endpoint: u8, _data: &[u8], _timeout: Duration) -> io::Result<usize> { unsupported() }
    pub fn interrupt_in(&mut self, _endpoint: u8, _data: &mut [u8], _timeout: Duration) -> io::Result<usize> { unsupported() }
    pub fn interrupt_out(&mut self, _endpoint: u8, _data: &[u8], _timeout: Duration) -> io::Result<usize> { unsupported() }
    pub fn clear_halt(&mut self, _endpoint: u8) -> io::Result<()> { unsupported() }
    pub fn release_all(&mut self) {}
}
