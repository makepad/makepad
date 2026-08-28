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

/// The CEF distribution this crate was built against (from the prebuilt's
/// `cef_version.h`), e.g. `138.0.59+g21d63d5+chromium-138.0.7204.306`.
pub const CEF_VERSION: &str = match option_env!("MAKEPAD_CEF_VERSION") {
    Some(version) => version,
    None => "unknown",
};

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
    accelerated_paint_requested, background_color, bootstrap, do_message_loop_work, initialize,
    is_initialized, prepare, reexec_into_app_bundle_if_needed, set_background_color, shutdown,
    startup_phases, AcceleratedStats, Browser, RenderMode,
};

#[cfg(not(target_os = "macos"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderMode {
    None,
    Software,
    Accelerated,
}

#[cfg(not(target_os = "macos"))]
#[derive(Clone, Copy, Debug, Default)]
pub struct AcceleratedStats {
    pub frames: u64,
    pub last_width: usize,
    pub last_height: usize,
    pub last_format: i32,
    pub last_blit_micros: u64,
    pub total_blit_micros: u64,
    pub dropped_no_target: u64,
    pub target_frames: u64,
    pub last_copy_width: usize,
    pub last_copy_height: usize,
}

#[cfg(not(target_os = "macos"))]
pub fn accelerated_paint_requested() -> bool {
    false
}

#[cfg(not(target_os = "macos"))]
pub fn set_background_color(_argb: u32) {}

#[cfg(not(target_os = "macos"))]
pub fn background_color() -> u32 {
    0
}

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

    pub fn is_accelerated(&self) -> bool {
        false
    }

    pub fn render_mode(&self) -> RenderMode {
        RenderMode::None
    }

    pub fn accelerated_stats(&self) -> AcceleratedStats {
        AcceleratedStats::default()
    }

    pub fn set_accelerated_target(
        &mut self,
        _iosurface: *mut std::ffi::c_void,
        _width: usize,
        _height: usize,
    ) -> Result<()> {
        Ok(())
    }

    pub fn clear_accelerated_target(&mut self) {}

    pub fn accelerated_frame_counter(&self) -> u64 {
        0
    }

    pub fn nav_generation(&self) -> u64 {
        0
    }

    pub fn title(&self) -> String {
        String::new()
    }

    pub fn url(&self) -> String {
        String::new()
    }

    pub fn is_loading(&self) -> bool {
        false
    }

    pub fn can_go_back(&self) -> bool {
        false
    }

    pub fn can_go_forward(&self) -> bool {
        false
    }

    pub fn go_back(&mut self) -> Result<()> {
        Ok(())
    }

    pub fn go_forward(&mut self) -> Result<()> {
        Ok(())
    }

    pub fn reload(&mut self) -> Result<()> {
        Ok(())
    }

    pub fn stop_load(&mut self) -> Result<()> {
        Ok(())
    }

    pub fn set_hidden(&mut self, _hidden: bool) -> Result<()> {
        Ok(())
    }

    pub fn is_hidden(&self) -> bool {
        false
    }

    pub fn take_popup_requests(&mut self) -> Vec<String> {
        Vec::new()
    }

    pub fn take_favicon(&mut self) -> Option<Frame> {
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
pub fn prepare() -> Result<()> {
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn is_initialized() -> bool {
    false
}

#[cfg(not(target_os = "macos"))]
pub fn startup_phases() -> Option<(u128, u128)> {
    None
}

#[cfg(not(target_os = "macos"))]
pub fn shutdown() {}

#[cfg(not(target_os = "macos"))]
pub fn reexec_into_app_bundle_if_needed() -> Result<()> {
    Ok(())
}
