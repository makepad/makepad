//! Ohos-sys
//!
//! This crate provides Raw FFI bindings to the native API of OpenHarmonyOS (`target_env = "ohos"`).
//! Each module corresponds to one OpenHarmony API feature, and is gated behind a cargo feature.
//! If you are an application developer, you probably do not want to use this crate directly,
//! and instead want to use a higher-level API built on top of this crate.
//!
//! Note: There are currently still quite a few missing bindings, which will slowly be added.
//!
//! ## Feature flags
#![cfg_attr(
    feature = "document-features",
    cfg_attr(doc, doc = ::document_features::document_features!())
)]
#![cfg_attr(docsrs, feature(doc_cfg))]

#[cfg(feature = "drawing")]
#[cfg_attr(docsrs, doc(cfg(feature = "drawing")))]
pub mod drawing {
    pub use ohos_drawing_sys::*;
}
#[cfg(feature = "hilog")]
#[cfg_attr(docsrs, doc(cfg(feature = "hilog")))]
pub mod hilog;

#[cfg(feature = "napi")]
#[cfg_attr(docsrs, doc(cfg(feature = "napi")))]
pub mod napi;

#[cfg(feature = "native_buffer")]
#[cfg_attr(docsrs, doc(cfg(feature = "native_buffer")))]
pub mod native_buffer;

#[cfg(feature = "native_window")]
#[cfg_attr(docsrs, doc(cfg(feature = "native_window")))]
pub mod native_window;

#[cfg(feature = "vsync")]
#[cfg_attr(docsrs, doc(cfg(feature = "vsync")))]
pub mod vsync;

#[cfg(feature = "xcomponent")]
#[cfg_attr(docsrs, doc(cfg(feature = "xcomponent")))]
pub mod xcomponent;
