use std::fmt;

#[derive(Clone, Debug)]
pub struct Error {
    message: String,
}

impl Error {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(f)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootstrapResult {
    Continue,
    Exit(i32),
}

#[derive(Debug)]
pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u32>,
}

pub const EVENTFLAG_NONE: u32 = 0;
pub const EVENTFLAG_CAPS_LOCK_ON: u32 = 1 << 0;
pub const EVENTFLAG_SHIFT_DOWN: u32 = 1 << 1;
pub const EVENTFLAG_CONTROL_DOWN: u32 = 1 << 2;
pub const EVENTFLAG_ALT_DOWN: u32 = 1 << 3;
pub const EVENTFLAG_LEFT_MOUSE_BUTTON: u32 = 1 << 4;
pub const EVENTFLAG_MIDDLE_MOUSE_BUTTON: u32 = 1 << 5;
pub const EVENTFLAG_RIGHT_MOUSE_BUTTON: u32 = 1 << 6;
pub const EVENTFLAG_COMMAND_DOWN: u32 = 1 << 7;
pub const EVENTFLAG_NUM_LOCK_ON: u32 = 1 << 8;
pub const EVENTFLAG_IS_KEY_PAD: u32 = 1 << 9;
pub const EVENTFLAG_IS_REPEAT: u32 = 1 << 13;
pub const EVENTFLAG_PRECISION_SCROLLING_DELTA: u32 = 1 << 14;

pub const KEY_EVENT_RAWKEYDOWN: i32 = 0;
pub const KEY_EVENT_KEYDOWN: i32 = 1;
pub const KEY_EVENT_KEYUP: i32 = 2;
pub const KEY_EVENT_CHAR: i32 = 3;

pub const MOUSE_BUTTON_LEFT: i32 = 0;
pub const MOUSE_BUTTON_MIDDLE: i32 = 1;
pub const MOUSE_BUTTON_RIGHT: i32 = 2;

pub const TEXT_INPUT_MODE_NONE: i32 = 1;

#[cfg(target_os = "macos")]
mod ffi;
#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
pub use macos::{
    bootstrap, do_message_loop_work, initialize, reexec_into_app_bundle_if_needed, shutdown,
    Browser,
};

#[cfg(not(target_os = "macos"))]
pub struct Browser;

#[cfg(not(target_os = "macos"))]
impl Browser {
    pub fn new(_url: &str, _width: usize, _height: usize, _scale_factor: f32) -> Result<Self> {
        Err(Error::new(
            "makepad-cef is only wired up for macOS right now",
        ))
    }

    pub fn resize(&mut self, _width: usize, _height: usize, _scale_factor: f32) -> Result<()> {
        Ok(())
    }

    pub fn set_url(&mut self, _url: &str) -> Result<()> {
        Ok(())
    }

    pub fn set_focus(&mut self, _focus: bool) -> Result<()> {
        Ok(())
    }

    pub fn send_mouse_move(
        &mut self,
        _x: i32,
        _y: i32,
        _modifiers: u32,
        _mouse_leave: bool,
    ) -> Result<()> {
        Ok(())
    }

    pub fn send_mouse_click(
        &mut self,
        _x: i32,
        _y: i32,
        _modifiers: u32,
        _button: i32,
        _mouse_up: bool,
        _click_count: i32,
    ) -> Result<()> {
        Ok(())
    }

    pub fn send_mouse_wheel(
        &mut self,
        _x: i32,
        _y: i32,
        _modifiers: u32,
        _delta_x: i32,
        _delta_y: i32,
    ) -> Result<()> {
        Ok(())
    }

    pub fn send_key_event(
        &mut self,
        _event_type: i32,
        _modifiers: u32,
        _windows_key_code: i32,
        _native_key_code: i32,
        _character: u16,
        _unmodified_character: u16,
        _is_system_key: bool,
    ) -> Result<()> {
        Ok(())
    }

    pub fn ime_commit_text(&mut self, _text: &str) -> Result<()> {
        Ok(())
    }

    pub fn take_frame(&mut self) -> Option<Frame> {
        None
    }
}

#[cfg(not(target_os = "macos"))]
pub fn bootstrap() -> Result<BootstrapResult> {
    Ok(BootstrapResult::Continue)
}

#[cfg(not(target_os = "macos"))]
pub fn do_message_loop_work() {}

#[cfg(not(target_os = "macos"))]
pub fn initialize() -> Result<()> {
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn shutdown() {}

#[cfg(not(target_os = "macos"))]
pub fn reexec_into_app_bundle_if_needed() -> Result<()> {
    Ok(())
}
