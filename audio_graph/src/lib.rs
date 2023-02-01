pub mod audio_graph;
pub mod audio_traits;
#[cfg(target_os = "macos")]
pub mod audio_unit_effect;
#[cfg(target_os = "macos")]
pub mod audio_unit_instrument;

pub mod mixer;
pub mod instrument;
pub mod audio_stream;

use makepad_platform::Cx;
pub use makepad_platform;
pub use makepad_platform::makepad_error_log;
pub use makepad_platform::makepad_math;
pub use crate::audio_graph::*;
pub use crate::audio_traits::*;

pub fn live_design(cx:&mut Cx){
    self::audio_graph::live_design(cx);
    self::mixer::live_design(cx);
    self::instrument::live_design(cx);
}
