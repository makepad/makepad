//! Serial ports: always 8N1, raw, with no hardware or software flow control,
//! at any positive baud rate (the number itself: Linux and Android set it
//! through the kernel's termios2 with BOTHER, never through libc's `speed_t`).
//! Timeouts apply per I/O call; a read that times out returns
//! [`io::ErrorKind::TimedOut`]. [`Duration::MAX`] waits without a deadline (on
//! Windows, for the longest finite timeout the OS takes, about 49 days).
use std::io::{self, Read, Write};
use std::time::Duration;

/// A byte stream with serial speed, timeout and modem-line controls.
pub trait SerialPort: Read + Write + Send {
    /// Change only the speed, preserving every other port setting.
    /// A zero baud rate is invalid.
    fn set_baud(&mut self, baud: u32) -> io::Result<()>;
    fn set_timeout(&mut self, timeout: Duration) -> io::Result<()>;
    fn bytes_to_read(&self) -> io::Result<u32>;
    fn clear_input(&self) -> io::Result<()>;
    fn write_data_terminal_ready(&mut self, level: bool) -> io::Result<()>;
    fn write_request_to_send(&mut self, level: bool) -> io::Result<()>;
}

/// The items for the unix targets whose ABI `ffi` declares: Apple, and Linux
/// and Android on the asm-generic termios2 architectures (mips, powerpc,
/// sparc and alpha have their own layout and are not declared). The second
/// group holds the items for every target that is neither these nor Windows.
macro_rules! tty {
    ({ $($item:item)* } else { $($other:item)* }) => {
        $(#[cfg(any(
            target_vendor = "apple",
            all(
                any(target_os = "linux", target_os = "android"),
                any(
                    target_arch = "x86",
                    target_arch = "x86_64",
                    target_arch = "arm",
                    target_arch = "aarch64",
                    target_arch = "riscv32",
                    target_arch = "riscv64",
                    target_arch = "loongarch64"
                )
            )
        ))]
        $item)*
        $(#[cfg(not(any(
            windows,
            target_vendor = "apple",
            all(
                any(target_os = "linux", target_os = "android"),
                any(
                    target_arch = "x86",
                    target_arch = "x86_64",
                    target_arch = "arm",
                    target_arch = "aarch64",
                    target_arch = "riscv32",
                    target_arch = "riscv64",
                    target_arch = "loongarch64"
                )
            )
        )))]
        $other)*
    };
}

tty! {
    {
        mod ffi;
        mod unix;
        pub use unix::TTYPort;
        fn open_port(path: &str, baud: u32, timeout: Duration) -> io::Result<Box<dyn SerialPort>> {
            Ok(Box::new(TTYPort::open(path, baud, timeout)?))
        }
    } else {
        fn open_port(_path: &str, _baud: u32, _timeout: Duration) -> io::Result<Box<dyn SerialPort>> {
            Err(io::Error::new(io::ErrorKind::Unsupported, "serial ports are not available on this target"))
        }
    }
}

pub mod devices;
pub mod line;
pub mod remote;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
fn open_port(path: &str, baud: u32, timeout: Duration) -> io::Result<Box<dyn SerialPort>> {
    Ok(Box::new(windows::Port::open(path, baud, timeout)?))
}

/// Open an exclusive serial port at a positive numeric baud rate.
/// Targets without a declared ABI return [`io::ErrorKind::Unsupported`].
pub fn open(path: &str, baud: u32, timeout: Duration) -> io::Result<Box<dyn SerialPort>> {
    open_port(path, baud, timeout)
}
