use super::super::libc_sys::ioctl;
use std::{
    fs::{File, OpenOptions},
    io,
    os::{
        fd::AsRawFd,
        raw::{c_int, c_ulong},
    },
};

// Linux UAPI: linux/kd.h, linux/vt.h and asm-generic/ioctls.h.
const KDSETMODE: c_ulong = 0x4b3a;
const KDGETMODE: c_ulong = 0x4b3b;
const KDGKBMODE: c_ulong = 0x4b44;
const KDSKBMODE: c_ulong = 0x4b45;
const KD_TEXT: c_int = 0;
const KD_GRAPHICS: c_int = 1;
const K_OFF: c_int = 4;
const VT_GETSTATE: c_ulong = 0x5603;
const TIOCGDEV: c_ulong = 0x80045432;

#[repr(C)]
#[derive(Default)]
struct VtState {
    active: u16,
    signal: u16,
    state: u16,
}

/// Keeps fbcon and the terminal keyboard queue out of the direct display.
/// Only changes our own active text console; never opens or switches another VT.
pub(super) struct DirectTerminal {
    file: File,
    keyboard_mode: c_int,
    display_mode: c_int,
}

impl DirectTerminal {
    pub(super) fn enter() -> Result<Option<Self>, String> {
        let file = match OpenOptions::new().read(true).write(true).open("/dev/tty") {
            Ok(file) => file,
            Err(_) => return Ok(None),
        };
        let fd = file.as_raw_fd();
        let mut display_mode = 0;
        if unsafe { ioctl(fd, KDGETMODE, &mut display_mode) } < 0 {
            // SSH and terminal-emulator ptys do not have a Linux console mode.
            return Ok(None);
        }
        if display_mode != KD_TEXT {
            return Err("Direct display requires a free text VT; the current VT is already in graphics mode".into());
        }
        let mut state = VtState::default();
        let mut device: u32 = 0;
        if unsafe { ioctl(fd, VT_GETSTATE, &mut state) } < 0
            || unsafe { ioctl(fd, TIOCGDEV, &mut device) } < 0
        {
            return Err(format!(
                "Query active Linux console: {}",
                io::Error::last_os_error()
            ));
        }
        // Linux virtual consoles use major 4 and minors 1..63.
        if device != (4 << 8) | u32::from(state.active) {
            return Err("Launch direct display from the active text VT".into());
        }
        let mut keyboard_mode = 0;
        if unsafe { ioctl(fd, KDGKBMODE, &mut keyboard_mode) } < 0 {
            return Err(format!(
                "Query console keyboard mode: {}",
                io::Error::last_os_error()
            ));
        }
        let guard = Self {
            file,
            keyboard_mode,
            display_mode,
        };
        if unsafe { ioctl(fd, KDSETMODE, KD_GRAPHICS) } < 0 {
            return Err(format!(
                "Enter console graphics mode: {}",
                io::Error::last_os_error()
            ));
        }
        // evdev supplies all app input. Do not leave typed app keys queued for
        // the shell, or let console shortcuts change VT underneath the renderer.
        if unsafe { ioctl(fd, KDSKBMODE, K_OFF) } < 0 {
            return Err(format!(
                "Disable console keyboard translation: {}",
                io::Error::last_os_error()
            ));
        }
        Ok(Some(guard))
    }
}

impl Drop for DirectTerminal {
    fn drop(&mut self) {
        let fd = self.file.as_raw_fd();
        if unsafe { ioctl(fd, KDSKBMODE, self.keyboard_mode) } < 0 {
            crate::warning!("Restore console keyboard: {}", io::Error::last_os_error());
        }
        if unsafe { ioctl(fd, KDSETMODE, self.display_mode) } < 0 {
            crate::warning!("Restore console display: {}", io::Error::last_os_error());
        }
    }
}
