//! Native XComponent bindings
//!
//! ## When to Use
//!
//! NativeXComponent provides an instance for the <XComponent> at the native layer, which can be
//! used as a bridge for binding with the <XComponent> at the JS layer. The NDK APIs provided by the
//! <XComponent> depend on this instance. The provided APIs include those for obtaining a native
//! window, obtaining the layout or event information of the <XComponent>, registering the lifecycle
//! callbacks of the <XComponent>, and registering the callbacks for the touch, mouse, and key events
//! of the <XComponent>. You can use the provided APIs in the following scenarios:
//!
//!  * Register the lifecycle and event callbacks of the <XComponent>.
//!  * Initialize the environment, obtain the current state, and respond to various events via these callbacks.
//!  * Use the native window and EGL APIs to develop custom drawing content, and apply for and submit buffers to the graphics queue.
//!
//! Source:
//!
//! [English Documentation](https://docs.openharmony.cn/pages/v5.0/en/application-dev/ui/napi-xcomponent-guidelines.md)
//! [Chinese Documentation](https://docs.openharmony.cn/pages/v5.0/zh-cn/application-dev/ui/napi-xcomponent-guidelines.md)

#[link(name = "ace_ndk.z")]
extern "C" {}

mod xcomponent_api10;
pub use xcomponent_api10::*;

#[cfg(feature = "api-11")]
#[cfg_attr(docsrs, doc(cfg(feature = "api-11")))]
mod api11_additions;
#[cfg(feature = "api-11")]
#[cfg_attr(docsrs, doc(cfg(feature = "api-11")))]
pub use api11_additions::*;
