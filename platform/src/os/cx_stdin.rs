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
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct LinuxSharedImagePlane {
    pub dma_buf_fd: aux_chan::AuxChannedImageFd,
    pub offset: u32,
    pub stride: u32,
}

#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct LinuxSharedImage {
    pub drm_format: crate::os::linux::dma_buf::DrmFormat,
    pub plane: LinuxSharedImagePlane,
}

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
impl SerBin for LinuxSharedImagePlane {
    fn ser_bin(&self, s: &mut Vec<u8>) {
        self.dma_buf_fd.ser_bin(s);
        self.offset.ser_bin(s);
        self.stride.ser_bin(s);
    }
}

#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
impl DeBin for LinuxSharedImagePlane {
    fn de_bin(o: &mut usize, d: &[u8]) -> Result<Self, DeBinErr> {
        Ok(Self {
            dma_buf_fd: DeBin::de_bin(o, d)?,
            offset: DeBin::de_bin(o, d)?,
            stride: DeBin::de_bin(o, d)?,
        })
    }
}

#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
impl SerJson for LinuxSharedImagePlane {
    fn ser_json(&self, d: usize, s: &mut SerJsonState) {
        s.st_pre();
        s.field(d + 1, "dma_buf_fd");
        self.dma_buf_fd.ser_json(d + 1, s);
        s.conl();
        s.field(d + 1, "offset");
        self.offset.ser_json(d + 1, s);
        s.conl();
        s.field(d + 1, "stride");
        self.stride.ser_json(d + 1, s);
        s.st_post(d);
    }
}

#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
impl DeJson for LinuxSharedImagePlane {
    fn de_json(s: &mut DeJsonState, i: &mut std::str::Chars) -> Result<Self, DeJsonErr> {
        let mut dma_buf_fd = None;
        let mut offset = None;
        let mut stride = None;

        s.curly_open(i)?;
        while s.tok != DeJsonTok::CurlyClose {
            let key = s.as_string()?;
            s.next_colon(i)?;
            match key.as_str() {
                "dma_buf_fd" => dma_buf_fd = Some(DeJson::de_json(s, i)?),
                "offset" => offset = Some(DeJson::de_json(s, i)?),
                "stride" => stride = Some(DeJson::de_json(s, i)?),
                _ => {
                    if s.lenient {
                        s.skip_value(i)?;
                    } else {
                        return Err(s.err_exp(&s.strbuf));
                    }
                }
            }
            s.eat_comma_curly(i)?;
        }
        s.curly_close(i)?;

        let dma_buf_fd = match dma_buf_fd {
            Some(v) => v,
            None => return Err(s.err_nf("dma_buf_fd")),
        };
        let offset = match offset {
            Some(v) => v,
            None => return Err(s.err_nf("offset")),
        };
        let stride = match stride {
            Some(v) => v,
            None => return Err(s.err_nf("stride")),
        };

        Ok(Self {
            dma_buf_fd,
            offset,
            stride,
        })
    }
}

#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
impl SerBin for LinuxSharedImage {
    fn ser_bin(&self, s: &mut Vec<u8>) {
        self.drm_format.ser_bin(s);
        self.plane.ser_bin(s);
    }
}

#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
impl DeBin for LinuxSharedImage {
    fn de_bin(o: &mut usize, d: &[u8]) -> Result<Self, DeBinErr> {
        Ok(Self {
            drm_format: DeBin::de_bin(o, d)?,
            plane: DeBin::de_bin(o, d)?,
        })
    }
}

#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
impl SerJson for LinuxSharedImage {
    fn ser_json(&self, d: usize, s: &mut SerJsonState) {
        s.st_pre();
        s.field(d + 1, "drm_format");
        self.drm_format.ser_json(d + 1, s);
        s.conl();
        s.field(d + 1, "plane");
        self.plane.ser_json(d + 1, s);
        s.st_post(d);
    }
}

#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
impl DeJson for LinuxSharedImage {
    fn de_json(s: &mut DeJsonState, i: &mut std::str::Chars) -> Result<Self, DeJsonErr> {
        let mut drm_format = None;
        let mut plane = None;

        s.curly_open(i)?;
        while s.tok != DeJsonTok::CurlyClose {
            let key = s.as_string()?;
            s.next_colon(i)?;
            match key.as_str() {
                "drm_format" => drm_format = Some(DeJson::de_json(s, i)?),
                "plane" => plane = Some(DeJson::de_json(s, i)?),
                _ => {
                    if s.lenient {
                        s.skip_value(i)?;
                    } else {
                        return Err(s.err_exp(&s.strbuf));
                    }
                }
            }
            s.eat_comma_curly(i)?;
        }
        s.curly_close(i)?;

        let drm_format = match drm_format {
            Some(v) => v,
            None => return Err(s.err_nf("drm_format")),
        };
        let plane = match plane {
            Some(v) => v,
            None => return Err(s.err_nf("plane")),
        };

        Ok(Self { drm_format, plane })
    }
}

#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
#[derive(Copy, Clone, Debug, PartialEq, SerBin, DeBin, SerJson, DeJson)]
pub struct SharedPresentableImage {
    pub id: PresentableImageId,
    pub image: LinuxSharedImage,
}

#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
#[derive(Debug)]
pub struct LinuxPresentableImage {
    pub id: PresentableImageId,
    pub image: LinuxOwnedImage,
}

#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
impl SharedPresentableImage {
    pub fn recv_fds_from_aux_chan(
        self,
        client_endpoint: &aux_chan::ClientEndpoint,
    ) -> std::io::Result<LinuxPresentableImage> {
        let image = aux_chan::recv_image_fds_from_aux_chan(self.id, self.image, client_endpoint)?;
        Ok(LinuxPresentableImage { id: self.id, image })
    }
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
#[derive(Debug)]
pub enum SharedSwapchainCreateError {
    AuxChannelSend(std::io::Error),
    SoftwareFallback(std::io::Error),
}

#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
impl SharedSwapchain {
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

    pub fn from_host_swapchain(
        host: &mut HostSwapchain,
        cx: &mut crate::cx::Cx,
        host_endpoint: &aux_chan::HostEndpoint,
    ) -> Result<Self, SharedSwapchainCreateError> {
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
                owned_images[i] = Some(Self::software_fallback_image(
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

        Ok(Self {
            window_id: host.window_id,
            alloc_width: host.alloc_width,
            alloc_height: host.alloc_height,
            presentable_images,
        })
    }

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
    pub fn from_host_swapchain(host: &HostSwapchain, _cx: &mut crate::cx::Cx) -> Self {
        Self {
            window_id: host.window_id,
            alloc_width: host.alloc_width,
            alloc_height: host.alloc_height,
            presentable_images: std::array::from_fn(|i| SharedPresentableImage {
                id: host.presentable_images[i].id,
                _dummy: None,
            }),
        }
    }

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

        pub fn accept_host_endpoint(self) -> io::Result<HostEndpoint> {
            let deadline = Instant::now() + Duration::from_secs(10);
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

    // HACK(eddyb) this type being serialized/deserialized doesn't really ensure
    // anything in and of itself, it's only used here to guide correct usage
    // through types - ideally host<->client (de)serialization itself would
    // handle all the file descriptors passing necessary, but for now this helps.
    #[derive(Copy, Clone, Debug, PartialEq, SerBin, DeBin, SerJson, DeJson)]
    pub struct AuxChannedImageFd {
        // HACK(eddyb) non-`()` field working around deriving limitations.
        _private: Option<u32>,
    }
    pub fn send_image_fds_to_aux_chan(
        id: PresentableImageId,
        image: LinuxOwnedImage,
        host_endpoint: &HostEndpoint,
    ) -> io::Result<LinuxSharedImage> {
        let LinuxOwnedImage { drm_format, plane } = image;
        host_endpoint.send((id, plane.dma_buf_fd))?;
        Ok(LinuxSharedImage {
            drm_format,
            plane: LinuxSharedImagePlane {
                dma_buf_fd: AuxChannedImageFd { _private: None },
                offset: plane.offset,
                stride: plane.stride,
            },
        })
    }

    pub fn recv_image_fds_from_aux_chan(
        id: PresentableImageId,
        image: LinuxSharedImage,
        client_endpoint: &ClientEndpoint,
    ) -> io::Result<LinuxOwnedImage> {
        let LinuxSharedImage { drm_format, plane } = image;
        let dma_buf_fd = client_endpoint.recv().and_then(|(recv_id, recv_fd)| {
            if recv_id != id {
                Err(io_error_other(format!(
                    "recv_fds_from_aux_chan: ID mismatch \
                     (expected {id:?}, got {recv_id:?}",
                )))
            } else {
                Ok(recv_fd)
            }
        })?;
        Ok(LinuxOwnedImage {
            drm_format,
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
    TextCopy,
    TextCut,
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
    CreateWindow {
        window_id: usize,
        kind_id: usize,
    },
    ReadyToStart,
    RequestAnimationFrame,
    SetCursor(MouseCursor),
    SetClipboard(String),
    // the client is done drawing, and the texture is completely updated
    DrawCompleteAndFlip(PresentableDraw),
    // headless backend emits PNG snapshots through this message.
    PngFrame {
        window_id: usize,
        path: String,
        width: u32,
        height: u32,
        frame_id: u64,
    },
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
