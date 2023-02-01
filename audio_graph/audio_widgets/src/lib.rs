
pub mod piano;
pub mod display_audio;

use makepad_platform::Cx;
pub use makepad_widgets;
pub use makepad_widgets::makepad_draw;
pub use makepad_widgets::makepad_platform;
pub use makepad_platform::makepad_error_log;
pub use makepad_platform::makepad_math;

pub fn live_design(cx:&mut Cx){
    makepad_widgets::live_design(cx);
    self::piano::live_design(cx);
    self::display_audio::live_design(cx);
}
