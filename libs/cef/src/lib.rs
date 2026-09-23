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

/// Times sampled inside a capture callback, before its payload is copied.
///
/// `callback_unix_ms` has the same epoch and units as [`AudioPacket::pts_ms`],
/// but is the callback's time, not an audio presentation timestamp. The
/// monotonic value is elapsed time since this browser was created; it is a
/// separate clock useful for detecting wall-clock jumps, not a Unix time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CaptureTimestamp {
    pub callback_unix_ms: i64,
    pub callback_elapsed_ns: u64,
    /// Committed main-frame document navigation. Same-document history and
    /// fragment changes do not advance this epoch.
    pub navigation_epoch: u64,
}

/// A software paint together with its capture-callback identity.
///
/// The timestamp describes painting, not a decoded video's media PTS. A
/// consumer must measure AV synchronization rather than assume those are
/// interchangeable. Accelerated paints do not produce this CPU payload.
#[derive(Debug)]
pub struct CapturedFrame {
    pub frame: Frame,
    pub sequence: u64,
    pub timestamp: CaptureTimestamp,
}

/// What a page's audio is captured as. Chromium mixes and resamples the
/// page's output to this before the first packet, so the embedder names the
/// format its own audio path wants and never converts a rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioCaptureConfig {
    pub sample_rate: u32,
    /// One or two; anything else captures stereo.
    pub channels: u32,
    /// Frames per packet. Small is low latency and more packets.
    pub frames_per_buffer: u32,
}

impl Default for AudioCaptureConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            channels: 2,
            frames_per_buffer: 1024,
        }
    }
}

/// The format of one capture stream, as Chromium reported it when the stream
/// started. `epoch` numbers the streams of one browser: a page that goes
/// quiet and sounds again is a new stream with the next epoch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AudioFormat {
    pub epoch: u32,
    pub sample_rate: u32,
    pub channels: u32,
    /// Chromium's channel layout number (`2` mono, `3` stereo).
    pub channel_layout: i32,
    pub frames_per_buffer: u32,
}

/// One packet of captured audio: interleaved f32, `frames * channels` long.
#[derive(Debug)]
pub struct AudioPacket {
    pub epoch: u32,
    pub channels: u32,
    pub frames: usize,
    /// Presentation time, milliseconds since the Unix epoch. A gap between
    /// one packet's end and the next one's `pts_ms` is audio that was dropped.
    pub pts_ms: i64,
    /// Callback clocks and the document epoch captured when this audio
    /// stream started. A late packet from an old stream keeps its old epoch.
    pub capture: CaptureTimestamp,
    pub samples: Vec<f32>,
}

#[derive(Debug)]
pub enum AudioEvent {
    Started(AudioFormat),
    Packet(AudioPacket),
    /// The page went quiet for a few seconds, navigated away or closed. The
    /// same browser may start again.
    Stopped,
    Error(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AudioCaptureStats {
    pub enabled: bool,
    pub streaming: bool,
    pub streams: u32,
    pub packets: u64,
    /// Packets that found the queue to the embedder full and were dropped
    /// rather than block Chromium's capture thread.
    pub dropped_packets: u64,
    pub dropped_frames: u64,
    /// Packets that found no recycled buffer and allocated one.
    pub pool_misses: u64,
}

/// One line a page wrote to its console: `console.log` and its siblings,
/// and what Chromium reports there (a script error, a blocked request).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsoleMessage {
    /// Chromium's severity: 0 default, 1 verbose, 2 info, 3 warning, 4 error.
    pub level: i32,
    pub message: String,
    /// The script the line came from, and the line in it; empty and zero
    /// for a line with no script behind it.
    pub source: String,
    pub line: i32,
}

/// The answer to one `Browser::evaluate_javascript`: the expression's
/// value as JSON, or the text of the exception it threw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Evaluation {
    pub number: u64,
    pub result: std::result::Result<String, String>,
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

#[cfg(any(target_os = "macos", windows, all(target_os = "linux", not(target_env = "ohos"))))]
mod ffi;
#[cfg(any(target_os = "macos", windows, all(target_os = "linux", not(target_env = "ohos"))))]
mod native;

#[cfg(any(target_os = "macos", windows, all(target_os = "linux", not(target_env = "ohos"))))]
pub use native::{
    accelerated_paint_requested, background_color, bootstrap, do_message_loop_work, initialize,
    is_initialized, prepare, reexec_into_app_bundle_if_needed, set_application_dark_mode,
    set_background_color, shutdown, flush_profile, startup_phases, AcceleratedStats, Browser,
    BrowserOptions, RenderMode,
};

#[cfg(not(any(target_os = "macos", windows, all(target_os = "linux", not(target_env = "ohos")))))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderMode {
    None,
    Software,
    Accelerated,
}

#[cfg(not(any(target_os = "macos", windows, all(target_os = "linux", not(target_env = "ohos")))))]
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

#[cfg(not(any(target_os = "macos", windows, all(target_os = "linux", not(target_env = "ohos")))))]
pub fn accelerated_paint_requested() -> bool {
    false
}

#[cfg(not(any(target_os = "macos", windows, all(target_os = "linux", not(target_env = "ohos")))))]
pub fn set_background_color(_argb: u32) {}

#[cfg(not(any(target_os = "macos", windows, all(target_os = "linux", not(target_env = "ohos")))))]
pub fn set_application_dark_mode(_dark: bool) {}

#[cfg(not(any(target_os = "macos", windows, all(target_os = "linux", not(target_env = "ohos")))))]
pub fn background_color() -> u32 {
    0
}

#[cfg(not(any(target_os = "macos", windows, all(target_os = "linux", not(target_env = "ohos")))))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BrowserOptions {
    pub software_frames: bool,
}

#[cfg(not(any(target_os = "macos", windows, all(target_os = "linux", not(target_env = "ohos")))))]
pub struct Browser;

#[cfg(not(any(target_os = "macos", windows, all(target_os = "linux", not(target_env = "ohos")))))]
impl Browser {
    pub fn set_dark_mode(&mut self, _dark: bool) -> Result<()> {
        Ok(())
    }

    pub fn new(_url: &str, _width: usize, _height: usize, _scale_factor: f32) -> Result<Self> {
        Err(Error::new(
            "makepad-cef is supported on macOS, Windows and desktop Linux",
        ))
    }

    pub fn new_with_options(
        url: &str,
        width: usize,
        height: usize,
        scale_factor: f32,
        _options: BrowserOptions,
    ) -> Result<Self> {
        Self::new(url, width, height, scale_factor)
    }

    pub fn resize(&mut self, _width: usize, _height: usize, _scale_factor: f32) -> Result<()> {
        Ok(())
    }

    pub fn request_repaint(&mut self) -> Result<()> {
        Ok(())
    }

    pub fn set_url(&mut self, _url: &str) -> Result<()> {
        Ok(())
    }

    pub fn execute_javascript(&mut self, _code: &str) -> Result<()> {
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

    pub fn send_capture_lost_event(&mut self) -> Result<()> {
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

    pub fn try_take_frame(&mut self) -> Option<CapturedFrame> {
        None
    }

    pub fn navigation_epoch(&self) -> u64 {
        0
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

    pub fn editable_focus(&self) -> bool {
        false
    }

    pub fn take_console_messages(&mut self) -> Vec<ConsoleMessage> {
        Vec::new()
    }

    pub fn evaluate_javascript(&mut self, _expression: &str) -> Result<u64> {
        Err(Error::new("CEF is not supported on this platform"))
    }

    pub fn take_evaluations(&mut self) -> Vec<Evaluation> {
        Vec::new()
    }

    pub fn enable_audio_capture(&mut self, _config: AudioCaptureConfig) {}

    pub fn disable_audio_capture(&mut self) {}

    pub fn poll_audio(&mut self) -> Option<AudioEvent> {
        None
    }

    pub fn recycle_audio_packet(&mut self, _packet: AudioPacket) {}

    pub fn audio_capture_stats(&self) -> AudioCaptureStats {
        AudioCaptureStats::default()
    }
}

#[cfg(not(any(target_os = "macos", windows, all(target_os = "linux", not(target_env = "ohos")))))]
pub fn bootstrap() -> Result<BootstrapResult> {
    Ok(BootstrapResult::Continue)
}

#[cfg(not(any(target_os = "macos", windows, all(target_os = "linux", not(target_env = "ohos")))))]
pub fn do_message_loop_work() {}

#[cfg(not(any(target_os = "macos", windows, all(target_os = "linux", not(target_env = "ohos")))))]
pub fn initialize() -> Result<()> {
    Ok(())
}

#[cfg(not(any(target_os = "macos", windows, all(target_os = "linux", not(target_env = "ohos")))))]
pub fn prepare() -> Result<()> {
    Ok(())
}

#[cfg(not(any(target_os = "macos", windows, all(target_os = "linux", not(target_env = "ohos")))))]
pub fn is_initialized() -> bool {
    false
}

#[cfg(not(any(target_os = "macos", windows, all(target_os = "linux", not(target_env = "ohos")))))]
pub fn startup_phases() -> Option<(u128, u128)> {
    None
}

#[cfg(not(any(target_os = "macos", windows, all(target_os = "linux", not(target_env = "ohos")))))]
pub fn shutdown() {}

#[cfg(not(any(target_os = "macos", windows, all(target_os = "linux", not(target_env = "ohos")))))]
pub fn flush_profile() {}

#[cfg(not(any(target_os = "macos", windows, all(target_os = "linux", not(target_env = "ohos")))))]
pub fn reexec_into_app_bundle_if_needed() -> Result<()> {
    Ok(())
}
