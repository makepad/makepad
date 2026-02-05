#![allow(dead_code)]
use {
    crate::{
        area::Area,
        cursor::MouseCursor,
        cx::Cx,
        event::{
            KeyEvent, KeyModifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
            ScrollEvent, TextInputEvent, TimerEvent,
        },
        makepad_math::{dvec2, Vec2d},
        makepad_micro_serde::*,
        window::WindowId,
    },
    std::cell::Cell,
    std::collections::HashMap,
};

// HACK(eddyb) more or less `<[T; N]>::each_ref`, which is still unstable.
fn ref_array_to_array_of_refs<T, const N: usize>(ref_array: &[T; N]) -> [&T; N] {
    let mut out_refs = std::mem::MaybeUninit::<[&T; N]>::uninit();
    for (i, ref_elem) in ref_array.iter().enumerate() {
        unsafe {
            *out_refs.as_mut_ptr().cast::<&T>().add(i) = ref_elem;
        }
    }
    unsafe { out_refs.assume_init() }
}

pub const SWAPCHAIN_IMAGE_COUNT: usize = match () {
    _ if cfg!(target_os = "linux") => 3,
    _ if cfg!(target_os = "macos") => 1,
    _ if cfg!(target_os = "windows") => 2,
    _ => 2,
};

/// Cross-process-unique ID of a presentable image.
#[derive(Copy, Clone, Debug, PartialEq, SerBin, DeBin, SerJson, DeJson)]
pub struct PresentableImageId {
    origin_pid: u32,
    per_origin_counter: u32,
}

impl PresentableImageId {
    pub fn alloc() -> Self {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        Self {
            origin_pid: std::process::id(),
            per_origin_counter: COUNTER.fetch_add(1, Ordering::Relaxed),
        }
    }

    pub fn as_u64(self) -> u64 {
        (u64::from(self.origin_pid) << 32) | u64::from(self.per_origin_counter)
    }

    fn from_u64(pid_and_counter: u64) -> Self {
        Self {
            origin_pid: (pid_and_counter >> 32) as u32,
            per_origin_counter: pid_and_counter as u32,
        }
    }
}

// ============================================================================
// Host-side swapchain (holds Textures, used by studio)
// ============================================================================
use crate::texture::Texture;

#[derive(Clone, Debug)]
pub struct HostPresentableImage {
    pub id: PresentableImageId,
    pub texture: Texture,
}

#[derive(Clone, Debug)]
pub struct HostSwapchain {
    pub window_id: usize,
    pub alloc_width: u32,
    pub alloc_height: u32,
    pub presentable_images: [HostPresentableImage; SWAPCHAIN_IMAGE_COUNT],
}

impl HostSwapchain {
    pub fn new(
        window_id: usize,
        alloc_width: u32,
        alloc_height: u32,
        cx: &mut crate::cx::Cx,
    ) -> Self {
        use crate::texture::TextureFormat;
        Self {
            window_id,
            alloc_width,
            alloc_height,
            presentable_images: std::array::from_fn(|_| {
                let id = PresentableImageId::alloc();
                HostPresentableImage {
                    id,
                    texture: Texture::new_with_format(
                        cx,
                        TextureFormat::SharedBGRAu8 {
                            id,
                            width: alloc_width as usize,
                            height: alloc_height as usize,
                            initial: true,
                        },
                    ),
                }
            }),
        }
    }

    pub fn get_image(&self, id: PresentableImageId) -> Option<&HostPresentableImage> {
        self.presentable_images.iter().find(|pi| pi.id == id)
    }

    pub fn regenerate_ids(&mut self) {
        for pi in &mut self.presentable_images {
            pi.id = PresentableImageId::alloc();
        }
    }
}

// ============================================================================
// macOS: IOSurface-based swapchain (for serialization)
// ============================================================================
#[cfg(target_os = "macos")]
#[derive(Copy, Clone, Debug, PartialEq, SerBin, DeBin, SerJson, DeJson)]
pub struct SharedPresentableImage {
    pub id: PresentableImageId,
    pub iosurface_id: u32,
}

#[cfg(target_os = "macos")]
#[derive(Copy, Clone, Debug, PartialEq, SerBin, DeBin, SerJson, DeJson)]
pub struct SharedSwapchain {
    pub window_id: usize,
    pub alloc_width: u32,
    pub alloc_height: u32,
    pub presentable_images: [SharedPresentableImage; SWAPCHAIN_IMAGE_COUNT],
}

#[cfg(target_os = "macos")]
impl SharedSwapchain {
    pub fn from_host_swapchain(host: &HostSwapchain, cx: &mut crate::cx::Cx) -> Self {
        Self {
            window_id: host.window_id,
            alloc_width: host.alloc_width,
            alloc_height: host.alloc_height,
            presentable_images: std::array::from_fn(|i| SharedPresentableImage {
                id: host.presentable_images[i].id,
                iosurface_id: cx
                    .share_texture_for_presentable_image(&host.presentable_images[i].texture),
            }),
        }
    }

    pub fn get_image(&self, id: PresentableImageId) -> Option<&SharedPresentableImage> {
        self.presentable_images.iter().find(|pi| pi.id == id)
    }
}

// ============================================================================
// Windows: HANDLE-based swapchain
// ============================================================================
#[cfg(target_os = "windows")]
#[derive(Copy, Clone, Debug, PartialEq, SerBin, DeBin, SerJson, DeJson)]
pub struct SharedPresentableImage {
    pub id: PresentableImageId,
    pub handle: u64,
}

#[cfg(target_os = "windows")]
#[derive(Copy, Clone, Debug, PartialEq, SerBin, DeBin, SerJson, DeJson)]
pub struct SharedSwapchain {
    pub window_id: usize,
    pub alloc_width: u32,
    pub alloc_height: u32,
    pub presentable_images: [SharedPresentableImage; SWAPCHAIN_IMAGE_COUNT],
}

#[cfg(target_os = "windows")]
impl SharedSwapchain {
    pub fn from_host_swapchain(host: &HostSwapchain, cx: &mut crate::cx::Cx) -> Self {
        Self {
            window_id: host.window_id,
            alloc_width: host.alloc_width,
            alloc_height: host.alloc_height,
            presentable_images: std::array::from_fn(|i| SharedPresentableImage {
                id: host.presentable_images[i].id,
                handle: cx.share_texture_for_presentable_image(&host.presentable_images[i].texture),
            }),
        }
    }

    pub fn get_image(&self, id: PresentableImageId) -> Option<&SharedPresentableImage> {
        self.presentable_images.iter().find(|pi| pi.id == id)
    }
}

// ============================================================================
// Linux: DMA-BUF-based swapchain
// ============================================================================
#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
#[derive(Copy, Clone, Debug, PartialEq, SerBin, DeBin, SerJson, DeJson)]
pub struct SharedPresentableImage {
    pub id: PresentableImageId,
    pub image: crate::os::linux::dma_buf::Image<aux_chan::AuxChannedImageFd>,
}

#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
#[derive(Copy, Clone, Debug, PartialEq, SerBin, DeBin, SerJson, DeJson)]
pub struct SharedSwapchain {
    pub window_id: usize,
    pub alloc_width: u32,
    pub alloc_height: u32,
    pub presentable_images: [SharedPresentableImage; SWAPCHAIN_IMAGE_COUNT],
}

#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
impl SharedSwapchain {
    pub fn get_image(&self, id: PresentableImageId) -> Option<&SharedPresentableImage> {
        self.presentable_images.iter().find(|pi| pi.id == id)
    }
}

// ============================================================================
// Fallback for unsupported platforms
// ============================================================================
#[cfg(not(any(
    all(target_os = "linux", not(target_env = "ohos")),
    target_os = "macos",
    target_os = "windows"
)))]
#[derive(Copy, Clone, Debug, PartialEq, SerBin, DeBin, SerJson, DeJson)]
pub struct SharedPresentableImage {
    pub id: PresentableImageId,
    pub _dummy: Option<u32>,
}

#[cfg(not(any(
    all(target_os = "linux", not(target_env = "ohos")),
    target_os = "macos",
    target_os = "windows"
)))]
#[derive(Copy, Clone, Debug, PartialEq, SerBin, DeBin, SerJson, DeJson)]
pub struct SharedSwapchain {
    pub window_id: usize,
    pub alloc_width: u32,
    pub alloc_height: u32,
    pub presentable_images: [SharedPresentableImage; SWAPCHAIN_IMAGE_COUNT],
}

#[cfg(not(any(
    all(target_os = "linux", not(target_env = "ohos")),
    target_os = "macos",
    target_os = "windows"
)))]
impl SharedSwapchain {
    pub fn new(window_id: usize, alloc_width: u32, alloc_height: u32) -> Self {
        Self {
            window_id,
            alloc_width,
            alloc_height,
            presentable_images: [(); SWAPCHAIN_IMAGE_COUNT].map(|()| SharedPresentableImage {
                id: PresentableImageId::alloc(),
                _dummy: None,
            }),
        }
    }

    pub fn get_image(&self, id: PresentableImageId) -> Option<&SharedPresentableImage> {
        self.presentable_images.iter().find(|pi| pi.id == id)
    }
}

/// Auxiliary communication channel, besides stdin (only on Linux).
#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
pub mod aux_chan {
    use super::*;
    use crate::os::linux::ipc::{self as linux_ipc, FixedSizeEncoding};
    use std::{
        io,
        os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd},
    };

    // HACK(eddyb) `io::Error::other` stabilization is too recent.
    fn io_error_other(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> io::Error {
        io::Error::new(io::ErrorKind::Other, error)
    }

    // Host->Client and Client->Host message types.
    pub type H2C = (PresentableImageId, OwnedFd);
    pub type C2H = linux_ipc::Never;

    impl FixedSizeEncoding<{ u64::BYTE_LEN }, 0> for PresentableImageId {
        fn encode(&self) -> ([u8; Self::BYTE_LEN], [std::os::fd::BorrowedFd<'_>; 0]) {
            let (bytes, []) = self.as_u64().encode();
            (bytes, [])
        }
        fn decode(bytes: [u8; Self::BYTE_LEN], fds: [OwnedFd; 0]) -> Self {
            Self::from_u64(u64::decode(bytes, fds))
        }
    }

    pub type HostEndpoint = linux_ipc::Channel<H2C, C2H>;
    pub type ClientEndpoint = linux_ipc::Channel<C2H, H2C>;
    pub fn make_host_and_client_endpoint_pair() -> io::Result<(HostEndpoint, ClientEndpoint)> {
        linux_ipc::channel()
    }

    pub type InheritableClientEndpoint = linux_ipc::InheritableChannel<C2H, H2C>;
    impl InheritableClientEndpoint {
        pub fn extra_args_for_client_spawning(&self) -> [String; 1] {
            [format!(
                "--stdin-loop-aux-chan-fd={}",
                self.as_fd().as_raw_fd()
            )]
        }
        pub fn from_process_args_in_client() -> io::Result<Self> {
            for arg in std::env::args() {
                if let Some(fd) = arg.strip_prefix("--stdin-loop-aux-chan-fd=") {
                    let raw_fd = fd.parse().map_err(io_error_other)?;
                    let owned_fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };
                    return Ok(Self::from(owned_fd));
                }
            }
            Err(io_error_other("missing --stdin-loop-aux-chan-fd argument"))
        }
    }

    // HACK(eddyb) this type being serialized/deserialized doesn't really ensure
    // anything in and of itself, it's only used here to guide correct usage
    // through types - ideally host<->client (de)serialization itself would
    // handle all the file descriptors passing necessary, but for now this helps.
    #[derive(Copy, Clone, Debug, PartialEq, SerBin, DeBin, SerJson, DeJson)]
    pub struct AuxChannedImageFd {
        // HACK(eddyb) non-`()` field working around deriving limitations.
        _private: Option<u32>,
    }
    type PrDmaBufImg<FD> = PresentableImage<crate::os::linux::dma_buf::Image<FD>>;
    impl PrDmaBufImg<OwnedFd> {
        pub fn send_fds_to_aux_chan(
            self,
            host_endpoint: &HostEndpoint,
        ) -> io::Result<PrDmaBufImg<AuxChannedImageFd>> {
            let Self { id, image } = self;
            let mut plane_idx = 0;
            let mut success = Ok(());
            let image = image.planes_fd_map(|fd| {
                assert_eq!(
                    plane_idx, 0,
                    "only images with one DMA-BUF plane are supported"
                );
                plane_idx += 1;
                if success.is_ok() {
                    success = host_endpoint.send((self.id, fd));
                }
                AuxChannedImageFd { _private: None }
            });
            success?;
            Ok(PresentableImage { id, image })
        }
    }
    impl PrDmaBufImg<AuxChannedImageFd> {
        pub fn recv_fds_from_aux_chan(
            self,
            client_endpoint: &ClientEndpoint,
        ) -> io::Result<PrDmaBufImg<OwnedFd>> {
            let Self { id, image } = self;
            let mut plane_idx = 0;
            let mut success = Ok(());
            let image = image.planes_fd_map(|_| {
                assert_eq!(
                    plane_idx, 0,
                    "only images with one DMA-BUF plane are supported"
                );
                plane_idx += 1;

                client_endpoint
                    .recv()
                    .and_then(|(recv_id, recv_fd)| {
                        if recv_id != id {
                            Err(io_error_other(format!(
                                "recv_fds_from_aux_chan: ID mismatch \
                         (expected {id:?}, got {recv_id:?}",
                            )))
                        } else {
                            Ok(recv_fd)
                        }
                    })
                    .map_err(|err| {
                        if success.is_ok() {
                            success = Err(err);
                        }
                    })
            });
            success?;
            Ok(PresentableImage {
                id,
                image: image.planes_fd_map(Result::unwrap),
            })
        }
    }
}
#[cfg(not(all(target_os = "linux", not(target_env = "ohos"))))]
pub mod aux_chan {
    use std::io;

    #[derive(Clone)]
    pub struct HostEndpoint {
        _private: (),
    }
    pub struct ClientEndpoint {
        _private: (),
    }
    pub fn make_host_and_client_endpoint_pair() -> io::Result<(HostEndpoint, ClientEndpoint)> {
        Ok((
            HostEndpoint { _private: () },
            ClientEndpoint { _private: () },
        ))
    }

    pub struct InheritableClientEndpoint(ClientEndpoint);
    impl ClientEndpoint {
        pub fn into_child_process_inheritable(self) -> io::Result<InheritableClientEndpoint> {
            Ok(InheritableClientEndpoint(self))
        }
    }
    impl InheritableClientEndpoint {
        pub fn into_uninheritable(self) -> io::Result<ClientEndpoint> {
            Ok(self.0)
        }
        pub fn extra_args_for_client_spawning(&self) -> [String; 0] {
            []
        }
    }
}

#[derive(Clone, Copy, Debug, Default, SerBin, DeBin, SerJson, DeJson, PartialEq)]
pub struct StdinKeyModifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub logo: bool,
}

impl StdinKeyModifiers {
    pub fn into_key_modifiers(&self) -> KeyModifiers {
        KeyModifiers {
            shift: self.shift,
            control: self.control,
            alt: self.alt,
            logo: self.logo,
        }
    }
    pub fn from_key_modifiers(km: &KeyModifiers) -> Self {
        Self {
            shift: km.shift,
            control: km.control,
            alt: km.alt,
            logo: km.logo,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, SerBin, DeBin, SerJson, DeJson, PartialEq)]
pub struct StdinMouseDown {
    pub button_raw_bits: u32,
    pub x: f64,
    pub y: f64,
    pub time: f64,
    pub modifiers: StdinKeyModifiers,
}

impl StdinMouseDown {
    pub fn into_event(self, window_id: WindowId, pos: Vec2d) -> MouseDownEvent {
        MouseDownEvent {
            abs: dvec2(self.x - pos.x, self.y - pos.y),
            button: MouseButton::from_bits_retain(self.button_raw_bits),
            window_id,
            modifiers: self.modifiers.into_key_modifiers(),
            time: self.time,
            handled: Cell::new(Area::Empty),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, SerBin, DeBin, SerJson, DeJson, PartialEq)]
pub struct StdinMouseMove {
    pub time: f64,
    pub x: f64,
    pub y: f64,
    pub modifiers: StdinKeyModifiers,
}

impl StdinMouseMove {
    pub fn into_event(self, window_id: WindowId, pos: Vec2d) -> MouseMoveEvent {
        MouseMoveEvent {
            abs: dvec2(self.x - pos.x, self.y - pos.y),
            window_id,
            modifiers: self.modifiers.into_key_modifiers(),
            time: self.time,
            handled: Cell::new(Area::Empty),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, SerBin, DeBin, SerJson, DeJson, PartialEq)]
pub struct StdinMouseUp {
    pub time: f64,
    pub button_raw_bits: u32,
    pub x: f64,
    pub y: f64,
    pub modifiers: StdinKeyModifiers,
}

#[derive(Clone, Copy, Debug, Default, SerBin, DeBin, SerJson, DeJson, PartialEq)]
pub struct StdinTextInput {
    pub time: f64,
    pub window_id: usize,
    pub raw_button: usize,
    pub x: f64,
    pub y: f64,
}

impl StdinMouseUp {
    pub fn into_event(self, window_id: WindowId, pos: Vec2d) -> MouseUpEvent {
        MouseUpEvent {
            abs: dvec2(self.x - pos.x, self.y - pos.y),
            button: MouseButton::from_bits_retain(self.button_raw_bits),
            window_id,
            modifiers: self.modifiers.into_key_modifiers(),
            time: self.time,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, SerBin, DeBin, SerJson, DeJson, PartialEq)]
pub struct StdinScroll {
    pub time: f64,
    pub sx: f64,
    pub sy: f64,
    pub x: f64,
    pub y: f64,
    pub is_mouse: bool,
    pub modifiers: StdinKeyModifiers,
}

impl StdinScroll {
    pub fn into_event(self, window_id: WindowId, pos: Vec2d) -> ScrollEvent {
        ScrollEvent {
            abs: dvec2(self.x - pos.x, self.y - pos.y),
            scroll: dvec2(self.sx, self.sy),
            window_id,
            modifiers: self.modifiers.into_key_modifiers(),
            handled_x: Cell::new(false),
            handled_y: Cell::new(false),
            is_mouse: self.is_mouse,
            time: self.time,
        }
    }
}

#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson)]
pub enum HostToStdin {
    Swapchain(SharedSwapchain),
    WindowGeomChange {
        dpi_factor: f64,
        window_id: usize,
        // HACK(eddyb) `DVec` (like `WindowGeom`'s `inner_size` field) can't
        // be used here due to it not implementing (de)serialization traits.
        left: f64,
        top: f64,
        width: f64,
        height: f64,
    },
    Tick,
    /*
    Tick{
        buffer_id: u64,
        frame: u64,
        time: f64,
    },
    */
    MouseDown(StdinMouseDown),
    MouseUp(StdinMouseUp),
    MouseMove(StdinMouseMove),
    KeyDown(KeyEvent),
    KeyUp(KeyEvent),
    TextInput(TextInputEvent),
    Scroll(StdinScroll),
    /*ReloadFile{
        file:String,
        contents:String
    },*/
}

/// After a successful client-side draw, all the host needs to know, so it can
/// present the result, is the swapchain image used, and the sub-area within
/// that image that was being used to draw the entire client window (with the
/// whole allocated area rarely used, except just before needing a new swapchain).
#[derive(Copy, Clone, Debug, SerBin, DeBin, SerJson, DeJson)]
pub struct PresentableDraw {
    pub window_id: usize,
    pub target_id: PresentableImageId,
    pub width: u32,
    pub height: u32,
}

#[repr(usize)]
pub enum WindowKindId {
    Main = 0,
    Design = 1,
    Outline = 2,
}

impl WindowKindId {
    pub fn from_usize(d: usize) -> Self {
        match d {
            0 => Self::Main,
            1 => Self::Design,
            2 => Self::Outline,
            _ => panic!(),
        }
    }
}

#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson)]
pub enum StdinToHost {
    CreateWindow { window_id: usize, kind_id: usize },
    ReadyToStart,
    RequestAnimationFrame,
    SetCursor(MouseCursor),
    // the client is done drawing, and the texture is completely updated
    DrawCompleteAndFlip(PresentableDraw),
}

impl StdinToHost {
    pub fn to_json(&self) -> String {
        let mut json = self.serialize_json();
        json.push('\n');
        json
    }
}

impl HostToStdin {
    pub fn to_json(&self) -> String {
        let mut json = self.serialize_json();
        json.push('\n');
        json
    }
}

impl Cx {}

use std::time::Duration;
use std::time::Instant;

pub struct PollTimer {
    pub start_time: Instant,
    pub interval: Duration,
    pub repeats: bool,
    pub step: u64,
}

impl PollTimer {
    pub fn new(interval_s: f64, repeats: bool) -> Self {
        Self {
            start_time: Instant::now(),
            interval: Duration::from_secs_f64(interval_s),
            repeats,
            step: 0,
        }
    }
}

pub struct PollTimers {
    pub timers: HashMap<u64, PollTimer>,
    pub time_start: Instant,
    pub last_time: Instant,
}
impl Default for PollTimers {
    fn default() -> Self {
        Self {
            time_start: Instant::now(),
            last_time: Instant::now(),
            timers: Default::default(),
        }
    }
}
impl PollTimers {
    pub fn time_now(&self) -> f64 {
        let time_now = Instant::now(); //unsafe {mach_absolute_time()};
        (time_now.duration_since(self.time_start)).as_secs_f64()
    }

    pub fn get_dispatch(&mut self) -> Vec<TimerEvent> {
        let mut to_be_dispatched = Vec::with_capacity(self.timers.len());
        let mut to_be_removed = Vec::with_capacity(self.timers.len());
        let now = Instant::now();
        let time = self.time_now();
        for (id, timer) in self.timers.iter_mut() {
            let elapsed_time = now - timer.start_time;
            let next_due_time =
                Duration::from_nanos(timer.interval.as_nanos() as u64 * (timer.step + 1));

            if elapsed_time > next_due_time {
                to_be_dispatched.push(TimerEvent {
                    timer_id: *id,
                    time: Some(time),
                });
                if timer.repeats {
                    timer.step += 1;
                } else {
                    to_be_removed.push(*id);
                }
            }
        }

        for id in to_be_removed {
            self.timers.remove(&id);
        }

        self.last_time = now;
        to_be_dispatched
    }
}
