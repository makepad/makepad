use crate::event::SafeAreaInsets;
use crate::Vec2d;

const DEFAULT_MIN_DESKTOP_WIDTH: f64 = 860.;

/// The current context data relevant to adaptive views.
/// Later to be expanded with more context data like platfrom information, accessibility settings, etc.
#[derive(Clone, Debug, Default)]
pub struct DisplayContext {
    /// The event ID that last updated the display context
    pub updated_on_event_id: u64,
    /// The current screen size
    pub screen_size: Vec2d,
    /// Safe area insets for the current window (non-zero on devices with notches,
    /// rounded corners, home indicators, etc.)
    pub safe_area_insets: SafeAreaInsets,
}

impl DisplayContext {
    pub fn is_desktop(&self) -> bool {
        self.screen_size.x >= DEFAULT_MIN_DESKTOP_WIDTH
    }

    pub fn is_screen_size_known(&self) -> bool {
        self.screen_size.x != 0.0 && self.screen_size.y != 0.0
    }
}
