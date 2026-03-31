#![doc = include_str!("../README.md")]

mod error;
mod recorder;
mod report;
mod runtime;
mod selector;
mod splash;
mod studio_remote;

pub use error::{IntoTestResult, TestError, TestResult};
pub use makepad_studio_protocol::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, StudioToApp, TextInputEvent, WidgetSnapshot,
};
pub use runtime::{run_with_config, Locator, TestApp, TestConfig};
pub use recorder::{
    run_splash_recorder, SplashRecorderOptions, SplashRecorderOutput, SplashRecorderSession,
};
pub use selector::Selector;
pub use splash::run_splash_suite;
