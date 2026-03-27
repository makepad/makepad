use super::libc_sys;
use crate::thread::{SignalToUI, WakeHookHandle};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

pub(crate) struct LinuxWakePipe {
    read_fd: OwnedFd,
    write_fd: OwnedFd,
    wake_hook: Option<WakeHookHandle>,
}

impl LinuxWakePipe {
    pub(crate) fn new() -> io::Result<Self> {
        let mut pipe_fds = [0; 2];
        if unsafe { libc_sys::pipe(pipe_fds.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }

        let read_fd = unsafe { OwnedFd::from_raw_fd(pipe_fds[0]) };
        let write_fd = unsafe { OwnedFd::from_raw_fd(pipe_fds[1]) };
        Self::set_nonblocking(read_fd.as_raw_fd())?;
        Self::set_nonblocking(write_fd.as_raw_fd())?;

        Ok(Self {
            read_fd,
            write_fd,
            wake_hook: None,
        })
    }

    pub(crate) fn install_signal_waker(&mut self) {
        self.uninstall_signal_waker();
        let write_fd = self.write_fd.as_raw_fd();
        self.wake_hook = Some(SignalToUI::set_wake_hook(move || {
            let byte = [1u8];
            let _ = unsafe {
                libc_sys::write(write_fd, byte.as_ptr() as *const std::os::raw::c_void, 1)
            };
        }));
    }

    pub(crate) fn uninstall_signal_waker(&mut self) {
        if let Some(wake_hook) = self.wake_hook.take() {
            SignalToUI::clear_wake_hook(wake_hook);
        }
    }

    pub(crate) fn read_fd(&self) -> i32 {
        self.read_fd.as_raw_fd()
    }

    pub(crate) fn drain(&self) {
        let mut buf = [0u8; 64];
        loop {
            let count = unsafe {
                libc_sys::read(
                    self.read_fd.as_raw_fd(),
                    buf.as_mut_ptr() as *mut std::os::raw::c_void,
                    buf.len(),
                )
            };
            if count <= 0 || count < buf.len() as i32 {
                break;
            }
        }
    }

    fn set_nonblocking(fd: i32) -> io::Result<()> {
        let flags = unsafe { libc_sys::fcntl(fd, libc_sys::F_GETFL, 0) };
        if flags < 0 {
            return Err(io::Error::last_os_error());
        }
        if unsafe { libc_sys::fcntl(fd, libc_sys::F_SETFL, flags | libc_sys::O_NONBLOCK) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

impl Drop for LinuxWakePipe {
    fn drop(&mut self) {
        self.uninstall_signal_waker();
    }
}
