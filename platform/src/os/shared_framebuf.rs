#![allow(dead_code)]
pub use makepad_studio_protocol::{
    AppToStudio, PresentableDraw, PresentableImageId, SharedPresentableImage, SharedSwapchain,
    StudioToApp, SWAPCHAIN_IMAGE_COUNT,
};
use {
    crate::{cx::Cx, event::TimerEvent},
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

// ============================================================================
// Host-side swapchain (holds Textures, used by studio)
// ============================================================================
use crate::texture::Texture;

#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
const LINUX_SOFTWARE_FALLBACK_DRM_FOURCC: u32 = 0;
#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
const LINUX_SOFTWARE_FALLBACK_DRM_MODIFIERS: u64 = u64::MAX;

#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
#[derive(Debug)]
pub struct LinuxSharedSoftwareBuffer {
    fd: std::os::fd::OwnedFd,
    ptr: *mut u8,
    len: usize,
    pub stride: u32,
}

#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
impl LinuxSharedSoftwareBuffer {
    pub fn create(len: usize, stride: u32) -> std::io::Result<Self> {
        use std::os::fd::{AsRawFd, FromRawFd};

        const MFD_CLOEXEC: u32 = 0x0001;

        unsafe extern "C" {
            fn memfd_create(name: *const std::os::raw::c_char, flags: u32) -> i32;
            fn ftruncate(fd: i32, length: i64) -> i32;
        }

        let name = b"makepad-runview\0";
        let raw_fd = unsafe { memfd_create(name.as_ptr() as *const _, MFD_CLOEXEC) };
        if raw_fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let fd = unsafe { std::os::fd::OwnedFd::from_raw_fd(raw_fd) };

        let len_i64 = i64::try_from(len).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "software buffer too large",
            )
        })?;
        if unsafe { ftruncate(fd.as_raw_fd(), len_i64) } != 0 {
            return Err(std::io::Error::last_os_error());
        }

        let ptr = unsafe {
            crate::os::linux::libc_sys::mmap(
                std::ptr::null_mut(),
                len,
                crate::os::linux::libc_sys::PROT_READ | crate::os::linux::libc_sys::PROT_WRITE,
                crate::os::linux::libc_sys::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            )
        };
        if ptr as isize == -1 {
            return Err(std::io::Error::last_os_error());
        }

        Ok(Self {
            fd,
            ptr: ptr.cast::<u8>(),
            len,
            stride,
        })
    }

    pub fn from_fd(fd: std::os::fd::OwnedFd, len: usize, stride: u32) -> std::io::Result<Self> {
        use std::os::fd::AsRawFd;

        let ptr = unsafe {
            crate::os::linux::libc_sys::mmap(
                std::ptr::null_mut(),
                len,
                crate::os::linux::libc_sys::PROT_READ | crate::os::linux::libc_sys::PROT_WRITE,
                crate::os::linux::libc_sys::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            )
        };
        if ptr as isize == -1 {
            return Err(std::io::Error::last_os_error());
        }

        Ok(Self {
            fd,
            ptr: ptr.cast::<u8>(),
            len,
            stride,
        })
    }

    pub fn clone_fd(&self) -> std::io::Result<std::os::fd::OwnedFd> {
        use std::os::fd::AsFd;
        self.fd.as_fd().try_clone_to_owned()
    }

    pub fn as_bytes(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }

    pub fn as_bytes_mut(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len) }
    }

    pub fn as_mut_ptr(&mut self) -> *mut std::os::raw::c_void {
        self.ptr.cast::<std::os::raw::c_void>()
    }
}

#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
impl Drop for LinuxSharedSoftwareBuffer {
    fn drop(&mut self) {
        let _ = unsafe {
            crate::os::linux::libc_sys::munmap(self.ptr.cast::<std::os::raw::c_void>(), self.len)
        };
    }
}

#[derive(Debug)]
pub struct HostPresentableImage {
    pub id: PresentableImageId,
    pub texture: Texture,
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    pub software_buffer: Option<LinuxSharedSoftwareBuffer>,
}

#[derive(Debug)]
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
                    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
                    software_buffer: None,
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
pub fn shared_swapchain_from_host_swapchain(
    host: &HostSwapchain,
    cx: &mut crate::cx::Cx,
) -> SharedSwapchain {
    SharedSwapchain {
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

// ============================================================================
// Windows: HANDLE-based swapchain
// ============================================================================
#[cfg(target_os = "windows")]
pub fn shared_swapchain_from_host_swapchain(
    host: &HostSwapchain,
    cx: &mut crate::cx::Cx,
) -> SharedSwapchain {
    SharedSwapchain {
        window_id: host.window_id,
        alloc_width: host.alloc_width,
        alloc_height: host.alloc_height,
        presentable_images: std::array::from_fn(|i| SharedPresentableImage {
            id: host.presentable_images[i].id,
            handle: cx.share_texture_for_presentable_image(&host.presentable_images[i].texture),
        }),
    }
}

pub fn shared_swapchain_get_image(
    swapchain: &SharedSwapchain,
    id: PresentableImageId,
) -> Option<&SharedPresentableImage> {
    swapchain.presentable_images.iter().find(|pi| pi.id == id)
}

// ============================================================================
// Linux: DMA-BUF-based swapchain
// ============================================================================
#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
#[derive(Debug)]
pub struct LinuxOwnedImagePlane {
    pub dma_buf_fd: std::os::fd::OwnedFd,
    pub offset: u32,
    pub stride: u32,
}

#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
#[derive(Debug)]
pub struct LinuxOwnedImage {
    pub drm_format: crate::os::linux::dma_buf::DrmFormat,
    pub plane: LinuxOwnedImagePlane,
}

#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
impl LinuxOwnedImage {
    pub fn is_software_fallback(&self) -> bool {
        self.drm_format.fourcc == LINUX_SOFTWARE_FALLBACK_DRM_FOURCC
            && self.drm_format.modifiers == LINUX_SOFTWARE_FALLBACK_DRM_MODIFIERS
    }

    pub fn software_fallback(dma_buf_fd: std::os::fd::OwnedFd, stride: u32) -> Self {
        Self {
            drm_format: crate::os::linux::dma_buf::DrmFormat {
                fourcc: LINUX_SOFTWARE_FALLBACK_DRM_FOURCC,
                modifiers: LINUX_SOFTWARE_FALLBACK_DRM_MODIFIERS,
            },
            plane: LinuxOwnedImagePlane {
                dma_buf_fd,
                offset: 0,
                stride,
            },
        }
    }
}

#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
#[derive(Debug)]
pub struct LinuxPresentableImage {
    pub id: PresentableImageId,
    pub image: LinuxOwnedImage,
}

#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
#[derive(Debug)]
pub enum SharedSwapchainCreateError {
    AuxChannelSend(std::io::Error),
    SoftwareFallback(std::io::Error),
}

#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
pub fn shared_presentable_image_recv_fds_from_aux_chan(
    image: SharedPresentableImage,
    client_endpoint: &aux_chan::ClientEndpoint,
) -> std::io::Result<LinuxPresentableImage> {
    let id = image.id;
    let owned_image = aux_chan::recv_image_fds_from_aux_chan(id, image.image, client_endpoint)?;
    Ok(LinuxPresentableImage {
        id,
        image: owned_image,
    })
}

#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
fn software_fallback_image(
    host_image: &mut HostPresentableImage,
    alloc_width: u32,
    alloc_height: u32,
) -> Result<LinuxOwnedImage, SharedSwapchainCreateError> {
    let stride = alloc_width.checked_mul(4).ok_or_else(|| {
        SharedSwapchainCreateError::SoftwareFallback(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "software fallback stride overflow",
        ))
    })?;
    let len = usize::try_from(stride)
        .ok()
        .and_then(|stride| {
            usize::try_from(alloc_height)
                .ok()
                .and_then(|height| stride.checked_mul(height))
        })
        .ok_or_else(|| {
            SharedSwapchainCreateError::SoftwareFallback(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "software fallback buffer size overflow",
            ))
        })?;

    let needs_new_buffer = match host_image.software_buffer.as_ref() {
        Some(buffer) => buffer.as_bytes().len() != len || buffer.stride != stride,
        None => true,
    };
    if needs_new_buffer {
        host_image.software_buffer = Some(
            LinuxSharedSoftwareBuffer::create(len, stride)
                .map_err(SharedSwapchainCreateError::SoftwareFallback)?,
        );
    }

    let send_fd = host_image
        .software_buffer
        .as_ref()
        .expect("software buffer initialized")
        .clone_fd()
        .map_err(SharedSwapchainCreateError::SoftwareFallback)?;

    Ok(LinuxOwnedImage::software_fallback(send_fd, stride))
}

#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
pub fn shared_swapchain_from_host_swapchain(
    host: &mut HostSwapchain,
    cx: &mut crate::cx::Cx,
    host_endpoint: &aux_chan::HostEndpoint,
) -> Result<SharedSwapchain, SharedSwapchainCreateError> {
    let mut owned_images: [Option<LinuxOwnedImage>; SWAPCHAIN_IMAGE_COUNT] =
        std::array::from_fn(|_| None);
    let mut use_software_fallback = false;
    for i in 0..SWAPCHAIN_IMAGE_COUNT {
        if let Some(image) =
            cx.share_texture_for_presentable_image(&host.presentable_images[i].texture)
        {
            owned_images[i] = Some(image);
        } else {
            use_software_fallback = true;
            break;
        }
    }

    if use_software_fallback {
        use std::sync::atomic::{AtomicBool, Ordering};
        static LOG_SOFTWARE_FALLBACK: AtomicBool = AtomicBool::new(false);
        if !LOG_SOFTWARE_FALLBACK.swap(true, Ordering::Relaxed) {
            crate::warning!(
                "Linux DMA-BUF export unavailable for RunView; using software readback fallback"
            );
        }
        for i in 0..SWAPCHAIN_IMAGE_COUNT {
            owned_images[i] = Some(software_fallback_image(
                &mut host.presentable_images[i],
                host.alloc_width,
                host.alloc_height,
            )?);
        }
    } else {
        for image in &mut host.presentable_images {
            image.software_buffer = None;
        }
    }

    let mut presentable_images: [Option<SharedPresentableImage>; SWAPCHAIN_IMAGE_COUNT] =
        [None; SWAPCHAIN_IMAGE_COUNT];
    for i in 0..SWAPCHAIN_IMAGE_COUNT {
        let id = host.presentable_images[i].id;
        let image = owned_images[i].take().expect("image exported");
        let image = aux_chan::send_image_fds_to_aux_chan(id, image, host_endpoint)
            .map_err(SharedSwapchainCreateError::AuxChannelSend)?;
        presentable_images[i] = Some(SharedPresentableImage { id, image });
    }
    let presentable_images = presentable_images.map(|image| image.expect("filled"));

    Ok(SharedSwapchain {
        window_id: host.window_id,
        alloc_width: host.alloc_width,
        alloc_height: host.alloc_height,
        presentable_images,
    })
}

// ============================================================================
// Fallback for unsupported platforms
// ============================================================================
#[cfg(not(any(
    all(target_os = "linux", not(target_env = "ohos")),
    target_os = "macos",
    target_os = "windows"
)))]
pub fn shared_swapchain_from_host_swapchain(
    host: &HostSwapchain,
    _cx: &mut crate::cx::Cx,
) -> SharedSwapchain {
    SharedSwapchain {
        window_id: host.window_id,
        alloc_width: host.alloc_width,
        alloc_height: host.alloc_height,
        presentable_images: std::array::from_fn(|i| SharedPresentableImage {
            id: host.presentable_images[i].id,
            _dummy: None,
        }),
    }
}

/// Auxiliary communication channel, besides stdin (only on Linux).
#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
pub mod aux_chan {
    use super::*;
    use crate::os::linux::ipc::{self as linux_ipc, FixedSizeEncoding};
    use std::{
        io,
        os::fd::OwnedFd,
        os::unix::net::{UnixListener, UnixStream},
        path::PathBuf,
        thread,
        time::{Duration, Instant},
    };

    // HACK(eddyb) `io::Error::other` stabilization is too recent.
    fn io_error_other(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> io::Error {
        io::Error::new(io::ErrorKind::Other, error)
    }

    fn path_for_studio(studio: &str, studio_build_id: &str) -> io::Result<PathBuf> {
        let without_scheme = studio
            .split_once("://")
            .map(|(_, rest)| rest)
            .unwrap_or(studio);
        let host_port = without_scheme
            .split_once('/')
            .map(|(host_port, _)| host_port)
            .unwrap_or(without_scheme);
        if host_port.trim().is_empty() {
            return Err(io_error_other("invalid STUDIO value"));
        }
        let port = host_port
            .rsplit_once(':')
            .map(|(_, port)| port)
            .unwrap_or("80");
        if studio_build_id.trim().is_empty() {
            return Err(io_error_other("missing STUDIO_BUILD_ID"));
        }
        Ok(PathBuf::from(format!(
            "/tmp/makepad-stdin-aux-{port}-{}.sock",
            studio_build_id.trim()
        )))
    }

    pub struct ExternalEndpointListener {
        path: PathBuf,
        listener: UnixListener,
    }

    impl ExternalEndpointListener {
        pub fn new_for_studio(studio: &str, studio_build_id: &str) -> io::Result<Self> {
            let path = path_for_studio(studio, studio_build_id)?;
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(err) => return Err(err),
            }
            let listener = UnixListener::bind(&path)?;
            listener.set_nonblocking(true)?;
            Ok(Self { path, listener })
        }

        pub fn accept_host_endpoint(&self) -> io::Result<HostEndpoint> {
            let deadline = Instant::now() + Duration::from_secs(120);
            loop {
                match self.listener.accept() {
                    Ok((stream, _)) => {
                        let owned_fd: OwnedFd = stream.into();
                        return linux_ipc::InheritableChannel::<H2C, C2H>::from(owned_fd)
                            .into_uninheritable();
                    }
                    Err(err)
                        if matches!(
                            err.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                        ) =>
                    {
                        if Instant::now() >= deadline {
                            return Err(io_error_other(
                                "timeout while waiting for child aux-channel connection",
                            ));
                        }
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(err) => return Err(err),
                }
            }
        }
    }

    impl Drop for ExternalEndpointListener {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
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

    impl ClientEndpoint {
        pub fn connect_from_studio_env() -> io::Result<Self> {
            let studio = std::env::var("STUDIO").map_err(io_error_other)?;
            let studio_build_id = std::env::var("STUDIO_BUILD_ID").map_err(io_error_other)?;
            let path = path_for_studio(&studio, &studio_build_id)?;
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                match UnixStream::connect(&path) {
                    Ok(stream) => {
                        let owned_fd: OwnedFd = stream.into();
                        return linux_ipc::InheritableChannel::<C2H, H2C>::from(owned_fd)
                            .into_uninheritable();
                    }
                    Err(err)
                        if matches!(
                            err.kind(),
                            io::ErrorKind::NotFound
                                | io::ErrorKind::ConnectionRefused
                                | io::ErrorKind::Interrupted
                        ) =>
                    {
                        if Instant::now() >= deadline {
                            return Err(err);
                        }
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(err) => return Err(err),
                }
            }
        }
    }

    pub fn send_image_fds_to_aux_chan(
        id: PresentableImageId,
        image: LinuxOwnedImage,
        host_endpoint: &HostEndpoint,
    ) -> io::Result<makepad_studio_protocol::LinuxSharedImage> {
        let LinuxOwnedImage { drm_format, plane } = image;
        host_endpoint.send((id, plane.dma_buf_fd))?;
        Ok(makepad_studio_protocol::LinuxSharedImage {
            drm_format: makepad_studio_protocol::DrmFormat {
                fourcc: drm_format.fourcc,
                modifiers: drm_format.modifiers,
            },
            plane: makepad_studio_protocol::LinuxSharedImagePlane {
                dma_buf_fd: makepad_studio_protocol::AuxChannedImageFd { _private: None },
                offset: plane.offset,
                stride: plane.stride,
            },
        })
    }

    pub fn recv_image_fds_from_aux_chan(
        id: PresentableImageId,
        image: makepad_studio_protocol::LinuxSharedImage,
        client_endpoint: &ClientEndpoint,
    ) -> io::Result<LinuxOwnedImage> {
        let makepad_studio_protocol::LinuxSharedImage { drm_format, plane } = image;
        let mut mismatches = 0usize;
        let dma_buf_fd = loop {
            let (recv_id, recv_fd) = client_endpoint.recv()?;
            if recv_id == id {
                break recv_fd;
            }
            mismatches += 1;
            if mismatches >= 64 {
                return Err(io_error_other(format!(
                    "recv_fds_from_aux_chan: ID mismatch \
                     (expected {id:?}, last got {recv_id:?}, dropped {mismatches} stale images)",
                )));
            }
        };
        if mismatches != 0 {
            crate::warning!(
                "recv_fds_from_aux_chan: dropped {mismatches} stale swapchain images before {id:?}"
            );
        }
        Ok(LinuxOwnedImage {
            drm_format: crate::os::linux::dma_buf::DrmFormat {
                fourcc: drm_format.fourcc,
                modifiers: drm_format.modifiers,
            },
            plane: LinuxOwnedImagePlane {
                dma_buf_fd,
                offset: plane.offset,
                stride: plane.stride,
            },
        })
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
    pub struct ExternalEndpointListener {
        _private: (),
    }
    impl ClientEndpoint {
        pub fn connect_from_studio_env() -> io::Result<ClientEndpoint> {
            Ok(ClientEndpoint { _private: () })
        }
    }
    impl ExternalEndpointListener {
        pub fn new_for_studio(_studio: &str, _studio_build_id: &str) -> io::Result<Self> {
            Ok(Self { _private: () })
        }
        pub fn accept_host_endpoint(self) -> io::Result<HostEndpoint> {
            Ok(HostEndpoint { _private: () })
        }
    }
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
