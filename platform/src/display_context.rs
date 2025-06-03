use crate::DVec2;

const DEFAULT_MIN_DESKTOP_WIDTH: f64 = 860.;

/// The current context data relevant to adaptive views.
/// Later to be expanded with more context data like platfrom information, accessibility settings, etc.
#[derive(Clone, Debug, Default)]
pub struct DisplayContext {
    /// The event ID that last updated the display context
    pub updated_on_event_id: u64,
    /// The current screen size
    pub screen_size: DVec2,
}

impl DisplayContext {
    pub fn is_desktop(&self) -> bool {
        self.screen_size.x >= DEFAULT_MIN_DESKTOP_WIDTH
    }

    pub fn is_screen_size_known(&self) -> bool {
        self.screen_size.x != 0.0 && self.screen_size.y != 0.0
    }
}