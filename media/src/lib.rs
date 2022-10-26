pub mod audio;
pub mod midi;
pub mod media_api;
pub mod os;
pub mod audio_graph;

pub use crate::{
    audio::*,
    midi::*,
    media_api::*,
    os::*,
};

use makepad_platform::Cx;
pub use makepad_widgets;
pub use makepad_widgets::makepad_platform;
pub use makepad_platform::makepad_error_log;

pub fn live_design(cx:&mut Cx){
    self::audio_graph::live_design(cx);
    self::os::live_design(cx);
}
