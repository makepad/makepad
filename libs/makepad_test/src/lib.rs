#![doc = include_str!("../README.md")]

mod error;
mod runtime;
mod selector;
mod studio_remote;

pub use error::{IntoTestResult, TestError, TestResult};
pub use makepad_studio_protocol::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, StudioToApp, TextInputEvent, WidgetSnapshot,
};
pub use makepad_test_macros::makepad_test;
pub use runtime::{run_with_config, Locator, TestApp, TestConfig, WidgetMatch};
pub use selector::Selector;

#[doc(hidden)]
pub mod __private {
    pub use crate::runtime::run_current_package_test;
}
