#![doc = include_str!("../README.md")]

mod app_process;
mod error;
#[cfg(test)]
mod fixture;
mod remote;
mod runtime;
mod selector;

pub use app_process::ShutdownOutcome;
pub use error::{IntoTestResult, TestError, TestResult};
pub use makepad_studio_protocol::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, RemoteKeyModifiers, RemoteMouseDown,
    RemoteMouseMove, RemoteMouseUp, StudioToApp, TextInputEvent, WidgetSnapshot,
};
pub use makepad_test_macros::makepad_test;
pub use remote::WindowInfo;
pub use runtime::{run_with_config, Locator, TestApp, TestConfig, WidgetMatch};
pub use selector::Selector;

#[doc(hidden)]
pub mod __private {
    pub use crate::runtime::run_current_package_test;
}
