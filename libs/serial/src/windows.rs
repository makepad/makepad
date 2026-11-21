use super::*;
use std::ffi::c_void;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

type Handle = *mut c_void;
#[repr(C)]
#[derive(Default)]
struct Dcb {
    length: u32,
    baud: u32,
    flags: u32,
    reserved: u16,
    xon_lim: u16,
    xoff_lim: u16,
    byte_size: u8,
    parity: u8,
    stop_bits: u8,
    xon_char: u8,
    xoff_char: u8,
    error_char: u8,
    eof_char: u8,
    evt_char: u8,
    reserved1: u16,
}
#[repr(C)]
struct CommTimeouts {
    read_interval: u32,
    read_multiplier: u32,
    read_constant: u32,
    write_multiplier: u32,
    write_constant: u32,
}
#[repr(C)]
#[derive(Default)]
struct CommStat {
    flags: u32,
    in_queue: u32,
    out_queue: u32,
}

#[link(name = "kernel32")]
extern "system" {
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: *const c_void,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    fn GetCommState(handle: Handle, dcb: *mut Dcb) -> i32;
    fn SetCommState(handle: Handle, dcb: *const Dcb) -> i32;
    fn SetCommTimeouts(handle: Handle, timeouts: *const CommTimeouts) -> i32;
    fn ReadFile(
        handle: Handle,
        buffer: *mut u8,
        count: u32,
        read: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
    fn WriteFile(
        handle: Handle,
        buffer: *const u8,
        count: u32,
        written: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
    fn FlushFileBuffers(handle: Handle) -> i32;
    fn PurgeComm(handle: Handle, flags: u32) -> i32;
    fn ClearCommError(handle: Handle, errors: *mut u32, status: *mut CommStat) -> i32;
    fn EscapeCommFunction(handle: Handle, function: u32) -> i32;
}
fn check(ok: i32) -> io::Result<()> {
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn check_baud(baud: u32) -> io::Result<()> {
    if baud == 0 {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "baud rate must be positive",
        ))
    } else {
        Ok(())
    }
}

pub struct Port {
    handle: OwnedHandle,
}
impl Port {
    pub fn open(path: &str, baud: u32, timeout: Duration) -> io::Result<Self> {
        check_baud(baud)?;
        if path.contains('\0') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "NUL in port path",
            ));
        }
        let path = if path.starts_with(r"\\.\") {
            path.to_owned()
        } else {
            format!(r"\\.\{path}")
        };
        let name: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
        // Synchronous, exclusive access; OwnedHandle closes even on setup failure.
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                0xc0000000,
                0,
                std::ptr::null(),
                3,
                0,
                std::ptr::null_mut(),
            )
        };
        if handle as isize == -1 {
            return Err(io::Error::last_os_error());
        }
        let mut port = Self {
            handle: unsafe { OwnedHandle::from_raw_handle(handle) },
        };
        let mut dcb = Dcb {
            length: std::mem::size_of::<Dcb>() as u32,
            ..Dcb::default()
        };
        unsafe {
            check(GetCommState(port.raw(), &mut dcb))?;
        }
        dcb.baud = baud;
        // fBinary only: disable parity, CTS/DSR, XON/XOFF, DTR/RTS and abort-on-error.
        dcb.flags = 1;
        dcb.byte_size = 8;
        dcb.parity = 0;
        dcb.stop_bits = 0;
        dcb.xon_char = 17;
        dcb.xoff_char = 19;
        dcb.error_char = 0;
        dcb.eof_char = 26;
        unsafe {
            check(SetCommState(port.raw(), &dcb))?;
        }
        port.set_timeout(timeout)?;
        Ok(port)
    }
    fn raw(&self) -> Handle {
        self.handle.as_raw_handle()
    }
}
impl Read for Port {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let mut count = 0;
        unsafe {
            check(ReadFile(
                self.raw(),
                buf.as_mut_ptr(),
                buf.len().min(u32::MAX as usize) as u32,
                &mut count,
                std::ptr::null_mut(),
            ))?;
        }
        if count == 0 {
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "serial read timed out",
            ))
        } else {
            Ok(count as usize)
        }
    }
}
impl Write for Port {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let mut count = 0;
        unsafe {
            check(WriteFile(
                self.raw(),
                buf.as_ptr(),
                buf.len().min(u32::MAX as usize) as u32,
                &mut count,
                std::ptr::null_mut(),
            ))?;
        }
        Ok(count as usize)
    }
    fn flush(&mut self) -> io::Result<()> {
        unsafe { check(FlushFileBuffers(self.raw())) }
    }
}
impl SerialPort for Port {
    fn set_baud(&mut self, baud: u32) -> io::Result<()> {
        check_baud(baud)?;
        let mut dcb = Dcb {
            length: std::mem::size_of::<Dcb>() as u32,
            ..Dcb::default()
        };
        unsafe {
            check(GetCommState(self.raw(), &mut dcb))?;
            dcb.baud = baud;
            check(SetCommState(self.raw(), &dcb))
        }
    }
    fn set_timeout(&mut self, timeout: Duration) -> io::Result<()> {
        let millis = timeout.as_millis().min((u32::MAX - 1) as u128) as u32;
        let timeouts = CommTimeouts {
            read_interval: u32::MAX,
            read_multiplier: u32::MAX,
            read_constant: millis,
            write_multiplier: 0,
            write_constant: millis,
        };
        unsafe { check(SetCommTimeouts(self.raw(), &timeouts)) }
    }
    fn bytes_to_read(&self) -> io::Result<u32> {
        let mut errors = 0;
        let mut stat = CommStat::default();
        unsafe {
            check(ClearCommError(self.raw(), &mut errors, &mut stat))?;
        }
        Ok(stat.in_queue)
    }
    fn clear_input(&self) -> io::Result<()> {
        unsafe { check(PurgeComm(self.raw(), 0x0008)) }
    }
    fn write_data_terminal_ready(&mut self, level: bool) -> io::Result<()> {
        unsafe { check(EscapeCommFunction(self.raw(), if level { 5 } else { 6 })) }
    }
    fn write_request_to_send(&mut self, level: bool) -> io::Result<()> {
        unsafe { check(EscapeCommFunction(self.raw(), if level { 3 } else { 4 })) }
    }
}

// Assert the hand-declared Win32 ABI even in builds that do not run tests.
const _: () = {
    assert!(std::mem::size_of::<Dcb>() == 28);
    assert!(std::mem::offset_of!(Dcb, byte_size) == 18);
    assert!(std::mem::offset_of!(Dcb, reserved1) == 26);
    assert!(std::mem::size_of::<CommTimeouts>() == 20);
    assert!(std::mem::size_of::<CommStat>() == 12);
};
