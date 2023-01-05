pub mod audio_graph;
pub mod audio_traits;
#[cfg(target_os = "macos")]
pub mod audio_unit_effect;
#[cfg(target_os = "macos")]
pub mod audio_unit_instrument;

pub mod mixer;
pub mod instrument;
pub mod piano;
pub mod display_audio;
pub mod audio_stream;

use makepad_platform::Cx;
pub use makepad_widgets;
pub use makepad_widgets::makepad_draw;
pub use makepad_widgets::makepad_platform;
pub use makepad_platform::makepad_error_log;
pub use makepad_platform::makepad_math;
pub use crate::audio_graph::*;
pub use crate::audio_traits::*;

pub fn live_design(cx:&mut Cx){
    self::makepad_widgets::live_design(cx);
    self::audio_graph::live_design(cx);
    self::piano::live_design(cx);
    self::display_audio::live_design(cx);
    self::mixer::live_design(cx);
    self::instrument::live_design(cx);
}
