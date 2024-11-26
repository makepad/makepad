use crate::DVec2;

const DEFAULT_MIN_DESKTOP_WIDTH: f64 = 860.;

/// The current context data relevant to adaptive views.
/// Later to be expanded with more context data like platfrom information, accessibility settings, etc.
#[derive(Clone, Debug, Default)]
pub struct DisplayContext {
    pub updated_on_event_id: u64,
    pub screen_size: DVec2,
}

impl DisplayContext {
    pub fn is_desktop(&self) -> bool {
        self.screen_size.x >= DEFAULT_MIN_DESKTOP_WIDTH
    }
}