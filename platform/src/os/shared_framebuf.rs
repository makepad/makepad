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

#[derive(Debug)]
pub struct HostPresentableImage {
    pub id: PresentableImageId,
    pub texture: Texture,
    #[cfg(target_os = "linux")]
    frame: std::cell::RefCell<Option<(u64, crate::texture::WeakTexture)>>,
}

impl HostPresentableImage {
    /// Return a texture retaining this specific completed frame. Vulkan reads
    /// hold a lease instead of exposing the writer's reusable allocation.
    ///
    /// `snapshot` is the swapchain's one copy of its newest frame (Linux), reused frame
    /// after frame.
    pub fn texture_for_draw(&self, cx: &mut Cx, draw: &PresentableDraw, width: u32, height: u32, snapshot: &std::cell::RefCell<Option<Texture>>) -> Option<Texture> {
        #[cfg(target_os = "linux")]
        {
            let _ = (width, height);
            if let Some((sequence, texture)) = &*self.frame.borrow() {
                if *sequence == draw.sequence { return texture.upgrade(); }
            }
            let previous = snapshot.borrow().clone();
            return match cx.hal_copy_presentable_frame(&self.texture, draw.sequence, previous.as_ref()) {
                Ok(texture) => {
                    makepad_hal::profile::child_frame(snapshot as *const _ as usize);
                    *self.frame.borrow_mut() = Some((draw.sequence, texture.downgrade()));
                    *snapshot.borrow_mut() = Some(texture.clone());
                    Some(texture)
                },
                Err(error) => {
                    // a snapshot without storage (its create failed, or it was evicted) is not
                    // reused: the next frame makes a new one
                    *snapshot.borrow_mut() = None;
                    crate::error!("Shared HAL frame rejected: {error}");
                    None
                }
            };
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (cx, draw, width, height, snapshot);
            Some(self.texture.clone())
        }
    }
}

#[derive(Debug)]
pub struct HostSwapchain {
    pub window_id: usize,
    pub alloc_width: u32,
    pub alloc_height: u32,
    pub presentable_images: [HostPresentableImage; SWAPCHAIN_IMAGE_COUNT],
    /// The copy of the child's newest frame that is drawn (Linux), reused for the next one.
    pub snapshot: std::cell::RefCell<Option<Texture>>,
}

impl HostSwapchain {
    pub fn new(
        window_id: usize,
        alloc_width: u32,
        alloc_height: u32,
        cx: &mut crate::cx::Cx,
    ) -> Self {
        #[cfg(target_arch = "wasm32")]
        use crate::texture::TextureFormat;
        Self {
            window_id,
            alloc_width,
            alloc_height,
            presentable_images: std::array::from_fn(|_| {
                let id = PresentableImageId::alloc();
                HostPresentableImage {
                    id,
                    #[cfg(target_os = "linux")]
                    frame: Default::default(),
                    #[cfg(not(target_arch = "wasm32"))]
                    texture: cx.create_shared_texture(alloc_width as usize, alloc_height as usize),
                    #[cfg(target_arch = "wasm32")]
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
            snapshot: Default::default(),
        }
    }

    pub fn get_image(&self, id: PresentableImageId) -> Option<&HostPresentableImage> {
        self.presentable_images.iter().find(|pi| pi.id == id)
    }

}

// ============================================================================
// macOS: IOSurface-based swapchain (for serialization)
// ============================================================================
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub fn shared_swapchain_from_host_swapchain(host: &HostSwapchain, cx: &mut Cx) -> Option<SharedSwapchain> {
    let mut images = Vec::with_capacity(SWAPCHAIN_IMAGE_COUNT);
    for image in &host.presentable_images {
        let handle = cx.shared_texture_handle(&image.texture)?;
        if handle == 0 { return None; }
        images.push(SharedPresentableImage { id: image.id,
            #[cfg(target_os = "macos")] iosurface_id: handle as u32,
            #[cfg(target_os = "windows")] handle,
        });
    }
    Some(SharedSwapchain {
        window_id: host.window_id, alloc_width: host.alloc_width, alloc_height: host.alloc_height,
        presentable_images: images.try_into().ok()?,
    })
}

// ============================================================================
// Linux: DMA-BUF-based swapchain
// ============================================================================
/// DRM layout metadata carried by the hosted image protocol.
#[cfg(target_os = "linux")]
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct DrmFormat {
    pub fourcc: u32,
    pub modifiers: u64,
}

#[cfg(target_os = "linux")]
#[derive(Debug)]
pub struct LinuxOwnedImagePlane {
    pub dma_buf_fd: std::os::fd::OwnedFd,
    pub offset: u32,
    pub stride: u32,
}

#[cfg(target_os = "linux")]
#[derive(Debug)]
pub struct LinuxOwnedVulkanImage {
    pub info: makepad_studio_protocol::LinuxVulkanSharedImage,
    pub semaphore_fd: std::os::fd::OwnedFd,
}

#[cfg(target_os = "linux")]
#[derive(Debug)]
pub struct LinuxOwnedImage {
    pub vulkan: Option<LinuxOwnedVulkanImage>,
    pub dma_buf_timeline: Option<std::os::fd::OwnedFd>,
    pub drm_format: DrmFormat,
    pub plane: LinuxOwnedImagePlane,
}

#[cfg(target_os = "linux")]
#[derive(Debug)]
pub struct LinuxPresentableImage {
    pub id: PresentableImageId,
    pub image: LinuxOwnedImage,
}

#[cfg(target_os = "linux")]
#[derive(Debug)]
pub enum SharedSwapchainCreateError {
    /// GPU allocation/export is pending; retain this swapchain and retry after its completion event.
    NotReady,
    Vulkan(String),
}

#[cfg(target_os = "linux")]
pub use crate::os::hosted_gpu_sender::{LinuxSwapchainSender, SentSwapchain};

/// An exported handle batch, with no CPU pixel payload. Ownership can move
/// to a worker after Vulkan export has completed on the renderer thread.
#[cfg(target_os = "linux")]
pub struct ExportedHostSwapchain {
    window_id: usize,
    alloc_width: u32,
    alloc_height: u32,
    images: [LinuxPresentableImage; SWAPCHAIN_IMAGE_COUNT],
}

#[cfg(target_os = "linux")]
impl ExportedHostSwapchain {
    pub(crate) fn send(self, endpoint: &makepad_hal::hosted_ipc::HostEndpoint) -> std::io::Result<SharedSwapchain> {
        let mut shared = Vec::with_capacity(SWAPCHAIN_IMAGE_COUNT);
        for image in self.images {
            shared.push(SharedPresentableImage { id: image.id,
                image: aux_chan::send_image_fds_to_aux_chan(image.id, image.image, endpoint)? });
        }
        Ok(SharedSwapchain { window_id: self.window_id,
            alloc_width: self.alloc_width, alloc_height: self.alloc_height,
            presentable_images: shared.try_into().map_err(|_| std::io::Error::other("incomplete framebuffer export"))? })
    }
}

#[cfg(target_os = "linux")]
pub fn export_host_swapchain(host: &mut HostSwapchain, cx: &mut Cx)
    -> Result<ExportedHostSwapchain, SharedSwapchainCreateError> {
    let mut owned_images: [Option<LinuxOwnedImage>; SWAPCHAIN_IMAGE_COUNT] =
        std::array::from_fn(|_| None);
    for i in 0..SWAPCHAIN_IMAGE_COUNT {
        if cx.shared_texture_handle(&host.presentable_images[i].texture).is_none() {
            return Err(SharedSwapchainCreateError::NotReady);
        }
        owned_images[i] = Some(cx.hal_export_presentable_image(&host.presentable_images[i].texture)
            .map_err(SharedSwapchainCreateError::Vulkan)?);
    }

    Ok(ExportedHostSwapchain {
        window_id: host.window_id,
        alloc_width: host.alloc_width,
        alloc_height: host.alloc_height,
        images: std::array::from_fn(|i| LinuxPresentableImage {
            id: host.presentable_images[i].id, image: owned_images[i].take().expect("image exported"),
        }),
    })
}

// ============================================================================
// Fallback for unsupported platforms
// ============================================================================
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub fn shared_swapchain_from_host_swapchain(
    host: &HostSwapchain,
    _cx: &mut crate::cx::Cx,
) -> Option<SharedSwapchain> {
    Some(SharedSwapchain {
        window_id: host.window_id,
        alloc_width: host.alloc_width,
        alloc_height: host.alloc_height,
        presentable_images: std::array::from_fn(|i| SharedPresentableImage {
            id: host.presentable_images[i].id,
            _dummy: None,
        }),
    })
}

/// Auxiliary communication channel, besides stdin (only on Linux).
#[cfg(target_os = "linux")]
// Unix-domain studio IPC is a native-only process boundary.
#[allow(clippy::disallowed_types, clippy::disallowed_methods)]
pub mod aux_chan {
    use super::*;
    use makepad_hal::hosted_ipc::{aux_path, HostEndpoint};
    use std::{
        io,
        os::unix::net::UnixListener,
        path::PathBuf,
    };

    // HACK(eddyb) `io::Error::other` stabilization is too recent.
    fn io_error_other(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> io::Error {
        io::Error::new(io::ErrorKind::Other, error)
    }

    fn path_for_studio(studio_host: &str, studio_build_id: Option<&str>) -> io::Result<PathBuf> {
        let without_scheme = studio_host
            .split_once("://")
            .map(|(_, rest)| rest)
            .unwrap_or(studio_host);
        let host_port = without_scheme
            .split_once('/')
            .map(|(host_port, _)| host_port)
            .unwrap_or(without_scheme);
        if host_port.trim().is_empty() {
            return Err(io_error_other("invalid STUDIO_HOST value"));
        }
        let port: u16 = host_port
            .rsplit_once(':')
            .map_or(Ok(80), |(_, port)| port.parse())
            .map_err(|_| io_error_other("invalid STUDIO_HOST port"))?;
        let studio_build_id = studio_build_id
            .map(str::trim)
            .filter(|build_id| !build_id.is_empty())
            .map(ToOwned::to_owned)
            .or_else(|| crate::app_main::resolve_studio_build());
        let Some(studio_build_id) = studio_build_id else {
            return Err(io_error_other("missing app id in STUDIO_BUILD"));
        };
        Ok(aux_path(port, &studio_build_id))
    }

    pub struct ExternalEndpointListener {
        path: PathBuf,
        listener: UnixListener,
    }

    impl ExternalEndpointListener {
        pub fn new_for_studio(studio: &str, studio_build_id: &str) -> io::Result<Self> {
            let path = path_for_studio(studio, Some(studio_build_id))?;
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(err) => return Err(err),
            }
            let listener = UnixListener::bind(&path)?;
            listener.set_nonblocking(true)?;
            Ok(Self { path, listener })
        }

        /// One nonblocking accept attempt. A UI poll may call this without
        /// occupying a shared worker while a hosted app is dormant.
        pub fn try_accept_host_endpoint(&self) -> io::Result<Option<HostEndpoint>> {
            match self.listener.accept() {
                Ok((stream, _)) => Ok(Some(HostEndpoint::from(stream))),
                Err(err)
                    if matches!(
                        err.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) => Ok(None),
                Err(err) => Err(err),
            }
        }

    }

    impl Drop for ExternalEndpointListener {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    /// Where this child's host listens for its descriptor channel, from the hub environment
    /// (or the window manager that admitted it).
    pub fn client_path() -> io::Result<PathBuf> {
        let studio_host = crate::app_main::resolve_studio_host();
        if studio_host.is_empty() { return Err(io_error_other("missing STUDIO_HOST")); }
        path_for_studio(&studio_host, None)
    }

    pub fn send_image_fds_to_aux_chan(
        id: PresentableImageId,
        image: LinuxOwnedImage,
        host_endpoint: &HostEndpoint,
    ) -> io::Result<makepad_studio_protocol::LinuxSharedImage> {
        let LinuxOwnedImage { drm_format, plane, vulkan, dma_buf_timeline } = image;
        if vulkan.is_some() && dma_buf_timeline.is_some() { return Err(io_error_other("ambiguous hosted image transport")); }
        host_endpoint.send((id, plane.dma_buf_fd))?;
        let vulkan = match vulkan {
            Some(image) => {
                host_endpoint.send((id, image.semaphore_fd))?;
                Some(image.info)
            }
            None => None,
        };
        let has_dma_buf_timeline = dma_buf_timeline.is_some();
        if let Some(fd) = dma_buf_timeline { host_endpoint.send((id, fd))?; }
        Ok(makepad_studio_protocol::LinuxSharedImage {
            vulkan,
            dma_buf_timeline: has_dma_buf_timeline,
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
}
#[cfg(not(target_os = "linux"))]
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

pub struct PollTimer {
    pub start_time: f64,
    pub interval: f64,
    pub repeats: bool,
    pub step: u64,
}

impl PollTimer {
    pub fn new(interval_s: f64, repeats: bool) -> Self {
        Self {
            start_time: Cx::monotonic_now(),
            interval: interval_s,
            repeats,
            step: 0,
        }
    }
}

pub struct PollTimers {
    pub timers: HashMap<u64, PollTimer>,
    pub time_start: f64,
    pub last_time: f64,
}
impl Default for PollTimers {
    fn default() -> Self {
        Self {
            time_start: Cx::monotonic_now(),
            last_time: Cx::monotonic_now(),
            timers: Default::default(),
        }
    }
}
impl PollTimers {
    pub fn time_now(&self) -> f64 {
        Cx::monotonic_now() - self.time_start
    }

    /// Seconds until the earliest timer is due (0 when one is overdue).
    pub fn next_due_in(&self) -> Option<f64> {
        let now = Cx::monotonic_now();
        self.timers
            .values()
            .map(|timer| (timer.start_time + timer.interval * (timer.step + 1) as f64 - now).max(0.0))
            .min_by(f64::total_cmp)
    }

    pub fn get_dispatch(&mut self) -> Vec<TimerEvent> {
        let mut to_be_dispatched = Vec::with_capacity(self.timers.len());
        let mut to_be_removed = Vec::with_capacity(self.timers.len());
        let now = Cx::monotonic_now();
        let time = self.time_now();
        for (id, timer) in self.timers.iter_mut() {
            let elapsed_time = now - timer.start_time;
            let next_due_time = timer.interval * (timer.step + 1) as f64;

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
