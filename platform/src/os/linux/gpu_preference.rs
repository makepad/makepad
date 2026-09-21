//! Which GPU API a desktop Linux window renders with, in a build that carries
//! both (the `vulkan` feature or `MAKEPAD=vulkan`). The default is decided at
//! startup: try Vulkan, and render with OpenGL ES when no usable hardware
//! device answers (no driver installed, only a software rasterizer such as
//! lavapipe, or an X11 session), so one binary works everywhere without
//! configuration. `MAKEPAD_GPU` overrides it: `gl` renders with OpenGL ES
//! without probing Vulkan; `vulkan` insists on Vulkan (any device, panicking
//! when it is unusable); `auto` is the default.
//!
//! Only the windowed Wayland and X11 paths consult this. The hosted
//! (`--stdin-loop`) and direct (DRM/KMS) renderers of such a build are
//! Vulkan-only, and an OpenGL-only build (`MAKEPAD=gl`, or no `vulkan`
//! feature) never reads the variable.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GpuPreference {
    Auto,
    OpenGl,
    Vulkan,
}

pub(crate) fn gpu_preference() -> GpuPreference {
    match std::env::var("MAKEPAD_GPU")
        .ok()
        .as_deref()
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        None | Some("") | Some("auto") => GpuPreference::Auto,
        Some("gl") | Some("opengl") | Some("gles") | Some("opengles") => GpuPreference::OpenGl,
        Some("vulkan") | Some("vk") => GpuPreference::Vulkan,
        Some(other) => {
            crate::warning!("MAKEPAD_GPU={other:?} is not one of auto, gl, vulkan; using auto");
            GpuPreference::Auto
        }
    }
}
