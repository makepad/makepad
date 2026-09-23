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
        event::{Event, WindowGeom},
        makepad_math::*,
        makepad_micro_serde::*,
        os::linux::vulkan::CxVulkan,
        os::shared_framebuf::{HostSwapchain, PollTimer, PresentableDraw, PresentableImageId},
        texture::{Texture, TextureFormat},
        thread::SignalToUI,
        web_socket::WebSocketMessage,
        window::CxWindowPool,
    },
    makepad_studio_protocol::{AppToStudio, StudioToApp, StudioToAppVec},
    std::{
        collections::HashMap,
        io::{Read, Write},
        os::fd::AsRawFd,
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

/// One child's connection: it asks for a frame by id (a length-prefixed
/// serialized `PresentableImageId`), the host answers one status byte and,
/// when it has the frame, the buffer itself.
fn serve_frames(mut stream: std::os::unix::net::UnixStream) {
    loop {
        let mut len = [0u8; 4];
        if stream.read_exact(&mut len).is_err() {
            return;
        }
        let mut key = vec![0u8; u32::from_le_bytes(len) as usize];
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
    pub fn android_share_host_swapchain(&mut self, swapchain: &HostSwapchain) {
        start_frame_server();
        let Some(mut vulkan) = self.os.vulkan.take() else {
            crate::error!("hosted frames: the host has no Vulkan renderer");
            return;
        };
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

fn fetch_shared_buffer(id: &PresentableImageId) -> Option<*mut AHardwareBuffer> {
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
            Ok(vulkan) => self.os.vulkan = Some(vulkan),
            Err(err) => {
                crate::error!("hosted: no Vulkan renderer: {err}");
                return;
            }
        }
        self.os.surface_alive = true;
        self.in_makepad_studio = true;
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
                        Self::stdin_coalesce_host_batch(&mut batch);
                        for msg in batch {
                            if self.hosted_handle_msg(msg, &mut windows) {
                                return;
                            }
                        }
                        self.handle_actions();
                        if closed {
                            break;
                        }
                    }
                    Err(err) => crate::error!("hosted: bad host batch: {err:?}"),
                },
                WebSocketMessage::String(text) => {
                    if let Ok(msg) = StudioToApp::deserialize_json(&text) {
                        if self.hosted_handle_msg(msg, &mut windows) {
                            return;
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
            StudioToApp::MouseDown(ref e) => {
                let (window_id, pos) = self.windows.window_id_contains(dvec2(e.x, e.y));
                return self.dispatch_studio_msg(msg, window_id, pos);
            }
            StudioToApp::Scroll(ref e) => {
                let (window_id, pos) = self.windows.window_id_contains(dvec2(e.x, e.y));
                return self.dispatch_studio_msg(msg, window_id, pos);
            }
            StudioToApp::Pinch(ref e) => {
                let (window_id, pos) = self.windows.window_id_contains(dvec2(e.x, e.y));
                return self.dispatch_studio_msg(msg, window_id, pos);
            }
            StudioToApp::MouseMove(ref e) => {
                let at = dvec2(e.x, e.y);
                let (window_id, pos) = self.hosted_pointer_window(at);
                return self.dispatch_studio_msg(msg, window_id, pos);
            }
            StudioToApp::MouseUp(ref e) | StudioToApp::MouseCancel(ref e) => {
                let at = dvec2(e.x, e.y);
                let (window_id, pos) = self.hosted_pointer_window(at);
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
        if SignalToUI::check_and_clear_action_signal() {
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
        if !self.new_next_frames.is_empty() {
            self.call_next_frame_event(time_now);
        }
        if self.need_redrawing() {
            crate::trace!("hosted", "draw at tick {tick}");
            self.call_draw_event(time_now);
        }
        let flipped = self.hosted_repaint(windows, time_now as f32);
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
        if !self.new_next_frames.is_empty()
            || self.need_redrawing()
            || !self.os.timers.timers.is_empty()
        {
            Self::hosted_send(AppToStudio::RequestAnimationFrame);
        }
        Self::hosted_send(AppToStudio::TickDone);
    }

    /// Repaint every dirty pass; true when a window frame went to the host.
    fn hosted_repaint(&mut self, windows: &mut [HostedWindow], time: f32) -> bool {
        let mut passes_todo = Vec::new();
        self.compute_pass_repaint_order(&mut passes_todo);
        self.repaint_id += 1;
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
            // The host reads the frame as soon as it hears of it.
            if !flips.is_empty() {
                if let Err(err) = vulkan.wait_queue_idle() {
                    crate::error!("hosted: {err}");
                }
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
        let path = std::path::PathBuf::from(dir).join(format!("grab-{value}.png"));
        crate::log!("grab: next frame -> {}", path.display());
        self.capture_next_frame_to_file(path);
    }
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

