//! Local IPC only. The game sends GPU allocation handles, never encoded video or
//! pixel arrays. The bridge alone owns headset pairing and network credentials.
use makepad_micro_serde::*;

pub const VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Default, SerBin, DeBin)]
pub struct Pose { pub orientation: [f32; 4], pub position: [f32; 3] }
#[derive(Clone, Copy, Debug, Default, SerBin, DeBin)]
pub struct Eye { pub pose: Pose, pub fov: [f32; 4] }
#[derive(Clone, Copy, Debug, Default, SerBin, DeBin)]
pub struct Controller {
    pub grip_pose: Pose,
    pub aim_pose: Pose,
    pub trigger: f32,
    pub grip: f32,
    pub buttons: u32,
    pub stick: [f32; 2],
}
#[derive(Clone, Debug, SerBin, DeBin)]
pub struct Headset {
    pub eye: [u32; 2],
    pub refresh_mhz: u32,
    pub render_device: u64,
    /// Importable DRM fourcc/modifier pairs, as advertised by the real renderer.
    pub formats: Vec<(u32, u64)>,
}
#[derive(Clone, Debug, SerBin, DeBin)]
pub struct Tracking {
    /// Identifies the headset pose used for this frame, preserved through rendering.
    pub token: u64,
    pub display_time: i64,
    pub period: i64,
    pub headset_time: u64,
    pub head: Pose,
    pub eyes: [Eye; 2],
    pub controllers: [Controller; 2],
}
#[derive(Clone, Debug, SerBin, DeBin)]
pub struct Image {
    pub id: u64,
    pub width: u32,
    pub height: u32,
    pub fourcc: u32,
    pub modifier: u64,
    pub offset: u32,
    pub stride: u32,
}
#[derive(Clone, Debug, SerBin, DeBin)]
pub struct SubmittedEye {
    pub image: u64,
    /// The submitted eye pose in headset tracking coordinates, not app-local space.
    pub eye: Eye,
}
#[derive(Clone, Debug, SerBin, DeBin)]
pub enum Message {
    Hello { version: u32, app: String },
    Headset(Option<Headset>),
    WaitFrame,
    Tracking(Tracking),
    /// Exactly one DMA-BUF FD accompanies this message; import once, reuse by id.
    Import(Image),
    Imported { id: u64 },
    Forget { id: u64 },
    /// The producer's Vulkan copy and release have completed before this arrives.
    Frame { token: u64, eyes: [SubmittedEye; 2], alpha: bool },
    /// The bridge's GPU has finished reading the frame. The producer may reuse it.
    Released { token: u64 },
    Empty { token: u64 },
    Haptic { hand: u32, amplitude: f32, duration_ns: i64 },
    Stop,
    Error(String),
}

pub fn socket_path() -> std::path::PathBuf {
    if let Some(path) = std::env::var_os("MAKEPAD_XR_SOCKET") { return path.into(); }
    std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            #[cfg(target_os = "linux")]
            { extern "C" { fn geteuid() -> u32; } std::path::PathBuf::from(format!("/run/user/{}", unsafe { geteuid() })) }
            #[cfg(not(target_os = "linux"))]
            { std::env::temp_dir() }
        }).join("makepad-xr.sock")
}

#[cfg(target_os = "linux")]
pub mod ipc {
    use super::Message;
    use makepad_micro_serde::{DeBin, SerBin};
    use std::ffi::c_void;
    use std::io::{self, Read, Write};
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::net::UnixStream;

    #[repr(C)]
    struct Iovec { base: *mut c_void, len: usize }
    #[repr(C)]
    struct Header {
        name: *mut c_void, name_len: u32, iov: *mut Iovec, iov_len: usize,
        control: *mut c_void, control_len: usize, flags: i32,
    }
    #[repr(C)]
    struct Control { len: usize, level: i32, kind: i32 }
    extern "C" {
        fn sendmsg(fd: i32, msg: *const Header, flags: i32) -> isize;
        fn recvmsg(fd: i32, msg: *mut Header, flags: i32) -> isize;
        fn getsockopt(fd: i32, level: i32, name: i32, value: *mut c_void, size: *mut u32) -> i32;
        fn geteuid() -> u32;
    }
    fn invalid(text: &str) -> io::Error { io::Error::new(io::ErrorKind::InvalidData, text) }

    /// One transaction at a time per direction. Blocking I/O belongs to the bridge's
    /// long-lived worker or the game's OpenXR call, never Makepad's UI thread.
    pub struct Channel { stream: UnixStream }
    impl Channel {
        pub fn new(stream: UnixStream) -> io::Result<Self> {
            let mut cred = [0u32; 3];
            let mut size = std::mem::size_of_val(&cred) as u32;
            if unsafe { getsockopt(stream.as_raw_fd(), 1, 17, cred.as_mut_ptr().cast(), &mut size) } != 0 {
                return Err(io::Error::last_os_error());
            }
            if cred[1] != unsafe { geteuid() } { return Err(io::Error::new(io::ErrorKind::PermissionDenied, "XR peer belongs to another user")); }
            Ok(Self { stream })
        }
        pub fn connect() -> io::Result<Self> { Self::new(UnixStream::connect(super::socket_path())?) }
        pub fn cancel_handle(&self) -> io::Result<UnixStream> { self.stream.try_clone() }
        pub fn send(&mut self, message: &Message, fd: Option<&OwnedFd>) -> io::Result<()> {
            let bytes = message.serialize_bin();
            let mut marker = u8::from(fd.is_some());
            let mut iov = Iovec { base: (&mut marker as *mut u8).cast(), len: 1 };
            let mut control = [0usize; 3];
            let mut header = Header { name: std::ptr::null_mut(), name_len: 0, iov: &mut iov, iov_len: 1,
                control: std::ptr::null_mut(), control_len: 0, flags: 0 };
            if let Some(fd) = fd {
                let base = control.as_mut_ptr().cast::<u8>();
                unsafe {
                    base.cast::<Control>().write(Control { len: std::mem::size_of::<Control>() + 4, level: 1, kind: 1 });
                    base.add(std::mem::size_of::<Control>()).cast::<i32>().write_unaligned(fd.as_raw_fd());
                }
                header.control = base.cast();
                header.control_len = std::mem::size_of_val(&control);
            }
            loop {
                let n = unsafe { sendmsg(self.stream.as_raw_fd(), &header, 0x4000) }; // MSG_NOSIGNAL
                if n == 1 { break; }
                let e = io::Error::last_os_error();
                if n < 0 && e.kind() == io::ErrorKind::Interrupted { continue; }
                return Err(if n < 0 { e } else { invalid("short XR descriptor marker") });
            }
            self.stream.write_all(&(bytes.len() as u64).to_le_bytes())?;
            self.stream.write_all(&bytes)
        }
        pub fn recv(&mut self) -> io::Result<(Message, Option<OwnedFd>)> {
            let mut marker = 0u8;
            let mut iov = Iovec { base: (&mut marker as *mut u8).cast(), len: 1 };
            let mut control = [0usize; 3];
            let mut header = Header { name: std::ptr::null_mut(), name_len: 0, iov: &mut iov, iov_len: 1,
                control: control.as_mut_ptr().cast(), control_len: std::mem::size_of_val(&control), flags: 0 };
            let n = loop {
                header.control_len = std::mem::size_of_val(&control);
                let n = unsafe { recvmsg(self.stream.as_raw_fd(), &mut header, 0x40000000) }; // MSG_CMSG_CLOEXEC
                if n >= 0 { break n; }
                let e = io::Error::last_os_error();
                if e.kind() != io::ErrorKind::Interrupted { return Err(e); }
            };
            let mut fds = Vec::new();
            if header.control_len >= std::mem::size_of::<Control>() {
                let base = control.as_ptr().cast::<u8>();
                let c = unsafe { &*base.cast::<Control>() };
                if c.level == 1 && c.kind == 1 && c.len >= std::mem::size_of::<Control>() && c.len <= header.control_len {
                    for i in 0..(c.len - std::mem::size_of::<Control>()) / 4 {
                        let raw = unsafe { base.add(std::mem::size_of::<Control>() + i * 4).cast::<i32>().read_unaligned() };
                        fds.push(unsafe { OwnedFd::from_raw_fd(raw) });
                    }
                }
            }
            if n == 0 { return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "XR peer disconnected")); }
            if header.flags & 8 != 0 || marker > 1 || fds.len() != marker as usize { return Err(invalid("invalid XR descriptor count")); }
            let mut length = [0u8; 8];
            self.stream.read_exact(&mut length)?;
            let length = usize::try_from(u64::from_le_bytes(length)).map_err(|_| invalid("XR message length overflow"))?;
            let mut bytes = Vec::new();
            bytes.try_reserve_exact(length).map_err(|_| io::Error::new(io::ErrorKind::OutOfMemory, "XR message allocation failed"))?;
            bytes.resize(length, 0);
            self.stream.read_exact(&mut bytes)?;
            let mut offset = 0;
            let message = Message::de_bin(&mut offset, &bytes).map_err(|_| invalid("invalid XR message"))?;
            if offset != bytes.len() { return Err(invalid("trailing XR message bytes")); }
            if matches!(message, Message::Import(_)) != !fds.is_empty() { return Err(invalid("unexpected XR image descriptor")); }
            Ok((message, fds.pop()))
        }
    }
}
