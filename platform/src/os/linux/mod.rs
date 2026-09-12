#[cfg(not(any(target_env = "ohos", target_os = "android")))]
pub mod opengl_cx;
#[cfg(not(any(linux_direct, target_env = "ohos", target_os = "android")))]
pub mod file_dialog;
// The wayland-* crates are dependencies of the linux-gnu targets only, so
// this has to carry the same gate as its siblings: Android and OHOS are
// `os::linux` too, and would otherwise fail to resolve `wayland_client`.
#[cfg(not(any(linux_direct, target_env = "ohos", target_os = "android")))]
pub mod wayland;
#[cfg(not(any(linux_direct, target_env = "ohos", target_os = "android")))]
pub mod windowing_backend;
#[cfg(not(any(linux_direct, target_env = "ohos", target_os = "android")))]
pub mod x11;

#[cfg(linux_direct)]
pub mod direct;

// Display inventory contract for the WM, defined at the crate root
// (`crate::linux_display`) so headless builds share it. Native Linux only:
// Android and OHOS report displays through their own platform layers.
#[cfg(not(any(target_env = "ohos", target_os = "android")))]
pub use crate::linux_display as display;

#[cfg(target_os = "android")]
pub mod openxr;
#[cfg(target_os = "android")]
pub mod openxr_anchor;
#[cfg(all(target_os = "android", use_vulkan))]
pub(crate) mod openxr_depth;
#[cfg(target_os = "android")]
pub mod openxr_input;
#[cfg(target_os = "android")]
pub mod openxr_sys;

#[cfg(target_env = "ohos")]
pub mod open_harmony;

pub mod egl_sys;
#[macro_use]
pub mod gl_sys;
pub(crate) mod gl_video_upload;
#[cfg(not(any(target_env = "ohos", target_os = "android")))]
pub(crate) mod va_dmabuf_modifier;
pub mod libc_sys;
pub mod module_loader;
pub mod opengl;
#[cfg(use_vulkan)]
pub mod vulkan;
#[cfg(use_vulkan)]
pub mod vulkan_naga;

#[cfg(not(any(target_env = "ohos", target_os = "android")))]
pub mod dma_buf;
#[cfg(not(any(target_env = "ohos", target_os = "android")))]
pub mod gst_gl_share;
#[cfg(not(any(target_env = "ohos", target_os = "android")))]
pub mod gstreamer_sys;
#[cfg(not(any(target_env = "ohos", target_os = "android")))]
pub mod ipc;
#[cfg(not(any(target_env = "ohos", target_os = "android")))]
pub mod linux_video_gpu;
#[cfg(not(any(target_env = "ohos", target_os = "android")))]
pub mod linux_video_playback;
#[cfg(not(any(target_env = "ohos", target_os = "android")))]
pub mod linux_video_player;

#[cfg(not(any(target_env = "ohos", target_os = "android")))]
pub mod alsa_audio;
#[cfg(not(any(target_env = "ohos", target_os = "android")))]
pub mod alsa_midi;
#[cfg(not(any(target_env = "ohos", target_os = "android")))]
pub mod alsa_sys;
#[cfg(not(any(target_env = "ohos", target_os = "android")))]
pub mod linux_media;
#[cfg(not(any(target_env = "ohos", target_os = "android")))]
pub mod v4l2_camera;
#[cfg(not(any(target_env = "ohos", target_os = "android")))]
pub mod v4l2_camera_player;
#[cfg(not(any(target_env = "ohos", target_os = "android")))]
pub mod v4l2_sys;

#[cfg(not(target_os = "android"))]
pub mod select_timer;

#[cfg(not(any(linux_direct, target_env = "ohos", target_os = "android")))]
pub(crate) fn wake_ui_event_loop() {
    select_timer::wake_ui_event_loop();
}

#[cfg(linux_direct)]
pub(crate) fn wake_ui_event_loop() {
    // Direct display apps sleep on the shared wake pipe while idle.
    select_timer::wake_ui_event_loop();
}

#[cfg(target_os = "android")]
pub(crate) fn wake_ui_event_loop() {
    android::android_jni::send_from_java_message(
        android::android_jni::FromJavaMessage::RenderLoop,
    );
}

#[cfg(target_env = "ohos")]
pub(crate) fn wake_ui_event_loop() {
    open_harmony::oh_callbacks::send_from_ohos_message(
        open_harmony::oh_callbacks::FromOhosMessage::VSync,
    );
}

#[cfg(not(any(target_env = "ohos", target_os = "android")))]
pub mod pulse_audio;
#[cfg(not(any(target_env = "ohos", target_os = "android")))]
pub mod pulse_sys;
#[cfg(not(any(target_env = "ohos", target_os = "android")))]
#[allow(dead_code)]
mod socket_stream;

#[cfg(target_os = "android")]
pub mod android;

#[cfg(target_os = "android")]
pub(crate) use self::android::android::CxOs;

#[cfg(not(any(linux_direct, target_os = "android", target_env = "ohos")))]
pub(crate) use self::windowing_backend::*;

#[cfg(target_env = "ohos")]
pub(crate) use self::open_harmony::open_harmony::*;

#[cfg(linux_direct)]
pub(crate) use self::direct::linux_direct::*;

pub(crate) use self::opengl::*;

#[cfg(not(any(target_os = "android", target_env = "ohos")))]
pub(crate) use self::alsa_midi::{OsMidiInput, OsMidiOutput};

#[cfg(target_os = "android")]
pub(crate) use self::android::android_midi::{OsMidiInput, OsMidiOutput};

//#[cfg(target_env="ohos")]
//pub(crate) use self::open_harmony::oh_media::{OsMidiInput, OsMidiOutput};

// Vulkan hosts upload the existing shared-memory transport on both display paths.
#[cfg(all(not(any(target_env = "ohos", target_os = "android")), all(linux_direct, not(use_vulkan))))]
mod presentable;
#[cfg(all(linux_direct, use_vulkan))]
#[path = "x11/linux_x11_stdin.rs"]
mod direct_stdin;
#[cfg(all(linux_direct, use_vulkan))]
pub(crate) mod hosted_gpu;
#[cfg(not(any(target_env = "ohos", target_os = "android")))]
pub(crate) mod hosted_gpu_sender;

#[cfg(not(any(target_env = "ohos", target_os = "android")))]
#[path = "direct/drm_sys.rs"]
pub mod drm_sys;
