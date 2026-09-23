//! A Makepad app hosted as a real child PROCESS of a window manager on
//! Android — the phone's version of what `--stdin-loop` is on the desktop.
//!
//! The WM (an ordinary Android app) starts each app as a process of its own:
//! a launcher ELF in the APK's native-library directory `dlopen`s the app's
//! library and calls `makepad_hosted_main` (the `app_main!` export). The child
//! has no JVM, no Activity and no window. It talks the desktop hosting
//! protocol with the WM over the WM's localhost hub (`StudioToApp` /
//! `AppToStudio`, `STUDIO_HOST`/`STUDIO_BUILD` in its environment), and it
//! draws its window pass into the WM's shared frames: `AHardwareBuffer`s the
//! WM allocates for a `HostSwapchain` and hands out over a unix socket
//! (`AHardwareBuffer_sendHandleToUnixSocket`), imported into Vulkan on both
//! sides. What the JVM would have given it comes from elsewhere: assets are
//! read from the APK file, sockets are plain `std::net`, paths come from the
//! environment the WM sets.
use {
    super::ndk_sys::{self, AHardwareBuffer},
    crate::{
        cx::{AndroidParams, Cx, OsType},
        cx_api::CxOsOp,
        draw_pass::{CxDrawPassParent, DrawPassClearColor},
        event::{finger::TouchState, Event, WindowGeom},
        makepad_math::*,
        makepad_micro_serde::*,
        os::linux::vulkan::{CxVulkan, HostedSyncRole},
        os::shared_framebuf::{HostSwapchain, PollTimer, PresentableDraw, PresentableImageId},
        texture::{Texture, TextureFormat},
        thread::SignalToUI,
        web_socket::WebSocketMessage,
        window::CxWindowPool,
    },
    makepad_studio_protocol::{AppToStudio, ChildRelay, RelayPermission, StudioToApp, StudioToAppVec},
    std::{
        collections::HashMap,
        io::{Read, Write},
        os::fd::{AsRawFd, OwnedFd},
        sync::{
            atomic::{AtomicBool, Ordering},
            Mutex, OnceLock,
        },
    },
};

static HOSTED: AtomicBool = AtomicBool::new(false);

/// A redraw asked for DURING a draw (font atlases growing while the first
/// frame lays out its text) is refused: the Activity build redraws once
/// more after the first draw at a new size (`first_after_resize`), and so
/// does a hosted child, or its first frame shows no text until an input.
static REDRAW_AFTER_FIRST: AtomicBool = AtomicBool::new(true);

/// The host's finger is down on this app (a MouseMove without it is the
/// host's hover, which a touch screen does not have).
static TOUCH_DOWN: AtomicBool = AtomicBool::new(false);

/// A worker thread's signal already asked the host for a Tick (cleared by
/// the Tick): the loop sleeps on the host socket, so a signal wakes it by
/// asking the host.
static WAKE_SENT: AtomicBool = AtomicBool::new(false);

/// Hosted child: `wake_ui_event_loop` from any thread.
pub(crate) fn hosted_wake() {
    // The network runtime also wakes the loop for every host message; those
    // are read by the loop itself. Only a raised signal needs a Tick.
    if !SignalToUI::any_pending() {
        return;
    }
    if !WAKE_SENT.swap(true, Ordering::Relaxed) {
        crate::trace!("pace", "child signal wake");
        Cx::send_studio_message(AppToStudio::RequestAnimationFrame);
    }
}

/// This child fences its frames with SYNC_FDs (`HostedSync`) rather than
/// waiting for its GPU before each flip.
static FENCED: AtomicBool = AtomicBool::new(false);

/// Host: its submissions wait on children's frame fds (`HostedSyncRole::Host`).
static HOST_FENCED: AtomicBool = AtomicBool::new(false);

/// True in a hosted child process (no JVM: every JNI path must stay shut).
pub fn is_hosted() -> bool {
    HOSTED.load(Ordering::Relaxed)
}

/// Mark this process as a hosted child (`app_main!`'s `makepad_hosted_main`).
#[doc(hidden)]
pub fn set_hosted() {
    HOSTED.store(true, Ordering::Relaxed);
}

/// The environment a host gives its children: the socket its shared frames
/// are fetched from, the APK the child's assets live in, and the app's
/// storage and cache directories.
pub const AHB_SOCKET_ENV: &str = "MAKEPAD_AHB_SOCKET";
pub const APK_ENV: &str = "MAKEPAD_ANDROID_APK";
pub const DATA_PATH_ENV: &str = "MAKEPAD_ANDROID_DATA_PATH";
pub const CACHE_PATH_ENV: &str = "MAKEPAD_ANDROID_CACHE_PATH";
pub const DENSITY_ENV: &str = "MAKEPAD_ANDROID_DENSITY";

// ---------------------------------------------------------------------------
// Assets: read straight from the APK (a zip), since AAssetManager needs Java.
// ---------------------------------------------------------------------------

struct Apk {
    file: std::fs::File,
    entries: HashMap<String, makepad_zip_file::CentralDirectoryFileHeader>,
}

fn apk() -> Option<&'static Mutex<Apk>> {
    static APK: OnceLock<Option<Mutex<Apk>>> = OnceLock::new();
    APK.get_or_init(|| {
        let path = std::env::var(APK_ENV).ok()?;
        let mut file = std::fs::File::open(&path).ok()?;
        let dir = match makepad_zip_file::zip_read_central_directory(&mut file) {
            Ok(dir) => dir,
            Err(err) => {
                crate::error!("hosted: cannot read {path}: {err:?}");
                return None;
            }
        };
        let entries = dir
            .file_headers
            .into_iter()
            .map(|header| (header.file_name.clone(), header))
            .collect();
        Some(Mutex::new(Apk { file, entries }))
    })
    .as_ref()
}

/// An asset of the APK the child was started from (`assets/<path>`).
pub(crate) fn load_asset(path: &str) -> Option<Vec<u8>> {
    let apk = apk()?;
    let mut apk = apk.lock().ok()?;
    let header = apk.entries.get(&format!("assets/{path}"))?.clone();
    header.extract(&mut apk.file).ok()
}

// ---------------------------------------------------------------------------
// Sockets: plain std::net streams (the Activity build asks Java for them).
// ---------------------------------------------------------------------------

pub(crate) struct HostedSocketFactory;

struct HostedSocketStream(std::net::TcpStream);

impl crate::makepad_network::AndroidSocketStreamFactory for HostedSocketFactory {
    fn connect(
        &self,
        host: &str,
        port: &str,
        use_tls: bool,
        _ignore_ssl_cert: bool,
    ) -> std::io::Result<Box<dyn crate::makepad_network::AndroidSocketStream>> {
        if use_tls {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "a hosted Android child has no TLS stream",
            ));
        }
        let stream = std::net::TcpStream::connect(format!("{host}:{port}"))?;
        let _ = stream.set_nodelay(true);
        Ok(Box::new(HostedSocketStream(stream)))
    }
}

impl crate::makepad_network::AndroidSocketStream for HostedSocketStream {
    fn set_read_timeout(&self, timeout: Option<std::time::Duration>) -> std::io::Result<()> {
        self.0.set_read_timeout(timeout)
    }
    fn set_write_timeout(&self, timeout: Option<std::time::Duration>) -> std::io::Result<()> {
        self.0.set_write_timeout(timeout)
    }
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(buf)
    }
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
    fn shutdown(&mut self) {
        let _ = self.0.shutdown(std::net::Shutdown::Both);
    }
}

// ---------------------------------------------------------------------------
// Shared frames. The HOST side: every presentable image of a HostSwapchain is
// an AHardwareBuffer, published under its PresentableImageId until the child
// that draws into it has fetched it.
// ---------------------------------------------------------------------------

struct SharedBuffer(*mut AHardwareBuffer);
unsafe impl Send for SharedBuffer {}

fn published() -> &'static Mutex<HashMap<Vec<u8>, SharedBuffer>> {
    static MAP: OnceLock<Mutex<HashMap<Vec<u8>, SharedBuffer>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

fn socket_name() -> String {
    format!("makepad-ahb-{}", std::process::id())
}

/// Start the host's frame server once and tell future children where it is
/// (a host calls it before it spawns its first child).
pub fn start_frame_server() {
    static STARTED: OnceLock<()> = OnceLock::new();
    STARTED.get_or_init(|| {
        use std::os::android::net::SocketAddrExt;
        let name = socket_name();
        let addr = match std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes()) {
            Ok(addr) => addr,
            Err(err) => {
                crate::error!("hosted frames: bad socket name {name}: {err}");
                return;
            }
        };
        let listener = match std::os::unix::net::UnixListener::bind_addr(&addr) {
            Ok(listener) => listener,
            Err(err) => {
                crate::error!("hosted frames: cannot listen on @{name}: {err}");
                return;
            }
        };
        std::env::set_var(AHB_SOCKET_ENV, &name);
        let _ = std::thread::Builder::new()
            .name("ahb-frames".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    let Ok(stream) = stream else { continue };
                    let _ = std::thread::Builder::new()
                        .name("ahb-client".into())
                        .spawn(move || serve_frames(stream));
                }
            });
        crate::log!("hosted frames: serving shared frames on @{name}");
    });
}

// ---------------------------------------------------------------------------
// GPU sync between host and children (`HostedSync` in vulkan_android.rs).
// A child hands the host the SYNC_FD of each frame's submission instead of
// waiting for its GPU; the host's next submission waits on it. The host's
// own latest submission fd goes back to a child before it draws into a
// shared image again, so that write waits for the host's reads.
// ---------------------------------------------------------------------------

/// Header bit: an acquire fd rides on this message (low bits: key length).
const OP_ACQUIRE: u32 = 0x8000_0000;
/// Header value: the child asks for the host's latest release fd.
const OP_RELEASE: u32 = 0x4000_0000;

/// Per child connection, the newest frame fd the host has not waited on yet
/// (a child's submissions complete in order: the newest covers the older).
fn acquire_fds() -> &'static Mutex<HashMap<u64, OwnedFd>> {
    static MAP: OnceLock<Mutex<HashMap<u64, OwnedFd>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

fn release_fd() -> &'static Mutex<Option<OwnedFd>> {
    static FD: Mutex<Option<OwnedFd>> = Mutex::new(None);
    &FD
}

/// Host: the children's frame fds its next submission waits on.
pub(crate) fn take_acquire_fds() -> Vec<OwnedFd> {
    acquire_fds().lock().map(|mut map| map.drain().map(|(_, fd)| fd).collect()).unwrap_or_default()
}

/// Host: the fd of its latest submission (it signals once every read of a
/// shared image submitted so far is done).
pub(crate) fn publish_release_fd(fd: OwnedFd) {
    if let Ok(mut slot) = release_fd().lock() {
        *slot = Some(fd);
    }
}

mod fd_msg {
    use std::{
        io::{self, Read, Write},
        os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
    };
    #[repr(C)]
    struct IoVec {
        base: *mut u8,
        len: usize,
    }
    #[repr(C)]
    struct MsgHdr {
        name: *mut u8,
        namelen: u32,
        iov: *mut IoVec,
        iovlen: usize,
        control: *mut u8,
        controllen: usize,
        flags: i32,
    }
    #[repr(C)]
    struct CmsgFd {
        len: usize,
        level: i32,
        kind: i32,
        fd: i32,
        _pad: i32,
    }
    const SOL_SOCKET: i32 = 1;
    const SCM_RIGHTS: i32 = 1;
    const MSG_NOSIGNAL: i32 = 0x4000;
    const MSG_CMSG_CLOEXEC: i32 = 0x4000_0000;
    extern "C" {
        fn sendmsg(fd: RawFd, msg: *const MsgHdr, flags: i32) -> isize;
        fn recvmsg(fd: RawFd, msg: *mut MsgHdr, flags: i32) -> isize;
    }

    /// Write `bytes`, the fd (if any) riding on the first byte.
    pub fn send(stream: &mut std::os::unix::net::UnixStream, bytes: &[u8], fd: Option<RawFd>) -> io::Result<()> {
        let Some(fd) = fd else { return stream.write_all(bytes) };
        let mut first = [bytes[0]];
        let mut iov = IoVec { base: first.as_mut_ptr(), len: 1 };
        let mut cmsg = CmsgFd { len: 16 + 4, level: SOL_SOCKET, kind: SCM_RIGHTS, fd, _pad: 0 };
        let msg = MsgHdr {
            name: std::ptr::null_mut(),
            namelen: 0,
            iov: &mut iov,
            iovlen: 1,
            control: &mut cmsg as *mut CmsgFd as *mut u8,
            controllen: std::mem::size_of::<CmsgFd>(),
            flags: 0,
        };
        loop {
            match unsafe { sendmsg(stream.as_raw_fd(), &msg, MSG_NOSIGNAL) } {
                1 => break,
                -1 if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted => continue,
                -1 => return Err(io::Error::last_os_error()),
                _ => return Err(io::ErrorKind::WriteZero.into()),
            }
        }
        stream.write_all(&bytes[1..])
    }

    /// Read exactly `bytes`; an fd riding on the first byte is returned.
    pub fn recv(stream: &mut std::os::unix::net::UnixStream, bytes: &mut [u8]) -> io::Result<Option<OwnedFd>> {
        let mut first = [0u8];
        let mut iov = IoVec { base: first.as_mut_ptr(), len: 1 };
        let mut control = [0u64; 8];
        let mut msg = MsgHdr {
            name: std::ptr::null_mut(),
            namelen: 0,
            iov: &mut iov,
            iovlen: 1,
            control: control.as_mut_ptr() as *mut u8,
            controllen: std::mem::size_of_val(&control),
            flags: 0,
        };
        loop {
            match unsafe { recvmsg(stream.as_raw_fd(), &mut msg, MSG_CMSG_CLOEXEC) } {
                1 => break,
                0 => return Err(io::ErrorKind::UnexpectedEof.into()),
                -1 if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted => continue,
                -1 => return Err(io::Error::last_os_error()),
                _ => return Err(io::ErrorKind::InvalidData.into()),
            }
        }
        let mut fds = Vec::new();
        let mut offset = 0usize;
        let base = control.as_ptr() as *const u8;
        while offset + 16 <= msg.controllen {
            let len = unsafe { (base.add(offset) as *const usize).read() };
            let level = unsafe { (base.add(offset + 8) as *const i32).read() };
            let kind = unsafe { (base.add(offset + 12) as *const i32).read() };
            if len < 16 || offset + len > msg.controllen {
                break;
            }
            if level == SOL_SOCKET && kind == SCM_RIGHTS {
                for i in 0..(len - 16) / 4 {
                    let raw = unsafe { (base.add(offset + 16 + i * 4) as *const i32).read() };
                    if raw >= 0 {
                        fds.push(unsafe { OwnedFd::from_raw_fd(raw) });
                    }
                }
            }
            offset += (len + 7) & !7;
        }
        bytes[0] = first[0];
        stream.read_exact(&mut bytes[1..])?;
        // One fd per message; extra ones (never sent) close here.
        Ok(fds.into_iter().next())
    }
}

/// One child's connection: it asks for a frame by id (a length-prefixed
/// serialized `PresentableImageId`), the host answers one status byte and,
/// when it has the frame, the buffer itself. The same connection carries the
/// GPU sync of its frames (`OP_ACQUIRE`, `OP_RELEASE`).
fn serve_frames(mut stream: std::os::unix::net::UnixStream) {
    static CONNECTIONS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let connection = CONNECTIONS.fetch_add(1, Ordering::Relaxed);
    loop {
        let mut len = [0u8; 4];
        let fd = match fd_msg::recv(&mut stream, &mut len) {
            Ok(fd) => fd,
            Err(_) => {
                if let Ok(mut map) = acquire_fds().lock() {
                    map.remove(&connection);
                }
                return;
            }
        };
        let header = u32::from_le_bytes(len);
        if header == OP_RELEASE {
            let release = release_fd().lock().ok().and_then(|slot| slot.as_ref().and_then(|fd| fd.try_clone().ok()));
            let sent = match &release {
                Some(fd) => fd_msg::send(&mut stream, &[1], Some(fd.as_raw_fd())),
                None => stream.write_all(&[0]),
            };
            if sent.is_err() {
                return;
            }
            continue;
        }
        if header & OP_ACQUIRE != 0 {
            let mut key = vec![0u8; (header & !OP_ACQUIRE) as usize];
            if stream.read_exact(&mut key).is_err() {
                return;
            }
            // 1 only when the host waits on it: otherwise the child waits
            // for its GPU before the flip, as without fences.
            let accepted = HOST_FENCED.load(Ordering::Relaxed)
                && fd.is_some_and(|fd| acquire_fds().lock().map(|mut map| { map.insert(connection, fd); }).is_ok());
            // The child sends its flip only after this: the fd is in place
            // before the host can draw the frame.
            if stream.write_all(&[accepted as u8]).is_err() {
                return;
            }
            continue;
        }
        let mut key = vec![0u8; header as usize];
        if stream.read_exact(&mut key).is_err() {
            return;
        }
        let buffer = published().lock().ok().and_then(|mut map| map.remove(&key));
        match buffer {
            Some(SharedBuffer(buffer)) => {
                if stream.write_all(&[1]).is_err() {
                    unsafe { ndk_sys::AHardwareBuffer_release(buffer) };
                    return;
                }
                let sent = unsafe {
                    ndk_sys::AHardwareBuffer_sendHandleToUnixSocket(buffer, stream.as_raw_fd())
                };
                unsafe { ndk_sys::AHardwareBuffer_release(buffer) };
                if sent != 0 {
                    crate::error!("hosted frames: sending a frame failed ({sent})");
                    return;
                }
            }
            None => {
                if stream.write_all(&[0]).is_err() {
                    return;
                }
            }
        }
    }
}

impl Cx {
    /// Back every image of `swapchain` with a shared hardware buffer the
    /// child fetches by id (host side of `StudioToApp::Swapchain`).
    /// The host fences its children's frames on the GPU (`HostedSync`):
    /// a child may draw its next frame before the host painted the last.
    pub fn hosted_frames_fenced(&self) -> bool {
        self.os.vulkan.as_ref().is_some_and(|vulkan| vulkan.hosted_sync_role() == HostedSyncRole::Host)
    }

    pub fn android_share_host_swapchain(&mut self, swapchain: &HostSwapchain) {
        start_frame_server();
        let Some(mut vulkan) = self.os.vulkan.take() else {
            crate::error!("hosted frames: the host has no Vulkan renderer");
            return;
        };
        if vulkan.hosted_sync_role() != HostedSyncRole::Host && !sync_disabled() {
            let fenced = vulkan.set_hosted_sync_role(HostedSyncRole::Host);
            HOST_FENCED.store(fenced, Ordering::Relaxed);
            crate::log!("hosted frames: GPU sync with children {}", if fenced { "by SYNC_FD" } else { "unavailable" });
        }
        static SHARED: OnceLock<Mutex<std::collections::HashSet<Vec<u8>>>> = OnceLock::new();
        let shared = SHARED.get_or_init(|| Mutex::new(Default::default()));
        for image in &swapchain.presentable_images {
            let key = image.id.serialize_bin();
            // A swapchain is re-announced on every bootstrap; share it once.
            if !shared.lock().map(|mut set| set.insert(key.clone())).unwrap_or(false) {
                continue;
            }
            let desc = ndk_sys::AHardwareBuffer_Desc {
                width: swapchain.alloc_width.max(1),
                height: swapchain.alloc_height.max(1),
                layers: 1,
                format: ndk_sys::AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM,
                usage: ndk_sys::AHARDWAREBUFFER_USAGE_GPU_SAMPLED_IMAGE
                    | ndk_sys::AHARDWAREBUFFER_USAGE_GPU_COLOR_OUTPUT,
                ..Default::default()
            };
            let mut buffer = std::ptr::null_mut();
            if unsafe { ndk_sys::AHardwareBuffer_allocate(&desc, &mut buffer) } != 0 || buffer.is_null() {
                crate::error!("hosted frames: AHardwareBuffer_allocate {}x{} failed", desc.width, desc.height);
                continue;
            }
            if let Err(err) = vulkan.bind_shared_hardware_buffer(
                image.texture.texture_id(),
                buffer,
                desc.width,
                desc.height,
                false,
            ) {
                crate::error!("hosted frames: host import failed: {err}");
                unsafe { ndk_sys::AHardwareBuffer_release(buffer) };
                continue;
            }
            // The map keeps the allocation's reference until the child
            // fetched it; the texture resource holds its own.
            if let Ok(mut map) = published().lock() {
                if let Some(SharedBuffer(old)) = map.insert(key, SharedBuffer(buffer)) {
                    unsafe { ndk_sys::AHardwareBuffer_release(old) };
                }
            }
        }
        self.os.vulkan = Some(vulkan);
    }
}

// ---------------------------------------------------------------------------
// The CHILD side: fetch a frame from the host by id.
// ---------------------------------------------------------------------------

fn host_connection() -> Option<std::sync::MutexGuard<'static, Option<std::os::unix::net::UnixStream>>> {
    static CONNECTION: Mutex<Option<std::os::unix::net::UnixStream>> = Mutex::new(None);
    let mut connection = CONNECTION.lock().ok()?;
    if connection.is_none() {
        use std::os::android::net::SocketAddrExt;
        // The launcher is exec'd by the host itself: without the variable,
        // the host's server is the one named after the parent process.
        let name = std::env::var(AHB_SOCKET_ENV)
            .unwrap_or_else(|_| format!("makepad-ahb-{}", std::os::unix::process::parent_id()));
        let addr = std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes()).ok()?;
        match std::os::unix::net::UnixStream::connect_addr(&addr) {
            Ok(stream) => *connection = Some(stream),
            Err(err) => {
                crate::error!("hosted: cannot reach the host's frames at @{name}: {err}");
                return None;
            }
        }
    }
    Some(connection)
}

/// Child: the host's latest release fd (none before its first submission).
fn fetch_release_fd() -> Option<OwnedFd> {
    let mut connection = host_connection()?;
    let stream = connection.as_mut()?;
    let mut status = [0u8; 1];
    let result = stream
        .write_all(&OP_RELEASE.to_le_bytes())
        .and_then(|_| fd_msg::recv(stream, &mut status));
    match result {
        Ok(fd) if status[0] == 1 => fd,
        Ok(_) => None,
        Err(_) => {
            *connection = None;
            None
        }
    }
}

/// Child: hand the host this frame's fd; returns once the host holds it.
fn send_acquire_fd(id: &PresentableImageId, fd: &OwnedFd) -> bool {
    let Some(mut connection) = host_connection() else { return false };
    let Some(stream) = connection.as_mut() else { return false };
    let key = id.serialize_bin();
    let mut request = (OP_ACQUIRE | key.len() as u32).to_le_bytes().to_vec();
    request.extend_from_slice(&key);
    let mut ack = [0u8; 1];
    let ok = fd_msg::send(stream, &request, Some(fd.as_raw_fd()))
        .and_then(|_| stream.read_exact(&mut ack))
        .is_ok();
    if !ok {
        *connection = None;
    }
    ok && ack[0] == 1
}

fn fetch_shared_buffer(id: &PresentableImageId) -> Option<*mut AHardwareBuffer> {
    let mut connection = host_connection()?;
    let stream = connection.as_mut()?;
    let key = id.serialize_bin();
    let mut request = (key.len() as u32).to_le_bytes().to_vec();
    request.extend_from_slice(&key);
    let mut status = [0u8; 1];
    if stream.write_all(&request).is_err() || stream.read_exact(&mut status).is_err() {
        *connection = None;
        return None;
    }
    if status[0] != 1 {
        return None;
    }
    let mut buffer = std::ptr::null_mut();
    let received =
        unsafe { ndk_sys::AHardwareBuffer_recvHandleFromUnixSocket(stream.as_raw_fd(), &mut buffer) };
    if received != 0 || buffer.is_null() {
        crate::error!("hosted: receiving a frame failed ({received})");
        *connection = None;
        return None;
    }
    Some(buffer)
}

struct HostedImage {
    id: PresentableImageId,
    texture: Option<Texture>,
}

struct HostedSwapchain {
    alloc_width: u32,
    alloc_height: u32,
    images: Vec<HostedImage>,
}

#[derive(Default)]
pub(crate) struct HostedWindow {
    swapchain: Option<HostedSwapchain>,
    present_index: usize,
}

impl Cx {
    fn hosted_send(msg: AppToStudio) {
        Cx::send_studio_message(msg);
    }

    fn hosted_file_dialog(kind: &str, dialog: &crate::file_dialogs::FileDialog) {
        Self::hosted_send(AppToStudio::Relay(ChildRelay::FileDialog(
            crate::hosted_relay::relay_file_dialog(kind, dialog),
        )));
    }

    /// The hosted child's whole life: bring up a headless renderer, announce
    /// startup to the host, then answer its messages until the socket closes.
    pub fn android_hosted_event_loop(&mut self) {
        let trace_topics = system_property("debug.makepad.trace");
        if !trace_topics.is_empty() {
            crate::makepad_error_log::set_trace_topics(&trace_topics);
        }
        let env = |key: &str| std::env::var(key).unwrap_or_default();
        let density = env(DENSITY_ENV).parse::<f64>().unwrap_or(2.625);
        self.os.dpi_factor = density;
        self.os_type = OsType::Android(AndroidParams {
            cache_path: env(CACHE_PATH_ENV),
            data_path: env(DATA_PATH_ENV),
            density,
            ..Default::default()
        });
        match CxVulkan::new_headless(1, 1) {
            Ok(mut vulkan) => {
                FENCED.store(!sync_disabled() && vulkan.set_hosted_sync_role(HostedSyncRole::Child), Ordering::Relaxed);
                self.os.vulkan = Some(vulkan);
            }
            Err(err) => {
                crate::error!("hosted: no Vulkan renderer: {err}");
                return;
            }
        }
        self.os.surface_alive = true;
        self.in_makepad_studio = true;
        // This loop blocks on the network runtime and reads the host socket
        // itself: a host message must not also raise the UI signals (they
        // would ask the host for a Tick after each of its own Ticks).
        self.net.set_quiet_socket(Some(crate::makepad_live_id::LiveId(0)));
        // As the Activity build's main loop does.
        self.gpu_info.performance = crate::gpu_info::GpuPerformance::Tier1;

        Self::hosted_send(AppToStudio::BeforeStartup);
        let mut windows: Vec<HostedWindow> = Vec::new();
        self.call_event_handler(&Event::Startup);
        self.redraw_all();
        Self::hosted_send(AppToStudio::AfterStartup);
        Self::hosted_send(AppToStudio::Custom(
            crate::ime::HostedPointerCaps::current().to_json(),
        ));
        if FENCED.load(Ordering::Relaxed) {
            Self::hosted_send(AppToStudio::Custom(crate::ime::HostedFenced { on: true }.to_json()));
        }

        loop {
            if !Self::has_studio_web_socket() {
                crate::error!("hosted: no host socket");
                break;
            }
            let Some(incoming) = self.recv_studio_websocket_message() else { break };
            match incoming {
                WebSocketMessage::Binary(data) => match StudioToAppVec::deserialize_bin(&data) {
                    Ok(msgs) => {
                        let mut batch = msgs.0;
                        let closed = self.stdin_drain_host_batches(&mut batch);
                        Self::stdin_coalesce_host_ticks(&mut batch);
                        let ticked = batch.iter().any(|msg| matches!(msg, StudioToApp::Tick));
                        for msg in batch {
                            if self.hosted_handle_msg(msg, &mut windows) {
                                return;
                            }
                        }
                        self.handle_actions();
                        if !ticked {
                            self.hosted_request_frame_if_dirty();
                        }
                        if closed {
                            break;
                        }
                    }
                    Err(err) => crate::error!("hosted: bad host batch: {err:?}"),
                },
                WebSocketMessage::String(text) => {
                    if let Ok(msg) = StudioToApp::deserialize_json(&text) {
                        let ticked = matches!(msg, StudioToApp::Tick);
                        if self.hosted_handle_msg(msg, &mut windows) {
                            return;
                        }
                        if !ticked {
                            self.hosted_request_frame_if_dirty();
                        }
                    }
                }
                WebSocketMessage::Error(err) => {
                    crate::error!("hosted: host socket error: {err}");
                    break;
                }
                WebSocketMessage::Closed => break,
                WebSocketMessage::Opened => {}
            }
        }
    }

    fn hosted_handle_msg(&mut self, msg: StudioToApp, windows: &mut Vec<HostedWindow>) -> bool {
        if !matches!(msg, StudioToApp::Tick | StudioToApp::MouseMove(_)) {
            crate::trace!("hosted", "rx {:?}", msg);
        }
        match msg {
            // The WM's pointer is the phone's finger: it reaches the app as a
            // touch (`Cx::dispatch_hosted_touch`), so lists drag-scroll.
            StudioToApp::MouseDown(ref e) => {
                let (window_id, pos) = self.windows.window_id_contains(dvec2(e.x, e.y));
                TOUCH_DOWN.store(true, Ordering::Relaxed);
                self.dispatch_hosted_touch(Some(TouchState::Start), dvec2(e.x, e.y), e.time, window_id, pos);
                return false;
            }
            StudioToApp::MouseMove(ref e) => {
                if !TOUCH_DOWN.load(Ordering::Relaxed) {
                    return false;
                }
                let at = dvec2(e.x, e.y);
                let (window_id, pos) = self.hosted_pointer_window(at);
                self.dispatch_hosted_touch(Some(TouchState::Move), at, e.time, window_id, pos);
                return false;
            }
            StudioToApp::MouseUp(ref e) | StudioToApp::MouseCancel(ref e) => {
                if !TOUCH_DOWN.swap(false, Ordering::Relaxed) {
                    return false;
                }
                let at = dvec2(e.x, e.y);
                let (window_id, pos) = self.hosted_pointer_window(at);
                let state = matches!(msg, StudioToApp::MouseUp(_)).then_some(TouchState::Stop);
                self.dispatch_hosted_touch(state, at, e.time, window_id, pos);
                return false;
            }
            StudioToApp::Scroll(ref e) => {
                let (window_id, pos) = self.windows.window_id_contains(dvec2(e.x, e.y));
                return self.dispatch_studio_msg(msg, window_id, pos);
            }
            StudioToApp::Pinch(ref e) => {
                let (window_id, pos) = self.windows.window_id_contains(dvec2(e.x, e.y));
                return self.dispatch_studio_msg(msg, window_id, pos);
            }
            StudioToApp::WindowGeomChange { dpi_factor, width, height, window_id, .. } => {
                REDRAW_AFTER_FIRST.store(true, Ordering::Relaxed);
                let window_id = CxWindowPool::from_usize(window_id);
                if self.windows.is_valid(window_id) {
                    let re = self.windows.stdin_apply_native_geom(
                        window_id,
                        WindowGeom {
                            position: dvec2(0.0, 0.0),
                            dpi_factor,
                            inner_size: dvec2(width, height),
                            ..Default::default()
                        },
                    );
                    if re.old_geom.dpi_factor != re.new_geom.dpi_factor
                        || re.old_geom.inner_size != re.new_geom.inner_size
                    {
                        if let Some(main_pass_id) = self.windows[re.window_id].main_pass_id {
                            self.redraw_pass_and_child_passes(main_pass_id);
                        }
                    }
                    self.call_event_handler(&Event::WindowGeomChange(re));
                }
            }
            StudioToApp::Swapchain(swapchain) => {
                while swapchain.window_id >= windows.len() {
                    windows.push(HostedWindow::default());
                }
                // The host re-announces its swapchain while it bootstraps;
                // the same frames stay imported (each is fetched only once).
                let same = windows[swapchain.window_id].swapchain.as_ref().is_some_and(|current| {
                    current.images.len() == swapchain.presentable_images.len()
                        && current
                            .images
                            .iter()
                            .zip(swapchain.presentable_images.iter())
                            .all(|(a, b)| a.id == b.id)
                });
                if same {
                    return false;
                }
                windows[swapchain.window_id].swapchain = Some(HostedSwapchain {
                    alloc_width: swapchain.alloc_width,
                    alloc_height: swapchain.alloc_height,
                    images: swapchain
                        .presentable_images
                        .iter()
                        .map(|image| HostedImage { id: image.id, texture: None })
                        .collect(),
                });
                self.redraw_all();
                self.hosted_platform_ops(windows);
            }
            StudioToApp::RunViewFrameRequest(_) => {}
            StudioToApp::Tick => self.hosted_tick(windows),
            other => {
                return self.dispatch_studio_msg(other, CxWindowPool::id_zero(), dvec2(0.0, 0.0));
            }
        }
        false
    }

    /// The window a pointer event belongs to: the one a held button started
    /// in, else the one under the pointer.
    fn hosted_pointer_window(&self, at: DVec2) -> (crate::window::WindowId, DVec2) {
        if let Some((_, window_id)) = self.fingers.first_mouse_button {
            (window_id, self.windows[window_id].window_geom.position)
        } else {
            self.windows.window_id_contains(at)
        }
    }

    /// Import any frame of the current swapchains that is not imported yet.
    fn hosted_import_frames(&mut self, windows: &mut [HostedWindow]) {
        for window in windows.iter_mut() {
            let Some(swapchain) = window.swapchain.as_mut() else { continue };
            for image in swapchain.images.iter_mut().filter(|image| image.texture.is_none()) {
                let Some(buffer) = fetch_shared_buffer(&image.id) else { continue };
                let texture = Texture::new_with_format(
                    self,
                    TextureFormat::SharedBGRAu8 {
                        id: image.id,
                        width: swapchain.alloc_width as usize,
                        height: swapchain.alloc_height as usize,
                        initial: true,
                    },
                );
                let Some(mut vulkan) = self.os.vulkan.take() else { return };
                let bound = vulkan.bind_shared_hardware_buffer(
                    texture.texture_id(),
                    buffer,
                    swapchain.alloc_width,
                    swapchain.alloc_height,
                    true,
                );
                self.os.vulkan = Some(vulkan);
                // The resource took its own reference.
                unsafe { ndk_sys::AHardwareBuffer_release(buffer) };
                match bound {
                    Ok(()) => image.texture = Some(texture),
                    Err(err) => crate::error!("hosted: importing a frame failed: {err}"),
                }
            }
        }
    }

    fn hosted_tick(&mut self, windows: &mut Vec<HostedWindow>) {
        static TICKS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let tick = TICKS.fetch_add(1, Ordering::Relaxed);
        if tick % 120 == 0 {
            crate::trace!("hosted", "tick {tick} redraw={} next_frames={}", self.need_redrawing(), self.new_next_frames.len());
        }
        self.hosted_import_frames(windows);
        // Before the signals are taken: a signal raised from here on asks
        // for another Tick (`hosted_wake`) instead of being swallowed.
        WAKE_SENT.store(false, Ordering::Relaxed);
        let internal_signal = SignalToUI::check_and_clear_internal_signal();
        let ui_signal = SignalToUI::check_and_clear_ui_signal();
        if internal_signal || ui_signal {
            self.handle_media_signals();
            self.handle_script_signals();
        }
        if ui_signal {
            crate::trace!("hosted", "ui signal at tick {tick}");
            self.call_event_handler(&Event::Signal);
        }
        let action_signal = SignalToUI::check_and_clear_action_signal();
        if internal_signal || ui_signal || action_signal {
            crate::trace!("pace", "child signals internal={} ui={} action={}", internal_signal, ui_signal, action_signal);
        }
        if action_signal {
            self.handle_action_receiver();
        }
        // Storage answers from the worker thread (the app's saved state).
        self.dispatch_storage_responses();
        let events = self.os.timers.get_dispatch();
        for event in events {
            self.handle_script_timer(&event);
            self.call_event_handler(&Event::Timer(event));
        }
        self.run_live_edit_if_needed("android-hosted");
        self.hosted_platform_ops(windows);

        let time_now = self.os.timers.time_now();
        let phase_started = std::time::Instant::now();
        let pace_tick = pace_ms();
        if !self.new_next_frames.is_empty() {
            self.call_next_frame_event(time_now);
        }
        let next_frame_done = std::time::Instant::now();
        if self.need_redrawing() {
            crate::trace!("hosted", "draw at tick {tick}");
            self.call_draw_event(time_now);
            // The first draw at a new size lays its text out while the
            // glyphs are still being made (its own redraw request is refused
            // inside the draw): draw once more BEFORE the frame goes out, so
            // the host never sees — and never confirms a face with — a frame
            // without its text (the tile-to-app zoom flashed one).
            if REDRAW_AFTER_FIRST.load(Ordering::Relaxed) {
                self.redraw_all();
                self.call_draw_event(time_now);
            }
        }
        let draw_done = std::time::Instant::now();
        let flipped = self.hosted_repaint(windows, time_now as f32);
        // Where a hosted frame's time goes, as the Activity build's
        // `frame.cpu`: NextFrame, the widget draw, and the repaint (record,
        // submit and the wait for the GPU before the host is told).
        // `adb shell setprop debug.makepad.trace frame.cpu`.
        if flipped {
            crate::trace!(
                "frame.cpu",
                "hosted next_frame_ms={:.3} draw_ms={:.3} repaint_ms={:.3}",
                next_frame_done.duration_since(phase_started).as_secs_f64() * 1000.0,
                draw_done.duration_since(next_frame_done).as_secs_f64() * 1000.0,
                draw_done.elapsed().as_secs_f64() * 1000.0,
            );
        }
        if flipped {
            crate::trace!("pace", "child tick={:.2} drawn={:.2}", pace_tick, pace_tick + draw_done.duration_since(phase_started).as_secs_f64() * 1000.0);
        }
        if flipped && REDRAW_AFTER_FIRST.swap(false, Ordering::Relaxed) {
            self.redraw_all();
        }
        self.with_vm(|vm| {
            if vm.heap().needs_gc() {
                vm.gc();
            }
        });
        if tick < 4 {
            crate::trace!("hosted", "after tick {tick}: redraw={} next_frames={} dirty={}", self.need_redrawing(), self.new_next_frames.len(), self.any_passes_dirty());
        }
        // A host that ticks on demand (paced) needs to hear what this child
        // still has to do: another frame now, or a timer later.
        if !self.new_next_frames.is_empty() || self.need_redrawing() || SignalToUI::any_pending() {
            crate::trace!("pace", "child raf next_frames={} redraw={}", self.new_next_frames.len(), self.need_redrawing());
            Self::hosted_send(AppToStudio::RequestAnimationFrame);
        } else if let Some(due) = self.hosted_next_timer_in() {
            crate::trace!("pace", "child wake_in={:.3} timers={}", due, self.os.timers.timers.len());
            Self::hosted_send(AppToStudio::Custom(crate::ime::HostedWake { in_secs: due }.to_json()));
        }
        Self::hosted_send(AppToStudio::TickDone);
    }

    /// A host message outside a Tick (a permission answer, a network
    /// response, an event) left work for the UI: ask the host for a Tick
    /// (a host that ticks on demand would never send one otherwise).
    fn hosted_request_frame_if_dirty(&mut self) {
        // A restyle (the host's appearance changed) runs on the next Tick
        // too: an idle child otherwise kept its old look until touched.
        if !self.new_next_frames.is_empty() || self.need_redrawing() || SignalToUI::any_pending()
            || self.pending_style_reload || self.pending_live_edit_request
        {
            if !WAKE_SENT.swap(true, Ordering::Relaxed) {
                Self::hosted_send(AppToStudio::RequestAnimationFrame);
            }
        }
    }

    /// Seconds until the next poll timer is due, if any.
    fn hosted_next_timer_in(&self) -> Option<f64> {
        let now = Cx::monotonic_now();
        self.os
            .timers
            .timers
            .values()
            .map(|timer| timer.start_time + timer.interval * (timer.step + 1) as f64 - now)
            .min_by(|a, b| a.total_cmp(b))
            .map(|due| due.max(0.0))
    }

    /// Repaint every dirty pass; true when a window frame went to the host.
    fn hosted_repaint(&mut self, windows: &mut [HostedWindow], time: f32) -> bool {
        let mut passes_todo = Vec::new();
        self.compute_pass_repaint_order(&mut passes_todo);
        self.repaint_id += 1;
        let fenced = FENCED.load(Ordering::Relaxed);
        // The shared image this repaint draws into may still be read by the
        // host's GPU: this submission waits for the host's latest one.
        if fenced && !passes_todo.is_empty() {
            if let Some(fd) = fetch_release_fd() {
                if let Some(vulkan) = self.os.vulkan.as_mut() {
                    if let Err(err) = vulkan.wait_sync_fd(fd) {
                        crate::error!("{err}");
                    }
                }
            }
        }
        let mut flips = Vec::new();
        for &draw_pass_id in &passes_todo {
            let uniforms_gen = self.next_uniform_gen();
            self.passes[draw_pass_id].set_time(time, uniforms_gen);
            match self.passes[draw_pass_id].parent.clone() {
                CxDrawPassParent::Xr => {}
                CxDrawPassParent::Window(window_id) => {
                    let Some(window) = windows.get_mut(window_id.id()) else { continue };
                    let Some(swapchain) = window.swapchain.as_mut() else { continue };
                    let count = swapchain.images.len();
                    if count == 0 {
                        continue;
                    }
                    // A resize outgrows the frames the host had shared: the
                    // bigger ones are on their way. Drawing into the old,
                    // smaller image clips the frame, and the host shows it
                    // stretched with its edges smeared (the tile-to-app zoom
                    // glitch). Stay dirty until the new frames are imported.
                    let dpi = self.passes[draw_pass_id].dpi_factor.unwrap_or(1.0);
                    if let Some(rect) = self.get_pass_rect(draw_pass_id, dpi) {
                        if (rect.size.x * dpi).ceil() as u32 > swapchain.alloc_width
                            || (rect.size.y * dpi).ceil() as u32 > swapchain.alloc_height
                        {
                            crate::trace!("hosted", "blocked: pass {:?} outgrows the {}x{} frames", rect.size * dpi, swapchain.alloc_width, swapchain.alloc_height);
                            continue;
                        }
                    }
                    let index = window.present_index % count;
                    let image = &swapchain.images[index];
                    let Some(texture) = image.texture.clone() else { continue };
                    window.present_index = (index + 1) % count;
                    {
                        let pass = &mut self.passes[draw_pass_id];
                        let clear = pass.clear_color;
                        if let Some(target) = pass.color_textures.get_mut(0) {
                            target.texture = texture;
                            target.clear_color = DrawPassClearColor::ClearWith(clear);
                        } else {
                            pass.color_textures.push(crate::draw_pass::CxDrawPassColorTexture {
                                clear_color: DrawPassClearColor::ClearWith(clear),
                                texture,
                                cube_face: None,
                            });
                        }
                    }
                    self.hosted_draw_pass(draw_pass_id);
                    let dpi = self.passes[draw_pass_id].dpi_factor.unwrap_or(1.0);
                    if let Some(rect) = self.get_pass_rect(draw_pass_id, dpi) {
                        flips.push(PresentableDraw {
                            sequence: 0,
                            target_id: image.id,
                            window_id: window_id.id(),
                            width: (rect.size.x * dpi) as u32,
                            height: (rect.size.y * dpi) as u32,
                        });
                    }
                }
                CxDrawPassParent::DrawPass(_) | CxDrawPassParent::None => {
                    self.hosted_draw_pass(draw_pass_id);
                }
            }
        }
        if let Some(mut vulkan) = self.os.vulkan.take() {
            if let Err(err) = vulkan.end_repaint() {
                crate::error!("hosted: repaint submit failed: {err}");
            }
            let submitted = pace_ms();
            // The host reads the frame as soon as it hears of it: it gets
            // the frame's fence first (its GPU waits on it), or, without
            // fences, the frame is complete before the flip goes out.
            let fence = if fenced { vulkan.take_exported_submit_fd() } else { None };
            if !flips.is_empty() {
                let handed = fence.as_ref().is_some_and(|fd| send_acquire_fd(&flips[0].target_id, fd));
                if !handed {
                    if let Err(err) = vulkan.wait_queue_idle() {
                        crate::error!("hosted: {err}");
                    }
                }
                crate::trace!("pace", "child submitted={:.2} gpu_done={:.2} fenced={}", submitted, pace_ms(), handed);
            }
            self.os.vulkan = Some(vulkan);
        }
        let flipped = !flips.is_empty();
        for flip in flips {
            crate::trace!("hosted", "flip {}x{} window {}", flip.width, flip.height, flip.window_id);
            Self::hosted_send(AppToStudio::DrawCompleteAndFlip(flip));
        }
        flipped
    }

    fn hosted_draw_pass(&mut self, draw_pass_id: crate::draw_pass::DrawPassId) {
        if let Some(mut vulkan) = self.os.vulkan.take() {
            let result = vulkan.draw_pass_to_texture(self, draw_pass_id);
            self.os.vulkan = Some(vulkan);
            if let Err(err) = result {
                crate::error!("hosted: draw failed: {err}");
            }
        }
    }

    fn hosted_platform_ops(&mut self, windows: &mut Vec<HostedWindow>) {
        while let Some(op) = self.platform_ops.pop_front() {
            match op {
                CxOsOp::CreateWindow(window_id) => {
                    while window_id.id() >= windows.len() {
                        windows.push(HostedWindow::default());
                    }
                    let window = &mut self.windows[window_id];
                    window.is_created = true;
                    Self::hosted_send(AppToStudio::CreateWindow {
                        window_id: window_id.id(),
                        kind_id: window.kind_id,
                    });
                }
                CxOsOp::CreatePopupWindow { window_id, .. } => {
                    while window_id.id() >= windows.len() {
                        windows.push(HostedWindow::default());
                    }
                    self.windows[window_id].is_created = true;
                }
                CxOsOp::SetCursor(cursor) => {
                    Self::hosted_send(AppToStudio::SetCursor(cursor.into()));
                }
                CxOsOp::StartTimer { timer_id, interval, repeats } => {
                    self.os.timers.timers.insert(timer_id, PollTimer::new(interval, repeats));
                }
                CxOsOp::StopTimer(timer_id) => {
                    self.os.timers.timers.remove(&timer_id);
                }
                CxOsOp::HttpRequest { request_id, request } => {
                    let _ = self.net.http_start(request_id, request);
                }
                CxOsOp::CancelHttpRequest { request_id } => {
                    let _ = self.net.http_cancel(request_id);
                }
                CxOsOp::CopyToClipboard(content) => {
                    Self::hosted_send(AppToStudio::SetClipboard(content));
                }
                // What needs the JVM goes to the WM (hosted_relay.rs); the
                // answers come back as StudioToApp::Relay.
                CxOsOp::ShowClipboardActions { has_selection, rect, keyboard_shift } => {
                    Self::hosted_send(AppToStudio::Relay(ChildRelay::ShowClipboardActions {
                        has_selection,
                        x: rect.pos.x,
                        y: rect.pos.y,
                        width: rect.size.x,
                        height: rect.size.y,
                        keyboard_shift,
                    }));
                }
                CxOsOp::HideClipboardActions => {
                    Self::hosted_send(AppToStudio::Relay(ChildRelay::HideClipboardActions));
                }
                CxOsOp::CheckPermission { permission, request_id } => {
                    Self::hosted_send(AppToStudio::Relay(ChildRelay::CheckPermission(RelayPermission {
                        request_id,
                        permission: crate::hosted_relay::permission_name(permission).into(),
                    })));
                }
                CxOsOp::RequestPermission { permission, request_id } => {
                    Self::hosted_send(AppToStudio::Relay(ChildRelay::RequestPermission(RelayPermission {
                        request_id,
                        permission: crate::hosted_relay::permission_name(permission).into(),
                    })));
                }
                CxOsOp::SelectFileDialog(dialog) => Self::hosted_file_dialog("select_file", &dialog),
                CxOsOp::SaveFileDialog(dialog) => Self::hosted_file_dialog("save_file", &dialog),
                CxOsOp::SelectFolderDialog(dialog) => Self::hosted_file_dialog("select_folder", &dialog),
                CxOsOp::SaveFolderDialog(dialog) => Self::hosted_file_dialog("save_folder", &dialog),
                _ => {}
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The app's own grab on a phone: `adb shell setprop debug.makepad.grab <n>`
// writes the next presented frame to the app's external files directory
// (`/sdcard/Android/data/<package>/files/grab-<n>.png`, readable by adb) —
// the phone's stand-in for a desktop `--remote` grab.
// ---------------------------------------------------------------------------

impl Cx {
    pub(crate) fn android_poll_debug_grab(&mut self) {
        static LAST: Mutex<(Option<std::time::Instant>, String)> = Mutex::new((None, String::new()));
        // `setprop debug.makepad.grab burst-<tag>-<count>`: the next <count>
        // presented frames, one file each (grab-<tag>-NN.png) — a transition
        // frame by frame.
        static BURST: Mutex<(String, u32, u32)> = Mutex::new((String::new(), 0, 0));
        if let Ok(mut burst) = BURST.lock() {
            if burst.1 > 0 {
                if let Some(dir) = unsafe { external_files_dir() } {
                    let path = std::path::PathBuf::from(dir).join(format!("grab-{}-{:02}.png", burst.0, burst.2));
                    self.capture_next_frame_to_file(path);
                }
                burst.1 -= 1;
                burst.2 += 1;
                return;
            }
        }
        let Ok(mut last) = LAST.lock() else { return };
        if last.0.is_some_and(|at| at.elapsed() < std::time::Duration::from_millis(400)) {
            return;
        }
        last.0 = Some(std::time::Instant::now());
        let value = system_property("debug.makepad.grab");
        if value.is_empty() || value == last.1 {
            return;
        }
        let first = last.1.is_empty();
        last.1 = value.clone();
        // A value already set when the app started is not a request.
        if first && self.os.frame_time == 0 {
            return;
        }
        let Some(dir) = (unsafe { external_files_dir() }) else {
            crate::error!("grab: the app has no external files directory");
            return;
        };
        if let Some(rest) = value.strip_prefix("burst-") {
            if let Some((tag, count)) = rest.rsplit_once('-') {
                if let (Ok(count), Ok(mut burst)) = (count.parse::<u32>(), BURST.lock()) {
                    crate::log!("grab: burst of {count} frames -> {dir}/grab-{tag}-NN.png");
                    *burst = (tag.to_string(), count.min(120), 0);
                    return;
                }
            }
        }
        let path = std::path::PathBuf::from(dir).join(format!("grab-{value}.png"));
        crate::log!("grab: next frame -> {}", path.display());
        self.capture_next_frame_to_file(path);
    }
}

/// Wall-clock milliseconds (mod 1e6) for `pace` traces: the host and its
/// children share the device clock, so their lines line up.
pub(crate) fn pace_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64() * 1000.0 % 1.0e6)
        .unwrap_or(0.0)
}

/// `adb shell setprop debug.makepad.hosted.nosync 1`: host and children
/// fall back to CPU waits (for A/B measurements).
fn sync_disabled() -> bool {
    system_property("debug.makepad.hosted.nosync") == "1"
}

fn system_property(name: &str) -> String {
    use std::ffi::{c_char, c_int, CString};
    extern "C" {
        fn __system_property_get(name: *const c_char, value: *mut c_char) -> c_int;
    }
    let Ok(name) = CString::new(name) else { return String::new() };
    let mut value = [0 as c_char; 92];
    let len = unsafe { __system_property_get(name.as_ptr(), value.as_mut_ptr()) };
    if len <= 0 {
        return String::new();
    }
    let bytes: Vec<u8> = value[..len as usize].iter().map(|c| *c as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// `Context.getExternalFilesDir(null)`: the app's own directory under
/// shared storage, created by the call, readable by `adb pull`.
unsafe fn external_files_dir() -> Option<String> {
    let env = super::android_jni::attach_jni_env();
    let activity = makepad_android_state::get_activity();
    if env.is_null() || activity.is_null() {
        return None;
    }
    let get_object_class = (**env).GetObjectClass.unwrap();
    let get_method_id = (**env).GetMethodID.unwrap();
    let call_object_method = (**env).CallObjectMethod.unwrap();
    let class = get_object_class(env, activity);
    let get_dir = get_method_id(
        env,
        class,
        c"getExternalFilesDir".as_ptr(),
        c"(Ljava/lang/String;)Ljava/io/File;".as_ptr(),
    );
    let file = call_object_method(env, activity, get_dir, std::ptr::null_mut::<std::ffi::c_void>());
    if file.is_null() {
        return None;
    }
    let file_class = get_object_class(env, file);
    let get_path = get_method_id(env, file_class, c"getAbsolutePath".as_ptr(), c"()Ljava/lang/String;".as_ptr());
    let path = call_object_method(env, file, get_path);
    if path.is_null() {
        return None;
    }
    let chars = ((**env).GetStringUTFChars.unwrap())(env, path, std::ptr::null_mut());
    let out = std::ffi::CStr::from_ptr(chars).to_string_lossy().into_owned();
    ((**env).ReleaseStringUTFChars.unwrap())(env, path, chars);
    Some(out)
}

