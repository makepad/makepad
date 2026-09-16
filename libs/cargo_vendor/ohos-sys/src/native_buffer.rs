//! Native Buffer bindings
//!
//! The native buffer module provides APIs that you can use to apply for, use, and release the
//! shared memory, and query memory properties.
//! The following scenario is common for native buffer development:
//! Use the native buffer APIs to create an OH_NativeBuffer instance, obtain memory properties,
//! and map the corresponding ION memory to the process address space.
//!
//! Source:
//!
//! [English Documentation](https://docs.openharmony.cn/pages/v5.0/en/application-dev/graphics/native-buffer-guidelines.md)
//! [Chinese Documentation](https://docs.openharmony.cn/pages/v5.0/en/application-dev/graphics/native-buffer-guidelines.md)

#[link(name = "native_buffer")]
extern "C" {}

mod native_buffer_api10;
pub use native_buffer_api10::*;

#[cfg(feature = "api-11")]
#[cfg_attr(docsrs, doc(cfg(feature = "api-11")))]
mod api11_additions;

#[cfg(feature = "api-11")]
#[cfg_attr(docsrs, doc(cfg(feature = "api-11")))]
pub use api11_additions::*;
