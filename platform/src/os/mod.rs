#[macro_use]
#[cfg(any(
    target_os = "android",
    target_os = "linux",
    target_os = "macos",
    target_os = "ios",
    target_os = "tvos",
    target_os = "windows"
))]
// Native clock backend: this is the implementation behind Cx's portable clock.
#[allow(clippy::disallowed_types, clippy::disallowed_methods)]
pub mod cx_native;

#[macro_use]
pub mod cx_shared;

pub mod shared_framebuf;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub(crate) mod termination_signal;

#[cfg(gpusim)]
// The gpusim (simulated-GPU) process backend is native-only and never compiled into a web app.
#[allow(clippy::disallowed_types, clippy::disallowed_methods)]
pub mod gpusim;

#[cfg(gpusim)]
pub use crate::os::gpusim::*;

#[cfg(all(
    not(gpusim),
    any(target_os = "macos", target_os = "ios", target_os = "tvos")
))]
// Apple process backend is native-only and never compiled into a web app.
#[allow(clippy::disallowed_types, clippy::disallowed_methods)]
pub mod apple;

#[cfg(all(
    not(gpusim),
    any(target_os = "macos", target_os = "ios", target_os = "tvos")
))]
pub use crate::os::apple::*;

#[cfg(all(
    not(gpusim),
    any(target_os = "macos", target_os = "ios", target_os = "tvos")
))]
pub use crate::os::apple::apple_media::*;

#[cfg(all(not(gpusim), target_os = "windows"))]
// Windows process backend is native-only and never compiled into a web app.
#[allow(clippy::disallowed_types, clippy::disallowed_methods)]
pub mod windows;

#[cfg(all(not(gpusim), target_os = "windows"))]
pub use crate::os::windows::*;

//#[cfg(target_os = "windows")]
//pub use crate::os::windows::windows_media::*;

#[cfg(all(not(gpusim), any(target_os = "android", target_os = "linux")))]
// Linux/Android process backends are native-only and never compiled into a web app.
#[allow(clippy::disallowed_types, clippy::disallowed_methods)]
pub mod linux;

#[cfg(all(not(gpusim), any(target_os = "android", target_os = "linux")))]
pub use crate::os::linux::*;

#[cfg(all(test, not(gpusim), target_os = "macos"))]
// Native Linux compatibility tests reuse OS-only timing code on macOS.
#[allow(clippy::disallowed_types, clippy::disallowed_methods)]
pub mod linux_test_stub;

#[cfg(all(test, not(gpusim), target_os = "macos"))]
pub use crate::os::linux_test_stub as linux;

#[cfg(all(not(gpusim), target_os = "android"))]
pub use crate::os::linux::android::android_media::*;

#[cfg(all(not(gpusim), target_os = "linux", not(target_env = "ohos")))]
pub use crate::os::linux::linux_media::*;

#[cfg(all(not(gpusim), target_env = "ohos"))]
pub use crate::os::linux::open_harmony::oh_media::*;

//#[cfg(target_os = "linux")]
//pub use crate::os::linux::*;

//#[cfg(target_os = "linux")]
//pub use crate::os::linux::linux_media::*;

#[cfg(all(not(gpusim), target_arch = "wasm32"))]
pub mod web;

#[cfg(all(not(gpusim), target_arch = "wasm32"))]
pub use crate::os::web::*;
