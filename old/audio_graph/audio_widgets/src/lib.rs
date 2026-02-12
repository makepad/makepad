pub mod display_audio;
pub mod piano;

pub use makepad_platform::makepad_math;
use makepad_platform::Cx;
pub use makepad_widgets;
pub use makepad_widgets::makepad_draw;
pub use makepad_widgets::makepad_platform;

pub fn live_design(cx: &mut Cx) {
    makepad_widgets::live_design(cx);
    self::piano::live_design(cx);
    self::display_audio::live_design(cx);
}
