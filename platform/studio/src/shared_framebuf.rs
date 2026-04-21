use makepad_micro_serde::*;

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

    pub fn from_u64(pid_and_counter: u64) -> Self {
        Self {
            origin_pid: (pid_and_counter >> 32) as u32,
            per_origin_counter: pid_and_counter as u32,
        }
    }
}

#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
#[derive(Copy, Clone, Debug, PartialEq, SerBin, DeBin, SerJson, DeJson)]
pub struct DrmFormat {
    pub fourcc: u32,
    pub modifiers: u64,
}

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

#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
#[derive(Copy, Clone, Debug, PartialEq, SerBin, DeBin, SerJson, DeJson)]
pub struct AuxChannedImageFd {
    pub _private: Option<u32>,
}

#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
#[derive(Copy, Clone, Debug, PartialEq, SerBin, DeBin, SerJson, DeJson)]
pub struct LinuxSharedImagePlane {
    pub dma_buf_fd: AuxChannedImageFd,
    pub offset: u32,
    pub stride: u32,
}

#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
#[derive(Copy, Clone, Debug, PartialEq, SerBin, DeBin, SerJson, DeJson)]
pub struct LinuxSharedImage {
    pub drm_format: DrmFormat,
    pub plane: LinuxSharedImagePlane,
}

#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
#[derive(Copy, Clone, Debug, PartialEq, SerBin, DeBin, SerJson, DeJson)]
pub struct SharedPresentableImage {
    pub id: PresentableImageId,
    pub image: LinuxSharedImage,
}

#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
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

/// After a successful client-side draw, all the host needs to know, so it can
/// present the result, is the swapchain image used, and the sub-area within
/// that image that was being used to draw the entire client window.
#[derive(Copy, Clone, Debug, SerBin, DeBin, SerJson, DeJson)]
pub struct PresentableDraw {
    pub window_id: usize,
    pub target_id: PresentableImageId,
    pub width: u32,
    pub height: u32,
}
