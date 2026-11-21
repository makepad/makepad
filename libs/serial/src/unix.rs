use super::*;
use crate::ffi;
use std::fs::{File, OpenOptions};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::time::Instant;

#[cfg(target_vendor = "apple")]
use ffi::termios as Termios;
#[cfg(any(target_os = "linux", target_os = "android"))]
use ffi::termios2 as Termios;

/// An owned Unix serial port, or one end of a pseudo-terminal pair.
pub struct TTYPort {
    file: File,
    timeout: Duration,
    exclusive: bool,
}

fn check(rc: ffi::c_int) -> io::Result<()> {
    if rc < 0 {
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

#[cfg(target_vendor = "apple")]
fn make_raw(termios: &mut Termios) {
    unsafe { ffi::cfmakeraw(termios) };
}

/// Raw mode, as glibc's cfmakeraw sets it.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn make_raw(termios: &mut Termios) {
    termios.c_iflag &= !(ffi::IGNBRK
        | ffi::BRKINT
        | ffi::PARMRK
        | ffi::ISTRIP
        | ffi::INLCR
        | ffi::IGNCR
        | ffi::ICRNL
        | ffi::IXON);
    termios.c_oflag &= !ffi::OPOST;
    termios.c_lflag &= !(ffi::ECHO | ffi::ECHONL | ffi::ICANON | ffi::ISIG | ffi::IEXTEN);
    termios.c_cflag &= !(ffi::CSIZE | ffi::PARENB);
    termios.c_cflag |= ffi::CS8;
}

impl TTYPort {
    pub fn open(path: &str, baud: u32, timeout: Duration) -> io::Result<Self> {
        check_baud(baud)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(ffi::O_NOCTTY | ffi::O_NONBLOCK | ffi::O_CLOEXEC)
            .open(path)?;
        let mut port = Self {
            file,
            timeout,
            exclusive: false,
        };
        let fd = port.file.as_raw_fd();
        unsafe {
            check(ffi::ioctl(fd, ffi::TIOCEXCL as _))?;
        }
        port.exclusive = true;
        port.configure(Some(baud))?;
        Ok(port)
    }

    fn attributes(&self) -> io::Result<Termios> {
        let mut term: Termios = unsafe { std::mem::zeroed() };
        #[cfg(target_vendor = "apple")]
        unsafe {
            check(ffi::tcgetattr(self.file.as_raw_fd(), &mut term))?;
        }
        #[cfg(any(target_os = "linux", target_os = "android"))]
        unsafe {
            check(ffi::ioctl(self.file.as_raw_fd(), ffi::TCGETS2, &mut term))?;
        }
        Ok(term)
    }

    /// Apple uses cfsetspeed before tcsetattr for the rates in termios.h;
    /// all other rates use IOSSIOSPEED after tcsetattr. Standard rates must
    /// avoid IOSSIOSPEED because pseudo-terminals do not implement it.
    /// Opening then flushes both queues with tcflush(TCIOFLUSH).
    fn apply(&self, term: &mut Termios, baud: Option<u32>) -> io::Result<()> {
        let fd = self.file.as_raw_fd();
        #[cfg(target_vendor = "apple")]
        unsafe {
            let standard = baud.is_some_and(|baud| {
                matches!(
                    baud,
                    50 | 75 | 110 | 134 | 150 | 200 | 300 | 600 | 1200 | 1800 | 2400
                        | 4800 | 7200 | 9600 | 14400 | 19200 | 28800 | 38400 | 57600
                        | 76800 | 115200 | 230400
                )
            });
            if let Some(baud) = baud.filter(|_| standard) {
                check(ffi::cfsetspeed(term, baud as ffi::speed_t))?;
            }
            check(ffi::tcsetattr(fd, ffi::TCSANOW, term))?;
            if let Some(baud) = baud.filter(|_| !standard) {
                let speed = baud as ffi::speed_t;
                check(ffi::ioctl(fd, ffi::IOSSIOSPEED, &speed))?;
            }
        }
        #[cfg(any(target_os = "linux", target_os = "android"))]
        unsafe {
            if let Some(baud) = baud {
                term.c_cflag = (term.c_cflag & !(ffi::CBAUD | ffi::CIBAUD)) | ffi::BOTHER;
                term.c_ispeed = baud;
                term.c_ospeed = baud;
            }
            check(ffi::ioctl(fd, ffi::TCSETS2, term as *const Termios))?;
        }
        Ok(())
    }

    fn configure(&self, baud: Option<u32>) -> io::Result<()> {
        let fd = self.file.as_raw_fd();
        let mut term = self.attributes()?;
        make_raw(&mut term);
        term.c_cflag &= !(ffi::CSIZE | ffi::PARENB | ffi::PARODD | ffi::CSTOPB | ffi::CRTSCTS);
        term.c_cflag |= ffi::CS8 | ffi::CREAD | ffi::CLOCAL;
        term.c_iflag &= !(ffi::IXON | ffi::IXOFF | ffi::IXANY | ffi::INPCK);
        term.c_iflag |= ffi::IGNPAR;
        term.c_cc[ffi::VMIN] = 1;
        term.c_cc[ffi::VTIME] = 0;
        self.apply(&mut term, baud)?;
        unsafe {
            #[cfg(target_vendor = "apple")]
            if baud.is_some() {
                check(ffi::tcflush(fd, ffi::TCIOFLUSH))?;
            }
            let flags = ffi::fcntl(fd, ffi::F_GETFL);
            check(flags)?;
            check(ffi::fcntl(fd, ffi::F_SETFL, flags & !ffi::O_NONBLOCK))?;
        }
        Ok(())
    }

    fn wait(&self, events: ffi::c_short) -> io::Result<()> {
        let start = Instant::now();
        loop {
            let millis = if self.timeout == Duration::MAX {
                -1
            } else {
                let remaining = self.timeout.saturating_sub(start.elapsed());
                remaining
                    .as_millis()
                    .saturating_add(u128::from(remaining.subsec_nanos() % 1_000_000 != 0))
                    .min(i32::MAX as u128) as i32
            };
            let mut pfd = ffi::pollfd {
                fd: self.file.as_raw_fd(),
                events,
                revents: 0,
            };
            let rc = unsafe { ffi::poll(&mut pfd, 1, millis) };
            if rc < 0 {
                let err = io::Error::last_os_error();
                if err.kind() != io::ErrorKind::Interrupted {
                    return Err(err);
                }
            } else if rc > 0 {
                if pfd.revents & ffi::POLLNVAL != 0 {
                    return Err(io::Error::from_raw_os_error(ffi::EBADF));
                }
                if pfd.revents & events != 0 {
                    return Ok(());
                }
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "serial port disconnected",
                ));
            }
            if self.timeout != Duration::MAX && start.elapsed() >= self.timeout {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "serial I/O timed out",
                ));
            }
        }
    }

    fn set_pin(&self, pin: ffi::c_int, level: bool) -> io::Result<()> {
        unsafe {
            check(ffi::ioctl(
                self.file.as_raw_fd(),
                if level {
                    ffi::TIOCMBIS
                } else {
                    ffi::TIOCMBIC
                },
                &pin,
            ))
        }
    }

    /// Create a connected raw pseudo-terminal pair for loopback I/O and tests.
    /// Neither end sets a baud rate; both start with a 100 ms timeout.
    pub fn pair() -> io::Result<(Self, Self)> {
        let (mut master, mut slave) = (-1, -1);
        unsafe {
            check(ffi::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            ))?;
        }
        let wrap = |fd| Self {
            file: unsafe { File::from_raw_fd(fd) },
            timeout: Duration::from_millis(100),
            exclusive: false,
        };
        let (master, slave) = (wrap(master), wrap(slave));
        master.configure(None)?;
        slave.configure(None)?;
        Ok((master, slave))
    }

    /// Duplicate the handle to keep a port alive or use it from another worker.
    /// The handles share port settings and queues, but have separate I/O timeouts.
    pub fn try_clone(&self) -> io::Result<Box<dyn SerialPort>> {
        Ok(Box::new(Self {
            file: self.file.try_clone()?,
            timeout: self.timeout,
            exclusive: false,
        }))
    }
}

impl Drop for TTYPort {
    fn drop(&mut self) {
        if self.exclusive {
            unsafe {
                ffi::ioctl(self.file.as_raw_fd(), ffi::TIOCNXCL as _);
            }
        }
    }
}
impl Read for TTYPort {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        self.wait(ffi::POLLIN)?;
        self.file.read(buf)
    }
}
impl Write for TTYPort {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        self.wait(ffi::POLLOUT)?;
        self.file.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        unsafe { check(ffi::tcdrain(self.file.as_raw_fd())) }
    }
}
impl SerialPort for TTYPort {
    fn set_baud(&mut self, baud: u32) -> io::Result<()> {
        check_baud(baud)?;
        let mut term = self.attributes()?;
        self.apply(&mut term, Some(baud))
    }
    fn set_timeout(&mut self, timeout: Duration) -> io::Result<()> {
        self.timeout = timeout;
        Ok(())
    }
    fn bytes_to_read(&self) -> io::Result<u32> {
        let mut count: ffi::c_int = 0;
        unsafe {
            check(ffi::ioctl(
                self.file.as_raw_fd(),
                ffi::FIONREAD,
                &mut count,
            ))?;
        }
        Ok(count.max(0) as u32)
    }
    fn clear_input(&self) -> io::Result<()> {
        unsafe { check(ffi::tcflush(self.file.as_raw_fd(), ffi::TCIFLUSH)) }
    }
    fn write_data_terminal_ready(&mut self, level: bool) -> io::Result<()> {
        self.set_pin(ffi::TIOCM_DTR, level)
    }
    fn write_request_to_send(&mut self, level: bool) -> io::Result<()> {
        self.set_pin(ffi::TIOCM_RTS, level)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pty_binary_io_timeout_and_input_clear() {
        let (mut master, mut slave) = TTYPort::pair().unwrap();
        slave.set_timeout(Duration::from_millis(20)).unwrap();
        let mut buf = [0; 5];
        let start = Instant::now();
        assert_eq!(
            slave.read(&mut buf).unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
        assert!(start.elapsed() >= Duration::from_millis(15));
        assert!(start.elapsed() < Duration::from_secs(1));
        let payload = [0, b'\r', b'\n', 0x11, 0xff];
        master.write_all(&payload).unwrap();
        slave.read_exact(&mut buf).unwrap();
        assert_eq!(buf, payload);
        slave.write_all(&payload).unwrap();
        master.read_exact(&mut buf).unwrap();
        assert_eq!(buf, payload);
        master.write_all(&payload).unwrap();
        slave.wait(ffi::POLLIN).unwrap();
        assert_eq!(slave.bytes_to_read().unwrap(), 5);
        slave.clear_input().unwrap();
        assert_eq!(slave.bytes_to_read().unwrap(), 0);
        assert_eq!(
            slave.read(&mut buf).unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
        assert_eq!(slave.read(&mut []).unwrap(), 0);
        assert_eq!(slave.write(&[]).unwrap(), 0);
    }

    #[test]
    fn open_configures_standard_baud_and_releases_exclusivity() {
        let (_master, slave) = TTYPort::pair().unwrap();
        let mut name = [0 as ffi::c_char; 1024];
        assert_eq!(
            unsafe { ffi::ttyname_r(slave.file.as_raw_fd(), name.as_mut_ptr(), name.len()) },
            0
        );
        let path = unsafe { std::ffi::CStr::from_ptr(name.as_ptr()) }
            .to_str()
            .unwrap();
        let port = TTYPort::open(path, 19_200, Duration::from_millis(50)).unwrap();
        #[cfg(target_vendor = "apple")]
        let mut term: ffi::termios = unsafe { std::mem::zeroed() };
        #[cfg(target_vendor = "apple")]
        check(unsafe { ffi::tcgetattr(port.file.as_raw_fd(), &mut term) }).unwrap();
        #[cfg(any(target_os = "linux", target_os = "android"))]
        let mut term: ffi::termios2 = unsafe { std::mem::zeroed() };
        #[cfg(any(target_os = "linux", target_os = "android"))]
        check(unsafe { ffi::ioctl(port.file.as_raw_fd(), ffi::TCGETS2, &mut term) }).unwrap();
        assert_eq!(term.c_ispeed, 19_200);
        assert_eq!(term.c_ospeed, 19_200);
        assert_eq!(term.c_cflag & ffi::CSIZE, ffi::CS8);
        assert_eq!(
            term.c_cflag & (ffi::PARENB | ffi::CSTOPB | ffi::CRTSCTS),
            0
        );
        assert_eq!(term.c_iflag & (ffi::IXON | ffi::IXOFF), 0);
        assert_eq!(term.c_lflag & (ffi::ICANON | ffi::ECHO | ffi::ISIG), 0);
        assert_eq!(
            unsafe { ffi::fcntl(port.file.as_raw_fd(), ffi::F_GETFL) } & ffi::O_NONBLOCK,
            0
        );
        // Darwin pseudo-terminals allow another open despite TIOCEXCL.
        // Linux enforces it for unprivileged callers.
        #[cfg(target_os = "linux")]
        if unsafe { ffi::geteuid() } != 0 {
            assert_eq!(
                TTYPort::open(path, 19_200, Duration::ZERO)
                    .err()
                    .unwrap()
                    .raw_os_error(),
                Some(ffi::EBUSY)
            );
        }
        #[cfg(any(target_os = "linux", target_os = "android"))]
        let mut port = port;
        #[cfg(any(target_os = "linux", target_os = "android"))]
        for baud in [921_600, 117_647] {
            port.set_baud(baud).unwrap();
            let mut changed: ffi::termios2 = unsafe { std::mem::zeroed() };
            check(unsafe { ffi::ioctl(port.file.as_raw_fd(), ffi::TCGETS2, &mut changed) }).unwrap();
            assert_eq!(changed.c_ispeed, baud, "input speed");
            assert_eq!(changed.c_ospeed, baud, "output speed");
            assert_eq!(changed.c_iflag, term.c_iflag, "input flags changed");
            assert_eq!(changed.c_oflag, term.c_oflag, "output flags changed");
            assert_eq!(changed.c_cflag, term.c_cflag, "control flags changed");
            assert_eq!(changed.c_lflag, term.c_lflag, "local flags changed");
            assert_eq!(changed.c_line, term.c_line, "line discipline changed");
            assert_eq!(changed.c_cc, term.c_cc, "control characters changed");
        }
        drop(port);
        TTYPort::open(path, 19_200, Duration::ZERO).unwrap();
    }

    #[test]
    fn pty_clone_keeps_slave_alive() {
        let (mut master, slave) = TTYPort::pair().unwrap();
        let mut clone = slave.try_clone().unwrap();
        drop(slave);
        clone.write_all(b"alive").unwrap();
        let mut bytes = [0; 5];
        master.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"alive");
    }
}
