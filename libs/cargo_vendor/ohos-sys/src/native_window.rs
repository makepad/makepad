//! Native Window bindings
//!
//! The native window module is a local platform-based window that represents the producer of a
//! graphics queue. It provides APIs for you to request and flush a buffer and configure buffer attributes.
//! The following scenarios are common for native window development:
//!  * Request a graphics buffer by using the native window API, write the produced graphics content
//!    to the buffer, and flush the buffer to the graphics queue.
//!  *  Request and flush a buffer when adapting to the `eglswapbuffer` interface at the EGL.
//!
//! Source:
//!
//! [English Documentation](https://docs.openharmony.cn/pages/v5.0/en/application-dev/graphics/native-window-guidelines.md)
//! [Chinese Documentation](https://docs.openharmony.cn/pages/v5.0/zh-cn/application-dev/graphics/native-window-guidelines.md)

#[link(name = "ace_ndk.z")]
#[link(name = "native_window")]
extern "C" {}

mod native_window_api10;
pub use native_window_api10::*;

mod native_window_api10_fixup;
pub use native_window_api10_fixup::NativeWindowOperation;

#[cfg(feature = "api-11")]
#[cfg_attr(docsrs, doc(cfg(feature = "api-11")))]
mod api11_additions;
#[cfg(feature = "api-11")]
#[cfg_attr(docsrs, doc(cfg(feature = "api-11")))]
pub use api11_additions::*;
